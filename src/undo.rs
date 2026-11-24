//! Undoing an add (and, later, an edit) of an existing solution.
//!
//! The crate keeps no history and opens no database. Before an add is applied, [`AddPlan::undo_record`] describes
//! it as data: for every file the add creates or edits, its path below the solution, and for an edited one the exact bytes
//! it had before. The host stores that record wherever it keeps history (a table keyed by the workspace), and later
//! hands it to [`revert_edit`], which puts the solution back.
//!
//! Reverting is careful by design. It only changes a file that is still exactly as the edit left it (checked by SHA-256),
//! and if any file was changed since, it changes **nothing** and names the files, so a later edit of yours is never
//! overwritten by an old backup. A file that is already as it was before the edit counts as reverted, so reverting twice
//! is harmless. If a write fails half way, what was done is undone again.

use crate::add::{goes_through_a_link, AddOutcome, AddPlan};
use crate::error::{Result, ScaffoldError};
use crate::stage::normalize_rel;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The version of the record format this crate writes and reads. A host stores it with the record.
pub const UNDO_FORMAT: u32 = 1;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UndoEntry {
    /// The edit created the file; reverting removes it.
    Created {
        /// Below the solution, `/`-separated.
        path: String,
        /// The SHA-256 of what the edit wrote.
        after: String,
    },
    /// The edit changed an existing file; reverting writes the old bytes back.
    Modified {
        /// The file, below the solution, `/`-separated.
        path: String,
        /// The SHA-256 of the file before the edit.
        before: String,
        /// The SHA-256 of what the edit wrote.
        after: String,
        /// The file as it was before the edit, base64.
        before_bytes: String,
    },
}

impl UndoEntry {
    fn path(&self) -> &str {
        match self {
            UndoEntry::Created { path, .. } | UndoEntry::Modified { path, .. } => path,
        }
    }
}

/// Everything needed to undo one add or edit. Serializable: `serde_json::to_string(&record)` is what a host stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoRecord {
    /// [`UNDO_FORMAT`] when it was written.
    pub format: u32,
    /// `add` or `edit` (whatever the record was made with).
    pub kind: String,
    /// What the host shows in a history list ("Added web part News").
    pub label: String,
    /// What the edit did to each file.
    pub entries: Vec<UndoEntry>,
    /// Folders the edit created (below the solution), removed again by a revert if they are empty by then.
    pub created_dirs: Vec<String>,
}

impl UndoRecord {
    /// The record as JSON text, for a host that stores it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an undo record is always valid JSON")
    }

    /// Reads a stored record. A record from a newer format than this crate knows is refused, not guessed at.
    pub fn from_json(text: &str) -> Result<UndoRecord> {
        let record: UndoRecord = serde_json::from_str(text).map_err(|e| ScaffoldError::UndoRecord(format!("not a valid undo record: {e}")))?;
        record.check()?;
        Ok(record)
    }

    fn check(&self) -> Result<()> {
        if self.format == 0 || self.format > UNDO_FORMAT {
            return Err(ScaffoldError::UndoRecord(format!(
                "this undo record has format {}, and this version only reads up to {UNDO_FORMAT}",
                self.format
            )));
        }
        // A stored record is data from a file or a database: its paths get the same checks as any template path.
        for p in self.entries.iter().map(UndoEntry::path).chain(self.created_dirs.iter().map(String::as_str)) {
            normalize_rel(p).map_err(|e| ScaffoldError::UndoRecord(format!("'{p}' is not a safe path: {e}")))?;
        }
        Ok(())
    }
}

impl AddPlan {
    /// Describes this add as an [`UndoRecord`]. **Call it before [`apply_add`](crate::apply_add)**: which folders the add
    /// creates can only be told while they are not there yet. Store the record only when the apply succeeded.
    pub fn undo_record(&self, label: &str) -> UndoRecord {
        self.undo_record_as("add", label)
    }

