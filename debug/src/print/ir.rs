//! Rendering the lowered program as surface syntax.
//!
//! Unlike [`ast`](super::ast), the IR names things by [`Symbol`](ruddy::symbol::Symbol)
//! rather than by the string that was written, so every node here needs a
//! [`Mint`] alongside it to print at all. That, and the orphan rule, is why the
//! wrapper exists.

use std::fmt;

use indexmap::IndexMap;
use ruddy::{
    ir::{
        Annotation, ClauseKind, Effect, EffectLabel, EffectRow, Field, Handler, PatternKind,
        Program, Row, SumCase, Term, TermKind, TypeField, TypeKind, When,
    },
    symbol::Mint,
    tracking::Tracked,
};

use crate::print::{
    Entry, Grouped, Mark, Prec, write_applied, write_apply, write_arrow, write_effects, write_let,
    write_match, write_project, write_row, write_struct, write_sum, write_tag,
};

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

/// An annotation prints as its type and the `where` clause after it, so the IR
/// tab shows the whole of what a definition was ascribed.
impl Node for Annotation {
    type Kind = Annotation;

    fn kind(&self) -> &Annotation {
        self
    }
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

    /// The fields of a struct as [`write_struct`] wants them. Unlike the parse
    /// tree's, the name comes from the map key, so the field's own span is not
    /// printed.
    fn pairs<F: Node>(
        &self,
        fields: &'a IndexMap<String, Field<F>>,
    ) -> impl Iterator<Item = (&'a String, Show<'a, F::Kind>)> {
        let mint = self.mint;
        fields.iter().map(move |(name, field)| {
            (
                name,
                Show {
                    node: field.value.kind(),
                    mint,
                },
            )
        })
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
        // Effects before types, which is the order lowering reads them in: a
        // `type` declaration may name an effect in an arrow it writes, and an
        // operation's signature may name a type declared below it.
        for (symbol, decl) in &self.node.effects {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "effect {} =", self.mint.name(*symbol))?;
            match &decl.value {
                Effect::Operations(operations) if operations.is_empty() => f.write_str(" |")?,
                Effect::Operations(operations) => {
                    for (name, operation) in operations {
                        write!(
                            f,
                            " | {name} : {} -> {}",
                            self.show(&operation.from),
                            self.show(&operation.to),
                        )?;
                    }
                }
                Effect::Alias(named) if named.is_empty() => f.write_str(" |")?,
                Effect::Alias(named) => {
                    for (at, name) in named.keys().enumerate() {
                        match at {
                            0 => write!(f, " !{name}")?,
                            _ => write!(f, " + !{name}")?,
                        }
                    }
                }
            }
        }
        for (symbol, decl) in &self.node.types {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "type {}", self.mint.name(*symbol))?;
            for param in &decl.params {
                write!(f, " {}", self.mint.name(param.symbol))?;
            }
            write!(f, " = {}", self.show(&decl.value))?;
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
            // A `let`'s body runs as far right as a lambda's does, so the two
            // group alike. The parse tree's printer says the same, because it
            // is the same syntax.
            TermKind::Fn { .. } | TermKind::Let { .. } => Prec::Lambda,
            // Self-delimiting on the right but not an application argument by
            // grammar; the parse tree's printer says the same, because it is
            // the same syntax.
            TermKind::Match { .. } | TermKind::Handle { .. } => Prec::Apply,
            TermKind::Raise(_) => Prec::Lambda,
            // A tag carrying something groups as the application it reads as;
            // carrying nothing it groups below one, because the argument would
            // be read as the payload it has not got. The parse tree's printer
            // says the same, because it is the same syntax.
            TermKind::Tag {
                payload: Some(_), ..
            } => Prec::Apply,
            TermKind::Tag { payload: None, .. } => Prec::Tag,
            TermKind::Apply { .. } => Prec::Apply,
            TermKind::Project { .. }
            | TermKind::Operation { .. }
            | TermKind::Struct(_)
            | TermKind::Ident(_)
            | TermKind::Natural(_)
            | TermKind::Error => Prec::Atom,
        }
    }
}

