//! The `forge-scaffold` program, run as a host would run it: every command, what it prints, and what it does when it fails.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("forge-cli-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    fn s(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the program; returns the exit code and every line of standard output as JSON.
fn run(args: &[&str], stdin: Option<&str>) -> (i32, Vec<Value>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_forge-scaffold"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(text) = stdin {
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output().unwrap();
    let lines = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l:?}: {e}")))
        .collect();
    (out.status.code().unwrap(), lines)
}

fn last(lines: &[Value]) -> &Value {
    lines.last().expect("the program printed nothing")
}

/// A templates folder with one generator, `demo` 1.0.0.
fn templates() -> Tmp {
    let t = Tmp::new();
    t.write(
        "demo.1.0.0/manifest.json",
        r#"{
          "id": "demo", "version": "1.0.0", "folderFrom": "projectName",
          "requires": { "node": ">=20.0.0" },
          "prompts": [
            { "key": "projectName", "type": "text", "label": "Project name", "rule": "package-name" },
            { "key": "install", "type": "bool", "default": false }
          ],
          "include": [ { "folder": "app", "rename": { "_gitignore": ".gitignore" } } ],
          "post": [ { "action": "install", "when": { "key": "install", "equals": true } } ]
        }"#,
    );
    t.write("demo.1.0.0/app/package.json", "{ \"name\": \"{__projectName__}\", \"scripts\": { \"postinstall\": \"node x.js\" } }\n");
    t.write("demo.1.0.0/app/src/{__projectName__}.ts", "export {};\n");
    t.write("demo.1.0.0/app/_gitignore", "node_modules\n");
    t
}

#[test]
fn versions_and_form_describe_what_a_host_needs_to_draw_a_screen() {
    let t = templates();
    let (code, lines) = run(&["versions", "--generator", "demo", "--templates", &t.s()], None);
    assert_eq!(code, 0);
    let v = last(&lines);
    assert_eq!(v["ok"], true);
    assert_eq!(v["versions"][0]["version"], "1.0.0");
    assert_eq!(v["versions"][0]["installed"], true);
    assert_eq!(v["versions"][0]["local"], true);

    let (code, lines) = run(&["form", "--generator", "demo", "--templates", &t.s()], None);
    assert_eq!(code, 0);
    let f = last(&lines);
    assert_eq!(f["generator"], "demo");
    assert_eq!(f["version"], "1.0.0", "the newest installed version is the default");
    assert_eq!(f["folderFrom"], "projectName");
    assert_eq!(f["requires"]["node"], ">=20.0.0");
    assert_eq!(f["prompts"][0]["key"], "projectName");
    assert_eq!(f["prompts"][0]["type"], "text");
    assert_eq!(f["prompts"][0]["rule"], "package-name");
    assert_eq!(f["prompts"][1]["type"], "bool");
    assert_eq!(f["post"][0], "install");
}

