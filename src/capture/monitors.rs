//! Monitor enumeration across ALL DXGI adapters (hybrid-GPU laptops expose the
//! internal panel on the iGPU and external ports on the dGPU).

use windows::core::PCWSTR;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Index across all adapters, in enumeration order (what `capture.monitor` means).
    pub global_index: u32,
    pub adapter_index: u32,
    pub output_index: u32,
    pub adapter_name: String,
    /// GDI device name, e.g. `\\.\DISPLAY2`.
    pub device_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Only attached outputs can be duplicated.
    pub attached: bool,
    /// EDID-based hardware id (e.g. `AOC3201`); stable across ports and index changes.
    pub monitor_id: String,
}

impl MonitorInfo {
    /// Text matched by `capture.monitor_name`.
    pub fn match_text(&self) -> String {
        format!("{} {} {}", self.adapter_name, self.device_name, self.monitor_id).to_lowercase()
    }
}

/// Pick a monitor: a non-empty `name` (case-insensitive substring) wins over `index`.
/// Unattached outputs are never selected by name.
pub fn select<'a>(monitors: &'a [MonitorInfo], index: u32, name: &str) -> Option<&'a MonitorInfo> {
    let name = name.trim().to_lowercase();
    if !name.is_empty() {
        return monitors.iter().find(|m| m.attached && m.match_text().contains(&name));
    }
    monitors.iter().find(|m| m.global_index == index)
}

/// Hardware id of the monitor attached to a GDI device name (`MONITOR\\<ID>\\{...}`).
fn monitor_hw_id(device_name: &[u16; 32]) -> String {
    unsafe {
        let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if EnumDisplayDevicesW(PCWSTR(device_name.as_ptr()), 0, &mut dd, 0).as_bool() {
            return parse_hw_id(&wide_to_string(&dd.DeviceID));
        }
    }
    String::new()
}

fn parse_hw_id(device_id: &str) -> String {
    let mut parts = device_id.split('\\');
    match (parts.next(), parts.next()) {
        (Some(kind), Some(id)) if kind.eq_ignore_ascii_case("MONITOR") => id.to_string(),
        _ => String::new(),
    }
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn enumerate() -> Result<Vec<MonitorInfo>, Box<dyn std::error::Error>> {
    let mut list = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let adesc = adapter.GetDesc1()?;
            let software = (adesc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0;
            if !software {
                let adapter_name = wide_to_string(&adesc.Description);
                let mut o = 0u32;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    let d = output.GetDesc()?;
                    let r = d.DesktopCoordinates;
                    list.push(MonitorInfo {
                        global_index: list.len() as u32,
                        adapter_index: a,
                        output_index: o,
                        adapter_name: adapter_name.clone(),
                        device_name: wide_to_string(&d.DeviceName),
                        x: r.left,
                        y: r.top,
                        width: (r.right - r.left).max(0) as u32,
                        height: (r.bottom - r.top).max(0) as u32,
                        attached: d.AttachedToDesktop.as_bool(),
                        monitor_id: monitor_hw_id(&d.DeviceName),
                    });
                    o += 1;
                }
            }
            a += 1;
        }
    }
    Ok(list)
}

/// `--list-monitors` output.
pub fn print_list() {
    match enumerate() {
        Ok(list) if list.is_empty() => println!("No monitors found."),
        Ok(list) => {
            println!("Monitors (use `monitor = <index>` or `monitor_name = \"<text>\"` in config.toml):\n");
            for m in &list {
                println!(
                    "  [{}] {}  |  {}  |  {}x{} at ({},{}){}",
                    m.global_index,
                    m.adapter_name,
                    m.device_name,
                    m.width,
                    m.height,
                    m.x,
                    m.y,
                    if m.attached { "" } else { "  (not attached)" }
                );
            }
        }
        Err(e) => println!("Enumeration failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(i: u32, adapter: &str, dev: &str, attached: bool) -> MonitorInfo {
        MonitorInfo {
            global_index: i,
            adapter_index: 0,
            output_index: 0,
            adapter_name: adapter.into(),
            device_name: dev.into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            attached,
            monitor_id: String::new(),
        }
    }

    fn sample() -> Vec<MonitorInfo> {
        vec![
            mon(0, "Intel(R) UHD Graphics", r"\\.\DISPLAY1", true),
            mon(1, "NVIDIA GeForce RTX 4060 Laptop GPU", r"\\.\DISPLAY2", true),
        ]
    }

    #[test]
    fn selects_by_global_index() {
        let s = sample();
        assert_eq!(select(&s, 1, "").unwrap().adapter_name, "NVIDIA GeForce RTX 4060 Laptop GPU");
        assert!(select(&s, 5, "").is_none());
    }

    #[test]
    fn name_overrides_index_case_insensitive() {
        let s = sample();
        assert_eq!(select(&s, 0, "nvidia").unwrap().global_index, 1);
        assert_eq!(select(&s, 1, "DISPLAY1").unwrap().global_index, 0);
        assert!(select(&s, 0, "amd").is_none());
    }

    #[test]
    fn unattached_not_selected_by_name() {
        let mut s = sample();
        s[1].attached = false;
        assert!(select(&s, 0, "nvidia").is_none());
    }

    #[test]
    fn hardware_id_parsing() {
        assert_eq!(parse_hw_id(r"MONITOR\AOC3201\{4d36e96e-e325-11ce-bfc1-08002be10318}\0001"), "AOC3201");
        assert_eq!(parse_hw_id(r"PCI\VEN_10DE"), "");
        assert_eq!(parse_hw_id(""), "");
    }

    #[test]
    fn selects_by_hardware_id() {
        let mut s = sample();
        s[0].monitor_id = "LEN40A1".into();
        s[1].monitor_id = "AOC3201".into();
        assert_eq!(select(&s, 0, "aoc3201").unwrap().global_index, 1);
    }

    #[test]
    fn wide_string_stops_at_nul() {
        let w: Vec<u16> = "ab\0cd".encode_utf16().collect();
        assert_eq!(wide_to_string(&w), "ab");
    }
}
