//! Battery Tray: one tray icon per wireless accessory, showing its battery level. No admin needed.
#![windows_subsystem = "windows"]

mod config;
mod input;
mod model;
mod providers;
mod render;
mod util;

use config::{Config, Seen};
use model::{Kind, Reading};
use render::{IconSpec, Style};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use util::{copy_wide, hash32, now_label, wide};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_READINGS: u32 = WM_APP + 2;
const FALLBACK_ID: u32 = 1;
const POLL_EVERY: Duration = Duration::from_secs(60);
const THRESHOLDS: [u8; 5] = [0, 10, 15, 20, 30];
const DBT_DEVNODES_CHANGED: usize = 0x0007;
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1; // NIN_SELECT | NINF_KEY

const ID_RENAME: usize = 100;
const ID_HIDE: usize = 101;
const ID_KIND: usize = 110;
const ID_UNHIDE: usize = 200;
const ID_MODE_BAR: usize = 300;
const ID_MODE_NUM: usize = 301;
const ID_THRESHOLD: usize = 310;
const ID_AUTOSTART: usize = 320;
const ID_REFRESH: usize = 321;
const ID_EXIT: usize = 322;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "BatteryTray";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct Device {
    name: String,
    kind: Kind,
    level: u8,
    approx: bool,
    charging: bool,
    connected: bool,
    when: String,
    source: String,
    tray_id: u32,
    icon: HICON,
    shown: bool,
    alerted: bool,
}

struct App {
    hwnd: HWND,
    cfg: Config,
    devices: BTreeMap<String, Device>,
    rx: Receiver<Vec<Reading>>,
    refresh: Sender<()>,
    fallback_icon: HICON,
    fallback_shown: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state. Returns None when re-entered (e.g. from inside a Shell call).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn tray_id(key: &str) -> u32 {
    // Version-4 tray callbacks carry only the low 16 bits of the id.
    (hash32(key) & 0xffff).max(2)
}

fn reg_dword(sub: &str, value: &str) -> Option<u32> {
    let (s, v) = (wide(sub), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(s.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut c_void), Some(&mut size))
    };
    (r == ERROR_SUCCESS).then_some(data)
}

fn light_taskbar() -> bool {
    reg_dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "SystemUsesLightTheme") == Some(1)
}

/// The command Windows should run at sign-in: this exe, quoted.
fn autostart_command() -> Option<String> {
    std::env::current_exe().ok().map(|exe| format!("\"{}\"", exe.display()))
}

fn autostart_value() -> Option<String> {
    let (s, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(s.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut size))
    };
    (r == ERROR_SUCCESS).then(|| util::from_wide(&buf))
}

/// On only when the Run entry exists and points at this exe (a stale path from an old location counts as off).
fn autostart_enabled() -> bool {
    match (autostart_value(), autostart_command()) {
        (Some(v), Some(c)) => v.trim().eq_ignore_ascii_case(&c),
        _ => false,
    }
}

fn set_autostart(on: bool) -> Result<(), String> {
    let (s, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    let r = unsafe {
        if on {
            let cmd = autostart_command().ok_or("couldn't find this program's path")?;
            let data = wide(&cmd);
            RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(s.as_ptr()), PCWSTR(v.as_ptr()), REG_SZ.0, Some(data.as_ptr() as *const c_void), (data.len() * 2) as u32)
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(s.as_ptr()), PCWSTR(v.as_ptr()));
            if r == ERROR_FILE_NOT_FOUND { ERROR_SUCCESS } else { r }
        }
    };
    if r != ERROR_SUCCESS {
        return Err(format!("Windows returned error {}", r.0));
    }
    // Read back so the menu never claims something that didn't stick.
    if autostart_enabled() != on {
        return Err("the setting didn't stick when read back".into());
    }
    Ok(())
}

fn icon_size() -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }
}

fn nid(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    }
}

