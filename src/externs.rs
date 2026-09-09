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
    pub ty: std::sync::Arc<crate::types::Ty>,
    /// The complete target-neutral conversion decision, made after inference.
    pub conversion: Conversion,
    pub intrinsic: Option<crate::reification::Intrinsic>,
    pub target: String,
    pub target_at: Anchor,
    pub declaration_at: Anchor,
}

/// How one value crosses an extern boundary. This is deliberately independent
/// of IR's written ABI tree: lowering consumes this reviewed adapter behavior,
/// not an ABI it would have to re-interpret.
#[derive(Debug, Clone)]
pub enum Conversion {
    Protocol {
        inner: Box<Conversion>,
        completion: Completion,
        callback: Callback,
    },
    Value,
    OrdinaryFunction,
    MarkedFunction {
        parameters: Vec<Conversion>,
        result: Box<Conversion>,
        nullary: bool,
    },
}

/// Completion timing belongs to the foreign boundary, independently of source types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Completion {
    Immediate,
    Promise,
}
/// A fixed contract for results delivered to JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Callback {
    Sync,
    Promise,
}

/// A JS-facing export may request a fixed contract without changing its source type.
pub(crate) fn export_request(metadata: &ir::Metadata) -> Result<Option<Callback>, ir::Error> {
    let Some(attribute) = metadata.get("export") else {
        return Ok(None);
    };
    match &attribute.value.anchored {
        ir::DataKind::String(s) if s == "sync" => Ok(Some(Callback::Sync)),
        ir::DataKind::String(s) if s == "promise" => Ok(Some(Callback::Promise)),
        _ => Err(invalid(
            attribute.value.at,
            "`@export` must be \"sync\" or \"promise\"",
        )),
    }
}
pub(crate) fn check_exports(
    accepted: &crate::compile::AcceptedProgram,
    output: &crate::lir::Output,
) -> Vec<ir::Error> {
    output
        .globals
        .iter()
        .filter_map(|g| {
            let declaration = accepted.ir().terms.get(&g.symbol)?;
            let attribute = declaration.metadata.get("export")?;
            if g.callable.is_none() {
                return Some(invalid(
                    attribute.key_at,
                    "this export is not a statically known function",
                ));
            }
            if g.adapter == Some(Callback::Sync)
                && g.callable != Some(crate::lir::Suspension::Synchronous)
            {
                return Some(invalid(
                    attribute.key_at,
                    "this function may suspend; use a Promise export or remove `@export`",
                ));
            }
            None
        })
        .collect()
}

fn invalid(at: Anchor, message: impl Into<String>) -> ir::Error {
    ir::Error {
        at,
        kind: ir::ErrorKind::ForeignProtocol {
            message: message.into(),
        },
    }
}
pub(crate) fn review(semantics: &inference::Semantics) -> Vec<ir::Error> {
    semantics
        .reviewed_externs()
        .values()
        .filter_map(|reviewed| {
            conversion(&reviewed.abi, reviewed.scheme.body(), semantics.aliases()).err()
        })
        .collect()
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
                        intrinsic: crate::reification::Intrinsic::recognize(
                            &reviewed.target,
                            reviewed.scheme.body(),
                            semantics.aliases(),
                        ),
                        conversion: conversion(
                            &reviewed.abi,
                            reviewed.scheme.body(),
                            semantics.aliases(),
                        )
                        .expect("accepted extern boundary was reviewed"),
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
    ty: &std::sync::Arc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> Result<Conversion, ir::Error> {
    Ok(match &abi.anchored {
        ir::ExternTypeKind::Annotated { metadata, inner } => {
            for (key, attribute) in metadata {
                if key != "async" {
                    return Err(invalid(
                        attribute.key_at,
                        format!(
                            "unsupported extern type attribute `@{key}`; use `@async` on a function boundary"
                        ),
                    ));
                }
                if !matches!(&attribute.value.anchored, ir::DataKind::Struct(fields) if fields.is_empty())
                {
                    return Err(invalid(
                        attribute.value.at,
                        "`@async` is a tag; omit its value",
                    ));
                }
                if !matches!(&*exposed(ty, aliases), Ty::Arrow(..)) {
                    return Err(invalid(
                        attribute.key_at,
                        "`@async` applies to a function boundary, not its result value",
                    ));
                }
            }
            let inner = conversion(inner, ty, aliases)?;
            if metadata.contains_key("async") && !matches!(inner, Conversion::Protocol { .. }) {
                Conversion::Protocol {
                    inner: Box::new(inner),
                    completion: Completion::Promise,
                    callback: Callback::Promise,
                }
            } else {
                inner
            }
        }
        ir::ExternTypeKind::Group(inner) => conversion(inner, ty, aliases)?,
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
                .collect::<Result<_, _>>()?;
            // A nullary host function still corresponds to the one unit arrow.
            if parameters.is_empty() {
                let (_, to) = arrow(&cursor, aliases);
                cursor = to;
            }
            Conversion::MarkedFunction {
                parameters: parameter_conversions,
                result: Box::new(conversion(result, &cursor, aliases)?),
                nullary: parameters.is_empty(),
            }
        }
    })
}

fn exposed(
    ty: &std::sync::Arc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> std::sync::Arc<Ty> {
    let mut exposed = inference::unfold(aliases, ty);
    while let Ty::Package(body) = &*exposed {
        exposed = inference::unfold(aliases, body);
    }
    exposed
}

fn arrow(
    ty: &std::sync::Arc<Ty>,
    aliases: &indexmap::IndexMap<Symbol, crate::types::Scheme>,
) -> (std::sync::Arc<Ty>, std::sync::Arc<Ty>) {
    let exposed = exposed(ty, aliases);
    let Ty::Arrow(from, to, _) = &*exposed else {
        panic!("reviewed extern ABI and semantic type agree on function shape")
    };
    (from.clone(), to.clone())
}
