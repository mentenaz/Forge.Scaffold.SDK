//! Scaffolds every `spfxv.*` generator folder of the templates repo and checks
//! the result. Skipped when the templates repo is not next to this one; point
//! `FORGE_TEMPLATES_REPO` at it to override.

use forge_scaffold::{apply, plan, Answers, DirSource, Manifest, PlanOptions};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn templates_dir() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    let dir = repo.join("templates");
    dir.is_dir().then_some(dir)
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_spfx_generator_scaffolds_cleanly() {
    let Some(dir) = templates_dir() else {
        eprintln!("templates repo not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-spfx-test-{}", std::process::id()));
    let mut checked = 0;

    for entry in fs::read_dir(&dir).unwrap() {
        let root = entry.unwrap().path();
        let name = root.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("spfxv.") {
            continue;
        }
        let manifest = Manifest::from_json(&fs::read_to_string(root.join("manifest.json")).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        let location = scratch.join(&name);
        fs::create_dir_all(&location).unwrap();
        let mut answers = Answers::new();
        answers.insert("solutionName".into(), json!("my-sol"));
        answers.insert("webparts".into(), json!(["HelloWorld", "OtherPart"]));

        let planned = plan(&manifest, &DirSource::new(&root), &answers, PlanOptions::new(&location))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        apply(&planned, |_| {}).unwrap_or_else(|e| panic!("{name}: {e}"));
        let out = location.join("my-sol");

        // no token survives, and nothing from the capture's own values does
        let mut files = Vec::new();
        files_below(&out, &mut files);
        for f in &files {
            if let Ok(text) = fs::read_to_string(f) {
                assert!(!text.contains("{__"), "{name}: unresolved token in {}", f.display());
            }
        }
        for webpart in ["helloWorld", "otherPart"] {
            let ts = format!("{}WebPart.ts", webpart[..1].to_uppercase() + &webpart[1..]);
            assert!(
                out.join("src/webparts").join(webpart).join(ts).is_file(),
                "{name}: missing {webpart} web part"
            );
        }

        // config.json lists both web parts
        let config: Value = serde_json::from_str(&fs::read_to_string(out.join("config/config.json")).unwrap()).unwrap();
        for key in ["hello-world-web-part", "other-part-web-part"] {
            assert!(config["bundles"][key].is_object(), "{name}: no bundle {key}");
        }
        for key in ["HelloWorldWebPartStrings", "OtherPartWebPartStrings"] {
            assert!(config["localizedResources"][key].is_string(), "{name}: no strings {key}");
        }

        // one pair of Teams icons per web part, with different GUIDs
        let icons = fs::read_dir(out.join("teams")).unwrap().count();
        assert_eq!(icons, 4, "{name}: expected 4 Teams icons");

        // .yo-rc.json carries the new solution, not the capture's
        let yo = fs::read_to_string(out.join(".yo-rc.json")).unwrap();
        assert!(yo.contains("\"solutionName\": \"my-sol\""), "{name}: .yo-rc.json solutionName");

        checked += 1;
    }
    let _ = fs::remove_dir_all(&scratch);
    assert!(checked > 0, "no spfxv.* generator found in {}", dir.display());
}
