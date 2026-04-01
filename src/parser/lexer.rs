use super::token::{Keyword, Operator, Punct, Token, TokenKind};
use crate::engine::Source;
use crate::reporting::{Diag, Diagnostic, TextRange, TextSize};
use salsa::Accumulator;

pub(super) fn lex(db: &dyn salsa::Database, source_file: Source, source: &str) -> Vec<Token> {
    Lexer::new(db, source_file, source).lex()
}

struct Lexer<'src> {
    db: &'src dyn salsa::Database,
    source_file: Source,
    source: &'src str,
    cursor: usize,
    tokens: Vec<Token>,
}

impl<'src> Lexer<'src> {
    fn new(db: &'src dyn salsa::Database, source_file: Source, source: &'src str) -> Self {
        Self {
            db,
            source_file,
            source,
            cursor: 0,
            tokens: Vec::new(),
        }
    }

    fn lex(mut self) -> Vec<Token> {
        while !self.is_eof() {
            self.skip_trivia();
            if self.is_eof() {
                break;
            }
            self.lex_token();
        }

        let eof = TextSize::from_usize(self.cursor);
        self.tokens.push(Token::new(
            TokenKind::EndOfFile,
            TextRange::empty(self.source_file, eof),
        ));

        self.tokens
    }

    fn lex_token(&mut self) {
        let start = self.cursor;

        match self.peek_char() {
            Some(ch) if is_ident_start(ch) => {
                self.lex_ident_or_keyword(start);
            }
            Some('[') if self.lex_bracketed_operator_ident(start) => {}
            Some(ch) if ch.is_ascii_digit() => {
                self.lex_number(start);
            }
            Some('"') => {
                self.lex_string(start);
            }
            Some('\'') => {
                self.lex_glyph(start);
            }
            Some('`') => {
                self.lex_format_string(start);
            }
            Some(_) if self.lex_multi_char_operator(start) => {}
            Some(ch) => {
                self.bump_char();
                if let Some(kind) = single_char_token_kind(ch) {
                    self.emit_token(start, kind);
                } else {
                    self.report(start, self.cursor, format!("unexpected character `{ch}`"));
                    self.emit_token(start, TokenKind::Error);
                }
            }
            None => {}
        }
    }

    fn lex_bracketed_operator_ident(&mut self, start: usize) -> bool {
        let Some(after_open) = self.cursor.checked_add(1) else {
            return false;
        };
        if after_open >= self.source.len() {
            return false;
        }

        let mut close = None;
        for (offset, ch) in self.source[after_open..].char_indices() {
            if ch == ']' {
                close = Some(after_open + offset);
                break;
            }
            if ch == '\n' || ch == '\r' {
                return false;
            }
        }

        let Some(close) = close else {
            return false;
        };

        let inside = &self.source[after_open..close];
        if !is_operator_like_bracketed_identifier(inside) {
            return false;
        }

        self.cursor = close + 1;
        self.emit_token(start, TokenKind::BracketedIdent);
        true
    }

    fn lex_ident_or_keyword(&mut self, start: usize) {
        self.bump_char();
        self.consume_ident_tail();
        let text = &self.source[start..self.cursor];
        let kind = keyword_kind(text)
            .map(TokenKind::Keyword)
            .unwrap_or(TokenKind::Ident);
        self.emit_token(start, kind);
    }

    fn consume_ident_tail(&mut self) {
        loop {
            self.consume_while(is_ident_continue);

            if self.peek_char() == Some('-') && self.peek_next_char().is_some_and(is_ident_start) {
                self.bump_char();
                self.bump_char();
                continue;
            }

            break;
        }
    }

