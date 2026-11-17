//! Adding to an existing solution.
//!
//! The host first asks [`crate::detect_spfx`] what is in the folder. A solution
//! means "add to it": [`plan_add`] builds what would change, the host shows it,
//! and [`apply_add`] writes it. No solution means a new one: `plan()` and `apply()`.
//!
//! An add runs only the per-component part of a generator: every `repeat`
//! folder, rendered for the new names, and every `merge` that is `per` a list
//! (the `config/config.json` entries of a web part). The solution-level parts
//! (`include`, `parts`, the other merges, the post steps) are not run: the
//! solution already has them. So the answers an add needs are the lists it
//! repeats over, not the solution name.
//!
//! Safety rules:
//!
//! * nothing is ever deleted;
//! * a file the add would create that already exists is an error
//!   ([`ScaffoldError::ExistsInSolution`]), and nothing is written;
//! * the SPFx version of the generator must be the solution's
//!   ([`ScaffoldError::Incompatible`]);
//! * an existing file is only ever edited by a `merge`, which keeps its indent,
//!   line endings and key order, and which needs plain JSON (comments are
//!   refused with a clear message, not dropped);
//! * [`apply_add`] checks that no file changed since the plan was made, and
//!   puts everything back if a write fails halfway.

use crate::detect::{major_minor, SpfxSolution};
use crate::error::{Result, ScaffoldError};
use crate::manifest::{when_holds, Manifest, PromptKind, When};
use crate::plan::{
    apply_merge, copy_folder, list_items, read_template_text, render_json, resolve_answers_for,
    unreachable_manifest, Answers,
};
use crate::source::TemplateSource;
use crate::stage::{StagedFs, WriteOutcome, WriteProgress};
use crate::tokens::{render, GuidSource, RandomGuids, TokenMap};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What happens to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// The file does not exist yet and is created.
    New,
    /// The file exists and is edited by a merge.
    Merged,
}

#[derive(Debug, Clone)]
pub struct AddFile {
    /// Path below the solution root, `/`-separated.
    pub path: String,
    pub outcome: AddOutcome,
    /// The full content after the add.
    pub content: Vec<u8>,
    /// The content before, for [`AddOutcome::Merged`]. Lets a host show a diff.
    pub previous: Option<Vec<u8>>,
}

/// Everything an add would do, as data. Writes nothing.
#[derive(Debug, Clone)]
pub struct AddPlan {
    pub root: PathBuf,
    /// Sorted by path.
    pub files: Vec<AddFile>,
    pub warnings: Vec<String>,
}

impl AddPlan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

pub struct AddOptions {
    pub guids: Box<dyn GuidSource>,
}

impl AddOptions {
    pub fn new() -> Self {
        Self { guids: Box::new(RandomGuids) }
    }
}

