use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    parse::{self, Expr, ExprKind, Stmt, StmtKind},
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked, TrackedString},
    types::{ParamKind, Prim, Ty},
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
    /// The parameters a `type` declaration binds, in order. Always empty for a
    /// term, which binds none of its own — a lambda's argument is bound inside
    /// its body rather than by the definition.
    pub params: Vec<Param>,
    pub value: T,
}

/// One parameter of a `type` declaration.
#[derive(Debug, Clone)]
pub struct Param {
    /// Where the name was written, so a repeat can point at what it repeats.
    pub span: Span,
    pub symbol: Symbol,
    /// What it stands for. Not known while the body is being lowered — it
    /// follows from how the body uses it — so it is [`ParamKind::Type`] until
    /// the kinds are worked out, once every body is in.
    pub kind: ParamKind,
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
        field: TrackedString,
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
    Struct {
        fields: IndexMap<String, TypeField>,
        /// The `..` tail, when the struct type was written open. Never
        /// `Some` inside a `type` declaration — see
        /// [`ErrorKind::OpenDeclaredType`].
        tail: Option<Tail>,
    },
    Arrow {
        from: Box<Type>,
        to: Box<Type>,
    },
    Ident(Symbol),
    /// A declared type applied to arguments.
    ///
    /// The head is a symbol rather than a type: lowering is where "only a
    /// declared type may be applied" is said, so by the time one of these
    /// exists the head has already been one. The spine arrives flat from the
    /// parser and stays flat, because a declaration is applied to everything it
    /// takes at once.
    Apply {
        head: Symbol,
        /// Where the head was written, so an arity complaint can point at the
        /// name rather than at the whole application.
        head_span: Span,
        args: Vec<Type>,
    },
    /// A parameter of the declaration this type is the body of.
    ///
    /// Both the symbol and the position, because the two readers want
    /// different things: the debugger names it and cross-highlights it, and
    /// inference substitutes for it by position — which is [`Ty::Bound`]
    /// exactly, so lowering one is a rename rather than a translation.
    Param {
        symbol: Symbol,
        index: u32,
    },
    Prim(Prim),
    Error,
}

/// One field of a struct type: the [`Field`] split of spans, plus whether the
/// field was marked `?` — there or not, with this type when it is.
#[derive(Debug, Clone)]
pub struct TypeField {
    pub name_span: Span,
    pub optional: bool,
    pub value: Type,
}

/// The `..` tail of a struct type: what is said about the fields not named.
#[derive(Debug, Clone)]
pub struct Tail {
    pub span: Span,
    pub of: Row,
}

