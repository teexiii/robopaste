//! robopaste: replaces Explorer's folder paste with robocopy.
//! Module declarations only; see AGENTS.md for the module map.

pub mod clipboard_folders;
pub mod explorer_target;
pub mod keyboard_hook;
pub mod paste_worker;
pub mod path_safety;
pub mod paste_plan;
pub mod robocopy_runner;
pub mod tray_icon;
