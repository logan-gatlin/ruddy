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

/// Which of the two things a row of labels is.
///
/// A row is a set of named things, each either there or not, plus a tail
/// saying what is known about the names it does not list. Both composites in
/// the language are one: the fields every type carries are a row, all of which
/// a value has, and a sum is a row of cases, one of which a value is. The two
/// are unified, flattened, generalized and printed by the same code, and this
/// is the only thing that tells them apart.
///
/// Stored nowhere. A [`Row`] does not say which it is, because where a row sits
/// already does: [`Ty::fields`] is a struct's fields and the row inside
/// [`Core::Sum`] is a sum's cases. So the shape is a *reading* the caller
/// carries down — the way `solve::Rowed` does — and never a second copy of a
/// fact the position already settles.
///
/// Never inferred and never defaulted: which one a row is, is decided by the
/// syntax that wrote it — braces or backticks — and travels with the type from
/// there. Two rows of different shapes are two types, and the solver refuses
/// them the way it refuses a `Nat` against an arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// `{ x: Nat, ..r }` — a value has every field the row says is there.
    Struct,
    /// `` `A Nat | ..r `` — a value is one of the cases the row says is there.
    Sum,
}

/// What one parameter of a `type` declaration stands for, without the labels a
/// row carries.
///
/// The question by itself, so that a complaint about a parameter read two ways
/// can name the two readings without quoting a set of labels nobody asked
/// about. See [`ir::ErrorKind::MixedParameter`](crate::ir::ErrorKind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sense {
    Type,
    Row(Shape),
}

/// What one parameter of a `type` declaration stands for.
///
/// Written nowhere: a parameter is a bare name, and which of these it is
/// follows from where the body uses it — `..r` makes a row, anything else makes
/// a type. Worked out in [`ir::build`](crate::ir::build), and carried here so
/// that the readers who need it — lowering, inference and the debugger — agree.
///
/// Three of them, and no way to write a fourth: a type, the rest of a struct,
/// or the rest of a sum. That is what keeps this a check rather than a
/// language: every parameter is one of these, every declaration takes a fixed
/// list of them, and nothing takes a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind {
    /// Stands for a type. `A` in `type Pair A B`.
    Type,
    /// Stands for the labels a row does not name — and, with them, the labels
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
    /// The shape is carried for the same reason and enforced in the same
    /// place: a struct's tail stands for fields and a sum's for cases, so
    /// `WithX` applied to a sum would splice cases into a struct, and nothing
    /// downstream could make sense of the result. See
    /// [`ir::ErrorKind::NotARow`](crate::ir::ErrorKind).
    ///
    /// A parameter handed straight on to another declaration collects that
    /// declaration's labels too, which is why this is a fixpoint over the
    /// whole table rather than a read of one body. Insertion-ordered, so a
    /// complaint about a row breaking the rule twice always names the same
    /// label first.
    Row {
        shape: Shape,
        lacks: IndexSet<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Scheme {
    count: u32,
    body: Rc<Ty>,
}

/// A type: what it is, and the struct fields it carries.
///
/// Every type has both halves. `Nat` is the `Nat` core with no fields; a struct
/// is the [`Core::Unit`] core with fields; and a `Nat` that carries an `x` is
/// the `Nat` core with an `x`, which the solver can infer and the printer can
/// show even though no source syntax writes one.
///
/// Splitting them this way is what makes "has fields" a property of every type
/// rather than of one shape of type. Two types are equal when their cores are
/// equal and their field rows are equal, which is one rule where there used to
/// be a rule about structs and a complaint for everything else.
#[derive(Debug, Clone, Default)]
pub struct Ty {
    pub core: Core,
    pub fields: Row,
}

/// What a type is, before the fields it carries.
#[derive(Debug, Clone, Default)]
pub enum Core {
    /// A type with nothing of its own. `{}` and `()` are this with no fields,
    /// which is why unit falls out of the printer as `{}` rather than as `()`:
    /// there is one type here and one spelling for it.
    Unit,
    Nat,
    Arrow(Rc<Ty>, Rc<Ty>),
    /// The cases a value may be: a row of labels, each with a presence, and a
    /// tail saying what is known about the cases not named.
    ///
    /// A value *is* one of the cases the row says is there, where the fields
    /// beside this core are labels a value *has*. Everything between those two
    /// sentences — unification, flattening, the lacks condition, generalization
    /// — is written once and reaches both, with [`Shape`] as the only thing
    /// saying which is being read.
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
    Bound(u32),
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
    #[default]
    Undecided,
}

/// A set of named things, each either there or not, and what is known about
/// the names it does not list.
///
/// A type of its own rather than a shape of [`Ty`], and that is what makes the
/// recursion terminate: a closed tail is a [`Rest::Closed`], not a type that
/// would need a tail of its own. What kind of row this is — a struct's fields
/// or a sum's cases — is decided by the position it sits in and carried down by
/// whoever is reading it. See [`Shape`].
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub labels: IndexMap<String, RowField>,
    pub rest: Rest,
}

