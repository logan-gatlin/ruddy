use salsa::Setter as _;
use semver::{Version, VersionReq};

use crate::engine::{Eng, Source};
use crate::parser::token::{Keyword, Operator, Punct, TokenKind};
use crate::parser::{AstVisitor, BundleDependencySource, lex_diagnostics};
use crate::reporting::{DiagnosticSeverity, TextSize};

use super::{
    Expr, KindExpr, LetStatementKind, NameRef, PathRoot, Pattern, RecordTypeMember, Statement,
    TypeDefinition, TypeExpr, TypeStatementKind, lex_source, parse_diagnostics, parse_source,
    parse_text,
};

#[test]
fn lex_query_tracks_source_contents() {
    let mut db = Eng::default();
    let source = Source::new(&db, "test.hc".to_owned(), "let x = 1".to_owned());

    let first = lex_source(&db, source);
    assert_eq!(
        first
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        vec![
            TokenKind::Keyword(Keyword::Let),
            TokenKind::Ident,
            TokenKind::Punct(Punct::Equals),
            TokenKind::IntegerLiteral,
            TokenKind::EndOfFile,
        ]
    );

    source.set_contents(&mut db).to("let x = 10".to_owned());

    let second = lex_source(&db, source);
    assert_eq!(second.tokens.len(), 5);
}

#[test]
fn lex_query_allows_kebab_case_identifiers() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "kebab_case.hc".to_owned(),
        "let foo-bar = baz-qux".to_owned(),
    );

    let lexed = lex_source(&db, source);
    assert_eq!(
        lexed
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        vec![
            TokenKind::Keyword(Keyword::Let),
            TokenKind::Ident,
            TokenKind::Punct(Punct::Equals),
            TokenKind::Ident,
            TokenKind::EndOfFile,
        ]
    );

    assert!(lex_diagnostics(&db, source).is_empty());
}

#[test]
fn lex_query_treats_edge_hyphens_as_minus_tokens() {
    let db = Eng::default();
    let source = Source::new(&db, "kebab_edges.hc".to_owned(), "-foo foo-".to_owned());

    let lexed = lex_source(&db, source);
    assert_eq!(
        lexed
            .tokens
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>(),
        vec![
            TokenKind::Operator(Operator::Minus),
            TokenKind::Ident,
            TokenKind::Ident,
            TokenKind::Operator(Operator::Minus),
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn parse_query_depends_on_lex_query() {
    let mut db = Eng::default();
    let source = Source::new(&db, "test.hc".to_owned(), "bundle demo".to_owned());

    let first = parse_source(&db, source);
    assert_eq!(first.tokens.len(), 3);
    assert_eq!(first.ast.statements.len(), 1);

    source.set_contents(&mut db).to(String::new());

    let second = parse_source(&db, source);
    assert_ne!(first.ast.range, second.ast.range);
    assert_eq!(second.ast.statements.len(), 0);
}

#[test]
fn parse_query_sets_bundle_name_from_top_declaration() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_name.hc".to_owned(),
        "bundle demo\nlet value = 1".to_owned(),
    );

    let parsed = parse_source(&db, source);
    let declared_name = match parsed.ast.statements.first() {
        Some(Statement::Bundle { name, .. }) => name.clone(),
        _ => None,
    };

    assert_eq!(parsed.ast.bundle_name, declared_name);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing normalized bundle metadata");
    assert_eq!(metadata.version, Version::new(0, 0, 0));
    assert!(metadata.raw.is_none());
    assert!(metadata.dependencies.is_empty());
    assert!(metadata.metadata.is_empty());
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_reports_non_top_bundle_declaration() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_position.hc".to_owned(),
        [
            "bundle demo",
            "let value = 1",
            "bundle later",
            "module M =",
            "bundle nested",
            "end",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    let declared_name = match parsed.ast.statements.first() {
        Some(Statement::Bundle { name, .. }) => name.clone(),
        _ => None,
    };

    assert_eq!(parsed.ast.bundle_name, declared_name);

    let diagnostics = parse_diagnostics(&db, source);
    let misplaced_bundle_diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .message
                .contains("bundle declaration must be the first statement in the file")
        })
        .count();
    assert_eq!(misplaced_bundle_diagnostics, 2);
}

