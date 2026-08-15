//! Rendering the parse tree as the source it was parsed from.
//!
//! The parse tree keeps every name as it was written, so unlike [`ir`](super::ir)
//! this printer needs no mint — the wrapper carries the node and nothing else.
//! It exists only because [`std::fmt::Display`] cannot be implemented on the
//! parser's types from outside the crate that declares them.

use std::fmt;

use indexmap::IndexMap;
use ruddy::{
    parse::{
        Annotation, Arg, ArgKind, ArmHead, ClauseKind, ClauseStmtKind, EffectCase, EffectLabel,
        EffectRow, ExprKind, HandlerArm, StmtKind, SumCase, TypeField, TypeKind, When, Where,
    },
    tracking::Tracked,
};

use crate::print::{
    Entry, Grouped, Mark, Prec, write_applied, write_apply, write_arrow, write_effects, write_let,
    write_match, write_project, write_row, write_struct, write_sum, write_tag,
};

/// A parse node, ready to print. A newtype rather than a bare impl because both
/// the node and [`fmt::Display`] are foreign to this crate.
struct Ast<'a, T>(&'a T);

impl Grouped for Ast<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            TypeKind::Arrow { .. } => Prec::Arrow,
            TypeKind::Sum { .. } => Prec::Sum,
            TypeKind::Apply { .. } => Prec::Apply,
            TypeKind::Struct { .. } | TypeKind::Ident { .. } | TypeKind::Hole | TypeKind::Unit => {
                Prec::Atom
            }
        }
    }
}

impl Grouped for Ast<'_, ExprKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            // The body runs as far right as it can, so anything appended after
            // a bare lambda would be read as part of it. A nested `let`'s body
            // runs the same way, so it groups the same way.
            ExprKind::Function { .. } | ExprKind::Let { .. } => Prec::Lambda,
            // Self-delimiting on the right — the `end` closes it — so it may
            // head an application and be projected from; but it is not an
            // application *argument* by grammar, so an argument position
            // brackets it. Below `Atom` is exactly that split.
            // Self-delimiting on the right — the `end` closes it — so a
            // handler groups exactly as a match does.
            ExprKind::Match { .. } | ExprKind::Handle { .. } => Prec::Apply,
            // The body runs as far right as it can, so anything appended after
            // a `raise` would be read as part of what it carries.
            ExprKind::Raise(_) => Prec::Lambda,
            // A tag carrying something groups as the application it reads as:
            // anything appended to `` `A x `` would be read as applying the
            // case rather than as a second argument to it. Carrying nothing it
            // is not a word but a word still waiting for one, so it groups
            // below an application — see [`Prec::Tag`].
            ExprKind::Tag {
                payload: Some(_), ..
            } => Prec::Apply,
            ExprKind::Tag { payload: None, .. } => Prec::Tag,
            ExprKind::Apply { .. } => Prec::Apply,
            // Self-delimiting: each ends at a token of its own, so nothing that
            // follows can be drawn into it.
            // An operation is written like a projection and closes itself the
            // same way.
            ExprKind::Project { .. }
            | ExprKind::Operation { .. }
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
            StmtKind::Let { pattern, ty, body } => {
                write!(f, "let {}", pattern.tracked)?;
                if let Some(ty) = ty {
                    write!(f, " : {}", annotation(ty))?;
                }
                write!(f, " = {}", Ast(&body.tracked.tracked))
            }
            StmtKind::Type { name, params, body } => {
                write!(f, "type {}", name.tracked)?;
                for param in params {
                    write!(f, " {}", param.tracked)?;
                }
                write!(f, " = {}", annotation(body))
            }
            // The `|` is written before every case, first included: the
            // grammar makes the leading one optional, so the printed form
            // re-parses, and the empty effect is `effect Nil = |` with the one
            // bar and nothing after it.
            StmtKind::Effect { name, cases } => {
                write!(f, "effect {} =", name.tracked)?;
                if cases.is_empty() {
                    return f.write_str(" |");
                }
                for (op, case) in cases {
                    write!(f, " | `{}", op.tracked)?;
                    if let EffectCase::Operation { signature } = case {
                        write!(f, " : {}", Ast(&signature.tracked))?;
                    }
                }
                Ok(())
            }
        }
    }
}

