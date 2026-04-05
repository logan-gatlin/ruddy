//! Canonical type-system IR used by type checking.
//!
//! Syntactic types from [`crate::lower::ir::TypeExpr`] are lowered into these
//! data structures, then allocated in [`store::TypeStore`]. Nodes refer to one
//! another through compact ids (`TypeId`, `KindId`, and friends) so inference
//! and unification can share structure efficiently.

pub mod store;
pub mod typed_ir;

use crate::lower::ir::QualifiedName;
use crate::reporting::TextRange;

/// Interned handle for a [`Type`] allocated in [`store::TypeStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeId(pub u32);

/// Interned handle for a [`Kind`] allocated in [`store::TypeStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct KindId(pub u32);

/// Stable identifier for a quantified type binder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeBinderId(pub u32);

/// Identifier for an inference meta-variable at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct MetaTypeVariableId(pub u32);

/// Identifier for an inference variable at the kind level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct KindVariableId(pub u32);

/// Metadata for a named, quantified type variable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeBinder {
    /// Stable id used by rigid type variables.
    pub id: TypeBinderId,
    /// Source-facing binder name.
    pub name: String,
    /// Kind assigned to this binder.
    pub kind: KindId,
    /// Source range where the binder was declared.
    pub range: TextRange,
}

/// A canonical type node.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Type {
    /// Structural shape of the type.
    pub kind: TypeKind,
    /// Kind of this type node.
    pub kind_id: KindId,
    /// Source range that produced this type.
    pub range: TextRange,
}

/// Structural variants of canonical types.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeKind {
    /// Rigid variable referencing a quantified binder.
    RigidTypeVariable(TypeBinderId),
    /// Unification meta-variable solved by [`crate::check::UnificationTable`].
    MetaTypeVariable(MetaTypeVariableId),
    /// Saturated or unsaturated type constructor.
    Constructor(TypeConstructor),
    /// Type application (`func arg`), represented explicitly.
    Application(TypeId, TypeId),
    /// Structural record type whose payload is a row.
    Record(TypeId),
    /// Empty row (`{}` at the row level).
    RowEmpty,
    /// Row extension by one labeled field.
    RowExtend {
        /// Field label being introduced.
        label: String,
        /// Type assigned to `label`.
        field: TypeId,
        /// Remaining row tail.
        tail: TypeId,
    },
    /// Explicit polymorphic type: `forall binders. predicates => body`.
    Forall(Vec<TypeBinder>, Vec<TraitPredicate>, TypeId),
    /// Sentinel used after type errors to suppress cascading diagnostics.
    Error,
}

/// Constructors that can appear at the head of a type application.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeConstructor {
    /// User-defined constructor resolved to a fully-qualified name.
    Named(QualifiedName),
    /// Function constructor `(->)`.
    Arrow,
    /// Tuple constructor of a specific arity.
    ///
    /// `Tuple(0)` is unit, also exposed as [`TypeConstructor::UNIT`].
    Tuple(usize),
    /// Array constructor.
    Array,
    /// Built-in boolean type constructor.
    Bool,
    /// Built-in signed integer type constructor.
    Integer,
    /// Built-in natural-number type constructor.
    Natural,
    /// Built-in floating-point/real type constructor.
    Real,
    /// Built-in string type constructor.
    String,
    /// Built-in single-glyph/character type constructor.
    Glyph,
}

impl TypeConstructor {
    /// Convenience alias for unit (`Tuple(0)`).
    pub const UNIT: Self = Self::Tuple(0);
}

/// Kinds classify types in the canonical representation.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Kind {
    /// The kind of inhabited runtime types (`*` / `Type`).
    Type,
    /// The kind of structural record rows.
    Row,
    /// Kind function from one kind to another.
    Arrow(KindId, KindId),
    /// Inference variable at the kind level.
    Variable(KindVariableId),
    /// Error sentinel used after kind failures.
    Error,
}

/// Trait constraint applied to one or more type arguments.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TraitPredicate {
    /// Resolved trait path, or `None` when resolution failed.
    pub trait_ref: Option<QualifiedName>,
    /// Type arguments supplied to the trait.
    pub arguments: Vec<TypeId>,
    /// Source range for this predicate.
    pub range: TextRange,
}

/// Explicitly quantified type with optional trait predicates.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TypeScheme {
    /// Quantified binders introduced by this scheme.
    pub binders: Vec<TypeBinder>,
    /// Constraints required for the body to hold.
    pub predicates: Vec<TraitPredicate>,
    /// Monomorphic body under the quantified binders.
    pub body: TypeId,
    /// Source range that produced this scheme.
    pub range: TextRange,
}

/// Checked form of a type declaration.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TypeDeclaration {
    /// Fully-qualified name of the declared type.
    pub name: QualifiedName,
    /// Type parameters in declaration order.
    pub type_parameters: Vec<TypeBinder>,
    /// Declaration form (alias or nominal).
    pub kind: TypeDeclarationKind,
    /// Source range for the declaration.
    pub range: TextRange,
}

/// Variants of type declarations.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeDeclarationKind {
    /// Transparent alias to another type.
    Alias { value: TypeId },
    /// Nominal declaration with an associated definition shape.
    Nominal { definition: TypeDefinition },
}

/// Definition shape for nominal type declarations.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeDefinition {
    Struct {
        /// Struct members in declaration order.
        members: Vec<RecordTypeMember>,
        /// Source range covering the struct body.
        range: TextRange,
    },
    Sum {
        /// Sum variants in declaration order.
        variants: Vec<SumVariant>,
        /// Source range covering the sum body.
        range: TextRange,
    },
    Opaque {
        /// Internal representation type hidden behind the opaque boundary.
        representation: TypeId,
        /// Source range covering the opaque body.
        range: TextRange,
    },
}

/// Member entry inside a record-like type definition.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct RecordTypeMember {
    /// Optional member name. `None` denotes a positional member.
    name: Option<String>,
    /// Type assigned to this member.
    ty: TypeId,
    /// Source range for this member.
    range: TextRange,
}

/// A single variant in a sum type definition.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct SumVariant {
    /// Optional constructor name; absent when parsing/recovery failed.
    pub name: Option<QualifiedName>,
    /// Optional payload type carried by this variant.
    pub argument: Option<TypeId>,
    /// Source range for the variant declaration.
    pub range: TextRange,
}
