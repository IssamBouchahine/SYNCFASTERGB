//! Platform-independent color pipeline: temporal smoothing and per-LED grading.
//! No Windows APIs here, so everything is unit-testable on any OS.

/// Temporal smoother: per-LED exponential smoothing with a fast attack (getting
/// brighter) and a slower decay (getting darker), frame-rate independent.
pub struct Smoother {
    state: Vec<[f32; 3]>,
    last_out: Vec<[u8; 3]>,
    pub attack_ms: f32,
    pub decay_ms: f32,
    /// Output hysteresis in 0..255 steps; kills 1-step shimmer.
    pub deadband: f32,
    /// Mean per-LED change (0..255) above which the smoother snaps (scene cut).
    pub cut_threshold: f32,
}

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

impl Smoother {
    pub fn new(attack_ms: f32, decay_ms: f32, deadband: f32) -> Self {
        Self {
            state: Vec::new(),
            last_out: Vec::new(),
            attack_ms,
            decay_ms,
            deadband,
            cut_threshold: 110.0,
        }
    }

    pub fn reset(&mut self) {
        self.state.clear();
        self.last_out.clear();
    }

    /// Advance by `dt_ms` toward `target` and return the LED colors to display.
    pub fn update(&mut self, target: &[[u8; 3]], dt_ms: f32) -> Vec<[u8; 3]> {
        let dt = dt_ms.clamp(0.0, 500.0);

        if self.state.len() != target.len() {
            self.state = target.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
            self.last_out = target.to_vec();
            return self.last_out.clone();
        }

        // Scene cut: snap instead of fading across a hard change.
        if !target.is_empty() {
            let mut sum = 0.0;
            for (s, t) in self.state.iter().zip(target) {
                let d = (0..3).map(|k| (t[k] as f32 - s[k]).abs()).fold(0.0, f32::max);
                sum += d;
            }
            if sum / target.len() as f32 > self.cut_threshold {
                self.state = target.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
                self.last_out = target.to_vec();
                return self.last_out.clone();
            }
        }

        for i in 0..target.len() {
            let t = [target[i][0] as f32, target[i][1] as f32, target[i][2] as f32];
            let tau = if luma(t) >= luma(self.state[i]) { self.attack_ms } else { self.decay_ms };
            // alpha is shared by all channels of one LED so hue does not drift.
            let alpha = if tau <= 0.0 { 1.0 } else { 1.0 - (-dt / tau).exp() };
            for k in 0..3 {
                self.state[i][k] += (t[k] - self.state[i][k]) * alpha;
                let want = self.state[i][k].round().clamp(0.0, 255.0);
                let last = self.last_out[i][k] as f32;
                if (want - last).abs() >= self.deadband || want == 0.0 || want == 255.0 || self.deadband <= 0.0 {
                    self.last_out[i][k] = want as u8;
                }
            }
        }
        self.last_out.clone()
    }
}

/// Per-LED grading parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    /// Multiplier on HSL saturation (1.0 = unchanged).
    pub saturation_gain: f32,
    /// Output gamma (1.0 = unchanged).
    pub gamma: f32,
    /// Make near-pure primaries purer (stock look).
    pub purify: bool,
    /// Max channel below this is black; ramps to full by `knee_hi`.
    pub knee_lo: u8,
    pub knee_hi: u8,
    /// Final software brightness scale 0.0..=1.0 (applied after the power limit).
    pub brightness: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Self {
            saturation_gain: 2.0,
            gamma: 1.0,
            purify: true,
            knee_lo: 8,
            knee_hi: 28,
            brightness: 1.0,
        }
    }
}

/// Soft black knee instead of a hard cutoff.
fn knee(c: [u8; 3], lo: u8, hi: u8) -> [u8; 3] {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= lo {
        return [0, 0, 0];
    }
    if m >= hi || hi <= lo {
        return c;
    }
    let f = (m - lo) as f32 / (hi - lo) as f32;
    [(c[0] as f32 * f) as u8, (c[1] as f32 * f) as u8, (c[2] as f32 * f) as u8]
}

