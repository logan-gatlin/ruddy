mod engine;
mod parser;
mod reporting;

pub use engine::Eng;
pub use engine::Source;
pub use parser::{lex_source, lex_text, parse_source, parse_text};
pub use reporting::{Diagnostic, DiagnosticSeverity, TextRange, TextSize};

fn main() -> std::io::Result<()> {
    let path = "demo.hc";
    let contents = std::fs::read_to_string(path)?;

    let db = Eng::default();
    let source = Source::new(&db, path.to_owned(), contents);
    let parsed = parse_text(&db, source);

    println!("{parsed:#?}");
    Ok(())
}
