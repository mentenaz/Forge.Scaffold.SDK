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

/// What a rendered template may hold. A template is read and rendered in memory before anything is written, so a
/// template folder that is too big is refused instead of filling the machine's memory. The limits are generous
/// for a project skeleton (a downloaded archive is held to less: see `archive.rs`).
pub(crate) const MAX_STAGED_FILES: usize = 20_000;
pub(crate) const MAX_STAGED_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_STAGED_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// How much a template may make: the three limits above, which a host may lower (or raise) for its own use
/// through `PlanOptions::limits` and `AddOptions::limits`. The default is the constants.
///
/// `max_file_bytes` is also what a [`DirSource`](crate::source::DirSource) refuses before it reads a file, but only when
/// the source is given the same number (`DirSource::with_max_file_bytes`): the source does not see the options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most files.
    pub max_files: usize,
    /// The most bytes in one file.
    pub max_file_bytes: u64,
    /// The most bytes in all files together.
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: MAX_STAGED_FILES,
            max_file_bytes: MAX_STAGED_FILE_BYTES,
            max_total_bytes: MAX_STAGED_TOTAL_BYTES,
        }
    }
}

/// Names Windows treats as devices, with or without an extension (`nul`, `con.txt`, `COM1.log`).
const WINDOWS_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Normalises a relative path to `a/b/c` form and rejects anything unsafe.
///
/// Rejected: empty paths, absolute paths, drive letters, `..`, characters that
/// Windows does not allow in names, segments ending in a dot or space, names
/// Windows keeps for devices (`nul`, `con`, `com1`...), and `.git` (a template
/// must not write into a repository's own folder, where hooks run code).
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
        let stem = seg.split('.').next().unwrap_or(seg).to_ascii_lowercase();
        if WINDOWS_DEVICE_NAMES.contains(&stem.as_str()) {
            return Err(bad("a name that Windows keeps for devices (nul, con, com1, ...)"));
        }
        if seg.eq_ignore_ascii_case(".git") {
            return Err(bad("a template must not write into a .git folder"));
        }
        parts.push(seg);
    }

    if parts.is_empty() {
        return Err(bad("is empty"));
    }
    Ok(parts.join("/"))
}

/// One file that `apply` has written, for a progress display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteProgress {
    /// 1-based index of the file that was just written.
    pub index: usize,
    /// How many files there are in all.
    pub total: usize,
    /// The file that was just written, below the new folder, with `/`.
    pub path: String,
}

/// What `apply` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    /// The folder that was created.
    pub root: PathBuf,
    /// How many files were written.
    pub files_written: usize,
    /// How many bytes were written in all.
    pub bytes_written: u64,
}

/// The files of a project in memory, before anything is written. Paths are checked as they are added (no `..`, no device
/// names, no two paths that differ only by letter case) and the limits are enforced.
#[derive(Debug, Default)]
pub struct StagedFs {
    files: BTreeMap<String, Vec<u8>>,
    /// lowercase path -> original path, for case-insensitive collision checks.
    lower: BTreeMap<String, String>,
    /// Bytes of all staged files, kept up to date so the limits are cheap to check.
    total: u64,
    limits: Limits,
}

fn too_large(what: String) -> ScaffoldError {
    ScaffoldError::TooLarge(what)
}

impl StagedFs {
    /// An empty stage with the default limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// A stage that refuses to grow past `limits`.
    pub fn with_limits(limits: Limits) -> Self {
        Self { limits, ..Self::default() }
    }

    /// Stages a new file. Fails on unsafe paths, duplicates (case-insensitive)
    /// and file/folder conflicts.
    pub fn insert(&mut self, path: &str, content: Vec<u8>) -> Result<()> {
        let norm = normalize_rel(path)?;
        let low = norm.to_lowercase();

        if self.files.len() >= self.limits.max_files {
            return Err(too_large(format!("the template makes more than {} files", self.limits.max_files)));
        }
        self.check_size(&norm, content.len() as u64, 0)?;

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
        self.total += content.len() as u64;
        self.files.insert(norm, content);
        Ok(())
    }

    /// Checks one file of `size` bytes against the limits, given that `replacing` bytes of the total go away.
    fn check_size(&self, path: &str, size: u64, replacing: u64) -> Result<()> {
        if size > self.limits.max_file_bytes {
            return Err(too_large(format!("'{path}' is larger than {} bytes", self.limits.max_file_bytes)));
        }
        if self.total - replacing + size > self.limits.max_total_bytes {
            return Err(too_large(format!("the template makes more than {} bytes in total", self.limits.max_total_bytes)));
        }
        Ok(())
    }

    /// Adds a text file.
    pub fn insert_text(&mut self, path: &str, content: &str) -> Result<()> {
        self.insert(path, content.as_bytes().to_vec())
    }

    /// Replaces the content of an already staged file (used by merges).
    pub fn replace(&mut self, path: &str, content: Vec<u8>) -> Result<()> {
        let norm = normalize_rel(path)?;
        let old = self.files.get(&norm).map_or(0, |v| v.len() as u64);
        if self.files.contains_key(&norm) {
            self.check_size(&norm, content.len() as u64, old)?;
        }
        match self.files.get_mut(&norm) {
            Some(slot) => {
                self.total = self.total - old + content.len() as u64;
                *slot = content;
                Ok(())
            }
            None => Err(ScaffoldError::InvalidPath {
                path: path.to_string(),
                reason: "is not staged".to_string(),
            }),
        }
    }

