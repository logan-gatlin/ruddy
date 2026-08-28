use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::symbol::Symbol;

/// A type built into the language rather than declared in it.
///
/// A primitive names nothing the mint could hand out — the same reason
/// numeric literals carry no symbol — so it is a value here rather than a
/// seeded declaration. Shared by the syntactic
/// [`ir::TypeKind`](crate::ir::TypeKind) and the semantic type language, so the
/// two can never disagree about which primitives exist.
///
/// Unit is deliberately not one of them. `()` is the empty closed
/// [`Ty::Struct`], so there is no primitive for it to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    /// The type of unsigned 64-bit integer literals.
    Nat,
    /// The type of signed 64-bit integers.
    Int,
    /// The type of 64-bit floating-point numbers.
    Real,
    /// The type of UTF-8 text.
    String,
    /// The type with the values true and false.
    Boolean,
}

pub type TyVar = u32;

/// The semantic identity of an effect label.
///
/// Effect names resolve nominally, but effect rows do not: two declarations
/// with the same leaf name and the same normalized operation interface are one
/// label. The interface is deliberately opaque here; lowering builds it from
/// the operation signatures after declared types have been normalized.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectId {
    /// An effect row while the IR builder is still collecting declarations.
    /// No semantic phase may receive one of these.
    Pending(Symbol),
    Structural {
        name: String,
        interface: String,
    },
}

impl EffectId {
    pub fn pending(symbol: Symbol) -> Self {
        Self::Pending(symbol)
    }

    pub fn structural(name: String, interface: String) -> Self {
        Self::Structural { name, interface }
    }

    /// The stable semantic row key. Both components are byte-length prefixed:
    /// imported recovery data is allowed to contain every character, including
    /// the separator used by older artifacts, so a delimiter alone cannot be
    /// an unambiguous identity boundary.
    pub fn row_key(&self) -> String {
        match self {
            Self::Structural { name, interface } => {
                format!(
                    "\u{1e}e{}:{name}{}:{interface}",
                    name.len(),
                    interface.len()
                )
            }
            Self::Pending(_) => panic!("effect row reached inference before structuralization"),
        }
    }

    /// Decode a semantic effect-row key. The length-prefixed spelling is
    /// canonical; the unit-separator spelling remains readable so old artifact
    /// rows can be normalized while importing them.
    pub fn parse_row_key(key: &str) -> Option<(&str, &str)> {
        Self::parse_canonical_row_key(key).or_else(|| key.split_once('\u{1f}'))
    }

    /// Decode only the canonical generated spelling. UI code uses this when a
    /// sum-shaped type argument has no explicit row sense: legacy separators
    /// are artifact input, not enough evidence that an ordinary sum is effects.
    pub fn parse_canonical_row_key(key: &str) -> Option<(&str, &str)> {
        fn component<'a>(key: &'a str, at: &mut usize) -> Option<&'a str> {
            let bytes = key.as_bytes();
            let start = *at;
            while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
                *at += 1;
            }
            if start == *at || bytes.get(*at) != Some(&b':') {
                return None;
            }
            let length: usize = key.get(start..*at)?.parse().ok()?;
            *at += 1;
            let end = at.checked_add(length)?;
            let value = key.get(*at..end)?;
            *at = end;
            Some(value)
        }

        let rest = key.strip_prefix("\u{1e}e")?;
        let mut at = 0;
        let name = component(rest, &mut at)?;
        let interface = component(rest, &mut at)?;
        (at == rest.len()).then_some((name, interface))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Structural { name, .. } => name,
            Self::Pending(_) => "<unresolved effect>",
        }
    }
}

