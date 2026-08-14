//! The stage registry — the extension point of the whole tool.
//!
//! A stage is one function from the compiler's output to a [`Stage`] of
//! [`Node`](crate::wire::Node)s. Adding a panel to the debugger means adding a
//! file here and a line to [`REGISTRY`]; the page needs no change, because it
//! builds its tabs, filters and cross-highlighting from the snapshot alone.

pub mod ast;
pub mod constraints;
pub mod ir;
pub mod patterns;
pub mod presence;
pub mod solve;
pub mod symbols;
pub mod tokens;
pub mod types;

use std::{collections::HashMap, rc::Rc};

use ruddy::{
    ir::Param,
    parse::Stmt,
    symbol::{Mint, Symbol},
    token::Token,
    types::{Shape, Ty},
    ui,
};

use crate::wire::{Node, Stage, Status, View};

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
    pub patterns: Option<&'a ruddy::patterns::Output>,
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
    pub patterns: u64,
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
    /// Whether those runs only name the same thing within one row. See
    /// [`Stage::scoped`].
    pub scoped: bool,
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
/// solver variable as `?4`, and an undecided type as `?`. Everything else a
/// type is made of — `Nat`, `->`, a struct's braces and field names — is
/// written by the language rather than stood in for.
///
/// A variable a scheme quantifies is deliberately not here. It prints as a
/// bare letter now, the way a `where let` declares it, and a bare letter is
/// exactly what an ordinary name looks like: a pattern matching one would light
/// up every field called `a` on the page. What tells a reader which letters a
/// scheme quantifies is the `where let` clause printed beside the type, which
/// is a better answer than a highlight anyway — it is the source they could
/// paste back.
///
/// The `?` an abandoned presence puts on its label — the one in `a?: Nat` — is
/// the language too, not a stand-in, and the `\B` is what keeps the highlighter
/// off it: that `?` always follows its label's last character, while a `?`
/// that begins a variable follows punctuation or space.
///
/// Declared once because four tabs render the same type language, and a
/// pattern that drifted on one of them would go quietly dead there.
const VARIABLES: &str = r"\B\?\d*";

/// Every stage, in tab order. Adding a panel is one line here. A stage that
/// annotates another owns no tab, so its place in the list does not matter to
/// the page; the annotators sit at the end to keep the tab order readable.
pub const REGISTRY: &[Spec] = &[
    Spec {
        id: "tokens",
        title: "Tokens",
        view: View::List,
        highlight: None,
        scoped: false,
        annotates: None,
        build: Build::Panel(tokens::build),
    },
    Spec {
        id: "ast",
        title: "AST",
        view: View::Tree,
        highlight: None,
        scoped: false,
        annotates: None,
        build: Build::Panel(ast::build),
    },
    Spec {
        id: "ir",
        title: "IR",
        view: View::Tree,
        highlight: None,
        scoped: false,
        annotates: None,
        build: Build::Traced(ir::build),
    },
    Spec {
        id: "constraints",
        title: "Constraints",
        view: View::Tree,
        // A constraint set is read by following one variable through it.
        highlight: Some(VARIABLES),
        scoped: false,
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
        scoped: false,
        annotates: None,
        build: Build::Panel(solve::build),
    },
    Spec {
        id: "types",
        title: "Types",
        view: View::Tree,
        // A scheme's own letters are not lit — they are spelled exactly as
        // names are, and the `where let` beside the type is what declares
        // them — so what is left to follow is the solver's own variables, and
        // only inside one scheme. Generalization numbers each definition's
        // quantifiers from scratch, so two rows spelling `?3` in the *types*
        // tab are two unrelated variables that happen to look alike.
        highlight: Some(VARIABLES),
        scoped: true,
        annotates: None,
        build: Build::Panel(types::build),
    },
    Spec {
        id: "presence",
        title: "Presence",
        view: View::Tree,
        // A store is read by following one presence variable through it: which
        // batch first mentions it, what the verdict was once it did, and which
        // scheme it ended up in. Not scoped, unlike the schemes above: a
        // presence variable in the store is the solver's own numbering, unique
        // across the whole program, so two rows spelling `?7` really are one
        // variable.
        highlight: Some(VARIABLES),
        scoped: false,
        annotates: None,
        build: Build::Panel(presence::build),
    },
    Spec {
        id: "patterns",
        title: "Patterns",
        view: View::Tree,
        // The solved scrutinee types render in the type language, so the
        // variables light up the way the Types tab's do — and scope the same
        // way, since each match's types were zonked with its own definition.
        highlight: Some(VARIABLES),
        scoped: true,
        annotates: None,
        build: Build::Panel(patterns::build),
    },
    Spec {
        id: "symbols",
        title: "Symbols",
        view: View::List,
        highlight: None,
        scoped: false,
        annotates: None,
        build: Build::Panel(symbols::build),
    },
    Spec {
        id: "types-ir",
        title: "Types",
        view: View::Tree,
        highlight: None,
        scoped: false,
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
            scoped: self.scoped,
            status,
            summary: summary.into(),
            micros: None,
            nodes: Vec::new(),
            text: None,
            debug: String::new(),
            annotates: self.annotates,
        }
    }
}

