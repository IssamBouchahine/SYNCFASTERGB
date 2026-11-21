// Hide the console window (show it with the --console flag)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod capture;
mod color;
mod config;
mod device;
mod foreground;
mod gui;
mod ipc;
mod single;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

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
    // --console flag: show a console even in release builds
    if std::env::args().any(|a| {
        matches!(
            a.as_str(),
            "--console" | "--list-monitors" | "--print-settings" | "--import-settings"
        )
    }) {
        unsafe {
            attach_console();
        }
    }

    // `--print-settings` writes the settings as text; `--import-settings FILE` stores a settings
    // file (e.g. an old config.toml) in the registry. Neither creates any file.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--print-settings") {
            match Config::load_or_default().to_toml() {
                Ok(t) => println!("{t}"),
                Err(e) => println!("error: {e}"),
            }
            return;
        }
        if let Some(i) = args.iter().position(|a| a == "--import-settings") {
            let result = args
                .get(i + 1)
                .ok_or_else(|| "usage: --import-settings <file>".to_string())
                .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
                .and_then(|t| Config::from_toml(&t).map_err(|e| e.to_string()))
                .and_then(|c| c.save().map_err(|e| e.to_string()));
            match result {
                Ok(()) => println!("settings imported"),
                Err(e) => println!("import failed: {e}"),
            }
            return;
        }
    }

    if std::env::args().any(|a| a == "--list-monitors") {
        capture::monitors::print_list();
        return;
    }

    init_logging();

    // With panic=abort and no console a panic would vanish without a trace, so log it
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {}", info);
    }));

    if std::env::args().any(|a| a == "--settings") {
        gui::run_settings_window();
        return;
    }

    // One tray app per session. Starting it again (taskbar, shortcut, autostart) just shows
    // the settings window of the running one. After a self-restart the old process may still
    // be shutting down, so wait for it.
    let wait = if restart_count_from_args() > 0 {
        Duration::from_secs(10)
    } else {
        Duration::ZERO
    };
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
    log::info!("settings: {:?}", config);

    let running = Arc::new(AtomicBool::new(true));
    let active = Arc::new(AtomicBool::new(true));
    let config_version = Arc::new(AtomicU32::new(0));
    let monitor_off = Arc::new(AtomicBool::new(false));
    let screen_black = Arc::new(AtomicBool::new(false));
    // Sleep/shutdown handshake: the listener asks (suspend_pending), the sender turns the LEDs off and releases the handle (device_released)
    let suspend_pending = Arc::new(AtomicBool::new(false));
    let device_released = Arc::new(AtomicBool::new(false));
    let link_status = Arc::new(AtomicU8::new(LINK_SEARCHING));

    spawn_shutdown_listener(
        running.clone(),
        monitor_off.clone(),
        suspend_pending.clone(),
        device_released.clone(),
    );

    let (tx, rx) = mpsc::sync_channel::<Vec<pipeline::Led>>(2);

    let capture_thread = {
        let running = running.clone();
        let active = active.clone();
        let config_version = config_version.clone();
        let config = config.clone();
        let screen_black = screen_black.clone();
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || capture_loop(config, running, active, config_version, tx, screen_black))
            .expect("failed to start the capture thread")
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
            .spawn(move || {
                sender_loop(
                    config,
                    running,
                    config_version,
                    rx,
                    monitor_off,
                    screen_black,
                    suspend_pending,
                    device_released,
                    link_status,
                )
            })
            .expect("failed to start the sender thread")
    };

    run_tray(running.clone(), active, config_version, locale, link_status);

    // Quitting the app also closes its settings window (a separate process): otherwise the
    // window would stay open, and in the taskbar, with nothing behind it.
    single::close_window(gui::SETTINGS_TITLE, Duration::from_millis(1500));

    // Stop the threads after the tray exits (normal shutdown path)
    running.store(false, Ordering::SeqCst);
    let _ = capture_thread.join();
    let _ = send_thread.join();
    log::info!("SYNCFASTERGB stopped");
}

