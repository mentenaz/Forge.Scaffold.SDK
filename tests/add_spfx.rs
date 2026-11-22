//! Adds a web part to a real SPFx 1.20.0 (gulp) solution made by our own
//! generator: detect it from its files, plan the add, apply it, and compare with
//! what `yo` was measured to change (`docs/spfx-add-findings.md`): the component
//! folder and two Teams icons are new, `config/config.json` is edited, and
//! nothing else changes.
//!
//! Needs the templates repo next to this one (`../Forge.Scaffold.Templates`, or
//! `FORGE_TEMPLATES_REPO`); skipped otherwise.

use forge_scaffold::{
    apply, apply_add, apply_edit, detect_spfx, plan, plan_add, plan_edit, revert_edit, AddOptions, AddOutcome, Answers, BuildSystem, DirSource,
    Manifest, PlanOptions, ScaffoldError, SequentialGuids, SolutionEdit, UndoRecord,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn generator() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    let dir = repo.join("templates/spfxv.1.20.0");
    dir.join("manifest.json").is_file().then_some(dir)
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn adding_a_web_part_to_a_1_20_0_solution_matches_what_yo_changes() {
    let Some(gen) = generator() else {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("templates repo not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-add-spfx-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();

    let manifest = Manifest::from_json(&fs::read_to_string(gen.join("manifest.json")).unwrap()).unwrap();
    let source = DirSource::new(&gen);

    // a solution with one web part, as the engine makes it
    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!("my-sol"));
    answers.insert("webparts".into(), json!(["HelloWorld"]));
    let mut options = PlanOptions::new(&scratch);
    options.guids = Box::new(SequentialGuids::new());
    apply(&plan(&manifest, &source, &answers, options).unwrap(), |_| {}).unwrap();
    let root = scratch.join("my-sol");

    // detection reads the files, from a file deep inside the solution
    let found = detect_spfx(&root.join("src/webparts/helloWorld/HelloWorldWebPart.ts"))
        .unwrap()
        .expect("a solution");
    assert_eq!(found.root, root);
    assert_eq!(found.version.as_deref(), Some("1.20.0"));
    assert_eq!(found.build_system, BuildSystem::Gulp);
    assert_eq!(found.framework.as_deref(), Some("react"));
    assert!(found.marker.is_some(), ".yo-rc.json is read as a hint");
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    // a folder outside any solution is not one
    assert!(detect_spfx(&scratch).unwrap().is_none());

    // plan the add of a second web part
    let before = snapshot(&root);
    let mut add_answers = Answers::new();
    add_answers.insert("webparts".into(), json!(["OtherPart"]));
    let add = plan_add(&found, &manifest, &source, &add_answers, AddOptions::new()).unwrap();

    let new: Vec<&str> = add.files.iter().filter(|f| f.outcome == AddOutcome::New).map(|f| f.path.as_str()).collect();
    let merged: Vec<&str> = add.files.iter().filter(|f| f.outcome == AddOutcome::Merged).map(|f| f.path.as_str()).collect();
    assert_eq!(merged, vec!["config/config.json"]);
    assert_eq!(new.len(), 11, "yo adds 11 files for a React web part: {new:?}");
    assert!(new.iter().filter(|p| p.starts_with("src/webparts/otherPart/")).count() == 9);
    assert!(new.iter().filter(|p| p.starts_with("teams/")).count() == 2);
    // planning wrote nothing
    assert_eq!(snapshot(&root), before);

    // apply, then compare with the "before" snapshot
    let out = apply_add(&add, |_| {}).unwrap();
    assert_eq!(out.files_written, 12);
    let after = snapshot(&root);
    for (path, bytes) in &before {
        if path != "config/config.json" {
            assert_eq!(after.get(path), Some(bytes), "{path} must not change");
        }
    }
    let added: Vec<&String> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    assert_eq!(added.len(), 11);

    // config.json: the old entries stay, the new ones are added, and no line of
    // the original was rewritten
    let old = String::from_utf8(before["config/config.json"].clone()).unwrap();
    let new_text = String::from_utf8(after["config/config.json"].clone()).unwrap();
    let doc: Value = serde_json::from_str(&new_text).unwrap();
    assert!(doc["bundles"]["hello-world-web-part"].is_object());
    assert!(doc["bundles"]["other-part-web-part"].is_object());
    assert!(doc["localizedResources"]["HelloWorldWebPartStrings"].is_string());
    assert!(doc["localizedResources"]["OtherPartWebPartStrings"].is_string());
    let mut rest = new_text.lines();
    for line in old.lines() {
        // each original line appears in order; only the entry's trailing comma may differ
        let want = line.trim_end_matches(',');
        assert!(rest.any(|l| l.trim_end_matches(',') == want), "line lost or rewritten: {line}");
    }

    // the new web part has its own ids and the class it should
    let class = String::from_utf8(after["src/webparts/otherPart/OtherPartWebPart.ts"].clone()).unwrap();
    assert!(class.contains("export default class OtherPartWebPart"));
    let ids: Vec<&String> = after.keys().filter(|k| k.starts_with("teams/")).collect();
    assert_eq!(ids.len(), 4, "two web parts, two icons each");

    // adding the same web part again is refused and changes nothing
    let again = plan_add(&found, &manifest, &source, &add_answers, AddOptions::new()).unwrap_err();
    assert!(matches!(again, ScaffoldError::ComponentExists { .. }), "{again}");
    assert_eq!(snapshot(&root), after);

    let _ = fs::remove_dir_all(&scratch);
}

fn add_again(found: &forge_scaffold::SpfxSolution, manifest: &Manifest, source: &DirSource) -> forge_scaffold::AddPlan {
    let mut add_answers = Answers::new();
    add_answers.insert("webparts".into(), json!(["OtherPart"]));
    plan_add(found, manifest, source, &add_answers, AddOptions::new()).unwrap()
}

/// The add can be undone from a stored record: the solution comes back byte for byte, and the record survives being
/// saved as text the way a host would keep it.
#[test]
fn an_add_to_a_real_1_20_0_solution_can_be_undone() {
    let Some(gen) = generator() else {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("templates repo not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-undo-spfx-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let manifest = Manifest::from_json(&fs::read_to_string(gen.join("manifest.json")).unwrap()).unwrap();
    let source = DirSource::new(&gen);

    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!("my-sol"));
    answers.insert("webparts".into(), json!(["HelloWorld"]));
    let mut options = PlanOptions::new(&scratch);
    options.guids = Box::new(SequentialGuids::new());
    apply(&plan(&manifest, &source, &answers, options).unwrap(), |_| {}).unwrap();
    let root = scratch.join("my-sol");
    let found = detect_spfx(&root).unwrap().unwrap();
    let before = snapshot(&root);

    let mut add_answers = Answers::new();
    add_answers.insert("webparts".into(), json!(["OtherPart"]));
    let add = plan_add(&found, &manifest, &source, &add_answers, AddOptions::new()).unwrap();
    let record_text = add.undo_record("Added web part OtherPart").to_json(); // before the apply, and stored as text
    apply_add(&add, |_| {}).unwrap();
    assert_ne!(snapshot(&root), before);

    let record = UndoRecord::from_json(&record_text).unwrap();
    let out = revert_edit(&root, &record).unwrap();
    assert_eq!(out.restored, ["config/config.json"]);
    assert_eq!(out.removed.len(), 11);
    assert_eq!(snapshot(&root), before, "the solution is exactly as it was, no file or folder left over");

    // a change made after an add is protected: the revert refuses and changes nothing
    let again = add_again(&found, &manifest, &source);
    let again_record = again.undo_record("Added web part OtherPart again");
    apply_add(&again, |_| {}).unwrap();
    fs::write(root.join("config/config.json"), "{ \"mine\": true }").unwrap();
    let applied = snapshot(&root);
    match revert_edit(&root, &again_record).unwrap_err() {
        ScaffoldError::RevertConflict(files) => assert_eq!(files, ["config/config.json"]),
        other => panic!("{other}"),
    }
    assert_eq!(snapshot(&root), applied, "nothing was changed");
    let _ = fs::remove_dir_all(&scratch);
}

/// Editing the name, description and version of a real solution changes only those values in
/// `config/package-solution.json`, never the id, and can be undone.
#[test]
fn editing_a_real_1_20_0_solution_changes_three_values_and_can_be_undone() {
    let Some(gen) = generator() else {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("templates repo not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-edit-spfx-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let manifest = Manifest::from_json(&fs::read_to_string(gen.join("manifest.json")).unwrap()).unwrap();
    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!("my-sol"));
    answers.insert("webparts".into(), json!(["HelloWorld"]));
    let mut options = PlanOptions::new(&scratch);
    options.guids = Box::new(SequentialGuids::new());
    apply(&plan(&manifest, &DirSource::new(&gen), &answers, options).unwrap(), |_| {}).unwrap();
    let root = scratch.join("my-sol");
    let found = detect_spfx(&root).unwrap().unwrap();
    let before = snapshot(&root);
    let read = |root: &Path| -> Value {
        let text = fs::read_to_string(root.join("config/package-solution.json")).unwrap();
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("not plain JSON: {text}"))
    };
    let original = read(&root);

    let edit = SolutionEdit { name: Some("Intranet".into()), description: Some("The company intranet".into()), version: Some("2.0.0.0".into()) };
    let planned = plan_edit(&found, &edit).unwrap();
    assert_eq!(planned.files.len(), 1);
    let record = planned.undo_record_as("edit", "Edited the solution");
    apply_edit(&planned, |_| {}).unwrap();

    let after = read(&root);
    assert_eq!(after["solution"]["name"], "Intranet");
    assert_eq!(after["solution"]["version"], "2.0.0.0");
    assert_eq!(after["solution"]["features"][0]["description"], "The company intranet");
    assert_eq!(after["solution"]["id"], original["solution"]["id"], "the id is never changed");
    // everything else in the file is the same
    let mut expected = original.clone();
    expected["solution"]["name"] = json!("Intranet");
    expected["solution"]["version"] = json!("2.0.0.0");
    expected["solution"]["features"][0]["description"] = json!("The company intranet");
    assert_eq!(after, expected);
    let changed: Vec<String> = snapshot(&root).into_iter().filter(|(k, v)| before.get(k) != Some(v)).map(|(k, _)| k).collect();
    assert_eq!(changed, ["config/package-solution.json"], "no other file changed");

    revert_edit(&root, &UndoRecord::from_json(&record.to_json()).unwrap()).unwrap();
    assert_eq!(snapshot(&root), before, "back to exactly as it was");
    let _ = fs::remove_dir_all(&scratch);
}
