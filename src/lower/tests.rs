use crate::engine::{Eng, Source};
use crate::lower::ir::{
    Expr, FormatStringSegment, KindExpr, LetStatementKind, Literal, LiteralValue, LoweredModule,
    Pattern, RecordTypeMember, ResolvedName, Statement, TypeDefinition, TypeExpr,
    TypeStatementKind, WasmTopLevelDeclaration,
};
use crate::lower::{lower_diagnostics, lower_text};
use crate::resolver::Resolver;
use crate::wasm;

struct TestResolver;

impl Resolver for TestResolver {
    fn canonize_bare(name: &str, _from: &str) -> Option<String> {
        Self::canonize(name, "")
    }

    fn canonize(name: &str, _from: &str) -> Option<String> {
        match name {
            "ref.hc" | "bad.hc" | "mod.hc" | "dup_bad.hc" => Some(name.to_owned()),
            _ => None,
        }
    }

    fn resolve(canon: &str) -> Option<String> {
        match canon {
            "ref.hc" => Some("let imported = 2".to_owned()),
            "bad.hc" => Some("bundle other\nlet imported = 2".to_owned()),
            "mod.hc" => Some("let value = 7".to_owned()),
            "dup_bad.hc" => Some("type ~T = Missing".to_owned()),
            _ => None,
        }
    }
}

