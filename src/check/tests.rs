use crate::ty::store::TypeStore;
use crate::ty::*;

use super::{UnificationError, UnificationTable};

// ---------------------------------------------------------------------------
// TypeStore tests
// ---------------------------------------------------------------------------

#[test]
fn kind_type_preallocated() {
    let store = TypeStore::new();
    let k = store.kind_type();
    assert_eq!(*store.get_kind(k), Kind::Type);
    // Should be the very first kind allocated.
    assert_eq!(k, KindId(0));
}

#[test]
fn alloc_and_get_type() {
    let mut store = TypeStore::new();
    let k = store.kind_type();
    let id = store.mk_constructor(TypeConstructor::Integer, k);
    let ty = store.get_type(id);
    assert_eq!(ty.kind, TypeKind::Constructor(TypeConstructor::Integer));
    assert_eq!(ty.kind_id, k);
}

#[test]
fn mk_arrow_structure() {
    let mut store = TypeStore::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let bool_ty = store.mk_constructor(TypeConstructor::Bool, k);

    // int -> bool
    let arrow = store.mk_arrow(int, bool_ty);
    let arrow_ty = store.get_type(arrow);

    // Result kind should be Type.
    assert_eq!(arrow_ty.kind_id, k);

    // Top level is Application(Application(Arrow, Int), Bool).
    let TypeKind::Application(partial, rhs) = &arrow_ty.kind else {
        panic!("expected Application, got {:?}", arrow_ty.kind);
    };
    assert_eq!(*rhs, bool_ty);

    let partial_ty = store.get_type(*partial);
    let TypeKind::Application(ctor, lhs) = &partial_ty.kind else {
        panic!("expected Application, got {:?}", partial_ty.kind);
    };
    assert_eq!(*lhs, int);

    let ctor_ty = store.get_type(*ctor);
    assert_eq!(ctor_ty.kind, TypeKind::Constructor(TypeConstructor::Arrow));
}

#[test]
fn mk_tuple_empty_is_unit() {
    let mut store = TypeStore::new();
    let unit = store.mk_tuple(&[]);
    let ty = store.get_type(unit);

    assert_eq!(ty.kind, TypeKind::Constructor(TypeConstructor::UNIT));
    assert_eq!(ty.kind_id, store.kind_type());
}

#[test]
fn mk_tuple_pair() {
    let mut store = TypeStore::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let bool_ty = store.mk_constructor(TypeConstructor::Bool, k);

    let pair = store.mk_tuple(&[int, bool_ty]);
    let pair_ty = store.get_type(pair);

    // Result kind is Type.
    assert_eq!(pair_ty.kind_id, k);

    // Structure: Application(Application(Tuple(2), Int), Bool)
    let TypeKind::Application(partial, rhs) = &pair_ty.kind else {
        panic!("expected Application");
    };
    assert_eq!(*rhs, bool_ty);

    let partial_ty = store.get_type(*partial);
    let TypeKind::Application(ctor, lhs) = &partial_ty.kind else {
        panic!("expected Application");
    };
    assert_eq!(*lhs, int);

    let ctor_ty = store.get_type(*ctor);
    assert_eq!(
        ctor_ty.kind,
        TypeKind::Constructor(TypeConstructor::Tuple(2))
    );
}

#[test]
fn mk_error_has_kind_type() {
    let mut store = TypeStore::new();
    let err = store.mk_error();
    let ty = store.get_type(err);
    assert_eq!(ty.kind, TypeKind::Error);
    assert_eq!(ty.kind_id, store.kind_type());
}

// ---------------------------------------------------------------------------
// Unification — basic type unification
// ---------------------------------------------------------------------------

#[test]
fn unify_same_type() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);

    assert!(table.unify_types(&store, int, int).is_ok());
}

#[test]
fn unify_meta_with_constructor() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (var, meta) = table.fresh_type_var(&mut store, k);

    assert!(table.unify_types(&store, meta, int).is_ok());
    assert_eq!(table.probe_type_var(var), Some(int));
}

#[test]
fn unify_constructor_with_meta() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (var, meta) = table.fresh_type_var(&mut store, k);

    // Reversed order compared to the test above.
    assert!(table.unify_types(&store, int, meta).is_ok());
    assert_eq!(table.probe_type_var(var), Some(int));
}

#[test]
fn unify_two_metas() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var_a, meta_a) = table.fresh_type_var(&mut store, k);
    let (_var_b, meta_b) = table.fresh_type_var(&mut store, k);

    assert!(table.unify_types(&store, meta_a, meta_b).is_ok());
    // One of them should point to the other.
    assert_eq!(table.probe_type_var(var_a), Some(meta_b));
}