#[test]
fn plan_says_what_would_be_written_and_the_warnings_and_writes_nothing() {
    let t = templates();
    let loc = Tmp::new();
    let answers = r#"{ "projectName": "my-app", "install": true }"#;
    let (code, lines) = run(&["plan", "--generator", "demo", "--templates", &t.s(), "--location", &loc.s(), "--answers", "-"], Some(answers));
    assert_eq!(code, 0, "{lines:?}");
    let p = last(&lines);
    assert_eq!(p["ok"], true);
    assert_eq!(p["folderName"], "my-app");
    let paths: Vec<&str> = p["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, [".gitignore", "package.json", "src/my-app.ts"]);
    assert!(p["totalBytes"].as_u64().unwrap() > 0);
    // the install step, as data, and the warning the host should show before running it
    assert_eq!(p["post"][0]["action"], "install");
    assert_eq!(p["post"][0]["packageManager"], "npm");
    let warnings = p["warnings"].as_array().unwrap();
    let kinds: Vec<&str> = warnings.iter().map(|w| w["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"untested_package_manager"), "{kinds:?}");
    let w = warnings.iter().find(|w| w["kind"] == "install_scripts").expect("the install script is reported");
    assert_eq!(w["scripts"][0], "postinstall");
    assert!(w["text"].as_str().unwrap().contains("read them before installing"));
    assert!(fs::read_dir(&loc.0).unwrap().next().is_none(), "plan wrote something");
}

#[test]
fn apply_reports_each_file_then_the_result_and_the_project_is_there() {
    let t = templates();
    let loc = Tmp::new();
    let answers = loc.0.join("answers.json");
    fs::write(&answers, r#"{ "projectName": "my-app" }"#).unwrap();
    let (code, lines) = run(
        &["apply", "--generator", "demo", "--version", "1.0.0", "--templates", &t.s(), "--location", &loc.s(), "--answers", answers.to_str().unwrap()],
        None,
    );
    assert_eq!(code, 0, "{lines:?}");
    // one line per file, then the result
    let progress: Vec<&Value> = lines.iter().filter(|l| l["event"] == "progress").collect();
    assert_eq!(progress.len(), 3);
    assert_eq!(progress[0]["index"], 1);
    assert_eq!(progress[2]["total"], 3);
    let done = last(&lines);
    assert_eq!((done["event"].as_str(), done["ok"].as_bool(), done["filesWritten"].as_u64()), (Some("done"), Some(true), Some(3)));
    assert_eq!(fs::read_to_string(loc.0.join("my-app/src/my-app.ts")).unwrap(), "export {};\n");
    assert!(loc.0.join("my-app/.gitignore").is_file());
    // the install was not asked for, so none is handed over
    assert_eq!(done["post"].as_array().unwrap().len(), 0);
}

#[test]
fn a_failure_is_json_with_a_kind_and_the_fields_that_need_fixing_and_exit_code_1() {
    let t = templates();
    let loc = Tmp::new();
    let (code, lines) = run(
        &["plan", "--generator", "demo", "--templates", &t.s(), "--location", &loc.s(), "--answers", "-"],
        Some(r#"{ "projectName": "My App" }"#),
    );
    assert_eq!(code, 1);
    let e = &last(&lines)["error"];
    assert_eq!(last(&lines)["ok"], false);
    assert_eq!(e["kind"], "validation");
    assert_eq!(e["fields"][0]["field"], "projectName");
    assert!(e["fields"].as_array().unwrap().iter().any(|f| f["message"].as_str().unwrap().contains("No spaces")));

    // a generator that is not there
    let (code, lines) = run(&["form", "--generator", "nope", "--templates", &t.s()], None);
    assert_eq!(code, 1);
    assert!(last(&lines)["error"]["kind"].is_string());

    // a folder that already exists is refused, and nothing is touched
    fs::create_dir_all(loc.0.join("taken")).unwrap();
    let (code, lines) = run(
        &["apply", "--generator", "demo", "--templates", &t.s(), "--location", &loc.s(), "--answers", "-"],
        Some(r#"{ "projectName": "taken" }"#),
    );
    assert_eq!(code, 1);
    assert_eq!(last(&lines)["error"]["kind"], "folder_exists");
}

#[test]
fn a_wrong_command_line_is_json_with_exit_code_2() {
    for args in [&[][..], &["frobnicate"][..], &["plan", "--generator"][..], &["plan", "--templates", "x"][..], &["check"][..]] {
        let (code, lines) = run(args, None);
        assert_eq!(code, 2, "{args:?}: {lines:?}");
        assert_eq!(last(&lines)["error"]["kind"], "usage", "{args:?}");
    }
    // bad answers on stdin are a usage error too, not a crash
    let t = templates();
    let loc = Tmp::new();
    let (code, lines) = run(&["plan", "--generator", "demo", "--templates", &t.s(), "--location", &loc.s(), "--answers", "-"], Some("not json"));
    assert_eq!(code, 2);
    assert!(last(&lines)["error"]["message"].as_str().unwrap().contains("not a JSON object"));

    let (code, lines) = run(&["--version"], None);
    assert_eq!((code, last(&lines)["version"].as_str().is_some()), (0, true));
}

#[test]
fn check_reports_problems_and_the_variants_it_rendered() {
    let t = templates();
    let folder = t.0.join("demo.1.0.0");
    let (code, lines) = run(&["check", folder.to_str().unwrap()], None);
    assert_eq!(code, 0);
    let r = last(&lines);
    assert_eq!(r["ok"], true);
    assert_eq!(r["variants"][0]["label"], "defaults");
    assert_eq!(r["variants"][1]["label"], "install = true");
    assert!(r["variants"][0]["warnings"][0].as_str().unwrap().contains("postinstall"));

    // a broken template: the exit code is still 0 (the check ran), and the report says it is not ok
    fs::remove_dir_all(folder.join("app")).unwrap();
    let (code, lines) = run(&["check", folder.to_str().unwrap()], None);
    assert_eq!(code, 0);
    assert_eq!(last(&lines)["ok"], false);
    assert!(!last(&lines)["problems"].as_array().unwrap().is_empty());
}

#[test]
fn create_makes_a_template_from_a_project_and_can_write_it() {
    let project = Tmp::new();
    project.write("package.json", "{ \"name\": \"my-app\" }\n");
    project.write("src/MyApp.ts", "export class MyApp {}\n");
    project.write(".env", "SECRET=1\n");
    let out = Tmp::new();
    let target = out.0.join("gen");

    // without --out nothing is written
    let project_path = project.s();
    let args = ["create", project_path.as_str(), "--id", "mine", "--version", "1.0.0", "--replace", "projectName=my-app", "--label", "projectName=The name"];
    let (code, lines) = run(&args, None);
    assert_eq!(code, 0, "{lines:?}");
    let r = last(&lines);
    assert_eq!(r["ok"], true);
    assert_eq!(r["files"], 2);
    assert!(r["wrote"].is_null());
    assert_eq!(r["skipped"][0]["path"], ".env");
    assert_eq!(r["counts"][0]["key"], "projectName");
    assert!(r["counts"][0]["occurrences"].as_u64().unwrap() >= 3);
    assert!(!target.exists());

    // with --out it is written, and it is a template that checks clean
    let mut with_out = args.to_vec();
    with_out.extend(["--out", target.to_str().unwrap()]);
    let (code, lines) = run(&with_out, None);
    assert_eq!(code, 0, "{lines:?}");
    assert_eq!(last(&lines)["wrote"]["filesWritten"], 3, "two files and the manifest");
    let (_, check) = run(&["check", target.to_str().unwrap()], None);
    assert_eq!(last(&check)["ok"], true, "{check:?}");
    assert!(fs::read_to_string(target.join("manifest.json")).unwrap().contains("The name"));

    // a text that is not in the project is an error with a kind
    let (code, lines) = run(&["create", project_path.as_str(), "--id", "m", "--version", "1.0.0", "--replace", "x=not-there"], None);
    assert_eq!(code, 1);
    assert!(last(&lines)["error"]["message"].as_str().unwrap().contains("does not appear"));
    // and a --replace without an equals sign is a usage error
    let (code, _) = run(&["create", project_path.as_str(), "--id", "m", "--version", "1.0.0", "--replace", "nokey"], None);
    assert_eq!(code, 2);
}

#[test]
fn form_says_which_prompts_are_asked_for_the_answers_given() {
    let t = Tmp::new();
    t.write(
        "cond.1.0.0/manifest.json",
        r#"{ "id": "cond", "version": "1.0.0", "folderFrom": "projectName",
          "prompts": [
            { "key": "projectName", "type": "text" },
            { "key": "database", "type": "bool", "default": false },
            { "key": "dbKind", "type": "choice", "options": ["postgres", "mysql"], "when": { "key": "database", "equals": true } }
          ],
          "include": [ { "folder": "base" } ] }"#,
    );
    t.write("cond.1.0.0/base/readme.txt", "{__projectName__}\n");
    let visible = |answers: &str| -> Vec<String> {
        let (code, lines) = run(&["form", "--generator", "cond", "--templates", &t.s(), "--answers", "-"], Some(answers));
        assert_eq!(code, 0, "{lines:?}");
        last(&lines)["visible"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    };
    assert_eq!(visible("{}"), ["projectName", "database"]);
    assert_eq!(visible(r#"{ "database": true }"#), ["projectName", "database", "dbKind"]);

    // the prompt's own condition is in the form, and without --answers there is no "visible" list
    let (_, lines) = run(&["form", "--generator", "cond", "--templates", &t.s()], None);
    let f = last(&lines);
    assert!(f["visible"].is_null());
    assert_eq!(f["prompts"][2]["when"]["key"], "database");
    assert_eq!(f["prompts"][2]["when"]["equals"], true);
}

#[test]
fn info_lists_the_solutions_in_a_workspace() {
    let t = Tmp::new();
    for (dir, name) in [("apps/web", "web"), ("apps/intranet", "intranet")] {
        t.write(&format!("{dir}/package.json"), &format!(
            r#"{{ "name": "{name}", "dependencies": {{ "@microsoft/sp-core-library": "1.21.1" }}, "devDependencies": {{ "@microsoft/sp-build-web": "1.21.1" }} }}"#));
        t.write(&format!("{dir}/config/config.json"), "{}");
        t.write(&format!("{dir}/config/package-solution.json"), &format!(r#"{{ "solution": {{ "name": "{name} solution", "version": "1.0.0.0" }} }}"#));
        t.write(&format!("{dir}/src/webparts/a/AWebPart.manifest.json"), r#"{ "alias": "AWebPart", "componentType": "WebPart", "id": "x" }"#);
    }
    let (code, lines) = run(&["info", &t.s()], None);
    assert_eq!(code, 0, "{lines:?}");
    let r = last(&lines);
    let names: Vec<&str> = r["solutions"].as_array().unwrap().iter().map(|s| s["packageName"].as_str().unwrap()).collect();
    assert_eq!(names, ["intranet", "web"]);
    assert_eq!(r["solutions"][0]["solutionName"], "intranet solution");
    assert_eq!(r["solutions"][0]["type"], "web part");
    assert_eq!(r["solutions"][0]["components"][0]["name"], "AWebPart");
    assert!(r["problems"].as_array().unwrap().is_empty());

    // a depth that is too small finds nothing, and a folder that is not there is an error
    let (_, lines) = run(&["info", &t.s(), "--depth", "1"], None);
    assert!(last(&lines)["solutions"].as_array().unwrap().is_empty());
    let (code, _) = run(&["info", &format!("{}/nope", t.s())], None);
    assert_eq!(code, 1);
}