fn lowered_binding_expr_by_name<'a>(module: &'a LoweredModule, name: &'a str) -> Option<&'a Expr> {
    module.statements.iter().find_map(|statement| {
        let Statement::Let {
            kind:
                LetStatementKind::PatternBinding {
                    pattern:
                        Pattern::Binding {
                            name: ResolvedName::Global(path),
                            ..
                        },
                    value,
                    ..
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

fn expect_integer_literal(literal: &Literal, expected: i64) {
    let LiteralValue::Integer(value) = literal.value else {
        panic!("expected integer literal, got {literal:?}");
    };
    assert_eq!(value, expected);
}

fn expect_natural_literal(literal: &Literal, expected: u64) {
    let LiteralValue::Natural(value) = literal.value else {
        panic!("expected natural literal, got {literal:?}");
    };
    assert_eq!(value, expected);
}

fn expect_real_literal(literal: &Literal, expected: f64) {
    let LiteralValue::Real(value) = literal.value else {
        panic!("expected real literal, got {literal:?}");
    };
    assert!((value.get() - expected).abs() < f64::EPSILON);
}

fn expect_bool_literal(literal: &Literal, expected: bool) {
    let LiteralValue::Bool(value) = literal.value else {
        panic!("expected bool literal, got {literal:?}");
    };
    assert_eq!(value, expected);
}

fn expect_format_string_literal(literal: &Literal, expected: &[FormatStringSegment]) {
    let LiteralValue::FormatString(ref segments) = literal.value else {
        panic!("expected format string literal, got {literal:?}");
    };
    assert_eq!(segments, expected);
}

#[test]
fn lowering_normalizes_inline_and_file_modules() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "root.hc".to_owned(),
        [
            "bundle demo",
            "module Inline =",
            "let local = 1",
            "end",
            "module Ref in \"ref.hc\"",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    assert!(matches!(
        root.statements.first(),
        Some(Statement::ModuleDecl { .. })
    ));
    assert!(matches!(
        root.statements.get(1),
        Some(Statement::ModuleDecl { .. })
    ));
    assert!(
        lowered
            .modules
            .iter()
            .any(|module| module.path.text() == "demo::Inline")
    );
    assert!(
        lowered
            .modules
            .iter()
            .any(|module| module.path.text() == "demo::Ref")
    );
}

#[test]
fn lowering_compiles_away_use_and_resolves_opened_names() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "use_root.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "use M",
            "let x = value",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    assert_eq!(
        root.statements
            .iter()
            .filter(|statement| matches!(statement, Statement::ModuleDecl { .. }))
            .count(),
        1
    );

    let lowered_let = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .expect("missing lowered let statement");

    match lowered_let {
        Expr::Name(ResolvedName::Global(path)) => {
            assert_eq!(path.text(), "demo::M::value");
        }
        other => panic!("expected resolved name, got {other:?}"),
    }
}

#[test]
fn expr_use_does_not_leak_opened_modules() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "expr_use_isolated.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "let x = use M in value",
            "let y = value",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let lowered_values: Vec<&Expr> = root
        .statements
        .iter()
        .filter_map(|statement| {
            if let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(lowered_values.len(), 2);

    match lowered_values[0] {
        Expr::Name(ResolvedName::Global(path)) => {
            assert_eq!(path.text(), "demo::M::value");
        }
        other => panic!("expected use body to resolve imported value, got {other:?}"),
    }

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("failed to resolve term name `value`"))
    );
}

#[test]
fn expr_use_alias_does_not_persist_to_statements() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "expr_use_alias_scope.hc".to_owned(),
        [
            "bundle demo",
            "module A in \"mod.hc\"",
            "let x = use A as Alias in Alias::value",
            "use A as Alias",
            "let y = Alias::value",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .all(|diag| !diag.message.contains("duplicate use alias `Alias`"))
    );

    let lowered_values: Vec<&Expr> = root
        .statements
        .iter()
        .filter_map(|statement| {
            if let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(lowered_values.len(), 2);

    for value in [lowered_values[0], lowered_values[1]] {
        match value {
            Expr::Name(ResolvedName::Global(path)) => {
                assert_eq!(path.text(), "demo::A::value");
            }
            other => panic!("expected module alias use to resolve value, got {other:?}"),
        }
    }
}

#[test]
fn use_bundle_shorthand_matches_explicit_root_path() {
    let db = Eng::default();
    let shorthand = Source::new(
        &db,
        "use_bundle_shorthand.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "use bundle as Root",
            "let x = Root::M::value",
            "let y = use bundle as LocalRoot in LocalRoot::M::value",
        ]
        .join("\n"),
    );
    let explicit = Source::new(
        &db,
        "use_bundle_explicit.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "use root::demo as Root",
            "let x = Root::M::value",
            "let y = use root::demo as LocalRoot in LocalRoot::M::value",
        ]
        .join("\n"),
    );

    let shorthand_diagnostics = lower_diagnostics::<TestResolver>(&db, shorthand);
    let explicit_diagnostics = lower_diagnostics::<TestResolver>(&db, explicit);
    assert!(
        shorthand_diagnostics.is_empty(),
        "unexpected diagnostics for bundle-prefixed path form: {shorthand_diagnostics:?}"
    );
    assert!(
        explicit_diagnostics.is_empty(),
        "unexpected diagnostics for explicit root path form: {explicit_diagnostics:?}"
    );

    let lowered_shorthand = lower_text::<TestResolver>(&db, shorthand);
    let lowered_explicit = lower_text::<TestResolver>(&db, explicit);

    let shorthand_root = lowered_shorthand
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing shorthand root lowered module");
    let explicit_root = lowered_explicit
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing explicit root lowered module");

    let shorthand_x = lowered_binding_expr_by_name(shorthand_root, "demo::x")
        .expect("missing shorthand x binding");
    let explicit_x =
        lowered_binding_expr_by_name(explicit_root, "demo::x").expect("missing explicit x binding");
    let shorthand_y = lowered_binding_expr_by_name(shorthand_root, "demo::y")
        .expect("missing shorthand y binding");
    let explicit_y =
        lowered_binding_expr_by_name(explicit_root, "demo::y").expect("missing explicit y binding");

    let shorthand_x_path = match shorthand_x {
        Expr::Name(ResolvedName::Global(path)) => path.text(),
        other => panic!("expected shorthand x to lower to a global name, got {other:?}"),
    };
    let explicit_x_path = match explicit_x {
        Expr::Name(ResolvedName::Global(path)) => path.text(),
        other => panic!("expected explicit x to lower to a global name, got {other:?}"),
    };
    assert_eq!(shorthand_x_path, "demo::M::value");
    assert_eq!(shorthand_x_path, explicit_x_path);

    let shorthand_y_path = match shorthand_y {
        Expr::Name(ResolvedName::Global(path)) => path.text(),
        other => panic!("expected shorthand y to lower to a global name, got {other:?}"),
    };
    let explicit_y_path = match explicit_y {
        Expr::Name(ResolvedName::Global(path)) => path.text(),
        other => panic!("expected explicit y to lower to a global name, got {other:?}"),
    };
    assert_eq!(shorthand_y_path, "demo::M::value");
    assert_eq!(shorthand_y_path, explicit_y_path);
}

#[test]
fn bundle_prefixed_paths_match_explicit_root_bundle_paths() {
    let db = Eng::default();
    let shorthand = Source::new(
        &db,
        "bundle_prefixed_paths_shorthand.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "use bundle::M as Alias",
            "let x = Alias::value",
            "let y = bundle::M::value",
            "let z = use bundle::M in value",
        ]
        .join("\n"),
    );
    let explicit = Source::new(
        &db,
        "bundle_prefixed_paths_explicit.hc".to_owned(),
        [
            "bundle demo",
            "module M in \"mod.hc\"",
            "use root::demo::M as Alias",
            "let x = Alias::value",
            "let y = root::demo::M::value",
            "let z = use root::demo::M in value",
        ]
        .join("\n"),
    );

    let shorthand_diagnostics = lower_diagnostics::<TestResolver>(&db, shorthand);
    let explicit_diagnostics = lower_diagnostics::<TestResolver>(&db, explicit);
    assert!(
        shorthand_diagnostics.is_empty(),
        "unexpected diagnostics for bundle-prefixed path form: {shorthand_diagnostics:?}"
    );
    assert!(
        explicit_diagnostics.is_empty(),
        "unexpected diagnostics for explicit root path form: {explicit_diagnostics:?}"
    );

    let lowered_shorthand = lower_text::<TestResolver>(&db, shorthand);
    let lowered_explicit = lower_text::<TestResolver>(&db, explicit);

    let shorthand_root = lowered_shorthand
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing shorthand root lowered module");
    let explicit_root = lowered_explicit
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing explicit root lowered module");

    for name in ["x", "y", "z"] {
        let binding_name = format!("demo::{name}");

        let shorthand_binding = lowered_binding_expr_by_name(shorthand_root, &binding_name)
            .unwrap_or_else(|| panic!("missing shorthand {name} binding"));
        let explicit_binding = lowered_binding_expr_by_name(explicit_root, &binding_name)
            .unwrap_or_else(|| panic!("missing explicit {name} binding"));

        let shorthand_path = match shorthand_binding {
            Expr::Name(ResolvedName::Global(path)) => path.text(),
            other => panic!("expected shorthand {name} to lower to a global name, got {other:?}"),
        };
        let explicit_path = match explicit_binding {
            Expr::Name(ResolvedName::Global(path)) => path.text(),
            other => panic!("expected explicit {name} to lower to a global name, got {other:?}"),
        };

        assert_eq!(shorthand_path, "demo::M::value");
        assert_eq!(shorthand_path, explicit_path);
    }
}

#[test]
fn lowering_reports_bundle_declaration_in_imported_file() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "root_bad.hc".to_owned(),
        ["bundle demo", "module Bad in \"bad.hc\""].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("must not declare `bundle`"))
    );
}

