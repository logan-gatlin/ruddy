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
        Annotation, Arg, ArgKind, ArmHead, Attribute, ClauseKind, DataKind, EffectBody,
        EffectLabel, EffectRow, Expr, ExprKind, ExternTypeKind, HandlerArm, Path, PatternKind,
        Rest, Stmt, StmtKind, SumCase, TypeField, TypeKind, When, Where,
    },
    tracking::Tracked,
};

use crate::print::{
    Entry, Grouped, Mark, Prec, Shape, label, string, tuple_field_order, write_applied,
    write_apply, write_array_pattern, write_arrow, write_binary, write_do, write_let, write_match,
    write_pipeline, write_project, write_row, write_struct, write_sum, write_tag, write_tuple,
    write_unary,
};

/// A parse node, ready to print. A newtype rather than a bare impl because both
/// the node and [`fmt::Display`] are foreign to this crate.
struct Ast<'a, T>(&'a T);

impl Grouped for Ast<'_, TypeKind> {
    fn prec(&self) -> Prec {
        ruddy::ui::type_prec(self.0)
    }
}

impl Grouped for Ast<'_, ExprKind> {
    fn prec(&self) -> Prec {
        ruddy::ui::expr_prec(self.0)
    }

    fn ends_in_numeric_projection(&self) -> bool {
        ruddy::ui::expr_ends_in_numeric_projection(self.0)
    }
}

/// Whether a following `|` would be claimed by a shorthand nested at the
/// right edge of this expression. Parentheses are the only way to hand that
/// bar back to an enclosing shorthand.
fn ends_in_match_function(expr: &ExprKind) -> bool {
    match expr {
        ExprKind::MatchFunction { .. } => true,
        ExprKind::Function { body, .. } => ends_in_match_function(&body.tracked),
        ExprKind::Raise(value) => ends_in_match_function(&value.tracked),
        _ => false,
    }
}

/// A statement prints as its attributes, one apiece and each in front of the
/// next, and then the definition — the order it was written in, and the order
/// the parser reads it back in.
impl fmt::Display for Ast<'_, Stmt> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for attribute in &self.0.attributes {
            write!(f, "{} ", Ast(attribute))?;
        }
        write!(f, "{}", Ast(&self.0.kind))
    }
}

/// `@key`, and its value after a space when it carries one. A written `()` is
/// the same value as none, so it prints as the bare key — the rule the
/// lowered tree's printer follows too, which is what keeps the two agreeing.
impl fmt::Display for Ast<'_, Attribute> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0.key.tracked)?;
        match &self.0.value {
            Some(value) if !unit_data(&value.tracked) => write!(f, " {}", Ast(&value.tracked)),
            _ => Ok(()),
        }
    }
}

/// Whether a written value is unit: `()`, or the `{}` lowering makes the same.
fn unit_data(data: &DataKind) -> bool {
    match data {
        DataKind::Unit => true,
        DataKind::Struct(fields) => fields.is_empty(),
        _ => false,
    }
}

/// Data groups as the expression it looks like: a tag carrying something is
/// the one form that reads as an application, and everything else closes
/// itself. A tag carrying a written unit is bare, as it prints.
impl Grouped for Ast<'_, DataKind> {
    fn prec(&self) -> Prec {
        match self.0 {
            DataKind::Tag { payload, .. } => match carried(payload.as_deref()) {
                Some(_) => Prec::Apply,
                None => Prec::Tag,
            },
            _ => Prec::Atom,
        }
    }
}

/// A tag's payload, when it carries anything: a written `()` or `{}` is the
/// unit a bare tag already carries, so it is not written back.
fn carried(payload: Option<&ruddy::parse::Data>) -> Option<&ruddy::parse::Data> {
    payload.filter(|payload| !unit_data(&payload.tracked))
}