#[test]
fn parse_query_bundle_with_metadata_populates_ast_file_metadata() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata.hc".to_owned(),
        [
            "bundle demo with (",
            "(version \"1.2.3\")",
            "(dependencies",
            "(dep core \"^1.0\")",
            "(dep util \"~2.4\" (path \"../util\"))",
            "(dep auth \">=1,<2\" (git \"https://example/repo.git\"))",
            ")",
            ")",
            "let value = 1",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(1, 2, 3));
    assert!(metadata.raw.is_some());
    assert_eq!(metadata.dependencies.len(), 3);
    assert_eq!(metadata.dependencies[0].name, "core");
    assert_eq!(
        metadata.dependencies[0].version,
        VersionReq::parse("^1.0").expect("failed to parse semver requirement")
    );
    assert_eq!(
        metadata.dependencies[0].source,
        BundleDependencySource::Managed
    );
    assert_eq!(metadata.dependencies[1].name, "util");
    assert_eq!(
        metadata.dependencies[1].source,
        super::BundleDependencySource::Path("../util".to_owned())
    );
    assert_eq!(metadata.dependencies[2].name, "auth");
    assert_eq!(
        metadata.dependencies[2].source,
        super::BundleDependencySource::Git("https://example/repo.git".to_owned())
    );
    assert!(metadata.metadata.is_empty());
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_bundle_with_missing_version_reports_error_and_defaults() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_missing_version.hc".to_owned(),
        "bundle demo with ((dependencies (dep core)))".to_owned(),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(0, 0, 0));
    assert_eq!(metadata.dependencies.len(), 0);
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle metadata requires `(version \"...\")`")
    }));
}

#[test]
fn parse_query_bundle_with_invalid_version_reports_error_and_defaults() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_invalid_version.hc".to_owned(),
        "bundle demo with ((version 123))".to_owned(),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(0, 0, 0));
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle metadata `version` value must be a string literal")
    }));
}

#[test]
fn parse_query_bundle_with_unparseable_semver_version_reports_error_and_defaults() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_unparseable_version.hc".to_owned(),
        "bundle demo with ((version \"not-semver\"))".to_owned(),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(0, 0, 0));
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle metadata `version` must be a valid semver version")
    }));
}

#[test]
fn parse_query_bundle_with_unknown_metadata_key_emits_warning() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_unknown_key.hc".to_owned(),
        "bundle demo with ((version \"1.0.0\") (tooling true))".to_owned(),
    );

    let diagnostics = parse_diagnostics(&db, source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == DiagnosticSeverity::Warning
            && diagnostic
                .message
                .contains("unknown bundle metadata key `tooling`")
    }));
}

#[test]
fn parse_query_bundle_with_duplicate_metadata_entries_merges_with_warnings() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_duplicates.hc".to_owned(),
        [
            "bundle demo with (",
            "(version \"1.0.0\")",
            "(dependencies (dep core \"^1.0\"))",
            "(version \"2.0.0\")",
            "(dependencies (dep util \"~2.4\"))",
            "(metadata (ci true))",
            "(metadata (owner \"team\"))",
            ")",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(2, 0, 0));
    assert_eq!(metadata.dependencies.len(), 2);
    assert_eq!(metadata.metadata.len(), 2);

    let diagnostics = parse_diagnostics(&db, source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == DiagnosticSeverity::Warning
            && diagnostic
                .message
                .contains("duplicate `version` metadata entry")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == DiagnosticSeverity::Warning
            && diagnostic
                .message
                .contains("duplicate `dependencies` metadata entry")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == DiagnosticSeverity::Warning
            && diagnostic
                .message
                .contains("duplicate `metadata` metadata entry")
    }));
}

#[test]
fn parse_query_uses_top_bundle_metadata_when_later_bundle_declarations_exist() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_top_only.hc".to_owned(),
        [
            "bundle demo with ((version \"1.0.0\"))",
            "let value = 1",
            "bundle later with ((version \"9.9.9\"))",
            "module M =",
            "bundle nested with ((version \"8.8.8\"))",
            "end",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");
    assert_eq!(metadata.version, Version::new(1, 0, 0));

    let diagnostics = parse_diagnostics(&db, source);
    let misplaced_bundle_diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .message
                .contains("bundle declaration must be the first statement in the file")
        })
        .count();
    assert_eq!(misplaced_bundle_diagnostics, 2);
}