/// What a `..` tail stands for.
#[derive(Debug, Clone)]
pub enum Row {
    /// `..` — any fields at all. Only an annotation may say this; a
    /// declaration holds for every definition and so cannot leave the question
    /// open. See [`ErrorKind::OpenDeclaredType`].
    Anything,
    /// `..r` in an annotation, where `r` binds nothing: a name scoped to that
    /// one annotation, staying a string for the reason [`Field`]'s keys do —
    /// it is not a path anything can refer to, so there is no symbol to
    /// resolve it to and nothing here can fail to resolve. Two `..r` in one
    /// annotation stand for one rest; another annotation's `r` is unrelated.
    Named(String),
    /// `..r` naming a row parameter of the declaration being lowered. This is
    /// the one tail a declaration may have, and the only way a declared type
    /// can be left open: what it stands for is supplied at every use rather
    /// than decided once here, so the body still mentions no solver variable.
    Param { symbol: Symbol, index: u32 },
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
    /// A type declared as a name that leads back to itself with nothing in
    /// between: `type t = t`, or a pair each declared as the other.
    ///
    /// This is not the same complaint as a type that contains itself. A type
    /// may name itself as much as it likes through a struct or an arrow —
    /// that is what makes recursive types writable — because unfolding such a
    /// type reaches a shape one step in. A chain of bare names never reaches
    /// one, so there is nothing for the declaration to mean.
    Circular,
    /// A `..` tail or a `?` field inside a `type` declaration, as in
    /// `type t = { x: Nat, .. }`.
    ///
    /// What a declaration stands for is lowered once, before any definition,
    /// and holds for all of them; a tail or an optional field stands for
    /// something a definition gets to decide, so there is nothing for one to
    /// mean here. Inference leans on the difference: an alias body mentions
    /// no solver variable, which is what lets every walk stop at a name.
    OpenDeclaredType,
    /// A type given a different number of arguments than it takes, including a
    /// name written bare that takes some.
    ///
    /// There is no partial application: a declaration takes what it takes
    /// wherever it is written, so a name short of its arguments is the same
    /// complaint as one given too many.
    Arity {
        expected: usize,
        found: usize,
    },
    /// Something that is not a declared type, applied: a primitive, a struct,
    /// a parenthesized arrow.
    NotAConstructor,
    /// A parameter used as the head of an application, as in
    /// `type Flip f a = f a`.
    ///
    /// A parameter stands for one type, never for something still waiting for
    /// types of its own. Refusing this is what keeps every declaration's
    /// parameters plain — each one a type, and nothing higher — so that
    /// checking an application is counting rather than a language of its own.
    ParameterApplied,
    /// One declaration binding a name twice: `type Pair A A = ...`.
    DuplicateParameter {
        previous: Span,
    },
    /// A type that leads back to itself having been given something other than
    /// its own parameters, as in `type T a = { next: T { x: a } }`.
    ///
    /// Unfolding one builds a bigger argument every time and never comes back
    /// round, so there is no finite answer to whether two of them are the same
    /// type. Handing a type its own parameters unchanged is what makes the
    /// question decidable, and it is what a recursive type is normally for.
    NonUniformRecursion,
    /// A parameter used both as a type and as the fields a row does not name,
    /// as in `type Bad r = { x: r, ..r }`.
    ///
    /// A parameter is written bare, so what it stands for is read off its
    /// uses. Two uses that disagree leave nothing to read, and neither of them
    /// is the wrong one — it is the declaration that has to say which it meant.
    MixedParameter,
    /// Something that cannot stand for a set of fields, written where a row
    /// parameter goes: `WithX Nat` against `type WithX r = { x: Nat, ..r }`.
    ///
    /// A struct can stand for one, and so can another row parameter. A
    /// declared name cannot, though it looks as though it should: a tail
    /// holding a name would have to be unfolded by the walks that flatten
    /// rows, and neither does.
    NotARow,
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
    /// How many arguments each declared type takes. Filled before any body is
    /// lowered, so an application can be counted wherever it appears —
    /// including above the declaration it names, which the hoist allows.
    arities: HashMap<Symbol, usize>,
    /// The parameters of the declaration being lowered, by symbol, and where
    /// each sits in its list. Empty outside a `type` body, which is what makes
    /// a parameter unwritable in an annotation.
    params: HashMap<Symbol, u32>,
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

/// Where a written type is being lowered from: the body of a `type`
/// declaration, or an annotation on a definition. Only an annotation may be
/// open; see [`ErrorKind::OpenDeclaredType`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    Declaration,
    Annotation,
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
        arities: HashMap::new(),
        params: HashMap::new(),
    };
    let mut program = Program {
        terms: IndexMap::new(),
        types: IndexMap::new(),
    };
    // Split before lowering rather than lowering in the order written: every
    // type is declared before any term is looked at, so a term can name a type
    // written anywhere in the program. Terms keep the order they were written
    // in, so a term can still only name a term above it — the hoist is over
    // the type group, not over the terms.
    let mut types = Vec::new();
    let mut terms = Vec::new();
    for stmt in stmts {
        match stmt.tracked {
            StmtKind::Type { name, params, body } => types.push((name, params, body)),
            StmtKind::Let { name, ty, body } => terms.push((name, ty, body)),
        }
    }
    // Every type's name is bound before any type's body is read, so a type can
    // name itself and two types can name each other. That is the whole of what
    // makes a recursive type writable: nothing downstream ties the knot, and
    // nothing downstream can, so a type is recursive exactly when a
    // declaration says it is. Names are bound in the order they were written,
    // so a repeated one is still reported against the first.
    let declared: Vec<_> = types
        .iter()
        .map(|(name, _, _)| b.declare(Namespace::Types, name))
        .collect();
    // How many arguments each declaration takes, known before any body is read.
    // That is what makes a forward reference applicable: `type A = B Nat` above
    // `type B x = ...` has to be counted, and counting it cannot wait for `B`
    // to be lowered.
    b.arities = declared
        .iter()
        .zip(&types)
        .filter_map(|(symbol, (_, params, _))| Some(((*symbol)?, params.len())))
        .collect();
    for (symbol, (name, params, body)) in declared.into_iter().zip(types) {
        // The parameters are bound for the length of the body and released
        // after it, the way a lambda's argument is — this is the type
        // language's only binder, and its only scope.
        let mark = b.types.mark();
        let bound = b.bind_params(params);
        let value = b.ty(body, Place::Declaration);
        b.types.release(mark);
        if let Some(symbol) = symbol {
            program.types.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation: None,
                    params: bound,
                    value,
                },
            );
        }
    }
    // A loop of bare names is the one recursion that cannot be allowed, and it
    // is what mutual visibility just made writable. See [`ErrorKind::Circular`]
    // for why it means nothing, and [`Solve::unify`](crate::inference) for what
    // it would cost the solver to be handed one.
    let circular: Vec<_> = program
        .types
        .keys()
        .copied()
        .filter(|symbol| returns_to_itself(&program.types, *symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        b.error(span, ErrorKind::Circular);
    }
    // The other recursion the solver cannot be handed: one that changes its
    // arguments on the way round. See [`ErrorKind::NonUniformRecursion`].
    for (symbol, at) in non_uniform(&program.types) {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        b.error(at, ErrorKind::NonUniformRecursion);
    }
    // What each parameter stands for, which only the finished bodies can say: a
    // parameter handed straight on to another declaration takes its kind from
    // there, so no one body decides its own.
    let (kinds, clashes) = kinds(&program.types);
    b.errors.extend(clashes);
    for (symbol, kinds) in &kinds {
        for (param, kind) in program.types[symbol].params.iter_mut().zip(kinds) {
            param.kind = *kind;
        }
    }
    for (name, ty, body) in terms {
        // Annotation and body are lowered in the order they were written, and
        // the body before the name is bound, so a definition cannot see itself
        // and there is no recursion to resolve.
        let annotation = ty.map(|ty| b.ty(ty, Place::Annotation));
        let value = b.term(body.tracked);
        if let Some(symbol) = b.declare(Namespace::Terms, &name) {
            program.terms.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation,
                    // A term binds no parameters of its own: a lambda's
                    // argument is bound inside its body, not by the definition.
                    params: Vec::new(),
                    value,
                },
            );
        }
    }
    // What the arguments handed to a row parameter are allowed to be. Last of
    // all, because an annotation is as much a place to write one as a
    // declaration's body is, and annotations are only just lowered.
    b.errors.extend(row_arguments(&program, &kinds));
    Output {
        program,
        errors: b.errors,
    }
}

