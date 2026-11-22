//! `forge-scaffold`: the engine as a program that speaks JSON, for hosts that are not written in Rust (a Node TUI such as
//! Helm starts it the way it starts `gh`).
//!
//! Every command prints JSON on standard output. A command prints one JSON document, except `apply`, which first prints
//! one line per file written. **Every line is a JSON object, and the last line is the result**, which has `"ok": true`
//! or `"ok": false` with an `"error"` (`kind`, `message`, and for a validation error one entry per `fields`). Every result
//! also carries `"protocol"`, the version of this JSON (now 1): adding a field never changes it, removing or renaming one, or
//! changing what it means, does.
//! Exit code: 0 done, 1 the command failed (see the error), 2 wrong command line.
//!
//! ```text
//! forge-scaffold versions --generator <id> [--templates <root>]
//! forge-scaffold form     --generator <id> [--version <v>] [--templates <root>]
//! forge-scaffold plan     --generator <id> [--version <v>] --location <dir> --answers <file|-> [--templates <root>]
//! forge-scaffold apply    --generator <id> [--version <v>] --location <dir> --answers <file|-> [--templates <root>]
//! forge-scaffold check    <template folder> [--answers <file|->] [--max-variants <n>]
//! forge-scaffold info     <workspace folder> [--depth <n>]
//! forge-scaffold create   <project folder> --id <id> --version <v> --replace <key>=<text>... [--label <key>=<label>]...
//!                         [--exclude <name>]... [--out <new folder>]
//! forge-scaffold --version | --help
//! ```
//!
//! The program opens no connections and runs no commands: `plan` and `apply` return the follow-up steps (`install`) as data
//! for the host to run, and the warnings the host should show first. Answers are a JSON object, `{"projectName": "my-app"}`.

use forge_scaffold::{
    apply, check_template, create_template, plan, resolve_templates_root, Answers, CheckOptions, CreateOptions, DirSource, Plan,
    PlanOptions, Replacement, ResolvedTemplate, ScaffoldError, TemplateStore,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const EXIT_OK: i32 = 0;
const EXIT_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

/// What was on the command line.
struct Flags {
    /// `--name value`, in order (a name may be given more than once).
    values: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

impl Flags {
    fn parse(args: &[String]) -> Result<Flags, String> {
        let mut flags = Flags { values: BTreeMap::new(), positional: Vec::new() };
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(name) = a.strip_prefix("--") {
                let value = args.get(i + 1).ok_or_else(|| format!("--{name} needs a value"))?;
                flags.values.entry(name.to_string()).or_default().push(value.clone());
                i += 2;
            } else {
                flags.positional.push(a.clone());
                i += 1;
            }
        }
        Ok(flags)
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(|v| v.last()).map(String::as_str)
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.one(name).ok_or_else(|| format!("--{name} is required"))
    }

    fn all(&self, name: &str) -> &[String] {
        self.values.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    fn folder(&self) -> Result<&str, String> {
        match self.positional.as_slice() {
            [one] => Ok(one),
            _ => Err("give exactly one folder".to_string()),
        }
    }
}

/// The version of the JSON this program speaks. A host can check it once; see the top of this file for when it changes.
const PROTOCOL: u32 = 1;

fn print(value: &Value) {
    // a result (it has "ok") says which protocol it speaks; the progress lines of `apply` do not need to
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("ok") {
            object.insert("protocol".to_string(), json!(PROTOCOL));
        }
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn usage_error(message: &str) -> i32 {
    print(&json!({ "ok": false, "error": { "kind": "usage", "message": message, "usage": usage() } }));
    EXIT_USAGE
}

fn failed(e: &ScaffoldError) -> i32 {
    print(&json!({ "ok": false, "error": e.to_json() }));
    EXIT_FAILED
}

fn usage() -> &'static str {
    "forge-scaffold versions|form|plan|apply --generator <id> [--version <v>] [--templates <root>] ...; check <folder>; create <folder> --id <id> --version <v> --replace key=text ...; see the top of src/bin/forge-scaffold.rs"
}

fn run(args: &[String]) -> i32 {
    let Some(command) = args.first() else { return usage_error("give a command") };
    match command.as_str() {
        "--help" | "-h" | "help" => {
            print(&json!({ "ok": true, "usage": usage() }));
            return EXIT_OK;
        }
        "--version" | "-V" => {
            print(&json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }));
            return EXIT_OK;
        }
        _ => {}
    }
    let flags = match Flags::parse(&args[1..]) {
        Ok(f) => f,
        Err(m) => return usage_error(&m),
    };
    let result = match command.as_str() {
        "versions" => versions(&flags),
        "form" => form(&flags),
        "plan" => plan_command(&flags, false),
        "apply" => plan_command(&flags, true),
        "check" => check(&flags),
        "info" => info(&flags),
        "create" => create(&flags),
        other => return usage_error(&format!("unknown command '{other}'")),
    };
    match result {
        Ok(value) => {
            print(&value);
            EXIT_OK
        }
        Err(Failure::Usage(m)) => usage_error(&m),
        Err(Failure::Scaffold(e)) => failed(&e),
    }
}