/// Which of the two sets of labels a rule is being read about.
///
/// A set of labels, each either there or not, plus something saying what is
/// known about the names it does not list. Both composites in the language are
/// one: the fields every type carries are a set of labels, all of which a value
/// has, and a sum is a set of cases, one of which a value is. The two are
/// unified, flattened, generalized and printed by the same code, and this is the
/// only thing that tells them apart.
///
/// Every shape uses an explicit [`Row`] and [`Rest`]; the containing `Ty`
/// variant determines whether its labels are fields, cases, or effects.
///
/// Stored nowhere. Where a set of labels sits already says which it is, so the
/// shape is a *reading* the caller carries down — the way `solve::Rowed` does —
/// and never a second copy of a fact the position already settles.
///
/// Never inferred and never defaulted: which one a set of labels is, is decided
/// by the syntax that wrote it — braces or `#`s — and travels with the type
/// from there. Two of different shapes are two types, and the solver refuses
/// them the way it refuses a `Nat` against an arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// `{ x: Nat, ..'r }` — a value has every field the row says is there.
    Struct,
    /// `#A Nat | ..'r` — a value is one of the cases the row says is there.
    Sum,
    /// `A -> B + !Log + ..'r` — calling the arrow may perform every effect
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
/// Struct fields, sum cases, and arrow effects are distinct row sorts. A row
/// parameter can only be forwarded to a position with the same sense; none is a
/// whole type. Presence is the additional annotation-only sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sense {
    Type,
    /// The rest of a struct's fields.
    Fields,
    Cases,
    /// The effects an arrow may perform: `..'e` in `type Runner 'e = (Nat -> Nat
    /// + ..'e) -> Nat + ..'e`.
    Effects,
    /// Whether one label is there — what a `when a` names and what a `where`
    /// clause's formula is written about.
    Presence,
}

/// What one parameter of a `type` declaration stands for.
///
/// Written nowhere: a parameter is a name and a sigil, and which of these it is
/// follows from where the body uses it — `..'r` in a sum makes a rest of cases,
/// anything else makes a type. Worked out in [`ir::build`](crate::ir::build),
/// and carried here so that the readers who need it — lowering, inference and
/// the debugger — agree.
///
/// Two of them, and no way to write a third: a type, or the rest of a sum. That
/// is what keeps this a check rather than a language: every parameter is one of
/// these, every declaration takes a fixed list of them, and nothing takes a
/// declaration.
///
/// Both carry the labels an argument written there may not name. `'r` in
/// `type WithX 'r = { x: Nat, ..'r }` is a [`ParamKind::Type`] whose set is
/// `{x}`: the constructor covers the fields the declaration does not write out, so an
/// `'r` with an `x` of its own would give the type two fields of one name, and
/// the two copies could disagree. Carrying the set rather than a bare flag is what lets
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
    /// Stands for a whole type. `'A` in `type Pair 'A 'B`.
    Type { lacks: IndexSet<String> },
    /// Stands for fields a struct does not name. `'r` in
    /// `type WithX 'r = { x: Nat, ..'r }`.
    Fields { lacks: IndexSet<String> },
    /// Stands for the cases a sum does not name — and, with them, the cases it
    /// may therefore not name itself. `'r` in `type Or 'r = #A | ..'r`.
    ///
    /// The one reading that is not a type, and the reason it is enforced rather
    /// than substituted: a sum's rest is spliced into [`Ty::Sum`]'s row, so
    /// anything else written there would leave a row holding what no row can
    /// hold. See [`ir::ErrorKind::NotARow`](crate::ir::ErrorKind).
    Cases { lacks: IndexSet<String> },
    /// Stands for the effects an arrow does not name — and, with them, the
    /// effects it may therefore not name itself. `'e` in
    /// `type Runner 'e = (Nat -> Nat + ..'e) -> Nat + ..'e`.
    ///
    /// [`ParamKind::Cases`]'s twin, and enforced for the same reason: an effect
    /// row's rest is spliced into the row [`Ty::Arrow`] carries, so anything
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
/// letter — a type as `'a`, a struct's rest as `..'a`, a presence as `when 'a` —
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

