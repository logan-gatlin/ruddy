//! The recognizer: a pushdown automaton over an explicit stack of frames.
//!
//! A recursive-descent parser keeps the grammar's nesting on the native
//! stack, so what it can read is bounded by a thread's stack size — and the
//! runtime the JavaScript backend generates nests as deeply as the program it
//! compiles. Here every point at which a recursive parser would call itself
//! is instead a [`Frame`] pushed onto a `Vec`: a frame that needs a
//! sub-expression pushes its own continuation, then the frame that reads the
//! sub-expression, and the main loop pops and runs whatever is on top. Memory
//! grows with nesting; the native stack does not.
//!
//! Expressions leave an [`Expr`] on a value stack for their continuation to
//! read. No tree is built: the backend only asks whether the module parses and
//! what its top-level items are, and the [`Expr`] carries the few facts the
//! grammar needs to look back on — whether what was just read could be
//! assigned to, could be a destructuring pattern, or was an arrow function.

use crate::{
    Error, Item,
    lexer::{Kind, Lexer, Token, Word},
};

/// Which statements a statement list runs until.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// The `}` closing a block or body.
    Brace,
    /// The next `case`, `default` or `}` of a switch.
    Case,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclKind {
    Var,
    Let,
    Const,
}

/// The logical operator that produced an expression unparenthesized, for the
/// rule that `??` cannot mix with `||` or `&&` without parentheses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Logical {
    #[default]
    None,
    AndOr,
    Nullish,
}

/// What the grammar can still ask about an expression after reading it.
#[derive(Debug, Clone, Copy, Default)]
struct Expr {
    /// A bare identifier: the only thing a parameter or `delete` looks for.
    bare_ident: bool,
    /// An identifier or member access — something `=` and `++` accept.
    assignable: bool,
    /// An object or array literal every part of which could be an assignment
    /// pattern, or an assignment whose left side could.
    pattern: bool,
    /// As `pattern`, but every leaf is an identifier, as a binding requires.
    binding: bool,
    /// Holds a `{ a = 1 }` shorthand initializer, which only a pattern may.
    cover_init: bool,
    /// Ends in an optional chain, so it is neither a target nor a tag.
    optional: bool,
    /// The bare identifier `async`, which may head an async arrow function.
    ident_async: bool,
    /// An arrow function, which nothing may continue.
    arrow: bool,
    /// An unparenthesized unary expression, which `**` refuses as a base.
    unary: bool,
    logical: Logical,
    /// For a list in parentheses: how many elements it had.
    count: u32,
}

/// What a function's body may do.
#[derive(Debug, Clone, Copy)]
struct Scope {
    is_async: bool,
    is_generator: bool,
    /// Whether `return` is allowed: inside any function, arrow or method.
    is_function: bool,
}

/// One suspended step of the grammar.
#[derive(Debug, Clone, Copy)]
enum Frame {
    // Statements.
    ModuleBody,
    StatementList(Stop),
    Statement,
    /// The body of an `if`, a loop or a label: a statement, never a
    /// declaration.
    Substatement,
    Block,
    ExpectRBrace,
    Semicolon,
    AfterIfCond,
    AfterIfThen,
    AfterWhileCond,
    AfterDoBody,
    AfterDoCond,
    ForTest,
    ForAfterTest,
    ForUpdate,
    ForAfterUpdate,
    ForAfterExprInit {
        for_await: bool,
    },
    ForInOfBody,
    AfterTryBlock,
    AfterCatchParam,
    AfterCatchBlock,
    AfterSwitchDisc,
    SwitchBody,
    AfterCaseExpr,
    VarDecls {
        kind: DeclKind,
        no_in: bool,
        in_for: bool,
        for_await: bool,
        first: bool,
    },
    VarAfterTarget {
        kind: DeclKind,
        no_in: bool,
        in_for: bool,
        for_await: bool,
        first: bool,
    },
    VarAfterInit {
        kind: DeclKind,
        no_in: bool,
        in_for: bool,
        for_await: bool,
    },
    /// Pop and discard a value, refusing a stray cover initializer.
    Discard,

    // Functions and classes.
    FunctionRest {
        expression: bool,
        is_async: bool,
        name_optional: bool,
    },
    Params,
    ParamsAfterOne,
    ParamsAfterRest,
    FunctionBody,
    FunctionEnd {
        expression: bool,
    },
    ArrowBody {
        no_in: bool,
    },
    ArrowEnd {
        expression_body: bool,
    },
    MethodRest {
        is_async: bool,
        is_generator: bool,
    },
    MethodEnd,
    ClassRest {
        expression: bool,
        name_optional: bool,
    },
    ClassAfterHeritage {
        expression: bool,
    },
    ClassBody,
    ClassAfterComputedKey {
        is_async: bool,
        is_generator: bool,
        accessor: bool,
    },
    ClassEnd {
        expression: bool,
    },

    // Bindings.
    BindingTarget,
    BindingElement,
    BindingAfterTarget,
    BindingArray,
    BindingArrayAfterElem,
    BindingArrayAfterRest,
    BindingObject,
    BindingObjectAfterComputedKey,
    BindingObjectAfterProp,

    // Expressions.
    ExprSeq {
        no_in: bool,
    },
    SeqNext {
        no_in: bool,
        count: u32,
    },
    Assign {
        no_in: bool,
    },
    AssignTail {
        no_in: bool,
    },
    AssignRhs {
        lhs: Expr,
    },
    CondThen {
        no_in: bool,
    },
    CondEnd,
    Binary {
        min: u8,
        no_in: bool,
    },
    BinaryLoop {
        min: u8,
        no_in: bool,
    },
    BinaryCombine {
        logical: Logical,
    },
    Unary {
        no_in: bool,
    },
    UnaryCombine {
        op: Kind,
        delete: bool,
    },
    Postfix,
    Lhs {
        no_in: bool,
    },
    NewAfterCallee {
        no_in: bool,
    },
    Chain {
        allow_call: bool,
        no_in: bool,
    },
    AfterComputed {
        optional: bool,
    },
    Args {
        no_in: bool,
        summary: Expr,
        rest_open: bool,
    },
    ArgsAfterOne {
        no_in: bool,
        summary: Expr,
        spread: bool,
    },
    AfterArgs {
        candidate: bool,
        optional: bool,
        no_in: bool,
    },
    CoverEnd {
        no_in: bool,
    },
    Primary {
        no_in: bool,
    },
    AfterImportCall,
    AfterImportCallSecond,
    Template,
    TemplateAfterSub,
    ArrayElems {
        summary: Expr,
    },
    ArrayAfterElem {
        summary: Expr,
        spread: bool,
    },
    ObjectProps {
        summary: Expr,
    },
    ObjectAfterComputedKey {
        summary: Expr,
        is_async: bool,
        is_generator: bool,
    },
    ObjectAfterValue {
        summary: Expr,
        spread: bool,
        cover: bool,
    },
    ObjectAfterMethod {
        summary: Expr,
    },
}

pub(crate) struct Parser<'s> {
    lexer: Lexer<'s>,
    source: &'s str,
    tok: Token,
    peeked: Option<Token>,
    frames: Vec<Frame>,
    values: Vec<Expr>,
    scopes: Vec<Scope>,
    items: Vec<Item>,
}

const PREC_NULLISH: u8 = 1;
const PREC_OR: u8 = 2;
const PREC_AND: u8 = 3;
const PREC_EXPONENT: u8 = 12;

impl Expr {
    fn value() -> Self {
        Self::default()
    }

    fn ident() -> Self {
        Self {
            bare_ident: true,
            assignable: true,
            binding: true,
            ..Self::default()
        }
    }

    fn member(optional: bool) -> Self {
        Self {
            assignable: !optional,
            optional,
            ..Self::default()
        }
    }

    /// A literal or list before any element has been read: a pattern until
    /// an element says otherwise.
    fn open_list() -> Self {
        Self {
            pattern: true,
            binding: true,
            ..Self::default()
        }
    }

    fn could_be_assignment_target(&self) -> bool {
        self.assignable || self.pattern
    }

    fn could_be_binding(&self) -> bool {
        self.bare_ident || self.binding
    }
}

impl<'s> Parser<'s> {
    pub(crate) fn new(source: &'s str) -> Result<Self, Error> {
        let mut lexer = Lexer::new(source);
        let tok = lexer.next()?;
        Ok(Self {
            lexer,
            source,
            tok,
            peeked: None,
            frames: Vec::new(),
            values: Vec::new(),
            scopes: vec![Scope {
                // Modules may `await` at their top level.
                is_async: true,
                is_generator: false,
                is_function: false,
            }],
            items: Vec::new(),
        })
    }

    pub(crate) fn parse_module(mut self) -> Result<Vec<Item>, Error> {
        self.frames.push(Frame::ModuleBody);
        while let Some(frame) = self.frames.pop() {
            self.step(frame)?;
        }
        debug_assert!(self.values.is_empty(), "every value was consumed");
        Ok(self.items)
    }

    // ── Tokens ───────────────────────────────────────────────────────────

