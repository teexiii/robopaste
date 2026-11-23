# AGENTS.md

`robopaste` is a Windows 11 tray app written in Rust. A low-level keyboard hook swallows Ctrl+V in an Explorer file list (or the Desktop) when the clipboard holds only folders, and a worker thread launches `robopaste.exe --run` in its own console, which runs robocopy for each folder. Everything else is handed back to Explorer unchanged.

## Build and test

```
cargo build --release
cargo test
cargo test --test explorer_target_live -- --ignored        # needs an interactive desktop, opens Explorer windows
cargo test --test clipboard_folders_roundtrip -- --ignored # overwrites the user's clipboard
ROBOPASTE_PROBE_DIR='X:\' cargo test --test path_safety_integration network_share_gives -- --ignored   # network share probe
```

`#[ignore]` tests: the two `explorer_target_live` tests need a desktop session; `clipboard_folders_roundtrip` overwrites the clipboard; the two `network_share_*` tests touch a share (they create and remove a throwaway `rp-probe-*` folder; the alias test also needs `ROBOPASTE_PROBE_UNC`). Integration tests use throwaway folders under `%TEMP%` and real robocopy. The default `cargo test` run needs no desktop and leaves the clipboard alone.

Modes of the exe: no arguments = tray + hook; `--run [--move] --dest <dir> <src>...` = runner (own console); `--where` = debug: after 3 s prints the folder resolved for the foreground window (prints to the parent console).

## Module map (`src/`)

- `main.rs`: argument dispatch (`--run`, `--where`, tray mode). The exe uses the GUI subsystem, so `--run` calls `AllocConsole` itself.
- `lib.rs`: module declarations only.
- `paste_plan.rs`: pure logic, no Win32: `plan_paste` (lexical safety), `robocopy_args`, `exit_code_is_failure`, `paths_equal`.
- `path_safety.rs`: Win32 safety gate `check_sources_and_destination` (reparse points, filesystem identity over lexical and resolved ancestor chains).
- `robocopy_runner.rs`: `run_jobs`, `execute` (gate + plan + run, no console), `run_entry` (console), `launch_runner`.
- `explorer_target.rs`: Shell COM resolver for the folder of an Explorer window's active tab or the Desktop.
- `clipboard_folders.rs`: CF_HDROP and `Preferred DropEffect` reader, `empty_clipboard`.
- `keyboard_hook.rs`: `WH_KEYBOARD_LL` hook, cheap context filter, shared flags (`ENABLED`, `PENDING`, `BYPASS_COUNT`).
- `paste_worker.rs`: worker thread: checks, launch, replay of the keystroke to Explorer.
- `tray_icon.rs`: tray icon and menu, single-instance mutex, HKCU Run autostart.
- `tests/`: integration tests (`common/mod.rs` has the temp folder and junction helpers).

## Robocopy flags

Hard-coded in `paste_plan::robocopy_args`: `/E /R:0 /W:0 /IT /MT:16`, plus `/MOVE /XJ` on cut. No config file. Robocopy is started from `%SystemRoot%\System32\robocopy.exe` by absolute path.

## Invariants (do not break)

- No cross-process COM in the hook callback; it only does cheap local checks and hands off to the worker.
- The hook swallows Ctrl+V only when: not our marker (`dwExtraInfo == ROBOPASTE_MARKER`, tested first), enabled, Ctrl down and Shift/Alt/Win up (`GetAsyncKeyState`), foreground class `CabinetWClass`/`Progman`/`WorkerW`, focused control not `Edit` and inside `SHELLDLL_DefView`, CF_HDROP on the clipboard, worker idle. A busy worker lets the key through and bumps `BYPASS_COUNT`, so the worker drops its stale request.
- One press, one request: auto-repeat of a swallowed V is swallowed without a request until the V keyup, but only while the window recorded with the flag is still foreground and Ctrl is still down (a missed keyup cannot swallow an unrelated press); `install_hook` clears that flag.
- Worker order: resolve the destination first, before any blocking filesystem call on the sources; a request older than 2 s is dropped (no launch, no replay); the destination is resolved again right before `launch_runner` and a changed or missing one drops the request.
- `/MOVE` deletes sources, so two layers must agree and both fail closed (any error means hand back to Explorer): `path_safety::check_sources_and_destination` (file identity of each source against the destination, its lexical ancestors and the ancestors of its resolved path; source and `dest\<name>` must not be reparse points; not the source's own parent; an existing `dest\<name>` must not be an ancestor of the source) and `plan_paste` (lexical). The gate is called by both the worker and `--run`.
- Never give robocopy a `\\?\` path. Resolved paths are used inside `path_safety` for the ancestor walk only; `plan_paste` rejects `\\?\` input.
- `/XJ` always accompanies `/MOVE` (a followed junction would be emptied by the move).
- No keystroke logging, including in debug builds. The hook looks at the `V` key and modifier state only.
- Tests and manual checks use throwaway folders only.

## Active-tab resolution (verified on Windows 11 build 26200)

All tabs of one Explorer window share the top-level `HWND` and appear as separate `IShellWindows` items. `FindWindowExW(top, null, "ShellTabWindowClass")` returns the first tab window in z-order, which is the active tab; the item whose `IShellBrowser::GetWindow()` equals it is the target. `IsWindowVisible` cannot tell tabs apart (all visible). Without a `ShellTabWindowClass` child the code falls back to matching the top `HWND` only, and only when exactly one item matches (several give `None`). Virtual folders (This PC, Libraries, zip, search results) give `None`. Details: `plans/261123-1220-rust-explorer-paste-robocopy-override/reports/spike-explorer-active-tab-result.md`.

## Known limits

- Native Paste menu item and drag and drop are not intercepted. An elevated Explorer is not seen by this non-elevated hook (stock paste happens). No undo.
- Cut on the same volume is copy plus delete, slower than Explorer's rename (user decision).
- Cut skips junctions and symlinks inside the folder, file symlinks included. They stay in the source together with their parent folders, so the source folder is not removed (robocopy exit code 1, console closes). Copy follows junctions and duplicates their contents; a junction pointing back at an ancestor recurses until the path-length limit (exit 9, console stays open).
- Cut onto an identical existing tree leaves the source in place.
- Merge overwrites a newer destination file with an older source file (`/IT` and robocopy's default "different = copy").
- The same server folder exposed under two share names (different volume serials) is not detected as one folder. A junction deep inside an already existing destination folder that points back into the source is not detected.
- Cut items stay dimmed in Explorer until refresh.
- Hook-side behaviour (swallow, replay, tray) has unit and integration coverage only for its parts; the end-to-end desktop checklist is in `plans/261123-1220-rust-explorer-paste-robocopy-override/reports/manual-e2e-checklist-result.md`.