/// A semantic type. Only `Struct` contains structural fields; every other
/// constructor is fieldless by construction.
#[derive(Debug, Clone, Default)]
pub enum Ty {
    Nat,
    Int,
    Real,
    String,
    Boolean,
    /// `A -> B + E` — what it takes, what it gives back, and the effects
    /// calling it may perform.
    ///
    /// The row is an *upper bound* and never a demand on the caller: a closed
    /// one says the function performs no more than those effects. A bare
    /// `A -> B` is [`Row::closed`] with no labels, which is what "pure" is, and
    /// what the printer writes as nothing at all.
    ///
    /// The third position is [`Shape::Effect`]'s only home, the way
    /// [`Ty::Sum`] is [`Shape::Sum`]'s: "may perform these effects" is not a
    /// property of every type, so there is nowhere else for the row to live.
    Arrow(Rc<Ty>, Rc<Ty>, Row),
    /// A structural record and its true field-row tail.
    Struct(Row),
    /// The cases a value may be: a row of labels, each with a presence, and a
    /// tail saying what is known about the cases not named.
    ///
    /// A value *is* one of the cases the row says is there, where the fields
    /// in this struct are labels a value *has*. Everything between those two
    /// sentences — unification, flattening, the lacks condition, generalization
    /// — is written once and reaches both, with [`Shape`] as the only thing
    /// saying which is being read.
    ///
    /// Sum cases and their explicit tail.
    Sum(Row),
    Var(TyVar),
    /// A variable some [`Scheme`] binds, by its position in that scheme.
    ///
    /// Which scheme depends on where the type came from, and the two never
    /// meet. In a definition's scheme it is a variable generalization
    /// quantified, and instantiation hands it a fresh [`Ty::Var`]. In a
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
    /// Which is exactly a struct's row parameter. `type WithX 'r = { x: Nat,
    /// ..'r }` lowers to this at index 0 with an `x` beside it, so `WithX Nat`
    /// opens to a `Nat` carrying an `x` and `WithX { y: Nat }` to a struct
    /// carrying both — one substitution, and the one the compiler already had.
    Bound(u32),
    /// A variable an annotation introduced, while the body under
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
    /// look. `name` is the spelling, carried for the reason [`Ty::Named`]
    /// carries one: [`Display`](std::fmt::Display) is handed a bare type with no
    /// table to ask.
    ///
    /// A leaf, unlike [`Ty::Bound`]. Nothing supplies a value for it — the
    /// scheme the annotation publishes re-quantifies it into a [`Ty::Bound`]
    /// at generalization — so the fields written in one are the whole of
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
    /// A parameter the declaration discards buys no distinction. `type Ptr 'a =
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

/// The fields/cases/effects a row allows and what is known beyond its labels.
/// the ones it does not list.
///
/// A type of its own rather than a shape of [`Ty`], and that is what makes the
/// recursion terminate: a closed tail is a [`Rest::Closed`], not a type that
/// would need a tail of its own.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub labels: IndexMap<String, RowField>,
    pub rest: Rest,
}

/// What is known beyond the labels a [`Row`] names. The containing type fixes
/// whether this is a field, case, or effect-row tail.
#[derive(Debug, Clone, Default)]
pub enum Rest {
    /// Every case not named is absent: the row lists all of them.
    #[default]
    Closed,
    Var(TyVar),
    /// A tail a scheme quantified, or one a declaration takes as a sum's rest.
    /// A leaf: what it stands for is supplied from outside.
    Bound(u32),
    /// A rest an annotation introduced: [`Ty::Rigid`] about a
    /// sum's cases, and rigid for the same reason and on the same terms.
    Rigid {
        id: u32,
        name: Rc<str>,
    },
    /// A failure abandoned the question, or a reporter froze it. Absorbs, the
    /// way [`Ty::Undecided`] does.
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
    /// and `\#Name` in a sum — the one spelling of absence the surface
    /// syntax has — and otherwise arising from solving, when an open row meets
    /// a closed one that lacks the label.
    Absent,
    Var(TyVar),
    /// A foreign solver variable retained only long enough to canonicalize an
    /// imported interface. Its namespace is disjoint from both local variables
    /// and scheme bounds; opening an imported scheme recovers it to undecided.
    Recovered(TyVar),
    /// A presence a scheme quantified, which prints as the `when` clause on its
    /// label: `{x when a: Nat}`, and `#A (when a) Nat`.
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
    /// of them: `'A` in `type Pair 'A 'B` sits in no row and forbids nothing.
    pub fn lacks(&self) -> &IndexSet<String> {
        match self {
            ParamKind::Type { lacks }
            | ParamKind::Fields { lacks }
            | ParamKind::Cases { lacks }
            | ParamKind::Effects { lacks } => lacks,
        }
    }

