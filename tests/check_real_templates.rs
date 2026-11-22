//! `check_template` over every template we ship: the 5 `yo`-derived generator folders (the templates repo next to this
//! one), the 20 official SPFx templates (a clone of the SPFx repo as `../spfx` or `FORGE_UPSTREAM_SPFX`) and the 16
//! create-vite templates (a clone of the Vite repo as `../vite` or `FORGE_UPSTREAM_VITE`). A part that is not there is
//! skipped. None of them may have a problem.

use forge_scaffold::CREATE_VITE_DIR;
use forge_scaffold::{check_template, check_with_manifest, CheckOptions, DirSource, TemplateSource, UpstreamSource, ViteSource};
use std::fs;
use std::path::PathBuf;

fn env_or(var: &str, relative: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative))
}

#[test]
fn the_yo_derived_generator_folders_check_clean() {
    let root = env_or("FORGE_TEMPLATES_REPO", "../Forge.Scaffold.Templates/templates");
    if !root.is_dir() {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the templates repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("templates repo not found, skipping");
        return;
    }
    let mut checked = 0;
    for entry in fs::read_dir(&root).unwrap().filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.join("manifest.json").is_file() {
            continue;
        }
        let report = check_template(&DirSource::new(&dir), &CheckOptions::default());
        assert!(report.ok(), "{}:\n{report}", dir.display());
        eprintln!("{}", report.to_string().lines().next().unwrap_or(""));
        checked += 1;
    }
    assert!(checked >= 5, "checked only {checked}");
}

#[test]
fn the_official_spfx_templates_check_clean() {
    let repo = env_or("FORGE_UPSTREAM_SPFX", "../spfx");
    if !repo.join("templates").is_dir() {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the SPFx repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("SPFx repo not found, skipping");
        return;
    }
    let mut checked = 0;
    for entry in fs::read_dir(repo.join("templates")).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().join("template.json").is_file() {
            continue;
        }
        let (def, source) = UpstreamSource::load(&DirSource::new(&repo), &format!("templates/{name}")).unwrap();
        let report = check_with_manifest(&def.manifest_json(), &source, &CheckOptions::default());
        assert!(report.ok(), "{name}:\n{report}");
        checked += 1;
    }
    assert!(checked >= 20, "checked only {checked}");
}

#[test]
fn the_create_vite_templates_check_clean() {
    let repo = env_or("FORGE_UPSTREAM_VITE", "../vite");
    let cv = repo.join(CREATE_VITE_DIR);
    if !cv.join("package.json").is_file() {
        assert!(
            std::env::var_os("FORGE_REQUIRE_UPSTREAM").is_none(),
            "the Vite repo is missing, and FORGE_REQUIRE_UPSTREAM is set: this suite must run here, not skip"
        );
        eprintln!("Vite repo not found, skipping");
        return;
    }
    let mut checked = 0;
    for entry in fs::read_dir(&cv).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(template) = name.strip_prefix("template-") else { continue };
        let (def, source) = ViteSource::load(&DirSource::new(&repo), CREATE_VITE_DIR, template).unwrap();
        let report = check_with_manifest(&def.manifest_json(), &source, &CheckOptions::default());
        assert!(report.ok(), "{template}:\n{report}");
        // every Vite template has the same two prompts and nothing branches, so there is one variant per bool value
        assert!(report.variants.iter().all(|v| v.error.is_none()));
        // the source has to list its own files for the checker to see them
        assert!(source.list_files("template").is_ok());
        checked += 1;
    }
    assert!(checked >= 10, "checked only {checked}");
}
