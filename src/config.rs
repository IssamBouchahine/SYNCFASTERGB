use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub device: DeviceConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub effect: EffectConfig,
    #[serde(default)]
    pub app: AppConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct DeviceConfig {
    pub com_port: String,
    pub wire_map: String,
    pub display_size: u32,
    pub lamps_amount: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(default)]
pub struct CaptureConfig {
    pub fps: u32,
    /// Frame rate while a full-screen app (a game) is focused: keeps the GPU/CPU free for it.
    pub game_fps: u32,
    /// Global output index across ALL GPUs (see `--list-monitors`).
    pub monitor: u32,
    /// Optional case-insensitive substring of "<gpu> <display name>"; overrides `monitor` when set.
    pub monitor_name: String,
    pub sample_width: u32,
    /// Shrink frames on the GPU before reading them back (falls back to CPU on failure).
    pub gpu_downscale: bool,
    /// Windows HDR only: nits that SDR white maps to (Settings > Display > HDR > SDR content brightness).
    pub hdr_sdr_white_nits: f32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct SyncConfig {
    pub speed: u32,
    pub brightness: u8,
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    #[serde(default = "default_saturation")]
    pub saturation: f32,
    #[serde(default = "default_true")]
    pub light_compression: bool,
    #[serde(default = "default_true")]
    pub smoothing: bool,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default = "default_edge_number")]
    pub edge_number: u8,
    /// Smoothing time constant when an LED gets brighter (ms).
    #[serde(default = "default_attack_ms")]
    pub attack_ms: f32,
    /// Smoothing time constant when an LED gets darker (ms).
    #[serde(default = "default_decay_ms")]
    pub decay_ms: f32,
    /// Output hysteresis in 0..255 steps (suppresses shimmer).
    #[serde(default = "default_deadband")]
    pub deadband: f32,
    /// Match the screen's colours as exactly as an RGB strip can: the screen's encoded values are
    /// turned back into real light, so shades, browns, pastels and fades look the same on the
    /// strip. Off = the older "vivid" look (`saturation` and `gamma` apply).
    #[serde(default = "default_true")]
    pub accurate_color: bool,
    /// Brightness curve of accurate matching. 2.2 follows the screen's own light exactly; lower
    /// values keep dark and mid scenes brighter. Colours (hue, saturation) are exact either way.
    #[serde(default = "default_match_gamma")]
    pub match_gamma: f32,
    /// Per-channel strength in percent (100 = unchanged): lowers a channel the strip's LEDs
    /// show too strongly, so white looks neutral.
    #[serde(default = "default_balance")]
    pub balance_r: u8,
    #[serde(default = "default_balance")]
    pub balance_g: u8,
    #[serde(default = "default_balance")]
    pub balance_b: u8,
    /// Curve that turns the screen's encoded values into light for the colour balance (2.2 is the
    /// standard; a little lower gives mid-tones such as orange a touch more green).
    #[serde(default = "default_colour_curve")]
    pub colour_curve: f32,
    /// Extra steepness of the colour curve for dark colours (fading out at half brightness):
    /// keeps a dark red-brown red instead of orange.
    #[serde(default = "default_dark_curve_extra")]
    pub dark_curve_extra: f32,
    /// Extra steepness of the brightness curve for dark colours (fading out at half brightness):
    /// keeps a dark colour as dim on the strip as it is on the screen.
    #[serde(default = "default_dark_level_extra")]
    pub dark_level_extra: f32,
    /// How much of the correction for the strip's LED colours is applied, in percent. The strip's
    /// green is bluer than the screen's, its red deeper: this mixes in what is needed so greens,
    /// yellows and whites look like the screen's.
    #[serde(default = "default_balance")]
    pub led_match_pct: u8,
    /// Strength of the strip's green LED relative to its red one (calibration).
    #[serde(default = "default_led_green")]
    pub led_green: f32,
    /// Strength of the strip's blue LED relative to its red one (calibration).
    #[serde(default = "default_led_blue")]
    pub led_blue: f32,
    /// How much more (or less) colourful the monitor looks than the captured picture, in percent
    /// (100 = the same). Graphics-driver adjustments such as NVIDIA "Digital vibrance" happen
    /// after the picture is captured, so the app cannot see them; this compensates.
    #[serde(default = "default_balance")]
    pub display_vibrance_pct: u8,
    /// Whole-strip power budget in percent of "every LED fully white" (used when
    /// `light_compression` is on). Highlights can reach full brightness; only the total is capped.
    #[serde(default = "default_power_limit_pct")]
    pub power_limit_pct: u8,
    /// Judge the screen in real time: bars are found by their edge and the depth adapts between
    /// 10% and 20%. When off, `edge_depth_pct`, `skip_black_bars` and `black_level` apply.
    #[serde(default = "default_true")]
    pub detect_auto: bool,
    /// How far in from each screen edge the LEDs look, in percent of the screen size (manual).
    #[serde(default = "default_edge_depth_pct")]
    pub edge_depth_pct: u8,
    /// Sample just inside black bars (letterboxed / pillarboxed video) instead of on them.
    #[serde(default = "default_true")]
    pub skip_black_bars: bool,
    /// A screen line darker than this (0..255) counts as part of a black bar.
    #[serde(default = "default_black_level")]
    pub black_level: u8,
}

