use eframe::egui;
use std::path::PathBuf;

use crate::capture::monitors::{self, MonitorInfo};
use crate::config::{self, Config, LedMode, RhythmSource, SoftEffect};
use eframe::egui::Color32;

const WIRE_MAPS: &[&str] = &["RGB", "RBG", "GRB", "GBR", "BRG", "BGR"];

// ── 다국어 (Locale) ──

pub struct Locale {
    pub tab_sync: &'static str,
    pub tab_dynamic: &'static str,
    pub tab_sound: &'static str,
    pub tab_static: &'static str,
    pub led_on: &'static str,
    pub led_off: &'static str,
    pub led_off_msg: &'static str,
    pub led_off_hint: &'static str,
    pub brightness: &'static str,
    pub settings: &'static str,
    pub applied: &'static str,
    pub save_failed_prefix: &'static str,
    pub failed_keyword: &'static str,
    pub sync_speed: &'static str,
    pub speed_fastest: &'static str,
    pub speed_fast: &'static str,
    pub speed_normal: &'static str,
    pub speed_slow: &'static str,
    pub color_processing: &'static str,
    pub saturation: &'static str,
    pub gamma: &'static str,
    pub light_compression: &'static str,
    pub smoothing: &'static str,
    pub led_layout: &'static str,
    pub direction: &'static str,
    pub dir_ltr: &'static str,
    pub dir_rtl: &'static str,
    pub edges: &'static str,
    pub edges_3: &'static str,
    pub edges_4: &'static str,
    pub pattern_hint: &'static str,
    pub effect_speed: &'static str,
    pub sound_source: &'static str,
    pub source_controller: &'static str,
    pub source_computer: &'static str,
    pub color_adjust: &'static str,
    pub quick_select: &'static str,
    pub static_animation: &'static str,
    pub anim_none: &'static str,
    pub anim_breathe: &'static str,
    pub anim_rotate: &'static str,
    pub speed: &'static str,
    pub device: &'static str,
    pub led_count: &'static str,
    pub monitor_label: &'static str,
    pub capture_fps: &'static str,
    pub app_section: &'static str,
    pub autostart: &'static str,
    pub autostart_failed_prefix: &'static str,
    pub save_settings: &'static str,
    pub dynamic_effects: &'static [&'static str],
    pub sound_effects: &'static [&'static str],
    pub color_presets: &'static [(&'static str, [u8; 3])],
    pub tray_pause: &'static str,
    pub tray_resume: &'static str,
    pub tray_settings: &'static str,
    pub tray_quit: &'static str,
    pub tray_paused_tooltip: &'static str,
    pub language_label: &'static str,
    pub lang_auto: &'static str,
    pub turn_off_on_sleep: &'static str,
    pub turn_off_on_black: &'static str,
}

static KO: Locale = Locale {
    tab_sync: "화면 동기화",
    tab_dynamic: "동적 효과",
    tab_sound: "음악 반응",
    tab_static: "단색",
    led_on: "LED ON",
    led_off: "LED OFF",
    led_off_msg: "LED가 꺼져 있습니다",
    led_off_hint: "모드 탭을 클릭하면 LED가 자동으로 켜집니다",
    brightness: "밝기:",
    settings: "설정",
    applied: "적용됨",
    save_failed_prefix: "저장 실패: ",
    failed_keyword: "실패",
    sync_speed: "동기화 속도",
    speed_fastest: "극빠름",
    speed_fast: "빠름",
    speed_normal: "보통",
    speed_slow: "느림",
    color_processing: "색상 처리",
    saturation: "채도",
    gamma: "감마",
    light_compression: "광량 압축 (USB 전력 최적화)",
    smoothing: "부드러운 전환 (스무딩)",
    led_layout: "LED 배치",
    direction: "방향:",
    dir_ltr: "왼쪽 → 오른쪽",
    dir_rtl: "오른쪽 → 왼쪽",
    edges: "변 수:",
    edges_3: "3면 (상/좌/우)",
    edges_4: "4면",
    pattern_hint: "패턴을 선택하면 즉시 적용됩니다",
    effect_speed: "효과 속도",
    sound_source: "음악 소스",
    source_controller: "컨트롤러 (내장 마이크)",
    source_computer: "컴퓨터 (PC 오디오)",
    color_adjust: "색상 조절",
    quick_select: "빠른 선택",
    static_animation: "단색 애니메이션",
    anim_none: "없음",
    anim_breathe: "숨쉬기",
    anim_rotate: "회전",
    speed: "속도",
    device: "디바이스",
    led_count: "LED 개수:",
    monitor_label: "모니터:",
    capture_fps: "캡처 FPS:",
    app_section: "앱",
    autostart: "Windows 시작 시 자동 실행",
    autostart_failed_prefix: "자동 시작 실패: ",
    save_settings: "  설정 저장  ",
    dynamic_effects: &[
        "무지개 흐름", "숨쉬기", "컬러 체이스",
        "유성", "반짝이", "그라데이션", "마키",
    ],
    sound_effects: &[
        "리듬 웨이브", "리듬 펄스", "리듬 스펙트럼",
        "리듬 플래시", "리듬 그라데이션", "리듬 체이스", "리듬 레인보우",
    ],
    color_presets: &[
        ("빨강", [255, 0, 0]),
        ("주황", [255, 120, 0]),
        ("노랑", [255, 255, 0]),
        ("초록", [0, 255, 0]),
        ("시안", [0, 255, 255]),
        ("파랑", [0, 0, 255]),
        ("보라", [150, 0, 255]),
        ("핑크", [255, 0, 150]),
        ("흰색", [255, 255, 255]),
        ("웜 화이트", [255, 180, 100]),
    ],
    tray_pause: "일시정지",
    tray_resume: "재개",
    tray_settings: "설정",
    tray_quit: "종료",
    tray_paused_tooltip: "SYNCFASTERGB (일시정지)",
    language_label: "언어",
    lang_auto: "자동",
    turn_off_on_sleep: "모니터 절전 시 LED 끄기",
    turn_off_on_black: "검정 화면 시 LED 끄기",
};

