//! A stack-safe recognizer for ECMAScript modules.
//!
//! The JavaScript backend validates every module it generates and every
//! extern target it embeds, so a generation bug surfaces as a compile error
//! rather than as a runtime one. The parsers available for that are
//! recursive descent, and so read no deeper than the native stack allows;
//! this crate parses with an explicit stack instead, so that the depth of
//! what it can validate is bounded by memory alone. It follows oxc's grammar
//! coverage and token vocabulary, but builds no tree: the answer is whether
//! the source is a module, and what its top-level items are.
//!
//! Modules are strict code, and the recognizer enforces the syntactic
//! strict-mode rules along with the grammar: no `with`, no legacy octal, no
//! deleting a variable. Early errors that need names resolved — duplicate
//! bindings, an undeclared label, a `break` outside a loop — are not checked,
//! and regular expression bodies are scanned for shape but not compiled.

use std::{error, fmt};

mod lexer;
mod parser;

/// A place in the source the recognizer could not read past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
    offset: usize,
    line: usize,
    column: usize,
}

/// One top-level item of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Import,
    /// An `export default` of an expression, function or class.
    ExportDefault,
    /// Any other export: a list, a re-export, or an exported declaration.
    Export,
    Statement,
}

/// What a module is made of, at the top level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    items: Vec<Item>,
}

impl Error {
    pub(crate) fn at(source: &str, message: &str, offset: usize) -> Self {
        let before = &source[..offset.min(source.len())];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count())
            + 1;
        Self {
            message: message.to_string(),
            offset,
            line,
            column,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The byte offset of the token the error is about.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// One-based line and column of that token.
    pub fn position(&self) -> (usize, usize) {
        (self.line, self.column)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl error::Error for Error {}

impl Module {
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

/// Read `source` as an ECMAScript module.
pub fn parse_module(source: &str) -> Result<Module, Error> {
    let items = parser::Parser::new(source)?.parse_module()?;
    Ok(Module { items })
}
