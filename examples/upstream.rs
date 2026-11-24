//! Scaffolds a solution from a template of the official SharePoint/spfx CLI.
//!
//!     cargo run --example upstream -- <spfx repo> <template> <location> <component name> [<library name>] [--overlay <folder>]
//!
//! e.g. `... -- E:\mentenaz\spfx webpart-react C:\work "Hello World" --overlay E:\mentenaz\Forge.Scaffold.Templates\layers`
//!
//! `--overlay` points at a folder with an `overlay.json`: files that replace the
//! images of the template (your own logo and icons).

use forge_scaffold::{apply, plan, DirSource, Overlay, PlanOptions, UpstreamInputs, UpstreamSource};

fn main() {
    let mut positional: Vec<String> = Vec::new();
    let mut overlay: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--overlay" {
            overlay = args.next();
        } else {
            positional.push(arg);
        }
    }
    if positional.len() < 4 || positional.len() > 5 {
        eprintln!("usage: upstream <spfx repo> <template> <location> <component name> [<library name>] [--overlay <folder>]");
        std::process::exit(2);
    }
    if let Err(e) = run(&positional, overlay.as_deref()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String], overlay: Option<&str>) -> forge_scaffold::Result<()> {
    let repo = DirSource::new(&args[0]);
    let (template, mut source) = UpstreamSource::load(&repo, &format!("templates/{}", args[1]))?;
    println!("{} for SPFx {} ({} files)", template.name, template.spfx_version, source.file_count());

    if let Some(dir) = overlay {
        let overlay = Overlay::load(&DirSource::new(dir), "")?;
        let replaced = source.apply_overlay(&template, &overlay);
        source.apply_rewrites(&template, &overlay)?;
        println!("overlay replaced {replaced} files");
    }

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
