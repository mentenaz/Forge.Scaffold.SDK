//! The worked example of `docs/authoring-templates.md` (`tests/fixtures/authoring-example/`): it checks clean, renders what
//! the guide says it renders, and the guide quotes its manifest exactly, so the guide cannot drift from what the engine takes.

use forge_scaffold::manifest::Manifest;
use forge_scaffold::plan::{plan, Answers, PlanOptions};
use forge_scaffold::{check_template, CheckOptions, DirSource};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/authoring-example")
}

fn answers(v: Value) -> Answers {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_example_checks_clean_in_every_branch() {
    let report = check_template(&DirSource::new(fixture()), &CheckOptions::default());
    assert!(report.ok(), "{report}");
    let labels: Vec<&str> = report.variants.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(labels, ["defaults", "language = js", "install = true"]);
    // the sample answers use one page ("Sample"): package.json, site.json, .env.example, _gitignore, one config, one page
    assert!(report.variants.iter().all(|v| v.files == 6), "{:?}", report.variants);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

#[test]
fn the_example_makes_what_the_guide_says() {
    let source = DirSource::new(fixture());
    let manifest = Manifest::from_json(&fs::read_to_string(fixture().join("manifest.json")).unwrap()).unwrap();
    let loc = std::env::temp_dir().join(format!("forge-authoring-{}", std::process::id()));
    fs::create_dir_all(&loc).unwrap();
    let a = answers(json!({ "projectName": "my-site", "language": "js", "pages": ["About Us", "Contact"], "install": true }));
    let p = plan(&manifest, &source, &a, PlanOptions::new(&loc)).unwrap();

    let paths: Vec<&str> = p.files.paths().collect();
    assert_eq!(paths, [".env.example", ".gitignore", "jsconfig.json", "package.json", "pages/about-us.html", "pages/contact.html", "site.json"]);
    let text = |path: &str| String::from_utf8(p.files.get(path).unwrap().to_vec()).unwrap();

    assert!(text("package.json").contains("\"name\": \"my-site\""));
    assert!(text("pages/about-us.html").contains("<h1>About Us</h1>") && text("pages/about-us.html").contains("Page AboutUs of my-site."));
    // the merge appended one entry per page, in order
    let site: Value = serde_json::from_str(&text("site.json")).unwrap();
    assert_eq!(site["title"], "MySite");
    assert_eq!(site["pages"][0], json!({ "name": "About Us", "file": "pages/about-us.html" }));
    assert_eq!(site["pages"][1], json!({ "name": "Contact", "file": "pages/contact.html" }));
    // a generated secret: letters and digits, with an upper case letter, a lower case letter and a digit
    let secret = text(".env.example").trim().strip_prefix("SESSION_SECRET=").unwrap().to_string();
    assert!(secret.len() >= 16 && secret.chars().all(|c| c.is_ascii_alphanumeric()), "{secret}");
    assert!(secret.chars().any(|c| c.is_ascii_uppercase()) && secret.chars().any(|c| c.is_ascii_lowercase()) && secret.chars().any(|c| c.is_ascii_digit()));
    // the install step is handed to the host, with the package manager the template lists first
    assert_eq!(p.post.len(), 1);
    assert_eq!(format!("{:?}", p.post[0].package_manager), "Some(Npm)");
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);

    // the other branch: TypeScript is the default
    let a = answers(json!({ "projectName": "my-site", "pages": [] }));
    let p = plan(&manifest, &source, &a, PlanOptions::new(&loc)).unwrap();
    let paths: Vec<&str> = p.files.paths().collect();
    assert_eq!(paths, [".env.example", ".gitignore", "package.json", "site.json", "tsconfig.json"]);
    let _ = fs::remove_dir_all(&loc);
}

#[test]
fn the_guide_quotes_the_manifest_of_the_example_exactly() {
    let manifest = fs::read_to_string(fixture().join("manifest.json")).unwrap().replace("\r\n", "\n");
    let guide = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/authoring-templates.md")).unwrap().replace("\r\n", "\n");
    assert!(guide.contains(manifest.trim_end()), "docs/authoring-templates.md does not contain tests/fixtures/authoring-example/manifest.json as it is");
}
