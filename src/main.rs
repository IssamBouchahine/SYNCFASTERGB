// 콘솔 창 숨기기 (--console 플래그로 표시 가능)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod capture;
mod foreground;
mod ipc;
mod single;
mod color;
mod config;
mod device;
mod gui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use capture::dxgi::{CaptureError, ScreenCapture};
use color::extractor::ColorExtractor;
use color::pipeline;
use config::{Config, LedMode, RhythmSource, SoftEffect};
use device::protocol::WireMap;
use device::serial::DeviceConnection;

fn main() {
    // --console 플래그: 릴리즈에서도 콘솔 표시
    if std::env::args().any(|a| a == "--console" || a == "--list-monitors") {
        unsafe { attach_console(); }
    }

    if std::env::args().any(|a| a == "--list-monitors") {
        capture::monitors::print_list();
        return;
    }

    init_logging();

    // panic=abort + 콘솔 없음 조합에서 패닉이 흔적 없이 사라지는 것 방지
    std::panic::set_hook(Box::new(|info| {
        log::error!("패닉: {}", info);
    }));

    if std::env::args().any(|a| a == "--settings") {
        gui::run_settings_window();
        return;
    }

    // One tray app per session. Starting it again (taskbar, shortcut, autostart) just shows
    // the settings window of the running one. After a self-restart the old process may still
    // be shutting down, so wait for it.
    let wait = if restart_count_from_args() > 0 { Duration::from_secs(10) } else { Duration::ZERO };
    let _instance = match single::acquire_with_retry(single::APP_MUTEX, wait) {
        Some(g) => g,
        None => {
            gui::open_settings();
            return;
        }
    };

    lower_process_priority();

    log::info!("SYNCFASTERGB starting");

    let locale = gui::detect_locale();
    let config = Config::load_or_default();
    log::info!("설정: {:?}", config);

    let running = Arc::new(AtomicBool::new(true));
    let active = Arc::new(AtomicBool::new(true));
    let config_version = Arc::new(AtomicU32::new(0));
    let monitor_off = Arc::new(AtomicBool::new(false));
    let screen_black = Arc::new(AtomicBool::new(false));
    // 절전·종료 전환 핸드셰이크: 리스너가 요청(suspend_pending) → sender 가 LED 끄고 핸들 해제(device_released)
    let suspend_pending = Arc::new(AtomicBool::new(false));
    let device_released = Arc::new(AtomicBool::new(false));
    let link_status = Arc::new(AtomicU8::new(LINK_SEARCHING));

    spawn_shutdown_listener(running.clone(), monitor_off.clone(), suspend_pending.clone(), device_released.clone());

    let (tx, rx) = mpsc::sync_channel::<Vec<(u8, u8, u8)>>(2);

    let capture_thread = {
        let running = running.clone();
        let active = active.clone();
        let config_version = config_version.clone();
        let config = config.clone();
        let screen_black = screen_black.clone();
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || capture_loop(config, running, active, config_version, tx, screen_black))
            .expect("캡처 스레드 생성 실패")
    };

    let send_thread = {
        let running = running.clone();
        let config_version = config_version.clone();
        let config = config.clone();
        let monitor_off = monitor_off.clone();
        let screen_black = screen_black.clone();
        let suspend_pending = suspend_pending.clone();
        let device_released = device_released.clone();
        let link_status = link_status.clone();
        std::thread::Builder::new()
            .name("sender".into())
            .spawn(move || sender_loop(config, running, config_version, rx, monitor_off, screen_black, suspend_pending, device_released, link_status))
            .expect("전송 스레드 생성 실패")
    };

    run_tray(running.clone(), active, config_version, locale, link_status);

    // 트레이 종료 후 스레드 정리 (정상 종료 경로)
    running.store(false, Ordering::SeqCst);
    let _ = capture_thread.join();
    let _ = send_thread.join();
    log::info!("SYNCFASTERGB stopped");
}

/// 로거 초기화. windows_subsystem="windows" 빌드에서 stderr는 유실되므로
/// exe 옆 syncrgb.log에 기록한다 (--console 시에는 기존대로 콘솔 출력).
fn init_logging() {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder.format_timestamp_millis();

    if !std::env::args().any(|a| a == "--console") {
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            let path = dir.join("syncfastergb.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
                let _ = std::fs::rename(&path, dir.join("syncfastergb.log.old"));
            }
            if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
        }
    }
    builder.init();
}

