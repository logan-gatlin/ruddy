use std::fmt;

use indexmap::IndexMap;

use crate::{
    parse::{self, Expr, ExprKind, Stmt, StmtKind, write_grouped},
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked},
};

#[derive(Debug, Clone)]
pub struct Program {
    pub terms: IndexMap<Symbol, Decl<Term>>,
    pub types: IndexMap<Symbol, Decl<Type>>,
}

/// A top-level definition. The symbol is the map key rather than part of the
/// value, so only the span the name was written at is carried here — the same
/// split [`Field`] uses.
#[derive(Debug, Clone)]
pub struct Decl<T> {
    pub name_span: Span,
    pub value: T,
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
        arg: Tracked<Symbol>,
        body: Box<Term>,
    },
    Struct(IndexMap<String, Field<Term>>),
    Ident(Symbol),
    /// A natural number literal. It carries no symbol: a literal names nothing,
    /// so there is nothing for the mint to hand out.
    Natural(u128),
    /// A name that did not resolve. Lowering stays total so that one typo
    /// produces one error rather than a cascade from a dropped definition.
    Error,
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
        arg: Tracked<Symbol>,
        body: Box<Type>,
    },
    Struct(IndexMap<String, Field<Type>>),
    Ident(Symbol),
    Error,
}

/// A struct field. The name is the map key rather than part of the value, so
/// that a field can be looked up by name alone; only the span the name was
/// written at is carried here. `value` keeps its own span as usual.
///
/// Field names stay strings: they are labels scoped to their own struct, not
/// paths anything can refer to, so they have no place in a module tree.
#[derive(Debug, Clone)]
pub struct Field<T> {
    pub name_span: Span,
    pub value: T,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    /// A name with no definition in scope at the point it was written.
    Undefined {
        namespace: Namespace,
    },
    /// A second definition of a name. The first one is the one that stands.
    Duplicate {
        namespace: Namespace,
        previous: Span,
    },
    DuplicateField,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub program: Program,
    pub errors: Vec<Error>,
}

struct Builder<'a> {
    mint: &'a mut Mint,
    /// The module being lowered into. The surface syntax has no modules yet, so
    /// this is always the top level of the bundle; threading it now keeps
    /// adding that syntax to one place.
    module: Option<Module>,
    errors: Vec<Error>,
    terms: Names,
    types: Names,
}

/// Which symbol a name means, for one namespace.
///
/// Name resolution lives here rather than in the mint: the mint's job is to
/// make symbols unique, and this decides which name refers to which of them.
#[derive(Debug, Default)]
struct Names {
    /// Most recent binding last. Top-level definitions accumulate as they are
    /// passed and are never removed; a lambda's arguments are pushed for the
    /// length of its body.
    bindings: Vec<Binding>,
}

#[derive(Debug)]
struct Binding {
    name: String,
    symbol: Symbol,
    /// Where the name was written, so a repeat can point at what it repeats.
    span: Span,
}

/// Pairs a node with the mint that can name its symbols. Printing an IR node
/// needs both, and going through one wrapper keeps [`write_grouped`] — which
/// takes a `&dyn Display` — usable unchanged.
struct Show<'a, T> {
    node: &'a T,
    mint: &'a Mint,
}

impl Program {
    pub fn display<'a>(&'a self, mint: &'a Mint) -> impl fmt::Display + 'a {
        Show { node: self, mint }
    }
}

impl TermKind {
    /// Render one term, for tools that print a node rather than a whole
    /// program. Shares the printer with [`Program::display`], so a debugger
    /// showing a subtree and the compiler showing the program cannot disagree.
    pub fn display<'a>(&'a self, mint: &'a Mint) -> impl fmt::Display + 'a {
        Show { node: self, mint }
    }
}

impl TypeKind {
    pub fn display<'a>(&'a self, mint: &'a Mint) -> impl fmt::Display + 'a {
        Show { node: self, mint }
    }
}