static EN: Locale = Locale {
    tab_sync: "Screen Sync",
    tab_dynamic: "Dynamic Effects",
    tab_sound: "Sound Reactive",
    tab_static: "Static Color",
    led_on: "LED ON",
    led_off: "LED OFF",
    led_off_msg: "LEDs are turned off",
    led_off_hint: "Click a mode tab to turn LEDs on",
    brightness: "Brightness:",
    settings: "Settings",
    applied: "Applied",
    save_failed_prefix: "Save failed: ",
    failed_keyword: "failed",
    sync_speed: "Sync Speed",
    speed_fastest: "Fastest",
    speed_fast: "Fast",
    speed_normal: "Normal",
    speed_slow: "Slow",
    color_processing: "Color Processing",
    saturation: "Saturation",
    gamma: "Gamma",
    light_compression: "Light Compression (USB power opt.)",
    smoothing: "Smooth Transition (Smoothing)",
    led_layout: "LED Layout",
    direction: "Direction:",
    dir_ltr: "Left → Right",
    dir_rtl: "Right → Left",
    edges: "Edges:",
    edges_3: "3 sides (Top/Left/Right)",
    edges_4: "4 sides",
    pattern_hint: "Select a pattern to apply instantly",
    effect_speed: "Effect Speed",
    sound_source: "Sound Source",
    source_controller: "Controller (Built-in Mic)",
    source_computer: "Computer (PC Audio)",
    color_adjust: "Color Adjustment",
    quick_select: "Quick Select",
    static_animation: "Static Animation",
    anim_none: "None",
    anim_breathe: "Breathe",
    anim_rotate: "Rotate",
    speed: "Speed",
    device: "Device",
    led_count: "LED Count:",
    monitor_label: "Monitor:",
    capture_fps: "Capture FPS:",
    app_section: "App",
    autostart: "Start with Windows",
    autostart_failed_prefix: "Autostart failed: ",
    save_settings: "  Save Settings  ",
    dynamic_effects: &[
        "Rainbow Flow", "Breathing", "Color Chase",
        "Meteor", "Sparkle", "Gradient", "Marquee",
    ],
    sound_effects: &[
        "Rhythm Wave", "Rhythm Pulse", "Rhythm Spectrum",
        "Rhythm Flash", "Rhythm Gradient", "Rhythm Chase", "Rhythm Rainbow",
    ],
    color_presets: &[
        ("Red", [255, 0, 0]),
        ("Orange", [255, 120, 0]),
        ("Yellow", [255, 255, 0]),
        ("Green", [0, 255, 0]),
        ("Cyan", [0, 255, 255]),
        ("Blue", [0, 0, 255]),
        ("Purple", [150, 0, 255]),
        ("Pink", [255, 0, 150]),
        ("White", [255, 255, 255]),
        ("Warm White", [255, 180, 100]),
    ],
    tray_pause: "Pause",
    tray_resume: "Resume",
    tray_settings: "Settings",
    tray_quit: "Quit",
    tray_paused_tooltip: "SYNCFASTERGB (Paused)",
    language_label: "Language",
    lang_auto: "Auto",
    turn_off_on_sleep: "Turn off LEDs on monitor sleep",
    turn_off_on_black: "Turn off LEDs on black screen",
};

pub fn resolve_locale(lang: &crate::config::Language) -> &'static Locale {
    use crate::config::Language;
    match lang {
        Language::Ko => &KO,
        Language::En => &EN,
        Language::Auto => {
            use windows::Win32::Globalization::GetUserDefaultUILanguage;
            let lang_id = unsafe { GetUserDefaultUILanguage() };
            if lang_id & 0x3FF == 0x12 { &KO } else { &EN }
        }
    }
}

pub fn detect_locale() -> &'static Locale {
    let config = crate::config::Config::load_or_default();
    resolve_locale(&config.app.language)
}

/// 설정 GUI를 별도 프로세스로 실행
pub fn open_settings() {
    if let Ok(exe) = std::env::current_exe() {
        if let Err(e) = std::process::Command::new(exe).arg("--settings").spawn() {
            log::error!("Failed to open settings GUI: {}", e);
        }
    }
}

/// RGB 스펙트럼 링 아이콘 RGBA 데이터 생성
pub fn generate_rgb_icon(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let outer_r = c - 0.5;
    let inner_r = outer_r * 0.5;
    let aa = if size <= 16 { 0.8 } else { 1.2 };

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - c;
            let dy = py - c;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * size + x) * 4) as usize;

            let outer_mask = icon_smoothstep(outer_r + aa, outer_r - aa, dist);
            let inner_mask = icon_smoothstep(inner_r - aa, inner_r + aa, dist);
            let alpha = outer_mask * inner_mask;

            if alpha > 0.001 {
                let angle = dx.atan2(-dy);
                let hue = (angle.to_degrees() + 360.0) % 360.0;
                let (r, g, b) = icon_hsl_to_rgb(hue, 1.0, 0.5);
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = (alpha * 255.0) as u8;
            }
        }
    }
    rgba
}

