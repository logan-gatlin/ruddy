//! Span-free, relocatable executable artifact data.
pub type Temp = u32;
pub type FuncId = u64;
pub type BlockId = u64;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub enum FieldKey {
    Named(String),
    UnnamedOperation,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lir {
    pub externs: Vec<Extern>,
    pub functions: Vec<Function>,
    pub globals: Vec<Global>,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Extern {
    pub name: String,
    pub target: String,
    pub rep: Rep,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub suspension: crate::lir::Suspension,
    pub name: String,
    pub params: Vec<Param>,
    pub continuation: Temp,
    pub entry: BlockId,
    pub blocks: Vec<Block>,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub temp: Temp,
    pub rep: Rep,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Global {
    pub adapter: Option<crate::externs::Callback>,
    pub callable: Option<crate::lir::Suspension>,
    pub name: String,
    pub initializer: FuncId,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub params: Vec<Param>,
    pub result: Option<Temp>,
    pub instrs: Vec<Instr>,
    pub end: End,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Instr {
    pub temp: Temp,
    pub rep: Rep,
    pub op: Op,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodeRef {
    pub function: FuncId,
    pub block: BlockId,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub block: BlockId,
    pub args: Vec<Temp>,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Test {
    Tag { on: Temp, name: String },
    Literal { on: Temp, value: Literal },
    Presence { on: Temp, field: String },
    Rest { on: Temp, fields: Vec<String> },
    Length { on: Temp, length: u64 },
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Callee {
    Direct(FuncId),
    Indirect(Temp),
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum End {
    Continue {
        continuation: Temp,
        value: Temp,
    },
    Jump(Edge),
    Branch {
        test: Test,
        yes: Edge,
        no: Edge,
    },
    Call {
        callee: Callee,
        args: Vec<Temp>,
        continuation: Temp,
    },
    RawCall {
        callee: Temp,
        args: Vec<Temp>,
        continuation: Temp,
        completion: crate::externs::Completion,
    },
    Enter {
        tag: Temp,
        body: Edge,
        continuation: Temp,
    },
    Leave {
        tag: Temp,
        value: Temp,
    },
    Abort {
        tag: Temp,
        value: Temp,
    },
    Unreachable,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Op {
    Callback {
        value: Temp,
        mode: crate::externs::Callback,
    },
    Continuation {
        code: CodeRef,
        captures: Vec<Temp>,
    },
    /// A primitive literal.
    Const(Literal),
    Neg(Temp),
    Not(Temp),
    And {
        left: Temp,
        right: Temp,
    },
    Or {
        left: Temp,
        right: Temp,
    },
    Xor {
        left: Temp,
        right: Temp,
    },
    Add {
        left: Temp,
        right: Temp,
    },
    Sub {
        left: Temp,
        right: Temp,
    },
    Mul {
        left: Temp,
        right: Temp,
    },
    Div {
        left: Temp,
        right: Temp,
    },
    /// A struct literal. Empty is the unit value.
    Struct(Vec<(FieldKey, Temp)>),
    /// An immutable persistent array literal.
    Array(Vec<Temp>),
    /// The arrays these temps hold, joined in order into one. Never fewer
    /// than one operand: the value of a spread literal, which is what puts
    /// arrays end to end.
    Concat(Vec<Temp>),
    /// One record carrying every field of each of these, laid over one another
    /// in order: a later one wins wherever two of them name the same field.
    ///
    /// The only way to build a record out of one whose fields are not known
    /// here — which is what a tail bundle is, and what the value a struct
    /// literal spreads is. Never fewer than two operands: laying one record
    /// over nothing is that record.
    Merge(Vec<Temp>),
    /// Read one field of a struct.
    Project {
        base: Temp,
        field: FieldKey,
    },
    /// One case of a sum. A bare case carries no payload temp.
    Tag {
        name: String,
        payload: Option<Temp>,
    },
    /// Extract a sum case's payload. Emitted by match lowering alone.
    Payload(Temp),
    /// One element of an array, counted from the front. Emitted by match
    /// lowering alone, under a length test that proved the index in range.
    Nth {
        base: Temp,
        index: u64,
    },
    /// One element of an array, counted from the back: index nought is the
    /// last. Emitted by match lowering alone, under a length test.
    NthBack {
        base: Temp,
        index: u64,
    },
    /// The elements of an array from the first `start` to the last `drop`,
    /// as an array. Emitted by match lowering alone, for a rest binder.
    Slice {
        base: Temp,
        start: u64,
        drop: u64,
    },
    /// Pair a function with the current values of its captures.
    Closure {
        func: FuncId,
        captures: Vec<Temp>,
    },
    /// Read a raw target-provided value. Raw externs live outside Ruddy's
    /// global namespace so an adapter may occupy the declaration's public name.
    Extern {
        target: String,
    },
    /// Read a top-level Ruddy definition or adapted extern value.
    Global {
        target: String,
        callable: Option<crate::lir::Suspension>,
    },
    /// Mint a fresh handler identity, once per dynamic evaluation of a `handle`.
    NewTag,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Rep {
    /// An unsigned 64-bit integer.
    Nat,
    /// A signed 64-bit integer.
    Int,
    /// A 64-bit floating-point number.
    Real,
    String,
    Boolean,
    /// The value with nothing in it: the empty struct.
    Unit,
    Struct,
    Array,
    Sum,
    /// A closure: a function paired with its captures.
    Fn,
    /// A saved destination and its explicit environment.
    Cont,
    /// A managed dynamic handler identity.
    Handler,
    /// Anything the solved type does not pin down — a quantified variable, a
    /// rigid, or a type nothing decided.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Literal {
    Natural(u64),
    Integer(i64),
    Real(u64),
    String(String),
    Boolean(bool),
}

impl Op {
    /// Values read by this instruction, in operand order.
    pub fn uses(&self) -> Vec<Temp> {
        match self {
            Self::Const(_) | Self::Extern { .. } | Self::Global { .. } | Self::NewTag => vec![],
            Self::Callback { value: v, .. } | Self::Neg(v) | Self::Not(v) | Self::Payload(v) => {
                vec![*v]
            }
            Self::And { left, right }
            | Self::Or { left, right }
            | Self::Xor { left, right }
            | Self::Add { left, right }
            | Self::Sub { left, right }
            | Self::Mul { left, right }
            | Self::Div { left, right } => vec![*left, *right],
            Self::Struct(fields) => fields.iter().map(|(_, v)| *v).collect(),
            Self::Array(v) | Self::Merge(v) | Self::Concat(v) => v.clone(),
            Self::Project { base, .. }
            | Self::Nth { base, .. }
            | Self::NthBack { base, .. }
            | Self::Slice { base, .. } => vec![*base],
            Self::Tag { payload, .. } => payload.iter().copied().collect(),
            Self::Closure { captures, .. } | Self::Continuation { captures, .. } => {
                captures.clone()
            }
        }
    }
}
impl Test {
    pub fn on(&self) -> Temp {
        match self {
            Self::Tag { on, .. }
            | Self::Literal { on, .. }
            | Self::Presence { on, .. }
            | Self::Rest { on, .. }
            | Self::Length { on, .. } => *on,
        }
    }
}
impl End {
    pub fn uses(&self) -> Vec<Temp> {
        match self {
            Self::Continue {
                continuation,
                value,
            } => vec![*continuation, *value],
            Self::Jump(e) => e.args.clone(),
            Self::Branch { test, yes, no } => std::iter::once(test.on())
                .chain(yes.args.iter().copied())
                .chain(no.args.iter().copied())
                .collect(),
            Self::Call {
                callee,
                args,
                continuation,
            } => {
                let mut v = args.clone();
                if let Callee::Indirect(c) = callee {
                    v.push(*c);
                }
                v.push(*continuation);
                v
            }
            Self::RawCall {
                callee,
                args,
                continuation,
                ..
            } => {
                let mut v = vec![*callee, *continuation];
                v.extend(args);
                v
            }
            Self::Enter {
                tag,
                body,
                continuation,
            } => {
                let mut v = vec![*tag, *continuation];
                v.extend(&body.args);
                v
            }
            Self::Leave { tag, value } | Self::Abort { tag, value } => vec![*tag, *value],
            Self::Unreachable => vec![],
        }
    }
}

/// Verify all executable references and closed block environments before a
/// linker or backend receives the artifact.
pub(super) fn validate(lir: &Lir) -> Result<(), String> {
    use std::collections::HashMap;
    let error = |message: &str| Err(message.to_owned());
    let remapped = remapped_captures(lir);
    for (function_id, f) in lir.functions.iter().enumerate() {
        let known = if f.suspension == crate::lir::Suspension::Synchronous {
            callable_proofs(lir, function_id, &remapped[function_id])
        } else {
            HashMap::new()
        };
        let Some(entry) = usize::try_from(f.entry)
            .ok()
            .and_then(|id| f.blocks.get(id))
        else {
            return error("entry outside block table");
        };
        if entry.result.is_some() {
            return error("function entry is a continuation entry");
        }
        let mut inputs: HashMap<_, _> = f.params.iter().map(|p| (p.temp, p.rep)).collect();
        if inputs.len() != f.params.len() || inputs.insert(f.continuation, Rep::Cont).is_some() {
            return error("duplicate function parameter");
        }
        for p in &entry.params {
            if inputs.get(&p.temp) != Some(&p.rep) {
                return error("unavailable function entry parameter");
            }
        }
        for b in &f.blocks {
            let mut available: HashMap<_, _> = b.params.iter().map(|p| (p.temp, p.rep)).collect();
            if available.len() != b.params.len() {
                return error("duplicate block parameter");
            }
            if let Some(result) = b.result
                && b.params.last().map(|p| p.temp) != Some(result)
            {
                return error("continuation result must be the final parameter");
            }
            for i in &b.instrs {
                if i.op.uses().iter().any(|t| !available.contains_key(t)) {
                    return error("instruction uses an unavailable temporary");
                }
                match &i.op {
                    Op::Closure { func, captures } => {
                        let Some(target) = usize::try_from(*func)
                            .ok()
                            .and_then(|id| lir.functions.get(id))
                        else {
                            return error("closure outside its function table");
                        };
                        if captures.len() > target.params.len() {
                            return error("closure captures exceed function parameters");
                        }
                        if i.rep != Rep::Fn {
                            return error("closure is not represented as a function");
                        }
                        for (param, temp) in target.params.iter().zip(captures) {
                            if !compatible(param.rep, available[temp]) {
                                return error("closure capture representation mismatch");
                            }
                        }
                    }
                    Op::Continuation { code, captures } => {
                        let Some(target) = usize::try_from(code.function)
                            .ok()
                            .and_then(|id| lir.functions.get(id))
                            .and_then(|f| {
                                usize::try_from(code.block)
                                    .ok()
                                    .and_then(|id| f.blocks.get(id))
                            })
                        else {
                            return error("continuation outside code table");
                        };
                        if target.result.is_none()
                            || target.params.len() != captures.len() + 1
                            || i.rep != Rep::Cont
                        {
                            return error("invalid continuation capture convention");
                        }
                        for (param, temp) in target.params.iter().zip(captures) {
                            if !compatible(param.rep, available[temp]) {
                                return error("continuation capture representation mismatch");
                            }
                        }
                    }
                    Op::NewTag if i.rep != Rep::Handler => {
                        return error("handler identity has the wrong representation");
                    }
                    Op::Global {
                        target,
                        callable: Some(crate::lir::Suspension::Synchronous),
                    } if lir.globals.iter().any(|g| {
                        g.name == *target && g.callable != Some(crate::lir::Suspension::Synchronous)
                    }) =>
                    {
                        return error("global read contradicts its producer's callable summary");
                    }
                    Op::Callback { value, .. }
                        if (!compatible(Rep::Fn, available[value]) || i.rep != Rep::Fn) =>
                    {
                        return error("callback operand is not a function");
                    }
                    _ => {}
                }
                if available.insert(i.temp, i.rep).is_some() {
                    return error("temporary assigned twice in a block");
                }
            }
            if b.end.uses().iter().any(|t| !available.contains_key(t)) {
                return error("terminator uses an unavailable temporary");
            }
            let check_edge = |edge: &Edge| -> Result<(), String> {
                let target = usize::try_from(edge.block)
                    .ok()
                    .and_then(|id| f.blocks.get(id))
                    .ok_or("jump outside block table")?;
                if target.params.len() != edge.args.len() {
                    return error("block argument arity mismatch");
                }
                for (param, temp) in target.params.iter().zip(&edge.args) {
                    if !compatible(param.rep, available[temp]) {
                        return error("block argument representation mismatch");
                    }
                }
                Ok(())
            };
            if let End::RawCall { completion, .. } = b.end
                && completion != crate::externs::Completion::Immediate
                && f.suspension == crate::lir::Suspension::Synchronous
            {
                return error("synchronous function contains a suspending foreign call");
            }
            match &b.end {
                End::Enter { tag, .. } | End::Leave { tag, .. } | End::Abort { tag, .. }
                    if !compatible(Rep::Handler, available[tag]) =>
                {
                    return error("handler operand is not an identity");
                }
                End::Call {
                    callee: Callee::Indirect(callee),
                    ..
                } if !compatible(Rep::Fn, available[callee]) => {
                    return error("indirect call operand is not a function");
                }
                _ => {}
            }
            if let End::Call {
                callee: Callee::Indirect(callee),
                ..
            } = b.end
                && f.suspension == crate::lir::Suspension::Synchronous
                && known.get(&callee) != Some(&crate::lir::Suspension::Synchronous)
            {
                return error("synchronous function has no proof for an indirect call");
            }
            match &b.end {
                End::Jump(edge) => check_edge(edge)?,
                End::Branch { yes, no, .. } => {
                    check_edge(yes)?;
                    check_edge(no)?;
                }
                End::Enter {
                    body, continuation, ..
                } => {
                    check_edge(body)?;
                    if available[continuation] != Rep::Cont {
                        return error("handler exit is not a continuation");
                    }
                }
                End::Call {
                    callee: Callee::Direct(id),
                    args,
                    continuation,
                } => {
                    let target = usize::try_from(*id)
                        .ok()
                        .and_then(|id| lir.functions.get(id))
                        .ok_or("call outside function table")?;
                    if args.len() != target.params.len() {
                        return error("function argument arity mismatch");
                    }
                    for (param, temp) in target.params.iter().zip(args) {
                        if !compatible(param.rep, available[temp]) {
                            return error("function argument representation mismatch");
                        }
                    }
                    if f.suspension == crate::lir::Suspension::Synchronous
                        && target.suspension == crate::lir::Suspension::MaySuspend
                    {
                        return error(
                            "synchronous function calls a potentially suspending function",
                        );
                    }
                    if available[continuation] != Rep::Cont {
                        return error("call destination is not a continuation");
                    }
                }
                End::Call { continuation, .. }
                | End::RawCall { continuation, .. }
                | End::Continue { continuation, .. }
                    if available[continuation] != Rep::Cont =>
                {
                    return error("destination is not a continuation");
                }
                _ => {}
            }
        }
    }
    for g in &lir.globals {
        if g.adapter == Some(crate::externs::Callback::Sync)
            && g.callable != Some(crate::lir::Suspension::Synchronous)
        {
            return error("synchronous export has no non-suspension proof");
        }
        if matches!(
            g.adapter,
            Some(crate::externs::Callback::Completion | crate::externs::Callback::Notification)
        ) {
            return error("invalid library export adapter");
        }
        let Some(f) = usize::try_from(g.initializer)
            .ok()
            .and_then(|id| lir.functions.get(id))
        else {
            return error("initializer outside function table");
        };
        if !f.params.is_empty() {
            return error("initializer takes visible parameters");
        }
        if let Some(summary) = g.callable {
            let target = f
                .blocks
                .first()
                .filter(|_| f.blocks.len() == 1)
                .and_then(|block| match block.end {
                    End::Continue { value, .. } => block.instrs.iter().find_map(|i| match i.op {
                        Op::Closure { func, .. } if i.temp == value => usize::try_from(func)
                            .ok()
                            .and_then(|id| lir.functions.get(id)),
                        _ => None,
                    }),
                    _ => None,
                });
            if summary == crate::lir::Suspension::Synchronous
                && target.is_none_or(|f| f.suspension != summary)
            {
                return error("callable summary has no matching non-suspension proof");
            }
        }
    }
    Ok(())
}
fn compatible(want: Rep, have: Rep) -> bool {
    want == have || want == Rep::Any || have == Rep::Any
}

/// A callable proof follows a value's identity through block environments.
/// Unknown parameters and computed results cannot certify synchronous calls.
fn callable_proofs(
    lir: &Lir,
    function_id: usize,
    remapped: &std::collections::HashSet<Temp>,
) -> std::collections::HashMap<Temp, crate::lir::Suspension> {
    use std::collections::{HashMap, HashSet};
    let f = &lir.functions[function_id];
    let mut proofs = HashMap::new();
    let mut defined = HashSet::new();
    let mut ambiguous = remapped.clone();
    ambiguous.extend(f.params.iter().map(|p| p.temp));
    for block in &f.blocks {
        ambiguous.extend(block.result);
        for i in &block.instrs {
            if !defined.insert(i.temp) {
                ambiguous.insert(i.temp);
            }
            let summary = match &i.op {
                Op::Closure { func, .. } => usize::try_from(*func)
                    .ok()
                    .and_then(|id| lir.functions.get(id))
                    .map(|f| f.suspension),
                Op::Global { callable, .. } => *callable,
                _ => None,
            };
            if let Some(summary) = summary {
                proofs.insert(i.temp, summary);
            }
        }
        let edges: Vec<_> = match &block.end {
            End::Jump(edge) => vec![edge],
            End::Branch { yes, no, .. } => vec![yes, no],
            End::Enter { body, .. } => vec![body],
            _ => vec![],
        };
        for edge in edges {
            if let Some(target) = usize::try_from(edge.block)
                .ok()
                .and_then(|id| f.blocks.get(id))
            {
                for (param, argument) in target.params.iter().zip(&edge.args) {
                    if param.temp != *argument {
                        ambiguous.insert(param.temp);
                    }
                }
            }
        }
    }
    proofs.retain(|temp, _| !ambiguous.contains(temp));
    proofs
}

/// Index incoming continuation environments once, keeping continuation verification linear in
/// the artifact size even for bundles with many small functions.
fn remapped_captures(lir: &Lir) -> Vec<std::collections::HashSet<Temp>> {
    let mut remapped = vec![std::collections::HashSet::new(); lir.functions.len()];
    for (owner, caller) in lir.functions.iter().enumerate() {
        for i in caller.blocks.iter().flat_map(|b| &b.instrs) {
            if let Op::Continuation { code, captures } = &i.op
                && let Ok(function) = usize::try_from(code.function)
                && let Some(target) = lir.functions.get(function).and_then(|f| {
                    usize::try_from(code.block)
                        .ok()
                        .and_then(|id| f.blocks.get(id))
                })
            {
                for (param, capture) in target.params.iter().zip(captures) {
                    if owner != function || param.temp != *capture {
                        remapped[function].insert(param.temp);
                    }
                }
            }
        }
    }
    remapped
}