    /// The same, for a plan that is not an add: `kind` is what the host files the record under (`edit` for the plan of
    /// [`plan_edit`](crate::plan_edit)).
    pub fn undo_record_as(&self, kind: &str, label: &str) -> UndoRecord {
        let mut created_dirs: Vec<String> = Vec::new();
        let mut entries = Vec::with_capacity(self.files.len());
        for f in &self.files {
            match f.outcome {
                AddOutcome::New => {
                    let parts: Vec<&str> = f.path.split('/').collect();
                    for i in 1..parts.len() {
                        let dir = parts[..i].join("/");
                        if !self.root.join(&dir).exists() && !created_dirs.contains(&dir) {
                            created_dirs.push(dir);
                        }
                    }
                    entries.push(UndoEntry::Created { path: f.path.clone(), after: sha256_hex(&f.content) });
                }
                _ => {
                    let before = f.previous.clone().unwrap_or_default();
                    entries.push(UndoEntry::Modified {
                        path: f.path.clone(),
                        before: sha256_hex(&before),
                        after: sha256_hex(&f.content),
                        before_bytes: base64::engine::general_purpose::STANDARD.encode(&before),
                    });
                }
            }
        }
        UndoRecord { format: UNDO_FORMAT, kind: kind.into(), label: label.into(), entries, created_dirs }
    }
}

/// What a revert did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertOutcome {
    /// Files put back as they were.
    pub restored: Vec<String>,
    /// Files the edit had created, now removed.
    pub removed: Vec<String>,
    /// Files that were already as they were before the edit (reverted earlier, or removed by hand): nothing to do.
    pub already: Vec<String>,
}

enum Step {
    Restore(PathBuf, Vec<u8>, String),
    Remove(PathBuf, String),
}

/// Puts the solution at `root` back as it was before the edit the record describes. See the module notes for the rules:
/// all or nothing, and only for files still exactly as the edit left them.
///
/// # Example
///
/// ```
/// use forge_scaffold::{apply_edit, detect_spfx, plan_edit, revert_edit, SolutionEdit, UndoRecord};
/// use std::fs;
///
/// # let root = std::env::temp_dir().join(format!("forge-doc-undo-{}", std::process::id()));
/// # let _ = fs::remove_dir_all(&root);
/// # fs::create_dir_all(root.join("config"))?;
/// # fs::write(root.join("package.json"), r#"{ "name": "web", "dependencies": { "@microsoft/sp-core-library": "1.21.1" }, "devDependencies": { "@microsoft/sp-build-web": "1.21.1" } }"#)?;
/// # fs::write(root.join("gulpfile.js"), "")?;
/// # fs::write(root.join("config/config.json"), "{}")?;
/// # let original = r#"{ "solution": { "name": "Web", "version": "1.0.0.0" } }"#;
/// # fs::write(root.join("config/package-solution.json"), original)?;
/// let solution = detect_spfx(&root)?.expect("a solution");
/// let edit = SolutionEdit { name: Some("Intranet".into()), ..Default::default() };
/// let plan = plan_edit(&solution, &edit)?;
///
/// // take the undo record BEFORE applying, and keep it (a host stores the JSON) only if the apply works
/// let record = plan.undo_record_as("edit", "Renamed to Intranet");
/// apply_edit(&plan, |_| {})?;
/// assert!(fs::read_to_string(root.join("config/package-solution.json"))?.contains("Intranet"));
///
/// // later: back to exactly what it was. The record survives being stored as text.
/// let record = UndoRecord::from_json(&record.to_json())?;
/// revert_edit(&root, &record)?;
/// assert_eq!(fs::read_to_string(root.join("config/package-solution.json"))?, original);
/// # fs::remove_dir_all(&root)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn revert_edit(root: &Path, record: &UndoRecord) -> Result<RevertOutcome> {
    revert_with(root, record, |path, bytes| fs::write(path, bytes))
}

