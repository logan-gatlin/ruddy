use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::symbol::Symbol;

/// A type built into the language rather than declared in it.
///
/// A primitive names nothing the mint could hand out — the same reason
/// [`TermKind::Natural`](crate::ir::TermKind::Natural) carries no symbol — so it
/// is a value here rather than a seeded declaration. Shared by the syntactic
/// [`ir::TypeKind`](crate::ir::TypeKind) and the semantic type language, so the
/// two can never disagree about which primitives exist.
///
/// Unit is deliberately not one of them. `()` is a spelling of the type with
/// nothing of its own and no fields, not a type of its own, so there is nothing
/// here for it to name; see [`Core::Unit`] for why it stays that way even though
/// it means the compiler answers in `{}` where the user wrote `()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    /// The type of a natural literal.
    Nat,
}

pub type TyVar = u32;

/// Which of the two sets of labels a rule is being read about.
///
/// A set of labels, each either there or not, plus something saying what is
/// known about the names it does not list. Both composites in the language are
/// one: the fields every type carries are a set of labels, all of which a value
/// has, and a sum is a set of cases, one of which a value is. The two are
/// unified, flattened, generalized and printed by the same code, and this is the
/// only thing that tells them apart.
///
/// What each says about the names it does not list is the one place the two
/// differ. A struct's is the core beside it — [`Ty::fields`] is a bare label map
/// and [`Ty::core`] is what its `..` stands for — and a sum's is the
/// [`Rest`] inside its [`Row`]. So a struct's fields have no tail of their own,
/// and [`Row`] and [`Rest`] survive for [`Core::Sum`] alone.
///
/// Stored nowhere. Where a set of labels sits already says which it is, so the
/// shape is a *reading* the caller carries down — the way `solve::Rowed` does —
/// and never a second copy of a fact the position already settles.
///
/// Never inferred and never defaulted: which one a set of labels is, is decided
/// by the syntax that wrote it — braces or backticks — and travels with the type
/// from there. Two of different shapes are two types, and the solver refuses
/// them the way it refuses a `Nat` against an arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// `{ x: Nat, ..r }` — a value has every field the row says is there.
    Struct,
    /// `` `A Nat | ..r `` — a value is one of the cases the row says is there.
    Sum,
    /// `` A -> B ! `Log | ..r `` — calling the arrow may perform every effect
    /// the row says is there, and no other.
    ///
    /// A third reading of the same machinery, not a third machinery: an effect
    /// row is unified, flattened, generalized and printed by the code the other
    /// two go through, and the label it carries holds nothing — see
    /// [`RowField::ty`], which for an effect is unit and is never inspected.
    Effect,
}

/// What one parameter of a `type` declaration stands for, without the labels it
/// carries.
///
/// The question by itself, so that a complaint about a parameter read two ways
/// can name the two readings without quoting a set of labels nobody asked
/// about. See [`ir::ErrorKind::MixedParameter`](crate::ir::ErrorKind).
///
/// Four of them rather than two, because a `where let` variable can be two
/// things a declaration's parameter never can: the presence a `when` names, and
/// nothing here — the rest of a struct *is* a type, since `..r` in a struct
/// puts whatever is written for `r` in the type's core. A sum's rest and an
/// arrow's effects are readings of their own, for the reason [`Rest`] gives:
/// neither is a position a whole type could go in.
///
/// [`ParamKind`] answers with the first three alone: a declaration binds no
/// presence, so nothing there can ever be read the fourth way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sense {
    Type,
    Cases,
    /// The effects an arrow may perform: `..e` in `type Runner e = (Nat -> Nat
    /// ! ..e) -> Nat ! ..e`.
    Effects,
    /// Whether one label is there — what a `when a` names and what a `where`
    /// clause's formula is written about.
    Presence,
}

