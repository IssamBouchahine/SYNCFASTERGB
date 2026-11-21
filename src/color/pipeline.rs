//! Platform-independent color pipeline: temporal smoothing, per-LED grading and the final
//! dithered rounding to the strip's 8-bit levels.
//! No Windows APIs here, so everything is unit-testable on any OS.

/// One LED colour with fractional channel values (0.0..=255.0). Smoothing, grading and the
/// power limit all work on these, so a slow fade moves in steps far finer than the strip's 256
/// levels; the rounding to 8 bits happens once, at the very end, with temporal dithering.
pub type Led = [f32; 3];

/// A channel counts as moving while its smoothed value stays this many levels behind its target
/// (averaged over `GAP_AVERAGE_MS`): a fade or a swell keeps it far behind, noise on a still
/// picture averages out to almost nothing.
const MOVING_GAP: f32 = 2.0;
/// Time constant (ms) over which the gap to the target is averaged.
const GAP_AVERAGE_MS: f32 = 200.0;

/// Temporal smoother: per-LED exponential smoothing with a fast attack (getting
/// brighter) and a slower decay (getting darker), frame-rate independent.
///
/// While a channel is moving its value is passed on unrounded, so a slow fade is smooth; while it
/// holds still, rounding plus a small hysteresis band keeps a noisy picture from shimmering.
pub struct Smoother {
    state: Vec<Led>,
    /// What is passed on: the state while moving, a rounded and held value while still.
    out: Vec<Led>,
    /// Averaged signed gap between target and state of each channel, in levels.
    gap: Vec<Led>,
    pub attack_ms: f32,
    pub decay_ms: f32,
    /// Output hysteresis in 0..255 steps for a channel that holds still; kills 1-step shimmer.
    pub deadband: f32,
    /// Mean per-LED change (0..255) above which the smoother snaps (scene cut).
    pub cut_threshold: f32,
}

fn luma(c: Led) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// How much the colour (not the brightness) differs between two LED values: 0 = same hue,
/// 1 = clearly a different colour. Near-black values have no meaningful hue and count as 0.
fn hue_change(a: [f32; 3], b: [f32; 3]) -> f32 {
    let top = |c: [f32; 3]| c[0].max(c[1]).max(c[2]);
    let (ma, mb) = (top(a), top(b));
    if ma < 16.0 || mb < 16.0 {
        return 0.0;
    }
    let d = (0..3)
        .map(|k| (a[k] / ma - b[k] / mb).abs())
        .fold(0.0, f32::max);
    (d / 0.35).clamp(0.0, 1.0)
}

