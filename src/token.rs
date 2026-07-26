use std::{iter::Peekable, str::CharIndices};

use crate::tracking::{FileID, Span, Tracked};

pub type Token = Tracked<Kind>;

#[derive(Debug, Clone)]
pub enum Kind {
    Let,
    In,
    Type,
    Trait,
    For,
    Impl,
    End,
    With,
    Fn,
    Equal,
    FatArrow,
    Colon,
    Comma,
    Dot,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    Identifier(String),
    /// A natural number literal. Bounded by `u128` rather than unbounded like
    /// the naturals themselves; a literal that does not fit is rejected by the
    /// lexer instead of silently wrapping.
    Natural(u128),
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A character that begins no token at all.
    Unrecognized,
    /// A natural literal running into identifier characters, as in `1x`. Read
    /// as one broken literal rather than as a number beside a name, since the
    /// latter is never what was meant.
    MalformedNatural,
    /// A natural literal too large for [`Kind::Natural`] to hold.
    NaturalTooLarge,
}

pub struct Output {
    pub tokens: Vec<Token>,
    pub errors: Vec<Error>,
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Let => f.write_str("let"),
            Kind::In => f.write_str("in"),
            Kind::Type => f.write_str("type"),
            Kind::Trait => f.write_str("trait"),
            Kind::For => f.write_str("for"),
            Kind::Impl => f.write_str("impl"),
            Kind::End => f.write_str("end"),
            Kind::With => f.write_str("with"),
            Kind::Fn => f.write_str("fn"),
            Kind::Equal => f.write_str("="),
            Kind::FatArrow => f.write_str("=>"),
            Kind::Colon => f.write_str(":"),
            Kind::Comma => f.write_str(","),
            Kind::Dot => f.write_str("."),
            Kind::LeftBrace => f.write_str("{"),
            Kind::RightBrace => f.write_str("}"),
            Kind::LeftParen => f.write_str("("),
            Kind::RightParen => f.write_str(")"),
            Kind::Identifier(name) => f.write_str(name),
            Kind::Natural(value) => write!(f, "{value}"),
        }
    }
}

pub fn lex(input: &str, file_id: FileID) -> Output {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    let mut chars = input.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        match c {
            // Whitespace is not significant.
            c if c.is_whitespace() => {
                chars.next();
            }
            '=' => {
                chars.next();
                // `=>` is the function arrow; a lone `=` is assignment.
                if let Some(&(_, '>')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::FatArrow));
                } else {
                    tokens.push(file_id.span(start, 1).track(Kind::Equal));
                }
            }
            ':' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Colon));
                chars.next();
            }
            ',' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Comma));
                chars.next();
            }
            '.' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Dot));
                chars.next();
            }
            '{' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::LeftBrace));
                chars.next();
            }
            '}' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::RightBrace));
                chars.next();
            }
            '(' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::LeftParen));
                chars.next();
            }
            ')' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::RightParen));
                chars.next();
            }
            // Identifiers and keywords: start with a letter or underscore,
            // continue with letters, digits, or underscores.
            c if c.is_alphabetic() || c == '_' => {
                let ident = word(&mut chars);
                // `ident` was built from the source chars, so its UTF-8 byte
                // length is exactly the span width.
                let span = file_id.span(start, ident.len());
                let kind = match ident.as_str() {
                    "let" => Kind::Let,
                    "in" => Kind::In,
                    "type" => Kind::Type,
                    "trait" => Kind::Trait,
                    "for" => Kind::For,
                    "impl" => Kind::Impl,
                    "end" => Kind::End,
                    "with" => Kind::With,
                    "fn" => Kind::Fn,
                    _ => Kind::Identifier(ident),
                };
                tokens.push(span.track(kind));
            }
            // A natural literal runs over the characters an identifier
            // continues with, not just digits, so that `1x` is one malformed
            // literal here rather than a `1` beside an `x` for the parser to
            // make sense of.
            c if c.is_ascii_digit() => {
                let digits = word(&mut chars);
                let span = file_id.span(start, digits.len());
                match natural(&digits) {
                    Ok(value) => tokens.push(span.track(Kind::Natural(value))),
                    Err(kind) => errors.push(Error { span, kind }),
                }
            }
            // Anything else is an unrecognized character.
            _ => {
                errors.push(Error {
                    span: file_id.span(start, c.len_utf8()),
                    kind: ErrorKind::Unrecognized,
                });
                chars.next();
            }
        }
    }

    Output { tokens, errors }
}

/// Consume the run of characters an identifier may continue with. Shared by
/// identifiers and natural literals so that the two can never disagree about
/// where one word ends and the next begins.
fn word(chars: &mut Peekable<CharIndices<'_>>) -> String {
    let mut word = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            chars.next();
        } else {
            break;
        }
    }
    word
}

/// Read a word that started with a digit as a natural number. Non-ASCII digits
/// are rejected along with everything else that is not `0..=9`: they are
/// alphanumeric, so they reach here, and accepting them would make two
/// spellings of one number.
fn natural(digits: &str) -> Result<u128, ErrorKind> {
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(ErrorKind::MalformedNatural);
    }
    digits.parse().map_err(|_| ErrorKind::NaturalTooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