#[test]
fn pattern_constructor_resolution_wins_over_binder() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "constructors.hc".to_owned(),
        [
            "bundle demo",
            "type Option = | Some | None",
            "let f = fn | Some => 1 | x => 2",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let function_expr = root
        .statements
        .iter()
        .find_map(|statement| {
            let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            else {
                return None;
            };
            if let Expr::Function { params, body, .. } = value {
                Some((params, body.as_ref()))
            } else {
                None
            }
        })
        .expect("missing function expression");

    let (params, body) = function_expr;
    assert_eq!(params.len(), 1);
    let parameter_id = match params.first() {
        Some(Pattern::Binding {
            name: ResolvedName::Local { id, name, .. },
            ..
        }) => {
            assert!(name.starts_with("$arg#"));
            *id
        }
        other => panic!("expected synthetic local parameter binding, got {other:?}"),
    };

    let Expr::Match {
        scrutinee, arms, ..
    } = body
    else {
        panic!("multi-clause function should lower to match body");
    };

    match scrutinee.as_ref() {
        Expr::Name(ResolvedName::Local { id, .. }) => assert_eq!(*id, parameter_id),
        other => panic!("expected match scrutinee to reference synthetic arg, got {other:?}"),
    }

    assert!(matches!(
        arms.first().map(|arm| &arm.pattern),
        Some(Pattern::ConstructorName { .. })
    ));
}

