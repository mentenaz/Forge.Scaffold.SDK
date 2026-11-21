//! A host in miniature: the whole flow a panel goes through, from the templates folder
//! to a written solution, using only the public API of the crate.
//!
//!     cargo run --example host_flow -- --generator <id> [options]
//!
//! What the crate does NOT do is what a host has to bring, and this example shows each piece:
//!
//! * HTTP: the crate runs no connection. A host implements [`Fetch`]; here it shells out to `curl`.
//! * `node -v` and `npm install`: the crate runs no commands. The host finds the installed Node
//!   (and checks it against the template's requirement) and runs the install step the plan describes.
//! * The form: the manifest's prompts are data; a host draws one input per prompt.
//! * The confirmation: the plan is data too; a host shows it before anything is written.
//!
//! Options:
//!     --templates <dir>       the templates folder (default: the crate's default location)
//!     --generator <id>        e.g. spfx-webpart or webpart-react (required)
//!     --version <x.y.z>       default: the newest installed
//!     --index <https url>     check this index for updates first (and --update to install the one wanted)
//!     --folder <dir>          where the user is. Inside an SPFx solution it adds to it, otherwise it creates
//!                             a new solution in it (default: the current folder)
//!     --answer key=value      an answer for a prompt (repeat; lists are comma separated)
//!     --node <x.y.z>          pretend this Node is installed (default: ask `node -v`)
//!     --yes                   do not ask for confirmation
//!     --run-post              run the install step the plan asks for (needs --yes)
//!     --probe <https url>     only test the HTTP glue: two requests, the second with the first one's ETag

