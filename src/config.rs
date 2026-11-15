use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct CaptureConfig {
    pub fps: u32,
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
    /// Max R+G+B per LED when `light_compression` is on (power budget; 255 = stock).
    #[serde(default = "default_compression_limit")]
    pub compression_limit: u16,
}

/// LED 동작 모드
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LedMode {
    Sync,
    Dynamic,
    Sound,
    Static,
    Off,
}

impl Default for LedMode {
    fn default() -> Self { Self::Sync }
}

/// 음악 반응 소스
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RhythmSource {
    Controller,
    Computer,
}

impl Default for RhythmSource {
    fn default() -> Self { Self::Controller }
}

/// UI 언어
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Auto,
    En,
    Ko,
}

impl Default for Language {
    fn default() -> Self { Self::Auto }
}

/// 소프트웨어 동적 효과 (단색)
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SoftEffect {
    None,
    Breathe,
    Rotate,
}

impl Default for SoftEffect {
    fn default() -> Self { Self::None }
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

fn default_gamma() -> f32 { 1.0 }
fn default_saturation() -> f32 { 2.0 }
fn default_attack_ms() -> f32 { 40.0 }
fn default_decay_ms() -> f32 { 250.0 }
fn default_deadband() -> f32 { 2.0 }
fn default_compression_limit() -> u16 { 255 }
fn default_true() -> bool { true }
fn default_edge_number() -> u8 { 3 }
fn default_255() -> u8 { 255 }
fn default_50() -> u8 { 50 }

impl SyncConfig {
    pub fn interval_ms(&self) -> u64 {
        self.speed as u64 + 20
    }
}

impl Config {
    /// Refuse absurdly large config files (the file is user-writable).
    const MAX_CONFIG_BYTES: u64 = 64 * 1024;

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if std::fs::metadata(path)?.len() > Self::MAX_CONFIG_BYTES {
            return Err("config file too large".into());
        }
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;
        config.sanitize();
        Ok(config)
    }

    /// Clamp every value into a range the rest of the program can safely use.
    pub fn sanitize(&mut self) {
        fn f(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
            if v.is_nan() { default } else { v.clamp(lo, hi) }
        }
        // LED index is a single byte on the wire
        self.device.lamps_amount = self.device.lamps_amount.clamp(1, 255);
        self.device.display_size = self.device.display_size.clamp(1, 200);
        self.capture.fps = self.capture.fps.clamp(10, 60);
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
        s.compression_limit = s.compression_limit.clamp(30, 765);
        if s.edge_number != 3 && s.edge_number != 4 {
            s.edge_number = 3;
        }
        self.effect.effect_speed = self.effect.effect_speed.clamp(1, 100);
    }

    pub fn load_or_default() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        let candidates = [
            exe_dir.as_ref().map(|d| d.join("config.toml")),
            Some(Path::new("config.toml").to_path_buf()),
        ];

        for candidate in candidates.iter().flatten() {
            if candidate.exists() {
                match Self::load(candidate) {
                    Ok(cfg) => {
                        log::debug!("설정 로드 완료: {}", candidate.display());
                        return cfg;
                    }
                    Err(e) => {
                        log::warn!("설정 파일 파싱 실패 ({}): {}", candidate.display(), e);
                    }
                }
            }
        }

