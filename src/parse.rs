use indexmap::IndexMap;

use crate::{
    token::{Kind, Token},
    tracking::{Span, Tracked},
};

pub type Stmt = Tracked<StmtKind>;

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        name: Tracked<String>,
        /// The written type, when the definition was ascribed one.
        ty: Option<Type>,
        body: Tracked<Expr>,
    },
    Type {
        name: Tracked<String>,
        body: Type,
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
        args: Vec<Tracked<String>>,
        body: Box<Expr>,
    },
    Struct(IndexMap<Tracked<String>, Expr>),
    Project {
        base: Box<Expr>,
        field: Tracked<String>,
    },
    Ident {
        name: Tracked<String>,
    },
    Natural(u128),
    Unit,
}

pub type Type = Tracked<TypeKind>;

#[derive(Debug, Clone)]
pub enum TypeKind {
    Struct(IndexMap<Tracked<String>, Type>),
    Arrow { from: Box<Type>, to: Box<Type> },
    Ident { name: Tracked<String> },
    Unit,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
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
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn error(&mut self, span: Span) {
        self.errors.push(Error { span });
    }

    /// The zero-width position just past the last token, so that running out of
    /// input has somewhere to point.
    fn eof_span(&self) -> Span {
        match self.toks.last() {
            Some(tok) => tok.span.file_id.span(tok.span.end(), 0),
            None => Span::default(),
        }
    }

    /// Report the next token — or the end of input — as one that cannot be used
    /// here, and fail. Every path that gives up goes through this: a production
    /// that returned `None` quietly would drop the statement it was parsing and
    /// leave the run looking successful.
    fn unexpected<T>(&mut self) -> Option<T> {
        let span = match self.peek() {
            Some(tok) => tok.span,
            None => self.eof_span(),
        };
        self.error(span);
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

    fn ident(&mut self) -> Option<Tracked<String>> {
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

    fn stmt(&mut self) -> Option<Stmt> {
        match self.peek().map(|tok| &tok.tracked) {
            Some(Kind::Let) => self.let_stmt(),
            Some(Kind::Type) => self.type_stmt(),
            _ => self.unexpected(),
        }
    }

    /// `let <name> [: <type>] = <expr>`. The ascription is optional: without it
    /// the definition's type is whatever is inferred for its body.
    fn let_stmt(&mut self) -> Option<Stmt> {
        let kw = self.advance()?; // `let`
        let name = self.ident()?;
        let ty = match self.eat_if(&Kind::Colon) {
            Some(_) => Some(self.type_expr()?),
            None => None,
        };
        self.eat(&Kind::Equal)?;
        let expr = self.expr()?;
        let body = expr.span.track(expr);
        let span = kw.span.merge(body.span);
        Some(span.track(StmtKind::Let { name, ty, body }))
    }

    fn type_stmt(&mut self) -> Option<Stmt> {
        let kw = self.advance()?; // `type`
        let name = self.ident()?;
        self.eat(&Kind::Equal)?;
        let body = self.type_expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(StmtKind::Type { name, body }))
    }

    /// Whether the next token can begin an atomic expression — used to decide
    /// when to stop gathering application arguments.
    fn at_expr_atom(&self) -> bool {
        matches!(
            self.peek(),
            Some(tok) if matches!(
                tok.tracked,
                Kind::Identifier(_)
                    | Kind::Natural(_)
                    | Kind::Fn
                    | Kind::LeftBrace
                    | Kind::LeftParen
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
    fn projection(&mut self) -> Option<Expr> {
        let mut base = self.atom()?;
        while self.eat_if(&Kind::Dot).is_some() {
            let field = self.ident()?;
            let span = base.span.merge(field.span);
            base = span.track(ExprKind::Project {
                base: Box::new(base),
                field,
            });
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<Expr> {
        let Some(tok) = self.peek() else {
            return self.unexpected();
        };
        let span = tok.span;
        match &tok.tracked {
            Kind::Fn => self.function_expr(),
            Kind::LeftBrace => self.struct_expr(),
            Kind::LeftParen => self.paren_expr(),
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
        let open = self.eat(&Kind::LeftBrace)?;
        let mut fields = IndexMap::new();

        while !matches!(
            self.peek().map(|t| &t.tracked),
            Some(Kind::RightBrace) | None
        ) {
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

    /// `( <expr> )` — grouping only. The parentheses override application's
    /// left-associativity while parsing and are then discarded: the inner node
    /// is returned as-is, widened to cover the delimiters, so no grouping node
    /// exists to reach the IR. An empty pair is the unit expression.
    fn paren_expr(&mut self) -> Option<Expr> {
        let open = self.eat(&Kind::LeftParen)?;
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
        let kw = self.advance()?; // `fn`
        let args = self.function_args()?;
        let body = self.expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(ExprKind::Function {
            args,
            body: Box::new(body),
        }))
    }

    /// The `<arg>+ =>` header of a function: gather the argument identifiers,
    /// then consume the `=>` arrow. A function must bind at least one argument,
    /// so an empty list is a parse error.
    fn function_args(&mut self) -> Option<Vec<Tracked<String>>> {
        let mut args = Vec::new();
        while matches!(self.peek().map(|t| &t.tracked), Some(Kind::Identifier(_))) {
            args.push(self.ident()?);
        }
        let arrow = self.eat(&Kind::FatArrow)?;
        if args.is_empty() {
            // `fn => ...` binds nothing; reject it at the arrow.
            self.error(arrow.span);
            return None;
        }
        Some(args)
    }

    /// `<atom> [-> <type>]` — the arrow is the only type operator, and it is
    /// right-associative, so `A -> B -> C` is `A -> (B -> C)`.
    fn type_expr(&mut self) -> Option<Type> {
        let from = self.type_atom()?;
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

    /// A name, a struct, a parenthesized type, or `()`. The type language has
    /// no binder and no application: every name written here denotes a type
    /// outright, never a function from types to types.
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
            // As in [`atom`](Self::atom): a type position with no type in it is
            // reported, so `let x : = ()` cannot pass for `let x : () = ()`.
            _ => self.unexpected(),
        }
    }

    /// `{ <field>: <type>, <field>: <type>, ... }` with an optional trailing
    /// comma.
    fn struct_type(&mut self) -> Option<Type> {
        let open = self.eat(&Kind::LeftBrace)?;
        let mut fields = IndexMap::new();

        while !matches!(
            self.peek().map(|t| &t.tracked),
            Some(Kind::RightBrace) | None
        ) {
            let name = self.ident()?;
            self.eat(&Kind::Colon)?;
            let ty = self.type_expr()?;
            fields.insert(name, ty);

            // A comma separates fields; its absence ends the field list.
            if self.eat_if(&Kind::Comma).is_none() {
                break;
            }
        }

        let close = self.eat(&Kind::RightBrace)?;
        let span = open.span.merge(close.span);
        Some(span.track(TypeKind::Struct(fields)))
    }

    /// `( <type> )` — the type-level counterpart of
    /// [`paren_expr`](Self::paren_expr), discarded just the same.
    fn paren_type(&mut self) -> Option<Type> {
        let open = self.eat(&Kind::LeftParen)?;
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
