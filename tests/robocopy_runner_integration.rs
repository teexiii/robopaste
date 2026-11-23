//! Real robocopy against throwaway folders under %TEMP%.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{make_junction, Scratch};
use robopaste::paste_plan::PasteMode;
use robopaste::robocopy_runner::{execute, run_jobs, RunArgs};

fn run(sources: &[PathBuf], dest: &PathBuf, mode: PasteMode) -> robopaste::robocopy_runner::RunSummary {
    run_jobs(sources, dest, mode, &mut std::io::sink())
}

fn read(p: PathBuf) -> String {
    fs::read_to_string(p).unwrap()
}

#[test]
fn copy_places_a_nested_tree_under_dest_and_leaves_the_source() {
    let s = Scratch::new("copy");
    let name = "My Dir \u{e9}\u{1f4c1}";
    let src = s.dir(name);
    s.file(&format!("{name}/a.txt"), "A");
    s.file(&format!("{name}/sub dir/b.txt"), "B");
    s.dir(&format!("{name}/empty"));
    let dest = s.dir("dst");
    let r = run(&[src.clone()], &dest, PasteMode::Copy);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert_eq!(read(dest.join(name).join("a.txt")), "A");
    assert_eq!(read(dest.join(name).join("sub dir").join("b.txt")), "B");
    assert!(dest.join(name).join("empty").is_dir());
    assert!(src.join("a.txt").exists());
}

#[test]
fn move_places_the_tree_under_dest_and_removes_the_source() {
    let s = Scratch::new("move");
    let src = s.dir("F");
    s.file("F/a.txt", "A");
    s.file("F/sub/b.txt", "B");
    let dest = s.dir("dst");
    let r = run(&[src.clone()], &dest, PasteMode::Move);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert_eq!(read(dest.join("F/sub/b.txt")), "B");
    assert!(!src.exists());
}

#[test]
fn merge_replaces_a_differing_file_and_keeps_extra_destination_files() {
    let s = Scratch::new("merge");
    let src = s.dir("F");
    s.file("F/same.txt", "from source");
    let dest = s.dir("dst");
    s.file("dst/F/same.txt", "old destination");
    s.file("dst/F/extra.txt", "keep me");
    let r = run(&[src], &dest, PasteMode::Copy);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert_eq!(read(dest.join("F/same.txt")), "from source");
    assert_eq!(read(dest.join("F/extra.txt")), "keep me");
}

#[test]
fn move_skips_a_junction_inside_the_source_and_leaves_its_target_intact() {
    let s = Scratch::new("movejunction");
    let outside = s.dir("outside");
    s.file("outside/keep1.txt", "1");
    s.file("outside/deep/keep2.txt", "2");
    let src = s.dir("F");
    s.file("F/own.txt", "own");
    make_junction(&src.join("link"), &outside);
    let dest = s.dir("dst");
    let r = run(&[src.clone()], &dest, PasteMode::Move);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert_eq!(read(outside.join("keep1.txt")), "1");
    assert_eq!(read(outside.join("deep/keep2.txt")), "2");
    // Skipped links and their parent folders stay behind; real files moved.
    assert!(src.is_dir());
    let left: Vec<_> = fs::read_dir(&src).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, vec![std::ffi::OsString::from("link")]);
    assert_eq!(read(dest.join("F/own.txt")), "own");
    assert!(!dest.join("F/link").exists());
}

#[test]
fn copy_follows_a_junction_and_duplicates_its_contents_as_a_real_folder() {
    let s = Scratch::new("copyjunction");
    let outside = s.dir("outside");
    s.file("outside/keep1.txt", "1");
    let src = s.dir("F");
    make_junction(&src.join("link"), &outside);
    let dest = s.dir("dst");
    let r = run(&[src], &dest, PasteMode::Copy);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert_eq!(read(outside.join("keep1.txt")), "1");
    assert_eq!(read(dest.join("F/link/keep1.txt")), "1");
    assert!(!fs::symlink_metadata(dest.join("F/link")).unwrap().file_type().is_symlink());
}

#[test]
fn move_onto_an_identical_existing_tree_succeeds_and_leaves_the_source() {
    let s = Scratch::new("moveidentical");
    let src = s.dir("F");
    s.file("F/a.txt", "A");
    let dest = s.dir("dst");
    assert!(!run(&[src.clone()], &dest, PasteMode::Copy).any_failed());
    let r = run(&[src.clone()], &dest, PasteMode::Move);
    assert!(!r.any_failed(), "{:?}", r.results);
    assert!(src.join("a.txt").exists());
}

#[test]
fn missing_source_is_reported_as_a_failure() {
    let s = Scratch::new("missing");
    let dest = s.dir("dst");
    let r = run(&[s.0.join("ghost")], &dest, PasteMode::Copy);
    assert!(r.any_failed());
}

#[test]
fn two_sources_are_both_copied() {
    let s = Scratch::new("two");
    let a = s.dir("A");
    s.file("A/a.txt", "A");
    let b = s.dir("B");
    s.file("B/b.txt", "B");
    let dest = s.dir("dst");
    let r = run(&[a, b], &dest, PasteMode::Copy);
    assert_eq!(r.results.len(), 2);
    assert!(!r.any_failed());
    assert_eq!(read(dest.join("A/a.txt")), "A");
    assert_eq!(read(dest.join("B/b.txt")), "B");
}

#[test]
fn run_entry_logic_refuses_destination_inside_source_without_running_robocopy() {
    let s = Scratch::new("refuse");
    let src = s.dir("F");
    s.file("F/a.txt", "A");
    let dest = s.dir("F/sub");
    let args = RunArgs { mode: PasteMode::Move, dest: dest.clone(), sources: vec![src.clone()] };
    let mut out: Vec<u8> = Vec::new();
    assert!(execute(&args, &mut out));
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("refused"), "{text}");
    assert!(!text.contains("robocopy "), "robocopy must not run: {text}");
    assert_eq!(read(src.join("a.txt")), "A");
    assert!(fs::read_dir(&dest).unwrap().next().is_none());
}
