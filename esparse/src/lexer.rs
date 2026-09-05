//! Tokens, and the scanner that reads them off the source one at a time.
//!
//! The scanner is driven by the parser rather than run ahead of it, because
//! two token classes cannot be told apart without knowing what the grammar
//! expects next: a `/` opens a regular expression where an operand may start
//! and divides everywhere else, and a `}` closes a block everywhere except
//! inside a template, where it resumes the literal. The parser asks for those
//! readings explicitly, by [`Lexer::regex_at`] and
//! [`Lexer::template_continuation_at`], re-reading from the token it holds.
//!
//! Scanning is over bytes, as oxc's is: nearly all of a JavaScript module is
//! ASCII, and a byte can be classified by a table lookup where a character
//! would first have to be decoded. Every loop below stops on an ASCII byte,
//! and only ever steps over a multi-byte character whole, so the cursor never
//! lands inside one. Unicode identifiers and escapes are read on slower paths
//! that decode characters, which the runtime the backend emits never takes.

use crate::Error;

/// What a token is. Keywords are identifiers here, distinguished by the
/// token's [`Word`] — the parser decides which spellings are reserved and
/// where, since `of`, `get` and `async` are only keywords in some positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Eof,
    Ident,
    PrivateName,
    Number,
    String,
    /// A template with no substitution: `` `text` ``.
    TemplateNoSubstitution,
    /// The opening part of a template up to its first `${`.
    TemplateHead,
    /// A middle part of a template: from a closing `}` to the next `${`.
    TemplateMiddle,
    /// The closing part of a template: from a closing `}` to the backtick.
    TemplateTail,
    Regex,
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    Ellipsis,
    Semicolon,
    Comma,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Eq2,
    Neq,
    Eq3,
    Neq2,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Star2,
    Plus2,
    Minus2,
    ShiftLeft,
    ShiftRight,
    ShiftRight3,
    Amp,
    Pipe,
    Caret,
    Bang,
    Tilde,
    Amp2,
    Pipe2,
    Question2,
    Question,
    QuestionDot,
    Colon,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    Star2Eq,
    ShiftLeftEq,
    ShiftRightEq,
    ShiftRight3Eq,
    AmpEq,
    PipeEq,
    CaretEq,
    Amp2Eq,
    Pipe2Eq,
    Question2Eq,
    Arrow,
}

/// The spelling of an identifier token, when it is one the grammar has a
/// use for. Classified once when the token is read, so the parser compares
/// an integer rather than a string each time it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// Any identifier the grammar has no special reading for.
    Other,
    // Reserved words: never identifiers in module code.
    Await,
    Break,
    Case,
    Catch,
    Class,
    Const,
    Continue,
    Debugger,
    Default,
    Delete,
    Do,
    Else,
    Enum,
    Export,
    Extends,
    False,
    Finally,
    For,
    Function,
    If,
    Implements,
    Import,
    In,
    Instanceof,
    Interface,
    Let,
    New,
    Null,
    Package,
    Private,
    Protected,
    Public,
    Return,
    Static,
    Super,
    Switch,
    This,
    Throw,
    True,
    Try,
    Typeof,
    Var,
    Void,
    While,
    With,
    Yield,
    // Contextual words: identifiers everywhere but in a position that reads
    // them specially.
    As,
    Async,
    From,
    Get,
    Meta,
    Of,
    Set,
    Target,
    /// `eval` and `arguments`, which strict code cannot bind.
    Unbindable,
}

/// One token: its kind, where it sits, and whether a line terminator came
/// before it, which is what automatic semicolon insertion and the restricted
/// productions ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub word: Word,
    pub start: usize,
    pub end: usize,
    pub newline_before: bool,
}

pub struct Lexer<'s> {
    source: &'s str,
    bytes: &'s [u8],
    pos: usize,
}

/// Which ASCII bytes may start an identifier, and which may continue one.
struct Ascii {
    start: [bool; 256],
    continue_: [bool; 256],
}

const ASCII: Ascii = {
    let mut start = [false; 256];
    let mut continue_ = [false; 256];
    let mut b = 0;
    while b < 128 {
        let byte = b as u8;
        let s = byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$';
        start[b] = s;
        continue_[b] = s || byte.is_ascii_digit();
        b += 1;
    }
    Ascii { start, continue_ }
};

