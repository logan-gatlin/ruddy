//! Allocation-backed storage for canonical types and kinds.

use super::{
    Kind, KindId, MetaTypeVariableId, TraitPredicate, Type, TypeBinder, TypeBinderId,
    TypeConstructor, TypeId, TypeKind,
};
use crate::reporting::TextRange;

/// Arena-style storage for types and kinds.
///
/// All [`TypeId`] and [`KindId`] values are indices into this store. Types and
/// kinds are immutable once allocated — unification records solutions for
/// meta-variables in a separate [`UnificationTable`](crate::check::UnificationTable),
/// and zonking produces fresh allocations.
#[derive(Debug)]
pub struct TypeStore {
    /// Append-only arena of canonical type nodes.
    types: Vec<Type>,
    /// Append-only arena of kind nodes.
    kinds: Vec<Kind>,
    /// Cached id of the pre-allocated [`Kind::Type`] node.
    kind_type: KindId,
    /// Cached id of the pre-allocated [`Kind::Row`] node.
    kind_row: KindId,
}

impl TypeStore {
    /// Create an empty store and pre-allocate [`Kind::Type`] and [`Kind::Row`].
    pub fn new() -> Self {
        let mut store = Self {
            types: Vec::new(),
            kinds: Vec::new(),
            kind_type: KindId(0),
            kind_row: KindId(0),
        };
        store.kind_type = store.alloc_kind(Kind::Type);
        store.kind_row = store.alloc_kind(Kind::Row);
        store
    }

    // --- Raw allocation ---

    /// Push a type node into the arena and return its stable id.
    pub fn alloc_type(&mut self, ty: Type) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty);
        id
    }

    /// Push a kind node into the arena and return its stable id.
    pub fn alloc_kind(&mut self, kind: Kind) -> KindId {
        let id = KindId(self.kinds.len() as u32);
        self.kinds.push(kind);
        id
    }

    /// Borrow a type by id.
    ///
    /// Panics if `id` was not allocated by this store.
    pub fn get_type(&self, id: TypeId) -> &Type {
        &self.types[id.0 as usize]
    }

    /// Borrow a kind by id.
    ///
    /// Panics if `id` was not allocated by this store.
    pub fn get_kind(&self, id: KindId) -> &Kind {
        &self.kinds[id.0 as usize]
    }

    /// Total number of allocated type nodes.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// Total number of allocated kind nodes.
    pub fn kind_count(&self) -> usize {
        self.kinds.len()
    }

    // --- Kind constructors ---

    /// The kind `*` (also written `Type`). Pre-allocated at index 0.
    pub fn kind_type(&self) -> KindId {
        self.kind_type
    }

    /// The kind `Row` used by structural record rows.
    pub fn kind_row(&self) -> KindId {
        self.kind_row
    }

    /// Allocate the kind `from -> to`.
    pub fn kind_arrow(&mut self, from: KindId, to: KindId) -> KindId {
        self.alloc_kind(Kind::Arrow(from, to))
    }

    // --- Type constructors ---

    /// Allocate a type constructor with the given kind.
    pub fn mk_constructor(&mut self, ctor: TypeConstructor, kind_id: KindId) -> TypeId {
        self.alloc_type(Type {
            kind: TypeKind::Constructor(ctor),
            kind_id,
            range: TextRange::Generated,
        })
    }

    /// Allocate a type application `func arg` with the given result kind.
    pub fn mk_application(&mut self, func: TypeId, arg: TypeId, result_kind: KindId) -> TypeId {
        self.alloc_type(Type {
            kind: TypeKind::Application(func, arg),
            kind_id: result_kind,
            range: TextRange::Generated,
        })
    }

    /// Build the function type `from -> to` (kind `*`).
    pub fn mk_arrow(&mut self, from: TypeId, to: TypeId) -> TypeId {
        let k = self.kind_type;
        let k1 = self.kind_arrow(k, k);
        let k2 = self.kind_arrow(k, k1);
        let arrow = self.mk_constructor(TypeConstructor::Arrow, k2);
        let partial = self.mk_application(arrow, from, k1);
        self.mk_application(partial, to, k)
    }

    /// Build a structural record type from a row.
    pub fn mk_record(&mut self, row: TypeId) -> TypeId {
        let k_type = self.kind_type;
        self.alloc_type(Type {
            kind: TypeKind::Record(row),
            kind_id: k_type,
            range: TextRange::Generated,
        })
    }

    /// Build the empty row.
    pub fn mk_row_empty(&mut self) -> TypeId {
        let k_row = self.kind_row;
        self.alloc_type(Type {
            kind: TypeKind::RowEmpty,
            kind_id: k_row,
            range: TextRange::Generated,
        })
    }

    /// Build a row-extension node.
    pub fn mk_row_extend(
        &mut self,
        label: impl Into<String>,
        field: TypeId,
        tail: TypeId,
    ) -> TypeId {
        let k_row = self.kind_row;
        self.alloc_type(Type {
            kind: TypeKind::RowExtend {
                label: label.into(),
                field,
                tail,
            },
            kind_id: k_row,
            range: TextRange::Generated,
        })
    }

    /// Allocate an unsolved meta-variable type.
    pub fn mk_meta(&mut self, var: MetaTypeVariableId, kind_id: KindId) -> TypeId {
        self.alloc_type(Type {
            kind: TypeKind::MetaTypeVariable(var),
            kind_id,
            range: TextRange::Generated,
        })
    }

    /// Allocate a rigid (skolem) type variable.
    pub fn mk_rigid(&mut self, binder: TypeBinderId, kind_id: KindId) -> TypeId {
        self.alloc_type(Type {
            kind: TypeKind::RigidTypeVariable(binder),
            kind_id,
            range: TextRange::Generated,
        })
    }

    /// Allocate an error type (kind `*`).
    pub fn mk_error(&mut self) -> TypeId {
        let k = self.kind_type;
        self.alloc_type(Type {
            kind: TypeKind::Error,
            kind_id: k,
            range: TextRange::Generated,
        })
    }

    /// Build a tuple type `(a, b, ...)` from the given element types.
    ///
    /// An empty slice produces the unit type `Tuple(0)`.
    pub fn mk_tuple(&mut self, elements: &[TypeId]) -> TypeId {
        let k = self.kind_type;
        let arity = elements.len();

        // Constructor kind: * -> * -> ... -> * (arity arrows)
        let mut ctor_kind = k;
        for _ in 0..arity {
            ctor_kind = self.kind_arrow(k, ctor_kind);
        }

        let mut ty = self.mk_constructor(TypeConstructor::Tuple(arity), ctor_kind);

        for (i, &elem) in elements.iter().enumerate() {
            let remaining = arity - i - 1;
            let mut result_kind = k;
            for _ in 0..remaining {
                result_kind = self.kind_arrow(k, result_kind);
            }
            ty = self.mk_application(ty, elem, result_kind);
        }

        ty
    }

    /// Build a `∀binders. predicates => body` type (kind `*`).
    pub fn mk_forall(
        &mut self,
        binders: Vec<TypeBinder>,
        predicates: Vec<TraitPredicate>,
        body: TypeId,
    ) -> TypeId {
        let k = self.kind_type;
        self.alloc_type(Type {
            kind: TypeKind::Forall(binders, predicates, body),
            kind_id: k,
            range: TextRange::Generated,
        })
    }
}

impl Default for TypeStore {
    /// Equivalent to [`TypeStore::new`].
    fn default() -> Self {
        Self::new()
    }
}