    fn lex_number(&mut self, start: usize) {
        if self.starts_with("0b") || self.starts_with("0B") {
            self.lex_prefixed_number(start, 2, is_binary_digit, "binary");
            return;
        }

        if self.starts_with("0o") || self.starts_with("0O") {
            self.lex_prefixed_number(start, 2, is_octal_digit, "octal");
            return;
        }

        if self.starts_with("0x") || self.starts_with("0X") {
            self.lex_prefixed_number(start, 2, is_hex_digit, "hex");
            return;
        }

        self.consume_while(|ch| ch.is_ascii_digit() || ch == '_');
        let int_text = &self.source[start..self.cursor];
        if !is_valid_digit_sequence(int_text, |ch| ch.is_ascii_digit()) {
            self.report(start, self.cursor, "invalid decimal literal");
            self.emit_token(start, TokenKind::Error);
            return;
        }

        let mut kind = TokenKind::IntegerLiteral;

        if self.peek_char() == Some('.')
            && self.peek_next_char().is_some_and(|ch| ch.is_ascii_digit())
        {
            self.bump_char();
            let frac_start = self.cursor;
            self.consume_while(|ch| ch.is_ascii_digit() || ch == '_');
            let frac_text = &self.source[frac_start..self.cursor];
            if !is_valid_digit_sequence(frac_text, |ch| ch.is_ascii_digit()) {
                self.report(start, self.cursor, "invalid real literal");
                self.emit_token(start, TokenKind::Error);
                return;
            }
            kind = TokenKind::RealLiteral;
        }

        match self.try_consume_exponent() {
            Ok(true) => {
                kind = TokenKind::RealLiteral;
            }
            Ok(false) => {}
            Err(()) => {
                self.report(start, self.cursor, "invalid exponent literal");
                self.emit_token(start, TokenKind::Error);
                return;
            }
        }

        if kind == TokenKind::IntegerLiteral && self.can_consume_n_suffix() {
            self.bump_char();
            kind = TokenKind::NaturalLiteral;
        }

        self.emit_token(start, kind);
    }

    fn lex_prefixed_number(
        &mut self,
        start: usize,
        prefix_len: usize,
        digit_predicate: fn(char) -> bool,
        base_name: &str,
    ) {
        self.bump_bytes(prefix_len);
        let digits_start = self.cursor;
        self.consume_while(|ch| digit_predicate(ch) || ch == '_');

        if self.cursor == digits_start {
            self.report(
                start,
                self.cursor,
                format!("expected digits after {base_name} prefix"),
            );
            self.emit_token(start, TokenKind::Error);
            return;
        }

        let digits_text = &self.source[digits_start..self.cursor];
        if !is_valid_digit_sequence(digits_text, digit_predicate) {
            self.report(start, self.cursor, format!("invalid {base_name} literal"));
            self.emit_token(start, TokenKind::Error);
            return;
        }

        let mut kind = TokenKind::IntegerLiteral;
        if self.can_consume_n_suffix() {
            self.bump_char();
            kind = TokenKind::NaturalLiteral;
        }

        self.emit_token(start, kind);
    }

    fn lex_string(&mut self, start: usize) {
        self.bump_char();
        let mut had_error = false;
        let mut terminated = false;

        while let Some(ch) = self.peek_char() {
            if ch == '"' {
                self.bump_char();
                terminated = true;
                break;
            }

            if ch == '\\' {
                let escape_start = self.cursor;
                self.bump_char();
                had_error |= !self.consume_escape_sequence(escape_start);
                continue;
            }

            self.bump_char();
        }

        if !terminated {
            self.report(start, self.cursor, "unterminated string literal");
            had_error = true;
        }

        self.emit_token(
            start,
            if had_error {
                TokenKind::Error
            } else {
                TokenKind::StringLiteral
            },
        );
    }

    fn lex_glyph(&mut self, start: usize) {
        self.bump_char();
        let mut scalar_count = 0u32;
        let mut terminated = false;
        let mut had_error = false;

        while let Some(ch) = self.peek_char() {
            if ch == '\'' {
                self.bump_char();
                terminated = true;
                break;
            }

            if ch == '\\' {
                let escape_start = self.cursor;
                self.bump_char();
                if self.consume_escape_sequence(escape_start) {
                    scalar_count += 1;
                } else {
                    had_error = true;
                }
                continue;
            }

            self.bump_char();
            scalar_count += 1;
        }

        if !terminated {
            self.report(start, self.cursor, "unterminated glyph literal");
            had_error = true;
        } else if scalar_count != 1 {
            self.report(
                start,
                self.cursor,
                "glyph literal must decode to one scalar value",
            );
            had_error = true;
        }

        self.emit_token(
            start,
            if had_error {
                TokenKind::Error
            } else {
                TokenKind::GlyphLiteral
            },
        );
    }

