//! Scaffolds a solution from a template of the official SharePoint/spfx CLI.
//!
//!     cargo run --example upstream -- <spfx repo> <template> <location> <component name> [<library name>]
//!
//! e.g. `... -- E:\mentenaz\spfx webpart-react C:\work "Hello World"`

use forge_scaffold::{apply, plan, DirSource, PlanOptions, UpstreamInputs, UpstreamSource};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 || args.len() > 5 {
        eprintln!("usage: upstream <spfx repo> <template> <location> <component name> [<library name>]");
        std::process::exit(2);
    }
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> forge_scaffold::Result<()> {
    let repo = DirSource::new(&args[0]);
    let (template, source) = UpstreamSource::load(&repo, &format!("templates/{}", args[1]))?;
    println!("{} for SPFx {} ({} files)", template.name, template.spfx_version, source.file_count());

    let inputs = UpstreamInputs {
        component_name: args[3].clone(),
        library_name: args.get(4).cloned(),
        ..Default::default()
    };
    let planned = plan(&template.manifest()?, &source, &inputs.answers(), PlanOptions::new(&args[2]))?;
    for w in &planned.warnings {
        println!("warning: {w:?}");
    }
    let outcome = apply(&planned, |_| {})?;
    println!("{} -> {outcome:?}", planned.target.display());
    Ok(())
}
