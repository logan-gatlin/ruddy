use indexmap::IndexMap;

use crate::{
    token::{Kind, Token},
    tracking::{Span, Tracked, TrackedString},
};

pub type Stmt = Tracked<StmtKind>;

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        /// What the definition binds: a bare name, which is every definition
        /// the language had before patterns, or a pattern that takes the value
        /// apart. Which names it binds is [`ir`](crate::ir)'s to work out; the
        /// parser records what was written.
        ///
        /// Boxed for the reason [`ExprKind::Let`] boxes its ascription: a
        /// pattern is a tree of its own, and inlining one here would grow
        /// every statement to the size of the largest thing a binding can be.
        pattern: Box<Pattern>,
        /// The written type, when the definition was ascribed one, with the
        /// `where` clause that may follow it.
        ty: Option<Annotation>,
        body: Tracked<Expr>,
    },
    Type {
        name: TrackedString,
        /// The parameters the declaration binds, in the order written. Empty
        /// for the plain `type T = ...`, which is every declaration the
        /// language had before it could take any.
        ///
        /// What each one stands for — a type, or the fields a row does not
        /// name — is not written here and is not the parser's to know: it
        /// follows from where the body uses it, which is a question for
        /// lowering.
        params: Vec<TrackedString>,
        /// The body, read as an annotation is: a declaration may write neither
        /// a `when` nor a `where`, and refusing them is lowering's — see
        /// [`ir::ErrorKind::OpenDeclaredType`](crate::ir::ErrorKind), which
        /// already refuses the `..` beside them for the same reason.
        body: Annotation,
    },
}

pub type Expr = Tracked<ExprKind>;

