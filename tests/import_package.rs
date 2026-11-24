//! Importing a package into a real SPFx 1.21.1 solution (made by our own generator): the made-up package in
//! `tests/fixtures/acme-data-services` has a shared core, two modules, peers on the SPFx packages and on `@pnp/sp`, a build
//! tool as a dev dependency, and one import that nothing declares. Nothing in it is a real package of anyone's.
//!
//! Needs the templates repo next to this one (`../Forge.Scaffold.Templates`, or `FORGE_TEMPLATES_REPO`); skipped otherwise.

use forge_scaffold::{
    apply, apply_import, detect_spfx, inspect_import, plan, plan_import, DependencyOutcome, DirSource, HaveFrom, ImportOptions, Manifest,
    ModuleStatus, PackageManager, PlanOptions, PostAction, ScaffoldError, SequentialGuids, SpfxSolution, UndoRecord,
};
use forge_scaffold::{revert_edit, Answers};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const TARGET: &str = "src/services/acme-data-services";

fn repo() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    repo.join("templates/spfxv.1.21.1/manifest.json").is_file().then_some(repo)
}

fn skip_or_fail() {
    assert!(
        std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
        "the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
    );
    eprintln!("templates repo not found, skipping");
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-import-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            fs::copy(e.path(), target).unwrap();
        }
    }
}

/// The package: the fixture, plus what the repository does not keep (a secret-looking file, build output).
fn package(scratch: &Path) -> (Manifest, DirSource) {
    let dir = scratch.join("acme-data-services");
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acme-data-services"), &dir);
    fs::write(dir.join("src/ItemOps/.env"), "API_TOKEN=not-a-real-token\n").unwrap();
    fs::create_dir_all(dir.join("dist")).unwrap();
    fs::write(dir.join("dist/index.js"), "// build output\n").unwrap();
    (Manifest::from_json(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap(), DirSource::new(&dir))
}

fn solution_in(repo: &Path, scratch: &Path) -> PathBuf {
    let dir = repo.join("templates/spfxv.1.21.1");
    let manifest = Manifest::from_json(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!("my-sol"));
    answers.insert("webparts".into(), json!(["HelloWorld"]));
    let mut options = PlanOptions::new(scratch);
    options.guids = Box::new(SequentialGuids::new());
    apply(&plan(&manifest, &DirSource::new(&dir), &answers, options).unwrap(), |_| {}).unwrap();
    let root = scratch.join("my-sol");
    // what `npm install` leaves: the solution does not declare @microsoft/sp-http itself, it comes with sp-webpart-base
    stub_installed(&root, "@microsoft/sp-http", "1.21.1");
    root
}

/// A package in `node_modules`, as `npm install` leaves it (its version is all the engine reads).
fn stub_installed(root: &Path, name: &str, version: &str) {
    let dir = root.join("node_modules").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("package.json"), format!(r#"{{ "name": "{name}", "version": "{version}" }}"#)).unwrap();
}

/// Sets (or, with `None`, removes) a dependency of the solution, keeping the file otherwise as it is.
fn set_dep(root: &Path, name: &str, range: Option<&str>) {
    let path = root.join("package.json");
    let mut pkg: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    match range {
        Some(r) => pkg["dependencies"][name] = json!(r),
        None => {
            pkg["dependencies"].as_object_mut().unwrap().remove(name);
        }
    }
    fs::write(&path, serde_json::to_string_pretty(&pkg).unwrap() + "\n").unwrap();
}

fn found(root: &Path) -> SpfxSolution {
    detect_spfx(root).unwrap().unwrap()
}

fn outcome<'a>(list: &'a [DependencyOutcome], name: &str) -> &'a DependencyOutcome {
    list.iter().find(|d| d.name() == name).unwrap_or_else(|| panic!("no outcome for {name}: {list:?}"))
}

fn new_files(plan: &forge_scaffold::ImportPlan) -> Vec<&str> {
    plan.add.files.iter().filter(|f| f.outcome == forge_scaffold::AddOutcome::New).map(|f| f.path.as_str()).collect()
}

