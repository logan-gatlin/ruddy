use std::{iter::Peekable, str::CharIndices};

use crate::tracking::{FileID, Span, Tracked};

pub type Token = Tracked<Kind>;

#[derive(Debug, Clone)]
pub enum Kind {
    Let,
    /// `extern`, declaring a target-provided top-level value.
    Extern,
    In,
    Type,
    End,
    With,
    /// `match`, opening a `match <expr> with <arms> end`. The `with` and `end`
    /// around it were reserved long before this keyword existed; this is what
    /// they were reserved for.
    Match,
    /// `if`, `then`, and `else` delimit a conditional expression.
    If,
    Then,
    Else,
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
    /// `#Some` or `#"some case"` — one of a sum type's cases, named. One
    /// token rather than a `#` beside another token, because that is what it
    /// is: one structural label and one span a reader can select. The `#` and
    /// any source quotes are not part of the decoded name it carries, the way
    /// a struct's braces are not part of its field names.
    Tag(String),
    /// `!Log` — an effect, named. Like a [`Tag`](Kind::Tag), it keeps its
    /// sigil and name in one token so its span can be selected in one go.
    /// Effect labels remain identifier-shaped; unlike sum variants, they do
    /// not have a quoted form.
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
    /// A canonical numeric field after a projection dot or in a structural
    /// field-label position. Kept distinct from a suffixless real so `pair.0`
    /// and `{0: value}` name field `"0"` without changing ordinary numeric
    /// expressions. Leading zeroes are discarded by the numeric value.
    NumericField(u64),
    /// A 64-bit floating-point literal. The suffixless spelling is real.
    Real(f64),
    /// UTF-8 text between double quotes. Escape sequences are decoded here so
    /// every later phase compares values rather than source spellings.
    String(String),
    /// One of the two boolean values.
    Boolean(bool),
    /// A lexeme the lexer diagnosed. Keeping its place in the token stream lets
    /// the parser recover without inventing a second complaint for the same
    /// source text.
    Invalid,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A character that begins no token at all.
    InvalidCharacter { character: char },
    /// `#` was not followed by an identifier-shaped (or quoted) tag name.
    MalformedTag,
    /// `!` was not followed by an identifier-shaped effect name.
    MalformedEffectLabel,
    /// `'` was not followed by an identifier-shaped variable name.
    MalformedVariable,
    /// A number ran directly into identifier characters, as in `1thing`.
    NumberFollowedByName,
    /// A whole-number suffix was attached to a decimal, as in `1.5n`.
    DecimalWithWholeSuffix { suffix: char },
    /// A structural numeric field used something other than decimal digits.
    MalformedNumericField,
    /// A natural literal too large for [`Kind::Natural`] to hold.
    NaturalTooLarge,
    /// An integer literal too large for [`Kind::Integer`] to hold.
    IntegerTooLarge,
    /// A real literal outside the finite range of [`Kind::Real`].
    RealTooLarge,
    /// A numeric field too large for [`Kind::NumericField`] to hold.
    NumericFieldTooLarge,
    /// A string escape the language does not define.
    UnknownStringEscape { escape: char },
    /// A string reached a line boundary or the end of input before its quote.
    MissingClosingQuote,
    /// A block comment reached the end of input with a `(*` still unmatched
    /// by any `*)` — its own or one nested inside it.
    MissingClosingComment,
}

pub struct Output {
    pub tokens: Vec<Token>,
    pub errors: Vec<Error>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    Brace,
    Paren,
}

