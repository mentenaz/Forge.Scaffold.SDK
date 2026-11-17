//! Builds the archives and `index.json` for a set of templates.
//!
//!     cargo run --example release -- <out dir> <base url> [options] <generator folder>...
//!
//! Options:
//!     --upstream <spfx repo> <template>   add a template of the official SharePoint/spfx CLI
//!                                         (repeat for more templates)
//!     --overlay <folder>                  a folder with overlay.json: files that replace the
//!                                         images in upstream templates (your logo and icons)
//!
//! If `<out dir>/index.json` already exists it is read first, so unchanged
//! archives keep their revision and changed ones get the next one. Archives are
//! written before the index, so an upload in the same order never publishes an
//! index that points at a missing file.

use forge_scaffold::{build_release_from, ReleaseInput, ReleaseOptions, RemoteIndex};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: release <out dir> <base url> [--upstream <spfx repo> <template>] [--overlay <folder>] <generator folder>...");
        std::process::exit(2);
    }
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(&args[0]);
    let mut options = ReleaseOptions::new(&args[1]);
    let mut inputs: Vec<ReleaseInput> = Vec::new();

    let mut rest = args[2..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--upstream" => {
                let repo = rest.next().ok_or("--upstream needs <spfx repo> <template>")?;
                let template = rest.next().ok_or("--upstream needs <spfx repo> <template>")?;
                inputs.push(ReleaseInput::Upstream { repo: PathBuf::from(repo), template: template.clone() });
            }
            "--overlay" => {
                options.overlay = Some(PathBuf::from(rest.next().ok_or("--overlay needs a folder")?));
            }
            folder => inputs.push(ReleaseInput::Folder(PathBuf::from(folder))),
        }
    }
    if inputs.is_empty() {
        return Err("nothing to release: give a generator folder or --upstream".into());
    }

    let index_path = out.join("index.json");
    let previous = match std::fs::read(&index_path) {
        Ok(bytes) => Some(RemoteIndex::from_json(&bytes)?),
        Err(_) => None,
    };

    let release = build_release_from(&inputs, &options, previous.as_ref())?;

    std::fs::create_dir_all(&out)?;
    for archive in &release.archives {
        std::fs::write(out.join(&archive.file_name), &archive.bytes)?;
        println!("{} ({} bytes)", archive.file_name, archive.bytes.len());
    }
    std::fs::write(&index_path, release.index_json()?)?;
    println!("index.json ({} entries)", release.index.templates.len());
    Ok(())
}
