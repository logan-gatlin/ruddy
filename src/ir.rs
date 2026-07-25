use indexmap::IndexMap;

use crate::{
    parse::{self, write_grouped, Expr, ExprKind, Stmt, StmtKind},
    symbol::Mint,
    tracking::{Span, Tracked},
};

#[derive(Debug, Clone)]
pub struct Program {
    pub terms: IndexMap<String, Term>,
    pub types: IndexMap<String, Type>,
}

pub type Term = Tracked<TermKind>;

#[derive(Debug, Clone)]
pub enum TermKind {
    Unit,
    Apply {
        func: Box<Term>,
        arg: Box<Term>,
    },
    Fn {
        arg: Tracked<String>,
        body: Box<Term>,
    },
    Struct(IndexMap<String, Field<Term>>),
    Ident(String),
}

pub type Type = Tracked<TypeKind>;

#[derive(Debug, Clone)]
pub enum TypeKind {
    Unit,
    Apply {
        func: Box<Type>,
        arg: Box<Type>,
    },
    Fn {
        arg: Tracked<String>,
        body: Box<Type>,
    },
    Struct(IndexMap<String, Field<Type>>),
    Ident(String),
}

/// A struct field. The name is the map key rather than part of the value, so
/// that a field can be looked up by name alone; only the span the name was
/// written at is carried here. `value` keeps its own span as usual.
#[derive(Debug, Clone)]
pub struct Field<T> {
    pub name_span: Span,
    pub value: T,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub program: Program,
    pub errors: Vec<Error>,
}

struct Builder<'a> {
    errors: Vec<Error>,
    mint: &'a Mint,
}

impl std::fmt::Display for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Terms and types live in separate maps, so the original interleaving
        // in the source is not recoverable; types are printed first, each group
        // in the order it was declared.
        let mut first = true;
        for (name, ty) in &self.types {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "type {name} = {}", ty.tracked)?;
        }
        for (name, term) in &self.terms {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "let {name} = {}", term.tracked)?;
        }
        Ok(())
    }
}

impl std::fmt::Display for TermKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TermKind::Unit => f.write_str("()"),
            TermKind::Ident(name) => f.write_str(name),
            TermKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, TermKind::Fn { .. }),
                    &func.tracked,
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(arg.tracked, TermKind::Apply { .. } | TermKind::Fn { .. }),
                    &arg.tracked,
                )
            }
            // Lowering curries multi-argument functions, so a nested `fn` per
            // argument is printed rather than the surface `fn a b => ...`.
            TermKind::Fn { arg, body } => write!(f, "fn {} => {}", arg.tracked, body.tracked),
            TermKind::Struct(fields) => write_struct(f, fields),
        }
    }
}

impl std::fmt::Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Unit => f.write_str("()"),
            TypeKind::Ident(name) => f.write_str(name),
            TypeKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, TypeKind::Fn { .. }),
                    &func.tracked,
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(arg.tracked, TypeKind::Apply { .. } | TypeKind::Fn { .. }),
                    &arg.tracked,
                )
            }
            TypeKind::Fn { arg, body } => write!(f, "fn {} => {}", arg.tracked, body.tracked),
            TypeKind::Struct(fields) => write_struct(f, fields),
        }
    }
}

/// Render a `{ name: value, ... }` literal. Unlike the parser's equivalent the
/// name comes from the map key, so the field's own span is not printed.
fn write_struct<V: std::fmt::Display>(
    f: &mut std::fmt::Formatter<'_>,
    fields: &IndexMap<String, Field<Tracked<V>>>,
) -> std::fmt::Result {
    f.write_str("{ ")?;
    for (i, (name, field)) in fields.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{name}: {}", field.value.tracked)?;
    }
    f.write_str(" }")
}

pub fn build(mint: &Mint, stmts: Vec<Stmt>) -> Output {
    let mut b = Builder {
        mint,
        errors: Vec::new(),
    };
    let mut terms = IndexMap::new();
    let mut types = IndexMap::new();
    for s in stmts {
        match s.tracked {
            StmtKind::Let { name, body } => {
                // `body` is a `Tracked<Expr>`; `body.tracked` is the `Expr`.
                let term = b.term(body.tracked);
                let span = name.span;
                if terms.insert(name.tracked, term).is_some() {
                    // The name was already bound at an earlier statement.
                    b.error(span);
                }
            }
            StmtKind::Type { name, body } => {
                let ty = b.ty(body);
                let span = name.span;
                if types.insert(name.tracked, ty).is_some() {
                    b.error(span);
                }
            }
        }
    }
    Output {
        program: Program { terms, types },
        errors: b.errors,
    }
}

