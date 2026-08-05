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
/// Unit is deliberately not one of them. `()` is a spelling of the empty
/// struct, not a type of its own, so there is nothing here for it to name; see
/// [`Ty::Struct`] for why it stays that way even though it means the compiler
/// answers in `{}` where the user wrote `()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    /// The type of a natural literal.
    Nat,
}

pub type TyVar = u32;

/// What one parameter of a `type` declaration stands for.
///
/// Written nowhere: a parameter is a bare name, and which of these it is
/// follows from where the body uses it — `..r` makes a row, anything else makes
/// a type. Worked out in [`ir::build`](crate::ir::build), and carried here so
/// that the readers who need it — lowering, inference and the debugger — agree.
///
/// Two of them, and no way to write a third. That is what keeps this a check
/// rather than a language: every parameter is one of these, every declaration
/// takes a fixed list of them, and nothing takes a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind {
    /// Stands for a type. `A` in `type Pair A B`.
    Type,
    /// Stands for the fields a row does not name — and, with them, the labels
    /// it may therefore not name itself.
    ///
    /// `r` in `type WithX r = { x: Nat, ..r }` is one, and the set is `{x}`:
    /// the tail covers the fields the declaration does not write out, so a `r`
    /// with an `x` of its own would give the type two fields of one name, and
    /// the two copies could disagree. Carrying the set rather than a bare flag
    /// is what lets the condition be said where the argument is written, at
    /// the span the reader can act on, instead of being discovered later by
    /// whatever happened to flatten the row — or never at all.
    ///
    /// A parameter handed straight on to another declaration collects that
    /// declaration's labels too, which is why this is a fixpoint over the
    /// whole table rather than a read of one body. Insertion-ordered, so a
    /// complaint about a row breaking the rule twice always names the same
    /// label first.
    Row(IndexSet<String>),
}

#[derive(Debug, Clone)]
pub struct Scheme {
    count: u32,
    body: Rc<Ty>,
}

/// One field of a [`Ty::Struct`]: whether it is there, and what it is when it
/// is.
///
/// `presence` is one of the presence-shaped types: [`Ty::Present`],
/// [`Ty::Absent`], a [`Ty::Var`] while the solver is still deciding,
/// [`Ty::Undecided`] where a failure abandoned the question or a reporter
/// froze it, or a [`Ty::Bound`] once a scheme has quantified it. Never
/// anything else — never a `Nat`, never an arrow — and nothing checks that,
/// because nothing ever writes anything else into it.
///
/// When it resolves to [`Ty::Absent`], `ty` is meaningless and deliberately
/// left unconstrained: a field that is not there has nothing to have a type.
#[derive(Debug, Clone)]
pub struct RowField {
    pub presence: Rc<Ty>,
    pub ty: Rc<Ty>,
}

#[derive(Debug, Clone, Default)]
pub enum Ty {
    Nat,
    Arrow(Rc<Ty>, Rc<Ty>),
    /// A record of named fields — each with a presence — a tail saying what is
    /// known about the fields not named, and, with no fields and a closed
    /// tail, unit.
    ///
    /// `rest` is one of the row-shaped types: [`Ty::Empty`] for a struct whose
    /// fields are all listed, a [`Ty::Var`] for one that may have more,
    /// [`Ty::Undecided`] where a failure abandoned the question, another
    /// [`Ty::Struct`] while a tail bound to a row — or handed one as an
    /// argument — is waiting to be spliced in, or a [`Ty::Bound`] for a tail a
    /// scheme quantified or a declaration takes as a row parameter.
    ///
    /// Never anything else, and unlike [`RowField::presence`] that is now
    /// checked rather than merely true: a row parameter can be handed anything
    /// a use site cares to write, so `WithX Nat` would put a number here.
    /// [`ir::build`](crate::ir::build) refuses one, which is the only thing
    /// standing between this sentence and a row holding a number.
    Struct {
        fields: IndexMap<String, RowField>,
        rest: Rc<Ty>,
    },
    /// The presence of a field that is there. Not a type a term can have: it
    /// lives only inside [`RowField::presence`], where a variable standing for
    /// a presence resolves to it.
    Present,
    /// The presence of a field that is not there. Arises only from solving —
    /// no literal and no written type puts one in a field map — when an open
    /// row meets a closed one that lacks the field.
    Absent,
    /// The closed row tail: every field the struct does not name is absent.
    /// Like [`Ty::Present`], not a type a term can have; it lives only in
    /// [`Ty::Struct::rest`].
    Empty,
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
    /// refuses a `..` or a `?` in a declaration for exactly that reason — but
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
    /// A leaf either way: whatever it stands for is supplied from outside, so
    /// there is nothing inside it for a walk to find.
    Bound(u32),
    #[default]
    Undecided,
}

impl ParamKind {
    /// The labels an argument written at this parameter may not name, or
    /// `None` when the parameter does not stand for a row at all. The one
    /// question every reader of a kind actually asks, so it is asked in one
    /// place rather than matched out at each of them.
    pub fn lacks(&self) -> Option<&IndexSet<String>> {
        match self {
            ParamKind::Type => None,
            ParamKind::Row(labels) => Some(labels),
        }
    }
}

impl From<Prim> for Ty {
    fn from(value: Prim) -> Self {
        match value {
            Prim::Nat => Ty::Nat,
        }
    }
}

impl RowField {
    /// A field that is definitely there: what a struct literal's fields are,
    /// and what a written `name: Ty` field lowers to.
    pub fn present(ty: Rc<Ty>) -> Self {
        Self {
            presence: Rc::new(Ty::Present),
            ty,
        }
    }
}

impl Scheme {
    /// Close `body` over the variables it binds. Every [`Ty::Bound`] in `body`
    /// must be an index below `count`; opening one trusts that.
    ///
    /// Two things are closed this way and the difference is only in who
    /// supplies the values: a definition's scheme binds what generalization
    /// quantified, and instantiation hands each one a fresh variable; a
    /// declaration's binds its parameters, and unfolding hands each one the
    /// argument written at the use site. See [`Ty::Bound`].
    pub fn new(count: u32, body: Rc<Ty>) -> Self {
        Self { count, body }
    }

    /// How many variables the scheme quantifies. Zero means the type is
    /// monomorphic and instantiation returns the body unchanged.
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn body(&self) -> &Rc<Ty> {
        &self.body
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