/// A dominant channel (>=200) with two weak ones (<=50) becomes a clean primary.
fn purify(c: [u8; 3]) -> [u8; 3] {
    let mut out = c;
    let mut dominant = None;
    let mut weak = [false; 3];
    let mut weak_count = 0;
    for (i, &v) in c.iter().enumerate() {
        if v >= 200 {
            dominant = Some(i);
        }
        if v <= 50 {
            weak[i] = true;
            weak_count += 1;
        }
    }
    if let Some(d) = dominant {
        if weak_count == 2 {
            out[d] = ((out[d] as u16 * 3) / 2).min(255) as u8;
            for i in 0..3 {
                if weak[i] {
                    out[i] = 0;
                }
            }
        }
    }
    out
}

fn saturate(c: [u8; 3], gain: f32) -> [u8; 3] {
    if (gain - 1.0).abs() < 1e-3 {
        return c;
    }
    let (rf, gf, bf) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta < 0.001 {
        return c;
    }
    let s = if l > 0.5 { delta / (2.0 - max - min) } else { delta / (max + min) };
    let mut h = if (max - rf).abs() < 0.001 {
        (gf - bf) / delta + if gf < bf { 6.0 } else { 0.0 }
    } else if (max - gf).abs() < 0.001 {
        (bf - rf) / delta + 2.0
    } else {
        (rf - gf) / delta + 4.0
    };
    h /= 6.0;
    let ns = (s * gain).clamp(0.0, 1.0);

    let hue = |p: f32, q: f32, mut t: f32| -> f32 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            return p + (q - p) * 6.0 * t;
        }
        if t < 0.5 {
            return q;
        }
        if t < 2.0 / 3.0 {
            return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
        }
        p
    };
    let q = if l < 0.5 { l * (1.0 + ns) } else { l + ns - l * ns };
    let p = 2.0 * l - q;
    let to8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        to8(hue(p, q, h + 1.0 / 3.0)),
        to8(hue(p, q, h)),
        to8(hue(p, q, h - 1.0 / 3.0)),
    ]
}