enum Failure {
    Usage(String),
    Scaffold(ScaffoldError),
}

impl From<ScaffoldError> for Failure {
    fn from(e: ScaffoldError) -> Self {
        Failure::Scaffold(e)
    }
}
impl From<String> for Failure {
    fn from(m: String) -> Self {
        Failure::Usage(m)
    }
}

type Done = Result<Value, Failure>;

fn open_store(flags: &Flags) -> Result<TemplateStore, Failure> {
    let root = resolve_templates_root(flags.one("templates").map(Path::new))?;
    Ok(TemplateStore::open(root)?)
}

/// The version asked for, else the newest one that is installed.
fn resolve(flags: &Flags) -> Result<ResolvedTemplate, Failure> {
    let store = open_store(flags)?;
    let generator = flags.required("generator")?;
    let version = match flags.one("version") {
        Some(v) => v.to_string(),
        None => store
            .list_versions(generator)?
            .versions
            .into_iter()
            .find(|v| v.installed)
            .map(|v| v.version)
            .ok_or_else(|| ScaffoldError::NotFound(format!("no version of '{generator}' is installed")))?,
    };
    Ok(store.resolve(generator, &version)?)
}

fn read_answers(source: &str) -> Result<Answers, Failure> {
    let text = if source == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| format!("cannot read the answers from standard input: {e}"))?;
        s
    } else {
        std::fs::read_to_string(source).map_err(|e| format!("cannot read '{source}': {e}"))?
    };
    serde_json::from_str::<Answers>(&text).map_err(|e| Failure::Usage(format!("the answers are not a JSON object: {e}")))
}

fn versions(flags: &Flags) -> Done {
    let store = open_store(flags)?;
    let generator = flags.required("generator")?;
    let list = store.list_versions(generator)?;
    Ok(json!({
        "ok": true,
        "generator": generator,
        "versions": list.versions.iter().map(|v| json!({
            "version": v.version, "installed": v.installed, "local": v.local,
            "installedRevision": v.installed_revision, "availableRevision": v.available_revision,
            "updateAvailable": v.update_available, "downloadable": v.downloadable,
        })).collect::<Vec<_>>(),
        "problems": list.problems,
    }))
}

fn form(flags: &Flags) -> Done {
    let t = resolve(flags)?;
    let m = &t.manifest;
    // With --answers, also say which prompts are asked for those answers (a prompt can have a `when`)
    let visible = match flags.one("answers") {
        Some(a) => Some(m.visible_prompts(&read_answers(a)?).iter().map(|p| p.key.clone()).collect::<Vec<_>>()),
        None => None,
    };
    Ok(json!({
        "visible": visible,
        "ok": true,
        "generator": m.id, "version": m.version, "local": t.local, "revision": t.revision,
        "folderFrom": m.folder_from,
        "prompts": serde_json::to_value(&m.prompts).map_err(|e| ScaffoldError::Manifest(e.to_string()))?,
        "requires": m.requires.as_ref().and_then(|r| r.node.as_ref()).map(|n| json!({ "node": n })),
        "post": m.post.iter().map(|p| format!("{:?}", p.action).to_lowercase()).collect::<Vec<_>>(),
    }))
}

