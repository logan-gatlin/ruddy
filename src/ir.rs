use std::{collections::HashMap, hash::Hash, rc::Rc};

use indexmap::{IndexMap, IndexSet};

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
        /// The `..` tail, when the struct type was written open. Inside a
        /// `type` declaration this is `Some` only for a tail naming a row
        /// parameter — see [`ErrorKind::OpenDeclaredType`].
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
    ///
    /// A declaration that stands for one of its own arguments is a link in
    /// such a chain rather than an end to it: `type A a = a` hands back
    /// whatever it was given, so `type B = A B` leads back to `B` with only a
    /// hand-off in between and reaches no shape either. What closes the loop
    /// is the whole chain, so the loop is looked for by following what each
    /// declaration stands for rather than by reading any one body.
    Circular,
    /// A `?` field inside a `type` declaration, or a `..` tail there that does
    /// not name one of the declaration's own parameters — as in
    /// `type t = { x: Nat, .. }`.
    ///
    /// What a declaration stands for is lowered once, before any definition,
    /// and holds for all of them; a `?` or a bare `..` stands for something a
    /// definition gets to decide, so there is nothing for one to mean here.
    ///
    /// A tail naming a row parameter is the exception, and the reason the rule
    /// is worth stating this precisely rather than as "a declaration is
    /// closed". What such a tail stands for is not decided here either — it is
    /// supplied at every use — so it lowers to a [`Ty::Bound`], not to a
    /// variable, and the property inference leans on survives untouched: a
    /// declaration's body mentions no solver variable, which is what lets every
    /// walk stop at a name instead of descending into what it stands for.
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
    /// A type that leads back to itself having been given an argument built out
    /// of what it takes, as in `type T a = { next: T { x: a } }`.
    ///
    /// One reason, and it is growth. Unfolding that declaration hands on
    /// `{ x: a }`, then `{ x: { x: a } }`, and so on: the argument is bigger
    /// every time and never comes back round, so there is no finite answer to
    /// whether two of them are the same type.
    ///
    /// What does come back round is allowed, and that is the whole of the rule.
    /// A parameter handed straight on is whatever came in. An argument
    /// mentioning no parameter is written out in the program and is the same
    /// type every round, so `type Forest = { head: Tree Nat }` inside `Tree`'s
    /// own group is an ordinary declaration and is accepted. See [`grows`] for
    /// the condition and [`Solve::unfold`](crate::inference) for what rests on
    /// it.
    GrowingRecursion,
    /// A parameter used both as a type and as the fields a row does not name,
    /// as in `type Bad r = { x: r, ..r }`.
    ///
    /// A parameter is written bare, so what it stands for is read off its
    /// uses. Two uses that disagree leave nothing to read, and neither of them
    /// is the wrong one — it is the declaration that has to say which it meant.
    ///
    /// The two readings can meet across declarations, when one hands its
    /// parameter to another and uses it as a type as well. The declaration
    /// told is the one that handed it on: the other one says one thing about
    /// its own parameter and is right about it.
    MixedParameter,
    /// Something that cannot stand for a set of fields, written where a row
    /// parameter goes: `WithX Nat` against `type WithX r = { x: Nat, ..r }`.
    ///
    /// A struct can stand for one, and so can another row parameter. A
    /// declared name cannot, though it looks as though it should: a tail
    /// holding a name would have to be unfolded by the walks that flatten
    /// rows, and neither does.
    ///
    /// The argument absorbs, so this is said once. Left standing it would be
    /// substituted into the tail all the same, and the reader would be told a
    /// second time in words about a row they never wrote.
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
    ///
    /// The count is the parameters the declaration bound, not the names it
    /// wrote: a repeated name binds nothing, so there is nothing in the body
    /// that could name the argument it would ask for.
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

/// What a declaration's body turns out to be once every name in the way has
/// been followed: a shape one step in, one of the declaration's own
/// parameters, or a loop that reaches neither.
#[derive(Debug, Clone, Copy)]
enum Stands {
    Shape,
    Param(u32),
    Loop,
}