fn icon_smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn icon_hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// --settings 모드 진입점
pub fn run_settings_window() {
    let locale = detect_locale();
    let config_path = Config::config_path();
    let config = Config::load(&config_path).unwrap_or_default();
    let app = SettingsGui::new(config, config_path, locale);

    let icon_data = generate_rgb_icon(32);
    let icon = egui::IconData { rgba: icon_data, width: 32, height: 32 };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 760.0])
            .with_min_inner_size([500.0, 560.0])
            .with_resizable(true)
            .with_icon(icon),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "SYNCFASTERGB",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    ) {
        eprintln!("GUI failed: {}", e);
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(data) = std::fs::read("C:\\Windows\\Fonts\\malgun.ttf") {
        fonts.font_data.insert("malgun".into(), egui::FontData::from_owned(data).into());
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "malgun".into());
    }
    ctx.set_fonts(fonts);
}

// ── 테마 ──

mod theme {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(11, 14, 20);
    pub const CARD: Color32 = Color32::from_rgb(20, 25, 36);
    pub const CARD_HI: Color32 = Color32::from_rgb(28, 34, 48);
    pub const BORDER: Color32 = Color32::from_rgb(38, 46, 64);
    pub const TEXT: Color32 = Color32::from_rgb(232, 237, 247);
    pub const MUTED: Color32 = Color32::from_rgb(138, 149, 172);
    pub const ACCENT: Color32 = Color32::from_rgb(56, 200, 255);
    pub const GOOD: Color32 = Color32::from_rgb(86, 224, 146);
    pub const WARN: Color32 = Color32::from_rgb(255, 186, 80);
    pub const BAD: Color32 = Color32::from_rgb(255, 104, 104);
}

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.slider_width = 220.0;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = theme::BG;
    v.window_fill = theme::CARD;
    v.extreme_bg_color = Color32X::INPUT;
    v.override_text_color = Some(theme::TEXT);
    v.selection.bg_fill = theme::ACCENT.gamma_multiply(0.55);
    v.selection.stroke = egui::Stroke::new(1.0_f32, theme::ACCENT);
    v.widgets.noninteractive.bg_fill = theme::CARD;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, theme::BORDER);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.rounding = egui::Rounding::same(9.0);
    }
    v.widgets.inactive.bg_fill = theme::CARD_HI;
    v.widgets.inactive.weak_bg_fill = theme::CARD_HI;
    v.widgets.hovered.bg_fill = Color32X::HOVER;
    v.widgets.hovered.weak_bg_fill = Color32X::HOVER;
    v.widgets.active.bg_fill = theme::ACCENT.gamma_multiply(0.6);
    v.widgets.active.weak_bg_fill = theme::ACCENT.gamma_multiply(0.6);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, theme::BORDER);
    v.popup_shadow = egui::Shadow::NONE;
    ctx.set_style(style);
}

struct Color32X;
impl Color32X {
    const INPUT: Color32 = Color32::from_rgb(14, 18, 27);
    const HOVER: Color32 = Color32::from_rgb(36, 44, 62);
}

// ── 모드 탭 ──

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Sync,
    Dynamic,
    Sound,
    Static,
}

impl Tab {
    fn to_mode(self) -> LedMode {
        match self {
            Tab::Sync => LedMode::Sync,
            Tab::Dynamic => LedMode::Dynamic,
            Tab::Sound => LedMode::Sound,
            Tab::Static => LedMode::Static,
        }
    }

    fn from_mode(mode: &LedMode) -> Self {
        match mode {
            LedMode::Sync => Tab::Sync,
            LedMode::Dynamic => Tab::Dynamic,
            LedMode::Sound => Tab::Sound,
            LedMode::Static => Tab::Static,
            LedMode::Off => Tab::Sync,
        }
    }
}

// ── GUI 상태 ──

/// Minimum seconds between config writes while a slider is being dragged.
const SAVE_DEBOUNCE: f64 = 0.12;

struct SettingsGui {
    config: Config,
    config_path: PathBuf,
    status_msg: String,
    status_time: f64,
    tab: Tab,
    led_on: bool,
    autostart: bool,
    show_advanced: bool,
    locale: &'static Locale,
    monitors: Vec<MonitorInfo>,
    dirty: bool,
    last_save: f64,
}

impl SettingsGui {
    fn new(config: Config, config_path: PathBuf, locale: &'static Locale) -> Self {
        let tab = Tab::from_mode(&config.effect.mode);
        let led_on = config.effect.mode != LedMode::Off;
        let autostart = config::is_autostart_enabled();
        Self {
            config,
            config_path,
            status_msg: String::new(),
            status_time: 0.0,
            tab,
            led_on,
            autostart,
            show_advanced: false,
            locale,
            monitors: monitors::enumerate().unwrap_or_default(),
            dirty: false,
            last_save: -10.0,
        }
    }

    /// Mark settings as changed; the write is debounced in `flush`.
    fn save_and_apply(&mut self, _time: f64) {
        self.dirty = true;
    }

