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
    /// `match`, opening a `match <expr> with <arms> end`. The `with` and `end`
    /// around it were reserved long before this keyword existed; this is what
    /// they were reserved for.
    Match,
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
    /// `!=`, the "these two presences differ" of a `where` clause. One token
    /// rather than a `!` beside an `=`, the way [`Arrow`](Kind::Arrow) is one
    /// token: a lone `!` begins nothing, so the only thing it can be is the
    /// head of this.
    NotEqual,
    /// `\`, marking a struct type's field — or a sum type's case — as one that
    /// is definitely *not* there: the `..` beside it may not stand for the
    /// label. A bare punctuation token, so `\ y` lexes the same as `\y` — the
    /// same separation `..` keeps from the name after it.
    Backslash,
    /// `|`, separating the cases of a sum type. Also the whole of the empty
    /// sum, which is the one type written with nothing but punctuation.
    Pipe,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    Identifier(String),
    /// `_`, the wildcard: a value being thrown away. Its own kind rather than
    /// an identifier because it binds nothing and can never be referred to —
    /// the parser has to tell a discard from a name, and a lexeme the two
    /// could share would leave it guessing. Only the exact word: `__`, `_x`
    /// and `_1` remain ordinary identifiers, read by the keyword rule that
    /// already keeps `matches` a name.
    Underscore,
    /// `` `Some `` — one of a sum type's cases, named. One token rather than a
    /// backtick beside a name, because that is what it is: a label with no
    /// spaces allowed inside it, and a span a reader can select in one go. The
    /// backtick is not part of the name it carries, the way a struct's braces
    /// are not part of its field names.
    Tag(String),
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
            // `!` begins nothing on its own — there is no negation operator —
            // so, like `-`, the only thing it can be is the head of something
            // longer, and a lone one is reported where it was written.
            '!' => {
                chars.next();
                if let Some(&(_, '=')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::NotEqual));
                } else {
                    errors.push(Error {
                        span: file_id.span(start, 1),
                        kind: ErrorKind::Unrecognized,
                    });
                }
            }
            '|' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Pipe));
                chars.next();
            }
            // Never a lex error, unlike `-` and the backtick: what may follow a
            // `\` is the parser's business.
            '\\' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Backslash));
                chars.next();
            }
            // A backtick begins nothing on its own, so — like `-` — the only
            // thing it can be is the head of something longer, and a lone one
            // is reported where it was written. The name runs over the
            // characters an identifier does, so `` `1x `` is one bad tag here
            // rather than a tag beside a number.
            '`' => {
                chars.next();
                let name = word(&mut chars);
                match name.chars().next() {
                    Some(c) if c.is_alphabetic() || c == '_' => {
                        // The backtick is one byte and `name` was built from
                        // the source, so the span is exactly what was written.
                        let span = file_id.span(start, name.len() + 1);
                        tokens.push(span.track(Kind::Tag(name)));
                    }
                    // The whole of what was consumed, not just the backtick:
                    // `` `1x `` was read as one lexeme, so it is underlined as
                    // one. A span narrower than what the lexer ate points the
                    // reader at a character that is not the mistake and leaves
                    // the rest of it unmarked. The natural literal below spans
                    // its own lexeme for the same reason.
                    _ => errors.push(Error {
                        span: file_id.span(start, name.len() + 1),
                        kind: ErrorKind::Unrecognized,
                    }),
                }
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
                    "match" => Kind::Match,
                    "fn" => Kind::Fn,
                    "_" => Kind::Underscore,
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