#[derive(Debug, Clone)]
pub enum ExprKind {
    Apply {
        func: Box<Expr>,
        arg: Box<Expr>,
    },
    Function {
        args: Vec<Arg>,
        body: Box<Expr>,
    },
    /// `let <name> [: <type>] = <value> in <body>` — a name given a value for
    /// the length of one expression.
    ///
    /// The same shape as [`StmtKind::Let`] and nothing else: a statement binds
    /// a name for the whole file and this one binds it for the body written
    /// after the `in`, so the two share their grammar and not a line of their
    /// meaning.
    Let {
        /// What the binding binds — a bare name, or a pattern taking the value
        /// apart — exactly as [`StmtKind::Let`] records it.
        pattern: Pattern,
        /// The written type, when the binding was ascribed one.
        ///
        /// Boxed, where [`StmtKind::Let`]'s is not, because this node nests
        /// inside another expression and that one does not. An ascription is
        /// the largest thing either can carry, so inlining it here would make
        /// every expression in the tree the size of the one construct that has
        /// one.
        ty: Option<Box<Annotation>>,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// `match <expr> with [|] <pattern> => <expr> (| <pattern> => <expr>)* end`
    /// — dispatch on what a value is.
    ///
    /// The scrutinee is a full expression: it ends at the `with` of its own
    /// accord, because `with` begins no atom. Zero arms parse — the empty
    /// match is the empty sum's eliminator — and the leading `|` before the
    /// first arm is optional, the same convention a sum type keeps.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },
    Struct(IndexMap<TrackedString, Expr>),
    /// `` `Some 1 `` — one case of a sum, with what it carries.
    ///
    /// The payload is optional because `` `None `` is how a case that carries
    /// nothing is written, and there is nothing there to parse. What it means
    /// is unit — see [`ir::TermKind::Tag`](crate::ir::TermKind::Tag) — but
    /// nothing was written, so nothing is recorded, and a printed tree reads
    /// back as the one it was printed from.
    Tag {
        name: TrackedString,
        payload: Option<Box<Expr>>,
    },
    Project {
        base: Box<Expr>,
        field: TrackedString,
    },
    Ident {
        name: TrackedString,
    },
    Natural(u128),
    Unit,
}

/// One arm of a match: a pattern and the expression it chooses.
#[derive(Debug, Clone)]
pub struct Arm {
    pub pattern: Pattern,
    pub body: Expr,
}

/// One argument of a `fn` header: the name it binds, or the `_` that binds
/// nothing. Not a pattern — the header takes plain names and the one discard,
/// and a struct pattern written there stays the parse error it has always
/// been — so this records which of the two was written and nothing more.
pub type Arg = Tracked<ArgKind>;

#[derive(Debug, Clone)]
pub enum ArgKind {
    /// A name, bound over the body as every `fn` argument always was.
    Name(String),
    /// `_` — the argument arrives, is typechecked, and is thrown away;
    /// nothing in the body can name it.
    Wildcard,
}

pub type Pattern = Tracked<PatternKind>;

/// What a value may be taken apart as, mirroring the expression grammar's
/// shape: a name binds, a literal matches itself, `()` matches unit, a struct
/// pattern reaches into fields, and a tag pattern asks which case a sum is.
/// Grouping parentheses are discarded here exactly as [`Parser::paren_expr`]
/// discards them, so no grouping node exists to reach the IR.
#[derive(Debug, Clone)]
pub enum PatternKind {
    /// A bare name: binds the whole value, matches anything.
    Ident { name: TrackedString },
    /// `_`: matches anything and binds nothing. What a name does minus the
    /// name, so it can never collide with a binder and can never be repeated
    /// too often.
    Wildcard,
    /// A natural literal: matches exactly that number.
    Natural(u128),
    /// `()`: matches unit, binds nothing.
    Unit,
    /// `{ f, g: <pattern>, ... }` — reach into a struct's fields. A bare field
    /// name puns, binding the field to its own name, so the value is `None`;
    /// `name: <pattern>` matches the field against the sub-pattern. `{}` is
    /// allowed and binds nothing.
    Struct {
        fields: IndexMap<TrackedString, Option<Pattern>>,
        /// The `..` ending the field list, when the pattern was written open.
        /// Without it the pattern is exact — it matches only values with
        /// exactly the fields it names; with it, values with at least them.
        /// The span is the `..`'s own, for the debugger to point at.
        rest: Option<Span>,
    },
    /// `` `Name [<pattern>] `` — one case of a sum. The payload pattern is
    /// taken greedily, exactly as [`Parser::tag_expr`] takes a payload, so
    /// `` `A `B x `` is `` `A `` carrying `` (`B x) ``. Written bare, the case
    /// is constrained to carry unit and binds nothing — the convention
    /// `` `None `` follows as an expression.
    Tag {
        name: TrackedString,
        payload: Option<Box<Pattern>>,
    },
}

pub type Type = Tracked<TypeKind>;

#[derive(Debug, Clone)]
pub enum TypeKind {
    Struct {
        fields: IndexMap<TrackedString, TypeField>,
        /// The `..` ending the field list, when the struct type was written
        /// open. `None` means the type lists every field it allows.
        tail: Option<Tail>,
    },
    /// `` `Some T | `None `` — a sum type, as the cases it allows.
    ///
    /// The same shape as a struct, and the same [`Tail`]: a `..` says what is
    /// known about the cases not written out, and its absence says the type
    /// lists every case there is. Written with no brackets around it, so the
    /// empty sum — no cases at all — is spelled `|`, and the parser reaches
    /// here on a leading `|` as much as on a leading tag or the `\` of a case
    /// written absent.
    Sum {
        cases: IndexMap<TrackedString, SumCase>,
        tail: Option<Tail>,
    },
    Arrow {
        from: Box<Type>,
        to: Box<Type>,
    },
    /// `<head> <arg>...` — a type applied to arguments.
    ///
    /// Flat rather than nested one argument at a time, unlike
    /// [`ExprKind::Apply`]: a declaration takes a fixed number of arguments and
    /// is applied to all of them at once, so there is no half-applied type for
    /// an intermediate node to stand for.
    ///
    /// `head` is a whole [`Type`] rather than a name because the parser judges
    /// nothing: `{ x: Nat } Nat` parses here and is refused where the rest of
    /// the rules about what may be applied live. The same division
    /// [`projection`](Parser::projection) keeps.
    Apply {
        head: Box<Type>,
        args: Vec<Type>,
    },
    Ident {
        name: TrackedString,
    },
    /// `_` — a type position left for inference to decide.
    ///
    /// The one thing a `where let` variable is not: a hole binds nothing, may
    /// not be referred to, and needs no declaration, so writing one is saying
    /// "infer this" rather than promising anything about it. That is what buys
    /// back the brevity a declared variable costs — see
    /// [`ir::ErrorKind::HoleInDeclaration`](crate::ir::ErrorKind), which is
    /// where the one position that still refuses a hole says so.
    Hole,
    Unit,
}

/// One field of a struct type: its type and the `when` clause it may wear, or
/// the `\` that says the label is not there at all.
#[derive(Debug, Clone)]
pub enum TypeField {
    /// `name [when a]: T` — a field that is there, or — with a `when` — one
    /// whose being there is the named presence variable's to say, with this
    /// type when it is.
    Written {
        when: Option<Box<When>>,
        value: Type,
    },
    /// `\name` — the label is definitely absent: the `..` beside it may not
    /// stand for it. No type and no `when`, so this variant carries nothing; the
    /// map key's span covers the whole `\name`, which is what a diagnostic
    /// about the entry underlines.
    Absent,
}

/// One case of a sum type: what it carries and the `when` clause it may wear,
/// or the `\` that says the case is not there at all.
///
/// A `when` is not what a `..` tail says. `` `A (when a) Nat | `B `` allows `A`
/// or not and nothing else beyond `B`; `` `B | ..r `` allows anything at all
/// beyond `B`. Neither can be written as the other, which is why both are here.
///
/// `payload` is `None` for a case written bare. That means unit — the same
/// thing `()` means — but it is recorded as the nothing it was written as, so
/// that printing the tree gives back the source it was parsed from.
#[derive(Debug, Clone)]
pub enum SumCase {
    /// `` `Name [(when a)] [T] `` — a case a value may be, carrying this when
    /// it is.
    Written {
        when: Option<Box<When>>,
        payload: Option<Type>,
    },
    /// `` \`Name `` — the case is definitely absent. No payload and no `when`,
    /// exactly as [`TypeField::Absent`]: the map key's span covers the whole
    /// `` \`Name ``.
    Absent,
}

/// The `when` clause on one label of a written type: the name it binds a
/// presence variable to.
///
/// Named rather than anonymous, which is the whole of what retired the old `?`.
/// A presence a `where` clause has to be able to talk about needs a name, the
/// way a type variable needs a name, and `?` gave every one of them the same
/// nothing. `when _` is what the `?` used to be: a presence this definition
/// decides and no formula may name.
///
/// The span covers the whole clause — the `when` and the name after it — which
/// is what a complaint about the label's openness underlines.
///
/// Boxed where a label holds one, for the reason [`ExprKind::Let`] boxes its
/// ascription: a clause is the rarest thing a label can carry and among the
/// largest, so inlining one would grow every field of every written type to the
/// size of the few that have one.
#[derive(Debug, Clone)]
pub struct When {
    pub span: Span,
    /// The name, or `None` for the anonymous `when _`.
    pub name: Option<TrackedString>,
}

/// A `where` clause's formula, as written.
///
/// The surface grammar, loosest to tightest: `=` and `!=`, non-associative, at
/// the top; then `or`, then `and`, then unary `not`, then parentheses and
/// names. Nothing here is resolved — a name is the string it was written as,
/// and whether the type binds it is [`ir`](crate::ir)'s to say.
pub type Clause = Tracked<ClauseKind>;

#[derive(Debug, Clone)]
pub enum ClauseKind {
    /// A presence variable, by the name a `when` in the same annotation bound
    /// it to.
    Name(String),
    Not(Box<Clause>),
    And(Box<Clause>, Box<Clause>),
    Or(Box<Clause>, Box<Clause>),
    /// `a = b` — both there or neither.
    Equal(Box<Clause>, Box<Clause>),
    /// `a != b` — exactly one of them there.
    NotEqual(Box<Clause>, Box<Clause>),
}

/// One statement of a `where` clause. Statements are separated by `;`, may
/// appear in any order and may be interleaved; several declaration statements
/// accumulate their names, and several constraint statements are conjoined in
/// written order.
pub type ClauseStmt = Tracked<ClauseStmtKind>;

#[derive(Debug, Clone)]
pub enum ClauseStmtKind {
    /// `let a, b` — the variables this annotation declares. A declaration
    /// statement never contains an `=`, which is what makes
    /// `let id : a -> a where let a = fn x => x` need no speculation to read.
    Let(Vec<TrackedString>),
    /// The boolean expression grammar the `where` clause has always had.
    Constraint(Clause),
}

/// The `where` clause of an annotation: a `;`-separated list of statements.
///
/// A wrapper rather than a bare `Vec`, so that the whole clause has a span of
/// its own — the one an annotation merges into itself, and the one a complaint
/// about the clause as a whole points at. Each statement keeps a span too,
/// which is what lets a declaration statement written in a `type` declaration
/// be refused where it stands rather than by a complaint about everything
/// beside it.
#[derive(Debug, Clone)]
pub struct Where {
    pub span: Span,
    pub stmts: Vec<ClauseStmt>,
}

/// A written type and the `where` clause that may follow it: what a definition
/// is ascribed.
///
/// A wrapper rather than a node of [`TypeKind`], because a `where` is not a
/// type: it ends one, once, at the outside. Nesting one would let a field's
/// type carry a clause naming presences from another part of the annotation —
/// and would be a higher-rank language than this one, since a `where let`
/// variable is quantified at the annotation's outermost level and nowhere else.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub ty: Type,
    /// The `where` clause, when one was written.
    pub clause: Option<Where>,
}

/// The `..` tail of a struct type: what is said about the fields not named.
/// Anonymous (`..`) when the rest may be anything; named (`..r`) when two
/// tails in one annotation are to stand for the same rest.
#[derive(Debug, Clone)]
pub struct Tail {
    pub span: Span,
    pub name: Option<TrackedString>,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A token no production had a reading for, or input that ran out where
    /// more was needed. The parser's one complaint before the wildcard got
    /// its own.
    Unexpected,
    /// `_` written where nothing is being thrown away: an expression, a field
    /// name, a projection, a type. One meaning wherever it lands — `_` stands
    /// for a value being discarded, so it can never be *used* — with the
    /// position carried for the wording alone. See [`Place`].
    Wildcard { place: Place },
}

