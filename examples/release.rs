//! Builds the archives and `index.json` for a set of generator folders.
//!
//!     cargo run --example release -- <out dir> <base url> <generator folder>...
//!
//! If `<out dir>/index.json` already exists it is read first, so unchanged
//! archives keep their revision and changed ones get the next one. Archives are
//! written before the index, so an upload in the same order never publishes an
//! index that points at a missing file.

use forge_scaffold::{build_release, RemoteIndex, ReleaseOptions};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: release <out dir> <base url> <generator folder>...");
        std::process::exit(2);
    }
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(&args[0]);
    let generators: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();

    let index_path = out.join("index.json");
    let previous = match std::fs::read(&index_path) {
        Ok(bytes) => Some(RemoteIndex::from_json(&bytes)?),
        Err(_) => None,
    };

    let release = build_release(&generators, &ReleaseOptions::new(&args[1]), previous.as_ref())?;

    std::fs::create_dir_all(&out)?;
    for archive in &release.archives {
        std::fs::write(out.join(&archive.file_name), &archive.bytes)?;
        println!("{} ({} bytes)", archive.file_name, archive.bytes.len());
    }
    std::fs::write(&index_path, release.index_json()?)?;
    println!("index.json ({} entries)", release.index.templates.len());
    Ok(())
}