/// A metadata value prints as the expression it would be, by the expression
/// printer's rules: unit and the empty struct as `()`, numbered fields as a
/// tuple, and the rest as written.
impl fmt::Display for Ast<'_, DataKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DataKind::Natural(value) => write!(f, "{value}n"),
            DataKind::Integer(value) => write!(f, "{value}i"),
            DataKind::Fixed(value) => write!(f, "{value}"),
            DataKind::Real(value) => write!(f, "{value}"),
            DataKind::String(value) => f.write_str(&string(value)),
            DataKind::Bool(value) => write!(f, "{value}"),
            DataKind::Unit => f.write_str("()"),
            DataKind::Tuple(elements) => {
                write_tuple(f, elements.iter().map(|element| Ast(&element.tracked)))
            }
            DataKind::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Ast(&item.tracked))?;
                }
                f.write_str("]")
            }
            DataKind::Struct(fields) => {
                if fields.is_empty() {
                    f.write_str("()")
                } else if let Some(order) =
                    tuple_field_order(fields.iter().map(|(name, _)| name.tracked.as_str()))
                {
                    write_tuple(
                        f,
                        order
                            .into_iter()
                            .map(|insertion| Ast(&fields[insertion].tracked)),
                    )
                } else {
                    write_struct(
                        f,
                        fields
                            .iter()
                            .map(|(name, value)| (&name.tracked, Ast(&value.tracked))),
                        None,
                    )
                }
            }
            DataKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                carried(payload.as_deref()).map(|payload| Ast(&payload.tracked)),
            ),
        }
    }
}

impl fmt::Display for Ast<'_, StmtKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            StmtKind::Extern {
                name,
                ty,
                abi,
                target,
            } => {
                write!(f, "extern {} : {}", name.tracked, Ast(&abi.tracked))?;
                if let Some(clause) = &ty.clause {
                    write!(f, " where {}", Ast(clause))?;
                }
                write!(f, " = {target}")
            }
            // `body` is a `Tracked<Expr>` and `Expr` is itself `Tracked`, hence
            // the doubled `.tracked` to reach the `ExprKind`.
            StmtKind::Let { pattern, ty, body } => write_let(
                f,
                &pattern.tracked,
                ty.as_ref().map(annotation),
                &Ast(&body.tracked.tracked),
            ),
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
                    write!(f, " {}", Ast(stmt))?;
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
            StmtKind::Effect { name, params, body } => {
                write!(f, "effect {}", name.tracked)?;
                for param in params {
                    write!(f, " '{}", param.tracked)?;
                }
                match body {
                    EffectBody::Empty => Ok(()),
                    EffectBody::Alias(row) => write!(f, " = {}", effect_row(row)),
                    EffectBody::Unnamed { signature } => {
                        write!(f, " = {}", Ast(&signature.tracked))
                    }
                    EffectBody::Named(fields) => {
                        f.write_str(" = {")?;
                        for (at, (name, signature)) in fields.iter().enumerate() {
                            if at > 0 {
                                f.write_str(",")?;
                            }
                            write!(f, " {}: {}", name.tracked, Ast(&signature.tracked))?;
                        }
                        f.write_str(" }")
                    }
                }
            }
        }
    }
}

impl fmt::Display for Ast<'_, ExternTypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ExternTypeKind::Annotated { attributes, inner } => {
                for attribute in attributes {
                    write!(f, "{} ", Ast(attribute))?;
                }
                write!(f, "{}", Ast(&inner.tracked))
            }
            ExternTypeKind::Group(inner) => write!(f, "({})", Ast(&inner.tracked)),
            ExternTypeKind::Ordinary(ty) => write!(f, "{}", Ast(&ty.tracked)),
            ExternTypeKind::Function {
                parameters,
                result,
                effects,
            } => {
                f.write_str("fn(")?;
                for (i, parameter) in parameters.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Ast(&parameter.tracked))?;
                }
                write!(f, ") -> {}", Ast(&result.tracked))?;
                if let Some(effects) = effects {
                    write!(f, " + {}", effect_row(effects))?;
                }
                Ok(())
            }
        }
    }
}

