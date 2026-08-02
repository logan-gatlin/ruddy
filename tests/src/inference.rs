//! Tests for [`ruddy::inference`].

use ruddy::{
    inference::{self, ErrorKind},
    ir::{self, Decl, Term, TermKind},
    parse,
    symbol::{Bundle, Mint, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::Ty,
};

fn dummy_mint() -> Mint {
    Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
}

/// Parse, lower and infer. Only the parse is required to be clean: some tests
/// are exactly about what inference does with a program lowering complained
/// about.
fn infer_src(src: &str) -> (Mint, ir::Output, inference::Output) {
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(
        parsed.errors.is_empty(),
        "unexpected parse errors: {:#?}",
        parsed.errors
    );
    let mut mint = dummy_mint();
    let mut out = ir::build(&mut mint, parsed.stmts);
    let inferred = inference::infer(&mut out.program);
    (mint, out, inferred)
}

/// [`infer_src`] for a program that should be entirely error-free.
fn inferred(src: &str) -> (Mint, ir::Output, inference::Output) {
    let (mint, out, output) = infer_src(src);
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert!(
        output.errors.is_empty(),
        "inference errors: {:#?}",
        output.errors
    );
    (mint, out, output)
}

fn symbol_named(mint: &Mint, symbols: impl IntoIterator<Item = Symbol>, name: &str) -> Symbol {
    symbols
        .into_iter()
        .find(|symbol| mint.name(*symbol) == name)
        .unwrap_or_else(|| panic!("nothing named {name}"))
}

/// How the scheme of a top-level definition prints — which is the shape the
/// debugger shows and the reader compares against, so it is what the tests
/// assert on.
fn scheme(mint: &Mint, inferred: &inference::Output, name: &str) -> String {
    let symbol = symbol_named(mint, inferred.schemes.keys().copied(), name);
    inferred.schemes[&symbol].to_string()
}

fn term_decl<'a>(mint: &Mint, out: &'a ir::Output, name: &str) -> &'a Decl<Term> {
    let symbol = symbol_named(mint, out.program.terms.keys().copied(), name);
    &out.program.terms[&symbol]
}

#[test]
fn a_literal_is_a_nat() {
    let (mint, out, output) = inferred("let n = 1");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    // The type is written into the term itself, not just the scheme table.
    assert_eq!(term_decl(&mint, &out, "n").value.ty.to_string(), "Nat");
}

#[test]
fn the_identity_generalizes() {
    let (mint, out, output) = inferred("let id = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "'a -> 'a");

    // Zonking spells the body's types with the same letters as the scheme:
    // the lambda is 'a -> 'a and the variable inside it is 'a.
    let decl = term_decl(&mint, &out, "id");
    assert_eq!(decl.value.ty.to_string(), "'a -> 'a");
    let TermKind::Fn { body, .. } = &decl.value.kind else {
        panic!("expected a lambda");
    };
    assert_eq!(body.ty.to_string(), "'a");
}

#[test]
fn quantifiers_are_named_in_first_occurrence_order() {
    let (mint, _, output) = inferred("let apply = fn f x => f x");
    assert_eq!(scheme(&mint, &output, "apply"), "('a -> 'b) -> 'a -> 'b");

    let (mint, _, output) = inferred("let compose = fn f g x => f (g x)");
    assert_eq!(
        scheme(&mint, &output, "compose"),
        "('a -> 'b) -> ('c -> 'a) -> 'c -> 'b"
    );
}

#[test]
fn each_use_instantiates_afresh() {
    // One polymorphic definition used at two different types: the uses must
    // not constrain each other, which is the whole point of the scheme.
    let (mint, _, output) = inferred("let id = fn x => x\nlet n = id 1\nlet u = id {}");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    assert_eq!(scheme(&mint, &output, "u"), "{}");
}

#[test]
fn a_lambda_argument_stays_monomorphic() {
    // `f` is a lambda argument, so its two uses share one type — using it at
    // Nat and at {} in one body has to be a mismatch.
    let (_, _, output) = infer_src("let both = fn f => { a: f 1, b: f {} }");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Mismatch { .. })),
        "expected a mismatch: {:#?}",
        output.errors
    );
}

