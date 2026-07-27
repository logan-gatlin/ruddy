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
    Apply {
        func: Box<Type>,
        arg: Box<Type>,
    },
    Struct(IndexMap<Tracked<String>, Type>),
    Arrow {
        from: Box<Type>,
        to: Box<Type>,
    },
    Lambda {
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

/// How tightly a printed node binds. Grouping is dropped rather than recorded,
/// so the printers reconstruct it from this alone: one table per node kind
/// ([`Grouped`]) and one rule per position ([`write_apply`] and friends), which
/// is what keeps the AST printer and the IR printer from drifting apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Prec {
    /// `fn a => body` — the body extends as far right as it can, so a lambda
    /// needs parentheses anywhere anything may follow it.
    Lambda,
    /// `from -> to`.
    Arrow,
    /// `func arg`.
    Apply,
    /// A form nothing can be appended to: a name, a literal, `()`, a braced
    /// struct, or a projection off one of those.
    Atom,
}

/// A node a printer has to parenthesize by precedence. Implemented by the four
/// node kinds that print as surface syntax — two here and two in the IR.
pub(crate) trait Grouped: std::fmt::Display {
    fn prec(&self) -> Prec;
}

impl std::fmt::Display for StmtKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `body` is a `Tracked<Expr>` and `Expr` is itself `Tracked`, hence
            // the doubled `.tracked` to reach the `ExprKind`.
            StmtKind::Let { name, ty, body } => {
                write!(f, "let {}", name.tracked)?;
                if let Some(ty) = ty {
                    write!(f, " : {}", ty.tracked)?;
                }
                write!(f, " = {}", body.tracked.tracked)
            }
            StmtKind::Type { name, body } => {
                write!(f, "type {} = {}", name.tracked, body.tracked)
            }
        }
    }
}

impl Grouped for ExprKind {
    fn prec(&self) -> Prec {
        match self {
            ExprKind::Function { .. } => Prec::Lambda,
            ExprKind::Apply { .. } => Prec::Apply,
            ExprKind::Project { .. }
            | ExprKind::Struct(_)
            | ExprKind::Ident { .. }
            | ExprKind::Natural(_)
            | ExprKind::Unit => Prec::Atom,
        }
    }
}

impl Grouped for TypeKind {
    fn prec(&self) -> Prec {
        match self {
            TypeKind::Lambda { .. } => Prec::Lambda,
            TypeKind::Arrow { .. } => Prec::Arrow,
            TypeKind::Apply { .. } => Prec::Apply,
            TypeKind::Struct(_) | TypeKind::Ident { .. } | TypeKind::Unit => Prec::Atom,
        }
    }
}

impl std::fmt::Display for ExprKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprKind::Apply { func, arg } => write_apply(f, &func.tracked, &arg.tracked),
            ExprKind::Function { args, body } => write_function(f, args, &body.tracked),
            ExprKind::Struct(fields) => write_struct(f, fields),
            ExprKind::Project { base, field } => write_project(f, &base.tracked, &field.tracked),
            ExprKind::Ident { name } => f.write_str(&name.tracked),
            ExprKind::Natural(value) => write!(f, "{value}"),
            ExprKind::Unit => f.write_str("()"),
        }
    }
}

impl std::fmt::Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Apply { func, arg } => write_apply(f, &func.tracked, &arg.tracked),
            TypeKind::Arrow { from, to } => write_arrow(f, &from.tracked, &to.tracked),
            TypeKind::Lambda { args, body } => write_function(f, args, &body.tracked),
            TypeKind::Struct(fields) => write_struct(f, fields),
            TypeKind::Ident { name } => f.write_str(&name.tracked),
            TypeKind::Unit => f.write_str("()"),
        }
    }
}

/// Render `func arg`. A lambda on the left would swallow the argument into its
/// own body, and anything that keeps consuming to its right would swallow
/// whatever follows the argument.
pub(crate) fn write_apply(
    f: &mut std::fmt::Formatter<'_>,
    func: &impl Grouped,
    arg: &impl Grouped,
) -> std::fmt::Result {
    write_grouped(f, func.prec() < Prec::Apply, func)?;
    f.write_str(" ")?;
    write_grouped(f, arg.prec() < Prec::Atom, arg)
}

/// Render `from -> to`. The arrow is right-associative, so only the left side
/// can ever need grouping.
pub(crate) fn write_arrow(
    f: &mut std::fmt::Formatter<'_>,
    from: &impl Grouped,
    to: &impl Grouped,
) -> std::fmt::Result {
    write_grouped(f, from.prec() < Prec::Apply, from)?;
    write!(f, " -> {to}")
}

/// Render `base.field`. Projection binds tighter than everything that follows a
/// space, so only the forms that extend rightward need grouping.
pub(crate) fn write_project(
    f: &mut std::fmt::Formatter<'_>,
    base: &impl Grouped,
    field: &str,
) -> std::fmt::Result {
    write_grouped(f, base.prec() < Prec::Atom, base)?;
    write!(f, ".{field}")
}

/// Render `body`, wrapping it in parentheses when leaving them off would make
/// the printed source re-parse as a different tree.
fn write_grouped(
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

    /// `<app> [-> <type>]` — the arrow binds looser than type application, so
    /// `F A -> G B` is one arrow between two applications, and is
    /// right-associative, so `A -> B -> C` is `A -> (B -> C)`.
    fn type_expr(&mut self) -> Option<Type> {
        let from = self.type_app()?;
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

    /// Type application is left-associative, like term application: `F A B`
    /// parses as `(F A) B`.
    fn type_app(&mut self) -> Option<Type> {
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
        let Some(tok) = self.peek() else {
            return self.unexpected();
        };
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
            // As in [`atom`](Self::atom): a type position with no type in it is
            // reported, so `let x : = ()` cannot pass for `let x : () = ()`.
            _ => self.unexpected(),
        }
    }

    /// `fn <arg>* => <type>` — a type-level lambda, sharing the `fn` header
    /// grammar with expression functions.
    fn function_type(&mut self) -> Option<Type> {
        let kw = self.advance()?; // `fn`
        let args = self.function_args()?;
        let body = self.type_expr()?;
        let span = kw.span.merge(body.span);
        Some(span.track(TypeKind::Lambda {
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