pub fn extern_type(kind: &ExternTypeKind) -> impl fmt::Display + '_ {
    Ast(kind)
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
    if path.absolute.is_some() {
        out.push_str("::");
    }
    for module in &path.modules {
        out.push_str(&module.tracked);
        out.push_str("::");
    }
    out.push_str(&label(Shape::Effect, &path.name.tracked));
    out
}

/// One effect label applied to its arguments, spelled as a type application
/// is: each argument an atom, and anything larger parenthesized so that it
/// reads back as one argument rather than several.
fn applied(path: &Path, args: &[ruddy::parse::Type]) -> String {
    let mut out = labelled(path);
    for arg in args {
        let rendered = Ast(&arg.tracked);
        out.push(' ');
        match rendered.prec() < Prec::Atom {
            true => out.push_str(&format!("({rendered})")),
            false => out.push_str(&rendered.to_string()),
        }
    }
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
            ExprKind::Unary { op, value } => write_unary(
                f,
                match op {
                    ruddy::parse::UnaryOp::Neg => "-",
                    ruddy::parse::UnaryOp::Not => "not ",
                    ruddy::parse::UnaryOp::Allocate => "mut ",
                    ruddy::parse::UnaryOp::Read => "~",
                },
                &Ast(&value.tracked),
            ),
            ExprKind::Binary { op, left, right } => {
                let (symbol, prec) = match op {
                    ruddy::parse::BinaryOp::Write => (":=", Prec::Assignment),
                    ruddy::parse::BinaryOp::Add => ("+", Prec::Addition),
                    ruddy::parse::BinaryOp::Sub => ("-", Prec::Addition),
                    ruddy::parse::BinaryOp::Mul => ("*", Prec::Multiplication),
                    ruddy::parse::BinaryOp::Div => ("/", Prec::Multiplication),
                    ruddy::parse::BinaryOp::And => ("and", Prec::And),
                    ruddy::parse::BinaryOp::Or => ("or", Prec::Or),
                    ruddy::parse::BinaryOp::Xor => ("xor", Prec::Xor),
                };
                write_binary(f, &Ast(&left.tracked), symbol, &Ast(&right.tracked), prec)
            }
            ExprKind::Apply { func, arg } => {
                write_apply(f, &Ast(&func.tracked), &Ast(&arg.tracked))
            }
            ExprKind::Function { args, body } => write_function(f, args, &Ast(&body.tracked)),
            ExprKind::MatchFunction { arms, .. } => {
                f.write_str("fn")?;
                for (at, arm) in arms.iter().enumerate() {
                    write!(f, " | {} => ", arm.pattern.tracked)?;
                    let body = Ast(&arm.body.tracked);
                    if at + 1 < arms.len() && ends_in_match_function(&arm.body.tracked) {
                        write!(f, "({body})")?;
                    } else {
                        write!(f, "{body}")?;
                    }
                }
                Ok(())
            }
            // The statements print through the statement printer, so a
            // binding in a block is written as the definition it reads like.
            ExprKind::Do { stmts, result } => write_do(
                f,
                stmts.iter().map(Ast),
                result.as_deref().map(|result| Ast(&result.tracked)),
            ),
            ExprKind::If {
                predicate,
                consequent,
                alternative,
            } => write_if(f, predicate, consequent, alternative, true),
            // The pattern prints through the compiler's own `Display`, so the
            // arm a match shows is the arm the parser read.
            ExprKind::Match { scrutinee, arms } => write_match(
                f,
                &Ast(&scrutinee.tracked),
                arms.iter()
                    .map(|arm| (&arm.pattern.tracked, Ast(&arm.body.tracked))),
            ),
            // A literal that spreads a value is written with its braces
            // whatever its fields are named: the tuple and unit spellings say
            // the fields written are all the fields there are, and a spread
            // says the opposite.
            ExprKind::Struct {
                fields,
                spread: Some(spread),
            } => write_struct(f, pairs(fields), Some(&Ast(&spread.value.tracked))),
            ExprKind::Struct {
                fields,
                spread: None,
            } => {
                if fields.is_empty() {
                    f.write_str("()")
                } else if let Some(order) =
                    tuple_field_order(fields.keys().map(|name| name.tracked.as_str()))
                {
                    write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            Ast(&fields
                                .get_index(insertion)
                                .expect("tuple field index")
                                .1
                                .tracked)
                        }),
                    )
                } else {
                    write_struct(f, pairs(fields), None)
                }
            }
            ExprKind::Tuple(elements) => {
                write_tuple(f, elements.iter().map(|element| Ast(&element.tracked)))
            }
            ExprKind::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    if item.spread.is_some() {
                        f.write_str("..")?;
                    }
                    write!(f, "{}", Ast(&item.value.tracked))?;
                }
                f.write_str("]")
            }
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
            ExprKind::Operation { effect, selector } => {
                write!(f, "{}{}", labelled(effect), selector.tracked)
            }
            ExprKind::Ident { name } => write!(f, "{name}"),
            ExprKind::Natural(value) => write!(f, "{value}n"),
            ExprKind::Integer(value) => write!(f, "{value}i"),
            ExprKind::Fixed(value) => write!(f, "{value}"),
            ExprKind::Real(value) => write!(f, "{value}"),
            ExprKind::String(value) => f.write_str(&string(value)),
            ExprKind::Bool(value) => write!(f, "{value}"),
            ExprKind::Unit => f.write_str("()"),
        }
    }
}

