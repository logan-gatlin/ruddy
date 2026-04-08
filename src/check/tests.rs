use crate::engine::{Eng, Source};
use crate::resolver::FailingResolver;
use crate::ty::store::TypeStore;
use crate::ty::typed_ir as tir;
use crate::ty::*;

use super::{UnificationError, UnificationTable, check_text};

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

    assert!(table.unify_types(&mut store, int, int).is_ok());
}

#[test]
fn unify_meta_with_constructor() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let (var, meta) = table.fresh_type_var(&mut store, k);

    assert!(table.unify_types(&mut store, meta, int).is_ok());
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
    assert!(table.unify_types(&mut store, int, meta).is_ok());
    assert_eq!(table.probe_type_var(var), Some(int));
}

#[test]
fn unify_two_metas() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var_a, meta_a) = table.fresh_type_var(&mut store, k);
    let (_var_b, meta_b) = table.fresh_type_var(&mut store, k);

    assert!(table.unify_types(&mut store, meta_a, meta_b).is_ok());
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

    assert!(table.unify_types(&mut store, arrow_a, arrow_b).is_ok());
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

    let result = table.unify_types(&mut store, int, bool_ty);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
}

#[test]
fn unify_mismatch_arrow_vs_constructor() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let arrow = store.mk_arrow(int, int);

    let result = table.unify_types(&mut store, arrow, int);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
}

#[test]
fn unify_meta_with_forall_is_rejected_predicatively() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var, meta) = table.fresh_type_var(&mut store, k);

    let binder_id = TypeBinderId(0);
    let binder = TypeBinder {
        id: binder_id,
        name: "a".to_owned(),
        kind: k,
        range: crate::reporting::TextRange::Generated,
    };
    let a = store.mk_rigid(binder_id, k);
    let body = store.mk_arrow(a, a);
    let poly = store.mk_forall(vec![binder], Vec::new(), body);

    let result = table.unify_types(&mut store, meta, poly);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
    assert_eq!(table.probe_type_var(var), None);
}

#[test]
fn unify_meta_with_nested_forall_is_rejected_predicatively_in_application() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var, meta) = table.fresh_type_var(&mut store, k);

    let binder_id = TypeBinderId(0);
    let binder = TypeBinder {
        id: binder_id,
        name: "a".to_owned(),
        kind: k,
        range: crate::reporting::TextRange::Generated,
    };
    let a = store.mk_rigid(binder_id, k);
    let arrow = store.mk_arrow(a, a);
    let inner = store.mk_forall(vec![binder], Vec::new(), arrow);
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let target = store.mk_arrow(inner, int);

    let result = table.unify_types(&mut store, meta, target);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
    assert_eq!(table.probe_type_var(var), None);
}

#[test]
fn unify_meta_with_nested_forall_is_rejected_predicatively_in_lambda() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let lambda_kind = store.kind_arrow(k, k);
    let (var, meta) = table.fresh_type_var(&mut store, lambda_kind);

    let binder_id = TypeBinderId(0);
    let binder = TypeBinder {
        id: binder_id,
        name: "a".to_owned(),
        kind: k,
        range: crate::reporting::TextRange::Generated,
    };
    let a = store.mk_rigid(binder_id, k);
    let arrow = store.mk_arrow(a, a);
    let inner = store.mk_forall(vec![binder], Vec::new(), arrow);

    let lambda_binder = TypeBinder {
        id: TypeBinderId(1),
        name: "x".to_owned(),
        kind: k,
        range: crate::reporting::TextRange::Generated,
    };
    let target = store.mk_lambda(lambda_binder, inner);

    let result = table.unify_types(&mut store, meta, target);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
    assert_eq!(table.probe_type_var(var), None);
}

