//! Live LED preview channel: the engine publishes the colors it sends to the strip into a
//! small named shared-memory block; the settings window reads it to draw a live preview.
//!
//! Scope: the mapping lives in the per-session `Local\` namespace with the default security
//! descriptor, so only processes of the same user can open it. It holds at most 255 RGB
//! triplets (the strip colors, not screen contents). Nothing touches the disk or network.
//!
//! This module contains the only shared-memory `unsafe` in the program.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
    FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// Shared-memory name used by the app.
pub const LED_SHARE_NAME: &str = "Local\\SYNCFASTERGB_Leds";

/// Layout: [0..4] sequence (u32 LE), [4..8] LED count (u32 LE), [8..] RGB triplets.
const HEADER: usize = 8;
const MAX_LEDS: usize = 255;
const SIZE: usize = HEADER + MAX_LEDS * 3;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Writer side (engine).
pub struct LedShare {
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    seq: u32,
}

// SAFETY: the raw view pointer is only used from the thread that owns the LedShare.
unsafe impl Send for LedShare {}

impl LedShare {
    pub fn create() -> Option<Self> {
        Self::create_named(LED_SHARE_NAME)
    }

    pub fn create_named(name: &str) -> Option<Self> {
        let w = wide(name);
        // SAFETY: plain Win32 calls with valid arguments; failures are handled below.
        unsafe {
            let map = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                SIZE as u32,
                PCWSTR(w.as_ptr()),
            )
            .ok()?;
            let view = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Self { map, view, seq: 0 })
        }
    }

    /// Publish the colors currently sent to the strip (extra LEDs beyond 255 are ignored).
    pub fn publish(&mut self, leds: &[[u8; 3]]) {
        let n = leds.len().min(MAX_LEDS);
        // SAFETY: `view` points to SIZE writable bytes for the lifetime of `self`, and
        // HEADER + n * 3 <= SIZE because n <= MAX_LEDS.
        unsafe {
            let base = self.view.Value as *mut u8;
            std::ptr::copy_nonoverlapping(leds.as_ptr() as *const u8, base.add(HEADER), n * 3);
            std::ptr::write_volatile(base.add(4) as *mut u32, (n as u32).to_le());
            self.seq = self.seq.wrapping_add(1);
            std::ptr::write_volatile(base as *mut u32, self.seq.to_le());
        }
    }
}

impl Drop for LedShare {
    fn drop(&mut self) {
        // SAFETY: handles/views were created in `create_named` and are released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.map);
        }
    }
}

/// Reader side (settings window).
pub struct LedView {
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl LedView {
    pub fn open() -> Option<Self> {
        Self::open_named(LED_SHARE_NAME)
    }

    pub fn open_named(name: &str) -> Option<Self> {
        let w = wide(name);
        // SAFETY: plain Win32 calls with valid arguments; failures are handled below.
        unsafe {
            let map = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(w.as_ptr())).ok()?;
            let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Self { map, view })
        }
    }

    /// Latest published frame: `(sequence, colors)`. The sequence changes on every publish.
    /// A torn read can at worst show one mixed frame; the count is bounds-checked.
    pub fn read(&self) -> Option<(u32, Vec<[u8; 3]>)> {
        // SAFETY: `view` points to SIZE readable bytes for the lifetime of `self`; the count
        // read from shared memory is clamped to MAX_LEDS before it is used as a length.
        unsafe {
            let base = self.view.Value as *const u8;
            let seq = u32::from_le(std::ptr::read_volatile(base as *const u32));
            let n = (u32::from_le(std::ptr::read_volatile(base.add(4) as *const u32)) as usize)
                .min(MAX_LEDS);
            if n == 0 {
                return None;
            }
            let mut out = vec![[0u8; 3]; n];
            std::ptr::copy_nonoverlapping(base.add(HEADER), out.as_mut_ptr() as *mut u8, n * 3);
            Some((seq, out))
        }
    }
}

impl Drop for LedView {
    fn drop(&mut self) {
        // SAFETY: released once, created in `open_named`.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.map);
        }
    }
}

/// Shared-memory name for the live (small) screen picture used by the settings window.
pub const FRAME_SHARE_NAME: &str = "Local\\SYNCFASTERGB_Frame";

/// Largest picture shared (the GPU path already delivers about 160x90).
const MAX_FRAME_W: usize = 192;
const MAX_FRAME_H: usize = 192;
/// Layout: [0..4] seq, [4..8] width, [8..12] height, [12..28] bars (top, bottom, left, right),
/// [28..32] depth in percent, [32..] BGRA pixels.
const FRAME_HEADER: usize = 32;
const FRAME_SIZE: usize = FRAME_HEADER + MAX_FRAME_W * MAX_FRAME_H * 4;