/// Adds or updates a tray icon. Falls back to add when the icon vanished (e.g. Explorer restarted).
unsafe fn show_icon(n: &mut NOTIFYICONDATAW, shown: &mut bool) {
    if *shown && Shell_NotifyIconW(NIM_MODIFY, n).as_bool() {
        return;
    }
    if Shell_NotifyIconW(NIM_ADD, n).as_bool() {
        n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, n);
        *shown = true;
    } else if Shell_NotifyIconW(NIM_MODIFY, n).as_bool() {
        *shown = true;
    }
}

unsafe fn delete_icon(hwnd: HWND, id: u32) {
    let n = nid(hwnd, id);
    let _ = Shell_NotifyIconW(NIM_DELETE, &n);
}

fn level_text(level: u8, approx: bool) -> String {
    if approx {
        format!("about {level}%")
    } else {
        format!("{level}%")
    }
}

fn tooltip(name: &str, d: &Device) -> String {
    let mut s = format!("{name} · {}", level_text(d.level, d.approx));
    if d.charging {
        s += " · charging";
    }
    if d.connected {
        s += &format!("\n{}", d.source);
    } else {
        s += &format!("\nAsleep or off · last seen {}", d.when);
    }
    s
}

fn add_item(m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let t = wide(text);
    unsafe {
        let _ = AppendMenuW(m, flags, id, PCWSTR(t.as_ptr()));
    }
}

fn checked(on: bool) -> MENU_ITEM_FLAGS {
    if on {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    }
}

impl App {
    fn name_of(&self, key: &str) -> String {
        self.cfg.names.get(key).cloned().or_else(|| self.devices.get(key).map(|d| d.name.clone())).unwrap_or_default()
    }

    fn kind_of(&self, key: &str) -> Kind {
        self.cfg.icons.get(key).copied().or_else(|| self.devices.get(key).map(|d| d.kind)).unwrap_or(Kind::Other)
    }

    fn style(&self) -> Style {
        Style { light_taskbar: light_taskbar(), number_mode: self.cfg.number_mode }
    }

    /// Brings the tray in line with the device list: one icon per visible device, or a placeholder.
    fn sync(&mut self) {
        let style = self.style();
        let size = icon_size();
        let hwnd = self.hwnd;
        let keys: Vec<String> = self.devices.keys().cloned().collect();
        let mut visible = 0;
        for key in keys {
            let hidden = self.cfg.hidden.contains(&key);
            let name = self.name_of(&key);
            let kind = self.kind_of(&key);
            let d = self.devices.get_mut(&key).unwrap();
            unsafe {
                if hidden {
                    if d.shown {
                        delete_icon(hwnd, d.tray_id);
                        d.shown = false;
                    }
                    continue;
                }
                visible += 1;
                let spec = IconSpec { kind, level: d.level, charging: d.charging, connected: d.connected, placeholder: false };
                let icon = render::to_hicon(size, &render::render(size, &spec, style));
                let mut n = nid(hwnd, d.tray_id);
                n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
                n.hIcon = icon;
                copy_wide(&mut n.szTip, &tooltip(&name, d));
                show_icon(&mut n, &mut d.shown);
                if !d.icon.is_invalid() {
                    let _ = DestroyIcon(d.icon);
                }
                d.icon = icon;
            }
        }
        unsafe {
            if visible == 0 {
                let spec = IconSpec { kind: Kind::Other, level: 100, charging: false, connected: true, placeholder: true };
                let icon = render::to_hicon(size, &render::render(size, &spec, style));
                let mut n = nid(hwnd, FALLBACK_ID);
                n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
                n.hIcon = icon;
                let tip = if self.devices.is_empty() {
                    "Battery Tray\nNo devices with a battery found yet"
                } else {
                    "Battery Tray\nAll devices are hidden"
                };
                copy_wide(&mut n.szTip, tip);
                show_icon(&mut n, &mut self.fallback_shown);
                if !self.fallback_icon.is_invalid() {
                    let _ = DestroyIcon(self.fallback_icon);
                }
                self.fallback_icon = icon;
            } else if self.fallback_shown {
                delete_icon(hwnd, FALLBACK_ID);
                self.fallback_shown = false;
            }
        }
    }