pub fn lex(input: &str, file_id: FileID) -> Output {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut delimiters = Vec::new();

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
            // `->` wins over the standalone minus, and `--` wins over both:
            // the longer lexeme decides before the shorter one is assumed,
            // the way `..` and `=>` already are. A line comment carries no
            // token, the way whitespace does not.
            '-' => {
                chars.next();
                match chars.peek() {
                    Some(&(_, '>')) => {
                        chars.next();
                        tokens.push(file_id.span(start, 2).track(Kind::Arrow));
                    }
                    Some(&(_, '-')) => {
                        chars.next();
                        line_comment(&mut chars);
                    }
                    _ => tokens.push(file_id.span(start, 1).track(Kind::Minus)),
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
                    let (kind, width) = sigilled(
                        &mut chars,
                        Kind::EffectLabel,
                        ErrorKind::MalformedEffectLabel,
                    );
                    let span = file_id.span(start, width);
                    match kind {
                        Ok(kind) => tokens.push(span.track(kind)),
                        Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
                    }
                }
            }
            // A `'` heads a variable and nothing else: there are no character
            // literals and no lifetimes to tell it from, so what follows it is
            // a name or the lexeme is an error.
            '\'' => {
                let (kind, width) =
                    sigilled(&mut chars, Kind::Variable, ErrorKind::MalformedVariable);
                let span = file_id.span(start, width);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
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
            // `\\` opens a raw string line, and wins over two absence marks
            // the way `--` wins over two minuses: nothing in the language
            // writes one `\` directly after another. Decided by looking past
            // the first `\` without consuming it, the way `(*` is.
            '\\' if matches!(chars.clone().nth(1), Some((_, '\\'))) => {
                let (value, width) = raw_string(&mut chars);
                tokens.push(file_id.span(start, width).track(Kind::String(value)));
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
                // A quoted tag is one contiguous sigilled label. Decode its
                // string with exactly the same rules as an ordinary literal,
                // while keeping the `#` in the token's span.
                let (kind, width) = if matches!(chars.clone().nth(1), Some((_, '"'))) {
                    chars.next();
                    let (value, string_width) = string(&mut chars);
                    (value.map(Kind::Tag), string_width + 1)
                } else {
                    sigilled(&mut chars, Kind::Tag, ErrorKind::MalformedTag)
                };
                let span = file_id.span(start, width);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    // The whole of what was consumed, not just the `#`: `#1x`
                    // was read as one lexeme, so it is underlined as one. A
                    // span narrower than what the lexer ate points the reader
                    // at a character that is not the mistake and leaves the
                    // rest of it unmarked. The natural literal below spans its
                    // own lexeme for the same reason.
                    Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
                }
            }
            '"' => {
                let (value, width) = string(&mut chars);
                let span = file_id.span(start, width);
                match value {
                    Ok(value) => tokens.push(span.track(Kind::String(value))),
                    Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
                }
            }
            '{' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::LeftBrace));
                delimiters.push(Delimiter::Brace);
                chars.next();
            }
            '}' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::RightBrace));
                if delimiters.last() == Some(&Delimiter::Brace) {
                    delimiters.pop();
                }
                chars.next();
            }
            // `(*` opens a block comment rather than a parenthesis — decided
            // by looking past the `(` without consuming it, the way a `!`
            // ahead of `=` is.
            '(' if matches!(chars.clone().nth(1), Some((_, '*'))) => {
                let (result, width) = block_comment(&mut chars);
                if let Err(kind) = result {
                    let span = file_id.span(start, width);
                    invalid(span, kind, &mut tokens, &mut errors);
                }
            }
            '(' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::LeftParen));
                delimiters.push(Delimiter::Paren);
                chars.next();
            }
            ')' => {
                tokens.push(file_id.span(start, c.len_utf8()).track(Kind::RightParen));
                if delimiters.last() == Some(&Delimiter::Paren) {
                    delimiters.pop();
                }
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
                    "extern" => Kind::Extern,
                    "in" => Kind::In,
                    "type" => Kind::Type,
                    "end" => Kind::End,
                    "with" => Kind::With,
                    "match" => Kind::Match,
                    "if" => Kind::If,
                    "then" => Kind::Then,
                    "else" => Kind::Else,
                    "fn" => Kind::Fn,
                    "effect" => Kind::Effect,
                    "handle" => Kind::Handle,
                    "raise" => Kind::Raise,
                    "and" => Kind::And,
                    "or" => Kind::Or,
                    "xor" => Kind::Xor,
                    "not" => Kind::Not,
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
            // A digit run after a projection dot or at the start of a field
            // entry names a positional field. The lexer keeps this contextual
            // distinction because ordinary suffixless numbers are reals.
            // Whitespace is insignificant in both positions.
            //
            // Read the whole number-shaped lexeme before validating it. This
            // makes `.0n`, `.0.0`, and `{0n: x}` one malformed field rather
            // than a valid field followed by surprising extra tokens.
            c if c.is_ascii_digit()
                && numeric_field_position(&tokens, &delimiters)
                && tokens.last().is_some_and(|tok| {
                    errors
                        .last()
                        .is_none_or(|error| error.span.start < tok.span.start)
                }) =>
            {
                let literal = number(&mut chars);
                let span = file_id.span(start, literal.len());
                let kind = numeric_field(&literal);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
                }
            }
            // A numeric literal is a real by default. An `i` or `n` suffix
            // selects a signed integer or natural respectively. A decimal
            // point belongs to the literal only when a digit follows it, so
            // `1.x` remains a projection.
            c if c.is_ascii_digit() => {
                let literal = number(&mut chars);
                let span = file_id.span(start, literal.len());
                let kind = numeric(&literal);
                match kind {
                    Ok(kind) => tokens.push(span.track(kind)),
                    Err(kind) => invalid(span, kind, &mut tokens, &mut errors),
                }
            }
            // Anything else is an unrecognized character.
            _ => {
                let span = file_id.span(start, c.len_utf8());
                invalid(
                    span,
                    ErrorKind::InvalidCharacter { character: c },
                    &mut tokens,
                    &mut errors,
                );
                chars.next();
            }
        }
    }

    Output { tokens, errors }
}