/// Render a written type and the `where` clause after it — an ascription as it
/// was written, so a printed annotation re-parses to the one it came from.
///
/// The clause follows the whole type, which is where the grammar puts it, and
/// is left off entirely when none was written.
pub fn annotation(annotation: &Annotation) -> impl fmt::Display + '_ {
    Written(annotation)
}

struct Written<'a>(&'a Annotation);

impl fmt::Display for Written<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Ast(&self.0.ty.tracked))?;
        match &self.0.clause {
            Some(clause) => write!(f, " where {}", Ast(clause)),
            None => Ok(()),
        }
    }
}

/// A whole `where` clause: its statements, separated by the `;`s the reader
/// wrote. Written here rather than by whoever shows one, so the clause on an
/// annotation's line and the clause on a row of its own cannot come out spelled
/// differently.
pub fn where_clause(clause: &Where) -> impl fmt::Display + '_ {
    Ast(clause)
}

impl fmt::Display for Ast<'_, Where> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, stmt) in self.0.stmts.iter().enumerate() {
            if at > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{}", Ast(&stmt.tracked))?;
        }
        Ok(())
    }
}

/// One statement of a `where` clause, as it was written: a `let` and the names
/// it declares, or the formula.
pub fn clause_stmt(kind: &ClauseStmtKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

impl fmt::Display for Ast<'_, ClauseStmtKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ClauseStmtKind::Let(names) => {
                f.write_str("let ")?;
                for (at, name) in names.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(&name.tracked)?;
                }
                Ok(())
            }
            ClauseStmtKind::Constraint(clause) => write!(f, "{}", Ast(&clause.tracked)),
        }
    }
}

/// A written `where` clause, with exactly the parentheses re-parsing needs.
///
/// Its own ladder rather than [`Prec`], for the reason the compiler's own
/// formula printer keeps one: nothing in a clause can be a type, so there is
/// nothing here for the type language's levels to be compared against.
impl fmt::Display for Ast<'_, ClauseKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        clause_at(f, self.0, 0)
    }
}

/// How tightly one clause binds: `0` for a comparison, `1` for `or`, `2` for
/// `and`, `3` for `not`, `4` for a name.
fn clause_prec(clause: &ClauseKind) -> u8 {
    match clause {
        ClauseKind::Equal(..) | ClauseKind::NotEqual(..) => 0,
        ClauseKind::Or(..) => 1,
        ClauseKind::And(..) => 2,
        ClauseKind::Not(_) => 3,
        ClauseKind::Name(_) => 4,
    }
}

/// Write `clause` for a position that binds at least as tightly as `level`,
/// bracketing it when it does not.
fn clause_at(f: &mut fmt::Formatter<'_>, clause: &ClauseKind, level: u8) -> fmt::Result {
    let parens = clause_prec(clause) < level;
    if parens {
        f.write_str("(")?;
    }
    match clause {
        ClauseKind::Name(name) => f.write_str(name)?,
        ClauseKind::Not(inner) => {
            f.write_str("not ")?;
            clause_at(f, &inner.tracked, 3)?;
        }
        // Left-associative, so the right side is written one level tighter.
        ClauseKind::And(left, right) => {
            clause_at(f, &left.tracked, 2)?;
            f.write_str(" and ")?;
            clause_at(f, &right.tracked, 3)?;
        }
        ClauseKind::Or(left, right) => {
            clause_at(f, &left.tracked, 1)?;
            f.write_str(" or ")?;
            clause_at(f, &right.tracked, 2)?;
        }
        // Non-associative, so both sides go one level tighter.
        ClauseKind::Equal(left, right) => {
            clause_at(f, &left.tracked, 1)?;
            f.write_str(" = ")?;
            clause_at(f, &right.tracked, 1)?;
        }
        ClauseKind::NotEqual(left, right) => {
            clause_at(f, &left.tracked, 1)?;
            f.write_str(" != ")?;
            clause_at(f, &right.tracked, 1)?;
        }
    }
    if parens {
        f.write_str(")")?;
    }
    Ok(())
}

