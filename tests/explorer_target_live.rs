//! Live test against a real Explorer window; needs an interactive desktop.
//! Run with: cargo test --test explorer_target_live -- --ignored

use std::path::PathBuf;
use std::time::{Duration, Instant};

use robopaste::explorer_target::{resolve_destination, window_class_name};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, PostMessageW, WM_CLOSE};

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    if window_class_name(hwnd) == "CabinetWClass" {
        list.push(hwnd);
    }
    BOOL(1)
}

fn explorer_windows() -> Vec<HWND> {
    let mut list: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

fn same(a: &PathBuf, b: &PathBuf) -> bool {
    let (a, b) = (a.to_string_lossy(), b.to_string_lossy());
    a.trim_end_matches('\\').eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

#[test]
#[ignore = "needs an interactive desktop session"]
fn resolves_folder_of_a_freshly_opened_explorer_window() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let dir = std::env::temp_dir().join(format!("rp-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::process::Command::new("explorer.exe").arg(&dir).spawn().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found: Option<HWND> = None;
    while Instant::now() < deadline && found.is_none() {
        std::thread::sleep(Duration::from_millis(300));
        found = explorer_windows()
            .into_iter()
            .find(|h| resolve_destination(*h).is_some_and(|p| same(&p, &dir)));
    }
    if let Some(h) = found {
        unsafe {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(found.is_some(), "no Explorer window resolved to {}", dir.display());
}

#[test]
#[ignore = "needs an interactive desktop session"]
fn desktop_window_resolves_to_the_user_desktop_folder() {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let progman = FindWindowW(w!("Progman"), None).expect("Progman window");
        let got = resolve_destination(progman).expect("desktop folder");
        let want = PathBuf::from(std::env::var("USERPROFILE").unwrap()).join("Desktop");
        // OneDrive-redirected desktops differ from %USERPROFILE%\Desktop: only require a real directory ending in Desktop.
        assert!(got.is_dir() && got.ends_with("Desktop"), "got {} (profile desktop {})", got.display(), want.display());
    }
}