fn invalid(span: Span, kind: ErrorKind, tokens: &mut Vec<Token>, errors: &mut Vec<Error>) {
    tokens.push(span.track(Kind::Invalid));
    errors.push(Error { span, kind });
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
    malformed: ErrorKind,
) -> (Result<Kind, ErrorKind>, usize) {
    chars.next();
    let name = word(chars);
    // The sigil is one byte and `name` was built from the source, so this is
    // exactly what was written.
    let width = name.len() + 1;
    match name.chars().next() {
        Some(c) if identifier_start(c) => (Ok(kind(name)), width),
        _ => (Err(malformed), width),
    }
}

fn identifier_start(c: char) -> bool {
    match c {
        '_' => true,
        _ => c.is_alphabetic(),
    }
}

/// Read one line-bounded double-quoted string.
///
/// Once an unknown escape has been seen, recovery still follows escape syntax
/// through the real closing quote. This keeps the entire broken string in one
/// invalid token and prevents its tail from producing further diagnostics.
fn string(chars: &mut Peekable<CharIndices<'_>>) -> (Result<String, ErrorKind>, usize) {
    let (_, quote) = chars.next().expect("the caller peeked the opening quote");
    debug_assert_eq!(quote, '"');
    let mut value = String::new();
    let mut width = 1;
    let mut error = None;

    loop {
        let Some(&(_, c)) = chars.peek() else {
            return (Err(error.unwrap_or(ErrorKind::MissingClosingQuote)), width);
        };
        // A source newline is never string content. Leave it for the main loop
        // so the declaration on the next line is recovered independently.
        if matches!(c, '\n' | '\r') {
            return (Err(error.unwrap_or(ErrorKind::MissingClosingQuote)), width);
        }
        chars.next();
        width += c.len_utf8();
        match c {
            '"' => return (error.map_or(Ok(value), Err), width),
            '\\' => {
                let Some(&(_, escaped)) = chars.peek() else {
                    return (Err(error.unwrap_or(ErrorKind::MissingClosingQuote)), width);
                };
                if matches!(escaped, '\n' | '\r') {
                    return (Err(error.unwrap_or(ErrorKind::MissingClosingQuote)), width);
                }
                chars.next();
                width += escaped.len_utf8();
                match escaped {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    _ => {
                        error.get_or_insert(ErrorKind::UnknownStringEscape { escape: escaped });
                    }
                }
            }
            _ => value.push(c),
        }
    }
}

/// Read a raw string: one or more `\\` lines, from the first `\\` onward.
///
/// Each line runs from the character after its `\\` to the end of that line,
/// verbatim — no escapes, and no closing delimiter, so unlike [`string`] this
/// cannot fail: the line is the only boundary it needs, and it always comes.
/// Whitespace before a line's `\\` is the file's indentation and is not
/// content; whitespace after it is.
///
/// Consecutive `\\` lines with nothing but whitespace between them are one
/// string, joined by a newline between each pair and by none after the last,
/// so a final empty `\\` line is how a trailing newline is written. Anything
/// else between two lines — a comment included — ends the string at the
/// first, and the second is a string of its own.
fn raw_string(chars: &mut Peekable<CharIndices<'_>>) -> (String, usize) {
    let mut value = String::new();
    let mut width = 0;
    loop {
        for expected in ['\\', '\\'] {
            let (_, opener) = chars.next().expect("the caller peeked both opening `\\`");
            debug_assert_eq!(opener, expected);
        }
        width += 2;
        while let Some(&(_, c)) = chars.peek() {
            if matches!(c, '\n' | '\r') {
                break;
            }
            chars.next();
            width += c.len_utf8();
            value.push(c);
        }

        // Look past the whitespace for the next line's `\\` without consuming
        // anything: if it is not there, the whitespace is the main loop's.
        let mut ahead = chars.clone();
        while ahead.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
            ahead.next();
        }
        let continues = ahead.next().is_some_and(|(_, c)| c == '\\')
            && ahead.next().is_some_and(|(_, c)| c == '\\');
        if !continues {
            return (value, width);
        }
        while let Some((_, c)) = chars.next_if(|&(_, c)| c.is_whitespace()) {
            width += c.len_utf8();
        }
        value.push('\n');
    }
}

/// Consume a `--` line comment's remaining text, up to but not including the
/// newline that ends it, or the end of input. Carries no token, the way
/// whitespace does not — the line itself is the only delimiter it needs.
fn line_comment(chars: &mut Peekable<CharIndices<'_>>) {
    while let Some(&(_, c)) = chars.peek() {
        if c == '\n' {
            break;
        }
        chars.next();
    }
}