/// Reduce a BGRA frame to at most MAX_FRAME_W x MAX_FRAME_H by nearest-neighbour sampling,
/// dropping row padding. Returns (width, height, step, pixels).
pub fn shrink_bgra(
    data: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
) -> (usize, usize, usize, Vec<u8>) {
    let step = w.div_ceil(MAX_FRAME_W).max(h.div_ceil(MAX_FRAME_H)).max(1);
    let (ow, oh) = ((w / step).max(1), (h / step).max(1));
    let mut out = vec![255u8; ow * oh * 4];
    for y in 0..oh {
        for x in 0..ow {
            let o = (y * step) * pitch + (x * step) * 4;
            let d = (y * ow + x) * 4;
            if o + 3 < data.len() {
                out[d..d + 4].copy_from_slice(&data[o..o + 4]);
            }
        }
    }
    (ow, oh, step, out)
}

/// Writer side for the live picture (engine).
pub struct FrameShare {
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    seq: u32,
}

// SAFETY: the raw view pointer is only used from the thread that owns the FrameShare.
unsafe impl Send for FrameShare {}

impl FrameShare {
    pub fn create() -> Option<Self> {
        Self::create_named(FRAME_SHARE_NAME)
    }

    pub fn create_named(name: &str) -> Option<Self> {
        let w = wide(name);
        // SAFETY: plain Win32 calls with valid arguments; failures are handled below.
        unsafe {
            let map = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                FRAME_SIZE as u32,
                PCWSTR(w.as_ptr()),
            )
            .ok()?;
            let view = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, FRAME_SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Self { map, view, seq: 0 })
        }
    }

    /// Publish a BGRA frame with the black bars currently skipped (top, bottom, left, right).
    pub fn publish_bgra(
        &mut self,
        data: &[u8],
        pitch: usize,
        w: usize,
        h: usize,
        bars: (usize, usize, usize, usize),
        depth_pct: u8,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let (ow, oh, step, pixels) = shrink_bgra(data, pitch, w, h);
        let header = [
            ow as u32,
            oh as u32,
            (bars.0 / step) as u32,
            (bars.1 / step) as u32,
            (bars.2 / step) as u32,
            (bars.3 / step) as u32,
            depth_pct as u32,
        ];
        // SAFETY: `view` points to FRAME_SIZE writable bytes for the lifetime of `self`;
        // ow <= MAX_FRAME_W and oh <= MAX_FRAME_H, so FRAME_HEADER + ow*oh*4 <= FRAME_SIZE.
        unsafe {
            let base = self.view.Value as *mut u8;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), base.add(FRAME_HEADER), pixels.len());
            for (i, v) in header.iter().enumerate() {
                std::ptr::write_volatile(base.add(4 + i * 4) as *mut u32, v.to_le());
            }
            self.seq = self.seq.wrapping_add(1);
            std::ptr::write_volatile(base as *mut u32, self.seq.to_le());
        }
    }
}

impl Drop for FrameShare {
    fn drop(&mut self) {
        // SAFETY: handles/views were created in `create_named` and are released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.map);
        }
    }
}

/// One picture read from the engine.
#[derive(Clone, Debug)]
pub struct FrameSnapshot {
    pub seq: u32,
    pub w: usize,
    pub h: usize,
    /// (top, bottom, left, right) black bars the engine is skipping, in picture lines.
    pub bars: (usize, usize, usize, usize),
    /// Detection depth in percent the engine is using (automatic or manual).
    pub depth_pct: u8,
    /// BGRA, `w * h * 4` bytes.
    pub bgra: Vec<u8>,
}

/// Reader side for the live picture (settings window).
pub struct FrameView {
    map: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl FrameView {
    pub fn open() -> Option<Self> {
        Self::open_named(FRAME_SHARE_NAME)
    }

    pub fn open_named(name: &str) -> Option<Self> {
        let w = wide(name);
        // SAFETY: plain Win32 calls with valid arguments; failures are handled below.
        unsafe {
            let map = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(w.as_ptr())).ok()?;
            let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, FRAME_SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Self { map, view })
        }
    }

    /// Latest picture, or `None` if nothing valid is published. Every header value read from
    /// shared memory is validated before it is used as a size.
    pub fn read(&self) -> Option<FrameSnapshot> {
        // SAFETY: `view` points to FRAME_SIZE readable bytes for the lifetime of `self`; sizes
        // are clamped/validated below before they are used for the copy length.
        unsafe {
            let base = self.view.Value as *const u8;
            let rd = |off: usize| {
                u32::from_le(std::ptr::read_volatile(base.add(off) as *const u32)) as usize
            };
            let (seq, w, h) = (rd(0) as u32, rd(4), rd(8));
            if w == 0 || h == 0 || w > MAX_FRAME_W || h > MAX_FRAME_H {
                return None;
            }
            let bars = (rd(12).min(h), rd(16).min(h), rd(20).min(w), rd(24).min(w));
            let depth_pct = if rd(28) == 0 {
                15
            } else {
                rd(28).clamp(3, 40) as u8
            };
            let mut bgra = vec![0u8; w * h * 4];
            std::ptr::copy_nonoverlapping(base.add(FRAME_HEADER), bgra.as_mut_ptr(), bgra.len());
            Some(FrameSnapshot {
                seq,
                w,
                h,
                bars,
                depth_pct,
                bgra,
            })
        }
    }
}

