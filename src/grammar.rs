//! The surface grammar's grouping rules, and the forms that need them.
//!
//! Grouping is dropped rather than recorded: `(a) -> b` and `a -> b` parse to
//! the same tree, so anything rendering one back as source has to work out from
//! precedence alone where the parentheses go. That rule is here, said once —
//! [`Prec`], one table per node kind ([`Grouped`]), and one writer per position.
//!
//! It lives in the compiler rather than in the debugger's printers because the
//! grammar belongs to the language rather than to any one reader of it. The
//! compiler prints on its own account — every type in a diagnostic goes through
//! [`Display for Ty`](crate::types::Ty) — and the debugger prints whole trees,
//! and both are writing the same surface syntax, which has to re-parse into
//! what it was printed from either way. Sharing it can only happen in this
//! direction: `ruddy-debug` depends on `ruddy`, and nothing may make the
//! dependency run back.
//!
//! Rendering a *tree* is still no part of the compiler's job. What is shared is
//! the grammar the trees are rendered in, not the walk that renders them.

use std::fmt;

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

/// A node a printer has to parenthesize by precedence. Implemented by every
/// wrapper that prints as surface syntax, and by [`Ty`](crate::types::Ty),
/// which prints as one directly.
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

/// Render a `{ name: value, ... }` literal, shared by every position one can
/// appear in: struct expressions and struct types, in either tree, and the
/// semantic type as well. The trees reach the name and the value differently —
/// one off a spanned key, another off the map's key and a field — so the pairs
/// arrive already rendered.
pub fn write_struct<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = (K, V)>,
) -> fmt::Result {
    let mut fields = fields.into_iter().peekable();
    // The empty struct is unit, which reads as `{}` — the padding a struct with
    // fields gets would only be two spaces around nothing.
    if fields.peek().is_none() {
        return f.write_str("{}");
    }

    f.write_str("{ ")?;
    for (i, (name, value)) in fields.enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{name}: {value}")?;
    }
    f.write_str(" }")
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