/// Following what every declaration stands for, once, remembering the loops
/// closed on the way. See [`ErrorKind::Circular`] for what a loop costs.
struct Follow<'a> {
    types: &'a IndexMap<Symbol, Decl<Type>>,
    /// What each declaration was found to stand for. [`Stands::Loop`] is
    /// absorbing — anything that reduces into a loop never reaches a shape
    /// either — so a result reached under an open assumption is still the
    /// right one to keep.
    done: HashMap<Symbol, Stands>,
    /// The declarations being followed, outermost first. Meeting one again is
    /// the loop, and everything from it inwards is on that loop.
    open: Vec<Symbol>,
    /// Every declaration found to be on a loop, in the order they were found.
    looping: IndexSet<Symbol>,
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
    // Every declaration's parameters, minted before any body is read, and how
    // many arguments each declaration therefore takes. Knowing the count above
    // the declaration itself is what makes a forward reference applicable:
    // `type A = B Nat` above `type B x = ...` has to be counted, and counting
    // it cannot wait for `B` to be lowered.
    let bound: Vec<Vec<Param>> = types
        .iter()
        .map(|(_, params, _)| b.declare_params(params))
        .collect();
    b.arities = declared
        .iter()
        .zip(&bound)
        .filter_map(|(symbol, params)| Some(((*symbol)?, params.len())))
        .collect();
    for ((symbol, (name, _, body)), params) in declared.into_iter().zip(types).zip(bound) {
        // The parameters are in scope for the length of the body and released
        // after it, the way a lambda's argument is — this is the type
        // language's only binder, and its only scope.
        let mark = b.types.mark();
        b.scope_params(&params);
        let value = b.ty(body, Place::Declaration);
        b.types.release(mark);
        if let Some(symbol) = symbol {
            program.types.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation: None,
                    params,
                    value,
                },
            );
        }
    }
    // A loop of bare names is the one recursion that cannot be allowed, and it
    // is what mutual visibility just made writable. See [`ErrorKind::Circular`]
    // for why it means nothing, and [`Solve::unify`](crate::inference) for what
    // it would cost the solver to be handed one.
    // Read back off the table in declaration order, so the reports come in the
    // order the reader wrote them rather than the order the loops were found.
    let on_a_loop = looping(&program.types);
    let circular: Vec<_> = program
        .types
        .keys()
        .copied()
        .filter(|symbol| on_a_loop.contains(symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        b.error(span, ErrorKind::Circular);
    }
    // The other recursion the solver cannot be handed: one that builds a bigger
    // argument on the way round. See [`ErrorKind::GrowingRecursion`].
    for (symbol, at) in growing(&program.types) {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        b.error(at, ErrorKind::GrowingRecursion);
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
    b.errors.extend(row_arguments(&mut program, &kinds));
    Output {
        program,
        errors: b.errors,
    }
}

/// Every declaration that leads back to itself through nothing but names, in
/// the order the loops were found.
///
/// Only what a declaration stands for is followed. A type with any structure
/// to it — `type t = { next: t }`, `type t = t -> Nat` — says what it is one
/// step in, and the loop through it is the recursion this language is for; it
/// is a name standing for a name standing for the first that never says
/// anything. A name, or a parameter, which is the argument written for it:
/// `type A a = a` says no more about `type B = A B` than a bare name would,
/// because what `A` stands for is whatever it was handed.
///
/// Each declaration is followed once and remembered, which is what keeps a
/// legal nesting from looking like a loop as much as it is what makes this
/// terminate: `Id (Id Nat)` never finds `Id` still open, because the first was
/// finished before the second was reached. Only the declarations *on* a loop
/// are named — one that merely leads into one has nothing to fix.
fn looping(types: &IndexMap<Symbol, Decl<Type>>) -> IndexSet<Symbol> {
    let mut follow = Follow {
        types,
        done: HashMap::new(),
        open: Vec::new(),
        looping: IndexSet::new(),
    };
    for symbol in types.keys() {
        follow.decl(*symbol);
    }
    follow.looping
}

impl Follow<'_> {
    /// What one declaration stands for, followed once and remembered.
    fn decl(&mut self, symbol: Symbol) -> Stands {
        if let Some(stands) = self.done.get(&symbol) {
            return *stands;
        }
        // Meeting a declaration that is still being followed is the loop, and
        // everything pushed since is on it with them.
        if let Some(at) = self.open.iter().position(|open| *open == symbol) {
            self.looping.extend(self.open[at..].iter().copied());
            return Stands::Loop;
        }
        // A symbol with no declaration behind it is one whose declaration was
        // refused as a repeat; counting it against this would be a second
        // complaint about the first thing that went wrong, as in
        // [`Builder::arity`].
        let types = self.types;
        let Some(decl) = types.get(&symbol) else {
            return Stands::Shape;
        };
        self.open.push(symbol);
        let stands = self.ty(&decl.value);
        self.open.pop();
        self.done.insert(symbol, stands);
        stands
    }

    /// What one written type stands for, in the scope of the declaration being
    /// followed.
    fn ty(&mut self, ty: &Type) -> Stands {
        match &ty.tracked {
            // A shape one step in, which is all a declaration has to reach.
            // `Error` absorbs, as everywhere else.
            TypeKind::Struct { .. }
            | TypeKind::Arrow { .. }
            | TypeKind::Prim(_)
            | TypeKind::Error => Stands::Shape,
            TypeKind::Param { index, .. } => Stands::Param(*index),
            TypeKind::Ident(symbol) => self.decl(*symbol),
            TypeKind::Apply { head, args, .. } => match self.decl(*head) {
                Stands::Shape => Stands::Shape,
                Stands::Loop => Stands::Loop,
                // The head hands back one of the arguments written here, so
                // following it lands on a type still in the current scope and
                // needs no substitution to do it — which is why a parameter of
                // the head is not a parameter of anything this walk is in the
                // middle of.
                Stands::Param(index) => match args.get(index as usize) {
                    Some(arg) => self.ty(arg),
                    None => Stands::Shape,
                },
            },
        }
    }
}