    fn lex_format_string(&mut self, start: usize) {
        self.bump_char();
        let mut terminated = false;
        let mut had_error = false;

        while let Some(ch) = self.peek_char() {
            if ch == '`' {
                self.bump_char();
                terminated = true;
                break;
            }

            if ch == '{' {
                let brace_start = self.cursor;
                self.bump_char();
                match self.peek_char() {
                    Some('{') | Some('}') => {
                        self.bump_char();
                    }
                    _ => {
                        self.report(
                            brace_start,
                            self.cursor,
                            "invalid format placeholder; expected `{}` or `{{`",
                        );
                        had_error = true;
                    }
                }
                continue;
            }

            if ch == '}' {
                let brace_start = self.cursor;
                self.bump_char();
                if self.peek_char() == Some('}') {
                    self.bump_char();
                } else {
                    self.report(brace_start, self.cursor, "unescaped `}` in format string");
                    had_error = true;
                }
                continue;
            }

            self.bump_char();
        }

        if !terminated {
            self.report(start, self.cursor, "unterminated format string");
            had_error = true;
        }

        self.emit_token(
            start,
            if had_error {
                TokenKind::Error
            } else {
                TokenKind::FormatStringLiteral
            },
        );
    }

    fn lex_multi_char_operator(&mut self, start: usize) -> bool {
        let kind = if self.starts_with("::") {
            Some(TokenKind::Operator(Operator::PathSep))
        } else if self.starts_with("..") {
            Some(TokenKind::Operator(Operator::Spread))
        } else if self.starts_with("=>") {
            Some(TokenKind::Operator(Operator::FatArrow))
        } else if self.starts_with("->") {
            Some(TokenKind::Operator(Operator::Arrow))
        } else if self.starts_with("==") {
            Some(TokenKind::Operator(Operator::EqualEqual))
        } else if self.starts_with("!=") {
            Some(TokenKind::Operator(Operator::BangEqual))
        } else if self.starts_with("<=") {
            Some(TokenKind::Operator(Operator::LessEqual))
        } else if self.starts_with(">=") {
            Some(TokenKind::Operator(Operator::GreaterEqual))
        } else if self.starts_with("<<") {
            Some(TokenKind::Operator(Operator::ComposeLeft))
        } else if self.starts_with(">>") {
            Some(TokenKind::Operator(Operator::ComposeRight))
        } else if self.starts_with("|>") {
            Some(TokenKind::Operator(Operator::PipeRight))
        } else if self.starts_with("+>") {
            Some(TokenKind::Operator(Operator::PlusPipe))
        } else if self.starts_with("*>") {
            Some(TokenKind::Operator(Operator::StarPipe))
        } else {
            None
        };

        if let Some(kind) = kind {
            self.bump_bytes(2);
            self.emit_token(start, kind);
            true
        } else {
            false
        }
    }

    fn consume_escape_sequence(&mut self, escape_start: usize) -> bool {
        match self.peek_char() {
            Some('n' | 't' | 'r' | '\\' | '"' | '\'') => {
                self.bump_char();
                true
            }
            Some('u') => {
                self.bump_char();
                self.consume_unicode_escape(escape_start)
            }
            Some(_) => {
                self.bump_char();
                self.report(escape_start, self.cursor, "invalid escape sequence");
                false
            }
            None => {
                self.report(escape_start, self.cursor, "unterminated escape sequence");
                false
            }
        }
    }

