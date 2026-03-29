use crate::reporting::TextRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Keyword {
    Bundle,
    Import,
    Module,
    End,
    Let,
    Do,
    Use,
    As,
    In,
    Type,
    Trait,
    Impl,
    For,
    Where,
    Fn,
    If,
    Then,
    Else,
    Match,
    With,
    Wasm,
    Root,
    True,
    False,
    Mod,
    Xor,
    Or,
    And,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Operator {
    Plus,
    Minus,
    Star,
    Slash,
    ComposeRight,
    ComposeLeft,
    PipeRight,
    PlusPipe,
    StarPipe,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Semicolon,
    Arrow,
    FatArrow,
    PathSep,
    Spread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    Pipe,
    Equals,
    Tilde,
    Dollar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum TokenKind {
    Keyword(Keyword),
    Operator(Operator),
    Punct(Punct),
    Ident,
    BracketedIdent,
    IntegerLiteral,
    NaturalLiteral,
    RealLiteral,
    StringLiteral,
    GlyphLiteral,
    FormatStringLiteral,
    EndOfFile,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct Token {
    pub kind: TokenKind,
    pub range: TextRange,
}

impl Token {
    pub fn new(kind: TokenKind, range: TextRange) -> Self {
        Self { kind, range }
    }
}
