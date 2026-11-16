//! A tiny modal text prompt (used for "Rename…"), built from plain Win32 controls.

use crate::util::{from_wide, wide};
use std::cell::{Cell, RefCell};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_EDIT: i32 = 10;
const IDOK_: usize = 1;
const IDCANCEL_: usize = 2;
const EM_SETSEL: u32 = 0x00B1;

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
    static RESULT: RefCell<Option<String>> = const { RefCell::new(None) };
    static EDIT: Cell<isize> = const { Cell::new(0) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = wp.0 & 0xffff;
            if id == IDOK_ {
                let edit = HWND(EDIT.get() as _);
                let mut buf = [0u16; 128];
                GetWindowTextW(edit, &mut buf);
                RESULT.with(|r| *r.borrow_mut() = Some(from_wide(&buf).trim().to_string()));
                DONE.set(true);
            } else if id == IDCANCEL_ {
                DONE.set(true);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            DONE.set(true);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Shows the prompt and returns the entered text, or None when cancelled.
pub fn prompt(title: &str, label: &str, initial: &str) -> Option<String> {
    unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = w!("BatteryTrayPrompt");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: inst,
            lpszClassName: class,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(inst, crate::util::APP_ICON).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc); // fails harmlessly when already registered

        let dpi = GetDpiForSystem() as i32;
        let s = |v: i32| v * dpi / 96;
        let (w, h) = (s(340), s(150));
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let title_w = wide(title);
        let dlg = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST | WS_EX_CONTROLPARENT,
            class,
            PCWSTR(title_w.as_ptr()),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            (sw - w) / 2,
            (sh - h) / 2,
            w,
            h,
            None,
            None,
            inst,
            None,
        )
        .ok()?;

        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let _ = SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, ncm.cbSize, Some(&mut ncm as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let font = CreateFontIndirectW(&ncm.lfMessageFont);

        let child = |cls: PCWSTR, text: &str, style: u32, x: i32, y: i32, cw: i32, ch: i32, id: i32, ex: WINDOW_EX_STYLE| {
            let t = wide(text);
            let hw = CreateWindowExW(
                ex,
                cls,
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                x,
                y,
                cw,
                ch,
                dlg,
                HMENU(id as _),
                inst,
                None,
            )
            .unwrap_or_default();
            SendMessageW(hw, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            hw
        };
        let inner = w - s(16) - s(24);
        child(w!("STATIC"), label, 0, s(12), s(12), inner, s(20), 0, WINDOW_EX_STYLE(0));
        let edit = child(w!("EDIT"), initial, WS_TABSTOP.0 | ES_AUTOHSCROLL as u32, s(12), s(34), inner, s(24), ID_EDIT, WS_EX_CLIENTEDGE);
        child(w!("BUTTON"), "OK", WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, w - s(16) - s(12) - s(170), s(70), s(80), s(26), IDOK_ as i32, WINDOW_EX_STYLE(0));
        child(w!("BUTTON"), "Cancel", WS_TABSTOP.0 | BS_PUSHBUTTON as u32, w - s(16) - s(12) - s(84), s(70), s(80), s(26), IDCANCEL_ as i32, WINDOW_EX_STYLE(0));
        EDIT.set(edit.0 as isize);
        DONE.set(false);
        RESULT.with(|r| *r.borrow_mut() = None);

        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
        let _ = SetFocus(edit);
        SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));

        let mut msg = MSG::default();
        while !DONE.get() {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                PostQuitMessage(0);
                break;
            }
            if !IsDialogMessageW(dlg, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = DestroyWindow(dlg);
        let _ = DeleteObject(font);
        RESULT.with(|r| r.borrow_mut().take())
    }
}
