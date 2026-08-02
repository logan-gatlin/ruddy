use std::{fmt, rc::Rc};

use indexmap::IndexMap;

/// A type built into the language rather than declared in it.
///
/// A primitive names nothing the mint could hand out — the same reason
/// [`TermKind::Natural`](crate::ir::TermKind::Natural) carries no symbol — so it
/// is a value here rather than a seeded declaration. Shared by the syntactic
/// [`ir::TypeKind`](crate::ir::TypeKind) and the semantic type language, so the
/// two can never disagree about which primitives exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    /// The type of a natural literal.
    Nat,
}

pub type TyVar = u32;

#[derive(Debug, Clone)]
pub enum Binding {
    Mono(Rc<Ty>),
    Poly(Scheme),
}

#[derive(Debug, Clone)]
pub struct Scheme {
    count: u32,
    body: Rc<Ty>,
}

#[derive(Debug, Clone)]
pub enum Slot {
    Unbound { level: u32 },
    Bound(Rc<Ty>),
}

#[derive(Debug, Clone, Default)]
pub enum Ty {
    Nat,
    Arrow(Rc<Ty>, Rc<Ty>),
    Struct(IndexMap<String, Rc<Ty>>),
    Var(TyVar),
    Bound(u32),
    #[default]
    Undecided,
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as `'a`, and an unsolved or
/// undecided type as `?` — inference's way of saying it has nothing to report.
impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Nat => f.write_str(Prim::Nat.name()),
            // The arrow is right-associative, so only an arrow on the left
            // needs grouping — the same rule the debugger's printers apply.
            Ty::Arrow(from, to) => match **from {
                Ty::Arrow(..) => write!(f, "({from}) -> {to}"),
                _ => write!(f, "{from} -> {to}"),
            },
            Ty::Struct(fields) if fields.is_empty() => f.write_str("{}"),
            Ty::Struct(fields) => {
                f.write_str("{ ")?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                f.write_str(" }")
            }
            // A solver variable has no name, only an index; it is numbered so
            // that two different unknowns in one message stay distinguishable.
            Ty::Var(var) => write!(f, "?{var}"),
            Ty::Bound(index) => {
                let letter = (b'a' + (index % 26) as u8) as char;
                match index / 26 {
                    0 => write!(f, "'{letter}"),
                    round => write!(f, "'{letter}{round}"),
                }
            }
            Ty::Undecided => f.write_str("?"),
        }
    }
}

/// A scheme prints as its body: the quantifier is implied by the `'a`s that
/// appear, the way ML type printers have always done it.
impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.body.fmt(f)
    }
}

impl From<Prim> for Ty {
    fn from(value: Prim) -> Self {
        match value {
            Prim::Nat => Ty::Nat,
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
