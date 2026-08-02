//! The stage registry — the extension point of the whole tool.
//!
//! A stage is one function from the compiler's output to a [`Stage`] of
//! [`Node`](crate::wire::Node)s. Adding a panel to the debugger means adding a
//! file here and a line to [`REGISTRY`]; the page needs no change, because it
//! builds its tabs, filters and cross-highlighting from the snapshot alone.

pub mod ast;
pub mod constraints;
pub mod ir;
pub mod solve;
pub mod symbols;
pub mod tokens;
pub mod types;

use std::{collections::HashMap, rc::Rc};

use ruddy::{
    parse::Stmt,
    symbol::{Mint, Symbol},
    token::Token,
    types::Ty,
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

/// Everything about a panel that does not depend on what the compiler produced.
/// A stage describes itself from here whether it ran, skipped or panicked, so
/// the three outcomes cannot disagree about what the stage *is* — which is what
/// the page reads to decide whether it owns a tab at all.
pub struct Spec {
    pub id: &'static str,
    pub title: &'static str,
    pub view: View,
    /// The runs of this stage's row text that name the same thing wherever
    /// they appear, as a regular expression. See [`Stage::highlight`].
    pub highlight: Option<&'static str>,
    /// The stage whose rows this one paints badges onto. A stage naming another
    /// owns no tab of its own.
    pub annotates: Option<&'static str>,
    pub build: Build,
}

/// What a stage is built from. A panel needs the compiler's output alone; an
/// annotator needs the [`Trace`] the stage it annotates published, because the
/// ids it paints onto are that stage's rather than ids of its own.
#[derive(Clone, Copy)]
pub enum Build {
    Panel(fn(&Spec, &Cx) -> Stage),
    /// A panel that also publishes a trace, for the annotator registered
    /// against it.
    Traced(fn(&Spec, &Cx) -> (Stage, Trace)),
    Annotator(fn(&Spec, &Cx, &Trace) -> Stage),
}

/// The rows of a stage paired with what the walk that laid them down built them
/// from. An annotator reads this instead of mirroring that walk: the two cannot
/// drift, because there is only one of them.
#[derive(Default)]
pub struct Trace {
    /// One id per declaration row, in the order the stage renders them: every
    /// `type` declaration, then every `let`.
    pub decls: Vec<u32>,
    /// One entry per term row, with the type of the term it stands for.
    pub terms: Vec<(u32, Rc<Ty>)>,
}

/// How the type printer spells the things that are *not* concrete types: a
/// solver variable as `?4`, an undecided type as `?`, and a quantified one as
/// `'a`. Everything else a type is made of — `Nat`, `->`, a struct's braces and
/// field names — is written by the language rather than stood in for.
///
/// Declared once because three tabs render the same type language, and a
/// pattern that drifted on one of them would go quietly dead there.
const VARIABLES: &str = r"\?\d*|'[a-z]\d*";

/// Every stage, in tab order. Adding a panel is one line here. A stage that
/// annotates another owns no tab, so its place in the list does not matter to
/// the page; the annotators sit at the end to keep the tab order readable.
pub const REGISTRY: &[Spec] = &[
    Spec {
        id: "tokens",
        title: "Tokens",
        view: View::List,
        highlight: None,
        annotates: None,
        build: Build::Panel(tokens::build),
    },
    Spec {
        id: "ast",
        title: "AST",
        view: View::Tree,
        highlight: None,
        annotates: None,
        build: Build::Panel(ast::build),
    },
    Spec {
        id: "ir",
        title: "IR",
        view: View::Tree,
        highlight: None,
        annotates: None,
        build: Build::Traced(ir::build),
    },
    Spec {
        id: "constraints",
        title: "Constraints",
        view: View::Tree,
        // A constraint set is read by following one variable through it.
        highlight: Some(VARIABLES),
        annotates: None,
        build: Build::Panel(constraints::build),
    },
    Spec {
        id: "solve",
        title: "Solve",
        view: View::Steps,
        // One variable followed through the solve is the whole point of
        // stepping it.
        highlight: Some(VARIABLES),
        annotates: None,
        build: Build::Panel(solve::build),
    },
    Spec {
        id: "types",
        title: "Types",
        view: View::Tree,
        // Schemes are mostly concrete, so the `'a`s are what the eye is
        // looking for.
        highlight: Some(VARIABLES),
        annotates: None,
        build: Build::Panel(types::build),
    },
    Spec {
        id: "symbols",
        title: "Symbols",
        view: View::List,
        highlight: None,
        annotates: None,
        build: Build::Panel(symbols::build),
    },
    Spec {
        id: "types-ir",
        title: "Types",
        view: View::Tree,
        highlight: None,
        annotates: Some("ir"),
        build: Build::Annotator(types::annotate),
    },
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

impl Spec {
    /// An empty panel wearing this stage's identity. Every outcome is built
    /// from here — the rows, the summary and the timing are filled in over the
    /// top with struct update syntax — so what a stage says about itself is
    /// written once, at registration, rather than once per outcome.
    pub fn stage(&self, status: Status, summary: impl Into<String>) -> Stage {
        Stage {
            id: self.id,
            title: self.title,
            view: self.view,
            highlight: self.highlight,
            status,
            summary: summary.into(),
            micros: 0,
            nodes: Vec::new(),
            text: None,
            debug: String::new(),
            annotates: self.annotates,
        }
    }
}

/// A stage whose input never arrived.
pub fn skipped(spec: &Spec, why: &str) -> Stage {
    spec.stage(Status::Skipped, why)
}

/// A stage that panicked while being built. The panic itself is reported once,
/// at the top level of the snapshot.
pub fn panicked(spec: &Spec) -> Stage {
    spec.stage(Status::Panicked, "panicked")
}