/// What each parameter of each declaration stands for, worked out from how the
/// bodies use them, and every parameter used both ways.
///
/// A parameter is written bare, so its kind is read off its uses: a name in a
/// `..` tail stands for a row, a name anywhere else stands for a type, and a
/// name handed straight on to another declaration stands for whatever that
/// declaration's parameter in that position stands for. The third is what makes
/// this an inference rather than a scan — declarations are hoisted and may name
/// each other, so `type A x = B x` and `type B y = A y` constrain each other in
/// a circle.
///
/// Which is why the answer is a reachability question and not an assignment: a
/// slot stands for whatever it says of itself together with whatever every slot
/// it hands itself on to says. Reachability needs no order, so no declaration
/// can win a race by being written first, and it has a direction, so a
/// disagreement is reported against the parameter that reached both readings
/// rather than against whichever of the two a symmetric answer would have had
/// to guess between.
///
/// Handing a parameter on is a demand on the *argument*, never on the callee. A
/// use site writing `WithX Nat` says nothing about what `WithX` takes; it is
/// checked against what `WithX` takes, by [`row_arguments`]. A declaration says
/// what it takes, and a use site is not the declaration.
///
/// A slot nothing said anything about stands for a type: `type Ghost a = Nat`
/// takes a type, because that is what a reader writing `Ghost Nat` will expect
/// and there is nothing to contradict it.
fn kinds(types: &IndexMap<Symbol, Decl<Type>>) -> (HashMap<Symbol, Vec<ParamKind>>, Vec<Error>) {
    // What each body says of its own parameters, as (used as a type, used as a
    // row), and which slots each one hands itself on to. Both gathered over the
    // whole table before anything is resolved, so the walk needs nothing from
    // the answer and the answer needs nothing from the order.
    let mut said: HashMap<(Symbol, u32), (bool, bool)> = HashMap::new();
    let mut handed: IndexMap<(Symbol, u32), Vec<(Symbol, u32)>> = IndexMap::new();
    for (symbol, decl) in types {
        constrain(
            &decl.value,
            &mut |index, kind| {
                let entry = said.entry((*symbol, index)).or_default();
                match kind {
                    ParamKind::Type => entry.0 = true,
                    ParamKind::Row => entry.1 = true,
                }
            },
            &mut |index, to| handed.entry((*symbol, index)).or_default().push(to),
        );
    }
    let reachable = closure(&handed);

    let mut errors = Vec::new();
    let mut out = HashMap::new();
    for (symbol, decl) in types {
        let mut kinds = Vec::with_capacity(decl.params.len());
        for (index, param) in decl.params.iter().enumerate() {
            let slot = (*symbol, index as u32);
            let reached = reachable.get(&slot).into_iter().flatten().copied();
            let (mut as_type, mut as_row) = (false, false);
            for demand in std::iter::once(slot).chain(reached) {
                let (says_type, says_row) = said.get(&demand).copied().unwrap_or_default();
                as_type |= says_type;
                as_row |= says_row;
            }
            // A parameter used both ways is reported against the parameter
            // rather than against either use: neither use is wrong on its own,
            // and it is the declaration that has to say which it meant. The
            // declaration told is the one that reached both readings, which is
            // the one that handed the parameter on.
            if as_type && as_row {
                errors.push(Error {
                    span: param.span,
                    kind: ErrorKind::MixedParameter,
                });
            }
            // A parameter read two ways is taken as a row. It still lowers to a
            // [`Ty::Bound`] in a tail, and what a row may hold is the one thing
            // nothing downstream can recover from — so calling it a row keeps
            // [`row_arguments`] enforcing that, and keeps the declaration that
            // mixed it to the one complaint it has earned.
            kinds.push(match as_row {
                true => ParamKind::Row,
                false => ParamKind::Type,
            });
        }
        out.insert(*symbol, kinds);
    }
    (out, errors)
}

