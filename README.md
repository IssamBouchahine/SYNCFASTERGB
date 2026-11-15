# SYNCFASTERGB

Lightweight screen-sync ambient lighting for USB HID LED strips (USB `1A86:FE07`) on Windows 10/11.
One self-contained `.exe`: tray app + settings window, no installer, no runtime to install, and
**no other files**: it never creates a config file, a log file or a folder next to itself.

> **Not independently audited. Provided as-is, with no warranty.** See [SECURITY.md](SECURITY.md).

## What it does

- Captures a monitor with DXGI Desktop Duplication, shrinks the frame **on the GPU**, and turns the
  screen edges into LED colors, about 30 times per second.
- Finds the strip automatically, reconnects after unplug/replug or sleep, and waits for the monitor
  if it is not there yet.
- Works on hybrid-GPU (Optimus) laptops: external DisplayPort/HDMI monitors wired to the NVIDIA GPU
  are captured on the right adapter.
- Handles Windows **HDR** desktops (tone-mapped to SDR).
- Smooth, time-based smoothing (fast attack, slower decay), live brightness slider, saturation,
  gamma and a per-LED power limit.
- Adjustable **detection area**: how deep from the screen edge each LED looks, and automatic skipping of
  black bars (letterboxed video). An interactive, live illustration of your screen shows the sampled strips
  in the colors the LEDs receive; drag a strip's inner edge to change the depth.
- Tray icon shows connection state. Settings apply live.

Typical cost on an RTX 4060 laptop at 2560x1440, 30 fps: about 40 MB RAM, a few percent of one CPU core.

## Use

1. Download `syncfastergb.exe` (and `SHA256SUMS.txt`) from the Releases page and verify the hash
   (see [SECURITY.md](SECURITY.md)).
2. Run it. A tray icon appears; right-click for **Settings**, Pause and Quit.
3. In Settings pick the monitor, LED count, edges and direction. Turn Windows HDR off if you prefer
   SDR-accurate colors, or leave it on and set *HDR white level* to match your SDR brightness.

Close the vendor's own app first: only one program can drive the strip at a time.

Command line: `--settings` (open the settings window), `--list-monitors`, `--console` (show log output),
`--print-settings` (show the saved settings as text), `--import-settings FILE` (load settings from a text file).

## Configuration

There is no config file. Settings are saved when you change them in the settings window, in one value
under `HKEY_CURRENT_USER\Software\SYNCFASTERGB` (per user, no admin rights), and are applied live.
Missing keys use defaults and every value is range-clamped. To edit by hand, run
`syncfastergb.exe --print-settings`, change the text, and load it with `--import-settings FILE`.
The settings window has a button that removes everything the app stored on the PC.

| Key | Meaning |
|---|---|
| `device.lamps_amount` | LED count (1..255) |
| `device.wire_map` | Color order: `RGB`, `GRB`, ... |
| `capture.monitor` / `capture.monitor_name` | Output index across all GPUs / display hardware id (wins when set) |
| `capture.fps` | 10..60 |
| `capture.gpu_downscale` | GPU shrink before readback (falls back to CPU automatically) |
| `capture.hdr_sdr_white_nits` | HDR only: what SDR white maps to |
| `sync.brightness` | 0..255 software brightness |
| `sync.saturation`, `sync.gamma` | Color grading |
| `sync.attack_ms`, `sync.decay_ms`, `sync.deadband` | Smoothing |
| `sync.edge_depth_pct` | How far in from the screen edge each LED looks, percent of the screen (3..40, default 15) |
| `sync.skip_black_bars`, `sync.black_level` | Sample just inside letterbox/pillarbox black bars (bars must appear on both opposite sides); lines darker than the level (default 26) count as bars |
| `sync.power_limit_pct` | Whole-strip power budget, percent of all-LEDs-white (default 40, max 80). Highlights can be full brightness; only the total is capped |

## Build

Windows, Rust stable (MSVC), Visual Studio Build Tools (C++ + Windows SDK):

```powershell
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Output: `target\release\syncfastergb.exe`. The C runtime is linked statically.

## Hardware safety

Designed to be gentle on the laptop and the strip:

- **USB power:** a whole-strip power budget (default 40%, hard ceiling 80% of "every LED fully white")
  keeps the draw within what a laptop USB port is built to supply. If the strip repeatedly drops off the
  bus while drawing a lot, the app lowers its own budget for the rest of the session.
- **Strip memory:** brightness is applied in software while syncing; commands that may be stored in
  the strip's flash (hardware-effect brightness) are debounced, so dragging a slider never floods it.
- **Disk:** the app writes no files at all. Settings go to a registry value (only when you change one,
  and only if it changed); the startup entry is touched only when you toggle "Start with Windows".
  Logging is off unless you start it with `--console`. No steady background writes.
- **CPU/GPU:** frames are shrunk on the GPU that already drives your monitor; typical load is a few
  percent of one core while the picture is moving, and almost nothing on a still screen.
- **Games:** the app runs at below-normal CPU priority and the lowest GPU scheduling priority, reads
  frames back without ever waiting for a busy GPU, drops to 30 fps (`capture.game_fps`) while a real
  full-screen app is focused (detected with Windows' own full-screen detector, so merely maximized
  windows do not count), and never restarts or retries aggressively while a game owns the screen.
  Measured on an RTX 4060 laptop with a synthetic load keeping the GPU 100% busy while the screen
  changed at 60 Hz: about 1.2% less GPU throughput at 60 fps capture (roughly half that at 30 fps) and
  about 4-6% of one CPU core. A game that is not completely GPU-bound sees no difference.
- **No admin rights, no network, no drivers.**

## Security stance

No networking and no telemetry (CI bans HTTP/TLS/socket crates), no elevation, no shell-outs.
Screen pixels never leave memory. Details and threat model: [SECURITY.md](SECURITY.md).

## Credits and license

Derived from [SyncRGB](https://github.com/Tonic-Jin/SyncRGB) by Tonic-Jin (MIT), forked at upstream
commit `857226f`. See [NOTICE](NOTICE) and [LICENSE](LICENSE). This project is not affiliated with
any LED strip vendor; vendor names are trademarks of their owners.