    /// What this parameter stands for, with the labels dropped. See [`Sense`].
    pub fn sense(&self) -> Sense {
        match self {
            ParamKind::Type { .. } => Sense::Type,
            ParamKind::Fields { .. } => Sense::Fields,
            ParamKind::Cases { .. } => Sense::Cases,
            ParamKind::Effects { .. } => Sense::Effects,
        }
    }

    /// The labels an argument written here may not name, and the row it would
    /// be spliced into — or `None` when the parameter stands for a type. What
    /// the one check that is still about a shape asks: a sum's rest and an
    /// arrow's effects are both spliced into a row, so only a row can go there.
    pub fn row(&self) -> Option<(Shape, &IndexSet<String>)> {
        match self {
            ParamKind::Type { .. } => None,
            ParamKind::Fields { lacks } => Some((Shape::Struct, lacks)),
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
    /// variable is an unconstrained type. A row or a presence reaching a type position
    /// would be a parameter used at two sorts, which nothing can write — so
    /// rather than a rule for it there is a type that says nothing, which
    /// absorbs the way every other unanswerable type does.
    pub fn as_ty(&self) -> Rc<Ty> {
        match self {
            Assigned::Ty(ty) => ty.clone(),
            Assigned::Row(_) | Assigned::Presence(_) => Rc::new(Ty::Undecided),
        }
    }

    /// This value read as a sum's cases: what a `..` at a sum's row parameter
    /// stands for.
    ///
    /// Three ways to arrive, and the middle one is why this is a conversion
    /// rather than a lookup. A row outright is the row. A *type* is what a use
    /// site writes — `Fallible (#Ok Nat)` hands a sum where a set of cases
    /// goes — so it is read for the cases it allows. And a type that is only a
    /// bare variable is the commonest of the three: instantiating a scheme mints
    /// one variable per quantified position, and a position the scheme used as a
    /// sum's tail wants that variable standing for the rest rather than for a
    /// type with no cases at all.
    ///
    /// No shape to be told any more. A struct's `..` is its row tail, and opening
    /// one is [`as_ty`](Self::as_ty); only a sum's rest is still a row, so this
    /// is about cases and nothing else.
    ///
    /// A presence cannot reach a tail, for the reason it cannot reach a type
    /// position, and closes the row rather than inventing a rule.
    pub fn as_row(&self) -> Row {
        match self {
            Assigned::Row(row) => (**row).clone(),
            Assigned::Ty(ty) => match &**ty {
                Ty::Var(var) => Row::of(Rest::Var(*var)),
                Ty::Struct(row) | Ty::Sum(row) => row.clone(),
                _ => Row::of(Rest::Undecided),
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
            Assigned::Ty(_) => Assigned::Ty(Rc::new(Ty::Var(var))),
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

fn take_row_children(row: &mut Row, types: &mut Vec<Rc<Ty>>, rows: &mut Vec<Rc<Row>>) {
    types.extend(
        std::mem::take(&mut row.labels)
            .into_values()
            .map(|field| field.ty),
    );
    if let Rest::More(more) = std::mem::take(&mut row.rest) {
        rows.push(more);
    }
}

fn take_ty_children(ty: &mut Ty, types: &mut Vec<Rc<Ty>>, rows: &mut Vec<Rc<Row>>) {
    match ty {
        Ty::Arrow(from, to, effects) => {
            types.push(std::mem::replace(from, Rc::new(Ty::Undecided)));
            types.push(std::mem::replace(to, Rc::new(Ty::Undecided)));
            take_row_children(effects, types, rows);
        }
        Ty::Struct(row) | Ty::Sum(row) => take_row_children(row, types, rows),
        Ty::Named { args, .. } => {
            types.extend(std::mem::replace(args, Rc::from([])).iter().cloned());
        }
        Ty::Nat
        | Ty::Int
        | Ty::Real
        | Ty::String
        | Ty::Boolean
        | Ty::Var(_)
        | Ty::Bound(_)
        | Ty::Rigid { .. }
        | Ty::Undecided => {}
    }
}

fn discard_semantic_children(types: &mut Vec<Rc<Ty>>, rows: &mut Vec<Rc<Row>>) {
    loop {
        while let Some(row) = rows.pop() {
            if let Ok(mut row) = Rc::try_unwrap(row) {
                take_row_children(&mut row, types, rows);
                // The recursive owners have been removed, so Row::drop sees an
                // empty shell and does constant-depth work.
                drop(row);
            }
        }
        let Some(ty) = types.pop() else { break };
        if let Ok(mut ty) = Rc::try_unwrap(ty) {
            take_ty_children(&mut ty, types, rows);
            // Every recursive owner was replaced above. Its ordinary Drop is
            // therefore constant-depth and can release scalar fields.
            drop(ty);
        }
    }
}

impl Drop for Ty {
    fn drop(&mut self) {
        let mut types = Vec::new();
        let mut rows = Vec::new();
        take_ty_children(self, &mut types, &mut rows);
        discard_semantic_children(&mut types, &mut rows);
    }
}

impl Drop for Row {
    fn drop(&mut self) {
        let mut types = Vec::new();
        let mut rows = Vec::new();
        take_row_children(self, &mut types, &mut rows);
        discard_semantic_children(&mut types, &mut rows);
    }
}

/// Exact equality of finite semantic type syntax.
///
/// Declared names and rigid spellings are diagnostic data, not identities:
/// their symbols and minted ids are. The explicit work list keeps deeply nested
/// alias arguments and [`Rest::More`] chains off the native stack.
pub fn same_finite_syntax(left: &Rc<Ty>, right: &Rc<Ty>) -> bool {
    enum Pair<'a> {
        Ty(&'a Ty, &'a Ty),
        Row(&'a Row, &'a Row),
    }

    fn same_presence(left: &Presence, right: &Presence) -> bool {
        match (left, right) {
            (Presence::Present, Presence::Present)
            | (Presence::Absent, Presence::Absent)
            | (Presence::Undecided, Presence::Undecided) => true,
            (Presence::Var(left), Presence::Var(right))
            | (Presence::Bound(left), Presence::Bound(right))
            | (Presence::Recovered(left), Presence::Recovered(right)) => left == right,
            _ => false,
        }
    }

    let mut pending = vec![Pair::Ty(left, right)];
    let mut seen_types = std::collections::HashSet::new();
    let mut seen_rows = std::collections::HashSet::new();
    while let Some(pair) = pending.pop() {
        match pair {
            Pair::Ty(left, right) => {
                if std::ptr::eq(left, right)
                    || !seen_types.insert((left as *const Ty, right as *const Ty))
                {
                    continue;
                }
                match (left, right) {
                    (Ty::Nat, Ty::Nat)
                    | (Ty::Int, Ty::Int)
                    | (Ty::Real, Ty::Real)
                    | (Ty::String, Ty::String)
                    | (Ty::Boolean, Ty::Boolean)
                    | (Ty::Undecided, Ty::Undecided) => {}
                    (Ty::Var(left), Ty::Var(right)) | (Ty::Bound(left), Ty::Bound(right)) => {
                        if left != right {
                            return false;
                        }
                    }
                    (Ty::Rigid { id: left, .. }, Ty::Rigid { id: right, .. }) => {
                        if left != right {
                            return false;
                        }
                    }
                    (
                        Ty::Arrow(left_from, left_to, left_row),
                        Ty::Arrow(right_from, right_to, right_row),
                    ) => {
                        pending.push(Pair::Row(left_row, right_row));
                        pending.push(Pair::Ty(left_to, right_to));
                        pending.push(Pair::Ty(left_from, right_from));
                    }
                    (Ty::Struct(left), Ty::Struct(right)) | (Ty::Sum(left), Ty::Sum(right)) => {
                        pending.push(Pair::Row(left, right))
                    }
                    (
                        Ty::Named {
                            symbol: left_symbol,
                            args: left_args,
                            ..
                        },
                        Ty::Named {
                            symbol: right_symbol,
                            args: right_args,
                            ..
                        },
                    ) => {
                        if left_symbol != right_symbol || left_args.len() != right_args.len() {
                            return false;
                        }
                        pending.extend(
                            left_args
                                .iter()
                                .zip(right_args.iter())
                                .map(|(left, right)| Pair::Ty(left, right)),
                        );
                    }
                    _ => return false,
                }
            }
            Pair::Row(left, right) => {
                if std::ptr::eq(left, right)
                    || !seen_rows.insert((left as *const Row, right as *const Row))
                {
                    continue;
                }
                if left.labels.len() != right.labels.len() {
                    return false;
                }
                for (name, left_field) in &left.labels {
                    let Some(right_field) = right.labels.get(name) else {
                        return false;
                    };
                    if !same_presence(&left_field.presence, &right_field.presence) {
                        return false;
                    }
                    // An absent label has no payload; recovery is free to leave
                    // any finite type in that semantically unreachable slot.
                    if !matches!(left_field.presence, Presence::Absent) {
                        pending.push(Pair::Ty(&left_field.ty, &right_field.ty));
                    }
                }
                match (&left.rest, &right.rest) {
                    (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => {}
                    (Rest::Var(left), Rest::Var(right))
                    | (Rest::Bound(left), Rest::Bound(right)) => {
                        if left != right {
                            return false;
                        }
                    }
                    (Rest::Rigid { id: left, .. }, Rest::Rigid { id: right, .. }) => {
                        if left != right {
                            return false;
                        }
                    }
                    (Rest::More(left), Rest::More(right)) => {
                        pending.push(Pair::Row(left, right));
                    }
                    _ => return false,
                }
            }
        }
    }
    true
}

impl Ty {
    /// `from -> to`, performing nothing: [`Row::closed`] with no labels, which
    /// is what a bare `A -> B` means and what the printer writes as nothing at
    /// all. Every position that builds an arrow with no effects to put on it
    /// goes through here rather than spelling the empty row again.
    pub fn pure(from: Rc<Ty>, to: Rc<Ty>) -> Self {
        Ty::Arrow(from, to, Row::closed())
    }
}

impl From<Prim> for Ty {
    fn from(value: Prim) -> Self {
        match value {
            Prim::Nat => Ty::Nat,
            Prim::Int => Ty::Int,
            Prim::Real => Ty::Real,
            Prim::String => Ty::String,
            Prim::Boolean => Ty::Boolean,
        }
    }
}

impl Ty {
    /// Compatibility constructor for callers that already have a complete
    /// explicit type value.
    pub fn plain(ty: Ty) -> Self {
        ty
    }

    /// The empty closed struct, also used for unit.
    pub fn unit() -> Self {
        Self::Struct(Row::closed())
    }

    /// The field row inside a struct, if this is one.
    pub fn fields(&self) -> Option<&Row> {
        match self {
            Ty::Struct(row) => Some(row),
            _ => None,
        }
    }

    /// The cases this type allows: the row inside a [`Ty::Sum`].
    /// thing anything still asks a type for a row about.
    ///
    /// Anything else is an argument [`ir::build`](crate::ir::build) already
    /// refused and erased — `Or Nat` for `type Or 'r = #A | ..'r` is the
    /// only way to reach it — so the tail it leaves behind is undecided rather
    /// than closed, which is what an erased argument has always been.
    pub fn cases(&self) -> Row {
        match self {
            Ty::Sum(cases) => cases.clone(),
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

    /// Consume a row into its labels and tail.
    ///
    /// Rows have a custom destructor so a deep [`Rest::More`] chain is released
    /// iteratively. This is the ownership-preserving replacement for moving the
    /// public fields directly out of a row.
    pub fn into_parts(mut self) -> (IndexMap<String, RowField>, Rest) {
        (
            std::mem::take(&mut self.labels),
            std::mem::take(&mut self.rest),
        )
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
    /// of its presences. Every [`Ty::Bound`] and [`Rest::Bound`] in `body`
    /// must be an index below `count`; opening one trusts that.
    ///
    /// Two things are closed this way and the difference is only in who
    /// supplies the values: a definition's scheme binds what generalization
    /// quantified, and instantiation hands each one a fresh variable; a
    /// declaration's binds its parameters, and unfolding hands each one the
    /// argument written at the use site. See [`Ty::Bound`].
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
    pub fn not(mut self) -> Self {
        if matches!(self, Formula::True) {
            return Formula::False;
        }
        if matches!(self, Formula::False) {
            return Formula::True;
        }
        if let Formula::Not(inner) = &mut self {
            let inner = std::mem::replace(inner, Rc::new(Formula::True));
            drop(self);
            return Rc::try_unwrap(inner).unwrap_or_else(|shared| (*shared).clone());
        }
        Formula::Not(Rc::new(self))
    }

    /// Both, with the constants folded away — which is what makes "says
    /// nothing" the value [`Formula::True`] rather than a tree of them.
    pub fn and(self, other: Self) -> Self {
        if matches!(self, Formula::False) {
            drop_formula_iterative(other);
            return Formula::False;
        }
        if matches!(self, Formula::True) {
            return other;
        }
        if matches!(other, Formula::False) {
            drop_formula_iterative(self);
            return Formula::False;
        }
        if matches!(other, Formula::True) {
            return self;
        }
        Formula::And(Rc::new(self), Rc::new(other))
    }

    /// Either, folded the same way.
    pub fn or(self, other: Self) -> Self {
        if matches!(self, Formula::True) {
            drop_formula_iterative(other);
            return Formula::True;
        }
        if matches!(self, Formula::False) {
            return other;
        }
        if matches!(other, Formula::True) {
            drop_formula_iterative(self);
            return Formula::True;
        }
        if matches!(other, Formula::False) {
            return self;
        }
        Formula::Or(Rc::new(self), Rc::new(other))
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
        let mut work = vec![self];
        while let Some(formula) = work.pop() {
            match formula {
                Formula::True | Formula::False => {}
                Formula::Atom(atom) => {
                    if !out.contains(atom) {
                        out.push(*atom);
                    }
                }
                Formula::Not(inner) => work.push(inner),
                Formula::And(left, right)
                | Formula::Or(left, right)
                | Formula::Iff(left, right)
                | Formula::Xor(left, right) => {
                    work.push(right);
                    work.push(left);
                }
            }
        }
    }

    /// Whether this formula holds when each atom is read by `assign`.
    pub fn eval(&self, assign: &dyn Fn(Atom) -> bool) -> bool {
        enum Work<'a> {
            Formula(&'a Formula),
            Not,
            Binary(u8),
        }

        let mut work = vec![Work::Formula(self)];
        let mut values = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Formula(Formula::True) => values.push(true),
                Work::Formula(Formula::False) => values.push(false),
                Work::Formula(Formula::Atom(atom)) => values.push(assign(*atom)),
                Work::Formula(Formula::Not(inner)) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    work.push(Work::Binary(0));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    work.push(Work::Binary(1));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    work.push(Work::Binary(2));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    work.push(Work::Binary(3));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Not => {
                    let value = values.pop().expect("a visited formula value");
                    values.push(!value);
                }
                Work::Binary(kind) => {
                    let right = values.pop().expect("a visited right formula value");
                    let left = values.pop().expect("a visited left formula value");
                    values.push(match kind {
                        0 => left && right,
                        1 => left || right,
                        2 => left == right,
                        _ => left != right,
                    });
                }
            }
        }
        values.pop().expect("every formula has a value")
    }

    /// One formula with each atom replaced by what `of` makes of it, rebuilt
    /// through the constructors so the constants fold on the way out.
    ///
    /// The one walk every substitution over a formula goes through — opening a
    /// scheme's, reading one through what the solve decided, and quantifying
    /// one into a scheme — so there is one place for a connective to be handled
    /// and no way for three copies to disagree about `Iff`.
    pub fn substitute(&self, of: &dyn Fn(Atom) -> Formula) -> Self {
        enum Work<'a> {
            Formula(&'a Formula),
            Not,
            Binary(u8),
        }

        let mut work = vec![Work::Formula(self)];
        let mut values = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Formula(Formula::True) => values.push(Formula::True),
                Work::Formula(Formula::False) => values.push(Formula::False),
                Work::Formula(Formula::Atom(atom)) => values.push(of(*atom)),
                Work::Formula(Formula::Not(inner)) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    work.push(Work::Binary(0));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    work.push(Work::Binary(1));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    work.push(Work::Binary(2));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    work.push(Work::Binary(3));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Not => {
                    let inner = values.pop().expect("a visited formula value");
                    values.push(inner.not());
                }
                Work::Binary(kind) => {
                    let right = values.pop().expect("a visited right formula value");
                    let left = values.pop().expect("a visited left formula value");
                    values.push(match kind {
                        0 => left.and(right),
                        1 => left.or(right),
                        2 => left.iff(right),
                        _ => left.xor(right),
                    });
                }
            }
        }
        values.pop().expect("every formula has a substitution")
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

impl Drop for Formula {
    fn drop(&mut self) {
        fn take_children(formula: &mut Formula, pending: &mut Vec<Rc<Formula>>) {
            match formula {
                Formula::Not(inner) => {
                    pending.push(std::mem::replace(inner, Rc::new(Formula::True)));
                }
                Formula::And(left, right)
                | Formula::Or(left, right)
                | Formula::Iff(left, right)
                | Formula::Xor(left, right) => {
                    pending.push(std::mem::replace(left, Rc::new(Formula::True)));
                    pending.push(std::mem::replace(right, Rc::new(Formula::True)));
                }
                Formula::True | Formula::False | Formula::Atom(_) => {}
            }
        }

        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(formula) = pending.pop() {
            if let Ok(mut formula) = Rc::try_unwrap(formula) {
                take_children(&mut formula, &mut pending);
                drop(formula);
            }
        }
    }
}

/// Release an owned formula without recursively dropping its `Rc` tree.
pub(crate) fn drop_formula_iterative(root: Formula) {
    drop(root);
}

impl Presence {
    /// This presence read as a formula: what a literal about the label says.
    pub fn formula(&self) -> Formula {
        match self {
            Presence::Present => Formula::True,
            Presence::Absent => Formula::False,
            Presence::Var(var) => Formula::var(*var),
            Presence::Bound(index) => Formula::bound(*index),
            // Recovery and an abandoned question impose no requirement; doing
            // otherwise would repeat the original failure in a second place.
            Presence::Recovered(_) | Presence::Undecided => Formula::True,
        }
    }
}

impl Prim {
    /// Every primitive there is. The one place a new one has to be listed, so
    /// that [`from_name`](Self::from_name) can never fall behind the enum.
    pub const ALL: &'static [Prim] = &[
        Prim::Nat,
        Prim::Int,
        Prim::Real,
        Prim::String,
        Prim::Boolean,
    ];

    /// The spelling that denotes this primitive in source. Injective, and
    /// inverted by [`from_name`](Self::from_name), which is what keeps a
    /// printed type re-lowering to the type it was printed from.
    pub const fn name(self) -> &'static str {
        match self {
            Prim::Nat => "Nat",
            Prim::Int => "Int",
            Prim::Real => "Real",
            Prim::String => "String",
            Prim::Boolean => "Boolean",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|prim| prim.name() == name)
    }
}
