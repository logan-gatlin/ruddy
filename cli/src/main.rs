use std::{env, io::Write as _, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let root = match root_argument() {
        Ok(root) => root,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: cli <bundle-root.hc>");
            return ExitCode::FAILURE;
        }
    };

    let artifact = match ruddy_cli::compile(root) {
        Ok(artifact) => artifact,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::io::stdout().write_all(artifact.print().as_bytes()) {
        eprintln!("error: could not write artifact: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn root_argument() -> Result<PathBuf, &'static str> {
    let mut args = env::args_os().skip(1);
    let root = args.next().ok_or("a bundle root file is required")?;
    if args.next().is_some() {
        return Err("expected exactly one bundle root file");
    }
    Ok(root.into())
}
