//! The Start with Windows toggle writes and removes the per-user Run value.
//! Uses a test-only value name; the real `robopaste` value is never touched.

use std::process::Command;

use robopaste::tray_icon::{autostart_enabled_as, set_autostart_as};

const TEST_VALUE: &str = "robopaste-test-autostart";

fn run_value_exists() -> bool {
    Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", TEST_VALUE])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Removes the test Run value when dropped, also when an assertion panics.
struct RemoveRunValue;

impl Drop for RemoveRunValue {
    fn drop(&mut self) {
        set_autostart_as(TEST_VALUE, false);
    }
}

#[test]
fn autostart_toggle_writes_and_removes_the_run_value() {
    let _cleanup = RemoveRunValue;
    set_autostart_as(TEST_VALUE, true);
    assert!(run_value_exists(), "reg query must see the value");
    assert!(autostart_enabled_as(TEST_VALUE), "value must point at this exe");
    set_autostart_as(TEST_VALUE, false);
    assert!(!run_value_exists());
    assert!(!autostart_enabled_as(TEST_VALUE));
}