/// LED operating mode
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum LedMode {
    #[default]
    Sync,
    Dynamic,
    Sound,
    Static,
    Off,
}

/// Sound-reactive source
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RhythmSource {
    #[default]
    Controller,
    Computer,
}

/// UI language
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Language {
    #[default]
    Auto,
    En,
    Ko,
}

/// Software dynamic effect (single color)
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SoftEffect {
    #[default]
    None,
    Breathe,
    Rotate,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EffectConfig {
    #[serde(default)]
    pub mode: LedMode,
    #[serde(default)]
    pub dynamic_index: u8,
    #[serde(default)]
    pub sound_index: u8,
    #[serde(default)]
    pub color_r: u8,
    #[serde(default)]
    pub color_g: u8,
    #[serde(default = "default_255")]
    pub color_b: u8,
    #[serde(default = "default_50")]
    pub effect_speed: u8,
    #[serde(default)]
    pub rhythm_source: RhythmSource,
    #[serde(default)]
    pub soft_effect: SoftEffect,
}

impl Default for EffectConfig {
    fn default() -> Self {
        Self {
            mode: LedMode::Sync,
            dynamic_index: 0,
            sound_index: 0,
            color_r: 0,
            color_g: 0,
            color_b: 255,
            effect_speed: 50,
            rhythm_source: RhythmSource::Controller,
            soft_effect: SoftEffect::None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub show_console: bool,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub turn_off_on_sleep: bool,
    #[serde(default)]
    pub turn_off_on_black: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            autostart: false,
            show_console: false,
            language: Language::Auto,
            turn_off_on_sleep: false,
            turn_off_on_black: false,
        }
    }
}

fn default_gamma() -> f32 {
    1.0
}
fn default_saturation() -> f32 {
    2.0
}
fn default_match_gamma() -> f32 {
    1.6
}
fn default_balance() -> u8 {
    100
}
// Calibrated by eye against a white wall on the target strip, colour by colour (yellow, orange,
// pastel orange, green, white, red, cyan, purple, dark red-brown, skin, sky blue, greys): the
// strip's green LED is strong against its red one, its blue a little stronger than typical, its
// red and green need a small trim, a monitor with driver vibrance looks more colourful than the
// capture, and dark colours need a steeper, dimmer curve.
fn default_led_green() -> f32 {
    3.0
}
fn default_led_blue() -> f32 {
    0.28
}
fn default_colour_curve() -> f32 {
    2.1
}
fn default_dark_curve_extra() -> f32 {
    1.35
}
fn default_dark_level_extra() -> f32 {
    0.4
}
fn default_attack_ms() -> f32 {
    40.0
}
fn default_decay_ms() -> f32 {
    250.0
}
fn default_deadband() -> f32 {
    2.0
}
/// Highest power budget the app will ever apply (percent of all-LEDs-white).
pub const MAX_POWER_LIMIT_PCT: u8 = 80;

fn default_power_limit_pct() -> u8 {
    40
}
fn default_edge_depth_pct() -> u8 {
    15
}
fn default_black_level() -> u8 {
    26
}
fn default_true() -> bool {
    true
}
fn default_edge_number() -> u8 {
    3
}
fn default_255() -> u8 {
    255
}
fn default_50() -> u8 {
    50
}

/// Where the settings live. A registry value instead of a file keeps the whole program a
/// single exe: no config file, no log file, nothing next to it.
const STORE_KEY: &str = r"Software\SYNCFASTERGB";
const VALUE_SETTINGS: &str = "Settings";
const VALUE_REVISION: &str = "Revision";

impl SyncConfig {
    pub fn interval_ms(&self) -> u64 {
        self.speed as u64 + 16
    }
}

/// What the settings store holds right now.
enum Stored {
    /// Nothing saved yet (or everything was removed).
    Missing,
    Valid(Config),
    /// Settings exist but could not be read or parsed, even after retrying.
    Unreadable(String),
}

impl Config {
    /// Stored settings text larger than this is rejected.
    pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

    /// Parse and sanitize settings text (TOML).
    pub fn from_toml(text: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if text.len() > Self::MAX_SETTINGS_BYTES {
            return Err("settings too large".into());
        }
        let mut config: Config = toml::from_str(text)?;
        config.sanitize();
        Ok(config)
    }

    pub fn to_toml(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Clamp every value into a range the rest of the program can safely use.
    pub fn sanitize(&mut self) {
        fn f(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
            if v.is_nan() {
                default
            } else {
                v.clamp(lo, hi)
            }
        }
        // LED index is a single byte on the wire
        self.device.lamps_amount = self.device.lamps_amount.clamp(1, 255);
        self.device.display_size = self.device.display_size.clamp(1, 200);
        self.capture.fps = self.capture.fps.clamp(10, 60);
        self.capture.game_fps = self.capture.game_fps.clamp(10, 60);
        self.capture.sample_width = self.capture.sample_width.clamp(1, 500);
        self.capture.monitor_name.truncate(128);
        self.capture.hdr_sdr_white_nits = f(self.capture.hdr_sdr_white_nits, 80.0, 1000.0, 240.0);
        let s = &mut self.sync;
        s.speed = s.speed.min(100);
        s.gamma = f(s.gamma, 0.1, 3.0, 1.0);
        s.saturation = f(s.saturation, 0.0, 3.0, 2.0);
        s.attack_ms = f(s.attack_ms, 0.0, 2000.0, 40.0);
        s.decay_ms = f(s.decay_ms, 0.0, 5000.0, 250.0);
        s.deadband = f(s.deadband, 0.0, 10.0, 2.0);
        // Hard ceiling: a laptop USB port cannot be asked for more than a fraction of the
        // strip's worst-case draw, whatever the config file says.
        s.power_limit_pct = s.power_limit_pct.clamp(10, MAX_POWER_LIMIT_PCT);
        s.edge_depth_pct = s.edge_depth_pct.clamp(3, 40);
        s.black_level = s.black_level.clamp(2, 80);
        s.match_gamma = f(s.match_gamma, 1.0, 3.0, 1.6);
        s.balance_r = s.balance_r.clamp(20, 100);
        s.balance_g = s.balance_g.clamp(20, 100);
        s.balance_b = s.balance_b.clamp(20, 100);
        s.display_vibrance_pct = s.display_vibrance_pct.clamp(50, 200);
        s.led_match_pct = s.led_match_pct.min(100);
        s.colour_curve = f(s.colour_curve, 1.6, 2.6, 2.1);
        s.dark_curve_extra = f(s.dark_curve_extra, 0.0, 3.0, 1.35);
        s.dark_level_extra = f(s.dark_level_extra, 0.0, 1.5, 0.4);
        s.led_green = f(s.led_green, 1.0, 3.5, 2.0);
        s.led_blue = f(s.led_blue, 0.1, 1.0, 0.25);
        if s.edge_number != 3 && s.edge_number != 4 {
            s.edge_number = 3;
        }
        self.effect.effect_speed = self.effect.effect_speed.clamp(1, 100);
    }

    /// Current settings, or the defaults when nothing has been saved yet (nothing is written).
    pub fn load_or_default() -> Self {
        Self::load_from_key(STORE_KEY).unwrap_or_default()
    }

    /// Reload for a running program: the stored settings, or the defaults when nothing is stored
    /// (the user removed them), but the settings already in use when stored ones exist and
    /// cannot be read right now. A failed read must never reset a running strip to factory values.
    pub fn load_or_keep(current: &Config) -> Config {
        Self::load_or_keep_from(STORE_KEY, current)
    }

    fn load_or_keep_from(key_path: &str, current: &Config) -> Config {
        match Self::read_stored(key_path) {
            Stored::Missing => Config::default(),
            Stored::Valid(c) => c,
            Stored::Unreadable(e) => {
                log::error!("stored settings unreadable ({e}): keeping the current settings");
                current.clone()
            }
        }
    }

    pub fn load_from_key(key_path: &str) -> Option<Self> {
        match Self::read_stored(key_path) {
            Stored::Valid(c) => Some(c),
            Stored::Missing => None,
            Stored::Unreadable(e) => {
                log::error!("stored settings unreadable ({e}): using defaults");
                None
            }
        }
    }

    /// One look at the store, retried a few times when settings exist but cannot be read or
    /// parsed (another process may be in the middle of writing them).
    fn read_stored(key_path: &str) -> Stored {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let mut last_error = String::new();
        for attempt in 0..4 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(75));
            }
            let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(key_path) {
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Stored::Missing,
                Err(e) => {
                    last_error = format!("cannot open the settings key: {e}");
                    continue;
                }
            };
            let text: String = match key.get_value(VALUE_SETTINGS) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Stored::Missing,
                Err(e) => {
                    last_error = format!("cannot read the settings value: {e}");
                    continue;
                }
            };
            match Self::from_toml(&text) {
                Ok(c) => return Stored::Valid(c),
                Err(e) => last_error = e.to_string(),
            }
        }
        Stored::Unreadable(last_error)
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.save_to_key(STORE_KEY)
    }

    /// Write the settings and bump the revision counter the tray polls for hot reload.
    /// An identical value is not rewritten.
    pub fn save_to_key(&self, key_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let text = self.to_toml()?;
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key_path)?;
        if key
            .get_value::<String, _>(VALUE_SETTINGS)
            .is_ok_and(|old| old == text)
        {
            return Ok(());
        }
        key.set_value(VALUE_SETTINGS, &text)?;
        let rev: u32 = key.get_value(VALUE_REVISION).unwrap_or(0);
        key.set_value(VALUE_REVISION, &rev.wrapping_add(1))?;
        Ok(())
    }

    /// Counter that changes whenever settings are saved (0 when nothing is stored).
    pub fn revision() -> u32 {
        Self::revision_of(STORE_KEY)
    }

    pub fn revision_of(key_path: &str) -> u32 {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(key_path)
            .and_then(|k| k.get_value::<u32, _>(VALUE_REVISION))
            .unwrap_or(0)
    }

    /// Remove everything this program stores on the PC: the settings and the autostart entry.
    pub fn forget_everything() -> Result<(), Box<dyn std::error::Error>> {
        Self::forget_store_key(STORE_KEY)?;
        set_autostart(false)
    }

    /// Delete one settings key; a key that does not exist is not an error.
    pub fn forget_store_key(key_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(key_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            com_port: "auto".into(),
            wire_map: "RGB".into(),
            // the 34" kit this program is built for: 71 LEDs, so even a fallback to defaults
            // lights the whole strip
            display_size: 34,
            lamps_amount: 71,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 60,
            game_fps: 30,
            monitor: 0,
            monitor_name: String::new(),
            sample_width: 50,
            gpu_downscale: true,
            hdr_sdr_white_nits: 240.0,
        }
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            speed: 0,
            brightness: 255,
            gamma: 1.0,
            saturation: default_saturation(),
            light_compression: true,
            smoothing: true,
            reverse: false,
            edge_number: 3,
            attack_ms: default_attack_ms(),
            decay_ms: default_decay_ms(),
            deadband: default_deadband(),
            accurate_color: true,
            match_gamma: default_match_gamma(),
            balance_r: 90,
            balance_g: 90,
            balance_b: default_balance(),
            display_vibrance_pct: 135,
            colour_curve: default_colour_curve(),
            dark_curve_extra: default_dark_curve_extra(),
            dark_level_extra: default_dark_level_extra(),
            led_match_pct: default_balance(),
            led_green: default_led_green(),
            led_blue: default_led_blue(),
            power_limit_pct: default_power_limit_pct(),
            detect_auto: true,
            edge_depth_pct: default_edge_depth_pct(),
            skip_black_bars: true,
            black_level: default_black_level(),
        }
    }
}

