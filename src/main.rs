use std::process::ExitCode;

use ruddy::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::{FileManager, Span},
};

const DEMO_PATH: &str = "demo.hc";

const DEMO_BUNDLE: &str = "demo";
const DEMO_VERSION: Version = Version::new(0, 1, 0);

fn main() -> ExitCode {
    let source = match std::fs::read_to_string(DEMO_PATH) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("error: could not read {DEMO_PATH}: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Register the source so its span offsets refer to a real file.
    let mut files = FileManager::new();
    let file_id = files.register_new_file(DEMO_PATH.to_string(), source.clone());

    let lexed = token::lex(&source, file_id);
    let parsed = parse::parse(lexed.tokens);

    // Neither the parse tree nor the lowered program is printed here. Rendering
    // a tree back as surface syntax lives in the debugger, which this crate
    // cannot depend on without a cycle — run `ruddy-debug` and open the AST or
    // IR tab to read either one. What is left is what a compiler driver owes
    // its caller regardless: the diagnostics below.

    // The mint lives as long as the symbols it hands out, which for a driver
    // means the rest of the run.
    let bundle = Bundle::new(DEMO_BUNDLE, DEMO_VERSION).expect("the demo bundle name is valid");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    let inferred = inference::infer(&mut built.program);

    let errors =
        lexed.errors.len() + parsed.errors.len() + built.errors.len() + inferred.errors.len();
    if errors == 0 {
        println!("built successfully with no errors");
        return ExitCode::SUCCESS;
    }

    eprintln!("{errors} error(s):");
    for err in &lexed.errors {
        report(
            &source,
            err.span,
            match err.kind {
                token::ErrorKind::Unrecognized => "unrecognized character",
                token::ErrorKind::MalformedNatural => "malformed natural number",
                token::ErrorKind::NaturalTooLarge => "natural number is too large",
            },
        );
    }
    for err in &parsed.errors {
        report(&source, err.span, "unexpected token");
    }
    for err in &built.errors {
        match &err.kind {
            ir::ErrorKind::Undefined { namespace } => {
                report(&source, err.span, &format!("undefined {namespace}"));
            }
            ir::ErrorKind::Duplicate {
                namespace,
                previous,
            } => {
                report(&source, err.span, &format!("duplicate {namespace}"));
                report(&source, *previous, "  first defined here");
            }
            ir::ErrorKind::DuplicateField => report(&source, err.span, "duplicate field"),
        }
    }
    // Inference words its own errors, so that this driver and the debugger's
    // diagnostic strip cannot describe the same program differently.
    for err in &inferred.errors {
        report(&source, err.span, &err.kind.to_string());
    }
    ExitCode::FAILURE
}

/// Print a single diagnostic as `path:line:col: message: "snippet"`, resolving
/// the span's byte offset back to a human-readable position in the source. A
/// span with no width covers no characters to quote: it marks where something
/// the parser needed would have gone, so it is named rather than quoted.
fn report(source: &str, span: Span, message: &str) {
    let (line, col) = line_col(source, span.start);
    match source.get(span.start..span.end()) {
        Some("") | None => eprintln!("  {DEMO_PATH}:{line}:{col}: {message}: end of input"),
        Some(snippet) => eprintln!("  {DEMO_PATH}:{line}:{col}: {message}: {snippet:?}"),
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