/// Everything one declaration's body says about its own parameters: which of
/// them it uses as a type, which as the rest of a row, and which it hands
/// straight on to another declaration's parameter. See [`kinds`], which
/// resolves the three into a kind apiece.
fn constrain(
    ty: &Type,
    says: &mut impl FnMut(u32, ParamKind),
    hands: &mut impl FnMut(u32, (Symbol, u32)),
) {
    match &ty.tracked {
        // A name reached as a type is one: this walk only descends through
        // positions a type goes in, so arriving here at all is the statement.
        TypeKind::Param { index, .. } => says(*index, ParamKind::Type),
        TypeKind::Struct { fields, tail } => {
            for field in fields.values() {
                constrain(&field.value, says, hands);
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                says(*index, ParamKind::Row);
            }
        }
        TypeKind::Arrow { from, to } => {
            constrain(from, says, hands);
            constrain(to, says, hands);
        }
        TypeKind::Apply { head, args, .. } => {
            for (at, arg) in args.iter().enumerate() {
                match &arg.tracked {
                    // A parameter handed straight on stands for whatever it is
                    // handed to. This is the statement that crosses
                    // declarations, and the only one that needs resolving
                    // rather than reading.
                    //
                    // Recorded here and *not* descended into: argument position
                    // is not type position, and walking in would say the
                    // parameter stands for a type — which is how a row handed
                    // straight on came to look like a parameter used both ways.
                    TypeKind::Param { index, .. } => hands(*index, (*head, at as u32)),
                    // Anything else says nothing about what the head takes.
                    // Writing `WithX Nat` is a claim about `Nat`, not about
                    // `WithX` — the argument is checked against the kind the
                    // declaration was read to have, by [`row_arguments`], and
                    // letting a use site vote here is what made that kind
                    // depend on which declaration was written first. What is
                    // inside the argument still speaks for itself, so this
                    // descends.
                    _ => constrain(arg, says, hands),
                }
            }
        }
        TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {}
    }
}

