//! Scaffolds a solution from a generator folder.
//!
//!     cargo run --example scaffold -- <generator folder> <location> <solutionName> <webpart>[,<webpart>...]

use forge_scaffold::{apply, plan, Answers, DirSource, Manifest, PlanOptions};
use serde_json::json;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: scaffold <generator folder> <location> <solutionName> <webpart>[,<webpart>...]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> forge_scaffold::Result<()> {
    let root = PathBuf::from(&args[0]);
    let json_text = std::fs::read_to_string(root.join("manifest.json"))
        .map_err(|e| forge_scaffold::ScaffoldError::Manifest(e.to_string()))?;
    let manifest = Manifest::from_json(&json_text)?;
    let source = DirSource::new(&root);

    let mut answers = Answers::new();
    answers.insert("solutionName".into(), json!(args[2]));
    answers.insert(
        "webparts".into(),
        json!(args[3].split(',').collect::<Vec<_>>()),
    );

    let planned = plan(&manifest, &source, &answers, PlanOptions::new(&args[1]))?;
    for w in &planned.warnings {
        println!("warning: {w:?}");
    }
    let outcome = apply(&planned, |_| {})?;
    println!("{} -> {outcome:?}", planned.target.display());
    Ok(())
}
