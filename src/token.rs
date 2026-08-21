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
    /// Boolean operators. Unlike the `where` clause's contextual words, these
    /// are reserved because they may appear wherever an expression does.
    And,
    Or,
    Xor,
    Not,
    /// `bundle`, opening the header the root file of a bundle begins with.
    Bundle,
    /// `module`, opening a module declaration — inline with an `=` and a body,
    /// or bare for a module whose body is another file.
    Module,
    Equal,
    FatArrow,
    /// `->`, the function type arrow. Distinct from [`FatArrow`](Kind::FatArrow),
    /// which introduces a lambda body.
    Arrow,
    Colon,
    /// `::`, separating the module segments of a path from each other and from
    /// the name at the end of it. One token rather than two
    /// [`Colon`](Kind::Colon)s, the way [`DotDot`](Kind::DotDot) is one: the
    /// longer lexeme wins, so a `:` followed by a `:` is this and never an
    /// ascription beside another.
    ColonColon,
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
    /// `+`, joining an effect row or adding two real numbers.
    Plus,
    /// `-`, an arrow's head or real-number subtraction and negation.
    Minus,
    /// `*`, multiplying two real numbers.
    Star,
    /// `/`, dividing two real numbers.
    Slash,
    /// `\`, marking a struct type's field — or a sum type's case — as one that
    /// is definitely *not* there: the `..` beside it may not stand for the
    /// label. A bare punctuation token, so `\ y` lexes the same as `\y` — the
    /// same separation `..` keeps from the name after it.
    Backslash,
    /// `|`, separating the cases of a sum type. Also the whole of the empty
    /// sum, which is the one type written with nothing but punctuation.
    Pipe,
    /// `|>`, feeding its left value to the function on its right.
    PipeForward,
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
    /// An unsigned 64-bit natural literal, written with an `n` suffix.
    Natural(u64),
    /// A signed 64-bit integer literal, written with an `i` suffix.
    Integer(i64),
    /// A 64-bit floating-point literal. The suffixless spelling is real.
    Real(f64),
    /// UTF-8 text between double quotes. Escape sequences are decoded here so
    /// every later phase compares values rather than source spellings.
    String(String),
    /// One of the two boolean values.
    Boolean(bool),
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
    /// A quoted string did not close or used an unsupported escape.
    MalformedString,
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
            // `->` wins over the standalone minus.
            '-' => {
                chars.next();
                if let Some(&(_, '>')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::Arrow));
                } else {
                    tokens.push(file_id.span(start, 1).track(Kind::Minus));
                }
            }
            // `::` separates a path's segments; a lone `:` ascribes. The longer
            // lexeme wins, so `A::x` is a path rather than an ascription of an
            // ascription — the rule `..` and `=>` already keep.
            ':' => {
                chars.next();
                if let Some(&(_, ':')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::ColonColon));
                } else {
                    tokens.push(file_id.span(start, 1).track(Kind::Colon));
                }
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
            '+' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Plus));
                chars.next();
            }
            '*' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Star));
                chars.next();
            }
            '/' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Slash));
                chars.next();
            }
            '|' => {
                chars.next();
                if let Some(&(_, '>')) = chars.peek() {
                    chars.next();
                    tokens.push(file_id.span(start, 2).track(Kind::PipeForward));
                } else {
                    tokens.push(file_id.span(start, c.len_utf8()).track(Kind::Pipe));
                }
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
            '"' => {
                let (value, width) = string(&mut chars);
                let span = file_id.span(start, width);
                match value {
                    Ok(value) => tokens.push(span.track(Kind::String(value))),
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
                    "and" => Kind::And,
                    "or" => Kind::Or,
                    "xor" => Kind::Xor,
                    "not" => Kind::Not,
                    "bundle" => Kind::Bundle,
                    "module" => Kind::Module,
                    "true" => Kind::Boolean(true),
                    "false" => Kind::Boolean(false),
                    // `return` is deliberately absent: it heads a handler arm
                    // and is an ordinary name everywhere else, so the parser
                    // recognizes it by spelling at the one position that reads
                    // it — the rule `when` and `where` already keep.
                    "_" => Kind::Underscore,
                    _ => Kind::Identifier(ident),
                };
                tokens.push(span.track(kind));
            }
            // A numeric literal is a real by default. An `i` or `n` suffix
            // selects a signed integer or natural respectively. A decimal
            // point belongs to the literal only when a digit follows it, so
            // `1.x` remains a projection.
            c if c.is_ascii_digit() => {
                let version_part = in_version(&tokens);
                let literal = number(&mut chars, !version_part);
                let span = file_id.span(start, literal.len());
                let kind = if version_part {
                    // A bundle version is spelled with bare integer components.
                    // Do not first turn one into an f64: values near `u64::MAX`
                    // cannot make that round trip without changing value.
                    literal
                        .parse()
                        .map(Kind::Natural)
                        .map_err(|_| ErrorKind::NaturalTooLarge)
                } else {
                    numeric(&literal)
                };
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
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

/// Whether the next digits are one of the three bare components of the root
/// bundle's version. Versions deliberately retain exact integer spelling even
/// though ordinary suffixless numbers are reals.
fn in_version(tokens: &[Tracked<Kind>]) -> bool {
    match tokens {
        [first, second] => {
            matches!(first.tracked, Kind::Bundle) && matches!(second.tracked, Kind::Identifier(_))
        }
        [first, second, component, dot] | [first, second, _, _, component, dot] => {
            matches!(first.tracked, Kind::Bundle)
                && matches!(second.tracked, Kind::Identifier(_))
                && matches!(component.tracked, Kind::Natural(_))
                && matches!(dot.tracked, Kind::Dot)
        }
        _ => false,
    }
}

/// Read a double-quoted string, accepting the standard compact escapes.
fn string(chars: &mut Peekable<CharIndices<'_>>) -> (Result<String, ErrorKind>, usize) {
    let (_, quote) = chars.next().expect("the caller peeked the opening quote");
    debug_assert_eq!(quote, '"');
    let mut value = String::new();
    let mut width = 1;
    while let Some((_, c)) = chars.next() {
        width += c.len_utf8();
        match c {
            '"' => return (Ok(value), width),
            '\\' => {
                let Some((_, escaped)) = chars.next() else {
                    return (Err(ErrorKind::MalformedString), width);
                };
                width += escaped.len_utf8();
                match escaped {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    _ => return (Err(ErrorKind::MalformedString), width),
                }
            }
            _ => value.push(c),
        }
    }
    (Err(ErrorKind::MalformedString), width)
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

/// Consume one numeric literal, including its optional fractional part and
/// type suffix. Any identifier character attached to it stays part of the
/// literal so `1thing` is one useful lexical error rather than two terms.
fn number(chars: &mut Peekable<CharIndices<'_>>, allow_decimal: bool) -> String {
    let mut literal = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c.is_ascii_digit() {
            literal.push(c);
            chars.next();
        } else {
            break;
        }
    }
    let decimal = if allow_decimal
        && matches!(chars.peek(), Some(&(_, '.')))
        && matches!(chars.clone().nth(1), Some((_, c)) if c.is_ascii_digit())
    {
        // A second dot after the fractional digits makes this a bundle version
        // component, not a decimal: `0.1.0` is three numeric tokens.
        let mut look = chars.clone();
        look.next();
        while matches!(look.peek(), Some(&(_, c)) if c.is_ascii_digit()) {
            look.next();
        }
        !matches!(look.peek(), Some(&(_, '.')))
    } else {
        false
    };
    if decimal {
        literal.push('.');
        chars.next();
        while let Some(&(_, c)) = chars.peek() {
            if c.is_ascii_digit() {
                literal.push(c);
                chars.next();
            } else {
                break;
            }
        }
    }
    while let Some(&(_, c)) = chars.peek() {
        if c.is_alphanumeric() || c == '_' {
            literal.push(c);
            chars.next();
        } else {
            break;
        }
    }
    literal
}

fn numeric(literal: &str) -> Result<Kind, ErrorKind> {
    let (digits, suffix) = match literal.strip_suffix('i') {
        Some(digits) => (digits, Some('i')),
        None => match literal.strip_suffix('n') {
            Some(digits) => (digits, Some('n')),
            None => (literal, None),
        },
    };
    if digits.is_empty()
        || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        || digits.bytes().filter(|&c| c == b'.').count() > 1
        || suffix.is_some_and(|_| digits.contains('.'))
    {
        return Err(ErrorKind::MalformedNatural);
    }
    match suffix {
        Some('n') => digits
            .parse()
            .map(Kind::Natural)
            .map_err(|_| ErrorKind::NaturalTooLarge),
        Some('i') => digits
            .parse()
            .map(Kind::Integer)
            .map_err(|_| ErrorKind::NaturalTooLarge),
        None => digits
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Kind::Real)
            .ok_or(ErrorKind::NaturalTooLarge),
        _ => unreachable!(),
    }
}