    fn text(&self) -> &'s str {
        self.lexer.text(self.tok)
    }

    fn advance(&mut self) -> Result<(), Error> {
        self.tok = match self.peeked.take() {
            Some(token) => token,
            None => self.lexer.next()?,
        };
        Ok(())
    }

    fn peek(&mut self) -> Result<Token, Error> {
        if let Some(token) = self.peeked {
            return Ok(token);
        }
        let token = self.lexer.next()?;
        self.peeked = Some(token);
        Ok(token)
    }

    fn peek_word(&mut self) -> Result<Word, Error> {
        Ok(self.peek()?.word)
    }

    fn is(&self, kind: Kind) -> bool {
        self.tok.kind == kind
    }

    fn is_word(&self, word: Word) -> bool {
        self.tok.kind == Kind::Ident && self.tok.word == word
    }

    fn eat(&mut self, kind: Kind) -> Result<bool, Error> {
        if self.is(kind) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn eat_word(&mut self, word: Word) -> Result<bool, Error> {
        if self.is_word(word) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, kind: Kind, what: &str) -> Result<(), Error> {
        if self.is(kind) {
            self.advance()
        } else {
            Err(self.error(&format!("expected {what}")))
        }
    }

    fn expect_word(&mut self, word: Word, spelling: &str) -> Result<(), Error> {
        if self.is_word(word) {
            self.advance()
        } else {
            Err(self.error(&format!("expected `{spelling}`")))
        }
    }

    fn error(&self, message: &str) -> Error {
        Error::at(self.source, message, self.tok.start)
    }

    /// Whether the current token is an identifier usable as a name that is
    /// bound or referenced, rather than a reserved word.
    fn is_identifier(&self) -> bool {
        self.tok.kind == Kind::Ident && !self.tok.word.is_reserved()
    }

    /// Consume an identifier a declaration may bind.
    fn binding_identifier(&mut self) -> Result<(), Error> {
        if !self.is_identifier() {
            return Err(self.error("expected a name"));
        }
        if self.tok.word == Word::Unbindable {
            return Err(self.error("`eval` and `arguments` cannot be bound in strict mode"));
        }
        self.advance()
    }

    fn scope(&self) -> Scope {
        *self
            .scopes
            .last()
            .expect("the module scope is never popped")
    }

    fn push_scope(&mut self, is_async: bool, is_generator: bool) {
        self.scopes.push(Scope {
            is_async,
            is_generator,
            is_function: true,
        });
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty());
    }

    // ── Values ───────────────────────────────────────────────────────────

    fn push_value(&mut self, value: Expr) {
        self.values.push(value);
    }

    fn pop_value(&mut self) -> Expr {
        self.values
            .pop()
            .expect("a frame pops only what its sub-parse pushed")
    }

    /// Pop a value that is being used as a plain expression, where a cover
    /// initializer has no meaning.
    fn pop_plain(&mut self) -> Result<Expr, Error> {
        let value = self.pop_value();
        if value.cover_init {
            return Err(self.error("a shorthand property initializer is only valid in a pattern"));
        }
        Ok(value)
    }

    fn push(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    // ── Steps ────────────────────────────────────────────────────────────

    fn step(&mut self, frame: Frame) -> Result<(), Error> {
        match frame {
            Frame::ModuleBody => self.module_body(),
            Frame::StatementList(stop) => self.statement_list(stop),
            Frame::Statement => self.statement(),
            Frame::Substatement => {
                let declaration = self.is(Kind::Ident)
                    && (matches!(
                        self.tok.word,
                        Word::Let | Word::Const | Word::Class | Word::Function
                    ) || (self.tok.word == Word::Async
                        && self.peek_word()? == Word::Function
                        && !self.peek()?.newline_before));
                if declaration {
                    return Err(self.error("a declaration is not allowed as a statement body"));
                }
                self.statement()
            }
            Frame::Block => {
                self.expect(Kind::LBrace, "`{`")?;
                self.push(Frame::ExpectRBrace);
                self.push(Frame::StatementList(Stop::Brace));
                Ok(())
            }
            Frame::ExpectRBrace => self.expect(Kind::RBrace, "`}`"),
            Frame::Semicolon => self.semicolon(),
            Frame::AfterIfCond => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                self.push(Frame::AfterIfThen);
                self.push(Frame::Substatement);
                Ok(())
            }
            Frame::AfterIfThen => {
                if self.eat_word(Word::Else)? {
                    self.push(Frame::Substatement);
                }
                Ok(())
            }
            Frame::AfterWhileCond => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                self.push(Frame::Substatement);
                Ok(())
            }
            Frame::AfterDoBody => {
                self.expect_word(Word::While, "while")?;
                self.expect(Kind::LParen, "`(`")?;
                self.push(Frame::AfterDoCond);
                self.push(Frame::ExprSeq { no_in: false });
                Ok(())
            }
            Frame::AfterDoCond => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                // A `do` statement's semicolon is always optional.
                self.eat(Kind::Semicolon)?;
                Ok(())
            }
            Frame::ForTest => {
                if self.eat(Kind::Semicolon)? {
                    self.push(Frame::ForUpdate);
                } else {
                    self.push(Frame::ForAfterTest);
                    self.push(Frame::ExprSeq { no_in: false });
                }
                Ok(())
            }
            Frame::ForAfterTest => {
                self.pop_plain()?;
                self.expect(Kind::Semicolon, "`;`")?;
                self.push(Frame::ForUpdate);
                Ok(())
            }
            Frame::ForUpdate => {
                if self.eat(Kind::RParen)? {
                    self.push(Frame::Substatement);
                } else {
                    self.push(Frame::ForAfterUpdate);
                    self.push(Frame::ExprSeq { no_in: false });
                }
                Ok(())
            }
            Frame::ForAfterUpdate => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                self.push(Frame::Substatement);
                Ok(())
            }
            Frame::ForAfterExprInit { for_await } => {
                let init = self.pop_value();
                if self.is_word(Word::Of) || self.is_word(Word::In) {
                    if !init.could_be_assignment_target() {
                        return Err(self.error("the left side of a `for` loop must be assignable"));
                    }
                    self.for_in_of_rhs(for_await)
                } else {
                    if init.cover_init {
                        return Err(self
                            .error("a shorthand property initializer is only valid in a pattern"));
                    }
                    self.for_classic_head(for_await)
                }
            }
            Frame::ForInOfBody => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                self.push(Frame::Substatement);
                Ok(())
            }
            Frame::AfterTryBlock => {
                if self.eat_word(Word::Catch)? {
                    if self.eat(Kind::LParen)? {
                        self.push(Frame::AfterCatchParam);
                        self.push(Frame::BindingTarget);
                    } else {
                        self.push(Frame::AfterCatchBlock);
                        self.push(Frame::Block);
                    }
                } else if self.eat_word(Word::Finally)? {
                    self.push(Frame::Block);
                } else {
                    return Err(self.error("expected `catch` or `finally`"));
                }
                Ok(())
            }
            Frame::AfterCatchParam => {
                self.pop_value();
                self.expect(Kind::RParen, "`)`")?;
                self.push(Frame::AfterCatchBlock);
                self.push(Frame::Block);
                Ok(())
            }
            Frame::AfterCatchBlock => {
                if self.eat_word(Word::Finally)? {
                    self.push(Frame::Block);
                }
                Ok(())
            }
            Frame::AfterSwitchDisc => {
                self.pop_plain()?;
                self.expect(Kind::RParen, "`)`")?;
                self.expect(Kind::LBrace, "`{`")?;
                self.push(Frame::SwitchBody);
                Ok(())
            }
            Frame::SwitchBody => {
                if self.eat(Kind::RBrace)? {
                    Ok(())
                } else if self.eat_word(Word::Case)? {
                    self.push(Frame::SwitchBody);
                    self.push(Frame::StatementList(Stop::Case));
                    self.push(Frame::AfterCaseExpr);
                    self.push(Frame::ExprSeq { no_in: false });
                    Ok(())
                } else if self.eat_word(Word::Default)? {
                    self.expect(Kind::Colon, "`:`")?;
                    self.push(Frame::SwitchBody);
                    self.push(Frame::StatementList(Stop::Case));
                    Ok(())
                } else {
                    Err(self.error("expected `case`, `default` or `}`"))
                }
            }
            Frame::AfterCaseExpr => {
                self.pop_plain()?;
                self.expect(Kind::Colon, "`:`")
            }
            Frame::VarDecls {
                kind,
                no_in,
                in_for,
                for_await,
                first,
            } => {
                self.push(Frame::VarAfterTarget {
                    kind,
                    no_in,
                    in_for,
                    for_await,
                    first,
                });
                self.push(Frame::BindingTarget);
                Ok(())
            }
            Frame::VarAfterTarget {
                kind,
                no_in,
                in_for,
                for_await,
                first,
            } => {
                let target = self.pop_value();
                if in_for && first && (self.is_word(Word::Of) || self.is_word(Word::In)) {
                    return self.for_in_of_rhs(for_await);
                }
                if self.eat(Kind::Eq)? {
                    self.push(Frame::VarAfterInit {
                        kind,
                        no_in,
                        in_for,
                        for_await,
                    });
                    self.push(Frame::Assign { no_in });
                    return Ok(());
                }
                if kind == DeclKind::Const || !target.bare_ident {
                    return Err(self.error("this declaration needs an initializer"));
                }
                self.var_next(kind, no_in, in_for, for_await)
            }
            Frame::VarAfterInit {
                kind,
                no_in,
                in_for,
                for_await,
            } => {
                self.pop_plain()?;
                self.var_next(kind, no_in, in_for, for_await)
            }
            Frame::Discard => self.pop_plain().map(drop),

            Frame::FunctionRest {
                expression,
                is_async,
                name_optional,
            } => {
                let is_generator = self.eat(Kind::Star)?;
                if self.is_identifier() {
                    self.binding_identifier()?;
                } else if !expression && !name_optional {
                    return Err(self.error("expected a function name"));
                }
                self.expect(Kind::LParen, "`(`")?;
                self.push_scope(is_async, is_generator);
                self.push(Frame::FunctionEnd { expression });
                self.push(Frame::FunctionBody);
                self.push(Frame::Params);
                Ok(())
            }
            Frame::Params => {
                if self.eat(Kind::RParen)? {
                    Ok(())
                } else if self.eat(Kind::Ellipsis)? {
                    self.push(Frame::ParamsAfterRest);
                    self.push(Frame::BindingTarget);
                    Ok(())
                } else {
                    self.push(Frame::ParamsAfterOne);
                    self.push(Frame::BindingElement);
                    Ok(())
                }
            }
            Frame::ParamsAfterOne => {
                if self.eat(Kind::Comma)? {
                    self.push(Frame::Params);
                    Ok(())
                } else {
                    self.expect(Kind::RParen, "`,` or `)`")
                }
            }
            Frame::ParamsAfterRest => {
                self.pop_value();
                self.expect(Kind::RParen, "`)` after the rest parameter")
            }
            Frame::FunctionBody => {
                self.expect(Kind::LBrace, "`{`")?;
                self.push(Frame::ExpectRBrace);
                self.push(Frame::StatementList(Stop::Brace));
                Ok(())
            }
            Frame::FunctionEnd { expression } => {
                self.pop_scope();
                if expression {
                    self.push_value(Expr::value());
                }
                Ok(())
            }
            Frame::ArrowBody { no_in } => {
                if self.is(Kind::LBrace) {
                    self.push(Frame::ArrowEnd {
                        expression_body: false,
                    });
                    self.push(Frame::FunctionBody);
                } else {
                    self.push(Frame::ArrowEnd {
                        expression_body: true,
                    });
                    self.push(Frame::Assign { no_in });
                }
                Ok(())
            }
            Frame::ArrowEnd { expression_body } => {
                if expression_body {
                    self.pop_plain()?;
                }
                self.pop_scope();
                self.push_value(Expr {
                    arrow: true,
                    ..Expr::value()
                });
                Ok(())
            }
            Frame::MethodRest {
                is_async,
                is_generator,
            } => {
                self.expect(Kind::LParen, "`(`")?;
                self.push_scope(is_async, is_generator);
                self.push(Frame::MethodEnd);
                self.push(Frame::FunctionBody);
                self.push(Frame::Params);
                Ok(())
            }
            Frame::MethodEnd => {
                self.pop_scope();
                Ok(())
            }
            Frame::ClassRest {
                expression,
                name_optional,
            } => {
                if self.is_identifier() {
                    self.binding_identifier()?;
                } else if !expression && !name_optional {
                    return Err(self.error("expected a class name"));
                }
                if self.eat_word(Word::Extends)? {
                    self.push(Frame::ClassAfterHeritage { expression });
                    self.push(Frame::Lhs { no_in: false });
                } else {
                    self.expect(Kind::LBrace, "`{`")?;
                    self.push(Frame::ClassEnd { expression });
                    self.push(Frame::ClassBody);
                }
                Ok(())
            }
            Frame::ClassAfterHeritage { expression } => {
                self.pop_plain()?;
                self.expect(Kind::LBrace, "`{`")?;
                self.push(Frame::ClassEnd { expression });
                self.push(Frame::ClassBody);
                Ok(())
            }
            Frame::ClassBody => self.class_body(),
            Frame::ClassAfterComputedKey {
                is_async,
                is_generator,
                accessor,
            } => {
                self.pop_plain()?;
                self.expect(Kind::RBracket, "`]`")?;
                self.class_member_rest(is_async, is_generator, accessor)
            }
            Frame::ClassEnd { expression } => {
                if expression {
                    self.push_value(Expr::value());
                }
                Ok(())
            }

            Frame::BindingTarget => self.binding_target(),
            Frame::BindingElement => {
                self.push(Frame::BindingAfterTarget);
                self.push(Frame::BindingTarget);
                Ok(())
            }
            Frame::BindingAfterTarget => {
                self.pop_value();
                if self.eat(Kind::Eq)? {
                    self.push(Frame::Discard);
                    self.push(Frame::Assign { no_in: false });
                }
                Ok(())
            }
            Frame::BindingArray => {
                if self.eat(Kind::RBracket)? {
                    self.push_value(Expr::open_list());
                } else if self.eat(Kind::Comma)? {
                    self.push(Frame::BindingArray);
                } else if self.eat(Kind::Ellipsis)? {
                    self.push(Frame::BindingArrayAfterRest);
                    self.push(Frame::BindingTarget);
                } else {
                    self.push(Frame::BindingArrayAfterElem);
                    self.push(Frame::BindingElement);
                }
                Ok(())
            }
            Frame::BindingArrayAfterElem => {
                if self.eat(Kind::Comma)? {
                    self.push(Frame::BindingArray);
                } else {
                    self.expect(Kind::RBracket, "`,` or `]`")?;
                    self.push_value(Expr::open_list());
                }
                Ok(())
            }
            Frame::BindingArrayAfterRest => {
                self.pop_value();
                self.expect(Kind::RBracket, "`]` after the rest element")?;
                self.push_value(Expr::open_list());
                Ok(())
            }
            Frame::BindingObject => self.binding_object(),
            Frame::BindingObjectAfterComputedKey => {
                self.pop_plain()?;
                self.expect(Kind::RBracket, "`]`")?;
                self.expect(Kind::Colon, "`:`")?;
                self.push(Frame::BindingObjectAfterProp);
                self.push(Frame::BindingElement);
                Ok(())
            }
            Frame::BindingObjectAfterProp => {
                if self.eat(Kind::Comma)? {
                    self.push(Frame::BindingObject);
                } else {
                    self.expect(Kind::RBrace, "`,` or `}`")?;
                    self.push_value(Expr::open_list());
                }
                Ok(())
            }

            Frame::ExprSeq { no_in } => {
                self.push(Frame::SeqNext { no_in, count: 1 });
                self.assign(no_in)
            }
            Frame::SeqNext { no_in, count } => {
                if self.is(Kind::Comma) {
                    self.pop_plain()?;
                    self.advance()?;
                    self.push(Frame::SeqNext {
                        no_in,
                        count: count + 1,
                    });
                    self.push(Frame::Assign { no_in });
                } else if count > 1 {
                    self.pop_plain()?;
                    self.push_value(Expr::value());
                }
                Ok(())
            }
            Frame::Assign { no_in } => self.assign(no_in),
            Frame::AssignTail { no_in } => self.assign_tail(no_in),
            Frame::AssignRhs { lhs } => {
                self.pop_plain()?;
                self.push_value(Expr {
                    pattern: lhs.could_be_assignment_target(),
                    binding: lhs.could_be_binding(),
                    ..Expr::value()
                });
                Ok(())
            }
            Frame::CondThen { no_in } => {
                self.pop_plain()?;
                self.expect(Kind::Colon, "`:`")?;
                self.push(Frame::CondEnd);
                self.push(Frame::Assign { no_in });
                Ok(())
            }
            Frame::CondEnd => {
                self.pop_plain()?;
                self.push_value(Expr::value());
                Ok(())
            }
            Frame::Binary { min, no_in } => self.binary(min, no_in),
            Frame::BinaryLoop { min, no_in } => self.binary_loop(min, no_in),
            Frame::BinaryCombine { logical } => {
                let right = self.pop_plain()?;
                if logical != Logical::None
                    && right.logical != Logical::None
                    && right.logical != logical
                {
                    return Err(
                        self.error("`??` cannot be mixed with `||` or `&&` without parentheses")
                    );
                }
                self.push_value(Expr {
                    logical,
                    ..Expr::value()
                });
                Ok(())
            }
            Frame::Unary { no_in } => self.unary(no_in),
            Frame::UnaryCombine { op, delete } => {
                let operand = self.pop_plain()?;
                if matches!(op, Kind::Plus2 | Kind::Minus2) && !operand.assignable {
                    return Err(self.error("the operand of `++` or `--` must be assignable"));
                }
                if delete && operand.bare_ident {
                    return Err(self.error("a variable cannot be deleted in strict mode"));
                }
                self.push_value(Expr {
                    unary: true,
                    ..Expr::value()
                });
                Ok(())
            }
            Frame::Postfix => {
                let operand = self.pop_value();
                if matches!(self.tok.kind, Kind::Plus2 | Kind::Minus2) && !self.tok.newline_before {
                    if !operand.assignable {
                        return Err(self.error("the operand of `++` or `--` must be assignable"));
                    }
                    self.advance()?;
                    self.push_value(Expr::value());
                } else {
                    self.push_value(operand);
                }
                Ok(())
            }
            Frame::Lhs { no_in } => self.lhs(no_in),
            Frame::NewAfterCallee { no_in } => {
                self.pop_plain()?;
                if self.is(Kind::LParen) {
                    self.advance()?;
                    self.push(Frame::Chain {
                        allow_call: true,
                        no_in,
                    });
                    self.push(Frame::AfterArgs {
                        candidate: false,
                        optional: false,
                        no_in,
                    });
                    self.push(Frame::Args {
                        no_in,
                        summary: Expr::open_list(),
                        rest_open: false,
                    });
                } else {
                    self.push_value(Expr::value());
                    self.push(Frame::Chain {
                        allow_call: true,
                        no_in,
                    });
                }
                Ok(())
            }
            Frame::Chain { allow_call, no_in } => self.chain(allow_call, no_in),
            Frame::AfterComputed { optional } => {
                self.pop_plain()?;
                self.expect(Kind::RBracket, "`]`")?;
                self.push_value(Expr::member(optional));
                Ok(())
            }
            Frame::Args {
                no_in,
                summary,
                rest_open,
            } => self.args(no_in, summary, rest_open),
            Frame::ArgsAfterOne {
                no_in,
                summary,
                spread,
            } => self.args_after_one(no_in, summary, spread),
            Frame::AfterArgs {
                candidate,
                optional,
                no_in,
            } => {
                let summary = self.pop_value();
                if candidate && self.is(Kind::Arrow) && !self.tok.newline_before {
                    if !summary.binding {
                        return Err(self.error("invalid parameters for an arrow function"));
                    }
                    self.advance()?;
                    self.push_scope(true, false);
                    self.push(Frame::ArrowBody { no_in });
                } else {
                    if summary.cover_init {
                        return Err(self
                            .error("a shorthand property initializer is only valid in a pattern"));
                    }
                    self.push_value(Expr {
                        optional,
                        ..Expr::value()
                    });
                }
                Ok(())
            }
            Frame::CoverEnd { no_in } => {
                let summary = self.pop_value();
                if self.is(Kind::Arrow) && !self.tok.newline_before {
                    if !summary.binding {
                        return Err(self.error("invalid parameters for an arrow function"));
                    }
                    self.advance()?;
                    self.push_scope(false, false);
                    self.push(Frame::ArrowBody { no_in });
                } else {
                    if summary.cover_init {
                        return Err(self
                            .error("a shorthand property initializer is only valid in a pattern"));
                    }
                    if summary.count == 0 || summary.optional {
                        // `optional` stands for a rest element here: neither
                        // an empty list nor one ending in `...x` is an
                        // expression.
                        return Err(self.error("expected an expression"));
                    }
                    self.push_value(Expr {
                        assignable: summary.assignable,
                        ..Expr::value()
                    });
                }
                Ok(())
            }
            Frame::Primary { no_in } => self.primary(no_in),
            Frame::AfterImportCall => {
                self.pop_plain()?;
                if self.eat(Kind::Comma)? && !self.is(Kind::RParen) {
                    self.push(Frame::AfterImportCallSecond);
                    self.push(Frame::Assign { no_in: false });
                    return Ok(());
                }
                self.expect(Kind::RParen, "`)`")?;
                self.push_value(Expr::value());
                Ok(())
            }
            Frame::AfterImportCallSecond => {
                self.pop_plain()?;
                self.eat(Kind::Comma)?;
                self.expect(Kind::RParen, "`)`")?;
                self.push_value(Expr::value());
                Ok(())
            }
            Frame::Template => {
                if self.eat(Kind::TemplateNoSubstitution)? {
                    self.push_value(Expr::value());
                } else {
                    self.expect(Kind::TemplateHead, "a template")?;
                    self.push(Frame::TemplateAfterSub);
                    self.push(Frame::ExprSeq { no_in: false });
                }
                Ok(())
            }
            Frame::TemplateAfterSub => {
                self.pop_plain()?;
                if !self.is(Kind::RBrace) {
                    return Err(self.error("expected `}` to close the template substitution"));
                }
                debug_assert!(self.peeked.is_none());
                self.tok = self.lexer.template_continuation_at(self.tok.start)?;
                if self.eat(Kind::TemplateMiddle)? {
                    self.push(Frame::TemplateAfterSub);
                    self.push(Frame::ExprSeq { no_in: false });
                } else {
                    self.expect(Kind::TemplateTail, "the end of the template")?;
                    self.push_value(Expr::value());
                }
                Ok(())
            }
            Frame::ArrayElems { summary } => self.array_elems(summary),
            Frame::ArrayAfterElem { summary, spread } => self.array_after_elem(summary, spread),
            Frame::ObjectProps { summary } => self.object_props(summary),
            Frame::ObjectAfterComputedKey {
                summary,
                is_async,
                is_generator,
            } => {
                self.pop_plain()?;
                self.expect(Kind::RBracket, "`]`")?;
                self.object_after_key(summary, is_async, is_generator, false, false)
            }
            Frame::ObjectAfterValue {
                summary,
                spread,
                cover,
            } => self.object_after_value(summary, spread, cover),
            Frame::ObjectAfterMethod { summary } => self.object_next(summary),
        }
    }

    // ── Module and statements ────────────────────────────────────────────

    fn module_body(&mut self) -> Result<(), Error> {
        if self.is(Kind::Eof) {
            return Ok(());
        }
        self.push(Frame::ModuleBody);
        if self.is_word(Word::Import) && !matches!(self.peek()?.kind, Kind::LParen | Kind::Dot) {
            self.items.push(Item::Import);
            return self.import_declaration();
        }
        if self.is_word(Word::Export) {
            return self.export_declaration();
        }
        self.items.push(Item::Statement);
        self.push(Frame::Statement);
        Ok(())
    }

    fn statement_list(&mut self, stop: Stop) -> Result<(), Error> {
        let done = match stop {
            Stop::Brace => self.is(Kind::RBrace),
            Stop::Case => {
                self.is(Kind::RBrace) || self.is_word(Word::Case) || self.is_word(Word::Default)
            }
        };
        if done {
            return Ok(());
        }
        if self.is(Kind::Eof) {
            return Err(self.error("unexpected end of input"));
        }
        self.push(Frame::StatementList(stop));
        self.push(Frame::Statement);
        Ok(())
    }

    fn statement(&mut self) -> Result<(), Error> {
        match self.tok.kind {
            Kind::LBrace => {
                self.advance()?;
                self.push(Frame::ExpectRBrace);
                self.push(Frame::StatementList(Stop::Brace));
                return Ok(());
            }
            Kind::Semicolon => return self.advance(),
            Kind::Ident => {}
            _ => return self.expression_statement(),
        }
        let word = self.tok.word;
        match word {
            Word::Var | Word::Let | Word::Const => {
                let kind = match self.tok.word {
                    Word::Var => DeclKind::Var,
                    Word::Let => DeclKind::Let,
                    _ => DeclKind::Const,
                };
                self.advance()?;
                self.push(Frame::Semicolon);
                self.push(Frame::VarDecls {
                    kind,
                    no_in: false,
                    in_for: false,
                    for_await: false,
                    first: true,
                });
            }
            Word::Function => {
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: false,
                    name_optional: false,
                });
            }
            Word::Async if self.peek_word()? == Word::Function && !self.peek()?.newline_before => {
                self.advance()?;
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: true,
                    name_optional: false,
                });
            }
            Word::Class => {
                self.advance()?;
                self.push(Frame::ClassRest {
                    expression: false,
                    name_optional: false,
                });
            }
            Word::If => {
                self.advance()?;
                self.expect(Kind::LParen, "`(`")?;
                self.push(Frame::AfterIfCond);
                self.push(Frame::ExprSeq { no_in: false });
            }
            Word::While => {
                self.advance()?;
                self.expect(Kind::LParen, "`(`")?;
                self.push(Frame::AfterWhileCond);
                self.push(Frame::ExprSeq { no_in: false });
            }
            Word::Do => {
                self.advance()?;
                self.push(Frame::AfterDoBody);
                self.push(Frame::Substatement);
            }
            Word::For => self.for_statement()?,
            Word::Return => {
                if !self.scope().is_function {
                    return Err(self.error("`return` is only valid inside a function"));
                }
                self.advance()?;
                self.push(Frame::Semicolon);
                if !self.statement_ends_here() {
                    self.push(Frame::Discard);
                    self.push(Frame::ExprSeq { no_in: false });
                }
            }
            Word::Throw => {
                self.advance()?;
                if self.tok.newline_before {
                    return Err(self.error("no line break is allowed after `throw`"));
                }
                self.push(Frame::Semicolon);
                self.push(Frame::Discard);
                self.push(Frame::ExprSeq { no_in: false });
            }
            Word::Break | Word::Continue => {
                self.advance()?;
                if self.is_identifier() && !self.tok.newline_before {
                    self.advance()?;
                }
                self.push(Frame::Semicolon);
            }
            Word::Try => {
                self.advance()?;
                self.push(Frame::AfterTryBlock);
                self.push(Frame::Block);
            }
            Word::Switch => {
                self.advance()?;
                self.expect(Kind::LParen, "`(`")?;
                self.push(Frame::AfterSwitchDisc);
                self.push(Frame::ExprSeq { no_in: false });
            }
            Word::Debugger => {
                self.advance()?;
                self.push(Frame::Semicolon);
            }
            Word::With => return Err(self.error("`with` is not allowed in strict mode")),
            Word::Import if !matches!(self.peek()?.kind, Kind::LParen | Kind::Dot) => {
                return Err(self.error("an import declaration must be at the top level"));
            }
            Word::Export => {
                return Err(self.error("an export declaration must be at the top level"));
            }
            _ if self.is_identifier() && self.peek()?.kind == Kind::Colon => {
                self.advance()?;
                self.advance()?;
                self.push(Frame::Substatement);
            }
            _ => return self.expression_statement(),
        }
        Ok(())
    }

    fn expression_statement(&mut self) -> Result<(), Error> {
        self.push(Frame::Semicolon);
        self.push(Frame::Discard);
        self.push(Frame::ExprSeq { no_in: false });
        Ok(())
    }

    /// Whether the statement being read ends before the current token, for
    /// the restricted productions that take an optional operand.
    fn statement_ends_here(&self) -> bool {
        matches!(self.tok.kind, Kind::Semicolon | Kind::RBrace | Kind::Eof)
            || self.tok.newline_before
    }

    fn semicolon(&mut self) -> Result<(), Error> {
        if self.eat(Kind::Semicolon)? || self.statement_ends_here() {
            Ok(())
        } else {
            Err(self.error("expected `;`"))
        }
    }

    fn for_statement(&mut self) -> Result<(), Error> {
        self.advance()?;
        let mut for_await = false;
        if self.is_word(Word::Await) {
            if !self.scope().is_async {
                return Err(self.error("`for await` is only valid in an async function"));
            }
            self.advance()?;
            for_await = true;
        }
        self.expect(Kind::LParen, "`(`")?;
        if self.is(Kind::Semicolon) {
            return self.for_classic_head(for_await);
        }
        let kind = match self.tok.word {
            Word::Var if self.is(Kind::Ident) => Some(DeclKind::Var),
            Word::Let if self.is(Kind::Ident) => Some(DeclKind::Let),
            Word::Const if self.is(Kind::Ident) => Some(DeclKind::Const),
            _ => None,
        };
        if let Some(kind) = kind {
            self.advance()?;
            self.push(Frame::VarDecls {
                kind,
                no_in: true,
                in_for: true,
                for_await,
                first: true,
            });
        } else {
            self.push(Frame::ForAfterExprInit { for_await });
            self.push(Frame::ExprSeq { no_in: true });
        }
        Ok(())
    }

    /// The `;` ending the initializer of a three-part `for` head, which is
    /// what a `for await` can never have.
    fn for_classic_head(&mut self, for_await: bool) -> Result<(), Error> {
        if for_await {
            return Err(self.error("`for await` must iterate with `of`"));
        }
        self.expect(Kind::Semicolon, "`;`")?;
        self.push(Frame::ForTest);
        Ok(())
    }

    /// The right side of `for (target in|of …)`, with the keyword current.
    fn for_in_of_rhs(&mut self, for_await: bool) -> Result<(), Error> {
        let of = self.is_word(Word::Of);
        if for_await && !of {
            return Err(self.error("`for await` must iterate with `of`"));
        }
        self.advance()?;
        self.push(Frame::ForInOfBody);
        if of {
            self.push(Frame::Assign { no_in: false });
        } else {
            self.push(Frame::ExprSeq { no_in: false });
        }
        Ok(())
    }

    fn var_next(
        &mut self,
        kind: DeclKind,
        no_in: bool,
        in_for: bool,
        for_await: bool,
    ) -> Result<(), Error> {
        if self.eat(Kind::Comma)? {
            self.push(Frame::VarDecls {
                kind,
                no_in,
                in_for,
                for_await,
                first: false,
            });
        } else if in_for {
            return self.for_classic_head(for_await);
        }
        Ok(())
    }

    fn import_declaration(&mut self) -> Result<(), Error> {
        self.advance()?;
        if self.is(Kind::String) {
            self.advance()?;
            self.import_attributes()?;
            self.push(Frame::Semicolon);
            return Ok(());
        }
        let mut need_from = false;
        if self.is_identifier() {
            self.binding_identifier()?;
            need_from = true;
            if !self.eat(Kind::Comma)? {
                self.expect_word(Word::From, "from")?;
                self.expect(Kind::String, "a module name")?;
                self.import_attributes()?;
                self.push(Frame::Semicolon);
                return Ok(());
            }
        }
        if self.eat(Kind::Star)? {
            self.expect_word(Word::As, "as")?;
            self.binding_identifier()?;
        } else if self.eat(Kind::LBrace)? {
            loop {
                if self.eat(Kind::RBrace)? {
                    break;
                }
                if self.is(Kind::String) {
                    self.advance()?;
                    self.expect_word(Word::As, "as")?;
                    self.binding_identifier()?;
                } else if self.is(Kind::Ident) && self.peek_word()? == Word::As {
                    self.advance()?;
                    self.advance()?;
                    self.binding_identifier()?;
                } else {
                    self.binding_identifier()?;
                }
                if !self.eat(Kind::Comma)? {
                    self.expect(Kind::RBrace, "`,` or `}`")?;
                    break;
                }
            }
        } else if need_from {
            return Err(self.error("expected `*` or `{` after the default import"));
        } else {
            return Err(self.error("expected an import binding or a module name"));
        }
        self.expect_word(Word::From, "from")?;
        self.expect(Kind::String, "a module name")?;
        self.import_attributes()?;
        self.push(Frame::Semicolon);
        Ok(())
    }

    /// The optional `with { type: "json" }` after a module name.
    fn import_attributes(&mut self) -> Result<(), Error> {
        if !self.is_word(Word::With) || self.tok.newline_before {
            return Ok(());
        }
        self.advance()?;
        self.expect(Kind::LBrace, "`{`")?;
        loop {
            if self.eat(Kind::RBrace)? {
                return Ok(());
            }
            if !matches!(self.tok.kind, Kind::Ident | Kind::String) {
                return Err(self.error("expected an attribute name"));
            }
            self.advance()?;
            self.expect(Kind::Colon, "`:`")?;
            self.expect(Kind::String, "an attribute value")?;
            if !self.eat(Kind::Comma)? {
                return self.expect(Kind::RBrace, "`,` or `}`");
            }
        }
    }

    fn export_declaration(&mut self) -> Result<(), Error> {
        self.advance()?;
        if self.eat_word(Word::Default)? {
            self.items.push(Item::ExportDefault);
            if self.is_word(Word::Function) {
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: false,
                    name_optional: true,
                });
            } else if self.is_word(Word::Async)
                && self.peek_word()? == Word::Function
                && !self.peek()?.newline_before
            {
                self.advance()?;
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: true,
                    name_optional: true,
                });
            } else if self.is_word(Word::Class) {
                self.advance()?;
                self.push(Frame::ClassRest {
                    expression: false,
                    name_optional: true,
                });
            } else {
                self.push(Frame::Semicolon);
                self.push(Frame::Discard);
                self.push(Frame::Assign { no_in: false });
            }
            return Ok(());
        }
        self.items.push(Item::Export);
        if self.eat(Kind::Star)? {
            if self.eat_word(Word::As)? {
                self.export_name()?;
            }
            self.expect_word(Word::From, "from")?;
            self.expect(Kind::String, "a module name")?;
            self.import_attributes()?;
            self.push(Frame::Semicolon);
            return Ok(());
        }
        if self.eat(Kind::LBrace)? {
            loop {
                if self.eat(Kind::RBrace)? {
                    break;
                }
                self.export_name()?;
                if self.eat_word(Word::As)? {
                    self.export_name()?;
                }
                if !self.eat(Kind::Comma)? {
                    self.expect(Kind::RBrace, "`,` or `}`")?;
                    break;
                }
            }
            if self.eat_word(Word::From)? {
                self.expect(Kind::String, "a module name")?;
                self.import_attributes()?;
            }
            self.push(Frame::Semicolon);
            return Ok(());
        }
        let word = self.tok.word;
        match word {
            Word::Var | Word::Let | Word::Const if self.is(Kind::Ident) => {
                let kind = match self.tok.word {
                    Word::Var => DeclKind::Var,
                    Word::Let => DeclKind::Let,
                    _ => DeclKind::Const,
                };
                self.advance()?;
                self.push(Frame::Semicolon);
                self.push(Frame::VarDecls {
                    kind,
                    no_in: false,
                    in_for: false,
                    for_await: false,
                    first: true,
                });
                Ok(())
            }
            Word::Function if self.is(Kind::Ident) => {
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: false,
                    name_optional: false,
                });
                Ok(())
            }
            Word::Async
                if self.is(Kind::Ident)
                    && self.peek_word()? == Word::Function
                    && !self.peek()?.newline_before =>
            {
                self.advance()?;
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: false,
                    is_async: true,
                    name_optional: false,
                });
                Ok(())
            }
            Word::Class if self.is(Kind::Ident) => {
                self.advance()?;
                self.push(Frame::ClassRest {
                    expression: false,
                    name_optional: false,
                });
                Ok(())
            }
            _ => Err(self.error("expected a declaration or `{` after `export`")),
        }
    }

    /// A name in an export list: any word, or a string.
    fn export_name(&mut self) -> Result<(), Error> {
        if matches!(self.tok.kind, Kind::Ident | Kind::String) {
            self.advance()
        } else {
            Err(self.error("expected an export name"))
        }
    }

    // ── Classes ──────────────────────────────────────────────────────────

    fn class_body(&mut self) -> Result<(), Error> {
        if self.eat(Kind::RBrace)? {
            return Ok(());
        }
        if self.eat(Kind::Semicolon)? {
            self.push(Frame::ClassBody);
            return Ok(());
        }
        // A modifier is only a modifier when a member name follows it;
        // `static`, `get` and `async` are otherwise ordinary member names.
        if self.is_word(Word::Static) && self.modifier_applies()? {
            self.advance()?;
            if self.eat(Kind::LBrace)? {
                self.push(Frame::ClassBody);
                self.push(Frame::ExpectRBrace);
                self.push(Frame::StatementList(Stop::Brace));
                return Ok(());
            }
        }
        let mut is_async = false;
        let mut is_generator = false;
        let mut accessor = false;
        if self.is_word(Word::Async) && self.modifier_applies()? && !self.peek()?.newline_before {
            self.advance()?;
            is_async = true;
        }
        if self.eat(Kind::Star)? {
            is_generator = true;
        }
        if !is_async
            && !is_generator
            && (self.is_word(Word::Get) || self.is_word(Word::Set))
            && self.modifier_applies()?
        {
            self.advance()?;
            accessor = true;
        }
        match self.tok.kind {
            Kind::Ident | Kind::String | Kind::Number | Kind::PrivateName => {
                self.advance()?;
                self.class_member_rest(is_async, is_generator, accessor)
            }
            Kind::LBracket => {
                self.advance()?;
                self.push(Frame::ClassAfterComputedKey {
                    is_async,
                    is_generator,
                    accessor,
                });
                self.push(Frame::Assign { no_in: false });
                Ok(())
            }
            _ => Err(self.error("expected a class member")),
        }
    }

    /// Whether the word that is current is a modifier of the member after it
    /// rather than a member name itself.
    fn modifier_applies(&mut self) -> Result<bool, Error> {
        Ok(!matches!(
            self.peek()?.kind,
            Kind::LParen | Kind::Eq | Kind::Semicolon | Kind::RBrace
        ))
    }

    /// After a member's name: a method, a field with or without an
    /// initializer.
    fn class_member_rest(
        &mut self,
        is_async: bool,
        is_generator: bool,
        accessor: bool,
    ) -> Result<(), Error> {
        self.push(Frame::ClassBody);
        if self.is(Kind::LParen) {
            self.push(Frame::MethodRest {
                is_async,
                is_generator,
            });
            return Ok(());
        }
        if is_async || is_generator || accessor {
            return Err(self.error("expected `(` to start the method"));
        }
        self.push(Frame::Semicolon);
        if self.eat(Kind::Eq)? {
            self.push(Frame::Discard);
            self.push(Frame::Assign { no_in: false });
        }
        Ok(())
    }

    // ── Bindings ─────────────────────────────────────────────────────────

    fn binding_target(&mut self) -> Result<(), Error> {
        match self.tok.kind {
            Kind::LBracket => {
                self.advance()?;
                self.push(Frame::BindingArray);
                Ok(())
            }
            Kind::LBrace => {
                self.advance()?;
                self.push(Frame::BindingObject);
                Ok(())
            }
            _ => {
                self.binding_identifier()?;
                self.push_value(Expr::ident());
                Ok(())
            }
        }
    }

    fn binding_object(&mut self) -> Result<(), Error> {
        if self.eat(Kind::RBrace)? {
            self.push_value(Expr::open_list());
            return Ok(());
        }
        if self.eat(Kind::Ellipsis)? {
            self.binding_identifier()?;
            self.expect(Kind::RBrace, "`}` after the rest property")?;
            self.push_value(Expr::open_list());
            return Ok(());
        }
        match self.tok.kind {
            Kind::LBracket => {
                self.advance()?;
                self.push(Frame::BindingObjectAfterComputedKey);
                self.push(Frame::Assign { no_in: false });
                Ok(())
            }
            Kind::String | Kind::Number => {
                self.advance()?;
                self.expect(Kind::Colon, "`:`")?;
                self.push(Frame::BindingObjectAfterProp);
                self.push(Frame::BindingElement);
                Ok(())
            }
            Kind::Ident => {
                let shorthand_ok = self.is_identifier() && self.tok.word != Word::Unbindable;
                self.advance()?;
                self.push(Frame::BindingObjectAfterProp);
                if self.eat(Kind::Colon)? {
                    self.push(Frame::BindingElement);
                } else if !shorthand_ok {
                    return Err(self.error("expected `:` after the property name"));
                } else if self.eat(Kind::Eq)? {
                    self.push(Frame::Discard);
                    self.push(Frame::Assign { no_in: false });
                }
                Ok(())
            }
            _ => Err(self.error("expected a property in the pattern")),
        }
    }

    // ── Expressions ──────────────────────────────────────────────────────

    fn assign(&mut self, no_in: bool) -> Result<(), Error> {
        if self.is_word(Word::Yield) && self.scope().is_generator {
            self.advance()?;
            let operand_absent = self.tok.newline_before
                || matches!(
                    self.tok.kind,
                    Kind::RParen
                        | Kind::RBracket
                        | Kind::RBrace
                        | Kind::Comma
                        | Kind::Semicolon
                        | Kind::Colon
                        | Kind::Eof
                )
                || (no_in && self.is_word(Word::In));
            if operand_absent {
                self.push_value(Expr::value());
                return Ok(());
            }
            self.eat(Kind::Star)?;
            self.push(Frame::CondEnd);
            self.push(Frame::Assign { no_in });
            return Ok(());
        }
        self.push(Frame::AssignTail { no_in });
        self.binary(0, no_in)
    }

    /// A binary expression: an operand, then whatever operators of at least
    /// `min` precedence follow it. The steps of the chain that always follow
    /// one another are direct calls rather than frames; none of them
    /// re-enters the main loop, so the native stack stays shallow.
    fn binary(&mut self, min: u8, no_in: bool) -> Result<(), Error> {
        self.push(Frame::BinaryLoop { min, no_in });
        self.unary(no_in)
    }

    fn assign_tail(&mut self, no_in: bool) -> Result<(), Error> {
        let lhs = self.pop_value();
        if lhs.arrow {
            self.push_value(lhs);
            return Ok(());
        }
        if self.is(Kind::Question) {
            if lhs.cover_init {
                return Err(
                    self.error("a shorthand property initializer is only valid in a pattern")
                );
            }
            self.advance()?;
            self.push(Frame::CondThen { no_in });
            self.push(Frame::Assign { no_in: false });
            return Ok(());
        }
        if self.tok.kind.is_assignment() {
            let valid = if self.is(Kind::Eq) {
                lhs.could_be_assignment_target()
            } else {
                lhs.assignable
            };
            if !valid {
                return Err(self.error("the left side of an assignment must be assignable"));
            }
            self.advance()?;
            self.push(Frame::AssignRhs { lhs });
            self.push(Frame::Assign { no_in });
            return Ok(());
        }
        self.push_value(lhs);
        Ok(())
    }

    /// The binary operator the current token is, with its precedence and
    /// whether it associates to the right.
    fn binary_operator(&self, no_in: bool) -> Option<(u8, bool, Logical)> {
        let (prec, right, logical) = match self.tok.kind {
            Kind::Question2 => (PREC_NULLISH, false, Logical::Nullish),
            Kind::Pipe2 => (PREC_OR, false, Logical::AndOr),
            Kind::Amp2 => (PREC_AND, false, Logical::AndOr),
            Kind::Pipe => (4, false, Logical::None),
            Kind::Caret => (5, false, Logical::None),
            Kind::Amp => (6, false, Logical::None),
            Kind::Eq2 | Kind::Neq | Kind::Eq3 | Kind::Neq2 => (7, false, Logical::None),
            Kind::Lt | Kind::Gt | Kind::LtEq | Kind::GtEq => (8, false, Logical::None),
            Kind::Ident if self.tok.word == Word::Instanceof => (8, false, Logical::None),
            Kind::Ident if self.tok.word == Word::In && !no_in => (8, false, Logical::None),
            Kind::ShiftLeft | Kind::ShiftRight | Kind::ShiftRight3 => (9, false, Logical::None),
            Kind::Plus | Kind::Minus => (10, false, Logical::None),
            Kind::Star | Kind::Slash | Kind::Percent => (11, false, Logical::None),
            Kind::Star2 => (PREC_EXPONENT, true, Logical::None),
            _ => return None,
        };
        Some((prec, right, logical))
    }

    fn binary_loop(&mut self, min: u8, no_in: bool) -> Result<(), Error> {
        let left = self.pop_value();
        let Some((prec, right, logical)) = self.binary_operator(no_in) else {
            self.push_value(left);
            return Ok(());
        };
        if left.arrow || prec < min {
            self.push_value(left);
            return Ok(());
        }
        if left.cover_init {
            return Err(self.error("a shorthand property initializer is only valid in a pattern"));
        }
        if prec == PREC_EXPONENT && left.unary {
            return Err(self.error("a unary expression must be parenthesized as the base of `**`"));
        }
        if logical != Logical::None && left.logical != Logical::None && left.logical != logical {
            return Err(self.error("`??` cannot be mixed with `||` or `&&` without parentheses"));
        }
        self.advance()?;
        self.push(Frame::BinaryLoop { min, no_in });
        self.push(Frame::BinaryCombine { logical });
        self.push(Frame::Binary {
            min: if right { prec } else { prec + 1 },
            no_in,
        });
        Ok(())
    }

    fn unary(&mut self, no_in: bool) -> Result<(), Error> {
        let op = match self.tok.kind {
            Kind::Bang | Kind::Tilde | Kind::Plus | Kind::Minus | Kind::Plus2 | Kind::Minus2 => {
                Some((self.tok.kind, false))
            }
            Kind::Ident => match self.tok.word {
                Word::Typeof | Word::Void => Some((Kind::Ident, false)),
                Word::Delete => Some((Kind::Ident, true)),
                Word::Await => {
                    if !self.scope().is_async {
                        return Err(self.error("`await` is only valid in an async function"));
                    }
                    Some((Kind::Ident, false))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((op, delete)) = op {
            self.advance()?;
            self.push(Frame::UnaryCombine { op, delete });
            self.push(Frame::Unary { no_in });
            return Ok(());
        }
        self.push(Frame::Postfix);
        self.lhs(no_in)
    }

    fn lhs(&mut self, no_in: bool) -> Result<(), Error> {
        if self.is_word(Word::New) {
            self.advance()?;
            if self.eat(Kind::Dot)? {
                self.expect_word(Word::Target, "target")?;
                self.push_value(Expr::value());
                self.push(Frame::Chain {
                    allow_call: true,
                    no_in,
                });
                return Ok(());
            }
            self.push(Frame::NewAfterCallee { no_in });
            self.push(Frame::Chain {
                allow_call: false,
                no_in,
            });
            self.push(Frame::Primary { no_in });
            return Ok(());
        }
        self.push(Frame::Chain {
            allow_call: true,
            no_in,
        });
        self.primary(no_in)
    }

    fn chain(&mut self, allow_call: bool, no_in: bool) -> Result<(), Error> {
        let object = self.pop_value();
        if object.arrow {
            self.push_value(object);
            return Ok(());
        }
        let again = Frame::Chain { allow_call, no_in };
        match self.tok.kind {
            Kind::Dot => {
                self.advance()?;
                if !matches!(self.tok.kind, Kind::Ident | Kind::PrivateName) {
                    return Err(self.error("expected a property name"));
                }
                self.advance()?;
                self.push_value(Expr::member(object.optional));
                self.push(again);
            }
            Kind::QuestionDot => {
                if !allow_call {
                    return Err(self.error("an optional chain cannot be constructed with `new`"));
                }
                self.advance()?;
                match self.tok.kind {
                    Kind::LParen => {
                        self.advance()?;
                        self.push(again);
                        self.push(Frame::AfterArgs {
                            candidate: false,
                            optional: true,
                            no_in,
                        });
                        self.push(Frame::Args {
                            no_in,
                            summary: Expr::open_list(),
                            rest_open: false,
                        });
                    }
                    Kind::LBracket => {
                        self.advance()?;
                        self.push(again);
                        self.push(Frame::AfterComputed { optional: true });
                        self.push(Frame::ExprSeq { no_in: false });
                    }
                    Kind::Ident | Kind::PrivateName => {
                        self.advance()?;
                        self.push_value(Expr::member(true));
                        self.push(again);
                    }
                    _ => return Err(self.error("expected a property name after `?.`")),
                }
            }
            Kind::LBracket => {
                self.advance()?;
                self.push(again);
                self.push(Frame::AfterComputed {
                    optional: object.optional,
                });
                self.push(Frame::ExprSeq { no_in: false });
            }
            Kind::LParen if allow_call => {
                let candidate = object.ident_async && !self.tok.newline_before;
                self.advance()?;
                self.push(again);
                self.push(Frame::AfterArgs {
                    candidate,
                    optional: object.optional,
                    no_in,
                });
                self.push(Frame::Args {
                    no_in,
                    summary: Expr::open_list(),
                    rest_open: false,
                });
            }
            Kind::TemplateNoSubstitution | Kind::TemplateHead => {
                if object.optional {
                    return Err(self.error("a tagged template cannot follow an optional chain"));
                }
                self.push(again);
                self.push(Frame::Template);
            }
            _ => self.push_value(object),
        }
        Ok(())
    }

    /// The elements of an argument list or a parenthesized list, whose
    /// summary says whether the whole could instead be arrow parameters.
    fn args(&mut self, no_in: bool, summary: Expr, rest_open: bool) -> Result<(), Error> {
        // Anything after a rest element, a trailing comma included, is fine
        // in a call and impossible in arrow parameters.
        let summary = Expr {
            binding: summary.binding && !rest_open,
            ..summary
        };
        if self.eat(Kind::RParen)? {
            self.push_value(summary);
            return Ok(());
        }
        self.args_continue(no_in, summary)
    }

    fn args_continue(&mut self, no_in: bool, summary: Expr) -> Result<(), Error> {
        let spread = self.eat(Kind::Ellipsis)?;
        self.push(Frame::ArgsAfterOne {
            no_in,
            summary,
            spread,
        });
        self.push(Frame::Assign { no_in: false });
        Ok(())
    }

    fn args_after_one(
        &mut self,
        no_in: bool,
        mut summary: Expr,
        spread: bool,
    ) -> Result<(), Error> {
        let element = self.pop_value();
        summary.count += 1;
        summary.cover_init |= element.cover_init;
        summary.binding &= element.could_be_binding();
        summary.pattern &= element.could_be_assignment_target();
        if spread {
            // Recorded as `optional`, which a list has no other use for: a
            // rest element, which only arrow parameters may keep.
            summary.optional = true;
            summary.binding &= element.bare_ident || element.binding;
        }
        // Only a lone parenthesized element stays a target: `(a) = 1`.
        summary.assignable = summary.count == 1 && !spread && element.assignable;
        if self.eat(Kind::Comma)? {
            self.push(Frame::Args {
                no_in,
                summary,
                rest_open: spread,
            });
        } else {
            self.expect(Kind::RParen, "`,` or `)`")?;
            self.push_value(summary);
        }
        Ok(())
    }

    fn primary(&mut self, no_in: bool) -> Result<(), Error> {
        match self.tok.kind {
            Kind::Number | Kind::String | Kind::Regex => {
                self.advance()?;
                self.push_value(Expr::value());
            }
            Kind::Slash | Kind::SlashEq => {
                debug_assert!(self.peeked.is_none());
                self.tok = self
                    .lexer
                    .regex_at(self.tok.start, self.tok.newline_before)?;
                self.advance()?;
                self.push_value(Expr::value());
            }
            Kind::TemplateNoSubstitution | Kind::TemplateHead => self.push(Frame::Template),
            Kind::LParen => {
                self.advance()?;
                if self.is(Kind::RParen) {
                    self.advance()?;
                    if !self.is(Kind::Arrow) || self.tok.newline_before {
                        return Err(self.error("expected `=>` after `()`"));
                    }
                    self.advance()?;
                    self.push_scope(false, false);
                    self.push(Frame::ArrowBody { no_in });
                    return Ok(());
                }
                self.push(Frame::CoverEnd { no_in });
                self.push(Frame::Args {
                    no_in,
                    summary: Expr::open_list(),
                    rest_open: false,
                });
            }
            Kind::LBracket => {
                self.advance()?;
                self.push(Frame::ArrayElems {
                    summary: Expr::open_list(),
                });
            }
            Kind::LBrace => {
                self.advance()?;
                self.push(Frame::ObjectProps {
                    summary: Expr::open_list(),
                });
            }
            Kind::PrivateName => {
                // `#name in object` is the one place a private name stands
                // alone; the binary loop then reads the `in`.
                self.advance()?;
                if no_in || !self.is_word(Word::In) {
                    return Err(self.error("a private name is only valid before `in` here"));
                }
                self.push_value(Expr::value());
            }
            Kind::Ident => return self.primary_word(no_in),
            _ => return Err(self.error("expected an expression")),
        }
        Ok(())
    }

    fn primary_word(&mut self, no_in: bool) -> Result<(), Error> {
        let word = self.tok.word;
        match word {
            Word::This | Word::Null | Word::True | Word::False => {
                self.advance()?;
                self.push_value(Expr::value());
            }
            Word::Function => {
                self.advance()?;
                self.push(Frame::FunctionRest {
                    expression: true,
                    is_async: false,
                    name_optional: true,
                });
            }
            Word::Class => {
                self.advance()?;
                self.push(Frame::ClassRest {
                    expression: true,
                    name_optional: true,
                });
            }
            Word::New => self.push(Frame::Lhs { no_in }),
            Word::Super => {
                self.advance()?;
                if !matches!(self.tok.kind, Kind::LParen | Kind::Dot | Kind::LBracket) {
                    return Err(self.error("`super` must be called or accessed"));
                }
                self.push_value(Expr::value());
            }
            Word::Import => {
                self.advance()?;
                if self.eat(Kind::Dot)? {
                    self.expect_word(Word::Meta, "meta")?;
                    self.push_value(Expr::value());
                } else {
                    self.expect(Kind::LParen, "`(` or `.meta` after `import`")?;
                    self.push(Frame::AfterImportCall);
                    self.push(Frame::Assign { no_in: false });
                }
            }
            Word::Async => {
                let next = self.peek()?;
                let heads_function = next.word == Word::Function;
                if next.kind == Kind::Ident
                    && !next.newline_before
                    && (heads_function || !next.word.is_reserved())
                {
                    self.advance()?;
                    if heads_function {
                        self.advance()?;
                        self.push(Frame::FunctionRest {
                            expression: true,
                            is_async: true,
                            name_optional: true,
                        });
                        return Ok(());
                    }
                    // `async x => …`: the only other thing a word after
                    // `async` on the same line can begin.
                    self.binding_identifier()?;
                    if !self.is(Kind::Arrow) || self.tok.newline_before {
                        return Err(self.error("expected `=>`"));
                    }
                    self.advance()?;
                    self.push_scope(true, false);
                    self.push(Frame::ArrowBody { no_in });
                    return Ok(());
                }
                self.advance()?;
                self.push_value(Expr {
                    ident_async: true,
                    ..Expr::ident()
                });
            }
            _ => {
                if !self.is_identifier() {
                    return Err(self.error(&format!("unexpected `{}`", self.text())));
                }
                let next = self.peek()?;
                if next.kind == Kind::Arrow && !next.newline_before {
                    self.binding_identifier()?;
                    self.advance()?;
                    self.push_scope(false, false);
                    self.push(Frame::ArrowBody { no_in });
                    return Ok(());
                }
                self.advance()?;
                self.push_value(Expr::ident());
            }
        }
        Ok(())
    }

    fn array_elems(&mut self, summary: Expr) -> Result<(), Error> {
        if self.eat(Kind::RBracket)? {
            self.push_value(summary);
            return Ok(());
        }
        if self.eat(Kind::Comma)? {
            // An elision: a hole, which every pattern kind allows.
            self.push(Frame::ArrayElems { summary });
            return Ok(());
        }
        let spread = self.eat(Kind::Ellipsis)?;
        self.push(Frame::ArrayAfterElem { summary, spread });
        self.push(Frame::Assign { no_in: false });
        Ok(())
    }

    fn array_after_elem(&mut self, mut summary: Expr, spread: bool) -> Result<(), Error> {
        let element = self.pop_value();
        summary.cover_init |= element.cover_init;
        summary.pattern &= element.could_be_assignment_target();
        summary.binding &= element.could_be_binding();
        if self.eat(Kind::Comma)? {
            if spread {
                // A rest element must be last, and takes no trailing comma.
                summary.pattern = false;
                summary.binding = false;
            }
            self.push(Frame::ArrayElems { summary });
        } else {
            self.expect(Kind::RBracket, "`,` or `]`")?;
            self.push_value(summary);
        }
        Ok(())
    }

    fn object_props(&mut self, summary: Expr) -> Result<(), Error> {
        if self.eat(Kind::RBrace)? {
            self.push_value(summary);
            return Ok(());
        }
        if self.eat(Kind::Ellipsis)? {
            self.push(Frame::ObjectAfterValue {
                summary,
                spread: true,
                cover: false,
            });
            self.push(Frame::Assign { no_in: false });
            return Ok(());
        }
        let mut is_async = false;
        let mut is_generator = false;
        let mut accessor = false;
        if self.is_word(Word::Async)
            && self.property_modifier_applies()?
            && !self.peek()?.newline_before
        {
            self.advance()?;
            is_async = true;
        }
        if self.eat(Kind::Star)? {
            is_generator = true;
        }
        if !is_async
            && !is_generator
            && (self.is_word(Word::Get) || self.is_word(Word::Set))
            && self.property_modifier_applies()?
        {
            self.advance()?;
            accessor = true;
        }
        match self.tok.kind {
            Kind::Ident => {
                let shorthand_ok = self.is_identifier();
                self.advance()?;
                self.object_after_key(summary, is_async, is_generator, accessor, shorthand_ok)
            }
            Kind::String | Kind::Number => {
                self.advance()?;
                self.object_after_key(summary, is_async, is_generator, accessor, false)
            }
            Kind::LBracket => {
                self.advance()?;
                self.push(Frame::ObjectAfterComputedKey {
                    summary,
                    is_async,
                    is_generator,
                });
                self.push(Frame::Assign { no_in: false });
                Ok(())
            }
            _ => Err(self.error("expected a property")),
        }
    }

    /// Whether the word that is current modifies the property after it
    /// rather than naming one.
    fn property_modifier_applies(&mut self) -> Result<bool, Error> {
        Ok(!matches!(
            self.peek()?.kind,
            Kind::LParen | Kind::Colon | Kind::Comma | Kind::RBrace | Kind::Eq
        ))
    }

    fn object_after_key(
        &mut self,
        mut summary: Expr,
        is_async: bool,
        is_generator: bool,
        accessor: bool,
        shorthand_ok: bool,
    ) -> Result<(), Error> {
        if self.is(Kind::LParen) {
            summary.pattern = false;
            summary.binding = false;
            self.push(Frame::ObjectAfterMethod { summary });
            self.push(Frame::MethodRest {
                is_async,
                is_generator,
            });
            return Ok(());
        }
        if is_async || is_generator || accessor {
            return Err(self.error("expected `(` to start the method"));
        }
        if self.eat(Kind::Colon)? {
            self.push(Frame::ObjectAfterValue {
                summary,
                spread: false,
                cover: false,
            });
            self.push(Frame::Assign { no_in: false });
            return Ok(());
        }
        if !shorthand_ok {
            return Err(self.error("expected `:` after the property name"));
        }
        if self.eat(Kind::Eq)? {
            self.push(Frame::ObjectAfterValue {
                summary,
                spread: false,
                cover: true,
            });
            self.push(Frame::Assign { no_in: false });
            return Ok(());
        }
        self.object_next(summary)
    }

    fn object_after_value(
        &mut self,
        mut summary: Expr,
        spread: bool,
        cover: bool,
    ) -> Result<(), Error> {
        let value = self.pop_value();
        if cover {
            // `{ a = 1 }`: the initializer's value is irrelevant, and the
            // property is a fine pattern element but no expression.
            if value.cover_init {
                return Err(
                    self.error("a shorthand property initializer is only valid in a pattern")
                );
            }
            summary.cover_init = true;
        } else if spread {
            summary.pattern &= value.assignable;
            summary.binding &= value.bare_ident;
            if value.cover_init {
                return Err(
                    self.error("a shorthand property initializer is only valid in a pattern")
                );
            }
            if self.is(Kind::Comma) {
                // A rest property must be last, and takes no trailing comma.
                summary.pattern = false;
                summary.binding = false;
            }
        } else {
            summary.cover_init |= value.cover_init;
            summary.pattern &= value.could_be_assignment_target();
            summary.binding &= value.could_be_binding();
        }
        self.object_next(summary)
    }

    fn object_next(&mut self, summary: Expr) -> Result<(), Error> {
        if self.eat(Kind::Comma)? {
            self.push(Frame::ObjectProps { summary });
        } else {
            self.expect(Kind::RBrace, "`,` or `}`")?;
            self.push_value(summary);
        }
        Ok(())
    }
}
