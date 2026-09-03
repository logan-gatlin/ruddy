//! Target-neutral reviewed foreign-interface plans.

use indexmap::IndexMap;

use crate::{inference, ir, symbol::Symbol, tracking::Span, types::Scheme};

/// The reviewed foreign declarations later lowering may absorb without another
/// inference walk.
#[derive(Debug, Clone)]
pub struct ExternPlan {
    entries: IndexMap<Symbol, Extern>,
}

#[derive(Debug, Clone)]
pub struct Extern {
    pub symbol: Symbol,
    pub scheme: Scheme,
    pub abi: ir::ExternType,
    pub target: String,
    pub target_span: Span,
    pub declaration_span: Span,
}

impl ExternPlan {
    pub fn get(&self, symbol: Symbol) -> &Extern {
        &self.entries[&symbol]
    }
    pub fn iter(&self) -> impl Iterator<Item = &Extern> { self.entries.values() }
}

/// Turn inference-reviewed extern facts into the in-memory lowering plan.
/// This is infallible: accepted inference has already established ABI facts.
pub fn plan(program: &ir::Program, semantics: &inference::Semantics) -> ExternPlan {
    ExternPlan {
        entries: program.externs.iter().map(|(symbol, declaration)| (*symbol, Extern {
            symbol: *symbol,
            scheme: semantics.externs()[symbol].clone(),
            abi: declaration.value.abi.clone(),
            target: declaration.value.target.tracked.clone(),
            target_span: declaration.value.target.span,
            declaration_span: declaration.name_span,
        })).collect(),
    }
}