fn to_led(c: [u8; 3]) -> Led {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

impl Smoother {
    pub fn new(attack_ms: f32, decay_ms: f32, deadband: f32) -> Self {
        Self {
            state: Vec::new(),
            out: Vec::new(),
            gap: Vec::new(),
            attack_ms,
            decay_ms,
            deadband,
            cut_threshold: 110.0,
        }
    }

    pub fn reset(&mut self) {
        self.state.clear();
        self.out.clear();
        self.gap.clear();
    }

    /// Advance by `dt_ms` toward `target` and return the LED colors to display.
    pub fn update(&mut self, target: &[[u8; 3]], dt_ms: f32) -> Vec<Led> {
        // a NaN time step must not poison the state (clamp passes NaN through)
        let dt = if dt_ms.is_finite() {
            dt_ms.clamp(0.0, 500.0)
        } else {
            0.0
        };

        if self.state.len() != target.len() {
            self.state = target.iter().map(|&c| to_led(c)).collect();
            self.out = self.state.clone();
            self.gap = vec![[0.0; 3]; target.len()];
            return self.out.clone();
        }

        // Scene cut: snap instead of fading across a hard change.
        if !target.is_empty() {
            let mut sum = 0.0;
            for (s, t) in self.state.iter().zip(target) {
                let d = (0..3)
                    .map(|k| (t[k] as f32 - s[k]).abs())
                    .fold(0.0, f32::max);
                sum += d;
            }
            if sum / target.len() as f32 > self.cut_threshold {
                self.state = target.iter().map(|&c| to_led(c)).collect();
                self.out = self.state.clone();
                self.gap = vec![[0.0; 3]; target.len()];
                return self.out.clone();
            }
        }

        // averaging factor for the gap estimate (frame-rate independent)
        let gap_alpha = if dt > 0.0 {
            1.0 - (-dt / GAP_AVERAGE_MS).exp()
        } else {
            0.0
        };
        for i in 0..target.len() {
            let t = to_led(target[i]);
            let tau = if luma(t) >= luma(self.state[i]) {
                self.attack_ms
            } else {
                // Getting darker eases slowly, but a change of colour (white turning red also
                // "gets darker") follows at the fast speed, so the old hue does not linger.
                let m = hue_change(t, self.state[i]);
                self.decay_ms + (self.attack_ms - self.decay_ms) * m
            };
            // alpha is shared by all channels of one LED so hue does not drift.
            let alpha = if tau <= 0.0 {
                1.0
            } else {
                1.0 - (-dt / tau).exp()
            };
            for k in 0..3 {
                self.state[i][k] += (t[k] - self.state[i][k]) * alpha;
                let s = self.state[i][k];
                self.gap[i][k] += ((t[k] - s) - self.gap[i][k]) * gap_alpha;
                let moving = self.gap[i][k].abs() > MOVING_GAP;
                let want = s.round().clamp(0.0, 255.0);
                let held = self.out[i][k];
                if moving {
                    // a fade or a swell: pass the exact value on
                    self.out[i][k] = s.clamp(0.0, 255.0);
                } else if (want - held).abs() >= self.deadband
                    || want == 0.0
                    || want == 255.0
                    || self.deadband <= 0.0
                {
                    self.out[i][k] = want;
                }
            }
        }
        self.out.clone()
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
    /// Accurate colour matching: the screen's encoded values become real light (`gamma`), no
    /// saturation boost and no purifying; `gains` correct a tint of the strip's LEDs.
    pub accurate: bool,
    /// Per-channel strength (R, G, B) in accurate mode, 0.0..=1.0.
    pub gains: [f32; 3],
    /// Colourfulness of the monitor relative to the captured picture (1.0 = the same), in accurate mode.
    pub vibrance: f32,
    /// Converts the screen's light into the strip's LED drive (see `led_matrix`), in accurate mode.
    pub matrix: [[f32; 3]; 3],
    /// How much of that conversion is applied, 0.0..=1.0.
    pub led_match: f32,
}

impl Default for Grade {
    /// The older "vivid" look; the app builds its own grade from the settings.
    fn default() -> Self {
        Self {
            saturation_gain: 2.0,
            gamma: 1.0,
            purify: true,
            knee_lo: 8,
            knee_hi: 28,
            brightness: 1.0,
            accurate: false,
            gains: [1.0; 3],
            vibrance: 1.0,
            matrix: IDENTITY,
            led_match: 0.0,
        }
    }
}

/// Soft black knee instead of a hard cutoff.
fn knee(c: Led, lo: f32, hi: f32) -> Led {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= lo {
        return [0.0; 3];
    }
    if m >= hi || hi <= lo {
        return c;
    }
    let f = (m - lo) / (hi - lo);
    [c[0] * f, c[1] * f, c[2] * f]
}

/// A dominant channel (>=200) with two weak ones (<=50) becomes a clean primary.
fn purify(c: Led) -> Led {
    let mut out = c;
    let mut dominant = None;
    let mut weak = [false; 3];
    let mut weak_count = 0;
    for (i, &v) in c.iter().enumerate() {
        if v >= 200.0 {
            dominant = Some(i);
        }
        if v <= 50.0 {
            weak[i] = true;
            weak_count += 1;
        }
    }
    if let Some(d) = dominant {
        if weak_count == 2 {
            out[d] = (out[d] * 1.5).min(255.0);
            for i in 0..3 {
                if weak[i] {
                    out[i] = 0.0;
                }
            }
        }
    }
    out
}

/// Faint stray channels beside a clearly dominant one are removed.
///
/// The strip's green and blue LEDs are far more efficient than its red one, so a few units of
/// blue beside a dimmer red (for example 180, 0, 6) already read as pink to the eye. A channel
/// below `LOW` of the strongest one is dropped, it fades back in up to `HIGH`, and everything
/// stronger than that (orange, pink, purple, skin tones, whites) is left exactly as it was.
fn suppress_stray(c: Led) -> Led {
    suppress_stray_between(c, 0.08, 0.30)
}

/// `suppress_stray` with its own thresholds (a channel below `low` of the strongest one is
/// dropped, from `high` up it is kept untouched).
fn suppress_stray_between(c: Led, low: f32, high: f32) -> Led {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= 0.0 {
        return c;
    }
    let keep = |v: f32| -> f32 {
        let r = v / m;
        let t = ((r - low) / (high - low)).clamp(0.0, 1.0);
        let w = t * t * (3.0 - 2.0 * t); // smoothstep
        v * w
    };
    // the strongest channel itself always stays
    [
        if c[0] >= m { m } else { keep(c[0]) },
        if c[1] >= m { m } else { keep(c[1]) },
        if c[2] >= m { m } else { keep(c[2]) },
    ]
}

fn saturate(c: Led, gain: f32) -> Led {
    if (gain - 1.0).abs() < 1e-3 {
        return c;
    }
    let (rf, gf, bf) = (c[0] / 255.0, c[1] / 255.0, c[2] / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta < 0.001 {
        return c;
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
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
    let q = if l < 0.5 {
        l * (1.0 + ns)
    } else {
        l + ns - l * ns
    };
    let p = 2.0 * l - q;
    let to255 = |v: f32| (v * 255.0).clamp(0.0, 255.0);
    [
        to255(hue(p, q, h + 1.0 / 3.0)),
        to255(hue(p, q, h)),
        to255(hue(p, q, h - 1.0 / 3.0)),
    ]
}

fn apply_gamma(c: Led, gamma: f32) -> Led {
    if (gamma - 1.0).abs() < 1e-3 {
        return c;
    }
    let f = |v: f32| ((v / 255.0).powf(gamma) * 255.0).clamp(0.0, 255.0);
    [f(c[0]), f(c[1]), f(c[2])]
}

/// Where the power limiter starts to ease in, as a fraction of the budget. Below it nothing is
/// touched; above it the total is compressed smoothly towards the budget, so a fade through the
/// limit never sits on a plateau, and the budget is still never exceeded.
const SOFT_KNEE: f64 = 0.7;

/// Whole-strip power budget. Individual LEDs may reach full intensity; only when the strip as a
/// whole approaches `max_fraction` of "every LED fully white" are all LEDs scaled down together
/// (keeps hue, protects weak USB ports). The total approaches the budget smoothly and never
/// exceeds it.
pub fn limit_total_power(leds: &mut [Led], max_fraction: f32) {
    if leds.is_empty() {
        return;
    }
    let f = if max_fraction.is_nan() {
        1.0
    } else {
        max_fraction.clamp(0.0, 1.0)
    };
    if f >= 1.0 {
        return; // a budget of everything-white can never be exceeded: no limit at all
    }
    let total: f64 = leds
        .iter()
        .map(|c| c[0] as f64 + c[1] as f64 + c[2] as f64)
        .sum();
    let allowed = (leds.len() as f64) * 765.0 * f as f64;
    if allowed <= 0.0 {
        // a zero budget switches the strip off
        for c in leds.iter_mut() {
            *c = [0.0; 3];
        }
        return;
    }
    let ratio = total / allowed;
    if ratio <= SOFT_KNEE || total <= 0.0 {
        return;
    }
    // ease in from the knee towards 1.0 (the budget), strictly increasing with the load
    let eased =
        SOFT_KNEE + (1.0 - SOFT_KNEE) * (1.0 - (-(ratio - SOFT_KNEE) / (1.0 - SOFT_KNEE)).exp());
    let k = (eased / ratio) as f32;
    for c in leds.iter_mut() {
        for ch in c.iter_mut() {
            *ch *= k;
        }
    }
}

/// Fraction of "every LED fully white" the frame would draw (0.0..=1.0).
pub fn power_fraction(leds: &[Led]) -> f32 {
    if leds.is_empty() {
        return 0.0;
    }
    let total: f64 = leds
        .iter()
        .map(|c| c[0] as f64 + c[1] as f64 + c[2] as f64)
        .sum();
    (total / (leds.len() as f64 * 765.0)) as f32
}

/// Full per-LED grade: knee, purify, saturation, stray removal, gamma, brightness. The whole-
/// strip power budget is applied afterwards by `limit_total_power`.
pub fn grade(c: Led, g: &Grade) -> Led {
    let mut c = knee(c, g.knee_lo as f32, g.knee_hi as f32);
    if g.accurate {
        // real light: the colour (hue and saturation) is exact, the overall level follows the
        // brightness curve, then the strip's tint is corrected
        c = exact_colour_at_level(c, ACCURATE_COLOUR_GAMMA, g.gamma, &g.matrix, g.led_match);
        c = vibrance(c, g.vibrance);
        c = suppress_stray_between(c, STRAY_LOW_LINEAR, STRAY_HIGH_LINEAR);
        c = [c[0] * g.gains[0], c[1] * g.gains[1], c[2] * g.gains[2]];
    } else {
        if g.purify {
            c = purify(c);
        }
        c = saturate(c, g.saturation_gain);
        c = suppress_stray(c);
        c = apply_gamma(c, g.gamma);
    }
    scale(c, g.brightness)
}

/// What the screen's encoding really means for the colour of a pixel: undoing a gamma of
/// `ACCURATE_COLOUR_GAMMA` on every channel gives the light of each primary.
const ACCURATE_COLOUR_GAMMA: f32 = 2.2;

const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// CIE xy chromaticities of a typical RGB strip's LEDs: about 625 nm red, 525 nm green and
/// 468 nm blue. They are more saturated than the screen's primaries and differ in hue: the strip's
/// green is bluer than the screen's yellow-green, its red deeper.
const LED_RED_XY: (f64, f64) = (0.69, 0.31);
const LED_GREEN_XY: (f64, f64) = (0.17, 0.70);
const LED_BLUE_XY: (f64, f64) = (0.14, 0.05);

/// Linear sRGB to CIE XYZ (D65).
const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.4124, 0.3576, 0.1805],
    [0.2126, 0.7152, 0.0722],
    [0.0193, 0.1192, 0.9505],
];

/// CIE XYZ of the strip's three LEDs at full drive, one per column. `green` and `blue` are their
/// luminance relative to the red LED's (so `[1, green, blue]`).
pub fn led_xyz_matrix(green: f32, blue: f32) -> [[f64; 3]; 3] {
    let column = |xy: (f64, f64), luminance: f64| {
        let (x, y) = xy;
        [x / y * luminance, luminance, (1.0 - x - y) / y * luminance]
    };
    let r = column(LED_RED_XY, 1.0);
    let g = column(LED_GREEN_XY, green as f64);
    let b = column(LED_BLUE_XY, blue as f64);
    [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]]
}

fn invert3(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let c = |r: usize, c: usize| m[r][c];
    let cof = [
        [
            c(1, 1) * c(2, 2) - c(1, 2) * c(2, 1),
            c(0, 2) * c(2, 1) - c(0, 1) * c(2, 2),
            c(0, 1) * c(1, 2) - c(0, 2) * c(1, 1),
        ],
        [
            c(1, 2) * c(2, 0) - c(1, 0) * c(2, 2),
            c(0, 0) * c(2, 2) - c(0, 2) * c(2, 0),
            c(0, 2) * c(1, 0) - c(0, 0) * c(1, 2),
        ],
        [
            c(1, 0) * c(2, 1) - c(1, 1) * c(2, 0),
            c(0, 1) * c(2, 0) - c(0, 0) * c(2, 1),
            c(0, 0) * c(1, 1) - c(0, 1) * c(1, 0),
        ],
    ];
    let det = c(0, 0) * cof[0][0] + c(0, 1) * cof[1][0] + c(0, 2) * cof[2][0];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = cof[i][j] / det;
        }
    }
    Some(out)
}

