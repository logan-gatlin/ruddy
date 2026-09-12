//! Source rewrites for recognized accumulator opportunities. The typed tree
//! proves binding identity and reorderability; source slices keep user text.
use super::Analysis;
use crate::{
    ir::{BinaryOp, Term, TermKind, UnaryOp},
    parse::{self, ExprKind, StmtKind},
    symbol::{Namespace, Symbol},
    tracking::{Anchor, FileID, Span},
    types::{Presence, Rest, Row, Ty},
};

/// One optional editor refactoring, computed entirely from one analysis revision.
#[derive(Debug, Clone)]
pub struct TailRecursion {
    pub binding: Span,
    pub span: Span,
    pub replacement: String,
}

impl Analysis {
    pub fn tail_recursion(&self, path: &str) -> Vec<TailRecursion> {
        let Some(text) = self.sources.get(path) else {
            return Vec::new();
        };
        if self.diagnostics.iter().any(|d| {
            self.paths
                .get(&d.primary.span.file_id)
                .is_some_and(|p| p == path)
        }) {
            return Vec::new();
        }
        let file = self
            .paths
            .iter()
            .find_map(|(file, known)| (known == path).then_some(*file))
            .unwrap_or(FileID::GENERATED);
        let blocks = block_spans(text, file);
        let mut opportunities = Vec::new();
        for (symbol, decl) in self.terms_in(path) {
            crate::cancellation::checkpoint();
            let bindings = std::iter::once((symbol, decl.name_at, &decl.value)).chain(
                decl.value.walk().filter_map(|term| match &term.kind {
                    TermKind::Let { name, value, .. } => {
                        Some((name.anchored, name.at, value.as_ref()))
                    }
                    _ => None,
                }),
            );
            for (symbol, name, function) in bindings {
                if let Some(fix) = (Candidate {
                    analysis: self,
                    text,
                    symbol,
                    blocks: &blocks,
                })
                .plan(name, function)
                {
                    opportunities.push(fix);
                }
            }
        }
        opportunities
    }
}

/// Lowering erases `do` wrappers: a binding's anchor covers its statement,
/// while a block with only imports inherits its result's anchor. Recover those
/// wrappers from syntax so copying an expression also copies its lexical scope.
fn block_spans(text: &str, file: FileID) -> std::collections::HashMap<Span, Span> {
    let parsed = parse::parse(crate::token::lex(text, file).tokens);
    let mut blocks = std::collections::HashMap::new();
    let mut statements: Vec<_> = parsed.stmts.iter().collect();
    let mut expressions = Vec::new();
    loop {
        if let Some(statement) = statements.pop() {
            match &statement.kind {
                StmtKind::Let { body, .. } => expressions.push(&body.tracked),
                StmtKind::Module {
                    body: Some(body), ..
                } => statements.extend(body),
                _ => {}
            }
            continue;
        }
        let Some(expression) = expressions.pop() else {
            break;
        };
        crate::cancellation::checkpoint();
        match &expression.tracked {
            ExprKind::Do { stmts, result } => {
                let origin = stmts
                    .iter()
                    .find(|stmt| matches!(stmt.kind, StmtKind::Let { .. }))
                    .map(|stmt| stmt.span)
                    .or_else(|| result.as_ref().map(|value| value.span));
                if let Some(origin) = origin {
                    blocks.insert(origin, expression.span);
                }
                statements.extend(stmts);
                expressions.extend(result.as_deref());
            }
            ExprKind::Function { body, .. }
            | ExprKind::Unary { value: body, .. }
            | ExprKind::Project { base: body, .. }
            | ExprKind::Raise(body) => expressions.push(body),
            ExprKind::Binary { left, right, .. } => {
                expressions.extend([left.as_ref(), right.as_ref()])
            }
            ExprKind::Apply { func, arg } => expressions.extend([func.as_ref(), arg.as_ref()]),
            ExprKind::Pipe { value, function } => {
                expressions.extend([value.as_ref(), function.as_ref()])
            }
            ExprKind::Match { scrutinee, arms } => {
                expressions.push(scrutinee);
                expressions.extend(arms.iter().map(|arm| &arm.body));
            }
            ExprKind::MatchFunction { arms, .. } => {
                expressions.extend(arms.iter().map(|arm| &arm.body))
            }
            ExprKind::If {
                predicate,
                consequent,
                alternative,
            } => expressions.extend([
                predicate.as_ref(),
                consequent.as_ref(),
                alternative.as_ref(),
            ]),
            ExprKind::Struct { fields, spread } => {
                expressions.extend(fields.values());
                if let Some(spread) = spread {
                    expressions.push(&spread.value);
                }
            }
            ExprKind::Tuple(items) => expressions.extend(items),
            ExprKind::Array(items) => expressions.extend(items.iter().map(|item| &item.value)),
            ExprKind::Tag { payload, .. } => expressions.extend(payload.as_deref()),
            ExprKind::Handle { body, arms } => {
                expressions.push(body);
                expressions.extend(arms.iter().map(|arm| &arm.body));
            }
            ExprKind::Ident { .. }
            | ExprKind::Operation { .. }
            | ExprKind::Natural(_)
            | ExprKind::Integer(_)
            | ExprKind::Fixed(_)
            | ExprKind::Real(_)
            | ExprKind::String(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit => {}
        }
    }
    blocks
}

struct Candidate<'a> {
    analysis: &'a Analysis,
    text: &'a str,
    symbol: Symbol,
    blocks: &'a std::collections::HashMap<Span, Span>,
}

