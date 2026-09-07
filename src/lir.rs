//! Portable continuation-passing LIR, constructed independently for each bundle.
//! Functions own parameterized blocks. Calls carry continuations; linking only
//! relocates function identities. Host storage and dispatch belong to backends.

mod control;
mod lower;
mod suspension;

use crate::{compile::AcceptedProgram, ir::Literal, symbol::Symbol, tracking::Span};
use indexmap::IndexMap;

pub type Temp = u32;
pub type FuncId = usize;
pub type BlockId = usize;

/// Independent of source types and domain effect rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Suspension {
    Synchronous,
    MaySuspend,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldKey {
    Named(String),
    UnnamedOperation,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub externs: Vec<Extern>,
    pub functions: Vec<Function>,
    pub globals: Vec<Global>,
}
#[derive(Debug, Clone)]
pub struct Extern {
    pub symbol: Symbol,
    pub name: String,
    pub target: String,
    pub span: Span,
    pub rep: Rep,
}
#[derive(Debug, Clone)]
pub struct Function {
    pub suspension: Suspension,
    pub name: String,
    pub params: Vec<Param>,
    pub continuation: Temp,
    pub entry: BlockId,
    pub blocks: Vec<Block>,
    pub span: Span,
}
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub temp: Temp,
    pub rep: Rep,
}
#[derive(Debug, Clone)]
pub struct Global {
    pub adapter: Option<crate::externs::Callback>,
    pub callable: Option<Suspension>,
    pub symbol: Symbol,
    pub name: String,
    pub initializer: FuncId,
    pub span: Span,
}
#[derive(Debug, Clone)]
pub struct Block {
    pub params: Vec<Param>,
    pub result: Option<Temp>,
    pub instrs: Vec<Instr>,
    pub end: Terminator,
}
#[derive(Debug, Clone)]
pub struct Instr {
    pub temp: Temp,
    pub rep: Rep,
    pub span: Span,
    pub op: Op,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeRef {
    pub function: FuncId,
    pub block: BlockId,
}
#[derive(Debug, Clone)]
pub struct Edge {
    pub block: BlockId,
    pub args: Vec<Temp>,
}
#[derive(Debug, Clone)]
pub enum Test {
    Tag { on: Temp, name: String },
    Literal { on: Temp, value: Literal },
    Presence { on: Temp, field: String },
    Rest { on: Temp, fields: Vec<String> },
    Length { on: Temp, length: usize },
}
#[derive(Debug, Clone, Copy)]
pub enum Callee {
    Direct(FuncId),
    Indirect(Temp),
}
#[derive(Debug, Clone)]
pub struct Terminator {
    pub span: Span,
    pub kind: End,
}
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
    Allocate(Temp),
    Read(Temp),
    Write {
        left: Temp,
        right: Temp,
    },
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
    Struct(IndexMap<FieldKey, Temp>),
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
        index: usize,
    },
    /// One element of an array, counted from the back: index nought is the
    /// last. Emitted by match lowering alone, under a length test.
    NthBack {
        base: Temp,
        index: usize,
    },
    /// The elements of an array from the first `start` to the last `drop`,
    /// as an array. Emitted by match lowering alone, for a rest binder.
    Slice {
        base: Temp,
        start: usize,
        drop: usize,
    },
    /// Pair a function with the current values of its captures.
    Closure {
        func: FuncId,
        captures: Vec<Temp>,
    },
    /// Read a raw target-provided value. Raw externs live outside Ruddy's
    /// global namespace so an adapter may occupy the declaration's public name.
    Extern {
        symbol: Symbol,
        name: String,
    },
    /// Read a top-level Ruddy definition or adapted extern value.
    Global {
        symbol: Symbol,
        name: String,
        /// Callable proof supplied by this bundle or a dependency producer.
        callable: Option<Suspension>,
    },
    /// Mint a fresh handler identity, once per dynamic evaluation of a `handle`.
    NewTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rep {
    /// A target-sized natural number.
    Nat,
    /// A target-sized signed integer.
    Int,
    Fixed(crate::types::FixedInt),
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

pub fn lower(accepted: &AcceptedProgram) -> Output {
    let mut output = control::lower(lower::lower(accepted));
    suspension::summarize(&mut output, accepted);
    output
}

impl Op {
    /// Values read by this instruction, in operand order.
    pub fn uses(&self) -> Vec<Temp> {
        match self {
            Self::Const(_) | Self::Extern { .. } | Self::Global { .. } | Self::NewTag => vec![],
            Self::Callback { value: v, .. }
            | Self::Neg(v)
            | Self::Not(v)
            | Self::Allocate(v)
            | Self::Read(v)
            | Self::Payload(v) => {
                vec![*v]
            }
            Self::And { left, right }
            | Self::Or { left, right }
            | Self::Xor { left, right }
            | Self::Write { left, right }
            | Self::Add { left, right }
            | Self::Sub { left, right }
            | Self::Mul { left, right }
            | Self::Div { left, right } => vec![*left, *right],
            Self::Struct(fields) => fields.values().copied().collect(),
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
