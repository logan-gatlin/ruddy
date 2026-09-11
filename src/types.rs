use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

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
    /// The target-sized natural number type.
    Nat,
    /// The target-sized signed integer type.
    Int,
    Fixed(FixedInt),
    /// The type of 64-bit floating-point numbers.
    Real,
    /// The type of UTF-8 text.
    String,
    /// The type with the values true and false.
    Bool,
    /// A value packaged with its structural runtime type.
    Any,
    /// An opaque JavaScript value.
    ForeignValue,
}

/// A target-independent integer width and signedness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FixedInt {
    Nat8,
    Nat16,
    Nat32,
    Nat64,
    Int8,
    Int16,
    Int32,
    Int64,
}

impl FixedInt {
    pub const ALL: [Self; 8] = [
        Self::Nat8,
        Self::Nat16,
        Self::Nat32,
        Self::Nat64,
        Self::Int8,
        Self::Int16,
        Self::Int32,
        Self::Int64,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Nat8 => "Nat8",
            Self::Nat16 => "Nat16",
            Self::Nat32 => "Nat32",
            Self::Nat64 => "Nat64",
            Self::Int8 => "Int8",
            Self::Int16 => "Int16",
            Self::Int32 => "Int32",
            Self::Int64 => "Int64",
        }
    }
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Nat8 => "n8",
            Self::Nat16 => "n16",
            Self::Nat32 => "n32",
            Self::Nat64 => "n64",
            Self::Int8 => "i8",
            Self::Int16 => "i16",
            Self::Int32 => "i32",
            Self::Int64 => "i64",
        }
    }
    pub const fn bits(self) -> u32 {
        match self {
            Self::Nat8 | Self::Int8 => 8,
            Self::Nat16 | Self::Int16 => 16,
            Self::Nat32 | Self::Int32 => 32,
            Self::Nat64 | Self::Int64 => 64,
        }
    }
    pub const fn signed(self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }
    pub const fn min(self) -> i128 {
        if self.signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }
    pub const fn max(self) -> i128 {
        (1i128 << (self.bits() - self.signed() as u32)) - 1
    }
}

/// A range-checked literal. Artifacts validate the same bounds as source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "FixedLiteralData")]
pub struct FixedLiteral {
    kind: FixedInt,
    value: i128,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FixedLiteralData {
    kind: FixedInt,
    value: i128,
}

impl TryFrom<FixedLiteralData> for FixedLiteral {
    type Error = &'static str;
    fn try_from(data: FixedLiteralData) -> Result<Self, Self::Error> {
        Self::new(data.kind, data.value).ok_or("fixed-width literal out of range")
    }
}

impl FixedLiteral {
    pub fn new(kind: FixedInt, value: i128) -> Option<Self> {
        (kind.min()..=kind.max())
            .contains(&value)
            .then_some(Self { kind, value })
    }
    pub fn kind(self) -> FixedInt {
        self.kind
    }
    pub fn value(self) -> i128 {
        self.value
    }
}

impl std::fmt::Display for FixedLiteral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.value, self.kind.suffix())
    }
}

pub type TyVar = u32;

