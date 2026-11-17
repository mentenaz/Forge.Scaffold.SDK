//! Adds web parts to an existing SPFx solution, or says there is none.
//!
//!     cargo run --example add -- <generator folder> <folder in the solution> <web part>[,<web part>...] [--apply]
//!
//! Without `--apply` it only shows the plan. Exit codes: 0 done (or planned),
//! 3 the folder is not inside an SPFx solution (scaffold a new one instead), 1 error.

use forge_scaffold::{apply_add, detect_spfx, plan_add, AddOptions, AddOutcome, Answers, DirSource, Manifest};
use serde_json::json;
use std::path::PathBuf;

fn main() {
    let mut positional: Vec<String> = Vec::new();
    let mut apply = false;
    for arg in std::env::args().skip(1) {
        if arg == "--apply" {
            apply = true;
        } else {
            positional.push(arg);
        }
    }
    if positional.len() != 3 {
        eprintln!("usage: add <generator folder> <folder in the solution> <web part>[,<web part>...] [--apply]");
        std::process::exit(2);
    }
    match run(&positional, apply) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String], apply: bool) -> forge_scaffold::Result<i32> {
    let generator = PathBuf::from(&args[0]);
    let manifest_text = std::fs::read_to_string(generator.join("manifest.json"))
        .map_err(|e| forge_scaffold::ScaffoldError::Manifest(e.to_string()))?;
    let manifest = Manifest::from_json(&manifest_text)?;

    let Some(solution) = detect_spfx(&PathBuf::from(&args[1]))? else {
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

    let mut answers = Answers::new();
    answers.insert("webparts".into(), json!(args[2].split(',').collect::<Vec<_>>()));
    let plan = plan_add(&solution, &manifest, &DirSource::new(&generator), &answers, AddOptions::new())?;
    for w in &plan.warnings {
        println!("warning: {w}");
    }
    for f in &plan.files {
        let what = match f.outcome {
            AddOutcome::New => "new   ",
            AddOutcome::Merged => "edited",
        };
        println!("  {what} {}", f.path);
    }
    if !apply {
        println!("{} files planned; run again with --apply to write them", plan.files.len());
        return Ok(0);
    }
    let out = apply_add(&plan, |_| {})?;
    println!("wrote {} files ({} bytes)", out.files_written, out.bytes_written);
    Ok(0)
}