/// Every argument written where a row parameter goes that could not be one,
/// erased where it stood.
///
/// The kinds themselves are a well-formedness check and nothing more — a
/// struct handed to a row parameter lowers to exactly the type it would
/// anywhere else, and substitution puts it wherever the parameter sat. What
/// this refuses is the argument that would leave a row holding something no row
/// can hold, which is the invariant [`Ty::Struct`] documents and nothing else
/// enforces.
///
/// Refusing it is not enough on its own: an argument left standing is
/// substituted into the tail anyway, and the reader is told a second time in
/// words about a row nobody wrote. So the argument absorbs, the way
/// [`ErrorKind::Circular`] and [`ErrorKind::OpenDeclaredType`] do — the
/// argument and not the whole application, because the mistake is the argument
/// and `WithX Nat -> Nat` is half correct. [`row_shaped`] already reads
/// [`TypeKind::Error`] as row-shaped, so nothing complains about the erasure,
/// and it lowers to [`Ty::Undecided`], which a tail is already allowed to be.
fn row_arguments(program: &mut Program, kinds: &HashMap<Symbol, Vec<ParamKind>>) -> Vec<Error> {
    fn walk(
        ty: &mut Type,
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        rows: &HashMap<Symbol, ParamKind>,
        out: &mut Vec<Error>,
    ) {
        match &mut ty.tracked {
            TypeKind::Apply { head, args, .. } => {
                let head = *head;
                for (at, arg) in args.iter_mut().enumerate() {
                    let wants_row = kinds
                        .get(&head)
                        .and_then(|kinds| kinds.get(at))
                        .is_some_and(|kind| *kind == ParamKind::Row);
                    if wants_row && !row_shaped(arg, rows) {
                        let span = arg.span;
                        out.push(Error {
                            span,
                            kind: ErrorKind::NotARow,
                        });
                        // Nothing left inside to walk: what it was made of is
                        // no longer part of the program.
                        *arg = span.track(TypeKind::Error);
                        continue;
                    }
                    walk(arg, kinds, rows, out);
                }
            }
            TypeKind::Arrow { from, to } => {
                walk(from, kinds, rows, out);
                walk(to, kinds, rows, out);
            }
            TypeKind::Struct { fields, .. } => {
                for field in fields.values_mut() {
                    walk(&mut field.value, kinds, rows, out);
                }
            }
            TypeKind::Ident(_) | TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
        }
    }

    let mut out = Vec::new();
    for decl in program.types.values_mut() {
        // Which of this declaration's own parameters are rows, so one handed
        // straight on is recognised as one. Read out first, so that walking the
        // body borrows nothing the kinds are still held in.
        let rows: HashMap<Symbol, ParamKind> = decl
            .params
            .iter()
            .map(|param| (param.symbol, param.kind))
            .collect();
        walk(&mut decl.value, kinds, &rows, &mut out);
    }
    // An annotation binds no parameters, so nothing in one can be a row by
    // being a parameter — but it is every bit as much a place to apply a
    // declaration, and was the way this check was first written round.
    for decl in program.terms.values_mut() {
        if let Some(annotation) = decl.annotation.as_mut() {
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

/// Every place a declaration leads back to itself with an argument that gets
/// bigger, as the declaration it was found in and the span to report at.
///
/// Two declarations are in one group when each leads to the other, and inside a
/// group every mention of a member must hand it arguments that cannot grow —
/// see [`grows`] for what that allows. Across groups nothing is restricted:
/// `type Rose a = { kids: List (Rose a) }` is fine because `List` is somebody
/// else's group, and only the `Rose a` inside it is the group's business.
///
/// See [`ErrorKind::GrowingRecursion`] for why the restriction is here, and
/// [`Solve::unfold`](crate::inference) for what rests on it.
fn growing(types: &IndexMap<Symbol, Decl<Type>>) -> Vec<(Symbol, Span)> {
    // Who each declaration mentions, directly, and then everything each one
    // leads to. Closed once rather than walked per pair: the table is one file
    // long, and a pair being mutually reachable is the whole of what a group
    // is.
    let mentions: IndexMap<Symbol, Vec<Symbol>> = types
        .iter()
        .map(|(symbol, decl)| {
            let mut out = Vec::new();
            mentioned(&decl.value, &mut out);
            (*symbol, out)
        })
        .collect();
    let reachable = closure(&mentions);

    let mut out = Vec::new();
    for (symbol, decl) in types {
        let group: Vec<Symbol> = types
            .keys()
            .copied()
            .filter(|other| reachable[symbol].contains(other) && reachable[other].contains(symbol))
            .collect();
        if group.is_empty() {
            continue;
        }
        grows(&decl.value, &group, &mut |at| out.push((*symbol, at)));
    }
    out
}

/// Everything each node leads to through one edge or more. A node leads to
/// itself exactly when it is on a loop, which is why the set holds no node for
/// free: two nodes each leading to the other is the whole of what a group of
/// mutually recursive declarations is, and one node leading to itself is what
/// makes a lone declaration one.
///
/// One walk per node, with a set that answers in one look — the pairwise
/// question is asked about every ordered pair, and asking it that way walked
/// the table twice per pair. Generic over the node because the same closure
/// answers "which parameter slots does this one hand itself on to", in
/// [`kinds`]; insertion-ordered so that nothing downstream depends on a hash.
fn closure<T: Copy + Eq + Hash>(edges: &IndexMap<T, Vec<T>>) -> IndexMap<T, IndexSet<T>> {
    edges
        .keys()
        .map(|from| {
            let mut seen = IndexSet::new();
            let mut stack = vec![*from];
            while let Some(at) = stack.pop() {
                for next in edges.get(&at).into_iter().flatten() {
                    if seen.insert(*next) {
                        stack.push(*next);
                    }
                }
            }
            (*from, seen)
        })
        .collect()
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

/// Report every mention of a `group` member in `ty` that hands it an argument
/// which could get bigger, in order.
///
/// An argument is safe when it is one of two things:
///
/// - one of the mentioning declaration's own parameters, written bare — it is
///   then whatever came in, passed straight through;
/// - a type mentioning no parameter at all — it is then written out in full in
///   the program and is the same type every time round, however many names or
///   applications it is built from.
///
/// Anything else is a type built *out of* a parameter, and that is what grows:
/// `type T a = { next: T { x: a } }` hands on `{ x: a }`, then `{ x: { x: a } }`,
/// and never comes back round.
///
/// So the arguments a group can reach are drawn from the arguments it was given
/// at the use site plus the finitely many param-free types written inside it —
/// a finite set, and therefore finitely many argument lists, which is what makes
/// the solver's assumption repeat. Order and repetition are free:
/// `type A a b = { x: B b a }` only ever permutes what it was handed. See
/// [`ErrorKind::GrowingRecursion`] and [`Solve::unfold`](crate::inference).
fn grows(ty: &Type, group: &[Symbol], report: &mut impl FnMut(Span)) {
    match &ty.tracked {
        // A group member written bare hands on nothing — and if it takes
        // something, the arity check has already spoken and this would be a
        // second complaint about one mistake.
        TypeKind::Ident(_) => {}
        TypeKind::Apply {
            head,
            head_span,
            args,
        } => {
            let safe = |arg: &Type| {
                matches!(arg.tracked, TypeKind::Param { .. }) || !mentions_a_parameter(arg)
            };
            if group.contains(head) && !args.iter().all(safe) {
                report(*head_span);
            }
            // The arguments are still walked: a member hidden inside one is as
            // much a way round as a member at the top.
            for arg in args {
                grows(arg, group, report);
            }
        }
        TypeKind::Arrow { from, to } => {
            grows(from, group, report);
            grows(to, group, report);
        }
        TypeKind::Struct { fields, .. } => {
            for field in fields.values() {
                grows(&field.value, group, report);
            }
        }
        TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
    }
}

/// Whether a written type is built out of any of the parameters of the
/// declaration whose body it is in. Every [`TypeKind::Param`] in a body is one
/// of them, since a parameter is scoped to the declaration that binds it, so
/// this asks about parameters at all rather than about a particular list.
///
/// The `..r` tail counts. It is the one place a parameter appears without being
/// a [`TypeKind::Param`] node, and a type handed on with a parameter in its tail
/// grows exactly as one with a parameter in a field does.
fn mentions_a_parameter(ty: &Type) -> bool {
    match &ty.tracked {
        TypeKind::Param { .. } => true,
        TypeKind::Apply { args, .. } => args.iter().any(mentions_a_parameter),
        TypeKind::Arrow { from, to } => mentions_a_parameter(from) || mentions_a_parameter(to),
        TypeKind::Struct { fields, tail } => {
            matches!(
                tail,
                Some(Tail {
                    of: Row::Param { .. },
                    ..
                })
            ) || fields
                .values()
                .any(|field| mentions_a_parameter(&field.value))
        }
        TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => false,
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

    /// Mint one declaration's parameters, in the order they were written.
    ///
    /// Minted as locals, the way a lambda's arguments are, so a parameter is a
    /// symbol like any other and the debugger lists and cross-highlights it
    /// with no special case. Nothing is put in scope here: the parameters of
    /// every declaration are minted before any body is read, and only the one
    /// declaration's own are in scope while its body is.
    ///
    /// A repeat is reported against the earlier name in this same list, and
    /// binds nothing — which is what makes the list this returns the number of
    /// arguments the declaration takes. Only against this declaration's own
    /// parameters: a parameter shadowing a declared type is what a scope is
    /// for, not a repeat.
    fn declare_params(&mut self, params: &[TrackedString]) -> Vec<Param> {
        let mut bound = Vec::new();
        let mut seen: Vec<(&str, Span)> = Vec::new();
        for name in params {
            if let Some(&(_, previous)) = seen.iter().find(|(seen, _)| *seen == name.tracked) {
                self.error(name.span, ErrorKind::DuplicateParameter { previous });
                continue;
            }
            seen.push((&name.tracked, name.span));
            let symbol = self
                .mint
                .local(self.module, Namespace::Types, &name.tracked);
            bound.push(Param {
                span: name.span,
                symbol,
                // Read off the body once every body is in; see [`kinds`].
                kind: ParamKind::Type,
            });
        }
        bound
    }

    /// Put one declaration's parameters in scope for the length of its body,
    /// as the parameter scope [`ty`](Self::ty) resolves against. The caller
    /// releases them; see [`build`].
    fn scope_params(&mut self, params: &[Param]) {
        self.params.clear();
        for (index, param) in params.iter().enumerate() {
            let name = self.mint.name(param.symbol).to_string();
            self.params.insert(param.symbol, index as u32);
            self.types.bind(name, param.symbol, param.span);
        }
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
    ///
    /// The scope this resolves against is pushed by the caller rather than
    /// here, because a declaration's parameters are bound for the whole of its
    /// body and this is called once per node in it. So every name reaching
    /// here is a parameter of the declaration being lowered, a top-level
    /// declaration, or a primitive — looked for in that order, since a
    /// parameter is meant to hide a declaration of the same name.
    ///
    /// A tail's name may or may not be a binder, and which it is decides what
    /// the tail means: naming a row parameter it is that parameter, and
    /// anything else is scoped to its annotation and resolved by inference, so
    /// it passes through here as the string it was written as. See
    /// [`row`](Self::row).
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