/// What is known about the labels a [`Row`] does not name.
#[derive(Debug, Clone, Default)]
pub enum Rest {
    /// Every label not named is absent: the row lists all of them.
    #[default]
    Closed,
    Var(TyVar),
    /// A tail a scheme quantified, or one a declaration takes as a row
    /// parameter. A leaf: what it stands for is supplied from outside.
    Bound(u32),
    /// A failure abandoned the question, or a reporter froze it. Absorbs, the
    /// way [`Core::Undecided`] does.
    Undecided,
    /// A tail that has been decided to be more labels, and then whatever is
    /// past them: a row parameter handed a written row, or a tail variable
    /// waiting to be spliced in. Flattened by the one function that resolves a
    /// row, so no reader ever sees the chain.
    More(Rc<Row>),
}

/// One label of a [`Row`]: whether it is there, and what it holds when it is.
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

/// Whether one label of a [`Row`] is there.
///
/// A type of its own, so that "there", "not there" and "still being decided"
/// are the only three answers a field can have and nothing has to say in prose
/// that no other type may be written here.
#[derive(Debug, Clone, Default)]
pub enum Presence {
    Present,
    /// Arises only from solving — no literal and no written type puts one in a
    /// label map — when an open row meets a closed one that lacks the label.
    Absent,
    Var(TyVar),
    /// A presence a scheme quantified, which prints as the `?` on its label.
    Bound(u32),
    /// A failure abandoned the question, or a reporter froze it.
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
    /// What an argument written at this parameter has to be — the shape it has
    /// to have, and the labels it may not name — or `None` when the parameter
    /// does not stand for a row at all. The one question every reader of a kind
    /// actually asks, so it is asked in one place rather than matched out at
    /// each of them.
    pub fn row(&self) -> Option<(Shape, &IndexSet<String>)> {
        match self {
            ParamKind::Type => None,
            ParamKind::Row { shape, lacks } => Some((*shape, lacks)),
        }
    }

    /// What this parameter stands for, with the labels dropped. See [`Sense`].
    pub fn sense(&self) -> Sense {
        match self {
            ParamKind::Type => Sense::Type,
            ParamKind::Row { shape, .. } => Sense::Row(*shape),
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

    /// This value read as a row of `shape`: what a `..` at a row parameter's
    /// position stands for.
    ///
    /// Three ways to arrive, and the middle one is why this is a conversion
    /// rather than a lookup. A row outright is the row. A *type* is what a use
    /// site writes — `Tagged { note: Nat }` hands a struct where a set of
    /// fields goes — so it is read for the row of that shape it carries, which
    /// is a struct's own fields or a sum's cases. And a type that is only a
    /// bare variable is the commonest of the three: instantiating a scheme
    /// mints one variable per quantified position, and a position the scheme
    /// used as a tail wants that variable standing for the rest rather than for
    /// a type with no fields at all.
    ///
    /// A presence cannot reach a tail, for the reason it cannot reach a type
    /// position, and closes the row rather than inventing a rule.
    pub fn as_row(&self, shape: Shape) -> Row {
        match self {
            Assigned::Row(row) => (**row).clone(),
            Assigned::Ty(ty) => match (&ty.core, ty.fields.is_trivial()) {
                (Core::Var(var), true) => Row::of(Rest::Var(*var)),
                _ => ty.row(shape),
            },
            Assigned::Presence(_) => Row::closed(),
        }
    }

    /// This value read as a presence: a presence outright, or the variable a
    /// scheme's fresh one arrives as. Anything else is undecided, for the
    /// reason [`as_ty`](Self::as_ty) gives.
    pub fn as_presence(&self) -> Presence {
        match self {
            Assigned::Presence(presence) => presence.clone(),
            Assigned::Ty(ty) => match (&ty.core, ty.fields.is_trivial()) {
                (Core::Var(var), true) => Presence::Var(*var),
                _ => Presence::Undecided,
            },
            Assigned::Row(_) => Presence::Undecided,
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
            fields: Row::closed(),
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

    /// The row a row parameter written in a position of this shape stands for:
    /// a struct's own fields, a sum's cases.
    ///
    /// Anything else is an argument [`ir::build`](crate::ir::build) already
    /// refused and erased — `WithX Nat` is the only way to reach it — so the
    /// tail it leaves behind is undecided rather than closed, which is what an
    /// erased argument was before the shapes were split.
    pub fn row(&self, shape: Shape) -> Row {
        match (shape, &self.core) {
            (Shape::Struct, Core::Unit) => self.fields.clone(),
            (Shape::Sum, Core::Sum(cases)) => cases.clone(),
            _ => Row::of(Rest::Undecided),
        }
    }
}

impl Row {
    /// A row that names nothing of its own, and then whatever `rest` allows.
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

    /// Whether this row says nothing at all: no labels, and no room for any.
    ///
    /// The test for a type that is only its core, which is what decides whether
    /// a printed type wears a `with`, whether unification has a second row to
    /// decide, and whether a variable is bare enough to take a whole type.
    pub fn is_trivial(&self) -> bool {
        self.labels.is_empty() && matches!(self.rest, Rest::Closed)
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
    /// Close `body` over the variables it binds. Every [`Core::Bound`],
    /// [`Rest::Bound`] and [`Presence::Bound`] in `body` must be an index below
    /// `count`; opening one trusts that.
    ///
    /// Two things are closed this way and the difference is only in who
    /// supplies the values: a definition's scheme binds what generalization
    /// quantified, and instantiation hands each one a fresh variable; a
    /// declaration's binds its parameters, and unfolding hands each one the
    /// argument written at the use site. See [`Core::Bound`].
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