/// Consume a `(* ... *)` block comment, from its opening `(*` onward.
///
/// A `(*` inside the comment reopens the count, and only the `*)` that
/// matches it closes that nesting rather than the outer one, so
/// `(* (* *) *)` is one comment rather than a comment followed by stray text.
/// Carries no token, the way [`string`]'s caller does carry one: a comment
/// has no value to keep, only a width to have consumed.
fn block_comment(chars: &mut Peekable<CharIndices<'_>>) -> (Result<(), ErrorKind>, usize) {
    let (_, open) = chars.next().expect("the caller peeked the opening `(`");
    debug_assert_eq!(open, '(');
    let (_, star) = chars.next().expect("the caller peeked the opening `*`");
    debug_assert_eq!(star, '*');
    let mut depth = 1u32;
    let mut width = 2;

    while depth > 0 {
        let Some(&(_, c)) = chars.peek() else {
            return (Err(ErrorKind::MissingClosingComment), width);
        };
        chars.next();
        width += c.len_utf8();
        match c {
            '(' if matches!(chars.peek(), Some(&(_, '*'))) => {
                chars.next();
                width += 1;
                depth += 1;
            }
            '*' if matches!(chars.peek(), Some(&(_, ')'))) => {
                chars.next();
                width += 1;
                depth -= 1;
            }
            _ => {}
        }
    }
    (Ok(()), width)
}

fn word(chars: &mut Peekable<CharIndices<'_>>) -> String {
    let mut word = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if identifier_continue(c) {
            word.push(c);
            chars.next();
        } else {
            break;
        }
    }
    word
}

fn identifier_continue(c: char) -> bool {
    match c {
        '_' => true,
        _ => c.is_alphanumeric(),
    }
}

/// Consume one numeric literal, including its optional fractional part and
/// type suffix. Any identifier character attached to it stays part of the
/// literal so `1thing` is one useful lexical error rather than two terms.
fn number(chars: &mut Peekable<CharIndices<'_>>) -> String {
    let mut literal = String::new();
    while let Some(&(_, c)) = chars.peek() {
        if c.is_ascii_digit() {
            literal.push(c);
            chars.next();
        } else {
            break;
        }
    }
    let decimal = matches!(chars.peek(), Some(&(_, '.')))
        && matches!(chars.clone().nth(1), Some((_, c)) if c.is_ascii_digit());
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
        if identifier_continue(c) {
            literal.push(c);
            chars.next();
        } else {
            break;
        }
    }
    literal
}

/// Whether the next token occupies a structural numeric-label position.
///
/// Braces are structural throughout Ruddy, but values inside them may contain
/// arbitrary nested expressions. The innermost unmatched delimiter keeps
/// `(1, 2)` and `{ x: (1, 2) }` as ordinary real expressions, while recognizing
/// the first field and every brace-level field after a comma. A backslash is
/// included for an absent struct-type field.
fn numeric_field_position(tokens: &[Token], delimiters: &[Delimiter]) -> bool {
    let Some(previous) = tokens.last() else {
        return false;
    };
    matches!(previous.tracked, Kind::Dot | Kind::LeftBrace)
        || (matches!(previous.tracked, Kind::Comma | Kind::Backslash)
            && delimiters.last() == Some(&Delimiter::Brace))
}

fn numeric_field(literal: &str) -> Result<Kind, ErrorKind> {
    if !literal.bytes().all(|c| c.is_ascii_digit()) {
        return Err(ErrorKind::MalformedNumericField);
    }
    literal
        .parse()
        .map(Kind::NumericField)
        .map_err(|_| ErrorKind::NumericFieldTooLarge)
}

fn numeric(literal: &str) -> Result<Kind, ErrorKind> {
    #[derive(Clone, Copy)]
    enum Suffix {
        Integer,
        Natural,
    }

    let (digits, suffix, suffix_char) = match literal.strip_suffix('i') {
        Some(digits) => (digits, Some(Suffix::Integer), Some('i')),
        None => match literal.strip_suffix('n') {
            Some(digits) => (digits, Some(Suffix::Natural), Some('n')),
            None => (literal, None, None),
        },
    };

    // If removing a possible suffix does not leave only the numeric spelling,
    // the suffix-like character was merely the end of an attached name.
    if !digits.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return Err(ErrorKind::NumberFollowedByName);
    }
    if let Some(suffix) = suffix_char.filter(|_| digits.contains('.')) {
        return Err(ErrorKind::DecimalWithWholeSuffix { suffix });
    }

    match suffix {
        Some(Suffix::Natural) => digits
            .parse()
            .map(Kind::Natural)
            .map_err(|_| ErrorKind::NaturalTooLarge),
        Some(Suffix::Integer) => digits
            .parse()
            .map(Kind::Integer)
            .map_err(|_| ErrorKind::IntegerTooLarge),
        None => digits
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Kind::Real)
            .ok_or(ErrorKind::RealTooLarge),
    }
}