#[test]
fn multi_parameter_functions_lower_to_curried_functions() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "curried_fn.hc".to_owned(),
        ["bundle demo", "let f = fn a b c => a"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let function_expr = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
                && let Expr::Function { .. } = value
            {
                return Some(value);
            }
            None
        })
        .expect("missing lowered function expression");

    let Expr::Function {
        params: params_a,
        body: body_b,
        ..
    } = function_expr
    else {
        panic!("expected function");
    };
    let a_id = match params_a.first() {
        Some(Pattern::Binding {
            name: ResolvedName::Local { id, name, .. },
            ..
        }) => {
            assert_eq!(name, "a");
            *id
        }
        other => panic!("expected first curried parameter binding, got {other:?}"),
    };
    assert_eq!(params_a.len(), 1);

    let Expr::Function {
        params: params_b,
        body: body_c,
        ..
    } = body_b.as_ref()
    else {
        panic!("expected second curried function layer");
    };
    let b_id = match params_b.first() {
        Some(Pattern::Binding {
            name: ResolvedName::Local { id, name, .. },
            ..
        }) => {
            assert_eq!(name, "b");
            *id
        }
        other => panic!("expected second curried parameter binding, got {other:?}"),
    };
    assert_eq!(params_b.len(), 1);

    let Expr::Function {
        params: params_c,
        body,
        ..
    } = body_c.as_ref()
    else {
        panic!("expected third curried function layer");
    };
    let c_id = match params_c.first() {
        Some(Pattern::Binding {
            name: ResolvedName::Local { id, name, .. },
            ..
        }) => {
            assert_eq!(name, "c");
            *id
        }
        other => panic!("expected third curried parameter binding, got {other:?}"),
    };
    assert_eq!(params_c.len(), 1);

    match body.as_ref() {
        Expr::Name(ResolvedName::Local { id, name, .. }) => {
            assert_eq!(*id, a_id);
            assert_eq!(name, "a");
            assert_ne!(*id, b_id);
            assert_ne!(*id, c_id);
        }
        other => panic!("expected curried body to reference first parameter, got {other:?}"),
    }
}

#[test]
fn top_level_recursive_let_binds_before_value_and_reports_non_function_recursion() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "recursive_top.hc".to_owned(),
        ["bundle demo", "let x = x"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let value_expr = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .expect("missing lowered let value");

    match value_expr {
        Expr::Name(ResolvedName::Global(path)) => {
            assert_eq!(path.text(), "demo::x");
        }
        other => panic!("expected recursive global name resolution, got {other:?}"),
    }

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(diagnostics.iter().any(|diag| {
        diag.message
            .contains("recursive reference to `demo::x` is only allowed inside a function")
    }));
    assert!(
        !diagnostics
            .iter()
            .any(|diag| diag.message.contains("failed to resolve term name"))
    );
}

#[test]
fn top_level_recursion_inside_function_is_allowed() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "recursive_top_fn.hc".to_owned(),
        ["bundle demo", "let x = fn arg => x"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(!diagnostics.iter().any(|diag| {
        diag.message
            .contains("recursive reference to `demo::x` is only allowed inside a function")
    }));
}

#[test]
fn local_recursion_outside_function_reports_diagnostic() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "recursive_local.hc".to_owned(),
        ["bundle demo", "do let x = x in x"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(diagnostics.iter().any(|diag| {
        diag.message
            .contains("recursive reference to `x` is only allowed inside a function")
    }));
}

#[test]
fn local_recursion_inside_function_is_allowed() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "recursive_local_fn.hc".to_owned(),
        ["bundle demo", "do let x = fn arg => x in x"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(!diagnostics.iter().any(|diag| {
        diag.message
            .contains("recursive reference to `x` is only allowed inside a function")
    }));
}

#[test]
fn bare_term_names_do_not_fallback_to_bundle_absolute_paths() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "forward_decl.hc".to_owned(),
        ["bundle demo", "let y = x", "let x = 1"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("failed to resolve term name"))
    );
}

#[test]
fn type_names_do_not_fallback_to_bundle_absolute_paths() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "type_forward_decl.hc".to_owned(),
        [
            "bundle demo",
            "type ~A = Missing",
            "type ~B = Missing::Thing",
        ]
        .join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("failed to resolve type name"))
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let alias_values: Vec<&TypeExpr> = root
        .statements
        .iter()
        .filter_map(|statement| {
            if let Statement::Type {
                kind: TypeStatementKind::Alias { value },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(alias_values.len(), 2);
    for value in alias_values {
        match value {
            TypeExpr::Name {
                name: ResolvedName::Error { .. },
            } => {}
            other => panic!("expected unresolved type name placeholder, got {other:?}"),
        }
    }
}

#[test]
fn unresolved_type_name_emits_single_diagnostic() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "single_unresolved_type_diag.hc".to_owned(),
        ["bundle demo", "type ~A = Missing"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    let unresolved_count = diagnostics
        .iter()
        .filter(|diag| diag.message == "failed to resolve type name")
        .count();

    assert_eq!(
        unresolved_count, 1,
        "expected one unresolved type diagnostic, got diagnostics: {diagnostics:?}"
    );
}

#[test]
fn duplicate_imported_source_diagnostics_are_deduplicated() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "dedupe_imported_source_diagnostics.hc".to_owned(),
        [
            "bundle demo",
            "module A in \"dup_bad.hc\"",
            "module B in \"dup_bad.hc\"",
        ]
        .join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    let unresolved_count = diagnostics
        .iter()
        .filter(|diag| diag.message == "failed to resolve type name")
        .count();

    assert_eq!(
        unresolved_count, 1,
        "expected one unresolved type diagnostic across duplicate imports, got diagnostics: {diagnostics:?}"
    );
}

#[test]
fn lower_source_accumulator_does_not_duplicate_single_import_diagnostics() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "single_import_duplicate_check.hc".to_owned(),
        ["bundle demo", "module A in \"dup_bad.hc\""].join("\n"),
    );

    let resolver = super::resolver_registry::resolver_token::<TestResolver>(&db);
    let diagnostics =
        super::query::lower_source::accumulated::<crate::reporting::Diag>(&db, source, resolver)
            .into_iter()
            .map(|diag| diag.0.clone())
            .collect::<Vec<_>>();

    let unresolved_count = diagnostics
        .iter()
        .filter(|diag| diag.message == "failed to resolve type name")
        .count();

    assert_eq!(
        unresolved_count, 1,
        "expected one raw unresolved type diagnostic for a single import, got diagnostics: {diagnostics:?}"
    );
}