/// What one parameter of a `type` declaration stands for.
///
/// Written nowhere: a parameter is a bare name, and which of these it is
/// follows from where the body uses it — `..r` in a sum makes a rest of cases,
/// anything else makes a type. Worked out in [`ir::build`](crate::ir::build),
/// and carried here so that the readers who need it — lowering, inference and
/// the debugger — agree.
///
/// Two of them, and no way to write a third: a type, or the rest of a sum. That
/// is what keeps this a check rather than a language: every parameter is one of
/// these, every declaration takes a fixed list of them, and nothing takes a
/// declaration.
///
/// Both carry the labels an argument written there may not name. `r` in
/// `type WithX r = { x: Nat, ..r }` is a [`ParamKind::Type`] whose set is `{x}`:
/// the core covers the fields the declaration does not write out, so an `r` with
/// an `x` of its own would give the type two fields of one name, and the two
/// copies could disagree. Carrying the set rather than a bare flag is what lets
/// the condition be said where the argument is written, at the span the reader
/// can act on, instead of being discovered later by whatever happened to
/// flatten the labels — or never at all.
///
/// A parameter handed straight on to another declaration collects that
/// declaration's labels too, which is why this is a fixpoint over the whole
/// table rather than a read of one body. Insertion-ordered, so a complaint
/// about an argument breaking the rule twice always names the same label first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind {
    /// Stands for a type. `A` in `type Pair A B`, and `r` in
    /// `type WithX r = { x: Nat, ..r }` alike — the rest of a struct is a type,
    /// so there is nothing else for it to be, and `WithX Nat` is as well-formed
    /// as `WithX { y: Nat }`.
    Type { lacks: IndexSet<String> },
    /// Stands for the cases a sum does not name — and, with them, the cases it
    /// may therefore not name itself. `r` in `` type Or r = `A | ..r ``.
    ///
    /// The one reading that is not a type, and the reason it is enforced rather
    /// than substituted: a sum's rest is spliced into [`Core::Sum`]'s row, so
    /// anything else written there would leave a row holding what no row can
    /// hold. See [`ir::ErrorKind::NotARow`](crate::ir::ErrorKind).
    Cases { lacks: IndexSet<String> },
    /// Stands for the effects an arrow does not name — and, with them, the
    /// effects it may therefore not name itself. `e` in
    /// `type Runner e = (Nat -> Nat ! ..e) -> Nat ! ..e`.
    ///
    /// [`ParamKind::Cases`]'s twin, and enforced for the same reason: an effect
    /// row's rest is spliced into the row [`Core::Arrow`] carries, so anything
    /// else written there would leave a row holding what no row can hold.
    Effects { lacks: IndexSet<String> },
}

/// One variable a [`Formula`] names: one the solver still owns, or one a
/// [`Scheme`] quantified.
///
/// The same two readings [`Presence::Var`] and [`Presence::Bound`] have, and
/// for the same reason — a formula is a statement about presences, so it names
/// them the way a type does. One value rather than two variants of [`Formula`]
/// so that walking, evaluating and printing a formula each ask about an atom
/// once instead of once per reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Atom {
    Var(TyVar),
    Bound(u32),
}

/// A propositional formula over presence variables: what a type says about
/// *which combinations* of its labels may be there, over and above what each
/// label says on its own.
///
/// `{x when a: 'a, y when b: 'b} -> {}` says each of `x` and `y` may or may not
/// be there and nothing about the two together; the same type `where a != b`
/// says exactly one of them is. No unconstrained type says that, which is the
/// whole reason this exists — see the `presence-sat` spec's motivation.
///
/// A tree rather than a normal form, because it is written as one: the surface
/// `where` grammar has `=`, `!=`, `or`, `and` and `not`, and an annotation that
/// re-parses as itself has to keep them. [`Formula::canonical`] is where a
/// normal form is taken, once, at generalization.
///
/// The constructors below simplify as they build — `and` with [`Formula::True`]
/// is the other side — so the common case of a formula that says nothing is the
/// value `True` rather than a tree of trues to be recognized later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Formula {
    /// The constraint that says nothing. A scheme carrying this prints with no
    /// `where` clause at all.
    True,
    /// The constraint nothing satisfies. Never written; what conjoining two
    /// batches that contradict each other comes to.
    False,
    Atom(Atom),
    Not(Rc<Formula>),
    And(Rc<Formula>, Rc<Formula>),
    Or(Rc<Formula>, Rc<Formula>),
    /// `a = b` — both there or neither.
    Iff(Rc<Formula>, Rc<Formula>),
    /// `a != b` — exactly one of them there.
    Xor(Rc<Formula>, Rc<Formula>),
}

/// A type closed over the variables it binds, and what it requires of the
/// presences among them.
///
/// One numbering, not two. Every variable a scheme quantifies prints as a bare
/// letter — a type as `a`, a struct's rest as `..a`, a presence as `when a` —
/// so two alphabets would be two things spelled the same way with nothing to
/// tell them apart. The presences take the low positions, `0..presences`, and
/// the types and rows the rest, `presences..count`, which is what lets a bare
/// [`Ty`] be printed with no scheme beside it to ask.
#[derive(Debug, Clone)]
pub struct Scheme {
    count: u32,
    presences: u32,
    body: Rc<Ty>,
    /// What has to hold of the presences this scheme quantifies. A constrained
    /// scheme in the HM(X) sense: instantiating one conjoins this, with fresh
    /// variables substituted for the bound ones, into the constraint store.
    formula: Formula,
}

