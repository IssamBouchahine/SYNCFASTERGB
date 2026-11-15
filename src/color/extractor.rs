//! Screen edge -> LED colors.
//!
//! 1. Optionally measure black bars (letterbox / pillarbox) on each side of the picture.
//! 2. Sample a strip of adjustable depth along each edge, just inside the bars.
//! 3. Order the strip colors along the LED strip (and reverse if configured).
//! 4. Map them onto the LEDs and smooth over time.
//!
//! Everything except `ColorExtractor::extract`'s clock and smoother is a pure function of the
//! pixel data, so it is unit-tested on synthetic frames.

use super::pipeline::Smoother;
use crate::config::SyncConfig;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

/// Sampler settings (a copy of the relevant `SyncConfig` fields).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeParams {
    /// Depth of the sampled strip, percent of the screen size (3..=40).
    pub depth_pct: u8,
    pub skip_black_bars: bool,
    /// Lines whose average brightness is below this count as a black bar.
    pub black_level: u8,
    pub reverse: bool,
    pub edge_number: u8,
}

impl EdgeParams {
    fn from_sync(s: &SyncConfig) -> Self {
        Self {
            depth_pct: s.edge_depth_pct,
            skip_black_bars: s.skip_black_bars,
            black_level: s.black_level,
            reverse: s.reverse,
            edge_number: s.edge_number,
        }
    }
}

/// Borrowed BGRA frame.
struct Frame<'a> {
    data: &'a [u8],
    pitch: usize,
    w: usize,
    h: usize,
}

impl Frame<'_> {
    /// (r, g, b) of one pixel; out-of-range reads give black instead of panicking.
    fn px(&self, x: usize, y: usize) -> (u32, u32, u32) {
        let o = y * self.pitch + x * 4;
        if x >= self.w || y >= self.h || o + 2 >= self.data.len() {
            return (0, 0, 0);
        }
        (self.data[o + 2] as u32, self.data[o + 1] as u32, self.data[o] as u32)
    }

    /// Average color of a rectangle (sampled on a coarse grid for speed).
    fn avg(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Rgb {
        let (x1, y1) = (x1.min(self.w), y1.min(self.h));
        if x0 >= x1 || y0 >= y1 {
            return Rgb { r: 0, g: 0, b: 0 };
        }
        let sx = ((x1 - x0) / 8).max(1);
        let sy = ((y1 - y0) / 8).max(1);
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        let mut y = y0;
        while y < y1 {
            let mut x = x0;
            while x < x1 {
                let (pr, pg, pb) = self.px(x, y);
                r += pr as u64;
                g += pg as u64;
                b += pb as u64;
                n += 1;
                x += sx;
            }
            y += sy;
        }
        match (r.checked_div(n), g.checked_div(n), b.checked_div(n)) {
            (Some(r), Some(g), Some(b)) => Rgb { r: r as u8, g: g as u8, b: b as u8 },
            _ => Rgb { r: 0, g: 0, b: 0 },
        }
    }

    fn luma(&self, x: usize, y: usize) -> u32 {
        let (r, g, b) = self.px(x, y);
        (r * 299 + g * 587 + b * 114) / 1000
    }

    fn row_luma(&self, y: usize) -> u32 {
        let step = (self.w / 64).max(1);
        let (mut sum, mut n) = (0u32, 0u32);
        let mut x = 0;
        while x < self.w {
            sum += self.luma(x, y);
            n += 1;
            x += step;
        }
        sum.checked_div(n).unwrap_or(0)
    }

    fn col_luma(&self, x: usize) -> u32 {
        let step = (self.h / 64).max(1);
        let (mut sum, mut n) = (0u32, 0u32);
        let mut y = 0;
        while y < self.h {
            sum += self.luma(x, y);
            n += 1;
            y += step;
        }
        sum.checked_div(n).unwrap_or(0)
    }
}

