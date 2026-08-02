use std::{iter::Peekable, str::CharIndices};

use crate::tracking::{FileID, Span, Tracked};

pub type Token = Tracked<Kind>;

#[derive(Debug, Clone)]
pub enum Kind {
    Let,
    In,
    Type,
    End,
    With,
    Fn,
    Equal,
    FatArrow,
    /// `->`, the function type arrow. Distinct from [`FatArrow`](Kind::FatArrow),
    /// which introduces a lambda body.
    Arrow,
    Colon,
    Comma,
    Dot,
    /// `..`, the tail of a struct type: the fields not named, absent when the
    /// struct is written closed. Distinct from two [`Dot`](Kind::Dot)s the way
    /// [`FatArrow`](Kind::FatArrow) is distinct from `=` then `>`.
    DotDot,
    /// `?`, marking a struct type's field as one that may or may not be there.
    Question,
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
            // `-` begins nothing on its own — there is no subtraction and no
            // negative literal — so the only thing it can be is the head of an
            // arrow, and a lone one is reported where it was written.
            '-' => {
                chars.next();
                if let Some(&(_, '>')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::Arrow));
                } else {
                    errors.push(Error {
                        span: file_id.span(start, 1),
                        kind: ErrorKind::Unrecognized,
                    });
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
                chars.next();
                // `..` is a struct type's tail; a lone `.` is projection.
                if let Some(&(_, '.')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::DotDot));
                } else {
                    tokens.push(file_id.span(start, 1).track(Kind::Dot));
                }
            }
            '?' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Question));
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