    fn consume_unicode_escape(&mut self, escape_start: usize) -> bool {
        if self.peek_char() != Some('{') {
            self.report(escape_start, self.cursor, "expected `{` after `\\u`");
            return false;
        }
        self.bump_char();

        let digits_start = self.cursor;
        self.consume_while(|ch| ch.is_ascii_hexdigit());
        if digits_start == self.cursor {
            self.report(escape_start, self.cursor, "empty unicode escape");
            return false;
        }

        if self.peek_char() != Some('}') {
            self.report(escape_start, self.cursor, "unterminated unicode escape");
            return false;
        }
        let digits_end = self.cursor;
        self.bump_char();

        let digits = &self.source[digits_start..digits_end];
        let value = u32::from_str_radix(digits, 16).ok();
        let is_valid_scalar = value.and_then(char::from_u32).is_some();
        if !is_valid_scalar {
            self.report(escape_start, self.cursor, "invalid unicode scalar value");
            return false;
        }

        true
    }

    fn try_consume_exponent(&mut self) -> Result<bool, ()> {
        if !matches!(self.peek_char(), Some('e' | 'E')) {
            return Ok(false);
        }

        let checkpoint = self.cursor;
        self.bump_char();
        if matches!(self.peek_char(), Some('+' | '-')) {
            self.bump_char();
        }

        if !self.peek_char().is_some_and(|ch| ch.is_ascii_digit()) {
            self.cursor = checkpoint;
            return Ok(false);
        }

        let exp_start = self.cursor;
        self.consume_while(|ch| ch.is_ascii_digit() || ch == '_');
        let exp_text = &self.source[exp_start..self.cursor];
        if is_valid_digit_sequence(exp_text, |ch| ch.is_ascii_digit()) {
            Ok(true)
        } else {
            Err(())
        }
    }

    fn can_consume_n_suffix(&self) -> bool {
        if self.peek_char() != Some('n') {
            return false;
        }

        !self.peek_next_char().is_some_and(is_ident_continue)
    }

