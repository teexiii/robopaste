//! Resolves the filesystem folder shown by an Explorer window's *active tab*.
//!
//! These are cross-process COM calls: they must never run inside the keyboard
//! hook callback. The caller owns COM initialisation (apartment-threaded) on
//! the calling thread.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, MAX_PATH};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IServiceProvider, CLSCTX_ALL};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, IFolderView, IPersistFolder2, IShellBrowser, IShellWindows, IWebBrowserApp, SHGetKnownFolderPath,
    SHGetPathFromIDListEx, ShellWindows, GPFIDL_DEFAULT, KF_FLAG_DEFAULT, SID_STopLevelBrowser,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW};

/// Class name of a window, empty string on failure.
pub fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Folder shown by `top` (an Explorer window or the Desktop), or `None` for
/// virtual folders, non-directories and any failure.
pub fn resolve_destination(top: HWND) -> Option<PathBuf> {
    let path = match window_class_name(top).as_str() {
        "Progman" | "WorkerW" => desktop_folder(),
        "CabinetWClass" => explorer_active_tab_folder(top),
        _ => None,
    }?;
    // The root of an opened .zip can resolve to the zip file itself.
    path.is_dir().then_some(path)
}

fn desktop_folder() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn explorer_active_tab_folder(top: HWND) -> Option<PathBuf> {
    unsafe {
        // Windows 11 tabs: every tab of one window shares the top-level HWND,
        // each tab owns a ShellTabWindowClass child; the active one is first
        // in z-order. No such child (older Explorer) -> match by HWND only.
        let active_tab = FindWindowExW(Some(top), None, w!("ShellTabWindowClass"), PCWSTR::null())
            .ok()
            .filter(|h| !h.is_invalid());

        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        let count = windows.Count().ok()?;
        // Without a tab child the top HWND must identify exactly one item;
        // several matches (tabs) cannot be told apart, so fail closed.
        let mut fallback: Option<IShellBrowser> = None;
        let mut fallback_matches = 0u32;
        for i in 0..count {
            let Ok(disp) = windows.Item(&VARIANT::from(i)) else { continue };
            let Ok(app) = disp.cast::<IWebBrowserApp>() else { continue };
            let Ok(h) = app.HWND() else { continue };
            if h.0 != top.0 as isize {
                continue;
            }
            let Ok(sp) = disp.cast::<IServiceProvider>() else { continue };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
            match active_tab {
                Some(tab) => match browser.GetWindow() {
                    Ok(w) if w == tab => return folder_path_of_browser(&browser),
                    _ => continue,
                },
                None => {
                    fallback_matches += 1;
                    fallback = Some(browser);
                }
            }
        }
        match (fallback_matches, fallback) {
            (1, Some(browser)) => folder_path_of_browser(&browser),
            _ => None,
        }
    }
}

unsafe fn folder_path_of_browser(browser: &IShellBrowser) -> Option<PathBuf> {
    unsafe {
        let view = browser.QueryActiveShellView().ok()?;
        let fv = view.cast::<IFolderView>().ok()?;
        let folder = fv.GetFolder::<IPersistFolder2>().ok()?;
        let pidl: *mut ITEMIDLIST = folder.GetCurFolder().ok()?;
        let mut buf = [0u16; MAX_PATH as usize * 2];
        let ok = SHGetPathFromIDListEx(pidl, &mut buf, GPFIDL_DEFAULT);
        CoTaskMemFree(Some(pidl as *const _));
        if !ok.as_bool() {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(OsString::from_wide(&buf[..len])))
    }
}
