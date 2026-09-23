//! Structural result joins preserve row correlations without untagged unions.

use ruddy::{
    compile, inference, parse,
    symbol::{Bundle, Mint, Version},
    token::lex,
    tracking::FileID,
};

fn compiles(source: &str) -> bool {
    let tokens = lex(source, FileID::GENERATED);
    assert!(tokens.errors.is_empty(), "{:#?}", tokens.errors);
    let parsed = parse::parse(tokens.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("contract-results", Version::new(0, 0, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .is_ok()
}

const RECORD: &str = "let produce = fn | #A => {a: 1n} | #B => {b: 2n}\n";
const VARIANT: &str = "let produce = fn | #A => #Left 1n | #B => #Right 2n\n";

#[test]
fn broad_record_results_cannot_be_chosen_by_the_consumer() {
    for use_ in [
        "let bad: {a: Nat} = produce selector",
        "let bad = (produce selector).a",
        "let result = produce selector\nlet bad: {a: Nat} = result",
        "let wrap = fn x => produce x\nlet bad: {a: Nat} = wrap selector",
    ] {
        let source = format!("{RECORD}\nlet selector: #A | #B = #B\n{use_}");
        assert!(
            !compiles(&source),
            "accepted consumer-selected result: {source}"
        );
    }
}

#[test]
fn known_record_selection_retains_its_exact_fields() {
    let source =
        format!("{RECORD}\nlet a: {{a: Nat}} = produce #A\nlet b: {{b: Nat}} = produce #B");
    assert!(compiles(&source), "{source}");
}

#[test]
fn broad_variant_results_admit_every_reachable_tag() {
    let source = format!(
        "{VARIANT}\nlet selector: #A | #B = #B\n\
         let result: #Left Nat | #Right Nat = produce selector\n\
         let read: Nat = match result with | #Left x => x | #Right x => x end"
    );
    assert!(compiles(&source), "{source}");
    let narrowed =
        format!("{VARIANT}\nlet selector: #A | #B = #B\nlet bad: #Left Nat = produce selector");
    assert!(!compiles(&narrowed), "{narrowed}");
}

#[test]
fn known_variant_selection_retains_its_exact_tag() {
    let source =
        format!("{VARIANT}\nlet a: #Left Nat = produce #A\nlet b: #Right Nat = produce #B");
    assert!(compiles(&source), "{source}");
}

#[test]
fn broad_record_results_keep_their_fields_correlated() {
    let source = "let produce = fn\n\
        | #A => {a: 1n, paired: 2n}\n\
        | #B => {b: 3n}\n\
        let selector: #A | #B = #B\n\
        let value = produce selector\n\
        let result: Nat = match value with\n\
        | {a: _, paired} => paired\n\
        | {b} => b\n\
        end";
    assert!(compiles(source), "{source}");
}

#[test]
fn nested_matches_keep_the_outer_case_admission() {
    let source = "let produce = fn selector => match selector with\n\
        | #A => match selector with | _ => {a: 1n} end\n\
        | #B => {b: 2n}\n\
        end\n\
        let a: {a: Nat} = produce #A\n\
        let b: {b: Nat} = produce #B";
    assert!(compiles(source), "{source}");
}
