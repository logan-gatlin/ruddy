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
        Ok(ruddy_cli::Outcome::Ran(_)) => ExitCode::SUCCESS,
        Ok(ruddy_cli::Outcome::Formatted(report)) => {
            for diagnostic in &report.diagnostics {
                eprintln!("{}", diagnostic.render(stderr_color()));
            }
            for path in &report.changed {
                println!(
                    "{} `{}`",
                    if report.check {
                        "Would format"
                    } else {
                        "Formatted"
                    },
                    path.display()
                );
            }
            let files = report.changed.len() + report.unchanged.len();
            println!(
                "{} {} {}, {} {}",
                if report.check { "Checked" } else { "Formatted" },
                files,
                if files == 1 { "file" } else { "files" },
                report.changed.len(),
                if report.check {
                    "would change"
                } else {
                    "changed"
                }
            );
            if report.failed() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Ok(ruddy_cli::Outcome::FormattedStdin { errors }) => {
            if errors {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
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