/// Whether following what `start` is declared as, through nothing but names,
/// arrives back at `start`.
///
/// Only a body that is a name outright is followed. A type with any structure
/// to it — `type t = { next: t }`, `type t = t -> Nat` — says what it is one
/// step in, and the loop through it is the recursion this language is for; it
/// is only a name standing for a name standing for the first that never says
/// anything. The walk is bounded by the number of declarations there are,
/// since a longer chain has already passed through one of them twice.
fn returns_to_itself(types: &IndexMap<Symbol, Decl<Type>>, start: Symbol) -> bool {
    let mut at = start;
    for _ in 0..types.len() {
        let Some(decl) = types.get(&at) else {
            return false;
        };
        let next = match &decl.value.tracked {
            TypeKind::Ident(next) => *next,
            // Applying a name is still only a name: `type A a = B a` says what
            // `A` is exactly as much as `type A = B` does, which is nothing.
            // The arguments are handed on rather than looked at, because they
            // cannot make the body say anything the head does not.
            TypeKind::Apply { head, .. } => *head,
            // Including a body that is a parameter. `type A a = a` is the
            // identity, useless but meaningful: what it stands for is whatever
            // the caller wrote, which is a shape one step in.
            _ => return false,
        };
        if next == start {
            return true;
        }
        at = next;
    }
    false
}