/// A type: what it is, and the struct fields it carries.
///
/// Every type has both halves. `Nat` is the `Nat` core with no fields; a struct
/// is the [`Core::Unit`] core with fields; and a `Nat` that carries an `x` is
/// the `Nat` core with an `x`, which the solver can infer and the printer can
/// show even though no source syntax writes one.
///
/// Splitting them this way is what makes "has fields" a property of every type
/// rather than of one shape of type. Two types are equal when they name the same
/// labels and their cores agree about everything else, which is one rule where
/// there used to be a rule about structs and a complaint for everything else.
///
/// The fields are a bare label map with no tail of their own, because the core
/// beside them *is* their tail: `{ x: Nat, ..r }` is the type `r` carrying an
/// `x`, and what the type says about the fields it does not name is the whole of
/// what its core says. So one variable does the work two used to — `fn a => a.x`
/// is `{ x: 'a, ..'b } -> 'a` — and substituting for a struct's `..` is
/// substituting a type, which is the one substitution the compiler already has.
#[derive(Debug, Clone, Default)]
pub struct Ty {
    pub core: Core,
    /// The labels this type carries, each either there or not. What the type
    /// says about the labels it does *not* name is [`Ty::core`]: a `Unit`,
    /// `Nat`, `Arrow`, `Sum` or `Named` core names no others, a [`Core::Var`]
    /// stands for whatever that variable has, a [`Core::Bound`] for whatever the
    /// parameter has, and [`Core::Undecided`] has nothing further to report.
    pub fields: IndexMap<String, RowField>,
}

/// What a type is, before the fields it carries.
#[derive(Debug, Clone, Default)]
pub enum Core {
    /// A type with nothing of its own. `{}` and `()` are this with no fields,
    /// which is why unit falls out of the printer as `{}` rather than as `()`:
    /// there is one type here and one spelling for it.
    Unit,
    Nat,
    /// `A -> B ! E` — what it takes, what it gives back, and the effects
    /// calling it may perform.
    ///
    /// The row is an *upper bound* and never a demand on the caller: a closed
    /// one says the function performs no more than those effects. A bare
    /// `A -> B` is [`Row::closed`] with no labels, which is what "pure" is, and
    /// what the printer writes as nothing at all.
    ///
    /// The third position is [`Shape::Effect`]'s only home, the way
    /// [`Core::Sum`] is [`Shape::Sum`]'s: "may perform these effects" is not a
    /// property of every type, so there is nowhere else for the row to live.
    Arrow(Rc<Ty>, Rc<Ty>, Row),
    /// The cases a value may be: a row of labels, each with a presence, and a
    /// tail saying what is known about the cases not named.
    ///
    /// A value *is* one of the cases the row says is there, where the fields
    /// beside this core are labels a value *has*. Everything between those two
    /// sentences — unification, flattening, the lacks condition, generalization
    /// — is written once and reaches both, with [`Shape`] as the only thing
    /// saying which is being read.
    ///
    /// The one place a [`Row`] survives, and so the one place a [`Rest`] does.
    /// A struct's `..` moved into the core beside its fields because every type
    /// has fields and so every core can carry them; "is one of these cases" is
    /// not a property of every type, and this is the only core with a case row,
    /// so there is no core position for a sum's tail to move into.
    Sum(Row),
    Var(TyVar),
    /// A variable some [`Scheme`] binds, by its position in that scheme.
    ///
    /// Which scheme depends on where the type came from, and the two never
    /// meet. In a definition's scheme it is a variable generalization
    /// quantified, and instantiation hands it a fresh [`Core::Var`]. In a
    /// declaration's scheme it is one of the declaration's parameters, and
    /// unfolding hands it the argument written at the use site. Both are the
    /// same substitution — see `open` in [`inference`](crate::inference) — which
    /// is why one representation serves both and there is no second way to get
    /// it wrong.
    ///
    /// Not a leaf, unlike [`Rest::Bound`] and [`Presence::Bound`]: what it
    /// stands for is supplied from outside, but the type it sits in may carry
    /// fields of its own, and those are spliced onto whatever arrives.
    ///
    /// Which is exactly a struct's row parameter. `type WithX r = { x: Nat,
    /// ..r }` lowers to this at index 0 with an `x` beside it, so `WithX Nat`
    /// opens to a `Nat` carrying an `x` and `WithX { y: Nat }` to a struct
    /// carrying both — one substitution, and the one the compiler already had.
    Bound(u32),
    /// A variable an annotation's `where let` declared, while the body under
    /// that annotation is being checked.
    ///
    /// Rigid: equal only to a rigid with the same `id`, and never bound to
    /// anything. That is the whole of what makes an annotation a promise rather
    /// than a suggestion — a body that decides what `a` is meets this and is
    /// refused at the expression that decided it, instead of being told
    /// afterwards that the annotation as a whole was too open.
    ///
    /// `id` is unique per declaration occurrence across the whole program, so
    /// two annotations that each write `a` never collide however alike they
    /// look. `name` is the spelling, carried for the reason [`Core::Named`]
    /// carries one: [`Display`](std::fmt::Display) is handed a bare type with no
    /// table to ask.
    ///
    /// A leaf, unlike [`Core::Bound`]. Nothing supplies a value for it — the
    /// scheme the annotation publishes re-quantifies it into a [`Core::Bound`]
    /// at generalization — so the fields written beside one are the whole of
    /// what a type carrying it says.
    Rigid {
        id: u32,
        name: Rc<str>,
    },
    /// A declared type, held as the name it was written as rather than as what
    /// it stands for, applied to whatever it was given.
    ///
    /// This is the only place a type can be recursive, and the only reason it
    /// can be: `symbol` keys
    /// [`inference::Output::aliases`](crate::inference::Output::aliases), so a
    /// declaration whose body names itself is a lookup that comes back round
    /// rather than a tree that never ends. Nothing here points at the body, so
    /// a type is still a finite tree — which is what keeps `Clone`, `Debug` and
    /// every walk in the compiler terminating.
    ///
    /// The arguments are the exception to that, and the one thing about this
    /// variant a walk may not skip. A body holds no solver variable — lowering
    /// refuses a `..` or a `when` in a declaration for exactly that reason — but
    /// `args` is written at the use site and holds whatever that site had. So
    /// every walk stops at the body and descends into the arguments: see
    /// [`Table::occurs`](crate::inference), which does both in one pass.
    ///
    /// Two of these are the same type exactly when what they stand for is the
    /// same type. A name is a barrier to *unfolding*, never to equality: two
    /// declarations written the same way are one type however differently they
    /// were spelled, and a declaration is not a new type merely for having a
    /// name of its own.
    ///
    /// Which leaves the name one job, and it is a shortcut. Where every
    /// parameter of a declaration survives unfolding — each one landing in a
    /// structural position of the body, as `Pair`'s do — two applications of it
    /// are equal if and only if their arguments are, so the arguments can be
    /// compared directly and the bodies never built. That is
    /// [`Rule::Congruent`](crate::inference::Rule), and it is sound *and*
    /// complete there, which is the whole of what makes it safe to take: it can
    /// only ever agree with unfolding.
    ///
    /// A parameter the declaration discards buys no distinction. `type Ptr a =
    /// Nat` stands for `Nat` whatever it is applied to, so `Ptr A` and `Ptr B`
    /// are one type — the language has no phantom types, and a declaration is
    /// nominal within itself only where being nominal agrees with what it
    /// stands for. Congruence is refused for such a declaration precisely
    /// because taking it would be a decision contradicting the rule above
    /// rather than a shortcut to it.
    ///
    /// None of this is what keeps unfolding finite. That is the assumption
    /// [`Solve::unfold`](crate::inference) records, which is keyed on the whole
    /// goal, arguments included, and comes back round because
    /// [`ir::build`](crate::ir::build) refuses a recursion that grows one. See
    /// [`Solve::unify`](crate::inference).
    ///
    /// `name` is what the type prints as and is never compared:
    /// [`Display`](std::fmt::Display) is handed a bare type with no mint to
    /// ask, and a spelling is cheaper to carry than a context to thread through
    /// every diagnostic in the compiler.
    Named {
        symbol: Symbol,
        name: Rc<str>,
        /// What the declaration was applied to, in order. Empty for one that
        /// takes nothing, which is every declaration the language had before
        /// type constructors.
        ///
        /// `Rc<[_]>` rather than `Vec`: a type is cloned on nearly every step
        /// the solver takes, and the arguments should not be copied with it.
        args: Rc<[Rc<Ty>]>,
    },
    #[default]
    Undecided,
}

