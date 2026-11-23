//! `--run` mode: runs robocopy for each source in its own console and keeps
//! the console open only when something failed. The tray process launches this
//! as a detached child so a long copy never blocks the keyboard hook.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::System::Console::{AllocConsole, SetConsoleTitleW};

use crate::path_safety::check_sources_and_destination;
use crate::paste_plan::{exit_code_is_failure, plan_paste, robocopy_args, Job, PasteDecision, PasteMode};

/// The console opened by `--run` (`CONOUT$`, `CONIN$`). When set, robocopy
/// children get these as stdio instead of inherited std handles, which may be
/// redirected pipes or invalid when the tray was started from another program.
struct ConsoleHandles {
    out: File,
    input: File,
}

static CONSOLE: OnceLock<ConsoleHandles> = OnceLock::new();

/// Opens `CONOUT$` and `CONIN$` after `AllocConsole`. `None` when either fails.
fn open_console_handles() -> Option<ConsoleHandles> {
    let out = OpenOptions::new().write(true).open("CONOUT$").ok()?;
    let input = OpenOptions::new().read(true).write(true).open("CONIN$").ok()?;
    Some(ConsoleHandles { out, input })
}

/// stdio for a robocopy child: the console when `--run` opened one, else inherit.
fn child_stdio() -> (Stdio, Stdio, Stdio) {
    let Some(c) = CONSOLE.get() else { return (Stdio::inherit(), Stdio::inherit(), Stdio::inherit()) };
    match (c.input.try_clone(), c.out.try_clone(), c.out.try_clone()) {
        (Ok(i), Ok(o), Ok(e)) => (Stdio::from(i), Stdio::from(o), Stdio::from(e)),
        _ => (Stdio::inherit(), Stdio::inherit(), Stdio::inherit()),
    }
}

/// Outcome of a run: one robocopy exit code per job, or a refusal before any job ran.
#[derive(Debug, Default)]
pub struct RunSummary {
    pub results: Vec<(Job, i32)>,
    /// Set when the safety gate or `plan_paste` refused; no robocopy ran.
    pub refused: Option<String>,
}

impl RunSummary {
    pub fn any_failed(&self) -> bool {
        self.refused.is_some() || self.results.iter().any(|(_, code)| exit_code_is_failure(*code))
    }
}

/// `%SystemRoot%\System32\robocopy.exe` by absolute path, never via `PATH`.
fn robocopy_exe() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    Path::new(&root).join("System32").join("robocopy.exe")
}

/// Runs robocopy for each source in turn. No console code in here, so tests
/// can call it directly. Continues with the next job after a failure.
/// Applies the lexical layer (`plan_paste`); the Win32 gate is applied by the
/// caller (`execute`, or the paste worker).
pub fn run_jobs(sources: &[PathBuf], dest_dir: &Path, mode: PasteMode, out: &mut dyn Write) -> RunSummary {
    let jobs = match plan_paste(sources, dest_dir, mode) {
        PasteDecision::Run(jobs) => jobs,
        PasteDecision::PassThrough(reason) => {
            let msg = format!("refused: {reason:?}");
            let _ = writeln!(out, "{msg}");
            return RunSummary { results: Vec::new(), refused: Some(msg) };
        }
    };
    let exe = robocopy_exe();
    let mut summary = RunSummary::default();
    for job in jobs {
        let args = robocopy_args(&job, mode);
        let shown: Vec<String> = args.iter().map(|a| format!("\"{}\"", a.to_string_lossy())).collect();
        let _ = writeln!(out, "\n> robocopy {}\n", shown.join(" "));
        let _ = out.flush();
        let (stdin, stdout, stderr) = child_stdio();
        let code = Command::new(&exe)
            .args(&args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .status()
            .ok()
            .and_then(|s| s.code())
            .unwrap_or(-1);
        summary.results.push((job, code));
    }
    summary
}

/// Parsed `--run [--move] --dest <dir> <src>...`.
pub struct RunArgs {
    pub mode: PasteMode,
    pub dest: PathBuf,
    pub sources: Vec<PathBuf>,
}

pub fn parse_run_args(args: &[OsString]) -> Result<RunArgs, String> {
    let mut mode = PasteMode::Copy;
    let mut dest: Option<PathBuf> = None;
    let mut sources = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--move" {
            mode = PasteMode::Move;
        } else if a == "--dest" {
            dest = Some(PathBuf::from(it.next().ok_or("--dest needs a folder")?));
        } else {
            sources.push(PathBuf::from(a));
        }
    }
    let dest = dest.ok_or("missing --dest")?;
    if sources.is_empty() {
        return Err("no source folders given".into());
    }
    Ok(RunArgs { mode, dest, sources })
}