    fn flush(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now - self.last_save < SAVE_DEBOUNCE {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
            return;
        }
        self.config.effect.mode = if self.led_on { self.tab.to_mode() } else { LedMode::Off };
        self.config.sanitize();
        match self.config.save(&self.config_path) {
            Ok(()) => self.status_msg = self.locale.applied.into(),
            Err(e) => self.status_msg = format!("{}{}", self.locale.save_failed_prefix, e),
        }
        self.status_time = now;
        self.last_save = now;
        self.dirty = false;
    }
}

// ── eframe 메인 루프 ──

impl eframe::App for SettingsGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let locale = self.locale;

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG).inner_margin(egui::Margin::symmetric(20.0, 16.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    self.draw_header(ui);
                    ui.add_space(14.0);
                    self.draw_brightness_card(ui);
                    ui.add_space(14.0);

                    // ── 모드 선택 ──
                    let labels = [locale.tab_sync, locale.tab_dynamic, locale.tab_sound, locale.tab_static];
                    let current = match self.tab {
                        Tab::Sync => 0,
                        Tab::Dynamic => 1,
                        Tab::Sound => 2,
                        Tab::Static => 3,
                    };
                    let selected = if self.led_on { Some(current) } else { None };
                    if let Some(i) = segmented(ui, &labels, selected) {
                        self.tab = [Tab::Sync, Tab::Dynamic, Tab::Sound, Tab::Static][i];
                        self.led_on = true;
                        self.save_and_apply(0.0);
                    }
                    ui.add_space(14.0);

                    if !self.led_on {
                        card(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space(18.0);
                                ui.label(egui::RichText::new(locale.led_off_msg).size(20.0).color(theme::MUTED));
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new(locale.led_off_hint).color(theme::MUTED));
                                ui.add_space(18.0);
                            });
                        });
                    } else {
                        match self.tab {
                            Tab::Sync => self.draw_sync_tab(ui),
                            Tab::Dynamic => self.draw_dynamic_tab(ui),
                            Tab::Sound => self.draw_sound_tab(ui),
                            Tab::Static => self.draw_static_tab(ui),
                        }
                    }

                    ui.add_space(6.0);
                    let arrow = if self.show_advanced { "▾" } else { "▸" };
                    if ui
                        .add(egui::Label::new(
                            egui::RichText::new(format!("{arrow}  {}", locale.settings)).size(14.0).color(theme::MUTED),
                        ).sense(egui::Sense::click()))
                        .clicked()
                    {
                        self.show_advanced = !self.show_advanced;
                    }
                    if self.show_advanced {
                        ui.add_space(8.0);
                        self.draw_settings_section(ui);
                    }

                    // ── 상태 메시지 ──
                    ui.add_space(10.0);
                    if !self.status_msg.is_empty() {
                        let elapsed = ctx.input(|i| i.time) - self.status_time;
                        if elapsed < 2.5 {
                            let color = if self.status_msg.contains(locale.failed_keyword) { theme::BAD } else { theme::GOOD };
                            ui.label(egui::RichText::new(format!("● {}", self.status_msg)).size(12.0).color(color));
                            ctx.request_repaint_after(std::time::Duration::from_millis(250));
                        }
                    }
                });
            });

        self.flush(ctx);
    }
}

// ── 헤더 / 밝기 ──

impl SettingsGui {
    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // 스펙트럼 링 로고
            let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
            let c = rect.center();
            let segments = 36;
            let (ro, ri) = (15.0f32, 8.5f32);
            for i in 0..segments {
                let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let (r, g, b) = icon_hsl_to_rgb((i as f32 / segments as f32) * 360.0, 1.0, 0.55);
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(c.x + ri * a0.cos(), c.y + ri * a0.sin()),
                        egui::pos2(c.x + ro * a0.cos(), c.y + ro * a0.sin()),
                        egui::pos2(c.x + ro * a1.cos(), c.y + ro * a1.sin()),
                        egui::pos2(c.x + ri * a1.cos(), c.y + ri * a1.sin()),
                    ],
                    Color32::from_rgb(r, g, b),
                    egui::Stroke::NONE,
                ));
            }

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(egui::RichText::new("SYNCFASTERGB").strong().size(21.0).extra_letter_spacing(1.5));
                ui.label(egui::RichText::new("Screen-sync ambient lighting").size(12.0).color(theme::MUTED));
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if toggle_switch(ui, &mut self.led_on).changed() {
                    self.save_and_apply(0.0);
                }
                let (txt, col) = if self.led_on { (self.locale.led_on, theme::GOOD) } else { (self.locale.led_off, theme::MUTED) };
                ui.label(egui::RichText::new(txt).strong().size(13.0).color(col));
            });
        });
    }

    fn draw_brightness_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(self.locale.brightness.trim_end_matches(':')).strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pct = (self.config.sync.brightness as f32 / 255.0 * 100.0).round() as u32;
                    ui.label(egui::RichText::new(format!("{pct}%")).strong().size(26.0).color(theme::ACCENT));
                });
            });
            ui.add_space(6.0);
            let mut pct = self.config.sync.brightness as f32 / 255.0 * 100.0;
            if fat_slider(ui, &mut pct, 2.0, 100.0, theme::CARD_HI, theme::ACCENT) {
                self.config.sync.brightness = (pct / 100.0 * 255.0).round().clamp(5.0, 255.0) as u8;
                self.save_and_apply(0.0);
            }
        });
    }
}

// ── 각 탭 UI ──

