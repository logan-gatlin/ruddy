use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    parse::{self, Expr, ExprKind, Stmt, StmtKind},
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked, TrackedString},
    types::{ParamKind, Prim, Sense, Shape, Ty},
};

#[derive(Debug, Clone)]
pub struct Program {
    pub terms: IndexMap<Symbol, Decl<Term>>,
    pub types: IndexMap<Symbol, Decl<Type>>,
    /// The definitions split into the smallest sets that have to be typed
    /// together, earliest first. See [`Group`] and [`grouping`].
    pub groups: Vec<Group>,
}

/// A set of definitions that all have to be typed at once, because each of them
/// is reachable from every other through the names their values mention.
///
/// This is what buys recursion without costing let-polymorphism. Every
/// definition in one group is monomorphic to every other while the group is
/// being solved — that is what lets a definition name itself — and each is
/// generalized once the group is done, so a definition that shares its group
/// with nothing keeps exactly the scheme it would have had if nothing here
/// existed. Treating the whole file as one group would type-check every
/// recursion and destroy the polymorphism of everything else.
#[derive(Debug, Clone)]
pub struct Group {
    /// The definitions in this group, in source order.
    pub members: Vec<Symbol>,
    /// Whether anything in the group refers back into it.
    ///
    /// Every group of two or more is one, since being a group at all is being
    /// mutually reachable. A group of one is only recursive when the definition
    /// names itself, and nothing about the members can say which — hence the
    /// flag, rather than a reader of this working it out again from a graph
    /// this is the published form of.
    pub recursive: bool,
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
    /// Whether the argument written here survives unfolding: whether it reaches
    /// a position of what the declaration stands for, rather than being handed
    /// to something that throws it away.
    ///
    /// `a` in `type Box a = { it: a }` does, and `a` in `type Ptr a = Nat` does
    /// not. The difference is the whole of what
    /// [`Rule::Congruent`](crate::inference::Rule) may be taken on: comparing
    /// two applications argument by argument agrees with comparing what they
    /// stand for exactly when every argument reaches the body, so a
    /// declaration with an argument that does not is compared by unfolding
    /// like any other type. See [`relevance`] for how it is worked out and
    /// [`Core::Named`] for what rests on it.
    ///
    /// Not known while the body is being lowered either — it follows from what
    /// every *other* declaration does with what it is handed — so it is `false`
    /// until the fixpoint has run, which is the reading that decides nothing.
    pub relevant: bool,
}

#[derive(Debug, Clone)]
pub struct Term {
    /// What the term was inferred to be. Lowering runs before inference, so
    /// until then this is [`Ty::default`], the undecided type — see
    /// [`TermKind::with_span`].
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
    /// `` `Some 1 `` — one case of a sum, with what it carries.
    ///
    /// The name stays a string for the reason [`Field`]'s keys do: it is a
    /// label scoped to whichever sum turns out to be on the other side, not a
    /// path anything can refer to, so there is no symbol to resolve it to and
    /// nothing here can fail to resolve.
    ///
    /// A case carrying nothing keeps its `None` rather than being handed a
    /// `{}` here. `()` and `{}` are two spellings of one *written* type and so
    /// meet in this pass; `` `None `` writes no type at all, and inventing one
    /// would put a node on the page the reader never wrote. What it means is
    /// unit all the same, which [`inference`](crate::inference) says where it
    /// builds the type rather than the tree.
    Tag {
        name: TrackedString,
        payload: Option<Box<Term>>,
    },
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
    /// `` `Some T | `None `` — a sum type, as the cases it allows.
    ///
    /// The struct's twin down to the [`Tail`]: what a `..` stands for, and
    /// what it may be inside a declaration, is the same question about cases
    /// that it is about fields.
    Sum {
        cases: IndexMap<String, SumCase>,
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
        /// Where the head was written, for the complaints and the rows that are
        /// about the name alone: the growth [`grows`] finds is a property of the
        /// declaration being mentioned, and the debugger gives the head a row of
        /// its own to cross-highlight against what it names.
        ///
        /// An arity complaint is deliberately *not* one of those. A wrong count
        /// is about the whole application — counting the arguments is the thing
        /// the reader has to do, and underlining four characters of a name says
        /// nothing about how many follow it. See [`Builder::apply`].
        head_span: Span,
        args: Vec<Type>,
    },
    /// A parameter of the declaration this type is the body of.
    ///
    /// Both the symbol and the position, because the two readers want
    /// different things: the debugger names it and cross-highlights it, and
    /// inference substitutes for it by position — which is [`Core::Bound`]
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

/// One case of a sum type: the [`Field`] split of spans, whether the case was
/// marked `?` — a case a value may or may not be — and what it carries.
///
/// `payload` keeps the `None` the parser gave it, for the reason
/// [`TermKind::Tag`] does: a case written bare means unit, and saying so here
/// would be this pass writing a type nobody wrote into the tree the debugger
/// shows.
#[derive(Debug, Clone)]
pub struct SumCase {
    pub name_span: Span,
    pub optional: bool,
    pub payload: Option<Type>,
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
    /// A second case of a name in one sum.
    DuplicateCase,
    /// A definition given as a name that leads back to itself with nothing in
    /// between: `type t = t`, `let x = x`, or a pair each given as the other.
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
    ///
    /// One rule about both namespaces, worded twice. A term is the same
    /// mistake made about values: `let f = fn n => f n` names itself through a
    /// shape and is an ordinary recursive function, and `let x = x` reaches no
    /// shape at all and so says nothing about what `x` is. The namespace is
    /// carried for the wording, the way [`ErrorKind::Undefined`] carries one.
    Circular {
        namespace: Namespace,
    },
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
    /// supplied at every use — so it lowers to a [`Core::Bound`], not to a
    /// variable, and the property inference leans on survives untouched: a
    /// declaration's body mentions no solver variable, which is what lets every
    /// walk stop at a name instead of descending into what it stands for.
    ///
    /// The shape is carried for the wording alone: `` type X = `A? Nat `` is the
    /// same mistake made about cases, and a complaint that said "fields" to
    /// someone who wrote backticks would be describing a type they never wrote.
    OpenDeclaredType {
        shape: Shape,
    },
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
    /// One name given to two rests of different shapes in one written type, as
    /// in `{ x: Nat, ..r } -> (`A Nat | ..r)`.
    ///
    /// Naming a tail is for saying that two `..`s stand for the same rest, and
    /// a rest is a set of labels of one kind: the fields a struct does not
    /// write out, or the cases a sum does not. One name cannot be both, and
    /// which of the two was meant is the writer's to say — so this is reported
    /// at the second use, the one that brought the two together, the way a
    /// mixed parameter is reported at the parameter.
    ///
    /// The row absorbs, for the reason every other row mistake does: left
    /// standing, the tail would be shared anyway, and a field would come back
    /// out of the solve as a case with the reader never told why.
    ///
    /// Where the name was first used is carried the way a repeat carries the
    /// definition it repeats — see [`ErrorKind::Duplicate`] — because half of
    /// what went wrong is somewhere else on the page, and a reader shown only
    /// the second `..` has to hunt for the first one themselves.
    MixedTail {
        first: Shape,
        second: Shape,
        previous: Span,
    },
    /// A parameter used as more than one of the things a parameter can be —
    /// a whole type, the rest of a struct, the rest of a sum — as in
    /// `type Bad r = { x: r, ..r }`.
    ///
    /// A parameter is written bare, so what it stands for is read off its
    /// uses. Two uses that disagree leave nothing to read, and neither of them
    /// is the wrong one — it is the declaration that has to say which it meant.
    ///
    /// The two readings can meet across declarations, when one hands its
    /// parameter to another and uses it as a type as well. The declaration
    /// told is the one that brought them together: one that merely hands the
    /// parameter on to a declaration already broken says one thing about its
    /// own and is right about it, however long the chain of them is.
    ///
    /// Every declaration the clash reaches is erased all the same, told or not.
    /// What such a body would stand for is exactly what could not be worked
    /// out, and leaving it standing would put whatever a use site handed it
    /// into a row — so the declaration absorbs, and nothing is asked of the
    /// arguments written at it either.
    ///
    /// The two readings are carried so the complaint can name them. A
    /// parameter read all three ways still names two: there is one thing to
    /// fix, and a sentence listing every way the declaration is wrong is not a
    /// better instruction for fixing it.
    MixedParameter {
        first: Sense,
        second: Sense,
    },
    /// Something that cannot stand for the rest of a row of this shape, written
    /// where a row parameter goes: `WithX Nat` against
    /// `type WithX r = { x: Nat, ..r }`.
    ///
    /// A row of the same shape can stand for one, and so can another row
    /// parameter of that shape. A struct written where a sum's tail goes
    /// cannot, and neither can a declared name, though the latter looks as
    /// though it should: a tail holding a name would have to be unfolded by the
    /// walks that flatten rows, and neither does.
    ///
    /// The argument absorbs, so this is said once. Left standing it would be
    /// substituted into the tail all the same, and the reader would be told a
    /// second time in words about a row they never wrote.
    NotARow {
        shape: Shape,
    },
    /// A row written where a row parameter goes, naming a field the
    /// declaration it is handed to already names: `WithX { x: Nat }` against
    /// `type WithX r = { x: Nat, ..r }`.
    ///
    /// A `..` covers the fields its row does not write out, so a row spliced
    /// into one may not write out any of them: the type would name the field
    /// twice, and the two copies could disagree. Which labels those are is
    /// part of what the parameter stands for — see [`ParamKind::Row`] — so it
    /// is known here, at the argument, rather than only wherever something
    /// later happened to flatten the row.
    ///
    /// The argument absorbs, for the reason [`ErrorKind::NotARow`] does: left
    /// standing it would be substituted into the tail all the same, and the
    /// reader would be told a second time about a row nobody wrote.
    RepeatedRowField {
        shape: Shape,
        field: String,
    },
}

#[derive(Debug, Clone)]
pub struct Output {
    pub program: Program,
    pub errors: Vec<Error>,
}

/// The namespaces the surface grammar writes into. [`Namespace`] has a third —
/// modules — and this language has no syntax that reaches it, so a builder
/// holding two name tables is told which of the two rather than asked to rule
/// out a value it can never be handed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Terms,
    Types,
}