// ── Autostart (registry) ──

const REG_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const REG_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const REG_VALUE_NAME: &str = "SYNCFASTERGB";

pub fn set_autostart(enable: bool) -> Result<(), Box<dyn std::error::Error>> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    if enable {
        let exe = std::env::current_exe()?;
        // Register under the Run key
        let run_key = hkcu.open_subkey_with_flags(REG_RUN_KEY, KEY_WRITE)?;
        run_key.set_value(REG_VALUE_NAME, &exe.to_string_lossy().as_ref())?;
        // Mark it enabled in StartupApproved (0x02 = enabled)
        if let Ok(approved_key) = hkcu.open_subkey_with_flags(REG_APPROVED_KEY, KEY_WRITE) {
            let enabled = winreg::RegValue {
                vtype: RegType::REG_BINARY,
                bytes: vec![0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
            };
            approved_key.set_raw_value(REG_VALUE_NAME, &enabled).ok();
        }
        log::info!("autostart registered (Run key + StartupApproved)");
    } else {
        if let Ok(run_key) = hkcu.open_subkey_with_flags(REG_RUN_KEY, KEY_WRITE) {
            run_key.delete_value(REG_VALUE_NAME).ok();
        }
        if let Ok(approved_key) = hkcu.open_subkey_with_flags(REG_APPROVED_KEY, KEY_WRITE) {
            approved_key.delete_value(REG_VALUE_NAME).ok();
        }
        log::info!("autostart removed");
    }

    Ok(())
}

