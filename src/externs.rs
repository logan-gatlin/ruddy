//! Target-neutral reviewed foreign-interface plans.

use indexmap::IndexMap;

use crate::{inference, ir, symbol::Symbol, tracking::Anchor, types::Ty};

/// The reviewed foreign declarations later lowering may absorb without another
/// inference walk.
#[derive(Debug, Clone)]
pub struct ExternPlan {
    entries: IndexMap<Symbol, Extern>,
}

#[derive(Debug, Clone)]
pub struct Extern {
    pub symbol: Symbol,
    /// The semantic type the adapter implements. Its scheme was instantiated
    /// during review, so lowering never needs to interpret a scheme here.
    pub ty: std::rc::Rc<crate::types::Ty>,
    /// The complete target-neutral conversion decision, made after inference.
    pub conversion: Conversion,
    pub target: String,
    pub target_at: Anchor,
    pub declaration_at: Anchor,
}

/// How one value crosses an extern boundary. This is deliberately independent
/// of IR's written ABI tree: lowering consumes this reviewed adapter behavior,
/// not an ABI it would have to re-interpret.
#[derive(Debug, Clone)]
pub enum Conversion {
    Value,
    OrdinaryFunction,
    MarkedFunction {
        parameters: Vec<Conversion>,
        result: Box<Conversion>,
        nullary: bool,
    },
}

impl ExternPlan {
    pub fn get(&self, symbol: Symbol) -> &Extern {
        &self.entries[&symbol]
    }
    pub fn iter(&self) -> impl Iterator<Item = &Extern> {
        self.entries.values()
    }
}

/// Turn inference-reviewed extern facts into the in-memory lowering plan.
/// This is infallible: accepted inference has already established ABI facts.
pub(crate) fn plan(semantics: &inference::Semantics) -> ExternPlan {
    ExternPlan {
        entries: semantics
            .reviewed_externs()
            .iter()
            .map(|(symbol, reviewed)| {
                (
                    *symbol,
                    Extern {
                        symbol: *symbol,
                        ty: reviewed.scheme.body().clone(),
                        conversion: conversion(
                            &reviewed.abi,
                            reviewed.scheme.body(),
                            semantics.aliases(),
                        ),
                        target: reviewed.target.clone(),
                        target_at: reviewed.target_span,
                        declaration_at: reviewed.declaration_span,
                    },
                )
            })
            .collect(),
    }
}

fn conversion(
    abi: &ir::ExternType,
    ty: &std::rc::Rc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> Conversion {
    match &abi.anchored {
        ir::ExternTypeKind::Group(inner) => conversion(inner, ty, aliases),
        ir::ExternTypeKind::Ordinary(_) => match &*exposed(ty, aliases) {
            Ty::Arrow(..) => Conversion::OrdinaryFunction,
            _ => Conversion::Value,
        },
        ir::ExternTypeKind::Function {
            parameters, result, ..
        } => {
            let mut cursor = ty.clone();
            let parameter_conversions = parameters
                .iter()
                .map(|parameter| {
                    let (from, to) = arrow(&cursor, aliases);
                    cursor = to;
                    conversion(parameter, &from, aliases)
                })
                .collect();
            // A nullary host function still corresponds to the one unit arrow.
            if parameters.is_empty() {
                let (_, to) = arrow(&cursor, aliases);
                cursor = to;
            }
            Conversion::MarkedFunction {
                parameters: parameter_conversions,
                result: Box::new(conversion(result, &cursor, aliases)),
                nullary: parameters.is_empty(),
            }
        }
    }
}

fn exposed(
    ty: &std::rc::Rc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> std::rc::Rc<Ty> {
    let mut exposed = inference::unfold(aliases, ty);
    while let Ty::Package(body) = &*exposed {
        exposed = inference::unfold(aliases, body);
    }
    exposed
}

fn arrow(
    ty: &std::rc::Rc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> (std::rc::Rc<Ty>, std::rc::Rc<Ty>) {
    let exposed = exposed(ty, aliases);
    let Ty::Arrow(from, to, _) = &*exposed else {
        panic!("reviewed extern ABI and semantic type agree on function shape")
    };
    (from.clone(), to.clone())
}