impl From<Scope> for Namespace {
    fn from(scope: Scope) -> Self {
        match scope {
            Scope::Terms => Namespace::Terms,
            Scope::Types => Namespace::Types,
        }
    }
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
    /// Which shape each named tail in the written type being lowered was first
    /// used at, and where. Cleared for every written type, which is the whole
    /// scope of a tail's name: `..r` twice in one annotation stands for one
    /// rest, and another annotation's `r` is unrelated.
    ///
    /// One rest is one shape, and this is what says so. See
    /// [`ErrorKind::MixedTail`].
    tails: HashMap<String, (Shape, Span)>,
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

/// Where one parameter sits: the declaration that binds it, and its position
/// in that declaration's list. The unit everything in [`kinds`] and
/// [`relevance`] is said about, since a parameter's own symbol says nothing
/// about what is handed to it.
type Slot = (Symbol, u32);

/// One thing a declaration's body says about one of its own parameters. See
/// [`constrain`], which reads them off a body, and [`kinds`], which resolves
/// the three into a kind apiece.
enum Fact {
    /// The parameter is used as this kind here. A [`ParamKind::Row`] carries
    /// the shape of the row it tails and the labels that row writes out, which
    /// are what an argument substituted for it has to be and may not name.
    Says(u32, ParamKind),
    /// The parameter is handed straight on to another declaration's slot, so
    /// it stands for whatever that slot stands for — and may not name whatever
    /// that slot may not name.
    Hands(u32, Slot),
    /// The parameter is the tail of a row written out as an argument, as `s` is
    /// in `WithX { y: Nat, ..s }`. The row goes where the callee's own tail
    /// sat, so this tail inherits the callee's obligation as well as its own
    /// row's — but the *argument* is a row rather than the parameter, so this
    /// says nothing about which of the two readings anything has.
    Tails(u32, Slot),
}

/// Everything one body says about one of its own parameters, gathered before
/// anything is resolved. [`Fact::Hands`] is what crosses declarations and lives
/// in an edge map instead; these are what a slot says of itself.
#[derive(Debug, Default)]
struct Reading {
    /// Every way the parameter was read: as a whole type, as the rest of a
    /// struct, as the rest of a sum. One of them is a parameter that means
    /// something; two or three is the clash [`ErrorKind::MixedParameter`]
    /// reports, and the set is what lets the complaint name which two.
    ///
    /// Insertion-ordered, so the reading a body states first is the one a
    /// mixed parameter is displayed as and the one a complaint names first.
    senses: IndexSet<Sense>,
    /// The labels the rows it tails write out beside it.
    lacks: IndexSet<String>,
}

/// What [`kinds`] worked out for the whole table.
struct Kinds {
    /// A kind per parameter, in the declaration's own order.
    kinds: HashMap<Symbol, Vec<ParamKind>>,
    /// Every declaration with a parameter read two ways. Its body is erased
    /// and nothing is asked of the arguments written at it: what a mixed
    /// parameter means is exactly what could not be worked out, and checking a
    /// use site against a reading nobody could settle is the second complaint
    /// about the first mistake.
    mixed: IndexSet<Symbol>,
    /// One complaint per declaration that has to say which reading it meant —
    /// which is fewer than the declarations in `mixed`, since a declaration
    /// that merely hands a broken parameter on has nothing to fix.
    errors: Vec<Error>,
}

/// What a body turns out to be once every name in the way has been followed: a
/// shape one step in, one of the declaration's own parameters, or a loop that
/// reaches neither.
///
/// [`Stands::Param`] is a declaration's answer alone. A definition binds no
/// parameters, so [`Chain`] only ever reaches the other two.
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

/// [`Follow`] about definitions: following what each one's value stands for,
/// once, remembering the loops closed on the way.
///
/// The twin rather than the same walk, because the two languages are different
/// enough that one walk over both would be a match on which it was in every
/// arm. What they share is the shape: a memo, a stack of what is still open,
/// and the set of everything found on a loop.
struct Chain<'a> {
    terms: &'a IndexMap<Symbol, Decl<Term>>,
    /// What each definition was found to stand for; [`Stands::Loop`] absorbs,
    /// exactly as it does for [`Follow`].
    done: HashMap<Symbol, Stands>,
    /// The definitions being followed, outermost first.
    open: Vec<Symbol>,
    /// Every definition found to be on a loop, in the order they were found.
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
        tails: HashMap::new(),
    };
    let mut program = Program {
        terms: IndexMap::new(),
        types: IndexMap::new(),
        groups: Vec::new(),
    };
    // Split before lowering rather than lowering in the order written: every
    // type is declared before any term is looked at, so a term can name a type
    // written anywhere in the program. Both halves keep the order they were
    // written in, and both are hoisted over themselves, so where a name is
    // written decides nothing about what can see it.
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
        .map(|(name, _, _)| b.declare(Scope::Types, name))
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
        let value = b.written(body, Place::Declaration);
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
        b.error(
            span,
            ErrorKind::Circular {
                namespace: Namespace::Types,
            },
        );
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
    let Kinds {
        mut kinds,
        mixed,
        errors: clashes,
    } = kinds(&program.types);
    b.errors.extend(clashes);
    for (symbol, kinds) in &kinds {
        for (param, kind) in program.types[symbol].params.iter_mut().zip(kinds) {
            param.kind = kind.clone();
        }
    }
    // A declaration whose parameter could not be read one way is erased, the
    // way a circular one is, and dropped from the table the use sites are
    // checked against. Both halves are the absorbing: a body left standing
    // would put whatever a use site handed it into a tail, which is the one
    // thing nothing downstream can recover from, and a slot left in the table
    // would complain at every ordinary type written at it — three complaints
    // about code the reader got right, for one mistake somewhere else.
    for symbol in &mixed {
        let decl = &mut program.types[symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        kinds.remove(symbol);
    }
    // Every definition's name is bound before any definition's body is read —
    // the hoist the `type` half above already gets, and for the same reason.
    // That is the whole of what makes a definition able to name itself and two
    // definitions able to name each other; nothing downstream ties the knot,
    // so a definition is recursive exactly when its body says so. Names are
    // bound in the order they were written, so a repeated one is still
    // reported against the first, and the first is still the one that stands.
    let defined: Vec<_> = terms
        .iter()
        .map(|(name, _, _)| b.declare(Scope::Terms, name))
        .collect();
    for (symbol, (name, ty, body)) in defined.into_iter().zip(terms) {
        // Annotation and body are lowered in the order they were written. A
        // repeat's body is lowered like any other, though nothing keeps it:
        // a bad name inside one is still the reader's to fix.
        let annotation = ty.map(|ty| b.written(ty, Place::Annotation));
        let value = b.term(body.tracked);
        if let Some(symbol) = symbol {
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
    // The term half of the loop refused above, and refused for the same
    // reason: a value given as a name that leads back to itself is never given
    // one. Read back off the table in definition order, so the reports come in
    // the order the reader wrote them.
    let circling = circling(&program.terms);
    let circular: Vec<_> = program
        .terms
        .keys()
        .copied()
        .filter(|symbol| circling.contains(symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.terms[&symbol];
        let span = decl.value.span;
        decl.value = TermKind::Error.with_span(span);
        b.error(
            span,
            ErrorKind::Circular {
                namespace: Namespace::Terms,
            },
        );
    }
    // Which definitions have to be typed together, read off the values as they
    // finally stand — so a refused loop is a group of one naming nobody rather
    // than the group its erased value used to describe.
    program.groups = grouping(&program.terms);
    // What the arguments handed to a row parameter are allowed to be. Last of
    // all, because an annotation is as much a place to write one as a
    // declaration's body is, and annotations are only just lowered.
    b.errors.extend(row_arguments(&mut program, &kinds));
    // Which parameters survive unfolding, read off the bodies as they finally
    // stand: every erasure above is a position a parameter no longer reaches,
    // and calling one relevant that nothing keeps would let the solver decide
    // by a name what unfolding decides otherwise.
    let relevant = relevance(&program.types);
    for (symbol, decl) in program.types.iter_mut() {
        for (index, param) in decl.params.iter_mut().enumerate() {
            param.relevant = relevant.contains(&(*symbol, index as u32));
        }
    }
    // Every complaint in the order the reader would meet it. The passes above
    // do not run in source order and cannot — what a parameter stands for is
    // not known until every body is in, and what a row argument may be is not
    // known until every annotation is — so the whole list is put back in the
    // one place that has all of it. The sort is stable, so two complaints about
    // one span keep the order the passes found them in.
    //
    // Said here rather than left to each reporter: the debugger sorts its
    // diagnostics for its own reasons, and a driver that prints them in the
    // order they arrive should not be the only one telling the reader about
    // line 3 before line 2.
    b.errors.sort_by_key(|error| error.span.start);
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
        // Every symbol a body can name was declared, and every declaration that
        // was made is in this table: a name that repeats one binds nothing and
        // so is never written into a type at all.
        let decl = &self.types[&symbol];
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
            | TypeKind::Sum { .. }
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
                // Indexed rather than looked up: the arity check ran where the
                // application was written, so a head standing for its own
                // parameter has an argument in that position.
                Stands::Param(index) => self.ty(&args[index as usize]),
            },
        }
    }
}

