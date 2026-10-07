//! Safe extraction of a template archive (`.tar.gz`).
//!
//! Archives come from the network, so nothing is trusted: only plain files and
//! folders are accepted (no symlinks, hard links or devices), every path goes
//! through the same checks as the rest of the crate, and the number of files
//! and total size are capped so a small download cannot fill the disk.

use crate::error::{Result, ScaffoldError};
use crate::stage::normalize_rel;
use flate2::read::GzDecoder;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tar::EntryType;

pub const MAX_FILES: usize = 5_000;
pub const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

fn bad(msg: impl Into<String>) -> ScaffoldError {
    ScaffoldError::Archive(msg.into())
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ScaffoldError + '_ {
    move |source| ScaffoldError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Extracts `bytes` into the (existing, empty) folder `dest` and returns the
/// folder that holds `manifest.json`: `dest` itself, or the single top-level
/// folder inside it (which is how GitHub archives are laid out).
pub fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let entries = archive.entries().map_err(|e| bad(format!("cannot read archive: {e}")))?;

    let mut files = 0usize;
    let mut total = 0u64;

    for entry in entries {
        let mut entry = entry.map_err(|e| bad(format!("cannot read archive entry: {e}")))?;
        let kind = entry.header().entry_type();
        match kind {
            EntryType::XGlobalHeader | EntryType::XHeader => continue,
            EntryType::Regular | EntryType::Continuous | EntryType::Directory => {}
            other => {
                return Err(bad(format!(
                    "entry type {other:?} is not allowed (only files and folders)"
                )))
            }
        }

        let raw = entry
            .path()
            .map_err(|e| bad(format!("bad path in archive: {e}")))?
            .to_str()
            .ok_or_else(|| bad("a path in the archive is not valid UTF-8"))?
            .replace('\\', "/");
        let trimmed = raw.trim_start_matches("./").trim_end_matches('/');
        if trimmed.is_empty() || trimmed == "." {
            continue;
        }
        let rel = normalize_rel(trimmed).map_err(|e| bad(e.to_string()))?;
        let target = dest.join(&rel);

        if kind == EntryType::Directory {
            fs::create_dir_all(&target).map_err(io_err(&target))?;
            continue;
        }

        let size = entry.header().size().map_err(|e| bad(e.to_string()))?;
        files += 1;
        total = total.saturating_add(size);
        if files > MAX_FILES {
            return Err(bad(format!("more than {MAX_FILES} files")));
        }
        if size > MAX_FILE_BYTES {
            return Err(bad(format!("'{rel}' is larger than {MAX_FILE_BYTES} bytes")));
        }
        if total > MAX_TOTAL_BYTES {
            return Err(bad(format!("more than {MAX_TOTAL_BYTES} bytes in total")));
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    bad(format!("'{rel}' appears twice"))
                } else {
                    ScaffoldError::Io {
                        path: target.clone(),
                        source: e,
                    }
                }
            })?;
        io::copy(&mut entry, &mut file).map_err(io_err(&target))?;
    }

    locate_template_root(dest)
}

/// `dest` itself if it has a `manifest.json`, else its only sub-folder if that has one.
fn locate_template_root(dest: &Path) -> Result<PathBuf> {
    if dest.join("manifest.json").is_file() {
        return Ok(dest.to_path_buf());
    }
    let mut dirs = Vec::new();
    let mut others = 0;
    for entry in fs::read_dir(dest).map_err(io_err(dest))? {
        let entry = entry.map_err(io_err(dest))?;
        if entry.file_type().map_err(io_err(dest))?.is_dir() {
            dirs.push(entry.path());
        } else {
            others += 1;
        }
    }
    match dirs.as_slice() {
        [only] if others == 0 && only.join("manifest.json").is_file() => Ok(only.clone()),
        _ => Err(bad(
            "manifest.json was not found at the top of the archive (or in its single top folder)",
        )),
    }
}
