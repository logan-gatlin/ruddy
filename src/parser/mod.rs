pub mod ast;
mod grammar;
mod lexer;
pub mod token;

use crate::engine::Source;
use crate::reporting::{Diag, Diagnostic, TextSize};

pub use ast::*;
pub use token::Token;

pub fn lex_text(db: &dyn salsa::Database, source: Source) -> LexedSource {
    lex_source(db, source)
}

pub fn parse_text(db: &dyn salsa::Database, source: Source) -> ParsedSource {
    parse_source(db, source)
}

pub fn lex_diagnostics(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    lex_source::accumulated::<Diag>(db, source)
        .into_iter()
        .map(|diag| diag.0.clone())
        .collect()
}

pub fn parse_diagnostics(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    parse_source::accumulated::<Diag>(db, source)
        .into_iter()
        .map(|diag| diag.0.clone())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct LexedSource {
    pub tokens: Vec<Token>,
}

impl LexedSource {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct ParsedSource {
    pub tokens: Vec<Token>,
    pub ast: AstFile,
}

impl ParsedSource {
    pub fn new(tokens: Vec<Token>, ast: AstFile) -> Self {
        Self { tokens, ast }
    }
}

#[salsa::tracked]
pub fn lex_source(db: &dyn salsa::Database, source: Source) -> LexedSource {
    let contents = source.contents(db);
    let tokens = lexer::lex(db, contents);
    LexedSource::new(tokens)
}

#[salsa::tracked]
pub fn parse_source(db: &dyn salsa::Database, source: Source) -> ParsedSource {
    let source_len = TextSize::from_usize(source.contents(db).len());
    let lexed = lex_source(db, source);
    let tokens = lexed.tokens;
    let parsed_file = grammar::parse_file(db, &tokens, source_len);
    ParsedSource::new(tokens, parsed_file.file)
}

#[cfg(test)]
mod tests;
