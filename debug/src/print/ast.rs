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
        Annotation, Arg, ArgKind, ArmHead, ClauseKind, EffectCase, EffectLabel, EffectRow,
        ExprKind, HandlerArm, Path, Rest, StmtKind, SumCase, TypeField, TypeKind, When, Where,
    },
    tracking::Tracked,
};

use crate::print::{
    Entry, Grouped, Mark, Prec, Shape, label, write_applied, write_apply, write_arrow,
    write_binary, write_let, write_match, write_pipeline, write_project, write_row, write_struct,
    write_sum, write_tag, write_unary,
};

/// A parse node, ready to print. A newtype rather than a bare impl because both
/// the node and [`fmt::Display`] are foreign to this crate.
struct Ast<'a, T>(&'a T);

impl Grouped for Ast<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            TypeKind::Arrow { .. } => Prec::Arrow,
            // A row of effects binds as a sum does: it is written with the same
            // labels and the same tail, and needs the same brackets around it.
            TypeKind::Sum { .. } | TypeKind::Effects(_) => Prec::Sum,
            TypeKind::Apply { .. } => Prec::Apply,
            TypeKind::Struct { .. }
            | TypeKind::Ident { .. }
            | TypeKind::Variable { .. }
            | TypeKind::Hole
            | TypeKind::Unit => Prec::Atom,
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
            ExprKind::Pipe { .. } => Prec::Pipeline,
            ExprKind::Binary {
                op: ruddy::parse::BinaryOp::Add | ruddy::parse::BinaryOp::Sub,
                ..
            } => Prec::Addition,
            ExprKind::Binary { .. } => Prec::Multiplication,
            ExprKind::Unary { .. } => Prec::Unary,
            // A tag carrying something groups as the application it reads as:
            // anything appended to `#A x` would be read as applying the
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
            | ExprKind::Integer(_)
            | ExprKind::Real(_)
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
            // A module written inline, with its own statements between the `=`
            // and the `end`; one whose body is another file has nothing after
            // the name, which is exactly how it was written.
            StmtKind::Module { name, body } => {
                write!(f, "module {}", name.tracked)?;
                let Some(body) = body else {
                    return Ok(());
                };
                f.write_str(" =")?;
                for stmt in body {
                    write!(f, " {}", Ast(&stmt.tracked))?;
                }
                f.write_str(" end")
            }
            StmtKind::Type { name, params, body } => {
                write!(f, "type {}", name.tracked)?;
                for param in params {
                    write!(f, " '{}", param.tracked)?;
                }
                write!(f, " = {}", annotation(body))
            }
            // The `|` is written before every operation, first included: the
            // grammar makes the leading one optional, so the printed form
            // re-parses, and the empty effect is `effect Nil = |` with the one
            // bar and nothing after it. An alias list writes its `+`s between
            // its effects instead, which is where a row writes them.
            StmtKind::Effect { name, cases } => {
                write!(f, "effect {} =", name.tracked)?;
                if cases.is_empty() {
                    return f.write_str(" |");
                }
                for (at, (name, case)) in cases.iter().enumerate() {
                    match case {
                        // An operation is a name and the signature it declares,
                        // written after the `|` that separates declarations; an
                        // alias is the effect it names, unioned onto the last
                        // with the `+` a row writes.
                        EffectCase::Operation { signature } => {
                            write!(f, " | {} : {}", name.name.tracked, Ast(&signature.tracked))?
                        }
                        EffectCase::Alias if at == 0 => write!(f, " {}", labelled(name))?,
                        EffectCase::Alias => write!(f, " + {}", labelled(name))?,
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
        for (at, clause) in self.0.clauses.iter().enumerate() {
            if at > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{}", Ast(&clause.tracked))?;
        }
        Ok(())
    }
}

/// One effect label as it was written: the modules it is reached through, then
/// the `!` and the name.
///
/// The sigil sits on the label rather than in front of the path, which is what
/// `Sys::!Log` says: the path qualifies the whole sigilled label, and a printer
/// that wrote `!Sys::Log` would be showing a spelling the source cannot use.
/// Every position that writes one goes through here — a row's labels, an
/// alias's cases, an operation, and a handler arm's head — so the four cannot
/// spell the same effect four ways.
fn labelled(path: &Path) -> String {
    let mut out = String::new();
    for module in &path.modules {
        out.push_str(&module.tracked);
        out.push_str("::");
    }
    out.push_str(&label(Shape::Effect, &path.name.tracked));
    out
}

/// What follows a `..`, as it was written: a variable with its sigil, or
/// nothing at all for the tail that names none.
fn rest(of: &Rest) -> String {
    match of {
        Rest::Anything => String::new(),
        Rest::Variable(name) => format!("'{}", name.tracked),
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
        // A formula names presences, and a presence is a variable: the sigil
        // is written back on so the clause re-parses.
        ClauseKind::Name(name) => write!(f, "'{name}")?,
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
            ExprKind::Pipe { value, function } => {
                write_pipeline(f, &Ast(&value.tracked), &Ast(&function.tracked))
            }
            ExprKind::Unary { value, .. } => write_unary(f, "-", &Ast(&value.tracked)),
            ExprKind::Binary { op, left, right } => {
                let (symbol, prec) = match op {
                    ruddy::parse::BinaryOp::Add => ("+", Prec::Addition),
                    ruddy::parse::BinaryOp::Sub => ("-", Prec::Addition),
                    ruddy::parse::BinaryOp::Mul => ("*", Prec::Multiplication),
                    ruddy::parse::BinaryOp::Div => ("/", Prec::Multiplication),
                };
                write_binary(f, &Ast(&left.tracked), symbol, &Ast(&right.tracked), prec)
            }
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
            // `#Some 1` and `#Some Nat` cannot come out spelled
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
                write!(f, "{}.{}", labelled(effect), op.tracked)
            }
            ExprKind::Ident { name } => write!(f, "{name}"),
            ExprKind::Natural(value) => write!(f, "{value}n"),
            ExprKind::Integer(value) => write!(f, "{value}i"),
            ExprKind::Real(value) => write!(f, "{value}"),
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
                let tail = tail.as_ref().map(|tail| rest(&tail.of));
                write_row(
                    f,
                    fields,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
            // The row an argument may be, written as an arrow's is minus the
            // `+` that hangs one off an arrow. [`effect_row`] is what an
            // arrow's own goes through, so the two cannot drift apart.
            TypeKind::Effects(row) => effect_row(row).fmt(f),
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
                let tail = tail.as_ref().map(|tail| rest(&tail.of));
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
            TypeKind::Ident { name } => write!(f, "{name}"),
            // The sigil is written back on: it is how the name was spelled, and
            // a variable printing bare would come back as a type's name.
            TypeKind::Variable { name } => write!(f, "'{}", name.tracked),
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
    // The sigil is written back on: a presence is a variable, and one printing
    // bare would read as a type's name. The anonymous `when _` names none.
    Some(Mark::When(match &when.name {
        Some(name) => format!("'{}", name.tracked.clone()),
        None => "_".to_string(),
    }))
}

/// Render one handler arm: what it answers, the name it binds, and its body.
fn write_arm(f: &mut fmt::Formatter<'_>, arm: &HandlerArm) -> fmt::Result {
    f.write_str(" | ")?;
    match &arm.head {
        ArmHead::Operation { effect, op } => write!(f, "{}.{}", labelled(effect), op.tracked)?,
        ArmHead::Return { .. } => f.write_str("return")?,
    }
    let binder = match &arm.binder.tracked {
        ArgKind::Name(name) => name.as_str(),
        ArgKind::Wildcard => "_",
    };
    write!(f, " {binder} => {}", Ast(&arm.body.tracked))
}

/// The `+ <effects>` clause an arrow may carry, as it was written — the row
/// after the `+`, which [`write_arrow`] writes the mark for.
///
/// Written as it stands rather than as what it means, which is what keeps the
/// AST tab honest: `A -> B + |` wrote a row and `A -> B` wrote none, and both
/// are the empty closed one.
fn effect_row(row: &EffectRow) -> Effects {
    let effects = row
        .effects
        .iter()
        .map(|(name, label)| match label {
            EffectLabel::Written { when } => Entry::Written {
                name: labelled(name),
                mark: mark(when),
                holds: (),
            },
            EffectLabel::Absent => Entry::Absent {
                name: labelled(name),
            },
        })
        .collect();
    let tail = row.tail.as_ref().map(|tail| rest(&tail.of));
    Effects { effects, tail }
}

/// One effect row, collected so that it can be handed to [`write_arrow`] as
/// something that prints itself. The entries have to be owned rather than
/// borrowed: the row is written *after* the result type, so the iterator would
/// have to outlive the borrow of the arrow it came from.
///
/// Each name is the whole written label, sigil and path alike; see
/// [`Display for Effects`](Effects).
struct Effects {
    effects: Vec<Entry<String, ()>>,
    tail: Option<String>,
}

/// A written row, spelled here rather than by [`write_effects`].
///
/// The one place the two printers part company, and only over where the sigil
/// goes. The compiler's writer puts the `!` in front of the name it is handed,
/// which is right for every row it ever prints: a semantic row is keyed by the
/// name an effect was *declared* under, and a declared name has no path in it.
/// A written one does — `Sys::!Log` qualifies the whole sigilled label — and
/// `!Sys::Log` is not a spelling the source can use, so a printed row would not
/// read back as the one it came from.
///
/// What the two still share is the label itself: each goes through
/// [`label`](ruddy::ui::label), so the sigil this row wears and the sigil a
/// complaint about the same effect quotes cannot drift apart.
impl fmt::Display for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for effect in &self.effects {
            if !first {
                f.write_str(" + ")?;
            }
            first = false;
            match effect {
                Entry::Written { name, mark, .. } => {
                    f.write_str(name)?;
                    match mark {
                        // The `?` no syntax reads. Nothing in a parse tree is
                        // ever one — only a failed inference produces it — so
                        // this arm is here to keep the two writers the same
                        // shape rather than because a row can reach it.
                        Some(Mark::Undecided) => f.write_str("?")?,
                        // Parenthesized for the reason a sum case's is: an
                        // effect has no colon to end a bare clause.
                        Some(Mark::When(name_of)) => write!(f, " (when {name_of})")?,
                        None => {}
                    }
                }
                Entry::Absent { name } => write!(f, "\\{name}")?,
            }
        }
        match &self.tail {
            Some(tail) => {
                if !first {
                    f.write_str(" + ")?;
                }
                write!(f, "..{tail}")
            }
            // The empty row, which a reader only reaches by writing `+ |` and
            // meaning it: the row that allows nothing at all.
            None if first => f.write_str("|"),
            None => Ok(()),
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

/// Render one written `where` clause, the [`ty`] counterpart for the formula
/// beside an annotation.
pub fn clause(kind: &ClauseKind) -> impl fmt::Display + '_ {
    Ast(kind)
}

/// Render one effect label as it was written — `!Log`, or `Sys::!Log` for one
/// reached through a module. The AST tab shows labels in rows of its own, and
/// this is what keeps them spelled the way the printed tree spells them.
pub fn effect(path: &Path) -> String {
    labelled(path)
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