/// What each parameter of each declaration stands for, worked out from how the
/// bodies use them, and every parameter used both ways.
///
/// A parameter is written bare, so its kind is read off its uses: a name in a
/// `..` tail stands for a row, a name anywhere else stands for a type, and a
/// name handed to another declaration stands for whatever that declaration's
/// parameter in that position stands for. The third is what makes this an
/// inference rather than a scan — declarations are hoisted and may name each
/// other, so `type A x = B x` and `type B y = A y` constrain each other in a
/// circle — and it is why the slots are joined rather than assigned. Joining
/// needs no order and no fixpoint; a circle just means two slots share a root.
///
/// A slot never joined to either constant is a parameter nothing said anything
/// about, and stands for a type: `type Ghost a = Nat` takes a type, because
/// that is what a reader writing `Ghost Nat` will expect and there is nothing
/// to contradict it.
fn kinds(types: &IndexMap<Symbol, Decl<Type>>) -> (HashMap<Symbol, Vec<ParamKind>>, Vec<Error>) {
    // Slot 0 is `Type` and slot 1 is `Row`; every parameter gets one after.
    // Making the two kinds slots of their own is what lets one `join` serve
    // both "this is a row" and "these two are whatever each other is".
    const TYPE: usize = 0;
    const ROW: usize = 1;
    let mut parent: Vec<usize> = vec![TYPE, ROW];
    let mut slots: HashMap<(Symbol, u32), usize> = HashMap::new();
    for (symbol, decl) in types {
        for (index, _) in decl.params.iter().enumerate() {
            slots.insert((*symbol, index as u32), parent.len());
            parent.push(parent.len());
        }
    }

    fn find(parent: &mut [usize], at: usize) -> usize {
        let mut at = at;
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }

    // Every constraint the bodies put on a slot, gathered before any is acted
    // on, so that joining needs nothing from the walk and the walk needs
    // nothing from the table.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (symbol, decl) in types {
        constrain(
            &decl.value,
            &mut |a, b| pairs.push((a, b)),
            &|index| slots[&(*symbol, index)],
            &slots,
        );
    }

    // A parameter used both ways is reported against the parameter rather than
    // against either use: neither use is wrong on its own, and it is the
    // declaration that has to say which it meant. The slot that dragged the
    // two constants together is the one to name.
    let mut errors = Vec::new();
    let mut clashed: Vec<usize> = Vec::new();
    for (a, b) in pairs {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra == rb {
            continue;
        }
        if ra <= ROW && rb <= ROW {
            clashed.push(a.max(b));
            continue;
        }
        // A constant is always made the root, so a slot's root is its kind as
        // soon as anything has said what it is.
        match ra <= ROW {
            true => parent[rb] = ra,
            false => parent[ra] = rb,
        }
    }

    for slot in clashed {
        if let Some(((symbol, index), _)) = slots.iter().find(|(_, at)| **at == slot) {
            let param = &types[symbol].params[*index as usize];
            errors.push(Error {
                span: param.span,
                kind: ErrorKind::MixedParameter,
            });
        }
    }

    let mut out = HashMap::new();
    for (symbol, decl) in types {
        let kinds = (0..decl.params.len())
            .map(|index| {
                let slot = slots[&(*symbol, index as u32)];
                match find(&mut parent, slot) {
                    ROW => ParamKind::Row,
                    _ => ParamKind::Type,
                }
            })
            .collect();
        out.insert(*symbol, kinds);
    }
    (out, errors)
}

/// Every constraint one declaration's body puts on a kind slot, as pairs to be
/// joined. Slot 0 is `Type` and slot 1 is `Row`, as in [`kinds`].
fn constrain(
    ty: &Type,
    join: &mut impl FnMut(usize, usize),
    slot: &impl Fn(u32) -> usize,
    slots: &HashMap<(Symbol, u32), usize>,
) {
    match &ty.tracked {
        // A name reached as a type is one: this walk only descends through
        // positions a type goes in, so arriving here at all is the constraint.
        TypeKind::Param { index, .. } => join(slot(*index), 0),
        TypeKind::Struct { fields, tail } => {
            for field in fields.values() {
                constrain(&field.value, join, slot, slots);
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                join(slot(*index), 1);
            }
        }
        TypeKind::Arrow { from, to } => {
            constrain(from, join, slot, slots);
            constrain(to, join, slot, slots);
        }
        TypeKind::Apply { head, args, .. } => {
            for (at, arg) in args.iter().enumerate() {
                let callee = slots.get(&(*head, at as u32));
                match (&arg.tracked, callee) {
                    // A parameter handed straight on stands for whatever it is
                    // handed to. This is the constraint that crosses
                    // declarations, and the only one that needs solving rather
                    // than reading.
                    //
                    // Decided here and *not* descended into: argument position
                    // is not type position, and walking in would say the
                    // parameter stands for a type — which is how a row handed
                    // straight on came to look like a parameter used both ways.
                    (TypeKind::Param { index, .. }, Some(&callee)) => join(slot(*index), callee),
                    (TypeKind::Param { .. }, None) => {}
                    // A struct could be either, so it says nothing of itself:
                    // handed to a row parameter its fields are spliced in,
                    // handed to a type parameter it is a type. What is inside
                    // it still speaks, so this descends.
                    (TypeKind::Struct { .. } | TypeKind::Error, _) => {
                        constrain(arg, join, slot, slots);
                    }
                    // Anything else is a type, and says so of the parameter it
                    // was handed to.
                    (_, callee) => {
                        if let Some(&callee) = callee {
                            join(callee, 0);
                        }
                        constrain(arg, join, slot, slots);
                    }
                }
            }
        }
        TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {}
    }
}