#[test]
fn unify_meta_with_nested_forall_is_rejected_predicatively_in_forall_body() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let (var, meta) = table.fresh_type_var(&mut store, k);

    let inner_binder = TypeBinderId(0);
    let inner_poly = {
        let arg = store.mk_rigid(inner_binder, k);
        let ret = store.mk_rigid(inner_binder, k);
        let body = store.mk_arrow(arg, ret);
        store.mk_forall(
            vec![TypeBinder {
                id: inner_binder,
                name: "a".to_owned(),
                kind: k,
                range: crate::reporting::TextRange::Generated,
            }],
            Vec::new(),
            body,
        )
    };

    let outer_poly = {
        let outer_binder = TypeBinder {
            id: TypeBinderId(1),
            name: "r".to_owned(),
            kind: k,
            range: crate::reporting::TextRange::Generated,
        };
        store.mk_forall(vec![outer_binder], Vec::new(), inner_poly)
    };

    let row_tail = store.mk_row_empty();
    let row = store.mk_row_extend("value", outer_poly, row_tail);
    let target = store.mk_record(row);

    let result = table.unify_types(&mut store, meta, target);
    assert!(matches!(result, Err(UnificationError::TypeMismatch { .. })));
    assert_eq!(table.probe_type_var(var), None);
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

    let result = table.unify_types(&mut store, meta, infinite);
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
    assert!(table.unify_types(&mut store, meta_a, meta_b).is_ok());

    // Now ?b ~ (?a -> Int) — since ?a -> ?b, this is ?b ~ (?b -> Int).
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let infinite = store.mk_arrow(meta_a, int);

    let result = table.unify_types(&mut store, meta_b, infinite);
    assert!(matches!(result, Err(UnificationError::OccursCheck { .. })));
}

#[test]
fn error_absorbs_mismatch() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let int = store.mk_constructor(TypeConstructor::Integer, k);
    let err = store.mk_error();

    assert!(table.unify_types(&mut store, err, int).is_ok());
    assert!(table.unify_types(&mut store, int, err).is_ok());
}

#[test]
fn unify_rigid_same() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let a = store.mk_rigid(TypeBinderId(0), k);

    assert!(table.unify_types(&mut store, a, a).is_ok());
}

#[test]
fn unify_rigid_different() {
    let mut store = TypeStore::new();
    let mut table = UnificationTable::new();
    let k = store.kind_type();
    let a = store.mk_rigid(TypeBinderId(0), k);
    let b = store.mk_rigid(TypeBinderId(1), k);

    let result = table.unify_types(&mut store, a, b);
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
    assert!(table.unify_types(&mut store, meta, int).is_ok());

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
    assert!(table.unify_types(&mut store, meta_a, meta_b).is_ok());
    assert!(table.unify_types(&mut store, meta_b, int).is_ok());

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
    assert!(table.unify_types(&mut store, meta_a, meta_b).is_ok());
    assert!(table.unify_types(&mut store, meta_b, int).is_ok());

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
    assert!(table.unify_types(&mut store, lhs, rhs).is_ok());

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

// ---------------------------------------------------------------------------
// HM inference / checker integration
// ---------------------------------------------------------------------------

fn typed_binding_expr_by_name<'a>(module: &'a tir::Module, name: &str) -> Option<&'a tir::Expr> {
    module.statements.iter().find_map(|statement| {
        let tir::Statement::Let {
            kind:
                tir::LetStatementKind::PatternBinding {
                    pattern:
                        tir::Pattern {
                            kind:
                                tir::PatternKind::Binding {
                                    name: crate::lower::ir::ResolvedName::Global(path),
                                },
                            ..
                        },
                    value,
                },
            ..
        } = statement
        else {
            return None;
        };

        if path.text() == name {
            Some(value)
        } else {
            None
        }
    })
}

