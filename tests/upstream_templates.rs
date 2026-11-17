//! Scaffolds the templates of the official SharePoint/spfx CLI through the
//! upstream reader. Needs a clone of `SharePoint/spfx` (or a fork) next to this
//! repo as `../spfx`; set `FORGE_UPSTREAM_SPFX` to use another place. Skipped when
//! there is none.

use forge_scaffold::{apply, plan, DirSource, PlanOptions, SequentialGuids, UpstreamInputs, UpstreamSource};
use std::fs;
use std::path::{Path, PathBuf};

fn upstream_repo() -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_UPSTREAM_SPFX")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spfx"));
    repo.join("templates").is_dir().then_some(repo)
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

/// Scaffolds `template` into a scratch folder and returns the new solution folder.
fn scaffold(repo: &Path, template: &str, scratch: &Path) -> Result<PathBuf, String> {
    let (def, source) = UpstreamSource::load(&DirSource::new(repo), &format!("templates/{template}"))
        .map_err(|e| e.to_string())?;
    let location = scratch.join(template);
    fs::create_dir_all(&location).unwrap();

    let inputs = UpstreamInputs { component_name: "Hello World".into(), ..Default::default() };
    let mut options = PlanOptions::new(&location);
    options.guids = Box::new(SequentialGuids::new());
    let planned = plan(&def.manifest().map_err(|e| e.to_string())?, &source, &inputs.answers(), options)
        .map_err(|e| e.to_string())?;
    apply(&planned, |_| {}).map_err(|e| e.to_string())?;
    Ok(location.join("hello-world"))
}

#[test]
fn webpart_react_scaffolds_like_upstream() {
    let Some(repo) = upstream_repo() else {
        eprintln!("SharePoint/spfx clone not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-upstream-test-{}", std::process::id()));
    let out = scaffold(&repo, "webpart-react", &scratch).unwrap();

    let mut files = Vec::new();
    files_below(&out, &mut files);
    for f in &files {
        if let Ok(text) = fs::read_to_string(f) {
            assert!(!text.contains("{__") && !text.contains("<%"), "unrendered tag in {}", f.display());
        }
    }
    let part = out.join("src/webparts/helloWorldWebPart");
    assert!(part.join("HelloWorldWebPart.ts").is_file());
    assert!(part.join("components/HelloWorld.tsx").is_file());
    assert!(part.join("loc/helloWorldWebPartStrings.d.ts").is_file());

    // the GUID in the Teams icon names is the one in the web part manifest
    let manifest = fs::read_to_string(part.join("HelloWorldWebPart.manifest.json")).unwrap();
    let teams: Vec<String> = fs::read_dir(out.join("teams"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(teams.len(), 2);
    for icon in &teams {
        let id = icon.split('_').next().unwrap();
        assert!(manifest.contains(id), "{icon} does not match the manifest id");
    }

    // upstream's repo-only build scripts are gone
    let package = fs::read_to_string(out.join("package.json")).unwrap();
    assert!(!package.contains("_phase:"));
    assert!(package.contains("\"name\": \"hello-world\""));
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn every_upstream_template_translates() {
    let Some(repo) = upstream_repo() else {
        eprintln!("SharePoint/spfx clone not found, skipping");
        return;
    };
    let scratch = std::env::temp_dir().join(format!("forge-upstream-all-{}", std::process::id()));
    let mut failures = Vec::new();
    let mut checked = 0;
    for entry in fs::read_dir(repo.join("templates")).unwrap() {
        let path = entry.unwrap().path();
        if !path.join("template.json").is_file() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match scaffold(&repo, &name, &scratch) {
            Ok(out) => {
                let mut files = Vec::new();
                files_below(&out, &mut files);
                for f in files {
                    if let Ok(text) = fs::read_to_string(&f) {
                        assert!(!text.contains("{__") && !text.contains("<%"), "{name}: unrendered tag in {}", f.display());
                    }
                }
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
        checked += 1;
    }
    let _ = fs::remove_dir_all(&scratch);
    assert!(checked > 0);
    assert!(failures.is_empty(), "templates that do not translate:\n{}", failures.join("\n"));
}