/// Compiler-owned mutation identity, independent of source declarations.
pub fn mutation_effect() -> EffectId {
    EffectId::structural("mut".into(), "0#13:<builtin:mut>;".into())
}

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

    /// The row key, or a provisional spelling for an effect whose identity is
    /// still being computed. What the parameter-kind fixpoint labels a row's
    /// lacks with: before identities exist it needs only to tell labels
    /// apart, and after they do it is the row key itself.
    pub fn label_key(&self) -> String {
        match self {
            Self::Structural { .. } => self.row_key(),
            Self::Pending(symbol) => format!("\u{1e}p{symbol:?}"),
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
    Region,
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
    Region {
        lacks: IndexSet<String>,
    },
    /// Stands for a whole type. `'A` in `type Pair 'A 'B`.
    Type {
        lacks: IndexSet<String>,
    },
    /// Stands for fields a struct does not name. `'r` in
    /// `type WithX 'r = { x: Nat, ..'r }`.
    Fields {
        lacks: IndexSet<String>,
    },
    /// Stands for the cases a sum does not name — and, with them, the cases it
    /// may therefore not name itself. `'r` in `type Or 'r = #A | ..'r`.
    ///
    /// The one reading that is not a type, and the reason it is enforced rather
    /// than substituted: a sum's rest is spliced into [`Ty::Sum`]'s row, so
    /// anything else written there would leave a row holding what no row can
    /// hold. See [`ir::ErrorKind::NotARow`](crate::ir::ErrorKind).
    Cases {
        lacks: IndexSet<String>,
    },
    /// Stands for the effects an arrow does not name — and, with them, the
    /// effects it may therefore not name itself. `'e` in
    /// `type Runner 'e = (Nat -> Nat + ..'e) -> Nat + ..'e`.
    ///
    /// [`ParamKind::Cases`]'s twin, and enforced for the same reason: an effect
    /// row's rest is spliced into the row [`Ty::Arrow`] carries, so anything
    /// else written there would leave a row holding what no row can hold.
    Effects {
        lacks: IndexSet<String>,
    },
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
#[derive(Debug, Clone)]
pub enum Formula {
    /// The constraint that says nothing. A scheme carrying this prints with no
    /// `where` clause at all.
    True,
    /// The constraint nothing satisfies. Never written; what conjoining two
    /// batches that contradict each other comes to.
    False,
    Atom(Atom),
    /// Logically transparent constraint owned by the package at this zero-based
    /// preorder in the enclosing scheme body.
    Owned(u32, Arc<Formula>),
    Not(Arc<Formula>),
    And(Arc<Formula>, Arc<Formula>),
    Or(Arc<Formula>, Arc<Formula>),
    /// `a = b` — both there or neither.
    Iff(Arc<Formula>, Arc<Formula>),
    /// `a != b` — exactly one of them there.
    Xor(Arc<Formula>, Arc<Formula>),
}

impl PartialEq for Formula {
    fn eq(&self, other: &Self) -> bool {
        let mut work = vec![(self, other)];
        while let Some((left, right)) = work.pop() {
            match (left, right) {
                (Formula::True, Formula::True) | (Formula::False, Formula::False) => {}
                (Formula::Atom(left), Formula::Atom(right)) if left == right => {}
                (Formula::Owned(lo, left), Formula::Owned(ro, right)) if lo == ro => {
                    work.push((left, right));
                }
                (Formula::Not(left), Formula::Not(right)) => work.push((left, right)),
                (Formula::And(ll, lr), Formula::And(rl, rr))
                | (Formula::Or(ll, lr), Formula::Or(rl, rr))
                | (Formula::Iff(ll, lr), Formula::Iff(rl, rr))
                | (Formula::Xor(ll, lr), Formula::Xor(rl, rr)) => {
                    work.push((lr, rr));
                    work.push((ll, rl));
                }
                _ => return false,
            }
        }
        true
    }
}

impl Eq for Formula {}

impl std::hash::Hash for Formula {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut work = vec![self];
        while let Some(formula) = work.pop() {
            match formula {
                Formula::True => 0u8.hash(state),
                Formula::False => 1u8.hash(state),
                Formula::Atom(atom) => {
                    2u8.hash(state);
                    atom.hash(state);
                }
                Formula::Owned(owner, inner) => {
                    3u8.hash(state);
                    owner.hash(state);
                    work.push(inner);
                }
                Formula::Not(inner) => {
                    4u8.hash(state);
                    work.push(inner);
                }
                Formula::And(left, right)
                | Formula::Or(left, right)
                | Formula::Iff(left, right)
                | Formula::Xor(left, right) => {
                    let tag = match formula {
                        Formula::And(_, _) => 5u8,
                        Formula::Or(_, _) => 6,
                        Formula::Iff(_, _) => 7,
                        _ => 8,
                    };
                    tag.hash(state);
                    work.push(right);
                    work.push(left);
                }
            }
        }
    }
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
    callable: Option<Arc<crate::reification::interface::Interface>>,
    representations: Vec<u32>,
    count: u32,
    presences: u32,
    /// Presence positions opened as abstract producer-owned identities rather
    /// than flexible caller-chosen variables.
    existentials: IndexSet<u32>,
    body: Arc<Ty>,
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
    Fixed(FixedInt),
    Real,
    String,
    Bool,
    /// A value packaged with its structural runtime type.
    Any,
    /// An opaque JavaScript value.
    ForeignValue,
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
    Arrow(Arc<Ty>, Arc<Ty>, Row),
    /// A producer-owned existential value. The wrapper marks the exact
    /// annotation result boundary at which hidden presence identities are
    /// opened; it is otherwise representation-transparent.
    Package(Arc<Ty>),
    /// `hide 'a => T` — a hidden type: one type variable bound over the body,
    /// standing for a type the value's producer chose and its consumers do
    /// not learn. The bound occurrences inside the body are
    /// [`Ty::HiddenVar`]s naming this binder.
    ///
    /// `binder` is unique per written `hide` across the program, so two
    /// hidden types nested inside one another can never confuse their
    /// variables; alpha-equivalence is what equality decides, so the number
    /// itself is no part of a type's identity. `name` is the spelling, carried
    /// for the reason [`Ty::Rigid`] carries one.
    ///
    /// Representation-transparent, like a package: a value of a hidden type
    /// is stored exactly as its body says.
    Hidden {
        binder: u32,
        name: Arc<str>,
        body: Arc<Ty>,
    },
    /// An occurrence of an enclosing [`Ty::Hidden`]'s variable inside its
    /// body. A leaf; nothing supplies a value for it until the hidden type is
    /// opened, at which point every occurrence becomes one fresh
    /// [`Ty::Rigid`] for the scope that opened it.
    HiddenVar {
        binder: u32,
        name: Arc<str>,
    },
    /// An immutable homogeneous array.
    Array(Arc<Ty>),
    /// `Mirror T` — authentic evidence of the type `T`: opaque, compiler-made,
    /// and invariant in its type, so two mirrors are one type exactly when the
    /// types they mirror are. Its values are the runtime type information the
    /// compiler already passes as hidden evidence, made a first-class value.
    Mirror(Arc<Ty>),
    /// A cell with a fixed region and invariant element type.
    Mut(Arc<Ty>, Arc<Ty>),
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
        name: Arc<str>,
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
        name: Arc<str>,
        /// What the declaration was applied to, in order. Empty for one that
        /// takes nothing, which is every declaration the language had before
        /// type constructors.
        ///
        /// `Arc<[_]>` rather than `Vec`: a type is cloned on nearly every step
        /// the solver takes, and the arguments should not be copied with it.
        args: Arc<[Arc<Ty>]>,
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
        name: Arc<str>,
    },
    /// A failure abandoned the question, or a reporter froze it. Absorbs, the
    /// way [`Ty::Undecided`] does.
    Undecided,
    /// A tail that has been decided to be more cases, and then whatever is
    /// past them: a sum's row parameter handed a written sum, or a tail variable
    /// waiting to be spliced in. Flattened by the one function that resolves a
    /// row, so no reader ever sees the chain.
    More(Arc<Row>),
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
    pub ty: Arc<Ty>,
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
    Ty(Arc<Ty>),
    Row(Arc<Row>),
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
            ParamKind::Region { lacks }
            | ParamKind::Type { lacks }
            | ParamKind::Fields { lacks }
            | ParamKind::Cases { lacks }
            | ParamKind::Effects { lacks } => lacks,
        }
    }

    /// What this parameter stands for, with the labels dropped. See [`Sense`].
    pub fn sense(&self) -> Sense {
        match self {
            ParamKind::Region { .. } => Sense::Region,
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
            ParamKind::Region { .. } | ParamKind::Type { .. } => None,
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
    pub fn as_ty(&self) -> Arc<Ty> {
        match self {
            Assigned::Ty(ty) => ty.clone(),
            Assigned::Row(_) | Assigned::Presence(_) => Arc::new(Ty::Undecided),
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
                // An effects argument: the row an arrow from nothing to
                // nothing carries, which is how one is handed to an effect.
                // See [`Ty::effects_argument`].
                Ty::Arrow(_, _, row) => row.clone(),
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
            Assigned::Ty(_) => Assigned::Ty(Arc::new(Ty::Var(var))),
            Assigned::Row(_) => Assigned::Row(Arc::new(Row::of(Rest::Var(var)))),
            Assigned::Presence(_) => Assigned::Presence(Presence::Var(var)),
        }
    }

    /// The undecided value of this value's sort. What a failure points a
    /// variable at, so that one complaint is not echoed by everything
    /// downstream of it.
    pub fn undecided(&self) -> Self {
        match self {
            Assigned::Ty(_) => Assigned::Ty(Arc::new(Ty::default())),
            Assigned::Row(_) => Assigned::Row(Arc::new(Row::of(Rest::Undecided))),
            Assigned::Presence(_) => Assigned::Presence(Presence::Undecided),
        }
    }
}

fn take_row_children(row: &mut Row, types: &mut Vec<Arc<Ty>>, rows: &mut Vec<Arc<Row>>) {
    types.extend(
        std::mem::take(&mut row.labels)
            .into_values()
            .map(|field| field.ty),
    );
    if let Rest::More(more) = std::mem::take(&mut row.rest) {
        rows.push(more);
    }
}

fn take_ty_children(ty: &mut Ty, types: &mut Vec<Arc<Ty>>, rows: &mut Vec<Arc<Row>>) {
    match ty {
        Ty::Arrow(from, to, effects) => {
            types.push(std::mem::replace(from, Arc::new(Ty::Undecided)));
            types.push(std::mem::replace(to, Arc::new(Ty::Undecided)));
            take_row_children(effects, types, rows);
        }
        Ty::Package(body) | Ty::Hidden { body, .. } => {
            types.push(std::mem::replace(body, Arc::new(Ty::Undecided)));
        }
        Ty::Mut(region, element) => {
            types.push(std::mem::replace(region, Arc::new(Ty::Undecided)));
            types.push(std::mem::replace(element, Arc::new(Ty::Undecided)));
        }
        Ty::Array(element) | Ty::Mirror(element) => {
            types.push(std::mem::replace(element, Arc::new(Ty::Undecided)));
        }
        Ty::Struct(row) | Ty::Sum(row) => take_row_children(row, types, rows),
        Ty::Named { args, .. } => {
            types.extend(std::mem::replace(args, Arc::from([])).iter().cloned());
        }
        Ty::Nat
        | Ty::Int
        | Ty::Fixed(_)
        | Ty::Real
        | Ty::String
        | Ty::Bool
        | Ty::Any
        | Ty::ForeignValue
        | Ty::Var(_)
        | Ty::Bound(_)
        | Ty::Rigid { .. }
        | Ty::HiddenVar { .. }
        | Ty::Undecided => {}
    }
}

fn discard_semantic_children(types: &mut Vec<Arc<Ty>>, rows: &mut Vec<Arc<Row>>) {
    loop {
        while let Some(row) = rows.pop() {
            if let Ok(mut row) = Arc::try_unwrap(row) {
                take_row_children(&mut row, types, rows);
                // The recursive owners have been removed, so Row::drop sees an
                // empty shell and does constant-depth work.
                drop(row);
            }
        }
        let Some(ty) = types.pop() else { break };
        if let Ok(mut ty) = Arc::try_unwrap(ty) {
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
pub fn same_finite_syntax(left: &Arc<Ty>, right: &Arc<Ty>) -> bool {
    let mut unlimited = usize::MAX;
    same_finite_syntax_metered(left, right, &mut unlimited).unwrap_or(false)
}

/// The bounded form of [`same_finite_syntax`]. Each distinct type or row pair
/// inspected consumes one unit; `None` means the caller's work allowance was
/// exhausted before equality was decided.
pub(crate) fn same_finite_syntax_metered(
    left: &Arc<Ty>,
    right: &Arc<Ty>,
    work_left: &mut usize,
) -> Option<bool> {
    /// The hidden binders in scope at a pair, innermost first, pairing the
    /// left side's binder with the right side's: what makes `hide 'a => 'a`
    /// and `hide 'b => 'b` the one type they are.
    struct Scope {
        left: u32,
        right: u32,
        outer: Option<std::rc::Rc<Scope>>,
    }
    type Env = Option<std::rc::Rc<Scope>>;

    fn same_hidden_var(env: &Env, left: u32, right: u32) -> bool {
        let mut scope = env.as_deref();
        while let Some(here) = scope {
            // The innermost binder of either variable decides: a variable
            // bound here is equal exactly to the one bound beside it.
            if here.left == left || here.right == right {
                return here.left == left && here.right == right;
            }
            scope = here.outer.as_deref();
        }
        left == right
    }

    enum Pair<'a> {
        Ty(&'a Ty, &'a Ty, Env),
        Row(&'a Row, &'a Row, Env),
    }

    fn charge(work_left: &mut usize) -> Option<()> {
        *work_left = work_left.checked_sub(1)?;
        Some(())
    }

    fn flatten<'a>(
        mut row: &'a Row,
        work_left: &mut usize,
    ) -> Option<(IndexMap<&'a str, &'a RowField>, &'a Rest)> {
        let mut labels = IndexMap::new();
        loop {
            for (name, field) in &row.labels {
                charge(work_left)?;
                // This is the same outer-label precedence as solver canon.
                labels.entry(name.as_str()).or_insert(field);
            }
            match &row.rest {
                Rest::More(more) => row = more,
                rest => return Some((labels, rest)),
            }
        }
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

    let mut pending = vec![Pair::Ty(left, right, None)];
    let mut seen_types = std::collections::HashSet::new();
    let mut seen_rows = std::collections::HashSet::new();
    let scope_key = |env: &Env| env.as_ref().map_or(std::ptr::null(), std::rc::Rc::as_ptr);
    while let Some(pair) = pending.pop() {
        if *work_left == 0 {
            return None;
        }
        *work_left -= 1;
        match pair {
            Pair::Ty(left, right, env) => {
                // Two bound occurrences are compared through their scope even
                // when one allocation is shared by both sides: the same node
                // under different binders may pair its variables differently.
                if (std::ptr::eq(left, right) && !matches!(left, Ty::HiddenVar { .. }))
                    || !seen_types.insert((left as *const Ty, right as *const Ty, scope_key(&env)))
                {
                    continue;
                }
                match (left, right) {
                    (Ty::Fixed(left), Ty::Fixed(right)) if left == right => {}
                    (Ty::Nat, Ty::Nat)
                    | (Ty::Int, Ty::Int)
                    | (Ty::Real, Ty::Real)
                    | (Ty::String, Ty::String)
                    | (Ty::Bool, Ty::Bool)
                    | (Ty::Any, Ty::Any)
                    | (Ty::ForeignValue, Ty::ForeignValue)
                    | (Ty::Undecided, Ty::Undecided) => {}
                    (Ty::Var(left), Ty::Var(right)) | (Ty::Bound(left), Ty::Bound(right)) => {
                        if left != right {
                            return Some(false);
                        }
                    }
                    (Ty::Rigid { id: left, .. }, Ty::Rigid { id: right, .. }) => {
                        if left != right {
                            return Some(false);
                        }
                    }
                    (Ty::HiddenVar { binder: left, .. }, Ty::HiddenVar { binder: right, .. }) => {
                        if !same_hidden_var(&env, *left, *right) {
                            return Some(false);
                        }
                    }
                    (
                        Ty::Hidden {
                            binder: left_binder,
                            body: left,
                            ..
                        },
                        Ty::Hidden {
                            binder: right_binder,
                            body: right,
                            ..
                        },
                    ) => {
                        let scope = Some(std::rc::Rc::new(Scope {
                            left: *left_binder,
                            right: *right_binder,
                            outer: env,
                        }));
                        pending.push(Pair::Ty(left, right, scope));
                    }
                    (
                        Ty::Arrow(left_from, left_to, left_row),
                        Ty::Arrow(right_from, right_to, right_row),
                    ) => {
                        pending.push(Pair::Row(left_row, right_row, env.clone()));
                        pending.push(Pair::Ty(left_to, right_to, env.clone()));
                        pending.push(Pair::Ty(left_from, right_from, env));
                    }
                    (Ty::Package(left), Ty::Package(right)) => {
                        pending.push(Pair::Ty(left, right, env))
                    }
                    (Ty::Mut(a, b), Ty::Mut(c, d)) => {
                        pending.push(Pair::Ty(a, c, env.clone()));
                        pending.push(Pair::Ty(b, d, env));
                    }
                    (Ty::Array(left), Ty::Array(right)) | (Ty::Mirror(left), Ty::Mirror(right)) => {
                        pending.push(Pair::Ty(left, right, env))
                    }
                    (Ty::Struct(left), Ty::Struct(right)) | (Ty::Sum(left), Ty::Sum(right)) => {
                        pending.push(Pair::Row(left, right, env))
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
                            return Some(false);
                        }
                        pending.extend(
                            left_args
                                .iter()
                                .zip(right_args.iter())
                                .map(|(left, right)| Pair::Ty(left, right, env.clone())),
                        );
                    }
                    _ => return Some(false),
                }
            }
            Pair::Row(left, right, env) => {
                if !seen_rows.insert((left as *const Row, right as *const Row, scope_key(&env))) {
                    continue;
                }
                let (left_labels, left_rest) = flatten(left, work_left)?;
                let (right_labels, right_rest) = flatten(right, work_left)?;
                if left_labels.len() != right_labels.len() {
                    return Some(false);
                }
                for (name, left_field) in left_labels {
                    charge(work_left)?;
                    let Some(right_field) = right_labels.get(name) else {
                        return Some(false);
                    };
                    if !same_presence(&left_field.presence, &right_field.presence) {
                        return Some(false);
                    }
                    // Present and variable-presence labels carry payloads the
                    // program can reach — lowering switches on them — so those
                    // payloads are part of the syntax. Absent, undecided, and
                    // recovered slots may retain arbitrary recovery types
                    // which are not.
                    if matches!(
                        left_field.presence,
                        Presence::Present | Presence::Var(_) | Presence::Bound(_)
                    ) {
                        pending.push(Pair::Ty(&left_field.ty, &right_field.ty, env.clone()));
                    }
                }
                match (left_rest, right_rest) {
                    (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => {}
                    (Rest::Var(left), Rest::Var(right))
                    | (Rest::Bound(left), Rest::Bound(right)) => {
                        if left != right {
                            return Some(false);
                        }
                    }
                    (Rest::Rigid { id: left, .. }, Rest::Rigid { id: right, .. }) => {
                        if left != right {
                            return Some(false);
                        }
                    }
                    _ => return Some(false),
                }
            }
        }
    }
    Some(true)
}

/// The body of a hidden type with every occurrence of its variable replaced:
/// what opening `hide 'a => T` at a type `X` comes to. Occurrences under a
/// nested `hide` that binds the same number belong to that inner binder and
/// are left alone, so a hidden type nested inside its own unfolding still
/// means what it says.
///
/// Iterative, like every walk over a type: a hidden body may hold deeply
/// nested imported types, and no native frame is spent per constructor.
pub fn open_hidden(body: &Arc<Ty>, binder: u32, replacement: &Arc<Ty>) -> Arc<Ty> {
    enum Work<'a> {
        Ty(&'a Arc<Ty>),
        Row(&'a Row),
        Arrow,
        Package,
        Hidden(u32, Arc<str>),
        Shadow,
        Unshadow,
        Array,
        Mirror,
        Mut,
        Struct,
        Sum,
        Named {
            symbol: Symbol,
            name: Arc<str>,
            args: usize,
        },
        BuiltRow(&'a Row, usize),
    }

    let mut work = vec![Work::Ty(body)];
    let mut types: Vec<Arc<Ty>> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    // How many binders of the same number enclose the current position: an
    // occurrence under one is the inner binder's, not the one being opened.
    let mut shadowed = 0usize;
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &**ty {
                Ty::HiddenVar { binder: found, .. } if *found == binder && shadowed == 0 => {
                    types.push(replacement.clone());
                }
                Ty::Hidden {
                    binder: inner,
                    name,
                    body,
                } => {
                    work.push(Work::Hidden(*inner, name.clone()));
                    if *inner == binder {
                        work.push(Work::Unshadow);
                        work.push(Work::Ty(body));
                        work.push(Work::Shadow);
                    } else {
                        work.push(Work::Ty(body));
                    }
                }
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects));
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                Ty::Package(inner) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(inner));
                }
                Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element));
                }
                Ty::Mirror(element) => {
                    work.push(Work::Mirror);
                    work.push(Work::Ty(element));
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Mut);
                    work.push(Work::Ty(element));
                    work.push(Work::Ty(region));
                }
                Ty::Struct(row) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(row));
                }
                Ty::Sum(row) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(row));
                }
                Ty::Named { symbol, name, args } => {
                    work.push(Work::Named {
                        symbol: *symbol,
                        name: name.clone(),
                        args: args.len(),
                    });
                    work.extend(args.iter().rev().map(Work::Ty));
                }
                Ty::Nat
                | Ty::Int
                | Ty::Fixed(_)
                | Ty::Real
                | Ty::String
                | Ty::Bool
                | Ty::Any
                | Ty::ForeignValue
                | Ty::Var(_)
                | Ty::Bound(_)
                | Ty::Rigid { .. }
                | Ty::HiddenVar { .. }
                | Ty::Undecided => types.push(ty.clone()),
            },
            Work::Shadow => shadowed += 1,
            Work::Unshadow => shadowed -= 1,
            Work::Row(row) => {
                let mut nested = 0;
                if let Rest::More(more) = &row.rest {
                    work.push(Work::BuiltRow(row, 1));
                    work.push(Work::Row(more));
                    nested = 1;
                }
                if nested == 0 {
                    work.push(Work::BuiltRow(row, 0));
                }
                work.extend(row.labels.values().rev().map(|field| Work::Ty(&field.ty)));
            }
            Work::Arrow => {
                let effects = rows.pop().expect("row postorder");
                let to = types.pop().expect("result postorder");
                let from = types.pop().expect("argument postorder");
                types.push(Arc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("package postorder");
                types.push(Arc::new(Ty::Package(body)));
            }
            Work::Hidden(binder, name) => {
                let body = types.pop().expect("hidden postorder");
                types.push(Arc::new(Ty::Hidden { binder, name, body }));
            }
            Work::Array => {
                let element = types.pop().expect("array postorder");
                types.push(Arc::new(Ty::Array(element)));
            }
            Work::Mirror => {
                let element = types.pop().expect("mirror postorder");
                types.push(Arc::new(Ty::Mirror(element)));
            }
            Work::Mut => {
                let element = types.pop().expect("cell element");
                let region = types.pop().expect("cell region");
                types.push(Arc::new(Ty::Mut(region, element)));
            }
            Work::Struct => {
                let row = rows.pop().expect("struct postorder");
                types.push(Arc::new(Ty::Struct(row)));
            }
            Work::Sum => {
                let row = rows.pop().expect("sum postorder");
                types.push(Arc::new(Ty::Sum(row)));
            }
            Work::Named { symbol, name, args } => {
                let mut opened = Vec::with_capacity(args);
                for _ in 0..args {
                    opened.push(types.pop().expect("named postorder"));
                }
                opened.reverse();
                types.push(Arc::new(Ty::Named {
                    symbol,
                    name,
                    args: opened.into(),
                }));
            }
            Work::BuiltRow(row, nested) => {
                let rest = match nested {
                    0 => row.rest.clone(),
                    _ => Rest::More(Arc::new(rows.pop().expect("nested row postorder"))),
                };
                let mut labels = Vec::with_capacity(row.labels.len());
                for (name, field) in row.labels.iter().rev() {
                    let ty = types.pop().expect("field postorder");
                    labels.push((
                        name.clone(),
                        RowField {
                            presence: field.presence.clone(),
                            ty,
                        },
                    ));
                }
                labels.reverse();
                rows.push(Row {
                    labels: labels.into_iter().collect(),
                    rest,
                });
            }
        }
    }
    types.pop().expect("an opened type")
}

