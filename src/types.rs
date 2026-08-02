use std::{fmt, rc::Rc};

use indexmap::IndexMap;

use crate::grammar::{Grouped, Prec, write_arrow, write_struct};

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

#[derive(Debug, Clone, Default)]
pub enum Ty {
    Nat,
    Arrow(Rc<Ty>, Rc<Ty>),
    /// A record of named fields, and — with no fields — unit.
    ///
    /// The surface language spells the empty struct two ways, `()` and `{}`,
    /// and both lower here. Unit is not a separate type: it is the struct that
    /// carries no information, which is what unit means anywhere it appears,
    /// and giving it its own variant would buy a second way to say the same
    /// thing plus a unification arm to keep the two agreeing.
    ///
    /// The cost is that the compiler answers in `{}` where the user may have
    /// written `()` — a mismatch against `()` reports ``found `{}` ``, and the
    /// debugger's IR tab shows `{}` beside an AST tab still showing `()`. That
    /// is accepted, not overlooked. Printing the empty struct as `()` would
    /// hide from the reader that unit and the empty record are one type, which
    /// is the thing worth learning about this language; `{}` is real surface
    /// syntax that re-lowers to exactly the type it was printed from, so the
    /// output stays something the user could have written.
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

/// How much of the surface grammar a semantic type can be. Only the arrow
/// extends rightward; everything else — a primitive, a braced struct, a
/// variable — is a form nothing can be appended to.
///
/// The type language has no lambda and no application, so two of [`Prec`]'s
/// four levels never arise here. They are still the right scale to answer on:
/// grouping is decided by comparing against the position a type is being
/// written into, and that comparison is the surface grammar's whether or not
/// this particular tree can reach every level of it.
impl Grouped for Ty {
    fn prec(&self) -> Prec {
        match self {
            Ty::Arrow(..) => Prec::Arrow,
            Ty::Nat | Ty::Struct(_) | Ty::Var(_) | Ty::Bound(_) | Ty::Undecided => Prec::Atom,
        }
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as `'a`, and an unsolved or
/// undecided type as `?` — inference's way of saying it has nothing to report.
///
/// The grouping and the braces come from [`crate::grammar`], which is also what
/// the debugger's two tree printers write through. A type in a diagnostic and
/// the same type on the debugger's IR tab are then the same string by
/// construction rather than by two copies of the rule agreeing.
impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Nat => f.write_str(Prim::Nat.name()),
            Ty::Arrow(from, to) => write_arrow(f, &**from, &**to),
            // Unit falls out of this as `{}` rather than `()`, on purpose:
            // there is one type here, and one spelling for it. See
            // [`Ty::Struct`].
            Ty::Struct(fields) => write_struct(f, fields),
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