#[test]
fn parse_query_malformed_dependencies_are_reported_and_dropped() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bundle_metadata_bad_dependencies.hc".to_owned(),
        [
            "bundle demo with (",
            "(version \"1.0.0\")",
            "(dependencies",
            "(dep missing-version)",
            "(dep bad-req \"not-a-requirement\")",
            "(dep bad-source \"^1.0\" (svn \"https://example.com/repo\"))",
            "(dep bad-path \"^1.0\" (path 123))",
            "(dep too-many \"^1.0\" (path \"../a\") (git \"https://example.com/repo\"))",
            "(dep good \"^1.2\" (git \"https://example.com/ok\"))",
            ")",
            ")",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    let metadata = parsed
        .ast
        .bundle_metadata
        .as_ref()
        .expect("missing parsed bundle metadata");

    assert_eq!(metadata.dependencies.len(), 1);
    let dependency = &metadata.dependencies[0];
    assert_eq!(dependency.name, "good");
    assert_eq!(
        dependency.version,
        VersionReq::parse("^1.2").expect("failed to parse semver requirement")
    );
    assert_eq!(
        dependency.source,
        super::BundleDependencySource::Git("https://example.com/ok".to_owned())
    );

    let diagnostics = parse_diagnostics(&db, source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle dependency entry must have shape")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle dependency version requirement must be a valid semver requirement")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle dependency source must be `path` or `git`")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle dependency `path` source value must be a string literal")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle dependency entry has too many arguments")
    }));
}

#[test]
fn parse_query_keeps_bundle_name_none_without_top_declaration() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "no_bundle_root.hc".to_owned(),
        "let value = 1\nbundle late".to_owned(),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.bundle_name, None);
    assert!(parsed.ast.bundle_metadata.is_none());
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("bundle declaration must be the first statement in the file")
    }));
}

#[test]
fn parse_query_handles_phase_three_declarations() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "demo.hc".to_owned(),
        [
            "bundle demo",
            "type Option = fn a => | Some a | None",
            "trait Eq : a = let eq : a -> a -> bool end",
            "impl Eq Option a = let eq = fn x y => true type Item = bool end",
            "let Some value : Option a = input",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 5);
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_accepts_kind_annotations_and_type_lambdas() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "type_lambdas.hc".to_owned(),
        [
            "bundle demo",
            "type Option :: Type -> Type = fn a => | Some a | None",
            "type ~Compose = fn (f :: Type -> Type) (g :: Type -> Type) a => f (g a)",
            "type ~Poly = for a (f :: Type -> Type) in f a -> f a",
            "type ~Applied = (fn a => a) []",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert!(parse_diagnostics(&db, source).is_empty());

    let Some(Statement::Type {
        declared_kind: Some(KindExpr::Arrow { .. }),
        kind: TypeStatementKind::Nominal { definition },
        ..
    }) = parsed.ast.statements.get(1)
    else {
        panic!("expected second statement to be a nominal type with a declared kind");
    };

    let TypeDefinition::Lambda { params, .. } = definition else {
        panic!("expected nominal definition lambda");
    };
    assert_eq!(params.len(), 1);
    assert!(params[0].kind.is_none());

    let Some(Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Lambda { params, .. },
            },
        ..
    }) = parsed.ast.statements.get(2)
    else {
        panic!("expected third statement to be a type lambda alias");
    };

    assert_eq!(params.len(), 3);
    assert!(matches!(params[0].kind, Some(KindExpr::Arrow { .. })));
    assert!(matches!(params[1].kind, Some(KindExpr::Arrow { .. })));
    assert!(params[2].kind.is_none());

    let Some(Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Forall { params, .. },
            },
        ..
    }) = parsed.ast.statements.get(3)
    else {
        panic!("expected fourth statement to be a forall alias");
    };
    assert_eq!(params.len(), 2);
    assert!(params[0].kind.is_none());
    assert!(matches!(params[1].kind, Some(KindExpr::Arrow { .. })));

    let Some(Statement::Type {
        kind: TypeStatementKind::Alias {
            value: TypeExpr::Apply { .. },
        },
        ..
    }) = parsed.ast.statements.get(4)
    else {
        panic!("expected fifth statement to be a type lambda application");
    };
}

