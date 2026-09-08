use std::{env, process::ExitCode};

use ruddy_cli::stderr_color;

fn main() -> ExitCode {
    match ruddy_cli::run(env::args_os().skip(1), ".") {
        Ok(ruddy_cli::Outcome::Created(path)) => {
            println!("Created Ruddy project `{}`", path.display());
            ExitCode::SUCCESS
        }
        Ok(ruddy_cli::Outcome::Built(path)) => {
            println!("Built `{}`", path.display());
            ExitCode::SUCCESS
        }
        Ok(ruddy_cli::Outcome::Cleaned(path)) => {
            println!("Cleaned `{}`", path.display());
            ExitCode::SUCCESS
        }
        Ok(ruddy_cli::Outcome::Checked(path)) => {
            println!("Checked `{}`", path.display());
            ExitCode::SUCCESS
        }
        Ok(ruddy_cli::Outcome::Ran(_) | ruddy_cli::Outcome::Served) => ExitCode::SUCCESS,
        Ok(ruddy_cli::Outcome::Formatted(report)) => {
            for diagnostic in &report.diagnostics {
                eprintln!("{}", diagnostic.render(stderr_color()));
            }
            let (per_file, verb, changed) = if report.check {
                ("Would format", "Checked", "would change")
            } else {
                ("Formatted", "Formatted", "changed")
            };
            for path in &report.changed {
                println!("{per_file} `{}`", path.display());
            }
            let files = report.changed.len() + report.unchanged.len();
            let noun = if files == 1 { "file" } else { "files" };
            println!("{verb} {files} {noun}, {} {changed}", report.changed.len());
            if report.failed() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Ok(ruddy_cli::Outcome::FormattedStdin { text, diagnostics }) => {
            for diagnostic in &diagnostics {
                eprintln!("{}", diagnostic.render(stderr_color()));
            }
            print!("{text}");
            if diagnostics.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) if error.is_success() => {
            println!("{error}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code())
        }
    }
}
