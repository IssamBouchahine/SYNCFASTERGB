use hidapi::{HidApi, HidDevice};
use std::time::Duration;

use super::protocol;

/// LED strip (USB HID)
pub const VENDOR_ID: u16 = 0x1A86;
pub const PRODUCT_ID: u16 = 0xFE07;

/// Wait after a write that expects a reply (ms)
const WRITE_DELAY_MS: u64 = 200;

/// Minimum gap between chunks (HID reports), in microseconds.
/// The vendor app (Electron) had a natural pause between reports because of JavaScript
/// event-loop overhead, but back-to-back writes from Rust can outrun the receive buffer of the
/// strip MCU and make the firmware stop responding until the USB cable is replugged.
/// A high-resolution timer is used for the wait (thread::sleep is too coarse on Windows).
const INTER_CHUNK_GAP_US: u64 = 500;

/// High-resolution waitable timer (Windows 10 1803+): waits without burning CPU.
/// Falls back to the old spin-wait if the timer cannot be created or armed.
struct GapTimer(windows::Win32::Foundation::HANDLE);

impl GapTimer {
    fn new() -> Option<Self> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Threading::CreateWaitableTimerExW;
        const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002;
        const SYNCHRONIZE_AND_TIMER_MODIFY: u32 = 0x0010_0000 | 0x0002;
        unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                SYNCHRONIZE_AND_TIMER_MODIFY,
            )
            .ok()
            .map(Self)
        }
    }

    fn wait_us(&self, us: u64) -> bool {
        use windows::Win32::System::Threading::{SetWaitableTimer, WaitForSingleObject};
        let due: i64 = -((us as i64) * 10); // negative = relative, 100 ns units
        unsafe {
            if SetWaitableTimer(self.0, &due, 0, None, None, false).is_err() {
                return false;
            }
            WaitForSingleObject(self.0, 100);
        }
        true
    }
}

impl Drop for GapTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

thread_local! {
    static GAP_TIMER: Option<GapTimer> = GapTimer::new();
}

fn inter_chunk_pause() {
    let slept = GAP_TIMER.with(|t| t.as_ref().is_some_and(|t| t.wait_us(INTER_CHUNK_GAP_US)));
    if slept {
        return;
    }
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_micros(INTER_CHUNK_GAP_US) {
        std::hint::spin_loop();
    }
}

/// Kinds of write failure, recognised by their error signatures:
/// - Hung: `hid_write/WaitForSingleObject: (0x3E5)`: the device stays enumerated on USB but
///   NAKs OUT reports forever (hidapi times out after 1 s); reconnecting does not help.
/// - Gone: `WriteFile: (0x48F) the device is not connected` / `(0x1F)`: removed from the bus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteFailure {
    Hung,
    Gone,
}

pub fn classify_error(e: &dyn std::error::Error) -> WriteFailure {
    let msg = e.to_string();
    if msg.contains("WaitForSingleObject") || msg.contains("0x000003E5") {
        WriteFailure::Hung
    } else {
        WriteFailure::Gone
    }
}

/// HID connection
pub struct DeviceConnection {
    device: HidDevice,
    pub mac: [u8; 6],
}

impl DeviceConnection {
    /// Connect to the HID device (interface 0).
    /// A full HID enumeration (`HidApi::new`) opens every device to read its strings, so a hung
    /// device could make it take tens of seconds (46 s was seen once). Only our VID/PID is enumerated.
    #[allow(deprecated)] // used to restrict the enumeration scope ourselves, as hidapi 2.6 documents
    pub fn connect(_com_port: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Enumerate ONLY our VID/PID: cheap enough to poll every second, and it never
        // touches unrelated (or hung) devices.
        let mut api = HidApi::new_without_enumerate()?;
        api.add_devices(VENDOR_ID, PRODUCT_ID)?;

        let dev_info = api
            .device_list()
            .find(|d| {
                d.vendor_id() == VENDOR_ID
                    && d.product_id() == PRODUCT_ID
                    && d.interface_number() == 0
            })
            .ok_or_else(|| {
                format!(
                    "LED strip HID device not found (VID={:#06x}, PID={:#06x})",
                    VENDOR_ID, PRODUCT_ID
                )
            })?;

        let path = dev_info.path().to_owned();
        let device = api.open_path(&path)?;
        device.set_blocking_mode(false)?;

        log::info!(
            "HID device connected: VID={:#06x}, PID={:#06x} (interface 0)",
            VENDOR_ID,
            PRODUCT_ID
        );

        let conn = Self {
            device,
            mac: [0u8; 6],
        };
        conn.drain_input();
        Ok(conn)
    }

