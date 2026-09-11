//! Tests for [`ruddy_debug::print`].

use std::sync::Arc;

use indexmap::IndexMap;
use ruddy::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
    types::{Formula, Presence, Rest, Row, RowField, Scheme, Ty},
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
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let bundle = Bundle::new("test", Version::new(0, 1, 0)).expect("the test bundle name is valid");
    let mut mint = Mint::new(bundle);
    let built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);

    (ast, print::ir::program(&built.program, &mint).to_string())
}

/// The surface printer keeps conditionals recognizable while the IR printer
/// honestly shows the ordinary Bool match they become. A chain remains a
/// single-final-`end` chain in the surface tree and nests in the false branch
/// after lowering.
#[test]
fn an_if_prints_as_surface_syntax_and_ir_match() {
    let (ast, ir) = printed("let choose = if true then 1n else 2n end");
    assert_eq!(ast, "let choose = if true then 1n else 2n end");
    assert_eq!(
        ir,
        "let choose = match true with | true => 1n | false => 2n end"
    );

    let source = "let choose = if p then 1n else if q then 2n else 3n end";
    let ast = ast_of(source);
    assert_eq!(ast, source);
    assert_eq!(ast_of(&ast), ast, "conditional printing is a fixed point");
}

/// A raw NUL is valid inside a Ruddy string. Rust's debug formatter spells it
/// as `\\0`, which the Ruddy lexer rejects, so extern targets must use the
/// language's literal formatter and remain stable when re-lowered.
#[test]
fn ir_extern_target_with_raw_control_character_prints_to_a_fixed_point() {
    let source = "extern host : Nat = \"raw\0target\"";
    let (_, ir) = printed(source);

    assert_eq!(ir, source);
    assert!(!ir.contains("\\0"));
    let (_, again) = printed(&ir);
    assert_eq!(again, ir, "IR extern target printing is a fixed point");
}

/// Rendering `{ name: value }` is one rule, in `print`, that both printers
/// read. It used to be a copy each, so the AST tab and the IR tab could come to
/// disagree about the same braces without anything noticing.
#[test]
fn both_trees_render_a_struct_the_same_way() {
    for source in [
        "let s = { x: 1n, y: 2n }",
        "let n = { p: { q: 3n } }",
        "let v : { a: Nat } = { a: 1n }",
        "let f = fn r => { wrapped: r.x }",
        // Open rows and named presences: the `..` tail, named or not, and the
        // `when` clause are one rendering rule too.
        "let g : { a: Nat, ..'r } -> Nat = fn p => p.a",
        "let h : { a when 'a: Nat, .. } -> Nat = fn p => p.a",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// A spread prints after the fields, as the `..` it was written as, in both
/// trees — never expanded into the fields inference found it to carry — and
/// with braces whatever the fields are named, since the tuple and unit
/// spellings would say the written fields are all there are. The value it
/// spreads is a whole expression and is never grouped: nothing an expression
/// can contain reads as the `,` or `}` that ends the literal.
#[test]
fn both_trees_render_a_struct_spread_the_same_way() {
    for source in [
        "let c = { y: 2n }\nlet a = { x: 1n, ..c }",
        "let c = { y: 2n }\nlet a = { ..c }",
        "let c = { y: 2n }\nlet a = { 0: 1n, 1: 2n, ..c }",
        "let a = { ..() }",
        "let f = fn v => v\nlet c = { y: 2n }\nlet a = { x: 1n, ..f c }",
        "let c = { y: 2n }\nlet a = { x: { ..c }, ..{ z: 3n, ..c } }",
        "let c = { y: 2n }\nlet a = { x: 1n, ..match c with | v => v end }",
        "let c = { y: 2n }\nlet a = { x: 1n, ..c.y }",
        "let c = { y: 2n }\nlet a = { x: 1n, ..c.y + 1n }",
        "let a = { x: 1n, ..fn v => v }",
        "let a = { x: 1n, ..do let c = { y: 2n } return c end }",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source);
        assert_eq!(ir, source);
        assert_eq!(ast_of(&ast), ast, "spread printing is a fixed point");
    }
}

#[test]
fn tuples_print_canonically_in_both_trees() {
    for source in [
        "let pair = (1n, 2n)",
        "let one = (1n,)",
        "let nested : ((Nat, Nat), Nat) = ((1n, 2n), 3n)",
        "let pick = fn p => p.0",
        "let destruct = match (1n, 2n) with | (x, y) => x end",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}: AST");
        assert_eq!(ir, source, "{source}: IR");
    }
}

/// The lexer intentionally rejects an unspaced `.0.0` as decimal-like
/// malformed syntax. Printers therefore retain the grouping that makes nested
/// numeric projections valid, in both trees and on every subsequent pass.
#[test]
fn nested_numeric_projections_print_to_a_fixed_point() {
    for source in [
        "let pick = fn p => (p.0).0",
        "let deep = fn p => ((p.0).0).0",
        "let mixed = fn p => (p.0).0.name",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}: AST");
        assert_eq!(ir, source, "{source}: IR");

        let (ast_again, ir_again) = printed(&ir);
        assert_eq!(ast_again, ir, "{source}: AST fixed point");
        assert_eq!(ir_again, ir, "{source}: IR fixed point");
    }
}

