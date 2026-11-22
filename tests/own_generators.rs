//! Scaffolds the generators written for the templates repo (`backend-api` and `fullstack`) and checks what the answers
//! change: CORS and a Dockerfile in the API, the port everywhere it is used, and the pieces that must never be there
//! when they were not asked for. Needs the templates repo next to this one (`../Forge.Scaffold.Templates`, or
//! `FORGE_TEMPLATES_REPO`); skipped otherwise.

use forge_scaffold::{apply, plan, Answers, DirSource, Manifest, PlanOptions, SequentialGuids};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn generator(folder: &str) -> Option<PathBuf> {
    let repo = std::env::var_os("FORGE_TEMPLATES_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Forge.Scaffold.Templates"));
    let dir = repo.join("templates").join(folder);
    dir.join("manifest.json").is_file().then_some(dir)
}

/// Scaffolds the generator with `answers` into a fresh folder and returns the project's root.
fn scaffold(gen: &Path, answers: Value, scratch: &Path) -> PathBuf {
    let manifest = Manifest::from_json(&fs::read_to_string(gen.join("manifest.json")).unwrap()).unwrap();
    let answers: Answers = serde_json::from_value(answers).unwrap();
    fs::create_dir_all(scratch).unwrap();
    let mut options = PlanOptions::new(scratch);
    options.guids = Box::new(SequentialGuids::new());
    let planned = plan(&manifest, &DirSource::new(gen), &answers, options).unwrap();
    let target = planned.target.clone();
    apply(&planned, |_| {}).unwrap();
    target
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn scratch(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("forge-own-{tag}-{}-{}", std::process::id(), uuid_like()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn uuid_like() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

#[test]
fn backend_api_adds_cors_and_docker_only_when_asked() {
    let Some(gen) = generator("backend-api.1.0.0") else {
        eprintln!("templates repo not found, skipping");
        return;
    };
    let s = scratch("api");

    // defaults: CORS on, no Docker
    let a = scaffold(&gen, json!({ "projectName": "demo-api" }), &s.join("a"));
    let server = read(&a, "src/server.ts");
    assert!(server.contains("import cors from '@fastify/cors'") && server.contains("await app.register(cors"));
    assert!(server.starts_with("import Fastify from 'fastify'\nimport cors"), "imports stay together at the top: {server}");
    assert!(read(&a, "package.json").contains("\"@fastify/cors\""));
    assert!(!a.join("Dockerfile").exists() && !a.join(".dockerignore").exists());
    assert!(a.join(".gitignore").is_file(), "_gitignore is written back under its real name");
    assert!(read(&a, "src/index.ts").contains("process.env.PORT ?? 3000"));
    assert!(read(&a, "README.md").contains("demo-api") && read(&a, "README.md").contains("A Fastify API"));
    let pkg: Value = serde_json::from_str(&read(&a, "package.json")).unwrap();
    assert_eq!(pkg["name"], "demo-api");

    // no CORS, a Dockerfile, another port
    let b = scaffold(&gen, json!({ "projectName": "demo-api", "cors": false, "docker": true, "port": "4000" }), &s.join("b"));
    let server = read(&b, "src/server.ts");
    assert!(!server.contains("cors"), "nothing of CORS is left: {server}");
    assert!(!read(&b, "package.json").contains("cors"));
    assert!(read(&b, "Dockerfile").contains("EXPOSE 4000") && read(&b, "Dockerfile").contains("PORT=4000"));
    assert!(b.join(".dockerignore").is_file());
    assert!(read(&b, "src/index.ts").contains("process.env.PORT ?? 4000"));
    assert!(read(&b, "src/server.ts").contains("app.get('/health'"));
    let _ = fs::remove_dir_all(&s);
}

#[test]
fn fullstack_wires_the_client_to_the_server_on_the_chosen_port() {
    let Some(gen) = generator("fullstack.1.0.0") else {
        eprintln!("templates repo not found, skipping");
        return;
    };
    let s = scratch("full");
    let root = scaffold(&gen, json!({ "projectName": "demo-app", "port": "3001" }), &s);

    let pkg: Value = serde_json::from_str(&read(&root, "package.json")).unwrap();
    assert_eq!(pkg["workspaces"], json!(["client", "server"]));
    assert_eq!(serde_json::from_str::<Value>(&read(&root, "server/package.json")).unwrap()["name"], "demo-app-server");
    assert_eq!(serde_json::from_str::<Value>(&read(&root, "client/package.json")).unwrap()["name"], "demo-app-client");
    // the proxy, the server and the README all use the one port
    assert!(read(&root, "client/vite.config.ts").contains("http://localhost:3001"));
    assert!(read(&root, "server/src/index.ts").contains("process.env.PORT ?? 3001"));
    assert!(read(&root, "README.md").contains("port 3001"));
    // the client calls the route the server serves
    assert!(read(&root, "client/src/App.tsx").contains("fetch('/api/health')"));
    assert!(read(&root, "server/src/server.ts").contains("'/api/health'"));
    assert!(read(&root, "client/index.html").contains("<title>demo-app</title>"));
    assert!(read(&root, "client/src/App.tsx").contains("<h1>demo-app</h1>"));
    assert!(root.join(".gitignore").is_file());
    let _ = fs::remove_dir_all(&s);
}