#[test]
fn binary_operator_lowers_to_bracketed_function_application() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "binary_ops.hc".to_owned(),
        ["bundle demo", "let [+] = fn a b => a", "do 1 + 2"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let do_expr = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Let {
                kind:
                    LetStatementKind::PatternBinding {
                        pattern: Pattern::Hole { .. },
                        value,
                    },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .expect("missing lowered do-as-let statement");

    let Expr::Apply {
        callee: outer_callee,
        argument: outer_arg,
        ..
    } = do_expr
    else {
        panic!("binary operator should lower to function application");
    };

    assert!(matches!(outer_arg.as_ref(), Expr::Literal(_)));
    let Expr::Apply {
        callee: inner_callee,
        argument: inner_arg,
        ..
    } = outer_callee.as_ref()
    else {
        panic!("binary operator should lower to nested function application");
    };

    assert!(matches!(inner_arg.as_ref(), Expr::Literal(_)));
    match inner_callee.as_ref() {
        Expr::Name(ResolvedName::Global(path)) => {
            assert_eq!(path.text(), "demo::[+]");
        }
        other => panic!("expected operator lookup via bracketed identifier, got {other:?}"),
    }
}

#[test]
fn unary_negation_uses_tilde_bracketed_identifier() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "unary_negate.hc".to_owned(),
        ["bundle demo", "let [~] = fn x => x", "do -1"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let do_expr = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Let {
                kind:
                    LetStatementKind::PatternBinding {
                        pattern: Pattern::Hole { .. },
                        value,
                    },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .expect("missing lowered do-as-let statement");

    let Expr::Apply {
        callee, argument, ..
    } = do_expr
    else {
        panic!("unary negation should lower to function application");
    };

    assert!(matches!(argument.as_ref(), Expr::Literal(_)));
    match callee.as_ref() {
        Expr::Name(ResolvedName::Global(path)) => {
            assert_eq!(path.text(), "demo::[~]");
        }
        other => panic!("expected negation operator to resolve as [~], got {other:?}"),
    }
}

#[test]
fn lowered_integer_natural_and_real_literals_are_typed() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "literal_values.hc".to_owned(),
        [
            "bundle demo",
            "let i = 1_024",
            "let n = 0xFFn",
            "let r = 1_000.5",
            "let b = true",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let i = lowered_binding_expr_by_name(root, "demo::i").expect("missing i binding");
    let n = lowered_binding_expr_by_name(root, "demo::n").expect("missing n binding");
    let r = lowered_binding_expr_by_name(root, "demo::r").expect("missing r binding");
    let b = lowered_binding_expr_by_name(root, "demo::b").expect("missing b binding");

    let Expr::Literal(i_literal) = i else {
        panic!("expected i to lower to literal, got {i:?}");
    };
    let Expr::Literal(n_literal) = n else {
        panic!("expected n to lower to literal, got {n:?}");
    };
    let Expr::Literal(r_literal) = r else {
        panic!("expected r to lower to literal, got {r:?}");
    };
    let Expr::Literal(b_literal) = b else {
        panic!("expected b to lower to literal, got {b:?}");
    };

    expect_integer_literal(i_literal, 1024);
    expect_natural_literal(n_literal, 255);
    expect_real_literal(r_literal, 1000.5);
    expect_bool_literal(b_literal, true);
}

#[test]
fn bool_literals_lower_to_actual_boolean_values() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bool_literals.hc".to_owned(),
        ["bundle demo", "let yes = true", "let no = false"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let yes = lowered_binding_expr_by_name(root, "demo::yes").expect("missing yes binding");
    let no = lowered_binding_expr_by_name(root, "demo::no").expect("missing no binding");

    let Expr::Literal(yes_literal) = yes else {
        panic!("expected yes to lower to literal, got {yes:?}");
    };
    let Expr::Literal(no_literal) = no else {
        panic!("expected no to lower to literal, got {no:?}");
    };

    expect_bool_literal(yes_literal, true);
    expect_bool_literal(no_literal, false);
}

#[test]
fn format_string_literals_lower_to_text_and_placeholder_segments() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "format_string_segments.hc".to_owned(),
        [
            "bundle demo",
            "let greeting = `hi {}, you have {} messages`",
            "let escaped = `{{ {} }}`",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let greeting =
        lowered_binding_expr_by_name(root, "demo::greeting").expect("missing greeting binding");
    let escaped =
        lowered_binding_expr_by_name(root, "demo::escaped").expect("missing escaped binding");

    let Expr::Literal(greeting_literal) = greeting else {
        panic!("expected greeting to lower to literal, got {greeting:?}");
    };
    let Expr::Literal(escaped_literal) = escaped else {
        panic!("expected escaped to lower to literal, got {escaped:?}");
    };

    expect_format_string_literal(
        greeting_literal,
        &[
            FormatStringSegment::Text("hi ".to_owned()),
            FormatStringSegment::Placeholder,
            FormatStringSegment::Text(", you have ".to_owned()),
            FormatStringSegment::Placeholder,
            FormatStringSegment::Text(" messages".to_owned()),
        ],
    );
    expect_format_string_literal(
        escaped_literal,
        &[
            FormatStringSegment::Text("{ ".to_owned()),
            FormatStringSegment::Placeholder,
            FormatStringSegment::Text(" }".to_owned()),
        ],
    );
}

#[test]
fn underscore_type_expression_lowers_to_hole() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "type_hole.hc".to_owned(),
        ["bundle demo", "type ~T = _"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let ty_value = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Type {
                kind: TypeStatementKind::Alias { value },
                ..
            } = statement
            {
                Some(value)
            } else {
                None
            }
        })
        .expect("missing lowered type alias");

    assert!(matches!(ty_value, TypeExpr::Hole { .. }));
}

