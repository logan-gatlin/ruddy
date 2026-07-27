/*!
Ruddy's type system has the following features:
* Type inference
* Structural subtyping
* Higher kinded types (fully general type lambdas)
* Trait system (future work)
*/

use std::fmt;

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
    /// The type of `()`, the one value that carries nothing. Written as
    /// punctuation rather than as a name, so no identifier denotes it.
    Unit,
}

impl Prim {
    /// Every primitive there is. The one place a new one has to be listed, so
    /// that [`from_name`](Self::from_name) can never fall behind the enum.
    pub const ALL: &'static [Prim] = &[Prim::Nat, Prim::Unit];

    /// The spelling that denotes this primitive in source. Injective, and
    /// inverted by [`from_name`](Self::from_name), which is what keeps a
    /// printed type re-lowering to the type it was printed from.
    pub const fn name(self) -> &'static str {
        match self {
            Prim::Nat => "Nat",
            Prim::Unit => "()",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|prim| prim.name() == name)
    }
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
