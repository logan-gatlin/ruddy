//! Tests for [`ruddy_debug::print`].

use ruddy::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};
use ruddy_debug::print;

/// One source, rendered back by both printers: as it was written, and as it
/// was lowered.
fn printed(source: &str) -> (String, String) {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    let ast = parsed
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(&stmt.tracked).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let bundle = Bundle::new("test", Version::new(0, 1, 0)).expect("the test bundle name is valid");
    let mut mint = Mint::new(bundle);
    let built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);

    (ast, print::ir::program(&built.program, &mint).to_string())
}

/// Rendering `{ name: value }` is one rule, in `print`, that both printers
/// read. It used to be a copy each, so the AST tab and the IR tab could come to
/// disagree about the same braces without anything noticing.
#[test]
fn both_trees_render_a_struct_the_same_way() {
    for source in [
        "let s = { x: 1, y: 2 }",
        "let n = { p: { q: 3 } }",
        "let v : { a: Nat } = { a: 1 }",
        "let f = fn r => { wrapped: r.x }",
        // Open rows and optional fields: the `..` tail, named or not, and the
        // `?` marker are one rendering rule too.
        "let g : { a: Nat, ..r } -> Nat = fn p => p.a",
        "let h : { a?: Nat, .. } -> Nat = fn p => p.a",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// The empty struct is the one case with no padding, because the padding would
/// be two spaces around nothing. Both printers have to agree about that too —
/// and the IR reaches it from `()` as well, which the AST still spells as
/// written.
#[test]
fn the_empty_struct_carries_no_padding() {
    let (ast, ir) = printed("let e = {}");
    assert_eq!(ast, "let e = {}");
    assert_eq!(ir, "let e = {}");

    let (ast, ir) = printed("let u : () = ()");
    assert_eq!(ast, "let u : () = ()");
    assert_eq!(ir, "let u : {} = {}");
}

/// The written type as the debugger renders it, and the type inference decided
/// the definition has as the compiler renders it.
fn types_of(source: &str) -> (String, String) {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    let bundle = Bundle::new("test", Version::new(0, 1, 0)).expect("the test bundle name is valid");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);
    let inferred = inference::infer(&mint, &mut built.program);
    assert!(
        inferred.errors.is_empty(),
        "{source}: {:#?}",
        inferred.errors
    );

    let (symbol, decl) = built
        .program
        .terms
        .first()
        .expect("the source declares one term");
    let annotation = decl.annotation.as_ref().expect("it is annotated");
    (
        print::ir::ty(&annotation.tracked, &mint).to_string(),
        inferred.schemes[symbol].to_string(),
    )
}

/// A type reaches a reader two ways — off the debugger's tabs, and out of a
/// diagnostic the compiler wrote — and both are the surface type grammar, so
/// both have to spell it the same. They are the same rule now, in
/// `ruddy::ui`, which the debugger's printers and `Display for Ty` both
/// go through; it used to be a copy each, with the compiler's own comment
/// conceding it was "the same rule the debugger's printers apply".
///
/// The arrow's grouping and the struct's braces are what the two copies had to
/// keep agreeing about, so those are what is checked: an annotation the user
/// wrote, and the type the definition was inferred to have from it.
#[test]
fn a_type_reads_the_same_whichever_printer_reached_it() {
    for (source, expected) in [
        ("let a : Nat = 1", "Nat"),
        ("let b : Nat -> Nat = fn x => x", "Nat -> Nat"),
        // Right-associative, so the left side is the only one that ever needs
        // parentheses — and the right side must not acquire any.
        (
            "let c : (Nat -> Nat) -> Nat -> Nat = fn f => f",
            "(Nat -> Nat) -> Nat -> Nat",
        ),
        (
            "let d : { x: Nat, y: Nat -> Nat } = { x: 1, y: fn n => n }",
            "{ x: Nat, y: Nat -> Nat }",
        ),
        ("let e : {} = {}", "{}"),
        (
            "let g : { p: { q: Nat } } -> Nat = fn r => r.p.q",
            "{ p: { q: Nat } } -> Nat",
        ),
        // A declared type is spelled as its name by both printers, and is an
        // atom to both: the arrow it stands for never leaks parentheses out
        // through the name.
        (
            "type Endo = Nat -> Nat\nlet h : Endo -> Endo = fn f => f",
            "Endo -> Endo",
        ),
    ] {
        let (written, inferred) = types_of(source);
        assert_eq!(written, expected, "{source}: as written");
        assert_eq!(inferred, expected, "{source}: as inferred");
    }
}

/// A row's two spellings, one per reader: the annotation keeps the tail as it
/// was written — `..`, or `..r` by name — while the scheme spells the
/// variable the quantifier numbered it as. Everything before the tail is the
/// same rule in both, which is what this pins.
#[test]
fn a_row_reads_as_written_and_as_quantified() {
    for (source, written, inferred) in [
        (
            "let f : { x: Nat, .. } -> Nat = fn p => p.x",
            "{ x: Nat, .. } -> Nat",
            "{ x: Nat, ..'a } -> Nat",
        ),
        (
            "let g : { x: Nat, ..r } -> { x: Nat, ..r } = fn p => p",
            "{ x: Nat, ..r } -> { x: Nat, ..r }",
            "{ x: Nat, ..'a } -> { x: Nat, ..'a }",
        ),
        (
            "let h : { x?: Nat, y: Nat } -> Nat = fn r => r.y",
            "{ x?: Nat, y: Nat } -> Nat",
            "{ x?: Nat, y: Nat } -> Nat",
        ),
    ] {
        let (as_written, as_inferred) = types_of(source);
        assert_eq!(as_written, written, "{source}: as written");
        assert_eq!(as_inferred, inferred, "{source}: as inferred");
    }
}

/// An application is one rendering rule too, and a parameter is the case that
/// could most easily drift: the IR knows it as a local symbol while the AST
/// knows it as the string it was written as, and the two have to spell it the
/// same. The declaration head has to print its parameters in both, or a
/// re-lowered program would bind nothing.
#[test]
fn both_trees_render_an_application_the_same_way() {
    for source in [
        "type Pair A B = { first: A, second: B }",
        "type Pair A B = { first: A, second: B }\ntype P = Pair Nat Nat",
        // An argument that is itself an application, and one that is an arrow:
        // the two positions that need parentheses to survive.
        "type Box A = { it: A }\ntype N = Box (Box Nat)",
        "type Box A = { it: A }\ntype F = Box (Nat -> Nat)",
        // An application on the left of an arrow needs none, because it stops
        // at the arrow of its own accord.
        "type Box A = { it: A }\ntype G = Box Nat -> Nat",
        // A parameter used bare, as the whole body.
        "type Id A = A",
        // Recursion through a parameterized declaration, handed its own
        // parameter.
        "type List a = { head: a, tail: List a }",
        // A row parameter: the `..` is written in the body, not at the head,
        // so both printers have to reach the parameter through the tail.
        "type WithX r = { x: Nat, ..r }",
        "type Both A r = { it: A, ..r }",
        "type WithX r = { x: Nat, ..r }\ntype P = WithX { y: Nat }",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}