/// A normalized pattern groups exactly as the surface pattern it prints as —
/// the same ladder the parse tree's patterns read.
impl Grouped for Show<'_, PatternKind> {
    fn prec(&self) -> Prec {
        match self.node {
            PatternKind::Tag {
                payload: Some(_), ..
            } => Prec::Apply,
            PatternKind::Tag { payload: None, .. } => Prec::Tag,
            PatternKind::Bind(_)
            | PatternKind::Wildcard
            | PatternKind::Natural(_)
            | PatternKind::Unit
            | PatternKind::Struct { .. } => Prec::Atom,
        }
    }
}

/// A normalized pattern prints as surface syntax, the binders named through
/// the mint. A pun does not come back — lowering expanded it, so `{x}` prints
/// as `{ x: x }` — but the printed form re-parses to the same normalized
/// pattern, which is the round-trip that matters here.
impl fmt::Display for Show<'_, PatternKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            PatternKind::Bind(name) => f.write_str(self.mint.name(name.tracked)),
            // The `_` as written: it names no symbol, so there is nothing for
            // the mint to spell.
            PatternKind::Wildcard => f.write_str("_"),
            PatternKind::Natural(value) => write!(f, "{value}"),
            PatternKind::Unit => f.write_str("()"),
            PatternKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_deref().map(|payload| self.show(payload)),
            ),
            // Through `write_row` rather than `write_struct`, because a
            // pattern's field list can end in the `..` that makes it open —
            // rendered as a tail with nothing after it, exactly as it was
            // written.
            PatternKind::Struct { fields, rest } => {
                let fields = self.pairs(fields).map(|(name, sub)| Entry::Written {
                    name,
                    mark: None,
                    holds: sub,
                });
                let rest = rest.as_ref().map(|_| "");
                write_row(
                    f,
                    fields,
                    rest.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
        }
    }
}

impl Grouped for Show<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TypeKind::Arrow { .. } => Prec::Arrow,
            // A row of effects binds as a sum does: same labels, same tail,
            // same brackets around it.
            TypeKind::Sum { .. } | TypeKind::Effects(_) => Prec::Sum,
            TypeKind::Apply { .. } => Prec::Apply,
            TypeKind::Struct { .. }
            | TypeKind::Ident(_)
            | TypeKind::Param { .. }
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole
            | TypeKind::Error => Prec::Atom,
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
            // The name comes off the mint, the way every other IR node's does,
            // so what is printed is the name that was written even though the
            // tree holds a symbol.
            TermKind::Let {
                name,
                annotation,
                value,
                body,
            } => write_let(
                f,
                &self.mint.name(name.tracked),
                annotation.as_deref().map(|ty| self.show(ty)),
                &self.show(&**value),
                &self.show(&**body),
            ),
            TermKind::Struct(fields) => write_struct(f, self.pairs(fields)),
            // A case carrying nothing prints as nothing, which is what it was
            // written as: lowering left the payload absent rather than filling
            // in the unit it means. See [`TermKind::Tag`].
            TermKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_ref().map(|payload| self.show(&**payload)),
            ),
            TermKind::Project { base, field } => {
                write_project(f, &self.show(&**base), &field.tracked)
            }
            // The arms print as they stand: the normalized pattern — binders
            // named through the mint, puns already expanded — and the body,
            // one arm per written arm.
            TermKind::Match { scrutinee, arms } => {
                let arms = arms
                    .iter()
                    .map(|(pattern, body)| (self.show(pattern), self.show(body)));
                write_match(f, &self.show(&**scrutinee), arms)
            }
            // The arms print in the order they were written, each with its
            // leading `|`, and the `return` arm last — where it stands is not
            // kept, because nothing depends on it.
            TermKind::Handle { body, handler } => self.write_handle(f, body, handler),
            TermKind::Raise(value) => write!(f, "raise {}", self.show(&**value)),
            TermKind::Operation { effect, op } => {
                write!(f, "!{}.{}", self.mint.name(effect.tracked), op.tracked)
            }
        }
    }
}

