//! Tray icon, popup menu, single-instance mutex and per-user autostart.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM, ERROR_ALREADY_EXISTS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, IDI_APPLICATION, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    MSG, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPED,
};

use crate::keyboard_hook::{self, ENABLED};

const WM_TRAY: u32 = WM_APP + 1;
const ID_ENABLED: usize = 1;
const ID_AUTOSTART: usize = 2;
const ID_EXIT: usize = 3;
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Name of the Run value of the real installation.
const RUN_VALUE_NAME: &str = "robopaste";

/// Registered `TaskbarCreated` message id (Explorer restarted: re-add the icon).
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// Takes the named single-instance mutex. `false` means another instance runs.
/// The handle is intentionally kept open for the life of the process.
pub fn acquire_single_instance() -> bool {
    unsafe {
        let Ok(_handle) = CreateMutexW(None, true, w!(r"Local\robopaste-single-instance")) else {
            return false;
        };
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn quoted_exe_path() -> Option<String> {
    std::env::current_exe().ok().map(|p| format!("\"{}\"", p.display()))
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Run value exists and points at this exe.
pub fn autostart_enabled() -> bool {
    autostart_enabled_as(RUN_VALUE_NAME)
}

/// Same as `autostart_enabled` for a Run value of another name (tests use their own).
pub fn autostart_enabled_as(value_name: &str) -> bool {
    let value = wide_z(value_name);
    let Some(want) = quoted_exe_path() else { return false };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_QUERY_VALUE, &mut key).is_err() {
            return false;
        }
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = RegQueryValueExW(key, PCWSTR(value.as_ptr()), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut bytes));
        let _ = RegCloseKey(key);
        if rc.is_err() {
            return false;
        }
        let len = (bytes as usize / 2).min(buf.len());
        let got = String::from_utf16_lossy(&buf[..len]);
        got.trim_end_matches('\0').eq_ignore_ascii_case(&want)
    }
}

/// Writes or removes `HKCU\...\Run\robopaste`. Per-user only; no elevation.
pub fn set_autostart(on: bool) {
    set_autostart_as(RUN_VALUE_NAME, on)
}

/// Same as `set_autostart` for a Run value of another name (tests use their own).
pub fn set_autostart_as(value_name: &str, on: bool) {
    let value = wide_z(value_name);
    let value = PCWSTR(value.as_ptr());
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            if let Some(path) = quoted_exe_path() {
                let wide = wide_z(&path);
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                let _ = RegSetValueExW(key, value, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, value);
        }
        let _ = RegCloseKey(key);
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    nid.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
    for (i, c) in "robopaste".encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    nid
}

fn add_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd));
    }
}

fn remove_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
}

/// Check marks are computed each time the menu opens.
fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let enabled = ENABLED.load(Ordering::SeqCst) && keyboard_hook::is_installed();
        let flag = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, flag(enabled), ID_ENABLED, w!("Enabled"));
        let _ = AppendMenuW(menu, flag(autostart_enabled()), ID_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        // The menu only dismisses correctly when its owner is the foreground window.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn toggle_enabled() {
    let now_enabled = !(ENABLED.load(Ordering::SeqCst) && keyboard_hook::is_installed());
    ENABLED.store(now_enabled, Ordering::SeqCst);
    if now_enabled {
        // Re-installing is also the recovery path when Windows dropped a timed-out hook.
        keyboard_hook::uninstall_hook();
        let _ = keyboard_hook::install_hook();
    } else {
        keyboard_hook::uninstall_hook();
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let mouse = (lparam.0 as u32) & 0xFFFF;
            if mouse == WM_LBUTTONUP || mouse == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xFFFF {
                ID_ENABLED => toggle_enabled(),
                ID_AUTOSTART => set_autostart(!autostart_enabled()),
                ID_EXIT => {
                    remove_icon(hwnd);
                    keyboard_hook::uninstall_hook();
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Also reached when the window is closed from outside, not only via Exit.
            remove_icon(hwnd);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            add_icon(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Creates the hidden window and tray icon, runs the message loop until Exit.
pub fn run_tray() -> windows::core::Result<()> {
    unsafe {
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::SeqCst);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: w!("RobopasteTrayWindow"),
            ..Default::default()
        };
        RegisterClassW(&class);
        // A real (never shown) top-level window: message-only windows do not
        // receive the TaskbarCreated broadcast.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("robopaste"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        add_icon(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
