//! Reads the file list (CF_HDROP) and the cut/copy intent from the clipboard.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use crate::paste_plan::PasteMode;

/// Standard clipboard format id for a file list.
pub const CF_HDROP: u32 = 15;
const DROPEFFECT_MOVE: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFiles {
    pub paths: Vec<PathBuf>,
    pub mode: PasteMode,
    /// Clipboard sequence number read while the clipboard was open, i.e. the
    /// number of exactly the content in `paths`.
    pub sequence: u32,
}

/// Closes the clipboard when dropped.
struct OpenedClipboard;

impl Drop for OpenedClipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// The clipboard can be briefly locked by another process: retry a few times.
fn open_clipboard() -> Option<OpenedClipboard> {
    for _ in 0..6 {
        if unsafe { OpenClipboard(Some(HWND::default())) }.is_ok() {
            return Some(OpenedClipboard);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Changes whenever the clipboard content changes.
pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Files on the clipboard with their copy/cut intent, or `None` when there is
/// no file list or the clipboard cannot be opened.
pub fn read_clipboard() -> Option<ClipboardFiles> {
    let _guard = open_clipboard()?;
    unsafe {
        let data = GetClipboardData(CF_HDROP).ok()?;
        let hdrop = HDROP(data.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            paths.push(PathBuf::from(OsString::from_wide(&buf[..got])));
        }
        Some(ClipboardFiles { paths, mode: read_drop_effect(), sequence: GetClipboardSequenceNumber() })
    }
}

/// `Preferred DropEffect` with the MOVE bit means cut; anything else is copy.
unsafe fn read_drop_effect() -> PasteMode {
    unsafe {
        let fmt = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        if fmt == 0 {
            return PasteMode::Copy;
        }
        let Ok(h) = GetClipboardData(fmt) else { return PasteMode::Copy };
        let hglobal = windows::Win32::Foundation::HGLOBAL(h.0);
        if GlobalSize(hglobal) < 4 {
            return PasteMode::Copy;
        }
        let p = GlobalLock(hglobal) as *const u32;
        if p.is_null() {
            return PasteMode::Copy;
        }
        let effect = p.read_unaligned();
        let _ = GlobalUnlock(hglobal);
        if effect & DROPEFFECT_MOVE != 0 { PasteMode::Move } else { PasteMode::Copy }
    }
}

/// Empties the clipboard (after a cut was handed to robocopy).
pub fn empty_clipboard() {
    if let Some(_guard) = open_clipboard() {
        unsafe {
            let _ = EmptyClipboard();
        }
    }
}

