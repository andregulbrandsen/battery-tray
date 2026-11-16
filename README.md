# Battery Tray

One tray icon per wireless accessory, showing its battery level. Single ~430 KB exe, ~2 MB RAM, no admin.

## Sources
- **Bluetooth**: the battery value Windows stores for paired devices (same as Settings → Bluetooth & devices).
- **Logitech**: HID++ 2.0 over Lightspeed, Bolt and Unifying receivers, and directly attached devices.
  Unifying receivers held by Logitech's legacy SetPoint driver can't be read.

Polls every 60 s and on device changes. Sleeping devices keep their last known level, drawn dimmed.

## Use
Right-click (or left-click) an icon: rename, change icon, hide, show hidden devices, display mode
(icon + level bar, or number only), low-battery alert threshold, start with Windows, refresh, exit.
Settings and last-known levels live in `%APPDATA%\BatteryTray\config.ini`.

Windows 11 puts new tray icons in the overflow (^) area: drag each icon onto the taskbar once to pin it.
Icon ids are stable per device, so the pin survives restarts.

## Build
```
cargo build --release
```
Uses the `x86_64-pc-windows-gnu` toolchain (no Visual Studio needed). If the project folder path is
very long, build scripts can fail with "cannot find the path specified"; set `CARGO_TARGET_DIR` to a
short folder.

Diagnostics:
- `battery-tray.exe --scan out.txt` writes one scan's readings to a file.
- `battery-tray.exe --preview out.bmp` renders every icon variant (both themes, 16/24/32 px).

To update the copy you run: exit Battery Tray from its menu, then copy `target\release\battery-tray.exe`
to `bin\` (not tracked by git) and start it again. Point "Start with Windows" at `bin\battery-tray.exe`.

## Icon, name and version
`build.rs` embeds `assets/battery-tray.ico`, version info ("Battery Tray", version from `Cargo.toml`)
and a manifest (common controls v6, per-monitor DPI, `asInvoker`). It writes the resource object
itself, so no `rc.exe`/`windres` is needed. Regenerate the icon with `pwsh -File tools\make-icon.ps1`
(add `-Preview` for a contact sheet in `assets\icon-preview.png`).

## Layout
- `build.rs` – embeds icon, version info and manifest
- `tools/probe/` – PowerShell discovery script that lists every device with a readable battery
- `tools/make-icon.ps1` – draws the app icon
- `src/main.rs` – hidden window, tray icons, menu, worker thread
- `src/providers/bluetooth.rs`, `src/providers/logitech.rs` – battery sources
- `src/render.rs` – icon drawing (Segoe Fluent Icons glyph + level bar, or number)
- `src/config.rs` – settings file
- `src/input.rs` – rename prompt
