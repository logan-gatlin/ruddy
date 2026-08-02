//! Tests for [`ruddy_debug::print`].

use ruddy::{
    ir, parse,
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
