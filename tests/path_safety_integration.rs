//! Path safety gate against real folders and junctions under %TEMP%.

mod common;

use std::path::PathBuf;

use common::{make_junction, Scratch};
use robopaste::path_safety::check_sources_and_destination;

fn check(src: &PathBuf, dest: &PathBuf) -> bool {
    check_sources_and_destination(std::slice::from_ref(src), dest).is_ok()
}

#[test]
fn unrelated_source_and_destination_are_allowed() {
    let s = Scratch::new("ok");
    let src = s.dir("a/F");
    let dest = s.dir("b");
    assert!(check(&src, &dest));
}

#[test]
fn destination_that_is_the_sources_parent_is_refused() {
    let s = Scratch::new("parent");
    let src = s.dir("a/F");
    assert!(!check(&src, &s.0.join("a")));
}

#[test]
fn destination_inside_the_source_is_refused() {
    let s = Scratch::new("inside");
    let src = s.dir("a/F");
    let dest = s.dir("a/F/sub");
    assert!(!check(&src, &dest));
    assert!(!check(&src, &src));
}

#[test]
fn destination_junction_to_a_folder_two_levels_below_the_source_is_refused() {
    let s = Scratch::new("deepjunction");
    let src = s.dir("work/A");
    let deep = s.dir("work/A/out/deep");
    let other = s.dir("other");
    let link = other.join("shortcut");
    make_junction(&link, &deep);
    assert!(!check(&src, &link));
}

#[test]
fn destination_junction_to_the_source_itself_is_refused() {
    let s = Scratch::new("selfjunction");
    let src = s.dir("work/A");
    let other = s.dir("other");
    let link = other.join("alias");
    make_junction(&link, &src);
    assert!(!check(&src, &link));
}

#[test]
fn source_reached_through_a_junction_alias_of_its_parent_is_refused_for_its_real_subfolder() {
    let s = Scratch::new("aliassource");
    let real_src = s.dir("work/A");
    let real_dest = s.dir("work/A/sub");
    let alias_parent = s.0.join("J");
    make_junction(&alias_parent, &s.0.join("work"));
    let src_via_alias = alias_parent.join("A");
    assert!(real_src.exists());
    assert!(!check(&src_via_alias, &real_dest));
}

#[test]
fn source_that_is_itself_a_junction_is_refused() {
    let s = Scratch::new("srcjunction");
    let target = s.dir("target");
    let link = s.0.join("link");
    make_junction(&link, &target);
    let dest = s.dir("dest");
    assert!(!check(&link, &dest));
}

#[test]
fn existing_destination_entry_that_is_a_junction_is_refused() {
    let s = Scratch::new("destentry");
    let src = s.dir("a/F");
    let dest = s.dir("b");
    let elsewhere = s.dir("elsewhere");
    make_junction(&dest.join("F"), &elsewhere);
    assert!(!check(&src, &dest));
}

#[test]
fn missing_source_or_destination_is_refused() {
    let s = Scratch::new("missing");
    let real = s.dir("real");
    let ghost = s.0.join("ghost");
    assert!(!check(&ghost, &real));
    assert!(!check(&real, &ghost));
}

#[test]
fn plain_file_as_source_is_refused() {
    let s = Scratch::new("filesrc");
    let f = s.file("f.txt", "x");
    let dest = s.dir("dest");
    assert!(!check(&f, &dest));
}

/// Probe for network drives (X:, Y:, Z:): opt in with ROBOPASTE_PROBE_DIR.
/// Prints identities and the gate result for a throwaway folder pair.
#[test]
#[ignore = "touches a network share; set ROBOPASTE_PROBE_DIR to a scratch folder on it"]
fn network_share_gives_usable_file_identities() {
    let base = PathBuf::from(std::env::var("ROBOPASTE_PROBE_DIR").expect("ROBOPASTE_PROBE_DIR"));
    let root = base.join(format!("rp-probe-{}", std::process::id()));
    let src = root.join("A");
    let dest = root.join("B");
    let inside = src.join("sub");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    let ids = [&src, &dest, &inside].map(|p| robopaste::path_safety::identity_of_path(p));
    println!("identities: {ids:?}");
    let ok_pair = check(&src, &dest);
    let inside_refused = !check(&src, &inside);
    let parent_refused = !check(&src, &root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(ids.iter().all(|i| i.is_ok()));
    assert!(ok_pair && inside_refused && parent_refused);
}

/// The refusal text, so a test cannot pass because of an unrelated refusal.
fn refusal_text(src: &PathBuf, dest: &PathBuf) -> String {
    check_sources_and_destination(std::slice::from_ref(src), dest).unwrap_err().to_string()
}

#[test]
fn junction_aliases_are_refused_for_the_identity_reason_not_by_accident() {
    let s = Scratch::new("reason");
    let src = s.dir("work/A");
    let deep = s.dir("work/A/out/deep");
    let other = s.dir("other");
    let to_deep = other.join("shortcut");
    make_junction(&to_deep, &deep);
    assert!(refusal_text(&src, &to_deep).contains("lies inside"), "{}", refusal_text(&src, &to_deep));
    let to_src = other.join("alias");
    make_junction(&to_src, &src);
    assert!(refusal_text(&src, &to_src).contains("lies inside"));
    assert!(refusal_text(&src, &s.0.join("work")).contains("parent folder"));
}

/// Same folder reached through a mapped drive and through its UNC name:
/// ROBOPASTE_PROBE_DIR=X:\ ROBOPASTE_PROBE_UNC=\server\share\ (same location).
#[test]
#[ignore = "touches a network share; set ROBOPASTE_PROBE_DIR and ROBOPASTE_PROBE_UNC"]
fn network_share_alias_is_detected_between_drive_letter_and_unc() {
    let drive = PathBuf::from(std::env::var("ROBOPASTE_PROBE_DIR").expect("ROBOPASTE_PROBE_DIR"));
    let unc = PathBuf::from(std::env::var("ROBOPASTE_PROBE_UNC").expect("ROBOPASTE_PROBE_UNC"));
    let leaf = format!("rp-probe-alias-{}", std::process::id());
    let inside_via_drive = drive.join(&leaf).join("A").join("sub");
    std::fs::create_dir_all(&inside_via_drive).unwrap();
    let src_via_unc = unc.join(&leaf).join("A");
    let refused = !check(&src_via_unc, &inside_via_drive);
    let _ = std::fs::remove_dir_all(drive.join(&leaf));
    assert!(refused, "destination inside the source via another name must be refused");
}

#[test]
fn target_that_is_an_ancestor_of_the_source_is_refused() {
    // Source T\a\b\a pasted into T: the target T\a contains the source.
    let s = Scratch::new("targetancestor");
    let src = s.dir("a/b/a");
    assert!(!check(&src, &s.0));
}
