//! Screen edge -> LED colors.
//!
//! 1. Optionally measure black bars (letterbox / pillarbox) on each side of the picture.
//! 2. Sample a strip of adjustable depth along each edge, just inside the bars.
//! 3. Order the strip colors along the LED strip (and reverse if configured).
//! 4. Map them onto the LEDs and smooth over time.
//!
//! Everything except `ColorExtractor::extract`'s clock and smoother is a pure function of the
//! pixel data, so it is unit-tested on synthetic frames.

use super::pipeline::{Led, Smoother};
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
    pub fn from_sync(s: &SyncConfig) -> Self {
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
        (
            self.data[o + 2] as u32,
            self.data[o + 1] as u32,
            self.data[o] as u32,
        )
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
            (Some(r), Some(g), Some(b)) => Rgb {
                r: r as u8,
                g: g as u8,
                b: b as u8,
            },
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

/// Thinnest bar (in lines of the small picture) worth skipping.
const MIN_BAR: usize = 2;

/// Black-bar thickness in lines on each side: (top, bottom, left, right).
fn detect_bars(f: &Frame, black_level: u8) -> (usize, usize, usize, usize) {
    let thr = black_level as u32;
    let cap_y = f.h * MAX_BAR_PCT / 100;
    let cap_x = f.w * MAX_BAR_PCT / 100;
    let count = |n: usize, cap: usize, luma: &dyn Fn(usize) -> u32| {
        (0..n.min(cap)).take_while(|&i| luma(i) < thr).count()
    };
    let top = count(f.h, cap_y, &|i| f.row_luma(i));
    let bottom = count(f.h, cap_y, &|i| f.row_luma(f.h - 1 - i));
    let left = count(f.w, cap_x, &|i| f.col_luma(i));
    let right = count(f.w, cap_x, &|i| f.col_luma(f.w - 1 - i));
    // A real letterbox / pillarbox has bars on BOTH opposite sides. A dark region on one side
    // only (a night sky, a dark floor) is picture, not a bar, and must not be skipped. When the
    // two sides differ slightly the smaller bar is used for both, the safe choice.
    let pair = |a: usize, b: usize| {
        if a >= MIN_BAR && b >= MIN_BAR {
            (a.min(b), a.min(b))
        } else {
            (0, 0)
        }
    };
    let (top, bottom) = pair(top, bottom);
    let (left, right) = pair(left, right);
    (top, bottom, left, right)
}

// ───────────────────────── automatic mode ─────────────────────────

/// A bar is never brighter than this (average line brightness), so a mid-grey border or a
/// bright page is picture, not a bar.
const BAR_MAX_LUMA: u32 = 40;
/// Lines of one bar may differ by at most this much (noise, slight gradient).
const BAR_UNIFORMITY: u32 = 14;
/// The picture must be at least this much brighter than the bar, on two lines in a row.
const BAR_STEP: u32 = 10;

/// Thickness of the bar on one side, found by its sharp edge instead of a brightness threshold:
/// a run of uniformly dark lines followed by an abrupt step up. Gradual fades are not bars.
fn bar_extent_edge(n: usize, cap: usize, luma: &dyn Fn(usize) -> u32) -> usize {
    let m = (cap + 2).min(n);
    if m < MIN_BAR + 2 {
        return 0;
    }
    let l: Vec<u32> = (0..m).map(luma).collect();
    for k in MIN_BAR..=cap.min(m - 2) {
        let run = &l[..k];
        let mean = run.iter().sum::<u32>() / k as u32;
        let (lo, hi) = (
            *run.iter().min().unwrap_or(&0),
            *run.iter().max().unwrap_or(&0),
        );
        if mean > BAR_MAX_LUMA || hi - lo > BAR_UNIFORMITY {
            return 0; // the dark run stopped being uniform before any sharp edge: no bar
        }
        if l[k] >= mean + BAR_STEP && l[k + 1] >= mean + BAR_STEP {
            return k;
        }
    }
    0
}

/// Bars on all four sides by edge detection, with the same both-sides rule as the manual mode.
fn detect_bars_edge(f: &Frame) -> (usize, usize, usize, usize) {
    let cap_y = f.h * MAX_BAR_PCT / 100;
    let cap_x = f.w * MAX_BAR_PCT / 100;
    let top = bar_extent_edge(f.h, cap_y, &|i| f.row_luma(i));
    let bottom = bar_extent_edge(f.h, cap_y, &|i| f.row_luma(f.h - 1 - i));
    let left = bar_extent_edge(f.w, cap_x, &|i| f.col_luma(i));
    let right = bar_extent_edge(f.w, cap_x, &|i| f.col_luma(f.w - 1 - i));
    let pair = |a: usize, b: usize| {
        if a >= MIN_BAR && b >= MIN_BAR {
            (a.min(b), a.min(b))
        } else {
            (0, 0)
        }
    };
    let (top, bottom) = pair(top, bottom);
    let (left, right) = pair(left, right);
    (top, bottom, left, right)
}

/// How busy the picture edge is: mean absolute difference between neighbouring strip segments
/// (0 = perfectly smooth, large for game HUDs, text and web pages).
fn edge_busyness(s: &EdgeSamples) -> f32 {
    let strip = |v: &Vec<[u8; 3]>| -> (f32, usize) {
        let d: f32 = v
            .windows(2)
            .map(|w| {
                ((w[0][0] as f32 - w[1][0] as f32).abs()
                    + (w[0][1] as f32 - w[1][1] as f32).abs()
                    + (w[0][2] as f32 - w[1][2] as f32).abs())
                    / 3.0
            })
            .sum();
        (d, v.len().saturating_sub(1))
    };
    let (mut sum, mut n) = (0.0, 0usize);
    for v in [&s.top, &s.bottom, &s.left, &s.right] {
        let (d, c) = strip(v);
        sum += d;
        n += c;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Automatic depth in percent: shallow and precise for smooth video, deeper and steadier for
/// busy edges. Always between 10% and 20%.
pub fn auto_depth_target(busyness: f32) -> f32 {
    if !busyness.is_finite() {
        return 10.0;
    }
    10.0 + 10.0 * ((busyness - 6.0) / 30.0).clamp(0.0, 1.0)
}

/// Move the automatic depth toward its target with a 1.5 s time constant, so it never jumps.
fn smooth_depth(cur: f32, target: f32, dt_ms: f32) -> f32 {
    let dt = dt_ms.clamp(0.0, 500.0);
    cur + (target - cur) * (1.0 - (-dt / 1500.0).exp())
}

/// Where the sampled strips are, in frame pixels. Used by the sampler AND by the settings
/// window's illustration, so what is drawn is exactly what is sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Black bars actually skipped (top, bottom, left, right), after sanity checks.
    pub t: usize,
    pub b: usize,
    pub l: usize,
    pub r: usize,
    /// Size of the content rectangle inside the bars.
    pub cw: usize,
    pub ch: usize,
    /// Strip thickness.
    pub depth_x: usize,
    pub depth_y: usize,
    /// Number of color segments along a horizontal / vertical strip.
    pub segs_x: usize,
    pub segs_y: usize,
}

/// `bars` = (top, bottom, left, right) lines to skip before sampling.
pub fn geometry(
    w: usize,
    h: usize,
    p: &EdgeParams,
    bars: (usize, usize, usize, usize),
) -> Geometry {
    let (mut t, mut b, mut l, mut r) = if p.skip_black_bars {
        bars
    } else {
        (0, 0, 0, 0)
    };
    // Degenerate result (bars eat the picture): ignore them.
    if t + b + 4 > h || l + r + 4 > w {
        (t, b, l, r) = (0, 0, 0, 0);
    }
    let (cw, ch) = (w - l - r, h - t - b);
    let depth_y = (h * p.depth_pct as usize / 100).clamp(1, (ch / 2).max(1));
    let depth_x = (w * p.depth_pct as usize / 100).clamp(1, (cw / 2).max(1));
    // Finer than the old 16x9 grid so 70+ LEDs each get their own color.
    let segs_x = if cw >= 64 { 32 } else { 16.min(cw.max(1)) };
    let segs_y = if ch >= 36 { 18 } else { 9.min(ch.max(1)) };
    Geometry {
        t,
        b,
        l,
        r,
        cw,
        ch,
        depth_x,
        depth_y,
        segs_x,
        segs_y,
    }
}

/// The sampled colors of each strip in natural order (top/bottom left to right,
/// left/right top to bottom).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSamples {
    pub top: Vec<[u8; 3]>,
    pub bottom: Vec<[u8; 3]>,
    pub left: Vec<[u8; 3]>,
    pub right: Vec<[u8; 3]>,
}

/// Sample the four strips of a BGRA frame.
pub fn sample_edges(
    data: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
    p: &EdgeParams,
    bars: (usize, usize, usize, usize),
) -> (Geometry, EdgeSamples) {
    let f = Frame { data, pitch, w, h };
    let g = geometry(w, h, p, bars);
    let rgb = |c: Rgb| [c.r, c.g, c.b];
    let strip_h = |y0: usize, y1: usize| -> Vec<[u8; 3]> {
        (0..g.segs_x)
            .map(|i| {
                rgb(f.avg(
                    g.l + g.cw * i / g.segs_x,
                    y0,
                    g.l + g.cw * (i + 1) / g.segs_x,
                    y1,
                ))
            })
            .collect()
    };
    let strip_v = |x0: usize, x1: usize| -> Vec<[u8; 3]> {
        (0..g.segs_y)
            .map(|i| {
                rgb(f.avg(
                    x0,
                    g.t + g.ch * i / g.segs_y,
                    x1,
                    g.t + g.ch * (i + 1) / g.segs_y,
                ))
            })
            .collect()
    };
    let samples = EdgeSamples {
        top: strip_h(g.t, g.t + g.depth_y),
        bottom: strip_h(h - g.b - g.depth_y, h - g.b),
        left: strip_v(g.l, g.l + g.depth_x),
        right: strip_v(w - g.r - g.depth_x, w - g.r),
    };
    (g, samples)
}

/// Strip colors in the order they run along the LED strip.
fn edge_colors(f: &Frame, p: &EdgeParams, bars: (usize, usize, usize, usize)) -> Vec<Rgb> {
    let (_, s) = sample_edges(f.data, f.pitch, f.w, f.h, p, bars);
    let to_rgb = |v: Vec<[u8; 3]>| -> Vec<Rgb> {
        v.into_iter()
            .map(|c| Rgb {
                r: c[0],
                g: c[1],
                b: c[2],
            })
            .collect()
    };
    let rev = |mut v: Vec<Rgb>| {
        v.reverse();
        v
    };
    let (top, bottom, left, right) = (
        to_rgb(s.top),
        to_rgb(s.bottom),
        to_rgb(s.left),
        to_rgb(s.right),
    );
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
    /// Automatic mode: the engine judges bars and depth itself, in real time.
    auto: bool,
    auto_depth: f32,
}

impl ColorExtractor {
    /// Black bars currently being skipped (top, bottom, left, right), in frame lines.
    pub fn bars(&self) -> (usize, usize, usize, usize) {
        self.bars
    }

    /// Depth in percent currently in use (automatic or manual).
    pub fn effective_depth(&self) -> u8 {
        if self.auto {
            self.auto_depth.round().clamp(3.0, 40.0) as u8
        } else {
            self.params.depth_pct
        }
    }
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
            auto: sync.detect_auto,
            auto_depth: 12.0,
        }
    }

    /// Apply changed settings (hot reload).
    pub fn update_config(&mut self, lamp_count: u32, sync: &SyncConfig) {
        if self.lamp_count != lamp_count {
            self.lamp_count = lamp_count;
            self.smoother.reset();
        }
        self.params = EdgeParams::from_sync(sync);
        self.auto = sync.detect_auto;
        self.smoothing = sync.smoothing;
        self.smoother.attack_ms = sync.attack_ms;
        self.smoother.decay_ms = sync.decay_ms;
        self.smoother.deadband = sync.deadband;
    }

    /// Extract one color per LED from a BGRA frame.
    pub fn extract(&mut self, data: &[u8], pitch: u32, width: u32, height: u32) -> Vec<Led> {
        if self.last_dims != (width, height) {
            self.last_dims = (width, height);
            self.smoother.reset();
            self.bars = (0, 0, 0, 0);
            log::info!("sampling a {}x{} frame", width, height);
        }
        let frame = Frame {
            data,
            pitch: pitch as usize,
            w: width as usize,
            h: height as usize,
        };

        let now = Instant::now();
        let dt_ms = self
            .last_extract
            .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
            .unwrap_or(33.0);
        self.last_extract = Some(now);

        // Bars: found by their edge in automatic mode, by the black level in manual mode.
        let skip = self.auto || self.params.skip_black_bars;
        if skip {
            let seen = if self.auto {
                detect_bars_edge(&frame)
            } else {
                detect_bars(&frame, self.params.black_level)
            };
            self.bars = stabilise(self.bars, seen);
        } else {
            self.bars = (0, 0, 0, 0);
        }
        let mut p = self.params;
        p.skip_black_bars = skip;
        if self.auto {
            // measure how busy the edge is at a fixed reference depth, then ease toward the target
            let mut probe = p;
            probe.depth_pct = 10;
            let (_, s) = sample_edges(
                data,
                pitch as usize,
                width as usize,
                height as usize,
                &probe,
                self.bars,
            );
            self.auto_depth =
                smooth_depth(self.auto_depth, auto_depth_target(edge_busyness(&s)), dt_ms);
            p.depth_pct = self.auto_depth.round().clamp(3.0, 40.0) as u8;
        }

        let border = edge_colors(&frame, &p, self.bars);
        let mapped = map_to_leds(&border, self.lamp_count);
        let raw: Vec<[u8; 3]> = mapped.iter().map(|c| [c.r, c.g, c.b]).collect();

        if self.smoothing {
            self.smoother.update(&raw, dt_ms)
        } else {
            raw.iter()
                .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
                .collect()
        }
    }
}

