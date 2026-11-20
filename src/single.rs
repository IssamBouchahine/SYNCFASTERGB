//! Single-instance helpers: named mutexes (one tray app, one settings window per Windows
//! session) and bringing an already-open window to the front.

use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, IsIconic, PostMessageW, SetForegroundWindow, ShowWindow,
    SW_RESTORE, SW_SHOW, WM_CLOSE,
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

/// Close the top-level window with exactly this title, as its close button would, and end its
/// process if it has not gone within `grace` (a window that cannot respond must not stay behind
/// in the taskbar). Returns whether such a window was found.
pub fn close_window(title: &str, grace: Duration) -> bool {
    let w = wide(title);
    unsafe {
        let hwnd = match FindWindowW(PCWSTR::null(), PCWSTR(w.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        // Never touch our own process
        let process = if pid != 0 && pid != std::process::id() {
            OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid).ok()
        } else {
            None
        };
        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        if let Some(p) = process {
            if WaitForSingleObject(p, grace.as_millis().min(u32::MAX as u128) as u32)
                != WAIT_OBJECT_0
            {
                let _ = TerminateProcess(p, 0);
            }
            let _ = CloseHandle(p);
        }
        true
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