impl Ty {
    /// `from -> to`, performing nothing: [`Row::closed`] with no labels, which
    /// is what a bare `A -> B` means and what the printer writes as nothing at
    /// all. Every position that builds an arrow with no effects to put on it
    /// goes through here rather than spelling the empty row again.
    pub fn pure(from: Arc<Ty>, to: Arc<Ty>) -> Self {
        Ty::Arrow(from, to, Row::closed())
    }
}

impl From<Prim> for Ty {
    fn from(value: Prim) -> Self {
        match value {
            Prim::Nat => Ty::Nat,
            Prim::Int => Ty::Int,
            Prim::Fixed(kind) => Ty::Fixed(kind),
            Prim::Real => Ty::Real,
            Prim::String => Ty::String,
            Prim::Bool => Ty::Bool,
            Prim::Any => Ty::Any,
            Prim::ForeignValue => Ty::ForeignValue,
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

    /// A row of effects handed to an effect's parameter, as the type it
    /// travels as: an arrow from nothing to nothing carrying the row. The
    /// arrow is what says the row is effects — a struct's row would be
    /// fields and a sum's cases — so two such arguments unify as effect rows,
    /// are forbidden labels in effect nouns, and print as a row of effects.
    pub fn effects_argument(row: Row) -> Self {
        Self::Arrow(Arc::new(Self::unit()), Arc::new(Self::unit()), row)
    }

    /// The row this is an effects argument of, if it is one. See
    /// [`Ty::effects_argument`].
    pub fn effects_argument_row(&self) -> Option<&Row> {
        match self {
            Ty::Arrow(from, to, row)
                if matches!(&**from, Ty::Struct(row) if row.labels.is_empty() && matches!(row.rest, Rest::Closed))
                    && matches!(&**to, Ty::Struct(row) if row.labels.is_empty() && matches!(row.rest, Rest::Closed)) =>
            {
                Some(row)
            }
            _ => None,
        }
    }

    /// The element type inside an array, if this is one.
    pub fn element(&self) -> Option<&Arc<Ty>> {
        let mut ty = self;
        while let Ty::Package(body) = ty {
            ty = body;
        }
        match ty {
            Ty::Array(element) => Some(element),
            _ => None,
        }
    }

    /// The field row inside a struct, if this is one.
    pub fn fields(&self) -> Option<&Row> {
        let mut ty = self;
        while let Ty::Package(body) = ty {
            ty = body;
        }
        match ty {
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
        let mut ty = self;
        while let Ty::Package(body) = ty {
            ty = body;
        }
        match ty {
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
    pub fn present(ty: Arc<Ty>) -> Self {
        Self {
            presence: Presence::Present,
            ty,
        }
    }
}

impl Scheme {
    pub fn callable(&self) -> Option<&crate::reification::interface::Interface> {
        self.callable.as_deref()
    }

    pub fn with_callable(
        mut self,
        callable: Option<crate::reification::interface::Interface>,
    ) -> Self {
        self.callable = callable.map(Arc::new);
        self
    }

    /// Quantified type positions supplied as hidden runtime representations.
    pub fn representations(&self) -> &[u32] {
        &self.representations
    }

    pub fn with_representations(mut self, mut parameters: Vec<u32>) -> Self {
        parameters.sort_unstable();
        parameters.dedup();
        self.representations = parameters;
        self
    }

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
    pub fn new(count: u32, body: Arc<Ty>) -> Self {
        Self {
            callable: None,
            representations: Vec::new(),
            count,
            presences: 0,
            existentials: IndexSet::new(),
            body,
            formula: Formula::True,
        }
    }

    /// [`new`](Self::new) with the presences a definition's generalization
    /// quantified, and what it requires of them.
    pub fn constrained(count: u32, presences: u32, body: Arc<Ty>, formula: Formula) -> Self {
        debug_assert!(presences <= count);
        Self {
            callable: None,
            representations: Vec::new(),
            count,
            presences,
            existentials: IndexSet::new(),
            body,
            formula,
        }
    }

    /// Construct a constrained scheme with producer-owned presence positions.
    pub fn existential(
        count: u32,
        presences: u32,
        existentials: IndexSet<u32>,
        body: Arc<Ty>,
        formula: Formula,
    ) -> Self {
        debug_assert!(presences <= count);
        debug_assert!(existentials.iter().all(|index| *index < presences));
        // Opening a package for a lexical alias deliberately gives that alias
        // a fresh, unrelated view (R16). Generalizing the opened view can leave
        // its existential slots structurally bare; reseal those slots at the
        // alias boundary rather than retaining the source package's identity.
        let body = match existential_outside_package(&body, &existentials) {
            true => Arc::new(Ty::Package(body)),
            false => body,
        };
        let formula = partition_package_formula(&body, &existentials, formula);
        Self {
            callable: None,
            representations: Vec::new(),
            count,
            presences,
            existentials,
            body,
            formula,
        }
    }

    pub fn is_existential(&self, index: u32) -> bool {
        self.existentials.contains(&index)
    }

    pub fn existentials(&self) -> &IndexSet<u32> {
        &self.existentials
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

    pub fn body(&self) -> &Arc<Ty> {
        &self.body
    }

    /// What has to hold of the presences this scheme quantifies.
    pub fn formula(&self) -> &Formula {
        &self.formula
    }
}

/// Whether an existential bound occurrence has not yet been resealed by a
/// package. The walk includes composed row tails because imported and inferred
/// rows may retain their finite shape in `Rest::More`.
fn existential_outside_package(body: &Arc<Ty>, existentials: &IndexSet<u32>) -> bool {
    enum Work {
        Ty(Arc<Ty>, bool),
        Row(Row, bool),
    }
    let mut work = vec![Work::Ty(body.clone(), false)];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty, packaged) => match &*ty {
                Ty::Package(inner) => work.push(Work::Ty(inner.clone(), true)),
                Ty::Hidden { body, .. } => work.push(Work::Ty(body.clone(), packaged)),
                Ty::Array(element) | Ty::Mirror(element) => {
                    work.push(Work::Ty(element.clone(), packaged))
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Ty(element.clone(), packaged));
                    work.push(Work::Ty(region.clone(), packaged));
                }
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Row(effects.clone(), packaged));
                    work.push(Work::Ty(to.clone(), packaged));
                    work.push(Work::Ty(from.clone(), packaged));
                }
                Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone(), packaged)),
                Ty::Named { args, .. } => {
                    work.extend(args.iter().rev().cloned().map(|ty| Work::Ty(ty, packaged)))
                }
                _ => {}
            },
            Work::Row(row, packaged) => {
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row((**more).clone(), packaged));
                }
                for field in row.labels.values().rev() {
                    if !packaged
                        && matches!(field.presence, Presence::Bound(index) if existentials.contains(&index))
                    {
                        return true;
                    }
                    work.push(Work::Ty(field.ty.clone(), packaged));
                }
            }
        }
    }
    false
}

