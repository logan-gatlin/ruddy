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
fn arrows_are_right_associative_and_looser_than_application() {
    assert_eq!(parse_one("type F = A -> B"), "type F = A -> B");
    // Right-associative, so the grouping on the right leaves no trace...
    assert_eq!(parse_one("type G = A -> (B -> C)"), "type G = A -> B -> C");
    assert_eq!(parse_one("type H = A -> B -> C"), "type H = A -> B -> C");
    // ...and the one on the left is reconstructed.
    assert_eq!(
        parse_one("type I = (A -> B) -> C"),
        "type I = (A -> B) -> C"
    );
    // Looser than application on both sides.
    assert_eq!(
        parse_one("type J = List A -> Map K V"),
        "type J = List A -> Map K V"
    );
    assert_eq!(parse_one("type K = (A -> B) C"), "type K = (A -> B) C");
    assert_eq!(parse_one("type L = F (A -> B)"), "type L = F (A -> B)");
    assert_eq!(
        parse_one("type M = { f: A -> B }"),
        "type M = { f: A -> B }"
    );
    assert_eq!(parse_one("type N = () -> ()"), "type N = () -> ()");
}

#[test]
fn arrows_and_type_lambdas_are_different_arrows() {
    // A lambda's body extends rightward over the arrow...
    assert_eq!(
        parse_one("type F = fn t => t -> t"),
        "type F = fn t => t -> t"
    );
    // ...so grouping is the only way to put one to the left of one, and the
    // printer has to put those parentheses back.
    assert_eq!(
        parse_one("type G = (fn t => t) -> A"),
        "type G = (fn t => t) -> A"
    );
    assert_eq!(
        parse_one("type H = A -> fn t => t"),
        "type H = A -> fn t => t"
    );
}

#[test]
fn projection_binds_tighter_than_application() {
    assert_eq!(parse_one("let a = p.x"), "let a = p.x");
    // Left-associative, so a chain needs no parentheses.
    assert_eq!(parse_one("let b = p.x.y"), "let b = p.x.y");
    // `f p.x` is `f (p.x)`, not `(f p).x` — which is why the second form
    // has to be written, and printed, with parentheses.
    assert_eq!(parse_one("let c = f p.x"), "let c = f p.x");
    assert_eq!(parse_one("let d = (f p).x"), "let d = (f p).x");
    assert_eq!(parse_one("let e = f p.x q.y"), "let e = f p.x q.y");
    // Any atom can be projected out of.
    assert_eq!(parse_one("let g = { x: a }.x"), "let g = { x: a }.x");
    assert_eq!(parse_one("let h = (fn p => p).x"), "let h = (fn p => p).x");
}

#[test]
fn a_projection_needs_a_field_name() {
    let out = parse(lex("let a = p.  let b = ()", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.stmts.len(), 1, "stmts: {:#?}", out.stmts);
}

/// Nothing is silently stood in for a missing expression or type: a position
/// with nothing usable in it is reported where it was written, so a truncated
/// edit cannot pass for a program that happens to mean something else.
#[test]
fn a_missing_expression_or_type_is_reported() {
    let errors = |src: &str| {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without error");
        out
    };

    // A stray dot is not a projection out of unit.
    errors("let a = .x");
    // An ascription with no type in it is not an ascription of `()`.
    errors("let b : = ()");
    // Nor is a dangling arrow an arrow to `()`.
    errors("type T = A ->  let y = ()");
}

/// Running out of input is reported too. A production that failed quietly at
/// EOF would drop the definition it was parsing and leave the run looking
/// successful; the error points at the end of the input, which is where the
/// missing piece would have gone.
#[test]
fn end_of_input_is_reported_where_it_runs_out() {
    for src in ["let a = p.", "let b :", "type T = A ->", "let c ="] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, src.len(), "{src:?}");
        assert_eq!(out.errors[0].span.width, 0, "{src:?}");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }
}

#[test]
fn a_let_may_be_ascribed_a_type() {
    assert_eq!(parse_one("let x : A = y"), "let x : A = y");
    assert_eq!(
        parse_one("let f : A -> B = fn a => a"),
        "let f : A -> B = fn a => a"
    );
    // The plan's witness for width subtyping, in one line.
    assert_eq!(
        parse_one("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"),
        "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"
    );
    // Without an ascription nothing is printed where one would go.
    assert_eq!(parse_one("let z = ()"), "let z = ()");
}

#[test]
fn only_a_let_may_be_ascribed() {
    // A `type` declaration is a type already; there is nothing to check it
    // against, so the colon is the unexpected token it looks like.
    let out = parse(lex("type T : K = ()", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "stmts: {:#?}", out.stmts);
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
