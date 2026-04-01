pub mod store;

use crate::lower::ir::QualifiedName;
use crate::reporting::TextRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct KindId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeBinderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct MetaTypeVariableId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct KindVariableId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeBinder {
    pub id: TypeBinderId,
    pub name: String,
    pub kind: KindId,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Type {
    pub kind: TypeKind,
    pub kind_id: KindId,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeKind {
    RigidTypeVariable(TypeBinderId),
    MetaTypeVariable(MetaTypeVariableId),
    Constructor(TypeConstructor),
    Application(TypeId, TypeId),
    Forall(Vec<TypeBinder>, Vec<TraitPredicate>, TypeId),
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum TypeConstructor {
    Named(QualifiedName),
    Arrow,
    Tuple(usize),
    Array,
    Bool,
    Integer,
    Natural,
    Real,
    String,
    Glyph,
}

impl TypeConstructor {
    pub const UNIT: Self = Self::Tuple(0);
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Kind {
    Type,
    Arrow(KindId, KindId),
    Variable(KindVariableId),
    Error,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TraitPredicate {
    pub trait_ref: Option<QualifiedName>,
    pub arguments: Vec<TypeId>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TypeScheme {
    pub binders: Vec<TypeBinder>,
    pub predicates: Vec<TraitPredicate>,
    pub body: TypeId,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TypeDeclaration {
    pub name: QualifiedName,
    pub type_parameters: Vec<TypeBinder>,
    pub kind: TypeDeclarationKind,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeDeclarationKind {
    Alias { value: TypeId },
    Nominal { definition: TypeDefinition },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeDefinition {
    Struct {
        members: Vec<RecordTypeMember>,
        range: TextRange,
    },
    Sum {
        variants: Vec<SumVariant>,
        range: TextRange,
    },
    Opaque {
        representation: TypeId,
        range: TextRange,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum RecordTypeMember {
    Field {
        name: Option<String>,
        ty: TypeId,
        range: TextRange,
    },
    Spread {
        ty: TypeId,
        range: TextRange,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct SumVariant {
    pub name: Option<QualifiedName>,
    pub argument: Option<TypeId>,
    pub range: TextRange,
}