/// Partition independent conjuncts by the exact package which owns all their
/// existential atoms. Package numbers are stable structural preorder; a
/// conjunct involving a universal or multiple owners stays scheme-wide.
fn partition_package_formula(
    body: &Arc<Ty>,
    existentials: &IndexSet<u32>,
    formula: Formula,
) -> Formula {
    enum Work {
        Ty(Arc<Ty>, Option<u32>),
        Row(Row, Option<u32>),
    }
    let mut slot_owners = HashMap::new();
    let mut package_count = 0u32;
    let mut work = vec![Work::Ty(body.clone(), None)];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty, owner) => match &*ty {
                Ty::Package(inner) => {
                    let here = package_count;
                    package_count += 1;
                    work.push(Work::Ty(inner.clone(), Some(here)));
                }
                Ty::Hidden { body, .. } => work.push(Work::Ty(body.clone(), owner)),
                Ty::Array(element) | Ty::Mirror(element) => {
                    work.push(Work::Ty(element.clone(), owner))
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Ty(element.clone(), owner));
                    work.push(Work::Ty(region.clone(), owner));
                }
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Row(effects.clone(), owner));
                    work.push(Work::Ty(to.clone(), owner));
                    work.push(Work::Ty(from.clone(), owner));
                }
                Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone(), owner)),
                Ty::Named { args, .. } => {
                    work.extend(args.iter().rev().cloned().map(|ty| Work::Ty(ty, owner)))
                }
                _ => {}
            },
            Work::Row(row, owner) => {
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row((**more).clone(), owner));
                }
                for field in row.labels.values().rev() {
                    if let Presence::Bound(index) = field.presence
                        && existentials.contains(&index)
                    {
                        let owner = owner.expect("an existential slot has a package owner");
                        if let Some(before) = slot_owners.insert(index, owner) {
                            debug_assert_eq!(
                                before, owner,
                                "existential slot crossed package owners"
                            );
                        }
                    }
                    work.push(Work::Ty(field.ty.clone(), owner));
                }
            }
        }
    }

    debug_assert!(
        existentials
            .iter()
            .all(|index| slot_owners.contains_key(index)),
        "every existential slot occurs in its package"
    );

    fn owned(part: Formula, owners: &HashMap<u32, u32>, existentials: &IndexSet<u32>) -> Formula {
        let mut atoms = Vec::new();
        part.atoms(&mut atoms);
        let mut selected = None;
        for atom in atoms {
            let Atom::Bound(index) = atom else {
                // A nested generalized scheme can retain a presence from its
                // enclosing closure as a free solver variable. Like a bound
                // universal input, it scopes the proposition but does not
                // choose which result package owns the fresh witness.
                continue;
            };
            // A conjunct that relates a caller-owned input to a hidden result
            // is still a guarantee of that result package. Keep the universal
            // atom in the clause, but select ownership from its existential
            // atoms so opening the package can rename only the fresh witness.
            if !existentials.contains(&index) {
                continue;
            }
            let Some(owner) = owners.get(&index).copied() else {
                return part;
            };
            if selected.is_some_and(|before| before != owner) {
                return part;
            }
            selected = Some(owner);
        }
        match selected {
            Some(owner) => Formula::owned(owner, part),
            None => part,
        }
    }

    let mut pending = vec![formula];
    let mut parts = Vec::new();
    while let Some(part) = pending.pop() {
        match &part {
            Formula::Owned(_, inner) => pending.push((**inner).clone()),
            Formula::And(left, right) => {
                pending.push((**right).clone());
                pending.push((**left).clone());
            }
            _ => parts.push(owned(part, &slot_owners, existentials)),
        }
    }
    Formula::all(parts)
}