/// Largest share of the screen that may be treated as a black bar on one side. Real
/// letterboxing (even 2.39:1 on 16:9) stays well below this, so a dark scene is not mistaken
/// for a huge bar.
const MAX_BAR_PCT: usize = 20;

/// Black-bar thickness in lines on each side: (top, bottom, left, right).
fn detect_bars(f: &Frame, black_level: u8) -> (usize, usize, usize, usize) {
    let thr = black_level as u32;
    let cap_y = f.h * MAX_BAR_PCT / 100;
    let cap_x = f.w * MAX_BAR_PCT / 100;
    let count = |n: usize, cap: usize, luma: &dyn Fn(usize) -> u32| (0..n.min(cap)).take_while(|&i| luma(i) < thr).count();
    let top = count(f.h, cap_y, &|i| f.row_luma(i));
    let bottom = count(f.h, cap_y, &|i| f.row_luma(f.h - 1 - i));
    let left = count(f.w, cap_x, &|i| f.col_luma(i));
    let right = count(f.w, cap_x, &|i| f.col_luma(f.w - 1 - i));
    (top, bottom, left, right)
}

/// Strip colors in the order they run along the LED strip.
/// `bars` = (top, bottom, left, right) lines to skip before sampling.
fn edge_colors(f: &Frame, p: &EdgeParams, bars: (usize, usize, usize, usize)) -> Vec<Rgb> {
    let (mut t, mut b, mut l, mut r) = if p.skip_black_bars { bars } else { (0, 0, 0, 0) };
    // Degenerate result (bars eat the picture): ignore them.
    if t + b + 4 > f.h || l + r + 4 > f.w {
        (t, b, l, r) = (0, 0, 0, 0);
    }
    let (cw, ch) = (f.w - l - r, f.h - t - b); // content rectangle
    let depth_y = (f.h * p.depth_pct as usize / 100).clamp(1, (ch / 2).max(1));
    let depth_x = (f.w * p.depth_pct as usize / 100).clamp(1, (cw / 2).max(1));

    // Finer than the old 16x9 grid so 70+ LEDs each get their own color.
    let segs_x = if cw >= 64 { 32 } else { 16.min(cw.max(1)) };
    let segs_y = if ch >= 36 { 18 } else { 9.min(ch.max(1)) };

    let strip_h = |y0: usize, y1: usize| -> Vec<Rgb> {
        (0..segs_x)
            .map(|i| f.avg(l + cw * i / segs_x, y0, l + cw * (i + 1) / segs_x, y1))
            .collect()
    };
    let strip_v = |x0: usize, x1: usize| -> Vec<Rgb> {
        (0..segs_y)
            .map(|i| f.avg(x0, t + ch * i / segs_y, x1, t + ch * (i + 1) / segs_y))
            .collect()
    };

    let top = strip_h(t, t + depth_y);
    let bottom = strip_h(f.h - b - depth_y, f.h - b);
    let left = strip_v(l, l + depth_x);
    let right = strip_v(f.w - r - depth_x, f.w - r);

    let rev = |mut v: Vec<Rgb>| {
        v.reverse();
        v
    };
    // Wiring order: left (bottom -> top), top (left -> right), right (top -> bottom),
    // [bottom (right -> left)]. `reverse` runs the strip the other way round.
    let mut out = Vec::new();
    if p.reverse {
        out.extend(rev(right));
        out.extend(rev(top));
        out.extend(rev(left));
        if p.edge_number >= 4 {
            out.extend(bottom);
        }
    } else {
        out.extend(rev(left));
        out.extend(top);
        out.extend(right);
        if p.edge_number >= 4 {
            out.extend(rev(bottom));
        }
    }
    out
}