impl Show<'_, TermKind> {
    /// Render `handle <expr> with <arms> end`, the binders named through the
    /// mint. A `_` binder came out of lowering as a fresh `%discard`, so it
    /// prints as that name rather than as the `_` it was written as — which is
    /// what every other wildcard binder in this tab does.
    fn write_handle(
        &self,
        f: &mut fmt::Formatter<'_>,
        body: &Term,
        handler: &Handler,
    ) -> fmt::Result {
        write!(f, "handle {} with", self.show(body))?;
        for arm in &handler.arms {
            write!(
                f,
                " | !{}.{} {} => {}",
                self.mint.name(arm.effect.tracked),
                arm.op.tracked,
                self.mint.name(arm.binder.tracked),
                self.show(&arm.body),
            )?;
        }
        if let Some(ret) = &handler.ret {
            write!(
                f,
                " | return {} => {}",
                self.mint.name(ret.binder.tracked),
                self.show(&*ret.body),
            )?;
        }
        f.write_str(" end")
    }
}

impl fmt::Display for Show<'_, TypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TypeKind::Error => f.write_str("<error>"),
            // The hole, as written: `_`, whether the reader left it or the
            // pattern desugar did.
            TypeKind::Hole => f.write_str("_"),
            // A variable, as the name it was declared with — which
            // is what the reader wrote and what re-parses to the same use.
            TypeKind::Var(name) => write!(f, "'{name}"),
            TypeKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            // A parameter prints as the name it was declared with, which is
            // what makes this printer's output match the parse tree's.
            TypeKind::Param { symbol, .. } => f.write_str(self.mint.name(*symbol)),
            TypeKind::Apply { head, args, .. } => write_applied(
                f,
                self.mint.name(*head),
                args.iter().map(|arg| self.show(arg)),
            ),
            // The row an argument may be, through the one function an arrow's
            // own row goes through. A row written here is always written, so
            // the `None` [`effect_row`](Show::effect_row) gives a row nobody
            // wrote cannot arrive.
            TypeKind::Effects(row) => match self.effect_row(row) {
                Some(row) => row.fmt(f),
                None => f.write_str("<error>"),
            },
            TypeKind::Sum { cases, tail } => {
                let cases = cases.iter().map(|(name, case)| match case {
                    SumCase::Written { when, payload, .. } => Entry::Written {
                        name,
                        mark: mark(when),
                        holds: payload.as_ref().map(|ty| self.show(ty)),
                    },
                    SumCase::Absent { .. } => Entry::Absent { name },
                });
                // The tail renders as it does for a struct, and a row
                // parameter as the name it was declared with.
                let tail = tail.as_ref().map(|tail| match &tail.of {
                    Row::Anything => String::new(),
                    Row::Named(name) => format!("'{name}"),
                    Row::Param { symbol, .. } => self.mint.name(*symbol).to_string(),
                });
                write_sum(
                    f,
                    cases,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
            TypeKind::Prim(prim) => f.write_str(prim.name()),
            TypeKind::Arrow { from, to, effects } => {
                let row = self.effect_row(effects);
                write_arrow(
                    f,
                    &self.show(&**from),
                    &self.show(&**to),
                    row.as_ref().map(|row| row as &dyn fmt::Display),
                )
            }
            TypeKind::Struct { fields, tail } => {
                let fields = fields.iter().map(|(name, field)| match field {
                    TypeField::Written { when, value, .. } => Entry::Written {
                        name,
                        mark: mark(when),
                        holds: self.show(value),
                    },
                    TypeField::Absent { .. } => Entry::Absent { name },
                });
                // The tail renders as what follows the `..`: a name, or
                // nothing for the anonymous one. `write_row` writes the dots.
                let tail = tail.as_ref().map(|tail| match &tail.of {
                    Row::Anything => String::new(),
                    Row::Named(name) => format!("'{name}"),
                    // A row parameter prints as the name it was declared with,
                    // for the reason a type parameter does.
                    Row::Param { symbol, .. } => self.mint.name(*symbol).to_string(),
                });
                write_row(
                    f,
                    fields,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
        }
    }
}

/// An annotation prints as the type it ascribes and the `where` clause after
/// it, which is what a reader wrote and what re-parses back to it.
///
/// The clause is one statement, however many the reader wrote: lowering
/// conjoined them into the one formula. What comes out re-lowers to the same
/// annotation, which is the round trip that matters here — a `;` the reader put
/// between two comparisons is not a fact the IR keeps, any more than the
/// parentheses around one are. The variables are not written out at all: each
/// is introduced where it is used, so the type beside them already says which
/// there are.
impl fmt::Display for Show<'_, Annotation> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.show(&self.node.ty))?;
        match &self.node.clause {
            Some(clause) => write!(f, " where {}", self.show(clause)),
            None => Ok(()),
        }
    }
}

