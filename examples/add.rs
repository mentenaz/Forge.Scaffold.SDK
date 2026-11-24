//! Adds a component to an existing SPFx solution, or says there is none.
//!
//!     cargo run --example add -- <generator folder> <folder in the solution> <web part>[,<web part>...] [--apply]
//!     cargo run --example add -- --upstream <spfx repo> <template> [--overlay <folder>] <folder in the solution> <component name> [--apply]
//!
//! The first form adds web parts with one of our own generator folders. The
//! second adds one component with a template of the official SharePoint/spfx CLI.
//!
//! Without `--apply` it only shows the plan. Exit codes: 0 done (or planned),
//! 3 the folder is not inside an SPFx solution (scaffold a new one instead), 1 error.

use forge_scaffold::{
    apply_add, detect_spfx, plan_add, AddOptions, AddOutcome, AddPlan, Answers, DirSource, Manifest, Overlay,
    SpfxSolution, UpstreamInputs, UpstreamSource,
};
use serde_json::json;
use std::path::PathBuf;

/// What plans the add once the solution is known.
type PlanFn = Box<dyn Fn(&SpfxSolution) -> forge_scaffold::Result<AddPlan>>;

fn main() {
    let mut positional: Vec<String> = Vec::new();
    let (mut apply, mut upstream, mut overlay) = (false, None::<(String, String)>, None::<String>);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--apply" => apply = true,
            "--upstream" => {
                let (Some(repo), Some(template)) = (args.next(), args.next()) else {
                    eprintln!("--upstream needs <spfx repo> <template>");
                    std::process::exit(2);
                };
                upstream = Some((repo, template));
            }
            "--overlay" => overlay = args.next(),
            _ => positional.push(arg),
        }
    }
    let wanted = if upstream.is_some() { 2 } else { 3 };
    if positional.len() != wanted {
        eprintln!("usage: add <generator folder> <folder in the solution> <web part>[,...] [--apply]");
        eprintln!("   or: add --upstream <spfx repo> <template> [--overlay <folder>] <folder in the solution> <component name> [--apply]");
        std::process::exit(2);
    }
    match run(&positional, upstream, overlay, apply) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String], upstream: Option<(String, String)>, overlay: Option<String>, apply: bool) -> forge_scaffold::Result<i32> {
    let (folder, plan_for): (&str, PlanFn) = match upstream {
        Some((repo, template)) => {
            let (def, mut source) = UpstreamSource::load(&DirSource::new(&repo), &format!("templates/{template}"))?;
            if let Some(dir) = overlay {
                let overlay = Overlay::load(&DirSource::new(dir), "")?;
                let replaced = source.apply_overlay(&def, &overlay);
                source.apply_rewrites(&def, &overlay)?;
                println!("overlay replaced {replaced} files");
            }
            let manifest = def.manifest()?;
            let answers = UpstreamInputs { component_name: args[1].clone(), ..Default::default() }.add_answers();
            (&args[0], Box::new(move |s| plan_add(s, &manifest, &source, &answers, AddOptions::new())))
        }
        None => {
            let generator = PathBuf::from(&args[0]);
            let manifest_text = std::fs::read_to_string(generator.join("manifest.json"))
                .map_err(|e| forge_scaffold::ScaffoldError::Manifest(e.to_string()))?;
            let manifest = Manifest::from_json(&manifest_text)?;
            let mut answers = Answers::new();
            answers.insert("webparts".into(), json!(args[2].split(',').collect::<Vec<_>>()));
            let source = DirSource::new(&generator);
            (&args[1], Box::new(move |s| plan_add(s, &manifest, &source, &answers, AddOptions::new())))
        }
    };

    let Some(solution) = detect_spfx(&PathBuf::from(folder))? else {
        println!("not inside an SPFx solution: scaffold a new one");
        return Ok(3);
    };
    println!(
        "SPFx solution at {}\n  version {}, {:?}, framework {}, package {}",
        solution.root.display(),
        solution.version.as_deref().unwrap_or("?"),
        solution.build_system,
        solution.framework.as_deref().unwrap_or("?"),
        solution.package_name.as_deref().unwrap_or("?"),
    );

    let plan = plan_for(&solution)?;
    for w in &plan.warnings {
        println!("warning: {w}");
    }
    for f in &plan.files {
        let what = match f.outcome {
            AddOutcome::New => "new   ",
            AddOutcome::Merged => "edited",
            _ => "changed", // an outcome this host does not know: say so plainly
        };
        println!("  {what} {}", f.path);
    }
    for k in &plan.kept {
        println!("  kept   {k}");
    }
    if !apply {
        println!("{} files planned; run again with --apply to write them", plan.files.len());
        return Ok(0);
    }
    let out = apply_add(&plan, |_| {})?;
    println!("wrote {} files ({} bytes)", out.files_written, out.bytes_written);
    Ok(0)
}