/// Nearest-neighbour map of the strip colors onto `lamp_count` LEDs.
fn map_to_leds(border_colors: &[Rgb], lamp_count: u32) -> Vec<Rgb> {
    if border_colors.is_empty() || lamp_count == 0 {
        return vec![Rgb { r: 0, g: 0, b: 0 }; lamp_count as usize];
    }
    let src_len = border_colors.len() as f32;
    let dst_len = lamp_count as f32;
    (0..lamp_count)
        .map(|i| {
            let src_idx = (i as f32 + 0.5) * src_len / dst_len;
            border_colors[(src_idx as usize).min(border_colors.len() - 1)]
        })
        .collect()
}

/// Color extractor: screen frame -> one smoothed raw RGB per LED.
/// Grading (saturation, gamma, knee, power limit) happens later, in `pipeline::grade`.
pub struct ColorExtractor {
    lamp_count: u32,
    params: EdgeParams,
    smoothing: bool,
    smoother: Smoother,
    last_extract: Option<Instant>,
    last_dims: (u32, u32),
    /// Black-bar thickness in use, stabilised over time so detection never flickers.
    bars: (usize, usize, usize, usize),
}

impl ColorExtractor {
    pub fn new(lamps_amount: u32, sync: &SyncConfig) -> Self {
        Self {
            lamp_count: lamps_amount,
            params: EdgeParams::from_sync(sync),
            smoothing: sync.smoothing,
            smoother: Smoother::new(sync.attack_ms, sync.decay_ms, sync.deadband),
            last_extract: None,
            last_dims: (0, 0),
            bars: (0, 0, 0, 0),
        }
    }

    /// Apply changed settings (hot reload).
    pub fn update_config(&mut self, lamp_count: u32, sync: &SyncConfig) {
        if self.lamp_count != lamp_count {
            self.lamp_count = lamp_count;
            self.smoother.reset();
        }
        self.params = EdgeParams::from_sync(sync);
        self.smoothing = sync.smoothing;
        self.smoother.attack_ms = sync.attack_ms;
        self.smoother.decay_ms = sync.decay_ms;
        self.smoother.deadband = sync.deadband;
    }

    /// Extract one color per LED from a BGRA frame.
    pub fn extract(&mut self, data: &[u8], pitch: u32, width: u32, height: u32) -> Vec<(u8, u8, u8)> {
        if self.last_dims != (width, height) {
            self.last_dims = (width, height);
            self.smoother.reset();
            self.bars = (0, 0, 0, 0);
            log::info!("sampling a {}x{} frame", width, height);
        }
        let frame = Frame { data, pitch: pitch as usize, w: width as usize, h: height as usize };

        if self.params.skip_black_bars {
            let seen = detect_bars(&frame, self.params.black_level);
            self.bars = stabilise(self.bars, seen);
        } else {
            self.bars = (0, 0, 0, 0);
        }

        let border = edge_colors(&frame, &self.params, self.bars);
        let mapped = map_to_leds(&border, self.lamp_count);
        let raw: Vec<[u8; 3]> = mapped.iter().map(|c| [c.r, c.g, c.b]).collect();

        let now = Instant::now();
        let dt_ms = self
            .last_extract
            .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
            .unwrap_or(33.0);
        self.last_extract = Some(now);

        let out = if self.smoothing { self.smoother.update(&raw, dt_ms) } else { raw };
        out.into_iter().map(|c| (c[0], c[1], c[2])).collect()
    }
}

