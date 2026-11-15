/// 화면 가장자리에서 LED 색상 추출 — SyncLight 호환 파이프라인
///
/// 처리 흐름:
/// 1. 화면을 cols×rows 그리드로 나눠 각 셀 평균 색상 계산
/// 2. 가장자리 셀 추출 (상/우/하/좌)
/// 3. LED 개수에 맞게 보간하여 매핑
/// 4. 방향 반전
/// 5. 시간축 스무딩 (최근 N 프레임 평균)
/// 6. 채도 부스트 (HSL 기반)
/// 7. 광량 압축 (R+G+B > 255 → 정규화)

use super::pipeline::Smoother;
use crate::config::SyncConfig;
use std::time::Instant;

/// 그리드 셀 색상
#[derive(Clone, Copy, Debug)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    fn is_black(&self, threshold: u8) -> bool {
        self.r < threshold && self.g < threshold && self.b < threshold
    }
}

/// 화면 비율에 따른 그리드 크기 결정 (SyncLight: getAspectRatio)
fn aspect_grid(width: u32, height: u32) -> (u32, u32) {
    let ratio = (width as f32 / height as f32 * 10.0).floor() / 10.0;
    let r_16_9 = (16.0_f32 / 9.0 * 10.0).floor() / 10.0;
    let r_16_10 = (16.0_f32 / 10.0 * 10.0).floor() / 10.0;
    let r_21_9 = (21.0_f32 / 9.0 * 10.0).floor() / 10.0;
    let r_4_3 = (4.0_f32 / 3.0 * 10.0).floor() / 10.0;

    if (ratio - r_16_9).abs() < 0.05 || (ratio - r_21_9).abs() < 0.05 {
        (16, 9)
    } else if (ratio - r_16_10).abs() < 0.05 {
        (8, 5)
    } else if (ratio - r_4_3).abs() < 0.05 {
        (4, 3)
    } else {
        (16, 9) // 기본값
    }
}

/// BGRA 프레임을 cols×rows 그리드로 나눠 각 셀 평균 색상 계산
fn capture_grid(data: &[u8], pitch: u32, width: u32, height: u32, cols: u32, rows: u32) -> Vec<Rgb> {
    let cell_w = width / cols;
    let cell_h = height / rows;
    let step = 4u32.max(cell_w / 8).max(cell_h / 8); // 다운샘플링

    let mut grid = Vec::with_capacity((cols * rows) as usize);

    for row in 0..rows {
        for col in 0..cols {
            let x0 = col * cell_w;
            let y0 = row * cell_h;
            let x1 = ((col + 1) * cell_w).min(width);
            let y1 = ((row + 1) * cell_h).min(height);

            let mut r_sum = 0u64;
            let mut g_sum = 0u64;
            let mut b_sum = 0u64;
            let mut count = 0u64;

            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let offset = (y * pitch + x * 4) as usize;
                    if offset + 2 < data.len() {
                        b_sum += data[offset] as u64;
                        g_sum += data[offset + 1] as u64;
                        r_sum += data[offset + 2] as u64;
                        count += 1;
                    }
                    x += step;
                }
                y += step;
            }

            if count > 0 {
                grid.push(Rgb {
                    r: (r_sum / count) as u8,
                    g: (g_sum / count) as u8,
                    b: (b_sum / count) as u8,
                });
            } else {
                grid.push(Rgb { r: 0, g: 0, b: 0 });
            }
        }
    }

    grid
}