/// The cases a sum allows, each either there or not, and what is known about
/// the ones it does not list.
///
/// A sum's business and nothing else: this appears inside [`Core::Sum`] and
/// nowhere else in the type language, because a struct's fields are a bare label
/// map on [`Ty`] whose tail is the core beside them. See [`Rest`] for why the
/// two are not the same shape of thing.
///
/// A type of its own rather than a shape of [`Ty`], and that is what makes the
/// recursion terminate: a closed tail is a [`Rest::Closed`], not a type that
/// would need a tail of its own.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub labels: IndexMap<String, RowField>,
    pub rest: Rest,
}

/// What is known about the cases a [`Row`] does not name.
///
/// A sum's alone. A struct once had one of these too, and it said the same thing
/// twice: a [`Core::Var`] beside a field map already stands for a whole type
/// that may carry fields of its own, so the tail variable next to it was a
/// second mechanism for one meaning. The two are now one, and what a struct says
/// about the fields it does not name is its core.
///
/// The same merge cannot be done here, and that asymmetry is why this type
/// survives. "Has fields" is a property of every type, so every core can carry
/// them; "is one of these cases" is not, and [`Core::Sum`] is the only core with
/// a case row, so there is no core position for a sum's tail to move into. It is
/// also why [`Sense`], [`ir::ErrorKind::NotARow`](crate::ir::ErrorKind) and
/// [`ir::ErrorKind::MixedParameter`](crate::ir::ErrorKind) still have two things
/// to tell apart.
#[derive(Debug, Clone, Default)]
pub enum Rest {
    /// Every case not named is absent: the row lists all of them.
    #[default]
    Closed,
    Var(TyVar),
    /// A tail a scheme quantified, or one a declaration takes as a sum's rest.
    /// A leaf: what it stands for is supplied from outside.
    Bound(u32),
    /// A rest an annotation's `where let` declared: [`Core::Rigid`] about a
    /// sum's cases, and rigid for the same reason and on the same terms.
    Rigid {
        id: u32,
        name: Rc<str>,
    },
    /// A failure abandoned the question, or a reporter froze it. Absorbs, the
    /// way [`Core::Undecided`] does.
    Undecided,
    /// A tail that has been decided to be more cases, and then whatever is
    /// past them: a sum's row parameter handed a written sum, or a tail variable
    /// waiting to be spliced in. Flattened by the one function that resolves a
    /// row, so no reader ever sees the chain.
    More(Rc<Row>),
}

