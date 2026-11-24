//! The 1.24 web part templates put a web part in `src/webparts/<name>/`, not in Microsoft's `src/webparts/<name>WebPart/`: the
//! Forge overlay (`layers/overlay.json` in the templates repo) has one `rewrite` rule for it. This checks, on the three real
//! templates, that the folder is renamed and that **everything that names it follows**: the entry point, the manifest and the
//! strings of `config/config.json` all point at files that exist. Also: adding the same name twice is refused, and a solution
//! that still has a folder from the old layout takes a new web part beside it.
//!
//! The solutions are not built here (that needs `npm install`); the real build is in `docs/upstream-spfx-findings.md`.
//!
//! Needs a clone of the SPFx repo (`../spfx` or `FORGE_UPSTREAM_SPFX`) and the templates repo (`../Forge.Scaffold.Templates` or
//! `FORGE_TEMPLATES_REPO`); skipped otherwise.

use forge_scaffold::{
    apply, apply_add, detect_spfx, plan, plan_add, AddOptions, AddPlan, DirSource, Overlay, PlanOptions, ScaffoldError,
    SequentialGuids, SpfxSolution, UpstreamInputs, UpstreamSource,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn repos() -> Option<(PathBuf, PathBuf)> {
    let spfx = std::env::var_os("FORGE_UPSTREAM_SPFX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spfx"));
    let templates = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    (spfx.join("templates").is_dir() && templates.join("layers/overlay.json").is_file()).then_some((spfx, templates.join("layers")))
}

fn skip_or_fail() {
    assert!(
        std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
        "the SPFx clone or the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
    );
    eprintln!("the SPFx clone or the templates repo was not found, skipping");
}

/// The template as it is released (overlay and rewrite applied), or as Microsoft has it (`released = false`).
fn template(spfx: &Path, layers: &Path, name: &str, released: bool) -> (forge_scaffold::UpstreamTemplate, UpstreamSource) {
    let (def, mut source) = UpstreamSource::load(&DirSource::new(spfx), &format!("templates/{name}")).unwrap();
    if released {
        let overlay = Overlay::load(&DirSource::new(layers), "").unwrap();
        source.apply_overlay(&def, &overlay);
        source.apply_rewrites(&def, &overlay).unwrap();
    }
    (def, source)
}

fn scaffold(spfx: &Path, layers: &Path, name: &str, released: bool, component: &str, scratch: &Path) -> PathBuf {
    let (def, source) = template(spfx, layers, name, released);
    fs::create_dir_all(scratch).unwrap();
    let mut options = PlanOptions::new(scratch);
    options.guids = Box::new(SequentialGuids::new());
    let answers = UpstreamInputs { component_name: component.into(), ..Default::default() }.answers();
    let planned = plan(&def.manifest().unwrap(), &source, &answers, options).unwrap();
    apply(&planned, |_| {}).unwrap();
    planned.target
}

fn plan_for(spfx: &Path, layers: &Path, name: &str, solution: &SpfxSolution, component: &str) -> Result<AddPlan, ScaffoldError> {
    let (def, source) = template(spfx, layers, name, true);
    let answers = UpstreamInputs { component_name: component.into(), ..Default::default() }.add_answers();
    plan_add(solution, &def.manifest().unwrap(), &source, &answers, AddOptions::new())
}

/// Every path `config/config.json` names must be a file of the solution: the manifest, the entry point (its source is the
/// same path in `src`, as `.ts`), and the strings (`loc/en-us.js`). What a build needs to find.
fn assert_config_points_at_files(root: &Path) {
    let config: Value = serde_json::from_str(&fs::read_to_string(root.join("config/config.json")).unwrap()).unwrap();
    let mut checked = 0;
    for (bundle, entry) in config["bundles"].as_object().unwrap() {
        for component in entry["components"].as_array().unwrap() {
            let manifest = component["manifest"].as_str().unwrap();
            assert!(root.join(manifest).is_file(), "{bundle}: the manifest {manifest} is not there");
            let entrypoint = entry["components"][0]["entrypoint"].as_str().unwrap();
            let source = entrypoint.replacen("./lib/", "./src/", 1).replace(".js", ".ts");
            assert!(root.join(&source).is_file(), "{bundle}: the entry point {entrypoint} has no source at {source}");
            checked += 1;
        }
    }
    for (name, path) in config["localizedResources"].as_object().unwrap() {
        let source = path.as_str().unwrap().replacen("lib/", "src/", 1).replace("{locale}", "en-us");
        assert!(root.join(&source).is_file(), "{name}: the strings {path} have no source at {source}");
    }
    assert!(checked > 0);
}

const TEMPLATES: [&str; 3] = ["webpart-react", "webpart-noframework", "webpart-minimal"];