#[test]
fn parse_query_accepts_anonymous_record_type_exprs() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "record_type_exprs.hc".to_owned(),
        [
            "bundle demo",
            "type ~Pair = {left: _, right: _}",
            "let value : {inner: {item: _}, ..{extra: _}} = ()",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert!(parse_diagnostics(&db, source).is_empty());

    let Some(Statement::Type {
        kind:
            TypeStatementKind::Alias {
                value: TypeExpr::Record { members, .. },
            },
        ..
    }) = parsed.ast.statements.get(1)
    else {
        panic!("expected second statement to be a record type alias");
    };
    assert_eq!(members.len(), 2);
    assert!(matches!(members[0], RecordTypeMember::Field { .. }));
    assert!(matches!(members[1], RecordTypeMember::Field { .. }));

    let Some(Statement::Let {
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
    }) = parsed.ast.statements.get(2)
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
fn parse_query_rejects_unparenthesized_annotated_binders() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_unparenthesized_binders.hc".to_owned(),
        [
            "bundle demo",
            "type ~BadFor = for a :: Type in a",
            "type ~BadFn = fn a :: Type => a",
        ]
        .join("\n"),
    );

    let diagnostics = parse_diagnostics(&db, source);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("expected `in` in forall type expression")
    }));
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("expected `=>` in type lambda") })
    );
}

#[test]
fn parse_query_rejects_malformed_kind_expressions() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "bad_kind_exprs.hc".to_owned(),
        [
            "bundle demo",
            "type Option :: Type -> = fn a => | Some a | None",
            "type ~Bad = fn (f :: (Type ->)) => f",
        ]
        .join("\n"),
    );

    let diagnostics = parse_diagnostics(&db, source);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("expected kind expression") })
    );
}

#[test]
fn parse_query_reports_missing_end() {
    let db = Eng::default();
    let source = Source::new(&db, "demo.hc".to_owned(), "module M = let x = 1".to_owned());

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 1);
    assert!(
        parse_diagnostics(&db, source)
            .iter()
            .any(|diagnostic| diagnostic.message.contains("expected `end`"))
    );
}

#[test]
fn parse_query_handles_phase_four_expressions() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "exprs.hc".to_owned(),
        [
            "bundle demo",
            "let value = let x = 1 in if x == 1 then fn y => y + 1 else match x with | n => n",
            "let f = fn | 0 => 1 | n => n",
            "do f value.y; g (h i)",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 4);
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_record_patterns_track_open_marker() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "record_patterns.hc".to_owned(),
        [
            "bundle demo",
            "let {x} = {x = 1}",
            "let {x, ..} = {x = 1, y = true}",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert!(parse_diagnostics(&db, source).is_empty());

    let closed_open = match parsed.ast.statements.get(1) {
        Some(Statement::Let {
            kind: LetStatementKind::PatternBinding { pattern, .. },
            ..
        }) => match pattern {
            Pattern::Record { open, .. } => *open,
            other => panic!("expected record pattern, got {other:?}"),
        },
        other => panic!("expected let statement, got {other:?}"),
    };
    assert!(!closed_open);

    let open_open = match parsed.ast.statements.get(2) {
        Some(Statement::Let {
            kind: LetStatementKind::PatternBinding { pattern, .. },
            ..
        }) => match pattern {
            Pattern::Record { open, .. } => *open,
            other => panic!("expected record pattern, got {other:?}"),
        },
        other => panic!("expected let statement, got {other:?}"),
    };
    assert!(open_open);
}

#[test]
fn parse_query_handles_inline_wasm_forms() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "wasm.hc".to_owned(),
        [
            "bundle demo",
            "let add_one = (wasm : i32) => (i32.add (local.get $x) 1)",
            "wasm => ((func $f (param $x i32) (result i32) local.get $x))",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 3);
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_reports_non_associative_comparison_chain() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "cmp.hc".to_owned(),
        "bundle demo\nlet bad = 1 < 2 < 3".to_owned(),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 2);
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("comparison operators are non-associative")
    }));
}