impl<'a, T> Show<'a, T> {
    fn wrap<U>(&self, node: &'a U) -> Show<'a, U> {
        Show {
            node,
            mint: self.mint,
        }
    }
}

impl fmt::Display for Show<'_, Program> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Terms and types live in separate maps, so the original interleaving
        // in the source is not recoverable; types are printed first, each group
        // in the order it was declared.
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
                self.wrap(&decl.value.tracked)
            )?;
        }
        for (symbol, decl) in &self.node.terms {
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(
                f,
                "let {} = {}",
                self.mint.name(*symbol),
                self.wrap(&decl.value.tracked)
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for Show<'_, TermKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TermKind::Unit => f.write_str("()"),
            TermKind::Error => f.write_str("<error>"),
            TermKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            TermKind::Natural(value) => write!(f, "{value}"),
            TermKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, TermKind::Fn { .. }),
                    &self.wrap(&func.tracked),
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(arg.tracked, TermKind::Apply { .. } | TermKind::Fn { .. }),
                    &self.wrap(&arg.tracked),
                )
            }
            // Lowering curries multi-argument functions, so a nested `fn` per
            // argument is printed rather than the surface `fn a b => ...`.
            TermKind::Fn { arg, body } => write!(
                f,
                "fn {} => {}",
                self.mint.name(arg.tracked),
                self.wrap(&body.tracked)
            ),
            TermKind::Struct(fields) => write_struct(f, self.mint, fields),
        }
    }
}

impl fmt::Display for Show<'_, TypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TypeKind::Unit => f.write_str("()"),
            TypeKind::Error => f.write_str("<error>"),
            TypeKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            TypeKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, TypeKind::Fn { .. }),
                    &self.wrap(&func.tracked),
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(arg.tracked, TypeKind::Apply { .. } | TypeKind::Fn { .. }),
                    &self.wrap(&arg.tracked),
                )
            }
            TypeKind::Fn { arg, body } => write!(
                f,
                "fn {} => {}",
                self.mint.name(arg.tracked),
                self.wrap(&body.tracked)
            ),
            TypeKind::Struct(fields) => write_struct(f, self.mint, fields),
        }
    }
}

/// Render a `{ name: value, ... }` literal. Unlike the parser's equivalent the
/// name comes from the map key, so the field's own span is not printed.
fn write_struct<V>(
    f: &mut fmt::Formatter<'_>,
    mint: &Mint,
    fields: &IndexMap<String, Field<Tracked<V>>>,
) -> fmt::Result
where
    for<'a> Show<'a, V>: fmt::Display,
{
    f.write_str("{ ")?;
    for (i, (name, field)) in fields.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(
            f,
            "{name}: {}",
            Show {
                node: &field.value.tracked,
                mint
            }
        )?;
    }
    f.write_str(" }")
}

pub fn build(mint: &mut Mint, stmts: Vec<Stmt>) -> Output {
    let mut b = Builder {
        mint,
        module: None,
        errors: Vec::new(),
        terms: Names::default(),
        types: Names::default(),
    };
    let mut program = Program {
        terms: IndexMap::new(),
        types: IndexMap::new(),
    };
    for stmt in stmts {
        match stmt.tracked {
            StmtKind::Let { name, body } => {
                // The body is lowered before the name is bound, so a definition
                // cannot see itself and there is no recursion to resolve.
                let value = b.term(body.tracked);
                if let Some(symbol) = b.declare(Namespace::Terms, &name) {
                    program.terms.insert(
                        symbol,
                        Decl {
                            name_span: name.span,
                            value,
                        },
                    );
                }
            }
            StmtKind::Type { name, body } => {
                let value = b.ty(body);
                if let Some(symbol) = b.declare(Namespace::Types, &name) {
                    program.types.insert(
                        symbol,
                        Decl {
                            name_span: name.span,
                            value,
                        },
                    );
                }
            }
        }
    }
    Output {
        program,
        errors: b.errors,
    }
}

impl Names {
    fn get(&self, name: &str) -> Option<Symbol> {
        self.find(name).map(|binding| binding.symbol)
    }

    /// Searched innermost first, so a lambda argument hides a top-level
    /// definition of the same name.
    fn find(&self, name: &str) -> Option<&Binding> {
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.name == name)
    }

    fn bind(&mut self, name: String, symbol: Symbol, span: Span) {
        self.bindings.push(Binding { name, symbol, span });
    }

    fn mark(&self) -> usize {
        self.bindings.len()
    }

    fn release(&mut self, mark: usize) {
        self.bindings.truncate(mark);
    }
}