/// Every definition whose value leads back to itself through nothing but names,
/// in the order the loops were found. [`looping`] about terms.
///
/// Only what a definition's value stands for is followed, and a value with any
/// structure to it stands for itself: `fn`, a struct, a tag, a call, a literal
/// all say what the definition is one step in, and a name that leads to one of
/// them has been given a value through it. `let f = fn n => f n` is the shape
/// of recursion this language is for, and it ends the chain at the `fn`. What
/// never says anything is a name standing for a name standing for the first.
///
/// A projection is a shape too, though it looks like a hand-off. `let a = b.x`
/// reads a field out of whatever `b` is, so it is not `b`; a loop closing
/// through one still describes a value, and whether it can be typed is
/// [`inference`](crate::inference)'s occurs check to answer.
fn circling(terms: &IndexMap<Symbol, Decl<Term>>) -> IndexSet<Symbol> {
    let mut chain = Chain {
        terms,
        done: HashMap::new(),
        open: Vec::new(),
        looping: IndexSet::new(),
    };
    for symbol in terms.keys() {
        chain.def(*symbol);
    }
    chain.looping
}

impl Chain<'_> {
    /// What one definition stands for, followed once and remembered.
    fn def(&mut self, symbol: Symbol) -> Stands {
        if let Some(stands) = self.done.get(&symbol) {
            return *stands;
        }
        // Meeting a definition that is still being followed is the loop, and
        // everything pushed since is on it with them.
        if let Some(at) = self.open.iter().position(|open| *open == symbol) {
            self.looping.extend(self.open[at..].iter().copied());
            return Stands::Loop;
        }
        // Every symbol a value can name bare was defined, and every definition
        // that was made is in this table: a name that repeats one binds nothing
        // and so is never written into a term at all, and a lambda's argument
        // is only in scope under the `fn` that binds it, which this walk stops
        // at.
        let decl = &self.terms[&symbol];
        self.open.push(symbol);
        let stands = self.value(&decl.value);
        self.open.pop();
        self.done.insert(symbol, stands);
        stands
    }

    /// What one value stands for. Every kind but a bare name is a shape, which
    /// is the whole rule.
    fn value(&mut self, term: &Term) -> Stands {
        match &term.kind {
            TermKind::Ident(symbol) => self.def(*symbol),
            TermKind::Apply { .. }
            | TermKind::Fn { .. }
            | TermKind::Struct(_)
            | TermKind::Tag { .. }
            | TermKind::Project { .. }
            | TermKind::Natural(_)
            | TermKind::Error => Stands::Shape,
        }
    }
}