fn apply_gamma(c: [u8; 3], gamma: f32) -> [u8; 3] {
    if (gamma - 1.0).abs() < 1e-3 {
        return c;
    }
    let f = |v: u8| ((v as f32 / 255.0).powf(gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(c[0]), f(c[1]), f(c[2])]
}

/// Whole-strip power budget. Individual LEDs may reach full intensity; only when the
/// strip as a whole would draw more than `max_fraction` of "every LED fully white" are all
/// LEDs scaled down together (keeps hue, protects weak USB ports).
pub fn limit_total_power(leds: &mut [[u8; 3]], max_fraction: f32) {
    if leds.is_empty() {
        return;
    }
    let f = if max_fraction.is_nan() { 1.0 } else { max_fraction.clamp(0.0, 1.0) };
    let total: u64 = leds.iter().map(|c| c[0] as u64 + c[1] as u64 + c[2] as u64).sum();
    let allowed = (leds.len() as f64) * 765.0 * f as f64;
    if (total as f64) <= allowed || total == 0 {
        return;
    }
    let k = allowed / total as f64;
    for c in leds.iter_mut() {
        for ch in c.iter_mut() {
            *ch = (*ch as f64 * k) as u8;
        }
    }
}

/// Fraction of "every LED fully white" the frame would draw (0.0..=1.0).
pub fn power_fraction(leds: &[[u8; 3]]) -> f32 {
    if leds.is_empty() {
        return 0.0;
    }
    let total: u64 = leds.iter().map(|c| c[0] as u64 + c[1] as u64 + c[2] as u64).sum();
    (total as f64 / (leds.len() as f64 * 765.0)) as f32
}

/// Full per-LED grade: knee, purify, saturation, gamma, brightness. The whole-strip power
/// budget is applied afterwards by `limit_total_power`.
pub fn grade(c: [u8; 3], g: &Grade) -> [u8; 3] {
    let mut c = knee(c, g.knee_lo, g.knee_hi);
    if g.purify {
        c = purify(c);
    }
    c = saturate(c, g.saturation_gain);
    c = apply_gamma(c, g.gamma);
    scale(c, g.brightness)
}

fn scale(c: [u8; 3], b: f32) -> [u8; 3] {
    let b = if b.is_nan() { 1.0 } else { b.clamp(0.0, 1.0) };
    if b >= 0.999 {
        return c;
    }
    [(c[0] as f32 * b).round() as u8, (c[1] as f32 * b).round() as u8, (c[2] as f32 * b).round() as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(n: usize, c: [u8; 3]) -> Vec<[u8; 3]> {
        vec![c; n]
    }

    #[test]
    fn first_frame_passes_through() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        assert_eq!(s.update(&solid(3, [10, 20, 30]), 33.0), solid(3, [10, 20, 30]));
    }

    #[test]
    fn attack_is_faster_than_decay() {
        let mut up = Smoother::new(40.0, 250.0, 0.0);
        up.update(&solid(1, [0, 0, 0]), 33.0);
        let a = up.update(&solid(1, [100, 100, 100]), 33.0)[0][0];

        let mut down = Smoother::new(40.0, 250.0, 0.0);
        down.update(&solid(1, [100, 100, 100]), 33.0);
        let d = 100 - down.update(&solid(1, [0, 0, 0]), 33.0)[0][0];

        assert!(a > d, "attack step {a} should exceed decay step {d}");
    }

    #[test]
    fn frame_rate_independent() {
        // one 66 ms step must equal two 33 ms steps (same target)
        let mut a = Smoother::new(100.0, 100.0, 0.0);
        let mut b = Smoother::new(100.0, 100.0, 0.0);
        a.update(&solid(1, [0, 0, 0]), 33.0);
        b.update(&solid(1, [0, 0, 0]), 33.0);
        let one = a.update(&solid(1, [80, 80, 80]), 66.0)[0][0] as i32;
        b.update(&solid(1, [80, 80, 80]), 33.0);
        let two = b.update(&solid(1, [80, 80, 80]), 33.0)[0][0] as i32;
        assert!((one - two).abs() <= 1, "{one} vs {two}");
    }

    #[test]
    fn converges_to_target() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        s.update(&solid(1, [0, 0, 0]), 33.0);
        let mut out = [0u8; 3];
        for _ in 0..200 {
            out = s.update(&solid(1, [200, 100, 50]), 33.0)[0];
        }
        assert!((out[0] as i32 - 200).abs() <= 2 && (out[1] as i32 - 100).abs() <= 2 && (out[2] as i32 - 50).abs() <= 2);
    }

    #[test]
    fn deadband_suppresses_single_step_flicker() {
        let mut s = Smoother::new(0.0, 0.0, 3.0);
        s.update(&solid(1, [100, 100, 100]), 33.0);
        // +-2 jitter stays invisible
        assert_eq!(s.update(&solid(1, [102, 98, 101]), 33.0)[0], [100, 100, 100]);
        // a real change passes
        assert_eq!(s.update(&solid(1, [110, 100, 100]), 33.0)[0], [110, 100, 100]);
    }

    #[test]
    fn scene_cut_snaps() {
        let mut s = Smoother::new(40.0, 5000.0, 0.0);
        s.update(&solid(10, [255, 255, 255]), 33.0);
        assert_eq!(s.update(&solid(10, [0, 0, 0]), 33.0), solid(10, [0, 0, 0]));
    }

    #[test]
    fn resize_resets() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        s.update(&solid(5, [1, 2, 3]), 33.0);
        assert_eq!(s.update(&solid(7, [9, 9, 9]), 33.0).len(), 7);
    }

    #[test]
    fn dt_extremes_do_not_blow_up() {
        let mut s = Smoother::new(40.0, 250.0, 0.0);
        s.update(&solid(2, [0, 0, 0]), 33.0);
        for dt in [f32::NAN, -5.0, 0.0, 1.0e9] {
            let o = s.update(&solid(2, [50, 50, 50]), dt);
            assert_eq!(o.len(), 2);
        }
    }

    #[test]
    fn knee_is_continuous_and_zero_at_black() {
        assert_eq!(knee([5, 5, 5], 8, 28), [0, 0, 0]);
        assert_eq!(knee([8, 8, 8], 8, 28), [0, 0, 0]);
        let mid = knee([18, 18, 18], 8, 28);
        assert!(mid[0] > 0 && mid[0] < 18);
        assert_eq!(knee([28, 28, 28], 8, 28), [28, 28, 28]);
    }

    #[test]
    fn saturation_identity_and_grey() {
        assert_eq!(saturate([120, 80, 40], 1.0), [120, 80, 40]);
        assert_eq!(saturate([90, 90, 90], 3.0), [90, 90, 90]);
        let boosted = saturate([140, 100, 100], 2.0);
        assert!(boosted[0] as i32 - boosted[1] as i32 > 40);
    }

    #[test]
    fn power_limit_leaves_dim_scenes_alone() {
        let mut leds = vec![[10, 10, 10]; 71];
        let before = leds.clone();
        limit_total_power(&mut leds, 0.5);
        assert_eq!(leds, before);
    }

    #[test]
    fn power_limit_allows_bright_highlights_on_dark_scene() {
        let mut leds = vec![[0, 0, 0]; 71];
        for l in leds.iter_mut().take(5) {
            *l = [255, 255, 255];
        }
        limit_total_power(&mut leds, 0.5);
        assert_eq!(leds[0], [255, 255, 255]);
    }

    #[test]
    fn power_limit_scales_everything_together_when_over_budget() {
        let mut leds = vec![[255, 255, 255]; 71];
        limit_total_power(&mut leds, 0.5);
        let sum: u64 = leds.iter().map(|c| c.iter().map(|&v| v as u64).sum::<u64>()).sum();
        assert!(sum as f64 <= 71.0 * 765.0 * 0.5 + 1.0);
        assert!(leds.iter().all(|c| *c == leds[0]), "uniform scene stays uniform");
        assert!(leds[0][0] >= 126, "about half brightness, not less: {}", leds[0][0]);
    }

    #[test]
    fn power_limit_preserves_hue_and_handles_edge_input() {
        let mut leds = vec![[200, 100, 50]; 10];
        limit_total_power(&mut leds, 0.1);
        let c = leds[0];
        assert!(c[0] > c[1] && c[1] > c[2]);
        limit_total_power(&mut [], 0.5);
        let mut z = vec![[0, 0, 0]; 4];
        limit_total_power(&mut z, 0.0);
        assert_eq!(z, vec![[0, 0, 0]; 4]);
        let mut n = vec![[255, 255, 255]; 4];
        limit_total_power(&mut n, f32::NAN);
        assert_eq!(n, vec![[255, 255, 255]; 4]);
    }

    #[test]
    fn power_fraction_bounds() {
        assert_eq!(power_fraction(&[]), 0.0);
        assert_eq!(power_fraction(&[[0, 0, 0]; 5]), 0.0);
        assert!((power_fraction(&[[255, 255, 255]; 5]) - 1.0).abs() < 1e-6);
        let mut leds = vec![[255, 255, 255]; 71];
        limit_total_power(&mut leds, 0.4);
        assert!(power_fraction(&leds) <= 0.4 + 0.001);
    }

    #[test]
    fn grade_applies_saturation_to_bright_pixels() {
        // stock bug: bright pixels lost the saturation boost entirely
        let g = Grade { purify: false, ..Grade::default() };
        let plain = Grade { saturation_gain: 1.0, purify: false, ..Grade::default() };
        let a = grade([220, 160, 140], &g);
        let b = grade([220, 160, 140], &plain);
        assert!((a[0] as i32 - a[2] as i32) > (b[0] as i32 - b[2] as i32));
    }

    #[test]
    fn grade_black_stays_black_and_full_white_stays_full() {
        let g = Grade::default();
        assert_eq!(grade([0, 0, 0], &g), [0, 0, 0]);
        assert_eq!(grade([255, 255, 255], &g), [255, 255, 255]);
    }

    #[test]
    fn brightness_scales_output_last() {
        let full = Grade::default();
        let half = Grade { brightness: 0.5, ..Grade::default() };
        let a = grade([200, 40, 40], &full);
        let b = grade([200, 40, 40], &half);
        for k in 0..3 {
            assert!((b[k] as i32 - (a[k] as f32 * 0.5).round() as i32).abs() <= 1);
        }
        assert_eq!(grade([200, 200, 200], &Grade { brightness: 0.0, ..full }), [0, 0, 0]);
        assert_eq!(scale([10, 20, 30], f32::NAN), [10, 20, 30]);
        assert_eq!(scale([10, 20, 30], 9.0), [10, 20, 30]);
    }

    #[test]
    fn purify_makes_clean_primary() {
        assert_eq!(purify([210, 30, 20]), [255, 0, 0]);
        assert_eq!(purify([100, 100, 100]), [100, 100, 100]);
    }

}
