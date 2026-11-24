//! Adds spfx-fast-serve to a real SPFx 1.21.1 (gulp) solution made by our own generator, and compares with what the
//! official `spfx-fast-serve` 4.0.2 was measured to change: `spfx-fast-serve-helpers` as a dev dependency at `~1.21.0`,
//! the `serve` script, and the marked block in `gulpfile.js` in front of `build.initialize`. It creates no `fast-serve/`
//! folder: the two files in it are optional.
//!
//! Needs the templates repo next to this one (`../Forge.Scaffold.Templates`, or `FORGE_TEMPLATES_REPO`); skipped otherwise.

use forge_scaffold::{
    apply, apply_add, detect_spfx, plan, plan_add, revert_edit, AddOptions, AddOutcome, Answers, DirSource, Manifest, PackageManager,
    PlanOptions, PostAction, ScaffoldError, SequentialGuids, SpfxSolution, UndoRecord,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    repo.join("templates/spfx-fast-serve.1.0.0/manifest.json").is_file().then_some(repo)
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

/// A solution of the given SPFx generator, made the way a user makes one. Returns its folder.
fn solution(repo: &Path, generator: &str, scratch: &Path) -> PathBuf {
    let dir = repo.join("templates").join(generator);
    let manifest = Manifest::from_json(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!("my-sol"));
    answers.insert("webparts".into(), json!(["HelloWorld"]));
    let mut options = PlanOptions::new(scratch);
    options.guids = Box::new(SequentialGuids::new());
    apply(&plan(&manifest, &DirSource::new(&dir), &answers, options).unwrap(), |_| {}).unwrap();
    scratch.join("my-sol")
}

fn fast_serve(repo: &Path) -> (Manifest, DirSource) {
    let dir = repo.join("templates/spfx-fast-serve.1.0.0");
    (Manifest::from_json(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap(), DirSource::new(&dir))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forge-fast-serve-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn answers(pairs: &[(&str, Value)]) -> Answers {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn text(files: &BTreeMap<String, Vec<u8>>, path: &str) -> String {
    String::from_utf8(files[path].clone()).unwrap()
}

#[test]
fn adding_fast_serve_changes_what_the_official_tool_changes_and_nothing_else() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("default");
    let root = solution(&repo, "spfxv.1.21.1", &scratch);
    let found = detect_spfx(&root).unwrap().unwrap();
    assert_eq!(found.version.as_deref(), Some("1.21.1"));
    let before = snapshot(&root);
    let (manifest, source) = fast_serve(&repo);

    let add = plan_add(&found, &manifest, &source, &Answers::new(), AddOptions::new()).unwrap();
    assert!(add.files.iter().all(|f| f.outcome == AddOutcome::Merged), "no new file by default: {:?}", add.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    let paths: Vec<&str> = add.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["gulpfile.js", "package.json"]);
    assert!(add.warnings.is_empty(), "{:?}", add.warnings);
    assert_eq!(snapshot(&root), before, "planning writes nothing");

    // the plan changes package.json (the helpers are a new dev dependency), so it asks the host to install, in the solution
    assert_eq!(add.post.len(), 1, "{:?}", add.post);
    assert_eq!(add.post[0].action, PostAction::Install);
    assert_eq!(add.post[0].package_manager, Some(PackageManager::Npm));
    assert_eq!(add.post[0].cwd, root);
    // the host's own package manager is used when it says so, and the question can turn the step off
    let mut options = AddOptions::new();
    options.package_manager = Some(PackageManager::Pnpm);
    let pnpm = plan_add(&found, &manifest, &source, &Answers::new(), options).unwrap();
    assert_eq!(pnpm.post[0].package_manager, Some(PackageManager::Pnpm));
    let no_install = plan_add(&found, &manifest, &source, &answers(&[("install", json!(false))]), AddOptions::new()).unwrap();
    assert!(no_install.post.is_empty());

    apply_add(&add, |_| {}).unwrap();
    let after = snapshot(&root);
    let changed: Vec<&String> = after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).map(|(k, _)| k).collect();
    assert_eq!(changed, ["gulpfile.js", "package.json"]);

    // package.json: the script and the helpers at the SPFx minor, as the official tool writes them
    let pkg: Value = serde_json::from_str(&text(&after, "package.json")).unwrap();
    assert_eq!(pkg["scripts"]["serve"], "fast-serve");
    assert_eq!(pkg["devDependencies"]["spfx-fast-serve-helpers"], "~1.21.0");
    let old: Value = serde_json::from_str(&text(&before, "package.json")).unwrap();
    assert_eq!(pkg["scripts"]["build"], old["scripts"]["build"], "the other scripts stay");
    assert_eq!(pkg["dependencies"], old["dependencies"]);

    // gulpfile.js: the block in front of build.initialize, and the rest as it was
    let gulp = text(&after, "gulpfile.js");
    let old_gulp = text(&before, "gulpfile.js");
    assert!(gulp.contains(
        "/* fast-serve */\nconst { addFastServe } = require(\"spfx-fast-serve-helpers\");\naddFastServe(build);\n/* end of fast-serve */\n\nbuild.initialize(require(\"gulp\"));"
    ));
    assert_eq!(gulp.replace("/* fast-serve */\nconst { addFastServe } = require(\"spfx-fast-serve-helpers\");\naddFastServe(build);\n/* end of fast-serve */\n\n", ""), old_gulp);

    // adding again changes nothing
    let again = plan_add(&detect_spfx(&root).unwrap().unwrap(), &manifest, &source, &Answers::new(), AddOptions::new()).unwrap();
    assert!(again.is_empty(), "{:?}", again.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    assert!(again.post.is_empty(), "nothing changed, nothing to install");
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn the_choices_decide_the_script_and_which_files_in_fast_serve_are_made() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("choices");
    let root = solution(&repo, "spfxv.1.21.1", &scratch);
    let found = detect_spfx(&root).unwrap().unwrap();
    let (manifest, source) = fast_serve(&repo);

    // the rebuild-first script, the config file and the stock webpack.extend.js
    let a = answers(&[
        ("serveScript", json!("gulp clean && gulp build && fast-serve")),
        ("configFile", json!(true)),
        ("webpackExtend", json!(true)),
    ]);
    let add = plan_add(&found, &manifest, &source, &a, AddOptions::new()).unwrap();
    let new: Vec<&str> = add.files.iter().filter(|f| f.outcome == AddOutcome::New).map(|f| f.path.as_str()).collect();
    assert_eq!(new, ["fast-serve/config.json", "fast-serve/webpack.extend.js"]);
    let merged_pkg = add.files.iter().find(|f| f.path == "package.json").unwrap();
    let pkg: Value = serde_json::from_slice(&merged_pkg.content).unwrap();
    assert_eq!(pkg["scripts"]["serve"], "gulp clean && gulp build && fast-serve");
    let webpack = String::from_utf8(add.files.iter().find(|f| f.path == "fast-serve/webpack.extend.js").unwrap().content.clone()).unwrap();
    assert!(webpack.contains("const transformConfig"), "the stock stub");
    assert!(!webpack.contains("@pnp"));

    // the @pnp stub brings its own webpack.extend.js and the empty stylesheet, and not the stock one
    let a = answers(&[("pnpStub", json!(true)), ("webpackExtend", json!(true))]);
    let add = plan_add(&found, &manifest, &source, &a, AddOptions::new()).unwrap();
    let new: Vec<&str> = add.files.iter().filter(|f| f.outcome == AddOutcome::New).map(|f| f.path.as_str()).collect();
    assert_eq!(new, ["fast-serve/empty.module.scss", "fast-serve/webpack.extend.js"]);
    let webpack = String::from_utf8(add.files.iter().find(|f| f.path == "fast-serve/webpack.extend.js").unwrap().content.clone()).unwrap();
    assert!(webpack.contains("NormalModuleReplacementPlugin") && webpack.contains("empty.module.scss"));
    assert!(!webpack.contains("alias"), "no project's aliases in a generator");
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_solution_that_has_its_own_serve_script_and_helpers_version_keeps_them_and_is_told() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("kept");
    let root = solution(&repo, "spfxv.1.21.1", &scratch);
    let pkg_path = root.join("package.json");
    let mut pkg: Value = serde_json::from_str(&fs::read_to_string(&pkg_path).unwrap()).unwrap();
    pkg["scripts"]["serve"] = json!("gulp clean && gulp build && fast-serve");
    pkg["devDependencies"]["spfx-fast-serve-helpers"] = json!("~1.20.0");
    fs::write(&pkg_path, serde_json::to_string_pretty(&pkg).unwrap()).unwrap();
    let found = detect_spfx(&root).unwrap().unwrap();
    let (manifest, source) = fast_serve(&repo);

    let add = plan_add(&found, &manifest, &source, &Answers::new(), AddOptions::new()).unwrap();
    assert!(add.files.iter().all(|f| f.path != "package.json"), "package.json is left as it is");
    assert!(add.post.is_empty(), "package.json does not change, so there is nothing to install");
    assert!(add.files.iter().any(|f| f.path == "gulpfile.js"), "the gulpfile still gets its block");
    let w = add.warnings.join("\n");
    assert!(w.contains("'serve'") && w.contains("gulp clean && gulp build && fast-serve"), "{w}");
    assert!(w.contains("spfx-fast-serve-helpers") && w.contains("~1.20.0"), "{w}");
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_solution_the_official_tool_already_patched_has_nothing_to_add() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("official");
    let root = solution(&repo, "spfxv.1.21.1", &scratch);
    // exactly what `spfx-fast-serve` 4.0.2 wrote into this solution: single quotes on the initialize line, no final
    // newline in package.json
    let gulp = fs::read_to_string(root.join("gulpfile.js")).unwrap().replace(
        "build.initialize(require(\"gulp\"));",
        "/* fast-serve */\nconst { addFastServe } = require(\"spfx-fast-serve-helpers\");\naddFastServe(build);\n/* end of fast-serve */\n\nbuild.initialize(require('gulp'));\n",
    );
    fs::write(root.join("gulpfile.js"), gulp).unwrap();
    let mut pkg: Value = serde_json::from_str(&fs::read_to_string(root.join("package.json")).unwrap()).unwrap();
    pkg["scripts"]["serve"] = json!("fast-serve");
    pkg["devDependencies"]["spfx-fast-serve-helpers"] = json!("~1.21.0");
    fs::write(root.join("package.json"), serde_json::to_string_pretty(&pkg).unwrap()).unwrap();

    let found = detect_spfx(&root).unwrap().unwrap();
    let (manifest, source) = fast_serve(&repo);
    let add = plan_add(&found, &manifest, &source, &Answers::new(), AddOptions::new()).unwrap();
    assert!(add.is_empty(), "{:?}", add.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    assert!(add.warnings.is_empty(), "{:?}", add.warnings);
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn the_add_can_be_undone_byte_for_byte() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("undo");
    let root = solution(&repo, "spfxv.1.21.1", &scratch);
    let found = detect_spfx(&root).unwrap().unwrap();
    let before = snapshot(&root);
    let (manifest, source) = fast_serve(&repo);
    let a = answers(&[("configFile", json!(true)), ("pnpStub", json!(true))]);
    let add = plan_add(&found, &manifest, &source, &a, AddOptions::new()).unwrap();
    let record = add.undo_record("Added fast-serve").to_json();
    apply_add(&add, |_| {}).unwrap();
    assert_ne!(snapshot(&root), before);

    let out = revert_edit(&root, &UndoRecord::from_json(&record).unwrap()).unwrap();
    assert_eq!(out.restored, ["gulpfile.js", "package.json"]);
    assert_eq!(snapshot(&root), before, "no file or folder left over");
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn a_heft_solution_is_refused_and_nothing_is_changed() {
    let Some(repo) = repo() else { return skip_or_fail() };
    let scratch = scratch("heft");
    let root = solution(&repo, "spfxv.1.22.2", &scratch);
    let found: SpfxSolution = detect_spfx(&root).unwrap().unwrap();
    assert_eq!(format!("{:?}", found.build_system), "Heft", "1.22 solutions are built with heft");
    let before = snapshot(&root);
    let (manifest, source) = fast_serve(&repo);
    let err = plan_add(&found, &manifest, &source, &Answers::new(), AddOptions::new()).unwrap_err();
    assert!(matches!(err, ScaffoldError::Incompatible(_)), "{err}");
    assert_eq!(snapshot(&root), before);
    let _ = fs::remove_dir_all(&scratch);
}
