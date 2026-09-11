//! `ruddy-interp <artifact> [export...]`: run a linked artifact's initializers
//! and print the named exports as JSON, or list the exports when none are named.
//!
//! Everything this decides lives in [`ruddy_interp::run`]; here it is only
//! read from the command line and written to the terminal.

use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match ruddy_interp::run(&arguments) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(if arguments.is_empty() { 2 } else { 1 })
        }
    }
}