    /// The bytes of a file.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        let norm = normalize_rel(path).ok()?;
        self.files.get(&norm).map(Vec::as_slice)
    }

    /// A file as text, when it is valid UTF-8.
    pub fn get_text(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Whether the path is staged.
    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// How many files are staged.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether nothing is staged.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The bytes of all staged files together.
    pub fn total_bytes(&self) -> u64 {
        self.total
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

    // ------------------------------------------------------------------ limits and names

    #[test]
    fn names_windows_keeps_for_devices_and_git_folders_are_refused() {
        for bad in ["nul", "NUL", "con", "a/Con.txt", "aux.tar.gz", "com1", "LPT9.log", "src/prn", ".git", "a/.GIT/config", ".git/hooks/pre-commit"] {
            assert!(normalize_rel(bad).is_err(), "{bad} was accepted");
        }
        // close, but fine
        for ok in [".gitignore", ".github/workflows/ci.yml", "console.ts", "nulls.txt", "com10", "auxiliary", "git/config", "a/.git-keep"] {
            assert!(normalize_rel(ok).is_ok(), "{ok} was refused");
        }
    }

    #[test]
    fn too_many_files_and_too_large_a_file_are_refused_and_the_totals_stay_right() {
        let mut s = StagedFs::new();
        for i in 0..MAX_STAGED_FILES {
            s.insert(&format!("f/{i}.txt"), Vec::new()).unwrap();
        }
        let err = s.insert("one-too-many.txt", Vec::new()).unwrap_err();
        assert!(matches!(err, ScaffoldError::TooLarge(_)), "{err}");
        assert_eq!(s.len(), MAX_STAGED_FILES, "the refused file is not there");

        let mut s = StagedFs::new();
        s.insert("a.txt", vec![0u8; 10]).unwrap();
        s.insert("b.txt", vec![0u8; 5]).unwrap();
        assert_eq!(s.total_bytes(), 15);
        s.replace("a.txt", vec![0u8; 3]).unwrap();
        assert_eq!(s.total_bytes(), 8, "a replaced file counts for its new size only");
        let big = vec![0u8; (MAX_STAGED_FILE_BYTES + 1) as usize];
        assert!(matches!(s.insert("big.bin", big.clone()), Err(ScaffoldError::TooLarge(_))));
        assert!(matches!(s.replace("a.txt", big), Err(ScaffoldError::TooLarge(_))));
        assert_eq!(s.total_bytes(), 8, "a refused file leaves the totals alone");
        assert_eq!(s.get("a.txt").unwrap().len(), 3);
    }

    #[test]
    fn a_host_can_lower_every_limit_and_each_one_is_enforced() {
        let limits = Limits { max_files: 3, max_file_bytes: 10, max_total_bytes: 25 };

        // the number of files
        let mut s = StagedFs::with_limits(limits);
        for i in 0..3 {
            s.insert(&format!("f{i}.txt"), vec![0; 1]).unwrap();
        }
        assert!(matches!(s.insert("f3.txt", vec![0; 1]), Err(ScaffoldError::TooLarge(m)) if m.contains("more than 3 files")));

        // one file
        let mut s = StagedFs::with_limits(limits);
        assert!(s.insert("ok.txt", vec![0; 10]).is_ok());
        assert!(matches!(s.insert("big.txt", vec![0; 11]), Err(ScaffoldError::TooLarge(m)) if m.contains("'big.txt' is larger than 10 bytes")));

        // the total: 10 + 10 fits (20 <= 25), another 10 does not (30 > 25), five more would (25)
        let mut s = StagedFs::with_limits(limits);
        s.insert("a", vec![0; 10]).unwrap();
        s.insert("b", vec![0; 10]).unwrap();
        let err = s.insert("c", vec![0; 10]).unwrap_err();
        assert!(matches!(&err, ScaffoldError::TooLarge(m) if m.contains("more than 25 bytes in total")), "{err}");
        assert_eq!(s.total_bytes(), 20, "a refused file leaves the total alone");
        s.insert("d", vec![0; 5]).unwrap();
        assert_eq!(s.total_bytes(), 25);
        assert!(s.insert("e", vec![0; 1]).is_err(), "exactly full");

        // replacing counts the new size against the total, not the old one on top of it
        let mut s = StagedFs::with_limits(limits);
        s.insert("a", vec![0; 10]).unwrap();
        s.insert("b", vec![0; 10]).unwrap();
        assert!(s.replace("a", vec![0; 10]).is_ok(), "the same size again fits");
        s.replace("a", vec![0; 5]).unwrap();
        assert!(matches!(s.replace("b", vec![0; 11]), Err(ScaffoldError::TooLarge(_))), "one file over its limit");
        assert!(s.replace("a", vec![0; 10]).is_ok(), "back to 20 in all");
        assert_eq!(s.total_bytes(), 20);
        // the defaults are the constants
        assert_eq!(Limits::default(), Limits { max_files: MAX_STAGED_FILES, max_file_bytes: MAX_STAGED_FILE_BYTES, max_total_bytes: MAX_STAGED_TOTAL_BYTES });
    }
}