/// A lowered `where` clause, with exactly the parentheses re-parsing needs. The
/// same ladder the compiler's own formula printer keeps, and for the same
/// reason: a clause is its own grammar, so the type language's levels have
/// nothing here to be compared against.
impl fmt::Display for Show<'_, ClauseKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        clause_at(f, self.node, 0)
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

impl Show<'_, TypeKind> {
    /// The `+ <effects>` clause an arrow carries, or `None` where none was
    /// written — which is what a bare `A -> B` has.
    ///
    /// The row a reader wrote, not what it means: `A -> B + |` wrote one and
    /// `A -> B` wrote none, and both are the empty closed row. Aliases *are*
    /// gone, since lowering expanded them, which is what the tab is for showing.
    fn effect_row(&self, row: &EffectRow) -> Option<Effects> {
        if !row.written {
            return None;
        }
        let effects = row
            .effects
            .iter()
            .map(|(name, label)| match label {
                EffectLabel::Written { when, .. } => Entry::Written {
                    name: name.clone(),
                    mark: mark(when),
                    holds: (),
                },
                EffectLabel::Absent { .. } => Entry::Absent { name: name.clone() },
            })
            .collect();
        let tail = row.tail.as_ref().map(|tail| match &tail.of {
            Row::Anything => String::new(),
            Row::Named(name) => format!("'{name}"),
            Row::Param { symbol, .. } => self.mint.name(*symbol).to_string(),
        });
        Some(Effects { effects, tail })
    }
}

/// One effect row, collected so that it can be handed to [`write_arrow`] as
/// something that prints itself. Owned rather than borrowed for the reason the
/// AST printer's twin is: the row is written after the result type, so a
/// borrowed iterator would have to outlive the arrow it came from.
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

/// One presence mark, copied. [`Mark`] carries no `Clone`, and the alternative
/// is borrowing a row that has already gone out of scope.
fn clone_mark(mark: &Mark) -> Mark {
    match mark {
        Mark::Undecided => Mark::Undecided,
        Mark::When(name) => Mark::When(name.clone()),
    }
}

/// The `when` clause a lowered label wears. `when _` is the anonymous presence,
/// spelled back as the `_` it was written as.
fn mark(when: &Option<Box<When>>) -> Option<Mark> {
    let when = when.as_ref()?;
    // The sigil is written back on: a presence is a variable, and one printing
    // bare would read as a type's name. The anonymous `when _` names none.
    Some(Mark::When(match &when.name {
        Some(name) => format!("'{}", name.clone()),
        None => "_".to_string(),
    }))
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

/// Render one lowered ascription: the type, the variables its a variable
/// declared, and the formula beside them. The [`ty`] counterpart for the whole
/// of what a definition was ascribed.
pub fn annotation<'a>(node: &'a Annotation, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node, mint }
}

/// Render one type, the [`term`] counterpart.
pub fn ty<'a>(kind: &'a TypeKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}

/// Render one lowered `where` clause, the [`ty`] counterpart for the formula
/// beside an annotation.
pub fn clause<'a>(kind: &'a ClauseKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}

/// Render one normalized pattern, the [`term`] counterpart for the arms of a
/// match.
pub fn pattern<'a>(kind: &'a PatternKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}