impl Builder<'_> {
    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.errors.push(Error { span, kind });
    }

    fn names(&mut self, namespace: Namespace) -> &mut Names {
        match namespace {
            Namespace::Terms => &mut self.terms,
            Namespace::Types => &mut self.types,
            Namespace::Modules => unreachable!("the surface syntax has no modules"),
        }
    }

    /// Bind a top-level name to a fresh symbol. `None` when the name is already
    /// defined: the first definition is the one that stands, and the repeat is
    /// reported against it.
    fn declare(&mut self, namespace: Namespace, name: &Tracked<String>) -> Option<Symbol> {
        if let Some(previous) = self.names(namespace).find(&name.tracked).map(|b| b.span) {
            self.error(
                name.span,
                ErrorKind::Duplicate {
                    namespace,
                    previous,
                },
            );
            return None;
        }
        let symbol = self
            .mint
            .global(self.module, namespace, &name.tracked)
            .expect("the name table already ruled out a repeat");
        self.names(namespace)
            .bind(name.tracked.clone(), symbol, name.span);
        Some(symbol)
    }

    /// Lower a surface expression into an IR term. Multi-argument functions are
    /// curried into nested single-argument [`TermKind::Fn`]s; the parser
    /// guarantees every function binds at least one argument, so the fold is
    /// never empty.
    fn term(&mut self, expr: Expr) -> Term {
        let span = expr.span;
        match expr.tracked {
            ExprKind::Unit => span.track(TermKind::Unit),
            ExprKind::Ident { name } => match self.terms.get(&name.tracked) {
                Some(symbol) => span.track(TermKind::Ident(symbol)),
                None => {
                    self.error(
                        name.span,
                        ErrorKind::Undefined {
                            namespace: Namespace::Terms,
                        },
                    );
                    span.track(TermKind::Error)
                }
            },
            ExprKind::Natural(value) => span.track(TermKind::Natural(value)),
            ExprKind::Apply { func, arg } => {
                let func = self.term(*func);
                let arg = self.term(*arg);
                span.track(TermKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                })
            }
            ExprKind::Function { args, body } => {
                let mark = self.terms.mark();
                let mut bound = Vec::with_capacity(args.len());
                for arg in args {
                    let span = arg.span;
                    let symbol = self.mint.local(self.module, Namespace::Terms, &arg.tracked);
                    self.terms.bind(arg.tracked, symbol, span);
                    bound.push(span.track(symbol));
                }
                let body = self.term(*body);
                self.terms.release(mark);
                bound.into_iter().rev().fold(body, |body, arg| {
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
    /// Type-level functions curry the same way term functions do, bind their
    /// arguments in the type namespace, and likewise always bind at least one.
    fn ty(&mut self, ty: parse::Type) -> Type {
        let span = ty.span;
        match ty.tracked {
            parse::TypeKind::Unit => span.track(TypeKind::Unit),
            parse::TypeKind::Ident { name } => match self.types.get(&name.tracked) {
                Some(symbol) => span.track(TypeKind::Ident(symbol)),
                None => {
                    self.error(
                        name.span,
                        ErrorKind::Undefined {
                            namespace: Namespace::Types,
                        },
                    );
                    span.track(TypeKind::Error)
                }
            },
            parse::TypeKind::Apply { func, arg } => {
                let func = self.ty(*func);
                let arg = self.ty(*arg);
                span.track(TypeKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                })
            }
            parse::TypeKind::Function { args, body } => {
                let mark = self.types.mark();
                let mut bound = Vec::with_capacity(args.len());
                for arg in args {
                    let span = arg.span;
                    let symbol = self.mint.local(self.module, Namespace::Types, &arg.tracked);
                    self.types.bind(arg.tracked, symbol, span);
                    bound.push(span.track(symbol));
                }
                let body = self.ty(*body);
                self.types.release(mark);
                bound.into_iter().rev().fold(body, |body, arg| {
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
                self.error(name_span, ErrorKind::DuplicateField);
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
    use crate::{
        parse,
        symbol::{Bundle, Version},
        token::lex,
        tracking::FileID,
    };

    /// A mint for the builder to mint into. Fresh per build, so one test's
    /// symbols cannot show up in another's.
    fn dummy_mint() -> Mint {
        Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
    }

    fn build_src(src: &str) -> (Mint, Output) {
        let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
        assert!(
            parsed.errors.is_empty(),
            "unexpected parse errors: {:#?}",
            parsed.errors
        );
        let mut mint = dummy_mint();
        let out = build(&mut mint, parsed.stmts);
        (mint, out)
    }

    fn built(src: &str) -> (Mint, Output) {
        let (mint, out) = build_src(src);
        assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
        (mint, out)
    }

    /// Find a top-level term by the name it was defined under. Nothing outside
    /// the builder maps names to symbols any more, so this walks the program.
    fn term_symbol(mint: &Mint, out: &Output, name: &str) -> Symbol {
        *out.program
            .terms
            .keys()
            .find(|symbol| mint.name(**symbol) == name)
            .unwrap_or_else(|| panic!("no term named {name}"))
    }

    fn term_value<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a TermKind {
        &out.program.terms[&term_symbol(mint, out, name)]
            .value
            .tracked
    }

    /// The struct a definition evaluates to, looking through any lambdas
    /// wrapped around it — a test needs those to bind the names its fields
    /// refer to, but they are not what it is asserting about.
    fn term_fields<'a>(
        mint: &Mint,
        out: &'a Output,
        name: &str,
    ) -> &'a IndexMap<String, Field<Term>> {
        let mut node = term_value(mint, out, name);
        while let TermKind::Fn { body, .. } = node {
            node = &body.tracked;
        }
        match node {
            TermKind::Struct(fields) => fields,
            other => panic!("expected a struct term, got {other:?}"),
        }
    }

    fn type_fields<'a>(
        mint: &Mint,
        out: &'a Output,
        name: &str,
    ) -> &'a IndexMap<String, Field<Type>> {
        let symbol = *out
            .program
            .types
            .keys()
            .find(|symbol| mint.name(**symbol) == name)
            .unwrap_or_else(|| panic!("no type named {name}"));
        let mut node = &out.program.types[&symbol].value.tracked;
        while let TypeKind::Fn { body, .. } = node {
            node = &body.tracked;
        }
        match node {
            TypeKind::Struct(fields) => fields,
            other => panic!("expected a struct type, got {other:?}"),
        }
    }

    /// Lower a program and render it back. The IR prints as surface syntax, so
    /// the rendering is re-lowered to confirm it parses and describes the same
    /// program — which is what makes the printer's parentheses trustworthy.
    fn display_program(src: &str) -> String {
        let (mint, out) = built(src);
        let printed = out.program.display(&mint).to_string();

        let (remint, relowered) = built(&printed);
        assert_eq!(
            relowered.program.display(&remint).to_string(),
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
            display_program("let k = fn f a b => f a b"),
            "let k = fn f => fn a => fn b => f a b"
        );
        assert_eq!(
            display_program("type F = fn Pair a b => Pair a b"),
            "type F = fn Pair => fn a => fn b => Pair a b"
        );
    }

    #[test]
    fn displays_application_grouping() {
        assert_eq!(
            display_program("let a = fn f g x => f (g x)"),
            "let a = fn f => fn g => fn x => f (g x)"
        );
        // Redundant grouping is gone; necessary grouping is reconstructed.
        assert_eq!(
            display_program("let b = fn f g x => (f g) x"),
            "let b = fn f => fn g => fn x => f g x"
        );
        assert_eq!(
            display_program("let c = fn map => map fn x => x"),
            "let c = fn map => map (fn x => x)"
        );
        assert_eq!(
            display_program("type M = fn Map List K V => Map (List K) V"),
            "type M = fn Map => fn List => fn K => fn V => Map (List K) V"
        );
    }

    #[test]
    fn displays_structs_and_unit() {
        assert_eq!(
            display_program("let p = fn a b => { x: a, y: b }"),
            "let p = fn a => fn b => { x: a, y: b }"
        );
        assert_eq!(display_program("let u = ()"), "let u = ()");
        assert_eq!(
            display_program("type T = fn List A => { items: List A, next: () }"),
            "type T = fn List => fn A => { items: List A, next: () }"
        );
    }

    #[test]
    fn displays_naturals() {
        assert_eq!(display_program("let n = 0"), "let n = 0");
        assert_eq!(
            display_program("let p = fn f => f 1 (f 2 3)"),
            "let p = fn f => f 1 (f 2 3)"
        );
        assert_eq!(
            display_program("let s = { width: 3, height: 4 }"),
            "let s = { width: 3, height: 4 }"
        );
    }

    #[test]
    fn a_natural_lowers_to_its_value_and_span() {
        let (mint, out) = built("let n = 4096");

        let TermKind::Natural(value) = term_value(&mint, &out, "n") else {
            panic!("expected a natural");
        };
        assert_eq!(*value, 4096);

        let span = out.program.terms[&term_symbol(&mint, &out, "n")].value.span;
        assert_eq!(span.start, 8);
        assert_eq!(span.width, 4);
    }

    #[test]
    fn a_natural_names_nothing() {
        // A literal is not a name, so lowering one mints no symbol and looks
        // none up: only the `let` itself is minted...
        let (mint, _) = built("let n = 7");
        assert_eq!(mint.symbols().count(), 1);

        // ...and an undefined name beside a literal is still the one error.
        let (_, out) = build_src("let m = f 7");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    }

    #[test]
    fn displays_types_before_terms() {
        assert_eq!(
            display_program("let x = ()  type T = { f: () }  let y = ()"),
            "type T = { f: () }\nlet x = ()\nlet y = ()"
        );
    }

    #[test]
    fn displays_empty_program_as_nothing() {
        let mut mint = dummy_mint();
        let out = build(&mut mint, Vec::new());
        assert_eq!(out.program.display(&mint).to_string(), "");
    }

    #[test]
    fn names_are_not_visible_before_their_definition() {
        let (_, out) = build_src("let a = b  let b = ()");

        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, 8);
        assert!(matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                namespace: Namespace::Terms
            }
        ));
    }

    #[test]
    fn a_definition_cannot_see_itself() {
        // No recursion: the body is lowered before the name is bound.
        let (_, out) = build_src("let f = f");

        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, 8);
    }

    #[test]
    fn duplicate_definitions_keep_the_first() {
        let (mint, out) = build_src("let x = ()  let x = fn a => a");

        // Reported at the repeat, pointing back at what it repeats.
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, 16);
        assert!(matches!(
            out.errors[0].kind,
            ErrorKind::Duplicate {
                namespace: Namespace::Terms,
                previous,
            } if previous.start == 4
        ));

        // One symbol, and it still holds the first definition's body.
        assert_eq!(out.program.terms.len(), 1);
        assert!(matches!(term_value(&mint, &out, "x"), TermKind::Unit));
    }

    #[test]
    fn namespaces_do_not_leak() {
        let (_, out) = build_src("type T = ()  let x = T");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert!(matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                namespace: Namespace::Terms
            }
        ));

        let (_, out) = build_src("let x = ()  type T = x");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert!(matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                namespace: Namespace::Types
            }
        ));
    }

    #[test]
    fn arguments_shadow_definitions_and_release() {
        let (mint, out) = built("let x = ()  let f = fn x => x  let g = x");

        let global = term_symbol(&mint, &out, "x");
        let TermKind::Fn { arg, body } = term_value(&mint, &out, "f") else {
            panic!("expected a function");
        };
        // The argument hides the definition for the length of the body...
        assert_ne!(arg.tracked, global);
        assert!(matches!(body.tracked, TermKind::Ident(s) if s == arg.tracked));
        // ...and the definition is back in scope afterwards.
        assert!(matches!(term_value(&mint, &out, "g"), TermKind::Ident(s) if *s == global));
    }

    #[test]
    fn sibling_lambdas_bind_distinct_symbols() {
        let (mint, out) = built("let f = fn x => x  let g = fn x => x");

        let arg_of = |name| match term_value(&mint, &out, name) {
            TermKind::Fn { arg, .. } => arg.tracked,
            other => panic!("expected a function, got {other:?}"),
        };
        assert_ne!(arg_of("f"), arg_of("g"));
        assert_ne!(mint.mangle(arg_of("f")), mint.mangle(arg_of("g")));
    }

    #[test]
    fn fields_are_keyed_by_name_in_source_order() {
        let (mint, out) = built("let p = { x: (), y: () }");

        let fields = term_fields(&mint, &out, "p");
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["x", "y"]
        );
        // The name is the key, but the span it was written at is still kept.
        assert_eq!(fields["x"].name_span.start, 10);
        assert_eq!(fields["x"].name_span.width, 1);
        assert_eq!(fields["y"].name_span.start, 17);
    }

    #[test]
    fn duplicate_term_fields_are_rejected() {
        let (mint, out) = build_src("let p = fn a b => { x: a, x: b }");

        // Reported at the offending repeat, not at the first occurrence.
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, 26);
        assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateField));

        // The first occurrence is the one that survives.
        let fields = term_fields(&mint, &out, "p");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["x"].name_span.start, 20);
        assert!(matches!(fields["x"].value.tracked, TermKind::Ident(s) if mint.name(s) == "a"));
    }

    #[test]
    fn duplicate_type_fields_are_rejected() {
        let (mint, out) = build_src("type T = fn A B => { a: A, a: B }");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);

        let fields = type_fields(&mint, &out, "T");
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields["a"].value.tracked, TypeKind::Ident(s) if mint.name(s) == "A"));
    }

    #[test]
    fn duplicate_fields_are_rejected_when_nested() {
        let (_, out) = build_src("let p = fn a b => { outer: { y: a, y: b } }");
        assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    }

    #[test]
    fn repeated_names_in_sibling_structs_are_fine() {
        // Field names are scoped to their own struct.
        let (_, out) = build_src("let p = fn a b => { x: a, inner: { x: b } }");
        assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    }
}