#[test]
fn canonical_tuple_fields_are_exact_and_insertion_order_independent() {
    assert_eq!(ruddy::ui::tuple_field_order(["1", "0"]), Some(vec![1, 0]));
    let reversed = Ty::Struct(Row {
        labels: IndexMap::from([
            (
                "1".to_string(),
                RowField {
                    presence: Presence::Present,
                    ty: Arc::new(Ty::String),
                },
            ),
            (
                "0".to_string(),
                RowField {
                    presence: Presence::Present,
                    ty: Arc::new(Ty::Nat),
                },
            ),
        ]),
        rest: Rest::Closed,
    });
    assert_eq!(reversed.to_string(), "(Nat, String)");

    assert_eq!(ruddy::ui::tuple_field_order(["0"]), Some(vec![0]));
    assert_eq!(ruddy::ui::tuple_field_order([]), None);
    assert_eq!(ruddy::ui::tuple_field_order(["00"]), None);
    assert_eq!(ruddy::ui::tuple_field_order(["0", "2"]), None);
    assert_eq!(ruddy::ui::tuple_field_order(["0", "x"]), None);
}

#[test]
fn numeric_struct_syntax_canonicalizes_only_when_exact() {
    for (source, expected) in [
        ("let p = { \"1\": 2n, \"0\": 1n }", "let p = (1n, 2n)"),
        (
            "let p : { \"1\": String, \"0\": Nat } = { \"0\": 1n, \"1\": \"x\" }",
            "let p : (Nat, String) = (1n, \"x\")",
        ),
        (
            "let first = fn p => match p with | { \"1\": y, \"0\": x } => x end",
            "let first = fn p => match p with | (x, y) => x end",
        ),
        (
            "let sparse = { \"0\": 1n, \"2\": 3n }",
            "let sparse = { 0: 1n, 2: 3n }",
        ),
        ("let padded = { \"00\": 1n }", "let padded = { \"00\": 1n }"),
        (
            "let huge = { \"18446744073709551616\": 1n }",
            "let huge = { \"18446744073709551616\": 1n }",
        ),
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, expected, "{source}: AST");
        assert_eq!(ir, expected, "{source}: IR");
    }
}

/// Exact empty structs share unit's canonical spelling in every surface
/// printer, whether they were written with braces or parentheses.
#[test]
fn the_empty_struct_canonicalizes_to_unit() {
    let (ast, ir) = printed("let e = {}");
    assert_eq!(ast, "let e = ()");
    assert_eq!(ir, "let e = ()");

    let (ast, ir) = printed("let u : () = ()");
    assert_eq!(ast, "let u : () = ()");
    assert_eq!(ir, "let u : () = ()");
}