        log::info!("config.toml 없음, 기본 설정 사용");
        Self::default()
    }

    pub fn config_path() -> PathBuf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        if let Some(ref dir) = exe_dir {
            let path = dir.join("config.toml");
            if path.exists() {
                return path;
            }
        }

        let cwd_path = PathBuf::from("config.toml");
        if cwd_path.exists() {
            return cwd_path;
        }

        exe_dir
            .map(|d| d.join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let header = "# SyncRGB 설정 파일\n\n";
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, format!("{}{}", header, content))?;
        Ok(())
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self { com_port: "auto".into(), wire_map: "RGB".into(), display_size: 27, lamps_amount: 65 }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { fps: 30, monitor: 0, monitor_name: String::new(), sample_width: 50, gpu_downscale: true, hdr_sdr_white_nits: 240.0 }
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
            compression_limit: default_compression_limit(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: DeviceConfig::default(),
            capture: CaptureConfig::default(),
            sync: SyncConfig::default(),
            effect: EffectConfig::default(),
            app: AppConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.device.lamps_amount, 65);
        assert_eq!(c.capture.fps, 30);
    }

    #[test]
    fn sanitize_clamps_dangerous_values() {
        let mut c: Config = toml::from_str(
            "[device]\nlamps_amount = 100000\n[capture]\nfps = 0\n[sync]\ngamma = nan\nedge_number = 9\ncompression_limit = 0\nattack_ms = -5.0",
        )
        .unwrap();
        c.sanitize();
        assert_eq!(c.device.lamps_amount, 255);
        assert_eq!(c.capture.fps, 10);
        assert_eq!(c.sync.gamma, 1.0);
        assert_eq!(c.sync.edge_number, 3);
        assert_eq!(c.sync.compression_limit, 30);
        assert_eq!(c.sync.attack_ms, 0.0);
    }

    #[test]
    fn oversized_config_file_is_rejected() {
        let dir = std::env::temp_dir().join("syncfastergb_cfg_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.toml");
        std::fs::write(&p, "#".repeat(70 * 1024)).unwrap();
        assert!(Config::load(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn partial_config_is_accepted() {
        let c: Config = toml::from_str("[device]
lamps_amount = 71
[capture]
monitor_name = \"dp\"").unwrap();
        assert_eq!(c.device.lamps_amount, 71);
        assert_eq!(c.device.display_size, 27);
        assert_eq!(c.capture.monitor_name, "dp");
        assert_eq!(c.capture.sample_width, 50);
    }
}

// ── 자동 시작 (레지스트리) ──

const REG_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const REG_APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const REG_VALUE_NAME: &str = "SyncRGB";

pub fn set_autostart(enable: bool) -> Result<(), Box<dyn std::error::Error>> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    if enable {
        let exe = std::env::current_exe()?;
        // Run 키에 등록
        let run_key = hkcu.open_subkey_with_flags(REG_RUN_KEY, KEY_WRITE)?;
        run_key.set_value(REG_VALUE_NAME, &exe.to_string_lossy().as_ref())?;
        // StartupApproved에 활성화 상태로 등록 (0x02 = enabled)
        if let Ok(approved_key) = hkcu.open_subkey_with_flags(REG_APPROVED_KEY, KEY_WRITE) {
            let enabled = winreg::RegValue {
                vtype: RegType::REG_BINARY,
                bytes: vec![0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            };
            approved_key.set_raw_value(REG_VALUE_NAME, &enabled).ok();
        }
        log::info!("자동 시작 등록 완료 (레지스트리 + StartupApproved)");
    } else {
        if let Ok(run_key) = hkcu.open_subkey_with_flags(REG_RUN_KEY, KEY_WRITE) {
            run_key.delete_value(REG_VALUE_NAME).ok();
        }
        if let Ok(approved_key) = hkcu.open_subkey_with_flags(REG_APPROVED_KEY, KEY_WRITE) {
            approved_key.delete_value(REG_VALUE_NAME).ok();
        }
        log::info!("자동 시작 해제 완료");
    }

    // 기존 Task Scheduler 항목 정리
    cleanup_legacy_task();

    Ok(())
}

pub fn is_autostart_enabled() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let has_run = hkcu.open_subkey(REG_RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(REG_VALUE_NAME))
        .is_ok();
    if !has_run { return false; }

    // StartupApproved에서 비활성화(0x01) 여부 확인
    if let Ok(approved) = hkcu.open_subkey(REG_APPROVED_KEY) {
        if let Ok(val) = approved.get_raw_value(REG_VALUE_NAME) {
            if !val.bytes.is_empty() && val.bytes[0] & 0x01 != 0 && val.bytes[0] & 0x02 == 0 {
                return false; // 사용자가 작업 관리자에서 비활성화
            }
        }
    }
    true
}

/// 기존 Task Scheduler 항목 정리 (마이그레이션)
fn cleanup_legacy_task() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", "SyncRGB", "/f"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
}