pub(crate) fn revert_with(
    root: &Path,
    record: &UndoRecord,
    mut writer: impl FnMut(&Path, &[u8]) -> io::Result<()>,
) -> Result<RevertOutcome> {
    record.check()?;
    if !root.is_dir() {
        return Err(ScaffoldError::InvalidPath { path: root.display().to_string(), reason: "the folder does not exist".into() });
    }

    // 1. Look at everything first; change nothing unless every file is as expected.
    let mut conflicts: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut already: Vec<String> = Vec::new();
    for entry in &record.entries {
        let rel = entry.path();
        if goes_through_a_link(root, rel) {
            return Err(ScaffoldError::LinkInSolution(rel.to_string()));
        }
        let full = root.join(rel);
        let now = match fs::read(&full) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                conflicts.push(rel.to_string()); // there but unreadable: not something to overwrite
                continue;
            }
        };
        match (entry, now) {
            (UndoEntry::Created { .. }, None) => already.push(rel.to_string()),
            (UndoEntry::Created { after, .. }, Some(bytes)) => {
                if &sha256_hex(&bytes) == after {
                    steps.push(Step::Remove(full, rel.to_string()));
                } else {
                    conflicts.push(rel.to_string());
                }
            }
            (UndoEntry::Modified { before, .. }, Some(bytes)) if &sha256_hex(&bytes) == before => already.push(rel.to_string()),
            (UndoEntry::Modified { after, before_bytes, before, .. }, Some(bytes)) if &sha256_hex(&bytes) == after => {
                let old = base64::engine::general_purpose::STANDARD
                    .decode(before_bytes)
                    .map_err(|e| ScaffoldError::UndoRecord(format!("'{rel}': the saved bytes are not valid base64: {e}")))?;
                if &sha256_hex(&old) != before {
                    return Err(ScaffoldError::UndoRecord(format!("'{rel}': the saved bytes do not match their checksum")));
                }
                steps.push(Step::Restore(full, old, rel.to_string()));
            }
            _ => conflicts.push(rel.to_string()),
        }
    }
    if !conflicts.is_empty() {
        return Err(ScaffoldError::RevertConflict(conflicts));
    }

    // 2. Do it; if a write fails, put back what was already changed.
    let mut done: Vec<(&PathBuf, Option<Vec<u8>>)> = Vec::new(); // what to write back to undo this step again
    let mut outcome = RevertOutcome { restored: Vec::new(), removed: Vec::new(), already };
    for step in &steps {
        let result = match step {
            Step::Restore(full, old, _) => {
                let current = fs::read(full).ok();
                let tmp = full.with_extension("forge-tmp");
                let r = writer(&tmp, old).and_then(|_| fs::rename(&tmp, full).inspect_err(|_| {
                    let _ = fs::remove_file(&tmp);
                }));
                if r.is_ok() {
                    done.push((full, current));
                }
                r.map_err(|e| (full.clone(), e))
            }
            Step::Remove(full, _) => {
                let current = fs::read(full).ok();
                let r = fs::remove_file(full);
                if r.is_ok() {
                    done.push((full, current));
                }
                r.map_err(|e| (full.clone(), e))
            }
        };
        match result {
            Ok(()) => match step {
                Step::Restore(_, _, rel) => outcome.restored.push(rel.clone()),
                Step::Remove(_, rel) => outcome.removed.push(rel.clone()),
            },
            Err((path, source)) => {
                let mut rolled_back = true;
                for (p, bytes) in done.iter().rev() {
                    if let Some(bytes) = bytes {
                        rolled_back &= fs::write(p, bytes).is_ok();
                    }
                }
                return Err(ScaffoldError::WriteFailed { path, source, rolled_back });
            }
        }
    }

    // 3. Folders the edit made, deepest first, only if they are empty now.
    let mut dirs: Vec<&String> = record.created_dirs.iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        let _ = fs::remove_dir(root.join(d)); // fails when something else is in it, which is the point
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::add::{apply_add, plan_add, AddOptions};
    use crate::detect::{BuildSystem, SpfxSolution};
    use crate::manifest::Manifest;
    use crate::source::DirSource;
    use crate::tokens::SequentialGuids;
    use serde_json::json;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-undo-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const MANIFEST: &str = r#"{
      "id": "demo", "version": "1.20.0", "folderFrom": "solutionName",
      "prompts": [
        { "key": "solutionName", "type": "text" },
        { "key": "webparts", "type": "list", "rule": "webpart-names" }
      ],
      "guids": ["solutionId"],
      "include": [ { "folder": "solution" } ],
      "repeat": [ { "over": "webparts", "as": "webpart", "folder": "webpart", "guids": ["webpartId"] } ],
      "merge": [
        { "file": "config/config.json", "strategy": "json-set", "per": "webparts",
          "path": "bundles.{__webpartKebab__}-web-part",
          "value": { "components": [ { "entrypoint": "./lib/{__webpartCamel__}.js" } ] } }
      ]
    }"#;

    /// A generator, a solution that already has `src/alpha` and one bundle, and the manifest.
    fn setup() -> (Tmp, Tmp, Manifest, SpfxSolution) {
        let gen = Tmp::new();
        gen.write("solution/package.json", "{}");
        gen.write("webpart/src/{__webpartCamel__}/{__webpartPascal__}.ts", "// {__webpartPascal__}\n");
        let sol = Tmp::new();
        sol.write("package.json", "{ \"name\": \"my-sol\" }\n");
        sol.write("gulpfile.js", "// untouched\n");
        sol.write("src/alpha/Alpha.ts", "// Alpha\n");
        sol.write(
            "config/config.json",
            "{\r\n    \"version\": \"2.0\",\r\n    \"bundles\": {\r\n        \"alpha-web-part\": { \"components\": [] }\r\n    },\r\n    \"localizedResources\": {}\r\n}\r\n",
        );
        let solution = SpfxSolution {
            root: sol.0.clone(),
            version: Some("1.20.0".into()),
            build_system: BuildSystem::Gulp,
            framework: None,
            package_name: Some("my-sol".into()),
            node_range: None,
            marker: None,
            warnings: Vec::new(),
        };
        (gen, sol, Manifest::from_json(MANIFEST).unwrap(), solution)
    }

    fn plan(gen: &Tmp, manifest: &Manifest, solution: &SpfxSolution, name: &str) -> AddPlan {
        let answers = serde_json::from_value(json!({ "webparts": [name] })).unwrap();
        let mut options = AddOptions::new();
        options.guids = Box::new(SequentialGuids::new());
        plan_add(solution, manifest, &DirSource::new(&gen.0), &answers, options).expect("plan")
    }

    #[test]
    fn an_add_can_be_undone_and_the_solution_is_exactly_as_before() {
        let (gen, sol, manifest, solution) = setup();
        let before = fs::read(sol.0.join("config/config.json")).unwrap();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("Added web part News");
        apply_add(&p, |_| {}).unwrap();
        assert!(sol.read("config/config.json").contains("news-web-part"));
        assert!(sol.0.join("src/news/News.ts").is_file());

        let out = revert_edit(&sol.0, &record).unwrap();
        assert_eq!(out.restored, ["config/config.json"]);
        assert_eq!(out.removed, ["src/news/News.ts"]);
        assert_eq!(fs::read(sol.0.join("config/config.json")).unwrap(), before, "byte for byte, line endings included");
        assert!(!sol.0.join("src/news").exists(), "the folder the add made is gone too");
        assert!(sol.0.join("src/alpha/Alpha.ts").is_file(), "what was there is kept");

        // reverting again is harmless
        let again = revert_edit(&sol.0, &record).unwrap();
        assert!(again.restored.is_empty() && again.removed.is_empty());
        assert_eq!(again.already.len(), 2);
    }

    #[test]
    fn a_folder_with_something_else_in_it_is_not_removed() {
        let (gen, sol, manifest, solution) = setup();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("x");
        assert_eq!(record.created_dirs, ["src/news"], "src existed already");
        apply_add(&p, |_| {}).unwrap();
        sol.write("src/news/mine.txt", "later work");
        revert_edit(&sol.0, &record).unwrap();
        assert_eq!(sol.read("src/news/mine.txt"), "later work");
        assert!(!sol.0.join("src/news/News.ts").exists());
    }

    #[test]
    fn a_file_changed_since_the_edit_is_never_overwritten_and_nothing_at_all_is_changed() {
        let (gen, sol, manifest, solution) = setup();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("x");
        apply_add(&p, |_| {}).unwrap();
        sol.write("src/news/News.ts", "// News, and my own work\n");
        let config_after = sol.read("config/config.json");

        let err = revert_edit(&sol.0, &record).unwrap_err();
        match &err {
            ScaffoldError::RevertConflict(files) => assert_eq!(files, &["src/news/News.ts"]),
            other => panic!("{other}"),
        }
        assert_eq!(err.kind(), "revert_conflict");
        assert_eq!(sol.read("config/config.json"), config_after, "the other file was not reverted either");
        assert!(sol.read("src/news/News.ts").contains("my own work"));
    }

    #[test]
    fn a_failed_write_while_reverting_puts_back_what_was_already_reverted() {
        let (gen, sol, manifest, solution) = setup();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("x");
        apply_add(&p, |_| {}).unwrap();
        let after_config = sol.read("config/config.json");
        // the removal of the new file works, the write of the old config.json fails: the removal must be undone
        let err = revert_with(&sol.0, &record, |_, _| Err(io::Error::other("disk full"))).unwrap_err();
        assert!(matches!(err, ScaffoldError::WriteFailed { rolled_back: true, .. }), "{err}");
        assert_eq!(sol.read("config/config.json"), after_config);
        assert!(sol.0.join("src/news/News.ts").is_file());
    }

    #[test]
    fn the_record_round_trips_through_json_and_refuses_what_it_cannot_trust() {
        let (gen, sol, manifest, solution) = setup();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("Added web part News");
        let text = record.to_json();
        assert!(text.contains("\"format\":1") && text.contains("\"kind\":\"add\"") && text.contains("beforeBytes"), "{text}");
        assert_eq!(UndoRecord::from_json(&text).unwrap(), record);

        // not JSON, a newer format, an unsafe path
        assert!(matches!(UndoRecord::from_json("nope"), Err(ScaffoldError::UndoRecord(_))));
        let newer = text.replace("\"format\":1", "\"format\":99");
        assert!(UndoRecord::from_json(&newer).unwrap_err().to_string().contains("format 99"));
        let unsafe_path = text.replace("src/news/News.ts", "../outside.txt");
        assert!(UndoRecord::from_json(&unsafe_path).unwrap_err().to_string().contains("not a safe path"));

        // saved bytes that do not match their checksum are refused before anything is changed
        apply_add(&p, |_| {}).unwrap();
        let mut tampered = record.clone();
        for e in tampered.entries.iter_mut() {
            if let UndoEntry::Modified { before_bytes, .. } = e {
                *before_bytes = base64::engine::general_purpose::STANDARD.encode(b"something else");
            }
        }
        let err = revert_edit(&sol.0, &tampered).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert!(sol.0.join("src/news/News.ts").is_file(), "nothing was changed");
    }

    /// A symbolic link, or on Windows without the privilege a junction; `false` when neither can be made here.
    fn make_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(target, link).is_ok();
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            let junction = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output();
            return junction.is_ok_and(|o| o.status.success());
        }
        #[allow(unreachable_code)]
        false
    }

    #[test]
    fn a_revert_does_not_write_through_a_link_that_appeared_since() {
        let (gen, sol, manifest, solution) = setup();
        let p = plan(&gen, &manifest, &solution, "News");
        let record = p.undo_record("x");
        apply_add(&p, |_| {}).unwrap();
        let outside = Tmp::new();
        outside.write("News.ts", "// News\n");
        fs::remove_dir_all(sol.0.join("src/news")).unwrap();
        let link = sol.0.join("src").join("news");
        let made = make_link(&outside.0, &link);
        if !made {
            eprintln!("cannot create a link here, skipping");
            return;
        }
        let err = revert_edit(&sol.0, &record).unwrap_err();
        assert!(matches!(err, ScaffoldError::LinkInSolution(_)), "{err}");
        assert_eq!(fs::read_to_string(outside.0.join("News.ts")).unwrap(), "// News\n", "the file outside was not touched");
    }
}
