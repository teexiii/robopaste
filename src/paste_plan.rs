//! Pure paste decisions: no Win32, no filesystem access.
//!
//! This is the lexical safety layer. `path_safety` (filesystem identities) is
//! the other layer; both must agree before robocopy is allowed to run, because
//! `/MOVE` deletes sources and a destination inside the source recurses
//! without end.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteMode {
    Copy,
    Move,
}

/// One robocopy invocation: `src` is copied into `dest` (= dest dir + source name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub src: PathBuf,
    pub dest: PathBuf,
}

/// Why a paste is handed back to Explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoSources,
    /// A source is a drive root or otherwise has no folder name.
    NoFileName,
    /// A `\\?\` path was supplied; robocopy rejects those (error 53).
    VerbatimPath,
    /// Pasting into the source's own parent (Explorer makes "name - Copy").
    IntoOwnParent,
    /// Destination is the source or lies inside it.
    DestinationInsideSource,
    /// Two sources would land on the same destination name.
    DuplicateName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteDecision {
    Run(Vec<Job>),
    PassThrough(Reason),
}

/// Decide whether robopaste may take over this paste. One bad source hands the
/// whole paste back to Explorer.
pub fn plan_paste(sources: &[PathBuf], dest_dir: &Path, _mode: PasteMode) -> PasteDecision {
    if sources.is_empty() {
        return PasteDecision::PassThrough(Reason::NoSources);
    }
    if is_verbatim(dest_dir) || sources.iter().any(|s| is_verbatim(s)) {
        return PasteDecision::PassThrough(Reason::VerbatimPath);
    }
    let dest_parts = normalize(dest_dir);
    let mut jobs: Vec<Job> = Vec::with_capacity(sources.len());
    let mut seen_names: Vec<String> = Vec::new();
    for src in sources {
        let Some(name) = src.file_name() else {
            return PasteDecision::PassThrough(Reason::NoFileName);
        };
        let src_parts = normalize(src);
        // Component-wise prefix test: `D:\ab` is not inside `D:\a`.
        if dest_parts.starts_with(&src_parts) {
            return PasteDecision::PassThrough(Reason::DestinationInsideSource);
        }
        let dest = dest_dir.join(name);
        if normalize(&dest) == src_parts {
            return PasteDecision::PassThrough(Reason::IntoOwnParent);
        }
        let key = name.to_string_lossy().to_lowercase();
        if seen_names.contains(&key) {
            return PasteDecision::PassThrough(Reason::DuplicateName);
        }
        seen_names.push(key);
        jobs.push(Job { src: src.clone(), dest });
    }
    PasteDecision::Run(jobs)
}

/// Robocopy arguments for one job. Hard-coded flags; `/XJ` always accompanies
/// `/MOVE` so a junction inside the source is skipped instead of followed
/// (a followed junction would be emptied by the move).
pub fn robocopy_args(job: &Job, mode: PasteMode) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        without_trailing_separator(&job.src).into_os_string(),
        without_trailing_separator(&job.dest).into_os_string(),
    ];
    for flag in ["/E", "/R:0", "/W:0", "/IT", "/MT:16"] {
        args.push(flag.into());
    }
    if mode == PasteMode::Move {
        args.push("/MOVE".into());
        args.push("/XJ".into());
    }
    args
}

/// Robocopy exit codes below 8 are success variants; 8 and above, or negative
/// (process could not run), are failures.
pub fn exit_code_is_failure(code: i32) -> bool {
    !(0..8).contains(&code)
}

