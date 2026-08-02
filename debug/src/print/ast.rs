//! Rendering the parse tree as the source it was parsed from.
//!
//! The parse tree keeps every name as it was written, so unlike [`ir`](super::ir)
//! this printer needs no mint — the wrapper carries the node and nothing else.
//! It exists only because [`std::fmt::Display`] cannot be implemented on the
//! parser's types from outside the crate that declares them.

use std::fmt;

use indexmap::IndexMap;
use ruddy::{
    parse::{ExprKind, StmtKind, TypeKind},
    tracking::Tracked,
};

use crate::print::{Grouped, Prec, write_apply, write_arrow, write_project, write_struct};

/// A parse node, ready to print. A newtype rather than a bare impl because both
/// the node and [`fmt::Display`] are foreign to this crate.
struct Ast<'a, T>(&'a T);

impl Grouped for Ast<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            TypeKind::Arrow { .. } => Prec::Arrow,
            TypeKind::Struct(_) | TypeKind::Ident { .. } | TypeKind::Unit => Prec::Atom,
        }
    }
}

impl Grouped for Ast<'_, ExprKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            // The body runs as far right as it can, so anything appended after
            // a bare lambda would be read as part of it.
            ExprKind::Function { .. } => Prec::Lambda,
            ExprKind::Apply { .. } => Prec::Apply,
            // Self-delimiting: each ends at a token of its own, so nothing that
            // follows can be drawn into it.
            ExprKind::Project { .. }
            | ExprKind::Struct(_)
            | ExprKind::Ident { .. }
            | ExprKind::Natural(_)
            | ExprKind::Unit => Prec::Atom,
        }
    }
}

impl fmt::Display for Ast<'_, StmtKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            // `body` is a `Tracked<Expr>` and `Expr` is itself `Tracked`, hence
            // the doubled `.tracked` to reach the `ExprKind`.
            StmtKind::Let { name, ty, body } => {
                write!(f, "let {}", name.tracked)?;
                if let Some(ty) = ty {
                    write!(f, " : {}", Ast(&ty.tracked))?;
                }
                write!(f, " = {}", Ast(&body.tracked.tracked))
            }
            StmtKind::Type { name, body } => {
                write!(f, "type {} = {}", name.tracked, Ast(&body.tracked))
            }
        }
    }
}

impl fmt::Display for Ast<'_, ExprKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ExprKind::Apply { func, arg } => {
                write_apply(f, &Ast(&func.tracked), &Ast(&arg.tracked))
            }
            ExprKind::Function { args, body } => write_function(f, args, &Ast(&body.tracked)),
            ExprKind::Struct(fields) => write_struct(f, pairs(fields)),
            ExprKind::Project { base, field } => {
                write_project(f, &Ast(&base.tracked), &field.tracked)
            }
            ExprKind::Ident { name } => f.write_str(&name.tracked),
            ExprKind::Natural(value) => write!(f, "{value}"),
            ExprKind::Unit => f.write_str("()"),
        }
    }
}

impl fmt::Display for Ast<'_, TypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            TypeKind::Arrow { from, to } => write_arrow(f, &Ast(&from.tracked), &Ast(&to.tracked)),
            TypeKind::Struct(fields) => write_struct(f, pairs(fields)),
            TypeKind::Ident { name } => f.write_str(&name.tracked),
            TypeKind::Unit => f.write_str("()"),
        }
    }
}

/// Render one statement, `let` or `type`, as it was written.
pub fn stmt(kind: &StmtKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render one expression, for the tree view, which labels a node with the source
/// it stands for.
pub fn expr(kind: &ExprKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render one written type, the [`expr`] counterpart.
pub fn ty(kind: &TypeKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render a `fn a b c => body` anonymous function. Only the parse tree needs
/// this: lowering curries, so the IR has no multi-argument function to print.
fn write_function(
    f: &mut fmt::Formatter<'_>,
    args: &[Tracked<String>],
    body: &dyn fmt::Display,
) -> fmt::Result {
    f.write_str("fn")?;
    for arg in args {
        write!(f, " {}", arg.tracked)?;
    }
    write!(f, " => {body}")
}

/// The fields of a struct as [`write_struct`] wants them. Unlike the IR's, this
/// tree keeps the name in the key, spans and all, so both halves of a pair have
/// to be unwrapped before the shared printer sees them.
fn pairs<V>(
    fields: &IndexMap<Tracked<String>, Tracked<V>>,
) -> impl Iterator<Item = (&String, Ast<'_, V>)>
where
    for<'a> Ast<'a, V>: fmt::Display,
{
    fields
        .iter()
        .map(|(name, value)| (&name.tracked, Ast(&value.tracked)))
}
