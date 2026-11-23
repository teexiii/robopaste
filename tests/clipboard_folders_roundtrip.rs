//! Puts a folder on the real clipboard and reads it back.
//! `#[ignore]`: it overwrites the user's clipboard.
//! Run with: cargo test --test clipboard_folders_roundtrip -- --ignored

use std::path::PathBuf;

use robopaste::clipboard_folders::{empty_clipboard, read_clipboard, CF_HDROP};
use robopaste::paste_plan::PasteMode;
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::DROPFILES;

unsafe fn put_hdrop(path: &PathBuf, effect: u32) {
    unsafe {
        OpenClipboard(Some(HWND::default())).unwrap();
        EmptyClipboard().unwrap();
        // DROPFILES header + UTF-16 path + double NUL.
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0, 0]).collect();
        let header = std::mem::size_of::<DROPFILES>();
        let h = GlobalAlloc(GMEM_MOVEABLE, header + wide.len() * 2).unwrap();
        let p = GlobalLock(h) as *mut u8;
        let df = DROPFILES { pFiles: header as u32, fWide: true.into(), ..Default::default() };
        std::ptr::copy_nonoverlapping(&df as *const _ as *const u8, p, header);
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, p.add(header), wide.len() * 2);
        let _ = GlobalUnlock(h);
        SetClipboardData(CF_HDROP, Some(HANDLE(h.0))).unwrap();

        let fmt = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        let he = GlobalAlloc(GMEM_MOVEABLE, 4).unwrap();
        let pe = GlobalLock(he) as *mut u32;
        pe.write(effect);
        let _ = GlobalUnlock(he);
        SetClipboardData(fmt, Some(HANDLE(he.0))).unwrap();
        CloseClipboard().unwrap();
    }
}

#[test]
#[ignore = "overwrites the user's clipboard"]
fn folder_and_drop_effect_survive_a_clipboard_round_trip() {
    let dir = std::env::temp_dir().join(format!("rp-clip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { put_hdrop(&dir, 2) };
    let got = read_clipboard().expect("clipboard has files");
    assert_eq!(got.paths, vec![dir.clone()]);
    assert_eq!(got.mode, PasteMode::Move);
    unsafe { put_hdrop(&dir, 5) };
    assert_eq!(read_clipboard().unwrap().mode, PasteMode::Copy);
    empty_clipboard();
    assert!(read_clipboard().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
