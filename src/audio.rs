//! Windows WASAPI audio peak meter
//! Reads the system output level (0.0 to 1.0) through IAudioMeterInformation
//! Noise gate (at most 0.01 becomes 0), raw value, no smoothing

use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

pub struct AudioMeter {
    meter: IAudioMeterInformation,
}

impl AudioMeter {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None)?;

            Ok(Self { meter })
        }
    }

    /// Peak level to an integer 0-100
    /// Noise gate: at most 0.01 gives 0, otherwise raw * 100
    pub fn peak_volume(&mut self) -> u8 {
        let raw = unsafe { self.meter.GetPeakValue().unwrap_or(0.0) };

        let level = if raw <= 0.01 { 0.0 } else { raw };

        (level * 100.0) as u8
    }
}