#[derive(Clone, Default)]
struct EffectScope {
    handled: std::collections::HashSet<String>,
    can_raise: bool,
}

impl EffectScope {
    fn allows(&self, mut row: &Row) -> bool {
        loop {
            if row.labels.iter().any(|(key, field)| {
                field.presence != Presence::Absent && !self.handled.contains(key)
            }) {
                return false;
            }
            match &row.rest {
                Rest::Closed => return true,
                Rest::More(more) => row = more,
                _ => return false,
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ReductionKind {
    Sum,
    Product,
}

impl ReductionKind {
    fn matches(self, op: &BinaryOp) -> bool {
        matches!(
            (self, op),
            (Self::Sum, BinaryOp::Add) | (Self::Product, BinaryOp::Mul)
        )
    }
    fn name(self) -> &'static str {
        match self {
            Self::Sum => "add",
            Self::Product => "multiply",
        }
    }
    fn identity(self) -> &'static str {
        match self {
            Self::Sum => "0",
            Self::Product => "1",
        }
    }
    fn infix(self) -> &'static str {
        match self {
            Self::Sum => "+",
            Self::Product => "*",
        }
    }
}

struct Reduction {
    kind: ReductionKind,
    symbol: Option<Symbol>,
    callable: Option<String>,
    identity: String,
}

impl Reduction {
    fn operands<'a>(&self, term: &'a Term) -> Option<(&'a Term, &'a Term)> {
        match &term.kind {
            TermKind::Binary { op, left, right }
                if self.callable.is_none() && self.kind.matches(op) =>
            {
                Some((left, right))
            }
            TermKind::Apply { func, arg: right } => {
                let TermKind::Apply { func, arg: left } = &func.kind else {
                    return None;
                };
                let TermKind::Ident(symbol) = func.kind else {
                    return None;
                };
                (Some(symbol) == self.symbol).then_some((left, right))
            }
            _ => None,
        }
    }
    fn combine(&self, accumulator: &str, value: &str) -> String {
        match &self.callable {
            Some(name) => format!("{name} {accumulator} ({value})"),
            None => format!("{accumulator} {} ({value})", self.kind.infix()),
        }
    }
}