/// `1 step`, `4 steps` — a count and its noun, agreeing. Every stage summary is
/// a phrase a person reads off the pane bar, and "1 constraints" in a tool whose
/// whole subject is getting the details right is a poor advertisement for it.
/// Written once because four stages counted something and three of them had
/// spelled this out again.
pub fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

/// What one parameter of a `type` declaration stands for, as a row of the IR
/// and Types tabs reads it: the name it was written as, a `..` in front of it
/// when it tails a row, which of the two readings it has, and — when there is
/// one — the labels an argument written there may not name.
///
/// Both tabs show a parameter and neither can show one usefully without this:
/// the meaning column spells every parameter as a bare letter alike, and which
/// of them tails
/// something, what kind, and what may not be in it, is nowhere else on the page.
/// Written once so the two cannot spell it differently.
///
/// A parameter that tails a *struct* stands for a whole type, so it is shown as
/// one — with the fields it may not name beside it, since that is the whole of
/// what its `..` still demands. Only a sum's rest is a reading of its own.
///
/// The labels are spelled the way their own shape spells them — a field bare, a
/// case with its backtick — through [`ui::label`], so this row and the
/// compiler's own complaint about the same parameter name it the same way.
pub fn stands_for(mint: &Mint, param: &Param) -> String {
    let name = mint.name(param.symbol);
    let lacks = param.kind.lacks();
    let (opener, shape) = match param.kind.cases() {
        Some(_) => (format!("..{name} (sum)"), Shape::Sum),
        None if lacks.is_empty() => return name.to_string(),
        None => (format!("..{name} (struct)"), Shape::Struct),
    };
    if lacks.is_empty() {
        return opener;
    }
    let labels: Vec<String> = lacks.iter().map(|label| ui::label(shape, label)).collect();
    format!("{opener} without {}", labels.join(", "))
}

/// Attach a symbol to the node that stands for it. A source name is an
/// occurrence — the page paints the node's span in the editor as a use of the
/// name — while the compiler's own `%`-named fresh symbols, the hidden
/// definitions a pattern `let` or a wildcard mints, are spelled nowhere in
/// the source: a node about one gets the association without the span. See
/// [`Node::symbol`] and [`Node::owner`], whose contract this is; written once
/// so no two stages can read it differently.
pub fn with_symbol(node: Node, cx: &Cx, mint: &Mint, symbol: Symbol) -> Node {
    match cx.symbols.get(&symbol) {
        Some(index) if mint.name(symbol).starts_with('%') => node.owner(*index),
        Some(index) => node.symbol(*index),
        None => node,
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
