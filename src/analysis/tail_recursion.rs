//! Source rewrites for recognized accumulator opportunities. The typed tree
//! proves binding identity and reorderability; source slices keep user text.
use super::Analysis;
use crate::{
    ir::{BinaryOp, Term, TermKind, UnaryOp},
    symbol::{Namespace, Symbol},
    tracking::{Anchor, Span},
    types::{Rest, Ty},
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

struct Candidate<'a> {
    analysis: &'a Analysis,
    text: &'a str,
    symbol: Symbol,
}

struct Addition {
    symbol: Option<Symbol>,
    callable: Option<String>,
    zero: String,
}

impl Addition {
    fn operands<'a>(&self, term: &'a Term) -> Option<(&'a Term, &'a Term)> {
        match &term.kind {
            TermKind::Binary {
                op: BinaryOp::Add,
                left,
                right,
            } if self.callable.is_none() => Some((left, right)),
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
            None => format!("{accumulator} + ({value})"),
        }
    }
}

impl Candidate<'_> {
    fn span(&self, term: &Term) -> Span {
        self.analysis.built.source.span(term.at)
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
        let mut work = vec![term];
        while let Some(term) = work.pop() {
            crate::cancellation::checkpoint();
            match &term.kind {
                TermKind::Apply { func, arg } => {
                    let ty = crate::inference::unfold(
                        self.analysis.inferred.semantics().aliases(),
                        &func.ty,
                    );
                    if !matches!(&*ty, Ty::Arrow(_, _, row) if row.labels.is_empty() && matches!(row.rest, Rest::Closed))
                    {
                        return false;
                    }
                    work.extend([func.as_ref(), arg.as_ref()]);
                }
                // Creating a closure does not execute its body. Its arrow row
                // is checked above if the surrounding expression calls it.
                TermKind::Fn { .. } => {}
                TermKind::Unary {
                    op: UnaryOp::Allocate | UnaryOp::Read,
                    ..
                }
                | TermKind::Binary {
                    op: BinaryOp::Write,
                    ..
                }
                | TermKind::Handle { .. }
                | TermKind::Raise(_)
                | TermKind::Error => return false,
                TermKind::Unary { value, .. } => work.push(value),
                TermKind::Binary { left, right, .. } => {
                    work.extend([left.as_ref(), right.as_ref()])
                }
                TermKind::Let { value, body, .. } => work.extend([value.as_ref(), body.as_ref()]),
                TermKind::Match { scrutinee, arms } => {
                    work.push(scrutinee);
                    work.extend(arms.iter().map(|(_, body)| body));
                }
                TermKind::Struct { fields, spread } => {
                    work.extend(fields.values().map(|field| &field.value));
                    if let Some(spread) = spread {
                        work.push(&spread.value);
                    }
                }
                TermKind::Array(items) => work.extend(items.iter().map(|item| &item.value)),
                TermKind::Tag { payload, .. } => work.extend(payload.as_deref()),
                TermKind::Project { base, .. } => work.push(base),
                TermKind::Ident(_)
                | TermKind::Operation { .. }
                | TermKind::Natural(_)
                | TermKind::Integer(_)
                | TermKind::Fixed(_)
                | TermKind::Real(_)
                | TermKind::String(_)
                | TermKind::Bool(_) => {}
            }
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
    fn addition(&self, body: &Term) -> Option<Addition> {
        let ty = crate::inference::unfold(self.analysis.inferred.semantics().aliases(), &body.ty);
        let (module, operation, zero) = match &*ty {
            Ty::Real => ("real", "add".to_owned(), "0".to_owned()),
            Ty::Nat => ("nat", "add".to_owned(), "0n".to_owned()),
            Ty::Int => ("int", "add".to_owned(), "0i".to_owned()),
            Ty::Fixed(kind) => (
                if kind.signed() { "int" } else { "nat" },
                format!("add{}", kind.bits()),
                format!("0{}", kind.suffix()),
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
                let Some((identity, path)) = qualified.split_once("::") else {
                    continue;
                };
                if !identity.starts_with("std@") || path != format!("{module}::{operation}") {
                    continue;
                }
                return Some(Addition {
                    symbol: Some(symbol),
                    callable: (!real).then(|| format!("::{alias}::{module}::{operation}")),
                    zero,
                });
            }
        }
        real.then_some(Addition {
            symbol: None,
            callable: None,
            zero,
        })
    }
    fn plan(&self, name: Anchor, function: &Term) -> Option<TailRecursion> {
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
        let addition = self.addition(body)?;
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
                    let replacement = if let Some((left, right)) = addition.operands(term)
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
                            &addition.combine(&accumulator, self.source(contribution)?),
                        )?
                    } else if let Some(head) =
                        self.recursive_call(term, parameters.len(), &function.ty)
                    {
                        self.redirect(term, head, &helper, &accumulator)?
                    } else {
                        if self.mentions_self(term) {
                            return None;
                        }
                        format!("({})", addition.combine(&accumulator, self.source(term)?))
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
        let zero = &addition.zero;
        let newline = if self.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let replacement = format!(
            "do{newline}  let {helper} = fn {accumulator} => {lambda}{rewritten}{newline}  return {helper} {zero}{args}{newline}end"
        );
        Some(TailRecursion {
            binding: self.analysis.built.source.span(name),
            span,
            replacement,
        })
    }
}
