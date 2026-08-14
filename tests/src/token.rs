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

/// The tokens of a source that also has lex errors, for the one thing that
/// wants both halves of what the lexer made of a line.
fn tokens_of(src: &str) -> Vec<Kind> {
    lex(src, FileID::GENERATED)
        .tokens
        .into_iter()
        .map(|token| token.tracked)
        .collect()
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
fn lexes_the_two_arrows_apart() {
    assert!(matches!(kinds("->")[..], [Kind::Arrow]));
    assert!(matches!(kinds("=>")[..], [Kind::FatArrow]));
    assert!(matches!(
        kinds("A -> B")[..],
        [Kind::Identifier(_), Kind::Arrow, Kind::Identifier(_)]
    ));

    let out = lex("A -> B", FileID::GENERATED);
    assert_eq!(out.tokens[1].span.start, 2);
    assert_eq!(out.tokens[1].span.width, 2);
}

#[test]
fn lexes_the_two_dots_apart() {
    assert!(matches!(kinds(".")[..], [Kind::Dot]));
    assert!(matches!(kinds("..")[..], [Kind::DotDot]));
    // Three dots are a tail and then a projection dot, which no production
    // accepts side by side — the parser reports it, not the lexer.
    assert!(matches!(kinds("...")[..], [Kind::DotDot, Kind::Dot]));
    assert!(matches!(
        kinds("p.x")[..],
        [Kind::Identifier(_), Kind::Dot, Kind::Identifier(_)]
    ));
    assert!(matches!(
        kinds("..r")[..],
        [Kind::DotDot, Kind::Identifier(_)]
    ));

    let out = lex("{ a: A, .. }", FileID::GENERATED);
    let dots = &out.tokens[5];
    assert!(matches!(dots.tracked, Kind::DotDot));
    assert_eq!(dots.span.start, 8);
    assert_eq!(dots.span.width, 2);
}

/// `!=` is one token, the way `->` is: a lone `!` begins nothing, so the only
/// thing it can be is the head of this.
#[test]
fn lexes_the_comparison() {
    assert!(matches!(
        kinds("a != b")[..],
        [Kind::Identifier(_), Kind::NotEqual, Kind::Identifier(_)]
    ));
}

/// A lone `!` is reported at the `!` itself, for the reason a lone `-` is.
#[test]
fn a_lone_bang_is_unrecognized() {
    let out = errors("a ! b");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
    assert_eq!(out[0].span.start, 2);
    assert_eq!(out[0].span.width, 1);
}

/// The `?` that used to mark an optional field is gone from the language, so
/// the character begins no token at all and is reported where it was written.
#[test]
fn the_question_mark_is_no_longer_a_token() {
    let out = errors("a?: A");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
    assert_eq!(out[0].span.start, 1);
    assert_eq!(out[0].span.width, 1);
    // And nothing in the stream stands for it: the rest lexes as it always did.
    assert!(matches!(
        tokens_of("a?: A")[..],
        [Kind::Identifier(_), Kind::Colon, Kind::Identifier(_)]
    ));
}

#[test]
fn a_lone_minus_is_unrecognized() {
    // Nothing else in the grammar starts with `-`, so the head of a
    // half-written arrow is reported at the `-` itself.
    let out = errors("A - B");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
    assert_eq!(out[0].span.start, 2);
    assert_eq!(out[0].span.width, 1);
    // The names on either side are still lexed.
    assert_eq!(lex("A - B", FileID::GENERATED).tokens.len(), 2);
}

#[test]
fn an_unrecognized_character_is_still_its_own_error() {
    let out = errors("@");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
}

#[test]
fn lexes_a_tag_as_one_token() {
    assert!(matches!(&kinds("`Some")[..], [Kind::Tag(name)] if name == "Some"));
    // The backtick is not part of the name, so a tag and an identifier of the
    // same spelling carry the same string.
    assert!(matches!(&kinds("`x1")[..], [Kind::Tag(name)] if name == "x1"));
    assert!(matches!(&kinds("`_a")[..], [Kind::Tag(name)] if name == "_a"));
}

#[test]
fn a_tag_is_spanned_and_printed_as_written() {
    let out = lex("let v = `Some 1", FileID::GENERATED);
    let tag = &out.tokens[3];
    // The span covers the backtick as well as the name: it is one lexeme, and
    // a reader selecting the case should get all of it.
    assert_eq!(tag.span.start, 8);
    assert_eq!(tag.span.width, 5);
    // And printing writes the backtick back on, so the stream re-lexes to
    // itself rather than to an identifier.
    assert_eq!(tag.tracked.to_string(), "`Some");
}

#[test]
fn a_backtick_that_begins_no_name_is_unrecognized() {
    // The `-` precedent: a character that begins nothing on its own is
    // reported where it was written rather than swallowing what follows.
    //
    // What it did swallow, it underlines. The name runs over the characters an
    // identifier continues with, so `` `1abc `` is one bad tag rather than a
    // backtick beside something else, and the span is the whole of it — the
    // rule the malformed natural below already keeps. A span narrower than the
    // lexeme points at a character the reader cannot act on and leaves the rest
    // of the mistake unmarked.
    for (src, width) in [("`", 1), ("` ", 1), ("`|", 1), ("`1", 2), ("`1abc", 5)] {
        let out = errors(src);
        assert_eq!(out.len(), 1, "{src}: {out:#?}");
        assert_eq!(out[0].kind, ErrorKind::Unrecognized, "{src}");
        assert_eq!(out[0].span.start, 0, "{src}");
        assert_eq!(out[0].span.width, width, "{src}");
    }
}

#[test]
fn lexes_the_backslash() {
    // One byte, and never a lex error on its own: what may follow a `\` is
    // the parser's business.
    assert!(matches!(kinds("\\")[..], [Kind::Backslash]));
    // `\y` is the mark and then the name, two tokens — the same separation
    // `..` keeps from the name after it — so `\ y` lexes identically.
    assert!(matches!(
        &kinds("\\y")[..],
        [Kind::Backslash, Kind::Identifier(name)] if name == "y"
    ));
    assert!(matches!(
        &kinds("\\ y")[..],
        [Kind::Backslash, Kind::Identifier(name)] if name == "y"
    ));
    // A case keeps its backtick, so `` \`B `` is the mark and then a tag.
    assert!(matches!(
        &kinds("\\`B")[..],
        [Kind::Backslash, Kind::Tag(name)] if name == "B"
    ));

    let out = lex("{ \\y, .. }", FileID::GENERATED);
    let slash = &out.tokens[1];
    assert!(matches!(slash.tracked, Kind::Backslash));
    assert_eq!(slash.span.start, 2);
    assert_eq!(slash.span.width, 1);
    // Printing writes the `\` back, so the stream re-lexes to itself.
    assert_eq!(slash.tracked.to_string(), "\\");
}

#[test]
fn lexes_the_case_separator() {
    assert!(matches!(
        &kinds("`A | `B")[..],
        [Kind::Tag(_), Kind::Pipe, Kind::Tag(_)]
    ));
    // The empty sum is one token and no name at all.
    assert!(matches!(kinds("|")[..], [Kind::Pipe]));
}

/// `match` is a keyword now — the spending of the reserved `with` and `end` —
/// so it lexes as its own kind and can no longer be an identifier.
#[test]
fn match_lexes_as_a_keyword() {
    assert!(matches!(kinds("match")[..], [Kind::Match]));
    assert!(matches!(
        kinds("match x with end")[..],
        [Kind::Match, Kind::Identifier(_), Kind::With, Kind::End]
    ));
}

/// `_` is its own token now: a discard, not a name. Only the exact word — the
/// lexer reads whole words, so nothing shorter than the whole of `__` can
/// change what it is.
#[test]
fn a_lone_underscore_lexes_as_the_wildcard() {
    assert!(matches!(kinds("_")[..], [Kind::Underscore]));
    assert!(matches!(
        kinds("let _ = 1")[..],
        [Kind::Let, Kind::Underscore, Kind::Equal, Kind::Natural(1)]
    ));

    // Spanned at the one byte it is, and printed back as it.
    let out = lex("let _ = 1", FileID::GENERATED);
    let wild = &out.tokens[1];
    assert_eq!(wild.span.start, 4);
    assert_eq!(wild.span.width, 1);
    assert_eq!(wild.tracked.to_string(), "_");
}

/// Words that merely contain underscores are the identifiers they always
/// were: `__`, `_x`, `x_` and `_1` all still name things.
#[test]
fn words_of_underscores_are_still_names() {
    for word in ["__", "_x", "x_", "_1"] {
        assert!(
            matches!(&kinds(word)[..], [Kind::Identifier(name)] if name == word),
            "{word}"
        );
    }
}

/// The tag `` `_ `` is untouched: a tag's name runs over identifier
/// characters, and the keyword rule never sees it.
#[test]
fn the_underscore_tag_is_still_a_tag() {
    assert!(matches!(&kinds("`_")[..], [Kind::Tag(name)] if name == "_"));
    assert!(matches!(
        &kinds("`_ _")[..],
        [Kind::Tag(name), Kind::Underscore] if name == "_"
    ));
}

/// Only the exact word is the keyword: a name that merely starts with it is
/// still a name, because the lexer reads whole words.
#[test]
fn names_containing_match_are_still_names() {
    assert!(matches!(&kinds("matches")[..], [Kind::Identifier(name)] if name == "matches"));
    assert!(matches!(&kinds("matchbox")[..], [Kind::Identifier(name)] if name == "matchbox"));
    assert!(matches!(&kinds("rematch")[..], [Kind::Identifier(name)] if name == "rematch"));
}