    fn skip_trivia(&mut self) {
        loop {
            self.consume_while(char::is_whitespace);

            if self.starts_with("--") {
                self.bump_bytes(2);
                self.consume_while(|ch| ch != '\n');
                continue;
            }

            if self.starts_with("(*") {
                self.skip_block_comment();
                continue;
            }

            break;
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.cursor;
        self.bump_bytes(2);
        let mut depth = 1usize;

        while !self.is_eof() {
            if self.starts_with("(*") {
                self.bump_bytes(2);
                depth += 1;
                continue;
            }

            if self.starts_with("*)") {
                self.bump_bytes(2);
                depth -= 1;
                if depth == 0 {
                    return;
                }
                continue;
            }

            self.bump_char();
        }

        self.report(start, self.cursor, "unterminated block comment");
    }

    fn emit_token(&mut self, start: usize, kind: TokenKind) {
        let range = TextRange::from_bounds(
            self.source_file,
            TextSize::from_usize(start),
            TextSize::from_usize(self.cursor),
        );
        self.tokens.push(Token::new(kind, range));
    }

    fn report(&mut self, start: usize, end: usize, message: impl Into<String>) {
        let range = TextRange::from_bounds(
            self.source_file,
            TextSize::from_usize(start),
            TextSize::from_usize(end),
        );
        Diag(Diagnostic::error(range, message)).accumulate(self.db);
    }

    fn consume_while(&mut self, mut predicate: impl FnMut(char) -> bool) {
        while let Some(ch) = self.peek_char() {
            if predicate(ch) {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn is_eof(&self) -> bool {
        self.cursor >= self.source.len()
    }

    fn starts_with(&self, text: &str) -> bool {
        self.source[self.cursor..].starts_with(text)
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.cursor..].chars().next()
    }

    fn peek_next_char(&self) -> Option<char> {
        let mut chars = self.source[self.cursor..].chars();
        chars.next()?;
        chars.next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.cursor += ch.len_utf8();
        Some(ch)
    }

    fn bump_bytes(&mut self, count: usize) {
        self.cursor += count;
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_start(ch)
}

fn is_ident_continue(ch: char) -> bool {
    unicode_ident::is_xid_continue(ch)
}

fn is_binary_digit(ch: char) -> bool {
    matches!(ch, '0' | '1')
}

fn is_octal_digit(ch: char) -> bool {
    matches!(ch, '0'..='7')
}

fn is_hex_digit(ch: char) -> bool {
    ch.is_ascii_hexdigit()
}

fn is_valid_digit_sequence(text: &str, is_digit: impl Fn(char) -> bool) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !is_digit(first) {
        return false;
    }

    let mut prev_underscore = false;
    for ch in chars {
        if is_digit(ch) {
            prev_underscore = false;
        } else if ch == '_' {
            if prev_underscore {
                return false;
            }
            prev_underscore = true;
        } else {
            return false;
        }
    }

    !prev_underscore
}

fn keyword_kind(text: &str) -> Option<Keyword> {
    match text {
        "bundle" => Some(Keyword::Bundle),
        "module" => Some(Keyword::Module),
        "end" => Some(Keyword::End),
        "let" => Some(Keyword::Let),
        "do" => Some(Keyword::Do),
        "use" => Some(Keyword::Use),
        "as" => Some(Keyword::As),
        "in" => Some(Keyword::In),
        "type" => Some(Keyword::Type),
        "trait" => Some(Keyword::Trait),
        "impl" => Some(Keyword::Impl),
        "for" => Some(Keyword::For),
        "where" => Some(Keyword::Where),
        "fn" => Some(Keyword::Fn),
        "if" => Some(Keyword::If),
        "then" => Some(Keyword::Then),
        "else" => Some(Keyword::Else),
        "match" => Some(Keyword::Match),
        "with" => Some(Keyword::With),
        "wasm" => Some(Keyword::Wasm),
        "root" => Some(Keyword::Root),
        "true" => Some(Keyword::True),
        "false" => Some(Keyword::False),
        "mod" => Some(Keyword::Mod),
        "xor" => Some(Keyword::Xor),
        "or" => Some(Keyword::Or),
        "and" => Some(Keyword::And),
        "not" => Some(Keyword::Not),
        _ => None,
    }
}

fn is_operator_like_bracketed_identifier(text: &str) -> bool {
    let trimmed = text.trim();
    matches!(
        trimmed,
        "+" | "-"
            | "*"
            | "/"
            | "mod"
            | "xor"
            | "or"
            | "and"
            | "not"
            | "~"
            | ">>"
            | "<<"
            | "|>"
            | "+>"
            | "*>"
            | ";"
            | "=="
            | "!="
            | "<"
            | "<="
            | ">"
            | ">="
    )
}

fn single_char_token_kind(ch: char) -> Option<TokenKind> {
    let kind = match ch {
        '+' => TokenKind::Operator(Operator::Plus),
        '-' => TokenKind::Operator(Operator::Minus),
        '*' => TokenKind::Operator(Operator::Star),
        '/' => TokenKind::Operator(Operator::Slash),
        '<' => TokenKind::Operator(Operator::Less),
        '>' => TokenKind::Operator(Operator::Greater),
        ';' => TokenKind::Operator(Operator::Semicolon),
        '(' => TokenKind::Punct(Punct::LParen),
        ')' => TokenKind::Punct(Punct::RParen),
        '{' => TokenKind::Punct(Punct::LBrace),
        '}' => TokenKind::Punct(Punct::RBrace),
        '[' => TokenKind::Punct(Punct::LBracket),
        ']' => TokenKind::Punct(Punct::RBracket),
        ',' => TokenKind::Punct(Punct::Comma),
        ':' => TokenKind::Punct(Punct::Colon),
        '.' => TokenKind::Punct(Punct::Dot),
        '|' => TokenKind::Punct(Punct::Pipe),
        '=' => TokenKind::Punct(Punct::Equals),
        '~' => TokenKind::Punct(Punct::Tilde),
        '$' => TokenKind::Punct(Punct::Dollar),
        _ => return None,
    };

    Some(kind)
}
