//! Builds a release from the real SPFx generator folders and installs it
//! through `TemplateStore`, the way the panel would: check the index, download,
//! verify, extract. The "server" is a map in memory. Skipped when the templates
//! repo is not next to this one (override with `FORGE_TEMPLATES_REPO`).

use forge_scaffold::{
    build_release, Fetch, FetchResult, ReleaseOptions, Result, ScaffoldError, TemplateStore,
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

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    files_below(dir, &mut files);
    let mut out: Vec<_> = files
        .into_iter()
        .map(|f| {
            let rel = f.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
            (rel, fs::read(&f).unwrap())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_built_release_installs_through_the_store() {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    let templates = repo.join("templates");
    if !templates.is_dir() {
        eprintln!("templates repo not found, skipping");
        return;
    }

    let mut folders: Vec<PathBuf> = fs::read_dir(&templates)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("spfxv."))
        .collect();
    folders.sort();
    assert!(!folders.is_empty());

    // Keep newlines as they are, so what is installed must match the folder byte for byte.
    let mut options = ReleaseOptions::new(BASE);
    options.normalize_newlines = false;
    let release = build_release(&folders, &options, None).unwrap();

    let mut served = BTreeMap::new();
    served.insert(format!("{BASE}/index.json"), release.index_json().unwrap().into_bytes());
    for a in &release.archives {
        served.insert(format!("{BASE}/{}", a.file_name), a.bytes.clone());
    }
    let server = Server(served);

    let root = std::env::temp_dir().join(format!("forge-release-store-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let store = TemplateStore::open(&root).unwrap();

    store
        .check_updates(&server, &format!("{BASE}/index.json"), 1_000, true, 86_400)
        .unwrap();
    for entry in &release.index.templates {
        store.update(&server, &entry.generator, &entry.version).unwrap();

        let installed = root.join(&entry.dir);
        let source = folders.iter().find(|f| f.file_name().unwrap().to_string_lossy() == entry.dir).unwrap();
        let (a, b) = (snapshot(source), snapshot(&installed));
        assert_eq!(
            a.iter().map(|f| &f.0).collect::<Vec<_>>(),
            b.iter().map(|f| &f.0).collect::<Vec<_>>(),
            "{}: different files",
            entry.dir
        );
        for (x, y) in a.iter().zip(&b) {
            assert!(x.1 == y.1, "{}: {} differs", entry.dir, x.0);
        }

        let resolved = store.resolve(&entry.generator, &entry.version).unwrap();
        assert_eq!(resolved.revision, Some(entry.revision));
    }
    let _ = fs::remove_dir_all(&root);
}
