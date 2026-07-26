//! Tests for [`ruddy::parse`].

use ruddy::{
    parse::{StmtKind, parse},
    token::lex,
    tracking::FileID,
};

#[test]
fn parses_let_and_type() {
    // `with` can neither extend the preceding type nor start a statement,
    // so it is the sole parse error and recovery resumes at `let z`.
    let src = "let x = y  type T = U  with  let z = w";
    let toks = lex(src, FileID::GENERATED).tokens;
    let out = parse(toks);
    assert_eq!(out.stmts.len(), 3, "stmts: {:#?}", out.stmts);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.stmts[0].tracked, StmtKind::Let { .. }));
    assert!(matches!(out.stmts[1].tracked, StmtKind::Type { .. }));
    assert!(matches!(out.stmts[2].tracked, StmtKind::Let { .. }));
}

/// Parse a single statement and render it back. Since grouping is dropped
/// rather than recorded, the rendered source is re-parsed to confirm the
/// printer emits enough parentheses to reproduce the same tree.
fn parse_one(src: &str) -> String {
    let printed = parse_print(src);
    assert_eq!(
        parse_print(&printed),
        printed,
        "printing {src:?} did not round-trip"
    );
    printed
}

fn parse_print(src: &str) -> String {
    let toks = lex(src, FileID::GENERATED).tokens;
    let out = parse(toks);
    assert!(
        out.errors.is_empty(),
        "unexpected errors for {src:?}: {:#?}",
        out.errors
    );
    assert_eq!(out.stmts.len(), 1, "stmts: {:#?}", out.stmts);
    out.stmts[0].tracked.to_string()
}

#[test]
fn application_is_left_associative() {
    assert_eq!(parse_one("let a = f x y"), "let a = f x y");
}

#[test]
fn struct_types() {
    assert_eq!(
        parse_one("type Point = { x: Int, y: Int }"),
        "type Point = { x: Int, y: Int }"
    );
    // Trailing comma is allowed.
    assert_eq!(parse_one("type T = { a: A, }"), "type T = { a: A }");
}

#[test]
fn functions() {
    assert_eq!(parse_one("let id = fn x => x"), "let id = fn x => x");
    // Body extends rightward over application.
    assert_eq!(
        parse_one("let k = fn a b => f a b"),
        "let k = fn a b => f a b"
    );
    // A function can be an application argument. The printer parenthesizes
    // it so the lambda body can't swallow whatever follows.
    assert_eq!(
        parse_one("let m = map fn x => x"),
        "let m = map (fn x => x)"
    );
}

#[test]
fn zero_arg_functions_are_rejected() {
    // Both term-level and type-level nullary functions are errors.
    let out = parse(lex("let z = fn => y", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);

    let out = parse(lex("type Z = fn => Y", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn struct_exprs() {
    assert_eq!(
        parse_one("let p = { x: a, y: b }"),
        "let p = { x: a, y: b }"
    );
    // Trailing comma, application values, and nesting.
    assert_eq!(
        parse_one("let q = { f: g x, inner: { z: w }, }"),
        "let q = { f: g x, inner: { z: w } }"
    );
    // A struct literal can be an application argument.
    assert_eq!(parse_one("let r = use { k: v }"), "let r = use { k: v }");
}

#[test]
fn duplicate_fields_are_allowed() {
    // The surface syntax records both; rejecting a repeated name is the
    // IR's job, so nothing is dropped or merged here.
    assert_eq!(
        parse_one("let p = { x: a, x: b }"),
        "let p = { x: a, x: b }"
    );
    assert_eq!(
        parse_one("type T = { a: A, a: B }"),
        "type T = { a: A, a: B }"
    );
}

#[test]
fn type_lambdas() {
    assert_eq!(parse_one("type F = fn a => a"), "type F = fn a => a");
    assert_eq!(
        parse_one("type G = fn t => { x: t }"),
        "type G = fn t => { x: t }"
    );
}

#[test]
fn parens_group_application() {
    // Grouping survives as tree shape, and the printer re-inserts the
    // parentheses it needs to round-trip.
    assert_eq!(parse_one("let a = f (g x)"), "let a = f (g x)");
    assert_eq!(parse_one("let b = (f g) x"), "let b = f g x");
    assert_eq!(parse_one("let c = f (g x) y"), "let c = f (g x) y");
    // Redundant parentheses leave no trace.
    assert_eq!(parse_one("let d = ((x))"), "let d = x");
    assert_eq!(parse_one("let e = (f) (x)"), "let e = f x");
}

#[test]
fn parens_group_lambdas() {
    // A lambda body extends rightward, so parenthesizing it is the only way
    // to apply one directly — and the only way to print it back.
    assert_eq!(parse_one("let a = (fn x => x) y"), "let a = (fn x => x) y");
    assert_eq!(
        parse_one("let b = f (fn x => x) y"),
        "let b = f (fn x => x) y"
    );
    // Without parentheses the lambda still swallows the rest, which the
    // printer then makes explicit.
    assert_eq!(
        parse_one("let c = f fn x => x y"),
        "let c = f (fn x => x y)"
    );
}

#[test]
fn naturals_are_atoms() {
    assert_eq!(parse_one("let n = 0"), "let n = 0");
    // An atom, so it takes part in application on either side and never
    // needs parentheses of its own.
    assert_eq!(parse_one("let a = f 1 2"), "let a = f 1 2");
    assert_eq!(parse_one("let b = 1 f"), "let b = 1 f");
    assert_eq!(parse_one("let c = f (g 3)"), "let c = f (g 3)");
    assert_eq!(parse_one("let d = fn x => 7"), "let d = fn x => 7");
    assert_eq!(
        parse_one("let e = { width: 3, height: 4 }"),
        "let e = { width: 3, height: 4 }"
    );
}

#[test]
fn naturals_are_terms_only() {
    // There is no type-level natural, so a literal in type position is the
    // unexpected token it looks like rather than a silently accepted one.
    let out = parse(lex("type T = 42", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn empty_parens_are_unit() {
    assert_eq!(parse_one("let a = ()"), "let a = ()");
    assert_eq!(parse_one("let b = f ()"), "let b = f ()");
    assert_eq!(parse_one("type T = ()"), "type T = ()");
}

#[test]
fn parens_group_types() {
    assert_eq!(
        parse_one("type M = Map (List K) V"),
        "type M = Map (List K) V"
    );
    assert_eq!(parse_one("type N = (Map K) V"), "type N = Map K V");
    assert_eq!(
        parse_one("type F = (fn a => a) T"),
        "type F = (fn a => a) T"
    );
    assert_eq!(
        parse_one("type R = { items: List (Pair A B) }"),
        "type R = { items: List (Pair A B) }"
    );
}

#[test]
fn unmatched_closing_paren_is_an_error() {
    let out = parse(lex("let x = y)  let z = w", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    // Recovery resumes at the next statement keyword.
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);
}

#[test]
fn type_application_is_left_associative() {
    assert_eq!(parse_one("type M = Map K V"), "type M = Map K V");
    // Type applications can appear as struct field types.
    assert_eq!(
        parse_one("type R = { items: List T }"),
        "type R = { items: List T }"
    );
}