/// New bars appear immediately; bars shrink by at most one line per frame, so a borderline
/// dark edge cannot make the sampled region jump back and forth.
fn stabilise(
    cur: (usize, usize, usize, usize),
    seen: (usize, usize, usize, usize),
) -> (usize, usize, usize, usize) {
    let step = |c: usize, s: usize| if s >= c { s } else { c - 1 };
    (
        step(cur.0, seen.0),
        step(cur.1, seen.1),
        step(cur.2, seen.2),
        step(cur.3, seen.3),
    )
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
        EdgeParams {
            depth_pct: depth,
            skip_black_bars: skip,
            black_level: 18,
            reverse: false,
            edge_number: 3,
        }
    }

    fn view(d: &[u8], w: usize, h: usize) -> Frame<'_> {
        Frame {
            data: d,
            pitch: w * 4,
            w,
            h,
        }
    }

    const ORANGE: (u8, u8, u8) = (200, 120, 40);

    /// 160x90 frame with 8-line black bars top and bottom (about 9%: a thin YouTube letterbox).
    fn letterboxed() -> Vec<u8> {
        frame(160, 90, |_, y| {
            if !(8..82).contains(&y) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        })
    }

    #[test]
    fn detects_letterbox_bars() {
        let d = letterboxed();
        let (t, b, l, r) = detect_bars(&view(&d, 160, 90), 18);
        assert_eq!((t, b, l, r), (8, 8, 0, 0));
    }

    #[test]
    fn detects_pillarbox_bars() {
        let d = frame(160, 90, |x, _| {
            if !(12..148).contains(&x) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        });
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
        assert!(
            (top_mid.r as i32 - 200).abs() <= 2 && (top_mid.g as i32 - 120).abs() <= 2,
            "{top_mid:?}"
        );
    }

    #[test]
    fn without_skipping_the_black_bars_dim_the_edge() {
        let d = letterboxed();
        let f = view(&d, 160, 90);
        let without = edge_colors(&f, &params(12, false), (0, 0, 0, 0));
        let top_mid = without[18 + 5];
        assert!(
            top_mid.r < 100,
            "bars should dilute the average, got {top_mid:?}"
        );
    }

    #[test]
    fn deeper_sampling_reaches_further_into_the_picture() {
        // black top half-bar of 10 lines, then a bright picture; skip is OFF here
        let d = frame(
            160,
            90,
            |_, y| if y < 10 { (0, 0, 0) } else { (220, 220, 220) },
        );
        let f = view(&d, 160, 90);
        let shallow = edge_colors(&f, &params(5, false), (0, 0, 0, 0))[18 + 5];
        let deep = edge_colors(&f, &params(30, false), (0, 0, 0, 0))[18 + 5];
        assert!(
            deep.r > shallow.r,
            "deeper strip includes more picture: {shallow:?} vs {deep:?}"
        );
    }

    #[test]
    fn uniform_picture_is_uniform_at_any_depth() {
        let d = frame(160, 90, |_, _| (90, 140, 200));
        let f = view(&d, 160, 90);
        for depth in [3u8, 12, 25, 40] {
            for c in edge_colors(&f, &params(depth, true), detect_bars(&f, 18)) {
                assert_eq!(
                    c,
                    Rgb {
                        r: 90,
                        g: 140,
                        b: 200
                    },
                    "depth {depth}"
                );
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
        let f = Frame {
            data: &short,
            pitch: 640,
            w: 160,
            h: 90,
        };
        let _ = edge_colors(&f, &params(12, true), detect_bars(&f, 18));
    }

    #[test]
    fn bars_that_would_eat_the_picture_are_ignored() {
        let d = frame(160, 90, |_, _| (120, 120, 120));
        let f = view(&d, 160, 90);
        // absurd skip values fall back to the whole frame
        let c = edge_colors(&f, &params(12, true), (60, 60, 100, 100));
        assert!(c.iter().all(|c| *c
            == Rgb {
                r: 120,
                g: 120,
                b: 120
            }));
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
        assert!(
            rev.first().unwrap().b > 200,
            "reversed: starts on the right edge"
        );
        assert!(
            rev.last().unwrap().r > 200,
            "reversed: ends on the left edge"
        );
        let mut p4 = params(12, false);
        p4.edge_number = 4;
        assert!(
            edge_colors(&f, &p4, (0, 0, 0, 0)).len() > fwd.len(),
            "4 edges adds the bottom strip"
        );
    }

    #[test]
    fn geometry_matches_what_is_sampled_and_respects_bars() {
        let d = letterboxed();
        let f = view(&d, 160, 90);
        let bars = detect_bars(&f, 18);
        let (g, s) = sample_edges(f.data, f.pitch, f.w, f.h, &params(12, true), bars);
        assert_eq!((g.t, g.b, g.l, g.r), (8, 8, 0, 0));
        assert_eq!(g.ch, 90 - 16);
        assert_eq!(g.depth_y, 90 * 12 / 100);
        assert_eq!(s.top.len(), g.segs_x);
        assert_eq!(s.left.len(), g.segs_y);
        // the geometry never extends past the frame
        assert!(g.t + g.depth_y <= 90 && g.l + g.depth_x <= 160);
        // skipping off: the strips start at the very edge
        let (g2, _) = sample_edges(f.data, f.pitch, f.w, f.h, &params(12, false), bars);
        assert_eq!((g2.t, g2.b, g2.l, g2.r), (0, 0, 0, 0));
    }

    #[test]
    fn one_sided_dark_regions_are_picture_not_bars() {
        // black sky on top, lit ground below: only the top is dark, so nothing is skipped
        let d = frame(
            160,
            90,
            |_, y| if y < 27 { (2, 2, 2) } else { (70, 80, 60) },
        );
        let f = view(&d, 160, 90);
        assert_eq!(detect_bars(&f, 26), (0, 0, 0, 0));
        let (t, b, l, r) = detect_bars(&f, 26);
        let top = edge_colors(&f, &params(15, true), (t, b, l, r))[18 + 5];
        assert!(
            top.r < 10,
            "the LEDs above a dark sky stay dark, got {top:?}"
        );
    }

    #[test]
    fn slightly_uneven_bars_use_the_smaller_one() {
        let d = frame(160, 90, |_, y| {
            if !(12..78).contains(&y) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        });
        let f = view(&d, 160, 90);
        assert_eq!(detect_bars(&f, 26), (12, 12, 0, 0));
        let d = frame(160, 90, |_, y| {
            if !(12..80).contains(&y) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        }); // 12 top, 10 bottom
        assert_eq!(detect_bars(&view(&d, 160, 90), 26), (10, 10, 0, 0));
        // a 1-line dark edge is not worth skipping
        let d = frame(
            160,
            90,
            |_, y| if y == 0 || y == 89 { (0, 0, 0) } else { ORANGE },
        );
        assert_eq!(detect_bars(&view(&d, 160, 90), 26), (0, 0, 0, 0));
    }

    #[test]
    fn default_black_level_catches_lifted_blacks_and_dark_page_chrome() {
        let level = 26u8;
        // studio-range black (16) with noise, and #181818 page chrome (24)
        let noisy = frame(160, 90, |x, y| {
            if !(12..78).contains(&y) {
                let v = (17 + ((x * 7919 + y * 104729) % 7) as i32 - 3) as u8;
                (v, v, v)
            } else {
                ORANGE
            }
        });
        assert_eq!(detect_bars(&view(&noisy, 160, 90), level), (12, 12, 0, 0));
        let chrome = frame(160, 90, |_, y| {
            if !(12..78).contains(&y) {
                (24, 24, 24)
            } else {
                ORANGE
            }
        });
        assert_eq!(detect_bars(&view(&chrome, 160, 90), level), (12, 12, 0, 0));
        // a mid-grey border is NOT a bar
        let grey = frame(160, 90, |_, y| {
            if !(12..78).contains(&y) {
                (60, 60, 60)
            } else {
                ORANGE
            }
        });
        assert_eq!(detect_bars(&view(&grey, 160, 90), level), (0, 0, 0, 0));
    }

    // ── automatic mode ──

    fn bars_edge(d: &[u8], w: usize, h: usize) -> (usize, usize, usize, usize) {
        detect_bars_edge(&view(d, w, h))
    }

    #[test]
    fn auto_finds_bars_of_any_dark_shade_without_a_threshold() {
        for bar in [0, 6, 17, 24, 32] {
            let d = frame(160, 90, |_, y| {
                if !(12..78).contains(&y) {
                    (bar, bar, bar)
                } else {
                    ORANGE
                }
            });
            assert_eq!(bars_edge(&d, 160, 90), (12, 12, 0, 0), "bar value {bar}");
        }
        let noisy = frame(160, 90, |x, y| {
            if !(12..78).contains(&y) {
                let v = (17 + ((x * 7919 + y * 104729) % 7) as i32 - 3) as u8;
                (v, v, v)
            } else {
                ORANGE
            }
        });
        assert_eq!(bars_edge(&noisy, 160, 90), (12, 12, 0, 0));
        let pillar = frame(160, 90, |x, _| {
            if !(20..140).contains(&x) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        });
        assert_eq!(bars_edge(&pillar, 160, 90), (0, 0, 20, 20));
        let boxed = frame(160, 90, |x, y| {
            if !(12..78).contains(&y) || !(20..140).contains(&x) {
                (0, 0, 0)
            } else {
                ORANGE
            }
        });
        assert_eq!(bars_edge(&boxed, 160, 90), (12, 12, 20, 20));
    }

    #[test]
    fn auto_does_not_mistake_picture_for_bars() {
        // mid-grey border, bright border, plain picture
        for v in [60u8, 120, 220] {
            let d = frame(160, 90, |_, y| {
                if !(12..78).contains(&y) {
                    (v, v, v)
                } else {
                    ORANGE
                }
            });
            assert_eq!(bars_edge(&d, 160, 90), (0, 0, 0, 0), "border value {v}");
        }
        assert_eq!(
            bars_edge(&frame(160, 90, |_, _| ORANGE), 160, 90),
            (0, 0, 0, 0)
        );
        // a gradual fade to black is a dark scene, not a bar
        let fade = frame(160, 90, |_, y| {
            let v = (y as u32 * 3).min(255) as u8;
            (v, v, v)
        });
        assert_eq!(bars_edge(&fade, 160, 90), (0, 0, 0, 0));
        // dark sky on one side only
        let sky = frame(
            160,
            90,
            |_, y| if y < 27 { (2, 2, 2) } else { (70, 80, 60) },
        );
        assert_eq!(bars_edge(&sky, 160, 90), (0, 0, 0, 0));
        // an all-black frame has no edge to find
        assert_eq!(
            bars_edge(&frame(160, 90, |_, _| (0, 0, 0)), 160, 90),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn auto_depth_is_shallow_for_smooth_video_and_deep_for_busy_edges() {
        let p = params(10, true);
        let busy_of = |d: &[u8]| {
            let f = view(d, 160, 90);
            let (_, s) = sample_edges(f.data, f.pitch, f.w, f.h, &p, (0, 0, 0, 0));
            edge_busyness(&s)
        };
        let smooth = frame(160, 90, |_, y| (40 + (y * 2) as u8, 90, 200 - y as u8)); // vertical gradient video
        let flat = frame(160, 90, |_, _| (90, 140, 200));
        // game HUD / text / web page: strongly varying colours along the edge
        let busy = frame(160, 90, |x, y| {
            if ((x / 10) + (y / 10)) % 2 == 0 {
                (240, 240, 240)
            } else {
                (20, 30, 120)
            }
        });
        // fine texture that the LED segments average out: busier than video, not as busy as HUD blocks
        let fine = frame(160, 90, |x, y| {
            if ((x / 5) + (y / 5)) % 2 == 0 {
                (240, 240, 240)
            } else {
                (20, 30, 120)
            }
        });
        let (ds, df, db) = (
            auto_depth_target(busy_of(&smooth)),
            auto_depth_target(busy_of(&flat)),
            auto_depth_target(busy_of(&busy)),
        );
        let dfine = auto_depth_target(busy_of(&fine));
        assert!(
            ds <= 11.0 && df <= 11.0,
            "smooth edges stay shallow: {ds} {df}"
        );
        assert!(db >= 19.0, "busy edges go deep: {db}");
        assert!(
            dfine > ds + 5.0 && dfine <= db,
            "fine texture lands in between: {dfine}"
        );
        for d in [ds, df, db] {
            assert!((10.0..=20.0).contains(&d));
        }
        assert_eq!(auto_depth_target(f32::NAN), 10.0);
    }

    #[test]
    fn auto_depth_moves_smoothly_and_converges() {
        let mut d = 10.0f32;
        let mut prev = d;
        for _ in 0..30 {
            d = smooth_depth(d, 20.0, 16.0);
            assert!(
                d >= prev && d - prev < 0.2,
                "one 16 ms frame moves the depth only a little"
            );
            prev = d;
        }
        for _ in 0..600 {
            d = smooth_depth(d, 20.0, 16.0);
        }
        assert!((d - 20.0).abs() < 0.05);
        assert_eq!(smooth_depth(12.0, 20.0, f32::NAN.max(0.0)), 12.0);
        assert!(
            smooth_depth(12.0, 20.0, 1.0e9) < 20.0 + 1e-3,
            "huge dt is clamped"
        );
    }

    #[test]
    fn auto_extractor_skips_bars_without_any_settings() {
        use crate::config::SyncConfig;
        let sync = SyncConfig {
            detect_auto: true,
            skip_black_bars: false,
            ..SyncConfig::default()
        }; // skip flag off: auto ignores it
        let mut ex = ColorExtractor::new(71, &sync);
        let d = letterboxed();
        let mut out = Vec::new();
        for _ in 0..5 {
            out = ex.extract(&d, 160 * 4, 160, 90);
        }
        assert_eq!(ex.bars(), (8, 8, 0, 0));
        assert!(
            out.iter().any(|c| c[0] > 100.0),
            "LEDs carry the picture colour, not black"
        );
        assert!((10..=20).contains(&ex.effective_depth()));
        // manual mode with the sliders keeps working
        let manual = SyncConfig {
            detect_auto: false,
            edge_depth_pct: 25,
            skip_black_bars: false,
            ..SyncConfig::default()
        };
        let mut ex = ColorExtractor::new(71, &manual);
        ex.extract(&d, 160 * 4, 160, 90);
        assert_eq!((ex.bars(), ex.effective_depth()), ((0, 0, 0, 0), 25));
    }

    /// Developer report used to choose the default depth and black level. Run with:
    /// `cargo test tuning_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tuning_report() {
        let pic = |y: f32| -> (u8, u8, u8) {
            (
                40 + (170.0 * y) as u8,
                90 + (30.0 * y) as u8,
                200 - (160.0 * y) as u8,
            )
        };
        // deterministic pseudo-noise in -3..=3
        let noise = |x: usize, y: usize| -> i32 { ((x * 7919 + y * 104729) % 7) as i32 - 3 };
        let (w, h) = (160usize, 90usize);
        let grey = |v: i32| -> (u8, u8, u8) {
            let v = v.clamp(0, 255) as u8;
            (v, v, v)
        };

        // letterboxed 2.39:1 (12 rows of bar at each side), bars of a given value (+ noise)
        let letterbox = |bar: i32, noisy: bool| {
            frame(w, h, move |x, y| {
                if !(12..78).contains(&y) {
                    grey(bar + if noisy { noise(x, y) } else { 0 })
                } else {
                    pic((y - 12) as f32 / 66.0)
                }
            })
        };
        let a = letterbox(0, false); // pure black bars
        let b = letterbox(17, true); // lifted blacks (studio-range black is 16) with noise
        let c = letterbox(24, false); // dark-grey page chrome (#181818)
        let d = frame(w, h, |x, y| {
            if !(20..140).contains(&x) {
                (0, 0, 0)
            } else {
                pic(y as f32 / 90.0)
            }
        }); // 4:3 pillarbox
        let e = frame(w, h, |x, y| {
            // window-boxed: bars on all four sides
            if !(12..78).contains(&y) || !(20..140).contains(&x) {
                (0, 0, 0)
            } else {
                pic((y - 12) as f32 / 66.0)
            }
        });
        let f_dark = frame(w, h, |x, y| {
            // dark scene, no bars, bright subject starting ~20% in
            if (18..72).contains(&y) && (40..120).contains(&x) {
                (200, 180, 150)
            } else {
                grey(6)
            }
        });

        let avg = |v: &Vec<[u8; 3]>| -> (f32, f32, f32) {
            let n = v.len() as f32;
            (
                v.iter().map(|c| c[0] as f32).sum::<f32>() / n,
                v.iter().map(|c| c[1] as f32).sum::<f32>() / n,
                v.iter().map(|c| c[2] as f32).sum::<f32>() / n,
            )
        };
        let err = |got: (f32, f32, f32), want: (u8, u8, u8)| -> f32 {
            ((got.0 - want.0 as f32).abs()
                + (got.1 - want.1 as f32).abs()
                + (got.2 - want.2 as f32).abs())
                / 3.0
        };
        let top_err = |d: &Vec<u8>, p: &EdgeParams| -> f32 {
            let f = view(d, w, h);
            let bars = detect_bars(&f, p.black_level);
            let (_, s) = sample_edges(f.data, f.pitch, f.w, f.h, p, bars);
            err(avg(&s.top), pic(0.0))
        };

        println!("
error = average colour error of the top LEDs vs the picture's own edge (0 = perfect, lower is better)");
        println!(
            "leak  = brightness a dark scene's top LEDs gain from the picture inside (0 = none)"
        );
        println!("flick = average LED change when a bright HUD block moves 4 px along the top edge (lower = steadier)
");
        println!("depth level | black  lifted  grey24 | pillar  boxed | night | leak(dark) | flick(game)");
        // black sky on top with a lit ground below: the LEDs above the sky should stay dark
        let night = frame(w, h, |_, y| if y < 27 { (2, 2, 2) } else { (70, 80, 60) });
        for depth in [10u8, 12, 15, 18] {
            for level in [18u8, 22, 26, 30] {
                let p = EdgeParams {
                    depth_pct: depth,
                    skip_black_bars: true,
                    black_level: level,
                    reverse: false,
                    edge_number: 3,
                };
                // pillar / boxed errors use the left strip vs the picture's mid colour
                let side_err = |d: &Vec<u8>| -> f32 {
                    let f = view(d, w, h);
                    let bars = detect_bars(&f, level);
                    let (_, s) = sample_edges(f.data, f.pitch, f.w, f.h, &p, bars);
                    err(avg(&s.left), pic(0.5))
                };
                // dark-scene leak: top LED brightness minus the true edge (6)
                let leak = {
                    let f = view(&f_dark, w, h);
                    let bars = detect_bars(&f, level);
                    let (_, s) = sample_edges(f.data, f.pitch, f.w, f.h, &p, bars);
                    avg(&s.top).0 - 6.0
                };
                // game HUD flicker: white block 12x6 sliding along the top edge of a colourful scene
                let flick = {
                    let scene = |bx: usize| {
                        frame(w, h, move |x, y| {
                            if (2..8).contains(&y) && x >= bx && x < bx + 12 {
                                (255, 255, 255)
                            } else {
                                pic(y as f32 / 90.0)
                            }
                        })
                    };
                    let mut total = 0.0f32;
                    let mut n = 0.0f32;
                    let mut prev: Option<Vec<[u8; 3]>> = None;
                    for bx in (0..140).step_by(4) {
                        let d = scene(bx);
                        let f = view(&d, w, h);
                        let (_, s) =
                            sample_edges(f.data, f.pitch, f.w, f.h, &p, detect_bars(&f, level));
                        if let Some(pv) = &prev {
                            let m = s
                                .top
                                .iter()
                                .zip(pv)
                                .map(|(a, b)| (a[0] as i32 - b[0] as i32).abs())
                                .max()
                                .unwrap_or(0) as f32;
                            total += m;
                            n += 1.0;
                        }
                        prev = Some(s.top);
                    }
                    total / n
                };
                let night_leak = {
                    let f = view(&night, w, h);
                    let (_, s) =
                        sample_edges(f.data, f.pitch, f.w, f.h, &p, detect_bars(&f, level));
                    avg(&s.top).0 - 2.0
                };
                println!(
                    "{:>5} {:>5} | {:>5.1} {:>7.1} {:>7.1} | {:>6.1} {:>6.1} | {:>5.1} | {:>10.1} | {:>10.1}",
                    depth, level, top_err(&a, &p), top_err(&b, &p), top_err(&c, &p), side_err(&d), side_err(&e), night_leak, leak, flick
                );
            }
        }
        // baseline: the old behaviour (no bar skipping, depth 12)
        let old = EdgeParams {
            depth_pct: 12,
            skip_black_bars: false,
            black_level: 18,
            reverse: false,
            edge_number: 3,
        };
        println!(
            "
old behaviour (skipping off, depth 12): black {:.1}  lifted {:.1}  grey24 {:.1}",
            top_err(&a, &old),
            top_err(&b, &old),
            top_err(&c, &old)
        );
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
