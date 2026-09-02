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
    assert_eq!(
        errors("1.5n")[0].kind,
        ErrorKind::DecimalWithWholeSuffix { suffix: 'n' }
    );
    assert_eq!(
        errors("1.5i")[0].kind,
        ErrorKind::DecimalWithWholeSuffix { suffix: 'i' }
    );
}

#[test]
fn strings_finish_and_fail_at_every_boundary() {
    assert!(
        matches!(&kinds("\"hello\\nworld\"")[..], [Kind::String(value)] if value == "hello\nworld")
    );
    assert!(
        matches!(&kinds(r#""\"\\\n\r\t""#)[..], [Kind::String(value)] if value == "\"\\\n\r\t")
    );
    for malformed in ["\"unterminated", "\"slash\\"] {
        assert_eq!(errors(malformed)[0].kind, ErrorKind::MissingClosingQuote);
    }
    assert_eq!(
        errors("\"bad\\q\"")[0].kind,
        ErrorKind::UnknownStringEscape { escape: 'q' }
    );
}

#[test]
fn strings_cannot_span_lines_and_recover_at_the_next_declaration() {
    let out = lex("\"first line\nlet recovered = 1n", FileID::GENERATED);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::MissingClosingQuote);
    assert!(matches!(out.tokens[0].tracked, Kind::Invalid));
    assert!(matches!(out.tokens[1].tracked, Kind::Let));
    assert!(matches!(&out.tokens[2].tracked, Kind::Identifier(name) if name == "recovered"));
    assert!(matches!(out.tokens[3].tracked, Kind::Equal));
    assert!(matches!(out.tokens[4].tracked, Kind::Natural(1)));
}

#[test]
fn unknown_escapes_make_one_invalid_lexeme_and_preserve_following_tokens() {
    let out = lex("\"bad\\q\\z\\\"tail\" let good = 2n", FileID::GENERATED);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(
        out.errors[0].kind,
        ErrorKind::UnknownStringEscape { escape: 'q' }
    );
    assert!(matches!(out.tokens[0].tracked, Kind::Invalid));
    assert!(matches!(out.tokens[1].tracked, Kind::Let));
    assert!(matches!(&out.tokens[2].tracked, Kind::Identifier(name) if name == "good"));
    assert!(matches!(out.tokens[3].tracked, Kind::Equal));
    assert!(matches!(out.tokens[4].tracked, Kind::Natural(2)));
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
    assert_eq!(out[0].kind, ErrorKind::NumberFollowedByName);
    // The whole word is the error, so the `x` is not left to be lexed as a
    // name of its own.
    assert_eq!(out[0].span.start, 0);
    assert_eq!(out[0].span.width, 2);
    assert!(matches!(tokens_of("1x")[..], [Kind::Invalid]));

    // A non-ASCII digit is alphanumeric, so it lands here too.
    assert_eq!(errors("1٣")[0].kind, ErrorKind::NumberFollowedByName);
}

#[test]
fn each_numeric_category_reports_its_own_failure() {
    let natural = format!("{}0n", u64::MAX);
    let integer = format!("{}0i", i64::MAX);
    let real = format!("1{}", "0".repeat(400));

    for (src, expected, message) in [
        (
            natural.as_str(),
            ErrorKind::NaturalTooLarge,
            "this whole number is too large",
        ),
        (
            integer.as_str(),
            ErrorKind::IntegerTooLarge,
            "this integer is too large",
        ),
        (
            real.as_str(),
            ErrorKind::RealTooLarge,
            "this number is too large",
        ),
    ] {
        let out = lex(src, FileID::GENERATED);
        assert_eq!(out.errors.len(), 1, "errors for {src}: {:#?}", out.errors);
        assert_eq!(out.errors[0].kind, expected);
        assert_eq!(out.errors[0].kind.to_string(), message);
        assert_eq!(out.errors[0].span.width, src.len());
        assert!(matches!(out.tokens[..], [ref token] if matches!(token.tracked, Kind::Invalid)));
    }
}

#[test]
fn lexes_numeric_projection_fields() {
    assert!(matches!(
        kinds("pair.0 pair. 001")[..],
        [
            Kind::Identifier(_),
            Kind::Dot,
            Kind::NumericField(0),
            Kind::Identifier(_),
            Kind::Dot,
            Kind::NumericField(1),
        ]
    ));

    let field = lex("pair.001", FileID::GENERATED).tokens.pop().unwrap();
    assert_eq!(field.span.start, 5);
    assert_eq!(field.span.width, 3);
}

#[test]
fn lexes_numeric_struct_fields_without_changing_real_expressions() {
    assert!(matches!(
        kinds("{ 001: 2, x: (3, 4), inner: { \\05, 6: 7 } }")[..],
        [
            Kind::LeftBrace,
            Kind::NumericField(1),
            Kind::Colon,
            Kind::Real(_),
            Kind::Comma,
            Kind::Identifier(_),
            Kind::Colon,
            Kind::LeftParen,
            Kind::Real(_),
            Kind::Comma,
            Kind::Real(_),
            Kind::RightParen,
            Kind::Comma,
            Kind::Identifier(_),
            Kind::Colon,
            Kind::LeftBrace,
            Kind::Backslash,
            Kind::NumericField(5),
            Kind::Comma,
            Kind::NumericField(6),
            Kind::Colon,
            Kind::Real(_),
            Kind::RightBrace,
            Kind::RightBrace,
        ]
    ));

    // Numeric expressions in field values and tuples retain suffixless-real
    // semantics; only the label position is contextual.
    assert!(matches!(
        kinds("{ 0: 1 + 2, x: (3, 4) }")[..],
        [
            Kind::LeftBrace,
            Kind::NumericField(0),
            Kind::Colon,
            Kind::Real(_),
            Kind::Plus,
            Kind::Real(_),
            Kind::Comma,
            Kind::Identifier(_),
            Kind::Colon,
            Kind::LeftParen,
            Kind::Real(_),
            Kind::Comma,
            Kind::Real(_),
            Kind::RightParen,
            Kind::RightBrace,
        ]
    ));
}

#[test]
fn numeric_struct_fields_reject_number_forms_and_overflow() {
    for malformed in ["{0n: x}", "{0i: x}", "{0.0: x}", "{0²: x}"] {
        let out = lex(malformed, FileID::GENERATED);
        assert_eq!(
            out.errors.len(),
            1,
            "errors for {malformed:?}: {:#?}",
            out.errors
        );
        assert_eq!(out.errors[0].kind, ErrorKind::MalformedNumericField);
        assert!(matches!(out.tokens[1].tracked, Kind::Invalid));
    }

    let over = format!("{{{}0: x}}", u64::MAX);
    assert_eq!(errors(&over)[0].kind, ErrorKind::NumericFieldTooLarge);
}

#[test]
fn numeric_projection_fields_reject_number_forms() {
    for malformed in ["pair.0n", "pair.0i", "pair.0.0", "pair.0²"] {
        let out = lex(malformed, FileID::GENERATED);
        assert_eq!(
            out.errors.len(),
            1,
            "errors for {malformed:?}: {:#?}",
            out.errors
        );
        assert_eq!(out.errors[0].kind, ErrorKind::MalformedNumericField);
        assert!(matches!(out.tokens[2].tracked, Kind::Invalid));
        assert_eq!(
            out.errors[0].span.width,
            malformed.len() - "pair.".len(),
            "the malformed field must consume its whole Unicode suffix"
        );
    }

    let over = format!("pair.{}0", u64::MAX);
    assert_eq!(errors(&over)[0].kind, ErrorKind::NumericFieldTooLarge);
    // Away from a dot the same spelling remains an ordinary real literal.
    assert!(matches!(kinds("001")[..], [Kind::Real(value)] if value == 1.0));
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
fn lexes_boolean_operators() {
    assert!(matches!(
        kinds("not true and false or true xor false")[..],
        [
            Kind::Not,
            Kind::Boolean(true),
            Kind::And,
            Kind::Boolean(false),
            Kind::Or,
            Kind::Boolean(true),
            Kind::Xor,
            Kind::Boolean(false),
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
    assert_eq!(out[0].kind, ErrorKind::MalformedEffectLabel);
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
        assert_eq!(out[0].kind, ErrorKind::MalformedVariable, "{src}");
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
    assert_eq!(out[0].kind, ErrorKind::InvalidCharacter { character: '?' });
    assert_eq!(out[0].span.start, 1);
    assert_eq!(out[0].span.width, 1);
    // And nothing in the stream stands for it: the rest lexes as it always did.
    assert!(matches!(
        tokens_of("a?: A")[..],
        [
            Kind::Identifier(_),
            Kind::Invalid,
            Kind::Colon,
            Kind::Identifier(_)
        ]
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
    assert_eq!(out[0].kind, ErrorKind::InvalidCharacter { character: '@' });
    assert!(matches!(
        tokens_of("@;")[..],
        [Kind::Invalid, Kind::Semicolon]
    ));
}

#[test]
fn every_malformed_lexeme_remains_as_an_invalid_token() {
    for src in ["@", "#1x", "!1x", "'1x", "1name", "1.5n", "\"bad\\q\""] {
        let out = lex(src, FileID::GENERATED);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(out.tokens[..], [ref token] if matches!(token.tracked, Kind::Invalid)),
            "{src:?}: {:#?}",
            out.tokens
        );
    }
}

#[test]
fn lexes_a_tag_as_one_token() {
    assert!(matches!(&kinds("#Some")[..], [Kind::Tag(name)] if name == "Some"));
    // The `#` is not part of the name, so a tag and an identifier of the same
    // spelling carry the same string.
    assert!(matches!(&kinds("#x1")[..], [Kind::Tag(name)] if name == "x1"));
    assert!(matches!(&kinds("#_a")[..], [Kind::Tag(name)] if name == "_a"));
}

/// A quoted tag is still one tag token: its `#` must touch the string, and the
/// ordinary string decoder supplies the structural label carried by the token.
#[test]
fn lexes_quoted_tags_as_one_decoded_token() {
    for (src, expected) in [
        (r###"#"Some case""###, "Some case"),
        (r###"#"""###, ""),
        (r###"#"λ case""###, "λ case"),
        (r###"#"line\n\"quote\"\\tail""###, "line\n\"quote\"\\tail"),
    ] {
        let out = lex(src, FileID::GENERATED);
        assert!(out.errors.is_empty(), "{src:?}: {:#?}", out.errors);
        assert_eq!(out.tokens.len(), 1, "{src:?}");
        assert!(matches!(&out.tokens[0].tracked, Kind::Tag(name) if name == expected));
        assert_eq!(out.tokens[0].span.start, 0, "{src:?}");
        assert_eq!(out.tokens[0].span.width, src.len(), "{src:?}");
    }
}

/// Quoting is part of the tag lexeme, not whitespace-sensitive punctuation
/// followed by an ordinary string. Malformed quoted tags report the same
/// string error and cover the complete sigilled lexeme.
#[test]
fn a_quoted_tag_must_be_adjacent_and_well_formed() {
    let separated = lex(r###"# "Case""###, FileID::GENERATED);
    assert_eq!(separated.errors.len(), 1, "{:#?}", separated.errors);
    assert_eq!(separated.errors[0].kind, ErrorKind::MalformedTag);
    assert_eq!(separated.errors[0].span.width, 1);
    assert!(matches!(
        &separated.tokens[..],
        [invalid, token]
            if matches!(invalid.tracked, Kind::Invalid)
                && matches!(&token.tracked, Kind::String(value) if value == "Case")
    ));

    let unterminated = r###"#"unterminated"###;
    let out = lex(unterminated, FileID::GENERATED);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::MissingClosingQuote);
    assert_eq!(out.errors[0].span.start, 0);
    assert_eq!(out.errors[0].span.width, unterminated.len());
    assert!(matches!(&out.tokens[..], [token] if matches!(token.tracked, Kind::Invalid)));

    // The quoted tag remains one malformed lexeme after an unsupported escape,
    // including when an escaped quote occurs before its real closing quote.
    for malformed_escape in [
        r###"#"bad\q""###,
        r###"#"bad\q\"still inside""###,
        r###"#"bad\q\\still inside""###,
        r###"#"bad\qwithout a close"###,
        r###"#"bad\qtrailing\"###,
    ] {
        let out = lex(malformed_escape, FileID::GENERATED);
        assert_eq!(
            out.errors.len(),
            1,
            "{malformed_escape:?}: {:#?}",
            out.errors
        );
        assert_eq!(
            out.errors[0].kind,
            ErrorKind::UnknownStringEscape { escape: 'q' }
        );
        assert_eq!(out.errors[0].span.start, 0);
        assert_eq!(out.errors[0].span.width, malformed_escape.len());
        assert!(matches!(&out.tokens[..], [token] if matches!(token.tracked, Kind::Invalid)));
    }
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
        assert_eq!(out[0].kind, ErrorKind::MalformedTag, "{src}");
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

/// `module` is reserved, while words that merely start the same way and the
/// former `bundle` keyword remain ordinary names.
#[test]
fn the_module_keyword_is_reserved() {
    assert!(matches!(&kinds("bundle")[..], [Kind::Identifier(name)] if name == "bundle"));
    assert!(matches!(kinds("module")[..], [Kind::Module]));
    assert!(matches!(&kinds("bundles")[..], [Kind::Identifier(name)] if name == "bundles"));
    assert!(matches!(&kinds("modules")[..], [Kind::Identifier(name)] if name == "modules"));
}

/// Conditionals spend three exact words. The lexer reads a complete word
/// before deciding whether it is reserved, so names which merely contain one
/// of them remain available.
#[test]
fn conditional_keywords_are_reserved_as_exact_words() {
    assert!(matches!(
        kinds("if p then a else b end")[..],
        [
            Kind::If,
            Kind::Identifier(_),
            Kind::Then,
            Kind::Identifier(_),
            Kind::Else,
            Kind::Identifier(_),
            Kind::End,
        ]
    ));

    for name in ["iffy", "s_if", "thenable", "athen", "elsewhere", "orelse"] {
        assert!(
            matches!(&kinds(name)[..], [Kind::Identifier(found)] if found == name),
            "{name}"
        );
    }
}

#[test]
fn extern_is_reserved_as_a_declaration_keyword() {
    assert!(
        matches!(&kinds("extern log")[..], [Kind::Extern, Kind::Identifier(name)] if name == "log")
    );
    assert!(matches!(&kinds("external")[..], [Kind::Identifier(name)] if name == "external"));
}

#[test]
fn a_sigil_followed_by_a_non_identifier_word_is_rejected() {
    let out = lex("'1", FileID::GENERATED);
    assert!(!out.errors.is_empty());
    let out = lex("1.a", FileID::GENERATED);
    assert!(out.errors.is_empty());
}