/// One label: whether it is there, and what it holds when it is. A field of
/// [`Ty::fields`], or a case of a [`Row`].
///
/// One type for both shapes, because the question is the same one twice. In a
/// struct it is a field: whether a value has it, and what it holds. In a sum it
/// is a case: whether a value may be it, and what it carries. Every rule the
/// solver has about presence is written once and reaches both.
///
/// When `presence` resolves to [`Presence::Absent`], `ty` is meaningless and
/// deliberately left unconstrained: a field that is not there has nothing to
/// have a type, and neither has a case a value can never be.
#[derive(Debug, Clone)]
pub struct RowField {
    pub presence: Presence,
    pub ty: Rc<Ty>,
}

/// Whether one label is there.
///
/// A type of its own, so that "there", "not there" and "still being decided"
/// are the only three answers a field can have and nothing has to say in prose
/// that no other type may be written here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Presence {
    Present,
    /// The label is definitely not there. Written as `\name` in a struct type
    /// and `` \`Name `` in a sum — the one spelling of absence the surface
    /// syntax has — and otherwise arising from solving, when an open row meets
    /// a closed one that lacks the label.
    Absent,
    Var(TyVar),
    /// A presence a scheme quantified, which prints as the `when` clause on its
    /// label: `{x when a: Nat}`, and `` `A (when a) Nat ``.
    Bound(u32),
    /// A failure abandoned the question, or a reporter froze it. The one
    /// presence that still prints as a `?`, and no syntax reads one back.
    #[default]
    Undecided,
}

/// What one variable stands for, whichever of the three sorts it was minted
/// for.
///
/// One value serving all three places a variable's meaning is handed about:
/// what the solver's table has bound it to, what a [`Scheme`]'s `open`
/// substitutes for it, and what
/// [`Effect::Bound`](crate::inference::Effect::Bound) reports. A variable's
/// sort is fixed by the position it was minted for and never changes, so the
/// three can share one table and one numbering without ever being confused.
#[derive(Debug, Clone)]
pub enum Assigned {
    Ty(Rc<Ty>),
    Row(Rc<Row>),
    Presence(Presence),
}

impl ParamKind {
    /// The labels an argument written at this parameter may not name, whichever
    /// of the two it is. The one question every reader of a kind actually asks,
    /// so it is asked in one place rather than matched out at each of them.
    ///
    /// Empty for the parameters that are only ever handed a type, which is most
    /// of them: `A` in `type Pair A B` sits in no row and forbids nothing.
    pub fn lacks(&self) -> &IndexSet<String> {
        match self {
            ParamKind::Type { lacks }
            | ParamKind::Cases { lacks }
            | ParamKind::Effects { lacks } => lacks,
        }
    }

    /// What this parameter stands for, with the labels dropped. See [`Sense`].
    pub fn sense(&self) -> Sense {
        match self {
            ParamKind::Type { .. } => Sense::Type,
            ParamKind::Cases { .. } => Sense::Cases,
            ParamKind::Effects { .. } => Sense::Effects,
        }
    }

    /// The labels an argument written here may not name, and the row it would
    /// be spliced into — or `None` when the parameter stands for a type. What
    /// the one check that is still about a shape asks: a sum's rest and an
    /// arrow's effects are both spliced into a row, so only a row can go there.
    pub fn cases(&self) -> Option<(Shape, &IndexSet<String>)> {
        match self {
            ParamKind::Type { .. } => None,
            ParamKind::Cases { lacks } => Some((Shape::Sum, lacks)),
            ParamKind::Effects { lacks } => Some((Shape::Effect, lacks)),
        }
    }
}

impl Assigned {
    /// This value read as a whole type.
    ///
    /// A type is what a type position is opened to, and every caller hands one:
    /// a declaration's argument is a written type, and a scheme's fresh
    /// variable is a bare core. A row or a presence reaching a type position
    /// would be a parameter used at two sorts, which nothing can write — so
    /// rather than a rule for it there is a type that says nothing, which
    /// absorbs the way every other unanswerable type does.
    pub fn as_ty(&self) -> Rc<Ty> {
        match self {
            Assigned::Ty(ty) => ty.clone(),
            Assigned::Row(_) | Assigned::Presence(_) => Rc::new(Ty::plain(Core::Undecided)),
        }
    }