/// Which definitions have to be typed together, and in which order.
///
/// A group is a set of definitions each of which is reachable from every other
/// through the names their values mention — the strongly connected components
/// of the reference graph — and the groups come back in dependency order, so
/// that a definition is always solved after everything it names outside its own
/// group. That is what lets an earlier group's definition be instantiated at a
/// use site rather than shared with it. See [`Group`].
///
/// Only the definitions in the table are nodes. A lambda's argument is not one,
/// being no definition at all, and a name that failed to resolve became
/// [`TermKind::Error`] and mentions nobody.
///
/// Deterministic throughout, and deliberately so: the groups decide the order
/// inference runs in, so a hash anywhere in here would be a program that
/// type-checks on one run and not the next.
fn grouping(terms: &IndexMap<Symbol, Decl<Term>>) -> Vec<Group> {
    // Who each definition names, and then everything each one leads to. The
    // same closure the type half takes for the same question; a pair being
    // mutually reachable is the whole of what a group is.
    let mentions: IndexMap<Symbol, Vec<Symbol>> = terms
        .iter()
        .map(|(symbol, decl)| {
            let mut out = Vec::new();
            references(&decl.value, &mut out);
            out.retain(|named| terms.contains_key(named));
            (*symbol, out)
        })
        .collect();
    let reachable = closure(&mentions);

    // The groups, and which one each definition landed in. Walked in source
    // order, so the first definition of a group reached is its earliest and the
    // members it collects are in source order too. A definition leading back to
    // itself is what makes its group recursive — including a group of one,
    // which is the case the flag exists for, and including every group of two
    // or more, where being a group at all makes it so.
    let mut of: HashMap<Symbol, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for symbol in terms.keys() {
        if of.contains_key(symbol) {
            continue;
        }
        let reaches = &reachable[symbol];
        let members: Vec<Symbol> = terms
            .keys()
            .copied()
            .filter(|other| {
                other == symbol || (reaches.contains(other) && reachable[other].contains(symbol))
            })
            .collect();
        for member in &members {
            of.insert(*member, groups.len());
        }
        groups.push(Group {
            members,
            recursive: reaches.contains(symbol),
        });
    }

    // Which groups each group has to wait for: the ones its members name and
    // are not in. A group never waits for itself, which is what makes this a
    // graph with no loops in it — everything mutually reachable is already one
    // node here.
    let mut needs: Vec<IndexSet<usize>> = vec![IndexSet::new(); groups.len()];
    for (at, group) in groups.iter().enumerate() {
        for named in group.members.iter().flat_map(|member| &mentions[member]) {
            let other = of[named];
            if other != at {
                needs[at].insert(other);
            }
        }
    }

    // Dependency order, taking the earliest group that is ready at every step.
    // `groups` is already in order of earliest member, so the lowest index that
    // is ready is the earliest one, and two groups with nothing between them
    // come out in the order they were written.
    //
    // Every group is placed. Each round places one unless nothing is ready, and
    // nothing being ready in a graph with no loops means nothing is left.
    let mut placed = vec![false; groups.len()];
    let mut order = Vec::with_capacity(groups.len());
    while let Some(next) =
        (0..groups.len()).find(|at| !placed[*at] && needs[*at].iter().all(|need| placed[*need]))
    {
        placed[next] = true;
        order.push(next);
    }
    order.into_iter().map(|at| groups[at].clone()).collect()
}

