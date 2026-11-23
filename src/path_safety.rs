//! Win32 safety gate in front of robocopy. Fail closed: any error while
//! inspecting a source or the destination means "do not run".
//!
//! `/MOVE` deletes sources, and a destination that physically lies inside a
//! source recurses without end. String comparison cannot see aliases
//! (junctions, symlinks, `subst`, mapped drives), so this module compares
//! filesystem identities (volume serial + file index) of every source against
//! the destination, its lexical ancestors, and the ancestors of its resolved
//! path. Resolved (`\\?\`) paths are used for that walk only; they are never
//! returned or handed to robocopy.

use std::fmt;
use std::fs;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_DOS,
};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Why the gate refuses; the text is shown in the runner console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn refuse<T>(msg: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(msg.into()))
}

/// (volume serial, file index high, file index low).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity(u32, u32, u32);

/// Open directory handle, closed on drop.
struct DirHandle(HANDLE);

impl Drop for DirHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn open_dir(path: &Path) -> Result<DirHandle, Refusal> {
    let wide = to_wide(path);
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    match h {
        Ok(h) => Ok(DirHandle(h)),
        Err(e) => refuse(format!("cannot open {}: {e}", path.display())),
    }
}

fn identity_of(handle: &DirHandle, path: &Path) -> Result<FileIdentity, Refusal> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if let Err(e) = unsafe { GetFileInformationByHandle(handle.0, &mut info) } {
        return refuse(format!("cannot read identity of {}: {e}", path.display()));
    }
    if info.nFileIndexHigh == 0 && info.nFileIndexLow == 0 {
        return refuse(format!("{} reports no usable file identity", path.display()));
    }
    Ok(FileIdentity(info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Identity of a directory addressed by path.
pub fn identity_of_path(path: &Path) -> Result<FileIdentity, Refusal> {
    let h = open_dir(path)?;
    identity_of(&h, path)
}

/// Final (resolved) path of an open handle, `\\?\`-prefixed.
fn resolved_path(handle: &DirHandle, path: &Path) -> Result<PathBuf, Refusal> {
    let mut buf = vec![0u16; 1024];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(handle.0, &mut buf, VOLUME_NAME_DOS) } as usize;
        if n == 0 {
            return refuse(format!("cannot resolve {}", path.display()));
        }
        if n < buf.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buf[..n])));
        }
        buf.resize(n + 1, 0);
    }
}

fn has_reparse_attribute(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// `Ok` means robocopy may run for this source/destination combination.
pub fn check_sources_and_destination(sources: &[PathBuf], dest_dir: &Path) -> Result<(), Refusal> {
    if sources.is_empty() {
        return refuse("no sources");
    }

    // Destination must be an existing directory.
    match fs::metadata(dest_dir) {
        Ok(m) if m.is_dir() => {}
        _ => return refuse(format!("destination {} is not an existing folder", dest_dir.display())),
    }

    // Each source: a real directory, not a reparse point; and the target
    // `dest\<name>` must not be a reparse point if it already exists.
    for src in sources {
        let meta = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => return refuse(format!("cannot inspect {}: {e}", src.display())),
        };
        if !meta.is_dir() || has_reparse_attribute(&meta) {
            return refuse(format!("{} is not a plain folder (link or file)", src.display()));
        }
        let Some(name) = src.file_name() else {
            return refuse(format!("{} has no folder name", src.display()));
        };
        match fs::symlink_metadata(dest_dir.join(name)) {
            Ok(m) if has_reparse_attribute(&m) => {
                return refuse(format!("{} already exists as a link", dest_dir.join(name).display()));
            }
            Ok(m) if !m.is_dir() => {
                return refuse(format!("{} already exists and is not a folder", dest_dir.join(name).display()));
            }
            _ => {}
        }
    }

    let source_ids: Vec<(&PathBuf, FileIdentity)> =
        sources.iter().map(|s| identity_of_path(s).map(|id| (s, id))).collect::<Result<_, _>>()?;

    // Destination chain: the destination, its lexical ancestors, plus the
    // resolved path and its ancestors. `Path::ancestors` stops at the volume
    // or share root (`\\srv\share\`), which is what we want: `\\srv` alone
    // cannot be opened.
    let dest_handle = open_dir(dest_dir)?;
    let mut chain: Vec<PathBuf> = dest_dir.ancestors().map(Path::to_path_buf).collect();
    let resolved = resolved_path(&dest_handle, dest_dir)?;
    chain.extend(resolved.ancestors().map(Path::to_path_buf));
    let dest_id = identity_of(&dest_handle, dest_dir)?;
    drop(dest_handle);

    let mut chain_ids: Vec<(PathBuf, FileIdentity)> = Vec::with_capacity(chain.len());
    for entry in chain {
        if entry.as_os_str().is_empty() {
            continue;
        }
        let id = identity_of_path(&entry)?;
        chain_ids.push((entry, id));
    }

    for (src, src_id) in &source_ids {
        if let Some((hit, _)) = chain_ids.iter().find(|(_, id)| id == src_id) {
            return refuse(format!(
                "destination {} is, or lies inside, source {} (via {})",
                dest_dir.display(),
                src.display(),
                hit.display()
            ));
        }
        // Pasting into the source's own parent: Explorer makes "name - Copy".
        let Some(parent) = src.parent().filter(|p| !p.as_os_str().is_empty()) else {
            return refuse(format!("{} has no parent folder", src.display()));
        };
        if identity_of_path(parent)? == dest_id {
            return refuse(format!("destination is the parent folder of {}", src.display()));
        }
        // The folder the paste would merge into must not be an ancestor of the
        // source (cut T/a/b/a, paste into T: target T/a contains the source);
        // the move would eat its own source.
        if let Some(name) = src.file_name() {
            let target = dest_dir.join(name);
            if fs::symlink_metadata(&target).is_ok() {
                let target_id = identity_of_path(&target)?;
                let src_handle = open_dir(src)?;
                let mut src_chain: Vec<PathBuf> = src.ancestors().map(Path::to_path_buf).collect();
                let src_resolved = resolved_path(&src_handle, src)?;
                src_chain.extend(src_resolved.ancestors().map(Path::to_path_buf));
                drop(src_handle);
                for entry in src_chain {
                    if entry.as_os_str().is_empty() {
                        continue;
                    }
                    if identity_of_path(&entry)? == target_id {
                        return refuse(format!(
                            "target {} is, or contains, source {} (via {})",
                            target.display(),
                            src.display(),
                            entry.display()
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
