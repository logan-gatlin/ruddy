use std::rc::Rc;

use indexmap::IndexMap;

use crate::{
    parse::{self, Expr, ExprKind, Stmt, StmtKind},
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked},
    types::{Prim, Ty},
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

#[derive(Debug, Clone)]
pub struct Term {
    /// What the term was inferred to be. Lowering runs before inference, so
    /// until then this is [`Ty::Undecided`] — see [`TermKind::with_span`].
    pub ty: Rc<Ty>,
    pub span: Span,
    pub kind: TermKind,
}

#[derive(Debug, Clone)]
pub enum TermKind {
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
    Struct(IndexMap<String, Field<Type>>),
    Arrow { from: Box<Type>, to: Box<Type> },
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

impl TermKind {
    fn with_span(self, span: Span) -> Term {
        Term {
            ty: Default::default(),
            span,
            kind: self,
        }
    }
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
            ExprKind::Unit => TermKind::Struct(Default::default()).with_span(span),
            ExprKind::Ident { name } => match self.terms.get(&name.tracked) {
                Some(symbol) => TermKind::Ident(symbol).with_span(span),
                None => {
                    self.error(
                        name.span,
                        ErrorKind::Undefined {
                            namespace: Namespace::Terms,
                        },
                    );
                    TermKind::Error.with_span(span)
                }
            },
            ExprKind::Natural(value) => TermKind::Natural(value).with_span(span),
            ExprKind::Apply { func, arg } => {
                let func = self.term(*func);
                let arg = self.term(*arg);
                TermKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                }
                .with_span(span)
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
                    TermKind::Fn {
                        arg,
                        body: Box::new(body),
                    }
                    .with_span(span)
                })
            }
            ExprKind::Struct(fields) => {
                TermKind::Struct(self.fields(fields, |b, value| b.term(value))).with_span(span)
            }
            ExprKind::Project { base, field } => {
                let base = self.term(*base);
                TermKind::Project {
                    base: Box::new(base),
                    field,
                }
                .with_span(span)
            }
        }
    }

    /// Lower a surface type into an IR type, mirroring [`term`](Self::term).
    /// The type language has no binder, so unlike [`term`](Self::term) this
    /// never pushes a scope: every name it resolves is a top-level declaration
    /// or a primitive.
    fn ty(&mut self, ty: parse::Type) -> Type {
        let span = ty.span;
        match ty.tracked {
            parse::TypeKind::Unit => span.track(TypeKind::Struct(Default::default())),
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