    fn on_readings(&mut self, batch: Vec<Reading>) {
        let mut present = BTreeSet::new();
        let mut dirty = false;
        let when = now_label();
        for r in batch {
            present.insert(r.key.clone());
            let d = self.devices.entry(r.key.clone()).or_insert_with(|| Device {
                name: String::new(),
                kind: r.kind,
                level: 0,
                approx: false,
                charging: false,
                connected: false,
                when: String::new(),
                source: String::new(),
                tray_id: tray_id(&r.key),
                icon: HICON::default(),
                shown: false,
                alerted: false,
            });
            let changed = d.level != r.level || d.name != r.name || d.kind != r.kind || !d.connected;
            d.name = r.name.clone();
            d.kind = r.kind;
            d.level = r.level;
            d.approx = r.approx;
            d.charging = r.charging;
            d.connected = true;
            d.when = when.clone();
            d.source = r.source.to_string();
            if changed {
                dirty = true;
                self.cfg.seen.insert(
                    r.key.clone(),
                    Seen { name: r.name, kind: r.kind, level: r.level, approx: r.approx, when: when.clone(), source: r.source.to_string() },
                );
            }
        }
        for (k, d) in self.devices.iter_mut() {
            if !present.contains(k) {
                d.connected = false;
                d.charging = false;
            }
        }
        if dirty {
            self.cfg.save();
        }
        self.sync();
        self.check_alerts();
    }

    fn check_alerts(&mut self) {
        let thr = self.cfg.threshold;
        let mut due = Vec::new();
        for (k, d) in self.devices.iter_mut() {
            if d.charging || thr == 0 || d.level > thr.saturating_add(5) {
                d.alerted = false;
            } else if d.connected && d.level <= thr && !d.alerted && !self.cfg.hidden.contains(k) {
                d.alerted = true;
                due.push(k.clone());
            }
        }
        for key in due {
            let (name, kind) = (self.name_of(&key), self.kind_of(&key));
            let d = &self.devices[&key];
            let mut n = nid(self.hwnd, d.tray_id);
            n.uFlags = NIF_INFO;
            n.dwInfoFlags = NIIF_WARNING;
            copy_wide(&mut n.szInfoTitle, &format!("{} battery low", kind.label()));
            copy_wide(&mut n.szInfo, &format!("{name} is at {}.", level_text(d.level, d.approx)));
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
            }
        }
    }

    fn key_for_tray(&self, id: u32) -> Option<String> {
        self.devices.iter().find(|(_, d)| d.tray_id == id).map(|(k, _)| k.clone())
    }

    /// Builds the right-click menu. Returns it with the device key it's for and the hidden-device list.
    fn build_menu(&self, id: u32) -> Option<(HMENU, Option<String>, Vec<String>)> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            let key = self.key_for_tray(id);
            if let Some(k) = &key {
                let d = &self.devices[k];
                add_item(m, MF_STRING | MF_GRAYED, 0, &format!("{} · {}", self.name_of(k), level_text(d.level, d.approx)));
                add_item(m, MF_STRING, ID_RENAME, "Rename…");
                let sub = CreatePopupMenu().ok()?;
                let current = self.kind_of(k);
                for (i, kind) in Kind::ALL.iter().enumerate() {
                    add_item(sub, checked(*kind == current), ID_KIND + i, kind.label());
                }
                add_item(m, MF_POPUP, sub.0 as usize, "Change icon");
                add_item(m, MF_STRING, ID_HIDE, "Hide this device");
                add_item(m, MF_SEPARATOR, 0, "");
            }
            let hidden: Vec<String> = self.cfg.hidden.iter().cloned().collect();
            let sub = CreatePopupMenu().ok()?;
            if hidden.is_empty() {
                add_item(sub, MF_STRING | MF_GRAYED, 0, "None");
            }
            for (i, k) in hidden.iter().enumerate().take(99) {
                let name = self.name_of(k);
                add_item(sub, MF_STRING, ID_UNHIDE + i, if name.is_empty() { k } else { &name });
            }
            add_item(m, MF_POPUP, sub.0 as usize, "Show hidden devices");

            let sub = CreatePopupMenu().ok()?;
            add_item(sub, checked(!self.cfg.number_mode), ID_MODE_BAR, "Icon and level bar");
            add_item(sub, checked(self.cfg.number_mode), ID_MODE_NUM, "Number only");
            add_item(m, MF_POPUP, sub.0 as usize, "Display");

            let sub = CreatePopupMenu().ok()?;
            for (i, t) in THRESHOLDS.iter().enumerate() {
                let label = if *t == 0 { "Off".to_string() } else { format!("Below {t}%") };
                add_item(sub, checked(self.cfg.threshold == *t), ID_THRESHOLD + i, &label);
            }
            add_item(m, MF_POPUP, sub.0 as usize, "Low-battery alert");
            add_item(m, checked(autostart_enabled()), ID_AUTOSTART, "Start with Windows");
            add_item(m, MF_STRING, ID_REFRESH, "Refresh now");
            add_item(m, MF_SEPARATOR, 0, "");
            add_item(m, MF_STRING, ID_EXIT, "Exit");
            Some((m, key, hidden))
        }
    }
}