pub fn is_autostart_enabled() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let has_run = hkcu
        .open_subkey(REG_RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(REG_VALUE_NAME))
        .is_ok();
    if !has_run {
        return false;
    }

    // Check whether it was disabled (0x01) in StartupApproved
    if let Ok(approved) = hkcu.open_subkey(REG_APPROVED_KEY) {
        if let Ok(val) = approved.get_raw_value(REG_VALUE_NAME) {
            if !val.bytes.is_empty() && val.bytes[0] & 0x01 != 0 && val.bytes[0] & 0x02 == 0 {
                return false; // the user disabled it in Task Manager
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.device.lamps_amount, 71);
        assert_eq!(c.capture.fps, 60);
        assert_eq!(c.capture.game_fps, 30);
        assert!(c.sync.detect_auto, "automatic detection is on by default");
        assert!(
            c.sync.accurate_color,
            "accurate colour matching is on by default"
        );
        assert_eq!(c.sync.match_gamma, 1.6);
        assert_eq!(
            (c.sync.balance_r, c.sync.balance_g, c.sync.balance_b),
            (90, 90, 100)
        );
        assert_eq!(c.sync.display_vibrance_pct, 135);
        assert_eq!(c.sync.led_match_pct, 100);
        assert_eq!((c.sync.led_green, c.sync.led_blue), (3.0, 0.28));
        assert_eq!(c.sync.colour_curve, 2.1);
        assert_eq!(c.sync.dark_curve_extra, 1.35);
        assert_eq!(c.sync.dark_level_extra, 0.4);
        assert_eq!(c.sync.edge_depth_pct, 15);
        assert!(c.sync.skip_black_bars);
        assert_eq!(c.sync.black_level, 26);
    }

    #[test]
    fn detection_settings_are_clamped() {
        let mut c: Config =
            toml::from_str("[sync]\nedge_depth_pct = 0\nblack_level = 255").unwrap();
        c.sanitize();
        assert_eq!(c.sync.edge_depth_pct, 3);
        assert_eq!(c.sync.black_level, 80);
        let mut c: Config =
            toml::from_str("[sync]\nedge_depth_pct = 200\nblack_level = 0").unwrap();
        c.sanitize();
        assert_eq!(c.sync.edge_depth_pct, 40);
        assert_eq!(c.sync.black_level, 2);
    }

    #[test]
    fn sanitize_clamps_dangerous_values() {
        let mut c: Config = toml::from_str(
            "[device]\nlamps_amount = 100000\n[capture]\nfps = 0\n[sync]\ngamma = nan\nedge_number = 9\npower_limit_pct = 99\nattack_ms = -5.0",
        )
        .unwrap();
        c.sanitize();
        assert_eq!(c.device.lamps_amount, 255);
        assert_eq!(c.capture.fps, 10);
        assert_eq!(c.sync.gamma, 1.0);
        assert_eq!(c.sync.edge_number, 3);
        assert_eq!(c.sync.power_limit_pct, MAX_POWER_LIMIT_PCT);
        assert_eq!(c.sync.attack_ms, 0.0);
    }

    #[test]
    fn oversized_settings_are_rejected() {
        assert!(Config::from_toml(&"#".repeat(70 * 1024)).is_err());
        assert!(Config::from_toml("").is_ok());
    }

    #[test]
    fn registry_round_trip_revision_and_no_rewrite() {
        let key = r"Software\SYNCFASTERGB_Test_roundtrip";
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(key);
        assert!(Config::load_from_key(key).is_none());
        assert_eq!(Config::revision_of(key), 0);

        let mut c = Config::default();
        c.device.lamps_amount = 71;
        c.sync.brightness = 123;
        c.save_to_key(key).unwrap();
        assert_eq!(Config::revision_of(key), 1);
        let back = Config::load_from_key(key).unwrap();
        assert_eq!(back.device.lamps_amount, 71);
        assert_eq!(back.sync.brightness, 123);

        c.save_to_key(key).unwrap(); // identical: no rewrite, revision unchanged
        assert_eq!(Config::revision_of(key), 1);
        c.sync.brightness = 200;
        c.save_to_key(key).unwrap();
        assert_eq!(Config::revision_of(key), 2);

        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(key);
        assert!(Config::load_from_key(key).is_none());
    }

    #[test]
    fn forgetting_removes_stored_settings_and_tolerates_missing_keys() {
        let key = r"Software\SYNCFASTERGB_Test_forget";
        Config::forget_store_key(key).unwrap(); // nothing there yet: fine
        Config::default().save_to_key(key).unwrap();
        assert!(Config::load_from_key(key).is_some());
        Config::forget_store_key(key).unwrap();
        assert!(Config::load_from_key(key).is_none());
        assert_eq!(Config::revision_of(key), 0);
        Config::forget_store_key(key).unwrap(); // already gone: still fine
    }

    #[test]
    fn corrupt_stored_settings_fall_back_to_none() {
        use winreg::enums::HKEY_CURRENT_USER;
        let key = r"Software\SYNCFASTERGB_Test_corrupt";
        let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let (k, _) = hk.create_subkey(key).unwrap();
        k.set_value("Settings", &"this is [not valid toml".to_string())
            .unwrap();
        assert!(Config::load_from_key(key).is_none());
        let _ = hk.delete_subkey_all(key);
    }

    #[test]
    fn a_failed_reload_keeps_the_running_settings_instead_of_resetting_them() {
        use winreg::enums::HKEY_CURRENT_USER;
        let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let key = r"Software\SYNCFASTERGB_Test_keep";
        let _ = hk.delete_subkey_all(key);

        let mut running = Config::default();
        running.device.lamps_amount = 99;
        running.sync.brightness = 77;

        // nothing stored (the user removed everything): defaults are right
        assert_eq!(
            Config::load_or_keep_from(key, &running).device.lamps_amount,
            71
        );

        // stored but unreadable: the settings in use stay
        let (k, _) = hk.create_subkey(key).unwrap();
        k.set_value("Settings", &"this is [not valid toml".to_string())
            .unwrap();
        let kept = Config::load_or_keep_from(key, &running);
        assert_eq!((kept.device.lamps_amount, kept.sync.brightness), (99, 77));

        // stored and valid: the stored settings win
        let mut stored = Config::default();
        stored.device.lamps_amount = 80;
        k.set_value("Settings", &stored.to_toml().unwrap()).unwrap();
        assert_eq!(
            Config::load_or_keep_from(key, &running).device.lamps_amount,
            80
        );
        let _ = hk.delete_subkey_all(key);
    }

    #[test]
    fn partial_config_is_accepted() {
        let c: Config = toml::from_str(
            "[device]
lamps_amount = 71
[capture]
monitor_name = \"dp\"",
        )
        .unwrap();
        assert_eq!(c.device.lamps_amount, 71);
        assert_eq!(c.device.display_size, 34);
        assert_eq!(c.capture.monitor_name, "dp");
        assert_eq!(c.capture.sample_width, 50);
    }
}