/// Every argument written where a row parameter goes that could not be one.
///
/// The kinds themselves are a well-formedness check and nothing more — a
/// struct handed to a row parameter lowers to exactly the type it would
/// anywhere else, and substitution puts it wherever the parameter sat. What
/// this refuses is the argument that would leave a row holding something no row
/// can hold, which is the invariant [`Ty::Struct`] documents and nothing else
/// enforces.
fn row_arguments(program: &Program, kinds: &HashMap<Symbol, Vec<ParamKind>>) -> Vec<Error> {
    fn walk(
        ty: &Type,
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        rows: &HashMap<Symbol, ParamKind>,
        out: &mut Vec<Error>,
    ) {
        match &ty.tracked {
            TypeKind::Apply { head, args, .. } => {
                for (at, arg) in args.iter().enumerate() {
                    let wants_row = kinds
                        .get(head)
                        .and_then(|kinds| kinds.get(at))
                        .is_some_and(|kind| *kind == ParamKind::Row);
                    if wants_row && !row_shaped(arg, rows) {
                        out.push(Error {
                            span: arg.span,
                            kind: ErrorKind::NotARow,
                        });
                    }
                    walk(arg, kinds, rows, out);
                }
            }
            TypeKind::Arrow { from, to } => {
                walk(from, kinds, rows, out);
                walk(to, kinds, rows, out);
            }
            TypeKind::Struct { fields, .. } => {
                for field in fields.values() {
                    walk(&field.value, kinds, rows, out);
                }
            }
            TypeKind::Ident(_) | TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
        }
    }

    let mut out = Vec::new();
    for decl in program.types.values() {
        // Which of this declaration's own parameters are rows, so one handed
        // straight on is recognised as one.
        let rows: HashMap<Symbol, ParamKind> = decl
            .params
            .iter()
            .map(|param| (param.symbol, param.kind))
            .collect();
        walk(&decl.value, kinds, &rows, &mut out);
    }
    // An annotation binds no parameters, so nothing in one can be a row by
    // being a parameter — but it is every bit as much a place to apply a
    // declaration, and was the way this check was first written round.
    for decl in program.terms.values() {
        if let Some(annotation) = &decl.annotation {
            walk(annotation, kinds, &HashMap::new(), &mut out);
        }
    }
    out
}

/// Whether a written type could stand for the fields a row does not name.
///
/// A struct can: its fields are spliced in where the tail was. A row parameter
/// can, by being one already. A declared name cannot, though it looks as though
/// it should — a tail holding a name would have to be unfolded by the two walks
/// that flatten rows, which neither does, so it is refused rather than silently
/// mishandled.
fn row_shaped(ty: &Type, rows: &HashMap<Symbol, ParamKind>) -> bool {
    match &ty.tracked {
        TypeKind::Struct { .. } | TypeKind::Error => true,
        TypeKind::Param { symbol, .. } => rows.get(symbol) == Some(&ParamKind::Row),
        _ => false,
    }
}

