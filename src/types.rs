use std::rc::Rc;

use indexmap::IndexMap;

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
    /// [`Ty::Struct`] while a tail the solve bound to a row is waiting to be
    /// spliced in, or a [`Ty::Bound`] once a scheme has quantified it. Never
    /// anything else, for the same unenforced reason as
    /// [`RowField::presence`].
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
    /// it stands for.
    ///
    /// This is the only place a type can be recursive, and the only reason it
    /// can be: `symbol` keys
    /// [`inference::Output::aliases`](crate::inference::Output::aliases), so a
    /// declaration whose body names itself is a lookup that comes back round
    /// rather than a tree that never ends. Nothing here points at the body, so
    /// a type is still a finite tree — which is what keeps `Clone`, `Debug` and
    /// every walk in the compiler terminating.
    ///
    /// It is a barrier to unfolding, never to equality. Two types are the same
    /// when they unfold the same way, whatever they are called, so `symbol` is
    /// compared only to spot the pair that needs no unfolding at all; see
    /// [`Solve::unify`](crate::inference). `name` is what the type prints as
    /// and is never compared: [`Display`](std::fmt::Display) is handed a bare
    /// type with no mint to ask, and a spelling is cheaper to carry than a
    /// context to thread through every diagnostic in the compiler.
    Named {
        symbol: Symbol,
        name: Rc<str>,
    },
    Var(TyVar),
    Bound(u32),
    #[default]
    Undecided,
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
    /// Close `body` over its quantified variables. Every [`Ty::Bound`] in
    /// `body` must be an index below `count`; instantiation trusts that.
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