use forge_scaffold::{
    apply, apply_add, detect_spfx, plan, plan_add, resolve_templates_root, unix_now, AddOptions, AddOutcome, Answers,
    CheckStatus, DirSource, Fetch, FetchResult, Manifest, NodeCheck, PackageManager, PlanOptions, PostAction, PromptKind,
    Result, ScaffoldError, TemplateStore, Warning,
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------------------------- HTTP

/// The host's half of the network contract: GET with an ETag. Uses `curl`, which a real panel would
/// replace with the HTTP client it already has.
struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, url: &str, etag: Option<&str>) -> Result<FetchResult> {
        let fail = |reason: String| ScaffoldError::Fetch { url: url.to_string(), reason };
        let dir = std::env::temp_dir().join(format!("forge-host-flow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
        let (body, headers) = (dir.join("body"), dir.join("headers"));

        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "-L", "--max-time", "60", "-w", "%{http_code}", "-D"]).arg(&headers).arg("-o").arg(&body);
        if let Some(tag) = etag {
            cmd.arg("-H").arg(format!("If-None-Match: {tag}"));
        }
        cmd.arg(url);
        let out = cmd.output().map_err(|e| fail(format!("cannot run curl: {e}")))?;
        if !out.status.success() {
            return Err(fail(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        let code: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
        match code {
            304 => Ok(FetchResult::NotModified),
            200..=299 => {
                let bytes = std::fs::read(&body).map_err(|e| fail(e.to_string()))?;
                // the last response's ETag (there may be redirects before it)
                let etag = std::fs::read_to_string(&headers).ok().and_then(|h| {
                    h.lines()
                        .rev()
                        .find_map(|l| l.to_ascii_lowercase().starts_with("etag:").then(|| l[5..].trim().to_string()))
                });
                Ok(FetchResult::Body { bytes, etag })
            }
            other => Err(fail(format!("the server answered HTTP {other}"))),
        }
    }
}

fn probe(url: &str) -> Result<()> {
    let first = CurlFetch.get(url, None)?;
    let FetchResult::Body { bytes, etag } = first else {
        return Err(ScaffoldError::Fetch { url: url.into(), reason: "a 304 without asking".into() });
    };
    println!("1st request: {} bytes, ETag {}", bytes.len(), etag.as_deref().unwrap_or("(none)"));
    match (&etag, CurlFetch.get(url, etag.as_deref())?) {
        (Some(_), FetchResult::NotModified) => println!("2nd request with that ETag: 304 Not Modified (conditional requests work)"),
        (Some(_), FetchResult::Body { .. }) => println!("2nd request with that ETag: the server sent the body again (it ignores If-None-Match)"),
        (None, _) => println!("no ETag, so nothing to condition on"),
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------- command line

struct Args {
    templates: Option<PathBuf>,
    generator: String,
    version: Option<String>,
    index: Option<String>,
    update: bool,
    folder: PathBuf,
    answers: Vec<(String, String)>,
    node: Option<String>,
    yes: bool,
    run_post: bool,
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut a = Args {
        templates: None,
        generator: String::new(),
        version: None,
        index: None,
        update: false,
        folder: std::env::current_dir().map_err(|e| e.to_string())?,
        answers: Vec::new(),
        node: None,
        yes: false,
        run_post: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |what: &str| it.next().ok_or_else(|| format!("{arg} needs {what}"));
        match arg.as_str() {
            "--templates" => a.templates = Some(PathBuf::from(value("a folder")?)),
            "--generator" => a.generator = value("an id")?,
            "--version" => a.version = Some(value("a version")?),
            "--index" => a.index = Some(value("an https url")?),
            "--update" => a.update = true,
            "--folder" => a.folder = PathBuf::from(value("a folder")?),
            "--answer" => {
                let kv = value("key=value")?;
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("--answer {kv}: use key=value"))?;
                a.answers.push((k.to_string(), v.to_string()));
            }
            "--node" => a.node = Some(value("a version")?),
            "--yes" => a.yes = true,
            "--run-post" => a.run_post = true,
            "--probe" => {
                let url = value("an https url")?;
                probe(&url).map_err(|e| describe(&e))?;
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if a.generator.is_empty() {
        return Err("--generator is required (see the options at the top of examples/host_flow.rs)".into());
    }
    Ok(a)
}

/// What a panel shows for an error. `Validation` has one message per field.
fn describe(e: &ScaffoldError) -> String {
    match e {
        ScaffoldError::Validation(issues) => {
            let mut out = String::from("some answers need fixing:");
            for i in issues {
                out.push_str(&format!("\n  {}: {}", i.field, i.message));
            }
            out
        }
        other => other.to_string(),
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    match run(args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("\nerror: {}", describe(&e));
            std::process::exit(1);
        }
    }
}

// ------------------------------------------------------------------------------------------------ the flow

fn installed_node(given: &Option<String>) -> Option<String> {
    if let Some(v) = given {
        return Some(v.clone());
    }
    let out = Command::new("node").arg("-v").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One answer per prompt, the way a form would give them.
fn collect_answers(manifest: &Manifest, given: &[(String, String)]) -> std::result::Result<Answers, ScaffoldError> {
    let mut answers = Answers::new();
    for (key, text) in given {
        let prompt = manifest
            .prompts
            .iter()
            .find(|p| &p.key == key)
            .ok_or_else(|| ScaffoldError::Answers(format!("this generator has no prompt '{key}'")))?;
        let value: Value = match prompt.kind {
            PromptKind::Text | PromptKind::Choice => json!(text),
            PromptKind::Bool => json!(matches!(text.as_str(), "true" | "yes" | "1")),
            PromptKind::List => json!(text.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>()),
        };
        answers.insert(key.clone(), value);
    }
    Ok(answers)
}

fn confirm(question: &str, yes: bool) -> bool {
    if yes {
        return true;
    }
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
}

fn run(args: Args) -> Result<()> {
    // 1. the templates folder
    let root = resolve_templates_root(args.templates.as_deref())?;
    let store = TemplateStore::open(&root)?;
    println!("templates folder: {}", root.display());

    // 2. updates: the host supplies the HTTP, the crate does the rest
    if let Some(url) = &args.index {
        let outcome = store.check_updates(&CurlFetch, url, unix_now(), true, 24 * 3600)?;
        let status = match outcome.status {
            CheckStatus::Skipped => "checked recently, nothing asked",
            CheckStatus::NotModified => "no change on the server (304)",
            CheckStatus::Refreshed => "a new index was downloaded",
        };
        println!("index: {status}; {} update(s) available", outcome.updates.len());
    }

    // 3. the version list, newest first, as a dropdown would show it
    let list = store.list_versions(&args.generator)?;
    for p in &list.problems {
        println!("note: {p}");
    }
    println!("versions of {}:", args.generator);
    for v in &list.versions {
        let state = match (v.installed, v.local, v.update_available, v.downloadable) {
            (true, true, _, _) => "installed (your own folder)".to_string(),
            (true, false, true, _) => format!("installed, revision {} -> {} available", v.installed_revision.unwrap_or(0), v.available_revision.unwrap_or(0)),
            (true, false, false, _) => format!("installed, revision {}", v.installed_revision.unwrap_or(0)),
            (false, _, _, true) => "can be downloaded".to_string(),
            _ => "not available".to_string(),
        };
        println!("  {:<8} {state}", v.version);
    }
    let version = args
        .version
        .clone()
        .or_else(|| list.versions.iter().find(|v| v.installed).map(|v| v.version.clone()))
        .ok_or_else(|| ScaffoldError::NotFound(format!("no version of {} is installed", args.generator)))?;
    if args.update {
        let done = store.update(&CurlFetch, &args.generator, &version)?;
        println!("installed {} {} revision {}", done.generator, done.version, done.revision);
    }

    // 4. the template, ready to use
    let template = store.resolve(&args.generator, &version)?;
    let manifest = &template.manifest;
    println!("\nusing {} {} ({})", manifest.id, manifest.version, template.dir.display());

    // 5. the form
    println!("the form:");
    for p in &manifest.prompts {
        let default = p.default.as_ref().map(|d| format!(" (default {d})")).unwrap_or_default();
        let options = if p.options.is_empty() { String::new() } else { format!(" one of {}", p.options.join("/")) };
        let masked = if p.sensitive { " [hidden input]" } else { "" };
        println!("  {:<22} {:?}{options}{default}{masked}  {}", p.key, p.kind, p.label.as_deref().unwrap_or(""));
    }
    let answers = collect_answers(manifest, &args.answers)?;

    // 6. is this machine able to run it?
    let node = installed_node(&args.node);
    let solution = detect_spfx(&args.folder)?;
    let mut blocked = false;
    match &node {
        None => println!("\nnode: not found on this machine (the host would offer to install it)"),
        Some(installed) => {
            let mut checks = vec![("the template", manifest.node_check(installed)?)];
            if let Some(s) = &solution {
                checks.push(("this solution", s.node_check(installed)?));
            }
            for (what, check) in checks {
                match check {
                    NodeCheck::NoRequirement => {}
                    NodeCheck::Fine => println!("\nnode {installed}: fine for {what}"),
                    NodeCheck::Mismatch { required, installed } => {
                        println!("\nnode {installed} does not fit {what}, which needs {required}");
                        blocked = true;
                    }
                }
            }
        }
    }
    if blocked {
        return Err(ScaffoldError::Incompatible(
            "the installed Node is not one this needs: switch Node (nvm, fnm, volta) and try again".into(),
        ));
    }

    // 7. add to the solution the folder is in, or create a new one
    let source = DirSource::new(&template.dir);
    match solution {
        Some(solution) => {
            println!(
                "\n{} is inside an SPFx solution: adding (SPFx {}, {:?}{})",
                args.folder.display(),
                solution.version.as_deref().unwrap_or("?"),
                solution.build_system,
                if solution.framework.is_some() { ", React" } else { "" }
            );
            let planned = plan_add(&solution, manifest, &source, &answers, AddOptions::new())?;
            for w in &solution.warnings.iter().chain(&planned.warnings).collect::<Vec<_>>() {
                println!("warning: {w}");
            }
            for f in &planned.files {
                let what = if f.outcome == AddOutcome::New { "new   " } else { "edited" };
                println!("  {what} {}", f.path);
            }
            for k in &planned.kept {
                println!("  kept   {k}  (differs from the template, left as it is)");
            }
            if planned.is_empty() {
                println!("nothing to add");
                return Ok(());
            }
            if !confirm(&format!("write {} file(s) into {}?", planned.files.len(), solution.root.display()), args.yes) {
                println!("cancelled; nothing was written");
                return Ok(());
            }
            let done = apply_add(&planned, |p| println!("  [{}/{}] {}", p.index, p.total, p.path))?;
            println!("wrote {} files", done.files_written);
            // an add has no post step of its own, but a changed package.json means dependencies to install
            if planned.files.iter().any(|f| f.path == "package.json") {
                println!("package.json changed: the host would offer to run the install now");
                if args.run_post && args.yes {
                    run_install(&solution.root, PackageManager::Npm)?;
                }
            }
        }
        None => {
            println!("\n{} is not inside an SPFx solution: creating a new one in it", args.folder.display());
            let planned = plan(manifest, &source, &answers, PlanOptions::new(&args.folder))?;
            for w in &planned.warnings {
                match w {
                    Warning::NestedSolution { found_in } => println!("warning: this is inside another solution ({})", found_in.display()),
                    Warning::LongPath { length, limit } => println!("warning: paths of about {length} characters, the Windows limit is {limit}"),
                    Warning::UntestedPackageManager(pm) => println!("warning: {pm:?} was not tested with this generator"),
                    Warning::InstallScripts { file, scripts } => {
                        println!("warning: {file} has scripts that run when it is installed ({}): read them before installing", scripts.join(", "))
                    }
                    Warning::UrlDependencies { file, packages } => {
                        println!("warning: {file} gets {} from a URL, a git repository or a local path, not the registry", packages.join(", "))
                    }
                }
            }
            println!("new folder: {} ({} files)", planned.target.display(), planned.files.len());
            for step in &planned.post {
                println!("afterwards: {:?} in {} {}", step.action, step.cwd.display(), step.package_manager.map(|p| format!("with {p:?}")).unwrap_or_default());
            }
            if !confirm(&format!("create {}?", planned.target.display()), args.yes) {
                println!("cancelled; nothing was written");
                return Ok(());
            }
            let done = apply(&planned, |p| println!("  [{}/{}] {}", p.index, p.total, p.path))?;
            println!("wrote {} files, {} bytes", done.files_written, done.bytes_written);
            for step in planned.post.iter().filter(|s| s.action == PostAction::Install) {
                if args.run_post && args.yes {
                    run_install(&step.cwd, step.package_manager.unwrap_or(PackageManager::Npm))?;
                }
            }
        }
    }
    Ok(())
}

/// The host runs the install the plan describes. The crate only says what and where.
fn run_install(cwd: &std::path::Path, pm: PackageManager) -> Result<()> {
    let program = match pm {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Yarn => "yarn",
    };
    println!("running `{program} install` in {} ...", cwd.display());
    let status = Command::new(if cfg!(windows) { "cmd" } else { program })
        .args(if cfg!(windows) { vec!["/C", program, "install"] } else { vec!["install"] })
        .current_dir(cwd)
        .status()
        .map_err(|e| ScaffoldError::Io { path: cwd.to_path_buf(), source: e })?;
    if status.success() {
        println!("install finished");
        Ok(())
    } else {
        Err(ScaffoldError::Incompatible(format!("`{program} install` failed ({status})")))
    }
}
