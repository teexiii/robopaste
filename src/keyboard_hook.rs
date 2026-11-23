//! Low-level keyboard hook: swallows Ctrl+V only in an Explorer file list
//! (or the Desktop) while files are on the clipboard, and hands the real work
//! to the paste worker thread.
//!
//! The callback must return fast and must not make cross-process COM calls.
//! It does only cheap local checks. The only inspected key is `V` plus the
//! modifier state; nothing is logged or stored.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetParent,
    GetWindowThreadProcessId, SetWindowsHookExW, UnhookWindowsHookEx, GA_PARENT, GUITHREADINFO, HC_ACTION, HHOOK,
    KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYUP,
};

use crate::clipboard_folders::CF_HDROP;
use crate::paste_worker::{now_tick_ms, PasteRequest};

/// Tag on `dwExtraInfo` of every key event robopaste injects ("ROBO").
pub const ROBOPASTE_MARKER: usize = 0x524F_424F;
const VK_V: u32 = 0x56;

/// Set by the tray (Enabled toggle); read by the callback.
pub static ENABLED: AtomicBool = AtomicBool::new(true);
/// A request is queued or being processed by the worker.
pub static PENDING: AtomicBool = AtomicBool::new(false);
/// Incremented each time an eligible V press is let through because the worker was busy.
pub static BYPASS_COUNT: AtomicU32 = AtomicU32::new(0);
/// A V keydown was swallowed and its keyup has not been seen yet (auto-repeat guard).
static V_SWALLOWED: AtomicBool = AtomicBool::new(false);
/// Foreground window recorded when `V_SWALLOWED` was set; the guard only holds while it stays foreground.
static V_SWALLOWED_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
static HOOK: AtomicIsize = AtomicIsize::new(0);
static WORKER: OnceLock<Sender<PasteRequest>> = OnceLock::new();

/// Registers the worker channel; call once before installing the hook.
pub fn set_worker(tx: Sender<PasteRequest>) {
    let _ = WORKER.set(tx);
}

/// Installs the hook (no-op when already installed). Also clears the
/// swallowed-key flag so a keyup missed while the hook was off cannot leave it stuck.
pub fn install_hook() -> windows::core::Result<()> {
    V_SWALLOWED.store(false, Ordering::SeqCst);
    if HOOK.load(Ordering::SeqCst) != 0 {
        return Ok(());
    }
    unsafe {
        let module = GetModuleHandleW(None)?;
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(module.into()), 0)?;
        HOOK.store(hook.0 as isize, Ordering::SeqCst);
    }
    Ok(())
}

pub fn uninstall_hook() {
    let raw = HOOK.swap(0, Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
        }
    }
}

pub fn is_installed() -> bool {
    HOOK.load(Ordering::SeqCst) != 0
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let next = || unsafe { CallNextHookEx(None, code, wparam, lparam) };
    if code != HC_ACTION as i32 {
        return next();
    }
    let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // Our own replayed keys always pass: this is the first test.
    if kb.dwExtraInfo == ROBOPASTE_MARKER || kb.vkCode != VK_V {
        return next();
    }
    let msg = wparam.0 as u32;
    if msg == WM_KEYUP || msg == WM_SYSKEYUP {
        V_SWALLOWED.store(false, Ordering::SeqCst);
        return next();
    }
    if msg != WM_KEYDOWN {
        return next();
    }
    // Auto-repeat of a press we already took over: swallow without a new request.
    // A missed keyup would leave the guard set: it only holds while the window
    // recorded with it is still foreground and Ctrl is still down.
    if V_SWALLOWED.load(Ordering::SeqCst) {
        let same_window = unsafe { GetForegroundWindow() }.0 as isize == V_SWALLOWED_FOREGROUND.load(Ordering::SeqCst);
        if same_window && key_down(VK_CONTROL) {
            return LRESULT(1);
        }
        V_SWALLOWED.store(false, Ordering::SeqCst);
    }
    let Some(foreground) = eligible_foreground() else {
        return next();
    };
    // Worker busy (for example stuck on a dead network path): Explorer must
    // never be blocked. Count the bypass so the worker drops its stale request.
    if PENDING.swap(true, Ordering::SeqCst) {
        BYPASS_COUNT.fetch_add(1, Ordering::SeqCst);
        return next();
    }
    let sent = WORKER.get().is_some_and(|tx| tx.send(PasteRequest { foreground: foreground.0 as isize, created_tick_ms: now_tick_ms() }).is_ok());
    if !sent {
        PENDING.store(false, Ordering::SeqCst);
        return next();
    }
    V_SWALLOWED_FOREGROUND.store(foreground.0 as isize, Ordering::SeqCst);
    V_SWALLOWED.store(true, Ordering::SeqCst);
    LRESULT(1)
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    // GetAsyncKeyState: GetKeyState reflects this thread's queue, stale in a hook.
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16) & 0x8000 != 0
}

/// Rules 2-6: returns the foreground window when this Ctrl+V should be taken over.
fn eligible_foreground() -> Option<HWND> {
    if !ENABLED.load(Ordering::SeqCst) {
        return None;
    }
    if !key_down(VK_CONTROL) || key_down(VK_SHIFT) || key_down(VK_MENU) || key_down(VK_LWIN) || key_down(VK_RWIN) {
        return None;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return None;
    }
    if !(class_is(fg, "CabinetWClass") || class_is(fg, "Progman") || class_is(fg, "WorkerW")) {
        return None;
    }
    if !focus_is_in_file_list(fg) {
        return None;
    }
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_err() {
        return None;
    }
    Some(fg)
}

/// Allow-list: the focused control is not an `Edit` and sits inside `SHELLDLL_DefView`.
fn focus_is_in_file_list(fg: HWND) -> bool {
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let thread = unsafe { GetWindowThreadProcessId(fg, None) };
    if thread == 0 || unsafe { GetGUIThreadInfo(thread, &mut info) }.is_err() {
        return false;
    }
    let focus = info.hwndFocus;
    if focus.is_invalid() || class_is(focus, "Edit") {
        return false;
    }
    let mut cur = focus;
    for _ in 0..16 {
        if class_is(cur, "SHELLDLL_DefView") {
            return true;
        }
        let parent = unsafe { GetParent(cur) }.unwrap_or_default();
        let parent = if parent.is_invalid() { unsafe { GetAncestor(cur, GA_PARENT) } } else { parent };
        if parent.is_invalid() {
            return false;
        }
        cur = parent;
    }
    false
}

/// Class name equals `expected`, compared without allocating.
fn class_is(hwnd: HWND, expected: &str) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    n > 0 && buf[..n as usize].iter().copied().eq(expected.encode_utf16())
}
