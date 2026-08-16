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
    /// `effect`, opening an effect declaration.
    Effect,
    /// `handle`, opening a `handle <expr> with <arms> end` — the `with` and
    /// `end` a `match` already uses, around a different set of arms.
    Handle,
    /// `raise`, aborting to the handler around it.
    ///
    /// Reserved rather than contextual, unlike the `return` at a handler arm's
    /// head: a `raise` may sit anywhere an expression may, so there is no one
    /// position that could read it and leave the word a name everywhere else.
    Raise,
    Equal,
    FatArrow,
    /// `->`, the function type arrow. Distinct from [`FatArrow`](Kind::FatArrow),
    /// which introduces a lambda body.
    Arrow,
    Colon,
    Comma,
    /// `;`, separating the statements of a `where` clause. Nothing else in the
    /// language writes one — a definition ends where the next `let` or `type`
    /// begins — so this is the whole of what it is for.
    Semicolon,
    Dot,
    /// `..`, the tail of a struct type: the fields not named, absent when the
    /// struct is written closed. Distinct from two [`Dot`](Kind::Dot)s the way
    /// [`FatArrow`](Kind::FatArrow) is distinct from `=` then `>`.
    DotDot,
    /// `!=`, the "these two presences differ" of a `where` clause. One token
    /// rather than a `!` beside an `=`, the way [`Arrow`](Kind::Arrow) is one
    /// token: the longer lexeme wins, so a `!` followed by an `=` is this and
    /// never an [`Effect`](Kind::Effect) beside an assignment.
    NotEqual,
    /// `+`, introducing the effect row an arrow carries: `A -> B + !Log`.
    ///
    /// The whole of what it is for. There is no addition and no other use of
    /// the character, so a `+` anywhere else is the unexpected token it looks
    /// like.
    Plus,
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
    /// `#Some` — one of a sum type's cases, named. One token rather than a `#`
    /// beside a name, because that is what it is: a label with no spaces
    /// allowed inside it, and a span a reader can select in one go. The `#` is
    /// not part of the name it carries, the way a struct's braces are not part
    /// of its field names.
    Tag(String),
    /// `!Log` — an effect, named. A [`Tag`](Kind::Tag)'s twin in every way but
    /// the character that heads it, and one token for the same reason: a label
    /// with no spaces inside it and a span a reader can select in one go.
    ///
    /// Its own kind rather than a `Tag`, because the two name different things
    /// and are never written in the same position: a tag is a case a value may
    /// be, and an effect is something a function may do. Sharing a kind would
    /// leave the parser to tell them apart by where it stood, and the two rows
    /// they build are already told apart everywhere else — see
    /// [`types::Shape`](crate::types::Shape).
    ///
    /// The `!` is not part of the name it carries, exactly as a tag's `#` is
    /// not. An effect is written bare where it is declared, and both spellings
    /// name one symbol.
    EffectLabel(String),
    /// `'a` — a variable: a type this definition's caller picks, the rest of a
    /// row, the presence a `when` names, or one of the parameters a `type`
    /// declaration's header binds.
    ///
    /// The third sigil, and the one thing every use of it has in common is that
    /// something outside the type decides what it stands for. A bare name in a
    /// type is a name that has to resolve — a declared type, a primitive — so a
    /// variable needs a mark of its own to be told from one, and the mark is
    /// what lets it be introduced where it is used. Two annotations that each
    /// write `'a` write two variables; the scope is the one annotation, or the
    /// one declaration, which is what makes a declaration statement
    /// unnecessary.
    Variable(String),
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
            ';' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Semicolon));
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
            // `!=` is the comparison of a `where` clause; a `!` in front of a
            // name is an effect. The longer lexeme wins, so `a !=b` is one
            // comparison rather than an effect beside an assignment.
            '!' => {
                if let Some((_, '=')) = chars.clone().nth(1) {
                    chars.next();
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::NotEqual));
                } else {
                    let (kind, width) = sigilled(&mut chars, Kind::EffectLabel);
                    let span = file_id.span(start, width);
                    match kind {
                        Ok(kind) => tokens.push(span.track(kind)),
                        Err(kind) => errors.push(Error { span, kind }),
                    }
                }
            }
            // A `'` heads a variable and nothing else: there are no character
            // literals and no lifetimes to tell it from, so what follows it is
            // a name or the lexeme is an error.
            '\'' => {
                let (kind, width) = sigilled(&mut chars, Kind::Variable);
                let span = file_id.span(start, width);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    Err(kind) => errors.push(Error { span, kind }),
                }
            }
            // The one thing a `+` is: the mark that hangs an effect row off an
            // arrow. There is no addition, so nothing else can be meant by one.
            '+' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Plus));
                chars.next();
            }
            '|' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Pipe));
                chars.next();
            }
            // Never a lex error, unlike `-` and the `#`: what may follow a
            // `\` is the parser's business.
            '\\' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Backslash));
                chars.next();
            }
            // A `#` begins nothing on its own, so — like `-` — the only thing
            // it can be is the head of something longer, and a lone one is
            // reported where it was written.
            '#' => {
                let (kind, width) = sigilled(&mut chars, Kind::Tag);
                let span = file_id.span(start, width);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    // The whole of what was consumed, not just the `#`: `#1x`
                    // was read as one lexeme, so it is underlined as one. A
                    // span narrower than what the lexer ate points the reader
                    // at a character that is not the mistake and leaves the
                    // rest of it unmarked. The natural literal below spans its
                    // own lexeme for the same reason.
                    Err(kind) => errors.push(Error { span, kind }),
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
                    "effect" => Kind::Effect,
                    "handle" => Kind::Handle,
                    "raise" => Kind::Raise,
                    // `return` is deliberately absent: it heads a handler arm
                    // and is an ordinary name everywhere else, so the parser
                    // recognizes it by spelling at the one position that reads
                    // it — the rule `when` and `where` already keep.
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
/// The label a sigil heads — a tag's `#` or an effect's `!` — as the kind it
/// makes, with the width of the whole lexeme so the caller spans what was
/// written either way.
///
/// One function for the two sigils, because they differ in nothing but the kind
/// they build: the name runs over the characters an identifier continues with,
/// so `#1x` is one bad label here rather than a sigil beside a number, and a
/// sigil in front of nothing at all is that same error with an empty name.
fn sigilled(
    chars: &mut Peekable<CharIndices<'_>>,
    kind: impl FnOnce(String) -> Kind,
) -> (Result<Kind, ErrorKind>, usize) {
    chars.next();
    let name = word(chars);
    // The sigil is one byte and `name` was built from the source, so this is
    // exactly what was written.
    let width = name.len() + 1;
    match name.chars().next() {
        Some(c) if c.is_alphabetic() || c == '_' => (Ok(kind(name)), width),
        _ => (Err(ErrorKind::Unrecognized), width),
    }
}

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
