pub mod ir;
pub mod parse;
pub mod symbol;
pub mod token;
pub mod tracking;

use std::process::ExitCode;

use symbol::{BundleId, Mint};
use tracking::{FileManager, Span};

const DEMO_PATH: &str = "demo.hc";

/// The demo is a single bundle, so it gets the first id.
const DEMO_BUNDLE: BundleId = BundleId::new(0);

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

    println!("== {DEMO_PATH} ==\n");
    for stmt in &parsed.stmts {
        println!("{}", stmt.tracked);
    }

    // The mint lives as long as the symbols it hands out, which for a driver
    // means the rest of the run.
    let mint: Mint = Mint::new(DEMO_BUNDLE, "demo");
    let built = ir::build(&mint, parsed.stmts);

    println!("\n== ir ==\n");
    println!("{}", built.program);

    let errors = lexed.errors.len() + parsed.errors.len() + built.errors.len();
    if errors == 0 {
        println!("\nbuilt successfully with no errors");
        return ExitCode::SUCCESS;
    }

    eprintln!("\n{errors} error(s):");
    for err in &lexed.errors {
        report(&source, err.span, "unrecognized character");
    }
    for err in &parsed.errors {
        report(&source, err.span, "unexpected token");
    }
    for err in &built.errors {
        report(&source, err.span, "duplicate name");
    }
    ExitCode::FAILURE
}

/// Print a single diagnostic as `path:line:col: message: "snippet"`, resolving
/// the span's byte offset back to a human-readable position in the source.
fn report(source: &str, span: Span, message: &str) {
    let (line, col) = line_col(source, span.start);
    let snippet = source.get(span.start..span.end()).unwrap_or("<eof>");
    eprintln!("  {DEMO_PATH}:{line}:{col}: {message}: {snippet:?}");
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