#[test]
fn infer_identity_is_polymorphic() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_identity.hc".to_owned(),
        [
            "bundle demo",
            "let id = fn x => x",
            "let a = id 1",
            "let b = id true",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");

    let a_expr = typed_binding_expr_by_name(module, "demo::a").expect("missing binding demo::a");
    let b_expr = typed_binding_expr_by_name(module, "demo::b").expect("missing binding demo::b");

    assert_eq!(
        checked.type_store.get_type(a_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        checked.type_store.get_type(b_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn infer_reports_if_condition_type_mismatch() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_if_mismatch.hc".to_owned(),
        ["bundle demo", "let x = if 1 then 2 else 3"].join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch")),
        "expected type mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infer_sum_constructors_and_match() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_sum_match.hc".to_owned(),
        [
            "bundle demo",
            "type Option = fn a => | Some a | None",
            "let some = Option::Some 1",
            "let unwrapped = match some with",
            "  | Option::Some x => x",
            "  | Option::None => 0",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let unwrapped = typed_binding_expr_by_name(module, "demo::unwrapped")
        .expect("missing binding demo::unwrapped");

    assert_eq!(
        checked.type_store.get_type(unwrapped.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
}

#[test]
fn infer_record_field_access_is_polymorphic() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_record_field_access.hc".to_owned(),
        [
            "bundle demo",
            "let get = fn r => r.x",
            "let a = get {x = 1, y = true}",
            "let b = get {x = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");

    let a_expr = typed_binding_expr_by_name(module, "demo::a").expect("missing binding demo::a");
    let b_expr = typed_binding_expr_by_name(module, "demo::b").expect("missing binding demo::b");

    assert_eq!(
        checked.type_store.get_type(a_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        checked.type_store.get_type(b_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn closed_record_pattern_rejects_extra_fields() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_closed_record_pattern.hc".to_owned(),
        [
            "bundle demo",
            "let pick = fn r => match r with",
            "  | {x} => x",
            "let value = pick {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch")),
        "expected a type mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn open_record_pattern_accepts_extra_fields() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_open_record_pattern.hc".to_owned(),
        [
            "bundle demo",
            "let pick = fn r => match r with",
            "  | {x, ..} => x",
            "let value = pick {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let value_expr =
        typed_binding_expr_by_name(module, "demo::value").expect("missing binding demo::value");

    assert_eq!(
        checked.type_store.get_type(value_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
}

#[test]
fn adding_unrelated_record_type_does_not_change_existing_inference() {
    let db = Eng::default();

    let base = Source::new(
        &db,
        "hm_record_stability_base.hc".to_owned(),
        [
            "bundle demo",
            "let get = fn r => r.x",
            "let a = get {x = 1}",
        ]
        .join("\n"),
    );

    let extended = Source::new(
        &db,
        "hm_record_stability_extended.hc".to_owned(),
        [
            "bundle demo",
            "type Other = fn t => {x: t}",
            "let get = fn r => r.x",
            "let a = get {x = 1}",
        ]
        .join("\n"),
    );

    let checked_base = check_text::<FailingResolver>(&db, base);
    let checked_extended = check_text::<FailingResolver>(&db, extended);

    assert!(
        checked_base.diagnostics.is_empty(),
        "expected no diagnostics in base program, got: {:?}",
        checked_base.diagnostics
    );
    assert!(
        checked_extended.diagnostics.is_empty(),
        "expected no diagnostics in extended program, got: {:?}",
        checked_extended.diagnostics
    );

    let base_module = checked_base
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked base root module");
    let extended_module = checked_extended
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked extended root module");

    let base_a = typed_binding_expr_by_name(base_module, "demo::a").expect("missing base demo::a");
    let extended_a =
        typed_binding_expr_by_name(extended_module, "demo::a").expect("missing extended demo::a");

    assert_eq!(
        checked_base.type_store.get_type(base_a.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        checked_extended.type_store.get_type(extended_a.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
}

#[test]
fn struct_spread_declarations_are_checked_as_closed_records() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_record_spread_decl.hc".to_owned(),
        [
            "bundle demo",
            "type Base = fn a => {x: a}",
            "type Pair = fn a b => {..Base a, y: b}",
            "let get = fn r => r.y",
            "let value = get {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let value_expr =
        typed_binding_expr_by_name(module, "demo::value").expect("missing binding demo::value");

    assert_eq!(
        checked.type_store.get_type(value_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn alias_types_are_transparent_in_record_spreads() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_alias_record_spread_decl.hc".to_owned(),
        [
            "bundle demo",
            "type Base = fn a => {x: a}",
            "type ~Alias = fn a => Base a",
            "type Pair = fn a b => {..Alias a, y: b}",
            "let get = fn r => r.y",
            "let value = get {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let value_expr =
        typed_binding_expr_by_name(module, "demo::value").expect("missing binding demo::value");

    assert_eq!(
        checked.type_store.get_type(value_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn opaque_type_constructor_blocking_structural_construction() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_opaque_type_constructor_blocking_structural.hc".to_owned(),
        [
            "bundle demo",
            "type S = {}",
            "let f: S -> S = fn a => a",
            "let x = f {}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch")),
        "expected type mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_type_is_constructible_via_name() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_opaque_type_constructor_name.hc".to_owned(),
        ["bundle demo", "type S = {}", "let x = S {}"].join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn existing_sum_variants_still_work() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_opaque_sum_variants_still_work.hc".to_owned(),
        [
            "bundle demo",
            "type Result = | Nil | Cons Int",
            "let a: Result = Nil",
            "let b: Result = Cons 1",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_record_still_allows_declaration_spread_typing() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_opaque_record_spread_decl.hc".to_owned(),
        [
            "bundle demo",
            "type Base = fn a => {x: a}",
            "type Pair = fn a b => {..Base a, y: b}",
            "let get = fn r => r.y",
            "let value = get {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let value_expr =
        typed_binding_expr_by_name(module, "demo::value").expect("missing binding demo::value");

    assert_eq!(
        checked.type_store.get_type(value_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn alias_type_application_reports_too_many_arguments() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_alias_arity_mismatch.hc".to_owned(),
        [
            "bundle demo",
            "type ~Pair = fn a b => (a, b)",
            "type ~Bad = Pair _ _ _",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("expects 2 type argument(s), found 3")),
        "expected alias arity diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn bare_alias_annotation_reports_kind_mismatch() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "core_bundle_alias_annotation_kind_mismatch.hc".to_owned(),
        ["bundle core", "type ~Id = fn a => a", "let a : Id = ()"].join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected kind mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn bare_alias_name_is_a_first_class_constructor() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_alias_bare_name_constructor.hc".to_owned(),
        [
            "bundle demo",
            "type ~Id = fn a => a",
            "type ~Lift :: Type -> Type = Id",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn alias_partial_application_stays_higher_kinded() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_alias_partial_application.hc".to_owned(),
        [
            "bundle demo",
            "type ~Const = fn a b => a",
            "type ~Left = Const ()",
            "type ~Use = Left ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn hk_alias_with_explicit_marker_type_params_checks() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_alias_explicit_markers.hc".to_owned(),
        [
            "bundle demo",
            "type ~Compose = fn (f :: Type -> Type) (g :: Type -> Type) a => f (g a)",
            "type ~MaybeUnit = Compose [] [] ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn hk_binders_without_markers_are_inferred_in_type_aliases() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_alias_inferred_binders.hc".to_owned(),
        [
            "bundle demo",
            "type ~MapLike = fn f a b => (a -> b) -> f a -> f b",
            "type ~Use = MapLike [] () ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_nominal_partial_application_infers_higher_kind() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_opaque_nominal_partial_apply.hc".to_owned(),
        [
            "bundle demo",
            "type Result = fn a b => | Ok a | Err b",
            "type Option = Result ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_nominal_partial_application_can_be_used_downstream() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_opaque_nominal_partial_apply_use.hc".to_owned(),
        [
            "bundle demo",
            "type Result = fn a b => | Ok a | Err b",
            "type Option = Result ()",
            "type ~Use = Option ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_nominal_partial_application_accepts_matching_declared_kind() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_opaque_nominal_declared_kind_ok.hc".to_owned(),
        [
            "bundle demo",
            "type Result = fn a b => | Ok a | Err b",
            "type Option :: Type -> Type = Result ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_nominal_can_wrap_first_class_type_lambda() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_opaque_nominal_type_lambda.hc".to_owned(),
        ["bundle demo", "type Id = (fn a => a)", "type ~Use = Id ()"].join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn forall_hk_marker_bindings_type_check() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_forall_hk_marker.hc".to_owned(),
        [
            "bundle demo",
            "let idf: for a (f :: Type -> Type) in f a -> f a = fn x => x",
            "let value = idf [1]",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn applying_non_constructor_at_type_level_reports_kind_mismatch() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_kind_mismatch_non_constructor.hc".to_owned(),
        ["bundle demo", "type ~Bad = fn a => Integer a"].join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected kind mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn unconstrained_alias_binder_kinds_default_to_type() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_unconstrained_defaults_to_type.hc".to_owned(),
        [
            "bundle demo",
            "type ~K = fn f => f",
            "type ~Good = K Integer",
            "type ~Bad = K []",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected kind mismatch diagnostic from `K []`, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn unconstrained_opaque_nominal_binder_kinds_default_to_type() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hk_opaque_nominal_unconstrained_defaults_to_type.hc".to_owned(),
        [
            "bundle demo",
            "type K = fn f => f",
            "type ~Good = K ()",
            "type ~Bad = K []",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected kind mismatch diagnostic from `K []`, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn type_lambda_application_beta_reduces() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_type_lambda_beta_reduce.hc".to_owned(),
        [
            "bundle demo",
            "type ~IdUnit = (fn a => a) ()",
            "let value: IdUnit = ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn declaration_kind_mismatches_are_reported() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_decl_kind_mismatch.hc".to_owned(),
        [
            "bundle demo",
            "type Option :: Type = fn a => | Some a | None",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected declaration kind mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn opaque_nominal_declared_kind_mismatches_are_reported() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_opaque_nominal_decl_kind_mismatch.hc".to_owned(),
        [
            "bundle demo",
            "type Result = fn a b => | Ok a | Err b",
            "type Option :: Type = Result ()",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("kind mismatch")),
        "expected declaration kind mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn trait_and_impl_statements_are_carried_without_solver_diagnostics() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_trait_surface_passthrough.hc".to_owned(),
        [
            "bundle demo",
            "type Flag = | Flag",
            "trait Keep: a =",
            "  let keep: a -> a",
            "end",
            "impl Keep Flag =",
            "  let keep = fn x => x",
            "end",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn where_constraints_are_carried_without_solver_diagnostics() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_where_constraint_passthrough.hc".to_owned(),
        [
            "bundle demo",
            "trait Keep: a =",
            "  let keep: a -> a",
            "end",
            "type ~Poly = for a in a where Keep a",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infer_annotated_polymorphic_binding_supports_distinct_instantiations() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_annotated_binding.hc".to_owned(),
        [
            "bundle demo",
            "let id: for a in a -> a = fn x => x",
            "let a = id 1",
            "let b = id true",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");

    let a_expr = typed_binding_expr_by_name(module, "demo::a").expect("missing binding demo::a");
    let b_expr = typed_binding_expr_by_name(module, "demo::b").expect("missing binding demo::b");

    assert_eq!(
        checked.type_store.get_type(a_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        checked.type_store.get_type(b_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn infer_polymorphic_parameter_supports_multiple_calls_same_body() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_multi_call_body.hc".to_owned(),
        [
            "bundle demo",
            "let apply_twice = fn (f: for a in a -> a) => (f 1, f true)",
            "let both = apply_twice (fn x => x)",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infer_nested_rankn_annotation_checks_argument_and_result() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_nested_rankn_argument_result.hc".to_owned(),
        [
            "bundle demo",
            "let rank3: for r in ((for a in a -> a) -> r) -> r = fn k => k (fn x => x)",
            "let value = rank3 (fn g => (g 1, g true))",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infer_polymorphic_value_reuse_through_let_binding() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_polymorphic_value_reuse.hc".to_owned(),
        [
            "bundle demo",
            "let id: for a in a -> a = fn x => x",
            "let alias = id",
            "let value = (alias 1, alias true)",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infer_rank2_argument_accepts_lambda_via_contextual_checking() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_rank2_argument.hc".to_owned(),
        [
            "bundle demo",
            "let use_int = fn (f: for a in a -> a) => f 1",
            "let use_bool = fn (f: for a in a -> a) => f true",
            "let a = use_int (fn x => x)",
            "let b = use_bool (fn x => x)",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");

    let a_expr = typed_binding_expr_by_name(module, "demo::a").expect("missing binding demo::a");
    let b_expr = typed_binding_expr_by_name(module, "demo::b").expect("missing binding demo::b");

    assert_eq!(
        checked.type_store.get_type(a_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
    assert_eq!(
        checked.type_store.get_type(b_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Bool)
    );
}

#[test]
fn infer_nested_rankn_annotation_round_trips() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_rank3_shape.hc".to_owned(),
        [
            "bundle demo",
            "let rank3: for r in ((for a in a -> a) -> r) -> r = fn k => k (fn x => x)",
            "let value = rank3 (fn g => g 1)",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );

    let module = checked
        .source
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing checked root module");
    let value_expr =
        typed_binding_expr_by_name(module, "demo::value").expect("missing binding demo::value");

    assert_eq!(
        checked.type_store.get_type(value_expr.ty).kind,
        TypeKind::Constructor(TypeConstructor::Integer)
    );
}

#[test]
fn term_level_forall_constraints_are_carried_without_solver_diagnostics() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_term_constraints_passthrough.hc".to_owned(),
        [
            "bundle demo",
            "trait Keep: a =",
            "  let keep: a -> a",
            "end",
            "let id: for a in a -> a where Keep a = fn x => x",
            "let value = id 1",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked.diagnostics.is_empty(),
        "expected no checker diagnostics, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn monomorphic_function_is_rejected_where_polymorphic_argument_is_required() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_mono_rejected.hc".to_owned(),
        [
            "bundle demo",
            "let consumer = fn (f: for a in a -> a) => (f 1, f true)",
            "let mono: () -> () = fn x => x",
            "let bad = consumer mono",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch")),
        "expected type mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}

#[test]
fn incompatible_polymorphic_annotation_is_rejected() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hm_hrp_incompatible_poly.hc".to_owned(),
        [
            "bundle demo",
            "let id: for a in a -> a = fn x => x",
            "let bad: for a in a -> (a, a) = id",
        ]
        .join("\n"),
    );

    let checked = check_text::<FailingResolver>(&db, source);
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("type mismatch")),
        "expected type mismatch diagnostic, got: {:?}",
        checked.diagnostics
    );
}
