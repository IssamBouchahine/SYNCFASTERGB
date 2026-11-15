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
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
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
            let map = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, SIZE as u32, PCWSTR(w.as_ptr())).ok()?;
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
            let n = (u32::from_le(std::ptr::read_volatile(base.add(4) as *const u32)) as usize).min(MAX_LEDS);
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
    fn reader_without_writer_gets_none() {
        assert!(LedView::open_named("Local\\SYNCFASTERGB_Leds_does_not_exist").is_none());
    }
}
