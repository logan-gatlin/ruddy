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
    Ident {
        name: Tracked<String>,
    },
    /// A natural number literal, already read from its digits by the lexer.
    Natural(u128),
    Unit,
}

pub type Type = Tracked<TypeKind>;

#[derive(Debug, Clone)]
pub enum TypeKind {
    Apply {
        func: Box<Type>,
        arg: Box<Type>,
    },
    Struct(IndexMap<Tracked<String>, Type>),
    Function {
        args: Vec<Tracked<String>>,
        body: Box<Type>,
    },
    Ident {
        name: Tracked<String>,
    },
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

impl std::fmt::Display for StmtKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `body` is a `Tracked<Expr>` and `Expr` is itself `Tracked`, hence
            // the doubled `.tracked` to reach the `ExprKind`.
            StmtKind::Let { name, body } => {
                write!(f, "let {} = {}", name.tracked, body.tracked.tracked)
            }
            StmtKind::Type { name, body } => {
                write!(f, "type {} = {}", name.tracked, body.tracked)
            }
        }
    }
}

impl std::fmt::Display for ExprKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, ExprKind::Function { .. }),
                    &func.tracked,
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(
                        arg.tracked,
                        ExprKind::Apply { .. } | ExprKind::Function { .. }
                    ),
                    &arg.tracked,
                )
            }
            ExprKind::Function { args, body } => write_function(f, args, &body.tracked),
            ExprKind::Struct(fields) => write_struct(f, fields),
            ExprKind::Ident { name } => f.write_str(&name.tracked),
            ExprKind::Natural(value) => write!(f, "{value}"),
            ExprKind::Unit => f.write_str("()"),
        }
    }
}

impl std::fmt::Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Apply { func, arg } => {
                write_grouped(
                    f,
                    matches!(func.tracked, TypeKind::Function { .. }),
                    &func.tracked,
                )?;
                f.write_str(" ")?;
                write_grouped(
                    f,
                    matches!(
                        arg.tracked,
                        TypeKind::Apply { .. } | TypeKind::Function { .. }
                    ),
                    &arg.tracked,
                )
            }
            TypeKind::Function { args, body } => write_function(f, args, &body.tracked),
            TypeKind::Struct(fields) => write_struct(f, fields),
            TypeKind::Ident { name } => f.write_str(&name.tracked),
            TypeKind::Unit => f.write_str("()"),
        }
    }
}

/// Render `body`, wrapping it in parentheses when leaving them off would make
/// the printed source re-parse as a different tree. Grouping is not recorded in
/// the AST, so the printer has to reconstruct it from the shape of the node.
/// Shared with the IR printer, which emits the same surface syntax.
pub(crate) fn write_grouped(
    f: &mut std::fmt::Formatter<'_>,
    parens: bool,
    body: &dyn std::fmt::Display,
) -> std::fmt::Result {
    match parens {
        true => write!(f, "({body})"),
        false => write!(f, "{body}"),
    }
}

/// Render a `fn a b c => body` header shared by expression and type lambdas.
fn write_function(
    f: &mut std::fmt::Formatter<'_>,
    args: &[Tracked<String>],
    body: &dyn std::fmt::Display,
) -> std::fmt::Result {
    f.write_str("fn")?;
    for arg in args {
        write!(f, " {}", arg.tracked)?;
    }
    write!(f, " => {body}")
}