#[test]
fn kind_annotations_and_type_lambdas_are_preserved_in_lowered_ir() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "lower_type_lambdas.hc".to_owned(),
        [
            "bundle demo",
            "type Option :: Type -> Type = fn a => | Some a | None",
            "type ~Compose = fn (f :: Type -> Type) (g :: Type -> Type) a => f (g a)",
            "type ~Poly = for a (f :: Type -> Type) in f a -> f a",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let Statement::Type {
        declared_kind: Some(KindExpr::Arrow { .. }),
        kind: TypeStatementKind::Nominal { definition },
        ..
    } = &root.statements[0]
    else {
        panic!("expected lowered nominal type with declared kind");
    };
    let TypeDefinition::Lambda { params, .. } = definition else {
        panic!("expected lowered nominal definition lambda");
    };
    assert_eq!(params.len(), 1);
    assert!(params[0].kind_annotation.is_none());

    let Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Lambda { params, .. },
            },
        ..
    } = &root.statements[1]
    else {
        panic!("expected lowered alias lambda");
    };
    assert_eq!(params.len(), 3);
    assert!(matches!(
        params[0].kind_annotation,
        Some(KindExpr::Arrow { .. })
    ));
    assert!(matches!(
        params[1].kind_annotation,
        Some(KindExpr::Arrow { .. })
    ));
    assert!(params[2].kind_annotation.is_none());

    let Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Forall { params, .. },
            },
        ..
    } = &root.statements[2]
    else {
        panic!("expected lowered forall alias");
    };
    assert_eq!(params.len(), 2);
    assert!(params[0].kind_annotation.is_none());
    assert!(matches!(
        params[1].kind_annotation,
        Some(KindExpr::Arrow { .. })
    ));
}

