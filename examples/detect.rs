//! Prints what `detect_solution` finds for a folder.
//!
//!     cargo run --example detect -- <folder>

use forge_scaffold::{detect_solution, DetectRule};
use std::path::PathBuf;

fn main() {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: detect <folder>");
        std::process::exit(2);
    };
    match detect_solution(&PathBuf::from(arg), &DetectRule::spfx()) {
        Ok(Some(s)) => println!(
            "solution\t{}\tversion={}\tframework={}\tbuild={:?}\tname={}",
            s.root.display(),
            s.version().unwrap_or("?"),
            s.framework().unwrap_or("?"),
            s.build_system(),
            s.solution_name().unwrap_or("?"),
        ),
        Ok(None) => println!("none"),
        Err(e) => {
            println!("error\t{e}");
            std::process::exit(1);
        }
    }
}
