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
    /// A name given a value for the length of a body.
    ///
    /// Kept as a node of its own rather than desugared into a lambda applied
    /// to the value: an application is monomorphic, and a nested binding gets a
    /// scheme — see [`inference`](crate::inference) — so the two are not the
    /// same term. It is also what the reader wrote, which is what the IR is for.
    Let {
        /// The name this binds, and where it was written.
        name: Tracked<Symbol>,
        /// The written type, lowered, when the binding was ascribed one.
        annotation: Option<Type>,
        value: Box<Term>,
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
    /// Dispatch on what a value is: the written match, one arm per written
    /// arm, each carrying its pattern normalized — names resolved, puns
    /// expanded, grouping gone — and its body exactly once.
    ///
    /// The matrix of arms carries the whole meaning: first-match, top to
    /// bottom. Nothing here says how a machine would dispatch them — typing is
    /// defined on the written match, and compiling it to a decision tree is a
    /// post-typing phase for a future evaluator. See [`Builder::match_term`]
    /// for the checks lowering runs on the matrix.
    Match {
        scrutinee: Box<Term>,
        arms: Vec<(Pattern, Term)>,
    },
    Ident(Symbol),
    /// A natural number literal. It carries no symbol: a literal names nothing,
    /// so there is nothing for the mint to hand out.
    Natural(u128),
    /// A name that did not resolve. Lowering stays total so that one typo
    /// produces one error rather than a cascade from a dropped definition.
    Error,
}

/// A surface pattern, normalized: every binder a resolved [`Symbol`], puns
/// expanded, grouping parentheses gone. What a [`TermKind::Match`] arm keeps
/// of what the reader wrote — the structure survives, only the surface
/// conveniences are erased.
pub type Pattern = Tracked<PatternKind>;

// spans carried per node as the IR's other types do
#[derive(Debug, Clone)]
pub enum PatternKind {
    /// An identifier: binds the whole value at this position.
    Bind(Tracked<Symbol>),
    /// Field name → sub-pattern. Puns are expanded here: surface `{x}`
    /// arrives as field "x" → Bind(x's symbol).
    Struct(IndexMap<String, Field<Pattern>>),
    /// Payload `None` means "written bare": constrains the payload to
    /// unit, binding nothing — the same convention TermKind::Tag keeps.
    Tag {
        name: TrackedString,
        payload: Option<Box<Pattern>>,
    },
    Natural(u128),
    Unit,
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
/// field was marked `?` — there or not, with this type when it is — or `\` —
/// definitely not there, with no type at all.
#[derive(Debug, Clone)]
pub enum TypeField {
    /// `name[?]: T`, as written.
    Written {
        name_span: Span,
        optional: bool,
        value: Type,
    },
    /// `\name` — the label is explicitly absent, so there is no type here to
    /// carry: what the entry lowers to is [`Presence::Absent`](crate::types::Presence)
    /// with its type deliberately unconstrained. `name_span` covers the whole
    /// `\name`, which is where a complaint about the entry points.
    Absent { name_span: Span },
}

/// One case of a sum type: the [`Field`] split of spans, whether the case was
/// marked `?` — a case a value may or may not be — and what it carries; or the
/// `\` that says the case is definitely absent, [`TypeField::Absent`]'s twin.
///
/// `payload` keeps the `None` the parser gave it, for the reason
/// [`TermKind::Tag`] does: a case written bare means unit, and saying so here
/// would be this pass writing a type nobody wrote into the tree the debugger
/// shows.
#[derive(Debug, Clone)]
pub enum SumCase {
    /// `` `Name[?] [T] ``, as written.
    Written {
        name_span: Span,
        optional: bool,
        payload: Option<Type>,
    },
    /// `` \`Name `` — the case is explicitly absent, carrying nothing.
    /// `name_span` covers the whole `` \`Name ``.
    Absent { name_span: Span },
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
    /// An explicitly absent label in a composite with no `..` tail, as in
    /// `{ a: Nat, \y }` or `` `A | \`B ``.
    ///
    /// A `\` says the `..` beside it may not stand for the label, and a type
    /// with no `..` already says that of every label it does not name — so
    /// there is nothing here for the mark to rule out. Refused rather than
    /// dropped: the mark says something about a tail that is not there, and a
    /// reader who wrote it meant one of the two to change.
    ///
    /// The shape and the label are carried for the wording alone, the way
    /// [`ErrorKind::RepeatedRowField`] carries them: the complaint quotes the
    /// label the way it was written, backtick and all for a case.
    AbsentInClosed {
        shape: Shape,
        label: String,
    },
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
    /// One name given to two rests of different senses in one written type, as
    /// in `{ x: Nat, ..r } -> (`A Nat | ..r)`.
    ///
    /// Naming a tail is for saying that two `..`s stand for the same rest, and
    /// the two `..`s do not stand for the same kind of thing: a struct's is the
    /// whole type its fields sit on, and a sum's is the cases it does not write
    /// out. One name cannot be both, and which of the two was meant is the
    /// writer's to say — so this is reported at the second use, the one that
    /// brought the two together, the way a mixed parameter is reported at the
    /// parameter.
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
        first: Sense,
        second: Sense,
        previous: Span,
    },
    /// A parameter used as both of the things a parameter can be — a whole type
    /// and the rest of a sum — as in `` type M r = { g: (`A | ..r), f: r } ``.
    ///
    /// `type W r = { f: r, ..r }` is *not* one. The rest of a struct is a whole
    /// type, so both uses say the same thing about `r` and the declaration is
    /// well-formed; only a sum's rest is a second reading to disagree with.
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
    /// Something that cannot stand for the rest of a sum's cases, written where
    /// a sum's row parameter goes: `` Or Nat `` against
    /// `` type Or r = `A | ..r ``.
    ///
    /// A sum can stand for one, and so can another sum's row parameter. A struct
    /// cannot, and neither can a declared name, though the latter looks as
    /// though it should: a tail holding a name would have to be unfolded by the
    /// walks that flatten rows, and neither does.
    ///
    /// Only ever about a sum now, which is why it carries nothing: a struct's
    /// `..` is its core, and a core takes any type at all, so `WithX Nat` is
    /// well-formed. The name stays because the code is stable and renaming it
    /// would churn a code and a test file for no gain.
    ///
    /// The argument absorbs, so this is said once. Left standing it would be
    /// substituted into the tail all the same, and the reader would be told a
    /// second time in words about a row they never wrote.
    NotARow,
    /// An argument naming a label the declaration it is handed to already
    /// names: `WithX { x: Nat }` against `type WithX r = { x: Nat, ..r }`, and
    /// `` Or (`A) `` against `` type Or r = `A | ..r ``.
    ///
    /// A `..` covers the labels its own row does not write out, so what is
    /// spliced in may not write out any of them: the type would name the label
    /// twice, and the two copies could disagree. Which labels those are is part
    /// of what the parameter stands for — see [`ParamKind`] — so it is known
    /// here, at the argument, rather than only wherever something later happened
    /// to flatten the row.
    ///
    /// A struct's argument is looked at through names as well as at what it
    /// writes out, since a `..` handed a declared type ends up carrying whatever
    /// *that* carries: `WithX (WithX Nat)` names `x` twice as much as
    /// `WithX { x: Nat }` does. See [`carrying`].
    ///
    /// The argument absorbs, for the reason [`ErrorKind::NotARow`] does: left
    /// standing it would be substituted in all the same, and the reader would be
    /// told a second time about a type nobody wrote.
    RepeatedRowField {
        shape: Shape,
        field: String,
    },
    /// A declaration whose fields never run out: `type T = WithX T` against
    /// `type WithX r = { x: Nat, ..r }`, or a pair reaching each other the same
    /// way.
    ///
    /// A struct's `..` is the type's core, so a declaration written at one is a
    /// declaration in the core position of what the first stands for. Following
    /// those positions round to the declaration itself means each unfolding adds
    /// the fields written beside the `..` and finds the same `..` again: `T` has
    /// an `x`, and past it a `T`, which has an `x`, and there is no finite set of
    /// fields for `T` to have.
    ///
    /// [`ErrorKind::Circular`]'s sibling and told apart from it by one step: a
    /// loop with no such `..` on it reaches no shape at all and is `Circular`,
    /// and one with a `..` on it reaches a shape every time round and is this.
    /// `type List = { next: List }` is neither — the recursion is in a field's
    /// type and the core is unit.
    ///
    /// Carries nothing. The span is the body, and every declaration on the loop
    /// is reported, so there is nothing further to say — the same choice
    /// [`ErrorKind::Circular`] makes.
    EndlessFields,
    /// A pattern that can fail, written on a `let` — `let `Some x = opt`. A
    /// binding has no arms to fall through to, so it has to accept every
    /// value, and a tag or a number anywhere in the pattern is a value it
    /// would not.
    ///
    /// Lowering stays total: every name the pattern would have bound is still
    /// bound, to [`TermKind::Error`] values, so downstream uses resolve and
    /// one mistake makes one complaint. What made the pattern able to fail is
    /// carried so the complaint can quote it.
    RefutableBinding {
        found: Refuter,
    },
    /// An arm no value can reach: everything it could match is matched by an
    /// arm above it — a second bare `` `A `` after `` `A x ``, a duplicate
    /// literal, and the like. Reported at the unreachable arm; its body is
    /// lowered for its own complaints and then dropped, the way a duplicate
    /// definition's is.
    UnreachableArm,
    /// An arm that accepts everything — a bare name, `()`, a struct pattern
    /// with nothing refutable inside — written anywhere but last. The arms
    /// after it can never be reached, so the mistake is the placement, and it
    /// is reported at the arm that accepts everything rather than at each arm
    /// it starves.
    MisplacedCatchAll,
    /// A match that leaves values unhandled — `` match e with `A => 1 end ``
    /// written where the row stays open, or two arms whose columns are each
    /// covered while the combination is not. Reported at the match, with a
    /// concrete example of a value no arm accepts, so the reader is shown what
    /// to add an arm for rather than told an analysis failed. See [`Matrix`]
    /// for the check.
    UnhandledValues {
        witness: Witness,
    },
    /// A match on numbers with no final arm accepting the rest —
    /// `match n with 0 => a end`. A sum's cases can be listed in full; the
    /// naturals cannot, so a match testing them has to end in an arm that
    /// takes whatever number was not listed. [`ErrorKind::UnhandledValues`]
    /// with the friendlier wording numbers deserve, kept apart because the
    /// witness — some number — is not worth quoting.
    UnhandledNumbers,
    /// One match testing both numbers and tags at the same level —
    /// `match e with 0 => a | `A x => b end`. No value is both, so the match
    /// is refused here, with a complaint about what was written, rather than
    /// surfacing later as a unification failure about types nobody wrote.
    MixedMatch,
    /// One pattern binding the same name twice — `{x, x}`, `{a: x, b: x}`, or
    /// across any nesting in one pattern. Reported at the repeat; the first
    /// binding is the one the body sees. Distinct patterns — two different
    /// arms — may of course bind the same name.
    DuplicateBinding {
        name: String,
    },
}

/// What made a binding's pattern able to fail: the first tag or number found
/// in it, carried so the complaint can quote what the reader wrote. See
/// [`ErrorKind::RefutableBinding`].
#[derive(Debug, Clone)]
pub enum Refuter {
    /// A tag pattern: a value here might not be this case.
    Case(String),
    /// A natural literal: a value here might be some other number.
    Number(u128),
}