/// Where a stray `_` was written, carried so [`ui`](crate::ui) can word the
/// complaint for the position. The meaning may not vary with it; only the
/// phrasing does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Expression position — `let x = _`, `f _` — and every position with no
    /// better name, since a value is what `_` most nearly fails to be.
    Value,
    /// A struct's field name, in an expression or a pattern: `{ _: 1 }`.
    Field,
    /// A struct pattern's pun, `{ _ }`: a pun binds a field to its own name,
    /// and `_` is not a name.
    Pun,
    /// A projection: `x._`.
    Projection,
    /// A `type` declaration's name, one of its parameters, or a name inside a
    /// `where` clause's formula.
    ///
    /// Not a type expression any more: `_` there is
    /// [`TypeKind::Hole`], the position left for inference to decide. What is
    /// left are the three places a `_` still names nothing — a declaration
    /// binds names rather than types, and a formula is written about presences
    /// a `when` gave names to.
    Type,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub stmts: Vec<Stmt>,
    pub errors: Vec<Error>,
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<Error>,
}

impl Annotation {
    /// Where the whole annotation was written: the type, and the `where` clause
    /// after it when there is one.
    pub fn span(&self) -> Span {
        match &self.clause {
            Some(clause) => self.ty.span.merge(clause.span),
            None => self.ty.span,
        }
    }
}

pub fn parse(toks: Vec<Token>) -> Output {
    let mut p = Parser::new(toks);
    let mut stmts = Vec::new();

    while p.peek().is_some() {
        let before = p.pos;
        match p.stmt() {
            Some(stmt) => stmts.push(stmt),
            None => {
                // Guarantee forward progress before recovering so a malformed
                // leading token can't spin the loop forever.
                if p.pos == before {
                    p.advance();
                }
                p.recover();
            }
        }
    }

    Output {
        stmts,
        errors: p.errors,
    }
}

impl Parser {
    fn new(toks: Vec<Token>) -> Self {
        Self {
            toks,
            pos: 0,
            errors: Vec::new(),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let tok = self.toks.get(self.pos).cloned();
        // Clamped rather than guarded: past the end there is nothing to step
        // over, and a cursor that ran on would only have to be checked again by
        // everything that reads it.
        self.pos = (self.pos + 1).min(self.toks.len());
        tok
    }

    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.errors.push(Error { span, kind });
    }

    /// The zero-width position just past the last token, so that running out of
    /// input has somewhere to point.
    fn eof_span(&self) -> Span {
        self.toks.last().map_or_else(Span::default, |tok| {
            tok.span.file_id.span(tok.span.end(), 0)
        })
    }

    /// Report the next token — or the end of input — as one that cannot be used
    /// here, and fail. Every path that gives up goes through this: a production
    /// that returned `None` quietly would drop the statement it was parsing and
    /// leave the run looking successful.
    ///
    /// A `_` at the cursor gets the wildcard's own complaint rather than the
    /// generic one, so every position it trips is told what the token means
    /// rather than only that it did not fit. The positions with a better
    /// wording — a field name, a pun, a projection, a type — say so before
    /// falling through to here.
    fn unexpected<T>(&mut self) -> Option<T> {
        if self.at_wildcard() {
            return self.wildcard(Place::Value);
        }
        let span = match self.peek() {
            Some(tok) => tok.span,
            None => self.eof_span(),
        };
        self.error(span, ErrorKind::Unexpected);
        None
    }

    /// Whether the next token is the wildcard `_`.
    fn at_wildcard(&self) -> bool {
        matches!(
            self.peek(),
            Some(tok) if matches!(tok.tracked, Kind::Underscore)
        )
    }

    /// Report the `_` at the cursor as one that discards nothing here, and
    /// fail — [`unexpected`](Self::unexpected) with the R9 wording in place of
    /// the generic one. The caller says which position tripped; the meaning is
    /// the same in all of them.
    fn wildcard<T>(&mut self, place: Place) -> Option<T> {
        let span = self.peek().expect("the caller peeked an underscore").span;
        self.error(span, ErrorKind::Wildcard { place });
        None
    }

    /// Consume the next token if it is the same variant as `want`, ignoring any
    /// payload. Records an error and consumes nothing on mismatch or EOF.
    fn eat(&mut self, want: &Kind) -> Option<Token> {
        match self.eat_if(want) {
            Some(tok) => Some(tok),
            None => self.unexpected(),
        }
    }

    fn ident(&mut self) -> Option<TrackedString> {
        let name = match self.peek() {
            Some(tok) => match &tok.tracked {
                Kind::Identifier(name) => Some(tok.span.track(name.clone())),
                _ => None,
            },
            None => None,
        };
        match name {
            Some(name) => {
                self.advance();
                Some(name)
            }
            None => self.unexpected(),
        }
    }

    /// Consume the next token if it is a tag, and hand back the name it
    /// carries without the backtick. Silent on anything else, unlike
    /// [`ident`](Self::ident): both callers have somewhere else to go — the
    /// case list ends, and the expression is not a tag — so there is nothing
    /// here to complain about.
    fn tag(&mut self) -> Option<TrackedString> {
        let name = match self.peek() {
            Some(tok) => match &tok.tracked {
                Kind::Tag(name) => Some(tok.span.track(name.clone())),
                _ => None,
            },
            None => None,
        };
        if name.is_some() {
            self.advance();
        }
        name
    }

    fn stmt(&mut self) -> Option<Stmt> {
        match self.peek().map(|tok| &tok.tracked) {
            Some(Kind::Let) => self.let_stmt(),
            Some(Kind::Type) => self.type_stmt(),
            _ => self.unexpected(),
        }
    }

    /// `let <pattern> [: <type>] = <expr>`. The ascription is optional: without
    /// it the definition's type is whatever is inferred for its body. What is
    /// bound is a pattern — a bare name being the ordinary case, and a struct
    /// pattern taking the value apart into several definitions at once.
    fn let_stmt(&mut self) -> Option<Stmt> {
        let kw = self.advance().expect("the caller peeked `let`");
        let pattern = self.pattern()?;
        let ty = match self.eat_if(&Kind::Colon) {
            Some(_) => Some(self.annotation(true)?),
            None => None,
        };
        self.eat(&Kind::Equal)?;
        let expr = self.expr()?;
        let body = expr.span.track(expr);
        let span = kw.span.merge(body.span);
        Some(span.track(StmtKind::Let {
            pattern: Box::new(pattern),
            ty,
            body,
        }))
    }

    /// `type <name> <param>* = <type>`. The parameters are plain names, and the
    /// list ends at the `=` — so an empty one is not the error an empty
    /// [`function_expr`](Self::function_expr) argument list is: a type taking
    /// no parameters is the ordinary case.
    fn type_stmt(&mut self) -> Option<Stmt> {
        let kw = self.advance().expect("the caller peeked `type`");
        // A declaration's name and parameters are names: a `_` is not one, and
        // a type is nothing a value could be thrown away from, so it gets the
        // wildcard complaint worded for a type rather than the generic one.
        if self.at_wildcard() {
            return self.wildcard(Place::Type);
        }
        let name = self.ident()?;
        let mut params = Vec::new();
        while matches!(
            self.peek().map(|tok| &tok.tracked),
            Some(Kind::Identifier(_))
        ) {
            params.push(self.ident().expect("the loop peeked a name"));
        }
        if self.at_wildcard() {
            return self.wildcard(Place::Type);
        }
        self.eat(&Kind::Equal)?;
        // Nothing follows a declaration's body, so a second comparison in a
        // `where` written there is a chain however it is spelled.
        let body = self.annotation(false)?;
        let span = kw.span.merge(body.span());
        Some(span.track(StmtKind::Type { name, params, body }))
    }

