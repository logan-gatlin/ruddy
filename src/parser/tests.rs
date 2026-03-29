use salsa::Setter as _;

use crate::engine::{Eng, Source};
use crate::parser::lex_diagnostics;
use crate::parser::token::{Keyword, Punct, TokenKind};
use crate::reporting::TextSize;

use super::{
    AstVisitor, Expr, LetStatementKind, Pattern, Statement, lex_source, parse_diagnostics,
    parse_source, parse_text,
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
fn parse_query_handles_phase_three_declarations() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "demo.hc".to_owned(),
        [
            "bundle demo",
            "type Option : a = | Some a | None",
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
fn parse_query_conforms_to_broad_program_shape() {
    let db = Eng::default();
    let source = Source::new(
        &db,
        "conformance.hc".to_owned(),
        [
            "bundle demo",
            "import \"std/core\", \"std/math\",",
            "module Main =",
            "use root::std::core as Core",
            "type Option : a = | Some a | None",
            "type ~Pair : a b = (a, b)",
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

#[derive(Default)]
struct CountingVisitor {
    statement_count: usize,
    expr_count: usize,
}

impl AstVisitor for CountingVisitor {
    fn visit_statement(&mut self, _statement: &Statement) {
        self.statement_count += 1;
    }

    fn visit_expr(&mut self, _expr: &Expr) {
        self.expr_count += 1;
    }
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
    let mut visitor = CountingVisitor::default();
    parsed.ast.walk(&mut visitor);

    assert!(visitor.statement_count >= 3);
    assert!(visitor.expr_count >= 3);
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
        assert_eq!(parsed.ast.range.end(), TextSize::from_usize(text.len()));
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