/// One source as the parse tree's printer renders it, without lowering it.
/// [`printed`] requires a clean lowering; this does not, because the debugger
/// shows the parse tree of every program a reader can type.
fn ast_of(source: &str) -> String {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    parsed
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The AST printer renders whatever was parsed, including programs lowering
/// refuses. So its grouping has to hold for heads the IR can never carry: only
/// a declared name can be applied, but anything at all can be *written*
/// applied, and the head of a flat application has to read back as one atom.
#[test]
fn an_applied_head_is_grouped_like_any_other_position() {
    for source in [
        "type X = (Nat -> Nat) Nat",
        "type Y = (Box Nat) Nat",
        // A struct is an atom, so this one must not acquire parentheses.
        "type Z = { x: Nat } Nat",
    ] {
        let printed = ast_of(source);
        assert_eq!(printed, source, "{source}");
        assert_eq!(
            ast_of(&printed),
            printed,
            "{source}: printing is a fixed point"
        );
    }
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
    let inferred = inference::infer(&mint, &built.program, inference::Trace::Off);
    inferred.apply_types(&mut built.program);
    assert!(
        inferred.errors().is_empty(),
        "{source}: {:#?}",
        inferred.errors()
    );

    let (symbol, decl) = built
        .program
        .terms
        .first()
        .expect("the source declares one term");
    let annotation = decl.annotation.as_ref().expect("it is annotated");
    (
        print::ir::annotation(annotation, &mint).to_string(),
        inferred.semantics().schemes()[symbol].to_string(),
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
        ("let a : Nat = 1n", "Nat"),
        ("let b : Nat -> Nat = fn x => x", "Nat -> Nat"),
        // Right-associative, so the left side is the only one that ever needs
        // parentheses — and the right side must not acquire any.
        (
            "let c : (Nat -> Nat) -> Nat -> Nat = fn f => f",
            "(Nat -> Nat) -> Nat -> Nat",
        ),
        (
            "let d : { x: Nat, y: Nat -> Nat } = { x: 1n, y: fn n => n }",
            "{ x: Nat, y: Nat -> Nat }",
        ),
        ("let e : {} = {}", "()"),
        ("let tuple : (Nat, String) = (1n, \"x\")", "(Nat, String)"),
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
/// was written — `..`, or `..'r` by name — while the scheme spells the
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
            "let g : { x: Nat, ..'r } -> { x: Nat, ..'r } = fn p => p",
            "{ x: Nat, ..'r } -> { x: Nat, ..'r }",
            "{ x: Nat, ..'a } -> { x: Nat, ..'a }",
        ),
        (
            "let h : { x when 'a: Nat, y: Nat } -> Nat = fn r => r.y",
            "{ x when 'a: Nat, y: Nat } -> Nat",
            "{ x when 'a: Nat, y: Nat } -> Nat",
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
        "type Pair 'A 'B = { first: 'A, second: 'B }",
        "type Pair 'A 'B = { first: 'A, second: 'B }\ntype P = Pair Nat Nat",
        // An argument that is itself an application, and one that is an arrow:
        // the two positions that need parentheses to survive.
        "type Box 'A = { it: 'A }\ntype N = Box (Box Nat)",
        "type Box 'A = { it: 'A }\ntype F = Box (Nat -> Nat)",
        // An application on the left of an arrow needs none, because it stops
        // at the arrow of its own accord.
        "type Box 'A = { it: 'A }\ntype G = Box Nat -> Nat",
        // A parameter used bare, as the whole body.
        "type Id 'A = 'A",
        // Recursion through a parameterized declaration, handed its own
        // parameter.
        "type List 'a = { head: 'a, tail: List 'a }",
        // A row parameter: the `..` is written in the body, not at the head,
        // so both printers have to reach the parameter through the tail.
        "type WithX 'r = { x: Nat, ..'r }",
        "type Both 'A 'r = { it: 'A, ..'r }",
        "type WithX 'r = { x: Nat, ..'r }\ntype P = WithX { y: Nat }",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// A sum renders the same way in both trees, for the reason a struct does: the
/// bars, the `#`s and the `..` are one rule in `print`, and both printers
/// read it.
#[test]
fn both_trees_render_a_sum_the_same_way() {
    for source in [
        "type Option 'T = #Some 'T | #None",
        "let v = #Some 1n",
        "let n = #None",
        "let f = fn x => #Wrap x",
        "let p = fn f => #Some (f 1n)",
        // A case whose presence a `when` names, and a tail: the two ways a sum
        // is left open, both of which only an annotation may write.
        "let o : #A (when 'a) Nat | #B = #B",
        "let t : #A Nat | .. = #A 1n",
        "let r : #A Nat | ..'s = #A 1n",
        // The two forms that write no case, and so print the leading bar the
        // rest of them do not.
        "type Void = |",
        "type Only 'r = | ..'r",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// A tag with no payload is a word still waiting for one, so both printers put
/// it in parentheses wherever an atom could follow it. Without them the printed
/// source reads back as a different tree: `f (#A) 1` is `f` applied to the
/// case and then to `1`, while `f #A 1` is `f` applied to a case carrying
/// `1` — one printed program, two meanings, and the one it re-parses as is not
/// the one it was printed from.
#[test]
fn a_tag_with_no_payload_is_kept_off_what_follows_it() {
    for source in [
        "let f = fn a => a\nlet v = f (#A) 1n",
        "let f = fn a => a\nlet v = f (#A)",
        "let f = fn a => a\nlet v = (#A) 1n",
        // As a payload, where the same argument applies: the inner tag would
        // take the `1` the outer one is applied to.
        "let f = fn a => a\nlet v = #Some (#A) 1n",
        // Carrying something it groups as the application it reads as, and
        // takes no parentheses at the head of one.
        "let v = #Some 1n 2n",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// A case carrying unit is written with no payload, and prints with none — so
/// `#None` survives lowering as itself rather than acquiring an explicit
/// unit payload.
#[test]
fn a_case_carrying_nothing_keeps_its_missing_payload() {
    let (ast, ir) = printed("type Flag = #On | #Off");
    assert_eq!(ast, "type Flag = #On | #Off");
    assert_eq!(ir, "type Flag = #On | #Off");

    // Written out, the unit stays written out: it is a payload the reader
    // put on the page.
    let (ast, ir) = printed("type Flag = #On () | #Off ()");
    assert_eq!(ast, "type Flag = #On () | #Off ()");
    assert_eq!(ir, "type Flag = #On () | #Off ()");
}

/// A printed program re-lowers into the one it was printed from, definitions
/// that name themselves and each other included. Terms print in the order they
/// were written, which is now the only order there is: a definition can name
/// one written below it, so no printing order could put every name after its
/// definition and none has to.
#[test]
fn recursion_and_forward_references_round_trip() {
    for source in [
        "let f = fn n => f n",
        "let even = fn n => odd n\nlet odd = fn n => even n",
        "let a = id 1n\nlet id = fn x => x",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");

        // And again, off what was printed: the second rendering is the first
        // one, or the printer said something the builder reads differently.
        let (_, again) = printed(&ir);
        assert_eq!(again, ir, "{source} did not round-trip");
    }
}

/// A match prints back as the surface syntax it was written as — the leading
/// `|` on every arm, each pattern in the grammar it was read by — and the
/// printed form re-parses and re-lowers to the same thing. Every kind of
/// pattern is on the page: names, naturals, unit, struct patterns with
/// renaming and nesting, tags bare and carrying, and the greedy payload that
/// needs its parentheses back.
#[test]
fn a_match_and_its_patterns_round_trip() {
    for source in [
        "let f = fn v => match v with | #Some x => x | #None => 0n end",
        "let f = fn v => match v with | 0n => 1n | 1n => 2n | k => k end",
        "let f = fn v => match v with | () => 1n end",
        "let f = fn v => match v with | { a: #A, b: { c: x } } => x | r => 0n end",
        "let f = fn v => match v with | #A (#X x) => x | #A w => 0n | r => 1n end",
        "let f = fn v => match v with end",
        "let f = fn v => (match v with | w => w end).x",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");

        let (_, again) = printed(&ir);
        assert_eq!(again, ir, "{source} did not round-trip");
    }

    let (ast, ir) = printed("let f = fn v => match v with | {} => 1n end");
    assert_eq!(ast, "let f = fn v => match v with | () => 1n end");
    assert_eq!(ir, ast);

    // The one printing the IR spells differently from the AST: a pun is
    // expanded by lowering, so the IR prints the field twice-named while the
    // AST keeps the spelling — and the IR's form still re-lowers to itself.
    let source = "let f = fn v => match v with | { x, y: a } => x end";
    let (ast, ir) = printed(source);
    assert_eq!(ast, source);
    assert_eq!(ir, "let f = fn v => match v with | { x: x, y: a } => x end");
    let (_, again) = printed(&ir);
    assert_eq!(again, ir);
}

/// A `do` block prints back as the surface syntax it was written as, in both
/// trees, and the printed form re-parses and re-lowers to the same thing.
///
/// The `end` is what makes this need no parentheses anywhere but the two
/// positions below that ask for them: a block closes itself, so it may be a
/// value, a `fn` body, a field, or the head of an application as it stands —
/// and it is grouped as an argument, as a `match` is.
#[test]
fn a_do_block_round_trips() {
    for source in [
        "let a = do let x = 1n return x end",
        "let a = do let x : Nat = 1n return x end",
        "let a = do let x = 1n let y = { v: x } return y end",
        "let a = do let x = do let y = 1n return y end return x end",
        "let a = do let x = 1n end",
        "let a = fn p => do let x = p.v return x end",
        "let a = { v: do let n = 1n return n end }",
        "let f = fn x => x\nlet a = f (do let n = 1n return n end)",
        "let a = do let f = fn x => x return f end 1n",
        "let a = (do let p = { x: 1n } return p end).x",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");

        let (_, again) = printed(&ir);
        assert_eq!(again, ir, "{source} did not round-trip");
    }
}

/// The IR has no block, only the nested bindings a block is a spelling of, so
/// its printer folds a chain of them back into one block: a block returned
/// from a block is one block, a `return ()` is no `return` at all, and a block
/// with no bindings is just its value. Either way the printed IR re-lowers to
/// the same IR.
#[test]
fn the_ir_prints_nested_bindings_as_one_block() {
    for (source, folded) in [
        (
            "let a = do let x = 1n return do let y = x return y end end",
            "let a = do let x = 1n let y = x return y end",
        ),
        (
            "let a = do let x = 1n return () end",
            "let a = do let x = 1n end",
        ),
        ("let a = do return () end", "let a = ()"),
        ("let a = do end", "let a = ()"),
        ("let a = do return 1n end", "let a = 1n"),
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, folded, "{source}");

        let (_, again) = printed(&ir);
        assert_eq!(again, ir, "{source} did not round-trip");
    }
}

/// A wildcard pattern prints as the `_` it was written as, in both trees, and
/// the printed form re-parses and re-lowers to the same thing — every pattern
/// position it can sit in.
#[test]
fn a_wildcard_pattern_round_trips() {
    for source in [
        "let f = fn v => match v with | #Some x => x | _ => 0n end",
        "let f = fn v => match v with | #Some _ => 1n | #None => 0n end",
        "let f = fn v => match v with | { a: _, b: x } => x end",
        "let f = fn v => match v with | #Pair { a: _, b: _ } => 1n | _ => 2n end",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");
    }

    // A struct-pattern `let` desugars, so only the AST keeps the `_` on the
    // page; the IR shows the hidden projection it became, and the exact
    // pattern's demand — its named fields, each type a hole — as the
    // annotation on the temporary.
    let source = "let f = fn p => do let { x: _, y } = p return y end";
    let (ast, ir) = printed(source);
    assert_eq!(ast, source);
    assert_eq!(
        ir,
        "let f = fn p => do let %struct : { x: _, y: _ } = p \
         let %discard = %struct.x let y = %struct.y return y end"
    );
}

/// A `_` fn argument prints as written in the AST; the IR spells the fresh
/// symbol it lowered to, marked with the `%` no identifier can spell — the
/// same convention every pattern-`let` temporary keeps.
#[test]
fn a_wildcard_fn_argument_prints_as_written_and_as_lowered() {
    let (ast, ir) = printed("let const = fn x _ => x");
    assert_eq!(ast, "let const = fn x _ => x");
    assert_eq!(ir, "let const = fn x => fn %discard => x");

    // The hidden definitions of a discarded `let` read the same way.
    let (ast, ir) = printed("let _ : Nat = 1n");
    assert_eq!(ast, "let _ : Nat = 1n");
    assert_eq!(ir, "let %discard : Nat = 1n");
}

/// The `..` that makes a struct pattern open renders in both trees — as
/// written in the AST, and surviving normalization in the IR — last in the
/// braces, after the trailing comma, so the printed form re-parses.
#[test]
fn both_trees_render_a_pattern_rest() {
    let (ast, ir) = printed("let f = fn v => match v with | { x, .. } => x | {} => 0n end");
    assert_eq!(
        ast,
        "let f = fn v => match v with | { x, .. } => x | () => 0n end"
    );
    // The pun expands in the IR, and the `..` stays put.
    assert_eq!(
        ir,
        "let f = fn v => match v with | { x: x, .. } => x | () => 0n end"
    );

    // Bare, the open pattern is nothing but its `..`.
    let (ast, ir) = printed("let g = fn v => match v with | { .. } => 1n end");
    assert_eq!(ast, "let g = fn v => match v with | { .. } => 1n end");
    assert_eq!(ir, "let g = fn v => match v with | { .. } => 1n end");
}

/// A `where` clause is part of a written ascription, so both trees render it —
/// and both render it back to the source it was parsed from, parentheses and
/// all.
#[test]
fn both_trees_render_a_where_clause_the_same_way() {
    for source in [
        "let f : { x when 'a: Nat } where 'a = { x: 1n }",
        "let f : { x when 'a: Nat, y when 'b: Nat } where 'a != 'b = { x: 1n }",
        "let f : { x when 'a: Nat, y when 'b: Nat } where 'a = 'b = { x: 1n }",
        "let f : { x when 'a: Nat, y when 'b: Nat } where not ('a and 'b) = { x: 1n }",
        "let f : { x when 'a: Nat, y when 'b: Nat } where 'a or 'b = { x: 1n }",
        // The anonymous presence is spelled back as the `_` it was written as.
        "let f : { x when _: Nat } = { x: 1n }",
        // And a sum's clause takes the parentheses the grammar needs.
        "let f : #A (when 'a) Nat | #B (when 'b) = #B",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, ir, "{source}");
        assert_eq!(ast, source, "{source}");
    }
}

/// The clause a definition ends up with is what its scheme prints, and it
/// follows the whole type — omitted entirely when the definition requires
/// nothing, which is what makes an ordinary program read exactly as before.
#[test]
fn a_scheme_prints_the_clause_it_requires() {
    for (source, expected) in [
        (
            "let p = fn a => match a with | {x} => {} | {y} => {} end",
            "{ x when 'a: 'c, y when 'b: 'd } -> () where 'a != 'b",
        ),
        ("let id = fn x => x", "'a -> 'a"),
    ] {
        let mut files = FileManager::new();
        let file = files.register_new_file("<test>".to_string(), source.to_string());
        let lexed = token::lex(source, file);
        let parsed = parse::parse(lexed.tokens);
        let bundle =
            Bundle::new("test", Version::new(0, 1, 0)).expect("the test bundle name is valid");
        let mut mint = Mint::new(bundle);
        let mut built = ir::build(&mut mint, parsed.stmts);
        assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);
        let output = inference::infer(&mint, &built.program, inference::Trace::Off);
        output.apply_types(&mut built.program);
        assert!(
            output.errors().is_empty(),
            "{source}: {:#?}",
            output.errors()
        );
        let scheme = output
            .semantics()
            .schemes()
            .values()
            .next()
            .expect("the source declares one term");
        assert_eq!(scheme.to_string(), expected, "{source}");
    }
}

/// R24's four cases, each asserted to re-lower to itself.
///
/// The `+` binds to the arrow parsed at its own level, so the printer
/// parenthesizes an arrow's result exactly when *that* arrow carries a row and
/// the result is itself an arrow — and in no other case. Bracketing whenever
/// the result carries one, which looks safer, would move the row to the wrong
/// arrow on re-reading; the round trip is what says so.
#[test]
fn an_effect_row_prints_on_the_arrow_it_belongs_to() {
    let effects = "effect Log = { write: Nat -> () }\n";
    for body in [
        // A row, and a result that is not an arrow: no parentheses.
        "Nat -> Nat + !Log",
        // No outer row, and the inner arrow carries one: none either.
        "Nat -> Nat -> Nat + !Log",
        // A row, and a result that is an arrow: parentheses, or the row would
        // read as the inner arrow's.
        "Nat -> (Nat -> Nat) + !Log",
        // Neither carries one: none.
        "Nat -> Nat -> Nat",
    ] {
        let source = format!("{effects}type T = {body}");
        let (ast, ir) = printed(&source);
        assert_eq!(ast, source, "{body}");
        assert_eq!(ir, source, "{body}");
        // And again off what was printed, which is what makes the parentheses
        // trustworthy rather than merely plausible.
        let (_, again) = printed(&ir);
        assert_eq!(again, ir, "{body}");
    }
}

/// A printed *semantic* type re-lowers to the type it was printed from, which
/// is what a diagnostic quoting one rests on. The two spellings of the empty
/// row meet here: `A -> B + |` and `A -> B` are one type, so the scheme prints
/// bare either way.
#[test]
fn a_printed_effect_row_re_lowers_to_itself() {
    let effects = "effect Log = { write: Nat -> () }\neffect IO = { print: Nat -> () }\n";
    // A scheme's quantified tail prints `..'a`, which is a letter the writer
    // did not choose and no re-lowering could recover — the rule every other
    // quantified variable already keeps. So what is re-lowered is the concrete
    // half, and the open half is only asserted to print as itself.
    for (annotation, expected) in [
        ("Nat -> Nat + !Log", "Nat -> Nat + !Log"),
        ("Nat -> Nat + !Log + !IO", "Nat -> Nat + !Log + !IO"),
        // The scheme spells its quantifiers positionally, so a declared `e`
        // comes back as the letter its position earns.
        ("Nat -> Nat + ..'e", "Nat -> Nat + ..'a"),
        ("Nat -> Nat + !Log + ..'e", "Nat -> Nat + !Log + ..'a"),
        // Both spellings of pure.
        ("Nat -> Nat", "Nat -> Nat"),
        ("Nat -> Nat + |", "Nat -> Nat"),
        // The two arrow-binding cases, as a definition's own type.
        ("Nat -> Nat -> Nat + !Log", "Nat -> Nat -> Nat + !Log"),
        ("Nat -> (Nat -> Nat) + !Log", "Nat -> (Nat -> Nat) + !Log"),
        // A `where` clause on effect presences prints as it was written.
        (
            "Nat -> Nat + !Log (when 'a) + !IO (when 'b) where 'a != 'b",
            "Nat -> Nat + !Log (when 'a) + !IO (when 'b) where 'a != 'b",
        ),
    ] {
        // Two arguments where the annotation takes two, so that pushing the
        // written type into the lambda has somewhere 'to put each of them.
        let arrows = annotation.matches("->").count();
        let body = match arrows {
            1 => "fn x => x".to_string(),
            _ => "fn x => fn y => y".to_string(),
        };
        let source = format!("{effects}let f : {annotation} = {body}");
        let (_, scheme) = types_of(&source);
        assert_eq!(scheme, expected, "{annotation}");
        // What was printed, read back: the scheme of a definition annotated
        // with it is the same scheme — a printed scheme is valid source, the
        // declared tail included.
        let again = format!("{effects}let f : {scheme} = {body}");
        assert_eq!(types_of(&again).1, expected, "{annotation}");
    }
}

/// An alias does not survive into the semantic type language, so a type
/// annotated with one prints as the effects it stands for — and that is what
/// re-lowers.
#[test]
fn an_alias_prints_as_the_effects_it_names() {
    let source = "effect Log = { write: Nat -> () }\n\
                  effect IO = { print: Nat -> () }\n\
                  effect Console = !Log + !IO\n\
                  let f : Nat -> Nat + !Console = fn x => x";
    let (written, scheme) = types_of(source);
    // The IR keeps the row expanded, since expansion is lowering's.
    assert_eq!(written, "Nat -> Nat + !Log + !IO");
    assert_eq!(scheme, "Nat -> Nat + !Log + !IO");
}

/// Both trees render an effect declaration, a handler and an operation
/// reference back as the source they came from — the AST as written, the IR
/// with its aliases expanded and its binders named through the mint.
#[test]
fn both_trees_render_the_effect_forms() {
    for source in [
        "effect Log = Nat -> ()",
        "effect Log = { value: Nat } -> ()",
        "effect Apply = (Nat -> Nat) -> ()",
        "effect Apply = { run: (Nat -> Nat) -> () }",
        "effect Log = { write: Nat -> () }",
        "effect Log = { write: Nat -> (), flush: () -> () }",
        "effect Nil",
        // An effect written absent, and a `when` clause on one: both trees
        // render the marks a row's labels may wear.
        "effect Log = { write: Nat -> () }\n\
         let f : Nat -> Nat + \\!Log + ..'e = fn x => x",
        "effect Log = { write: Nat -> () }\n\
         let f : Nat -> Nat + !Log (when 'a) + ..'e = fn x => x",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");
    }

    let source = "effect Log = { write: Nat -> () }\n\
                  let h = fn n => handle !Log.write n with | !Log.write s => () | return x => x end";
    let (ast, ir) = printed(source);
    assert_eq!(ast, source);
    assert_eq!(ir, source);
    let (_, again) = printed(&ir);
    assert_eq!(again, ir);
}

/// Every variable a scheme quantifies prints as the bare letter a `where 'let`
/// declares it as — no prefix, whichever of the three sorts it is — and the
/// clause beside the type is what says which letters those are.
#[test]
fn a_scheme_declares_the_letters_it_quantifies() {
    // A type, a struct's rest, a sum's rest and a presence, each in its own
    // position and each spelled the way the surface grammar writes it.
    let cases: IndexMap<String, RowField> = [(
        "A".to_string(),
        RowField {
            presence: Presence::Bound(0),
            ty: Arc::new(Ty::plain(Ty::Bound(1))),
        },
    )]
    .into_iter()
    .collect();
    let body = Arc::new(Ty::plain(Ty::pure(
        Arc::new(Ty::Struct(Row {
            labels: [(
                "x".to_string(),
                RowField::present(Arc::new(Ty::plain(Ty::Bound(1)))),
            )]
            .into_iter()
            .collect(),
            rest: Rest::Bound(2),
        })),
        Arc::new(Ty::plain(Ty::Sum(Row {
            labels: cases,
            rest: Rest::Bound(3),
        }))),
    )));
    let scheme = Scheme::constrained(4, 1, body, Formula::True);
    assert_eq!(
        scheme.to_string(),
        "{ x: 'b, ..'c } -> #A (when 'a) 'b | ..'d"
    );

    // A formula follows the declarations, separated by the `;` the grammar
    // reads the two statements apart with.
    let both = Scheme::constrained(
        2,
        2,
        Arc::new(Ty::Struct(Row {
            labels: [
                (
                    "x".to_string(),
                    RowField {
                        presence: Presence::Bound(0),
                        ty: Arc::new(Ty::plain(Ty::Nat)),
                    },
                ),
                (
                    "y".to_string(),
                    RowField {
                        presence: Presence::Bound(1),
                        ty: Arc::new(Ty::plain(Ty::Nat)),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        })),
        Formula::bound(0).xor(Formula::bound(1)),
    );
    assert_eq!(
        both.to_string(),
        "{ x when 'a: Nat, y when 'b: Nat } where 'a != 'b"
    );

    // A scheme requiring something of a presence it does not itself quantify
    // writes the clause and no `let`: there are no letters to declare, and the
    // formula is still what the scheme requires.
    let free = Scheme::constrained(0, 0, Arc::new(Ty::plain(Ty::Nat)), Formula::var(3));
    assert_eq!(free.to_string(), "Nat where ?3");

    // A scheme quantifying nothing and requiring nothing writes no `where` at
    // all, which is every scheme in a monomorphic program.
    let plain = Scheme::new(0, Arc::new(Ty::plain(Ty::Nat)));
    assert_eq!(plain.to_string(), "Nat");

    // A rigid prints as the name its `where 'let` gave it, in either sort.
    let rigid = Arc::new(Ty::Struct(Row {
        labels: [(
            "x".to_string(),
            RowField::present(Arc::new(Ty::plain(Ty::Rigid {
                id: 1,
                name: "a".into(),
            }))),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Rigid {
            id: 0,
            name: "r".into(),
        },
    }));
    assert_eq!(rigid.to_string(), "{ x: 'a, ..'r }");

    // And a solver variable is unchanged: `?3` for a type or a rest, `?3` in a
    // `when` for a presence, a bare `?` for the undecided.
    let solver = Arc::new(Ty::Struct(Row {
        labels: [(
            "x".to_string(),
            RowField {
                presence: Presence::Var(4),
                ty: Arc::new(Ty::default()),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Var(3),
    }));
    assert_eq!(solver.to_string(), "{ x when ?4: ?, ..?3 }");
}

/// A printed scheme is valid source. `let id = fn x => x` reports
/// `a -> a`, and pasting that back as an annotation re-lowers to
/// the type it was printed from — which an implied quantifier could not do,
/// since a bare `a` in an annotation is a name that has to have been declared.
#[test]
fn a_printed_scheme_reads_back_as_source() {
    for source in [
        "let id = fn x => x",
        "let getx = fn p => p.x",
        "let wrap = fn x => #Some x",
        "let both = fn a => match a with | {x} => () | {y} => () end",
    ] {
        let printed = printed_scheme(source);
        // Pasted back as an annotation over the very same body, which is what
        // "valid source" has to mean: it parses, it lowers, and what it
        // publishes is what it was printed from.
        let value = source.split_once(" = ").expect("a definition").1;
        let again = format!("let pasted : {printed} = {value}");
        assert_eq!(printed_scheme(&again), printed, "{again}");
    }
}

/// The scheme of the last definition in `source`, as it prints.
fn printed_scheme(source: &str) -> String {
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
    let inferred = inference::infer(&mint, &built.program, inference::Trace::Off);
    inferred.apply_types(&mut built.program);
    assert!(
        inferred.errors().is_empty(),
        "{source}: {:#?}",
        inferred.errors()
    );
    let (symbol, _) = built.program.terms.last().expect("a definition");
    inferred.semantics().schemes()[symbol].to_string()
}

/// Both module forms and a path, rendered back as the source they were parsed
/// from — and re-parsed, since a printed tree that does not read back is a tree
/// the AST tab is lying about.
#[test]
fn the_ast_printer_round_trips_modules_and_paths() {
    for source in [
        "module A",
        "module A = end",
        "module A = let x = 1n end",
        "module A = module B = let x = 1n end let y = B::x end",
        "let a = Math::double 2n",
        "let a = Math::Vec::zero",
        "let a = Math::mk 1n 2n",
        "let a = Math::p.x",
        "let a : Math::Pair Nat Nat = z",
        "let a = Sys::!Log.write 1n",
        "let a = handle e with | Sys::!Log.write s => s end",
        "let f : () -> Nat + Sys::!Log = g",
        "let f : () -> Nat + \\Sys::!Log + ..'r = g",
        "effect Console = Sys::!Log + !IO",
    ] {
        assert_eq!(reprinted(source), source);
    }
}

#[test]
fn quoted_fields_and_variants_round_trip_canonically_in_ast_and_ir() {
    for source in [
        r###"let value : { "field name": Nat, "let": Nat, "line\n\"quote\"\\tail": Nat } = { "field name": 1n, "let": 2n, "line\n\"quote\"\\tail": 3n }"###,
        r###"let get = fn record => record."field name"."let""###,
        r###"let get = fn record => match record with | { "field name": x, "let": y } => x end"###,
        r###"type Choice = #"some case" Nat | #let | #"line\n\"quote\"\\tail""###,
        r###"let get = fn choice => match choice with | #"some case" n => n | #let => 0n | #"line\n\"quote\"\\tail" => 1n end"###,
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}: AST canonical form");
        assert_eq!(ir, source, "{source}: IR canonical form");
        let (ast_again, ir_again) = printed(&ir);
        assert_eq!(ast_again, ast, "{source}: AST fixed point");
        assert_eq!(ir_again, ir, "{source}: IR fixed point");
    }

    // Quotes which are unnecessary disappear, while field keywords keep them.
    // The result is still accepted as the same decoded structural labels.
    for (source, canonical) in [
        (
            r###"let value = { "plain": 1n, "let": 2n }"###,
            r###"let value = { plain: 1n, "let": 2n }"###,
        ),
        (r###"let value = #"Plain""###, "let value = #Plain"),
        (r###"let value = #"let""###, "let value = #let"),
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, canonical, "{source}: AST canonicalized");
        assert_eq!(ir, canonical, "{source}: IR canonicalized");
        let (_, again) = printed(canonical);
        assert_eq!(again, canonical, "{source}: canonical form round-trips");
    }
}

/// Print one source through the AST printer and read the result back, so a
/// printed tree that needs a bracket it did not write is caught here rather
/// than by a reader pasting it into a file.
fn reprinted(source: &str) -> String {
    let printed = printed_ast(source);
    assert_eq!(
        printed_ast(&printed),
        printed,
        "printing {source:?} did not round-trip"
    );
    printed
}

fn printed_ast(source: &str) -> String {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    parsed
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Both trees render a parameterized effect declaration, an applied label,
/// and a parameterized alias back as the source they came from — the IR with
/// the alias expanded.
#[test]
fn both_trees_render_effect_parameters_and_applications() {
    for source in [
        "effect Ask 'a = { get: () -> 'a }",
        "effect Log 'a = 'a -> ()",
        "effect Nil 'a 'b",
        "effect Ask 'a = { get: () -> 'a }\n\
         let f : () -> Nat + !Ask Nat = fn x => 0n",
        "effect Ask 'a = { get: () -> 'a }\n\
         let f : () -> Nat + !Ask (Nat -> Nat) + ..'e = fn x => 0n",
        "effect Ask 'a = { get: () -> 'a }\n\
         let f : () -> Nat + \\!Ask Nat + ..'e = fn x => 0n",
        "effect State 'r = { get: () -> { x: Nat, ..'r } }\n\
         let f : () -> Nat + !State { y: Nat } (when 'p) + ..'e = fn x => 0n",
        "effect Log = { write: Nat -> () }\n\
         effect Ask 'a = { get: () -> 'a }\n\
         effect Both 'a 'e = !Ask 'a + !Log + ..'e",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, source, "{source}");
        assert_eq!(ir, source, "{source}");
    }
    let source = "effect Log = { write: Nat -> () }\n\
                  effect Ask 'a = { get: () -> 'a }\n\
                  effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
                  let f : () -> Nat + !Both Nat (..'e) = fn x => 0n";
    let (ast, ir) = printed(source);
    assert_eq!(ast, source);
    assert_eq!(
        ir,
        "effect Log = { write: Nat -> () }\n\
         effect Ask 'a = { get: () -> 'a }\n\
         effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
         let f : () -> Nat + !Ask Nat + !Log + ..'e = fn x => 0n"
    );
}

/// A printed scheme carrying applied effects re-lowers to itself.
#[test]
fn a_printed_applied_effect_re_lowers_to_itself() {
    let effects = "effect Ask 'a = { get: () -> 'a }\n\
                   effect State 'r = { get: () -> { x: Nat, ..'r } }\n\
                   effect Log = { write: Nat -> () }\n\
                   effect Run 'e = { run: (() -> () + ..'e) -> () }\n";
    for annotation in [
        "Nat -> Nat + !Ask Nat",
        "Nat -> Nat + !Ask (Nat -> Nat) + !Log",
        "Nat -> Nat + !State { y: Nat }",
        "Nat -> Nat + !Run (!Log)",
        "Nat -> Nat + !Run (|)",
    ] {
        let source = format!("{effects}let f : {annotation} = fn x => x");
        let (_, scheme) = types_of(&source);
        assert_eq!(scheme, annotation, "{annotation}");
        let again = format!("{effects}let f : {scheme} = fn x => x");
        assert_eq!(types_of(&again).1, annotation, "{annotation}");
    }
}

/// Both printers write a definition's metadata in front of it, and agree on
/// every literal form: unit spellings collapse to the bare key, a tuple
/// prints as one, and grouping follows the expression printer's rules.
#[test]
fn both_trees_render_metadata_the_same_way() {
    for (source, expected) in [
        ("@k let x = 1n", "@k let x = 1n"),
        ("@k () let x = 1n", "@k let x = 1n"),
        ("@k {} let x = 1n", "@k let x = 1n"),
        (
            "@a 1n @b 2i @c 1.5 @d \"s\" @e true let x = 1n",
            "@a 1n @b 2i @c 1.5 @d \"s\" @e true let x = 1n",
        ),
        ("@k (1n, 2n) let x = 1n", "@k (1n, 2n) let x = 1n"),
        ("@k { 0: 1n, 1: 2n } let x = 1n", "@k (1n, 2n) let x = 1n"),
        ("@k (1n,) let x = 1n", "@k (1n,) let x = 1n"),
        ("@k [1n, 2n] let x = 1n", "@k [1n, 2n] let x = 1n"),
        (
            "@k { a: 1n, \"b c\": [true] } let x = 1n",
            "@k { a: 1n, \"b c\": [true] } let x = 1n",
        ),
        ("@k #Tag let x = 1n", "@k #Tag let x = 1n"),
        ("@k #Tag () let x = 1n", "@k #Tag let x = 1n"),
        ("@k #Tag 1n let x = 1n", "@k #Tag 1n let x = 1n"),
        (
            "@k #Tag #Inner 1n let x = 1n",
            "@k #Tag (#Inner 1n) let x = 1n",
        ),
        (
            "@k #\"two words\" let x = 1n",
            "@k #\"two words\" let x = 1n",
        ),
        (
            "@k \\\\ line one\n   \\\\ line two\nlet x = 1n",
            "@k \" line one\\n line two\" let x = 1n",
        ),
        ("@t type T = Nat", "@t type T = Nat"),
        ("@e #On effect E = Nat -> ()", "@e #On effect E = Nat -> ()"),
        (
            "@h \"host\" extern f : Nat = \"f\"",
            "@h \"host\" extern f : Nat = \"f\"",
        ),
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(ast, expected, "surface: {source:?}");
        assert_eq!(ir, expected, "lowered: {source:?}");
    }
}

#[test]
fn mutation_syntax_round_trips_with_assignment_grouping() {
    let source = "let copy = fn a => fn b => a := b := ~a\nlet cell = fn x => mut x";
    let (ast, ir) = printed(source);
    assert_eq!(ast_of(&ast), ast);
    assert_eq!(printed(&ir).1, ir);
    let source = "let update = fn a => fn b => fn c => (a := b) := c";
    let (ast, _) = printed(source);
    assert!(ast.contains("(a := b) := c"));
    assert_eq!(ast_of(&ast), ast);
}

#[test]
fn signed_literals_and_operator_negation_print_without_becoming_comments() {
    for source in [
        "let value = - -1.5",
        "let value = - - 1.5",
        "let value = (fn x => x) -42i",
        "@negative -1.5\nlet value = -0.0",
        "let value = fn x => match x with | -1i => -2i | _ => 0i end",
    ] {
        let (ast, ir) = printed(source);
        assert_eq!(printed(&ast).0, ast);
        assert_eq!(printed(&ir).1, ir);
        assert!(!ast.contains("--"), "{ast}");
        assert!(!ir.contains("--"), "{ir}");
    }
}