/// The conversion from the screen's light (linear sRGB) to the strip's LED drive that makes the
/// strip emit light of the same colour: `drive = M * linear_srgb`. White comes out neutral, and
/// every colour keeps its chromaticity (within what the LEDs can make). `green` and `blue` are
/// the strength of the strip's green and blue LEDs relative to its red one. Invalid input gives
/// the identity (no correction).
pub fn led_matrix(green: f32, blue: f32) -> [[f32; 3]; 3] {
    if !(green.is_finite() && blue.is_finite() && green > 0.0 && blue > 0.0) {
        return IDENTITY;
    }
    let Some(inv) = invert3(led_xyz_matrix(green, blue)) else {
        return IDENTITY;
    };
    let mut m = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let v: f64 = (0..3).map(|k| inv[i][k] * SRGB_TO_XYZ[k][j]).sum();
            m[i][j] = v as f32;
        }
    }
    m
}

fn mul3(m: &[[f32; 3]; 3], v: Led) -> Led {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// A colour with the exact hue and saturation of the screen's light, at a brightness set by its
/// own curve. The balance between the channels comes from real light (`colour_gamma`), converted
/// to the strip's LEDs by `matrix` (applied as far as `strength`, 0.0..=1.0); the level of the
/// strongest channel follows `level_gamma`. With both gammas equal and no correction this is a
/// plain gamma; a lower `level_gamma` keeps dark and mid scenes brighter without changing any
/// colour.
fn exact_colour_at_level(
    c: Led,
    colour_gamma: f32,
    level_gamma: f32,
    matrix: &[[f32; 3]; 3],
    strength: f32,
) -> Led {
    let m = c[0].max(c[1]).max(c[2]);
    let s = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if m <= 0.0 || (s <= 0.0 && (colour_gamma - level_gamma).abs() < 1e-3) {
        return apply_gamma(c, level_gamma);
    }
    let light = |v: f32| (v / 255.0).powf(colour_gamma);
    let lin = [light(c[0]), light(c[1]), light(c[2])];
    let top = lin[0].max(lin[1]).max(lin[2]);
    if top <= 0.0 {
        return [0.0; 3];
    }
    // the colour as ratios: the screen's own, and the LED drive that reproduces it
    let plain = [lin[0] / top, lin[1] / top, lin[2] / top];
    let mut ratios = plain;
    if s > 0.0 {
        let driven = mul3(matrix, lin).map(|v| v.max(0.0));
        let dtop = driven[0].max(driven[1]).max(driven[2]);
        if dtop > 0.0 {
            for k in 0..3 {
                ratios[k] = (1.0 - s) * plain[k] + s * driven[k] / dtop;
            }
        }
    }
    let rtop = ratios[0].max(ratios[1]).max(ratios[2]);
    if rtop <= 0.0 {
        return [0.0; 3];
    }
    let level = 255.0 * (m / 255.0).powf(level_gamma);
    [
        (ratios[0] / rtop * level).clamp(0.0, 255.0),
        (ratios[1] / rtop * level).clamp(0.0, 255.0),
        (ratios[2] / rtop * level).clamp(0.0, 255.0),
    ]
}

/// Scale the colourfulness of a colour in light, keeping its brightness. `s` of 1.0 changes
/// nothing; above 1.0 the colour gets more saturated (what a graphics driver's "digital
/// vibrance" does to the monitor), below 1.0 less.
fn vibrance(c: Led, s: f32) -> Led {
    if !s.is_finite() || (s - 1.0).abs() < 1e-3 {
        return c;
    }
    let luma = |c: Led| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let y = luma(c);
    let mut o = [0.0f32; 3];
    for k in 0..3 {
        o[k] = (y + s * (c[k] - y)).max(0.0);
    }
    // cutting negative light adds brightness back: restore the original
    let y2 = luma(o);
    if y2 > 1e-6 {
        let f = y / y2;
        for v in o.iter_mut() {
            *v = (*v * f).min(255.0);
        }
    }
    o
}

/// In accurate mode the strong LEDs' trace amounts are what make a deep red look pink; a channel
/// under 1.5% of the strongest one (in light) is removed, from 6% up it is kept untouched.
const STRAY_LOW_LINEAR: f32 = 0.015;
const STRAY_HIGH_LINEAR: f32 = 0.06;

fn scale(c: Led, b: f32) -> Led {
    let b = if b.is_nan() { 1.0 } else { b.clamp(0.0, 1.0) };
    if b >= 0.999 {
        return c;
    }
    [c[0] * b, c[1] * b, c[2] * b]
}

/// A channel whose value changes by less than this between frames counts as holding still.
const STILL_EPS: f32 = 0.02;

/// The last step: rounding to the strip's 8-bit levels with temporal dithering.
///
/// While a channel is changing, it alternates between the two neighbouring levels in proportion
/// to its fractional part (a low-discrepancy sequence, offset per LED and channel so the strip
/// does not flicker in step). Averaged by the eye over a few frames, a slow fade then moves in
/// steps far smaller than one level instead of in visible jumps. A channel that is not changing
/// is simply rounded, so a still picture never shimmers.
#[derive(Default)]
pub struct Dither {
    phase: u32,
    prev: Vec<Led>,
}

impl Dither {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, leds: &[Led]) -> Vec<[u8; 3]> {
        if self.prev.len() != leds.len() {
            self.prev = leds.to_vec(); // first frame (or a different strip): plain rounding
        }
        self.phase = self.phase.wrapping_add(1);
        let mut out = Vec::with_capacity(leds.len());
        for (i, c) in leds.iter().enumerate() {
            let mut o = [0u8; 3];
            for k in 0..3 {
                let v = if c[k].is_finite() {
                    c[k].clamp(0.0, 255.0)
                } else {
                    0.0
                };
                let moving = (v - self.prev[i][k]).abs() > STILL_EPS;
                let level = if moving {
                    (v + dither_threshold(self.phase, i, k)).floor()
                } else {
                    v.round()
                };
                o[k] = level.clamp(0.0, 255.0) as u8;
            }
            out.push(o);
        }
        self.prev = leds
            .iter()
            .map(|c| c.map(|v| if v.is_finite() { v } else { 0.0 }))
            .collect();
        out
    }
}

