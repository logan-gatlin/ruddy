use crate::engine::{Eng, Source};
use crate::parser::token::{Keyword, Operator, Punct, TokenKind};
use crate::parser::{lex_diagnostics, lex_source};

fn kinds(source_text: &str) -> (Vec<TokenKind>, Vec<String>) {
    let db = Eng::default();
    let source = Source::new(&db, "test.hc".to_owned(), source_text.to_owned());

    let lexed = lex_source(&db, source);
    let kinds = lexed.tokens.into_iter().map(|token| token.kind).collect();
    let messages = lex_diagnostics(&db, source)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    (kinds, messages)
}

#[test]
fn lexes_keywords_and_identifiers() {
    let (kinds, diagnostics) = kinds("bundle demo use root as alias");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::Keyword(Keyword::Bundle),
            TokenKind::Ident,
            TokenKind::Keyword(Keyword::Use),
            TokenKind::Keyword(Keyword::Root),
            TokenKind::Keyword(Keyword::As),
            TokenKind::Ident,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn lexes_unicode_identifiers() {
    let (kinds, diagnostics) = kinds("let λ = 漢字");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::Keyword(Keyword::Let),
            TokenKind::Ident,
            TokenKind::Punct(Punct::Equals),
            TokenKind::Ident,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn lexes_operators_and_punctuation_with_longest_match() {
    let (kinds, diagnostics) =
        kinds(":: .. => -> == != <= >= << >> |> +> *> + - * / < > ; . | = : , ( ) { } [ ] ~ $");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::Operator(Operator::PathSep),
            TokenKind::Operator(Operator::Spread),
            TokenKind::Operator(Operator::FatArrow),
            TokenKind::Operator(Operator::Arrow),
            TokenKind::Operator(Operator::EqualEqual),
            TokenKind::Operator(Operator::BangEqual),
            TokenKind::Operator(Operator::LessEqual),
            TokenKind::Operator(Operator::GreaterEqual),
            TokenKind::Operator(Operator::ComposeLeft),
            TokenKind::Operator(Operator::ComposeRight),
            TokenKind::Operator(Operator::PipeRight),
            TokenKind::Operator(Operator::PlusPipe),
            TokenKind::Operator(Operator::StarPipe),
            TokenKind::Operator(Operator::Plus),
            TokenKind::Operator(Operator::Minus),
            TokenKind::Operator(Operator::Star),
            TokenKind::Operator(Operator::Slash),
            TokenKind::Operator(Operator::Less),
            TokenKind::Operator(Operator::Greater),
            TokenKind::Operator(Operator::Semicolon),
            TokenKind::Punct(Punct::Dot),
            TokenKind::Punct(Punct::Pipe),
            TokenKind::Punct(Punct::Equals),
            TokenKind::Punct(Punct::Colon),
            TokenKind::Punct(Punct::Comma),
            TokenKind::Punct(Punct::LParen),
            TokenKind::Punct(Punct::RParen),
            TokenKind::Punct(Punct::LBrace),
            TokenKind::Punct(Punct::RBrace),
            TokenKind::Punct(Punct::LBracket),
            TokenKind::Punct(Punct::RBracket),
            TokenKind::Punct(Punct::Tilde),
            TokenKind::Punct(Punct::Dollar),
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn skips_line_and_nested_block_comments() {
    let (kinds, diagnostics) = kinds("let x = 1 -- trailing\n(* outer (* inner *) done *) in");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::Keyword(Keyword::Let),
            TokenKind::Ident,
            TokenKind::Punct(Punct::Equals),
            TokenKind::IntegerLiteral,
            TokenKind::Keyword(Keyword::In),
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn lexes_numeric_literals() {
    let (kinds, diagnostics) = kinds("0 1_024 0b1010 0o755 0xFF 99n 0x10n 1.5 2e10 3.5e-2");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntegerLiteral,
            TokenKind::IntegerLiteral,
            TokenKind::IntegerLiteral,
            TokenKind::IntegerLiteral,
            TokenKind::IntegerLiteral,
            TokenKind::NaturalLiteral,
            TokenKind::NaturalLiteral,
            TokenKind::RealLiteral,
            TokenKind::RealLiteral,
            TokenKind::RealLiteral,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn lexes_string_glyph_and_format_literals() {
    let (kinds, diagnostics) = kinds("\"ok\\n\" 'x' `{}{{}}`");
    assert!(diagnostics.is_empty());
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral,
            TokenKind::GlyphLiteral,
            TokenKind::FormatStringLiteral,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn reports_unterminated_block_comment() {
    let (kinds, diagnostics) = kinds("(* never closes");
    assert_eq!(kinds, vec![TokenKind::EndOfFile]);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("unterminated block comment"));
}

#[test]
fn reports_invalid_escape_sequence() {
    let (kinds, diagnostics) = kinds("\"bad\\q\"");
    assert_eq!(kinds, vec![TokenKind::Error, TokenKind::EndOfFile,]);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("invalid escape sequence"));
}

#[test]
fn reports_unexpected_character() {
    let (kinds, diagnostics) = kinds("@");
    assert_eq!(kinds, vec![TokenKind::Error, TokenKind::EndOfFile,]);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("unexpected character"));
}