impl Grouped for Ast<'_, PatternKind> {
    fn prec(&self) -> Prec {
        ruddy::ui::pattern_prec(self.0)
    }
}

impl fmt::Display for Ast<'_, PatternKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            PatternKind::Ident { name } => f.write_str(&name.tracked),
            PatternKind::Wildcard => f.write_str("_"),
            PatternKind::Natural(value) => write!(f, "{value}n"),
            PatternKind::Integer(value) => write!(f, "{value}i"),
            PatternKind::Fixed(value) => write!(f, "{value}"),
            PatternKind::Real(value) => write!(f, "{value}"),
            PatternKind::String(value) => f.write_str(&string(value)),
            PatternKind::Bool(value) => write!(f, "{value}"),
            PatternKind::Unit => f.write_str("()"),
            PatternKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_deref().map(|payload| Ast(&payload.tracked)),
            ),
            // The payload is grouped by the rule a tag's is: taken greedily
            // when read, so anything but an atom needs its parentheses back.
            PatternKind::Hidden { variable, pattern } => {
                write!(f, "hide '{} ", variable.tracked)?;
                let payload = Ast(&pattern.tracked);
                match payload.prec() < Prec::Atom {
                    true => write!(f, "({payload})"),
                    false => write!(f, "{payload}"),
                }
            }
            PatternKind::Tuple(elements) => {
                write_tuple(f, elements.iter().map(|element| Ast(&element.tracked)))
            }
            PatternKind::Array {
                before,
                rest,
                after,
            } => write_array_pattern(
                f,
                before.iter().map(|element| Ast(&element.tracked)),
                rest.as_ref()
                    .map(|rest| rest.name.as_ref().map(|name| name.tracked.as_str())),
                after.iter().map(|element| Ast(&element.tracked)),
            ),
            PatternKind::Struct { fields, rest } => {
                if rest.is_none() && fields.is_empty() {
                    return f.write_str("()");
                }
                if rest.is_none()
                    && let Some(order) =
                        tuple_field_order(fields.keys().map(|name| name.tracked.as_str()))
                    && fields.values().all(Option::is_some)
                {
                    return write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            let pattern = fields
                                .get_index(insertion)
                                .expect("tuple field index")
                                .1
                                .as_ref()
                                .expect("tuple fields were checked as explicit");
                            Ast(&pattern.tracked)
                        }),
                    );
                }
                f.write_str("{")?;
                let mut first = true;
                for (name, pattern) in fields {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    let name = label(Shape::Struct, &name.tracked);
                    match pattern {
                        Some(pattern) => write!(f, " {name}: {}", Ast(&pattern.tracked))?,
                        None => write!(f, " {name}")?,
                    }
                }
                if rest.is_some() {
                    if !first {
                        f.write_str(", ")?;
                    }
                    f.write_str(" ..")?;
                }
                f.write_str(" }")
            }
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
                if tail.is_none() && fields.is_empty() {
                    return f.write_str("()");
                }
                if tail.is_none()
                    && fields
                        .values()
                        .all(|field| matches!(field, TypeField::Written { when: None, .. }))
                    && let Some(order) =
                        tuple_field_order(fields.keys().map(|name| name.tracked.as_str()))
                {
                    return write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            let field = fields.get_index(insertion).expect("tuple field index").1;
                            let TypeField::Written { value, .. } = field else {
                                unreachable!("tuple fields were checked as written")
                            };
                            Ast(&value.tracked)
                        }),
                    );
                }
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
            TypeKind::Tuple(elements) => {
                write_tuple(f, elements.iter().map(|element| Ast(&element.tracked)))
            }
            TypeKind::Mut(region, element) => write!(
                f,
                "mut {} ({})",
                Ast(&region.tracked),
                Ast(&element.tracked)
            ),
            TypeKind::Array(element) => write!(f, "[{}]", Ast(&element.tracked)),
            TypeKind::Apply { head, args } => write_applied(
                f,
                Ast(&head.tracked),
                args.iter().map(|arg| Ast(&arg.tracked)),
            ),
            TypeKind::Ident { name } => write!(f, "{name}"),
            // The body runs to the end of the type, so it is never grouped.
            TypeKind::Hidden { variable, body } => {
                write!(f, "hide '{} => {}", variable.tracked, Ast(&body.tracked))
            }
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

