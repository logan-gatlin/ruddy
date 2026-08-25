use std::{env, io::Write as _, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let project = match project_argument() {
        Ok(project) => project,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: ruddy [project-directory]");
            return ExitCode::FAILURE;
        }
    };

    let artifact = match ruddy_cli::compile(project) {
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

fn project_argument() -> Result<PathBuf, &'static str> {
    let mut args = env::args_os().skip(1);
    let project = args
        .next()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    if args.next().is_some() {
        return Err("expected at most one project directory");
    }
    Ok(project)
}