    /// Whether the next token can begin an atomic expression — used to decide
    /// when to stop gathering application arguments.
    ///
    /// [`Kind::Let`] is deliberately not one, though [`atom`](Self::atom)
    /// reads one: an application stops in front of a nested `let`, so
    /// `let a = 1 let b = 2` is two definitions rather than `a` applied to
    /// one, and a `let` handed to a function is written in parentheses.
    ///
    /// [`Kind::Underscore`] is one, though no expression can ever be made of
    /// it: `f _` is somebody reaching for a discard where a value goes, and
    /// stopping in front of it would leave the complaint to whatever comes
    /// after the application instead of pointing at the `_` itself.
    fn at_expr_atom(&self) -> bool {
        matches!(
            self.peek(),
            Some(tok) if matches!(
                tok.tracked,
                Kind::Identifier(_)
                    | Kind::Natural(_)
                    | Kind::Tag(_)
                    | Kind::Fn
                    | Kind::LeftBrace
                    | Kind::LeftParen
                    | Kind::Underscore
            )
        )
    }

    /// Application binds tighter than nothing and is left-associative, ML-style:
    /// `f x y` parses as `(f x) y`.
    fn expr(&mut self) -> Option<Expr> {
        let mut func = self.projection()?;
        while self.at_expr_atom() {
            let arg = self.projection()?;
            let span = func.span.merge(arg.span);
            func = span.track(ExprKind::Apply {
                func: Box::new(func),
                arg: Box::new(arg),
            });
        }
        Some(func)
    }

    /// `<atom>.<field>*` — postfix projection, left-associative. It sits under
    /// application rather than beside it so that `f p.x` reads as `f (p.x)`,
    /// which is what reaching into a record before passing it along looks like.
    ///
    /// A `..` here is refused rather than left for whoever comes next. It is
    /// one token, not two dots — the lexer decides that — and there is nothing
    /// an expression can do with it, so `p..x` has no reading. Stopping
    /// quietly would end the projection, end the application, and end the
    /// `let` with a perfectly good `p` in it; the reader would be told
    /// "unexpected token" about the line below and `a` would silently be
    /// whatever `p` is.
    fn projection(&mut self) -> Option<Expr> {
        let mut base = self.atom()?;
        loop {
            if matches!(self.peek().map(|tok| &tok.tracked), Some(Kind::DotDot)) {
                return self.unexpected();
            }
            if self.eat_if(&Kind::Dot).is_none() {
                return Some(base);
            }
            // `x._` reads a field named nothing: the complaint is the
            // wildcard's, worded for the projection it sits in.
            if self.at_wildcard() {
                return self.wildcard(Place::Projection);
            }
            let field = self.ident()?;
            let span = base.span.merge(field.span);
            base = span.track(ExprKind::Project {
                base: Box::new(base),
                field,
            });
        }
    }

    fn atom(&mut self) -> Option<Expr> {
        let Some(tok) = self.peek() else {
            return self.unexpected();
        };
        let span = tok.span;
        match &tok.tracked {
            Kind::Fn => self.function_expr(),
            Kind::Let => self.let_expr(),
            // Reachable from atom position, like a nested `let` — and, like
            // one, deliberately absent from `at_expr_atom`, so `f match ... end`
            // is not `f` applied to a match. Projection off the `end` works
            // because the projection loop sits above this call.
            Kind::Match => self.match_expr(),
            Kind::LeftBrace => self.struct_expr(),
            Kind::LeftParen => self.paren_expr(),
            Kind::Tag(_) => self.tag_expr(),
            Kind::Identifier(name) => {
                let name = span.track(name.clone());
                self.advance();
                Some(span.track(ExprKind::Ident { name }))
            }
            &Kind::Natural(value) => {
                self.advance();
                Some(span.track(ExprKind::Natural(value)))
            }
            // Nothing here begins an expression
            _ => self.unexpected(),
        }
    }

    /// `{ <field>: <expr>, <field>: <expr>, ... }` with an optional trailing
    /// comma — the same shape as a struct type, but with expression values.
    fn struct_expr(&mut self) -> Option<Expr> {
        let open = self.eat(&Kind::LeftBrace).expect("the caller peeked `{`");
        let mut fields = IndexMap::new();

        while !matches!(
            self.peek().map(|t| &t.tracked),
            Some(Kind::RightBrace) | None
        ) {
            // `{ _: 1 }` names a field nothing: the complaint is the
            // wildcard's, worded for the field name it fails to be.
            if self.at_wildcard() {
                return self.wildcard(Place::Field);
            }
            let name = self.ident()?;
            self.eat(&Kind::Colon)?;
            let value = self.expr()?;
            fields.insert(name, value);

            // A comma separates fields; its absence ends the field list.
            if self.eat_if(&Kind::Comma).is_none() {
                break;
            }
        }

        let close = self.eat(&Kind::RightBrace)?;
        let span = open.span.merge(close.span);
        Some(span.track(ExprKind::Struct(fields)))
    }

    /// `` `Some <atom> `` — one case of a sum, with what it carries.
    ///
    /// The payload is one atom, taken greedily, which is what makes a tag bind
    /// tighter than application: `` f `A 1 `` is `f` applied to `` `A 1 ``
    /// rather than `` f `A `` applied to `1`. A tag carries one thing, so
    /// there is nothing for the second reading to mean, and the greedy one is
    /// what a reader writing a constructor expects.
    ///
    /// Through [`projection`](Self::projection) rather than
    /// [`atom`](Self::atom), so that `` `Some p.x `` carries the field rather
    /// than the record it was read off.
    ///
    /// A case carrying nothing is written with nothing after it, and means
    /// unit. That is not decided here: this records what was written, and
    /// lowering is where `` `None `` and `` `None () `` meet — the same
    /// division `()` and `{}` already keep.
    fn tag_expr(&mut self) -> Option<Expr> {
        let name = self.tag().expect("the caller peeked a tag");
        let payload = match self.at_expr_atom() {
            true => Some(self.projection()?),
            false => None,
        };
        let span = payload
            .as_ref()
            .map_or(name.span, |payload| name.span.merge(payload.span));
        Some(span.track(ExprKind::Tag {
            name,
            payload: payload.map(Box::new),
        }))
    }

    /// `( <expr> )` — grouping only. The parentheses override application's
    /// left-associativity while parsing and are then discarded: the inner node
    /// is returned as-is, widened to cover the delimiters, so no grouping node
    /// exists to reach the IR. An empty pair is the unit expression.
    fn paren_expr(&mut self) -> Option<Expr> {
        let open = self.eat(&Kind::LeftParen).expect("the caller peeked `(`");
        if let Some(close) = self.eat_if(&Kind::RightParen) {
            return Some(open.span.merge(close.span).track(ExprKind::Unit));
        }
        let inner = self.expr()?;
        let close = self.eat(&Kind::RightParen)?;
        Some(open.span.merge(close.span).track(inner.tracked))
    }