fn plan_json(p: &Plan) -> Value {
    json!({
        "target": p.target.display().to_string(),
        "folderName": p.folder_name,
        "files": p.files.paths().map(|path| json!({ "path": path, "bytes": p.files.get(path).map_or(0, <[u8]>::len) })).collect::<Vec<_>>(),
        "totalBytes": p.files.total_bytes(),
        "warnings": p.warnings.iter().map(|w| w.to_json()).collect::<Vec<_>>(),
        "post": p.post.iter().map(|s| json!({
            "action": format!("{:?}", s.action).to_lowercase(),
            "cwd": s.cwd.display().to_string(),
            "packageManager": s.package_manager.map(|pm| format!("{pm:?}").to_lowercase()),
        })).collect::<Vec<_>>(),
    })
}

fn plan_command(flags: &Flags, write: bool) -> Done {
    let t = resolve(flags)?;
    let location = PathBuf::from(flags.required("location")?);
    let answers = read_answers(flags.required("answers")?)?;
    let planned = plan(&t.manifest, &DirSource::new(&t.dir), &answers, PlanOptions::new(&location))?;
    let mut result = plan_json(&planned);
    result["ok"] = json!(true);
    if !write {
        return Ok(result);
    }
    let outcome = apply(&planned, |p| {
        print(&json!({ "event": "progress", "index": p.index, "total": p.total, "path": p.path }));
    })?;
    // TODO(forge-analysis): this is the point where the written project could be analysed, before the host runs the
    // `install` step it is handed below. Add the analysis report's summary to `result` (a new key, so existing hosts are
    // not broken). Not wired until the analysis engine is finished, tested and confirmed.
    result["event"] = json!("done");
    result["filesWritten"] = json!(outcome.files_written);
    result["bytesWritten"] = json!(outcome.bytes_written);
    Ok(result)
}

/// Every SPFx solution in a workspace folder, with what a panel shows about each one. Reads files only.
fn info(flags: &Flags) -> Done {
    let folder = flags.folder()?;
    let depth = match flags.one("depth") {
        Some(n) => n.parse().map_err(|_| format!("--depth must be a number, not '{n}'"))?,
        None => 4,
    };
    let scan = forge_scaffold::find_spfx_solutions(Path::new(folder), depth)?;
    let mut solutions = Vec::new();
    for s in &scan.solutions {
        solutions.push(forge_scaffold::solution_info(s)?.to_json());
    }
    Ok(json!({ "solutions": solutions, "problems": scan.problems }))
}

fn check(flags: &Flags) -> Done {
    let folder = flags.folder()?;
    let mut options = CheckOptions::default();
    if let Some(a) = flags.one("answers") {
        options.answers = read_answers(a)?;
    }
    if let Some(n) = flags.one("max-variants") {
        options.max_variants = n.parse().map_err(|_| format!("--max-variants must be a number, not '{n}'"))?;
    }
    let report = check_template(&DirSource::new(folder), &options);
    // TODO(forge-analysis): see check_template in src/check.rs; the analysis summary would join `report` here.
    Ok(report.to_json())
}

fn create(flags: &Flags) -> Done {
    let project = flags.folder()?;
    let labels: BTreeMap<String, String> = flags
        .all("label")
        .iter()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let mut replacements = Vec::new();
    for r in flags.all("replace") {
        let (key, text) = r.split_once('=').ok_or_else(|| format!("--replace takes key=text, not '{r}'"))?;
        replacements.push(Replacement { key: key.to_string(), text: text.to_string(), label: labels.get(key).cloned() });
    }
    let mut options = CreateOptions::new(flags.required("id")?, flags.required("version")?, replacements);
    options.exclude = flags.all("exclude").to_vec();

    let draft = create_template(Path::new(project), &options)?;
    let mut result = json!({
        "ok": true,
        "files": draft.file_count(),
        "paths": draft.paths(),
        "skipped": draft.skipped.iter().map(|s| json!({ "path": s.path, "reason": s.reason })).collect::<Vec<_>>(),
        "counts": draft.counts.iter().map(|c| json!({ "key": c.key, "text": c.text, "occurrences": c.occurrences, "files": c.files })).collect::<Vec<_>>(),
        "notes": draft.notes,
        "wrote": null,
    });
    if let Some(out) = flags.one("out") {
        let outcome = draft.write_to(Path::new(out))?;
        result["wrote"] = json!({ "root": outcome.root.display().to_string(), "filesWritten": outcome.files_written, "bytesWritten": outcome.bytes_written });
    }
    Ok(result)
}