unsafe fn show_menu(hwnd: HWND, id: u32) {
    let Some(Some((menu, key, hidden))) = with_app(|a| a.build_menu(id)) else { return };
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, None).0 as usize;
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    if cmd != 0 {
        handle_command(hwnd, cmd, key, hidden);
    }
}

unsafe fn handle_command(hwnd: HWND, cmd: usize, key: Option<String>, hidden: Vec<String>) {
    match cmd {
        ID_RENAME => {
            let Some(k) = key else { return };
            let Some(current) = with_app(|a| a.name_of(&k)) else { return };
            // No app borrow held here: the prompt runs its own message loop.
            if let Some(new) = input::prompt("Rename device", "Name shown in the tooltip (leave empty to reset):", &current) {
                with_app(|a| {
                    if new.is_empty() {
                        a.cfg.names.remove(&k);
                    } else {
                        a.cfg.names.insert(k.clone(), new);
                    }
                    a.cfg.save();
                    a.sync();
                });
            }
        }
        ID_HIDE => {
            if let Some(k) = key {
                with_app(|a| {
                    a.cfg.hidden.insert(k);
                    a.cfg.save();
                    a.sync();
                });
            }
        }
        c if (ID_KIND..ID_KIND + Kind::ALL.len()).contains(&c) => {
            if let Some(k) = key {
                with_app(|a| {
                    a.cfg.icons.insert(k, Kind::ALL[c - ID_KIND]);
                    a.cfg.save();
                    a.sync();
                });
            }
        }
        c if (ID_UNHIDE..ID_UNHIDE + 99).contains(&c) => {
            if let Some(k) = hidden.get(c - ID_UNHIDE) {
                with_app(|a| {
                    a.cfg.hidden.remove(k);
                    a.cfg.save();
                    a.sync();
                });
            }
        }
        ID_MODE_BAR | ID_MODE_NUM => {
            with_app(|a| {
                a.cfg.number_mode = cmd == ID_MODE_NUM;
                a.cfg.save();
                a.sync();
            });
        }
        c if (ID_THRESHOLD..ID_THRESHOLD + THRESHOLDS.len()).contains(&c) => {
            with_app(|a| {
                a.cfg.threshold = THRESHOLDS[c - ID_THRESHOLD];
                a.cfg.save();
                for d in a.devices.values_mut() {
                    d.alerted = false;
                }
                a.check_alerts();
            });
        }
        ID_AUTOSTART => {
            if let Err(e) = set_autostart(!autostart_enabled()) {
                let text = wide(&format!("Couldn't change \"Start with Windows\": {e}."));
                MessageBoxW(hwnd, PCWSTR(text.as_ptr()), w!("Battery Tray"), MB_OK | MB_ICONWARNING);
            }
        }
        ID_REFRESH => {
            with_app(|a| a.refresh.send(()));
        }
        ID_EXIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tc = TASKBAR_CREATED.load(Ordering::Relaxed);
    if tc != 0 && msg == tc {
        with_app(|a| {
            for d in a.devices.values_mut() {
                d.shown = false;
            }
            a.fallback_shown = false;
            a.sync();
        });
        return LRESULT(0);
    }
    match msg {
        WM_TRAY => {
            let event = (lp.0 as u32) & 0xffff;
            let id = ((lp.0 as u32) >> 16) & 0xffff;
            if event == WM_CONTEXTMENU || event == NIN_SELECT || event == NIN_KEYSELECT {
                show_menu(hwnd, id);
            }
            LRESULT(0)
        }
        WM_READINGS => {
            with_app(|a| {
                let mut latest = None;
                while let Ok(b) = a.rx.try_recv() {
                    latest = Some(b);
                }
                if let Some(b) = latest {
                    a.on_readings(b);
                }
            });
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            with_app(|a| a.sync());
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DEVICECHANGE => {
            if wp.0 == DBT_DEVNODES_CHANGED {
                with_app(|a| a.refresh.send(()));
            }
            LRESULT(1)
        }
        WM_ENDSESSION => {
            with_app(|a| a.cfg.save());
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app(|a| {
                a.cfg.save();
                for d in a.devices.values() {
                    if d.shown {
                        delete_icon(hwnd, d.tray_id);
                    }
                }
                if a.fallback_shown {
                    delete_icon(hwnd, FALLBACK_ID);
                }
            });
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// `--scan <file>`: writes one scan's readings to a text file (handy for diagnosing devices).
fn scan_to_file(path: &str) {
    let mut out = String::new();
    for r in providers::poll_all() {
        out += &format!("{}\t{}\t{}\t{}\t{}{}\n", r.key, r.name, r.kind.id(), level_text(r.level, r.approx), r.source, if r.charging { "\tcharging" } else { "" });
    }
    if out.is_empty() {
        out = "no devices with a battery found\n".into();
    }
    let _ = std::fs::write(path, out);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--preview") => {
            let _ = render::write_preview(args.get(2).map(String::as_str).unwrap_or("preview.bmp"));
            return;
        }
        Some("--scan") => {
            scan_to_file(args.get(2).map(String::as_str).unwrap_or("scan.txt"));
            return;
        }
        _ => {}
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _mutex = CreateMutexW(None, true, w!("Local\\BatteryTray.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = w!("BatteryTrayWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            lpszClassName: class,
            hIcon: LoadIconW(inst, util::APP_ICON).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Battery Tray"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, inst, None) else {
            return;
        };
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let (tx, rx) = mpsc::channel::<Vec<Reading>>();
        let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
        let cfg = Config::load();
        // Start with last-known levels (dimmed) so icons appear immediately and survive sleeping devices.
        let devices = cfg
            .seen
            .iter()
            .map(|(k, s)| {
                (
                    k.clone(),
                    Device {
                        name: s.name.clone(),
                        kind: s.kind,
                        level: s.level,
                        approx: s.approx,
                        charging: false,
                        connected: false,
                        when: s.when.clone(),
                        source: s.source.clone(),
                        tray_id: tray_id(k),
                        icon: HICON::default(),
                        shown: false,
                        alerted: false,
                    },
                )
            })
            .collect();
        let app = App { hwnd, cfg, devices, rx, refresh: refresh_tx, fallback_icon: HICON::default(), fallback_shown: false };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(|a| a.sync());

        let hw = hwnd.0 as isize;
        std::thread::spawn(move || loop {
            let readings = providers::poll_all();
            if tx.send(readings).is_err() {
                break;
            }
            let _ = PostMessageW(HWND(hw as _), WM_READINGS, WPARAM(0), LPARAM(0));
            match refresh_rx.recv_timeout(POLL_EVERY) {
                Ok(()) => {
                    // Device changes come in bursts; wait a moment and coalesce them.
                    std::thread::sleep(Duration::from_millis(1500));
                    while refresh_rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