#[test]
fn anonymous_record_type_exprs_are_preserved_in_lowered_ir() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "lower_record_type_exprs.hc".to_owned(),
        [
            "bundle demo",
            "type Box = fn a => {value: a}",
            "type ~Pair = {left: _, right: _}",
            "let value : {inner: {item: _}, ..{extra: _}} = ()",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics.is_empty(),
        "expected no lowering diagnostics, got: {diagnostics:?}"
    );

    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let Statement::Type {
        kind: TypeStatementKind::Nominal { definition },
        ..
    } = &root.statements[0]
    else {
        panic!("expected first statement to be a nominal type");
    };
    let TypeDefinition::Lambda { body, .. } = definition else {
        panic!("expected nominal record type to keep its lambda wrapper");
    };
    assert!(matches!(body.as_ref(), TypeDefinition::Struct { .. }));

    let Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Record { members, .. },
            },
        ..
    } = &root.statements[1]
    else {
        panic!("expected second statement to be a record type alias");
    };
    assert_eq!(members.len(), 2);
    assert!(matches!(members[0], RecordTypeMember::Field { .. }));
    assert!(matches!(members[1], RecordTypeMember::Field { .. }));

    let Statement::Let {
        kind:
            LetStatementKind::PatternBinding {
                pattern:
                    Pattern::Annotated {
                        ty: TypeExpr::Record { members, .. },
                        ..
                    },
                ..
            },
        ..
    } = &root.statements[2]
    else {
        panic!("expected third statement to be an annotated binding with a record type");
    };
    assert_eq!(members.len(), 2);
    assert!(matches!(
        &members[0],
        RecordTypeMember::Field {
            ty: TypeExpr::Record { .. },
            ..
        }
    ));
    assert!(matches!(
        &members[1],
        RecordTypeMember::Spread {
            ty: TypeExpr::Record { .. },
            ..
        }
    ));
}

#[test]
fn malformed_kind_annotations_emit_lower_diagnostics() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "lower_bad_kinds.hc".to_owned(),
        [
            "bundle demo",
            "type ~BadFor = for a :: Type in a",
            "type ~BadKind = fn (f :: Type ->) => f",
        ]
        .join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(diagnostics.iter().any(|diag| {
        diag.message
            .contains("expected `in` in forall type expression")
    }));
    assert!(
        diagnostics
            .iter()
            .any(|diag| { diag.message.contains("expected kind expression") })
    );
}

#[test]
fn underscore_is_rejected_as_module_name() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_module_name.hc".to_owned(),
        ["bundle demo", "module _ =", "end"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("`_` is not a valid module name"))
    );
}

#[test]
fn underscore_is_rejected_as_trait_name() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_trait_name.hc".to_owned(),
        ["bundle demo", "trait _ =", "end"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("`_` is not a valid trait name"))
    );
}

#[test]
fn unresolved_trait_alias_path_emits_resolution_diagnostic() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_trait_alias_path.hc".to_owned(),
        ["bundle demo", "trait ~Alias = _::Missing"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    let unresolved_count = diagnostics
        .iter()
        .filter(|diag| diag.message.contains("failed to resolve trait path"))
        .count();
    assert_eq!(
        unresolved_count, 1,
        "expected one unresolved trait path diagnostic, got diagnostics: {diagnostics:?}"
    );
}

#[test]
fn underscore_is_rejected_as_term_expression_name() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_term_name.hc".to_owned(),
        ["bundle demo", "do _"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("`_` is not a valid term name"))
    );
}

#[test]
fn underscore_pattern_lowers_to_hole_and_does_not_bind() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "hole_pattern.hc".to_owned(),
        ["bundle demo", "let _ = 1", "let x = 2"].join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    assert!(matches!(
        root.statements.first(),
        Some(Statement::Let {
            kind: LetStatementKind::PatternBinding {
                pattern: Pattern::Hole { .. },
                ..
            },
            ..
        })
    ));
    assert!(!root.exports.terms.iter().any(|term| term.name == "_"));
}

#[test]
fn record_pattern_open_marker_is_preserved() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "record_pattern_open.hc".to_owned(),
        [
            "bundle demo",
            "let {x} = {x = 1}",
            "let {x, ..} = {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let mut record_pattern_open_flags = Vec::new();
    for statement in &root.statements {
        if let Statement::Let {
            kind: LetStatementKind::PatternBinding { pattern, .. },
            ..
        } = statement
            && let Pattern::Record { open, .. } = pattern
        {
            record_pattern_open_flags.push(*open);
        }
    }

    assert_eq!(record_pattern_open_flags, vec![false, true]);
}

#[test]
fn wasm_module_scope_resolves_forward_function_references() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "wasm_forward_ref.hc".to_owned(),
        [
            "bundle demo",
            "wasm => (",
            "  (func $main call $helper)",
            "  (func $helper)",
            ")",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let declarations = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Wasm { declarations, .. } = statement {
                Some(declarations)
            } else {
                None
            }
        })
        .expect("missing wasm statement");

    let WasmTopLevelDeclaration::Function(main_fn) = declarations
        .first()
        .expect("expected first wasm declaration")
    else {
        panic!("expected first wasm declaration to be a function");
    };

    assert_eq!(
        main_fn.instructions.first().map(|node| &node.instruction),
        Some(&wasm::Instruction::Call(1))
    );
    assert_eq!(
        main_fn.instructions.first().map(|node| node.symbols.len()),
        Some(1)
    );
    let symbol = &main_fn.instructions[0].symbols[0];
    assert_eq!(symbol.symbol, "$helper");
    assert_eq!(symbol.resolved_index, 1);
}

