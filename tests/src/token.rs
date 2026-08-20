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
fn lexes_numeric_literals() {
    assert!(matches!(kinds("0")[..], [Kind::Real(value)] if value == 0.0));
    assert!(matches!(kinds("42")[..], [Kind::Real(value)] if value == 42.0));
    assert!(matches!(kinds("1.25")[..], [Kind::Real(value)] if value == 1.25));
    assert!(matches!(kinds("42i")[..], [Kind::Integer(42)]));
    assert!(matches!(kinds("42n")[..], [Kind::Natural(42)]));
    assert!(matches!(
        kinds(&format!("{}n", u64::MAX))[..],
        [Kind::Natural(u64::MAX)]
    ));
}

#[test]
fn a_natural_is_spanned_and_printed_as_written() {
    let out = lex("let n = 4096n", FileID::GENERATED);
    let last = out.tokens.last().expect("the literal was lexed");
    assert_eq!(last.span.start, 8);
    assert_eq!(last.span.width, 5);
    assert_eq!(last.tracked.to_string(), "4096n");
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
    let over = format!("{}0n", u64::MAX);
    let out = errors(&over);
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::NaturalTooLarge);
    assert_eq!(
        out[0].kind.to_string(),
        "natural number too large to fit in 64 bits"
    );
    assert_eq!(out[0].span.width, over.len());
}

#[test]
fn lexes_real_number_operators() {
    assert!(matches!(
        kinds("-1 + 2 * 3 / 4")[..],
        [
            Kind::Minus,
            Kind::Real(_),
            Kind::Plus,
            Kind::Real(_),
            Kind::Star,
            Kind::Real(_),
            Kind::Slash,
            Kind::Real(_)
        ]
    ));
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
    assert!(matches!(
        kinds("..'r")[..],
        [Kind::DotDot, Kind::Variable(_)]
    ));

    let out = lex("{ a: A, .. }", FileID::GENERATED);
    let dots = &out.tokens[5];
    assert!(matches!(dots.tracked, Kind::DotDot));
    assert_eq!(dots.span.start, 8);
    assert_eq!(dots.span.width, 2);
}

/// `;` separates the statements of a `where` clause and nothing else, so it is
/// one character and one token — never the head of something longer, which is
/// what makes two of them two tokens rather than one the lexer has to tell
/// apart from a `;` beside a `;`.
#[test]
fn lexes_the_statement_separator() {
    assert!(matches!(kinds(";")[..], [Kind::Semicolon]));
    assert!(matches!(
        kinds(";;")[..],
        [Kind::Semicolon, Kind::Semicolon]
    ));
    assert!(matches!(
        &kinds("where 'a = 'b; 'c")[..],
        [
            Kind::Identifier(_),
            Kind::Variable(_),
            Kind::Equal,
            Kind::Variable(_),
            Kind::Semicolon,
            Kind::Variable(_),
        ]
    ));

    let out = lex("a ; b", FileID::GENERATED);
    let semi = &out.tokens[1];
    assert!(matches!(semi.tracked, Kind::Semicolon));
    assert_eq!(semi.span.start, 2);
    assert_eq!(semi.span.width, 1);
}

/// `!=` is one token, the way `->` is: the longer lexeme wins, so a `!` in
/// front of an `=` is this rather than an effect beside an assignment.
#[test]
fn lexes_the_comparison() {
    assert!(matches!(
        kinds("a != b")[..],
        [Kind::Identifier(_), Kind::NotEqual, Kind::Identifier(_)]
    ));
}

/// A `!` in front of a name is an effect, and `!=` is still the comparison: the
/// longer lexeme wins, the way `..` beats two dots, so `a !=b` is one comparison
/// rather than an effect beside an assignment.
///
/// A `!` in front of anything else begins nothing, so it is the lex error a lone
/// `#` is — the row it used to introduce is written `+` now.
#[test]
fn a_sigilled_name_is_an_effect() {
    assert!(matches!(&kinds("!Log")[..], [Kind::EffectLabel(name)] if name == "Log"));
    assert!(matches!(
        &kinds("Nat -> Nat+!Log")[..],
        [_, _, _, Kind::Plus, Kind::EffectLabel(name)] if name == "Log"
    ));
    assert!(matches!(kinds("a !=b")[..], [_, Kind::NotEqual, _]));
    assert!(errors("!Log").is_empty());

    let out = errors("a ! b");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
    assert_eq!(out[0].span.width, 1);
}

