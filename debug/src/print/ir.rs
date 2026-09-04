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
        Annotation, Attribute, ClauseKind, DataKind, Effect, EffectLabel, EffectRow, Field,
        Handler, Metadata, PatternKind, Program, Row, SumCase, Term, TermKind, TypeField, TypeKind,
        When,
    },
    symbol::Mint,
    tracking::Tracked,
};

use crate::print::{
    Entry, Grouped, Mark, Prec, Shape, label, string, tuple_field_order, write_applied,
    write_apply, write_array_pattern, write_arrow, write_binary, write_do, write_let, write_match,
    write_project, write_row, write_struct, write_sum, write_tag, write_tuple, write_unary,
};

/// Pairs a node with the mint that can name its symbols. Printing an IR node
/// needs both, and going through one wrapper is what lets the node implement
/// [`Grouped`] and so share the parser's grouping rules unchanged.
struct Show<'a, T> {
    node: &'a T,
    mint: &'a Mint,
}

/// One statement of a printed `do` block: the binding a `Let` term is, with
/// its body left to the block. Exists so that [`write_do`] can be handed the
/// statements as a sequence of things that print, the way the parse tree's
/// printer hands it statements.
struct Binding<'a> {
    name: &'a str,
    annotation: Option<Show<'a, Annotation>>,
    value: Show<'a, TermKind>,
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

impl fmt::Display for Binding<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_let(f, &self.name, self.annotation.as_ref(), &self.value)
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
            self.write_metadata(f, &decl.metadata)?;
            write!(f, "effect {}", self.mint.name(*symbol))?;
            for param in &decl.params {
                write!(f, " '{}", self.mint.name(param.symbol))?;
            }
            match &decl.value {
                Effect::Operations(operations) if operations.is_empty() => {}
                Effect::Operations(operations) => {
                    if let Some(operation) = operations.get(&ruddy::ir::OperationSelector::Unnamed)
                    {
                        f.write_str(" = ")?;
                        write_arrow(
                            f,
                            &self.show(&operation.from),
                            &self.show(&operation.to),
                            None,
                        )?;
                    } else {
                        f.write_str(" = {")?;
                        for (at, (selector, operation)) in operations.iter().enumerate() {
                            if at > 0 {
                                f.write_str(",")?;
                            }
                            let ruddy::ir::OperationSelector::Named(name) = selector else {
                                continue;
                            };
                            write!(f, " {name}: ")?;
                            write_arrow(
                                f,
                                &self.show(&operation.from),
                                &self.show(&operation.to),
                                None,
                            )?;
                        }
                        f.write_str(" }")?;
                    }
                }
                Effect::Alias(alias)
                    if alias.body.cases.is_empty() && alias.body.tail.is_none() => {}
                Effect::Alias(alias) => {
                    f.write_str(" =")?;
                    let mut first = true;
                    for case in &alias.body.cases {
                        f.write_str(if first { " " } else { " + " })?;
                        first = false;
                        f.write_str(&label(Shape::Effect, self.mint.name(case.symbol)))?;
                        for arg in &case.args {
                            let arg = self.show(arg);
                            f.write_str(" ")?;
                            match arg.prec() < Prec::Atom {
                                true => write!(f, "({arg})")?,
                                false => write!(f, "{arg}")?,
                            }
                        }
                    }
                    if let Some(tail) = &alias.body.tail {
                        f.write_str(if first { " .." } else { " + .." })?;
                        f.write_str(&match &tail.of {
                            Row::Anything => String::new(),
                            Row::Named(name) => format!("'{name}"),
                            Row::Param { symbol, .. } => format!("'{}", self.mint.name(*symbol)),
                        })?;
                    }
                }
            }
        }
        for (symbol, decl) in &self.node.types {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            self.write_metadata(f, &decl.metadata)?;
            write!(f, "type {}", self.mint.name(*symbol))?;
            for param in &decl.params {
                write!(f, " '{}", self.mint.name(param.symbol))?;
            }
            write!(f, " = {}", self.show(&decl.value))?;
        }
        for (symbol, decl) in &self.node.externs {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            self.write_metadata(f, &decl.metadata)?;
            write!(f, "extern {} : ", self.mint.name(*symbol))?;
            if let Some(annotation) = &decl.annotation {
                write!(f, "{}", self.show(annotation))?;
            }
            write!(f, " = {}", decl.value.target)?;
        }
        for (symbol, decl) in &self.node.terms {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            self.write_metadata(f, &decl.metadata)?;
            write!(f, "let {}", self.mint.name(*symbol))?;
            if let Some(annotation) = &decl.annotation {
                write!(f, " : {}", self.show(annotation))?;
            }
            write!(f, " = {}", self.show(&decl.value))?;
        }
        Ok(())
    }
}