#[test]
fn parse_text_api_matches_direct_query_result() {
    let db = Eng::default();
    let contents = "bundle demo\nlet x = 1\ndo x".to_owned();

    let source = Source::new(&db, "api.hc".to_owned(), contents.clone());
    let via_api = parse_text(&db, source);
    let via_query = parse_source(&db, source);

    assert_eq!(via_api, via_query);
}

#[test]
fn parser_outputs_ranges_with_source_origin() {
    let db = Eng::default();
    let source = Source::new(&db, "origin.hc".to_owned(), "bundle demo\nlet =".to_owned());

    let lexed = lex_source(&db, source);
    assert!(
        lexed
            .tokens
            .iter()
            .all(|token| token.range.source() == Some(source))
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.range.source(), Some(source));

    let lex_diags = lex_diagnostics(&db, source);
    assert!(
        lex_diags
            .iter()
            .all(|diagnostic| diagnostic.range.source() == Some(source))
    );

    let parse_diags = parse_diagnostics(&db, source);
    assert!(
        parse_diags
            .iter()
            .all(|diagnostic| diagnostic.range.source() == Some(source))
    );
}

#[test]
fn parse_query_conforms_to_broad_program_shape() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "conformance.hc".to_owned(),
        [
            "bundle demo",
            "module m in \"m.hc\"",
            "module Main =",
            "use root::std::core as Core",
            "type Option = fn a => | Some a | None",
            "type ~Pair = fn a b => (a, b)",
            "trait Show : a = let show : a -> [] end",
            "trait ~Display = root::std::core::Show",
            "impl Show Option a = let show = fn v => v type Item = [] end",
            "let value = use root::std::core::id as id in id 1",
            "do match value with | x => x",
            "end",
            "wasm => ()",
            "let entry = if true then 1 else 0",
            "do entry",
        ]
        .join("\n"),
    );

    let parsed = parse_text(&db, source);

    assert_eq!(parsed.ast.statements.len(), 6);
    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_accepts_bundle_use_shorthand() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "use_bundle_shorthand.hc".to_owned(),
        [
            "bundle demo",
            "use bundle",
            "let value = use bundle as Root in Root::std::core",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);

    let Some(Statement::Use {
        target: Some(NameRef::Path(path)),
        ..
    }) = parsed.ast.statements.get(1)
    else {
        panic!("expected module-level `use bundle` statement");
    };
    assert_eq!(path.root, PathRoot::Bundle);
    assert!(path.segments.is_empty());

    let Some(Statement::Let {
        kind:
            LetStatementKind::PatternBinding {
                value:
                    Expr::Use {
                        target: Some(NameRef::Path(path)),
                        ..
                    },
                ..
            },
        ..
    }) = parsed.ast.statements.get(2)
    else {
        panic!("expected expression-level `use bundle` expression");
    };
    assert_eq!(path.root, PathRoot::Bundle);
    assert!(path.segments.is_empty());

    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_keeps_bundle_prefixed_use_paths() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "use_bundle_prefixed_path.hc".to_owned(),
        [
            "bundle demo",
            "use bundle::M as RootMod",
            "let value = use bundle::M as LocalMod in LocalMod::value",
        ]
        .join("\n"),
    );

    let parsed = parse_source(&db, source);

    let Some(Statement::Use {
        target: Some(NameRef::Path(path)),
        ..
    }) = parsed.ast.statements.get(1)
    else {
        panic!("expected module-level `use bundle::M` statement");
    };
    assert_eq!(path.root, PathRoot::Bundle);
    assert_eq!(path.segments.len(), 1);

    let Some(Statement::Let {
        kind:
            LetStatementKind::PatternBinding {
                value:
                    Expr::Use {
                        target: Some(NameRef::Path(path)),
                        ..
                    },
                ..
            },
        ..
    }) = parsed.ast.statements.get(2)
    else {
        panic!("expected expression-level `use bundle::M` expression");
    };
    assert_eq!(path.root, PathRoot::Bundle);
    assert_eq!(path.segments.len(), 1);

    assert!(parse_diagnostics(&db, source).is_empty());
}