    /// `fn <arg>* => <expr>` — an anonymous function with zero or more
    /// arguments. The body is a full expression and extends as far right as it
    /// can, ML-style.
    fn function_expr(&mut self) -> Option<Expr> {
        let kw = self.advance().expect("the caller peeked `fn`");
        let args = self.function_args()?;
        let body = self.expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(ExprKind::Function {
            args,
            body: Box::new(body),
        }))
    }

    /// `let <name> [: <type>] = <value> in <body>` — the expression form of
    /// [`let_stmt`](Self::let_stmt), which the two share their shape with and
    /// nothing else.
    ///
    /// The body is a full expression and extends as far right as it can, the
    /// way a `fn` body does. Nothing has to be done to stop it running past
    /// the end of the enclosing form: `in` begins no atom, so an application
    /// ends in front of one and a nested `let` reaches its own `in` and no
    /// other.
    fn let_expr(&mut self) -> Option<Expr> {
        let kw = self.advance().expect("the caller peeked `let`");
        let pattern = self.pattern()?;
        let ty = match self.eat_if(&Kind::Colon) {
            Some(_) => Some(Box::new(self.annotation(true)?)),
            None => None,
        };
        self.eat(&Kind::Equal)?;
        let value = self.expr()?;
        self.eat(&Kind::In)?;
        let body = self.expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(ExprKind::Let {
            pattern,
            ty,
            value: Box::new(value),
            body: Box::new(body),
        }))
    }

    /// `match <expr> with [|] <arm> (| <arm>)* end`, where an arm is
    /// `<pattern> => <expr>`.
    ///
    /// The leading `|` is optional and a trailing one is refused, the same
    /// convention a sum type keeps; a `|` between arms promises another arm,
    /// so nothing after one is reported where the missing pattern was
    /// expected. Zero arms parse — `match e with end` — and the leading `|`
    /// with no arm after it does not: the bar promised one.
    ///
    /// Each arm's body is a full expression and extends as far right as it
    /// can; it ends in front of the next `|` or the `end` of its own accord,
    /// because neither begins an atom.
    fn match_expr(&mut self) -> Option<Expr> {
        let kw = self.advance().expect("the caller peeked `match`");
        let scrutinee = self.expr()?;
        self.eat(&Kind::With)?;
        let mut arms = Vec::new();
        let leading = self.eat_if(&Kind::Pipe).is_some();
        if leading || !matches!(self.peek().map(|tok| &tok.tracked), Some(Kind::End)) {
            loop {
                let pattern = self.pattern()?;
                self.eat(&Kind::FatArrow)?;
                let body = self.expr()?;
                arms.push(Arm { pattern, body });
                if self.eat_if(&Kind::Pipe).is_none() {
                    break;
                }
            }
        }
        let close = self.eat(&Kind::End)?;
        let span = kw.span.merge(close.span);
        Some(span.track(ExprKind::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        }))
    }

    /// Whether the next token can begin a pattern — what decides whether a tag
    /// pattern has a payload, the same question [`at_expr_atom`](Self::at_expr_atom)
    /// answers for a tag expression.
    fn at_pattern(&self) -> bool {
        matches!(
            self.peek(),
            Some(tok) if matches!(
                tok.tracked,
                Kind::Identifier(_)
                    | Kind::Natural(_)
                    | Kind::Tag(_)
                    | Kind::LeftBrace
                    | Kind::LeftParen
                    | Kind::Underscore
            )
        )
    }

    /// One pattern: a name, a natural, `()`, a parenthesized pattern, a struct
    /// pattern, or a tag pattern carrying another. See [`PatternKind`].
    fn pattern(&mut self) -> Option<Pattern> {
        let Some(tok) = self.peek() else {
            return self.unexpected();
        };
        let span = tok.span;
        match &tok.tracked {
            // The payload is taken greedily, like a tag expression's: a tag
            // carries one thing, and the recursion through `pattern` is what
            // makes `` `A `B x `` come out as `` `A `` carrying `` (`B x) ``.
            Kind::Tag(_) => {
                let name = self.tag().expect("the caller peeked a tag");
                let payload = match self.at_pattern() {
                    true => Some(self.pattern()?),
                    false => None,
                };
                let span = payload
                    .as_ref()
                    .map_or(name.span, |payload| name.span.merge(payload.span));
                Some(span.track(PatternKind::Tag {
                    name,
                    payload: payload.map(Box::new),
                }))
            }
            Kind::Identifier(name) => {
                let name = span.track(name.clone());
                self.advance();
                Some(span.track(PatternKind::Ident { name }))
            }
            // The one position `_` is at home in: it matches anything, the way
            // a name does, and binds nothing, which is the point of it.
            Kind::Underscore => {
                self.advance();
                Some(span.track(PatternKind::Wildcard))
            }
            &Kind::Natural(value) => {
                self.advance();
                Some(span.track(PatternKind::Natural(value)))
            }
            Kind::LeftBrace => self.struct_pattern(),
            Kind::LeftParen => self.paren_pattern(),
            // Nothing here begins a pattern — an arm written `=> 1` is missing
            // one, and this is where it is told so.
            _ => self.unexpected(),
        }
    }

    /// `{ <field>, <field>: <pattern>, ..., [..] }` with an optional trailing
    /// comma among the fields — the struct expression's shape, with a bare name
    /// allowed to pun, and a `..` allowed to end the list. The `..` makes the
    /// pattern open — at-least matching — and comes last, the way a struct
    /// type's tail does: the fields it stands for have no order among the
    /// named ones to claim. Only `}` may follow it; a name after it — the
    /// named rest a future spec may want — is the unexpected token it looks
    /// like, reported where it was written.
    fn struct_pattern(&mut self) -> Option<Pattern> {
        let open = self.eat(&Kind::LeftBrace).expect("the caller peeked `{`");
        let mut fields = IndexMap::new();
        let mut rest = None;

        while !matches!(
            self.peek().map(|t| &t.tracked),
            Some(Kind::RightBrace) | None
        ) {
            if let Some(dots) = self.eat_if(&Kind::DotDot) {
                rest = Some(dots.span);
                // Nothing but the brace may follow: the `eat` below reports
                // whatever else was written, at the token itself.
                break;
            }
            // A `_` here is a field named nothing — or, with no colon after
            // it, a pun of nothing: a pun binds a field to its own name, and
            // `_` is not a name. Which of the two decides the wording, and
            // one token of lookahead decides which.
            if self.at_wildcard() {
                let place = match self.toks.get(self.pos + 1).map(|tok| &tok.tracked) {
                    Some(Kind::Colon) => Place::Field,
                    _ => Place::Pun,
                };
                return self.wildcard(place);
            }
            let name = self.ident()?;
            // A colon gives the field a sub-pattern; its absence puns, binding
            // the field to its own name. What punning means is not decided
            // here: the parser records that nothing was written.
            let value = match self.eat_if(&Kind::Colon) {
                Some(_) => Some(self.pattern()?),
                None => None,
            };
            fields.insert(name, value);

            // A comma separates fields; its absence ends the field list.
            if self.eat_if(&Kind::Comma).is_none() {
                break;
            }
        }

        let close = self.eat(&Kind::RightBrace)?;
        let span = open.span.merge(close.span);
        Some(span.track(PatternKind::Struct { fields, rest }))
    }

    /// `( <pattern> )` — grouping only, discarded exactly as
    /// [`paren_expr`](Self::paren_expr) discards it. An empty pair is the unit
    /// pattern.
    fn paren_pattern(&mut self) -> Option<Pattern> {
        let open = self.eat(&Kind::LeftParen).expect("the caller peeked `(`");
        if let Some(close) = self.eat_if(&Kind::RightParen) {
            return Some(open.span.merge(close.span).track(PatternKind::Unit));
        }
        let inner = self.pattern()?;
        let close = self.eat(&Kind::RightParen)?;
        Some(open.span.merge(close.span).track(inner.tracked))
    }

    /// The `<arg>+ =>` header of a function: gather the arguments — names, and
    /// the `_` that binds nothing — then consume the `=>` arrow. A function
    /// must take at least one argument, so an empty list is a parse error;
    /// `fn _ => e` takes one and discards it, which is not the same thing.
    fn function_args(&mut self) -> Option<Vec<Arg>> {
        let mut args = Vec::new();
        while let Some(tok) = self.peek() {
            let arg = match &tok.tracked {
                Kind::Identifier(name) => tok.span.track(ArgKind::Name(name.clone())),
                Kind::Underscore => tok.span.track(ArgKind::Wildcard),
                _ => break,
            };
            self.advance();
            args.push(arg);
        }
        let arrow = self.eat(&Kind::FatArrow)?;
        if args.is_empty() {
            // `fn => ...` takes nothing; reject it at the arrow.
            self.error(arrow.span, ErrorKind::Unexpected);
            return None;
        }
        Some(args)
    }

    /// Whether the token at `at` is the contextual keyword `word`.
    ///
    /// `when`, `where`, `or`, `and` and `not` are ordinary identifiers
    /// everywhere but the few type positions that read them, so they are
    /// recognized by spelling here rather than reserved by the lexer. That is
    /// what keeps a term or a label called `when` writable: only the positions
    /// below ask, and one token of lookahead is all any of them needs.
    fn keyword_at(&self, at: usize, word: &str) -> bool {
        matches!(
            self.toks.get(at).map(|tok| &tok.tracked),
            Some(Kind::Identifier(name)) if name == word
        )
    }

    /// Whether the next token is the contextual keyword `word`.
    fn at_keyword(&self, word: &str) -> bool {
        self.keyword_at(self.pos, word)
    }

    /// Consume the next token if it is the contextual keyword `word`.
    fn eat_keyword(&mut self, word: &str) -> Option<Token> {
        match self.at_keyword(word) {
            true => self.advance(),
            false => None,
        }
    }

    /// `<type> [where <clause>]` — what a definition is ascribed, and what a
    /// `type` declaration's body is read as.
    ///
    /// The clause is read here and nowhere deeper: a `where` ends a whole
    /// written type, once, so the names it may use are exactly the ones the
    /// `when`s of that type bound. Which of them it actually names is
    /// [`ir`](crate::ir)'s to check.
    fn annotation(&mut self, defined: bool) -> Option<Annotation> {
        let ty = self.type_expr()?;
        let clause = match self.eat_keyword("where") {
            Some(kw) => Some(self.where_clause(kw.span, defined)?),
            None => None,
        };
        Some(Annotation { ty, clause })
    }

    /// `<statement> (';' <statement>)*` — the whole of a `where` clause.
    ///
    /// A trailing `;` is a parse error rather than a courtesy, which is what
    /// every other separator in the language already is: the `;` promises
    /// another statement, and whatever follows it is reported where it was
    /// written.
    fn where_clause(&mut self, kw: Span, defined: bool) -> Option<Where> {
        let mut stmts = Vec::new();
        let mut span = kw;
        loop {
            let stmt = self.clause_stmt(defined)?;
            span = span.merge(stmt.span);
            stmts.push(stmt);
            if self.eat_if(&Kind::Semicolon).is_none() {
                break;
            }
        }
        Some(Where { span, stmts })
    }

    /// One statement of a `where` clause: `let <name> (, <name>)*`, or the
    /// boolean expression grammar.
    ///
    /// The speculative read that tells a comparison's `=` from the definition's
    /// own applies to the last statement alone, since only the last can be
    /// followed by the definition's `=`. Which statement is the last is decided
    /// by reading one without the speculation and looking for the `;` that would
    /// promise another: a statement followed by one was never a candidate, and
    /// what was read is what was meant. Anything else is put back — cursor and
    /// complaints alike — and read again the way an annotation's last statement
    /// has always been read.
    fn clause_stmt(&mut self, defined: bool) -> Option<ClauseStmt> {
        if let Some(kw) = self.eat_if(&Kind::Let) {
            let mut names = vec![self.ident()?];
            while self.eat_if(&Kind::Comma).is_some() {
                names.push(self.ident()?);
            }
            let last = names.last().expect("a declaration statement names one");
            let span = kw.span.merge(last.span);
            return Some(span.track(ClauseStmtKind::Let(names)));
        }
        if defined {
            let mark = (self.pos, self.errors.len());
            if let Some(clause) = self.clause(false)
                && matches!(self.peek().map(|tok| &tok.tracked), Some(Kind::Semicolon))
            {
                return Some(clause.span.track(ClauseStmtKind::Constraint(clause)));
            }
            self.pos = mark.0;
            self.errors.truncate(mark.1);
        }
        let clause = self.clause(defined)?;
        Some(clause.span.track(ClauseStmtKind::Constraint(clause)))
    }

    /// `<or> [('='|'!=') <or>]` — the loosest level of a `where` clause, and
    /// the one that does not associate: `a = b = c` is refused rather than
    /// read one way or the other, since neither reading is what a person
    /// writing it meant.
    fn clause(&mut self, defined: bool) -> Option<Clause> {
        let left = self.clause_or()?;
        let equal = match self.peek().map(|tok| &tok.tracked) {
            Some(Kind::Equal) => true,
            Some(Kind::NotEqual) => false,
            _ => return Some(left),
        };
        // A definition's own `=` follows its annotation, so an `=` here is
        // either the clause's comparison or the one that introduces the value —
        // and what tells them apart is that the clause's is followed by the
        // definition's. Read speculatively and put back: `where a = b = v`
        // compares, and `where a = v` is the clause `a` and then the value.
        //
        // `!=` needs none of this: nothing but a clause can hold one, so it is
        // always the comparison.
        let mark = (self.pos, self.errors.len());
        self.advance();
        if defined && equal {
            let read = self
                .clause_or()
                .filter(|_| matches!(self.peek().map(|tok| &tok.tracked), Some(Kind::Equal)));
            let Some(right) = read else {
                self.pos = mark.0;
                self.errors.truncate(mark.1);
                return Some(left);
            };
            let span = left.span.merge(right.span);
            return Some(span.track(ClauseKind::Equal(Box::new(left), Box::new(right))));
        }
        let right = self.clause_or()?;
        let span = left.span.merge(right.span);
        let kind = match equal {
            true => ClauseKind::Equal(Box::new(left), Box::new(right)),
            false => ClauseKind::NotEqual(Box::new(left), Box::new(right)),
        };
        // Non-associative: a second comparison after the first has no reading,
        // so it is reported where it was written rather than folded in.
        //
        // With one exception, and it is not the grammar bending. A definition's
        // own `=` follows its annotation, so in `let p: T where a != b = v` the
        // `=` after the clause is the one that introduces the value — the
        // clause has already ended, and there is nothing here to refuse. `!=`
        // is never that, so a chain written with one is still reported wherever
        // it appears.
        let chained = match defined {
            true => matches!(self.peek().map(|tok| &tok.tracked), Some(Kind::NotEqual)),
            false => matches!(
                self.peek().map(|tok| &tok.tracked),
                Some(Kind::Equal | Kind::NotEqual)
            ),
        };
        if chained {
            return self.unexpected();
        }
        Some(span.track(kind))
    }

    /// `<and> (or <and>)*`, left-associative.
    fn clause_or(&mut self) -> Option<Clause> {
        let mut left = self.clause_and()?;
        while self.eat_keyword("or").is_some() {
            let right = self.clause_and()?;
            let span = left.span.merge(right.span);
            left = span.track(ClauseKind::Or(Box::new(left), Box::new(right)));
        }
        Some(left)
    }

    /// `<not> (and <not>)*`, left-associative.
    fn clause_and(&mut self) -> Option<Clause> {
        let mut left = self.clause_not()?;
        while self.eat_keyword("and").is_some() {
            let right = self.clause_not()?;
            let span = left.span.merge(right.span);
            left = span.track(ClauseKind::And(Box::new(left), Box::new(right)));
        }
        Some(left)
    }

    /// `not <not>`, or an atom. Unary and stacking, so `not not a` reads.
    fn clause_not(&mut self) -> Option<Clause> {
        let Some(kw) = self.eat_keyword("not") else {
            return self.clause_atom();
        };
        let inner = self.clause_not()?;
        let span = kw.span.merge(inner.span);
        Some(span.track(ClauseKind::Not(Box::new(inner))))
    }

    /// `( <clause> )` or a name. A `_` here is the wildcard's own complaint:
    /// `when _` mints a presence no formula may name, so there is nothing for
    /// a clause to be saying about it.
    fn clause_atom(&mut self) -> Option<Clause> {
        if let Some(open) = self.eat_if(&Kind::LeftParen) {
            // Inside parentheses nothing follows the clause but the `)`, so a
            // chain written there is a chain however the annotation ends.
            let inner = self.clause(false)?;
            let close = self.eat(&Kind::RightParen)?;
            return Some(open.span.merge(close.span).track(inner.tracked));
        }
        if self.at_wildcard() {
            return self.wildcard(Place::Type);
        }
        let name = self.ident()?;
        Some(name.span.track(ClauseKind::Name(name.tracked)))
    }

    /// The `when` clause a label may wear, when one is written there.
    ///
    /// `parens` says whether the clause is bracketed, which is the one thing
    /// the two positions differ in: a struct field writes it bare between the
    /// label and the colon, and a sum case has no colon to end it, so it takes
    /// parentheses. Both bind a name, and both allow `_`.
    fn when(&mut self, parens: bool) -> Option<When> {
        let open = match parens {
            true => Some(self.eat(&Kind::LeftParen).expect("the caller peeked `(`")),
            false => None,
        };
        let kw = self.eat_keyword("when").expect("the caller peeked `when`");
        let mut span = match &open {
            Some(open) => open.span.merge(kw.span),
            None => kw.span,
        };
        // `when _` is the anonymous presence: this definition decides it, and
        // no formula may name it.
        let name = match self.at_wildcard() {
            true => {
                let discard = self.advance().expect("just peeked `_`");
                span = span.merge(discard.span);
                None
            }
            false => {
                let name = self.ident()?;
                span = span.merge(name.span);
                Some(name)
            }
        };
        if parens {
            let close = self.eat(&Kind::RightParen)?;
            span = span.merge(close.span);
        }
        Some(When { span, name })
    }

    /// `<sum> [-> <type>]` — the arrow is right-associative, so
    /// `A -> B -> C` is `A -> (B -> C)`, and everything else binds tighter, so
    /// `Pair Nat Nat -> Nat` is `(Pair Nat Nat) -> Nat` and
    /// `` `A Nat | `B -> Nat `` is a function from the sum rather than a sum
    /// whose last case carries an arrow.
    fn type_expr(&mut self) -> Option<Type> {
        let from = self.type_sum()?;
        if self.eat_if(&Kind::Arrow).is_none() {
            return Some(from);
        }
        let to = self.type_expr()?;
        let span = from.span.merge(to.span);
        Some(span.track(TypeKind::Arrow {
            from: Box::new(from),
            to: Box::new(to),
        }))
    }

    /// `['|'] <case> ('|' <case>)*` — a sum type, or whatever
    /// [`type_apply`](Self::type_apply) makes of a type that is not one.
    ///
    /// A sum is recognised by how it starts and by nothing else: a `|`, a
    /// tag, or the `\` of a case written absent. The leading `|` is optional,
    /// as it is in every language that writes sums this way, and it is what
    /// makes the two degenerate forms writable — `|` alone is the sum with no
    /// cases, and `| ..r` is the sum that is nothing but its tail.
    ///
    /// The tail is the `..` a struct type already has, standing for the cases
    /// not written out rather than the fields, and it comes last for the same
    /// reason: what it covers has no place among the cases that are named.
    fn type_sum(&mut self) -> Option<Type> {
        let bar = self.eat_if(&Kind::Pipe);
        if bar.is_none()
            && !matches!(
                self.peek().map(|t| &t.tracked),
                Some(Kind::Tag(_) | Kind::Backslash)
            )
        {
            return self.type_apply();
        }

        let mut cases = IndexMap::new();
        let mut tail = None;
        let mut span = match &bar {
            Some(bar) => bar.span,
            // A sum written with no leading `|` starts at its first case, and
            // the loop below is about to read it.
            None => self.peek().expect("a case is next").span,
        };
        // The `|` that separated the last case from what comes next, when the
        // loop has read one. The leading `|` is not one of these: it may be
        // followed by nothing at all, and `|` alone is the empty sum.
        let mut separator = None;
        loop {
            if let Some(dots) = self.eat_if(&Kind::DotDot) {
                let name = match self.peek().map(|t| &t.tracked) {
                    Some(Kind::Identifier(_)) => Some(self.ident().expect("just peeked a name")),
                    _ => None,
                };
                let at = name
                    .as_ref()
                    .map_or(dots.span, |name| dots.span.merge(name.span));
                span = span.merge(at);
                tail = Some(Tail { span: at, name });
                break;
            }
            if let Some(slash) = self.eat_if(&Kind::Backslash) {
                // A `\` promises a case, so anything but a tag after it is
                // reported — the case keeps its backtick, spelled the same
                // way present or absent. No payload and no `when` either;
                // whatever follows but a `|` is somebody else's token to
                // refuse, exactly as it is after a written case's payload.
                let Some(name) = self.tag() else {
                    return self.unexpected();
                };
                span = span.merge(name.span);
                // The key's span is the whole `` \`Name ``, for the reason a
                // struct's absent field's is.
                let key = slash.span.merge(name.span).track(name.tracked);
                cases.insert(key, SumCase::Absent);
            } else {
                // `|` with nothing after it is the empty sum, and `| ..r` broke
                // out above, so the only thing left that a case can begin with
                // is a tag. Anything else ends the sum rather than failing: a
                // `->` or a `)` after the last case is somebody else's token.
                //
                // Unless a `|` between cases promised another one. Nothing
                // came, so the bar is reported and the cases read so far stand:
                // the reader has one thing to delete, and the sum they wrote is
                // still there to be checked.
                let Some(name) = self.tag() else {
                    if let Some(bar) = separator {
                        self.error(bar, ErrorKind::Unexpected);
                    }
                    break;
                };
                span = span.merge(name.span);
                // A case has no colon to end a bare `when`, so the clause takes
                // parentheses — and two tokens of lookahead tell one from a
                // parenthesized payload, since only the clause has `when`
                // inside it.
                let when = match self.at_left_paren() && self.keyword_at(self.pos + 1, "when") {
                    true => {
                        let when = self.when(true)?;
                        span = span.merge(when.span);
                        Some(Box::new(when))
                    }
                    false => None,
                };
                // What a case carries is one atom, which is the same rule
                // [`tag_expr`](Self::tag_expr) keeps for a term: a tag carries
                // one thing, and anything with a space in it takes parentheses.
                // So `` `Some Pair A B `` is not a case with three payloads,
                // and `` `A Nat -> Nat `` is a function from the sum rather
                // than a case carrying an arrow.
                let payload = match self.at_type_atom() {
                    true => {
                        let payload = self.type_atom()?;
                        span = span.merge(payload.span);
                        Some(payload)
                    }
                    false => None,
                };
                cases.insert(name, SumCase::Written { when, payload });
            }

            match self.eat_if(&Kind::Pipe) {
                Some(pipe) => separator = Some(pipe.span),
                None => break,
            }
        }

        Some(span.track(TypeKind::Sum { cases, tail }))
    }

    /// `<atom> <atom>*` — a type applied to arguments, gathered flat.
    ///
    /// Flat rather than folded pairwise the way [`expr`](Self::expr) folds an
    /// application: `Pair Nat Nat` is one node, and there is no `Pair Nat` in
    /// between for anything to mean. The recursion goes through
    /// [`type_atom`](Self::type_atom) rather than back through here, so
    /// `Pair Pair Nat` is `Pair` applied to two arguments rather than a nested
    /// application — which is what keeps a type from being applied to a type
    /// that is itself waiting for arguments.
    fn type_apply(&mut self) -> Option<Type> {
        let head = self.type_atom()?;
        if !self.at_type_atom() {
            return Some(head);
        }
        let mut args = Vec::new();
        let mut span = head.span;
        while self.at_type_atom() {
            let arg = self.type_atom()?;
            span = span.merge(arg.span);
            args.push(arg);
        }
        Some(span.track(TypeKind::Apply {
            head: Box::new(head),
            args,
        }))
    }

    /// Whether the next token can begin an atomic type — the type-level
    /// counterpart of [`at_expr_atom`](Self::at_expr_atom), and what decides
    /// where an application stops. `_` is one because it *is* a type now: the
    /// hole, which a type application may be given as an argument like any
    /// other atom.
    fn at_type_atom(&self) -> bool {
        // `where` ends a written type rather than continuing it. It is an
        // ordinary identifier everywhere else — the one position that reads it
        // is this one, which is what "contextual" means here — so a type
        // application stops in front of one instead of taking it as another
        // argument.
        if self.at_keyword("where") {
            return false;
        }
        matches!(
            self.peek(),
            Some(tok) if matches!(
                tok.tracked,
                Kind::Identifier(_) | Kind::LeftBrace | Kind::LeftParen | Kind::Underscore
            )
        )
    }

    /// Whether the next token opens a parenthesis — the first half of the two
    /// tokens a sum case's `when` clause is told apart by.
    fn at_left_paren(&self) -> bool {
        matches!(self.peek(), Some(tok) if matches!(tok.tracked, Kind::LeftParen))
    }

    /// A name, a struct, a parenthesized type, or `()`. Application is
    /// [`type_apply`](Self::type_apply)'s; what is here is what can be an
    /// argument without parentheses.
    fn type_atom(&mut self) -> Option<Type> {
        let Some(tok) = self.peek() else {
            return self.unexpected();
        };
        let span = tok.span;
        match &tok.tracked {
            Kind::LeftBrace => self.struct_type(),
            Kind::LeftParen => self.paren_type(),
            Kind::Identifier(name) => {
                let name = span.track(name.clone());
                self.advance();
                Some(span.track(TypeKind::Ident { name }))
            }
            // `_` in a type is the hole: a position left for inference to
            // decide, which binds nothing and can be written as often as one
            // likes. The two positions where it is still not a type — a
            // declaration's name and its parameters — say so before reaching
            // here.
            Kind::Underscore => {
                self.advance();
                Some(span.track(TypeKind::Hole))
            }
            // As in [`atom`](Self::atom): a type position with no type in it is
            // reported, so `let x : = ()` cannot pass for `let x : () = ()`.
            _ => self.unexpected(),
        }
    }

    /// `{ <field> [when <a>]: <type> | \<field>, ..., [..[<name>]] }` with an
    /// optional trailing comma among the fields. A `when` names the presence
    /// variable that says whether the field is there; `\name` says it definitely
    /// is not, which takes no type and no `when` — whatever follows it but a
    /// comma or the brace is the
    /// unexpected token it looks like, since nothing here reads one. The `..`
    /// tail, when present, comes last — the fields it stands for have no order
    /// among the named ones to claim — and takes no comma after it.
    fn struct_type(&mut self) -> Option<Type> {
        let open = self.eat(&Kind::LeftBrace).expect("the caller peeked `{`");
        let mut fields = IndexMap::new();
        let mut tail = None;

        while !matches!(
            self.peek().map(|t| &t.tracked),
            Some(Kind::RightBrace) | None
        ) {
            if let Some(dots) = self.eat_if(&Kind::DotDot) {
                let name = match self.peek().map(|t| &t.tracked) {
                    Some(Kind::Identifier(_)) => Some(self.ident().expect("just peeked a name")),
                    _ => None,
                };
                let span = name
                    .as_ref()
                    .map_or(dots.span, |name| dots.span.merge(name.span));
                tail = Some(Tail { span, name });
                break;
            }
            if let Some(slash) = self.eat_if(&Kind::Backslash) {
                // The key's span is the whole `\name`, so a complaint about
                // the entry — a repeat, a `\` in a closed struct — underlines
                // the absence mark along with the name it marks.
                let name = self.ident()?;
                let key = slash.span.merge(name.span).track(name.tracked);
                fields.insert(key, TypeField::Absent);
            } else {
                let name = self.ident()?;
                // One token of lookahead is the whole disambiguation: after a
                // field's name, a `when` can only be the clause, because
                // `{when: Nat}` has already spent its `when` on the name.
                let when = match self.at_keyword("when") {
                    true => Some(Box::new(self.when(false)?)),
                    false => None,
                };
                self.eat(&Kind::Colon)?;
                let value = self.type_expr()?;
                fields.insert(name, TypeField::Written { when, value });
            }

            // A comma separates fields; its absence ends the field list.
            if self.eat_if(&Kind::Comma).is_none() {
                break;
            }
        }

        let close = self.eat(&Kind::RightBrace)?;
        let span = open.span.merge(close.span);
        Some(span.track(TypeKind::Struct { fields, tail }))
    }

    /// `( <type> )` — the type-level counterpart of
    /// [`paren_expr`](Self::paren_expr), discarded just the same.
    fn paren_type(&mut self) -> Option<Type> {
        let open = self.eat(&Kind::LeftParen).expect("the caller peeked `(`");
        if let Some(close) = self.eat_if(&Kind::RightParen) {
            return Some(open.span.merge(close.span).track(TypeKind::Unit));
        }
        let inner = self.type_expr()?;
        let close = self.eat(&Kind::RightParen)?;
        Some(open.span.merge(close.span).track(inner.tracked))
    }

    /// Like [`eat`], but silent on mismatch: consume the token only if it
    /// matches, otherwise leave the cursor untouched and report nothing.
    fn eat_if(&mut self, want: &Kind) -> Option<Token> {
        match self.peek() {
            Some(tok) if std::mem::discriminant(&tok.tracked) == std::mem::discriminant(want) => {
                self.advance()
            }
            _ => None,
        }
    }

    /// Skip tokens until the start of the next statement (or EOF) so a single
    /// malformed statement doesn't cascade into a flood of errors.
    fn recover(&mut self) {
        while let Some(tok) = self.peek() {
            if matches!(tok.tracked, Kind::Let | Kind::Type) {
                break;
            }
            self.advance();
        }
    }
}
