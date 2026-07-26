//! Tests for [`ruddy::token`].

use ruddy::{
    token::{Error, ErrorKind, Kind, lex},
    tracking::FileID,
};

fn kinds(src: &str) -> Vec<Kind> {
    let out = lex(src, FileID::GENERATED);
    assert!(out.errors.is_empty(), "lex errors: {:#?}", out.errors);
    out.tokens.into_iter().map(|token| token.tracked).collect()
}

fn errors(src: &str) -> Vec<Error> {
    lex(src, FileID::GENERATED).errors
}

#[test]
fn lexes_natural_literals() {
    assert!(matches!(kinds("0")[..], [Kind::Natural(0)]));
    assert!(matches!(kinds("42")[..], [Kind::Natural(42)]));
    // Leading zeros are not a second spelling of a different number.
    assert!(matches!(kinds("007")[..], [Kind::Natural(7)]));
    assert!(matches!(
        kinds(&u128::MAX.to_string())[..],
        [Kind::Natural(u128::MAX)]
    ));
}

#[test]
fn a_natural_is_spanned_and_printed_as_written() {
    let out = lex("let n = 4096", FileID::GENERATED);
    let last = out.tokens.last().expect("the literal was lexed");
    assert_eq!(last.span.start, 8);
    assert_eq!(last.span.width, 4);
    assert_eq!(last.tracked.to_string(), "4096");
}

#[test]
fn an_identifier_may_still_contain_digits() {
    // Only a *leading* digit makes a literal, so `x1` is one name.
    assert!(matches!(&kinds("x1")[..], [Kind::Identifier(name)] if name == "x1"));
    assert!(matches!(&kinds("_0")[..], [Kind::Identifier(name)] if name == "_0"));
}

#[test]
fn a_literal_running_into_a_name_is_one_broken_literal() {
    let out = errors("1x");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::MalformedNatural);
    // The whole word is the error, so the `x` is not left to be lexed as a
    // name of its own.
    assert_eq!(out[0].span.start, 0);
    assert_eq!(out[0].span.width, 2);

    // A non-ASCII digit is alphanumeric, so it lands here too.
    assert_eq!(errors("1٣")[0].kind, ErrorKind::MalformedNatural);
}

#[test]
fn a_literal_too_large_to_hold_is_rejected() {
    let over = format!("{}0", u128::MAX);
    let out = errors(&over);
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::NaturalTooLarge);
    assert_eq!(out[0].span.width, over.len());
}

#[test]
fn an_unrecognized_character_is_still_its_own_error() {
    let out = errors("@");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
}
