//! Packs a generator folder into one `pack.v1.json`.
//!
//!     cargo run --example pack -- <generator folder> <revision> <output file>

use forge_scaffold::{pack_dir, PackOptions};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: pack <generator folder> <revision> <output file>");
        std::process::exit(2);
    }
    let Ok(revision) = args[1].parse::<u32>() else {
        eprintln!("the revision must be a whole number of 1 or more");
        std::process::exit(2);
    };
    match pack_dir(&PathBuf::from(&args[0]), &PackOptions::new(revision)) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&args[2], &json) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!("{} ({} bytes)", args[2], json.len());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