impl SettingsGui {
    fn draw_sync_tab(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;

        // 모니터 선택
        section(ui, locale.monitor_label.trim_end_matches(':'), |ui| {
            let selected_text = self.monitor_label_text();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("monitor_combo")
                    .width(ui.available_width() - 44.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for m in &self.monitors {
                            let text = format!(
                                "[{}] {} · {}x{}{}",
                                m.global_index,
                                short_gpu(&m.adapter_name),
                                m.width,
                                m.height,
                                if m.attached { "" } else { " (off)" }
                            );
                            let cfg = &self.config.capture;
                            let is_sel = if cfg.monitor_name.is_empty() {
                                cfg.monitor == m.global_index
                            } else {
                                !m.monitor_id.is_empty() && cfg.monitor_name.eq_ignore_ascii_case(&m.monitor_id)
                            };
                            if ui.selectable_label(is_sel, text).clicked() && m.attached {
                                self.config.capture.monitor = m.global_index;
                                // Remember the display by its hardware ID: stable even when the
                                // laptop panel turning on/off shuffles the output indices.
                                self.config.capture.monitor_name = m.monitor_id.clone();
                                changed = true;
                            }
                        }
                        if changed {
                            self.dirty = true;
                        }
                    });
                if ui.button("⟳").on_hover_text("Rescan displays").clicked() {
                    self.monitors = monitors::enumerate().unwrap_or_default();
                }
            });
        });

        // LED 배치 미리보기 + 방향/엣지
        section(ui, locale.led_layout, |ui| {
            draw_layout_preview(ui, self.config.device.lamps_amount, self.config.sync.edge_number, self.config.sync.reverse);
            ui.add_space(6.0);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(locale.direction).color(theme::MUTED));
                let sel = if self.config.sync.reverse { 1 } else { 0 };
                if let Some(i) = segmented_compact(ui, &[locale.dir_ltr, locale.dir_rtl], sel) {
                    self.config.sync.reverse = i == 1;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(locale.edges).color(theme::MUTED));
                let sel = if self.config.sync.edge_number == 4 { 1 } else { 0 };
                if let Some(i) = segmented_compact(ui, &[locale.edges_3, locale.edges_4], sel) {
                    self.config.sync.edge_number = if i == 1 { 4 } else { 3 };
                    changed = true;
                }
            });
            if changed {
                self.save_and_apply(0.0);
            }
        });

        // 반응성
        section(ui, locale.sync_speed, |ui| {
            let presets = [
                (0u32, locale.speed_fastest),
                (10, locale.speed_fast),
                (20, locale.speed_normal),
                (100, locale.speed_slow),
            ];
            let sel = presets.iter().position(|p| p.0 == self.config.sync.speed);
            let labels: Vec<&str> = presets.iter().map(|p| p.1).collect();
            if let Some(i) = segmented_compact_opt(ui, &labels, sel) {
                self.config.sync.speed = presets[i].0;
                self.save_and_apply(0.0);
            }
            ui.add_space(4.0);
            if ui.checkbox(&mut self.config.sync.smoothing, locale.smoothing).changed() {
                self.save_and_apply(0.0);
            }
            if self.config.sync.smoothing {
                let mut ch = false;
                ch |= ui.add(egui::Slider::new(&mut self.config.sync.attack_ms, 0.0..=400.0).text("Attack ms").suffix(" ms")).changed();
                ch |= ui.add(egui::Slider::new(&mut self.config.sync.decay_ms, 0.0..=1500.0).text("Decay ms").suffix(" ms")).changed();
                if ch {
                    self.save_and_apply(0.0);
                }
            }
        });

        // 색상
        section(ui, locale.color_processing, |ui| {
            let mut ch = false;
            ch |= ui.add(egui::Slider::new(&mut self.config.sync.saturation, 0.0..=3.0).text(locale.saturation)).changed();
            ch |= ui.add(egui::Slider::new(&mut self.config.sync.gamma, 0.3..=3.0).text(locale.gamma)).changed();
            ch |= ui.checkbox(&mut self.config.sync.light_compression, locale.light_compression).changed();
            if self.config.sync.light_compression {
                ch |= ui
                    .add(egui::Slider::new(&mut self.config.sync.compression_limit, 120..=765).text("Power limit"))
                    .changed();
                let (msg, col) = if self.config.sync.compression_limit > 400 {
                    ("High: a long bright scene can overload a laptop USB port and make the strip dim or reset.", theme::WARN)
                } else {
                    ("Caps R+G+B per LED to protect the USB port. Raise it for brighter whites.", theme::MUTED)
                };
                ui.label(egui::RichText::new(msg).size(11.5).color(col));
            }
            if ch {
                self.save_and_apply(0.0);
            }
        });
    }

    fn monitor_label_text(&self) -> String {
        let cfg = &self.config.capture;
        let found = if cfg.monitor_name.is_empty() {
            self.monitors.iter().find(|m| m.global_index == cfg.monitor)
        } else {
            monitors::select(&self.monitors, cfg.monitor, &cfg.monitor_name)
        };
        match found {
            Some(m) => format!("[{}] {} · {}x{}", m.global_index, short_gpu(&m.adapter_name), m.width, m.height),
            None if !cfg.monitor_name.is_empty() => format!("\"{}\" (not connected, waiting)", cfg.monitor_name),
            None => format!("Display #{} (not found)", cfg.monitor),
        }
    }

    fn draw_dynamic_tab(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        ui.label(egui::RichText::new(locale.pattern_hint).size(12.0).color(theme::MUTED));
        ui.add_space(6.0);

        let colors = [
            Color32::from_rgb(255, 70, 90),
            Color32::from_rgb(70, 220, 110),
            Color32::from_rgb(70, 120, 255),
            Color32::from_rgb(255, 170, 40),
            Color32::from_rgb(210, 90, 230),
            Color32::from_rgb(40, 210, 215),
            Color32::from_rgb(255, 220, 60),
        ];
        effect_grid(ui, "dynamic_grid", locale.dynamic_effects, &colors, &mut self.config.effect.dynamic_index, &mut self.dirty);

        ui.add_space(10.0);
        section(ui, locale.effect_speed, |ui| {
            if ui.add(egui::Slider::new(&mut self.config.effect.effect_speed, 1..=100)).changed() {
                self.save_and_apply(0.0);
            }
        });
    }

    fn draw_sound_tab(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;

        section(ui, locale.sound_source, |ui| {
            let sel = if self.config.effect.rhythm_source == RhythmSource::Computer { 1 } else { 0 };
            if let Some(i) = segmented_compact(ui, &[locale.source_controller, locale.source_computer], sel) {
                self.config.effect.rhythm_source = if i == 1 { RhythmSource::Computer } else { RhythmSource::Controller };
                self.save_and_apply(0.0);
            }
        });

        ui.label(egui::RichText::new(locale.pattern_hint).size(12.0).color(theme::MUTED));
        ui.add_space(6.0);

        let colors = [
            Color32::from_rgb(255, 90, 90),
            Color32::from_rgb(90, 255, 120),
            Color32::from_rgb(90, 110, 255),
            Color32::from_rgb(255, 200, 60),
            Color32::from_rgb(200, 90, 255),
            Color32::from_rgb(90, 225, 225),
            Color32::from_rgb(255, 110, 200),
        ];
        effect_grid(ui, "sound_grid", locale.sound_effects, &colors, &mut self.config.effect.sound_index, &mut self.dirty);
    }

    fn draw_static_tab(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let e = &mut self.config.effect;

        // 색상 미리보기
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 64.0), egui::Sense::hover());
        let color = Color32::from_rgb(e.color_r, e.color_g, e.color_b);
        ui.painter().rect_filled(rect, 14.0, color);
        ui.painter().rect_stroke(rect, 14.0, egui::Stroke::new(1.0_f32, theme::BORDER));
        let hex = format!("#{:02X}{:02X}{:02X}", e.color_r, e.color_g, e.color_b);
        let lum = e.color_r as u16 * 3 + e.color_g as u16 * 6 + e.color_b as u16;
        let text_c = if lum > 1280 { Color32::BLACK } else { Color32::WHITE };
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, hex, egui::FontId::monospace(17.0), text_c);
        ui.add_space(10.0);

        let mut color_changed = false;
        section(ui, locale.color_adjust, |ui| {
            for (label, col, val) in [
                ("R", Color32::from_rgb(255, 90, 90), &mut e.color_r),
                ("G", Color32::from_rgb(90, 220, 110), &mut e.color_g),
                ("B", Color32::from_rgb(90, 130, 255), &mut e.color_b),
            ] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label).color(col).strong());
                    if ui.add(egui::Slider::new(val, 0..=255)).changed() {
                        color_changed = true;
                    }
                });
            }
        });

        section(ui, locale.quick_select, |ui| {
            ui.horizontal_wrapped(|ui| {
                for &(name, [r, g, b]) in locale.color_presets {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 28.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 8.0, Color32::from_rgb(r, g, b));
                    let is_sel = e.color_r == r && e.color_g == g && e.color_b == b;
                    if resp.hovered() || is_sel {
                        let sc = if is_sel { theme::ACCENT } else { Color32::WHITE };
                        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(2.0_f32, sc));
                    }
                    if resp.clicked() {
                        e.color_r = r;
                        e.color_g = g;
                        e.color_b = b;
                        color_changed = true;
                    }
                    resp.on_hover_text(name);
                }
            });
        });

        section(ui, locale.static_animation, |ui| {
            let sel = match e.soft_effect {
                SoftEffect::None => 0,
                SoftEffect::Breathe => 1,
                SoftEffect::Rotate => 2,
            };
            if let Some(i) = segmented_compact(ui, &[locale.anim_none, locale.anim_breathe, locale.anim_rotate], sel) {
                e.soft_effect = [SoftEffect::None, SoftEffect::Breathe, SoftEffect::Rotate][i].clone();
                color_changed = true;
            }
            if e.soft_effect != SoftEffect::None
                && ui.add(egui::Slider::new(&mut e.effect_speed, 5..=100).text(locale.speed)).changed()
            {
                color_changed = true;
            }
        });

        if color_changed {
            self.save_and_apply(0.0);
        }
    }

    fn draw_settings_section(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;

        section(ui, locale.device, |ui| {
            let mut ch = false;
            ui.horizontal(|ui| {
                ui.label(locale.led_count);
                ch |= ui.add(egui::DragValue::new(&mut self.config.device.lamps_amount).range(1..=255)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Wire map");
                egui::ComboBox::from_id_salt("wire_map")
                    .selected_text(&self.config.device.wire_map)
                    .show_ui(ui, |ui| {
                        for &wm in WIRE_MAPS {
                            ch |= ui.selectable_value(&mut self.config.device.wire_map, wm.to_string(), wm).changed();
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label(locale.capture_fps);
                ch |= ui.add(egui::DragValue::new(&mut self.config.capture.fps).range(10..=60)).changed();
            });
            ch |= ui.checkbox(&mut self.config.capture.gpu_downscale, "GPU downscale (recommended: lowest CPU use)").changed();
            ui.horizontal(|ui| {
                ui.label("HDR white level");
                ch |= ui
                    .add(egui::DragValue::new(&mut self.config.capture.hdr_sdr_white_nits).range(80.0..=1000.0).suffix(" nits"))
                    .on_hover_text("Only used when Windows HDR is on. Match Settings > Display > HDR > SDR content brightness.")
                    .changed();
            });
            if ch {
                self.save_and_apply(0.0);
            }
        });

        section(ui, locale.app_section, |ui| {
            ui.horizontal(|ui| {
                ui.label(locale.language_label);
                let lang = &mut self.config.app.language;
                let mut changed = false;
                changed |= ui.selectable_value(lang, config::Language::Auto, locale.lang_auto).changed();
                changed |= ui.selectable_value(lang, config::Language::En, "English").changed();
                changed |= ui.selectable_value(lang, config::Language::Ko, "한국어").changed();
                if changed {
                    self.locale = resolve_locale(&self.config.app.language);
                    self.save_and_apply(0.0);
                }
            });
            if ui.checkbox(&mut self.autostart, locale.autostart).changed() {
                if let Err(e) = config::set_autostart(self.autostart) {
                    self.status_msg = format!("{}{}", locale.autostart_failed_prefix, e);
                    self.status_time = ui.input(|i| i.time);
                }
            }
            if ui.checkbox(&mut self.config.app.turn_off_on_sleep, locale.turn_off_on_sleep).changed() {
                self.save_and_apply(0.0);
            }
            if ui.checkbox(&mut self.config.app.turn_off_on_black, locale.turn_off_on_black).changed() {
                self.save_and_apply(0.0);
            }
        });

        if ui
            .add(egui::Button::new(egui::RichText::new(locale.save_settings).size(13.0)).fill(theme::ACCENT.gamma_multiply(0.5)))
            .clicked()
        {
            self.dirty = true;
            self.last_save = -10.0;
        }
    }
}

// ── 위젯 ──

fn short_gpu(name: &str) -> String {
    name.replace("(R)", "").replace("NVIDIA GeForce ", "").replace("Graphics", "").replace("  ", " ").trim().to_string()
}

/// Rounded card container.
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .rounding(egui::Rounding::same(16.0))
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}

/// Titled card.
fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    card(ui, |ui| {
        ui.label(egui::RichText::new(title).strong().size(13.0).color(theme::MUTED));
        ui.add_space(4.0);
        content(ui);
    });
    ui.add_space(10.0);
}

/// iOS-style toggle switch.
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let track = lerp_color(theme::CARD_HI, theme::GOOD.gamma_multiply(0.85), t);
    ui.painter().rect_filled(rect, 13.0, track);
    ui.painter().rect_stroke(rect, 13.0, egui::Stroke::new(1.0_f32, theme::BORDER));
    let cx = egui::lerp((rect.left() + 13.0)..=(rect.right() - 13.0), t);
    ui.painter().circle_filled(egui::pos2(cx, rect.center().y), 9.5, Color32::WHITE);
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Big gradient slider; returns true while the value is being changed by the user.
fn fat_slider(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32, from: Color32, to: Color32) -> bool {
    let h = 30.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click_and_drag());
    let mut changed = false;
    let pad = 12.0;
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + pad, rect.center().y - 6.0),
        egui::pos2(rect.right() - pad, rect.center().y + 6.0),
    );
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let v = min + t * (max - min);
            if (v - *value).abs() > f32::EPSILON {
                *value = v;
                changed = true;
            }
        }
    }
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);

    // 트랙 (어두운 배경 + 채워진 그라디언트)
    ui.painter().rect_filled(track, 6.0, theme::CARD_HI);
    let fill_w = track.width() * t;
    if fill_w > 1.0 {
        let fill = egui::Rect::from_min_size(track.min, egui::vec2(fill_w, track.height()));
        let mut mesh = egui::epaint::Mesh::default();
        let c0 = lerp_color(from, to, 0.35);
        let c1 = lerp_color(from, to, 1.0);
        mesh.colored_vertex(fill.left_top(), c0);
        mesh.colored_vertex(fill.right_top(), c1);
        mesh.colored_vertex(fill.right_bottom(), c1);
        mesh.colored_vertex(fill.left_bottom(), c0);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().circle_filled(fill.left_center() + egui::vec2(3.0, 0.0), 6.0, c0);
    }
    // 노브
    let knob = egui::pos2(track.left() + fill_w, rect.center().y);
    let r = if resp.dragged() || resp.hovered() { 13.0 } else { 11.5 };
    ui.painter().circle_filled(knob, r + 3.0, to.gamma_multiply(0.18));
    ui.painter().circle_filled(knob, r, Color32::WHITE);
    ui.painter().circle_stroke(knob, r, egui::Stroke::new(2.0_f32, to));
    changed
}