impl Kind {
    /// Whether this kind is an assignment operator, compound or plain.
    pub fn is_assignment(self) -> bool {
        matches!(
            self,
            Kind::Eq
                | Kind::PlusEq
                | Kind::MinusEq
                | Kind::StarEq
                | Kind::SlashEq
                | Kind::PercentEq
                | Kind::Star2Eq
                | Kind::ShiftLeftEq
                | Kind::ShiftRightEq
                | Kind::ShiftRight3Eq
                | Kind::AmpEq
                | Kind::PipeEq
                | Kind::CaretEq
                | Kind::Amp2Eq
                | Kind::Pipe2Eq
                | Kind::Question2Eq
        )
    }
}

impl Word {
    /// The word `text` is, if any. Reserved words are two to ten lowercase
    /// ASCII letters, which rules most identifiers out before any comparison.
    fn classify(text: &str) -> Self {
        let bytes = text.as_bytes();
        if bytes.len() < 2 || bytes.len() > 10 || !bytes[0].is_ascii_lowercase() {
            return Word::Other;
        }
        match text {
            "as" => Word::As,
            "do" => Word::Do,
            "if" => Word::If,
            "in" => Word::In,
            "of" => Word::Of,
            "for" => Word::For,
            "get" => Word::Get,
            "let" => Word::Let,
            "new" => Word::New,
            "set" => Word::Set,
            "try" => Word::Try,
            "var" => Word::Var,
            "case" => Word::Case,
            "else" => Word::Else,
            "enum" => Word::Enum,
            "eval" => Word::Unbindable,
            "from" => Word::From,
            "meta" => Word::Meta,
            "null" => Word::Null,
            "this" => Word::This,
            "true" => Word::True,
            "void" => Word::Void,
            "with" => Word::With,
            "async" => Word::Async,
            "await" => Word::Await,
            "break" => Word::Break,
            "catch" => Word::Catch,
            "class" => Word::Class,
            "const" => Word::Const,
            "false" => Word::False,
            "super" => Word::Super,
            "throw" => Word::Throw,
            "while" => Word::While,
            "yield" => Word::Yield,
            "delete" => Word::Delete,
            "export" => Word::Export,
            "import" => Word::Import,
            "public" => Word::Public,
            "return" => Word::Return,
            "static" => Word::Static,
            "switch" => Word::Switch,
            "target" => Word::Target,
            "typeof" => Word::Typeof,
            "default" => Word::Default,
            "extends" => Word::Extends,
            "finally" => Word::Finally,
            "package" => Word::Package,
            "private" => Word::Private,
            "continue" => Word::Continue,
            "debugger" => Word::Debugger,
            "function" => Word::Function,
            "arguments" => Word::Unbindable,
            "interface" => Word::Interface,
            "protected" => Word::Protected,
            "implements" => Word::Implements,
            "instanceof" => Word::Instanceof,
            _ => Word::Other,
        }
    }

    /// Whether this word is never an identifier in module code.
    pub fn is_reserved(self) -> bool {
        !matches!(
            self,
            Word::Other
                | Word::As
                | Word::Async
                | Word::From
                | Word::Get
                | Word::Meta
                | Word::Of
                | Word::Set
                | Word::Target
                | Word::Unbindable
        )
    }
}

impl<'s> Lexer<'s> {
    pub fn new(source: &'s str) -> Self {
        // A hashbang line is a comment only at the very start.
        let pos = if source.starts_with("#!") {
            source.find(is_line_terminator).unwrap_or(source.len())
        } else {
            0
        };
        Self {
            source,
            bytes: source.as_bytes(),
            pos,
        }
    }

