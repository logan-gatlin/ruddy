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
