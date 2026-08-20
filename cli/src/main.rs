use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ruddy::{
    bundle::{self, Disk, Files},
    inference, ir, lir, patterns,
    symbol::Mint,
    tracking::{FileManager, Span},
};

fn main() -> ExitCode {
    let root = match root_argument() {
        Ok(root) => root,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: cli <bundle-root.hc>");
            return ExitCode::FAILURE;
        }
    };

    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
        eprintln!("error: bundle root must have a UTF-8 file name");
        return ExitCode::FAILURE;
    };

    let disk = Disk::new(parent);
    if disk.read(name).is_none() {
        eprintln!("error: could not read {}", root.display());
        return ExitCode::FAILURE;
    }

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &disk, name);
    let mut mint = Mint::new(loaded.bundle.clone().unwrap_or_else(bundle::fallback));
    let mut built = ir::build(&mut mint, loaded.stmts);
    let inferred = inference::infer(&mint, &mut built.program);
    let checked = patterns::check(&built.program, &inferred);

    let errors = loaded
        .loaded
        .iter()
        .map(|file| file.lex_errors.len() + file.parse_errors.len())
        .sum::<usize>()
        + loaded.errors.len()
        + built.errors.len()
        + inferred.errors.len()
        + checked.errors.len();

    // LIR has no diagnostics of its own, but reaching it verifies the final
    // compiler phase without producing an artifact.
    if errors == 0 {
        let _ = lir::lower(&mint, &built.program, &inferred);
    }

    for file in &loaded.loaded {
        for error in &file.lex_errors {
            report(
                &mut files,
                "lex",
                error.kind.code(),
                error.span,
                &error.kind,
            );
        }
        for error in &file.parse_errors {
            report(&mut files, "parse", error.code(), error.span, error);
        }
    }
    for error in &loaded.errors {
        report(
            &mut files,
            "bundle",
            error.kind.code(),
            error.span,
            &error.kind,
        );
    }
    for error in &built.errors {
        report(&mut files, "ir", error.kind.code(), error.span, &error.kind);
    }
    for error in &inferred.errors {
        report(
            &mut files,
            "types",
            error.kind.code(),
            error.span,
            &error.kind,
        );
    }
    for error in &checked.errors {
        report(
            &mut files,
            "patterns",
            error.kind.code(),
            error.span,
            &error.kind,
        );
    }

    if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn root_argument() -> Result<PathBuf, &'static str> {
    let mut args = env::args_os().skip(1);
    let root = args.next().ok_or("a bundle root file is required")?;
    if args.next().is_some() {
        return Err("expected exactly one bundle root file");
    }
    Ok(root.into())
}

fn report(
    files: &mut FileManager,
    phase: &str,
    code: &str,
    span: Span,
    message: &impl std::fmt::Display,
) {
    let file = files.get_file(span.file_id);
    let before = file.content.get(..span.start).unwrap_or(&file.content);
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    eprintln!(
        "{}:{line}:{column}: error[{phase}/{code}]: {message}",
        file.path
    );
}