impl Drop for FrameView {
    fn drop(&mut self) {
        // SAFETY: released once, created in `open_named`.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_sequence() {
        let name = "Local\\SYNCFASTERGB_Leds_test_roundtrip";
        let mut w = LedShare::create_named(name).expect("create");
        let r = LedView::open_named(name).expect("open");
        assert!(r.read().is_none(), "nothing published yet");

        let a = vec![[1, 2, 3], [4, 5, 6], [250, 251, 252]];
        w.publish(&a);
        let (s1, got) = r.read().unwrap();
        assert_eq!(got, a);

        w.publish(&[[9, 9, 9]]);
        let (s2, got2) = r.read().unwrap();
        assert_eq!(got2, vec![[9, 9, 9]]);
        assert_ne!(s1, s2);
    }

    #[test]
    fn oversized_input_is_truncated_not_overflowed() {
        let name = "Local\\SYNCFASTERGB_Leds_test_oversize";
        let mut w = LedShare::create_named(name).unwrap();
        let r = LedView::open_named(name).unwrap();
        w.publish(&vec![[7, 7, 7]; 1000]);
        assert_eq!(r.read().unwrap().1.len(), MAX_LEDS);
    }

    #[test]
    fn frame_round_trip_with_bars() {
        let name = "Local\\SYNCFASTERGB_Frame_test_roundtrip";
        let mut w = FrameShare::create_named(name).unwrap();
        let r = FrameView::open_named(name).unwrap();
        assert!(r.read().is_none(), "nothing published yet");
        // 4x2 BGRA frame, distinct pixels
        let px: Vec<u8> = (0..4 * 2 * 4).map(|i| i as u8).collect();
        w.publish_bgra(&px, 4 * 4, 4, 2, (1, 0, 2, 0), 17);
        let s = r.read().unwrap();
        assert_eq!((s.w, s.h, s.bars, s.depth_pct), (4, 2, (1, 0, 2, 0), 17));
        assert_eq!(s.bgra, px);
        let seq1 = s.seq;
        w.publish_bgra(&px, 16, 4, 2, (0, 0, 0, 0), 12);
        assert_ne!(r.read().unwrap().seq, seq1);
    }

    #[test]
    fn big_frames_are_shrunk_and_padding_is_dropped() {
        // 400x200 with row padding: reduced to fit 192x192, bars scale with it
        let (w, h, pitch) = (400usize, 200usize, 400 * 4 + 64);
        let data = vec![7u8; pitch * h];
        let (ow, oh, step, px) = shrink_bgra(&data, pitch, w, h);
        assert_eq!(step, 3);
        assert_eq!((ow, oh), (133, 66));
        assert_eq!(px.len(), ow * oh * 4);
        assert!(px.iter().all(|&b| b == 7));
        // small frames pass through unchanged
        let small = shrink_bgra(&[1, 2, 3, 4, 5, 6, 7, 8], 8, 2, 1);
        assert_eq!((small.0, small.1, small.2), (2, 1, 1));
        assert_eq!(small.3, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        // a truncated buffer must not panic
        let _ = shrink_bgra(&[0u8; 10], 1600, 400, 200);
    }

    #[test]
    fn frame_reader_rejects_a_hostile_header() {
        let name = "Local\\SYNCFASTERGB_Frame_test_hostile";
        let mut w = FrameShare::create_named(name).unwrap();
        let r = FrameView::open_named(name).unwrap();
        w.publish_bgra(&[1, 2, 3, 4], 4, 1, 1, (0, 0, 0, 0), 12);
        assert!(r.read().is_some());
        // corrupt the shared header from the writer's side: absurd width
        // SAFETY: test-only write into the writer's own mapping header.
        unsafe {
            std::ptr::write_volatile(
                (w.view.Value as *mut u8).add(4) as *mut u32,
                1_000_000u32.to_le(),
            );
        }
        assert!(
            r.read().is_none(),
            "oversized dimensions are refused, not trusted"
        );
    }

    #[test]
    fn reader_without_writer_gets_none() {
        assert!(LedView::open_named("Local\\SYNCFASTERGB_Leds_does_not_exist").is_none());
    }
}