/// Every place a declaration leads back to itself having changed its
/// arguments, as the declaration it was found in and the span to report at.
///
/// Two declarations are in one group when each leads to the other, and inside a
/// group every mention of a member must hand it that member's own parameters,
/// in order and unchanged. Across groups nothing is restricted:
/// `type Rose a = { kids: List (Rose a) }` is fine because `List` is somebody
/// else's group, and only the `Rose a` inside it has to be verbatim.
///
/// See [`ErrorKind::NonUniformRecursion`] for why the restriction is here, and
/// [`Solve::unfold`](crate::inference) for what rests on it.
fn non_uniform(types: &IndexMap<Symbol, Decl<Type>>) -> Vec<(Symbol, Span)> {
    // Who each declaration mentions, directly. Reachability is closed over this
    // rather than computed with a stack of its own: the table is one file long,
    // and a pair being mutually reachable is the whole of what a group is.
    let mentions: IndexMap<Symbol, Vec<Symbol>> = types
        .iter()
        .map(|(symbol, decl)| {
            let mut out = Vec::new();
            mentioned(&decl.value, &mut out);
            (*symbol, out)
        })
        .collect();

    let mut out = Vec::new();
    for (symbol, decl) in types {
        let group: Vec<Symbol> = types
            .keys()
            .copied()
            .filter(|other| {
                reaches(&mentions, *symbol, *other) && reaches(&mentions, *other, *symbol)
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        let want: Vec<Symbol> = decl.params.iter().map(|param| param.symbol).collect();
        verbatim(&decl.value, &group, &want, &mut |at| {
            out.push((*symbol, at))
        });
    }
    out
}

/// Whether `from` leads to `to` through any chain of mentions, itself included
/// when it mentions itself.
fn reaches(mentions: &IndexMap<Symbol, Vec<Symbol>>, from: Symbol, to: Symbol) -> bool {
    let mut seen = Vec::new();
    let mut stack = vec![from];
    while let Some(at) = stack.pop() {
        for next in mentions.get(&at).into_iter().flatten() {
            if *next == to {
                return true;
            }
            if !seen.contains(next) {
                seen.push(*next);
                stack.push(*next);
            }
        }
    }
    false
}

/// Every declaration a type mentions, at any depth. A parameter is not one: it
/// is a local, and no declaration answers to it.
fn mentioned(ty: &Type, out: &mut Vec<Symbol>) {
    match &ty.tracked {
        TypeKind::Ident(symbol) => out.push(*symbol),
        TypeKind::Apply { head, args, .. } => {
            out.push(*head);
            for arg in args {
                mentioned(arg, out);
            }
        }
        TypeKind::Arrow { from, to } => {
            mentioned(from, out);
            mentioned(to, out);
        }
        TypeKind::Struct { fields, .. } => {
            for field in fields.values() {
                mentioned(&field.value, out);
            }
        }
        TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
    }
}

/// Report every mention of a `group` member in `ty` that is not that member
/// applied to `want`, in order.
fn verbatim(ty: &Type, group: &[Symbol], want: &[Symbol], report: &mut impl FnMut(Span)) {
    match &ty.tracked {
        // A group member written bare is uniform exactly when the group takes
        // nothing — and if it takes something, the arity check has already
        // spoken and this would be a second complaint about one mistake.
        TypeKind::Ident(_) => {}
        TypeKind::Apply {
            head,
            head_span,
            args,
        } => {
            if group.contains(head) {
                let same = args.len() == want.len()
                    && args.iter().zip(want).all(|(arg, expected)| {
                        matches!(&arg.tracked, TypeKind::Param { symbol, .. } if symbol == expected)
                    });
                if !same {
                    report(*head_span);
                }
            }
            // The arguments are still walked: a member hidden inside one is as
            // much a way round as a member at the top.
            for arg in args {
                verbatim(arg, group, want, report);
            }
        }
        TypeKind::Arrow { from, to } => {
            verbatim(from, group, want, report);
            verbatim(to, group, want, report);
        }
        TypeKind::Struct { fields, .. } => {
            for field in fields.values() {
                verbatim(&field.value, group, want, report);
            }
        }
        TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
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
    fn declare(&mut self, namespace: Namespace, name: &TrackedString) -> Option<Symbol> {
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

    /// Bind a declaration's parameters for the length of its body, and take
    /// them as the parameter scope [`ty`](Self::ty) resolves against.
    ///
    /// Minted as locals, the way a lambda's arguments are, so a parameter is a
    /// symbol like any other and the debugger lists and cross-highlights it
    /// with no special case.
    fn bind_params(&mut self, params: Vec<TrackedString>) -> Vec<Param> {
        self.params.clear();
        let mut bound = Vec::new();
        for name in params {
            // Only against this declaration's own parameters: a parameter
            // shadowing a declared type is what a scope is for, not a repeat.
            if let Some(previous) = self
                .types
                .find(&name.tracked)
                .filter(|binding| self.params.contains_key(&binding.symbol))
                .map(|binding| binding.span)
            {
                self.error(name.span, ErrorKind::DuplicateParameter { previous });
                continue;
            }
            let symbol = self
                .mint
                .local(self.module, Namespace::Types, &name.tracked);
            self.params.insert(symbol, bound.len() as u32);
            self.types.bind(name.tracked.clone(), symbol, name.span);
            bound.push(Param {
                span: name.span,
                symbol,
                // Read off the body once every body is in; see [`kinds`].
                kind: ParamKind::Type,
            });
        }
        bound
    }

    /// What a `..` tail stands for, and the one place a declaration is allowed
    /// to be open.
    ///
    /// A name is looked for among the parameters first, because that is the
    /// only reading under which a declaration's tail means anything: what it
    /// stands for is supplied at every use rather than decided here, so the
    /// body still mentions no solver variable and every walk can still stop at
    /// a name. Anything else in a declaration — a bare `..`, or a name that
    /// binds nothing — is a question a declaration cannot leave open, and is
    /// reported here rather than by the caller, since only here is it known
    /// which of the three a tail turned out to be.
    fn row(&mut self, span: Span, name: Option<TrackedString>, place: Place) -> Row {
        let Some(name) = name else {
            if place == Place::Declaration {
                self.error(span, ErrorKind::OpenDeclaredType);
            }
            return Row::Anything;
        };
        if let Some(symbol) = self.types.get(&name.tracked)
            && let Some(&index) = self.params.get(&symbol)
        {
            return Row::Param { symbol, index };
        }
        if place == Place::Declaration {
            self.error(span, ErrorKind::OpenDeclaredType);
        }
        Row::Named(name.tracked)
    }

    /// How many arguments a declared type takes. A symbol with no entry is one
    /// whose declaration was refused, and counting against it would be a second
    /// complaint about the first thing that went wrong.
    fn arity(&self, symbol: Symbol) -> usize {
        self.arities.get(&symbol).copied().unwrap_or(0)
    }

    /// Lower `<head> <arg>...`.
    ///
    /// The arguments are lowered before the head is judged, so a bad name
    /// inside an application nobody could have applied is still reported — the
    /// precedent [`ty`](Self::ty) already sets for an open declared type.
    fn apply(
        &mut self,
        span: Span,
        head: parse::Type,
        args: Vec<parse::Type>,
        place: Place,
    ) -> Type {
        let found = args.len();
        let args: Vec<Type> = args.into_iter().map(|arg| self.ty(arg, place)).collect();
        let head_span = head.span;
        let parse::TypeKind::Ident { name } = head.tracked else {
            self.error(head_span, ErrorKind::NotAConstructor);
            return span.track(TypeKind::Error);
        };
        let Some(symbol) = self.types.get(&name.tracked) else {
            // A primitive takes nothing, so applying one is an arity complaint
            // rather than a "not a constructor": the reader wrote a type that
            // exists and gave it too much.
            if Prim::from_name(&name.tracked).is_some() {
                self.error(span, ErrorKind::Arity { expected: 0, found });
            } else {
                self.error(
                    name.span,
                    ErrorKind::Undefined {
                        namespace: Namespace::Types,
                    },
                );
            }
            return span.track(TypeKind::Error);
        };
        if self.params.contains_key(&symbol) {
            self.error(head_span, ErrorKind::ParameterApplied);
            return span.track(TypeKind::Error);
        }
        let expected = self.arity(symbol);
        if expected != found {
            // Once, at the application, and then the whole thing absorbs: a
            // wrong count makes every position after the first guesswork, and
            // pairing them up to say more would be inventing what was meant.
            self.error(span, ErrorKind::Arity { expected, found });
            return span.track(TypeKind::Error);
        }
        span.track(TypeKind::Apply {
            head: symbol,
            head_span,
            args,
        })
    }

    /// Lower a surface expression into an IR term. Multi-argument functions are
    /// curried into nested single-argument [`TermKind::Fn`]s; the parser
    /// guarantees every function binds at least one argument, so the fold is
    /// never empty.
    fn term(&mut self, expr: Expr) -> Term {
        let span = expr.span;
        match expr.tracked {
            // `()` is the empty struct rather than a form of its own, so it is
            // erased here instead of surviving into the IR. See [`Ty::Struct`]
            // for why, and for what it costs when the compiler answers.
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
    /// or a primitive. A tail's name is not a binder either — it is scoped to
    /// its annotation and resolved there by inference, so it passes through
    /// here as the string it was written as.
    fn ty(&mut self, ty: parse::Type, place: Place) -> Type {
        let span = ty.span;
        match ty.tracked {
            // As in [`term`](Self::term): the two surface spellings of the
            // empty struct, `()` and `{}`, meet here. See [`Ty::Struct`].
            parse::TypeKind::Unit => span.track(TypeKind::Struct {
                fields: Default::default(),
                tail: None,
            }),
            // A declaration is looked for before a primitive, so a `type Nat`
            // of one's own shadows the built-in rather than colliding with a
            // declaration nobody wrote. Types being hoisted, every term sees
            // such a declaration wherever it was written; a type sees only the
            // ones above it, and reaches the built-in otherwise.
            parse::TypeKind::Ident { name } => match self.types.get(&name.tracked) {
                Some(symbol) => match self.params.get(&symbol) {
                    // A parameter stands for one type outright, so a bare name
                    // is the only way to write one and there is nothing to
                    // count.
                    Some(&index) => span.track(TypeKind::Param { symbol, index }),
                    // A declaration written bare is applied to nothing, which
                    // is only enough if it takes nothing. See
                    // [`ErrorKind::Arity`].
                    None => match self.arity(symbol) {
                        0 => span.track(TypeKind::Ident(symbol)),
                        expected => {
                            self.error(name.span, ErrorKind::Arity { expected, found: 0 });
                            span.track(TypeKind::Error)
                        }
                    },
                },
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
            parse::TypeKind::Apply { head, args } => self.apply(span, *head, args, place),
            parse::TypeKind::Struct { fields, tail } => {
                // The values are lowered before openness is judged, so a bad
                // name inside an open declared type is still reported: the
                // reader should not have to fix the `..` to be told about it.
                //
                // A type's field has a `?` to it that [`Field`] has no room
                // for, so the mark rides down with the value and the pair is
                // taken apart again here. Re-keying and repeats are the same
                // question they are for a struct literal, and are asked in the
                // one place that answers it.
                let lowered: IndexMap<String, TypeField> = self
                    .fields(fields, |b, field| {
                        (field.optional, b.ty(field.value, place))
                    })
                    .into_iter()
                    .map(|(name, field)| {
                        let (optional, value) = field.value;
                        (
                            name,
                            TypeField {
                                name_span: field.name_span,
                                optional,
                                value,
                            },
                        )
                    })
                    .collect();
                let tail = tail.map(|tail| {
                    let span = tail.span;
                    Tail {
                        span,
                        of: self.row(span, tail.name, place),
                    }
                });
                // A declaration must be closed, except through a parameter;
                // see [`ErrorKind::OpenDeclaredType`]. Each `?` is its own
                // report, and the struct lowers to the error type, which
                // absorbs — the `Circular` precedent. A tail has already
                // reported for itself in [`row`](Self::row), because only
                // there is it known which of the three it is.
                if place == Place::Declaration {
                    let offending: Vec<Span> = lowered
                        .values()
                        .filter(|field| field.optional)
                        .map(|field| field.name_span)
                        .collect();
                    let open_tail = matches!(
                        &tail,
                        Some(Tail {
                            of: Row::Anything | Row::Named(_),
                            ..
                        })
                    );
                    if !offending.is_empty() || open_tail {
                        for span in offending {
                            self.error(span, ErrorKind::OpenDeclaredType);
                        }
                        return span.track(TypeKind::Error);
                    }
                }
                span.track(TypeKind::Struct {
                    fields: lowered,
                    tail,
                })
            }
            parse::TypeKind::Arrow { from, to } => {
                let from = self.ty(*from, place);
                let to = self.ty(*to, place);
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
        fields: IndexMap<TrackedString, S>,
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