    /// This value read as a sum's cases: what a `..` at a sum's row parameter
    /// stands for.
    ///
    /// Three ways to arrive, and the middle one is why this is a conversion
    /// rather than a lookup. A row outright is the row. A *type* is what a use
    /// site writes — `` Fallible (`Ok Nat) `` hands a sum where a set of cases
    /// goes — so it is read for the cases it allows. And a type that is only a
    /// bare variable is the commonest of the three: instantiating a scheme mints
    /// one variable per quantified position, and a position the scheme used as a
    /// sum's tail wants that variable standing for the rest rather than for a
    /// type with no cases at all.
    ///
    /// No shape to be told any more. A struct's `..` is its core, and opening
    /// one is [`as_ty`](Self::as_ty); only a sum's rest is still a row, so this
    /// is about cases and nothing else.
    ///
    /// A presence cannot reach a tail, for the reason it cannot reach a type
    /// position, and closes the row rather than inventing a rule.
    pub fn as_row(&self) -> Row {
        match self {
            Assigned::Row(row) => (**row).clone(),
            Assigned::Ty(ty) => match (&ty.core, ty.fields.is_empty()) {
                (Core::Var(var), true) => Row::of(Rest::Var(*var)),
                _ => ty.cases(),
            },
            Assigned::Presence(_) => Row::closed(),
        }
    }

    /// This value read as whether one label is there.
    ///
    /// [`as_ty`](Self::as_ty)'s twin at the third sort, and the one a scheme's
    /// low positions are opened at. A type or a row arriving here would be a
    /// scheme numbering a presence above its own `presences`, which nothing
    /// builds — so, rather than a rule for it, there is a presence that says
    /// nothing and absorbs the way every other unanswerable value does.
    pub fn presence(&self) -> Presence {
        match self {
            Assigned::Presence(presence) => presence.clone(),
            Assigned::Ty(_) | Assigned::Row(_) => Presence::Undecided,
        }
    }

    /// A variable of this value's sort. What a binding that was refused
    /// abandons: the variable it would have bound, said in the sort it was
    /// minted for.
    pub fn variable(&self, var: TyVar) -> Self {
        match self {
            Assigned::Ty(_) => Assigned::Ty(Rc::new(Ty::plain(Core::Var(var)))),
            Assigned::Row(_) => Assigned::Row(Rc::new(Row::of(Rest::Var(var)))),
            Assigned::Presence(_) => Assigned::Presence(Presence::Var(var)),
        }
    }

    /// The undecided value of this value's sort. What a failure points a
    /// variable at, so that one complaint is not echoed by everything
    /// downstream of it.
    pub fn undecided(&self) -> Self {
        match self {
            Assigned::Ty(_) => Assigned::Ty(Rc::new(Ty::default())),
            Assigned::Row(_) => Assigned::Row(Rc::new(Row::of(Rest::Undecided))),
            Assigned::Presence(_) => Assigned::Presence(Presence::Undecided),
        }
    }
}

impl Core {
    /// `from -> to`, performing nothing: [`Row::closed`] with no labels, which
    /// is what a bare `A -> B` means and what the printer writes as nothing at
    /// all. Every position that builds an arrow with no effects to put on it
    /// goes through here rather than spelling the empty row again.
    pub fn pure(from: Rc<Ty>, to: Rc<Ty>) -> Self {
        Core::Arrow(from, to, Row::closed())
    }
}

impl From<Prim> for Core {
    fn from(value: Prim) -> Self {
        match value {
            Prim::Nat => Core::Nat,
        }
    }
}

impl Ty {
    /// A type that is only its core, carrying no fields at all. Every type the
    /// language can currently *write* is one of these or a [`Ty::unit`] with
    /// fields, so this is what nearly every constructor in the compiler wants.
    pub fn plain(core: Core) -> Self {
        Self {
            core,
            fields: IndexMap::new(),
        }
    }

    /// The type with nothing of its own and no fields: what `()` and `{}` both
    /// spell, what a case written with no payload carries, and what a struct
    /// type is before its fields are put in.
    ///
    /// One spelling and one constructor, because a second empty type would be
    /// one the solver could find not quite equal to the first.
    pub fn unit() -> Self {
        Self::plain(Core::Unit)
    }

    /// The cases this type allows: the row inside a [`Core::Sum`], and the one
    /// thing anything still asks a type for a row about.
    ///
    /// Anything else is an argument [`ir::build`](crate::ir::build) already
    /// refused and erased — `` Or Nat `` for `` type Or r = `A | ..r `` is the
    /// only way to reach it — so the tail it leaves behind is undecided rather
    /// than closed, which is what an erased argument has always been.
    pub fn cases(&self) -> Row {
        match &self.core {
            Core::Sum(cases) => cases.clone(),
            _ => Row::of(Rest::Undecided),
        }
    }
}

