//! Single-instance helpers: named mutexes (one tray app, one settings window per Windows
//! session) and bringing an already-open window to the front.

use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
};

/// Mutex held by the running tray app (the "engine").
pub const APP_MUTEX: &str = "Local\\SYNCFASTERGB_App";

/// Mutex held by the open settings window.
pub const SETTINGS_MUTEX: &str = "Local\\SYNCFASTERGB_Settings";

/// True if some process currently holds the named mutex.
pub fn is_held(name: &str) -> bool {
    acquire(name).is_none()
}

/// Holds a named mutex for as long as it lives.
pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Take the named mutex. `None` means another process already holds it.
pub fn acquire(name: &str) -> Option<InstanceGuard> {
    let w = wide(name);
    unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(w.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard(handle))
    }
}

/// Like `acquire`, but keeps trying for `wait` (used after a self-restart, while the old
/// process is still shutting down).
pub fn acquire_with_retry(name: &str, wait: Duration) -> Option<InstanceGuard> {
    let start = Instant::now();
    loop {
        if let Some(g) = acquire(name) {
            return Some(g);
        }
        if start.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Restore (if minimized) and focus the top-level window with exactly this title.
pub fn focus_window(title: &str) -> bool {
    let w = wide(title);
    unsafe {
        let hwnd = match FindWindowW(PCWSTR::null(), PCWSTR(w.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);
        true
    }
}