/// Logging never touches the disk. With `--console` the log goes to that console; otherwise it
/// is discarded (the program is a single exe and leaves no files behind).
fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder.format_timestamp_millis();
    if !std::env::args().any(|a| a == "--console") {
        builder.target(env_logger::Target::Pipe(Box::new(std::io::sink())));
    }
    builder.init();
}

/// Attach a console in release builds
#[allow(unused_unsafe)]
unsafe fn attach_console() {
    #[cfg(not(debug_assertions))]
    {
        use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
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
    tx: mpsc::SyncSender<Vec<pipeline::Led>>,
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
                    log::warn!(
                        "screen capture not available yet ({}): retrying every 2 s",
                        e
                    );
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

    log::info!(
        "capture thread started ({} fps, {}x{})",
        config.capture.fps,
        first.width,
        first.height
    );
    let mut mon_rect: Option<foreground::Rect4> = Some((
        first.origin.0,
        first.origin.1,
        first.origin.0 + first.width as i32,
        first.origin.1 + first.height as i32,
    ));
    let mut capturer: Option<ScreenCapture> = Some(first);
    let mut local_version = 0u32;
    // Settings in use: a reload that cannot read the store keeps these
    let mut in_use = config.clone();
    // Counter of consecutive black frames (the flag is set after about a second)
    let mut black_frame_count = 0u32;
    let mut black_threshold_frames = config.capture.fps; // about 1 second
                                                         // Consecutive error counter (re-creates capture when it piles up)
    let mut consecutive_errors = 0u32;
    // Last send (lets smoothing settle on a still screen, and drives the keepalive)
    let mut last_sent: Option<Vec<pipeline::Led>> = None;
    let mut last_send_at = Instant::now();
    const KEEPALIVE: Duration = Duration::from_millis(500);
    // Performance statistics (debug log only)
    let mut stat_cap = Duration::ZERO;
    let mut stat_ext = Duration::ZERO;
    let mut stat_frames = 0u32;
    let mut stat_since = Instant::now();
    // Access loss (exclusive full-screen games, secure desktop) can repeat every second: only the start and the recovery are logged
    // (one earlier log grew to 7.5 MB with 19,355 repeats over 5 hours)
    let mut access_lost_since: Option<Instant> = None;
    let mut access_lost_reinits = 0u32;
    // The live picture for the settings window is shared only while that window is open.
    let mut frame_share: Option<ipc::FrameShare> = None;
    let mut viewer_open = false;
    let mut last_viewer_check = Instant::now() - Duration::from_secs(10);
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
            let c = Config::load_or_keep(&in_use);
            in_use = c.clone();
            base_interval = Duration::from_millis(1000 / c.capture.fps as u64);
            game_interval = Duration::from_millis(1000 / c.capture.game_fps.max(1) as u64);
            frame_interval = if game_mode {
                base_interval.max(game_interval)
            } else {
                base_interval
            };
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

        if last_viewer_check.elapsed() >= Duration::from_secs(1) {
            last_viewer_check = Instant::now();
            viewer_open = single::is_held(single::SETTINGS_MUTEX);
            if viewer_open && frame_share.is_none() {
                frame_share = ipc::FrameShare::create();
            } else if !viewer_open {
                frame_share = None;
            }
        }

        // Once a second: is a full-screen app (a game) focused on the captured monitor?
        if last_game_check.elapsed() >= Duration::from_secs(1) {
            last_game_check = Instant::now();
            if let Some(rect) = mon_rect {
                let now_game = foreground::fullscreen_app_covering(rect);
                if now_game != game_mode {
                    game_mode = now_game;
                    frame_interval = if game_mode {
                        base_interval.max(game_interval)
                    } else {
                        base_interval
                    };
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
                    mon_rect = Some((
                        c.origin.0,
                        c.origin.1,
                        c.origin.0 + c.width as i32,
                        c.origin.1 + c.height as i32,
                    ));
                    capturer = Some(c);
                    frames_since_create = 0;
                    create_failures = 0;
                }
                Err(e) => {
                    create_failures += 1;
                    dead_since.get_or_insert_with(Instant::now);
                    if create_failures == 1 || create_failures.is_multiple_of(60) {
                        log::warn!(
                            "cannot re-create capture yet ({}); retrying (attempt {})",
                            e,
                            create_failures
                        );
                    }
                    if !game_mode
                        && dead_since.is_some_and(|t| t.elapsed() >= WATCHDOG)
                        && restarts < MAX_RESTARTS
                    {
                        log::error!(
                            "capture dead for {} s: restarting the app",
                            WATCHDOG.as_secs()
                        );
                        if relaunch_self(restarts) {
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(lost_backoff_ms(
                        create_failures.min(3),
                    )));
                    continue;
                }
            }
        }
        let Some(cap) = capturer.as_mut() else {
            continue;
        };

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
                    log::info!(
                        "screen capture recovered after {} s ({} re-creations)",
                        since.elapsed().as_secs(),
                        access_lost_reinits
                    );
                    access_lost_reinits = 0;
                }
                let t_ext = Instant::now();
                let colors = extractor.extract(&frame.data, frame.pitch, frame.width, frame.height);
                stat_ext += t_ext.elapsed();
                if viewer_open {
                    if let Some(share) = frame_share.as_mut() {
                        share.publish_bgra(
                            &frame.data,
                            frame.pitch as usize,
                            frame.width as usize,
                            frame.height as usize,
                            extractor.bars(),
                            extractor.effective_depth(),
                        );
                    }
                }

                // Black screen detection: are all LED colors nearly black?
                const BLACK_SUM_THRESHOLD: f32 = 15.0;
                if turn_off_on_black {
                    let is_black = colors
                        .iter()
                        .all(|c| c[0] + c[1] + c[2] < BLACK_SUM_THRESHOLD);
                    if is_black {
                        black_frame_count = black_frame_count.saturating_add(1);
                    } else {
                        if black_frame_count >= black_threshold_frames {
                            log::info!("black screen ended: restoring the LEDs");
                        }
                        black_frame_count = 0;
                    }
                    let was_black = screen_black.load(Ordering::Relaxed);
                    let now_black = black_frame_count >= black_threshold_frames;
                    if now_black != was_black {
                        screen_black.store(now_black, Ordering::SeqCst);
                        if now_black {
                            log::info!("black screen detected: turning the LEDs off");
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
                                   // The newest change may still be waiting in the GPU readback: take it now.
                let fresh = cap.flush_pending();
                if let Some(f) = fresh.as_ref().or_else(|| cap.last_frame()) {
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
                        log::warn!(
                            "capture lost again right after re-creation ({} times)",
                            rapid_losses
                        );
                    }
                } else {
                    rapid_losses = 0;
                }
                // Drop every GPU object BEFORE building new ones.
                capturer = None;
                std::thread::sleep(Duration::from_millis(lost_backoff_ms(rapid_losses)));
                if !game_mode
                    && dead_since.is_some_and(|t| t.elapsed() >= WATCHDOG)
                    && restarts < MAX_RESTARTS
                    && capture::dxgi::attach_input_desktop()
                {
                    log::error!(
                        "capture dead for {} s: restarting the app",
                        WATCHDOG.as_secs()
                    );
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
                    log::warn!(
                        "{} capture errors in a row: re-creating capture",
                        consecutive_errors
                    );
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
    use windows::Win32::System::Threading::{
        GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
    };
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
        .find_map(|a| {
            a.strip_prefix("--restart-count=")
                .and_then(|v| v.parse().ok())
        })
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

/// Device link state (logged only when it changes: an earlier log was rotated away within 1.5 days
/// by reconnect spam every 49 seconds, losing the evidence around sleep and resume)
#[derive(Debug, Clone, Copy, PartialEq)]
enum LinkState {
    /// Not connected yet / not on the bus
    Absent,
    /// Enumerates but NAKs OUT reports (hid_write timeout): firmware hung
    Hung,
    /// Connected and answered getDeviceInfo
    Up,
    /// Connected but getDeviceInfo got no reply: only the reply path is dead (streaming works)
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
                log::error!(
                    "strip firmware stopped accepting data ({}): unplug and replug the strip",
                    e
                );
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
    rx: mpsc::Receiver<Vec<pipeline::Led>>,
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
    // Rounds the graded colours to the strip's 8-bit levels, dithering over time while they fade
    let mut dither = pipeline::Dither::new();
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
    // Settings in use: a reload that cannot read the store keeps these
    let mut in_use = config.clone();

    let mut conn: Option<DeviceConnection> = None;
    let mut link = LinkState::Absent;
    let mut next_connect_at = Instant::now();
    let mut last_hung_reminder = Instant::now();

    let mut local_version = 0u32;
    let mut send_count = 0u64;
    // Send statistics (debug log only): how often the strip really gets a frame, and how long
    // a write takes
    let mut stat_sends = 0u32;
    let mut stat_write = Duration::ZERO;
    let mut stat_write_max = Duration::ZERO;
    let mut stat_since = Instant::now();
    let mut blanked_by_screen = false;

    // Audio meter for computer rhythm (created on demand)
    let mut audio_meter: Option<audio::AudioMeter> = None;
    // Software effect timer
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

        // ── Sleep entry / session end notice: turn the LEDs off, release the handle, wait for resume ──
        // The device stays powered from 5VSB even when the PC is off and keeps glowing in its last state.
        // So that it stops "off" even if it hangs during the transition, always turn it off first.
        if suspend_pending.load(Ordering::SeqCst) {
            if let Some(c) = conn.take() {
                log::info!(
                    "sleep/shutdown transition: turning the LEDs off and releasing the HID handle"
                );
                if let Err(e) = c.turn_off() {
                    log::warn!("could not turn the LEDs off before the transition: {}", e);
                }
                blanked_by_screen = false;
                link = LinkState::Absent;
            }
            device_released.store(true, Ordering::SeqCst);
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(100));
            // Give USB re-enumeration time to finish right after resume
            next_connect_at = Instant::now() + Duration::from_secs(2);
            continue;
        }

        // ── Make sure we are connected (with backoff) ──
        if conn.is_none() {
            if Instant::now() < next_connect_at {
                while rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            match DeviceConnection::connect(&config.device.com_port) {
                Ok(mut c) => {
                    let responded = c.init_device().unwrap_or(false);
                    if let Err(e) = c.set_brightness(firmware_brightness(&current_mode, brightness))
                    {
                        log::warn!("setting brightness failed: {}", e);
                    }
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
                            log::warn!(
                                "no strip found yet ({}): checking every second, quietly",
                                e
                            );
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
            let c = Config::load_or_keep(&in_use);
            in_use = c.clone();
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
                        // Applying the mode failed: reconnect on the next loop
                        log::warn!("applying the mode failed: reconnecting");
                        conn = None;
                        next_connect_at = Instant::now() + Duration::from_secs(1);
                        continue;
                    }
                    let _ = d.set_brightness(firmware_brightness(&current_mode, brightness));
                }
            }
        }

        // Turn the LEDs off while the monitor sleeps or the screen is black
        let should_blank = (turn_off_on_sleep && monitor_off.load(Ordering::Relaxed))
            || (turn_off_on_black && screen_black.load(Ordering::Relaxed));
        if should_blank {
            if !blanked_by_screen {
                log::info!("screen off detected: turning the LEDs off");
                if let Some(d) = conn.as_ref() {
                    if let Err(e) = d.turn_off() {
                        handle_write_failure(
                            e.as_ref(),
                            &mut conn,
                            &mut link,
                            &mut next_connect_at,
                        );
                        continue;
                    }
                }
                blanked_by_screen = true;
            }
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(200));
            continue;
        } else if blanked_by_screen {
            log::info!("screen back: restoring the LEDs");
            blanked_by_screen = false;
            if let Some(d) = conn.as_ref() {
                let _ = d.set_brightness(firmware_brightness(&current_mode, brightness));
                apply_mode(d, &effect_cfg, lamps_amount);
            }
        }

        // Everything below needs a connection
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
                    let mut graded: Vec<pipeline::Led> = colors
                        .iter()
                        .map(|&c| pipeline::grade(c, &grade_params))
                        .collect();
                    if let Some(fraction) = effective_power_limit(power_limit, derate) {
                        pipeline::limit_total_power(&mut graded, fraction);
                    }
                    let last_draw = pipeline::power_fraction(&graded);
                    // the one rounding in the whole chain, with temporal dithering while fading
                    let graded = dither.apply(&graded);
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
                        log::info!(
                            "first SC packet: {} LEDs, {} bytes",
                            color_data.len() / 5,
                            color_data.len()
                        );
                    }
                    send_count += 1;

                    let t_write = Instant::now();
                    let sent = d.set_sync_screen(&color_data);
                    let took = t_write.elapsed();
                    stat_sends += 1;
                    stat_write += took;
                    stat_write_max = stat_write_max.max(took);
                    if stat_since.elapsed() >= Duration::from_secs(5) {
                        log::debug!(
                            "strip updates: {:.1}/s, write {:.2} ms avg / {:.2} ms max",
                            stat_sends as f64 / stat_since.elapsed().as_secs_f64(),
                            stat_write.as_secs_f64() * 1000.0 / stat_sends.max(1) as f64,
                            stat_write_max.as_secs_f64() * 1000.0
                        );
                        stat_sends = 0;
                        stat_write = Duration::ZERO;
                        stat_write_max = Duration::ZERO;
                        stat_since = Instant::now();
                    }
                    if let Err(e) = sent {
                        if device::serial::classify_error(e.as_ref())
                            == device::serial::WriteFailure::Gone
                            && last_draw > 0.3
                        {
                            let now = Instant::now();
                            gone_events
                                .retain(|t| now.duration_since(*t) < Duration::from_secs(120));
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
                        handle_write_failure(
                            e.as_ref(),
                            &mut conn,
                            &mut link,
                            &mut next_connect_at,
                        );
                        continue;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else if current_mode == LedMode::Sound
            && effect_cfg.rhythm_source == RhythmSource::Computer
        {
            // Computer rhythm: audio volume, sent repeatedly with setComputerRhythm
            if audio_meter.is_none() {
                audio_meter = audio::AudioMeter::new().ok();
                if audio_meter.is_none() {
                    log::warn!("audio meter could not be started");
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
            // Software dynamic effects (single-color breathing / rotation)
            soft_tick += 0.05;
            let speed_factor = effect_cfg.effect_speed as f64 / 50.0;
            let t = soft_tick * speed_factor;

            let result = match effect_cfg.soft_effect {
                SoftEffect::Breathe => {
                    // Sine-wave brightness (0.05 to 1.0)
                    let bright = ((t.sin() + 1.0) / 2.0 * 0.95 + 0.05) as f32;
                    let r = (effect_cfg.color_r as f32 * bright) as u8;
                    let g = (effect_cfg.color_g as f32 * bright) as u8;
                    let b = (effect_cfg.color_b as f32 * bright) as u8;
                    // setSectionLED (0x86) is a one-shot color command: sending it at 33 Hz risks hanging the firmware.
                    // Like Rotate, send through the streaming-only SC (setSyncScreen) path.
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
                    // Rotating gradient by LED position
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
            // Hardware dynamic / sound / static / off modes need no loop
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
        log::info!("app exiting: turning the LEDs off");
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

/// Apply a mode. Returns true on success, false on failure.
fn apply_mode(conn: &DeviceConnection, effect: &config::EffectConfig, lamps_amount: u32) -> bool {
    log::info!("mode applied: {:?}", effect.mode);
    let result = match effect.mode {
        LedMode::Sync => {
            // Sync mode: no command needed, sender_loop streams SC packets
            Ok(())
        }
        LedMode::Dynamic => {
            // Sequence: setSectionLED, then setLedEffect(2 = dynamic, index)
            conn.set_section_led(0, 0, 0, lamps_amount).ok();
            std::thread::sleep(Duration::from_millis(20));
            if let Err(e) = conn.set_led_effect(2, effect.dynamic_index) {
                log::warn!("setting the dynamic effect failed: {}", e);
            }
            std::thread::sleep(Duration::from_millis(80));
            conn.set_dynamic_speed(effect.effect_speed)
        }
        LedMode::Sound => {
            // Sequence: setSectionLED, then setLedEffect(3 = sound, index)
            conn.set_section_led(0, 0, 0, lamps_amount).ok();
            std::thread::sleep(Duration::from_millis(20));
            conn.set_led_effect(3, effect.sound_index)
        }
        LedMode::Static => {
            // Send a single color with setSectionLED
            conn.set_section_led(effect.color_r, effect.color_g, effect.color_b, lamps_amount)
        }
        LedMode::Off => conn.turn_off(),
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            log::warn!("applying the mode failed: {}", e);
            false
        }
    }
}

/// Firmware brightness: full in Sync mode (software scaling handles it), user level otherwise.
fn firmware_brightness(mode: &LedMode, brightness: u8) -> u8 {
    if *mode == LedMode::Sync {
        255
    } else {
        brightness
    }
}

fn power_from_config(sync: &config::SyncConfig) -> Option<f32> {
    if sync.light_compression {
        Some(sync.power_limit_pct as f32 / 100.0)
    } else {
        None
    }
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
        // accurate matching uses its own brightness curve; vivid keeps the older gamma setting
        gamma: if sync.accurate_color {
            sync.match_gamma
        } else {
            sync.gamma
        },
        brightness: sync.brightness as f32 / 255.0,
        accurate: sync.accurate_color,
        vibrance: sync.display_vibrance_pct as f32 / 100.0,
        matrix: pipeline::led_matrix(sync.led_green, sync.led_blue),
        led_match: sync.led_match_pct as f32 / 100.0,
        colour_gamma: sync.colour_curve,
        dark_extra: sync.dark_curve_extra,
        dark_level: sync.dark_level_extra,
        gains: [
            sync.balance_r as f32 / 100.0,
            sync.balance_g as f32 / 100.0,
            sync.balance_b as f32 / 100.0,
        ],
        ..pipeline::Grade::default()
    }
}

fn create_rgb_tray_icon() -> Icon {
    let size = 32u32;
    let rgba = gui::generate_rgb_icon(size);
    Icon::from_rgba(rgba, size, size).expect("failed to create the icon")
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
    Icon::from_rgba(gray, size, size).expect("failed to create the icon")
}

// ── Windows shutdown / sleep detection (WM_QUERYENDSESSION / WM_ENDSESSION / PBT_APMSUSPEND) ──
//
// A handshake confirms that the sender thread turned the LEDs off and released the HID handle before the transition.
// An earlier version just slept 800 ms in WM_ENDSESSION, so when the sender was stuck in a write timeout (1 s)
// the process died before the off command went out (that shutdown left no exit entry in the log).

static SHUTDOWN_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static MONITOR_OFF_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static SUSPEND_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static RELEASED_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Ask the sender to turn the LEDs off and release the handle, then wait for it (or the deadline).
fn request_device_release(reason: &str, max_wait: Duration) {
    let (Some(suspend), Some(released)) = (SUSPEND_FLAG.get(), RELEASED_FLAG.get()) else {
        return;
    };
    log::info!("{}: requesting LED off", reason);
    suspend.store(true, Ordering::SeqCst);
    let start = Instant::now();
    while !released.load(Ordering::SeqCst) && start.elapsed() < max_wait {
        std::thread::sleep(Duration::from_millis(20));
    }
    if released.load(Ordering::SeqCst) {
        log::info!(
            "{}: LED off done ({} ms)",
            reason,
            start.elapsed().as_millis()
        );
    } else {
        log::warn!(
            "{}: LED off not completed (deadline of {} ms exceeded)",
            reason,
            max_wait.as_millis()
        );
    }
}

fn resume_device(reason: &str) {
    if let Some(suspend) = SUSPEND_FLAG.get() {
        if suspend.swap(false, Ordering::SeqCst) {
            log::info!("{}: the device will reconnect", reason);
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
            use windows::Win32::System::Power::RegisterPowerSettingNotification;
            use windows::Win32::System::Threading::SetProcessShutdownParameters;
            use windows::Win32::UI::WindowsAndMessaging::*;
            const SHUTDOWN_NORETRY: u32 = 0x0001; // winbase.h
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;

            // Receive the shutdown notice before other apps to get time to turn the LEDs off (0x3FF = highest priority)
            let _ = SetProcessShutdownParameters(0x3FF, SHUTDOWN_NORETRY);

            let class_name = windows::core::w!("SYNCFASTERGB_Shutdown");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(shutdown_wnd_proc),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&wc);

            // Hidden top-level window (receives WM_ENDSESSION and power notifications)
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                windows::core::w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            );

            // Register for monitor power state changes
            if let Ok(hwnd) = hwnd {
                let _ = RegisterPowerSettingNotification(
                    HANDLE(hwnd.0 as _),
                    &GUID_CONSOLE_DISPLAY_STATE,
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
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    // WM_POWERBROADCAST wParam (winuser.h)
    const PBT_APMSUSPEND: usize = 0x0004;
    const PBT_APMRESUMESUSPEND: usize = 0x0007;
    const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
    const PBT_POWERSETTINGCHANGE: usize = 0x8013;

    match msg {
        WM_QUERYENDSESSION => {
            // Finish within the 5-second deadline (a healthy device takes ~0.7 s, a hung one 1 s+ for the write timeout)
            request_device_release("session end notice", Duration::from_millis(3000));
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                // One more time, for paths where QUERY was not received (ENDSESSION_CRITICAL etc.)
                request_device_release("session end", Duration::from_millis(2000));
                if let Some(running) = SHUTDOWN_FLAG.get() {
                    running.store(false, Ordering::SeqCst);
                }
            } else {
                // Cancelled because another app blocked the shutdown
                resume_device("session end cancelled");
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            use windows::Win32::System::Power::POWERBROADCAST_SETTING;
            use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
            match wparam.0 {
                PBT_APMSUSPEND => {
                    // The app gets about 2 seconds
                    request_device_release("entering sleep", Duration::from_millis(1800));
                }
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                    resume_device("resuming from sleep");
                }
                PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
                    let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                    if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                        let state = setting.Data[0];
                        if let Some(flag) = MONITOR_OFF_FLAG.get() {
                            // 0 = off, 1 = on, 2 = dimmed
                            flag.store(state == 0, Ordering::SeqCst);
                            log::info!(
                                "monitor power state: {}",
                                match state {
                                    0 => "off",
                                    1 => "on",
                                    _ => "dimmed",
                                }
                            );
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

fn run_tray(
    running: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    config_version: Arc<AtomicU32>,
    locale: &'static gui::Locale,
    link_status: Arc<AtomicU8>,
) {
    let event_loop = EventLoop::new().expect("failed to create the event loop");
    let mut app = TrayApp {
        running,
        active,
        config_version,
        last_revision: Config::revision(),
        tick_counter: 0,
        tray: None,
        toggle_item: None,
        settings_item: None,
        quit_item: None,
        locale,
        link_status,
        last_link: u8::MAX,
    };
    event_loop.run_app(&mut app).ok();
}

struct TrayApp {
    running: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    config_version: Arc<AtomicU32>,
    last_revision: u32,
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
        if self.tray.is_some() {
            return;
        }

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
            .expect("failed to create the tray icon");

        self.toggle_item = Some(toggle);
        self.settings_item = Some(settings);
        self.quit_item = Some(quit);
        self.tray = Some(tray);
        self.last_revision = Config::revision();
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
                if event.id() == s.id() {
                    gui::open_settings();
                }
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
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => gui::open_settings(),
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
                    LINK_HUNG => (
                        create_gray_tray_icon(),
                        "SYNCFASTERGB: strip not responding, unplug and replug it",
                    ),
                    _ => (create_gray_tray_icon(), "SYNCFASTERGB: searching for strip"),
                };
                tray.set_icon(Some(icon)).ok();
                tray.set_tooltip(Some(tip)).ok();
            }
        }

        self.tick_counter += 1;
        if self.tick_counter.is_multiple_of(10) {
            let revision = Config::revision();
            if revision != self.last_revision {
                self.last_revision = revision;
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

        // Wake ~10x/s instead of busy-looping (menu clicks still respond within 100 ms).
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(100),
        ));
    }
}