#[test]
fn a_new_solution_from_each_web_part_template_has_the_folder_without_the_suffix_and_a_config_that_points_at_it() {
    let Some((spfx, layers)) = repos() else { return skip_or_fail() };
    for name in TEMPLATES {
        let scratch = std::env::temp_dir().join(format!("forge-webpart-folder-new-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        let root = scaffold(&spfx, &layers, name, true, "Request Form", &scratch);
        assert!(root.join("src/webparts/requestForm/RequestFormWebPart.ts").is_file(), "{name}");
        assert!(root.join("src/webparts/requestForm/RequestFormWebPart.manifest.json").is_file(), "{name}");
        assert!(root.join("src/webparts/requestForm/loc/requestFormWebPartStrings.d.ts").is_file(), "{name}: the strings typings keep their name");
        assert!(!root.join("src/webparts/requestFormWebPart").exists(), "{name}: no folder with the suffix");
        assert_config_points_at_files(&root);
        // the file name and the module name of the strings are untouched
        let source = fs::read_to_string(root.join("src/webparts/requestForm/RequestFormWebPart.ts")).unwrap();
        if name != "webpart-minimal" {
            assert!(source.contains("RequestFormWebPartStrings"), "{name}");
        }
        let _ = fs::remove_dir_all(&scratch);
    }
}

#[test]
fn three_web_parts_added_to_a_solution_get_their_own_folders_and_a_name_twice_is_refused() {
    let Some((spfx, layers)) = repos() else { return skip_or_fail() };
    for name in TEMPLATES {
        let scratch = std::env::temp_dir().join(format!("forge-webpart-folder-add-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        let root = scaffold(&spfx, &layers, name, true, "Hello World", &scratch);

        for (typed, folder, main) in [
            ("test", "test", "TestWebPart.ts"),
            ("Request Form", "requestForm", "RequestFormWebPart.ts"),
            ("approval-panel", "approvalPanel", "ApprovalPanelWebPart.ts"),
        ] {
            let found = detect_spfx(&root).unwrap().unwrap();
            let add = plan_for(&spfx, &layers, name, &found, typed).unwrap();
            let in_folder = format!("src/webparts/{folder}/");
            assert!(
                add.files.iter().any(|f| f.path == format!("{in_folder}{main}")),
                "{name}: {typed}: {:?}",
                add.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            );
            assert!(add.files.iter().all(|f| !f.path.contains("WebPart/")), "{name}: {typed}: a folder with the suffix");
            apply_add(&add, |_| {}).unwrap();
            assert!(root.join(&in_folder).join(main).is_file());
            assert_config_points_at_files(&root);
        }

        // the same name again, however it is typed, is the same web part
        for again in ["test", "Test", "Request Form", "request-form", "approval-panel"] {
            let found = detect_spfx(&root).unwrap().unwrap();
            assert!(
                matches!(plan_for(&spfx, &layers, name, &found, again), Err(ScaffoldError::ComponentExists { .. })),
                "{name}: '{again}' must be refused"
            );
        }
        // and the first one, made with the suffix by the original layout, is still there beside them
        assert!(root.join("src/webparts/helloWorld/HelloWorldWebPart.ts").is_file());
        let _ = fs::remove_dir_all(&scratch);
    }
}

#[test]
fn a_solution_with_folders_from_the_old_layout_takes_a_new_web_part_beside_them() {
    let Some((spfx, layers)) = repos() else { return skip_or_fail() };
    for name in TEMPLATES {
        let scratch = std::env::temp_dir().join(format!("forge-webpart-folder-old-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        // made with Microsoft's own layout: src/webparts/helloWorldWebPart/
        let root = scaffold(&spfx, &layers, name, false, "Hello World", &scratch);
        assert!(root.join("src/webparts/helloWorldWebPart/HelloWorldWebPart.ts").is_file(), "{name}");
        assert_config_points_at_files(&root);

        let found = detect_spfx(&root).unwrap().unwrap();
        let add = plan_for(&spfx, &layers, name, &found, "Other Part").unwrap();
        apply_add(&add, |_| {}).unwrap();
        assert!(root.join("src/webparts/otherPart/OtherPartWebPart.ts").is_file(), "{name}");
        assert!(root.join("src/webparts/helloWorldWebPart/HelloWorldWebPart.ts").is_file(), "{name}: the old folder stays");
        assert_config_points_at_files(&root); // both layouts, one config.json

        // the old web part's own name is still known to the add: it is not added again, in either layout
        let again = detect_spfx(&root).unwrap().unwrap();
        assert!(
            matches!(plan_for(&spfx, &layers, name, &again, "Hello World"), Err(ScaffoldError::ComponentExists { .. })),
            "{name}: Hello World is already there (as helloWorldWebPart)"
        );
        let _ = fs::remove_dir_all(&scratch);
    }
}