/// True when two paths denote the same location, lexically: case-insensitive,
/// `/` == `\`, trailing separators ignored, `\\?\` prefixes dropped.
pub fn paths_equal(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

pub(crate) fn is_verbatim(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('/', "\\");
    s.starts_with("\\\\?\\")
}

/// `PathBuf` re-assembled from its components: drops a trailing separator but
/// keeps a drive root such as `D:\` intact.
fn without_trailing_separator(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Lower-cased components; verbatim prefixes removed; a UNC path keeps a
/// leading `\\` marker so `\\srv\x` differs from a local `srv\x`.
fn normalize(path: &Path) -> Vec<String> {
    let mut s = path.to_string_lossy().replace('/', "\\");
    let lower = s.to_lowercase();
    if lower.starts_with("\\\\?\\unc\\") {
        s = format!("\\\\{}", &s["\\\\?\\unc\\".len()..]);
    } else if lower.starts_with("\\\\?\\") {
        s = s["\\\\?\\".len()..].to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if s.starts_with("\\\\") {
        parts.push("\\\\".to_string());
    }
    parts.extend(s.split('\\').filter(|c| !c.is_empty()).map(str::to_lowercase));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn run(srcs: &[&str], dest: &str) -> PasteDecision {
        let v: Vec<PathBuf> = srcs.iter().map(|s| p(s)).collect();
        plan_paste(&v, Path::new(dest), PasteMode::Copy)
    }

    fn args_text(job: &Job, mode: PasteMode) -> Vec<String> {
        robocopy_args(job, mode).iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn copy_into_other_folder_makes_one_job_without_move_flags() {
        let PasteDecision::Run(jobs) = run(&[r"C:\a\F"], r"D:\x") else { panic!("expected Run") };
        assert_eq!(jobs, vec![Job { src: p(r"C:\a\F"), dest: p(r"D:\x\F") }]);
        let a = args_text(&jobs[0], PasteMode::Copy);
        assert_eq!(a, [r"C:\a\F", r"D:\x\F", "/E", "/R:0", "/W:0", "/IT", "/MT:16"]);
    }

    #[test]
    fn move_adds_move_and_xj_and_copy_has_neither() {
        let job = Job { src: p(r"C:\a\F"), dest: p(r"D:\x\F") };
        let m = args_text(&job, PasteMode::Move);
        assert_eq!(&m[m.len() - 2..], ["/MOVE", "/XJ"]);
        let c = args_text(&job, PasteMode::Copy);
        assert!(!c.iter().any(|a| a == "/MOVE" || a == "/XJ"));
    }

    #[test]
    fn trailing_separators_are_not_passed_to_robocopy() {
        let job = Job { src: p(r"C:\a\F\"), dest: p(r"D:\x\F\") };
        let a = args_text(&job, PasteMode::Copy);
        assert_eq!(&a[..2], [r"C:\a\F", r"D:\x\F"]);
    }

    #[test]
    fn verbatim_prefix_is_ignored_when_comparing() {
        assert!(paths_equal(Path::new(r"\\?\C:\a\F"), Path::new(r"C:\a\F")));
        assert!(paths_equal(Path::new(r"\\?\UNC\srv\sh\F"), Path::new(r"\\srv\sh\F")));
        assert!(!paths_equal(Path::new(r"\\srv\sh\F"), Path::new(r"srv\sh\F")));
    }

    #[test]
    fn verbatim_source_or_destination_is_handed_back() {
        assert_eq!(run(&[r"\\?\C:\a\F"], r"D:\x"), PasteDecision::PassThrough(Reason::VerbatimPath));
        assert_eq!(run(&[r"C:\a\F"], r"\\?\D:\x"), PasteDecision::PassThrough(Reason::VerbatimPath));
    }

    #[test]
    fn paste_into_own_parent_is_handed_back() {
        assert_eq!(run(&[r"C:\a\F"], r"C:\a"), PasteDecision::PassThrough(Reason::IntoOwnParent));
    }

    #[test]
    fn destination_inside_source_is_handed_back() {
        assert_eq!(run(&[r"C:\a\F"], r"C:\a\F\sub"), PasteDecision::PassThrough(Reason::DestinationInsideSource));
    }

    #[test]
    fn destination_equal_to_source_is_handed_back() {
        assert_eq!(run(&[r"C:\a\F"], r"C:\a\F"), PasteDecision::PassThrough(Reason::DestinationInsideSource));
    }

    #[test]
    fn sibling_sharing_a_name_prefix_is_not_inside_the_source() {
        assert!(matches!(run(&[r"C:\a\F"], r"C:\a\Foo"), PasteDecision::Run(_)));
    }

    #[test]
    fn case_and_separator_differences_are_equal() {
        assert_eq!(run(&["c:/A/f/"], r"C:\a\F\sub"), PasteDecision::PassThrough(Reason::DestinationInsideSource));
        assert_eq!(run(&["c:/A/f/"], r"C:\a"), PasteDecision::PassThrough(Reason::IntoOwnParent));
    }

    #[test]
    fn drive_root_source_is_handed_back() {
        assert_eq!(run(&[r"C:\"], r"D:\x"), PasteDecision::PassThrough(Reason::NoFileName));
    }

    #[test]
    fn duplicate_names_from_two_parents_are_handed_back() {
        assert_eq!(run(&[r"C:\a\F", r"C:\b\f"], r"D:\x"), PasteDecision::PassThrough(Reason::DuplicateName));
    }

    #[test]
    fn one_unsafe_source_hands_back_the_whole_paste() {
        assert_eq!(
            run(&[r"C:\a\F", r"C:\a\G"], r"C:\a\G\sub"),
            PasteDecision::PassThrough(Reason::DestinationInsideSource)
        );
    }

    #[test]
    fn several_sources_make_several_jobs_and_empty_input_is_handed_back() {
        let PasteDecision::Run(jobs) = run(&[r"C:\a\F", r"C:\b\G"], r"D:\x") else { panic!("expected Run") };
        assert_eq!(jobs.len(), 2);
        assert_eq!(run(&[], r"D:\x"), PasteDecision::PassThrough(Reason::NoSources));
    }

    #[test]
    fn exit_code_classification() {
        for ok in [0, 1, 3, 7] {
            assert!(!exit_code_is_failure(ok), "{ok}");
        }
        for bad in [8, 16, -1] {
            assert!(exit_code_is_failure(bad), "{bad}");
        }
    }
}
