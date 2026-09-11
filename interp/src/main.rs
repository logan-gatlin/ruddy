//! `ruddy-interp <artifact> [export...]`: run a linked artifact's initializers
//! and print the named exports as JSON, or list the exports when none are named.

use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: ruddy-interp <artifact> [export...]");
        return ExitCode::from(2);
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("could not read {path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let artifact = match ruddy::artifact::try_parse(&text)
        .map_err(|error| error.to_string())
        .and_then(|unchecked| unchecked.validate().map_err(|error| error.to_string()))
    {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("invalid artifact: {error}");
            return ExitCode::FAILURE;
        }
    };
    let program = match ruddy_interp::Program::load(&artifact) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let names: Vec<String> = args.collect();
    if names.is_empty() {
        for export in program.exports() {
            println!("{export}");
        }
        return ExitCode::SUCCESS;
    }
    for name in names {
        match program.export(&name) {
            Ok(value) => println!("{}", ruddy_interp::render::json(&value)),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
