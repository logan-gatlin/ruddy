//! A local webapp for debugging the `ruddy` compiler.
//!
//! Run it through `just dev`, which rebuilds and restarts it whenever the
//! compiler changes; `cargo run -p ruddy-debug` serves the same thing without
//! the supervision.

mod docs;
mod server;
mod snapshot;
mod stage;
mod watch;
mod wire;

use std::{env, path::PathBuf, process::ExitCode};

const DEFAULT_PORT: u16 = 7878;

/// Set by the supervisor so the server knows it may exit on a source change and
/// be started again.
const SUPERVISED: &str = "RUDDY_DEBUG_SUPERVISED";
/// The build number the supervisor is on, so a reconnecting page can tell that
/// the binary underneath it changed.
const BUILD: &str = "RUDDY_DEBUG_BUILD";
/// Path to the rustc output of a rebuild that failed, when the supervisor fell
/// back to the last good binary.
const BUILD_ERROR: &str = "RUDDY_DEBUG_BUILD_ERROR";

struct Args {
    port: u16,
    watch: Option<bool>,
    open: bool,
    doc: Option<String>,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: ruddy-debug [--port N] [--watch|--no-watch] [--open] [--doc NAME]");
            return ExitCode::FAILURE;
        }
    };

    // The binary may be running from a copy under `debug/.dev`, so every path
    // is resolved against the crate it was built from rather than the
    // executable or the working directory.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.parent().unwrap_or(&crate_dir).to_path_buf();
    let scratch = crate_dir.join("scratch");

    if let Err(err) = docs::ensure(&scratch, &repo.join("demo.hc")) {
        eprintln!("error: could not prepare {}: {err}", scratch.display());
        return ExitCode::FAILURE;
    }

    let cfg = server::Config {
        port: args.port,
        web: crate_dir.join("web"),
        scratch,
        doc: args.doc.filter(|name| docs::valid_name(name)),
        build: env::var(BUILD)
            .ok()
            .and_then(|build| build.parse().ok())
            .unwrap_or(1),
        build_error: build_error(),
        watch: args.watch.unwrap_or_else(|| env::var(SUPERVISED).is_ok()),
        watch_roots: vec![
            repo.join("src"),
            repo.join("Cargo.toml"),
            crate_dir.join("src"),
            crate_dir.join("Cargo.toml"),
        ],
    };

    if args.open {
        let url = format!("http://127.0.0.1:{}/", cfg.port);
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }

    match server::serve(cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        port: DEFAULT_PORT,
        watch: None,
        open: false,
        doc: None,
    };
    let mut rest = env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--port" => {
                let value = rest.next().ok_or("--port needs a number")?;
                args.port = value.parse().map_err(|_| format!("bad port: {value}"))?;
            }
            "--watch" => args.watch = Some(true),
            "--no-watch" => args.watch = Some(false),
            "--open" => args.open = true,
            "--doc" => args.doc = Some(rest.next().ok_or("--doc needs a name")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// The supervisor points at a log file rather than passing the text, because a
/// rustc error is bigger than an environment variable ought to be.
fn build_error() -> Option<String> {
    let path = env::var(BUILD_ERROR).ok().filter(|path| !path.is_empty())?;
    let text = std::fs::read_to_string(path).ok()?;
    match text.trim().is_empty() {
        true => None,
        false => Some(text),
    }
}
