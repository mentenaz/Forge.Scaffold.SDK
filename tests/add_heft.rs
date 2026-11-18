//! Adds components to real heft solutions (SPFx 1.24.1) made from the official
//! SharePoint/spfx templates: detect the solution from its files, plan the add,
//! apply it. Needs a clone of the SPFx repo (`../spfx` or `FORGE_UPSTREAM_SPFX`);
//! skipped otherwise.

use forge_scaffold::{
    apply, apply_add, detect_spfx, plan, plan_add, AddOptions, AddOutcome, AddPlan, BuildSystem, DirSource,
    PlanOptions, ScaffoldError, SequentialGuids, SpfxSolution, UpstreamInputs, UpstreamSource,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn upstream_repo() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_UPSTREAM_SPFX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spfx"));
    repo.join("templates").is_dir().then_some(repo)
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

/// A new solution from an official template, the way the panel would make it.
fn scaffold(repo: &Path, template: &str, component: &str, scratch: &Path) -> PathBuf {
    let (def, source) = UpstreamSource::load(&DirSource::new(repo), &format!("templates/{template}")).unwrap();
    fs::create_dir_all(scratch).unwrap();
    let mut options = PlanOptions::new(scratch);
    options.guids = Box::new(SequentialGuids::new());
    let answers = UpstreamInputs { component_name: component.into(), ..Default::default() }.answers();
    let planned = plan(&def.manifest().unwrap(), &source, &answers, options).unwrap();
    apply(&planned, |_| {}).unwrap();
    planned.target
}

fn plan_for(repo: &Path, template: &str, solution: &SpfxSolution, component: &str) -> Result<AddPlan, ScaffoldError> {
    let (def, source) = UpstreamSource::load(&DirSource::new(repo), &format!("templates/{template}")).unwrap();
    let answers = UpstreamInputs { component_name: component.into(), ..Default::default() }.add_answers();
    plan_add(solution, &def.manifest().unwrap(), &source, &answers, AddOptions::new())
}

fn paths(plan: &AddPlan, outcome: AddOutcome) -> Vec<&str> {
    plan.files.iter().filter(|f| f.outcome == outcome).map(|f| f.path.as_str()).collect()
}

#[test]
fn a_second_react_web_part_goes_into_a_react_solution_by_changing_only_config_json() {
    let Some(repo) = upstream_repo() else {
        eprintln!("SharePoint/spfx clone not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-add-heft-a-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let root = scaffold(&repo, "webpart-react", "Hello World", &scratch);

    let found = detect_spfx(&root.join("src/webparts/helloWorldWebPart/HelloWorldWebPart.ts"))
        .unwrap()
        .expect("a solution");
    assert_eq!(found.root, root);
    assert_eq!(found.version.as_deref(), Some("1.24.1"));
    assert_eq!(found.build_system, BuildSystem::Heft);
    assert_eq!(found.framework.as_deref(), Some("react"));
    assert!(found.marker.is_none(), "the official CLI writes no .yo-rc.json");

    let before = snapshot(&root);
    let add = plan_for(&repo, "webpart-react", &found, "Other Part").unwrap();
    assert_eq!(paths(&add, AddOutcome::Merged), ["config/config.json"]);
    let new = paths(&add, AddOutcome::New);
    assert_eq!(new.len(), 11, "{new:?}");
    assert_eq!(new.iter().filter(|p| p.starts_with("src/webparts/otherPartWebPart/")).count(), 9);
    assert_eq!(new.iter().filter(|p| p.starts_with("teams/")).count(), 2);
    // the files that only differ by the component name are left alone and reported
    assert!(add.kept.contains(&"README.md".to_string()), "{:?}", add.kept);
    assert_eq!(snapshot(&root), before, "planning writes nothing");

    apply_add(&add, |_| {}).unwrap();
    let after = snapshot(&root);
    for (path, bytes) in &before {
        if path != "config/config.json" {
            assert_eq!(after.get(path), Some(bytes), "{path} must not change");
        }
    }
    assert_eq!(after.keys().filter(|k| !before.contains_key(*k)).count(), 11);
    let config: Value = serde_json::from_slice(&after["config/config.json"]).unwrap();
    assert!(config["bundles"]["hello-world-web-part"].is_object() && config["bundles"]["other-part-web-part"].is_object());
    assert!(config["localizedResources"]["HelloWorldWebPartStrings"].is_string());
    assert!(config["localizedResources"]["OtherPartWebPartStrings"].is_string());

    // the solution's feature list is not padded with an empty feature per component
    let solution: Value = serde_json::from_slice(&after["config/package-solution.json"]).unwrap();
    assert_eq!(solution["solution"]["features"].as_array().unwrap().len(), 1);

    // the same component again is refused, and so is anything that would overwrite
    let again = detect_spfx(&root).unwrap().unwrap();
    assert!(matches!(
        plan_for(&repo, "webpart-react", &again, "Other Part"),
        Err(ScaffoldError::ExistsInSolution(_))
    ));
    // a third one still goes in
    assert!(plan_for(&repo, "webpart-react", &again, "Third Part").is_ok());
    assert_eq!(snapshot(&root), after);
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_react_web_part_added_to_a_no_framework_solution_brings_its_dependencies_and_keeps_the_solutions_files() {
    let Some(repo) = upstream_repo() else {
        eprintln!("SharePoint/spfx clone not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-add-heft-b-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let root = scaffold(&repo, "webpart-noframework", "Plain Part", &scratch);

    let found = detect_spfx(&root).unwrap().unwrap();
    assert_eq!(found.build_system, BuildSystem::Heft);
    assert_eq!(found.framework, None);

    let before = snapshot(&root);
    let add = plan_for(&repo, "webpart-react", &found, "Fancy Part").unwrap();
    let mut merged = paths(&add, AddOutcome::Merged);
    merged.sort_unstable();
    assert_eq!(merged, ["config/config.json", "package.json"]);
    // the solution's own lint and ignore files differ and are kept, not replaced
    assert!(add.kept.contains(&".eslintrc.js".to_string()), "{:?}", add.kept);

    apply_add(&add, |_| {}).unwrap();
    let after = snapshot(&root);
    for (path, bytes) in &before {
        if path != "config/config.json" && path != "package.json" {
            assert_eq!(after.get(path), Some(bytes), "{path} must not change");
        }
    }

    let old: Value = serde_json::from_slice(&before["package.json"]).unwrap();
    let new: Value = serde_json::from_slice(&after["package.json"]).unwrap();
    for section in ["dependencies", "devDependencies"] {
        // every existing entry is still there, with the same version, in the same order, first
        let old_keys: Vec<&String> = old[section].as_object().unwrap().keys().collect();
        let new_keys: Vec<&String> = new[section].as_object().unwrap().keys().collect();
        assert_eq!(&new_keys[..old_keys.len()], &old_keys[..], "{section}: existing entries first and in order");
        for k in &old_keys {
            assert_eq!(new[section][k.as_str()], old[section][k.as_str()]);
        }
    }
    for dep in ["react", "react-dom", "@fluentui/react"] {
        assert!(new["dependencies"][dep].is_string(), "{dep} was added");
    }
    assert!(new["devDependencies"]["@types/react"].is_string());
    assert_eq!(new["name"], old["name"]);
    assert_eq!(new["scripts"], old["scripts"]);

    // only lines were added: every original line of package.json is still there, in order
    let old_text = String::from_utf8(before["package.json"].clone()).unwrap();
    let new_text = String::from_utf8(after["package.json"].clone()).unwrap();
    let mut rest = new_text.lines();
    for line in old_text.lines() {
        let want = line.trim_end_matches(',');
        assert!(rest.any(|l| l.trim_end_matches(',') == want), "line lost or rewritten: {line}");
    }
    let _ = fs::remove_dir_all(&scratch);
}