/// Every definition a value names, at any depth, in the order it names them. A
/// lambda's argument is not one: it is a local, and no definition answers to it.
///
/// [`mentioned`] about terms, down to what it is for — the edges of the graph
/// [`grouping`] closes.
fn references(term: &Term, out: &mut Vec<Symbol>) {
    match &term.kind {
        TermKind::Ident(symbol) => out.push(*symbol),
        TermKind::Apply { func, arg } => {
            references(func, out);
            references(arg, out);
        }
        // The binder names nothing here; only the body can name anything, and
        // a use of the binder inside it is a symbol this walk pushes and
        // [`grouping`] then drops, since it is in no definition table.
        TermKind::Fn { body, .. } => references(body, out),
        TermKind::Struct(fields) => {
            for field in fields.values() {
                references(&field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                references(payload, out);
            }
        }
        TermKind::Project { base, .. } => references(base, out),
        TermKind::Natural(_) | TermKind::Error => {}
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
///
/// A row parameter carries one thing more than the reading: the labels an
/// argument written at it may not name. Those join the same way — a slot may
/// not name what it writes out beside itself, nor what any slot it hands itself
/// on to may not name — so the set falls out of the same closure with union in
/// place of a boolean or. It is finite because the labels in a program are, so
/// no amount of handing on can make it grow forever.
fn kinds(types: &IndexMap<Symbol, Decl<Type>>) -> Kinds {
    // What each body says of its own parameters, which slots each one hands
    // itself on to, and which ones sit in the tail of a row handed on. All
    // gathered over the whole table before anything is resolved, so the walk
    // needs nothing from the answer and the answer needs nothing from the
    // order.
    let mut said: HashMap<Slot, Reading> = HashMap::new();
    let mut handed: IndexMap<Slot, Vec<Slot>> = IndexMap::new();
    let mut tails: IndexMap<Slot, Vec<Slot>> = IndexMap::new();
    for (symbol, decl) in types {
        constrain(&decl.value, &mut |fact| match fact {
            Fact::Says(index, kind) => {
                let entry = said.entry((*symbol, index)).or_default();
                entry.senses.insert(kind.sense());
                if let Some((_, labels)) = kind.row() {
                    entry.lacks.extend(labels.iter().cloned());
                }
            }
            Fact::Hands(index, to) => handed.entry((*symbol, index)).or_default().push(to),
            Fact::Tails(index, to) => tails.entry((*symbol, index)).or_default().push(to),
        });
    }
    // Two closures over two edge sets, because the two questions travel
    // differently. Which reading a slot has travels only along a parameter
    // handed straight on: a row written out as an argument is a row whatever
    // the callee is, so letting that edge carry the reading would call every
    // tail inside an ordinary argument a mixed parameter. What a slot may not
    // name travels along both, since either way the argument lands where the
    // callee's tail sat.
    let reads = closure(&handed);
    let mut edges = handed.clone();
    for (slot, to) in tails {
        edges.entry(slot).or_default().extend(to);
    }
    let carries = closure(&edges);

    let mut out = HashMap::new();
    let mut mixed = IndexSet::new();
    let mut told = Vec::new();
    for (symbol, decl) in types {
        let mut kinds = Vec::with_capacity(decl.params.len());
        for (index, param) in decl.params.iter().enumerate() {
            let slot = (*symbol, index as u32);
            let read_as = senses(&reads, &said, slot);
            let mut lacks = IndexSet::new();
            for demand in reached(&carries, slot) {
                if let Some(reading) = said.get(&demand) {
                    lacks.extend(reading.lacks.iter().cloned());
                }
            }
            // A parameter used more than one way is reported against the
            // parameter rather than against any one use: no use is wrong on its
            // own, and it is the declaration that has to say which it meant. The
            // declaration told is the one that brought the readings
            // together — a declaration that merely hands the parameter on to
            // one already broken is right about its own and has nothing to
            // change, which is what the second condition says. A slot leading
            // back to itself is not below itself, so a pair that disagree
            // around a circle are each told rather than neither.
            //
            // The first two readings are the ones named. A parameter read all
            // three ways has one mistake to point at like any other, and the
            // sentence that names two of them says enough to find it.
            if read_as.len() > 1 {
                mixed.insert(*symbol);
                let below = handed.get(&slot).into_iter().flatten().any(|to| {
                    senses(&reads, &said, *to).len() > 1
                        && !reached(&carries, *to).any(|at| at == slot)
                });
                if !below {
                    told.push(Error {
                        span: param.span,
                        kind: ErrorKind::MixedParameter {
                            first: read_as[0],
                            second: read_as[1],
                        },
                    });
                }
            }
            // A parameter read more than one way is still taken as the first
            // row among them, so that the debugger and the Types tab show what
            // the body actually said of it. Nothing is enforced against it —
            // the declaration is a write-off and `mixed` says so — but calling
            // it a type would be this pass reporting one thing and displaying
            // another.
            let row = read_as.iter().find_map(|sense| match sense {
                Sense::Row(shape) => Some(*shape),
                Sense::Type => None,
            });
            kinds.push(match row {
                Some(shape) => ParamKind::Row { shape, lacks },
                None => ParamKind::Type,
            });
        }
        out.insert(*symbol, kinds);
    }
    Kinds {
        kinds: out,
        mixed,
        errors: told,
    }
}

/// A slot together with everything it leads to, which is the set every question
/// in [`kinds`] is answered over: a slot stands for whatever it says of itself
/// joined with whatever every slot it hands itself on to says.
fn reached(closed: &IndexMap<Slot, IndexSet<Slot>>, slot: Slot) -> impl Iterator<Item = Slot> + '_ {
    std::iter::once(slot).chain(closed.get(&slot).into_iter().flatten().copied())
}

/// Every way one slot is read: what it says of itself joined with what every
/// slot it hands itself on to says. More than one is the clash, so this is both
/// what [`kinds`] decides a parameter's kind from and what it asks of a slot's
/// successors to decide whether the clash it found is already somebody else's
/// to fix.
///
/// Insertion-ordered, and the order is the order the bodies were read in, so a
/// complaint naming two readings names the same two every run.
fn senses(
    reads: &IndexMap<Slot, IndexSet<Slot>>,
    said: &HashMap<Slot, Reading>,
    slot: Slot,
) -> IndexSet<Sense> {
    let mut senses = IndexSet::new();
    for demand in reached(reads, slot) {
        if let Some(reading) = said.get(&demand) {
            senses.extend(reading.senses.iter().copied());
        }
    }
    senses
}

/// Everything one declaration's body says about its own parameters. See
/// [`Fact`] for the three, and [`kinds`], which resolves them into a kind
/// apiece.
fn constrain(ty: &Type, out: &mut impl FnMut(Fact)) {
    match &ty.tracked {
        // A name reached as a type is one: this walk only descends through
        // positions a type goes in, so arriving here at all is the statement.
        TypeKind::Param { index, .. } => out(Fact::Says(*index, ParamKind::Type)),
        TypeKind::Struct { fields, tail } => {
            for field in fields.values() {
                constrain(&field.value, out);
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                // The fields written beside the tail are exactly what it may
                // not stand for: they are already named here, and a `..`
                // covers what is not.
                let lacks = fields.keys().cloned().collect();
                out(Fact::Says(
                    *index,
                    ParamKind::Row {
                        shape: Shape::Struct,
                        lacks,
                    },
                ));
            }
        }
        // The struct arm again, about cases: a payload is a type position, and
        // the cases written beside a tail are what it may not name.
        TypeKind::Sum { cases, tail } => {
            for case in cases.values() {
                if let Some(payload) = &case.payload {
                    constrain(payload, out);
                }
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                let lacks = cases.keys().cloned().collect();
                out(Fact::Says(
                    *index,
                    ParamKind::Row {
                        shape: Shape::Sum,
                        lacks,
                    },
                ));
            }
        }
        TypeKind::Arrow { from, to } => {
            constrain(from, out);
            constrain(to, out);
        }
        TypeKind::Apply { head, args, .. } => {
            for (at, arg) in args.iter().enumerate() {
                let slot = (*head, at as u32);
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
                    TypeKind::Param { index, .. } => out(Fact::Hands(*index, slot)),
                    // Anything else says nothing about what the head takes.
                    // Writing `WithX Nat` is a claim about `Nat`, not about
                    // `WithX` — the argument is checked against the kind the
                    // declaration was read to have, by [`row_arguments`], and
                    // letting a use site vote here is what made that kind
                    // depend on which declaration was written first. What is
                    // inside the argument still speaks for itself, so this
                    // descends.
                    //
                    // A row written out here is the one thing that carries
                    // something back across the same edge: its own tail ends up
                    // where the callee's tail sat, so it inherits what the
                    // callee may not name. That is an obligation and not a
                    // reading, which is why it is a [`Fact::Tails`] rather than
                    // a second [`Fact::Hands`].
                    _ => {
                        // Either shape of row: a sum written out as an argument
                        // ends up where the callee's tail sat exactly as a
                        // struct does, and skipping one of the two left a sum's
                        // row parameter never told what the declaration it is
                        // handed to already names.
                        let tail = match &arg.tracked {
                            TypeKind::Struct { tail, .. } | TypeKind::Sum { tail, .. } => {
                                tail.as_ref()
                            }
                            _ => None,
                        };
                        if let Some(Tail {
                            of: Row::Param { index, .. },
                            ..
                        }) = tail
                        {
                            out(Fact::Tails(*index, slot));
                        }
                        constrain(arg, out);
                    }
                }
            }
        }
        TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {}
    }
}

/// Every argument written where a row parameter goes that could not be one,
/// erased where it stood.
///
/// Two ways to fail, and they are the two halves of what [`ParamKind::Row`]
/// says. An argument has to be something that *can* stand for a set of fields
/// — [`row_shaped`] — and it has to be a set of fields the declaration does not
/// already name, since a `..` covers only what its row leaves out.
///
/// The kinds themselves are a well-formedness check and nothing more — a
/// struct handed to a row parameter lowers to exactly the type it would
/// anywhere else, and substitution puts it wherever the parameter sat. What
/// this refuses is the argument that would leave a row holding something no row
/// can hold, which is the invariant [`Rest`](crate::types::Rest) documents and
/// nothing else enforces, and the argument that would leave one naming a field twice, which
/// nothing downstream can recover from either.
///
/// Refusing it is not enough on its own: an argument left standing is
/// substituted into the tail anyway, and the reader is told a second time in
/// words about a row nobody wrote. So the argument absorbs, the way
/// [`ErrorKind::Circular`] and [`ErrorKind::OpenDeclaredType`] do — the
/// argument and not the whole application, because the mistake is the argument
/// and `WithX Nat -> Nat` is half correct. [`row_shaped`] already reads
/// [`TypeKind::Error`] as row-shaped, so nothing complains about the erasure,
/// and it lowers to the undecided type, which a tail is already allowed to be.
fn row_arguments(program: &mut Program, kinds: &HashMap<Symbol, Vec<ParamKind>>) -> Vec<Error> {
    fn walk(
        ty: &mut Type,
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        rows: &HashMap<Symbol, Shape>,
        out: &mut Vec<Error>,
    ) {
        match &mut ty.tracked {
            TypeKind::Apply { head, args, .. } => {
                let head = *head;
                for (at, arg) in args.iter_mut().enumerate() {
                    let row = kinds
                        .get(&head)
                        .and_then(|kinds| kinds.get(at))
                        .and_then(ParamKind::row);
                    if let Some((shape, lacks)) = row {
                        let refused = match row_shaped(arg, shape, rows) {
                            false => Some(ErrorKind::NotARow { shape }),
                            true => repeats(arg, lacks).map(|field| ErrorKind::RepeatedRowField {
                                shape,
                                field: field.clone(),
                            }),
                        };
                        if let Some(kind) = refused {
                            let span = arg.span;
                            out.push(Error { span, kind });
                            // Nothing left inside to walk: what it was made of
                            // is no longer part of the program.
                            *arg = span.track(TypeKind::Error);
                            continue;
                        }
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
            TypeKind::Sum { cases, .. } => {
                for case in cases.values_mut() {
                    if let Some(payload) = case.payload.as_mut() {
                        walk(payload, kinds, rows, out);
                    }
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
        //
        // Nothing more than the set is wanted: a parameter handed straight on
        // has already collected everything the slot it goes to may not name —
        // that is what [`kinds`] closed the sets over — so there is no second
        // condition here for it to fail.
        let rows: HashMap<Symbol, Shape> = decl
            .params
            .iter()
            .filter_map(|param| Some((param.symbol, param.kind.row()?.0)))
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

/// Whether a written type could stand for the labels a row of `shape` does not
/// name.
///
/// A row of the same shape can: its labels are spliced in where the tail was. A
/// row parameter can, by being one already — of that shape, since a struct's
/// tail stands for fields and a sum's for cases, and splicing one into the
/// other would leave a type nothing downstream could read. A declared name
/// cannot either, though it looks as though it should: a tail holding a name
/// would have to be unfolded by the two walks that flatten rows, which neither
/// does, so it is refused rather than silently mishandled.
fn row_shaped(ty: &Type, shape: Shape, rows: &HashMap<Symbol, Shape>) -> bool {
    match &ty.tracked {
        TypeKind::Error => true,
        TypeKind::Struct { .. } => shape == Shape::Struct,
        TypeKind::Sum { .. } => shape == Shape::Sum,
        TypeKind::Param { symbol, .. } => rows.get(symbol) == Some(&shape),
        _ => false,
    }
}

/// The first label a row written where a row parameter goes names that the
/// declaration already names, if any.
///
/// First in the row's own order rather than in the order the declaration wrote
/// them, so the complaint names the field a reader would reach first reading
/// the argument left to right — the rule [`Table::repeated`](crate::inference)
/// states for the same complaint reached through a variable.
///
/// Only a struct can name anything. A parameter handed straight on names
/// nothing here — what it stands for is supplied at every use of *its*
/// declaration, and already carries this condition — and an erased argument
/// absorbed a complaint already made.
fn repeats<'a>(ty: &'a Type, lacks: &IndexSet<String>) -> Option<&'a String> {
    let labels: Box<dyn Iterator<Item = &String>> = match &ty.tracked {
        TypeKind::Struct { fields, .. } => Box::new(fields.keys()),
        TypeKind::Sum { cases, .. } => Box::new(cases.keys()),
        _ => return None,
    };
    labels.into_iter().find(|name| lacks.contains(*name))
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

    // Which group each declaration is in, worked out once for the group rather
    // than once per member. A declaration reaching itself is what puts it in
    // one at all; everything mutually reachable with it is in the same one, and
    // shares the answer — so the members are collected from the first of them
    // reached and handed to the rest, and a declaration on no loop has no entry.
    let mut groups: HashMap<Symbol, Rc<[Symbol]>> = HashMap::new();
    for (symbol, reaches) in &reachable {
        if groups.contains_key(symbol) || !reaches.contains(symbol) {
            continue;
        }
        let group: Rc<[Symbol]> = reaches
            .iter()
            .copied()
            .filter(|other| {
                reachable
                    .get(other)
                    .is_some_and(|back| back.contains(symbol))
            })
            .collect();
        for member in group.iter() {
            groups.insert(*member, group.clone());
        }
    }

    let mut out = Vec::new();
    for (symbol, decl) in types {
        let Some(group) = groups.get(symbol) else {
            continue;
        };
        grows(&decl.value, group, &mut |at| out.push((*symbol, at)));
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

/// Which parameters survive unfolding: which slots the argument written at them
/// reaches a position of what the declaration stands for.
///
/// A parameter written anywhere in the body reaches one, except inside an
/// argument to another declaration — there it reaches one only if *that* slot
/// does, because unfolding the head is what decides whether the argument is
/// kept or thrown away. `a` in `type Box a = { it: a }` reaches one; `a` in
/// `type Ptr a = Nat` never appears and so reaches none; `a` in
/// `type Alias a = Ptr a` appears only where `Ptr` discards it, so it reaches
/// none either.
///
/// Which makes this a fixpoint over the declaration graph for the same reason
/// [`kinds`] is one: declarations are hoisted and may name each other, so no
/// one body decides its own answer. Each occurrence contributes the slots it
/// sits inside, all of which must survive for the occurrence to; a slot
/// survives if any one of its occurrences does. Both are monotone in a set that
/// only grows and is bounded by the parameters in the program, so the loop
/// stops. Starting from nothing is the safe start: a slot wrongly called
/// irrelevant costs an unfolding, and a slot wrongly called relevant would let
/// [`Rule::Congruent`](crate::inference::Rule) decide something unfolding
/// disagrees with.
///
/// A `..r` tail counts as an occurrence, being the one place a parameter is not
/// written as a [`TypeKind::Param`]: what is spliced in there is as much part
/// of what the declaration stands for as a field is.
fn relevance(types: &IndexMap<Symbol, Decl<Type>>) -> HashSet<Slot> {
    /// Every occurrence of a parameter in one body, as the slots it is nested
    /// inside — innermost order does not matter, since all of them have to
    /// survive.
    fn occurrences(ty: &Type, under: &mut Vec<Slot>, out: &mut impl FnMut(u32, &[Slot])) {
        match &ty.tracked {
            TypeKind::Param { index, .. } => out(*index, under),
            TypeKind::Struct { fields, tail } => {
                for field in fields.values() {
                    occurrences(&field.value, under, out);
                }
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = tail
                {
                    out(*index, under);
                }
            }
            TypeKind::Sum { cases, tail } => {
                for case in cases.values() {
                    if let Some(payload) = &case.payload {
                        occurrences(payload, under, out);
                    }
                }
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = tail
                {
                    out(*index, under);
                }
            }
            TypeKind::Arrow { from, to } => {
                occurrences(from, under, out);
                occurrences(to, under, out);
            }
            TypeKind::Apply { head, args, .. } => {
                for (at, arg) in args.iter().enumerate() {
                    under.push((*head, at as u32));
                    occurrences(arg, under, out);
                    under.pop();
                }
            }
            TypeKind::Ident(_) | TypeKind::Prim(_) | TypeKind::Error => {}
        }
    }

    let mut demands: IndexMap<Slot, Vec<Vec<Slot>>> = IndexMap::new();
    for (symbol, decl) in types {
        let mut under = Vec::new();
        occurrences(&decl.value, &mut under, &mut |index, guards| {
            demands
                .entry((*symbol, index))
                .or_default()
                .push(guards.to_vec());
        });
    }

    let mut relevant: HashSet<Slot> = HashSet::new();
    loop {
        let mut grew = false;
        for (slot, occurrences) in &demands {
            if relevant.contains(slot) {
                continue;
            }
            // An occurrence nested inside nothing is a position of the body
            // outright, which is the base case the empty `all` gives for free.
            if occurrences
                .iter()
                .any(|guards| guards.iter().all(|slot| relevant.contains(slot)))
            {
                relevant.insert(*slot);
                grew = true;
            }
        }
        if !grew {
            return relevant;
        }
    }
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
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = &case.payload {
                    mentioned(payload, out);
                }
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
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = &case.payload {
                    grows(payload, group, report);
                }
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
            tails_a_parameter(tail)
                || fields
                    .values()
                    .any(|field| mentions_a_parameter(&field.value))
        }
        TypeKind::Sum { cases, tail } => {
            tails_a_parameter(tail)
                || cases
                    .values()
                    .any(|case| case.payload.as_ref().is_some_and(mentions_a_parameter))
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

    fn names(&mut self, scope: Scope) -> &mut Names {
        match scope {
            Scope::Terms => &mut self.terms,
            Scope::Types => &mut self.types,
        }
    }

    /// Bind a top-level name to a fresh symbol. `None` when the name is already
    /// defined: the first definition is the one that stands, and the repeat is
    /// reported against it.
    fn declare(&mut self, scope: Scope, name: &TrackedString) -> Option<Symbol> {
        let namespace = Namespace::from(scope);
        if let Some(previous) = self.names(scope).find(&name.tracked).map(|b| b.span) {
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
        self.names(scope)
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
                // Both read off the bodies once every body is in; see [`kinds`]
                // and [`relevance`].
                kind: ParamKind::Type,
                relevant: false,
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
    fn row(
        &mut self,
        span: Span,
        name: Option<TrackedString>,
        place: Place,
        shape: Shape,
    ) -> Option<Row> {
        let Some(name) = name else {
            if place == Place::Declaration {
                self.error(span, ErrorKind::OpenDeclaredType { shape });
            }
            return Some(Row::Anything);
        };
        if let Some(symbol) = self.types.get(&name.tracked)
            && let Some(&index) = self.params.get(&symbol)
        {
            return Some(Row::Param { symbol, index });
        }
        if place == Place::Declaration {
            self.error(span, ErrorKind::OpenDeclaredType { shape });
        }
        // A name is one rest, and one rest is one shape. Recorded at the first
        // use and checked at every one after, so the complaint lands on the
        // tail that brought the two together rather than on whichever the
        // writer happened to put first. `None` says the row absorbs; see
        // [`ErrorKind::MixedTail`].
        match self.tails.get(&name.tracked) {
            Some(&(first, previous)) if first != shape => {
                self.error(
                    span,
                    ErrorKind::MixedTail {
                        first,
                        second: shape,
                        previous,
                    },
                );
                return None;
            }
            Some(_) => {}
            None => {
                self.tails.insert(name.tracked.clone(), (shape, span));
            }
        }
        Some(Row::Named(name.tracked))
    }

    /// Lower one whole written type — a declaration's body, or a definition's
    /// annotation — which is the scope a tail's name lives in and so the scope
    /// this clears. Every other caller of [`ty`](Self::ty) is inside one of
    /// these and shares its scope, which is exactly what makes two `..r` in one
    /// annotation stand for one rest.
    fn written(&mut self, ty: parse::Type, place: Place) -> Type {
        self.tails.clear();
        self.ty(ty, place)
    }

    /// The `..` a row was written with, lowered, or `Err` when it named a rest
    /// the written type already gave another shape to — which is the row
    /// absorbing, the way [`ErrorKind::OpenDeclaredType`] makes one absorb.
    fn tail(
        &mut self,
        tail: Option<parse::Tail>,
        place: Place,
        shape: Shape,
    ) -> Result<Option<Tail>, ()> {
        let Some(tail) = tail else {
            return Ok(None);
        };
        let span = tail.span;
        match self.row(span, tail.name, place, shape) {
            Some(of) => Ok(Some(Tail { span, of })),
            None => Err(()),
        }
    }

    /// Whether a row written here may stand as it is: a declaration must be
    /// closed, except through a parameter, and everything else may be as open
    /// as it likes. See [`ErrorKind::OpenDeclaredType`].
    ///
    /// `marks` is where each `?` was written — every one is its own report,
    /// since every one is a label the reader can close — and `tail` is the `..`
    /// the row ended with, which has already reported for itself in
    /// [`row`](Self::row), because only there is it known which of the three a
    /// tail turned out to be. A row refused here lowers to the error type,
    /// which absorbs: the `Circular` precedent.
    ///
    /// One rule for both shapes, because it is one rule. The two arms of
    /// [`ty`](Self::ty) that call it differ in the nouns they are written about
    /// and in nothing else, and a rule about openness kept in two places is one
    /// a sum can quietly stop obeying.
    fn closed(
        &mut self,
        place: Place,
        shape: Shape,
        marks: impl IntoIterator<Item = Span>,
        tail: &Option<Tail>,
    ) -> bool {
        if place != Place::Declaration {
            return true;
        }
        let marks: Vec<Span> = marks.into_iter().collect();
        let open_tail = matches!(
            tail,
            Some(Tail {
                of: Row::Anything | Row::Named(_),
                ..
            })
        );
        if marks.is_empty() && !open_tail {
            return true;
        }
        for span in marks {
            self.error(span, ErrorKind::OpenDeclaredType { shape });
        }
        false
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
    /// precedent [`ty`](Self::ty) already sets for an open declared type. The
    /// head goes the same way: something that is not a name cannot be applied,
    /// but it is still a written type, and `{ x: Bogus } Nat` should tell the
    /// reader about `Bogus` rather than make fixing the application the price
    /// of hearing about it. So it is lowered for its complaints and the result
    /// dropped, since there is nothing here for a head to be part of.
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
        let name = match head.tracked {
            parse::TypeKind::Ident { name } => name,
            // Something that is not a name cannot be applied, but it is still a
            // written type: it is lowered for its own complaints and the result
            // dropped, since there is nothing here for a head to be part of.
            written => {
                self.ty(head_span.track(written), place);
                self.error(head_span, ErrorKind::NotAConstructor);
                return span.track(TypeKind::Error);
            }
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
            // Once, at the application — the whole of it, head and arguments
            // together, because counting them is what the reader has to do and
            // a span around the name alone shows none of what was counted. Then
            // the whole thing absorbs: a wrong count makes every position after
            // the first guesswork, and pairing them up to say more would be
            // inventing what was meant. See [`TypeKind::Apply::head_span`],
            // which exists for the complaints that *are* about the name.
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
            // erased here instead of surviving into the IR. See
            // [`Core::Unit`](crate::types::Core::Unit) for why, and for what it
            // costs when the compiler answers.
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
            ExprKind::Tag { name, payload } => {
                let payload = payload.map(|payload| Box::new(self.term(*payload)));
                TermKind::Tag { name, payload }.with_span(span)
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
            // empty struct, `()` and `{}`, meet here. See
            // [`Core::Unit`](crate::types::Core::Unit).
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
                let tail = match self.tail(tail, place, Shape::Struct) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                // Where a declaration is held to being closed; see
                // [`closed`](Self::closed), which is the same check the sum
                // arm below makes.
                let marks = lowered
                    .values()
                    .filter(|field| field.optional)
                    .map(|field| field.name_span);
                if !self.closed(place, Shape::Struct, marks, &tail) {
                    return span.track(TypeKind::Error);
                }
                span.track(TypeKind::Struct {
                    fields: lowered,
                    tail,
                })
            }
            // The struct arm again, about cases. Everything it says holds
            // here — the payloads are lowered before openness is judged,
            // re-keying is the same question, and openness is literally the
            // same check — so what differs is the two nouns and the payload a
            // case may not have.
            parse::TypeKind::Sum { cases, tail } => {
                let lowered: IndexMap<String, SumCase> = self
                    .labels(cases, ErrorKind::DuplicateCase, |b, case| {
                        let payload = case.payload.map(|payload| b.ty(payload, place));
                        (case.optional, payload)
                    })
                    .into_iter()
                    .map(|(name, case)| {
                        let (optional, payload) = case.value;
                        (
                            name,
                            SumCase {
                                name_span: case.name_span,
                                optional,
                                payload,
                            },
                        )
                    })
                    .collect();
                let tail = match self.tail(tail, place, Shape::Sum) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                let marks = lowered
                    .values()
                    .filter(|case| case.optional)
                    .map(|case| case.name_span);
                if !self.closed(place, Shape::Sum, marks, &tail) {
                    return span.track(TypeKind::Error);
                }
                span.track(TypeKind::Sum {
                    cases: lowered,
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

    /// [`labels`](Self::labels) for a struct's fields, which is every caller
    /// but one.
    fn fields<S, T>(
        &mut self,
        fields: IndexMap<TrackedString, S>,
        lower: impl Fn(&mut Self, S) -> T,
    ) -> IndexMap<String, Field<T>> {
        self.labels(fields, ErrorKind::DuplicateField, lower)
    }

    /// Re-key surface labels by name, lowering each value with `lower`.
    /// The surface syntax tolerates a name appearing twice; the IR does not, so
    /// a repeat is reported at the offending name — as `repeat`, since a field
    /// and a case are two complaints — and the first occurrence is the one that
    /// survives.
    fn labels<S, T>(
        &mut self,
        labels: IndexMap<TrackedString, S>,
        repeat: ErrorKind,
        lower: impl Fn(&mut Self, S) -> T,
    ) -> IndexMap<String, Field<T>> {
        let mut lowered = IndexMap::new();
        for (name, value) in labels {
            let name_span = name.span;
            let value = lower(self, value);
            if lowered.contains_key(&name.tracked) {
                self.error(name_span, repeat.clone());
                continue;
            }
            lowered.insert(name.tracked, Field { name_span, value });
        }
        lowered
    }
}

/// Whether a row's `..` names one of its declaration's parameters. The one
/// place a parameter appears without being a [`TypeKind::Param`] node, so every
/// walk that asks about parameters has to ask about this too — and both shapes
/// of row have one.
fn tails_a_parameter(tail: &Option<Tail>) -> bool {
    matches!(
        tail,
        Some(Tail {
            of: Row::Param { .. },
            ..
        })
    )
}