/// Threshold in [0, 1) for this frame, LED and channel (golden-ratio low-discrepancy sequence).
fn dither_threshold(phase: u32, led: usize, channel: usize) -> f32 {
    const STEP: f64 = 0.618_033_988_749_894_8; // consecutive frames land far apart
    const OFFSET: f64 = 0.754_877_666_246_692_7; // each LED/channel starts somewhere else
    (phase as f64 * STEP + (led * 3 + channel) as f64 * OFFSET).fract() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(n: usize, c: [u8; 3]) -> Vec<[u8; 3]> {
        vec![c; n]
    }

    fn led(c: [u8; 3]) -> Led {
        [c[0] as f32, c[1] as f32, c[2] as f32]
    }

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn first_frame_passes_through() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        assert_eq!(
            s.update(&solid(3, [10, 20, 30]), 33.0),
            vec![[10.0, 20.0, 30.0]; 3]
        );
    }

    #[test]
    fn attack_is_faster_than_decay() {
        let mut up = Smoother::new(40.0, 250.0, 0.0);
        up.update(&solid(1, [0, 0, 0]), 33.0);
        let a = up.update(&solid(1, [100, 100, 100]), 33.0)[0][0];

        let mut down = Smoother::new(40.0, 250.0, 0.0);
        down.update(&solid(1, [100, 100, 100]), 33.0);
        let d = 100.0 - down.update(&solid(1, [0, 0, 0]), 33.0)[0][0];

        assert!(a > d, "attack step {a} should exceed decay step {d}");
    }

    #[test]
    fn frame_rate_independent() {
        // one 66 ms step must equal two 33 ms steps (same target)
        let mut a = Smoother::new(100.0, 100.0, 0.0);
        let mut b = Smoother::new(100.0, 100.0, 0.0);
        a.update(&solid(1, [0, 0, 0]), 33.0);
        b.update(&solid(1, [0, 0, 0]), 33.0);
        let one = a.update(&solid(1, [80, 80, 80]), 66.0)[0][0];
        b.update(&solid(1, [80, 80, 80]), 33.0);
        let two = b.update(&solid(1, [80, 80, 80]), 33.0)[0][0];
        assert!(near(one, two, 1.0), "{one} vs {two}");
    }

    #[test]
    fn converges_to_target() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        s.update(&solid(1, [0, 0, 0]), 33.0);
        let mut out = [0.0f32; 3];
        for _ in 0..200 {
            out = s.update(&solid(1, [200, 100, 50]), 33.0)[0];
        }
        assert!(near(out[0], 200.0, 2.0) && near(out[1], 100.0, 2.0) && near(out[2], 50.0, 2.0));
    }

    #[test]
    fn deadband_suppresses_single_step_flicker() {
        let mut s = Smoother::new(0.0, 0.0, 3.0);
        s.update(&solid(1, [100, 100, 100]), 33.0);
        // +-2 jitter stays invisible
        assert_eq!(
            s.update(&solid(1, [102, 98, 101]), 33.0)[0],
            [100.0, 100.0, 100.0]
        );
        // a real change passes
        assert_eq!(
            s.update(&solid(1, [110, 100, 100]), 33.0)[0],
            [110.0, 100.0, 100.0]
        );
    }

    #[test]
    fn scene_cut_snaps() {
        let mut s = Smoother::new(40.0, 5000.0, 0.0);
        s.update(&solid(10, [255, 255, 255]), 33.0);
        assert_eq!(s.update(&solid(10, [0, 0, 0]), 33.0), vec![[0.0; 3]; 10]);
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
            assert!(o.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn knee_is_continuous_and_zero_at_black() {
        assert_eq!(knee(led([5, 5, 5]), 8.0, 28.0), [0.0; 3]);
        assert_eq!(knee(led([8, 8, 8]), 8.0, 28.0), [0.0; 3]);
        let mid = knee(led([18, 18, 18]), 8.0, 28.0);
        assert!(mid[0] > 0.0 && mid[0] < 18.0);
        assert_eq!(knee(led([28, 28, 28]), 8.0, 28.0), led([28, 28, 28]));
    }

    #[test]
    fn saturation_identity_and_grey() {
        assert_eq!(saturate(led([120, 80, 40]), 1.0), led([120, 80, 40]));
        assert_eq!(saturate(led([90, 90, 90]), 3.0), led([90, 90, 90]));
        let boosted = saturate(led([140, 100, 100]), 2.0);
        assert!(boosted[0] - boosted[1] > 40.0);
    }

    #[test]
    fn power_limit_leaves_dim_scenes_alone() {
        let mut leds = vec![[10.0; 3]; 71];
        let before = leds.clone();
        limit_total_power(&mut leds, 0.5);
        assert_eq!(leds, before);
    }

    #[test]
    fn power_limit_allows_bright_highlights_on_dark_scene() {
        let mut leds = vec![[0.0; 3]; 71];
        for l in leds.iter_mut().take(5) {
            *l = [255.0; 3];
        }
        limit_total_power(&mut leds, 0.5);
        assert_eq!(leds[0], [255.0; 3]);
    }

    #[test]
    fn power_limit_scales_everything_together_when_over_budget() {
        let mut leds = vec![[255.0; 3]; 71];
        limit_total_power(&mut leds, 0.5);
        let sum: f64 = leds
            .iter()
            .map(|c| c.iter().map(|&v| v as f64).sum::<f64>())
            .sum();
        assert!(sum <= 71.0 * 765.0 * 0.5 + 1.0);
        assert!(
            leds.iter().all(|c| *c == leds[0]),
            "uniform scene stays uniform"
        );
        assert!(
            leds[0][0] >= 126.0,
            "about half brightness, not less: {}",
            leds[0][0]
        );
    }

    #[test]
    fn power_limit_preserves_hue_and_handles_edge_input() {
        let mut leds = vec![[200.0, 100.0, 50.0]; 10];
        limit_total_power(&mut leds, 0.1);
        let c = leds[0];
        assert!(c[0] > c[1] && c[1] > c[2]);
        limit_total_power(&mut [], 0.5);
        let mut z = vec![[0.0; 3]; 4];
        limit_total_power(&mut z, 0.0);
        assert_eq!(z, vec![[0.0; 3]; 4]);
        let mut n = vec![[255.0; 3]; 4];
        limit_total_power(&mut n, f32::NAN);
        assert_eq!(n, vec![[255.0; 3]; 4]);
    }

    #[test]
    fn power_fraction_bounds() {
        assert_eq!(power_fraction(&[]), 0.0);
        assert_eq!(power_fraction(&[[0.0; 3]; 5]), 0.0);
        assert!((power_fraction(&[[255.0; 3]; 5]) - 1.0).abs() < 1e-6);
        let mut leds = vec![[255.0; 3]; 71];
        limit_total_power(&mut leds, 0.4);
        assert!(power_fraction(&leds) <= 0.4 + 0.001);
    }

    #[test]
    fn grade_applies_saturation_to_bright_pixels() {
        // stock bug: bright pixels lost the saturation boost entirely
        let g = Grade {
            purify: false,
            ..Grade::default()
        };
        let plain = Grade {
            saturation_gain: 1.0,
            purify: false,
            ..Grade::default()
        };
        let a = grade(led([220, 160, 140]), &g);
        let b = grade(led([220, 160, 140]), &plain);
        assert!((a[0] - a[2]) > (b[0] - b[2]));
    }

    #[test]
    fn grade_black_stays_black_and_full_white_stays_full() {
        let g = Grade::default();
        assert_eq!(grade([0.0; 3], &g), [0.0; 3]);
        assert_eq!(grade([255.0; 3], &g), [255.0; 3]);
    }

    #[test]
    fn brightness_scales_output_last() {
        let full = Grade::default();
        let half = Grade {
            brightness: 0.5,
            ..Grade::default()
        };
        let a = grade(led([200, 40, 40]), &full);
        let b = grade(led([200, 40, 40]), &half);
        for k in 0..3 {
            assert!(near(b[k], a[k] * 0.5, 0.01));
        }
        assert_eq!(
            grade(
                led([200, 200, 200]),
                &Grade {
                    brightness: 0.0,
                    ..full
                }
            ),
            [0.0; 3]
        );
        assert_eq!(scale(led([10, 20, 30]), f32::NAN), led([10, 20, 30]));
        assert_eq!(scale(led([10, 20, 30]), 9.0), led([10, 20, 30]));
    }

    #[test]
    fn a_colour_change_does_not_linger_in_the_old_hue() {
        // white turning red: brightness falls, but the colour must follow quickly
        let mut s = Smoother::new(40.0, 250.0, 0.0);
        s.cut_threshold = 1.0e9; // no scene-cut snap: test the filter itself
        s.update(&[[200, 200, 200]], 16.0);
        let mut out = [200.0f32; 3];
        for _ in 0..5 {
            out = s.update(&[[200, 0, 0]], 16.0)[0]; // 80 ms
        }
        assert!(
            out[1] < 60.0 && out[2] < 60.0,
            "green/blue still lingering: {out:?}"
        );
        assert!(out[0] >= 190.0);
    }

    #[test]
    fn plain_dimming_still_fades_slowly() {
        let mut s = Smoother::new(40.0, 250.0, 0.0);
        s.cut_threshold = 1.0e9;
        s.update(&[[200, 200, 200]], 16.0);
        let mut out = [200.0f32; 3];
        for _ in 0..5 {
            out = s.update(&[[100, 100, 100]], 16.0)[0]; // 80 ms
        }
        assert!(out[0] > 160.0, "dimming must stay gentle: {out:?}");
    }

    #[test]
    fn hue_change_is_zero_for_the_same_hue_and_ignores_near_black() {
        assert_eq!(hue_change([200.0, 100.0, 50.0], [100.0, 50.0, 25.0]), 0.0);
        assert_eq!(hue_change([5.0, 0.0, 0.0], [0.0, 0.0, 5.0]), 0.0);
        assert_eq!(hue_change([255.0, 255.0, 255.0], [255.0, 0.0, 0.0]), 1.0);
        let m = hue_change([200.0, 200.0, 200.0], [200.0, 170.0, 200.0]);
        assert!(m > 0.0 && m < 1.0);
    }

    #[test]
    fn purify_makes_clean_primary() {
        assert_eq!(purify(led([210, 30, 20])), [255.0, 0.0, 0.0]);
        assert_eq!(purify(led([100, 100, 100])), led([100, 100, 100]));
    }

    #[test]
    fn stray_blue_beside_a_dimmer_red_is_removed() {
        // the observed case: a dark red sent as 180, 0, 6 looked pink on the strip
        assert_eq!(suppress_stray(led([180, 0, 6])), [180.0, 0.0, 0.0]);
        let c = suppress_stray(led([120, 5, 10]));
        assert!(c[1] < 0.01 && c[2] < 0.05, "{c:?}");
        assert_eq!(suppress_stray(led([60, 2, 4])), [60.0, 0.0, 0.0]);
        // the same for the other primaries
        assert_eq!(suppress_stray(led([4, 150, 8])), [0.0, 150.0, 0.0]);
        assert_eq!(suppress_stray(led([3, 5, 200])), [0.0, 0.0, 200.0]);
    }

    #[test]
    fn real_colors_are_left_exactly_as_they_were() {
        for c in [
            [255, 0, 0],
            [0, 0, 0],
            [255, 255, 255],
            [100, 100, 100],
            [255, 140, 0],   // orange
            [255, 120, 160], // pink stays pink
            [200, 0, 120],   // magenta
            [230, 180, 150], // skin
            [0, 200, 200],   // cyan
            [200, 200, 0],   // yellow
            [90, 40, 30],    // brown
        ] {
            assert_eq!(suppress_stray(led(c)), led(c), "{c:?}");
        }
    }

    #[test]
    fn stray_removal_fades_in_gradually_and_never_touches_the_strongest_channel() {
        let mut prev = 0.0;
        for b in 0..=100u8 {
            let out = suppress_stray([200.0, 0.0, b as f32]);
            assert_eq!(out[0], 200.0);
            assert!(out[2] >= prev, "monotonic in the weak channel");
            assert!(out[2] <= b as f32);
            prev = out[2];
        }
        // fully gone below 8% of the strongest channel, fully kept from 30%
        assert_eq!(suppress_stray([200.0, 0.0, 16.0])[2], 0.0);
        assert!(near(suppress_stray([200.0, 0.0, 60.0])[2], 60.0, 0.01));
        // ties keep both strongest channels
        assert_eq!(suppress_stray([200.0, 200.0, 3.0]), [200.0, 200.0, 0.0]);
    }

    fn accurate(gamma: f32, gains: [f32; 3]) -> Grade {
        Grade {
            accurate: true,
            gamma,
            gains,
            ..Grade::default()
        }
    }

    /// Light the screen emits for an encoded value, in the strip's 0..255 drive units.
    fn light(v: f32, gamma: f32) -> f32 {
        255.0 * (v / 255.0).powf(gamma)
    }

    #[test]
    fn accurate_mode_turns_encoded_values_into_real_light() {
        let g = accurate(2.2, [1.0; 3]);
        // the colours reported wrong on the strip: a dark red-brown and a pastel tan
        let brown = grade(led([60, 30, 20]), &g);
        let tan = grade(led([200, 140, 90]), &g);
        for (out, src) in [(brown, [60.0, 30.0, 20.0]), (tan, [200.0, 140.0, 90.0])] {
            for k in 0..3 {
                assert!(
                    near(out[k], light(src[k], 2.2), 0.6),
                    "{src:?} channel {k}: {} vs {}",
                    out[k],
                    light(src[k], 2.2)
                );
            }
        }
        // no saturation boost: the brown stays dark and red-brown, the tan stays a soft amber
        assert!(brown[0] < 12.0 && brown[1] < 3.0);
        assert!(tan[1] / tan[0] > 0.4 && tan[2] / tan[0] > 0.15);
        // pure primaries, white and black are exact
        assert_eq!(grade([255.0, 0.0, 0.0], &g), [255.0, 0.0, 0.0]);
        assert_eq!(grade([0.0, 255.0, 0.0], &g), [0.0, 255.0, 0.0]);
        assert_eq!(grade([255.0; 3], &g), [255.0; 3]);
        assert_eq!(grade([0.0; 3], &g), [0.0; 3]);
    }

    #[test]
    fn accurate_mode_keeps_every_colour_in_a_sweep() {
        // 17 steps per channel: 4913 colours covering shades, pastels, browns, skin, greys...
        let g = accurate(2.2, [1.0; 3]);
        let steps: Vec<f32> = (0..=16).map(|i| i as f32 * 15.9375).collect();
        let mut checked = 0;
        for &r in &steps {
            for &gr in &steps {
                for &b in &steps {
                    let out = grade([r, gr, b], &g);
                    assert!(
                        out.iter()
                            .all(|v| v.is_finite() && (0.0..=255.0).contains(v)),
                        "{r} {gr} {b} -> {out:?}"
                    );
                    let m = r.max(gr).max(b);
                    if m < 40.0 {
                        continue; // the black knee shapes near-black on purpose
                    }
                    let want = [light(r, 2.2), light(gr, 2.2), light(b, 2.2)];
                    let top = want[0].max(want[1]).max(want[2]);
                    // the strongest channel is exact; every other channel is exact unless it is
                    // a trace (under 6% of the strongest) that the stray filter may remove
                    for k in 0..3 {
                        if want[k] / top >= STRAY_HIGH_LINEAR {
                            assert!(
                                near(out[k], want[k], 0.01 * want[k] + 0.3),
                                "{r} {gr} {b} channel {k}: {} vs {}",
                                out[k],
                                want[k]
                            );
                        } else {
                            assert!(out[k] <= want[k] + 0.3, "traces are only ever removed");
                        }
                    }
                    // the order of the channels is never swapped
                    for (i, j) in [(0, 1), (1, 2), (0, 2)] {
                        if want[i] > want[j] {
                            assert!(out[i] >= out[j], "{r} {gr} {b}: order swapped {out:?}");
                        }
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 4000, "{checked}");
    }

    #[test]
    fn vibrance_keeps_brightness_and_greys_and_only_changes_colourfulness() {
        let luma = |c: Led| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let chroma = |c: Led| {
            let top = c[0].max(c[1]).max(c[2]).max(1e-6);
            (top - c[0].min(c[1]).min(c[2])) / top
        };
        // 1.0 changes nothing, a grey never changes
        assert_eq!(vibrance([120.0, 80.0, 40.0], 1.0), [120.0, 80.0, 40.0]);
        for s in [0.5, 1.4, 2.0] {
            let g = vibrance([90.0; 3], s);
            assert!(g.iter().all(|v| near(*v, 90.0, 0.01)), "grey at {s}: {g:?}");
        }
        for c in [
            [150.0, 80.0, 60.0],
            [60.0, 120.0, 90.0],
            [100.0, 90.0, 150.0],
        ] {
            let more = vibrance(c, 1.5);
            let less = vibrance(c, 0.6);
            assert!(
                chroma(more) > chroma(c) && chroma(less) < chroma(c),
                "{c:?}"
            );
            assert!(
                near(luma(more), luma(c), 0.5) && near(luma(less), luma(c), 0.5),
                "{c:?}"
            );
            assert!(more.iter().all(|v| (0.0..=255.0).contains(v)));
        }
        assert_eq!(vibrance([10.0, 20.0, 30.0], f32::NAN), [10.0, 20.0, 30.0]);
    }

    /// Chromaticity (CIE xy) of the light a drive vector makes on the modelled strip.
    fn led_xy(drive: Led, green: f32, blue: f32) -> (f64, f64) {
        let p = led_xyz_matrix(green, blue);
        let d = [drive[0] as f64, drive[1] as f64, drive[2] as f64];
        let xyz: Vec<f64> = (0..3)
            .map(|i| p[i][0] * d[0] + p[i][1] * d[1] + p[i][2] * d[2])
            .collect();
        let sum = xyz.iter().sum::<f64>();
        (xyz[0] / sum, xyz[1] / sum)
    }

    /// Chromaticity (CIE xy) of what the screen shows for a linear sRGB colour.
    fn screen_xy(lin: [f64; 3]) -> (f64, f64) {
        let xyz: Vec<f64> = (0..3)
            .map(|i| (0..3).map(|j| SRGB_TO_XYZ[i][j] * lin[j]).sum::<f64>())
            .collect();
        let sum = xyz.iter().sum::<f64>();
        (xyz[0] / sum, xyz[1] / sum)
    }

    #[test]
    fn the_led_matrix_makes_white_neutral_and_keeps_every_colour_of_the_screen() {
        for (green, blue) in [(2.0f32, 0.25f32), (2.4, 0.21), (1.6, 0.5), (3.0, 0.15)] {
            let m = led_matrix(green, blue);
            // white: the strip's light is D65, as the screen's
            let white = mul3(&m, [1.0; 3]);
            let (wx, wy) = led_xy(white, green, blue);
            assert!(
                (wx - 0.3127).abs() < 0.003 && (wy - 0.3290).abs() < 0.003,
                "white at {green}/{blue}: {wx:.4} {wy:.4}"
            );
            // the screen's primaries come out with the screen's chromaticity
            for (k, want) in [(0usize, (0.64, 0.33)), (1, (0.30, 0.60)), (2, (0.15, 0.06))] {
                let mut lin = [0.0f32; 3];
                lin[k] = 1.0;
                let d = mul3(&m, lin);
                let (x, y) = led_xy(d, green, blue);
                assert!(
                    d.iter().all(|v| *v >= -1e-3),
                    "primary {k} needs a negative LED at {green}/{blue}: {d:?}"
                );
                assert!(
                    (x - want.0).abs() < 0.004 && (y - want.1).abs() < 0.004,
                    "primary {k} at {green}/{blue}: {x:.4} {y:.4}"
                );
            }
        }
        // no or invalid strip data: no correction at all
        assert_eq!(led_matrix(f32::NAN, 0.3), IDENTITY);
        assert_eq!(led_matrix(2.0, 0.0), IDENTITY);
        assert_eq!(led_matrix(-1.0, 0.3), IDENTITY);
    }

    #[test]
    fn corrected_colours_match_the_screens_chromaticity_across_a_sweep() {
        let (green, blue) = (2.0f32, 0.25f32);
        let m = led_matrix(green, blue);
        let steps: Vec<f32> = (0..=16).map(|i| i as f32 * 15.9375).collect();
        let mut worst = 0.0f64;
        let mut checked = 0;
        for &r in &steps {
            for &gr in &steps {
                for &b in &steps {
                    if r.max(gr).max(b) < 60.0 {
                        continue;
                    }
                    let out = exact_colour_at_level([r, gr, b], 2.2, 2.2, &m, 1.0);
                    let lin = [
                        (r as f64 / 255.0).powf(2.2),
                        (gr as f64 / 255.0).powf(2.2),
                        (b as f64 / 255.0).powf(2.2),
                    ];
                    // colours the LEDs cannot make without a negative drive are clamped: skip
                    let raw = mul3(&m, [lin[0] as f32, lin[1] as f32, lin[2] as f32]);
                    if raw.iter().any(|v| *v < -1e-4) {
                        continue;
                    }
                    let (x, y) = led_xy(out, green, blue);
                    let (sx, sy) = screen_xy(lin);
                    let err = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt();
                    worst = worst.max(err);
                    checked += 1;
                }
            }
        }
        assert!(checked > 3000, "{checked}");
        // within the rounding of the drive values, a colour is the same colour
        assert!(worst < 0.01, "worst chromaticity error {worst}");
    }

    #[test]
    fn the_observed_wrong_colours_are_corrected_in_the_right_direction() {
        let m = led_matrix(2.0, 0.25);
        let drive = |c: [f32; 3]| exact_colour_at_level(c, 2.2, 2.2, &m, 1.0);
        let plain = |c: [f32; 3]| exact_colour_at_level(c, 2.2, 2.2, &IDENTITY, 0.0);
        // bright green looked light blue: it needs red mixed in
        let green = drive([0.0, 200.0, 0.0]);
        assert!(green[0] > 0.2 * green[1] && green[2] < 1.0, "{green:?}");
        assert_eq!(plain([0.0, 200.0, 0.0])[0], 0.0);
        // yellow looked warm: less red against green than before
        let (y, y0) = (drive([255.0, 255.0, 0.0]), plain([255.0, 255.0, 0.0]));
        assert!(y[0] / y[1] < y0[0] / y0[1] - 0.05, "{y:?} vs {y0:?}");
        // white looked bluish: less blue (and red) against green
        let (w, w0) = (drive([255.0; 3]), plain([255.0; 3]));
        assert!(w[2] < 0.85 * w0[2] && w[0] < w0[0], "{w:?} vs {w0:?}");
        // pure red and blue stay essentially pure
        let red = drive([255.0, 0.0, 0.0]);
        assert!(red[1] < 0.15 * red[0] && red[2] < 0.05 * red[0], "{red:?}");
        let blue = drive([0.0, 0.0, 255.0]);
        assert!(
            blue[0] < 0.08 * blue[2] && blue[1] < 0.05 * blue[2],
            "{blue:?}"
        );
        // a strength of zero is no correction, half is in between
        let half = exact_colour_at_level([0.0, 200.0, 0.0], 2.2, 2.2, &m, 0.5);
        assert!(half[0] > 0.0 && half[0] < green[0]);
        assert_eq!(
            exact_colour_at_level([60.0, 30.0, 20.0], 2.2, 2.2, &m, 0.0),
            plain([60.0, 30.0, 20.0])
        );
    }

    #[test]
    fn a_gentler_brightness_curve_changes_the_level_but_never_the_colour() {
        let exact = accurate(2.2, [1.0; 3]);
        let bright = accurate(1.6, [1.0; 3]);
        for c in [
            [60, 30, 20],
            [200, 140, 90],
            [170, 10, 15],
            [135, 206, 235],
            [40, 40, 40],
            [141, 85, 36],
        ] {
            let a = grade(led(c), &exact);
            let b = grade(led(c), &bright);
            let (ta, tb) = (a[0].max(a[1]).max(a[2]), b[0].max(b[1]).max(b[2]));
            assert!(
                tb > ta,
                "{c:?}: the gentler curve is brighter ({tb} vs {ta})"
            );
            for k in 0..3 {
                // same ratio between every channel and the strongest one
                assert!(
                    near(a[k] / ta, b[k] / tb, 0.01),
                    "{c:?} channel {k}: colour changed {} vs {}",
                    a[k] / ta,
                    b[k] / tb
                );
            }
            // the strongest channel follows the level curve exactly
            let m = c.iter().copied().max().unwrap() as f32;
            assert!(
                near(tb, light(m, 1.6), 0.6),
                "{c:?}: {tb} vs {}",
                light(m, 1.6)
            );
        }
        // equal curves are a plain gamma; white and primaries stay full
        assert_eq!(grade([255.0; 3], &bright), [255.0; 3]);
        assert_eq!(grade([255.0, 0.0, 0.0], &bright), [255.0, 0.0, 0.0]);
        // and a fade is still strictly monotonic
        let mut prev = -1.0f32;
        for v in 40..=255 {
            let out = grade([v as f32; 3], &bright)[0];
            assert!(out > prev);
            prev = out;
        }
    }

    #[test]
    fn accurate_balance_lowers_one_channel_only() {
        let full = grade(led([200, 150, 120]), &accurate(2.2, [1.0; 3]));
        let blue_down = grade(led([200, 150, 120]), &accurate(2.2, [1.0, 1.0, 0.5]));
        assert!(near(blue_down[0], full[0], 0.01) && near(blue_down[1], full[1], 0.01));
        assert!(near(blue_down[2], full[2] * 0.5, 0.01));
    }

    #[test]
    fn accurate_grey_ramp_follows_the_screen_in_every_step() {
        // a fade of the whole screen: the strip must change with every step of the screen
        let g = accurate(2.2, [1.0; 3]);
        let mut prev = -1.0f32;
        for v in 0..=255 {
            let out = grade([v as f32; 3], &g)[0];
            assert!(out >= prev, "never brighter before darker: {v} -> {out}");
            if v >= 40 {
                assert!(out > prev, "no flat step at {v}");
            }
            prev = out;
        }
    }

    #[test]
    fn the_power_limit_eases_in_so_a_fade_has_no_plateau() {
        // a uniform scene getting brighter: the limiter never flattens the response completely
        let mut prev = 0.0f32;
        for step in 1..=100 {
            let level = step as f32 * 2.55;
            let mut leds = vec![[level; 3]; 71];
            limit_total_power(&mut leds, 0.4);
            let out = leds[0][0];
            assert!(out > prev, "plateau at level {level}: {out} after {prev}");
            assert!(power_fraction(&leds) <= 0.4 + 1e-4, "budget exceeded");
            prev = out;
        }
        // a zero budget switches the strip off
        let mut leds = vec![[200.0; 3]; 4];
        limit_total_power(&mut leds, 0.0);
        assert_eq!(leds, vec![[0.0; 3]; 4]);
    }

    #[test]
    fn deep_red_goes_through_the_whole_grade_as_clean_red() {
        let g = Grade::default();
        // screen (170, 10, 15), as seen in testing
        let out = grade(led([170, 10, 15]), &g);
        assert!(out[1] < 0.01 && out[2] < 0.01, "{out:?}");
        assert!(out[0] >= 170.0);
        assert_eq!(grade([255.0, 0.0, 0.0], &g), [255.0, 0.0, 0.0]);
    }

    // ── fractional values and dithering ──

    #[test]
    fn dither_rounds_a_still_value_and_never_flickers_it() {
        let mut d = Dither::new();
        let frame = vec![[100.4, 50.6, 0.3]; 5];
        for _ in 0..200 {
            assert_eq!(d.apply(&frame), vec![[100, 51, 0]; 5]);
        }
    }

    #[test]
    fn dither_averages_to_the_exact_value_on_a_slow_ramp() {
        // 0.025 level per frame: far below one level, so plain rounding would stair-step
        let mut d = Dither::new();
        let (mut sum_in, mut sum_out) = (0.0f64, 0.0f64);
        let mut prev = None::<u8>;
        let mut max_jump = 0i32;
        for n in 0..1200 {
            let v = 40.0 + 0.025 * n as f32;
            let out = d.apply(&[[v, v, v]])[0][0];
            sum_in += v as f64;
            sum_out += out as f64;
            if let Some(p) = prev {
                max_jump = max_jump.max((out as i32 - p as i32).abs());
            }
            prev = Some(out);
        }
        assert!(
            (sum_in - sum_out).abs() < 2.0,
            "dithered output must average to the true value: {sum_in} vs {sum_out}"
        );
        assert!(max_jump <= 1, "never more than one level between frames");
    }

    #[test]
    fn dither_handles_nan_and_out_of_range() {
        let mut d = Dither::new();
        assert_eq!(
            d.apply(&[[f32::NAN, 300.0, -5.0], [f32::INFINITY, 0.0, 255.0]]),
            vec![[0, 255, 0], [0, 0, 255]]
        );
        // a different strip length starts over cleanly
        assert_eq!(d.apply(&[[1.0, 2.0, 3.0]]), vec![[1, 2, 3]]);
        assert_eq!(d.apply(&[]), Vec::<[u8; 3]>::new());
    }

    #[test]
    fn dither_threshold_stays_in_range_and_differs_between_leds() {
        for phase in [0u32, 1, 2, 1000, u32::MAX] {
            for led in [0usize, 1, 70, 254] {
                for k in 0..3 {
                    let t = dither_threshold(phase, led, k);
                    assert!((0.0..1.0).contains(&t), "{phase} {led} {k} -> {t}");
                }
            }
        }
        assert_ne!(dither_threshold(7, 0, 0), dither_threshold(7, 1, 0));
    }

    #[test]
    fn the_smoother_passes_exact_values_while_fading() {
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        s.cut_threshold = 1.0e9;
        s.update(&solid(1, [200, 200, 200]), 16.0);
        let mut fractional = 0;
        for _ in 0..60 {
            let v = s.update(&solid(1, [0, 0, 0]), 16.0)[0][0];
            if v > 5.0 && (v - v.round()).abs() > 0.05 {
                fractional += 1;
            }
        }
        assert!(
            fractional > 20,
            "a fade must pass on fractional values: {fractional}"
        );
    }

    #[test]
    fn a_noisy_still_picture_does_not_shimmer() {
        // compression noise of +-2 levels on a dark, otherwise still scene
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        let mut d = Dither::new();
        let g = Grade::default();
        let mut changes = 0;
        let mut prev: Option<Vec<[u8; 3]>> = None;
        for n in 0..900u32 {
            let noise = ((n.wrapping_mul(7919).wrapping_add(13)) % 5) as i32 - 2;
            let v = (60 + noise) as u8;
            let smoothed = s.update(&solid(10, [v, v / 2, v / 3]), 16.7);
            let graded: Vec<Led> = smoothed.iter().map(|&c| grade(c, &g)).collect();
            let out = d.apply(&graded);
            if n >= 300 && prev.as_ref().is_some_and(|p| *p != out) {
                changes += 1;
            }
            prev = Some(out);
        }
        assert!(
            changes <= 6,
            "the strip must stay quiet on a noisy still picture: {changes} changes in 600 frames"
        );
    }

    /// Slow fade of a bright white scene to black as the strip is driven: smoothing, grade, power
    /// limit and dithering together, 60 frames a second. Returns, per frame, the strip's level
    /// (what is sent) and the ideal fractional level before rounding.
    fn run_fade(seconds: f32) -> Vec<(i32, f32)> {
        let g = Grade::default();
        let mut s = Smoother::new(40.0, 250.0, 2.0);
        s.cut_threshold = 1.0e9;
        let mut d = Dither::new();
        let frames = (seconds * 60.0) as usize;
        let mut out = Vec::new();
        s.update(&solid(71, [255, 255, 255]), 16.7);
        for n in 0..frames + 90 {
            let level = if n < frames {
                255.0 * (1.0 - n as f32 / frames as f32)
            } else {
                0.0
            };
            let v = level.round() as u8;
            let smoothed = s.update(&solid(71, [v, v, v]), 16.7);
            let mut graded: Vec<Led> = smoothed.iter().map(|&c| grade(c, &g)).collect();
            limit_total_power(&mut graded, 0.4);
            let ideal = graded[0][0];
            out.push((d.apply(&graded)[0][0] as i32, ideal));
        }
        out
    }

    #[test]
    fn a_slow_fade_to_black_follows_the_exact_curve_in_fine_steps() {
        for seconds in [3.0, 6.0, 12.0] {
            let run = run_fade(seconds);
            // ends at black
            assert_eq!(run.last().map(|r| r.0), Some(0));
            // what the eye integrates (6 frames) stays within half a level of the exact value,
            // where plain rounding with a hold band is up to a whole level or more off
            let mut worst = 0.0f32;
            for w in run.windows(6) {
                let sent = w.iter().map(|r| r.0 as f32).sum::<f32>() / 6.0;
                let ideal = w.iter().map(|r| r.1).sum::<f32>() / 6.0;
                worst = worst.max((sent - ideal).abs());
            }
            assert!(
                worst <= 0.75,
                "{seconds} s fade: dithered output strays {worst} levels from the exact curve"
            );
            let levels: Vec<i32> = run.iter().map(|r| r.0).collect();
            // and no single frame jumps by more than the grade's own steepest slope allows
            let max_jump = levels
                .windows(2)
                .map(|p| (p[0] - p[1]).abs())
                .max()
                .unwrap_or(0);
            assert!(
                max_jump <= 5,
                "{seconds} s fade: single-frame jump of {max_jump}"
            );
        }
    }
}
