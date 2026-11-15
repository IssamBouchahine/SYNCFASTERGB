//! Detects a full-screen application (a game, a video) on a given monitor, so the engine can
//! lower its own frame rate while it is focused and leave the GPU and CPU to that application.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Shell::SHQueryUserNotificationState;
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect};

/// Rectangles are `(left, top, right, bottom)` in virtual-screen coordinates.
pub type Rect4 = (i32, i32, i32, i32);

/// Does a window rectangle fully cover the monitor rectangle?
pub fn rect_covers(win: Rect4, mon: Rect4) -> bool {
    win.0 <= mon.0 && win.1 <= mon.1 && win.2 >= mon.2 && win.3 >= mon.3
}

/// The desktop itself (wallpaper/shell) covers the whole monitor but is not an application.
pub fn is_shell_class(class: &str) -> bool {
    matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// Windows' own full-screen detector (the one Focus Assist uses) reports these states for a
/// real full-screen app: BUSY = 2, RUNNING_D3D_FULL_SCREEN = 3, PRESENTATION_MODE = 4. A
/// merely maximized window is NOT one of them (it reports ACCEPTS_NOTIFICATIONS = 5), which
/// is why a plain "covers the monitor" test is not enough: maximized windows overhang the
/// monitor edges and would look like games.
pub fn is_fullscreen_state(raw: i32) -> bool {
    matches!(raw, 2..=4)
}

fn system_says_fullscreen() -> bool {
    // SAFETY: a plain shell query with no arguments; failure maps to `false`.
    unsafe { SHQueryUserNotificationState().map(|s| is_fullscreen_state(s.0)).unwrap_or(false) }
}

/// Is a real full-screen application focused on `monitor`?
pub fn fullscreen_app_covering(monitor: Rect4) -> bool {
    if !system_says_fullscreen() {
        return false;
    }
    // SAFETY: plain Win32 queries with valid out-pointers; every failure maps to `false`.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut buf) as usize;
        let class = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        !is_shell_class(&class) && rect_covers((r.left, r.top, r.right, r.bottom), monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect4 = (0, 0, 2560, 1440);

    #[test]
    fn covering_and_non_covering_windows() {
        assert!(rect_covers((0, 0, 2560, 1440), MON));
        assert!(rect_covers((-8, -8, 2568, 1448), MON)); // maximized windows overhang slightly
        assert!(!rect_covers((100, 100, 2000, 1200), MON));
        assert!(!rect_covers((0, 0, 2560, 1400), MON)); // taskbar visible: not full screen
    }

    #[test]
    fn works_on_a_secondary_monitor_offset() {
        let mon = (2560, 0, 4480, 1080);
        assert!(rect_covers((2560, 0, 4480, 1080), mon));
        assert!(!rect_covers((0, 0, 2560, 1440), mon));
    }

    #[test]
    fn only_real_fullscreen_states_count() {
        assert!(is_fullscreen_state(2)); // BUSY (borderless full screen)
        assert!(is_fullscreen_state(3)); // RUNNING_D3D_FULL_SCREEN (exclusive)
        assert!(is_fullscreen_state(4)); // PRESENTATION_MODE
        assert!(!is_fullscreen_state(1)); // NOT_PRESENT
        assert!(!is_fullscreen_state(5)); // ACCEPTS_NOTIFICATIONS: normal use, maximized windows
        assert!(!is_fullscreen_state(6));
        assert!(!is_fullscreen_state(7));
        assert!(!is_fullscreen_state(0));
    }

    #[test]
    fn desktop_shell_is_not_a_game() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("WorkerW"));
        assert!(!is_shell_class("UnrealWindow"));
    }
}