impl Row {
    /// A row that names no cases of its own, and then whatever `rest` allows.
    ///
    /// What a tail *is*, written as the row it stands for: the sort a row
    /// variable has is the row sort, so a tail is compared and bound as a row
    /// with no labels in front of it. Also what every "nothing yet" row is —
    /// the undecided one a failure abandons a tail to, and the fresh one a
    /// variable stands for — so there is one spelling of a bare row instead of
    /// six copies of a literal that have to agree.
    pub fn of(rest: Rest) -> Self {
        Self {
            labels: IndexMap::new(),
            rest,
        }
    }

    /// The row that names nothing and allows nothing more.
    pub fn closed() -> Self {
        Self::of(Rest::Closed)
    }
}

impl RowField {
    /// A label that is definitely there: what a struct literal's fields are,
    /// what a written `name: Ty` field lowers to, and what the one case a tag
    /// literal names is.
    pub fn present(ty: Rc<Ty>) -> Self {
        Self {
            presence: Presence::Present,
            ty,
        }
    }
}

impl Scheme {
    /// Close `body` over the type and row variables it binds, requiring nothing
    /// of its presences. Every [`Core::Bound`] and [`Rest::Bound`] in `body`
    /// must be an index below `count`; opening one trusts that.
    ///
    /// Two things are closed this way and the difference is only in who
    /// supplies the values: a definition's scheme binds what generalization
    /// quantified, and instantiation hands each one a fresh variable; a
    /// declaration's binds its parameters, and unfolding hands each one the
    /// argument written at the use site. See [`Core::Bound`].
    ///
    /// A declaration's scheme is always one of these: a declaration's body
    /// holds no presence variable — lowering refuses a `when` there for the
    /// reason it refuses a `..` — so there is nothing for it to quantify or to
    /// require.
    pub fn new(count: u32, body: Rc<Ty>) -> Self {
        Self {
            count,
            presences: 0,
            body,
            formula: Formula::True,
        }
    }

    /// [`new`](Self::new) with the presences a definition's generalization
    /// quantified, and what it requires of them.
    pub fn constrained(count: u32, presences: u32, body: Rc<Ty>, formula: Formula) -> Self {
        Self {
            count,
            presences,
            body,
            formula,
        }
    }

    /// How many variables the scheme quantifies, of every sort together. Zero
    /// means the type is monomorphic and instantiation copies nothing.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// How many of them are presences, which is where the type and row
    /// variables start: the low positions are the presences, for the reason
    /// [`Scheme`] gives.
    pub fn presences(&self) -> u32 {
        self.presences
    }

    pub fn body(&self) -> &Rc<Ty> {
        &self.body
    }

    /// What has to hold of the presences this scheme quantifies.
    pub fn formula(&self) -> &Formula {
        &self.formula
    }
}

impl Formula {
    /// The formula naming one solver variable.
    pub fn var(var: TyVar) -> Self {
        Formula::Atom(Atom::Var(var))
    }

    /// The formula naming one variable a scheme quantified.
    pub fn bound(index: u32) -> Self {
        Formula::Atom(Atom::Bound(index))
    }

    /// Whether this formula says nothing, and so writes no `where` clause.
    pub fn is_true(&self) -> bool {
        matches!(self, Formula::True)
    }