    /// Discard replies left in the device IN queue before opening (unread by a previous process).
    /// The count is logged as evidence when tracking down hangs.
    fn drain_input(&self) {
        let mut buf = [0u8; 256];
        let mut n_stale = 0u32;
        while let Ok(n) = self.device.read_timeout(&mut buf, 0) {
            if n == 0 {
                break;
            }
            n_stale += 1;
            if n_stale >= 64 {
                break;
            }
        }
        if n_stale > 0 {
            log::warn!(
                "discarded {} unread replies on connect (IN reports left by a previous session)",
                n_stale
            );
        }
    }

    /// Send as a HID output report (report id 0x00 + data)
    fn hid_write(&self, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let mut report = Vec::with_capacity(data.len() + 1);
        report.push(0x00);
        report.extend_from_slice(data);
        self.device.write(&report)?;
        Ok(())
    }

    /// Send a packet in 64-byte chunks, without waiting for a reply
    pub fn write_without_response(&self, packet: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        for (i, chunk) in protocol::chunk_packet(packet).iter().enumerate() {
            if i > 0 {
                inter_chunk_pause();
            }
            self.hid_write(chunk)?;
        }
        Ok(())
    }

    /// Send a packet and wait for the reply (200 ms delay)
    fn write_with_response(
        &self,
        packet: &[u8],
    ) -> Result<Option<protocol::RbResponse>, Box<dyn std::error::Error>> {
        for (i, chunk) in protocol::chunk_packet(packet).iter().enumerate() {
            if i > 0 {
                inter_chunk_pause();
            }
            self.hid_write(chunk)?;
        }

        std::thread::sleep(Duration::from_millis(WRITE_DELAY_MS));

        let mut buf = [0u8; 256];
        match self.device.read_timeout(&mut buf, 500) {
            Ok(n) if n > 0 => Ok(protocol::parse_response(&buf[..n])),
            Ok(_) => Ok(None),
            Err(e) => {
                log::warn!("HID read failed: {}", e);
                Ok(None)
            }
        }
    }

    /// Initialise the device (read its MAC). Returns true if it replied.
    /// false = enumerates but the reply path is dead ("half hung"); streaming may still work.
    pub fn init_device(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let packet = protocol::get_device_info();
        let resp = self.write_with_response(&packet)?;

        let responded = match resp {
            Some(r) if r.payload.len() >= 7 => {
                self.mac.copy_from_slice(&r.payload[1..7]);
                log::info!(
                    "device MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    self.mac[0],
                    self.mac[1],
                    self.mac[2],
                    self.mac[3],
                    self.mac[4],
                    self.mac[5]
                );
                true
            }
            Some(_) => true,
            None => false,
        };

        std::thread::sleep(Duration::from_millis(80));
        Ok(responded)
    }

    // ── Screen sync ──

    pub fn set_sync_screen(&self, colors: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_sync_screen(colors);
        self.write_without_response(&packet)
    }

    // ── Basic control (dedicated action codes) ──

    pub fn set_brightness(&self, val: u8) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_brightness(val);
        self.write_with_response(&packet)?;
        Ok(())
    }

    pub fn turn_off(&self) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::turn_off_light();
        self.write_with_response(&packet)?;
        Ok(())
    }

    // ── LED effects (dedicated action codes) ──

    /// Set LED effect (effectType: 2 = dynamic, 3 = sound reactive)
    pub fn set_led_effect(
        &self,
        effect_type: u8,
        effect_index: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_led_effect(effect_type, effect_index);
        self.write_with_response(&packet)?;
        Ok(())
    }

    /// Set a single LED color (setSectionLED)
    pub fn set_section_led(
        &self,
        r: u8,
        g: u8,
        b: u8,
        lamps_amount: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_section_led(r, g, b, lamps_amount);
        self.write_without_response(&packet)
    }

    pub fn set_dynamic_speed(&self, speed: u8) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_dynamic_speed(speed);
        self.write_with_response(&packet)?;
        Ok(())
    }

    /// Send computer rhythm (effectIndex + volume 0-100), fast, without waiting for a reply
    pub fn set_computer_rhythm(
        &self,
        effect_index: u8,
        volume: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let packet = protocol::set_computer_rhythm(effect_index, volume);
        self.write_without_response(&packet)
    }
}
