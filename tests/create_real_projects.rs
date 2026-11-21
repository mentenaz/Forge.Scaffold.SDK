//! The template creator on real projects: scaffold a project from an official template, turn it back into a template
//! with the creator, and use that template to make a project with another name. The creator itself verifies that its
//! draft reproduces the project byte for byte; this also checks that the second project is the first one with the names
//! changed. Needs the SPFx repo (`../spfx` or `FORGE_UPSTREAM_SPFX`) and the Vite repo (`../vite` or `FORGE_UPSTREAM_VITE`);
//! a part that is not there is skipped.

use forge_scaffold::create::{create_template, CreateOptions, Replacement};
use forge_scaffold::create_vite::CREATE_VITE_DIR;
use forge_scaffold::manifest::Manifest;
use forge_scaffold::plan::{apply, plan, Answers, PlanOptions};
use forge_scaffold::{DirSource, UpstreamInputs, UpstreamSource, ViteSource};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn env_or(var: &str, relative: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative))
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

fn scratch(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("forge-create-real-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn a_vite_project_becomes_a_template_and_makes_the_same_project_under_another_name() {
    let repo = env_or("FORGE_UPSTREAM_VITE", "../vite");
    if !repo.join(CREATE_VITE_DIR).join("package.json").is_file() {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the Vite repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("Vite repo not found, skipping");
        return;
    }
    let work = scratch("vite");

    // a project from the engine
    let (def, source) = ViteSource::load(&DirSource::new(&repo), CREATE_VITE_DIR, "react-ts").unwrap();
    let answers: Answers = serde_json::from_value(json!({ "projectName": "demo-app" })).unwrap();
    let p = plan(&def.manifest().unwrap(), &source, &answers, PlanOptions::new(&work)).unwrap();
    apply(&p, |_| {}).unwrap();
    let original = work.join("demo-app");

    // back into a template (it verifies itself), then a new project from it
    let draft = create_template(&original, &CreateOptions::new("my-react", "1.0.0", vec![Replacement::new("projectName", "demo-app")])).unwrap();
    assert!(draft.skipped.is_empty(), "{:?}", draft.skipped);
    let out = work.join("gen");
    draft.write_to(&out).unwrap();

    let manifest = Manifest::from_json(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let answers: Answers = serde_json::from_value(json!({ "projectName": "other-app" })).unwrap();
    let p = plan(&manifest, &DirSource::new(&out), &answers, PlanOptions::new(&work)).unwrap();
    apply(&p, |_| {}).unwrap();

    // the same files; the contents are the first project's with the name changed
    let a = snapshot(&original);
    let b = snapshot(&work.join("other-app"));
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    for (path, bytes) in &a {
        let expected = match String::from_utf8(bytes.clone()) {
            Ok(text) => text.replace("demo-app", "other-app").into_bytes(),
            Err(_) => bytes.clone(),
        };
        assert_eq!(b[path], expected, "{path}");
    }
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn an_spfx_web_part_project_becomes_a_template_with_its_names_in_every_form() {
    let repo = env_or("FORGE_UPSTREAM_SPFX", "../spfx");
    if !repo.join("templates/webpart-react").is_dir() {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the SPFx repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("SPFx repo not found, skipping");
        return;
    }
    let work = scratch("spfx");

    let (def, source) = UpstreamSource::load(&DirSource::new(&repo), "templates/webpart-react").unwrap();
    let inputs = UpstreamInputs { component_name: "Hello World".into(), library_name: Some("hello-lib".into()), ..Default::default() };
    let p = plan(&def.manifest().unwrap(), &source, &inputs.answers(), PlanOptions::new(&work)).unwrap();
    apply(&p, |_| {}).unwrap();
    let original = work.join("hello-world");
    assert!(original.is_dir(), "{:?}", fs::read_dir(&work).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());

    let draft = create_template(
        &original,
        &CreateOptions::new(
            "my-webpart",
            "1.0.0",
            vec![Replacement::new("projectName", "hello-world"), Replacement::new("partName", "Hello World")],
        ),
    )
    .unwrap();

    // the names became tokens in contents and in paths, and nothing was lost
    assert!(draft.counts.iter().all(|c| c.occurrences > 0), "{:?}", draft.counts);
    let names: Vec<String> = draft.paths();
    // "hello-world" and "Hello World" share their forms, so the first named (projectName) holds them in the paths
    assert!(names.iter().any(|p| p.contains("{__projectNameCamel__}WebPart") && p.contains("{__projectNamePascal__}")), "{names:?}");
    assert!(draft.notes.iter().any(|n| n.contains("a form of both")), "{:?}", draft.notes);
    assert!(draft.skipped.iter().all(|s| !s.path.starts_with("src")), "{:?}", draft.skipped);

    // a project from the template under other names: the files exist under the new names
    let out = work.join("gen");
    draft.write_to(&out).unwrap();
    let manifest = Manifest::from_json(&fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    let answers: Answers = serde_json::from_value(json!({ "projectName": "news-feed", "partName": "News Feed" })).unwrap();
    let p = plan(&manifest, &DirSource::new(&out), &answers, PlanOptions::new(&work)).unwrap();
    apply(&p, |_| {}).unwrap();
    let made = snapshot(&work.join("news-feed"));
    assert!(made.keys().any(|k| k.contains("NewsFeed")), "{:?}", made.keys().take(20).collect::<Vec<_>>());
    assert!(!made.keys().any(|k| k.contains("HelloWorld") || k.contains("hello-world")), "an old name was left in a path");
    let all_text: String = made.values().filter_map(|b| String::from_utf8(b.clone()).ok()).collect::<Vec<_>>().join("\n");
    assert!(!all_text.contains("HelloWorld") && !all_text.contains("hello-world"), "an old name was left in a file");
    let _ = fs::remove_dir_all(&work);
}