#[test]
fn an_annotation_is_checked_and_kept() {
    let (mint, out, output) =
        inferred("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x\nlet n = fst { x: 1, y: 2 }");
    assert_eq!(scheme(&mint, &output, "fst"), "{ x: Nat, y: Nat } -> Nat");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");

    // Checking pushed the annotation into the binder: the projection knows it
    // produced a Nat even though nothing after it demanded one.
    let decl = term_decl(&mint, &out, "fst");
    let TermKind::Fn { body, .. } = &decl.value.kind else {
        panic!("expected a lambda");
    };
    assert_eq!(body.ty.to_string(), "Nat");
}

#[test]
fn checking_reaches_into_struct_literals() {
    // The lambda sits inside a struct literal; only checking mode can hand it
    // the arrow it needs before its body projects nothing.
    inferred("let s : { f: { n: Nat } -> Nat } = { f: fn r => r.n }");
}

#[test]
fn struct_fields_unify_by_name_not_position() {
    inferred("let p : { a: Nat, b: {} } = { b: {}, a: 1 }");
}

#[test]
fn an_annotation_mismatch_is_one_error() {
    let (_, _, output) = infer_src("let n : Nat = fn x => x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let ErrorKind::Mismatch { expected, .. } = &error.kind else {
        panic!("expected a mismatch: {:#?}", error.kind);
    };
    assert_eq!(expected.to_string(), "Nat");
}

#[test]
fn a_missing_field_names_the_struct() {
    let src = "let f : { x: Nat } -> Nat = fn p => p.y";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let ErrorKind::MissingField { base } = &error.kind else {
        panic!("expected a missing field: {:#?}", error.kind);
    };
    assert_eq!(base.to_string(), "{ x: Nat }");
    // Reported at the field name, which is the only thing the user can fix.
    assert_eq!(error.span.start, src.find(".y").expect("the field") + 1);
}

#[test]
fn an_unannotated_projection_asks_for_an_annotation() {
    // Unification can only equate a variable with a type it is given, and
    // nothing here gives it one: `p.x` says the base has an `x`, not what the
    // base is. So this is an error, and the payload being a variable rather
    // than a concrete type is what tells the reporter to suggest an
    // annotation instead of naming what went wrong.
    let (_, _, output) = infer_src("let f = fn p => p.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let ErrorKind::NotAStruct { base } = &error.kind else {
        panic!("expected a projection error: {:#?}", error.kind);
    };
    assert!(matches!(**base, Ty::Var(_)), "base was {base}");
}

#[test]
fn self_application_is_recursive_not_divergent() {
    let (_, _, output) = infer_src("let w = fn x => x x");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Recursive)),
        "expected a recursive-type error: {:#?}",
        output.errors
    );
}

#[test]
fn aliases_unfold_to_their_meaning() {
    let (mint, _, output) =
        inferred("type Endo = Nat -> Nat\ntype Pair = { f: Endo }\nlet id : Endo = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "Nat -> Nat");

    let pair = symbol_named(&mint, output.aliases.keys().copied(), "Pair");
    assert_eq!(output.aliases[&pair].to_string(), "{ f: Nat -> Nat }");
}

#[test]
fn lowering_errors_are_absorbed_not_echoed() {
    // `origin` is undefined: lowering reported it, and the error term must
    // sail through inference without a second complaint.
    let (mint, out, output) = infer_src("let point = { x: origin }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        output.errors.is_empty(),
        "inference echoed a lowering error: {:#?}",
        output.errors
    );
    assert_eq!(scheme(&mint, &output, "point"), "{ x: ? }");
}
