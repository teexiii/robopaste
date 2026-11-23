//! Worker thread behind the keyboard hook. The hook swallows Ctrl+V cheaply;
//! this thread does the expensive checks (clipboard, COM, filesystem) and
//! either launches robocopy or replays the keystroke to Explorer.
//!
//! Fail closed: any error while checking means "give the paste back to Explorer".

use std::fs;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::clipboard_folders::{empty_clipboard, read_clipboard, sequence_number};
use crate::explorer_target::resolve_destination;
use crate::keyboard_hook::{BYPASS_COUNT, PENDING, ROBOPASTE_MARKER};
use crate::path_safety::check_sources_and_destination;
use crate::paste_plan::{paths_equal, plan_paste, PasteDecision, PasteMode};
use crate::robocopy_runner::launch_runner;

/// What the hook hands to the worker. The window is kept as a raw value so the
/// request is `Send`.
pub struct PasteRequest {
    pub foreground: isize,
    /// `GetTickCount64` when the hook swallowed the key press.
    pub created_tick_ms: u64,
}

/// Milliseconds after which the user is assumed to have moved on.
pub const MAX_REQUEST_AGE_MS: u64 = 2000;

/// Tick count for stamping a request; cheap and non-blocking, fine in the hook.
pub fn now_tick_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// What to do with a request once the slow checks are done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestVerdict {
    Proceed,
    /// Older than `MAX_REQUEST_AGE_MS`: neither launch nor replay.
    DropStale,
    /// The window now shows another folder (or none): neither launch nor replay.
    DropDestinationChanged,
}

/// Pure decision: a request proceeds only when it is fresh and the destination
/// resolved again equals the one resolved at the start.
pub fn judge_request(age_ms: u64, first_dest: &Path, second_dest: Option<&Path>) -> RequestVerdict {
    if is_stale(age_ms) {
        return RequestVerdict::DropStale;
    }
    match second_dest {
        Some(d) if paths_equal(first_dest, d) => RequestVerdict::Proceed,
        _ => RequestVerdict::DropDestinationChanged,
    }
}

pub fn is_stale(age_ms: u64) -> bool {
    age_ms > MAX_REQUEST_AGE_MS
}

fn age_ms(req: &PasteRequest) -> u64 {
    now_tick_ms().saturating_sub(req.created_tick_ms)
}

/// Clears the busy flag when dropped, so a panic cannot leave it stuck.
struct PendingGuard;

impl Drop for PendingGuard {
    fn drop(&mut self) {
        PENDING.store(false, Ordering::SeqCst);
    }
}

/// Starts the worker thread and returns the channel the hook sends to.
pub fn start_worker() -> Sender<PasteRequest> {
    let (tx, rx) = channel::<PasteRequest>();
    std::thread::Builder::new()
        .name("robopaste-worker".into())
        .spawn(move || worker_loop(rx))
        .expect("spawn worker thread");
    tx
}

fn worker_loop(rx: Receiver<PasteRequest>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    for req in rx {
        let _guard = PendingGuard;
        // A panic in one request must not kill the worker.
        let _ = std::panic::catch_unwind(|| handle(&req));
    }
    unsafe { CoUninitialize() };
}

enum Outcome {
    Launched,
    PassThrough,
    /// The user moved on (stale request or other folder): do nothing at all.
    Dropped,
}

fn handle(req: &PasteRequest) {
    let bypass_at_start = BYPASS_COUNT.load(Ordering::SeqCst);
    let outcome = decide_and_launch(req, bypass_at_start);
    // A bypass while this request was processed means Explorer already
    // handled a paste of the same clipboard: drop the request entirely.
    if BYPASS_COUNT.load(Ordering::SeqCst) != bypass_at_start {
        return;
    }
    // A pass-through decided too late must not hit whatever the window shows now.
    if let Outcome::PassThrough = outcome {
        if is_stale(age_ms(req)) {
            return;
        }
        replay_paste(HWND(req.foreground as *mut _));
    }
}

/// Steps: destination (before any blocking source I/O), clipboard, folders
/// only, safety gate, plan, freshness and destination re-check, launch.
fn decide_and_launch(req: &PasteRequest, bypass_at_start: u32) -> Outcome {
    let hwnd = HWND(req.foreground as *mut _);
    let Some(dest) = resolve_destination(hwnd) else { return Outcome::PassThrough };
    let Some(clip) = read_clipboard() else { return Outcome::PassThrough };
    let seq = clip.sequence;
    if clip.paths.is_empty() {
        return Outcome::PassThrough;
    }
    // Every clipboard entry must be a real directory; any file hands back to Explorer.
    for p in &clip.paths {
        match fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => {}
            _ => return Outcome::PassThrough,
        }
    }
    if check_sources_and_destination(&clip.paths, &dest).is_err() {
        return Outcome::PassThrough;
    }
    // Lexical second layer on the original (never resolved) paths.
    if !matches!(plan_paste(&clip.paths, &dest, clip.mode), PasteDecision::Run(_)) {
        return Outcome::PassThrough;
    }
    // Explorer handled this paste already (busy bypass): do not also launch.
    if BYPASS_COUNT.load(Ordering::SeqCst) != bypass_at_start {
        return Outcome::PassThrough;
    }
    match judge_request(age_ms(req), &dest, resolve_destination(hwnd).as_deref()) {
        RequestVerdict::Proceed => {}
        RequestVerdict::DropStale | RequestVerdict::DropDestinationChanged => return Outcome::Dropped,
    }
    if launch_runner(&clip.paths, &dest, clip.mode).is_err() {
        return Outcome::PassThrough;
    }
    if clip.mode == PasteMode::Move && sequence_number() == seq {
        empty_clipboard();
    }
    Outcome::Launched
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: ROBOPASTE_MARKER,
            },
        },
    }
}

/// Gives the swallowed paste back to Explorer, tagged so the hook lets it
/// through. Does nothing when the user already switched window.
fn replay_paste(original_foreground: HWND) {
    if unsafe { GetForegroundWindow() } != original_foreground {
        return;
    }
    const V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
    let ctrl_held = (unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16) & 0x8000 != 0;
    let inputs: Vec<INPUT> = if ctrl_held {
        vec![key_event(V, false), key_event(V, true)]
    } else {
        vec![key_event(VK_CONTROL, false), key_event(V, false), key_event(V, true), key_event(VK_CONTROL, true)]
    };
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_request_with_same_destination_proceeds() {
        let d = Path::new(r"D:\x");
        assert_eq!(judge_request(0, d, Some(Path::new(r"d:\X"))), RequestVerdict::Proceed);
        assert_eq!(judge_request(MAX_REQUEST_AGE_MS, d, Some(d)), RequestVerdict::Proceed);
    }

    #[test]
    fn request_older_than_two_seconds_is_dropped() {
        let d = Path::new(r"D:\x");
        assert_eq!(judge_request(MAX_REQUEST_AGE_MS + 1, d, Some(d)), RequestVerdict::DropStale);
        assert!(is_stale(2001));
        assert!(!is_stale(2000));
    }

    #[test]
    fn changed_or_unresolvable_destination_is_dropped() {
        let d = Path::new(r"D:\x");
        assert_eq!(judge_request(10, d, Some(Path::new(r"D:\y"))), RequestVerdict::DropDestinationChanged);
        assert_eq!(judge_request(10, d, None), RequestVerdict::DropDestinationChanged);
    }
}
