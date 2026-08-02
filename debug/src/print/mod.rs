//! Rendering compiler trees back as surface syntax.
//!
//! The compiler does not print its own trees. Lowering and parsing produce
//! structures; turning one back into something a person reads is a debugging
//! concern, so all of it lives here — the [`ast`] tree as written and the [`ir`]
//! tree as lowered.
//!
//! Both print the *same* surface grammar, so both group by the same rules. That
//! is what this module holds: [`Prec`], one table per node kind ([`Grouped`]),
//! and one rule per position ([`write_apply`] and friends). A printed tree has
//! to re-parse into the tree it was printed from, and sharing the grouping rules
//! between the two printers is what keeps that true of both at once.
//!
//! Neither printer can implement [`std::fmt::Display`] on a compiler type
//! directly: the types are foreign to this crate and so is `Display`. Each
//! submodule therefore wraps a node before printing it, and it is the wrapper,
//! not the node, that implements [`Grouped`].

use std::fmt;

pub mod ast;
pub mod ir;

/// How tightly a printed node binds. Grouping is dropped rather than recorded,
/// so the printers reconstruct it from this alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    /// `fn a => body` — the body extends as far right as it can, so a lambda
    /// needs parentheses anywhere anything may follow it.
    Lambda,
    /// `from -> to`.
    Arrow,
    /// `func arg`.
    Apply,
    /// A form nothing can be appended to: a name, a literal, `()`, a braced
    /// struct, or a projection off one of those.
    Atom,
}

/// A node a printer has to parenthesize by precedence. Implemented by the four
/// wrappers that print as surface syntax — two in [`ast`], two in [`ir`].
pub trait Grouped: fmt::Display {
    fn prec(&self) -> Prec;
}

/// Render `func arg`. A lambda on the left would swallow the argument into its
/// own body, and anything that keeps consuming to its right would swallow
/// whatever follows the argument.
pub fn write_apply(
    f: &mut fmt::Formatter<'_>,
    func: &impl Grouped,
    arg: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, func.prec() < Prec::Apply, func)?;
    f.write_str(" ")?;
    write_grouped(f, arg.prec() < Prec::Atom, arg)
}

/// Render `from -> to`. The arrow is right-associative, so only the left side
/// can ever need grouping — an arrow there would otherwise re-parse as the outer
/// arrow's right half.
pub fn write_arrow(
    f: &mut fmt::Formatter<'_>,
    from: &impl Grouped,
    to: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, from.prec() < Prec::Atom, from)?;
    write!(f, " -> {to}")
}

/// Render `base.field`. Projection binds tighter than everything that follows a
/// space, so only the forms that extend rightward need grouping.
pub fn write_project(f: &mut fmt::Formatter<'_>, base: &impl Grouped, field: &str) -> fmt::Result {
    write_grouped(f, base.prec() < Prec::Atom, base)?;
    write!(f, ".{field}")
}

/// Render `body`, wrapping it in parentheses when leaving them off would make
/// the printed source re-parse as a different tree.
fn write_grouped(f: &mut fmt::Formatter<'_>, parens: bool, body: &dyn fmt::Display) -> fmt::Result {
    match parens {
        true => write!(f, "({body})"),
        false => write!(f, "{body}"),
    }
}