/// Everything `--run` does except console handling: gate, lexical plan, run,
/// report. Returns true when something failed or was refused.
pub fn execute(args: &RunArgs, out: &mut dyn Write) -> bool {
    if let Err(refusal) = check_sources_and_destination(&args.sources, &args.dest) {
        let _ = writeln!(out, "robopaste refused to run robocopy: {refusal}");
        return true;
    }
    let summary = run_jobs(&args.sources, &args.dest, args.mode, out);
    if summary.any_failed() {
        let _ = writeln!(out, "\nSome jobs failed:");
        for (job, code) in &summary.results {
            let state = if exit_code_is_failure(*code) { "FAILED" } else { "ok" };
            let _ = writeln!(out, "  [{state}] exit {code}: {}", job.src.display());
        }
        return true;
    }
    false
}

/// `--run` entry point: own console, title, run, wait for Enter on failure.
/// Returns the process exit code. Output goes to the console handles opened
/// here and every write ignores errors, so a broken inherited std handle can
/// neither panic nor hide the messages.
pub fn run_entry(raw_args: &[OsString]) -> i32 {
    // The exe uses the GUI subsystem, so it starts without a console.
    unsafe {
        let _ = AllocConsole();
    }
    let mut out: Box<dyn Write> = match open_console_handles() {
        Some(c) => {
            let writer = c.out.try_clone();
            let _ = CONSOLE.set(c);
            match writer {
                Ok(w) => Box::new(w),
                Err(_) => Box::new(std::io::stdout()),
            }
        }
        None => Box::new(std::io::stdout()),
    };
    let parsed = match parse_run_args(raw_args) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "robopaste --run: {e}");
            wait_for_enter();
            return 2;
        }
    };
    let verb = if parsed.mode == PasteMode::Move { "move" } else { "copy" };
    let title: Vec<u16> = format!("robopaste: {verb} -> {}\0", parsed.dest.display()).encode_utf16().collect();
    unsafe {
        let _ = SetConsoleTitleW(PCWSTR(title.as_ptr()));
    }
    let failed = execute(&parsed, &mut out);
    if failed {
        let _ = writeln!(out, "\nPress Enter to close.");
        let _ = out.flush();
        wait_for_enter();
        1
    } else {
        0
    }
}

fn wait_for_enter() {
    // The explicit CONIN$ handle works even when the std handles are redirected or invalid.
    let mut line = String::new();
    match CONSOLE.get().and_then(|c| c.input.try_clone().ok()) {
        Some(f) => {
            let _ = std::io::BufReader::new(f).read_line(&mut line);
        }
        None => {
            if let Ok(f) = File::open("CONIN$") {
                let _ = std::io::BufReader::new(f).read_line(&mut line);
            }
        }
    }
}

/// Spawns this exe in `--run` mode without waiting for it. Original paths only.
pub fn launch_runner(sources: &[PathBuf], dest_dir: &Path, mode: PasteMode) -> std::io::Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--run");
    if mode == PasteMode::Move {
        cmd.arg("--move");
    }
    cmd.arg("--dest").arg(dest_dir).args(sources);
    cmd.spawn().map(|_| ())
}