impl<T> Show<'_, T> {
    /// A declaration's metadata, one `@key value` apiece and a space after
    /// each, in front of the declaration it belongs to.
    fn write_metadata(&self, f: &mut fmt::Formatter<'_>, metadata: &Metadata) -> fmt::Result {
        for (key, attribute) in metadata {
            write!(
                f,
                "{} ",
                Shown {
                    key,
                    attribute,
                    mint: self.mint
                }
            )?;
        }
        Ok(())
    }
}

/// One lowered attribute, ready to print: `@key`, and its value after a space
/// when it is anything but unit. Unit — the empty struct — prints as the bare
/// key, which is what the parse tree's printer writes for it too, so the two
/// printers agree.
struct Shown<'a> {
    key: &'a str,
    attribute: &'a Attribute,
    mint: &'a Mint,
}

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.key)?;
        match unit_data(&self.attribute.value.tracked) {
            true => Ok(()),
            false => write!(f, " {}", data(&self.attribute.value.tracked, self.mint)),
        }
    }
}

/// Whether lowered data is unit: the empty struct, which `()`, `{}`, and a
/// bare attribute all became.
fn unit_data(data: &DataKind) -> bool {
    matches!(data, DataKind::Struct(fields) if fields.is_empty())
}

/// Data groups as the expression it reads as: a tag carrying something is an
/// application, and everything else closes itself.
impl Grouped for Show<'_, DataKind> {
    fn prec(&self) -> Prec {
        match self.node {
            DataKind::Tag { payload, .. } if !unit_data(&payload.tracked) => Prec::Apply,
            DataKind::Tag { .. } => Prec::Tag,
            _ => Prec::Atom,
        }
    }
}

/// Lowered data prints as the expression printer prints the value it stands
/// for: the empty struct as `()`, numbered fields as a tuple, a tag carrying
/// unit as the bare tag.
impl fmt::Display for Show<'_, DataKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            DataKind::Natural(value) => write!(f, "{value}n"),
            DataKind::Integer(value) => write!(f, "{value}i"),
            DataKind::Real(value) => write!(f, "{value}"),
            DataKind::String(value) => f.write_str(&string(value)),
            DataKind::Boolean(value) => write!(f, "{value}"),
            DataKind::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", self.show(item))?;
                }
                f.write_str("]")
            }
            DataKind::Struct(fields) => {
                if fields.is_empty() {
                    f.write_str("()")
                } else if let Some(order) = tuple_field_order(fields.keys().map(String::as_str)) {
                    write_tuple(
                        f,
                        order
                            .into_iter()
                            .map(|insertion| self.show(&fields[insertion].value)),
                    )
                } else {
                    write_struct(f, self.pairs(fields), None)
                }
            }
            DataKind::Tag { name, payload } => write_tag(
                f,
                name,
                None,
                (!unit_data(&payload.tracked)).then(|| self.show(payload.as_ref())),
            ),
        }
    }
}