/// A concrete example of a value a match leaves unhandled, in the shape of the
/// value rather than in words, so the complaint can write it in source syntax.
/// See [`ErrorKind::UnhandledValues`] and [`Matrix::unhandled`].
#[derive(Debug, Clone)]
pub enum Witness {
    /// Any value at all: a position the arms never test, so no example is
    /// more instructive than another.
    Any,
    /// This number.
    Natural(u128),
    /// This case. `None` says any payload serves — rendered bare, the way a
    /// case carrying unit is written.
    Tag {
        name: String,
        payload: Option<Box<Witness>>,
    },
    /// A value carrying these fields. Only the fields that matter are named:
    /// a field any value serves for is left out, the way a struct pattern
    /// leaves out the fields it does not constrain.
    Struct(IndexMap<String, Witness>),
    /// A value that is none of the listed cases — what an open position's
    /// "anything else" is written as.
    Other(Vec<String>),
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
    /// One rest stands for one thing, and this is what says so. See
    /// [`ErrorKind::MixedTail`].
    tails: HashMap<String, (Sense, Span)>,
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
    /// The parameter is used as this kind here. The kind carries the labels the
    /// row it tails writes out beside it, which are what an argument
    /// substituted for it may not name.
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
    /// Every way the parameter was read: as a whole type, or as the rest of a
    /// sum. One of them is a parameter that means something; both is the clash
    /// [`ErrorKind::MixedParameter`] reports, and the set is what lets the
    /// complaint name which two.
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
    /// One of the declaration's own parameters, and whether reaching it went
    /// through a struct's fields.
    ///
    /// `type Id a = a` stands for its parameter outright, and
    /// `type WithX r = { x: Nat, ..r }` stands for its parameter *with an `x` in
    /// front of it* — the fields are written beside the `..`, and the `..` is
    /// the core, so what the declaration stands for is the argument carrying
    /// them. That is the one step that tells [`ErrorKind::EndlessFields`] from
    /// [`ErrorKind::Circular`]: a loop with such a step on it adds a field every
    /// time round.
    Param {
        index: u32,
        fields: bool,
    },
    Loop,
}

/// Following what every declaration stands for, once, remembering the loops
/// closed on the way. See [`ErrorKind::Circular`] for what a loop costs and
/// [`ErrorKind::EndlessFields`] for what the fielded kind costs.
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
    /// How many fielded steps had been taken when each open declaration was
    /// pushed. The walk is one chain — a struct answers without descending, and
    /// an application descends into exactly one argument — so every step counted
    /// since a frame was pushed is a step on the path from it, and a loop is
    /// endless exactly when that count moved.
    ///
    /// Counted rather than flagged per frame because the count only ever grows:
    /// nothing is unwound, so the mark taken at the push is the whole of what a
    /// frame has to remember.
    marks: Vec<u32>,
    /// How many fielded steps the walk has taken. See [`Follow::marks`].
    fielded: u32,
    /// Every declaration found to be on a loop, in the order they were found.
    looping: IndexSet<Symbol>,
    /// Those of them whose loop had a fielded step on it, which is the loop that
    /// never runs out of fields rather than the one that reaches no shape.
    endless: IndexSet<Symbol>,
}

/// Every declaration on a loop, split by what kind of loop it is. See
/// [`looping`].
struct Loops {
    /// Every declaration on a loop of any kind, in the order the loops were
    /// found.
    looping: IndexSet<Symbol>,
    /// Those on a loop with a fielded step on it. A subset of `looping`, and the
    /// ones told about [`ErrorKind::EndlessFields`] rather than
    /// [`ErrorKind::Circular`].
    endless: IndexSet<Symbol>,
}

/// A pattern that cannot fail: a bare name, `()`, or a struct pattern all of
/// whose fields cannot fail either. What a `let` accepts — read off a
/// normalized [`Pattern`] by [`calm`], which answers the refutability question
/// in the type: a walk over one of these never meets a test it would have to
/// call unreachable.
#[derive(Debug, Clone)]
enum Calm {
    /// Binds the whole value.
    Bind(Tracked<Symbol>),
    /// Binds nothing, and constrains the value to unit.
    Unit(Span),
    /// Reaches into fields, each with a pattern that cannot fail.
    Struct {
        span: Span,
        fields: Vec<(TrackedString, Calm)>,
    },
}

/// One step from a position of the scrutinee to a position inside it: into a
/// struct's field, or into the payload of one tag. A path of these names a
/// *position* of a match — the unit R7 types column-wise and the matrix
/// algorithm draws values from — and the payload step carries its tag because
/// different cases carry different payloads: `` `A ``'s payload and `` `B ``'s
/// are two positions, not one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Step {
    Field(String),
    Payload(String),
}

/// What one match's written arms say, position by position: the tags and
/// numbers tested at each, and where the arms stop testing. The universes the
/// matrix checks draw values from — usefulness in the sense of Maranget (JFP
/// 2007), with each tag position's universe being its listed cases when no arm
/// is irrefutable there, its listed cases plus "anything else" when one is,
/// and the numbers always infinite.
///
/// Syntactic throughout: built from the normalized patterns and consulting no
/// types. One implementation serves the three questions asked of a matrix —
/// unhandled values with a witness ([`Matrix::unhandled`]), per-arm
/// reachability ([`Matrix::reachable`]), and whether earlier arms fully handle
/// a case at a position ([`Matrix::handled`], which is what refines a binder's
/// view in inference).
pub(crate) struct Matrix {
    /// The tags and numbers tested at each position, over every arm.
    tests: HashMap<Vec<Step>, Tests>,
    /// Every position an identifier pattern sits at. A position is open —
    /// its universe holds "anything else" — exactly when one of these is a
    /// prefix of it: an arm irrefutable at a position is one whose walk meets
    /// a binder at or before it.
    binds: Vec<Vec<Step>>,
}

/// The tests one position has seen, over every arm. Both kinds are collected
/// so a position testing both can be refused; see [`Matrix::mixed`].
#[derive(Debug, Default)]
struct Tests {
    tags: IndexSet<String>,
    naturals: IndexSet<u128>,
}

/// One cell of the matrix the usefulness walk works on: a [`Pattern`] with
/// everything that only binds flattened to the one wildcard it matches as.
/// `()` and a bare name accept every value alike — what they demand of the
/// type is typing's business, and the check is syntactic — and a bare tag's
/// payload arrives as [`Mat::Wild`] the same way.
///
/// Owned rather than borrowed, because the walk builds rows no pattern wrote:
/// specializing a row appends its payload or its fields, and the question
/// [`Matrix::handled`] asks arrives as a synthetic row built from a path.
#[derive(Debug, Clone)]
enum Mat {
    /// Accepts everything: a binder, `()`, a bare tag's payload.
    Wild,
    Tag {
        name: String,
        payload: Box<Mat>,
    },
    Natural(u128),
    /// A struct pattern with at least one field. `{}` tests nothing and
    /// arrives as [`Mat::Wild`] instead, so a struct cell always has a field
    /// for the widening step to find.
    Struct(Vec<(String, Mat)>),
}

/// Where a lowered pattern's binder symbols come from.
///
/// A pattern in an expression mints its own locals and puts them in scope for
/// the body being lowered. A pattern on a top-level `let` binds names the
/// declare pass already declared — declared before any body was read, which is
/// what hoists them — so lowering consumes that list instead, in the same
/// order the declare pass walked.
enum Binders {
    /// Mint a local per name; bind everything that is not a repeat.
    Local,
    /// The declare pass's symbols, in pattern order. `None` is a name that
    /// bound nothing — a repeat — and lowering mints an unbound stand-in so
    /// the walk stays total.
    Declared(std::vec::IntoIter<Option<Symbol>>),
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
    /// The value each nested `let` in the program binds its name to.
    ///
    /// A local symbol is no definition, so the table above has no key for it —
    /// and a nested `let` given as itself is the same mistake a definition
    /// given as itself is, so the walk has to be able to follow one. Collected
    /// over the whole program before anything is followed, for the reason
    /// definitions are hoisted: which of the two maps a name comes out of is
    /// not something the walk should have to reach the binder to know.
    locals: HashMap<Symbol, &'a Term>,
    /// What each definition was found to stand for; [`Stands::Loop`] absorbs,
    /// exactly as it does for [`Follow`].
    done: HashMap<Symbol, Stands>,
    /// The definitions being followed, outermost first.
    open: Vec<Symbol>,
    /// Every definition found to be on a loop, in the order they were found.
    looping: IndexSet<Symbol>,
}

impl TypeField {
    /// Where the label was written: the name of an ordinary field, the whole
    /// `\name` of an absent one.
    pub fn name_span(&self) -> Span {
        match self {
            TypeField::Written { name_span, .. } | TypeField::Absent { name_span } => *name_span,
        }
    }

    /// The written type, when the field has one. An absent label writes none,
    /// which is what lets every walk over a struct's fields visit exactly the
    /// types that are there.
    pub fn value(&self) -> Option<&Type> {
        match self {
            TypeField::Written { value, .. } => Some(value),
            TypeField::Absent { .. } => None,
        }
    }
}

impl SumCase {
    /// Where the label was written: the [`TypeField::name_span`] of a case.
    pub fn name_span(&self) -> Span {
        match self {
            SumCase::Written { name_span, .. } | SumCase::Absent { name_span } => *name_span,
        }
    }

