//! In-memory staged filesystem.
//!
//! Everything is rendered into a [`StagedFs`] first. Problems with templates,
//! tokens, duplicate paths and path conflicts surface here, before a single
//! byte touches the disk. Only then is [`StagedFs::write_to`] called, which
//! always writes into a brand-new folder and removes it again if an IO error
//! happens halfway.

use crate::error::{Result, ScaffoldError};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Normalises a relative path to `a/b/c` form and rejects anything unsafe.
///
/// Rejected: empty paths, absolute paths, drive letters, `..`, characters that
/// Windows does not allow in names, and segments ending in a dot or space.
pub(crate) fn normalize_rel(path: &str) -> Result<String> {
    let bad = |reason: &str| ScaffoldError::InvalidPath {
        path: path.to_string(),
        reason: reason.to_string(),
    };

    let unified = path.replace('\\', "/");
    if unified.starts_with('/') {
        return Err(bad("must be relative"));
    }

    let mut parts: Vec<&str> = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return Err(bad("must not contain '..'")),
            _ => {}
        }
        if seg
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(bad("contains a character that is not allowed in Windows file names"));
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(bad("a name must not end with a dot or a space"));
        }
        parts.push(seg);
    }

    if parts.is_empty() {
        return Err(bad("is empty"));
    }
    Ok(parts.join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteProgress {
    /// 1-based index of the file that was just written.
    pub index: usize,
    pub total: usize,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub root: PathBuf,
    pub files_written: usize,
    pub bytes_written: u64,
}

#[derive(Debug, Default)]
pub struct StagedFs {
    files: BTreeMap<String, Vec<u8>>,
    /// lowercase path -> original path, for case-insensitive collision checks.
    lower: BTreeMap<String, String>,
}

impl StagedFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stages a new file. Fails on unsafe paths, duplicates (case-insensitive)
    /// and file/folder conflicts.
    pub fn insert(&mut self, path: &str, content: Vec<u8>) -> Result<()> {
        let norm = normalize_rel(path)?;
        let low = norm.to_lowercase();

        if self.lower.contains_key(&low) {
            return Err(ScaffoldError::DuplicatePath(norm));
        }

        // An existing *file* may not be an ancestor folder of the new path.
        let segments: Vec<&str> = low.split('/').collect();
        for i in 1..segments.len() {
            let ancestor = segments[..i].join("/");
            if let Some(existing) = self.lower.get(&ancestor) {
                return Err(ScaffoldError::PathConflict {
                    existing: existing.clone(),
                    new: norm,
                });
            }
        }

        // The new file may not be an ancestor folder of an existing path.
        let prefix = format!("{low}/");
        if let Some((k, existing)) = self.lower.range(prefix.clone()..).next() {
            if k.starts_with(&prefix) {
                return Err(ScaffoldError::PathConflict {
                    existing: existing.clone(),
                    new: norm,
                });
            }
        }

        self.lower.insert(low, norm.clone());
        self.files.insert(norm, content);
        Ok(())
    }

    pub fn insert_text(&mut self, path: &str, content: &str) -> Result<()> {
        self.insert(path, content.as_bytes().to_vec())
    }

    /// Replaces the content of an already staged file (used by merges).
    pub fn replace(&mut self, path: &str, content: Vec<u8>) -> Result<()> {
        let norm = normalize_rel(path)?;
        match self.files.get_mut(&norm) {
            Some(slot) => {
                *slot = content;
                Ok(())
            }
            None => Err(ScaffoldError::InvalidPath {
                path: path.to_string(),
                reason: "is not staged".to_string(),
            }),
        }
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        let norm = normalize_rel(path).ok()?;
        self.files.get(&norm).map(Vec::as_slice)
    }

    pub fn get_text(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|v| v.len() as u64).sum()
    }

    /// Staged paths in stable (sorted) order. This is the "plan" a dry run shows.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Writes everything into `target`, which must not exist yet.
    ///
    /// The parent of `target` must exist. If any IO error happens, `target` is
    /// removed again. That is safe because this method created it.
    pub fn write_to(
        &self,
        target: &Path,
        mut on_progress: impl FnMut(WriteProgress),
    ) -> Result<WriteOutcome> {
        self.write_with(target, &mut on_progress, |path, bytes| fs::write(path, bytes))
    }

    pub(crate) fn write_with(
        &self,
        target: &Path,
        on_progress: &mut dyn FnMut(WriteProgress),
        mut writer: impl FnMut(&Path, &[u8]) -> io::Result<()>,
    ) -> Result<WriteOutcome> {
        if target.exists() {
            return Err(ScaffoldError::FolderExists(target.to_path_buf()));
        }
        match fs::create_dir(target) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(ScaffoldError::FolderExists(target.to_path_buf()));
            }
            Err(source) => {
                return Err(ScaffoldError::WriteFailed {
                    path: target.to_path_buf(),
                    source,
                    rolled_back: true, // nothing was created
                });
            }
        }

        let total = self.files.len();
        let mut bytes_written = 0u64;

        for (i, (rel, bytes)) in self.files.iter().enumerate() {
            let full = target.join(rel);
            let result = (|| {
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)?;
                }
                writer(&full, bytes)
            })();

            if let Err(source) = result {
                let rolled_back = fs::remove_dir_all(target).is_ok();
                return Err(ScaffoldError::WriteFailed {
                    path: full,
                    source,
                    rolled_back,
                });
            }

            bytes_written += bytes.len() as u64;
            on_progress(WriteProgress {
                index: i + 1,
                total,
                path: rel.clone(),
            });
        }

        Ok(WriteOutcome {
            root: target.to_path_buf(),
            files_written: total,
            bytes_written,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal temp dir that cleans up after itself (no extra dependency).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "forge-scaffold-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn normalize_accepts_and_cleans_paths() {
        assert_eq!(normalize_rel("a/b/c.ts").unwrap(), "a/b/c.ts");
        assert_eq!(normalize_rel("a\\b\\c.ts").unwrap(), "a/b/c.ts");
        assert_eq!(normalize_rel("./a//b.ts").unwrap(), "a/b.ts");
    }

    #[test]
    fn normalize_rejects_unsafe_paths() {
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "C:/x", "a/b?.ts", "a/b.", "a/b ", "a/\u{0}b"] {
            assert!(normalize_rel(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn duplicates_are_case_insensitive() {
        let mut fs_ = StagedFs::new();
        fs_.insert_text("src/A.ts", "1").unwrap();
        assert!(matches!(
            fs_.insert_text("src/a.ts", "2"),
            Err(ScaffoldError::DuplicatePath(_))
        ));
    }

    #[test]
    fn file_versus_folder_conflicts_are_caught_both_ways() {
        let mut a = StagedFs::new();
        a.insert_text("src", "file").unwrap();
        assert!(matches!(
            a.insert_text("src/x.ts", "1"),
            Err(ScaffoldError::PathConflict { .. })
        ));

        let mut b = StagedFs::new();
        b.insert_text("src/x.ts", "1").unwrap();
        assert!(matches!(
            b.insert_text("src", "file"),
            Err(ScaffoldError::PathConflict { .. })
        ));
        // A sibling with a shared name prefix is fine.
        b.insert_text("src2", "ok").unwrap();
    }

    #[test]
    fn replace_and_get_work() {
        let mut s = StagedFs::new();
        s.insert_text("a/b.json", "{}").unwrap();
        s.replace("a/b.json", b"{\"x\":1}".to_vec()).unwrap();
        assert_eq!(s.get_text("a/b.json"), Some("{\"x\":1}"));
        assert!(s.replace("nope.txt", vec![]).is_err());
        assert_eq!(s.total_bytes(), 7);
    }

    #[test]
    fn paths_are_sorted_for_stable_dry_runs() {
        let mut s = StagedFs::new();
        s.insert_text("b.txt", "").unwrap();
        s.insert_text("a/z.txt", "").unwrap();
        s.insert_text("a/a.txt", "").unwrap();
        assert_eq!(s.paths().collect::<Vec<_>>(), vec!["a/a.txt", "a/z.txt", "b.txt"]);
    }

    #[test]
    fn writes_into_a_new_folder_and_reports_progress() {
        let tmp = TempDir::new();
        let target = tmp.0.join("my-solution");

        let mut s = StagedFs::new();
        s.insert_text("package.json", "{}").unwrap();
        s.insert_text("src/webparts/hello/Hello.tsx", "export {}").unwrap();

        let mut seen = Vec::new();
        let out = s.write_to(&target, |p| seen.push(p)).unwrap();

        assert_eq!(out.files_written, 2);
        assert_eq!(seen.len(), 2);
        assert_eq!(seen.last().unwrap().index, 2);
        assert_eq!(fs::read_to_string(target.join("package.json")).unwrap(), "{}");
        assert!(target.join("src/webparts/hello/Hello.tsx").is_file());
    }

    #[test]
    fn refuses_an_existing_folder_and_leaves_it_untouched() {
        let tmp = TempDir::new();
        let target = tmp.0.join("existing");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "mine").unwrap();

        let mut s = StagedFs::new();
        s.insert_text("package.json", "{}").unwrap();

        let err = s.write_to(&target, |_| {}).unwrap_err();
        assert!(matches!(err, ScaffoldError::FolderExists(_)));
        assert!(err.to_string().contains("already exists, please choose a different name"));
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "mine");
        assert!(!target.join("package.json").exists());
    }

    #[test]
    fn io_failure_halfway_removes_the_folder_we_created() {
        let tmp = TempDir::new();
        let target = tmp.0.join("half");

        let mut s = StagedFs::new();
        s.insert_text("a.txt", "1").unwrap();
        s.insert_text("b.txt", "2").unwrap();
        s.insert_text("c.txt", "3").unwrap();

        let mut calls = 0;
        let err = s
            .write_with(&target, &mut |_| {}, |path, bytes| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("disk full"))
                } else {
                    fs::write(path, bytes)
                }
            })
            .unwrap_err();

        match err {
            ScaffoldError::WriteFailed { rolled_back, .. } => assert!(rolled_back),
            other => panic!("unexpected error: {other}"),
        }
        assert!(!target.exists(), "partial output must be removed");
    }

    #[test]
    fn missing_parent_is_a_clean_error() {
        let tmp = TempDir::new();
        let target = tmp.0.join("no-such-parent").join("sol");
        let s = StagedFs::new();
        assert!(matches!(
            s.write_to(&target, |_| {}),
            Err(ScaffoldError::WriteFailed { .. })
        ));
    }
}