/// Render a `{ name: value, ... }` literal shared by struct expressions and
/// struct types (the values differ, but the shape is identical).
fn write_struct<V: std::fmt::Display>(
    f: &mut std::fmt::Formatter<'_>,
    fields: &IndexMap<Tracked<String>, Tracked<V>>,
) -> std::fmt::Result {
    f.write_str("{ ")?;
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{}: {}", name.tracked, value.tracked)?;
    }
    f.write_str(" }")
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

    /// Consume the next token if it is the same variant as `want`, ignoring any
    /// payload. Records an error and consumes nothing on mismatch or EOF.
    fn eat(&mut self, want: &Kind) -> Option<Token> {
        let span = match self.peek() {
            Some(tok) if std::mem::discriminant(&tok.tracked) == std::mem::discriminant(want) => {
                return self.advance();
            }
            Some(tok) => tok.span,
            None => return None,
        };
        self.error(span);
        None
    }

    fn ident(&mut self) -> Option<Tracked<String>> {
        let (span, name) = match self.peek() {
            Some(tok) => match &tok.tracked {
                Kind::Identifier(name) => (tok.span, Some(name.clone())),
                _ => (tok.span, None),
            },
            None => return None,
        };
        match name {
            Some(name) => {
                self.advance();
                Some(span.track(name))
            }
            None => {
                self.error(span);
                None
            }
        }
    }

    fn stmt(&mut self) -> Option<Stmt> {
        let tok = self.peek()?;
        match &tok.tracked {
            Kind::Let => self.let_stmt(),
            Kind::Type => self.type_stmt(),
            _ => {
                let span = tok.span;
                self.error(span);
                None
            }
        }
    }

    fn let_stmt(&mut self) -> Option<Stmt> {
        let kw = self.advance()?; // `let`
        let name = self.ident()?;
        self.eat(&Kind::Equal)?;
        let expr = self.expr()?;
        let body = expr.span.track(expr);
        let span = kw.span.merge(body.span);
        Some(span.track(StmtKind::Let { name, body }))
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
        let mut func = self.atom()?;
        while self.at_expr_atom() {
            let arg = self.atom()?;
            let span = func.span.merge(arg.span);
            func = span.track(ExprKind::Apply {
                func: Box::new(func),
                arg: Box::new(arg),
            });
        }
        Some(func)
    }

    fn atom(&mut self) -> Option<Expr> {
        let tok = self.peek()?;
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
            // An empty / unrecognized expression position parses as unit and
            // consumes nothing, leaving recovery to the caller.
            _ => Some(span.track(ExprKind::Unit)),
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

    /// The `<arg>+ =>` header shared by expression and type functions: gather
    /// the argument identifiers, then consume the `=>` arrow. A function must
    /// bind at least one argument, so an empty list is a parse error.
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

    /// Whether the next token can begin an atomic type — used to bound type
    /// application the same way [`at_expr_atom`](Self::at_expr_atom) bounds
    /// expression application.
    fn at_type_atom(&self) -> bool {
        matches!(
            self.peek(),
            Some(tok) if matches!(
                tok.tracked,
                Kind::Identifier(_) | Kind::Fn | Kind::LeftBrace | Kind::LeftParen
            )
        )
    }

    /// Type application is left-associative, like term application: `F A B`
    /// parses as `(F A) B`.
    fn type_expr(&mut self) -> Option<Type> {
        let mut func = self.type_atom()?;
        while self.at_type_atom() {
            let arg = self.type_atom()?;
            let span = func.span.merge(arg.span);
            func = span.track(TypeKind::Apply {
                func: Box::new(func),
                arg: Box::new(arg),
            });
        }
        Some(func)
    }

    fn type_atom(&mut self) -> Option<Type> {
        let tok = self.peek()?;
        let span = tok.span;
        match &tok.tracked {
            Kind::Fn => self.function_type(),
            Kind::LeftBrace => self.struct_type(),
            Kind::LeftParen => self.paren_type(),
            Kind::Identifier(name) => {
                let name = span.track(name.clone());
                self.advance();
                Some(span.track(TypeKind::Ident { name }))
            }
            _ => Some(span.track(TypeKind::Unit)),
        }
    }

    /// `fn <arg>* => <type>` — a type-level lambda, sharing the `fn` header
    /// grammar with expression functions.
    fn function_type(&mut self) -> Option<Type> {
        let kw = self.advance()?; // `fn`
        let args = self.function_args()?;
        let body = self.type_expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(TypeKind::Function {
            args,
            body: Box::new(body),
        }))
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