/// 그리드에서 가장자리 색상 추출 (SyncLight: backSideColors)
/// 검은 가장자리면 한 줄 안쪽 사용
fn extract_border_colors(
    grid: &[Rgb],
    cols: u32,
    rows: u32,
    edge_number: u8,
    reverse: bool,
) -> Vec<Rgb> {
    let black_threshold = 10u8;

    // 상단 행 (1차: row 0, 2차: row 1)
    let top1: Vec<Rgb> = (0..cols).map(|c| grid[c as usize]).collect();
    let top2: Vec<Rgb> = if rows > 1 {
        (0..cols).map(|c| grid[(cols + c) as usize]).collect()
    } else {
        top1.clone()
    };
    let top = if top1.iter().all(|c| c.is_black(black_threshold)) { &top2 } else { &top1 };

    // 하단 행
    let bot1: Vec<Rgb> = (0..cols).map(|c| grid[((rows - 1) * cols + c) as usize]).collect();
    let bot2: Vec<Rgb> = if rows > 2 {
        (0..cols).map(|c| grid[((rows - 2) * cols + c) as usize]).collect()
    } else {
        bot1.clone()
    };
    let bottom = if bot1.iter().all(|c| c.is_black(black_threshold)) { &bot2 } else { &bot1 };

    // 좌측 열
    let left: Vec<Rgb> = (0..rows).map(|r| grid[(r * cols) as usize]).collect();
    // 우측 열
    let right: Vec<Rgb> = (0..rows).map(|r| grid[(r * cols + cols - 1) as usize]).collect();

    // SyncLight 순서: left(reversed) → top → right → bottom(if 4 edges)
    if reverse {
        let mut colors = Vec::new();
        let mut r_rev: Vec<Rgb> = right.clone();
        r_rev.reverse();
        colors.extend_from_slice(&r_rev);

        let mut t_rev: Vec<Rgb> = top.clone();
        t_rev.reverse();
        colors.extend_from_slice(&t_rev);

        let mut l_rev: Vec<Rgb> = left.clone();
        l_rev.reverse();
        colors.extend_from_slice(&l_rev);

        if edge_number >= 4 {
            colors.extend_from_slice(bottom);
        }
        colors
    } else {
        let mut colors = Vec::new();
        let mut l_rev = left.clone();
        l_rev.reverse();
        colors.extend_from_slice(&l_rev);
        colors.extend_from_slice(top);
        colors.extend_from_slice(&right);
        if edge_number >= 4 {
            let mut b_rev: Vec<Rgb> = bottom.clone();
            b_rev.reverse();
            colors.extend_from_slice(&b_rev);
        }
        colors
    }
}

/// 가장자리 색상을 LED 개수에 맞게 보간 매핑
fn map_to_leds(border_colors: &[Rgb], lamp_count: u32) -> Vec<Rgb> {
    if border_colors.is_empty() || lamp_count == 0 {
        return vec![Rgb { r: 0, g: 0, b: 0 }; lamp_count as usize];
    }

    let src_len = border_colors.len() as f32;
    let dst_len = lamp_count as f32;

    (0..lamp_count)
        .map(|i| {
            let src_idx = (i as f32 + 0.5) * src_len / dst_len;
            let idx = (src_idx as usize).min(border_colors.len() - 1);
            border_colors[idx]
        })
        .collect()
}

/// Color extractor: screen frame -> one smoothed raw RGB per LED.
/// Grading (saturation, gamma, knee, power limit) happens later, in `pipeline::grade`.
pub struct ColorExtractor {
    cols: u32,
    rows: u32,
    lamp_count: u32,
    reverse: bool,
    edge_number: u8,
    smoothing: bool,
    smoother: Smoother,
    last_extract: Option<Instant>,
}

impl ColorExtractor {
    pub fn new(lamps_amount: u32, sync: &SyncConfig) -> Self {
        Self {
            cols: 0,
            rows: 0,
            lamp_count: lamps_amount,
            reverse: sync.reverse,
            edge_number: sync.edge_number,
            smoothing: sync.smoothing,
            smoother: Smoother::new(sync.attack_ms, sync.decay_ms, sync.deadband),
            last_extract: None,
        }
    }

    /// Apply changed settings (hot reload).
    pub fn update_config(&mut self, lamp_count: u32, sync: &SyncConfig) {
        if self.lamp_count != lamp_count {
            self.lamp_count = lamp_count;
            self.cols = 0; // force grid recompute
            self.smoother.reset();
        }
        self.reverse = sync.reverse;
        self.edge_number = sync.edge_number;
        self.smoothing = sync.smoothing;
        self.smoother.attack_ms = sync.attack_ms;
        self.smoother.decay_ms = sync.decay_ms;
        self.smoother.deadband = sync.deadband;
    }

    /// Extract one color per LED from a BGRA frame.
    pub fn extract(&mut self, data: &[u8], pitch: u32, width: u32, height: u32) -> Vec<(u8, u8, u8)> {
        if self.cols == 0 {
            let (c, r) = aspect_grid(width, height);
            self.cols = c;
            self.rows = r;
            self.smoother.reset();
            log::info!("grid {}x{} ({}x{})", c, r, width, height);
        }

        let grid = capture_grid(data, pitch, width, height, self.cols, self.rows);
        let border = extract_border_colors(&grid, self.cols, self.rows, self.edge_number, self.reverse);
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