#[test]
fn unify_application_pairwise() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let bool_ty = store.mk_constructor(TypeConstructor::Bool, k);
    let (var, meta) = table.fresh_type_var(&mut store, k);

    // Build: Int -> ?a   and   Int -> Bool
    let arrow_a = store.mk_arrow(int, meta);
    let arrow_b = store.mk_arrow(int, bool_ty);

    assert!(table.unify_types(&store, arrow_a, arrow_b).is_ok());
    // ?a should be solved to Bool.
    assert_eq!(table.probe_type_var(var), Some(bool_ty));
}

#[test]
fn unify_mismatch_different_constructors() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let bool_ty = store.mk_constructor(TypeConstructor::Bool, k);

    let result = table.unify_types(&store, int, bool_ty);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
}

#[test]
fn unify_mismatch_arrow_vs_constructor() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let arrow = store.mk_arrow(int, int);

    let result = table.unify_types(&store, arrow, int);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
}

#[test]
fn occurs_check_direct() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, meta) = table.fresh_type_var(&mut store, k);

    // ?a ~ (?a -> Int)  — would create infinite type.
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let infinite = store.mk_arrow(meta, int);

    let result = table.unify_types(&store, meta, infinite);
    assert!(matches!(result, Err(UnificationError::OccursCheck { .. })));
}

#[test]
fn occurs_check_indirect() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, meta_a) = table.fresh_type_var(&mut store, k);
    let (_, meta_b) = table.fresh_type_var(&mut store, k);

    // ?a ~ ?b  (OK)
    assert!(table.unify_types(&store, meta_a, meta_b).is_ok());

    // Now ?b ~ (?a -> Int) — since ?a -> ?b, this is ?b ~ (?b -> Int).
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let infinite = store.mk_arrow(meta_a, int);

    let result = table.unify_types(&store, meta_b, infinite);
    assert!(matches!(result, Err(UnificationError::OccursCheck { .. })));
}

#[test]
fn error_absorbs_mismatch() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let err = store.mk_error();

    assert!(table.unify_types(&store, err, int).is_ok());
    assert!(table.unify_types(&store, int, err).is_ok());
}

#[test]
fn unify_rigid_same() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let a = store.mk_rigid(TypeBinderId(0), k);

    assert!(table.unify_types(&store, a, a).is_ok());
}

#[test]
fn unify_rigid_different() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let a = store.mk_rigid(TypeBinderId(0), k);
    let b = store.mk_rigid(TypeBinderId(1), k);

    let result = table.unify_types(&store, a, b);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
}

// ---------------------------------------------------------------------------
// Unification — kind unification
// ---------------------------------------------------------------------------

#[test]
fn unify_kind_type_with_type() {
    let store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();

    assert!(table.unify_kinds(&store, k, k).is_ok());
}

#[test]
fn unify_kind_arrows() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let k1 = store.kind_arrow(k, k);
    let k2 = store.kind_arrow(k, k);

    assert!(table.unify_kinds(&store, k1, k2).is_ok());
}

#[test]
fn unify_kind_var_with_type() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var, kvar) = table.fresh_kind_var(&mut store);

    assert!(table.unify_kinds(&store, kvar, k).is_ok());
    assert_eq!(table.probe_kind_var(var), Some(k));
}

#[test]
fn unify_kind_mismatch() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let k_arrow = store.kind_arrow(k, k);

    let result = table.unify_kinds(&store, k, k_arrow);
    assert!(matches!(result, Err(UnificationError::KindMismatch { .. })));
}

#[test]
fn kind_occurs_check() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, kvar) = table.fresh_kind_var(&mut store);

    // ?k ~ (?k -> Type) — infinite kind.
    let infinite = store.kind_arrow(kvar, k);
    let result = table.unify_kinds(&store, kvar, infinite);
    assert!(matches!(
        result,
        Err(UnificationError::KindOccursCheck { .. })
    ));
}

// ---------------------------------------------------------------------------
// Zonking
// ---------------------------------------------------------------------------

#[test]
fn zonk_solved_meta() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (_, meta) = table.fresh_type_var(&mut store, k);

    // ?a = Int
    assert!(table.unify_types(&store, meta, int).is_ok());

    // Build F(?a) = ?a -> ?a
    let arrow = store.mk_arrow(meta, meta);

    // Zonk should produce Int -> Int.
    let zonked = table.zonk_type(&mut store, arrow);

    // Verify structure: Application(Application(Arrow, Int), Int)
    let outer = store.get_type(zonked);
    let TypeKind::Application(partial, rhs) = &outer.kind else {
        panic!("expected Application, got {:?}", outer.kind);
    };
    assert_eq!(
        store.get_type(*rhs).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );

    let mid = store.get_type(*partial);
    let TypeKind::Application(ctor, lhs) = &mid.kind else {
        panic!("expected Application, got {:?}", mid.kind);
    };
    assert_eq!(
        store.get_type(*lhs).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        store.get_type(*ctor).kind,
        TypeKind::Constructor(TypeConstructor::Arrow)
    );
}