    /// The written payload, when the case has one — which an absent case never
    /// does, any more than a case written bare.
    pub fn payload(&self) -> Option<&Type> {
        match self {
            SumCase::Written { payload, .. } => payload.as_ref(),
            SumCase::Absent { .. } => None,
        }
    }
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
            StmtKind::Let { pattern, ty, body } => terms.push((pattern, ty, body)),
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
    let Loops { looping, endless } = looping(&program.types);
    let circular: Vec<_> = program
        .types
        .keys()
        .copied()
        .filter(|symbol| looping.contains(symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        // Two loops and one erasure. A loop with a struct's `..` on it reaches a
        // shape every time round and adds a field doing it, which is a different
        // thing gone wrong from a loop that reaches no shape at all. See
        // [`ErrorKind::EndlessFields`].
        let kind = match endless.contains(&symbol) {
            true => ErrorKind::EndlessFields,
            false => ErrorKind::Circular {
                namespace: Namespace::Types,
            },
        };
        b.error(span, kind);
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
    //
    // A pattern declares every name it binds, in the order the pattern walk
    // meets them, so the names a struct-pattern `let` takes apart are hoisted
    // like any other definition. A name a pattern repeats binds nothing here —
    // the lowering walk reports it as the pattern mistake it is, not as a
    // second definition.
    let defined: Vec<Vec<Option<Symbol>>> = terms
        .iter()
        .map(|(pattern, _, _)| {
            let mut names = Vec::new();
            pattern_names(pattern, &mut names);
            let mut seen: Vec<String> = Vec::new();
            names
                .iter()
                .map(|name| {
                    if seen.contains(&name.tracked) {
                        return None;
                    }
                    seen.push(name.tracked.clone());
                    b.declare(Scope::Terms, name)
                })
                .collect()
        })
        .collect();
    for (declared, (pattern, ty, body)) in defined.into_iter().zip(terms) {
        match pattern.tracked {
            // A bare name is exactly the definition the language has always
            // had. Annotation and body are lowered in the order they were
            // written. A repeat's body is lowered like any other, though
            // nothing keeps it: a bad name inside one is still the reader's
            // to fix.
            parse::PatternKind::Ident { name } => {
                let symbol = declared
                    .into_iter()
                    .next()
                    .expect("a bare name declares one symbol");
                let annotation = ty.map(|ty| b.written(ty, Place::Annotation));
                let value = b.term(body.tracked);
                if let Some(symbol) = symbol {
                    program.terms.insert(
                        symbol,
                        Decl {
                            name_span: name.span,
                            annotation,
                            // A term binds no parameters of its own: a lambda's
                            // argument is bound inside its body, not by the
                            // definition.
                            params: Vec::new(),
                            value,
                        },
                    );
                }
            }
            // A pattern becomes ordinary top-level definitions: a fresh one
            // holding the value — with the written annotation — and one per
            // name, in written order. A pattern that could fail is refused,
            // and its names are still defined, as error values, so downstream
            // uses resolve.
            tracked => {
                let pspan = pattern.span;
                let pattern = pspan.track(tracked);
                let annotation = ty.map(|ty| b.written(ty, Place::Annotation));
                let value = b.term(body.tracked);
                let mut binders = Binders::Declared(declared.into_iter());
                let mut seen = Vec::new();
                let mut dropped = Vec::new();
                let pattern = b.pattern(pattern, &mut seen, &mut binders, &mut dropped);
                match calm(&pattern) {
                    Some(calm) => b.destructure_stmt(calm, annotation, value, &mut program.terms),
                    None => {
                        let (at, found) = refuter(&pattern)
                            .expect("a pattern that is not calm names what refutes it");
                        b.error(at, ErrorKind::RefutableBinding { found });
                        let held = b.fresh("%value", pspan);
                        program.terms.insert(
                            held.tracked,
                            Decl {
                                name_span: pspan,
                                annotation,
                                params: Vec::new(),
                                value,
                            },
                        );
                        let mut names = Vec::new();
                        pattern_binders(&pattern, &mut names);
                        dropped.extend(names);
                    }
                }
                // The names with no position left — a refused binding's, and
                // a dropped duplicate field's — are still ordinary top-level
                // definitions, of error values, so downstream uses resolve
                // and one mistake makes one complaint.
                for name in dropped {
                    program.terms.insert(
                        name.tracked,
                        Decl {
                            name_span: name.span,
                            annotation: None,
                            params: Vec::new(),
                            value: TermKind::Error.with_span(name.span),
                        },
                    );
                }
            }
        }
    }
    // The term half of the loop refused above, and refused for the same
    // reason: a value given as a name that leads back to itself is never given
    // one. Read back off the table in definition order, so the reports come in
    // the order the reader wrote them.
    let circling = circling(&program.terms);
    // The nested bindings on a loop first, since a definition that is on one
    // too has its whole value erased below and would take them with it. Each
    // is reported where the top-level walk reports one: at the value's span.
    let mut nested_loops = Vec::new();
    for decl in program.terms.values_mut() {
        erase_circular(&mut decl.value, &circling, &mut nested_loops);
    }
    for span in nested_loops {
        b.error(
            span,
            ErrorKind::Circular {
                namespace: Namespace::Terms,
            },
        );
    }
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

/// Every declaration that leads back to itself through nothing but core
/// positions, in the order the loops were found, split by whether the loop adds
/// fields on the way round.
///
/// Only what a declaration stands for is followed. A type with any structure
/// to it — `type t = { next: t }`, `type t = t -> Nat` — says what it is one
/// step in, and the loop through it is the recursion this language is for; it
/// is a name standing for a name standing for the first that never says
/// anything. A name, or a parameter, which is the argument written for it:
/// `type A a = a` says no more about `type B = A B` than a bare name would,
/// because what `A` stands for is whatever it was handed.
///
/// A struct whose `..` names a parameter is such a hand-off too, and that is the
/// one thing this walk had to learn: `type WithX r = { x: Nat, ..r }` stands for
/// its argument with an `x` in front of it, because the `..` is the type's core.
/// So `type T = WithX T` reaches `T` again with a field added, which is a loop
/// like any other and is [`ErrorKind::EndlessFields`] rather than
/// [`ErrorKind::Circular`]. See [`Stands::Param`].
///
/// Each declaration is followed once and remembered, which is what keeps a
/// legal nesting from looking like a loop as much as it is what makes this
/// terminate: `Id (Id Nat)` never finds `Id` still open, because the first was
/// finished before the second was reached. Only the declarations *on* a loop
/// are named — one that merely leads into one has nothing to fix.
fn looping(types: &IndexMap<Symbol, Decl<Type>>) -> Loops {
    let mut follow = Follow {
        types,
        done: HashMap::new(),
        open: Vec::new(),
        marks: Vec::new(),
        fielded: 0,
        looping: IndexSet::new(),
        endless: IndexSet::new(),
    };
    for symbol in types.keys() {
        follow.decl(*symbol);
    }
    Loops {
        looping: follow.looping,
        endless: follow.endless,
    }
}

impl Follow<'_> {
    /// What one declaration stands for, followed once and remembered.
    fn decl(&mut self, symbol: Symbol) -> Stands {
        if let Some(stands) = self.done.get(&symbol) {
            return *stands;
        }
        // Meeting a declaration that is still being followed is the loop, and
        // everything pushed since is on it with them. Whether a field was added
        // on the way is whether the count has moved since it was pushed.
        if let Some(at) = self.open.iter().position(|open| *open == symbol) {
            self.looping.extend(self.open[at..].iter().copied());
            if self.fielded > self.marks[at] {
                self.endless.extend(self.open[at..].iter().copied());
            }
            return Stands::Loop;
        }
        // Every symbol a body can name was declared, and every declaration that
        // was made is in this table: a name that repeats one binds nothing and
        // so is never written into a type at all.
        let decl = &self.types[&symbol];
        self.open.push(symbol);
        self.marks.push(self.fielded);
        let stands = self.ty(&decl.value);
        self.open.pop();
        self.marks.pop();
        self.done.insert(symbol, stands);
        stands
    }

    /// What one written type stands for, in the scope of the declaration being
    /// followed.
    fn ty(&mut self, ty: &Type) -> Stands {
        match &ty.tracked {
            // A struct whose `..` names a parameter stands for that parameter:
            // the `..` is the type's core, so what the declaration is, is
            // whatever is written there, carrying the fields beside it. Every
            // other struct — closed, or open in the way only an annotation may
            // be — is a shape like any other.
            TypeKind::Struct {
                tail:
                    Some(Tail {
                        of: Row::Param { index, .. },
                        ..
                    }),
                ..
            } => Stands::Param {
                index: *index,
                fields: true,
            },
            // A shape one step in, which is all a declaration has to reach.
            // `Error` absorbs, as everywhere else.
            TypeKind::Struct { .. }
            | TypeKind::Sum { .. }
            | TypeKind::Arrow { .. }
            | TypeKind::Prim(_)
            | TypeKind::Error => Stands::Shape,
            TypeKind::Param { index, .. } => Stands::Param {
                index: *index,
                fields: false,
            },
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
                //
                // Counted here rather than where the struct answered, because a
                // head already followed answers from the memo without walking
                // anything: the flag rides on the answer so that the count moves
                // whether or not this is the first time through.
                Stands::Param { index, fields } => {
                    if fields {
                        self.fielded += 1;
                    }
                    match self.ty(&args[index as usize]) {
                        Stands::Param {
                            index,
                            fields: deeper,
                        } => Stands::Param {
                            index,
                            fields: fields || deeper,
                        },
                        stands => stands,
                    }
                }
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
///
/// A nested `let` is followed too, and by the same rules: `let x = x in x`
/// says no more about what `x` is than `let x = x` at the top level does. What
/// a `let` expression stands for is what its *body* stands for, since a body
/// that is a bare name is a value given as that name.
fn circling(terms: &IndexMap<Symbol, Decl<Term>>) -> IndexSet<Symbol> {
    let mut locals = HashMap::new();
    for decl in terms.values() {
        nested(&decl.value, &mut locals);
    }
    // Every nested binding is followed from itself as well as from the
    // definitions, because a definition's value stops at the first `fn` it
    // meets: without this, a loop written inside a lambda would be found only
    // if something outside it happened to lead there.
    let bound: Vec<Symbol> = locals.keys().copied().collect();
    let mut chain = Chain {
        terms,
        locals,
        done: HashMap::new(),
        open: Vec::new(),
        looping: IndexSet::new(),
    };
    for symbol in terms.keys() {
        chain.def(*symbol);
    }
    for symbol in bound {
        chain.def(symbol);
    }
    chain.looping
}

/// Erase the value of every nested `let` on a loop, and collect the span each
/// is to be reported at.
///
/// [`build`]'s treatment of a circular definition, said about a binding written
/// inside one: the value is replaced by [`TermKind::Error`], which absorbs, and
/// the span reported is the value's — the same span, and the same complaint,
/// that `let x = x` gets at the top level.
///
/// Innermost first, because a loop can close through a binding written inside
/// another binding's value — `let x = let y = x in y in x` is two bindings on
/// one loop — and erasing the outer one takes the inner one's value out of the
/// program with it. Every binding on the loop is told, which is what the
/// top-level walk already does.
fn erase_circular(term: &mut Term, looping: &IndexSet<Symbol>, out: &mut Vec<Span>) {
    match &mut term.kind {
        TermKind::Let {
            name, value, body, ..
        } => {
            erase_circular(value, looping, out);
            if looping.contains(&name.tracked) {
                out.push(value.span);
                **value = TermKind::Error.with_span(value.span);
            }
            erase_circular(body, looping, out);
        }
        TermKind::Apply { func, arg } => {
            erase_circular(func, looping, out);
            erase_circular(arg, looping, out);
        }
        TermKind::Fn { body, .. } => erase_circular(body, looping, out),
        TermKind::Struct(fields) => {
            for field in fields.values_mut() {
                erase_circular(&mut field.value, looping, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                erase_circular(payload, looping, out);
            }
        }
        TermKind::Project { base, .. } => erase_circular(base, looping, out),
        TermKind::Match { scrutinee, arms } => {
            erase_circular(scrutinee, looping, out);
            for (_, body) in arms {
                erase_circular(body, looping, out);
            }
        }
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
    }
}

/// Every nested `let` in a term, as the symbol it binds and the value it binds
/// it to. See [`Chain::locals`].
fn nested<'a>(term: &'a Term, out: &mut HashMap<Symbol, &'a Term>) {
    match &term.kind {
        TermKind::Let {
            name, value, body, ..
        } => {
            out.insert(name.tracked, value);
            nested(value, out);
            nested(body, out);
        }
        TermKind::Apply { func, arg } => {
            nested(func, out);
            nested(arg, out);
        }
        TermKind::Fn { body, .. } => nested(body, out),
        TermKind::Struct(fields) => {
            for field in fields.values() {
                nested(&field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                nested(payload, out);
            }
        }
        TermKind::Project { base, .. } => nested(base, out),
        // A match binds through its patterns' binders, which — like a lambda's
        // argument — are handed values at no `let` and so have nothing here to
        // collect; only what sits inside is walked, each arm body once.
        TermKind::Match { scrutinee, arms } => {
            nested(scrutinee, out);
            for (_, body) in arms {
                nested(body, out);
            }
        }
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
    }
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
        // A name written bare is a definition, a nested binding, or a lambda's
        // argument, and only the first two have a value here to follow. An
        // argument is handed one at every call site rather than given one where
        // it is written, so it ends the chain the way a shape does — there is
        // nothing about it that could lead back round.
        let value = match self.terms.get(&symbol) {
            Some(decl) => &decl.value,
            None => match self.locals.get(&symbol) {
                Some(value) => value,
                None => return Stands::Shape,
            },
        };
        self.open.push(symbol);
        let stands = self.value(value);
        self.open.pop();
        self.done.insert(symbol, stands);
        stands
    }

    /// What one value stands for. Every kind but a bare name and a nested
    /// `let` is a shape, which is the whole rule.
    fn value(&mut self, term: &Term) -> Stands {
        match &term.kind {
            TermKind::Ident(symbol) => self.def(*symbol),
            // A `let` is whatever its body is: the value it binds is beside the
            // question, and a body written as a bare name is a value given as
            // that name.
            TermKind::Let { body, .. } => self.value(body),
            // A match is a shape too, though nothing has run yet to pick an
            // arm: `let x = match x with ... end` asks something of `x` the
            // way a projection does, so the loop through it still describes a
            // value and is inference's to judge.
            TermKind::Apply { .. }
            | TermKind::Fn { .. }
            | TermKind::Struct(_)
            | TermKind::Tag { .. }
            | TermKind::Project { .. }
            | TermKind::Match { .. }
            | TermKind::Natural(_)
            | TermKind::Error => Stands::Shape,
        }
    }
}

/// The first tag or number in a normalized pattern — what a complaint about a
/// binding that can fail quotes, and where it points. `None` exactly for an
/// irrefutable pattern, which is the syntactic rule of R3: a pattern is
/// refutable iff it contains a tag or a natural anywhere inside it.
fn refuter(pattern: &Pattern) -> Option<(Span, Refuter)> {
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Unit => None,
        PatternKind::Tag { name, .. } => Some((name.span, Refuter::Case(name.tracked.clone()))),
        PatternKind::Natural(value) => Some((pattern.span, Refuter::Number(*value))),
        PatternKind::Struct(fields) => fields.values().find_map(|field| refuter(&field.value)),
    }
}

/// A normalized pattern read as one that cannot fail, or `None` when it can.
/// The `let` walk goes through this rather than matching the pattern itself,
/// so destructuring takes a value that cannot fail and meets no test it would
/// have to call unreachable.
fn calm(pattern: &Pattern) -> Option<Calm> {
    match &pattern.tracked {
        PatternKind::Bind(name) => Some(Calm::Bind(*name)),
        PatternKind::Unit => Some(Calm::Unit(pattern.span)),
        PatternKind::Struct(fields) => {
            let mut lowered = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                lowered.push((field.name_span.track(name.clone()), calm(&field.value)?));
            }
            Some(Calm::Struct {
                span: pattern.span,
                fields: lowered,
            })
        }
        PatternKind::Tag { .. } | PatternKind::Natural(_) => None,
    }
}

/// Every name a normalized pattern binds, in the order the pattern walk met
/// them. What a refused binding still has to bind — to error values — so
/// downstream uses resolve.
fn pattern_binders(pattern: &Pattern, out: &mut Vec<Tracked<Symbol>>) {
    match &pattern.tracked {
        PatternKind::Bind(name) => out.push(*name),
        PatternKind::Unit | PatternKind::Natural(_) => {}
        PatternKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                pattern_binders(payload, out);
            }
        }
        PatternKind::Struct(fields) => {
            for field in fields.values() {
                pattern_binders(&field.value, out);
            }
        }
    }
}

/// Bind each name to an error value around `body`, innermost last, so the
/// names resolve and absorb rather than cascade. What a refused binding's
/// names get, and what the binders of a struct pattern's dropped duplicate
/// field get: the value they would have named is no longer part of the
/// program, and one mistake should make one complaint.
fn bound_to_errors(names: Vec<Tracked<Symbol>>, body: Term) -> Term {
    let mut inner = body;
    for name in names.into_iter().rev() {
        let error = TermKind::Error.with_span(name.span);
        let at = name.span.merge(inner.span);
        inner = TermKind::Let {
            name,
            annotation: None,
            value: Box::new(error),
            body: Box::new(inner),
        }
        .with_span(at);
    }
    inner
}

/// A normalized pattern as the matrix walk matches it: everything that only
/// binds is the one wildcard, a bare tag carries a wildcard payload, and `{}`
/// — which reaches into nothing — is a wildcard too.
fn mat(pattern: &Pattern) -> Mat {
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Unit => Mat::Wild,
        PatternKind::Natural(value) => Mat::Natural(*value),
        PatternKind::Tag { name, payload } => Mat::Tag {
            name: name.tracked.clone(),
            payload: Box::new(payload.as_deref().map(mat).unwrap_or(Mat::Wild)),
        },
        PatternKind::Struct(fields) if fields.is_empty() => Mat::Wild,
        PatternKind::Struct(fields) => Mat::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), mat(&field.value)))
                .collect(),
        ),
    }
}

