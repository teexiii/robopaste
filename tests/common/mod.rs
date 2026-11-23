//! Shared helpers for integration tests: throwaway folders under %TEMP% and junctions.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Unique scratch folder, removed (links not followed) on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rp-test-{}-{}-{}", label, std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn file(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // remove_dir_all removes a junction itself without descending into its target.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `mklink /J link target`; panics when it fails.
pub fn make_junction(link: &Path, target: &Path) {
    let st = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(st.status.success(), "mklink failed: {}", String::from_utf8_lossy(&st.stdout));
}