#[test]
fn zonk_unsolved_meta_unchanged() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, meta) = table.fresh_type_var(&mut store, k);

    let zonked = table.zonk_type(&mut store, meta);
    assert_eq!(zonked, meta);
}

#[test]
fn zonk_chain() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (_, meta_a) = table.fresh_type_var(&mut store, k);
    let (_, meta_b) = table.fresh_type_var(&mut store, k);

    // ?a = ?b, ?b = Int
    assert!(table.unify_types(&store, meta_a, meta_b).is_ok());
    assert!(table.unify_types(&store, meta_b, int).is_ok());

    let zonked = table.zonk_type(&mut store, meta_a);

    // Should resolve through the chain to Int.
    assert_eq!(
        store.get_type(zonked).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
}

#[test]
fn zonk_constructor_unchanged() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);

    let zonked = table.zonk_type(&mut store, int);
    // No meta-variables — should return the same TypeId.
    assert_eq!(zonked, int);
}

#[test]
fn zonk_kind_variable() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, kvar) = table.fresh_kind_var(&mut store);

    // ?k = Type
    assert!(table.unify_kinds(&store, kvar, k).is_ok());

    let arrow_kind = store.kind_arrow(kvar, kvar);
    let zonked = table.zonk_kind(&mut store, arrow_kind);

    // Should be Type -> Type now.
    let Kind::Arrow(from, to) = store.get_kind(zonked) else {
        panic!("expected Arrow");
    };
    assert_eq!(*store.get_kind(*from), Kind::Type);
    assert_eq!(*store.get_kind(*to), Kind::Type);
}

// ---------------------------------------------------------------------------
// Shallow resolution
// ---------------------------------------------------------------------------

#[test]
fn shallow_resolve_follows_chain() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (_, meta_a) = table.fresh_type_var(&mut store, k);
    let (_, meta_b) = table.fresh_type_var(&mut store, k);

    // ?a -> ?b -> Int
    assert!(table.unify_types(&store, meta_a, meta_b).is_ok());
    assert!(table.unify_types(&store, meta_b, int).is_ok());

    let resolved = table.shallow_resolve_type(&store, meta_a);
    assert_eq!(resolved, int);
}

#[test]
fn shallow_resolve_stops_at_unsolved() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (_, meta) = table.fresh_type_var(&mut store, k);

    let resolved = table.shallow_resolve_type(&store, meta);
    assert_eq!(resolved, meta);
}

// ---------------------------------------------------------------------------
// Integration: unify + zonk round-trip
// ---------------------------------------------------------------------------

#[test]
fn unify_nested_arrows_then_zonk() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let bool_ty = store.mk_constructor(TypeConstructor::Bool, k);
    let (_, meta_a) = table.fresh_type_var(&mut store, k);
    let (_, meta_b) = table.fresh_type_var(&mut store, k);

    // (?a -> ?b) ~ (Int -> Bool)
    let lhs = store.mk_arrow(meta_a, meta_b);
    let rhs = store.mk_arrow(int, bool_ty);
    assert!(table.unify_types(&store, lhs, rhs).is_ok());

    // Zonking the metas directly should yield the solved types.
    let za = table.zonk_type(&mut store, meta_a);
    let zb = table.zonk_type(&mut store, meta_b);
    assert_eq!(
        store.get_type(za).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        store.get_type(zb).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn unify_with_kind_inference() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();

    // Create a type constructor F with unknown kind ?k.
    let (_, kvar) = table.fresh_kind_var(&mut store);
    let f = store.mk_constructor(
        TypeConstructor::Named(crate::lower::ir::QualifiedName {
            segments: vec!["F".to_owned()],
            range: crate::reporting::TextRange::Generated,
        }),
        kvar,
    );

    // Apply F to Int: this tells us F has kind (* -> ?result).
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (_, result_kind) = table.fresh_kind_var(&mut store);
    let _app = store.mk_application(f, int, result_kind);

    // Unify ?k with (* -> ?result)
    let expected_kind = store.kind_arrow(k, result_kind);
    assert!(table.unify_kinds(&store, kvar, expected_kind).is_ok());

    // Now if we learn the result is also Type:
    assert!(table.unify_kinds(&store, result_kind, k).is_ok());

    // Zonk the original kind variable — should be * -> *.
    let zonked = table.zonk_kind(&mut store, kvar);
    let Kind::Arrow(from, to) = store.get_kind(zonked) else {
        panic!("expected Arrow kind");
    };
    assert_eq!(*store.get_kind(*from), Kind::Type);
    assert_eq!(*store.get_kind(*to), Kind::Type);
}