#[test]
fn a_solution_without_a_peer_of_the_package_is_refused_with_the_reason_and_nothing_changes() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("peer");
    let root = solution_in(&repo, &scratch);
    let (manifest, source) = package(&scratch);
    let before = snapshot(&root);

    // the generator's solution has no @pnp/sp
    let err = plan_import(&found(&root), &source, &manifest, ImportOptions::new()).unwrap_err();
    assert!(matches!(err, ScaffoldError::Incompatible(_)), "{err}");
    assert!(err.to_string().contains("@pnp/sp ^4.0.0") && err.to_string().contains("peer"), "{err}");
    assert_eq!(snapshot(&root), before);

    // @microsoft/sp-http is not declared by this solution either: it is the installed one that counts
    fs::remove_dir_all(root.join("node_modules/@microsoft/sp-http")).unwrap();
    let err = plan_import(&found(&root), &source, &manifest, ImportOptions::new()).unwrap_err().to_string();
    assert!(err.contains("@microsoft/sp-http") && err.contains("@pnp/sp"), "both are named: {err}");
    stub_installed(&root, "@microsoft/sp-http", "1.21.1");

    // inspect says the same as data, and does not fail
    let state = inspect_import(&found(&root), &source, &manifest).unwrap();
    assert!(matches!(outcome(&state.dependencies, "@pnp/sp"), DependencyOutcome::MissingPeer { range, .. } if range == "^4.0.0"));
    assert!(state.dependencies.iter().any(DependencyOutcome::blocks));
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn importing_one_module_copies_the_core_and_it_and_merges_what_the_package_needs() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("one");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^4.4.0"));
    set_dep(&root, "zod", Some("^3.20.0"));
    let (manifest, source) = package(&scratch);
    let before = snapshot(&root);

    let mut options = ImportOptions::new();
    options.modules = vec!["ItemOps".into()];
    let plan = plan_import(&found(&root), &source, &manifest, options).unwrap();
    assert_eq!(snapshot(&root), before, "planning writes nothing");

    // the files: the core and ItemOps below the target, with the package's folders; no ListOps, no tests, no build output, no secret
    let mut want: Vec<String> =
        ["ctx.ts", "errors.ts", "types/index.ts", "ItemOps/Create.ts", "ItemOps/Get.ts", "ItemOps/index.ts"].iter().map(|f| format!("{TARGET}/{f}")).collect();
    want.sort();
    let mut got: Vec<String> = new_files(&plan).iter().map(|s| s.to_string()).collect();
    got.sort();
    assert_eq!(got, want);
    assert!(plan.add.kept.is_empty());
    assert!(plan.notes.iter().any(|n| n.contains("ItemOps/.env") && n.contains("secret")), "{:?}", plan.notes);

    // the needs
    let d = &plan.dependencies;
    assert_eq!(outcome(d, "date-fns"), &DependencyOutcome::Add { name: "date-fns".into(), range: "^3.6.0".into() });
    assert!(matches!(outcome(d, "zod"), DependencyOutcome::UpgradeAvailable { from, to, .. } if from == "3.20.0" && to == "^3.23.0"));
    assert!(matches!(outcome(d, "@pnp/sp"), DependencyOutcome::Skip { have, have_from: HaveFrom::Declared, .. } if have == "4.4.0"));
    assert!(matches!(outcome(d, "@microsoft/sp-http"), DependencyOutcome::Untouched { category, .. } if category == "spfx"));
    assert!(matches!(outcome(d, "@microsoft/sp-core-library"), DependencyOutcome::Untouched { .. }));
    assert!(d.iter().all(|o| o.name() != "typescript" && o.name() != "tsup"), "the author's dev dependencies are not the solution's business");
    assert!(!plan.has_clashes());

    // package.json: date-fns added, zod and the rest as they were, format kept
    let merged = plan.add.files.iter().find(|f| f.path == "package.json").expect("package.json is merged");
    let pkg: Value = serde_json::from_slice(&merged.content).unwrap();
    assert_eq!(pkg["dependencies"]["date-fns"], "^3.6.0");
    assert_eq!(pkg["dependencies"]["zod"], "^3.20.0", "an upgrade is reported, not applied");
    let old: Value = serde_json::from_slice(&before["package.json"]).unwrap();
    assert_eq!(pkg["dependencies"].as_object().unwrap().len(), old["dependencies"].as_object().unwrap().len() + 1);

    // what the user is told
    let w = plan.add.warnings.join("\n");
    assert!(w.contains("zod") && w.contains("^3.23.0") && w.contains("3.20.0") && w.contains("left as it is"), "{w}");
    assert!(!w.contains("lodash-es"), "ListOps was not chosen, so its undeclared import is not this plan's business: {w}");
    assert!(plan.notes.iter().any(|n| n.contains("devDependencies") && n.contains("tsup") && n.contains("typescript")), "{:?}", plan.notes);
    assert!(plan.notes.iter().any(|n| n.contains("import { getItems }")), "the package's own note");

    // npm install is asked for, because package.json changes
    assert_eq!(plan.add.post.len(), 1);
    assert_eq!(plan.add.post[0].action, PostAction::Install);
    assert_eq!(plan.add.post[0].package_manager, Some(PackageManager::Npm));
    assert_eq!(plan.add.post[0].cwd, root);

    // apply, and undo back to the exact bytes
    let record = plan.undo_record("Imported acme-data-services").to_json();
    apply_import(&plan, |_| {}).unwrap();
    let after = snapshot(&root);
    let changed: Vec<&String> = after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).map(|(k, _)| k).collect();
    assert_eq!(changed.len(), 7, "{changed:?}");
    assert!(!after.keys().any(|k| k.contains("ListOps") || k.ends_with(".env") || k.contains("dist/")));
    let out = revert_edit(&root, &UndoRecord::from_json(&record).unwrap()).unwrap();
    assert_eq!(out.restored, ["package.json"]);
    assert_eq!(snapshot(&root), before, "no file or folder left over");
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_second_import_adds_the_other_module_and_skips_the_core_that_is_already_there() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("two");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^4.4.0"));
    let (manifest, source) = package(&scratch);

    let mut first = ImportOptions::new();
    first.modules = vec!["ItemOps".into()];
    apply_import(&plan_import(&found(&root), &source, &manifest, first).unwrap(), |_| {}).unwrap();
    let after_first = snapshot(&root);

    let mut second = ImportOptions::new();
    second.modules = vec!["listops".into()]; // the case of a name does not matter
    let plan = plan_import(&found(&root), &source, &manifest, second).unwrap();
    let mut got: Vec<&str> = new_files(&plan);
    got.sort();
    assert_eq!(got, [format!("{TARGET}/ListOps/Get.ts"), format!("{TARGET}/ListOps/index.ts")]);
    assert!(plan.add.files.iter().all(|f| f.path != "package.json"), "date-fns is there now: nothing to merge");
    assert!(plan.add.post.is_empty(), "nothing to install");
    assert!(matches!(outcome(&plan.dependencies, "date-fns"), DependencyOutcome::Skip { .. }));
    // this module's code imports lodash-es, which nobody declares
    let w = plan.add.warnings.join("\n");
    assert!(w.contains("ListOps/Get.ts") && w.contains("'lodash-es'"), "{w}");
    assert!(!w.contains("@pnp/sp/"), "a subpath of a declared package is not undeclared: {w}");
    apply_import(&plan, |_| {}).unwrap();
    let after_second = snapshot(&root);
    for (path, bytes) in &after_first {
        assert_eq!(after_second.get(path), Some(bytes), "{path} must not change");
    }

    // everything again: nothing to do
    let mut all = ImportOptions::new();
    all.modules = vec!["ItemOps".into(), "ListOps".into()];
    let again = plan_import(&found(&root), &source, &manifest, all).unwrap();
    assert!(again.add.is_empty(), "{:?}", again.add.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_clash_is_reported_and_the_solutions_version_stays() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("clash");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^3.0.0")); // the package wants ^4.0.0 of its peer
    set_dep(&root, "zod", Some("^2.1.0")); // and ^3.23.0
    let (manifest, source) = package(&scratch);

    let plan = plan_import(&found(&root), &source, &manifest, ImportOptions::new()).unwrap();
    assert!(plan.has_clashes());
    for name in ["zod", "@pnp/sp"] {
        match outcome(&plan.dependencies, name) {
            DependencyOutcome::Clash { have, want, reason, .. } => {
                assert!(!have.is_empty() && !want.is_empty() && reason.contains("no version fits both"), "{name}: {reason}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    let merged: Value = serde_json::from_slice(&plan.add.files.iter().find(|f| f.path == "package.json").unwrap().content).unwrap();
    assert_eq!(merged["dependencies"]["zod"], "^2.1.0");
    assert_eq!(merged["dependencies"]["@pnp/sp"], "^3.0.0");
    assert!(plan.add.warnings.iter().any(|w| w.starts_with("zod:") && w.contains("left as it is")));
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn the_version_that_is_installed_is_read_from_node_modules_before_the_lock_and_the_range() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("installed");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^4.4.0"));
    set_dep(&root, "zod", Some("^3.0.0")); // the range alone would say 3.0.0, which is too old
    fs::write(
        root.join("package-lock.json"),
        r#"{ "lockfileVersion": 3, "packages": { "node_modules/zod": { "version": "3.23.8" }, "node_modules/@pnp/sp": { "version": "4.4.1" } } }"#,
    )
    .unwrap();
    let (manifest, source) = package(&scratch);

    let from_lock = plan_import(&found(&root), &source, &manifest, ImportOptions::new()).unwrap();
    assert!(matches!(outcome(&from_lock.dependencies, "zod"), DependencyOutcome::Skip { have, have_from: HaveFrom::Lock, .. } if have == "3.23.8"));

    // what is really installed wins over the lock
    stub_installed(&root, "zod", "3.21.4");
    let from_disk = plan_import(&found(&root), &source, &manifest, ImportOptions::new()).unwrap();
    assert!(matches!(outcome(&from_disk.dependencies, "zod"), DependencyOutcome::UpgradeAvailable { from, .. } if from == "3.21.4"));
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn what_the_solution_has_of_the_package_is_told_per_module() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("inspect");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^4.4.0"));
    let (manifest, source) = package(&scratch);
    let status = |root: &Path| -> (ModuleStatus, Vec<(String, ModuleStatus)>) {
        let s = inspect_import(&found(root), &source, &manifest).unwrap();
        (s.core.state, s.modules.iter().map(|m| (m.name.clone(), m.state)).collect())
    };

    assert_eq!(status(&root), (ModuleStatus::Absent, vec![("ItemOps".into(), ModuleStatus::Absent), ("ListOps".into(), ModuleStatus::Absent)]));
    let state = inspect_import(&found(&root), &source, &manifest).unwrap();
    assert_eq!(state.modules[0].title, "Item operations");
    assert_eq!(state.modules[0].missing.len(), 3, "{:?}", state.modules[0].missing);
    assert!(state.modules[0].missing.iter().all(|p| p.starts_with(&format!("{TARGET}/ItemOps/"))), "paths only, below the solution");
    assert_eq!(state.core.title, "Shared files");

    let mut options = ImportOptions::new();
    options.modules = vec!["ItemOps".into()];
    apply_import(&plan_import(&found(&root), &source, &manifest, options).unwrap(), |_| {}).unwrap();
    assert_eq!(status(&root), (ModuleStatus::Present, vec![("ItemOps".into(), ModuleStatus::Present), ("ListOps".into(), ModuleStatus::Absent)]));

    // line endings do not count
    let file = root.join(TARGET).join("ItemOps/index.ts");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, text.replace('\n', "\r\n")).unwrap();
    assert_eq!(status(&root).1[0].1, ModuleStatus::Present);

    // a changed file: modified, and it is listed, and an import keeps it and says so
    fs::write(root.join(TARGET).join("ItemOps/Get.ts"), "// mine\n").unwrap();
    let state = inspect_import(&found(&root), &source, &manifest).unwrap();
    assert_eq!(state.modules[0].state, ModuleStatus::Modified);
    assert_eq!(state.modules[0].differs, [format!("{TARGET}/ItemOps/Get.ts")]);
    let mut again = ImportOptions::new();
    again.modules = vec!["ItemOps".into()];
    let plan = plan_import(&found(&root), &source, &manifest, again).unwrap();
    assert_eq!(plan.add.kept, [format!("{TARGET}/ItemOps/Get.ts")]);
    assert!(plan.add.warnings.iter().any(|w| w.contains("left as they are") && w.contains("ItemOps/Get.ts")));
    assert!(plan.add.is_empty(), "the file is kept, so there is nothing to write");

    // a missing file: partial
    fs::remove_file(root.join(TARGET).join("ctx.ts")).unwrap();
    let s = inspect_import(&found(&root), &source, &manifest).unwrap();
    assert_eq!(s.core.state, ModuleStatus::Partial);
    assert_eq!(s.core.missing, [format!("{TARGET}/ctx.ts")]);
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_module_the_package_does_not_have_is_refused_and_so_are_a_bad_folder_name_and_a_folder_that_differs_only_by_case() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("refused");
    let root = solution_in(&repo, &scratch);
    set_dep(&root, "@pnp/sp", Some("^4.4.0"));
    let (manifest, source) = package(&scratch);
    let before = snapshot(&root);

    let mut options = ImportOptions::new();
    options.modules = vec!["NopeOps".into()];
    match plan_import(&found(&root), &source, &manifest, options).unwrap_err() {
        ScaffoldError::Validation(issues) => {
            assert_eq!(issues[0].field, "modules");
            assert!(issues[0].message.contains("NopeOps") && issues[0].message.contains("ItemOps, ListOps"), "{}", issues[0].message);
        }
        other => panic!("{other}"),
    }
    let mut options = ImportOptions::new();
    options.slug = Some("../escape".into());
    assert!(matches!(plan_import(&found(&root), &source, &manifest, options).unwrap_err(), ScaffoldError::Validation(_)));

    // the folder name can be chosen
    let mut options = ImportOptions::new();
    options.slug = Some("acme".into());
    let plan = plan_import(&found(&root), &source, &manifest, options).unwrap();
    assert!(plan.add.files.iter().any(|f| f.path == "src/services/acme/ctx.ts"));

    // another way to write the same folder is the same folder
    fs::create_dir_all(root.join("src/services/Acme-Data-Services")).unwrap();
    let err = plan_import(&found(&root), &source, &manifest, ImportOptions::new());
    if cfg!(any(windows, target_os = "macos")) {
        assert!(err.unwrap_err().to_string().contains("only by letter case"));
    }
    fs::remove_dir_all(root.join("src/services")).ok();
    assert_eq!(snapshot(&root), before);
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_library_is_only_its_needs_and_a_package_without_an_import_section_is_not_one() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("library");
    let root = solution_in(&repo, &scratch);

    // a recipe: no files, one dependency the solution lacks
    let lib = scratch.join("axios-recipe");
    fs::create_dir_all(&lib).unwrap();
    fs::write(
        lib.join("manifest.json"),
        r#"{ "id": "axios-recipe", "version": "1.0.0", "prompts": [], "import": { "kind": "library", "notes": ["Create one instance and pass it to your services."] } }"#,
    )
    .unwrap();
    fs::write(lib.join("package.json"), r#"{ "dependencies": { "axios": "^1.7.0" } }"#).unwrap();
    let manifest = Manifest::from_json(&fs::read_to_string(lib.join("manifest.json")).unwrap()).unwrap();
    let plan = plan_import(&found(&root), &DirSource::new(&lib), &manifest, ImportOptions::new()).unwrap();
    assert_eq!(plan.add.files.len(), 1);
    assert_eq!(plan.add.files[0].path, "package.json");
    assert_eq!(plan.dependencies, [DependencyOutcome::Add { name: "axios".into(), range: "^1.7.0".into() }]);
    assert!(plan.notes.iter().any(|n| n.contains("Create one instance")));
    let state = inspect_import(&found(&root), &DirSource::new(&lib), &manifest).unwrap();
    assert!(state.modules.is_empty() && state.core.state == ModuleStatus::Absent && state.core.present.is_empty());

    // a library with files is a mistake in the manifest
    let broken = r#"{ "id": "x", "version": "1.0.0", "prompts": [], "import": { "kind": "library", "core": ["a.ts"] } }"#;
    assert!(Manifest::from_json(broken).unwrap_err().to_string().contains("library copies no files"));

    // a template is not a package
    let (manifest, source) = (
        Manifest::from_json(&fs::read_to_string(repo.join("templates/spfxv.1.21.1/manifest.json")).unwrap()).unwrap(),
        DirSource::new(repo.join("templates/spfxv.1.21.1")),
    );
    assert!(matches!(plan_import(&found(&root), &source, &manifest, ImportOptions::new()), Err(ScaffoldError::Incompatible(_))));
    let _ = fs::remove_dir_all(&scratch);
}
