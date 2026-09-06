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
    Callback,
}
/// A fixed contract for results delivered to JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Callback {
    Sync,
    Promise,
    Completion,
    Notification,
}

/// Reviewed metadata follows the written marked-function ABI tree.
#[derive(Debug, Clone, Default)]
pub(crate) struct Protocol {
    at: Anchor,
    completion: Option<Completion>,
    callback: Option<Callback>,
    parameters: Option<Vec<Protocol>>,
    result: Option<Box<Protocol>>,
}

pub(crate) fn protocol(metadata: &ir::Metadata) -> Result<Option<Protocol>, ir::Error> {
    let Some(attribute) = metadata.get("ffi") else {
        return Ok(None);
    };
    read_protocol(&attribute.value).map(Some)
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
fn read_protocol(data: &ir::Data) -> Result<Protocol, ir::Error> {
    let ir::DataKind::Struct(fields) = &data.anchored else {
        return Err(invalid(
            data.at,
            "`@ffi` needs a record of boundary protocols",
        ));
    };
    let mut protocol = Protocol {
        at: data.at,
        ..Protocol::default()
    };
    for (key, field) in fields {
        let value = &field.value;
        match key.as_str() {
            "completion" => {
                protocol.completion = Some(match &value.anchored {
                    ir::DataKind::String(s) if s == "immediate" => Completion::Immediate,
                    ir::DataKind::String(s) if s == "promise" => Completion::Promise,
                    ir::DataKind::String(s) if s == "callback" => Completion::Callback,
                    _ => {
                        return Err(invalid(
                            value.at,
                            "completion must be \"immediate\", \"promise\", or \"callback\"",
                        ));
                    }
                })
            }
            "callback" => {
                protocol.callback = Some(match &value.anchored {
                    ir::DataKind::String(s) if s == "sync" => Callback::Sync,
                    ir::DataKind::String(s) if s == "promise" => Callback::Promise,
                    ir::DataKind::String(s) if s == "completion" => Callback::Completion,
                    ir::DataKind::String(s) if s == "notification" => Callback::Notification,
                    _ => {
                        return Err(invalid(
                            value.at,
                            "callback must be \"sync\", \"promise\", \"completion\", or \"notification\"",
                        ));
                    }
                })
            }
            "parameters" => {
                let ir::DataKind::Array(values) = &value.anchored else {
                    return Err(invalid(
                        value.at,
                        "parameters must be an array of protocol records",
                    ));
                };
                protocol.parameters =
                    Some(values.iter().map(read_protocol).collect::<Result<_, _>>()?);
            }
            "result" => protocol.result = Some(Box::new(read_protocol(value)?)),
            _ => {
                return Err(invalid(
                    value.at,
                    format!("unknown foreign protocol field `{key}`"),
                ));
            }
        }
    }
    Ok(protocol)
}
pub(crate) fn review(semantics: &inference::Semantics) -> Vec<ir::Error> {
    semantics
        .reviewed_externs()
        .values()
        .filter_map(|reviewed| {
            let protocol = reviewed.protocol.as_ref()?;
            let conversion = conversion(&reviewed.abi, reviewed.scheme.body(), semantics.aliases());
            check_protocol(&conversion, protocol, false).err()
        })
        .collect()
}
fn check_protocol(
    conversion: &Conversion,
    protocol: &Protocol,
    callback: bool,
) -> Result<(), ir::Error> {
    if protocol.completion.is_some() && callback {
        return Err(invalid(
            protocol.at,
            "use `callback` to choose how this callback delivers its result",
        ));
    }
    if protocol.callback.is_some() && !callback {
        return Err(invalid(
            protocol.at,
            "use `completion` to choose how this host function completes",
        ));
    }
    match conversion {
        Conversion::Value => {
            if protocol.completion.is_some()
                || protocol.callback.is_some()
                || protocol.parameters.is_some()
                || protocol.result.is_some()
            {
                return Err(invalid(protocol.at, "foreign protocols apply to functions"));
            }
        }
        Conversion::MarkedFunction {
            parameters, result, ..
        } => {
            if let Some(protocols) = &protocol.parameters {
                if protocols.len() != parameters.len() {
                    return Err(invalid(
                        protocol.at,
                        "protocol parameters must match the marked function's arity",
                    ));
                }
                for (conversion, protocol) in parameters.iter().zip(protocols) {
                    check_protocol(conversion, protocol, !callback)?;
                }
            }
            if let Some(protocol) = &protocol.result {
                check_protocol(result, protocol, callback)?;
            }
        }
        Conversion::OrdinaryFunction => {
            if protocol.parameters.is_some() || protocol.result.is_some() {
                return Err(invalid(
                    protocol.at,
                    "write a marked `fn(...)` ABI to specify nested boundary protocols",
                ));
            }
        }
        Conversion::Protocol { .. } => unreachable!("protocol review precedes planning"),
    }
    Ok(())
}

fn apply_protocol(mut conversion: Conversion, protocol: &Protocol) -> Conversion {
    if let Conversion::MarkedFunction {
        parameters, result, ..
    } = &mut conversion
    {
        if let Some(protocols) = &protocol.parameters {
            for (parameter, protocol) in parameters.iter_mut().zip(protocols) {
                *parameter = apply_protocol(parameter.clone(), protocol);
            }
        }
        if let Some(protocol) = &protocol.result {
            **result = apply_protocol(*result.clone(), protocol);
        }
    }
    if protocol.completion.is_none() && protocol.callback.is_none() {
        return conversion;
    }
    Conversion::Protocol {
        inner: Box::new(conversion),
        completion: protocol.completion.unwrap_or(Completion::Immediate),
        callback: protocol.callback.unwrap_or(Callback::Sync),
    }
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
                        conversion: {
                            let conversion = conversion(
                                &reviewed.abi,
                                reviewed.scheme.body(),
                                semantics.aliases(),
                            );
                            match &reviewed.protocol {
                                Some(protocol) => apply_protocol(conversion, protocol),
                                None => conversion,
                            }
                        },
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