impl Formula {
    /// Mark a constraint with its exact package owner. Ownership is metadata;
    /// propositional operations deliberately treat this wrapper transparently.
    pub fn owned(owner: u32, formula: Formula) -> Self {
        Formula::Owned(owner, Arc::new(formula))
    }

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
            let inner = std::mem::replace(inner, Arc::new(Formula::True));
            drop(self);
            return Arc::try_unwrap(inner).unwrap_or_else(|shared| (*shared).clone());
        }
        Formula::Not(Arc::new(self))
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
        Formula::And(Arc::new(self), Arc::new(other))
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
        Formula::Or(Arc::new(self), Arc::new(other))
    }

    /// Both or neither: what `a = b` says.
    pub fn iff(self, other: Self) -> Self {
        Formula::Iff(Arc::new(self), Arc::new(other))
    }

    /// Exactly one: what `a != b` says.
    pub fn xor(self, other: Self) -> Self {
        Formula::Xor(Arc::new(self), Arc::new(other))
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
        let mut seen: HashSet<_> = out.iter().copied().collect();
        let mut work = vec![self];
        while let Some(formula) = work.pop() {
            match formula {
                Formula::True | Formula::False => {}
                Formula::Atom(atom) => {
                    if seen.insert(*atom) {
                        out.push(*atom);
                    }
                }
                Formula::Owned(_, inner) | Formula::Not(inner) => work.push(inner),
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
                Work::Formula(Formula::Owned(_, inner)) => work.push(Work::Formula(inner)),
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
            Owned(u32),
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
                Work::Formula(Formula::Owned(owner, inner)) => {
                    work.push(Work::Owned(*owner));
                    work.push(Work::Formula(inner));
                }
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
                Work::Owned(owner) => {
                    let inner = values.pop().expect("a visited owned formula value");
                    values.push(Formula::owned(owner, inner));
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
        fn take_children(formula: &mut Formula, pending: &mut Vec<Arc<Formula>>) {
            match formula {
                Formula::Owned(_, inner) | Formula::Not(inner) => {
                    pending.push(std::mem::replace(inner, Arc::new(Formula::True)));
                }
                Formula::And(left, right)
                | Formula::Or(left, right)
                | Formula::Iff(left, right)
                | Formula::Xor(left, right) => {
                    pending.push(std::mem::replace(left, Arc::new(Formula::True)));
                    pending.push(std::mem::replace(right, Arc::new(Formula::True)));
                }
                Formula::True | Formula::False | Formula::Atom(_) => {}
            }
        }

        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(formula) = pending.pop() {
            if let Ok(mut formula) = Arc::try_unwrap(formula) {
                take_children(&mut formula, &mut pending);
                drop(formula);
            }
        }
    }
}

/// Release an owned formula without recursively dropping its `Arc` tree.
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
        Prim::Fixed(FixedInt::Nat8),
        Prim::Fixed(FixedInt::Nat16),
        Prim::Fixed(FixedInt::Nat32),
        Prim::Fixed(FixedInt::Nat64),
        Prim::Fixed(FixedInt::Int8),
        Prim::Fixed(FixedInt::Int16),
        Prim::Fixed(FixedInt::Int32),
        Prim::Fixed(FixedInt::Int64),
        Prim::Real,
        Prim::String,
        Prim::Bool,
        Prim::Any,
        Prim::ForeignValue,
    ];

    /// The spelling that denotes this primitive in source. Injective, and
    /// inverted by [`from_name`](Self::from_name), which is what keeps a
    /// printed type re-lowering to the type it was printed from.
    pub const fn name(self) -> &'static str {
        match self {
            Prim::Nat => "Nat",
            Prim::Int => "Int",
            Prim::Fixed(kind) => kind.name(),
            Prim::Real => "Real",
            Prim::String => "String",
            Prim::Bool => "Bool",
            Prim::Any => "Any",
            Prim::ForeignValue => "ForeignValue",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|prim| prim.name() == name)
    }
}