    /// The negation, with the two constants and a double negative folded away.
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        match self {
            Formula::True => Formula::False,
            Formula::False => Formula::True,
            Formula::Not(inner) => (*inner).clone(),
            other => Formula::Not(Rc::new(other)),
        }
    }

    /// Both, with the constants folded away — which is what makes "says
    /// nothing" the value [`Formula::True`] rather than a tree of them.
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Formula::False, _) | (_, Formula::False) => Formula::False,
            (Formula::True, kept) | (kept, Formula::True) => kept,
            (left, right) => Formula::And(Rc::new(left), Rc::new(right)),
        }
    }

    /// Either, folded the same way.
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Formula::True, _) | (_, Formula::True) => Formula::True,
            (Formula::False, kept) | (kept, Formula::False) => kept,
            (left, right) => Formula::Or(Rc::new(left), Rc::new(right)),
        }
    }

    /// Both or neither: what `a = b` says.
    pub fn iff(self, other: Self) -> Self {
        Formula::Iff(Rc::new(self), Rc::new(other))
    }

    /// Exactly one: what `a != b` says.
    pub fn xor(self, other: Self) -> Self {
        Formula::Xor(Rc::new(self), Rc::new(other))
    }

    /// Every one of them, left to right.
    pub fn all(parts: impl IntoIterator<Item = Self>) -> Self {
        parts.into_iter().fold(Formula::True, Formula::and)
    }

    /// Any one of them, left to right.
    pub fn any(parts: impl IntoIterator<Item = Self>) -> Self {
        parts.into_iter().fold(Formula::False, Formula::or)
    }

    /// Every atom this formula names, in the order it first names them.
    /// First-appearance order is what decides the printed alphabet, so it is
    /// what the walk preserves.
    pub fn atoms(&self, out: &mut Vec<Atom>) {
        match self {
            Formula::True | Formula::False => {}
            Formula::Atom(atom) => {
                if !out.contains(atom) {
                    out.push(*atom);
                }
            }
            Formula::Not(inner) => inner.atoms(out),
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Iff(left, right)
            | Formula::Xor(left, right) => {
                left.atoms(out);
                right.atoms(out);
            }
        }
    }

    /// Whether this formula holds when each atom is read by `assign`.
    pub fn eval(&self, assign: &dyn Fn(Atom) -> bool) -> bool {
        match self {
            Formula::True => true,
            Formula::False => false,
            Formula::Atom(atom) => assign(*atom),
            Formula::Not(inner) => !inner.eval(assign),
            Formula::And(left, right) => left.eval(assign) && right.eval(assign),
            Formula::Or(left, right) => left.eval(assign) || right.eval(assign),
            Formula::Iff(left, right) => left.eval(assign) == right.eval(assign),
            Formula::Xor(left, right) => left.eval(assign) != right.eval(assign),
        }
    }

    /// One formula with each atom replaced by what `of` makes of it, rebuilt
    /// through the constructors so the constants fold on the way out.
    ///
    /// The one walk every substitution over a formula goes through — opening a
    /// scheme's, reading one through what the solve decided, and quantifying
    /// one into a scheme — so there is one place for a connective to be handled
    /// and no way for three copies to disagree about `Iff`.
    pub fn substitute(&self, of: &dyn Fn(Atom) -> Formula) -> Self {
        match self {
            Formula::True => Formula::True,
            Formula::False => Formula::False,
            Formula::Atom(atom) => of(*atom),
            Formula::Not(inner) => inner.substitute(of).not(),
            Formula::And(left, right) => left.substitute(of).and(right.substitute(of)),
            Formula::Or(left, right) => left.substitute(of).or(right.substitute(of)),
            Formula::Iff(left, right) => left.substitute(of).iff(right.substitute(of)),
            Formula::Xor(left, right) => left.substitute(of).xor(right.substitute(of)),
        }
    }

    /// [`substitute`](Self::substitute) over the *solver's* variables alone: a
    /// variable a scheme quantified is left where it stands.
    ///
    /// What both readers of a store want. Following a variable to what the
    /// solve decided it is, and numbering one into the scheme being
    /// generalized, are the same walk over the same half of the atoms — and
    /// neither has anything to say about a quantified one, since a store is
    /// written about variables that exist.
    pub fn rename(&self, of: &dyn Fn(TyVar) -> Formula) -> Self {
        self.substitute(&|atom| match atom {
            Atom::Var(var) => of(var),
            Atom::Bound(index) => Formula::bound(index),
        })
    }

    /// Replace each variable a scheme bound with what instantiation minted for
    /// it — the formula half of [`Ty::open`], and the whole of what
    /// "instantiating a constrained scheme conjoins its formula with fresh
    /// variables substituted for bound ones" means.
    ///
    /// A presence that has already been decided folds to a constant: a label
    /// certainly there satisfies every literal about it, and one certainly not
    /// there satisfies none. A presence nothing knows anything about claims
    /// nothing, which is [`Formula::True`] — the same answer the undecided type
    /// gives every question.
    pub fn open(&self, fresh: &[Assigned]) -> Self {
        self.substitute(&|atom| match atom {
            Atom::Var(var) => Formula::var(var),
            Atom::Bound(index) => fresh[index as usize].presence().formula(),
        })
    }
}

impl Presence {
    /// This presence read as a formula: what a literal about the label says.
    pub fn formula(&self) -> Formula {
        match self {
            Presence::Present => Formula::True,
            Presence::Absent => Formula::False,
            Presence::Var(var) => Formula::var(*var),
            Presence::Bound(index) => Formula::bound(*index),
            // A failure abandoned the question, so there is nothing here to
            // require — and requiring anything would be the first complaint
            // said again in a second place.
            Presence::Undecided => Formula::True,
        }
    }
}

impl Prim {
    /// Every primitive there is. The one place a new one has to be listed, so
    /// that [`from_name`](Self::from_name) can never fall behind the enum.
    pub const ALL: &'static [Prim] = &[Prim::Nat];

    /// The spelling that denotes this primitive in source. Injective, and
    /// inverted by [`from_name`](Self::from_name), which is what keeps a
    /// printed type re-lowering to the type it was printed from.
    pub const fn name(self) -> &'static str {
        match self {
            Prim::Nat => "Nat",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|prim| prim.name() == name)
    }
}
