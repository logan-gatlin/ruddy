//! Read each argument — or the file it names, given as `@path` — as a
//! module with this crate and with oxc, and print both verdicts. For
//! settling what the grammar says about a snippet while working on the
//! recognizer:
//!
//! ```text
//! cargo run -p esparse --example check -- 'x = a ?? b || c;' @deep.js
//! ```
fn main() {
    for argument in std::env::args().skip(1) {
        let source = match argument.strip_prefix('@') {
            Some(path) => std::fs::read_to_string(path).unwrap(),
            None => argument.clone(),
        };
        let ours = match esparse::parse_module(&source) {
            Ok(module) => format!("ok {:?}", module.items()),
            Err(error) => format!("ERR {error}"),
        };
        let theirs = if source.len() > 20_000 {
            // oxc is recursive descent; deep input would overflow this thread.
            "skipped".to_string()
        } else {
            let allocator = oxc_allocator::Allocator::default();
            let parsed =
                oxc_parser::Parser::new(&allocator, &source, oxc_span::SourceType::mjs()).parse();
            let errors: Vec<String> = parsed.diagnostics.errors().map(|d| d.to_string()).collect();
            if parsed.panicked || !errors.is_empty() {
                format!("ERR {errors:?}")
            } else {
                "ok".to_string()
            }
        };
        let shown: String = source.chars().take(50).collect();
        println!("{shown:?}\n    esparse: {ours}\n    oxc:     {theirs}");
    }
}