impl Candidate<'_> {
    fn span(&self, term: &Term) -> Span {
        let mut span = self.analysis.built.source.span(term.at);
        while let Some(block) = self.blocks.get(&span) {
            if block.width <= span.width {
                break;
            }
            span = *block;
        }
        span
    }
    fn source(&self, term: &Term) -> Option<&str> {
        let span = self.span(term);
        self.text.get(span.start..span.end())
    }
    fn mentions_self(&self, term: &Term) -> bool {
        term.walk()
            .any(|t| matches!(t.kind, TermKind::Ident(s) if s == self.symbol))
    }
    fn pure(&self, term: &Term) -> bool {
        let mut scopes = vec![EffectScope::default()];
        let mut work = vec![(term, 0)];
        while let Some((term, scope)) = work.pop() {
            crate::cancellation::checkpoint();
            let mut children = Vec::new();
            match &term.kind {
                TermKind::Apply { func, arg } => {
                    let ty = crate::inference::unfold(
                        self.analysis.inferred.semantics().aliases(),
                        &func.ty,
                    );
                    if !matches!(&*ty, Ty::Arrow(_, _, row) if scopes[scope].allows(row)) {
                        return false;
                    }
                    children.extend([func.as_ref(), arg.as_ref()]);
                }
                // Creating a closure does not execute its body. Its arrow row
                // is checked above if the surrounding expression calls it.
                TermKind::Fn { .. } => {}
                TermKind::Handle { body, handler } => {
                    let mut inside = scopes[scope].clone();
                    for effect in &handler.discharges {
                        let Some(identity) =
                            self.analysis.built.program.effect_ids.get(&effect.anchored)
                        else {
                            return false;
                        };
                        inside.handled.insert(identity.row_key());
                    }
                    work.push((body, scopes.len()));
                    scopes.push(inside);
                    // Arms run outside their own handler: a rethrow must be
                    // covered by an outer handler, never by the arm itself.
                    let mut arms = scopes[scope].clone();
                    arms.can_raise = true;
                    work.extend(handler.arms.iter().map(|arm| (&arm.body, scopes.len())));
                    if let Some(ret) = &handler.ret {
                        work.push((&ret.body, scopes.len()));
                    }
                    scopes.push(arms);
                }
                TermKind::Raise(value) if scopes[scope].can_raise => children.push(value),
                TermKind::Unary {
                    op: UnaryOp::Allocate | UnaryOp::Read,
                    ..
                }
                | TermKind::Binary {
                    op: BinaryOp::Write,
                    ..
                }
                | TermKind::Raise(_)
                | TermKind::Error => return false,
                TermKind::Unary { value, .. } => children.push(value),
                TermKind::Binary { left, right, .. } => {
                    children.extend([left.as_ref(), right.as_ref()])
                }
                TermKind::Let { value, body, .. } => {
                    children.extend([value.as_ref(), body.as_ref()])
                }
                TermKind::Match { scrutinee, arms } => {
                    children.push(scrutinee);
                    children.extend(arms.iter().map(|(_, body)| body));
                }
                TermKind::Struct { fields, spread } => {
                    children.extend(fields.values().map(|field| &field.value));
                    if let Some(spread) = spread {
                        children.push(&spread.value);
                    }
                }
                TermKind::Array(items) => children.extend(items.iter().map(|item| &item.value)),
                TermKind::Tag { payload, .. } => children.extend(payload.as_deref()),
                TermKind::Project { base, .. } => children.push(base),
                TermKind::Ident(_)
                | TermKind::Operation { .. }
                | TermKind::Natural(_)
                | TermKind::Integer(_)
                | TermKind::Fixed(_)
                | TermKind::Real(_)
                | TermKind::String(_)
                | TermKind::Bool(_) => {}
            }
            work.extend(children.into_iter().map(|child| (child, scope)));
        }
        true
    }
    fn recursive_call<'a>(
        &self,
        term: &'a Term,
        arity: usize,
        signature: &std::sync::Arc<Ty>,
    ) -> Option<&'a Term> {
        let mut head = term;
        let mut args = 0;
        while let TermKind::Apply { func, arg } = &head.kind {
            if self.mentions_self(arg) {
                return None;
            }
            args += 1;
            head = func;
        }
        // The unannotated helper is monomorphically recursive. A recursive
        // use at a different instantiation needs its own annotation strategy.
        (args == arity
            && matches!(head.kind, TermKind::Ident(s) if s == self.symbol)
            && crate::types::same_finite_syntax(&head.ty, signature))
        .then_some(head)
    }
    fn redirect(
        &self,
        call: &Term,
        head: &Term,
        helper: &str,
        accumulator: &str,
    ) -> Option<String> {
        let span = self.span(call);
        let head = self.span(head);
        if head.start < span.start || head.end() > span.end() {
            return None;
        }
        let mut text = self.source(call)?.to_owned();
        text.replace_range(
            head.start - span.start..head.end() - span.start,
            &format!("({helper} ({accumulator}))"),
        );
        Some(text)
    }
    fn reduction(&self, body: &Term, kind: ReductionKind) -> Option<Reduction> {
        let operation = kind.name();
        let identity = kind.identity();
        let ty = crate::inference::unfold(self.analysis.inferred.semantics().aliases(), &body.ty);
        let (module, operation, identity) = match &*ty {
            Ty::Real => ("real", operation.to_owned(), identity.to_owned()),
            Ty::Nat => ("nat", operation.to_owned(), format!("{identity}n")),
            Ty::Int => ("int", operation.to_owned(), format!("{identity}i")),
            Ty::Fixed(kind) => (
                if kind.signed() { "int" } else { "nat" },
                format!("{operation}{}", kind.bits()),
                format!("{identity}{}", kind.suffix()),
            ),
            _ => return None,
        };
        let real = matches!(&*ty, Ty::Real);
        let names = &self.analysis.built.names;
        let span = self.span(body);
        // Use an absolute dependency path in generated code. A branch-local
        // import or a shadowed module alias must not change the combiner used
        // by another branch of the helper.
        for (_, alias, _) in names.dependency_names() {
            let Some(parent) = names.resolve_module_path(
                &self.analysis.mint,
                None,
                span.file_id,
                span.start,
                &["", alias, module],
            ) else {
                continue;
            };
            for (namespace, name, symbol) in names.globals(parent) {
                if namespace != Namespace::Terms || name != operation {
                    continue;
                }
                let Some(qualified) = self.analysis.mint.external(symbol) else {
                    continue;
                };
                let Some((dependency, path)) = qualified.split_once("::") else {
                    continue;
                };
                if !dependency.starts_with("std@") || path != format!("{module}::{operation}") {
                    continue;
                }
                return Some(Reduction {
                    kind,
                    symbol: Some(symbol),
                    callable: (!real).then(|| format!("::{alias}::{module}::{operation}")),
                    identity,
                });
            }
        }
        real.then_some(Reduction {
            kind,
            symbol: None,
            callable: None,
            identity,
        })
    }
    fn plan(&self, name: Anchor, function: &Term) -> Option<TailRecursion> {
        [ReductionKind::Sum, ReductionKind::Product]
            .into_iter()
            .find_map(|kind| self.plan_reduction(name, function, kind))
    }
    fn plan_reduction(
        &self,
        name: Anchor,
        function: &Term,
        kind: ReductionKind,
    ) -> Option<TailRecursion> {
        let mut body = function;
        let mut parameters = Vec::new();
        while let TermKind::Fn { arg, body: next } = &body.kind {
            let name = self.analysis.mint.name(arg.anchored);
            let span = self.analysis.built.source.span(arg.at);
            // Destructured and compiler-generated arguments need a different
            // wrapper strategy; never print an internal symbol into source.
            if self.text.get(span.start..span.end())? != name
                || name.starts_with('%')
                || name == "_"
            {
                return None;
            }
            parameters.push(name);
            body = next;
        }
        if parameters.is_empty() {
            return None;
        }
        let reduction = self.reduction(body, kind)?;
        let fresh = |base: &str| {
            (0..)
                .map(|n| {
                    if n == 0 {
                        base.to_owned()
                    } else {
                        format!("{base}{n}")
                    }
                })
                .find(|name| !self.text.contains(name))
                .unwrap()
        };
        let helper = fresh("tail_loop");
        let accumulator = fresh("tail_acc");
        let mut work = vec![body];
        let mut edits = Vec::new();
        let mut recursive = 0;
        while let Some(term) = work.pop() {
            crate::cancellation::checkpoint();
            match &term.kind {
                TermKind::Match { scrutinee, arms } => {
                    if self.mentions_self(scrutinee) {
                        return None;
                    }
                    work.extend(arms.iter().map(|(_, body)| body));
                }
                TermKind::Let { value, body, .. } => {
                    if self.mentions_self(value) {
                        return None;
                    }
                    work.push(body);
                }
                _ => {
                    let replacement = if let Some((left, right)) = reduction.operands(term)
                        && let Some((call, head, contribution)) = self
                            .recursive_call(left, parameters.len(), &function.ty)
                            .map(|head| (left, head, right))
                            .or_else(|| {
                                self.recursive_call(right, parameters.len(), &function.ty)
                                    .map(|head| (right, head, left))
                            }) {
                        if self.mentions_self(contribution) || !self.pure(contribution) {
                            return None;
                        }
                        recursive += 1;
                        self.redirect(
                            call,
                            head,
                            &helper,
                            &reduction.combine(&accumulator, self.source(contribution)?),
                        )?
                    } else if let Some(head) =
                        self.recursive_call(term, parameters.len(), &function.ty)
                    {
                        self.redirect(term, head, &helper, &accumulator)?
                    } else {
                        if self.mentions_self(term) {
                            return None;
                        }
                        format!("({})", reduction.combine(&accumulator, self.source(term)?))
                    };
                    // Comments inside a replaced expression must not be lost.
                    // Other comments in the body are retained by the slice edit.
                    if crate::token::lex(self.source(term)?, self.span(term).file_id)
                        .tokens
                        .iter()
                        .any(|t| t.tracked.is_comment())
                    {
                        return None;
                    }
                    edits.push((self.span(term), replacement));
                }
            }
        }
        if recursive == 0 {
            return None;
        }
        let span = self.span(body);
        let mut rewritten = self.source(body)?.to_owned();
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut end = span.end();
        for (edit, replacement) in edits {
            if edit.start < span.start || edit.end() > end {
                return None;
            }
            rewritten.replace_range(
                edit.start - span.start..edit.end() - span.start,
                &replacement,
            );
            end = edit.start;
        }
        let args = parameters
            .iter()
            .map(|name| format!(" ({name})"))
            .collect::<String>();
        let lambda = parameters
            .iter()
            .map(|name| format!("fn {name} => "))
            .collect::<String>();
        let identity = &reduction.identity;
        let newline = if self.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let replacement = format!(
            "do{newline}  let {helper} = fn {accumulator} => {lambda}{rewritten}{newline}  return {helper} {identity}{args}{newline}end"
        );
        Some(TailRecursion {
            binding: self.analysis.built.source.span(name),
            span,
            replacement,
        })
    }
}
