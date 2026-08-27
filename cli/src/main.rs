use std::{env, process::ExitCode};

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