#[test]
fn inline_wasm_local_declarations_are_order_sensitive() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "inline_wasm_locals.hc".to_owned(),
        [
            "bundle demo",
            "let value = (wasm local.get $x (local $x i32) local.get $x)",
        ]
        .join("\n"),
    );

    let lowered = lower_text::<TestResolver>(&db, source);
    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message.contains("unresolved wasm local binding `$x`"))
    );

    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");

    let inline_wasm = root
        .statements
        .iter()
        .find_map(|statement| {
            let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            else {
                return None;
            };
            if let Expr::InlineWasm {
                locals,
                instructions,
                ..
            } = value
            {
                Some((locals, instructions))
            } else {
                None
            }
        })
        .expect("missing lowered inline wasm expression");

    let (locals, instructions) = inline_wasm;
    assert_eq!(locals.len(), 1);
    assert_eq!(instructions.len(), 2);
    assert_eq!(instructions[0].instruction, wasm::Instruction::Unreachable);
    assert_eq!(instructions[1].instruction, wasm::Instruction::LocalGet(0));
    assert_eq!(
        instructions[1].symbols.first().map(|s| s.symbol.as_str()),
        Some("$x")
    );
}

#[test]
fn wasm_symbolic_operands_require_dollar_prefix() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "wasm_dollar_refs.hc".to_owned(),
        ["bundle demo", "let value = (wasm call helper)"].join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(diagnostics.iter().any(|diag| {
        diag.message
            .contains("wasm function references must be numeric indices or bare `$ident` symbols")
    }));

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");
    let inline_wasm = root
        .statements
        .iter()
        .find_map(|statement| {
            let Statement::Let {
                kind: LetStatementKind::PatternBinding { value, .. },
                ..
            } = statement
            else {
                return None;
            };
            if let Expr::InlineWasm { instructions, .. } = value {
                Some(instructions)
            } else {
                None
            }
        })
        .expect("missing inline wasm expression");
    assert_eq!(inline_wasm[0].instruction, wasm::Instruction::Unreachable);
}

#[test]
fn wasm_top_level_function_name_is_optional() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "wasm_unnamed_func.hc".to_owned(),
        [
            "bundle demo",
            "wasm => (",
            "  (func (param $x i32) local.get $x)",
            ")",
        ]
        .join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(!diagnostics.iter().any(|diag| {
        diag.message
            .contains("wasm function bindings must be `$` prefixed")
    }));

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");
    let declarations = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Wasm { declarations, .. } = statement {
                Some(declarations)
            } else {
                None
            }
        })
        .expect("missing wasm statement");

    let WasmTopLevelDeclaration::Function(function_decl) =
        declarations.first().expect("expected function declaration")
    else {
        panic!("expected function declaration");
    };
    assert!(function_decl.binding.is_none());
    assert_eq!(function_decl.params.len(), 1);
    assert_eq!(
        function_decl.instructions[0].instruction,
        wasm::Instruction::LocalGet(0)
    );
}

#[test]
fn duplicate_wasm_function_bindings_keep_first_resolution() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "wasm_duplicate_bindings.hc".to_owned(),
        [
            "bundle demo",
            "wasm => (",
            "  (func $a)",
            "  (func $a)",
            "  (func $caller call $a)",
            ")",
        ]
        .join("\n"),
    );

    let diagnostics = lower_diagnostics::<TestResolver>(&db, source);
    assert!(diagnostics.iter().any(|diag| {
        diag.message
            .contains("duplicate wasm function binding `$a`")
    }));

    let lowered = lower_text::<TestResolver>(&db, source);
    let root = lowered
        .modules
        .iter()
        .find(|module| module.path.text() == "demo")
        .expect("missing root lowered module");
    let declarations = root
        .statements
        .iter()
        .find_map(|statement| {
            if let Statement::Wasm { declarations, .. } = statement {
                Some(declarations)
            } else {
                None
            }
        })
        .expect("missing wasm statement");
    let WasmTopLevelDeclaration::Function(caller) =
        declarations.get(2).expect("expected caller declaration")
    else {
        panic!("expected third declaration to be function");
    };

    assert_eq!(
        caller.instructions[0].instruction,
        wasm::Instruction::Call(0)
    );
    assert_eq!(caller.instructions[0].symbols[0].resolved_index, 0);
}
