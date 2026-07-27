use std::fmt;

use indexmap::IndexMap;

use crate::{
    parse::{
        self, write_apply, write_arrow, write_project, Expr, ExprKind, Grouped, Prec, Stmt,
        StmtKind,
    },
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked},
    types::Prim,
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
    /// The written type the definition is to be checked against, when it was
    /// ascribed one. Always `None` for a `type` declaration: that *is* a type,
    /// so there is nothing to check it against.
    pub annotation: Option<Type>,
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
    /// Reading one field out of a struct. The name stays a string for the
    /// reason [`Field`]'s keys do: it is a label scoped to whichever struct
    /// turns out to be on the left, not a path anything can refer to, so there
    /// is no symbol to resolve it to and nothing here can fail to resolve.
    Project {
        base: Box<Term>,
        field: Tracked<String>,
    },
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
    Apply {
        func: Box<Type>,
        arg: Box<Type>,
    },
    Lambda {
        arg: Tracked<Symbol>,
        body: Box<Type>,
    },
    Struct(IndexMap<String, Field<Type>>),
    Arrow {
        from: Box<Type>,
        to: Box<Type>,
    },
    Ident(Symbol),
    Prim(Prim),
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
/// needs both, and going through one wrapper is what lets the node implement
/// [`Grouped`] and so share the parser's grouping rules unchanged.
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
        // in the source is not recoverable — but there is nothing to recover,
        // because lowering hoists types the same way this prints them: every
        // type in declaration order, then every term. Printing in the order the
        // builder lowers in is what makes a printed program re-lower into the
        // one it was printed from.
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
            write!(f, "let {}", self.mint.name(*symbol))?;
            if let Some(annotation) = &decl.annotation {
                write!(f, " : {}", self.wrap(&annotation.tracked))?;
            }
            write!(f, " = {}", self.wrap(&decl.value.tracked))?;
        }
        Ok(())
    }
}

/// The IR prints as surface syntax, so it groups by the surface grammar's rules
/// — the same [`Prec`] ladder the parse tree's printer reads.
impl Grouped for Show<'_, TermKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TermKind::Fn { .. } => Prec::Lambda,
            TermKind::Apply { .. } => Prec::Apply,
            TermKind::Project { .. }
            | TermKind::Struct(_)
            | TermKind::Ident(_)
            | TermKind::Natural(_)
            | TermKind::Unit
            | TermKind::Error => Prec::Atom,
        }
    }
}

impl Grouped for Show<'_, TypeKind> {
    fn prec(&self) -> Prec {
        match self.node {
            TypeKind::Lambda { .. } => Prec::Lambda,
            TypeKind::Arrow { .. } => Prec::Arrow,
            TypeKind::Apply { .. } => Prec::Apply,
            TypeKind::Struct(_) | TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {
                Prec::Atom
            }
        }
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
                write_apply(f, &self.wrap(&func.tracked), &self.wrap(&arg.tracked))
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
            TermKind::Project { base, field } => {
                write_project(f, &self.wrap(&base.tracked), &field.tracked)
            }
        }
    }
}

impl fmt::Display for Show<'_, TypeKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            TypeKind::Error => f.write_str("<error>"),
            TypeKind::Ident(symbol) => f.write_str(self.mint.name(*symbol)),
            TypeKind::Prim(prim) => f.write_str(prim.name()),
            TypeKind::Apply { func, arg } => {
                write_apply(f, &self.wrap(&func.tracked), &self.wrap(&arg.tracked))
            }
            TypeKind::Arrow { from, to } => {
                write_arrow(f, &self.wrap(&from.tracked), &self.wrap(&to.tracked))
            }
            TypeKind::Lambda { arg, body } => write!(
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
    // Split before lowering rather than lowering in the order written: every
    // type is declared before any term is looked at, so a term can name a type
    // written anywhere in the program. Each group keeps the order it was
    // written in, so a type can still only name a type declared above it and a
    // term only a term above it — the hoist is between the groups, not inside
    // them.
    let mut types = Vec::new();
    let mut terms = Vec::new();
    for stmt in stmts {
        match stmt.tracked {
            StmtKind::Type { name, body } => types.push((name, body)),
            StmtKind::Let { name, ty, body } => terms.push((name, ty, body)),
        }
    }
    for (name, body) in types {
        let value = b.ty(body);
        if let Some(symbol) = b.declare(Namespace::Types, &name) {
            program.types.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation: None,
                    value,
                },
            );
        }
    }
    for (name, ty, body) in terms {
        // Annotation and body are lowered in the order they were written, and
        // the body before the name is bound, so a definition cannot see itself
        // and there is no recursion to resolve.
        let annotation = ty.map(|ty| b.ty(ty));
        let value = b.term(body.tracked);
        if let Some(symbol) = b.declare(Namespace::Terms, &name) {
            program.terms.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation,
                    value,
                },
            );
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
            ExprKind::Project { base, field } => {
                let base = self.term(*base);
                span.track(TermKind::Project {
                    base: Box::new(base),
                    field,
                })
            }
        }
    }

    /// Lower a surface type into an IR type, mirroring [`term`](Self::term).
    /// Type-level functions curry the same way term functions do, bind their
    /// arguments in the type namespace, and likewise always bind at least one.
    fn ty(&mut self, ty: parse::Type) -> Type {
        let span = ty.span;
        match ty.tracked {
            parse::TypeKind::Unit => span.track(TypeKind::Prim(Prim::Unit)),
            // A declaration is looked for before a primitive, so a `type Nat`
            // of one's own shadows the built-in rather than colliding with a
            // declaration nobody wrote. Types being hoisted, every term sees
            // such a declaration wherever it was written; a type sees only the
            // ones above it, and reaches the built-in otherwise.
            parse::TypeKind::Ident { name } => match self.types.get(&name.tracked) {
                Some(symbol) => span.track(TypeKind::Ident(symbol)),
                None => match Prim::from_name(&name.tracked) {
                    Some(prim) => span.track(TypeKind::Prim(prim)),
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
            },
            parse::TypeKind::Apply { func, arg } => {
                let func = self.ty(*func);
                let arg = self.ty(*arg);
                span.track(TypeKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                })
            }
            parse::TypeKind::Lambda { args, body } => {
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
                    span.track(TypeKind::Lambda {
                        arg,
                        body: Box::new(body),
                    })
                })
            }
            parse::TypeKind::Struct(fields) => {
                span.track(TypeKind::Struct(self.fields(fields, |b, ty| b.ty(ty))))
            }
            parse::TypeKind::Arrow { from, to } => {
                let from = self.ty(*from);
                let to = self.ty(*to);
                span.track(TypeKind::Arrow {
                    from: Box::new(from),
                    to: Box::new(to),
                })
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