impl Matrix {
    /// Read the tests and the binder positions off one match's arms. The
    /// universes are a property of the whole written match — R7 closes a row
    /// over every arm's tests, so the checks that mirror it must draw from
    /// the same set — which is why they are collected once here rather than
    /// re-read from whatever rows a recursive step still holds.
    pub(crate) fn new<'a>(patterns: impl IntoIterator<Item = &'a Pattern>) -> Self {
        let mut matrix = Matrix {
            tests: HashMap::new(),
            binds: Vec::new(),
        };
        let mut path = Vec::new();
        for pattern in patterns {
            // A catch-all arm is irrefutable at every position, whichever
            // spelling it is — a bare name, `()`, a struct of binders — so it
            // opens the root, and the root is a prefix of everything.
            if refuter(pattern).is_none() {
                matrix.binds.push(Vec::new());
            }
            matrix.collect(pattern, &mut path);
        }
        matrix
    }

    /// One arm's contribution: its tests by position, and where it binds.
    fn collect(&mut self, pattern: &Pattern, path: &mut Vec<Step>) {
        match &pattern.tracked {
            PatternKind::Bind(_) => self.binds.push(path.clone()),
            PatternKind::Unit => {}
            PatternKind::Natural(value) => {
                self.tests
                    .entry(path.clone())
                    .or_default()
                    .naturals
                    .insert(*value);
            }
            PatternKind::Tag { name, payload } => {
                self.tests
                    .entry(path.clone())
                    .or_default()
                    .tags
                    .insert(name.tracked.clone());
                if let Some(payload) = payload {
                    path.push(Step::Payload(name.tracked.clone()));
                    self.collect(payload, path);
                    path.pop();
                }
            }
            PatternKind::Struct(fields) => {
                for (name, field) in fields {
                    path.push(Step::Field(name.clone()));
                    self.collect(&field.value, path);
                    path.pop();
                }
            }
        }
    }

    /// Whether any one position is tested against both numbers and cases. No
    /// value is both, so such a match is refused at lowering with its own
    /// complaint rather than surfacing as a unification failure. See
    /// [`ErrorKind::MixedMatch`].
    pub(crate) fn mixed(&self) -> bool {
        self.tests
            .values()
            .any(|tests| !tests.tags.is_empty() && !tests.naturals.is_empty())
    }

    /// A value no arm accepts, when one exists: the wildcard's usefulness
    /// against the whole matrix, which is the exhaustiveness question. The
    /// witness comes back in the shape of the value, for the complaint to
    /// write in source syntax.
    pub(crate) fn unhandled(&self, arms: &[&Pattern]) -> Option<Witness> {
        let rows: Vec<Vec<Mat>> = arms.iter().map(|pattern| vec![mat(pattern)]).collect();
        let mut witness = self.useful(&rows, &[Vec::new()], &[Mat::Wild])?;
        Some(witness.remove(0))
    }

    /// Whether some value reaches this arm past the ones above it: the arm's
    /// own usefulness against the matrix of earlier arms.
    pub(crate) fn reachable(&self, earlier: &[&Pattern], arm: &Pattern) -> bool {
        let rows: Vec<Vec<Mat>> = earlier.iter().map(|pattern| vec![mat(pattern)]).collect();
        self.useful(&rows, &[Vec::new()], &[mat(arm)]).is_some()
    }

    /// Whether the earlier arms alone leave no unhandled value carrying this
    /// case at this position — what lets a later binder's view mark the case
    /// absent. The same usefulness question, asked of a synthetic row that is
    /// wildcards everywhere except the path down to the case.
    pub(crate) fn handled(&self, earlier: &[&Pattern], path: &[Step], case: &str) -> bool {
        let rows: Vec<Vec<Mat>> = earlier.iter().map(|pattern| vec![mat(pattern)]).collect();
        let mut forced = Mat::Tag {
            name: case.to_string(),
            payload: Box::new(Mat::Wild),
        };
        for step in path.iter().rev() {
            forced = match step {
                Step::Field(name) => Mat::Struct(vec![(name.clone(), forced)]),
                Step::Payload(name) => Mat::Tag {
                    name: name.clone(),
                    payload: Box::new(forced),
                },
            };
        }
        self.useful(&rows, &[Vec::new()], &[forced]).is_none()
    }

    /// Maranget's usefulness, with a witness: a value that matches `q` and no
    /// row of `rows`, or `None` when every such value is covered. `cols` names
    /// the position each column stands for, which is where the universes come
    /// from; the three are always the same width.
    ///
    /// The witness the wildcard question gets back is built from the listed
    /// tests, so it is concrete wherever the tests are — a listed tag, the
    /// smallest number not listed — and "anything else" exactly where an open
    /// row's rest is what goes unhandled.
    fn useful(&self, rows: &[Vec<Mat>], cols: &[Vec<Step>], q: &[Mat]) -> Option<Vec<Witness>> {
        // No columns left: every value matching q matches every row, so q is
        // useful exactly when no row is left to cover it.
        let Some((pos, later)) = cols.split_first() else {
            return rows.is_empty().then(Vec::new);
        };
        // A column with a struct pattern in it is widened first: one column
        // per field any of its structs names, plus the position itself for the
        // tags and numbers, so the rest of the walk only ever sees flat cells.
        // A struct reaches into its fields and says nothing about the core, so
        // it widens to a wildcard beside its fields; everything else says
        // nothing about the fields and widens to wildcards beside itself.
        let named: IndexSet<&String> = std::iter::once(&q[0])
            .chain(rows.iter().map(|row| &row[0]))
            .filter_map(|cell| match cell {
                Mat::Struct(fields) => Some(fields),
                _ => None,
            })
            .flat_map(|fields| fields.iter().map(|(name, _)| name))
            .collect();
        if !named.is_empty() {
            let widen = |cell: &Mat, rest: &[Mat]| -> Vec<Mat> {
                let (core, fields) = match cell {
                    Mat::Struct(fields) => (
                        Mat::Wild,
                        named
                            .iter()
                            .map(|name| {
                                fields
                                    .iter()
                                    .find(|(field, _)| field == *name)
                                    .map(|(_, sub)| sub.clone())
                                    .unwrap_or(Mat::Wild)
                            })
                            .collect::<Vec<Mat>>(),
                    ),
                    cell => (cell.clone(), vec![Mat::Wild; named.len()]),
                };
                std::iter::once(core)
                    .chain(fields)
                    .chain(rest.iter().cloned())
                    .collect()
            };
            let wide_cols: Vec<Vec<Step>> = std::iter::once(pos.clone())
                .chain(named.iter().map(|name| {
                    let mut sub = pos.clone();
                    sub.push(Step::Field((*name).clone()));
                    sub
                }))
                .chain(later.iter().cloned())
                .collect();
            let wide_rows: Vec<Vec<Mat>> =
                rows.iter().map(|row| widen(&row[0], &row[1..])).collect();
            let wide_q = widen(&q[0], &q[1..]);
            let mut witness = self.useful(&wide_rows, &wide_cols, &wide_q)?;
            // Fold the widened columns back into one witness: the fields that
            // matter in braces, or the core's own answer when none of them do.
            let after = witness.split_off(1 + named.len());
            let mut parts = witness.into_iter();
            let core = parts.next().expect("the widening put the core first");
            let fields: IndexMap<String, Witness> = named
                .iter()
                .map(|name| (*name).clone())
                .zip(parts)
                .filter(|(_, witness)| !matches!(witness, Witness::Any))
                .collect();
            let folded = match fields.is_empty() {
                true => core,
                false => Witness::Struct(fields),
            };
            return Some(std::iter::once(folded).chain(after).collect());
        }
        match &q[0] {
            // q tests a tag: only the rows that could take the same values
            // matter, and the question moves into the payload.
            Mat::Tag { name, payload } => {
                let rows = specialize_tag(rows, name);
                let cols = payload_cols(pos, name, later);
                let q: Vec<Mat> = std::iter::once((**payload).clone())
                    .chain(q[1..].iter().cloned())
                    .collect();
                let mut witness = self.useful(&rows, &cols, &q)?;
                let payload = witness.remove(0);
                Some(
                    std::iter::once(tag_witness(name.clone(), payload))
                        .chain(witness)
                        .collect(),
                )
            }
            // q tests a number: the rows that test another are dropped, and
            // a number carries nothing, so the column is simply consumed.
            Mat::Natural(value) => {
                let rows = specialize_natural(rows, *value);
                let mut witness = self.useful(&rows, later, &q[1..])?;
                witness.insert(0, Witness::Natural(*value));
                Some(witness)
            }
            // q accepts everything here, so it is useful if any value of the
            // position's universe escapes the rows: each listed case in turn,
            // each listed number and then some other number — the numbers
            // never run out — and "anything else" when the position is open.
            // A struct cannot reach this arm — the widening above took every
            // column holding one — so a cell here is a wildcard exactly when
            // it is not a test.
            Mat::Wild | Mat::Struct(_) => {
                let empty = Tests::default();
                let tests = self.tests.get(pos).unwrap_or(&empty);
                if !tests.tags.is_empty() {
                    for name in &tests.tags {
                        let rows = specialize_tag(rows, name);
                        let cols = payload_cols(pos, name, later);
                        let q: Vec<Mat> = std::iter::once(Mat::Wild)
                            .chain(q[1..].iter().cloned())
                            .collect();
                        if let Some(mut witness) = self.useful(&rows, &cols, &q) {
                            let payload = witness.remove(0);
                            return Some(
                                std::iter::once(tag_witness(name.clone(), payload))
                                    .chain(witness)
                                    .collect(),
                            );
                        }
                    }
                    if self.open(pos) {
                        let rows = defaults(rows);
                        let mut witness = self.useful(&rows, later, &q[1..])?;
                        witness.insert(0, Witness::Other(tests.tags.iter().cloned().collect()));
                        return Some(witness);
                    }
                    None
                } else if !tests.naturals.is_empty() {
                    for value in &tests.naturals {
                        let rows = specialize_natural(rows, *value);
                        if let Some(mut witness) = self.useful(&rows, later, &q[1..]) {
                            witness.insert(0, Witness::Natural(*value));
                            return Some(witness);
                        }
                    }
                    let unlisted = (0..)
                        .find(|value| !tests.naturals.contains(value))
                        .expect("a finite set of naturals leaves one out");
                    let rows = defaults(rows);
                    let mut witness = self.useful(&rows, later, &q[1..])?;
                    witness.insert(0, Witness::Natural(unlisted));
                    Some(witness)
                } else {
                    // Nothing tests the position at all, so every cell in the
                    // column is a wildcard and any value serves.
                    let rows = defaults(rows);
                    let mut witness = self.useful(&rows, later, &q[1..])?;
                    witness.insert(0, Witness::Any);
                    Some(witness)
                }
            }
        }
    }

    /// Whether a position's universe holds values beyond the listed cases:
    /// whether any arm is irrefutable at it, which is a binder at or before
    /// it — or a catch-all arm, irrefutable everywhere. What decides, in
    /// inference, whether the position's row closes over its listed cases;
    /// the matrix checks here read the same answer, which is what keeps them
    /// mirrors of one rule.
    pub(crate) fn open(&self, pos: &[Step]) -> bool {
        self.binds.iter().any(|bind| pos.starts_with(bind))
    }
}