/// Render a conditional, flattening a conditional alternative into the
/// language's one-`end` `else if` spelling.
///
/// Grouping is intentionally absent from the parse tree, so an explicitly
/// parenthesized nested alternative canonicalizes to the same chain.
fn write_if(
    f: &mut fmt::Formatter<'_>,
    predicate: &Expr,
    consequent: &Expr,
    alternative: &Expr,
    final_end: bool,
) -> fmt::Result {
    write!(
        f,
        "if {} then {} else ",
        Ast(&predicate.tracked),
        Ast(&consequent.tracked)
    )?;
    match &alternative.tracked {
        ExprKind::If {
            predicate,
            consequent,
            alternative,
        } => write_if(f, predicate, consequent, alternative, false)?,
        other => write!(f, "{}", Ast(other))?,
    }
    if final_end {
        f.write_str(" end")?;
    }
    Ok(())
}

/// Render one handler arm: what it answers, the name it binds, and its body.
fn write_arm(f: &mut fmt::Formatter<'_>, arm: &HandlerArm) -> fmt::Result {
    f.write_str(" | ")?;
    match &arm.head {
        ArmHead::Operation { effect, selector } => {
            write!(f, "{}{}", labelled(effect), selector.tracked)?
        }
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
            EffectLabel::Written { args, when } => Entry::Written {
                name: applied(name, args),
                mark: mark(when),
                holds: (),
            },
            EffectLabel::Absent { args } => Entry::Absent {
                name: applied(name, args),
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

/// Render one statement as it was written: its attributes, then the
/// definition.
pub fn stmt(stmt: &Stmt) -> impl fmt::Display + '_ {
    Ast(stmt)
}

/// Render one attribute as it was written, `@key` or `@key value`, for the
/// row the AST tab gives each one.
pub fn attribute(attribute: &Attribute) -> impl fmt::Display + '_ {
    Ast(attribute)
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

/// Render one written pattern, including every primitive literal spelling.
pub fn pattern(kind: &PatternKind) -> impl fmt::Display + '_ {
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
