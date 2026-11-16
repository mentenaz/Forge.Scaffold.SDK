//! Where template files come from.
//!
//! `plan()` never touches the disk itself. It asks a [`TemplateSource`] for
//! "all files below this folder" and "the bytes of this file". Two sources
//! exist: [`DirSource`] reads a real folder (authoring, tests, user-made
//! templates) and [`Pack`](crate::pack::Pack) reads a pack held in memory
//! (the templates compiled into the app).
//!
//! All paths given to a source are relative to the template root, use `/`, and
//! have already been checked with `normalize_rel`.

use crate::error::{Result, ScaffoldError};
use std::fs;
use std::path::{Path, PathBuf};

pub trait TemplateSource {
    /// All files below `folder`: relative paths (to `folder`), `/`-separated,
    /// sorted. An empty folder gives an empty list; a missing folder is an error.
    fn list_files(&self, folder: &str) -> Result<Vec<String>>;

    /// The bytes of one file. `path` is relative to the template root.
    fn read_file(&self, path: &str) -> Result<Vec<u8>>;

    /// A file that must be UTF-8 text (fragments, merge values).
    fn read_text(&self, path: &str) -> Result<String> {
        let bytes = self.read_file(path)?;
        String::from_utf8(bytes).map_err(|_| ScaffoldError::Template {
            path: path.to_string(),
            reason: "the file is not valid UTF-8 text".into(),
        })
    }
}

/// Reads templates from a folder on disk. Symbolic links are refused.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl TemplateSource for DirSource {
    fn list_files(&self, folder: &str) -> Result<Vec<String>> {
        let mut found = Vec::new();
        walk(&self.root.join(folder), "", &mut found, folder)?;
        Ok(found)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let full = self.root.join(path);
        if let Ok(meta) = fs::symlink_metadata(&full) {
            if meta.file_type().is_symlink() {
                return Err(ScaffoldError::Template {
                    path: path.to_string(),
                    reason: "symbolic links are not allowed in templates".into(),
                });
            }
        }
        fs::read(&full).map_err(|e| ScaffoldError::Template {
            path: path.to_string(),
            reason: e.to_string(),
        })
    }
}

/// Lists all files below `dir` (relative, `/`-separated, sorted). Symlinks are refused.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>, label: &str) -> Result<()> {
    let read = fs::read_dir(dir).map_err(|e| ScaffoldError::Template {
        path: label.to_string(),
        reason: format!("cannot read template folder: {e}"),
    })?;
    let mut entries: Vec<_> = read
        .collect::<std::io::Result<_>>()
        .map_err(|e| ScaffoldError::Template {
            path: label.to_string(),
            reason: e.to_string(),
        })?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| ScaffoldError::Template {
            path: format!("{label}/{rel}"),
            reason: e.to_string(),
        })?;
        if meta.file_type().is_symlink() {
            return Err(ScaffoldError::Template {
                path: format!("{label}/{rel}"),
                reason: "symbolic links are not allowed in templates".into(),
            });
        }
        if meta.is_dir() {
            walk(&entry.path(), &rel, out, label)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "forge-scaffold-src-{tag}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lists_sorted_and_reads_bytes() {
        let t = Tmp::new("list");
        fs::create_dir_all(t.0.join("base/src")).unwrap();
        fs::write(t.0.join("base/b.txt"), "b").unwrap();
        fs::write(t.0.join("base/a.txt"), "a").unwrap();
        fs::write(t.0.join("base/src/x.ts"), [0xff, 0x00]).unwrap();
        let s = DirSource::new(&t.0);
        assert_eq!(s.list_files("base").unwrap(), vec!["a.txt", "b.txt", "src/x.ts"]);
        assert_eq!(s.read_file("base/src/x.ts").unwrap(), vec![0xff, 0x00]);
        assert!(s.read_text("base/src/x.ts").is_err(), "invalid UTF-8 is not text");
        assert_eq!(s.read_text("base/a.txt").unwrap(), "a");
    }

    #[test]
    fn missing_folder_is_an_error_and_empty_folder_is_not() {
        let t = Tmp::new("missing");
        fs::create_dir_all(t.0.join("empty")).unwrap();
        let s = DirSource::new(&t.0);
        assert!(s.list_files("empty").unwrap().is_empty());
        assert!(s.list_files("nope").is_err());
        assert!(s.read_file("nope/file").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let t = Tmp::new("link");
        fs::create_dir_all(t.0.join("base")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", t.0.join("base/leak")).unwrap();
        let s = DirSource::new(&t.0);
        assert!(s.list_files("base").is_err());
        assert!(s.read_file("base/leak").is_err());
    }
}
