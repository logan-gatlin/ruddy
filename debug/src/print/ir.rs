//! Rendering the lowered program as surface syntax.
//!
//! Unlike [`ast`](super::ast), the IR names things by [`Symbol`](ruddy::symbol::Symbol)
//! rather than by the string that was written, so every node here needs a
//! [`Mint`] alongside it to print at all. That, and the orphan rule, is why the
//! wrapper exists.

use std::fmt;

use indexmap::IndexMap;
use ruddy::{
    ir::{Field, Program, Term, TermKind, TypeKind},
    symbol::Mint,
    tracking::Tracked,
};

use crate::print::{Grouped, Prec, write_apply, write_arrow, write_project};

/// Pairs a node with the mint that can name its symbols. Printing an IR node
/// needs both, and going through one wrapper is what lets the node implement
/// [`Grouped`] and so share the parser's grouping rules unchanged.
struct Show<'a, T> {
    node: &'a T,
    mint: &'a Mint,
}

/// An IR node the printer can reach the kind of.
///
/// A term and a type wrap their kind in different bookkeeping — a type carries
/// a span, a term carries a span and its type — and none of that bookkeeping is
/// printed, since the IR prints as surface syntax. So the printers are generic
/// over this rather than over [`Tracked`]: what they need from a node is its
/// kind, not the shape of the wrapper around it.
trait Node {
    type Kind;

    fn kind(&self) -> &Self::Kind;
}

impl Node for Term {
    type Kind = TermKind;

    fn kind(&self) -> &TermKind {
        &self.kind
    }
}

impl<T> Node for Tracked<T> {
    type Kind = T;

    fn kind(&self) -> &T {
        &self.tracked
    }
}

impl<'a, T> Show<'a, T> {
    /// Point the printer at a child node, keeping the mint. Takes the node
    /// rather than its kind so that a call site reads the same either way.
    fn show<N: Node>(&self, node: &'a N) -> Show<'a, N::Kind> {
        Show {
            node: node.kind(),
            mint: self.mint,
        }
    }
}

impl fmt::Display for Show<'_, Program> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Terms and types live in separate maps, so the original interleaving
        // in the source is not recoverable — but there is nothing to recover,
        // because lowering hoists types the same way this prints them: every
        // type in declaration order, then every term. Printing in the order the
        // builder lowers in is what makes a printed program re-lower into the
        // one it was printed from.
        let mut first = true;
        for (symbol, decl) in &self.node.types {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(
                f,
                "type {} = {}",
                self.mint.name(*symbol),
                self.show(&decl.value)
            )?;
        }
        for (symbol, decl) in &self.node.terms {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "let {}", self.mint.name(*symbol))?;
            if let Some(annotation) = &decl.annotation {
                write!(f, " : {}", self.show(annotation))?;
            }
            write!(f, " = {}", self.show(&decl.value))?;
        }
        Ok(())
    }
}

/// The IR prints as surface syntax, so it groups by the surface grammar's rules
/// — the same [`Prec`] ladder the parse tree's printer reads.
impl Grouped for Show<'_, TermKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TermKind::Fn { .. } => Prec::Lambda,
            TermKind::Apply { .. } => Prec::Apply,
            TermKind::Project { .. }
            | TermKind::Struct(_)
            | TermKind::Ident(_)
            | TermKind::Natural(_)
            | TermKind::Error => Prec::Atom,
        }
    }
}

impl Grouped for Show<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TypeKind::Arrow { .. } => Prec::Arrow,
            TypeKind::Struct(_) | TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {
                Prec::Atom
            }
        }
    }
}

impl fmt::Display for Show<'_, TermKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TermKind::Error => f.write_str("<error>"),
            TermKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            TermKind::Natural(value) => write!(f, "{value}"),
            TermKind::Apply { func, arg } => {
                write_apply(f, &self.show(&**func), &self.show(&**arg))
            }
            // Lowering curries multi-argument functions, so a nested `fn` per
            // argument is printed rather than the surface `fn a b => ...`.
            TermKind::Fn { arg, body } => write!(
                f,
                "fn {} => {}",
                self.mint.name(arg.tracked),
                self.show(&**body)
            ),
            TermKind::Struct(fields) => write_struct(f, self.mint, fields),
            TermKind::Project { base, field } => {
                write_project(f, &self.show(&**base), &field.tracked)
            }
        }
    }
}

impl fmt::Display for Show<'_, TypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TypeKind::Error => f.write_str("<error>"),
            TypeKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            TypeKind::Prim(prim) => f.write_str(prim.name()),
            TypeKind::Arrow { from, to } => write_arrow(f, &self.show(&**from), &self.show(&**to)),
            TypeKind::Struct(fields) => write_struct(f, self.mint, fields),
        }
    }
}

/// Render a whole program, every type in declaration order and then every term.
pub fn program<'a>(program: &'a Program, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show {
        node: program,
        mint,
    }
}

/// Render one term, for the tree view, which labels a node with the source it
/// stands for. Shares the printer with [`program`], so a subtree and the whole
/// program cannot disagree about how the same node reads.
pub fn term<'a>(kind: &'a TermKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}

/// Render one type, the [`term`] counterpart.
pub fn ty<'a>(kind: &'a TypeKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}

/// Render a `{ name: value, ... }` literal. Unlike the parser's equivalent the
/// name comes from the map key, so the field's own span is not printed.
fn write_struct<T>(
    f: &mut fmt::Formatter<'_>,
    mint: &Mint,
    fields: &IndexMap<String, Field<T>>,
) -> fmt::Result
where
    T: Node,
    for<'a> Show<'a, T::Kind>: fmt::Display,
{
    // The empty struct is unit, which reads as `{}` — the padding a struct with
    // fields gets would only be two spaces around nothing.
    if fields.is_empty() {
        return f.write_str("{}");
    }

    f.write_str("{ ")?;
    for (i, (name, field)) in fields.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(
            f,
            "{name}: {}",
            Show {
                node: field.value.kind(),
                mint
            }
        )?;
    }
    f.write_str(" }")
}