/// 릴리즈 빌드에서 콘솔 붙이기
#[allow(unused_unsafe)]
unsafe fn attach_console() {
    #[cfg(not(debug_assertions))]
    {
        use windows::Win32::System::Console::{AttachConsole, AllocConsole, ATTACH_PARENT_PROCESS};
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

fn capture_loop(
    config: Config,
    running: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    config_version: Arc<AtomicU32>,
    tx: mpsc::SyncSender<Vec<(u8, u8, u8)>>,
    screen_black: Arc<AtomicBool>,
) {
    let mut frame_interval = Duration::from_millis(1000 / config.capture.fps as u64);
    // Game mode: while a full-screen app is focused, capture at a lower rate.
    let mut base_interval = frame_interval;
    let mut game_interval = Duration::from_millis(1000 / config.capture.game_fps.max(1) as u64);
    let mut game_mode = false;
    let mut last_game_check = Instant::now() - Duration::from_secs(10);
    let mut turn_off_on_black = config.app.turn_off_on_black;

    // The monitor may not exist yet (unplugged, asleep, still enumerating): keep trying
    // with the freshest config until it appears or the app quits.
    let mut capture_cfg = config.capture.clone();
    let mut waiting_logged = false;
    let first = loop {
        if !running.load(Ordering::Relaxed) {
            return;
        }
        capture::dxgi::attach_input_desktop();
        match ScreenCapture::new(&capture_cfg) {
            Ok(c) => break c,
            Err(e) => {
                if !waiting_logged {
                    log::warn!("screen capture not available yet ({}): retrying every 2 s", e);
                    waiting_logged = true;
                }
                for _ in 0..10 {
                    std::thread::sleep(Duration::from_millis(200));
                    if !running.load(Ordering::Relaxed) {
                        return;
                    }
                }
                capture_cfg = Config::load_or_default().capture;
            }
        }
    };

    let mut extractor = ColorExtractor::new(config.device.lamps_amount, &config.sync);

    log::info!("capture thread started ({} fps, {}x{})", config.capture.fps, first.width, first.height);
    let mut mon_rect: Option<foreground::Rect4> = Some((first.origin.0, first.origin.1, first.origin.0 + first.width as i32, first.origin.1 + first.height as i32));
    let mut capturer: Option<ScreenCapture> = Some(first);
    let mut local_version = 0u32;
    // 검정 화면 연속 프레임 카운터 (1초 이상 검정이면 플래그 설정)
    let mut black_frame_count = 0u32;
    let mut black_threshold_frames = config.capture.fps; // ~1초
    // 연속 에러 카운터 (누적 시 DXGI 재초기화)
    let mut consecutive_errors = 0u32;
    // 마지막 전송 (정지 화면에서 스무딩 수렴 + keepalive)
    let mut last_sent: Option<Vec<(u8, u8, u8)>> = None;
    let mut last_send_at = Instant::now();
    const KEEPALIVE: Duration = Duration::from_millis(500);
    // 성능 통계 (로컬 로그 전용)
    let mut stat_cap = Duration::ZERO;
    let mut stat_ext = Duration::ZERO;
    let mut stat_frames = 0u32;
    let mut stat_since = Instant::now();
    // 접근 손실(전체화면 독점 게임·보안 데스크톱)은 1초마다 반복된다 — 시작·복구만 남긴다
    // (2026-10-14 로그: 5시간 동안 19,355회 반복돼 7.5MB)
    let mut access_lost_since: Option<Instant> = None;
    let mut access_lost_reinits = 0u32;
    // recovery bookkeeping
    let mut frames_since_create = 1u32; // the first capturer already exists
    let mut rapid_losses = 0u32;
    let mut create_failures = 0u32;
    let mut secure_desktop_logged = false;
    let mut dead_since: Option<Instant> = None; // capture unavailable for a reason other than the secure desktop
    let restarts = restart_count_from_args();
    const WATCHDOG: Duration = Duration::from_secs(45);
    const MAX_RESTARTS: u32 = 3;

    while running.load(Ordering::Relaxed) {
        let frame_start = Instant::now();

        let current_version = config_version.load(Ordering::Relaxed);
        if current_version != local_version {
            local_version = current_version;
            let c = Config::load_or_default();
            base_interval = Duration::from_millis(1000 / c.capture.fps as u64);
            game_interval = Duration::from_millis(1000 / c.capture.game_fps.max(1) as u64);
            frame_interval = if game_mode { base_interval.max(game_interval) } else { base_interval };
            turn_off_on_black = c.app.turn_off_on_black;
            black_threshold_frames = c.capture.fps;
            extractor.update_config(c.device.lamps_amount, &c.sync);
            if c.capture != capture_cfg {
                // monitor / GPU path / HDR level changed in the settings window
                capture_cfg = c.capture.clone();
                log::info!("capture settings changed: recreating capture");
                capturer = None; // recreated below with the new settings
            }
        }

        if !active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        // Once a second: is a full-screen app (a game) focused on the captured monitor?
        if last_game_check.elapsed() >= Duration::from_secs(1) {
            last_game_check = Instant::now();
            if let Some(rect) = mon_rect {
                let now_game = foreground::fullscreen_app_covering(rect);
                if now_game != game_mode {
                    game_mode = now_game;
                    frame_interval = if game_mode { base_interval.max(game_interval) } else { base_interval };
                    log::info!(
                        "full-screen app {}: capturing at {} fps",
                        if game_mode { "focused" } else { "closed" },
                        1000 / frame_interval.as_millis().max(1)
                    );
                }
            }
        }
        if game_mode {
            // A full-screen game owns the screen: never treat a lost capture as a fault.
            dead_since = None;
        }

        // (Re)create the capturer when there is none. Everything old has been dropped first.
        if capturer.is_none() {
            if !capture::dxgi::attach_input_desktop() {
                if !secure_desktop_logged {
                    log::info!("secure desktop active (UAC prompt or lock screen): capture paused");
                    secure_desktop_logged = true;
                }
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            if secure_desktop_logged {
                log::info!("secure desktop closed: resuming capture");
                secure_desktop_logged = false;
            }
            match ScreenCapture::new(&capture_cfg) {
                Ok(c) => {
                    log::info!("capture re-created ({}x{})", c.width, c.height);
                    mon_rect = Some((c.origin.0, c.origin.1, c.origin.0 + c.width as i32, c.origin.1 + c.height as i32));
                    capturer = Some(c);
                    frames_since_create = 0;
                    create_failures = 0;
                }
                Err(e) => {
                    create_failures += 1;
                    dead_since.get_or_insert_with(Instant::now);
                    if create_failures == 1 || create_failures.is_multiple_of(60) {
                        log::warn!("cannot re-create capture yet ({}); retrying (attempt {})", e, create_failures);
                    }
                    if !game_mode && dead_since.is_some_and(|t| t.elapsed() >= WATCHDOG) && restarts < MAX_RESTARTS {
                        log::error!("capture dead for {} s: restarting the app", WATCHDOG.as_secs());
                        if relaunch_self(restarts) {
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(lost_backoff_ms(create_failures.min(3))));
                    continue;
                }
            }
        }
        let Some(cap) = capturer.as_mut() else { continue };

        let t_cap = Instant::now();
        let cap_result = cap.capture_frame(frame_interval.as_millis().max(1) as u32);
        if cap_result.is_ok() {
            stat_cap += t_cap.elapsed();
            stat_frames += 1;
        }
        match cap_result {
            Ok(frame) => {
                consecutive_errors = 0;
                frames_since_create = frames_since_create.saturating_add(1);
                rapid_losses = 0;
                dead_since = None;
                if let Some(since) = access_lost_since.take() {
                    log::info!("screen capture recovered after {} s ({} re-creations)",
                        since.elapsed().as_secs(), access_lost_reinits);
                    access_lost_reinits = 0;
                }
                let t_ext = Instant::now();
                let colors = extractor.extract(&frame.data, frame.pitch, frame.width, frame.height);
                stat_ext += t_ext.elapsed();

                // 검정 화면 감지: 모든 LED 색상이 거의 검정인지 확인
                const BLACK_SUM_THRESHOLD: u16 = 15;
                if turn_off_on_black {
                    let is_black = colors.iter().all(|&(r, g, b)| {
                        (r as u16 + g as u16 + b as u16) < BLACK_SUM_THRESHOLD
                    });
                    if is_black {
                        black_frame_count = black_frame_count.saturating_add(1);
                    } else {
                        if black_frame_count >= black_threshold_frames {
                            log::info!("검정 화면 해제 — LED 복원");
                        }
                        black_frame_count = 0;
                    }
                    let was_black = screen_black.load(Ordering::Relaxed);
                    let now_black = black_frame_count >= black_threshold_frames;
                    if now_black != was_black {
                        screen_black.store(now_black, Ordering::SeqCst);
                        if now_black {
                            log::info!("검정 화면 감지 — LED 끄기");
                        }
                    }
                } else {
                    if screen_black.load(Ordering::Relaxed) {
                        screen_black.store(false, Ordering::SeqCst);
                    }
                    black_frame_count = 0;
                }

                // Identical colors are not re-sent (saves USB traffic and CPU); a keepalive
                // still goes out every 500 ms so the strip never goes quiet.
                if last_sent.as_ref() != Some(&colors) || last_send_at.elapsed() >= KEEPALIVE {
                    last_sent = Some(colors.clone());
                    last_send_at = Instant::now();
                    let _ = tx.try_send(colors);
                }
            }
            Err(CaptureError::Timeout) => {
                dead_since = None; // a timeout just means the screen is static
                // Static screen: keep smoothing converging, and send a keepalive.
                if let Some(f) = cap.last_frame() {
                    let colors = extractor.extract(&f.data, f.pitch, f.width, f.height);
                    if last_sent.as_ref() != Some(&colors) || last_send_at.elapsed() >= KEEPALIVE {
                        last_sent = Some(colors.clone());
                        last_send_at = Instant::now();
                        let _ = tx.try_send(colors);
                    }
                }
            }
            Err(CaptureError::AccessLost) => {
                if access_lost_since.is_none() {
                    log::warn!("screen capture access lost: re-creating it");
                    access_lost_since = Some(Instant::now());
                }
                dead_since.get_or_insert_with(Instant::now);
                access_lost_reinits += 1;
                if frames_since_create == 0 {
                    rapid_losses += 1;
                    if rapid_losses == 1 || rapid_losses.is_multiple_of(30) {
                        log::warn!("capture lost again right after re-creation ({} times)", rapid_losses);
                    }
                } else {
                    rapid_losses = 0;
                }
                // Drop every GPU object BEFORE building new ones.
                capturer = None;
                std::thread::sleep(Duration::from_millis(lost_backoff_ms(rapid_losses)));
                if !game_mode && dead_since.is_some_and(|t| t.elapsed() >= WATCHDOG) && restarts < MAX_RESTARTS
                    && capture::dxgi::attach_input_desktop()
                {
                    log::error!("capture dead for {} s: restarting the app", WATCHDOG.as_secs());
                    if relaunch_self(restarts) {
                        running.store(false, Ordering::SeqCst);
                        return;
                    }
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors == 1 || consecutive_errors.is_multiple_of(30) {
                    log::error!("capture error: {} ({} in a row)", e, consecutive_errors);
                }
                if consecutive_errors >= 30 {
                    log::warn!("{} capture errors in a row: re-creating capture", consecutive_errors);
                    consecutive_errors = 0;
                    capturer = None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        if stat_since.elapsed() >= Duration::from_secs(10) {
            if stat_frames > 0 {
                log::debug!(
                    "perf: {} frames/10s, capture {:.2} ms/frame, extract {:.3} ms/frame",
                    stat_frames,
                    stat_cap.as_secs_f64() * 1000.0 / stat_frames as f64,
                    stat_ext.as_secs_f64() * 1000.0 / stat_frames as f64,
                );
            }
            stat_cap = Duration::ZERO;
            stat_ext = Duration::ZERO;
            stat_frames = 0;
            stat_since = Instant::now();
        }

        let elapsed = frame_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }
}

/// Run the engine below normal priority: a game's threads always win any CPU contention.
fn lower_process_priority() {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS};
    // SAFETY: the pseudo-handle from GetCurrentProcess is always valid for this process.
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

/// How long to wait after the screen capture was lost. Normal losses (fullscreen app, mode
/// change) retry quickly; if capture is lost again right after being recreated, back off up
/// to 5 s so a stuck state never spins.
fn lost_backoff_ms(rapid_losses: u32) -> u64 {
    if rapid_losses == 0 {
        500
    } else {
        (rapid_losses.min(5) as u64) * 1000
    }
}

/// Start a fresh copy of this program (same arguments, bumped restart counter). Last-resort
/// recovery when screen capture stays dead; capped so a persistent problem cannot loop.
fn relaunch_self(restarts: u32) -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--restart-count="))
        .collect();
    std::process::Command::new(exe)
        .args(args)
        .arg(format!("--restart-count={}", restarts + 1))
        .spawn()
        .is_ok()
}

fn restart_count_from_args() -> u32 {
    std::env::args()
        .find_map(|a| a.strip_prefix("--restart-count=").and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

#[cfg(test)]
mod recovery_tests {
    use super::{effective_power_limit, lost_backoff_ms};

    #[test]
    fn derate_applies_even_without_a_configured_limit() {
        assert_eq!(effective_power_limit(Some(0.4), 1.0), Some(0.4));
        assert_eq!(effective_power_limit(None, 1.0), None);
        assert_eq!(effective_power_limit(None, 0.5), Some(0.5));
        assert!((effective_power_limit(Some(0.4), 0.5).unwrap() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(lost_backoff_ms(0), 500);
        assert_eq!(lost_backoff_ms(1), 1000);
        assert_eq!(lost_backoff_ms(3), 3000);
        assert_eq!(lost_backoff_ms(5), 5000);
        assert_eq!(lost_backoff_ms(500), 5000);
    }
}

/// Strip link state shown by the tray icon/tooltip.
const LINK_SEARCHING: u8 = 0;
const LINK_UP: u8 = 1;
const LINK_HUNG: u8 = 2;

/// 디바이스 링크 상태 (로그는 상태가 바뀔 때만 남긴다 — 2026-10-15 로그는 49초마다
/// 5줄씩 재연결 스팸으로 1.5일 만에 회전돼 절전·복귀 시점의 증거가 날아갔다)
#[derive(Debug, Clone, Copy, PartialEq)]
enum LinkState {
    /// 아직 연결 안 됨 / 버스에 없음
    Absent,
    /// 열거는 되지만 OUT 리포트를 NAK (hid_write 타임아웃) — 펌웨어 먹통
    Hung,
    /// 연결됐고 getDeviceInfo 응답도 받음
    Up,
    /// 연결됐지만 getDeviceInfo 무응답 — 응답 경로만 죽은 반먹통(스트리밍은 가능)
    Deaf,
}

/// After a failed write: drop the connection and schedule the next attempt.
/// No process is ever spawned here: a hung firmware (OUT NAK) can only be cured by
/// replugging the strip, so we say so once and keep polling gently.
fn handle_write_failure(
    e: &dyn std::error::Error,
    conn: &mut Option<DeviceConnection>,
    link: &mut LinkState,
    next_connect_at: &mut Instant,
) {
    use device::serial::{classify_error, WriteFailure};
    *conn = None;
    match classify_error(e) {
        WriteFailure::Hung => {
            if *link != LinkState::Hung {
                log::error!("strip firmware stopped accepting data ({}): unplug and replug the strip", e);
            }
            *link = LinkState::Hung;
            *next_connect_at = Instant::now() + Duration::from_secs(5);
        }
        WriteFailure::Gone => {
            if *link != LinkState::Absent {
                log::warn!("strip disconnected ({}): waiting for it to come back", e);
            }
            *link = LinkState::Absent;
            *next_connect_at = Instant::now() + Duration::from_secs(1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sender_loop(
    config: Config,
    running: Arc<AtomicBool>,
    config_version: Arc<AtomicU32>,
    rx: mpsc::Receiver<Vec<(u8, u8, u8)>>,
    monitor_off: Arc<AtomicBool>,
    screen_black: Arc<AtomicBool>,
    suspend_pending: Arc<AtomicBool>,
    device_released: Arc<AtomicBool>,
    link_status: Arc<AtomicU8>,
) {
    let mut wire_map = WireMap::from_str(&config.device.wire_map);
    let mut send_interval = Duration::from_millis(config.sync.interval_ms());
    let mut brightness = config.sync.brightness;
    let mut grade_params = grade_from_config(&config.sync);
    let mut power_limit = power_from_config(&config.sync);
    let mut led_share = ipc::LedShare::create();
    // Brownout guard: if the strip keeps dropping off the bus while drawing a lot of power,
    // that looks like the USB port tripping its overcurrent protection. Lower our own
    // budget for the rest of the session instead of asking for the same power again.
    let mut derate = 1.0f32;
    let mut gone_events: Vec<Instant> = Vec::new();
    // Hardware-effect brightness is debounced: some firmware stores it in flash.
    let mut fw_brightness_due: Option<Instant> = None;
    let mut current_mode = config.effect.mode.clone();
    let mut effect_cfg = config.effect.clone();
    let mut turn_off_on_sleep = config.app.turn_off_on_sleep;
    let mut turn_off_on_black = config.app.turn_off_on_black;
    let mut lamps_amount = config.device.lamps_amount;

    let mut conn: Option<DeviceConnection> = None;
    let mut link = LinkState::Absent;
    let mut next_connect_at = Instant::now();
    let mut last_hung_reminder = Instant::now();

    let mut local_version = 0u32;
    let mut send_count = 0u64;
    let mut blanked_by_screen = false;

    // 컴퓨터 리듬용 오디오 미터 (필요 시 초기화)
    let mut audio_meter: Option<audio::AudioMeter> = None;
    // 소프트웨어 효과 타이머
    let mut soft_tick: f64 = 0.0;

    while running.load(Ordering::Relaxed) {
        let send_start = Instant::now();

        link_status.store(
            match (&link, conn.is_some()) {
                (LinkState::Hung, _) => LINK_HUNG,
                (LinkState::Up, true) | (LinkState::Deaf, true) => LINK_UP,
                _ => LINK_SEARCHING,
            },
            Ordering::Relaxed,
        );

        // ── 절전 진입 / 세션 종료 예고: LED 끄고 핸들을 놓은 뒤 복귀까지 대기 ──
        // 디바이스는 5VSB로 PC가 꺼져도 전원이 유지돼 마지막 상태로 계속 빛난다.
        // 전환 중에 먹통이 되더라도 "꺼진 상태"로 멈추도록, 전환 전에 반드시 끈다.
        if suspend_pending.load(Ordering::SeqCst) {
            if let Some(c) = conn.take() {
                log::info!("절전/종료 전환 — LED 끄고 HID 핸들 해제");
                if let Err(e) = c.turn_off() {
                    log::warn!("전환 전 LED 끄기 실패: {}", e);
                }
                blanked_by_screen = false;
                link = LinkState::Absent;
            }
            device_released.store(true, Ordering::SeqCst);
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(100));
            // 복귀 직후 USB 재열거가 끝날 시간을 준다
            next_connect_at = Instant::now() + Duration::from_secs(2);
            continue;
        }

        // ── 연결 보장 (백오프) ──
        if conn.is_none() {
            if Instant::now() < next_connect_at {
                while rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            match DeviceConnection::connect(&config.device.com_port) {
                Ok(mut c) => {
                    let responded = c.init_device().unwrap_or(false);
                    if let Err(e) = c.set_brightness(firmware_brightness(&current_mode, brightness)) { log::warn!("밝기 실패: {}", e); }
                    if responded {
                        log::info!("strip connected (MAC={:02x?})", c.mac);
                        link = LinkState::Up;
                    } else {
                        // Some firmware revisions never answer the info request but stream fine.
                        log::warn!("strip connected (no info reply; streaming anyway)");
                        link = LinkState::Deaf;
                    }
                    apply_mode(&c, &effect_cfg, lamps_amount);
                    device_released.store(false, Ordering::SeqCst);
                    blanked_by_screen = false;
                    conn = Some(c);
                }
                Err(e) => {
                    if link == LinkState::Hung {
                        if last_hung_reminder.elapsed() >= Duration::from_secs(600) {
                            last_hung_reminder = Instant::now();
                            log::error!("strip still not responding: unplug and replug it");
                        }
                    } else {
                        link = LinkState::Absent;
                        static ABSENT_LOGGED: AtomicBool = AtomicBool::new(false);
                        if !ABSENT_LOGGED.swap(true, Ordering::Relaxed) {
                            log::warn!("no strip found yet ({}): checking every second, quietly", e);
                        }
                    }
                    next_connect_at = Instant::now() + Duration::from_secs(1);
                    continue;
                }
            }
        }

        let cv = config_version.load(Ordering::Relaxed);
        if cv != local_version {
            local_version = cv;
            let c = Config::load_or_default();
            wire_map = WireMap::from_str(&c.device.wire_map);
            send_interval = Duration::from_millis(c.sync.interval_ms());
            grade_params = grade_from_config(&c.sync);
            power_limit = power_from_config(&c.sync);

            // Sync mode scales brightness in software (smooth, no firmware command per change);
            // the firmware level only matters for the strip's built-in effects.
            if brightness != c.sync.brightness {
                brightness = c.sync.brightness;
                if current_mode != LedMode::Sync {
                    fw_brightness_due = Some(Instant::now() + Duration::from_millis(600));
                }
            }

            lamps_amount = c.device.lamps_amount;

            turn_off_on_sleep = c.app.turn_off_on_sleep;
            turn_off_on_black = c.app.turn_off_on_black;

            if current_mode != c.effect.mode || effect_cfg_changed(&effect_cfg, &c.effect) {
                current_mode = c.effect.mode.clone();
                effect_cfg = c.effect.clone();
                if let Some(d) = conn.as_ref() {
                    if !apply_mode(d, &effect_cfg, lamps_amount) {
                        // 모드 적용 실패 → 다음 루프에서 재연결
                        log::warn!("모드 적용 실패 — 디바이스 재연결");
                        conn = None;
                        next_connect_at = Instant::now() + Duration::from_secs(1);
                        continue;
                    }
                    let _ = d.set_brightness(firmware_brightness(&current_mode, brightness));
                }
            }
        }

        // 모니터 절전 또는 검정 화면 시 LED 끄기
        let should_blank = (turn_off_on_sleep && monitor_off.load(Ordering::Relaxed))
            || (turn_off_on_black && screen_black.load(Ordering::Relaxed));
        if should_blank {
            if !blanked_by_screen {
                log::info!("화면 꺼짐 감지 — LED 끄기");
                if let Some(d) = conn.as_ref() {
                    if let Err(e) = d.turn_off() {
                        handle_write_failure(e.as_ref(), &mut conn, &mut link, &mut next_connect_at);
                        continue;
                    }
                }
                blanked_by_screen = true;
            }
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(200));
            continue;
        } else if blanked_by_screen {
            log::info!("화면 복귀 — LED 복원");
            blanked_by_screen = false;
            if let Some(d) = conn.as_ref() {
                let _ = d.set_brightness(firmware_brightness(&current_mode, brightness));
                apply_mode(d, &effect_cfg, lamps_amount);
            }
        }

        // 이 아래는 연결이 있어야 한다
        let d = match conn.as_ref() {
            Some(d) => d,
            None => continue,
        };

        if fw_brightness_due.is_some_and(|due| Instant::now() >= due) {
            fw_brightness_due = None;
            let _ = d.set_brightness(firmware_brightness(&current_mode, brightness));
        }

        if current_mode == LedMode::Sync {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(colors) => {
                    let mut graded: Vec<[u8; 3]> = colors
                        .iter()
                        .map(|&(r, g, b)| pipeline::grade([r, g, b], &grade_params))
                        .collect();
                    if let Some(fraction) = effective_power_limit(power_limit, derate) {
                        pipeline::limit_total_power(&mut graded, fraction);
                    }
                    let last_draw = pipeline::power_fraction(&graded);
                    if let Some(share) = led_share.as_mut() {
                        share.publish(&graded);
                    }
                    let mut color_data = Vec::with_capacity(graded.len() * 5);
                    for (i, g) in graded.iter().enumerate() {
                        let wired = wire_map.apply(g[0], g[1], g[2]);
                        let idx = (i + 1) as u8;
                        color_data.push(idx);
                        color_data.push(wired[0]);
                        color_data.push(wired[1]);
                        color_data.push(wired[2]);
                        color_data.push(idx);
                    }

                    if send_count == 0 {
                        log::info!("첫 SC 패킷: {}LED, {}바이트", color_data.len() / 5, color_data.len());
                    }
                    send_count += 1;

                    if let Err(e) = d.set_sync_screen(&color_data) {
                        if device::serial::classify_error(e.as_ref()) == device::serial::WriteFailure::Gone && last_draw > 0.3 {
                            let now = Instant::now();
                            gone_events.retain(|t| now.duration_since(*t) < Duration::from_secs(120));
                            gone_events.push(now);
                            if gone_events.len() >= 2 && derate > 0.3 {
                                derate = (derate * 0.75).max(0.3);
                                gone_events.clear();
                                log::warn!(
                                    "strip dropped off the bus repeatedly while drawing {:.0}% power: lowering the power budget to {:.0}% of the configured value (likely a USB port overcurrent trip)",
                                    last_draw * 100.0,
                                    derate * 100.0
                                );
                            }
                        }
                        handle_write_failure(e.as_ref(), &mut conn, &mut link, &mut next_connect_at);
                        continue;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else if current_mode == LedMode::Sound && effect_cfg.rhythm_source == RhythmSource::Computer {
            // 컴퓨터 리듬: 오디오 볼륨 → setComputerRhythm 반복
            if audio_meter.is_none() {
                audio_meter = audio::AudioMeter::new().ok();
                if audio_meter.is_none() {
                    log::warn!("오디오 미터 초기화 실패");
                }
            }
            if let Some(ref mut meter) = audio_meter {
                let vol = meter.peak_volume();
                if let Err(e) = d.set_computer_rhythm(effect_cfg.sound_index, vol) {
                    handle_write_failure(e.as_ref(), &mut conn, &mut link, &mut next_connect_at);
                    continue;
                }
            }
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(40));
            continue;
        } else if current_mode == LedMode::Static && effect_cfg.soft_effect != SoftEffect::None {
            // 소프트웨어 동적 효과 (단색 숨쉬기/회전)
            soft_tick += 0.05;
            let speed_factor = effect_cfg.effect_speed as f64 / 50.0;
            let t = soft_tick * speed_factor;

            let result = match effect_cfg.soft_effect {
                SoftEffect::Breathe => {
                    // 사인파로 밝기 변화 (0.05 ~ 1.0)
                    let bright = ((t.sin() + 1.0) / 2.0 * 0.95 + 0.05) as f32;
                    let r = (effect_cfg.color_r as f32 * bright) as u8;
                    let g = (effect_cfg.color_g as f32 * bright) as u8;
                    let b = (effect_cfg.color_b as f32 * bright) as u8;
                    // setSectionLED(0x86)는 단발성 색 변경 명령 — 33Hz 연사 시 펌웨어 먹통 위험.
                    // Rotate와 동일하게 스트리밍 전용 SC(setSyncScreen) 경로로 전송한다.
                    let mapped = wire_map.apply(r, g, b);
                    let n = lamps_amount as usize;
                    let mut data = Vec::with_capacity(n * 5);
                    for i in 0..n {
                        let idx = (i + 1) as u8;
                        data.push(idx);
                        data.push(mapped[0]);
                        data.push(mapped[1]);
                        data.push(mapped[2]);
                        data.push(idx);
                    }
                    d.set_sync_screen(&data)
                }
                SoftEffect::Rotate => {
                    // LED 위치별 그라데이션 회전
                    let n = lamps_amount as usize;
                    let mut data = Vec::with_capacity(n * 5);
                    for i in 0..n {
                        let phase = (i as f64 / n as f64 + t * 0.1) % 1.0;
                        let brightness = ((phase * std::f64::consts::TAU).sin() + 1.0) / 2.0;
                        let r = (effect_cfg.color_r as f64 * brightness) as u8;
                        let g = (effect_cfg.color_g as f64 * brightness) as u8;
                        let b = (effect_cfg.color_b as f64 * brightness) as u8;
                        let mapped = wire_map.apply(r, g, b);
                        let idx = (i + 1) as u8;
                        data.push(idx);
                        data.push(mapped[0]);
                        data.push(mapped[1]);
                        data.push(mapped[2]);
                        data.push(idx);
                    }
                    d.set_sync_screen(&data)
                }
                SoftEffect::None => Ok(()),
            };
            if let Err(e) = result {
                handle_write_failure(e.as_ref(), &mut conn, &mut link, &mut next_connect_at);
                continue;
            }
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(30));
            continue;
        } else {
            // 하드웨어 동적/음악/단색/끄기 — 별도 루프 불필요
            std::thread::sleep(Duration::from_millis(200));
            while rx.try_recv().is_ok() {}
            continue;
        }

        let elapsed = send_start.elapsed();
        if elapsed < send_interval {
            std::thread::sleep(send_interval - elapsed);
        }
    }

    if let Some(c) = conn.take() {
        log::info!("앱 종료 — LED 끄기");
        let _ = c.turn_off();
    }
    device_released.store(true, Ordering::SeqCst);
}

fn effect_cfg_changed(a: &config::EffectConfig, b: &config::EffectConfig) -> bool {
    a.dynamic_index != b.dynamic_index
        || a.sound_index != b.sound_index
        || a.color_r != b.color_r
        || a.color_g != b.color_g
        || a.color_b != b.color_b
        || a.effect_speed != b.effect_speed
        || a.rhythm_source != b.rhythm_source
        || a.soft_effect != b.soft_effect
}

/// 모드 적용. 성공 시 true, 실패 시 false 반환.
fn apply_mode(conn: &DeviceConnection, effect: &config::EffectConfig, lamps_amount: u32) -> bool {
    log::info!("모드 적용: {:?}", effect.mode);
    let result = match effect.mode {
        LedMode::Sync => {
            // 동기화 모드: 별도 명령 불필요, sender_loop이 SC 패킷 전송
            Ok(())
        }
        LedMode::Dynamic => {
            // 원본 흐름: setSectionLED → setLedEffect(2=동적, index)
            conn.set_section_led(0, 0, 0, lamps_amount).ok();
            std::thread::sleep(Duration::from_millis(20));
            if let Err(e) = conn.set_led_effect(2, effect.dynamic_index) {
                log::warn!("동적 효과 설정 실패: {}", e);
            }
            std::thread::sleep(Duration::from_millis(80));
            conn.set_dynamic_speed(effect.effect_speed)
        }
        LedMode::Sound => {
            // 원본 흐름: setSectionLED → setLedEffect(3=음악, index)
            conn.set_section_led(0, 0, 0, lamps_amount).ok();
            std::thread::sleep(Duration::from_millis(20));
            conn.set_led_effect(3, effect.sound_index)
        }
        LedMode::Static => {
            // setSectionLED로 단색 전송
            conn.set_section_led(effect.color_r, effect.color_g, effect.color_b, lamps_amount)
        }
        LedMode::Off => {
            conn.turn_off()
        }
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            log::warn!("모드 적용 실패: {}", e);
            false
        }
    }
}

/// Firmware brightness: full in Sync mode (software scaling handles it), user level otherwise.
fn firmware_brightness(mode: &LedMode, brightness: u8) -> u8 {
    if *mode == LedMode::Sync { 255 } else { brightness }
}

fn power_from_config(sync: &config::SyncConfig) -> Option<f32> {
    if sync.light_compression { Some(sync.power_limit_pct as f32 / 100.0) } else { None }
}

/// Effective whole-strip budget: the configured one, further reduced by the brownout guard.
fn effective_power_limit(base: Option<f32>, derate: f32) -> Option<f32> {
    match base {
        Some(f) => Some(f * derate),
        None if derate < 1.0 => Some(derate),
        None => None,
    }
}

fn grade_from_config(sync: &config::SyncConfig) -> pipeline::Grade {
    pipeline::Grade {
        saturation_gain: sync.saturation,
        gamma: sync.gamma,
        brightness: sync.brightness as f32 / 255.0,
        ..pipeline::Grade::default()
    }
}

fn create_rgb_tray_icon() -> Icon {
    let size = 32u32;
    let rgba = gui::generate_rgb_icon(size);
    Icon::from_rgba(rgba, size, size).expect("아이콘 생성 실패")
}

fn create_gray_tray_icon() -> Icon {
    let size = 32u32;
    let rgba = gui::generate_rgb_icon(size);
    let gray: Vec<u8> = rgba
        .chunks(4)
        .flat_map(|px| {
            let lum = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as u8;
            [lum, lum, lum, px[3] / 2]
        })
        .collect();
    Icon::from_rgba(gray, size, size).expect("아이콘 생성 실패")
}

// ── Windows 종료·절전 감지 (WM_QUERYENDSESSION / WM_ENDSESSION / PBT_APMSUSPEND) ──
//
// 전환 전에 sender 스레드가 LED를 끄고 HID 핸들을 놓았는지를 핸드셰이크로 확인한다.
// 이전 구현은 WM_ENDSESSION에서 800ms 잠들고 끝이라, sender 가 쓰기 타임아웃(1초)에
// 걸려 있으면 끄기 명령이 나가기 전에 프로세스가 죽었다(2026-10-16 02:04 로그에 종료 기록 없음).

static SHUTDOWN_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static MONITOR_OFF_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static SUSPEND_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static RELEASED_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// sender 에게 "LED 끄고 핸들 놓아라"를 요청하고 완료(또는 시한)까지 기다린다.
fn request_device_release(reason: &str, max_wait: Duration) {
    let (Some(suspend), Some(released)) = (SUSPEND_FLAG.get(), RELEASED_FLAG.get()) else { return; };
    log::info!("{} — LED 끄기 요청", reason);
    suspend.store(true, Ordering::SeqCst);
    let start = Instant::now();
    while !released.load(Ordering::SeqCst) && start.elapsed() < max_wait {
        std::thread::sleep(Duration::from_millis(20));
    }
    if released.load(Ordering::SeqCst) {
        log::info!("{} — LED 끄기 완료 ({}ms)", reason, start.elapsed().as_millis());
    } else {
        log::warn!("{} — LED 끄기 미완료 (시한 {}ms 초과)", reason, max_wait.as_millis());
    }
}

fn resume_device(reason: &str) {
    if let Some(suspend) = SUSPEND_FLAG.get() {
        if suspend.swap(false, Ordering::SeqCst) {
            log::info!("{} — 디바이스 재연결 예정", reason);
        }
    }
}

fn spawn_shutdown_listener(
    running: Arc<AtomicBool>,
    monitor_off: Arc<AtomicBool>,
    suspend_pending: Arc<AtomicBool>,
    device_released: Arc<AtomicBool>,
) {
    SHUTDOWN_FLAG.set(running).ok();
    MONITOR_OFF_FLAG.set(monitor_off).ok();
    SUSPEND_FLAG.set(suspend_pending).ok();
    RELEASED_FLAG.set(device_released).ok();
    std::thread::Builder::new()
        .name("shutdown".into())
        .spawn(|| unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            use windows::Win32::System::Power::RegisterPowerSettingNotification;
            use windows::Win32::System::Threading::SetProcessShutdownParameters;
            const SHUTDOWN_NORETRY: u32 = 0x0001; // winbase.h
            use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
            use windows::Win32::Foundation::HANDLE;

            // 종료 알림을 다른 앱보다 먼저 받아 LED 끄기 시간을 확보한다 (0x3FF = 최우선)
            let _ = SetProcessShutdownParameters(0x3FF, SHUTDOWN_NORETRY);

            let class_name = windows::core::w!("SYNCFASTERGB_Shutdown");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(shutdown_wnd_proc),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&wc);

            // 숨겨진 top-level 윈도우 (WM_ENDSESSION + 전원 알림 수신용)
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0), class_name,
                windows::core::w!(""), WINDOW_STYLE(0),
                0, 0, 0, 0,
                None, None, None, None,
            );

            // 모니터 전원 상태 변경 알림 등록
            if let Ok(hwnd) = hwnd {
                let _ = RegisterPowerSettingNotification(
                    HANDLE(hwnd.0 as _), &GUID_CONSOLE_DISPLAY_STATE,
                    REGISTER_NOTIFICATION_FLAGS(0),
                );
            }

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

unsafe extern "system" fn shutdown_wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    // WM_POWERBROADCAST wParam (winuser.h)
    const PBT_APMSUSPEND: usize = 0x0004;
    const PBT_APMRESUMESUSPEND: usize = 0x0007;
    const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
    const PBT_POWERSETTINGCHANGE: usize = 0x8013;

    match msg {
        WM_QUERYENDSESSION => {
            // 응답 시한 5초 안에 끄기를 끝낸다 (정상 디바이스 ~0.7초, 먹통이면 쓰기 타임아웃 1초+)
            request_device_release("세션 종료 예고", Duration::from_millis(3000));
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                // QUERY 를 못 받은 경로(ENDSESSION_CRITICAL 등) 대비 한 번 더
                request_device_release("세션 종료", Duration::from_millis(2000));
                if let Some(running) = SHUTDOWN_FLAG.get() {
                    running.store(false, Ordering::SeqCst);
                }
            } else {
                // 다른 앱이 종료를 막아 취소됨
                resume_device("세션 종료 취소");
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            use windows::Win32::System::Power::POWERBROADCAST_SETTING;
            use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
            match wparam.0 {
                PBT_APMSUSPEND => {
                    // 앱에 주어지는 시간은 약 2초
                    request_device_release("절전 진입", Duration::from_millis(1800));
                }
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                    resume_device("절전 복귀");
                }
                PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
                    let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                    if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                        let state = setting.Data[0];
                        if let Some(flag) = MONITOR_OFF_FLAG.get() {
                            // 0 = 꺼짐, 1 = 켜짐, 2 = 어두워짐
                            flag.store(state == 0, Ordering::SeqCst);
                            log::info!("모니터 전원 상태: {}", match state {
                                0 => "꺼짐", 1 => "켜짐", _ => "어두워짐"
                            });
                        }
                    }
                }
                _ => {}
            }
            LRESULT(1)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn run_tray(running: Arc<AtomicBool>, active: Arc<AtomicBool>, config_version: Arc<AtomicU32>, locale: &'static gui::Locale, link_status: Arc<AtomicU8>) {
    let event_loop = EventLoop::new().expect("이벤트 루프 생성 실패");
    let mut app = TrayApp {
        running, active, config_version,
        config_path: Config::config_path(),
        last_mtime: None, tick_counter: 0,
        tray: None, toggle_item: None, settings_item: None, quit_item: None,
        locale,
        link_status, last_link: u8::MAX,
    };
    event_loop.run_app(&mut app).ok();
}

struct TrayApp {
    running: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    config_version: Arc<AtomicU32>,
    config_path: PathBuf,
    last_mtime: Option<SystemTime>,
    tick_counter: u32,
    tray: Option<TrayIcon>,
    toggle_item: Option<MenuItem>,
    settings_item: Option<MenuItem>,
    quit_item: Option<MenuItem>,
    locale: &'static gui::Locale,
    link_status: Arc<AtomicU8>,
    last_link: u8,
}

impl ApplicationHandler for TrayApp {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray.is_some() { return; }

        let menu = Menu::new();
        let toggle = MenuItem::new(self.locale.tray_pause, true, None);
        let settings = MenuItem::new(self.locale.tray_settings, true, None);
        let quit = MenuItem::new(self.locale.tray_quit, true, None);
        menu.append(&toggle).ok();
        menu.append(&settings).ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        menu.append(&quit).ok();

        let tray = TrayIconBuilder::new()
            .with_icon(create_rgb_tray_icon())
            .with_tooltip("SYNCFASTERGB: searching for strip")
            .with_menu_on_left_click(false)
            .with_menu(Box::new(menu))
            .build()
            .expect("트레이 아이콘 생성 실패");

        self.toggle_item = Some(toggle);
        self.settings_item = Some(settings);
        self.quit_item = Some(quit);
        self.tray = Some(tray);
        self.last_mtime = std::fs::metadata(&self.config_path).ok().and_then(|m| m.modified().ok());
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.running.load(Ordering::Relaxed) {
            event_loop.exit();
            return;
        }
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(ref toggle) = self.toggle_item {
                if event.id() == toggle.id() {
                    let was = self.active.load(Ordering::Relaxed);
                    self.active.store(!was, Ordering::Relaxed);
                    if was {
                        toggle.set_text(self.locale.tray_resume);
                        if let Some(ref tray) = self.tray {
                            tray.set_icon(Some(create_gray_tray_icon())).ok();
                            tray.set_tooltip(Some(self.locale.tray_paused_tooltip)).ok();
                        }
                    } else {
                        toggle.set_text(self.locale.tray_pause);
                        self.last_link = u8::MAX; // force a status refresh below
                    }
                }
            }
            if let Some(ref s) = self.settings_item {
                if event.id() == s.id() { gui::open_settings(); }
            }
            if let Some(ref q) = self.quit_item {
                if event.id() == q.id() {
                    self.running.store(false, Ordering::Relaxed);
                    event_loop.exit();
                }
            }
        }

        // Left click / double click on the tray icon opens (or focuses) the settings window.
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            match ev {
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => gui::open_settings(),
                _ => {}
            }
        }

        // Tray icon/tooltip follow the strip connection (unless paused by the user).
        let link = self.link_status.load(Ordering::Relaxed);
        if link != self.last_link && self.active.load(Ordering::Relaxed) {
            self.last_link = link;
            if let Some(ref tray) = self.tray {
                let (icon, tip) = match link {
                    LINK_UP => (create_rgb_tray_icon(), "SYNCFASTERGB: strip connected"),
                    LINK_HUNG => (create_gray_tray_icon(), "SYNCFASTERGB: strip not responding, unplug and replug it"),
                    _ => (create_gray_tray_icon(), "SYNCFASTERGB: searching for strip"),
                };
                tray.set_icon(Some(icon)).ok();
                tray.set_tooltip(Some(tip)).ok();
            }
        }

        self.tick_counter += 1;
        if self.tick_counter.is_multiple_of(10) {
            if let Ok(meta) = std::fs::metadata(&self.config_path) {
                if let Ok(mtime) = meta.modified() {
                    if self.last_mtime != Some(mtime) {
                        self.last_mtime = Some(mtime);
                        self.config_version.fetch_add(1, Ordering::Relaxed);

                        let new_locale = gui::detect_locale();
                        if !std::ptr::eq(new_locale, self.locale) {
                            self.locale = new_locale;
                            if let Some(ref t) = self.toggle_item {
                                if self.active.load(Ordering::Relaxed) {
                                    t.set_text(self.locale.tray_pause);
                                } else {
                                    t.set_text(self.locale.tray_resume);
                                }
                            }
                            if let Some(ref s) = self.settings_item {
                                s.set_text(self.locale.tray_settings);
                            }
                            if let Some(ref q) = self.quit_item {
                                q.set_text(self.locale.tray_quit);
                            }
                        }
                    }
                }
            }
        }

        // Wake ~10x/s instead of busy-looping (menu clicks still respond within 100 ms).
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100)));
    }
}