/// Full-width segmented control. `selected = None` shows nothing selected.
fn segmented(ui: &mut egui::Ui, labels: &[&str], selected: Option<usize>) -> Option<usize> {
    segmented_inner(ui, labels, selected, ui.available_width(), 40.0, 14.0)
}

fn segmented_compact(ui: &mut egui::Ui, labels: &[&str], selected: usize) -> Option<usize> {
    segmented_compact_opt(ui, labels, Some(selected))
}

fn segmented_compact_opt(ui: &mut egui::Ui, labels: &[&str], selected: Option<usize>) -> Option<usize> {
    let w = ui.available_width();
    segmented_inner(ui, labels, selected, w, 32.0, 12.5)
}

fn segmented_inner(ui: &mut egui::Ui, labels: &[&str], selected: Option<usize>, width: f32, height: f32, font: f32) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, height / 2.0, Color32X::INPUT);
    ui.painter().rect_stroke(rect, height / 2.0, egui::Stroke::new(1.0_f32, theme::BORDER));
    let n = labels.len().max(1);
    let seg_w = (rect.width() - 6.0) / n as f32;
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let seg = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 3.0 + seg_w * i as f32, rect.top() + 3.0),
            egui::vec2(seg_w, height - 6.0),
        );
        let resp = ui.interact(seg, ui.id().with(("seg", label, i)), egui::Sense::click());
        let is_sel = selected == Some(i);
        if is_sel {
            ui.painter().rect_filled(seg, (height - 6.0) / 2.0, theme::ACCENT);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, (height - 6.0) / 2.0, Color32X::HOVER);
        }
        let col = if is_sel { Color32::from_rgb(8, 14, 22) } else { theme::MUTED };
        ui.painter().text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(font),
            if is_sel { col } else if resp.hovered() { theme::TEXT } else { col },
        );
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Monitor outline with the LED strip drawn as dots in wire order (red -> violet),
/// so edge selection and direction are visible at a glance.
fn draw_layout_preview(ui: &mut egui::Ui, lamps: u32, edges: u8, reverse: bool) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 150.0), egui::Sense::hover());
    let screen = egui::Rect::from_center_size(rect.center() + egui::vec2(0.0, 2.0), egui::vec2((w * 0.62).min(300.0), 96.0));
    ui.painter().rect_filled(screen, 6.0, Color32X::INPUT);
    ui.painter().rect_stroke(screen, 6.0, egui::Stroke::new(1.5_f32, theme::BORDER));
    let stand_y = screen.bottom();
    ui.painter().line_segment(
        [egui::pos2(screen.center().x - 26.0, stand_y + 12.0), egui::pos2(screen.center().x + 26.0, stand_y + 12.0)],
        egui::Stroke::new(3.0_f32, theme::BORDER),
    );

    // 경로: left(bottom->top) -> top(left->right) -> right(top->bottom) [-> bottom(right->left)]
    let m = 9.0;
    let tl = egui::pos2(screen.left() - m, screen.top() - m);
    let tr = egui::pos2(screen.right() + m, screen.top() - m);
    let br = egui::pos2(screen.right() + m, screen.bottom() + m);
    let bl = egui::pos2(screen.left() - m, screen.bottom() + m);
    let mut path = vec![bl, tl, tr, br];
    if edges >= 4 {
        path.push(bl);
    }
    let seg_len: Vec<f32> = path.windows(2).map(|p| (p[1] - p[0]).length()).collect();
    let total: f32 = seg_len.iter().sum();
    let n = (lamps as usize).clamp(2, 255);
    for i in 0..n {
        let mut t = (i as f32 + 0.5) / n as f32;
        let hue_t = t;
        if reverse {
            t = 1.0 - t;
        }
        let mut d = t * total;
        let mut pos = path[0];
        for (k, l) in seg_len.iter().enumerate() {
            if d <= *l {
                pos = path[k] + (path[k + 1] - path[k]) * (d / l.max(0.001));
                break;
            }
            d -= l;
        }
        let (r, g, b) = icon_hsl_to_rgb(hue_t * 300.0, 0.9, 0.58);
        ui.painter().circle_filled(pos, 2.6, Color32::from_rgb(r, g, b));
    }
    ui.painter().text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        format!("{} LEDs", lamps),
        egui::FontId::proportional(12.0),
        theme::MUTED,
    );
}

