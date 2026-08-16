use std::process::ExitCode;

use ruddy::{
    bundle::{self, Disk, Files},
    inference, ir, lir, patterns,
    symbol::Mint,
    tracking::{FileManager, Span},
    ui,
};

/// The bundle root, read from the working directory. The directory holding it
/// is what every module's file is looked for under.
const DEMO_PATH: &str = "demo.hc";

fn main() -> ExitCode {
    // The whole bundle comes off the disk beside `demo.hc`, so a `module A`
    // written in it is looked for at `A.hc` or `A/module.hc` the way it would
    // be anywhere else.
    let fs = Disk::new(".");
    if fs.read(DEMO_PATH).is_none() {
        eprintln!("error: could not read {DEMO_PATH}");
        return ExitCode::FAILURE;
    }

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &fs, DEMO_PATH);

    // Neither the parse tree nor the lowered program is printed here. Rendering
    // a tree back as surface syntax lives in the debugger, which this crate
    // cannot depend on without a cycle — run `ruddy-debug` and open the AST or
    // IR tab to read either one. What is left is what a compiler driver owes
    // its caller regardless: the diagnostics below.

    // The mint lives as long as the symbols it hands out, which for a driver
    // means the rest of the run. The identity is the root file's own, or the
    // fallback when it had none the loader could use — the later phases still
    // run either way, so one missing line does not hide every other complaint.
    let mut mint = Mint::new(loaded.bundle.clone().unwrap_or_else(bundle::fallback));
    let mut built = ir::build(&mut mint, loaded.stmts.clone());
    let inferred = inference::infer(&mint, &mut built.program);
    let checked = patterns::check(&built.program, &inferred);

    let read: usize = loaded
        .loaded
        .iter()
        .map(|file| file.lex_errors.len() + file.parse_errors.len())
        .sum();
    let errors = loaded.errors.len()
        + read
        + built.errors.len()
        + inferred.errors.len()
        + checked.errors.len();
    if errors == 0 {
        // LIR runs only here, on a program every earlier phase accepted — which
        // is what lets it be infallible. Nothing of it is printed: the driver
        // prints diagnostics, and lowering has none. Read the listing in the
        // debugger's LIR tab.
        let _lowered = lir::lower(&mint, &built.program, &inferred);
        println!("built successfully with no errors");
        return ExitCode::SUCCESS;
    }

    // Every phase's errors are worded by `ruddy::ui` and printed here, so that
    // this driver and the debugger's diagnostic strip cannot describe the same
    // program differently. What is left to a reporter is layout: the order, the
    // indentation, and how a span is quoted.
    //
    // A span names the file it was written in, so a bundle of several is quoted
    // out of whichever one the complaint is about. The sources are read back
    // off the loader rather than off the disk a second time: what was compiled
    // is what is quoted.
    let sources: Vec<(String, String)> = loaded
        .loaded
        .iter()
        .map(|file| {
            let registered = files.get_file(file.id);
            (registered.path.clone(), registered.content.clone())
        })
        .collect();
    let at = |span: Span| {
        loaded
            .loaded
            .iter()
            .position(|file| file.id == span.file_id)
            .map(|index| &sources[index])
    };

    eprintln!("{errors} error(s):");
    for file in &loaded.loaded {
        for err in &file.lex_errors {
            report(at(err.span), err.span, &err.kind.to_string());
        }
        for err in &file.parse_errors {
            report(at(err.span), err.span, &err.to_string());
        }
    }
    for err in &loaded.errors {
        report(at(err.span), err.span, &err.kind.to_string());
    }
    for err in &built.errors {
        report(at(err.span), err.span, &err.kind.to_string());
        // A repeat is only legible next to what it repeats, so the definition
        // that stands gets a line of its own, indented under the error. A
        // repeated parameter is the same thing said about a smaller scope, and
        // carries the same second span for the same reason.
        if let Some((previous, note)) = elsewhere(&err.kind) {
            report(at(previous), previous, &format!("  {note}"));
        }
    }
    for err in &inferred.errors {
        report(at(err.span), err.span, &err.kind.to_string());
        // A broken promise is only legible next to the promise, which is on
        // another line: the first use of the variable gets a line
        // of its own, indented under the error, exactly as a repeat's first
        // definition does.
        if let Some((declared, note)) = promised(&err.kind) {
            report(at(declared), declared, &format!("  {note}"));
        }
    }
    for err in &checked.errors {
        report(at(err.span), err.span, &err.kind.to_string());
    }
    ExitCode::FAILURE
}

/// The second place a complaint points at, and what to call it: where the name
/// a repeat repeats was first written, or where the `..` a tail clashes with
/// was first used. Every kind that carries such a span is listed, and a
/// reporter that renders one and not the others tells the reader half of what
/// the compiler knows — see [`ui::FIRST_DEFINITION`], [`ui::FIRST_DECLARATION`]
/// and [`ui::FIRST_USE`].
fn elsewhere(kind: &ir::ErrorKind) -> Option<(Span, &'static str)> {
    match kind {
        ir::ErrorKind::Duplicate { previous, .. }
        | ir::ErrorKind::DuplicateParameter { previous } => Some((*previous, ui::FIRST_DEFINITION)),
        ir::ErrorKind::MixedTail { previous, .. } => Some((*previous, ui::FIRST_USE)),
        _ => None,
    }
}

/// Inference's second place: where the variable a body broke its
/// promise about was declared. [`elsewhere`]'s twin one phase later — see
/// [`ui::DECLARED_HERE`].
fn promised(kind: &inference::ErrorKind) -> Option<(Span, &'static str)> {
    match kind {
        inference::ErrorKind::RigidBroken { declared, .. }
        | inference::ErrorKind::RigidField { declared, .. } => Some((*declared, ui::DECLARED_HERE)),
        _ => None,
    }
}

/// Print a single diagnostic as `path:line:col: message: "snippet"`, resolving
/// the span's byte offset back to a human-readable position in the source. A
/// span with no width covers no characters to quote: it marks where something
/// the parser needed would have gone, so it is named rather than quoted.
///
/// A span whose file is not one the loader read — the generated file every
/// compiler-made span sits in — has no source to quote and no path to name, so
/// it is printed as the position it is.
fn report(file: Option<&(String, String)>, span: Span, message: &str) {
    let Some((path, source)) = file else {
        eprintln!("  {message}");
        return;
    };
    let (line, col) = line_col(source, span.start);
    match source.get(span.start..span.end()) {
        Some("") | None => eprintln!("  {path}:{line}:{col}: {message}: end of input"),
        Some(snippet) => eprintln!("  {path}:{line}:{col}: {message}: {snippet:?}"),
    }
}

/// Convert a byte offset into a 1-based `(line, column)` pair.
fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
