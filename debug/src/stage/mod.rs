//! The stage registry — the extension point of the whole tool.
//!
//! A stage is one function from the compiler's output to a [`Stage`] of
//! [`Node`](crate::wire::Node)s. Adding a panel to the debugger means adding a
//! file here and a line to [`REGISTRY`]; the page needs no change, because it
//! builds its tabs, filters and cross-highlighting from the snapshot alone.

pub mod ast;
pub mod ir;
pub mod symbols;
pub mod tokens;
pub mod types;

use std::collections::HashMap;

use ruddy::{
    parse::Stmt,
    symbol::{Mint, Symbol},
    token::Token,
};

use crate::wire::{Stage, Status, View};

/// Everything a stage may look at. Each field is `None` when the phase that
/// would have produced it did not run, which is what a stage reports as
/// [`Status::Skipped`] rather than rendering an empty panel.
pub struct Cx<'a> {
    /// Unused by the stages that exist today, but a stage rendering text — an
    /// assembly listing, a formatted source — starts from it.
    #[allow(dead_code)]
    pub source: &'a str,
    pub tokens: Option<&'a [Token]>,
    pub stmts: Option<&'a [Stmt]>,
    pub program: Option<&'a ruddy::ir::Program>,
    pub inference: Option<&'a ruddy::inference::Output>,
    pub mint: Option<&'a Mint>,
    /// Stable index per symbol, so a node can point at a row of the symbols
    /// stage and the page can highlight every occurrence of one symbol.
    pub symbols: &'a HashMap<Symbol, u32>,
    pub micros: Phases,
    /// Whether an earlier phase reported errors, which downgrades an otherwise
    /// fine stage to [`Status::Partial`].
    pub errored: bool,
}

/// How long each compiler phase took, in microseconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct Phases {
    pub lex: u64,
    pub parse: u64,
    pub build: u64,
    pub infer: u64,
}

/// What every stage is: one function from the compiler's output to a panel.
pub type Builder = fn(&Cx) -> Stage;

/// Every stage, in tab order. Adding a panel is one line here. A stage that
/// annotates another owns no tab, so its place in the list does not matter to
/// the page; the annotators sit at the end to keep the tab order readable.
pub const REGISTRY: &[(&str, Builder)] = &[
    ("tokens", tokens::build),
    ("ast", ast::build),
    ("ir", ir::build),
    ("types", types::build),
    ("symbols", symbols::build),
    ("types-ir", types::annotate),
];

/// Hands out node ids that are unique within one stage.
#[derive(Default)]
pub struct Ids(u32);

impl Ids {
    // A counter, not an iterator: it never runs out, so there is nothing for
    // `Iterator::next` to return `None` for.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        let id = self.0;
        self.0 += 1;
        id
    }
}

impl Cx<'_> {
    /// The status a stage that produced output should report.
    pub fn status(&self) -> Status {
        match self.errored {
            true => Status::Partial,
            false => Status::Ok,
        }
    }
}

/// A stage whose input never arrived.
pub fn skipped(id: &'static str, title: &'static str, view: View, why: &str) -> Stage {
    Stage {
        id,
        title,
        view,
        status: Status::Skipped,
        summary: why.to_string(),
        micros: 0,
        nodes: Vec::new(),
        text: None,
        display: String::new(),
        debug: String::new(),
        annotates: None,
    }
}

/// A stage that panicked while being built. The panic itself is reported once,
/// at the top level of the snapshot.
pub fn panicked(id: &'static str, title: &'static str, view: View) -> Stage {
    Stage {
        id,
        title,
        view,
        status: Status::Panicked,
        summary: "panicked".to_string(),
        micros: 0,
        nodes: Vec::new(),
        text: None,
        display: String::new(),
        debug: String::new(),
        annotates: None,
    }
}

/// Look up the stage title registered for an id, for messages that name a
/// stage the page has not received yet.
pub fn title_of(id: &str) -> &'static str {
    match id {
        "tokens" => "Tokens",
        "ast" => "AST",
        "ir" => "IR",
        "types" | "types-ir" => "Types",
        "symbols" => "Symbols",
        _ => "Stage",
    }
}
