//! Scaffolds a project from a template of create-vite (the Vite repo, `packages/create-vite`).
//!
//!     cargo run --example create_vite -- <vite repo> <template> <location> <project name>
//!
//! e.g. `... -- E:\\vite react-ts C:\\work my-app`. The project is made in `<location>/<project name>`.
//! Installing is left to the host: this only writes the files.

use forge_scaffold::{apply, plan, DirSource, PlanOptions, ViteSource, CREATE_VITE_DIR};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        eprintln!("usage: create_vite <vite repo> <template> <location> <project name>");
        std::process::exit(2);
    }
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> forge_scaffold::Result<()> {
    let repo = DirSource::new(&args[0]);
    let (template, source) = ViteSource::load(&repo, CREATE_VITE_DIR, &args[1])?;
    println!("{} from create-vite {} ({} files)", template.id(), template.version, source.file_count());

    let answers = [("projectName".to_string(), serde_json::json!(args[3]))].into_iter().collect();
    let planned = plan(&template.manifest()?, &source, &answers, PlanOptions::new(&args[2]))?;
    for w in &planned.warnings {
        println!("warning: {w:?}");
    }
    let outcome = apply(&planned, |_| {})?;
    println!("{} -> {outcome:?}", planned.target.display());
    Ok(())
}