impl fmt::Display for Ast<'_, ExprKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ExprKind::Apply { func, arg } => {
                write_apply(f, &Ast(&func.tracked), &Ast(&arg.tracked))
            }
            ExprKind::Function { args, body } => write_function(f, args, &Ast(&body.tracked)),
            ExprKind::Let {
                pattern,
                ty,
                value,
                body,
            } => write_let(
                f,
                &pattern.tracked,
                ty.as_ref().map(|ty| annotation(ty)),
                &Ast(&value.tracked),
                &Ast(&body.tracked),
            ),
            // The pattern prints through the compiler's own `Display`, so the
            // arm a match shows is the arm the parser read.
            ExprKind::Match { scrutinee, arms } => write_match(
                f,
                &Ast(&scrutinee.tracked),
                arms.iter()
                    .map(|arm| (&arm.pattern.tracked, Ast(&arm.body.tracked))),
            ),
            ExprKind::Struct(fields) => write_struct(f, pairs(fields)),
            ExprKind::Project { base, field } => {
                write_project(f, &Ast(&base.tracked), &field.tracked)
            }
            // A term's tag is written by the same rule a type's case is, so
            // `` `Some 1 `` and `` `Some Nat `` cannot come out spelled
            // differently. It never wears a `when`: that says a case may or may
            // not be allowed, which is a claim about a type and not something
            // a value can be.
            ExprKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_ref().map(|payload| Ast(&payload.tracked)),
            ),
            // The arms print with a leading `|` apiece, first included, the
            // way a match's do: the grammar makes it optional there, so the
            // printed form re-parses, and a handler with no arms writes none.
            ExprKind::Handle { body, arms } => {
                write!(f, "handle {} with", Ast(&body.tracked))?;
                for arm in arms {
                    write_arm(f, arm)?;
                }
                f.write_str(" end")
            }
            ExprKind::Raise(value) => write!(f, "raise {}", Ast(&value.tracked)),
            ExprKind::Operation { effect, op } => {
                write!(f, "{}.`{}", effect.tracked, op.tracked)
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
            TypeKind::Arrow { from, to, effects } => {
                let row = effects.as_deref().map(effect_row);
                write_arrow(
                    f,
                    &Ast(&from.tracked),
                    &Ast(&to.tracked),
                    row.as_ref().map(|row| row as &dyn fmt::Display),
                )
            }
            TypeKind::Struct { fields, tail } => {
                let fields = fields.iter().map(|(name, field)| match field {
                    TypeField::Written { when, value } => Entry::Written {
                        name: &name.tracked,
                        mark: mark(when),
                        holds: Ast(&value.tracked),
                    },
                    TypeField::Absent => Entry::Absent {
                        name: &name.tracked,
                    },
                });
                // The tail renders as what follows the `..`: a name, or
                // nothing for the anonymous one. `write_row` writes the dots.
                let tail = tail
                    .as_ref()
                    .map(|tail| tail.name.as_ref().map_or("", |name| name.tracked.as_str()));
                write_row(
                    f,
                    fields,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
            TypeKind::Sum { cases, tail } => {
                let cases = cases.iter().map(|(name, case)| match case {
                    SumCase::Written { when, payload } => Entry::Written {
                        name: &name.tracked,
                        mark: mark(when),
                        holds: payload.as_ref().map(|ty| Ast(&ty.tracked)),
                    },
                    SumCase::Absent => Entry::Absent {
                        name: &name.tracked,
                    },
                });
                // The tail renders as it does for a struct: what follows the
                // `..`, with `write_sum` writing the dots and the bars.
                let tail = tail
                    .as_ref()
                    .map(|tail| tail.name.as_ref().map_or("", |name| name.tracked.as_str()));
                write_sum(
                    f,
                    cases,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
            TypeKind::Apply { head, args } => write_applied(
                f,
                Ast(&head.tracked),
                args.iter().map(|arg| Ast(&arg.tracked)),
            ),
            TypeKind::Ident { name } => f.write_str(&name.tracked),
            // The hole as written: `_`, a position left for inference.
            TypeKind::Hole => f.write_str("_"),
            TypeKind::Unit => f.write_str("()"),
        }
    }
}

/// The `when` clause a written label wears, as the compiler's own row printer
/// takes it. `when _` is the anonymous presence, spelled back as the `_` it was
/// written as; nothing in a parse tree is ever the undecided-presence artifact,
/// which only a failed inference produces.
fn mark(when: &Option<Box<When>>) -> Option<Mark> {
    let when = when.as_ref()?;
    Some(Mark::When(match &when.name {
        Some(name) => name.tracked.clone(),
        None => "_".to_string(),
    }))
}

/// Render one handler arm: what it answers, the name it binds, and its body.
fn write_arm(f: &mut fmt::Formatter<'_>, arm: &HandlerArm) -> fmt::Result {
    f.write_str(" | ")?;
    match &arm.head {
        ArmHead::Operation { effect, op } => write!(f, "{}.`{}", effect.tracked, op.tracked)?,
        ArmHead::Return { .. } => f.write_str("return")?,
    }
    let binder = match &arm.binder.tracked {
        ArgKind::Name(name) => name.as_str(),
        ArgKind::Wildcard => "_",
    };
    write!(f, " {binder} => {}", Ast(&arm.body.tracked))
}

/// The `! <effects>` clause an arrow may carry, as it was written — the row
/// after the `!`, which [`write_arrow`] writes the mark for.
///
/// Written as it stands rather than as what it means, which is what keeps the
/// AST tab honest: `A -> B ! |` wrote a row and `A -> B` wrote none, and both
/// are the empty closed one.
fn effect_row(row: &EffectRow) -> Effects {
    let effects = row
        .effects
        .iter()
        .map(|(name, label)| match label {
            EffectLabel::Written { when } => Entry::Written {
                name: name.tracked.clone(),
                mark: mark(when),
                holds: (),
            },
            EffectLabel::Absent => Entry::Absent {
                name: name.tracked.clone(),
            },
        })
        .collect();
    let tail = row.tail.as_ref().map(|tail| {
        tail.name
            .as_ref()
            .map_or_else(String::new, |name| name.tracked.clone())
    });
    Effects { effects, tail }
}

/// One effect row, collected so that it can be handed to [`write_arrow`] as
/// something that prints itself. The entries have to be owned rather than
/// borrowed: the row is written *after* the result type, so the iterator would
/// have to outlive the borrow of the arrow it came from.
struct Effects {
    effects: Vec<Entry<String, ()>>,
    tail: Option<String>,
}

impl fmt::Display for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let effects: Vec<Entry<&str, ()>> = self
            .effects
            .iter()
            .map(|entry| match entry {
                Entry::Written { name, mark, .. } => Entry::Written {
                    name: name.as_str(),
                    mark: mark.as_ref().map(clone_mark),
                    holds: (),
                },
                Entry::Absent { name } => Entry::Absent {
                    name: name.as_str(),
                },
            })
            .collect();
        write_effects(
            f,
            &effects,
            self.tail.as_ref().map(|tail| tail as &dyn fmt::Display),
        )
    }
}

/// One presence mark, copied. [`Mark`] is a compiler type and carries no
/// `Clone`, so the debugger makes its own — one line, and the alternative is
/// borrowing a row that has already gone out of scope.
fn clone_mark(mark: &Mark) -> Mark {
    match mark {
        Mark::Undecided => Mark::Undecided,
        Mark::When(name) => Mark::When(name.clone()),
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

/// Render one written `where` clause, the [`ty`] counterpart for the formula
/// beside an annotation.
pub fn clause(kind: &ClauseKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render one written type, the [`expr`] counterpart.
pub fn ty(kind: &TypeKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render a `fn a b c => body` anonymous function — a `_` argument as the `_`
/// it was written as. Only the parse tree needs this: lowering curries, so the
/// IR has no multi-argument function to print.
fn write_function(
    f: &mut fmt::Formatter<'_>,
    args: &[Arg],
    body: &dyn fmt::Display,
) -> fmt::Result {
    f.write_str("fn")?;
    for arg in args {
        match &arg.tracked {
            ArgKind::Name(name) => write!(f, " {name}")?,
            ArgKind::Wildcard => f.write_str(" _")?,
        }
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