impl Default for AddOptions {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_when(when: &Option<When>, keys: &mut HashSet<String>) {
    if let Some(w) = when {
        if let Some(k) = &w.key {
            keys.insert(k.clone());
        }
        for nested in w.all.iter().flatten() {
            collect_when(&Some(nested.clone()), keys);
        }
    }
}

/// Plans adding the components in `answers` to `solution`.
pub fn plan_add(
    solution: &SpfxSolution,
    manifest: &Manifest,
    templates: &dyn TemplateSource,
    answers: &Answers,
    mut options: AddOptions,
) -> Result<AddPlan> {
    let root = &solution.root;
    let mut warnings = solution.warnings.clone();

    // The templates of one minor version fit every patch of it (1.20.0 templates
    // add to a 1.20.2 solution); another minor version is a different generator.
    match &solution.version {
        Some(v) if major_minor(v) != major_minor(&manifest.version) => {
            return Err(ScaffoldError::Incompatible(format!(
                "this generator is for SPFx {}, but the solution uses SPFx {v}",
                manifest.version
            )));
        }
        Some(v) if *v != manifest.version => warnings.push(format!(
            "the solution is on SPFx {v} and this generator was made for {}; the same minor version, so it should fit",
            manifest.version
        )),
        Some(_) => {}
        None => warnings.push("the SPFx version of the solution could not be read".into()),
    }
    if manifest.repeat.is_empty() {
        return Err(ScaffoldError::Incompatible(
            "this generator has nothing to add: it has no per-component folders".into(),
        ));
    }

    // Only the prompts the add uses are asked for.
    let mut keys: HashSet<String> = HashSet::new();
    for r in &manifest.repeat {
        keys.insert(r.over.clone());
        collect_when(&r.when, &mut keys);
    }
    for m in manifest.merge.iter().filter(|m| m.per.is_some()) {
        keys.extend(m.per.clone());
        collect_when(&m.when, &mut keys);
    }
    let answers = resolve_answers_for(manifest, answers, Some(&keys))?;

    let mut base = TokenMap::new();
    for p in &manifest.prompts {
        let Some(value) = answers.get(&p.key) else { continue };
        match p.kind {
            PromptKind::Text => {
                let text = value.as_str().unwrap_or_default();
                if p.sensitive {
                    base.insert(&p.key, text)?;
                } else {
                    base.insert_name(&p.key, text)?;
                }
            }
            PromptKind::Choice => base.insert(&p.key, value.as_str().unwrap_or_default())?,
            PromptKind::Bool | PromptKind::List => {}
        }
    }

    // Render the new components.
    let mut stage = StagedFs::new();
    let mut item_tokens: Vec<(String, Vec<TokenMap>)> = Vec::new();
    for r in manifest.repeat.iter().filter(|r| when_holds(&r.when, &answers)) {
        let mut maps = Vec::new();
        for item in &list_items(&answers, &r.over) {
            let mut tokens = base.clone();
            tokens.insert_name(&r.item, item)?;
            for g in &r.guids {
                tokens.insert_guid(g, options.guids.as_mut())?;
            }
            copy_folder(&mut stage, templates, &r.folder, None, &tokens)?;
            maps.push(tokens);
        }
        item_tokens.push((r.over.clone(), maps));
    }

    // Nothing new may already be there.
    for path in stage.paths() {
        if root.join(path).exists() {
            return Err(ScaffoldError::ExistsInSolution(path.to_string()));
        }
    }

    // Edit the existing files the components register themselves in.
    let mut previous: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for m in manifest.merge.iter().filter(|m| when_holds(&m.when, &answers)) {
        let Some(per) = &m.per else { continue }; // solution-level: already there
        let Some((_, maps)) = item_tokens.iter().find(|(over, _)| over == per) else { continue };
        for tokens in maps {
            let file = render(&m.file, tokens, "merge file")?;
            let path = render(&m.path, tokens, &format!("merge path in {file}"))?;
            if !stage.contains(&file) {
                let on_disk = root.join(&file);
                let bytes = fs::read(&on_disk).map_err(|_| ScaffoldError::Merge {
                    file: file.clone(),
                    reason: "not found in the solution; is this the right folder?".into(),
                })?;
                previous.insert(file.clone(), bytes.clone());
                stage.insert(&file, bytes)?;
            }
            let value = match (&m.value, &m.value_file) {
                (Some(v), _) => render_json(v, tokens, &format!("merge value for {file}"))?,
                (None, Some(vf)) => {
                    let text = render(&read_template_text(templates, vf)?, tokens, vf)?;
                    serde_json::from_str(&text).map_err(|e| ScaffoldError::Merge {
                        file: file.clone(),
                        reason: format!("'{vf}' is not valid JSON after rendering: {e}"),
                    })?
                }
                (None, None) => unreachable_manifest(&file)?,
            };
            apply_merge(&mut stage, &file, m.strategy, &path, value)?;
        }
    }

    let mut files = Vec::new();
    for path in stage.paths() {
        let content = stage.get(path).expect("a staged path has content").to_vec();
        match previous.get(path) {
            Some(before) if *before == content => {} // a merge that changed nothing
            Some(before) => files.push(AddFile {
                path: path.to_string(),
                outcome: AddOutcome::Merged,
                content,
                previous: Some(before.clone()),
            }),
            None => files.push(AddFile {
                path: path.to_string(),
                outcome: AddOutcome::New,
                content,
                previous: None,
            }),
        }
    }
    Ok(AddPlan { root: root.clone(), files, warnings })
}

/// Writes an add plan into the solution.
///
/// Checks first that nothing changed since the plan was made. If a write fails,
/// the files it created are removed and the files it edited are put back.
pub fn apply_add(plan: &AddPlan, on_progress: impl FnMut(WriteProgress)) -> Result<WriteOutcome> {
    apply_add_with(plan, on_progress, |path, bytes| fs::write(path, bytes))
}

pub(crate) fn apply_add_with(
    plan: &AddPlan,
    mut on_progress: impl FnMut(WriteProgress),
    mut writer: impl FnMut(&Path, &[u8]) -> io::Result<()>,
) -> Result<WriteOutcome> {
    let root = &plan.root;

    for f in &plan.files {
        let full = root.join(&f.path);
        let unchanged = match (&f.outcome, &f.previous) {
            (AddOutcome::New, _) => !full.exists(),
            (AddOutcome::Merged, Some(before)) => fs::read(&full).is_ok_and(|now| now == *before),
            (AddOutcome::Merged, None) => false,
        };
        if !unchanged {
            return Err(ScaffoldError::ChangedSincePlan(f.path.clone()));
        }
    }

    let mut created_files: Vec<PathBuf> = Vec::new();
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut edited: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut bytes_written = 0u64;
    let total = plan.files.len();

    for (i, f) in plan.files.iter().enumerate() {
        let full = root.join(&f.path);
        let result = (|| -> io::Result<()> {
            match f.outcome {
                AddOutcome::New => {
                    let mut dir = root.clone();
                    let parts: Vec<&str> = f.path.split('/').collect();
                    for part in &parts[..parts.len() - 1] {
                        dir.push(part);
                        if !dir.exists() {
                            fs::create_dir(&dir)?;
                            created_dirs.push(dir.clone());
                        }
                    }
                    writer(&full, &f.content)?;
                    created_files.push(full.clone());
                }
                AddOutcome::Merged => {
                    // Write beside it and rename over it, so the file is never half written.
                    let tmp = full.with_extension("forge-tmp");
                    writer(&tmp, &f.content)?;
                    edited.push((full.clone(), f.previous.clone().unwrap_or_default()));
                    if let Err(e) = fs::rename(&tmp, &full) {
                        let _ = fs::remove_file(&tmp);
                        edited.pop();
                        return Err(e);
                    }
                }
            }
            Ok(())
        })();

        if let Err(source) = result {
            let mut rolled_back = true;
            for (path, before) in edited.iter().rev() {
                rolled_back &= fs::write(path, before).is_ok();
            }
            for path in created_files.iter().rev() {
                rolled_back &= fs::remove_file(path).is_ok();
            }
            for dir in created_dirs.iter().rev() {
                let _ = fs::remove_dir(dir);
            }
            return Err(ScaffoldError::WriteFailed { path: full, source, rolled_back });
        }

        bytes_written += f.content.len() as u64;
        on_progress(WriteProgress { index: i + 1, total, path: f.path.clone() });
    }

    Ok(WriteOutcome { root: root.clone(), files_written: total, bytes_written })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::BuildSystem;
    use crate::source::DirSource;
    use crate::tokens::SequentialGuids;
    use serde_json::json;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("forge-scaffold-add-{}", uuid::Uuid::new_v4()));
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
          "value": { "components": [ { "entrypoint": "./lib/{__webpartCamel__}.js" } ] } },
        { "file": "config/config.json", "strategy": "json-set", "per": "webparts",
          "path": "localizedResources.{__webpartPascal__}Strings", "value": "lib/{__webpartCamel__}/{locale}.js" },
        { "file": "package.json", "strategy": "json-set", "path": "solutionLevel", "value": "never touched by an add" }
      ]
    }"#;

    /// A generator folder and an existing solution that already has `Alpha`.
    fn setup() -> (Tmp, Tmp, Manifest, SpfxSolution) {
        let gen = Tmp::new();
        gen.write("solution/package.json", "{}");
        gen.write("webpart/src/{__webpartCamel__}/{__webpartPascal__}.ts", "// {__webpartPascal__} {__webpartId__}\n");
        gen.write("webpart/teams/{__webpartId__}_color.png", "png");

        let sol = Tmp::new();
        sol.write("package.json", "{ \"name\": \"my-sol\" }\n");
        sol.write("gulpfile.js", "// untouched\n");
        sol.write("src/alpha/Alpha.ts", "// Alpha\n");
        sol.write(
            "config/config.json",
            "{\r\n    \"version\": \"2.0\",\r\n    \"bundles\": {\r\n        \"alpha-web-part\": { \"components\": [] }\r\n    },\r\n    \"localizedResources\": {}\r\n}\r\n",
        );
        let manifest = Manifest::from_json(MANIFEST).unwrap();
        let solution = SpfxSolution {
            root: sol.0.clone(),
            version: Some("1.20.0".into()),
            build_system: BuildSystem::Gulp,
            framework: Some("react".into()),
            package_name: Some("my-sol".into()),
            marker: None,
            warnings: vec![],
        };
        (gen, sol, manifest, solution)
    }

    fn answers(names: &[&str]) -> Answers {
        let mut a = Answers::new();
        a.insert("webparts".into(), json!(names));
        a
    }

    fn options() -> AddOptions {
        AddOptions { guids: Box::new(SequentialGuids::new()) }
    }

    fn plan(gen: &Tmp, m: &Manifest, s: &SpfxSolution, names: &[&str]) -> Result<AddPlan> {
        plan_add(s, m, &DirSource::new(&gen.0), &answers(names), options())
    }

    #[test]
    fn plans_new_files_and_one_merge_and_keeps_the_formatting_of_the_edited_file() {
        let (gen, _sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();

        let by_path: BTreeMap<&str, &AddFile> = p.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert_eq!(by_path.len(), 3, "{:?}", by_path.keys());
        assert_eq!(by_path["src/beta/Beta.ts"].outcome, AddOutcome::New);
        assert_eq!(by_path["teams/00000000-0000-0000-0000-000000000001_color.png"].outcome, AddOutcome::New);
        let config = by_path["config/config.json"];
        assert_eq!(config.outcome, AddOutcome::Merged);

        let after = String::from_utf8(config.content.clone()).unwrap();
        // 4-space indent, CRLF and the final newline of the original survive
        assert!(after.contains("\r\n    \"bundles\": {\r\n        \"alpha-web-part\""), "{after:?}");
        assert!(after.ends_with("}\r\n") && !after.replace("\r\n", "").contains('\n'));
        let doc: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert!(doc["bundles"]["alpha-web-part"].is_object(), "the existing bundle is kept");
        assert!(doc["bundles"]["beta-web-part"].is_object());
        assert_eq!(doc["localizedResources"]["BetaStrings"], "lib/beta/{locale}.js");
        // key order: the new bundle comes after the old one
        assert!(after.find("alpha-web-part").unwrap() < after.find("beta-web-part").unwrap());
    }

    #[test]
    fn solution_level_parts_and_answers_are_not_needed_and_not_touched() {
        let (gen, sol, m, s) = setup();
        // no solutionName answer: an add does not ask for it
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        assert!(p.files.iter().all(|f| f.path != "package.json"));
        assert_eq!(sol.read("package.json"), "{ \"name\": \"my-sol\" }\n");
    }

    #[test]
    fn several_components_in_one_add() {
        let (gen, _sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta", "GammaPart"]).unwrap();
        assert_eq!(p.files.len(), 5);
        let config = p.files.iter().find(|f| f.path == "config/config.json").unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&config.content).unwrap();
        assert!(doc["bundles"]["beta-web-part"].is_object() && doc["bundles"]["gamma-part-web-part"].is_object());
    }

    #[test]
    fn an_empty_list_is_refused_by_the_name_rule_like_in_a_new_solution() {
        let (gen, _sol, m, s) = setup();
        assert!(matches!(plan(&gen, &m, &s, &[]), Err(ScaffoldError::Validation(_))));
    }

    #[test]
    fn a_generator_whose_list_may_be_empty_plans_nothing() {
        let (gen, _sol, _m, s) = setup();
        let m = Manifest::from_json(&MANIFEST.replace(", \"rule\": \"webpart-names\"", "")).unwrap();
        assert!(plan(&gen, &m, &s, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_component_that_already_exists_is_refused() {
        let (gen, _sol, m, s) = setup();
        let err = plan(&gen, &m, &s, &["Alpha"]).unwrap_err();
        assert!(matches!(err, ScaffoldError::ExistsInSolution(ref p) if p == "src/alpha/Alpha.ts"), "{err}");
    }

    #[test]
    fn another_patch_of_the_same_minor_version_only_warns() {
        let (gen, _sol, m, mut s) = setup();
        s.version = Some("1.20.2".into());
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("1.20.2") && w.contains("1.20.0")), "{:?}", p.warnings);
    }

    #[test]
    fn another_spfx_version_is_refused_and_an_unknown_one_only_warns() {
        let (gen, _sol, m, mut s) = setup();
        s.version = Some("1.21.1".into());
        let err = plan(&gen, &m, &s, &["Beta"]).unwrap_err();
        assert!(matches!(err, ScaffoldError::Incompatible(_)));
        assert!(err.to_string().contains("1.20.0") && err.to_string().contains("1.21.1"));

        s.version = None;
        assert!(plan(&gen, &m, &s, &["Beta"]).unwrap().warnings.iter().any(|w| w.contains("version")));
    }

    #[test]
    fn a_solution_without_the_file_to_edit_is_refused_with_a_hint() {
        let (gen, sol, m, s) = setup();
        fs::remove_file(sol.0.join("config/config.json")).unwrap();
        let err = plan(&gen, &m, &s, &["Beta"]).unwrap_err().to_string();
        assert!(err.contains("config/config.json") && err.contains("right folder"), "{err}");
    }

    #[test]
    fn a_json_file_with_comments_is_refused_not_rewritten() {
        let (gen, sol, m, s) = setup();
        sol.write("config/config.json", "{\n  // my note\n  \"bundles\": {}\n}\n");
        let err = plan(&gen, &m, &s, &["Beta"]).unwrap_err().to_string();
        assert!(err.contains("comments"), "{err}");
        assert!(sol.read("config/config.json").contains("// my note"));
    }

    #[test]
    fn a_generator_without_per_component_folders_cannot_add() {
        let (gen, _sol, _m, s) = setup();
        let m = Manifest::from_json(
            r#"{ "id": "x", "version": "1.20.0", "prompts": [ { "key": "n", "type": "text" } ], "folderFrom": "n" }"#,
        )
        .unwrap();
        assert!(matches!(plan(&gen, &m, &s, &[]), Err(ScaffoldError::Incompatible(_))));
    }

    #[test]
    fn applying_writes_the_new_files_edits_one_and_leaves_everything_else_alone() {
        let (gen, sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        let before_gulp = sol.read("gulpfile.js");

        let out = apply_add(&p, |_| {}).unwrap();
        assert_eq!(out.files_written, 3);
        assert_eq!(sol.read("src/beta/Beta.ts"), "// Beta 00000000-0000-0000-0000-000000000001\n");
        assert!(sol.read("config/config.json").contains("beta-web-part"));
        assert_eq!(sol.read("gulpfile.js"), before_gulp);
        assert_eq!(sol.read("src/alpha/Alpha.ts"), "// Alpha\n");
        assert!(!sol.0.join("config/config.forge-tmp").exists(), "no temp file is left");
    }

    #[test]
    fn a_file_changed_after_the_plan_stops_the_apply_before_anything_is_written() {
        let (gen, sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        let mut edited = sol.read("config/config.json");
        edited.push_str("\r\n");
        sol.write("config/config.json", &edited);

        let err = apply_add(&p, |_| {}).unwrap_err();
        assert!(matches!(err, ScaffoldError::ChangedSincePlan(ref f) if f == "config/config.json"), "{err}");
        assert!(!sol.0.join("src/beta").exists());
    }

    #[test]
    fn a_file_that_appeared_after_the_plan_stops_the_apply() {
        let (gen, sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        sol.write("src/beta/Beta.ts", "mine");
        assert!(matches!(apply_add(&p, |_| {}), Err(ScaffoldError::ChangedSincePlan(_))));
        assert_eq!(sol.read("src/beta/Beta.ts"), "mine");
    }

    #[test]
    fn a_failed_write_puts_everything_back() {
        let (gen, sol, m, s) = setup();
        let p = plan(&gen, &m, &s, &["Beta"]).unwrap();
        let config_before = sol.read("config/config.json");

        // files are written in path order: config/config.json (merged), src/..., teams/...
        // fail on the last one, after the config was already replaced
        let mut writes = 0;
        let err = apply_add_with(&p, |_| {}, |path, bytes| {
            writes += 1;
            if writes == 3 {
                return Err(io::Error::other("disk full"));
            }
            fs::write(path, bytes)
        })
        .unwrap_err();
        assert!(matches!(err, ScaffoldError::WriteFailed { rolled_back: true, .. }), "{err}");

        assert_eq!(sol.read("config/config.json"), config_before, "the edited file is restored");
        assert!(!sol.0.join("src/beta").exists(), "created folders are removed");
        assert!(!sol.0.join("teams").exists());
        assert!(!sol.0.join("config/config.forge-tmp").exists());
        assert_eq!(sol.read("src/alpha/Alpha.ts"), "// Alpha\n");
    }
}