impl Builder<'_> {
    fn error(&mut self, span: Span) {
        self.errors.push(Error { span });
    }

    /// Lower a surface expression into an IR term. Multi-argument functions are
    /// curried into nested single-argument [`TermKind::Fn`]s; the parser
    /// guarantees every function binds at least one argument, so the fold is
    /// never empty.
    fn term(&mut self, expr: Expr) -> Term {
        let span = expr.span;
        match expr.tracked {
            ExprKind::Unit => span.track(TermKind::Unit),
            ExprKind::Ident { name } => span.track(TermKind::Ident(name.tracked)),
            ExprKind::Apply { func, arg } => {
                let func = self.term(*func);
                let arg = self.term(*arg);
                span.track(TermKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                })
            }
            ExprKind::Function { args, body } => {
                let body = self.term(*body);
                args.into_iter().rev().fold(body, |body, arg| {
                    let span = arg.span.merge(body.span);
                    span.track(TermKind::Fn {
                        arg,
                        body: Box::new(body),
                    })
                })
            }
            ExprKind::Struct(fields) => span.track(TermKind::Struct(
                self.fields(fields, |b, value| b.term(value)),
            )),
        }
    }

    /// Lower a surface type into an IR type, mirroring [`term`](Self::term).
    /// Type-level functions curry the same way term functions do and likewise
    /// always bind at least one argument.
    fn ty(&mut self, ty: parse::Type) -> Type {
        let span = ty.span;
        match ty.tracked {
            parse::TypeKind::Unit => span.track(TypeKind::Unit),
            parse::TypeKind::Ident { name } => span.track(TypeKind::Ident(name.tracked)),
            parse::TypeKind::Apply { func, arg } => {
                let func = self.ty(*func);
                let arg = self.ty(*arg);
                span.track(TypeKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                })
            }
            parse::TypeKind::Function { args, body } => {
                let body = self.ty(*body);
                args.into_iter().rev().fold(body, |body, arg| {
                    let span = arg.span.merge(body.span);
                    span.track(TypeKind::Fn {
                        arg,
                        body: Box::new(body),
                    })
                })
            }
            parse::TypeKind::Struct(fields) => {
                span.track(TypeKind::Struct(self.fields(fields, |b, ty| b.ty(ty))))
            }
        }
    }

    /// Re-key surface struct fields by name, lowering each value with `lower`.
    /// The surface syntax tolerates a name appearing twice; the IR does not, so
    /// a repeat is reported at the offending name and the first occurrence is
    /// the one that survives.
    fn fields<S, T>(
        &mut self,
        fields: IndexMap<Tracked<String>, S>,
        lower: impl Fn(&mut Self, S) -> T,
    ) -> IndexMap<String, Field<T>> {
        let mut lowered = IndexMap::new();
        for (name, value) in fields {
            let name_span = name.span;
            let value = lower(self, value);
            if lowered.contains_key(&name.tracked) {
                self.error(name_span);
                continue;
            }
            lowered.insert(name.tracked, Field { name_span, value });
        }
        lowered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse, symbol::BundleId, token::lex, tracking::FileID};

    /// A mint for the builder to hold on to. Leaked rather than shared, so
    /// that each build gets an empty one once the builder starts minting.
    fn dummy_mint() -> &'static Mint {
        Box::leak(Box::new(Mint::new(BundleId::new(0), "test")))
    }

    fn build_src(src: &str) -> Output {
        let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
        assert!(
            parsed.errors.is_empty(),
            "unexpected parse errors: {:#?}",
            parsed.errors
        );
        build(dummy_mint(), parsed.stmts)
    }

    fn term_fields<'a>(out: &'a Output, name: &str) -> &'a IndexMap<String, Field<Term>> {
        match &out.program.terms[name].tracked {
            TermKind::Struct(fields) => fields,
            other => panic!("expected a struct term, got {other:?}"),
        }
    }

    fn type_fields<'a>(out: &'a Output, name: &str) -> &'a IndexMap<String, Field<Type>> {
        match &out.program.types[name].tracked {
            TypeKind::Struct(fields) => fields,
            other => panic!("expected a struct type, got {other:?}"),
        }
    }

    /// Lower a program and render it back. The IR prints as surface syntax, so
    /// the rendering is re-lowered to confirm it parses and describes the same
    /// program — which is what makes the printer's parentheses trustworthy.
    fn display_program(src: &str) -> String {
        let out = build_src(src);
        assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);

        let printed = out.program.to_string();
        let relowered = build_src(&printed);
        assert!(
            relowered.errors.is_empty(),
            "ir errors re-lowering {printed:?}: {:#?}",
            relowered.errors
        );
        assert_eq!(
            relowered.program.to_string(),
            printed,
            "printing {src:?} did not round-trip"
        );
        printed
    }

    #[test]
    fn displays_curried_functions() {
        // The surface form binds both arguments at one `fn`; the IR does not,
        // and the printer shows the currying rather than hiding it.
        assert_eq!(
            display_program("let k = fn a b => f a b"),
            "let k = fn a => fn b => f a b"
        );
        assert_eq!(
            display_program("type F = fn a b => Pair a b"),
            "type F = fn a => fn b => Pair a b"
        );
    }

    #[test]
    fn displays_application_grouping() {
        assert_eq!(display_program("let a = f (g x)"), "let a = f (g x)");
        // Redundant grouping is gone; necessary grouping is reconstructed.
        assert_eq!(display_program("let b = (f g) x"), "let b = f g x");
        assert_eq!(
            display_program("let c = map fn x => x"),
            "let c = map (fn x => x)"
        );
        assert_eq!(
            display_program("type M = Map (List K) V"),
            "type M = Map (List K) V"
        );
    }

    #[test]
    fn displays_structs_and_unit() {
        assert_eq!(
            display_program("let p = { x: a, y: b }"),
            "let p = { x: a, y: b }"
        );
        assert_eq!(display_program("let u = ()"), "let u = ()");
        assert_eq!(
            display_program("type T = { items: List A, next: () }"),
            "type T = { items: List A, next: () }"
        );
    }

    #[test]
    fn displays_types_before_terms() {
        assert_eq!(
            display_program("let x = a  type T = { f: A }  let y = b"),
            "type T = { f: A }\nlet x = a\nlet y = b"
        );
    }

    #[test]
    fn displays_empty_program_as_nothing() {
        assert_eq!(build(dummy_mint(), Vec::new()).program.to_string(), "");
    }

    #[test]
    fn fields_are_keyed_by_name_in_source_order() {
        let out = build_src("let p = { x: a, y: b }");
        assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);

        let fields = term_fields(&out, "p");
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["x", "y"]
        );
        // The name is the key, but the span it was written at is still kept.
        assert_eq!(fields["x"].name_span.start, 10);
        assert_eq!(fields["x"].name_span.width, 1);
        assert_eq!(fields["y"].name_span.start, 16);
    }

    #[test]
    fn duplicate_term_fields_are_rejected() {
        let out = build_src("let p = { x: a, x: b }");

        // Reported at the offending repeat, not at the first occurrence.
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, 16);

        // The first occurrence is the one that survives.
        let fields = term_fields(&out, "p");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["x"].name_span.start, 10);
        assert!(matches!(fields["x"].value.tracked, TermKind::Ident(ref n) if n == "a"));
    }

    #[test]
    fn duplicate_type_fields_are_rejected() {
        let out = build_src("type T = { a: A, a: B }");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);

        let fields = type_fields(&out, "T");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields["a"].value.tracked, TypeKind::Ident(ref n) if n == "A"));
    }

    #[test]
    fn duplicate_fields_are_rejected_when_nested() {
        let out = build_src("let p = { outer: { y: a, y: b } }");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    }

    #[test]
    fn repeated_names_in_sibling_structs_are_fine() {
        // Field names are scoped to their own struct.
        let out = build_src("let p = { x: a, inner: { x: b } }");
        assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    }
}
