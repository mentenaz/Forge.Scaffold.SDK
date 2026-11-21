//! Scaffolds every template of create-vite from a real checkout of the Vite repo (your fork) and checks the
//! project is the template folder with exactly the changes create-vite makes: `_gitignore` and `_oxlintrc.json`
//! written under their real names, the name in `package.json` and the page title. Needs a clone of the Vite
//! repo (`../vite` or `FORGE_UPSTREAM_VITE`); skipped otherwise.

use forge_scaffold::{apply, plan, DirSource, PlanOptions, ViteSource, CREATE_VITE_DIR};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn vite_repo() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_UPSTREAM_VITE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vite"));
    repo.join(CREATE_VITE_DIR).join("package.json").is_file().then_some(repo)
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
fn every_create_vite_template_scaffolds_as_the_template_with_the_changes_create_vite_makes() {
    let Some(repo) = vite_repo() else {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the Vite repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("Vite repo not found (../vite or FORGE_UPSTREAM_VITE), skipping");
        return;
    };
    let cv = repo.join(CREATE_VITE_DIR);
    let scratch = std::env::temp_dir().join(format!("forge-vite-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();

    let mut names: Vec<String> = fs::read_dir(&cv)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("template-")).map(str::to_string))
        .collect();
    names.sort();
    assert!(names.len() >= 10, "found only {names:?}");

    for name in &names {
        let (def, source) = ViteSource::load(&DirSource::new(&repo), CREATE_VITE_DIR, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let location = scratch.join(name);
        fs::create_dir_all(&location).unwrap();
        let answers = [("projectName".to_string(), serde_json::json!("my-app"))].into_iter().collect();
        let planned = plan(&def.manifest().unwrap(), &source, &answers, PlanOptions::new(&location)).unwrap_or_else(|e| panic!("{name}: {e}"));
        apply(&planned, |_| {}).unwrap();

        let made = snapshot(&location.join("my-app"));
        let mut expected = snapshot(&cv.join(format!("template-{name}")));
        for (from, to) in [("_gitignore", ".gitignore"), ("_oxlintrc.json", ".oxlintrc.json")] {
            if let Some(bytes) = expected.remove(from) {
                expected.insert(to.to_string(), bytes);
            }
        }
        // the two changes to file contents
        let package: Value = serde_json::from_slice(&expected["package.json"]).unwrap();
        let old_name = package["name"].as_str().unwrap().to_string();
        let package_text = String::from_utf8(expected["package.json"].clone()).unwrap();
        expected.insert("package.json".into(), package_text.replacen(&format!("\"name\": \"{old_name}\""), "\"name\": \"my-app\"", 1).into_bytes());
        let html = String::from_utf8(expected["index.html"].clone()).unwrap();
        let (start, end) = (html.find("<title>").unwrap() + 7, html.find("</title>").unwrap());
        expected.insert("index.html".into(), format!("{}my-app{}", &html[..start], &html[end..]).into_bytes());

        assert_eq!(made.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>(), "{name}: the same files");
        for (path, bytes) in &expected {
            assert_eq!(&made[path], bytes, "{name}: {path}");
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    eprintln!("checked {} templates: {names:?}", names.len());
}