/// The rows still in the running once the value is known to be this case,
/// with the payload column opened up: a row testing the same tag matches on
/// through its payload, a wildcard matches whatever the payload is, and a row
/// testing anything else can never take the value.
fn specialize_tag(rows: &[Vec<Mat>], name: &str) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| {
            let payload = match &row[0] {
                Mat::Tag { name: tag, payload } if tag == name => (**payload).clone(),
                Mat::Wild => Mat::Wild,
                _ => return None,
            };
            Some(
                std::iter::once(payload)
                    .chain(row[1..].iter().cloned())
                    .collect(),
            )
        })
        .collect()
}

/// [`specialize_tag`] about a number, which carries nothing: the column is
/// consumed rather than replaced.
fn specialize_natural(rows: &[Vec<Mat>], value: u128) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| match &row[0] {
            Mat::Natural(natural) if *natural == value => Some(row[1..].to_vec()),
            Mat::Wild => Some(row[1..].to_vec()),
            _ => None,
        })
        .collect()
}

/// The rows still in the running once the value is known to escape every
/// listed test: exactly the ones that accept everything at the column.
fn defaults(rows: &[Vec<Mat>]) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| match &row[0] {
            Mat::Wild => Some(row[1..].to_vec()),
            _ => None,
        })
        .collect()
}

/// The columns after a tag specialization: the payload's position, then
/// whatever columns were already waiting.
fn payload_cols(pos: &[Step], name: &str, later: &[Vec<Step>]) -> Vec<Vec<Step>> {
    let mut payload = pos.to_vec();
    payload.push(Step::Payload(name.to_string()));
    std::iter::once(payload)
        .chain(later.iter().cloned())
        .collect()
}

/// A tag's witness, with a payload any value serves for folded away — the
/// bare spelling, which is how a case carrying nothing is written.
fn tag_witness(name: String, payload: Witness) -> Witness {
    let payload = match payload {
        Witness::Any => None,
        payload => Some(Box::new(payload)),
    };
    Witness::Tag { name, payload }
}