/// A `'` in front of a name is a variable of the annotation it is written in,
/// and in front of anything else it begins nothing: the lex error a lone `#`
/// is, for the reason a lone `#` is one.
#[test]
fn lexes_a_variable() {
    assert!(matches!(&kinds("'a")[..], [Kind::Variable(name)] if name == "a"));
    assert!(matches!(&kinds("'rest")[..], [Kind::Variable(name)] if name == "rest"));
    assert!(matches!(&kinds("'_x")[..], [Kind::Variable(name)] if name == "_x"));
    assert_eq!(kinds("'a")[0].to_string(), "'a");
    assert!(errors("'a").is_empty());

    // The whole of what was consumed is underlined, as it is for a tag.
    for (src, width) in [("'", 1), ("' ", 1), ("'1", 2), ("'1abc", 5)] {
        let out = errors(src);
        assert_eq!(out.len(), 1, "{src}: {out:#?}");
        assert_eq!(out[0].kind, ErrorKind::Unrecognized, "{src}");
        assert_eq!(out[0].span.start, 0, "{src}");
        assert_eq!(out[0].span.width, width, "{src}");
    }
}

/// The `+` that hangs a row off an arrow is one byte and its own token, and it
/// is the only thing the character is: there is no addition to tell it from.
#[test]
fn lexes_the_effect_mark() {
    assert!(matches!(kinds("+")[..], [Kind::Plus]));
    assert!(matches!(
        kinds("Nat -> Nat + ..'e")[..],
        [_, _, _, Kind::Plus, Kind::DotDot, Kind::Variable(_)]
    ));
    assert!(errors("+").is_empty());
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
fn a_lone_minus_is_a_token() {
    assert!(matches!(
        tokens_of("A - B")[..],
        [Kind::Identifier(_), Kind::Minus, Kind::Identifier(_)]
    ));
}

#[test]
fn an_unrecognized_character_is_still_its_own_error() {
    let out = errors("@");
    assert_eq!(out.len(), 1, "errors: {out:#?}");
    assert_eq!(out[0].kind, ErrorKind::Unrecognized);
}

#[test]
fn lexes_a_tag_as_one_token() {
    assert!(matches!(&kinds("#Some")[..], [Kind::Tag(name)] if name == "Some"));
    // The `#` is not part of the name, so a tag and an identifier of the same
    // spelling carry the same string.
    assert!(matches!(&kinds("#x1")[..], [Kind::Tag(name)] if name == "x1"));
    assert!(matches!(&kinds("#_a")[..], [Kind::Tag(name)] if name == "_a"));
}

#[test]
fn a_tag_is_spanned_and_printed_as_written() {
    let out = lex("let v = #Some 1n", FileID::GENERATED);
    let tag = &out.tokens[3];
    // The span covers the `#` as well as the name: it is one lexeme, and a
    // reader selecting the case should get all of it.
    assert_eq!(tag.span.start, 8);
    assert_eq!(tag.span.width, 5);
    // And printing writes the `#` back on, so the stream re-lexes to itself
    // rather than to an identifier.
    assert_eq!(tag.tracked.to_string(), "#Some");
}

#[test]
fn a_sigil_that_begins_no_name_is_unrecognized() {
    // The `-` precedent: a character that begins nothing on its own is
    // reported where it was written rather than swallowing what follows.
    //
    // What it did swallow, it underlines. The name runs over the characters an
    // identifier continues with, so `#1abc` is one bad tag rather than a `#`
    // beside something else, and the span is the whole of it — the
    // rule the malformed natural below already keeps. A span narrower than the
    // lexeme points at a character the reader cannot act on and leaves the rest
    // of the mistake unmarked.
    for (src, width) in [("#", 1), ("# ", 1), ("#|", 1), ("#1", 2), ("#1abc", 5)] {
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
    // A case keeps its `#`, so `\#B` is the mark and then a tag.
    assert!(matches!(
        &kinds("\\#B")[..],
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
fn lexes_the_pipeline_as_one_token() {
    assert!(matches!(
        kinds("x |> f")[..],
        [Kind::Identifier(_), Kind::PipeForward, Kind::Identifier(_)]
    ));
}

#[test]
fn lexes_the_case_separator() {
    assert!(matches!(
        &kinds("#A | #B")[..],
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
        kinds("let _ = 1n")[..],
        [Kind::Let, Kind::Underscore, Kind::Equal, Kind::Natural(1)]
    ));

    // Spanned at the one byte it is, and printed back as it.
    let out = lex("let _ = 1n", FileID::GENERATED);
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

/// The tag `#_` is untouched: a tag's name runs over identifier
/// characters, and the keyword rule never sees it.
#[test]
fn the_underscore_tag_is_still_a_tag() {
    assert!(matches!(&kinds("#_")[..], [Kind::Tag(name)] if name == "_"));
    assert!(matches!(
        &kinds("#_ _")[..],
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

/// The three words an effect declaration, a handler and an abort are written
/// with are reserved: they stop being usable as names, which is what makes one
/// token of lookahead enough everywhere they appear.
#[test]
fn the_effect_keywords_are_reserved() {
    assert!(matches!(kinds("effect")[..], [Kind::Effect]));
    assert!(matches!(kinds("handle")[..], [Kind::Handle]));
    assert!(matches!(kinds("raise")[..], [Kind::Raise]));
    // The rule that keeps `matches` a name keeps these apart from words that
    // merely start the same way.
    assert!(matches!(kinds("effects")[..], [Kind::Identifier(_)]));
    assert!(matches!(kinds("handler")[..], [Kind::Identifier(_)]));
    assert!(matches!(kinds("raised")[..], [Kind::Identifier(_)]));
}

/// `return` is not one of them. It heads a handler arm and is an ordinary name
/// everywhere 'else, so the lexer hands it over as the identifier it is and the
/// one position that reads it recognizes it by spelling — the rule `when` and
/// `where` already keep.
#[test]
fn return_is_an_ordinary_identifier() {
    assert!(matches!(&kinds("return")[..], [Kind::Identifier(name)] if name == "return"));
}

/// `::` is one token, the way `..` and `=>` are: the longer lexeme wins, so a
/// path's separator can never be read as an ascription of an ascription.
#[test]
fn a_double_colon_is_one_token() {
    assert!(matches!(kinds("::")[..], [Kind::ColonColon]));
    assert!(matches!(
        &kinds("Math::double")[..],
        [Kind::Identifier(a), Kind::ColonColon, Kind::Identifier(b)]
            if a == "Math" && b == "double"
    ));
    // Spanned as the two characters it is, so a complaint about a path's
    // separator underlines the whole of it.
    let out = lex("a::b", FileID::GENERATED);
    assert_eq!(out.tokens[1].span.start, 1);
    assert_eq!(out.tokens[1].span.width, 2);
    assert_eq!(out.tokens[1].tracked.to_string(), "::");
}

/// Two colons with a space between them are two ascriptions. The rule is about
/// the lexeme rather than about the characters, so nothing here reads across
/// whitespace.
#[test]
fn colons_written_apart_stay_two_colons() {
    assert!(matches!(kinds(": :")[..], [Kind::Colon, Kind::Colon]));
    assert!(matches!(kinds(":")[..], [Kind::Colon]));
}

/// The two words the module grammar reserves, and the rule that keeps `modules`
/// and `bundles` ordinary names — the one `matches` already relies on.
#[test]
fn the_module_keywords_are_reserved() {
    assert!(matches!(kinds("bundle")[..], [Kind::Bundle]));
    assert!(matches!(kinds("module")[..], [Kind::Module]));
    assert!(matches!(&kinds("bundles")[..], [Kind::Identifier(name)] if name == "bundles"));
    assert!(matches!(&kinds("modules")[..], [Kind::Identifier(name)] if name == "modules"));
}

/// A header needs no literal of its own: `0.1.0` is already three naturals with
/// dots between them, which is why a prerelease is simply unwritable.
#[test]
fn a_version_lexes_as_naturals_and_dots() {
    assert!(matches!(
        kinds("bundle demo 0.1.0")[..],
        [
            Kind::Bundle,
            Kind::Identifier(_),
            Kind::Natural(0),
            Kind::Dot,
            Kind::Natural(1),
            Kind::Dot,
            Kind::Natural(0)
        ]
    ));
}

#[test]
fn a_version_component_keeps_all_u64_bits() {
    let source = format!("bundle demo {}.0.0", u64::MAX);
    assert!(matches!(
        kinds(&source)[..],
        [
            Kind::Bundle,
            Kind::Identifier(_),
            Kind::Natural(value),
            Kind::Dot,
            Kind::Natural(0),
            Kind::Dot,
            Kind::Natural(0)
        ] if value == u64::MAX
    ));
}