/// Grid of selectable effect tiles; returns nothing, updates `index` and `dirty`.
fn effect_grid(ui: &mut egui::Ui, id: &str, names: &[&str], colors: &[Color32], index: &mut u8, dirty: &mut bool) {
    let cols = 3;
    egui::Grid::new(id).num_columns(cols).spacing([10.0, 10.0]).show(ui, |ui| {
        for (i, name) in names.iter().enumerate() {
            let selected = *index == i as u8;
            let color = colors[i % colors.len()];
            if effect_button(ui, name, color, selected) {
                *index = i as u8;
                *dirty = true;
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
}

fn effect_button(ui: &mut egui::Ui, name: &str, color: Color32, selected: bool) -> bool {
    let w = ((ui.available_width() - 20.0) / 3.0).max(110.0).min(170.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 54.0), egui::Sense::click());

    let bg = if selected {
        color.gamma_multiply(0.22)
    } else if resp.hovered() {
        theme::CARD_HI
    } else {
        theme::CARD
    };
    ui.painter().rect_filled(rect, 12.0, bg);
    ui.painter().rect_stroke(rect, 12.0, egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, if selected { color } else { theme::BORDER }));
    ui.painter().circle_filled(rect.left_center() + egui::vec2(16.0, 0.0), 5.0, color);
    ui.painter().text(
        rect.left_center() + egui::vec2(30.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        if selected { theme::TEXT } else { theme::MUTED },
    );
    resp.clicked()
}