    /// The text of a token.
    pub fn text(&self, token: Token) -> &'s str {
        &self.source[token.start..token.end]
    }

    /// The next token, with `/` read as division and `}` as a brace.
    pub fn next(&mut self) -> Result<Token, Error> {
        let newline_before = self.skip_trivia()?;
        let start = self.pos;
        let Some(&b) = self.bytes.get(self.pos) else {
            return Ok(self.token(Kind::Eof, start, newline_before));
        };
        if ASCII.start[b as usize] {
            self.pos += 1;
            self.identifier_tail(start)?;
            let mut token = self.token(Kind::Ident, start, newline_before);
            token.word = Word::classify(self.text(token));
            return Ok(token);
        }
        let kind = match b {
            b'"' | b'\'' => self.string(b)?,
            b'`' => self.template_part(true)?,
            b'#' => {
                self.pos += 1;
                if !self.identifier_head()? {
                    return Err(self.error("expected a name after `#`", start));
                }
                Kind::PrivateName
            }
            b'0'..=b'9' => self.number()?,
            b'.' if self.byte_at(1).is_some_and(|b| b.is_ascii_digit()) => self.number()?,
            b'\\' | 0x80.. => {
                if self.identifier_head()? {
                    let mut token = self.token(Kind::Ident, start, newline_before);
                    token.word = Word::classify(self.text(token));
                    return Ok(token);
                }
                return Err(self.error("unexpected character", start));
            }
            _ => self.punctuator()?,
        };
        Ok(self.token(kind, start, newline_before))
    }

    /// The regular expression literal starting at `at`, which is where the
    /// parser's current `/` or `/=` token began.
    pub fn regex_at(&mut self, at: usize, newline_before: bool) -> Result<Token, Error> {
        self.pos = at + 1;
        let mut in_class = false;
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(self.error("unterminated regular expression", at));
            };
            self.pos += 1;
            match b {
                b'\\' => match self.next_char() {
                    Some(c) if !is_line_terminator(c) => {}
                    _ => return Err(self.error("unterminated regular expression", at)),
                },
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => break,
                b'\n' | b'\r' => {
                    return Err(self.error("unterminated regular expression", at));
                }
                0x80.. => {
                    self.pos -= 1;
                    if self.next_char().is_some_and(is_line_terminator) {
                        return Err(self.error("unterminated regular expression", at));
                    }
                }
                _ => {}
            }
        }
        let flags_start = self.pos;
        while let Some(c) = self.peek_char() {
            if is_id_continue(c) {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let mut seen = [false; 128];
        for flag in self.source[flags_start..self.pos].chars() {
            if !"dgimsuvy".contains(flag) || seen[flag as usize] {
                return Err(self.error("invalid regular expression flags", flags_start));
            }
            seen[flag as usize] = true;
        }
        if seen[b'u' as usize] && seen[b'v' as usize] {
            return Err(self.error(
                "the `u` and `v` regular expression flags cannot be combined",
                flags_start,
            ));
        }
        Ok(self.token(Kind::Regex, at, newline_before))
    }

    /// The template part resuming at `at`, which is where the parser's
    /// current `}` token began: a middle if another substitution follows, a
    /// tail if the literal closes.
    pub fn template_continuation_at(&mut self, at: usize) -> Result<Token, Error> {
        self.pos = at;
        let kind = self.template_part(false)?;
        Ok(self.token(kind, at, false))
    }

    fn token(&self, kind: Kind, start: usize, newline_before: bool) -> Token {
        Token {
            kind,
            word: Word::Other,
            start,
            end: self.pos,
            newline_before,
        }
    }

    fn error(&self, message: &str, at: usize) -> Error {
        Error::at(self.source, message, at)
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.bytes.get(self.pos) == Some(&expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Skip whitespace and comments, saying whether a line terminator was
    /// among them.
    fn skip_trivia(&mut self) -> Result<bool, Error> {
        let mut newline = false;
        while let Some(&b) = self.bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | 0x0B | 0x0C => self.pos += 1,
                b'\n' | b'\r' => {
                    newline = true;
                    self.pos += 1;
                }
                b'/' => match self.byte_at(1) {
                    Some(b'/') => {
                        self.pos += 2;
                        // Line comments end at the first line terminator, the
                        // Unicode ones included, which the loop above the
                        // comment is left to consume.
                        while let Some(&b) = self.bytes.get(self.pos) {
                            if matches!(b, b'\n' | b'\r') {
                                break;
                            }
                            if b < 0x80 {
                                self.pos += 1;
                                continue;
                            }
                            let c = self.peek_char().expect("a byte was there");
                            if is_line_terminator(c) {
                                break;
                            }
                            self.pos += c.len_utf8();
                        }
                    }
                    Some(b'*') => {
                        let start = self.pos;
                        self.pos += 2;
                        loop {
                            let Some(&b) = self.bytes.get(self.pos) else {
                                return Err(self.error("unterminated comment", start));
                            };
                            self.pos += 1;
                            match b {
                                b'*' if self.eat(b'/') => break,
                                b'\n' | b'\r' => newline = true,
                                0x80.. => {
                                    self.pos -= 1;
                                    if self.next_char().is_some_and(is_line_terminator) {
                                        newline = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => break,
                },
                0x80.. => {
                    let c = self.peek_char().expect("a byte was there");
                    if is_line_terminator(c) {
                        newline = true;
                    } else if !is_whitespace(c) {
                        break;
                    }
                    self.pos += c.len_utf8();
                }
                _ => break,
            }
        }
        Ok(newline)
    }

    /// Read an identifier at the cursor, saying whether there was one.
    fn identifier_head(&mut self) -> Result<bool, Error> {
        let start = self.pos;
        match self.bytes.get(self.pos) {
            Some(&b) if ASCII.start[b as usize] => self.pos += 1,
            Some(&(b'\\' | 0x80..)) => {
                let escaped = self.bytes[self.pos] == b'\\';
                let c = self.escaped_or_unicode_char()?;
                if !is_id_start(c) {
                    if escaped {
                        return Err(self.error("invalid character in identifier", start));
                    }
                    self.pos = start;
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        self.identifier_tail(start)?;
        Ok(true)
    }

    /// The rest of an identifier whose first character has been read: ASCII
    /// by table lookup, with a slower path once an escape or a non-ASCII
    /// character appears.
    fn identifier_tail(&mut self, start: usize) -> Result<(), Error> {
        while let Some(&b) = self.bytes.get(self.pos) {
            if ASCII.continue_[b as usize] {
                self.pos += 1;
            } else if b == b'\\' || b >= 0x80 {
                let at = self.pos;
                let c = self.escaped_or_unicode_char()?;
                if !is_id_continue(c) {
                    if b == b'\\' {
                        return Err(self.error("invalid character in identifier", start));
                    }
                    self.pos = at;
                    return Ok(());
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    /// One character of an identifier that is either escaped or non-ASCII,
    /// consumed.
    fn escaped_or_unicode_char(&mut self) -> Result<char, Error> {
        if self.eat(b'\\') {
            let at = self.pos - 1;
            if !self.eat(b'u') {
                return Err(self.error("invalid escape in identifier", at));
            }
            return self.unicode_escape(at);
        }
        Ok(self.next_char().expect("a byte was there"))
    }

    /// The character a `\u` escape spells, with the cursor just past the `u`.
    fn unicode_escape(&mut self, at: usize) -> Result<char, Error> {
        let code = if self.eat(b'{') {
            let digits_start = self.pos;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_hexdigit) {
                self.pos += 1;
            }
            let digits = &self.source[digits_start..self.pos];
            if digits.is_empty() || !self.eat(b'}') {
                return Err(self.error("invalid unicode escape", at));
            }
            u32::from_str_radix(digits, 16).ok()
        } else {
            let digits_start = self.pos;
            for _ in 0..4 {
                if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_hexdigit) {
                    return Err(self.error("invalid unicode escape", at));
                }
                self.pos += 1;
            }
            u32::from_str_radix(&self.source[digits_start..self.pos], 16).ok()
        };
        code.and_then(char::from_u32)
            .ok_or_else(|| self.error("invalid unicode escape", at))
    }

    fn number(&mut self) -> Result<Kind, Error> {
        let start = self.pos;
        let mut integer_only = false;
        if self.eat(b'0') {
            match self.bytes.get(self.pos) {
                Some(b'x' | b'X') => {
                    self.pos += 1;
                    self.digits(start, |b| b.is_ascii_hexdigit())?;
                    integer_only = true;
                }
                Some(b'o' | b'O') => {
                    self.pos += 1;
                    self.digits(start, |b| matches!(b, b'0'..=b'7'))?;
                    integer_only = true;
                }
                Some(b'b' | b'B') => {
                    self.pos += 1;
                    self.digits(start, |b| matches!(b, b'0' | b'1'))?;
                    integer_only = true;
                }
                Some(b'0'..=b'9' | b'_') => {
                    return Err(self.error("legacy octal literals are not allowed", start));
                }
                _ => {}
            }
        } else if self.bytes.get(self.pos) != Some(&b'.') {
            self.digits(start, |b| b.is_ascii_digit())?;
        }
        if !integer_only {
            let mut fraction = false;
            if self.eat(b'.') {
                fraction = true;
                if self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.digits(start, |b| b.is_ascii_digit())?;
                }
            }
            if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
                fraction = true;
                self.pos += 1;
                if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    return Err(self.error("missing exponent digits", start));
                }
                self.digits(start, |b| b.is_ascii_digit())?;
            }
            if fraction && self.bytes.get(self.pos) == Some(&b'n') {
                return Err(self.error("a BigInt literal must be an integer", start));
            }
        }
        self.eat(b'n');
        let separated = match self.bytes.get(self.pos) {
            None => true,
            Some(&b) if b < 0x80 => !ASCII.continue_[b as usize],
            Some(_) => !self.peek_char().is_some_and(is_id_start),
        };
        if !separated {
            return Err(self.error("a numeric literal must be followed by a separator", start));
        }
        Ok(Kind::Number)
    }

    /// Digits of one radix with numeric separators between them: at least
    /// one digit, no separator at either end or beside another.
    fn digits(&mut self, start: usize, digit: impl Fn(u8) -> bool) -> Result<(), Error> {
        let mut count = 0;
        let mut last_separator = false;
        while let Some(&b) = self.bytes.get(self.pos) {
            if digit(b) {
                count += 1;
                last_separator = false;
            } else if b == b'_' && count > 0 && !last_separator {
                last_separator = true;
            } else {
                break;
            }
            self.pos += 1;
        }
        if count == 0 || last_separator {
            return Err(self.error("invalid numeric literal", start));
        }
        Ok(())
    }

    fn string(&mut self, quote: u8) -> Result<Kind, Error> {
        let start = self.pos;
        self.pos += 1;
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(self.error("unterminated string", start));
            };
            self.pos += 1;
            match b {
                b if b == quote => return Ok(Kind::String),
                b'\\' => self.escape(start, false)?,
                b'\n' | b'\r' => return Err(self.error("unterminated string", start)),
                _ => {}
            }
        }
    }

    /// One escape sequence with the cursor just past its backslash. Modules
    /// are strict code, where the legacy octal escapes are errors; a template
    /// forgives nothing more, except that its parts are read the same whether
    /// tagged or not, so a tagged template's invalid escapes are refused here
    /// where the language would accept them.
    fn escape(&mut self, literal_start: usize, template: bool) -> Result<(), Error> {
        let at = self.pos - 1;
        let Some(c) = self.next_char() else {
            return Err(self.error("unterminated string", literal_start));
        };
        match c {
            'x' => {
                for _ in 0..2 {
                    if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_hexdigit) {
                        return Err(self.error("invalid hexadecimal escape", at));
                    }
                    self.pos += 1;
                }
            }
            'u' => {
                self.unicode_escape(at)?;
            }
            '0' if !self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) => {}
            '0'..='9' => {
                return Err(self.error(
                    if template {
                        "octal escapes are not allowed in templates"
                    } else {
                        "octal escapes are not allowed in strict mode"
                    },
                    at,
                ));
            }
            '\r' => {
                self.eat(b'\n');
            }
            _ => {}
        }
        Ok(())
    }

    /// A template part from the cursor, which sits on the opening backtick
    /// when `opening`, and on the `}` closing a substitution otherwise.
    fn template_part(&mut self, opening: bool) -> Result<Kind, Error> {
        let start = self.pos;
        self.pos += 1;
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(self.error("unterminated template", start));
            };
            self.pos += 1;
            match b {
                b'`' => {
                    return Ok(if opening {
                        Kind::TemplateNoSubstitution
                    } else {
                        Kind::TemplateTail
                    });
                }
                b'$' if self.eat(b'{') => {
                    return Ok(if opening {
                        Kind::TemplateHead
                    } else {
                        Kind::TemplateMiddle
                    });
                }
                b'\\' => self.escape(start, true)?,
                _ => {}
            }
        }
    }

    /// A punctuator, read by its first byte and then by however many of the
    /// following bytes extend it.
    fn punctuator(&mut self) -> Result<Kind, Error> {
        let start = self.pos;
        let b = self.bytes[self.pos];
        self.pos += 1;
        let kind = match b {
            b'{' => Kind::LBrace,
            b'}' => Kind::RBrace,
            b'(' => Kind::LParen,
            b')' => Kind::RParen,
            b'[' => Kind::LBracket,
            b']' => Kind::RBracket,
            b';' => Kind::Semicolon,
            b',' => Kind::Comma,
            b':' => Kind::Colon,
            b'~' => Kind::Tilde,
            b'.' => {
                if self.bytes[self.pos..].starts_with(b"..") {
                    self.pos += 2;
                    Kind::Ellipsis
                } else {
                    Kind::Dot
                }
            }
            b'=' => {
                if self.eat(b'=') {
                    if self.eat(b'=') { Kind::Eq3 } else { Kind::Eq2 }
                } else if self.eat(b'>') {
                    Kind::Arrow
                } else {
                    Kind::Eq
                }
            }
            b'!' => {
                if self.eat(b'=') {
                    if self.eat(b'=') {
                        Kind::Neq2
                    } else {
                        Kind::Neq
                    }
                } else {
                    Kind::Bang
                }
            }
            b'<' => {
                if self.eat(b'<') {
                    if self.eat(b'=') {
                        Kind::ShiftLeftEq
                    } else {
                        Kind::ShiftLeft
                    }
                } else if self.eat(b'=') {
                    Kind::LtEq
                } else {
                    Kind::Lt
                }
            }
            b'>' => {
                if self.eat(b'>') {
                    if self.eat(b'>') {
                        if self.eat(b'=') {
                            Kind::ShiftRight3Eq
                        } else {
                            Kind::ShiftRight3
                        }
                    } else if self.eat(b'=') {
                        Kind::ShiftRightEq
                    } else {
                        Kind::ShiftRight
                    }
                } else if self.eat(b'=') {
                    Kind::GtEq
                } else {
                    Kind::Gt
                }
            }
            b'+' => {
                if self.eat(b'+') {
                    Kind::Plus2
                } else if self.eat(b'=') {
                    Kind::PlusEq
                } else {
                    Kind::Plus
                }
            }
            b'-' => {
                if self.eat(b'-') {
                    Kind::Minus2
                } else if self.eat(b'=') {
                    Kind::MinusEq
                } else {
                    Kind::Minus
                }
            }
            b'*' => {
                if self.eat(b'*') {
                    if self.eat(b'=') {
                        Kind::Star2Eq
                    } else {
                        Kind::Star2
                    }
                } else if self.eat(b'=') {
                    Kind::StarEq
                } else {
                    Kind::Star
                }
            }
            b'/' => {
                if self.eat(b'=') {
                    Kind::SlashEq
                } else {
                    Kind::Slash
                }
            }
            b'%' => {
                if self.eat(b'=') {
                    Kind::PercentEq
                } else {
                    Kind::Percent
                }
            }
            b'&' => {
                if self.eat(b'&') {
                    if self.eat(b'=') {
                        Kind::Amp2Eq
                    } else {
                        Kind::Amp2
                    }
                } else if self.eat(b'=') {
                    Kind::AmpEq
                } else {
                    Kind::Amp
                }
            }
            b'|' => {
                if self.eat(b'|') {
                    if self.eat(b'=') {
                        Kind::Pipe2Eq
                    } else {
                        Kind::Pipe2
                    }
                } else if self.eat(b'=') {
                    Kind::PipeEq
                } else {
                    Kind::Pipe
                }
            }
            b'^' => {
                if self.eat(b'=') {
                    Kind::CaretEq
                } else {
                    Kind::Caret
                }
            }
            b'?' => {
                if self.eat(b'?') {
                    if self.eat(b'=') {
                        Kind::Question2Eq
                    } else {
                        Kind::Question2
                    }
                } else if self.bytes.get(self.pos) == Some(&b'.')
                    && !self.byte_at(1).is_some_and(|b| b.is_ascii_digit())
                {
                    // `?.` is optional chaining unless a digit follows,
                    // where `a?.5:b` is a conditional over a fraction.
                    self.pos += 1;
                    Kind::QuestionDot
                } else {
                    Kind::Question
                }
            }
            _ => return Err(self.error("unexpected character", start)),
        };
        Ok(kind)
    }
}

pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\u{000B}' | '\u{000C}' | ' ' | '\u{00A0}' | '\u{FEFF}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

fn is_id_start(c: char) -> bool {
    c == '$'
        || c == '_'
        || c.is_ascii_alphabetic()
        || (!c.is_ascii() && unicode_id_start::is_id_start(c))
}

fn is_id_continue(c: char) -> bool {
    c == '$'
        || c == '_'
        || c.is_ascii_alphanumeric()
        || c == '\u{200C}'
        || c == '\u{200D}'
        || (!c.is_ascii() && unicode_id_start::is_id_continue(c))
}