/// The IR prints as surface syntax, so it groups by the surface grammar's rules
/// — the same [`Prec`] ladder the parse tree's printer reads.
impl Grouped for Show<'_, TermKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TermKind::Fn { .. } => Prec::Lambda,
            // Self-delimiting on the right but not an application argument by
            // grammar; the parse tree's printer says the same, because it is
            // the same syntax. A `let` prints as the `do` block it was written
            // in, which closes with an `end` like the others.
            TermKind::Match { .. } | TermKind::Handle { .. } | TermKind::Let { .. } => Prec::Apply,
            TermKind::Raise(_) => Prec::Lambda,
            TermKind::Binary {
                op: ruddy::ir::BinaryOp::Or,
                ..
            } => Prec::Or,
            TermKind::Binary {
                op: ruddy::ir::BinaryOp::Xor,
                ..
            } => Prec::Xor,
            TermKind::Binary {
                op: ruddy::ir::BinaryOp::And,
                ..
            } => Prec::And,
            TermKind::Binary {
                op: ruddy::ir::BinaryOp::Add | ruddy::ir::BinaryOp::Sub,
                ..
            } => Prec::Addition,
            TermKind::Binary { .. } => Prec::Multiplication,
            TermKind::Unary { .. } => Prec::Unary,
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
            | TermKind::Struct { .. }
            | TermKind::Array(_)
            | TermKind::Ident(_)
            | TermKind::Natural(_)
            | TermKind::Integer(_)
            | TermKind::Real(_)
            | TermKind::String(_)
            | TermKind::Boolean(_)
            | TermKind::Error => Prec::Atom,
        }
    }

    fn ends_in_numeric_projection(&self) -> bool {
        matches!(
            self.node,
            TermKind::Project { field, .. }
                if ruddy::ui::canonical_tuple_index(&field.tracked).is_some()
        )
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
            | PatternKind::Integer(_)
            | PatternKind::Real(_)
            | PatternKind::String(_)
            | PatternKind::Boolean(_)
            | PatternKind::Unit
            | PatternKind::Struct { .. }
            | PatternKind::Array { .. } => Prec::Atom,
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
            PatternKind::Natural(value) => write!(f, "{value}n"),
            PatternKind::Integer(value) => write!(f, "{value}i"),
            PatternKind::Real(value) => write!(f, "{value}"),
            PatternKind::String(value) => f.write_str(&string(value)),
            PatternKind::Boolean(value) => write!(f, "{value}"),
            PatternKind::Unit => f.write_str("()"),
            PatternKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_deref().map(|payload| self.show(payload)),
            ),
            PatternKind::Array {
                before,
                rest,
                after,
            } => write_array_pattern(
                f,
                before.iter().map(|element| self.show(element)),
                rest.as_ref()
                    .map(|rest| rest.name.as_ref().map(|name| self.mint.name(name.tracked))),
                after.iter().map(|element| self.show(element)),
            ),
            // Through `write_row` rather than `write_struct`, because a
            // pattern's field list can end in the `..` that makes it open —
            // rendered as a tail with nothing after it, exactly as it was
            // written.
            PatternKind::Struct { fields, rest } => {
                if rest.is_none() {
                    if fields.is_empty() {
                        return f.write_str("()");
                    }
                    if let Some(order) = tuple_field_order(fields.keys().map(String::as_str)) {
                        return write_tuple(
                            f,
                            order.into_iter().map(|insertion| {
                                let field =
                                    &fields.get_index(insertion).expect("tuple field index").1;
                                self.show(&field.value)
                            }),
                        );
                    }
                }
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
            | TypeKind::Array(_)
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
            TermKind::Natural(value) => write!(f, "{value}n"),
            TermKind::Integer(value) => write!(f, "{value}i"),
            TermKind::Real(value) => write!(f, "{value}"),
            TermKind::String(value) => f.write_str(&string(value)),
            TermKind::Boolean(value) => write!(f, "{value}"),
            TermKind::Unary { op, value } => write_unary(
                f,
                match op {
                    ruddy::ir::UnaryOp::Neg => "-",
                    ruddy::ir::UnaryOp::Not => "not ",
                },
                &self.show(&**value),
            ),
            TermKind::Binary { op, left, right } => {
                let (symbol, prec) = match op {
                    ruddy::ir::BinaryOp::Add => ("+", Prec::Addition),
                    ruddy::ir::BinaryOp::Sub => ("-", Prec::Addition),
                    ruddy::ir::BinaryOp::Mul => ("*", Prec::Multiplication),
                    ruddy::ir::BinaryOp::Div => ("/", Prec::Multiplication),
                    ruddy::ir::BinaryOp::And => ("and", Prec::And),
                    ruddy::ir::BinaryOp::Or => ("or", Prec::Or),
                    ruddy::ir::BinaryOp::Xor => ("xor", Prec::Xor),
                };
                write_binary(f, &self.show(&**left), symbol, &self.show(&**right), prec)
            }
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
            // A `let` is one statement of the `do` block that wrote it, and a
            // chain of them is the block's statements in order: each body that
            // is itself a `let` folds into the same block, and the innermost
            // body is what the block returns — or nothing, when it is the unit
            // literal a block without a `return` lowers to. The names come off
            // the mint, the way every other IR node's do, so what is printed is
            // the name that was written even though the tree holds a symbol.
            TermKind::Let { .. } => {
                let mut stmts = Vec::new();
                let mut term = self.node;
                while let TermKind::Let {
                    name,
                    annotation,
                    value,
                    body,
                } = term
                {
                    stmts.push(Binding {
                        name: self.mint.name(name.tracked),
                        annotation: annotation.as_deref().map(|ty| self.show(ty)),
                        value: self.show(&**value),
                    });
                    term = body.kind();
                }
                let result = match term {
                    TermKind::Struct {
                        fields,
                        spread: None,
                    } if fields.is_empty() => None,
                    result => Some(Show {
                        node: result,
                        mint: self.mint,
                    }),
                };
                write_do(f, stmts, result)
            }
            // Braces whatever the fields are named, for the reason the parse
            // tree's printer gives: a spread says the fields written are not
            // all there are, and the tuple and unit spellings say they are.
            TermKind::Struct {
                fields,
                spread: Some(spread),
            } => write_struct(f, self.pairs(fields), Some(&self.show(&*spread.value))),
            TermKind::Struct {
                fields,
                spread: None,
            } => {
                if fields.is_empty() {
                    f.write_str("()")
                } else if let Some(order) = tuple_field_order(fields.keys().map(String::as_str)) {
                    write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            let field = &fields.get_index(insertion).expect("tuple field index").1;
                            self.show(&field.value)
                        }),
                    )
                } else {
                    write_struct(f, self.pairs(fields), None)
                }
            }
            TermKind::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    if item.spread.is_some() {
                        f.write_str("..")?;
                    }
                    write!(f, "{}", self.show(&item.value))?;
                }
                f.write_str("]")
            }
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
            TermKind::Operation { effect, selector } => {
                write!(f, "!{}{}", self.mint.name(effect.tracked), selector.tracked)
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
                " | !{}{} {} => {}",
                self.mint.name(arm.effect.tracked),
                arm.selector.tracked,
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
            // A parameter prints as the name it was declared with, sigil and
            // all, which is what makes this printer's output match the parse
            // tree's — and what makes it re-parse.
            TypeKind::Param { symbol, .. } => write!(f, "'{}", self.mint.name(*symbol)),
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
                    Row::Param { symbol, .. } => format!("'{}", self.mint.name(*symbol)),
                });
                write_sum(
                    f,
                    cases,
                    tail.as_ref().map(|tail| tail as &dyn fmt::Display),
                )
            }
            TypeKind::Prim(prim) => f.write_str(prim.name()),
            TypeKind::Array(element) => write!(f, "[{}]", self.show(&**element)),
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
                if tail.is_none() && fields.is_empty() {
                    return f.write_str("()");
                }
                if tail.is_none()
                    && fields
                        .values()
                        .all(|field| matches!(field, TypeField::Written { when: None, .. }))
                    && let Some(order) = tuple_field_order(fields.keys().map(String::as_str))
                {
                    return write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            let field = fields.get_index(insertion).expect("tuple field index").1;
                            let TypeField::Written { value, .. } = field else {
                                unreachable!("tuple fields were checked as written")
                            };
                            self.show(value)
                        }),
                    );
                }
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
                    Row::Param { symbol, .. } => format!("'{}", self.mint.name(*symbol)),
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
            .map(|(name, entry)| {
                // The label applied to its arguments, spelled as a type
                // application is: each argument an atom, anything larger
                // parenthesized.
                let mut applied = label(Shape::Effect, name.name());
                for arg in entry.args() {
                    let arg = self.show(arg);
                    applied.push(' ');
                    match arg.prec() < Prec::Atom {
                        true => applied.push_str(&format!("({arg})")),
                        false => applied.push_str(&arg.to_string()),
                    }
                }
                match entry {
                    EffectLabel::Written { when, .. } => Entry::Written {
                        name: applied,
                        mark: mark(when),
                        holds: (),
                    },
                    EffectLabel::Absent { .. } => Entry::Absent { name: applied },
                }
            })
            .collect();
        let tail = row.tail.as_ref().map(|tail| match &tail.of {
            Row::Anything => String::new(),
            Row::Named(name) => format!("'{name}"),
            Row::Param { symbol, .. } => format!("'{}", self.mint.name(*symbol)),
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

/// A lowered row, spelled here rather than by [`write_effects`]: each entry's
/// name is already the whole applied label, sigil and arguments alike, so the
/// compiler's writer — which puts the sigil in front of a bare name — would
/// have nothing left to do but get the arguments wrong.
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
                        Some(Mark::Undecided) => f.write_str("?")?,
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
            None if first => f.write_str("|"),
            None => Ok(()),
        }
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

/// Render one lowered metadata value, the way [`program`] writes it after a
/// definition's `@key`. Data names no symbols, but it prints by the same
/// grouping rules as everything else here, so it takes the mint the rest do.
pub fn data<'a>(kind: &'a DataKind, mint: &'a Mint) -> impl fmt::Display + 'a {
    Show { node: kind, mint }
}

/// Render one lowered attribute, `@key value`, for the row the IR tab gives
/// each one.
pub fn attribute<'a>(
    key: &'a str,
    attribute: &'a Attribute,
    mint: &'a Mint,
) -> impl fmt::Display + 'a {
    Shown {
        key,
        attribute,
        mint,
    }
}
