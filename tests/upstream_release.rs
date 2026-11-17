//! Releases official SharePoint/spfx templates with the Forge overlay, installs
//! the release through `TemplateStore`, and scaffolds from the installed copy.
//! Needs a clone of the SPFx repo (`../spfx` or `FORGE_UPSTREAM_SPFX`) and the
//! templates repo (`../Forge.Scaffold.Templates` or `FORGE_TEMPLATES_REPO`);
//! skipped when either is missing.

use forge_scaffold::{
    apply, build_release_from, plan, DirSource, Fetch, FetchResult, Manifest, PlanOptions, ReleaseInput,
    ReleaseOptions, Result, ScaffoldError, SequentialGuids, TemplateStore, UpstreamInputs,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const BASE: &str = "https://downloads.example.test/forge-scaffold";

struct Server(BTreeMap<String, Vec<u8>>);

impl Fetch for Server {
    fn get(&self, url: &str, _etag: Option<&str>) -> Result<FetchResult> {
        self.0
            .get(url)
            .map(|bytes| FetchResult::Body { bytes: bytes.clone(), etag: None })
            .ok_or_else(|| ScaffoldError::Fetch { url: url.into(), reason: "404".into() })
    }
}

fn repo(env: &str, sibling: &str, must_have: &str) -> Option<PathBuf> {
    let dir = std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(sibling));
    dir.join(must_have).exists().then_some(dir)
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
fn an_upstream_release_installs_and_scaffolds_with_the_overlay() {
    let (Some(spfx), Some(templates)) = (
        repo("FORGE_UPSTREAM_SPFX", "spfx", "templates"),
        repo("FORGE_TEMPLATES_REPO", "Forge.Scaffold.Templates", "layers/overlay.json"),
    ) else {
        eprintln!("SharePoint/spfx clone or templates repo not found, skipping");
        return;
    };
    let layers = templates.join("layers");

    let inputs: Vec<ReleaseInput> = ["webpart-react", "library"]
        .iter()
        .map(|t| ReleaseInput::Upstream { repo: spfx.clone(), template: t.to_string() })
        .collect();
    let mut options = ReleaseOptions::new(BASE);
    options.overlay = Some(layers.clone());

    let release = build_release_from(&inputs, &options, None).unwrap();
    assert_eq!(release.index.templates.len(), 2);

    // the same inputs give the same release, and the revision stays 1
    let again = build_release_from(&inputs, &options, Some(&release.index)).unwrap();
    assert_eq!(again.index, release.index);

    let mut served = BTreeMap::new();
    served.insert(format!("{BASE}/index.json"), release.index_json().unwrap().into_bytes());
    for a in &release.archives {
        served.insert(format!("{BASE}/{}", a.file_name), a.bytes.clone());
    }
    let server = Server(served);

    let root = std::env::temp_dir().join(format!("forge-upstream-release-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let store = TemplateStore::open(&root).unwrap();
    store.check_updates(&server, &format!("{BASE}/index.json"), 1_000, true, 86_400).unwrap();

    let logo = fs::read(layers.join("assets/common/forge-logo-welcome.png")).unwrap();
    let teams_color = fs::read(layers.join("assets/common/teams-color-192.png")).unwrap();
    let teams_outline = fs::read(layers.join("assets/common/teams-outline-32.png")).unwrap();

    for entry in &release.index.templates {
        assert_eq!(entry.version, "1.24.1");
        store.update(&server, &entry.generator, &entry.version).unwrap();
        assert!(store.resolve(&entry.generator, &entry.version).is_ok());

        // scaffold from the installed copy, the way the panel would
        let installed = root.join(&entry.dir);
        let manifest = Manifest::from_json(&fs::read_to_string(installed.join("manifest.json")).unwrap()).unwrap();
        let location = root.join(format!("out-{}", entry.generator));
        fs::create_dir_all(&location).unwrap();
        let mut plan_options = PlanOptions::new(&location);
        plan_options.guids = Box::new(SequentialGuids::new());
        let answers = UpstreamInputs { component_name: "Hello World".into(), ..Default::default() }.answers();
        let planned = plan(&manifest, &DirSource::new(&installed), &answers, plan_options).unwrap();
        apply(&planned, |_| {}).unwrap();

        let out = location.join("hello-world");
        let mut files = Vec::new();
        files_below(&out, &mut files);
        for f in &files {
            if let Ok(text) = fs::read_to_string(f) {
                assert!(!text.contains("{__") && !text.contains("<%"), "{}: unrendered tag in {}", entry.generator, f.display());
            }
            let name = f.file_name().unwrap().to_string_lossy();
            if name == "welcome-dark.png" || name == "welcome-light.png" {
                assert_eq!(fs::read(f).unwrap(), logo, "{}: {name} is not the Forge logo", entry.generator);
            }
            if name.ends_with("_color.png") && f.parent().unwrap().ends_with("teams") {
                assert_eq!(fs::read(f).unwrap(), teams_color, "{}: Teams colour icon", entry.generator);
            }
            if name.ends_with("_outline.png") && f.parent().unwrap().ends_with("teams") {
                assert_eq!(fs::read(f).unwrap(), teams_outline, "{}: Teams outline icon", entry.generator);
            }
        }
        if entry.generator == "webpart-react" {
            // the overlay really did reach the web part
            assert!(files.iter().any(|f| f.file_name().unwrap() == "welcome-dark.png"));
            assert!(files.iter().any(|f| f.parent().unwrap().ends_with("teams")));
        }
    }
    let _ = fs::remove_dir_all(&root);
}