/// Every name a surface pattern binds, in the order the lowering walk meets
/// them — repeats included, so the declare pass and [`Builder::pattern`] agree
/// position for position. See [`Binders::Declared`].
fn pattern_names(pattern: &parse::Pattern, out: &mut Vec<TrackedString>) {
    match &pattern.tracked {
        parse::PatternKind::Ident { name } => out.push(name.clone()),
        parse::PatternKind::Natural(_) | parse::PatternKind::Unit => {}
        parse::PatternKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                pattern_names(payload, out);
            }
        }
        parse::PatternKind::Struct(fields) => {
            for (name, sub) in fields {
                match sub {
                    Some(sub) => pattern_names(sub, out),
                    None => out.push(name.clone()),
                }
            }
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
        // Both halves, and no filtering of its own: the name a nested `let`
        // binds is a local, so a use of it is pushed here and dropped by
        // [`grouping`] along with a lambda argument's, and a top-level name
        // mentioned from inside one still lands in the right group.
        TermKind::Let { value, body, .. } => {
            references(value, out);
            references(body, out);
        }
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
        TermKind::Match { scrutinee, arms } => {
            references(scrutinee, out);
            for (_, body) in arms {
                references(body, out);
            }
        }
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
                entry.lacks.extend(kind.lacks().iter().cloned());
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
            // A parameter read more than one way is still taken as the sum's
            // rest among them, so that the debugger and the Types tab show what
            // the body actually said of it. Nothing is enforced against it —
            // the declaration is a write-off and `mixed` says so — but calling
            // it a type would be this pass reporting one thing and displaying
            // another.
            kinds.push(match read_as.contains(&Sense::Cases) {
                true => ParamKind::Cases { lacks },
                false => ParamKind::Type { lacks },
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
        TypeKind::Param { index, .. } => out(Fact::Says(
            *index,
            ParamKind::Type {
                lacks: IndexSet::new(),
            },
        )),
        TypeKind::Struct { fields, tail } => {
            for field in fields.values() {
                if let Some(value) = field.value() {
                    constrain(value, out);
                }
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                // A struct's `..` is the type's core, so the parameter stands
                // for a whole type — the same reading a parameter written
                // anywhere else has, which is why `type W r = { f: r, ..r }` is
                // well-formed.
                //
                // The fields written beside it are exactly what it may not
                // name: they are already named here, and a `..` covers what is
                // not. An absent label is named as surely as a written one —
                // `\y` says the tail has no `y`, which is the same sentence a
                // field named `y` makes it say — and sits in the same map, so
                // the keys are the whole set.
                let lacks = fields.keys().cloned().collect();
                out(Fact::Says(*index, ParamKind::Type { lacks }));
            }
        }
        // The struct arm again, about cases: a payload is a type position, and
        // the cases written beside a tail — absent ones included — are what it
        // may not name.
        TypeKind::Sum { cases, tail } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
                    constrain(payload, out);
                }
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                let lacks = cases.keys().cloned().collect();
                out(Fact::Says(*index, ParamKind::Cases { lacks }));
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

/// Every argument written where a parameter's conditions refuse it, erased where
/// it stood.
///
/// Two ways to fail, and which of them a parameter can be failed by is what
/// [`ParamKind`] says. A [`ParamKind::Cases`] parameter has both: a sum's rest is
/// spliced into a row, so the argument has to be something a row can hold —
/// [`row_shaped`] — and it has to name none of the cases the declaration already
/// names, since a `..` covers only what its row leaves out. A
/// [`ParamKind::Type`] parameter has only the second. A struct's `..` is the
/// type's core, and a core takes every type there is, so there is no shape left
/// to refuse; what is left is the labels the argument would bring with it, which
/// [`carried`] reads through a name as readily as off a struct written out.
///
/// The kinds themselves are a well-formedness check and nothing more — an
/// argument lowers to exactly the type it would anywhere else, and substitution
/// puts it wherever the parameter sat. What this refuses is the argument that
/// would leave a row holding something no row can hold, which is the invariant
/// [`Rest`](crate::types::Rest) documents and nothing else enforces, and the
/// argument that would leave a row or a type naming a label twice, which nothing
/// downstream can recover from either.
///
/// Refusing it is not enough on its own: an argument left standing is
/// substituted where the parameter sat anyway, and the reader is told a second
/// time in words about a type nobody wrote. So the argument absorbs, the way
/// [`ErrorKind::Circular`] and [`ErrorKind::OpenDeclaredType`] do — the
/// argument and not the whole application, because the mistake is the argument
/// and `WithX { x: Nat } -> Nat` is half correct. [`row_shaped`] already reads
/// [`TypeKind::Error`] as row-shaped and [`carried`] reads it as carrying
/// nothing, so nothing complains about the erasure, and it lowers to the
/// undecided type, which a tail and a core are both already allowed to be.
fn row_arguments(program: &mut Program, kinds: &HashMap<Symbol, Vec<ParamKind>>) -> Vec<Error> {
    fn walk(
        ty: &mut Type,
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        carries: &HashMap<Symbol, Carried>,
        rows: &HashSet<Symbol>,
        out: &mut Vec<Error>,
    ) {
        match &mut ty.tracked {
            TypeKind::Apply { head, args, .. } => {
                let head = *head;
                for (at, arg) in args.iter_mut().enumerate() {
                    let kind = kinds.get(&head).and_then(|kinds| kinds.get(at));
                    let refused =
                        match kind {
                            // A sum's rest is spliced into a row, so only a sum can
                            // go there — and only one naming none of the cases the
                            // declaration already names.
                            Some(ParamKind::Cases { lacks }) => match row_shaped(arg, rows) {
                                false => Some(ErrorKind::NotARow),
                                true => cases_named(arg).find(|name| lacks.contains(*name)).map(
                                    |field| ErrorKind::RepeatedRowField {
                                        shape: Shape::Sum,
                                        field: field.clone(),
                                    },
                                ),
                            },
                            // A struct's `..` is the type's core, and a core takes
                            // any type at all — so there is no shape left to check,
                            // and the only question is the fields the argument would
                            // bring with it, which reach through a name as much as
                            // they are written out.
                            Some(ParamKind::Type { lacks }) if !lacks.is_empty() => {
                                carried(arg, carries)
                                    .labels
                                    .into_iter()
                                    .find(|name| lacks.contains(name))
                                    .map(|field| ErrorKind::RepeatedRowField {
                                        shape: Shape::Struct,
                                        field,
                                    })
                            }
                            _ => None,
                        };
                    if let Some(kind) = refused {
                        let span = arg.span;
                        out.push(Error { span, kind });
                        // Nothing left inside to walk: what it was made of is no
                        // longer part of the program.
                        *arg = span.track(TypeKind::Error);
                        continue;
                    }
                    walk(arg, kinds, carries, rows, out);
                }
            }
            TypeKind::Arrow { from, to } => {
                walk(from, kinds, carries, rows, out);
                walk(to, kinds, carries, rows, out);
            }
            TypeKind::Struct { fields, .. } => {
                for field in fields.values_mut() {
                    if let TypeField::Written { value, .. } = field {
                        walk(value, kinds, carries, rows, out);
                    }
                }
            }
            TypeKind::Sum { cases, .. } => {
                for case in cases.values_mut() {
                    if let SumCase::Written {
                        payload: Some(payload),
                        ..
                    } = case
                    {
                        walk(payload, kinds, carries, rows, out);
                    }
                }
            }
            TypeKind::Ident(_) | TypeKind::Param { .. } | TypeKind::Prim(_) | TypeKind::Error => {}
        }
    }

    // What each declaration's fields come to, read once over the whole table
    // before anything is walked: an argument written at a struct's `..` carries
    // whatever the declaration it names carries, which is what the repeated
    // field check is asked against. See [`carrying`].
    let carries = carrying(&program.types);

    let mut out = Vec::new();
    for decl in program.types.values_mut() {
        // Which of this declaration's own parameters are a sum's rest, so one
        // handed straight on is recognised as one. Read out first, so that
        // walking the body borrows nothing the kinds are still held in.
        //
        // Nothing more than the set is wanted: a parameter handed straight on
        // has already collected everything the slot it goes to may not name —
        // that is what [`kinds`] closed the sets over — so there is no second
        // condition here for it to fail.
        let rows: HashSet<Symbol> = decl
            .params
            .iter()
            .filter(|param| param.kind.cases().is_some())
            .map(|param| param.symbol)
            .collect();
        walk(&mut decl.value, kinds, &carries, &rows, &mut out);
    }
    // An annotation binds no parameters, so nothing in one can be a sum's rest
    // by being a parameter — but it is every bit as much a place to apply a
    // declaration, and was the way this check was first written round.
    for decl in program.terms.values_mut() {
        if let Some(annotation) = decl.annotation.as_mut() {
            walk(annotation, kinds, &carries, &HashSet::new(), &mut out);
        }
        // And so is a nested binding's, which is a place to write one as much
        // as a definition's is. An annotation this walk never reaches is a
        // [`ErrorKind::RepeatedRowField`] never reported.
        annotations(&mut decl.value, &mut |annotation| {
            walk(annotation, kinds, &carries, &HashSet::new(), &mut out);
        });
    }
    out
}

/// Every annotation written inside a term, in the order they were written.
///
/// A nested binding is the one place a written type appears anywhere but at the
/// top level of a definition, so this is where the passes that ask something of
/// every written type reach the ones inside a body.
fn annotations(term: &mut Term, out: &mut impl FnMut(&mut Type)) {
    match &mut term.kind {
        TermKind::Let {
            annotation,
            value,
            body,
            ..
        } => {
            if let Some(annotation) = annotation {
                out(annotation);
            }
            annotations(value, out);
            annotations(body, out);
        }
        TermKind::Apply { func, arg } => {
            annotations(func, out);
            annotations(arg, out);
        }
        TermKind::Fn { body, .. } => annotations(body, out),
        TermKind::Struct(fields) => {
            for field in fields.values_mut() {
                annotations(&mut field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                annotations(payload, out);
            }
        }
        TermKind::Project { base, .. } => annotations(base, out),
        TermKind::Match { scrutinee, arms } => {
            annotations(scrutinee, out);
            for (_, body) in arms {
                annotations(body, out);
            }
        }
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
    }
}

/// Whether a written type could stand for the cases a sum does not name.
///
/// A sum can: its cases are spliced in where the tail was. A row parameter can,
/// by being a sum's already — a struct written there would splice fields into a
/// row of cases, and nothing downstream could make sense of the result. A
/// declared name cannot either, though it looks as though it should: a tail
/// holding a name would have to be unfolded by the two walks that flatten rows,
/// which neither does, so it is refused rather than silently mishandled.
///
/// About cases and nothing else, and so about no shape. A struct's `..` is the
/// type's core, and every type at all is one of those: `WithX Nat` splices no
/// row anywhere, it hands a core a core.
fn row_shaped(ty: &Type, rows: &HashSet<Symbol>) -> bool {
    match &ty.tracked {
        TypeKind::Error => true,
        TypeKind::Sum { .. } => true,
        TypeKind::Param { symbol, .. } => rows.contains(symbol),
        _ => false,
    }
}

/// The cases a written type names outright, in the order it names them.
///
/// Only a sum names any. A parameter handed straight on names nothing here —
/// what it stands for is supplied at every use of *its* declaration, and already
/// carries this condition — and an erased argument absorbed a complaint already
/// made.
fn cases_named(ty: &Type) -> impl Iterator<Item = &String> {
    let cases = match &ty.tracked {
        TypeKind::Sum { cases, .. } => Some(cases),
        _ => None,
    };
    cases.into_iter().flat_map(|cases| cases.keys())
}

/// What a written type carries at the top level of its field row: the labels it
/// names outright, and the parameters whose arguments' labels join them.
///
/// Two sets rather than one because a struct's `..` is the type's core, so
/// `type WithX r = { x: Nat, ..r }` carries an `x` *and* whatever is written for
/// `r`. Which that is cannot be known until a use site writes one, so the slot
/// is recorded here and read where the argument is.
#[derive(Debug, Clone, Default)]
struct Carried {
    /// Insertion-ordered, so a complaint about an argument breaking the rule
    /// twice always names the same label first — the one a reader would reach
    /// first reading the argument left to right, which is the rule
    /// [`Table::repeated`](crate::inference) states for the same complaint
    /// reached through a variable.
    labels: IndexSet<String>,
    /// The parameters that land in the core of what this stands for, so that
    /// whatever is written at them is carried too.
    slots: IndexSet<u32>,
}

/// What each declaration carries, over the whole table.
///
/// A fixpoint, because a body may be another name and declarations are hoisted:
/// `type Foo = Bar` carries what `Bar` carries, and which was written first
/// decides nothing. Both sets only grow and both are bounded — by the labels
/// written in the program, and by its parameters — so the loop stops. Starting
/// from nothing is the safe start: a label missed costs a complaint that is not
/// made, never one made about a program that is right.
///
/// Without this the two `x`s of `WithX (WithX Nat)` would meet in
/// [`Table::resolve`](crate::inference)'s splice, the outer would win without a
/// word, and a definition would come out with a type nothing showed it has —
/// the exact failure the lacks condition exists to prevent.
fn carrying(types: &IndexMap<Symbol, Decl<Type>>) -> HashMap<Symbol, Carried> {
    let mut out: HashMap<Symbol, Carried> = types
        .keys()
        .map(|symbol| (*symbol, Carried::default()))
        .collect();
    loop {
        let mut grew = false;
        for (symbol, decl) in types {
            let found = carried(&decl.value, &out);
            let entry = out.get_mut(symbol).expect("every declaration was seeded");
            for label in found.labels {
                grew |= entry.labels.insert(label);
            }
            for slot in found.slots {
                grew |= entry.slots.insert(slot);
            }
        }
        if !grew {
            return out;
        }
    }
}

/// What one written type carries, given what each declaration carries so far:
/// the step [`carrying`] iterates, and the read [`row_arguments`] makes of one
/// argument.
///
/// A struct writes its own field names, and then whatever its `..` carries. A
/// name, or an application, carries what the declaration it names carries,
/// joined with what is carried by each argument written at a parameter that
/// declaration puts in its core. Everything else carries nothing — a sum carries
/// no *fields*, so `` WithX (`A | `B) `` is fine.
fn carried(ty: &Type, decls: &HashMap<Symbol, Carried>) -> Carried {
    match &ty.tracked {
        TypeKind::Struct { fields, tail } => {
            let mut slots = IndexSet::new();
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                slots.insert(*index);
            }
            Carried {
                labels: fields.keys().cloned().collect(),
                slots,
            }
        }
        // The body is the parameter, as in `type Id a = a`: whatever is written
        // there is the whole of what the declaration stands for, fields
        // included.
        TypeKind::Param { index, .. } => Carried {
            labels: IndexSet::new(),
            slots: std::iter::once(*index).collect(),
        },
        // A name written bare takes no arguments — the arity check saw to that —
        // so it names no slot this scope could fill.
        TypeKind::Ident(symbol) => decls.get(symbol).cloned().unwrap_or_default(),
        TypeKind::Apply { head, args, .. } => {
            let head = decls.get(head).cloned().unwrap_or_default();
            let mut out = Carried {
                labels: head.labels,
                slots: IndexSet::new(),
            };
            // The head's slots are its own parameters, and the arguments are in
            // the scope of whoever wrote this — so what comes back from one is
            // said about *this* declaration's parameters, which is what makes
            // the slots compose.
            //
            // Indexed rather than looked up: the arity check ran where the
            // application was written, so every slot the head names has an
            // argument in that position.
            for index in head.slots {
                let inner = carried(&args[index as usize], decls);
                out.labels.extend(inner.labels);
                out.slots.extend(inner.slots);
            }
            out
        }
        TypeKind::Sum { .. } | TypeKind::Arrow { .. } | TypeKind::Prim(_) | TypeKind::Error => {
            Carried::default()
        }
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
                    if let Some(value) = field.value() {
                        occurrences(value, under, out);
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
            TypeKind::Sum { cases, tail } => {
                for case in cases.values() {
                    if let Some(payload) = case.payload() {
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
                if let Some(value) = field.value() {
                    mentioned(value, out);
                }
            }
        }
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
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
                if let Some(value) = field.value() {
                    grows(value, group, report);
                }
            }
        }
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
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
                    .any(|field| field.value().is_some_and(mentions_a_parameter))
        }
        TypeKind::Sum { cases, tail } => {
            tails_a_parameter(tail)
                || cases
                    .values()
                    .any(|case| case.payload().is_some_and(mentions_a_parameter))
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
                kind: ParamKind::Type {
                    lacks: IndexSet::new(),
                },
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
        // A name is one rest, and one rest stands for one thing. A struct's `..`
        // is the type its fields sit on and a sum's is the cases it does not
        // write out, so the two are a whole type and the rest of a sum — which
        // is what a name given both would have to be at once. Recorded at the
        // first use and checked at every one after, so the complaint lands on
        // the tail that brought the two together rather than on whichever the
        // writer happened to put first. `None` says the row absorbs; see
        // [`ErrorKind::MixedTail`].
        let sense = sense(shape);
        match self.tails.get(&name.tracked) {
            Some(&(first, previous)) if first != sense => {
                self.error(
                    span,
                    ErrorKind::MixedTail {
                        first,
                        second: sense,
                        previous,
                    },
                );
                return None;
            }
            Some(_) => {}
            None => {
                self.tails.insert(name.tracked.clone(), (sense, span));
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

    /// Whether a row's explicit absences have a `..` to speak about. A `\`
    /// says the tail beside it may not stand for the label, and a row with no
    /// tail already says that of every label it does not name — so each one in
    /// a closed row is reported where it was written, and the row absorbs. See
    /// [`ErrorKind::AbsentInClosed`].
    ///
    /// [`closed`](Self::closed)'s sibling, and one rule for both shapes for
    /// the same reason: the two arms of [`ty`](Self::ty) that call it differ
    /// in the nouns they are written about and in nothing else.
    fn tailed(
        &mut self,
        shape: Shape,
        absences: impl IntoIterator<Item = (String, Span)>,
        tail: &Option<Tail>,
    ) -> bool {
        if tail.is_some() {
            return true;
        }
        let mut tailed = true;
        for (label, span) in absences {
            self.error(span, ErrorKind::AbsentInClosed { shape, label });
            tailed = false;
        }
        tailed
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
            // A bare name is bound before the value is lowered, so a nested
            // `let` may name itself the way a definition may; and released
            // after the body, so nothing written past the expression can see
            // it. Bound rather than declared, which is what makes it shadow
            // silently: two definitions of one name are a repeat, and a scope
            // inside one is not.
            //
            // A pattern is the other way round: the value is lowered first —
            // the temporary is bound before any of the pattern's names, so
            // none of them is in scope in it — and then the binding desugars
            // to that temporary and a projection per name. A pattern that
            // could fail is refused; see [`ErrorKind::RefutableBinding`].
            ExprKind::Let {
                pattern,
                ty,
                value,
                body,
            } => match pattern.tracked {
                parse::PatternKind::Ident { name } => {
                    let annotation = ty.map(|ty| self.written(*ty, Place::Annotation));
                    let mark = self.terms.mark();
                    let symbol = self
                        .mint
                        .local(self.module, Namespace::Terms, &name.tracked);
                    self.terms.bind(name.tracked, symbol, name.span);
                    let value = self.term(*value);
                    let body = self.term(*body);
                    self.terms.release(mark);
                    TermKind::Let {
                        name: name.span.track(symbol),
                        annotation,
                        value: Box::new(value),
                        body: Box::new(body),
                    }
                    .with_span(span)
                }
                tracked => {
                    let pspan = pattern.span;
                    let pattern = pspan.track(tracked);
                    let annotation = ty.map(|ty| self.written(*ty, Place::Annotation));
                    let value = self.term(*value);
                    let mark = self.terms.mark();
                    let mut seen = Vec::new();
                    let mut dropped = Vec::new();
                    let pattern =
                        self.pattern(pattern, &mut seen, &mut Binders::Local, &mut dropped);
                    let body = self.term(*body);
                    self.terms.release(mark);
                    let body = bound_to_errors(dropped, body);
                    match calm(&pattern) {
                        Some(calm) => {
                            let mut term = self.destructure(calm, value, annotation, body);
                            term.span = span;
                            term
                        }
                        // The binding has to accept every value, and this
                        // pattern would not. Every name it would have bound is
                        // still bound — to error values, which absorb — and
                        // the value keeps its place, so its own mistakes are
                        // still its own complaints.
                        None => {
                            let (at, found) = refuter(&pattern)
                                .expect("a pattern that is not calm names what refutes it");
                            self.error(at, ErrorKind::RefutableBinding { found });
                            let mut names = Vec::new();
                            pattern_binders(&pattern, &mut names);
                            let inner = bound_to_errors(names, body);
                            let held = self.fresh("%value", pspan);
                            TermKind::Let {
                                name: held,
                                annotation,
                                value: Box::new(value),
                                body: Box::new(inner),
                            }
                            .with_span(span)
                        }
                    }
                }
            },
            ExprKind::Match { scrutinee, arms } => self.match_term(span, *scrutinee, arms),
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
                // A type's field has a `?` and a `\` to it that [`Field`] has
                // no room for, so the marks ride down with the value and are
                // taken apart again here. Re-keying and repeats are the same
                // question they are for a struct literal, and are asked in the
                // one place that answers it.
                let lowered: IndexMap<String, TypeField> = self
                    .fields(fields, |b, field| match field {
                        parse::TypeField::Written { optional, value } => {
                            Some((optional, b.ty(value, place)))
                        }
                        parse::TypeField::Absent => None,
                    })
                    .into_iter()
                    .map(|(name, field)| {
                        let lowered = match field.value {
                            Some((optional, value)) => TypeField::Written {
                                name_span: field.name_span,
                                optional,
                                value,
                            },
                            None => TypeField::Absent {
                                name_span: field.name_span,
                            },
                        };
                        (name, lowered)
                    })
                    .collect();
                let tail = match self.tail(tail, place, Shape::Struct) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                // Where a declaration is held to being closed, and where a `\`
                // is held to having a `..` to speak about; see
                // [`closed`](Self::closed) and [`tailed`](Self::tailed), the
                // same two checks the sum arm below makes. Both run, so a row
                // wrong both ways is told about both.
                let marks = lowered.values().filter_map(|field| match field {
                    TypeField::Written {
                        optional: true,
                        name_span,
                        ..
                    } => Some(*name_span),
                    _ => None,
                });
                let closed = self.closed(place, Shape::Struct, marks, &tail);
                let absences = lowered.iter().filter_map(|(name, field)| match field {
                    TypeField::Absent { name_span } => Some((name.clone(), *name_span)),
                    TypeField::Written { .. } => None,
                });
                let tailed = self.tailed(Shape::Struct, absences, &tail);
                if !closed || !tailed {
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
                    .labels(cases, ErrorKind::DuplicateCase, |b, case| match case {
                        parse::SumCase::Written { optional, payload } => {
                            Some((optional, payload.map(|payload| b.ty(payload, place))))
                        }
                        parse::SumCase::Absent => None,
                    })
                    .into_iter()
                    .map(|(name, case)| {
                        let lowered = match case.value {
                            Some((optional, payload)) => SumCase::Written {
                                name_span: case.name_span,
                                optional,
                                payload,
                            },
                            None => SumCase::Absent {
                                name_span: case.name_span,
                            },
                        };
                        (name, lowered)
                    })
                    .collect();
                let tail = match self.tail(tail, place, Shape::Sum) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                let marks = lowered.values().filter_map(|case| match case {
                    SumCase::Written {
                        optional: true,
                        name_span,
                        ..
                    } => Some(*name_span),
                    _ => None,
                });
                let closed = self.closed(place, Shape::Sum, marks, &tail);
                let absences = lowered.iter().filter_map(|(name, case)| match case {
                    SumCase::Absent { name_span } => Some((name.clone(), *name_span)),
                    SumCase::Written { .. } => None,
                });
                let tailed = self.tailed(Shape::Sum, absences, &tail);
                if !closed || !tailed {
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

    /// A fresh symbol no source name can reach: minted like a local, never
    /// bound into any scope, so nothing written can name or capture it. The
    /// name starts with `%`, which no identifier can, so the debugger shows it
    /// recognizably as the compiler's own.
    fn fresh(&mut self, name: &str, span: Span) -> Tracked<Symbol> {
        span.track(self.mint.local(self.module, Namespace::Terms, name))
    }

    /// One name a pattern binds. `seen` is every name the whole pattern has
    /// bound so far — one pattern binds a name once, however deep the nesting,
    /// and the repeat is reported here, pointing at itself. See
    /// [`Binders`] for where the symbol comes from.
    fn bound(
        &mut self,
        name: TrackedString,
        seen: &mut Vec<String>,
        binders: &mut Binders,
    ) -> Tracked<Symbol> {
        let repeat = seen.contains(&name.tracked);
        if repeat {
            self.error(
                name.span,
                ErrorKind::DuplicateBinding {
                    name: name.tracked.clone(),
                },
            );
        } else {
            seen.push(name.tracked.clone());
        }
        match binders {
            Binders::Local => {
                let symbol = self
                    .mint
                    .local(self.module, Namespace::Terms, &name.tracked);
                // The repeat binds nothing: the first binding is the one the
                // body sees, the way a repeated definition stands.
                if !repeat {
                    self.terms.bind(name.tracked.clone(), symbol, name.span);
                }
                name.span.track(symbol)
            }
            Binders::Declared(declared) => {
                let declared = declared
                    .next()
                    .expect("the declare pass walked this same pattern");
                let symbol = match declared {
                    Some(symbol) => symbol,
                    // A name that bound nothing — a repeat, within the pattern
                    // or of an earlier definition — still gets a stand-in, so
                    // the walk stays total.
                    None => self
                        .mint
                        .local(self.module, Namespace::Terms, &name.tracked),
                };
                name.span.track(symbol)
            }
        }
    }

    /// Lower one surface pattern into the normalized [`Pattern`]: resolve its
    /// binders, expand punning, discard grouping — the parser already did —
    /// and keep everything else as written. The duplicate-binding and
    /// duplicate-field complaints are made here, at the repeats.
    ///
    /// `dropped` collects the binders of a duplicate field's sub-pattern: the
    /// field the normalized map keeps is the first, so the repeat's names have
    /// no position left to be bound at, and the caller binds them to error
    /// values instead — the way a refused binding's names are bound — so
    /// downstream uses resolve.
    fn pattern(
        &mut self,
        pattern: parse::Pattern,
        seen: &mut Vec<String>,
        binders: &mut Binders,
        dropped: &mut Vec<Tracked<Symbol>>,
    ) -> Pattern {
        let span = pattern.span;
        match pattern.tracked {
            parse::PatternKind::Ident { name } => {
                span.track(PatternKind::Bind(self.bound(name, seen, binders)))
            }
            parse::PatternKind::Natural(value) => span.track(PatternKind::Natural(value)),
            parse::PatternKind::Unit => span.track(PatternKind::Unit),
            // A bare tag keeps its `None`: what it constrains the payload to —
            // unit — is said where the type is built rather than written into
            // a tree node the reader never wrote, the convention
            // [`TermKind::Tag`] keeps.
            parse::PatternKind::Tag { name, payload } => {
                let payload =
                    payload.map(|payload| Box::new(self.pattern(*payload, seen, binders, dropped)));
                span.track(PatternKind::Tag { name, payload })
            }
            parse::PatternKind::Struct(entries) => {
                let mut named: Vec<(String, bool)> = Vec::new();
                let mut fields: IndexMap<String, Field<Pattern>> = IndexMap::new();
                for (name, sub) in entries {
                    let pun = sub.is_none();
                    let keep = match named.iter().find(|(seen, _)| *seen == name.tracked) {
                        // Two puns of one name are `{x, x}`: the same name
                        // bound twice, which the binder walk below words
                        // better than a complaint about the field would.
                        Some((_, earlier)) if !(pun && *earlier) => {
                            self.error(name.span, ErrorKind::DuplicateField);
                            false
                        }
                        Some(_) => false,
                        None => {
                            named.push((name.tracked.clone(), pun));
                            true
                        }
                    };
                    // The repeat's sub-pattern is lowered all the same — its
                    // own mistakes are still its own complaints, and the
                    // declare pass walked the same names — and then dropped:
                    // the first field is the one that stands.
                    let sub = match sub {
                        Some(sub) => self.pattern(sub, seen, binders, dropped),
                        None => {
                            let bound = self.bound(name.clone(), seen, binders);
                            name.span.track(PatternKind::Bind(bound))
                        }
                    };
                    match keep {
                        true => {
                            fields.insert(
                                name.tracked,
                                Field {
                                    name_span: name.span,
                                    value: sub,
                                },
                            );
                        }
                        false => pattern_binders(&sub, dropped),
                    }
                }
                span.track(PatternKind::Struct(fields))
            }
        }
    }

    /// Wrap `inner` in the bindings a calm pattern makes against `value`: a
    /// name is one `let`, `()` is a fresh binding annotated unit — the
    /// pattern's whole demand — and a struct pattern is R6's chain, a fresh
    /// temporary and one binding per field, in written order.
    fn destructure(
        &mut self,
        calm: Calm,
        value: Term,
        annotation: Option<Type>,
        inner: Term,
    ) -> Term {
        match calm {
            Calm::Bind(name) => {
                let span = name.span.merge(inner.span);
                TermKind::Let {
                    name,
                    annotation,
                    value: Box::new(value),
                    body: Box::new(inner),
                }
                .with_span(span)
            }
            Calm::Unit(span) => match annotation {
                // The written type is the contract on the whole value, and the
                // pattern's own demand — unit — goes on a second binding of
                // it, so both are said and neither displaces the other.
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    let constrained = self.destructure(Calm::Unit(span), again, None, inner);
                    let at = span.merge(constrained.span);
                    TermKind::Let {
                        name: held,
                        annotation: Some(annotation),
                        value: Box::new(value),
                        body: Box::new(constrained),
                    }
                    .with_span(at)
                }
                None => {
                    let unit = span.track(TypeKind::Struct {
                        fields: IndexMap::new(),
                        tail: None,
                    });
                    let name = self.fresh("%unit", span);
                    let at = span.merge(inner.span);
                    TermKind::Let {
                        name,
                        annotation: Some(unit),
                        value: Box::new(value),
                        body: Box::new(inner),
                    }
                    .with_span(at)
                }
            },
            Calm::Struct { span, fields } => {
                let held = self.fresh("%struct", span);
                let mut inner = inner;
                for (name, sub) in fields.into_iter().rev() {
                    let base = TermKind::Ident(held.tracked).with_span(name.span);
                    let field = TermKind::Project {
                        base: Box::new(base),
                        field: name.clone(),
                    }
                    .with_span(name.span);
                    inner = self.destructure(sub, field, None, inner);
                }
                let at = span.merge(inner.span);
                TermKind::Let {
                    name: held,
                    annotation,
                    value: Box::new(value),
                    body: Box::new(inner),
                }
                .with_span(at)
            }
        }
    }

    /// R6's statement half: a calm pattern on a top-level `let` becomes
    /// ordinary top-level definitions — a fresh one holding the value, with
    /// the written annotation, then one per name, fields in written order.
    fn destructure_stmt(
        &mut self,
        calm: Calm,
        annotation: Option<Type>,
        value: Term,
        out: &mut IndexMap<Symbol, Decl<Term>>,
    ) {
        match calm {
            Calm::Bind(name) => {
                out.insert(
                    name.tracked,
                    Decl {
                        name_span: name.span,
                        annotation,
                        params: Vec::new(),
                        value,
                    },
                );
            }
            Calm::Unit(span) => match annotation {
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    out.insert(
                        held.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(annotation),
                            params: Vec::new(),
                            value,
                        },
                    );
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    self.destructure_stmt(Calm::Unit(span), None, again, out);
                }
                None => {
                    let unit = span.track(TypeKind::Struct {
                        fields: IndexMap::new(),
                        tail: None,
                    });
                    let name = self.fresh("%unit", span);
                    out.insert(
                        name.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(unit),
                            params: Vec::new(),
                            value,
                        },
                    );
                }
            },
            Calm::Struct { span, fields } => {
                let held = self.fresh("%struct", span);
                out.insert(
                    held.tracked,
                    Decl {
                        name_span: span,
                        annotation,
                        params: Vec::new(),
                        value,
                    },
                );
                for (name, sub) in fields {
                    let base = TermKind::Ident(held.tracked).with_span(name.span);
                    let field = TermKind::Project {
                        base: Box::new(base),
                        field: name.clone(),
                    }
                    .with_span(name.span);
                    self.destructure_stmt(sub, None, field, out);
                }
            }
        }
    }

    /// Lower `match <expr> with <arms> end`: the scrutinee, then each arm's
    /// pattern and body — the pattern's names in scope for its own body and
    /// released after it — into the one [`TermKind::Match`] node, arms in
    /// written order. Nothing about how a machine would dispatch them is
    /// decided here; what is decided is what the matrix itself can be wrong
    /// about:
    ///
    /// - an arm that accepts everything belongs last, and one written anywhere
    ///   else is reported at itself — the placement is the mistake — with the
    ///   arms it starves dropped, bodies already lowered for their own
    ///   complaints (R11);
    /// - a position tested against both numbers and cases is refused whole
    ///   (R9);
    /// - an arm no value can reach is reported at itself and dropped (R10);
    /// - a match that leaves values unhandled is reported at the match, with
    ///   a concrete example — worded about numbers when the example is one
    ///   (R8, R9).
    fn match_term(&mut self, span: Span, scrutinee: Expr, arms: Vec<parse::Arm>) -> Term {
        let scrutinee = self.term(scrutinee);
        let mut rows: Vec<(Pattern, Term, Span)> = Vec::new();
        for arm in arms {
            let at = arm.pattern.span.merge(arm.body.span);
            let mark = self.terms.mark();
            let mut seen = Vec::new();
            let mut dropped = Vec::new();
            let pattern = self.pattern(arm.pattern, &mut seen, &mut Binders::Local, &mut dropped);
            let body = self.term(arm.body);
            self.terms.release(mark);
            let body = bound_to_errors(dropped, body);
            rows.push((pattern, body, at));
        }
        // An arm that accepts everything starves every arm after it, so each
        // one written anywhere but last is reported — at itself, since the
        // placement is the mistake, and once, rather than once per arm it
        // starves — and the arms past the first stand no chance and are
        // dropped.
        for (at, row) in rows.iter().enumerate() {
            if at + 1 < rows.len() && refuter(&row.0).is_none() {
                self.error(row.2, ErrorKind::MisplacedCatchAll);
            }
        }
        if let Some(catch) = rows.iter().position(|row| refuter(&row.0).is_none()) {
            rows.truncate(catch + 1);
        }
        // One position compares one kind of thing: numbers and cases answer
        // to different types, and no value is both, so the mix is refused
        // here as the written mistake it is rather than surfacing as a
        // unification failure. The whole match absorbs: which arms mean what
        // is exactly what could not be worked out.
        let matrix = Matrix::new(rows.iter().map(|row| &row.0));
        if matrix.mixed() {
            self.error(span, ErrorKind::MixedMatch);
            return TermKind::Error.with_span(span);
        }
        // An arm nothing can reach is dropped after being reported, the way a
        // duplicate definition is: the matrix inference reads should hold the
        // arms that mean something. Each arm is asked against the arms kept
        // so far — a dropped arm covered nothing its predecessors did not, so
        // dropping it moves no later answer.
        let mut arms: Vec<(Pattern, Term)> = Vec::new();
        for (pattern, body, at) in rows {
            let earlier: Vec<&Pattern> = arms.iter().map(|(pattern, _)| pattern).collect();
            match matrix.reachable(&earlier, &pattern) {
                true => arms.push((pattern, body)),
                false => self.error(at, ErrorKind::UnreachableArm),
            }
        }
        // Unhandled values, with a witness. The empty match is exempt by
        // construction rather than by exception: it constrains the scrutinee
        // to the empty sum, which has no values to leave unhandled. The
        // universes are re-read from the arms as they finally stand, since
        // those are the arms inference will close the rows over.
        if !arms.is_empty() {
            let matrix = Matrix::new(arms.iter().map(|(pattern, _)| pattern));
            let kept: Vec<&Pattern> = arms.iter().map(|(pattern, _)| pattern).collect();
            if let Some(witness) = matrix.unhandled(&kept) {
                let kind = match witness {
                    // The friendlier wording about numbers: some number went
                    // unhandled, and no one number is worth quoting.
                    Witness::Natural(_) => ErrorKind::UnhandledNumbers,
                    witness => ErrorKind::UnhandledValues { witness },
                };
                self.error(span, kind);
            }
        }
        TermKind::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        }
        .with_span(span)
    }
}

/// What a `..` written on a row of this shape stands for: a struct's is the
/// whole type its fields sit on, and a sum's is the cases it does not write out.
/// The one place the two shapes still have different answers, and the reason
/// [`Sense`] has two variants rather than one. See [`ErrorKind::MixedTail`].
fn sense(shape: Shape) -> Sense {
    match shape {
        Shape::Struct => Sense::Type,
        Shape::Sum => Sense::Cases,
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