#[test]
fn parse_query_recovers_after_lex_and_parse_errors() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "recover.hc".to_owned(),
        ["bundle demo", "let value = 1", "@", "do value"].join("\n"),
    );

    let parsed = parse_source(&db, source);
    assert_eq!(parsed.ast.statements.len(), 4);
    let mut merged = lex_diagnostics(&db, source);
    merged.extend(parse_diagnostics(&db, source));
    assert!(
        merged
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unexpected character"))
    );
    assert!(
        merged
            .iter()
            .any(|diagnostic| diagnostic.message.contains("expected statement"))
    );
}

#[test]
fn parse_query_diagnostic_snapshot_for_missing_bundle_name() {
    let db = Eng::default();
    let source = Source::new(&db, "snapshot.hc".to_owned(), "bundle".to_owned());
    let parsed = parse_text(&db, source);

    assert_eq!(parsed.ast.statements.len(), 1);
    let diagnostics = parse_diagnostics(&db, source);
    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages, vec!["expected identifier for bundle name"]);
}

#[test]
fn parse_query_incremental_round_trip_updates() {
    let mut db = Eng::default();
    let source = Source::new(
        &db,
        "incremental.hc".to_owned(),
        "bundle demo\nlet x = 1".to_owned(),
    );

    let first = parse_source(&db, source);
    assert_eq!(first.ast.statements.len(), 2);
    assert!(parse_diagnostics(&db, source).is_empty());

    source
        .set_contents(&mut db)
        .to("bundle demo\nlet x = 1 < 2 < 3".to_owned());
    let second = parse_source(&db, source);
    assert_eq!(second.ast.statements.len(), 2);
    assert!(parse_diagnostics(&db, source).iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("comparison operators are non-associative")
    }));

    source
        .set_contents(&mut db)
        .to("bundle demo\nlet x = 1".to_owned());
    let third = parse_source(&db, source);
    assert_eq!(third, first);
}

#[test]
fn parse_query_builds_typed_ast_nodes() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "typed.hc".to_owned(),
        "bundle demo\nlet value = if true then 1 else 0".to_owned(),
    );

    let parsed = parse_source(&db, source);
    assert!(matches!(
        parsed.ast.statements.get(1),
        Some(Statement::Let {
            kind: LetStatementKind::PatternBinding {
                pattern: Pattern::Name(_),
                value: Expr::If { .. },
            },
            ..
        })
    ));
}

#[test]
fn parse_query_ast_is_traversable() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "walk.hc".to_owned(),
        "bundle demo\nlet value = fn x => x + 1\ndo value".to_owned(),
    );

    let parsed = parse_source(&db, source);
    let mut statement_count = 0;
    let mut expr_count = 0;
    let mut visitor = AstVisitor::new()
        .statement(|_| statement_count += 1)
        .expr(|_| expr_count += 1);
    parsed.ast.walk(&mut visitor);
    drop(visitor);

    assert!(statement_count >= 3);
    assert!(expr_count >= 3);
}

#[test]
fn parse_query_randomized_inputs_do_not_panic() {
    let mut db = Eng::default();
    let source = Source::new(&db, "fuzz.hc".to_owned(), String::new());
    let mut state = 0x9E3779B97F4A7C15u64;

    for _ in 0..300 {
        let len = (next_u64(&mut state) % 96 + 1) as usize;
        let text = random_input(&mut state, len);
        source.set_contents(&mut db).to(text.clone());

        let parsed = parse_source(&db, source);
        assert!(!parsed.tokens.is_empty());
        assert_eq!(
            parsed.ast.range.end(),
            Some(TextSize::from_usize(text.len()))
        );
    }
}

fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state
}

fn random_input(state: &mut u64, len: usize) -> String {
    const ALPHABET: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_+-*/<>=:;,.|(){}[]$~`'\" \\n\t";

    let mut text = String::with_capacity(len);
    for _ in 0..len {
        let idx = (next_u64(state) % (ALPHABET.len() as u64)) as usize;
        text.push(ALPHABET[idx] as char);
    }
    text
}