/// New bars appear immediately; bars shrink by at most one line per frame, so a borderline
/// dark edge cannot make the sampled region jump back and forth.
fn stabilise(
    cur: (usize, usize, usize, usize),
    seen: (usize, usize, usize, usize),
) -> (usize, usize, usize, usize) {
    let step = |c: usize, s: usize| if s >= c { s } else { c - 1 };
    (step(cur.0, seen.0), step(cur.1, seen.1), step(cur.2, seen.2), step(cur.3, seen.3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a BGRA frame; `paint(x, y)` returns (r, g, b).
    fn frame(w: usize, h: usize, paint: impl Fn(usize, usize) -> (u8, u8, u8)) -> Vec<u8> {
        let mut d = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = paint(x, y);
                let o = (y * w + x) * 4;
                d[o] = b;
                d[o + 1] = g;
                d[o + 2] = r;
            }
        }
        d
    }

    fn params(depth: u8, skip: bool) -> EdgeParams {
        EdgeParams { depth_pct: depth, skip_black_bars: skip, black_level: 18, reverse: false, edge_number: 3 }
    }

    fn view(d: &[u8], w: usize, h: usize) -> Frame<'_> {
        Frame { data: d, pitch: w * 4, w, h }
    }

    const ORANGE: (u8, u8, u8) = (200, 120, 40);

    /// 160x90 frame with 8-line black bars top and bottom (about 9%: a thin YouTube letterbox).
    fn letterboxed() -> Vec<u8> {
        frame(160, 90, |_, y| if !(8..82).contains(&y) { (0, 0, 0) } else { ORANGE })
    }

    #[test]
    fn detects_letterbox_bars() {
        let d = letterboxed();
        let (t, b, l, r) = detect_bars(&view(&d, 160, 90), 18);
        assert_eq!((t, b, l, r), (8, 8, 0, 0));
    }

    #[test]
    fn detects_pillarbox_bars() {
        let d = frame(160, 90, |x, _| if !(12..148).contains(&x) { (0, 0, 0) } else { ORANGE });
        let (t, b, l, r) = detect_bars(&view(&d, 160, 90), 18);
        assert_eq!((t, b, l, r), (0, 0, 12, 12));
    }

    #[test]
    fn skipping_bars_gives_the_picture_color_at_the_top() {
        let d = letterboxed();
        let f = view(&d, 160, 90);
        let bars = detect_bars(&f, 18);
        let with = edge_colors(&f, &params(12, true), bars);
        // every strip color along the top edge is the picture color, not a black-diluted one
        let top_mid = with[18 + 5]; // left strip has 18 segments, then the top strip starts
        assert!((top_mid.r as i32 - 200).abs() <= 2 && (top_mid.g as i32 - 120).abs() <= 2, "{top_mid:?}");
    }

    #[test]
    fn without_skipping_the_black_bars_dim_the_edge() {
        let d = letterboxed();
        let f = view(&d, 160, 90);
        let without = edge_colors(&f, &params(12, false), (0, 0, 0, 0));
        let top_mid = without[18 + 5];
        assert!(top_mid.r < 100, "bars should dilute the average, got {top_mid:?}");
    }

    #[test]
    fn deeper_sampling_reaches_further_into_the_picture() {
        // black top half-bar of 10 lines, then a bright picture; skip is OFF here
        let d = frame(160, 90, |_, y| if y < 10 { (0, 0, 0) } else { (220, 220, 220) });
        let f = view(&d, 160, 90);
        let shallow = edge_colors(&f, &params(5, false), (0, 0, 0, 0))[18 + 5];
        let deep = edge_colors(&f, &params(30, false), (0, 0, 0, 0))[18 + 5];
        assert!(deep.r > shallow.r, "deeper strip includes more picture: {shallow:?} vs {deep:?}");
    }

    #[test]
    fn uniform_picture_is_uniform_at_any_depth() {
        let d = frame(160, 90, |_, _| (90, 140, 200));
        let f = view(&d, 160, 90);
        for depth in [3u8, 12, 25, 40] {
            for c in edge_colors(&f, &params(depth, true), detect_bars(&f, 18)) {
                assert_eq!(c, Rgb { r: 90, g: 140, b: 200 }, "depth {depth}");
            }
        }
    }

    #[test]
    fn a_dark_scene_never_creates_a_huge_bar_and_does_not_panic() {
        let d = frame(160, 90, |_, _| (0, 0, 0));
        let f = view(&d, 160, 90);
        let (t, b, l, r) = detect_bars(&f, 18);
        assert!(t <= 90 * MAX_BAR_PCT / 100 && b <= 90 * MAX_BAR_PCT / 100);
        assert!(l <= 160 * MAX_BAR_PCT / 100 && r <= 160 * MAX_BAR_PCT / 100);
        let colors = edge_colors(&f, &params(12, true), (t, b, l, r));
        assert!(colors.iter().all(|c| *c == Rgb { r: 0, g: 0, b: 0 }));
    }

    #[test]
    fn tiny_and_odd_frames_do_not_panic() {
        for (w, h) in [(1usize, 1usize), (2, 2), (4, 4), (5, 3), (16, 9), (3, 200)] {
            let d = frame(w, h, |x, y| ((x * 40) as u8, (y * 40) as u8, 90));
            let f = view(&d, w, h);
            let bars = detect_bars(&f, 18);
            let _ = edge_colors(&f, &params(40, true), bars);
            let _ = edge_colors(&f, &params(3, false), (0, 0, 0, 0));
        }
        // truncated buffer: reads past the end are black, not a crash
        let short = vec![255u8; 100];
        let f = Frame { data: &short, pitch: 640, w: 160, h: 90 };
        let _ = edge_colors(&f, &params(12, true), detect_bars(&f, 18));
    }

    #[test]
    fn bars_that_would_eat_the_picture_are_ignored() {
        let d = frame(160, 90, |_, _| (120, 120, 120));
        let f = view(&d, 160, 90);
        // absurd skip values fall back to the whole frame
        let c = edge_colors(&f, &params(12, true), (60, 60, 100, 100));
        assert!(c.iter().all(|c| *c == Rgb { r: 120, g: 120, b: 120 }));
    }

    #[test]
    fn strip_order_and_reverse() {
        // left edge red, right edge blue, top/bottom green
        let d = frame(160, 90, |x, y| {
            if x < 20 {
                (255, 0, 0)
            } else if x >= 140 {
                (0, 0, 255)
            } else if !(15..75).contains(&y) {
                (0, 255, 0)
            } else {
                (50, 50, 50)
            }
        });
        let f = view(&d, 160, 90);
        let fwd = edge_colors(&f, &params(12, false), (0, 0, 0, 0));
        assert!(fwd.first().unwrap().r > 200, "starts on the left edge");
        assert!(fwd.last().unwrap().b > 200, "ends on the right edge");
        let mut p = params(12, false);
        p.reverse = true;
        let rev = edge_colors(&f, &p, (0, 0, 0, 0));
        assert!(rev.first().unwrap().b > 200, "reversed: starts on the right edge");
        assert!(rev.last().unwrap().r > 200, "reversed: ends on the left edge");
        let mut p4 = params(12, false);
        p4.edge_number = 4;
        assert!(edge_colors(&f, &p4, (0, 0, 0, 0)).len() > fwd.len(), "4 edges adds the bottom strip");
    }

    #[test]
    fn bar_detection_is_stabilised_over_time() {
        // appears at once
        assert_eq!(stabilise((0, 0, 0, 0), (9, 9, 0, 0)), (9, 9, 0, 0));
        // shrinks one line per frame
        assert_eq!(stabilise((9, 9, 0, 0), (0, 0, 0, 0)), (8, 8, 0, 0));
        // grows at once even from a partial value
        assert_eq!(stabilise((3, 3, 0, 0), (9, 3, 0, 0)), (9, 3, 0, 0));
    }

    #[test]
    fn map_to_leds_covers_every_led_and_handles_empty_input() {
        let src = vec![Rgb { r: 1, g: 2, b: 3 }; 10];
        assert_eq!(map_to_leds(&src, 71).len(), 71);
        assert_eq!(map_to_leds(&[], 5), vec![Rgb { r: 0, g: 0, b: 0 }; 5]);
        assert!(map_to_leds(&src, 0).is_empty());
    }
}
