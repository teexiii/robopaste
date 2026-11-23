#![windows_subsystem = "windows"]

use std::ffi::OsString;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("--where") => where_mode(),
        Some("--run") => std::process::exit(robopaste::robocopy_runner::run_entry(&args[1..])),
        _ => tray_mode(),
    }
}

/// Default mode: single instance, worker thread, keyboard hook, tray icon.
fn tray_mode() {
    use robopaste::{keyboard_hook, paste_worker, tray_icon};
    if !tray_icon::acquire_single_instance() {
        return;
    }
    keyboard_hook::set_worker(paste_worker::start_worker());
    if keyboard_hook::install_hook().is_err() {
        return;
    }
    let _ = tray_icon::run_tray();
    keyboard_hook::uninstall_hook();
}

/// Debug mode: after 3 s print the folder resolved for the foreground window.
/// Output goes to the parent console (the exe has the GUI subsystem).
fn where_mode() {
    use robopaste::explorer_target::resolve_destination;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        std::thread::sleep(std::time::Duration::from_secs(3));
        match resolve_destination(GetForegroundWindow()) {
            Some(p) => println!("{}", p.display()),
            None => println!("none"),
        }
    }
}
