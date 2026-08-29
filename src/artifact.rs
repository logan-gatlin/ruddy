//! A portable, span-free bundle artifact.
//!
//! Artifacts are the compiler's disk boundary.  They intentionally contain no
//! source locations, file paths, or [`crate::symbol::Symbol`]s: external value
//! references are qualified by the identity of the bundle that owns them.
//! [`parse`] accepts only compiler-produced text and deliberately panics for
//! malformed input. Use [`try_parse`] at trust boundaries. This is an internal
//! v1 format, not a compatibility promise.

use std::{error::Error, fmt};

use crate::{
    inference, ir, lir,
    symbol::{Mint, Symbol},
    types,
};

/// An error encountered while parsing artifact text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    message: String,
    offset: Option<usize>,
}

impl ParseError {
    fn syntax(message: impl Into<String>, offset: usize) -> Self {
        Self {
            message: message.into(),
            offset: Some(offset),
        }
    }

    fn structure(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            offset: None,
        }
    }

    /// A description of the malformed input.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The byte offset for syntax errors, when one is available.
    pub fn offset(&self) -> Option<usize> {
        self.offset
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(formatter, "{} at byte {offset}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl Error for ParseError {}

/// A complete, serializable bundle artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub header: Header,
    pub lir: Lir,
}

impl Drop for Artifact {
    fn drop(&mut self) {
        text::discard_artifact(self);
    }
}

/// The public interface of one bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub identity: Identity,
    /// The bundles this artifact depends on.
    pub dependencies: Vec<Dependency>,
    /// Every source-addressable top-level value exported by the bundle. Externs
    /// precede `let`s; each kind retains its source declaration order. Hidden
    /// definitions generated for patterns such as `let _` are not exported.
    pub values: Vec<Value>,
    /// Every declared type, in source declaration order.
    pub types: Vec<DeclaredType>,
    /// Every declared effect, in source declaration order.
    pub effects: Vec<DeclaredEffect>,
}

/// The identity that owns an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub version: String,
}

/// The identity of one bundle this artifact depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub version: String,
}

/// A globally addressable declaration.  Its spelling is
/// `bundle@version::module::name`.
pub type QualifiedName = String;

/// One exported value and its normalized semantic scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub name: QualifiedName,
    pub scheme: Scheme,
}

/// A declared type and the semantics of its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredType {
    pub name: QualifiedName,
    pub params: Vec<Parameter>,
    pub scheme: Scheme,
}

/// The semantic role of a declared type parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub sense: Sense,
    pub lacks: Vec<String>,
    pub relevant: bool,
}

/// The role a parameter has in its declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sense {
    Type,
    Fields,
    Cases,
    Effects,
}

/// A declared effect and its semantic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredEffect {
    pub name: QualifiedName,
    /// `None` for an alias: aliases expand to other effects and do not name a
    /// row label of their own.
    pub identity: Option<EffectIdentity>,
    pub kind: EffectKind,
}

/// The structural identity used by semantic effect rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIdentity {
    pub name: String,
    pub interface: String,
}

/// An effect's operations, or the effects an alias expands to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectKind {
    Operations(Vec<Operation>),
    Alias(Vec<QualifiedName>),
}

/// One operation's public signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub selector: OperationSelector,
    pub from: Type,
    pub to: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationSelector {
    Unnamed,
    Named(String),
}

/// A normalized scheme.  Quantifier positions use the compiler's one shared
/// index space: presences are `0..presences`, then types and rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    pub count: u32,
    pub presences: u32,
    pub formula: Formula,
    pub body: Type,
}

/// A normalized semantic type. Structural fields are representable only by
/// the `Struct` constructor.
#[derive(Debug)]
pub enum Type {
    Nat,
    Int,
    Real,
    String,
    Boolean,
    Arrow(Box<Type>, Box<Type>, Row),
    Struct(Row),
    Sum(Row),
    Var(u32),
    Bound(u32),
    Rigid {
        id: u32,
        name: String,
    },
    Named {
        name: QualifiedName,
        args: Vec<Type>,
    },
    Undecided,
}

/// A normalized sum or effect row.
#[derive(Debug)]
pub struct Row {
    pub labels: Vec<(String, RowField)>,
    pub rest: Rest,
}

/// The part of a sum or effect row beyond its named labels.
#[derive(Debug, Clone)]
pub enum Rest {
    Closed,
    Var(u32),
    Bound(u32),
    Rigid { id: u32, name: String },
    Undecided,
    More(Box<Row>),
}

/// One row label and its (possibly conditional) payload type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowField {
    pub presence: Presence,
    pub ty: Type,
}

/// Whether a structural label is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
    Var(u32),
    Bound(u32),
    Undecided,
}

/// A propositional constraint over presence variables.
#[derive(Debug)]
pub enum Formula {
    True,
    False,
    Var(u32),
    Bound(u32),
    Not(Box<Formula>),
    And(Box<Formula>, Box<Formula>),
    Or(Box<Formula>, Box<Formula>),
    Iff(Box<Formula>, Box<Formula>),
    Xor(Box<Formula>, Box<Formula>),
}

// These values are part of the public artifact schema, so callers can own and
// destroy and compare them independently of `Artifact`. Keep their recursive
// ownership and equality walks on explicit heap stacks: real generated schemes
// can be tens of thousands of constructors deep.
enum SemanticRef<'a> {
    Type(&'a Type),
    Row(&'a Row),
}

enum SemanticPair<'a> {
    Type(&'a Type, &'a Type),
    Row(&'a Row, &'a Row),
    Rest(&'a Rest, &'a Rest),
}

fn semantic_eq(root: SemanticPair<'_>) -> bool {
    let mut pending = vec![root];
    while let Some(pair) = pending.pop() {
        match pair {
            SemanticPair::Type(left, right) => match (left, right) {
                (Type::Nat, Type::Nat)
                | (Type::Int, Type::Int)
                | (Type::Real, Type::Real)
                | (Type::String, Type::String)
                | (Type::Boolean, Type::Boolean)
                | (Type::Undecided, Type::Undecided) => {}
                (Type::Var(left), Type::Var(right)) | (Type::Bound(left), Type::Bound(right))
                    if left == right => {}
                (
                    Type::Rigid {
                        id: left_id,
                        name: left_name,
                    },
                    Type::Rigid {
                        id: right_id,
                        name: right_name,
                    },
                ) if left_id == right_id && left_name == right_name => {}
                (
                    Type::Arrow(left_from, left_to, left_effects),
                    Type::Arrow(right_from, right_to, right_effects),
                ) => {
                    pending.push(SemanticPair::Row(left_effects, right_effects));
                    pending.push(SemanticPair::Type(left_to, right_to));
                    pending.push(SemanticPair::Type(left_from, right_from));
                }
                (Type::Struct(left), Type::Struct(right)) | (Type::Sum(left), Type::Sum(right)) => {
                    pending.push(SemanticPair::Row(left, right));
                }
                (
                    Type::Named {
                        name: left_name,
                        args: left_args,
                    },
                    Type::Named {
                        name: right_name,
                        args: right_args,
                    },
                ) if left_name == right_name && left_args.len() == right_args.len() => {
                    pending.extend(
                        left_args
                            .iter()
                            .zip(right_args)
                            .rev()
                            .map(|(left, right)| SemanticPair::Type(left, right)),
                    );
                }
                _ => return false,
            },
            SemanticPair::Row(left, right) => {
                if left.labels.len() != right.labels.len() {
                    return false;
                }
                for ((left_name, left_field), (right_name, right_field)) in
                    left.labels.iter().zip(&right.labels)
                {
                    if left_name != right_name || left_field.presence != right_field.presence {
                        return false;
                    }
                    pending.push(SemanticPair::Type(&left_field.ty, &right_field.ty));
                }
                pending.push(SemanticPair::Rest(&left.rest, &right.rest));
            }
            SemanticPair::Rest(left, right) => match (left, right) {
                (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => {}
                (Rest::Var(left), Rest::Var(right)) | (Rest::Bound(left), Rest::Bound(right))
                    if left == right => {}
                (
                    Rest::Rigid {
                        id: left_id,
                        name: left_name,
                    },
                    Rest::Rigid {
                        id: right_id,
                        name: right_name,
                    },
                ) if left_id == right_id && left_name == right_name => {}
                (Rest::More(left), Rest::More(right)) => {
                    pending.push(SemanticPair::Row(left, right));
                }
                _ => return false,
            },
        }
    }
    true
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        semantic_eq(SemanticPair::Type(self, other))
    }
}

impl Eq for Type {}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        semantic_eq(SemanticPair::Row(self, other))
    }
}

impl Eq for Row {}

impl PartialEq for Rest {
    fn eq(&self, other: &Self) -> bool {
        semantic_eq(SemanticPair::Rest(self, other))
    }
}

impl Eq for Rest {}

impl PartialEq for Formula {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (Formula::True, Formula::True) | (Formula::False, Formula::False) => {}
                (Formula::Var(left), Formula::Var(right))
                | (Formula::Bound(left), Formula::Bound(right))
                    if left == right => {}
                (Formula::Not(left), Formula::Not(right)) => pending.push((left, right)),
                (Formula::And(left_a, left_b), Formula::And(right_a, right_b))
                | (Formula::Or(left_a, left_b), Formula::Or(right_a, right_b))
                | (Formula::Iff(left_a, left_b), Formula::Iff(right_a, right_b))
                | (Formula::Xor(left_a, left_b), Formula::Xor(right_a, right_b)) => {
                    pending.push((left_b, right_b));
                    pending.push((left_a, right_a));
                }
                _ => return false,
            }
        }
        true
    }
}

impl Eq for Formula {}

enum CloneWork<'a> {
    Semantic(SemanticRef<'a>),
    Arrow,
    Struct,
    Sum,
    Named {
        name: String,
        count: usize,
    },
    FinishRow {
        labels: Vec<(String, Presence)>,
        rest: Option<Rest>,
    },
}

fn clone_semantic(root: SemanticRef<'_>) -> (Vec<Type>, Vec<Row>) {
    let mut work = vec![CloneWork::Semantic(root)];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            CloneWork::Semantic(SemanticRef::Type(value)) => match value {
                Type::Nat => types.push(Type::Nat),
                Type::Int => types.push(Type::Int),
                Type::Real => types.push(Type::Real),
                Type::String => types.push(Type::String),
                Type::Boolean => types.push(Type::Boolean),
                Type::Arrow(from, to, effects) => {
                    work.push(CloneWork::Arrow);
                    work.push(CloneWork::Semantic(SemanticRef::Row(effects)));
                    work.push(CloneWork::Semantic(SemanticRef::Type(to)));
                    work.push(CloneWork::Semantic(SemanticRef::Type(from)));
                }
                Type::Struct(row) => {
                    work.push(CloneWork::Struct);
                    work.push(CloneWork::Semantic(SemanticRef::Row(row)));
                }
                Type::Sum(row) => {
                    work.push(CloneWork::Sum);
                    work.push(CloneWork::Semantic(SemanticRef::Row(row)));
                }
                Type::Var(value) => types.push(Type::Var(*value)),
                Type::Bound(value) => types.push(Type::Bound(*value)),
                Type::Rigid { id, name } => types.push(Type::Rigid {
                    id: *id,
                    name: name.clone(),
                }),
                Type::Named { name, args } => {
                    work.push(CloneWork::Named {
                        name: name.clone(),
                        count: args.len(),
                    });
                    work.extend(
                        args.iter()
                            .rev()
                            .map(|arg| CloneWork::Semantic(SemanticRef::Type(arg))),
                    );
                }
                Type::Undecided => types.push(Type::Undecided),
            },
            CloneWork::Semantic(SemanticRef::Row(value)) => {
                let labels = value
                    .labels
                    .iter()
                    .map(|(name, field)| (name.clone(), field.presence.clone()))
                    .collect();
                let rest = match &value.rest {
                    Rest::Closed => Some(Rest::Closed),
                    Rest::Var(value) => Some(Rest::Var(*value)),
                    Rest::Bound(value) => Some(Rest::Bound(*value)),
                    Rest::Rigid { id, name } => Some(Rest::Rigid {
                        id: *id,
                        name: name.clone(),
                    }),
                    Rest::Undecided => Some(Rest::Undecided),
                    Rest::More(_) => None,
                };
                work.push(CloneWork::FinishRow { labels, rest });
                if let Rest::More(more) = &value.rest {
                    work.push(CloneWork::Semantic(SemanticRef::Row(more)));
                }
                work.extend(
                    value
                        .labels
                        .iter()
                        .rev()
                        .map(|(_, field)| CloneWork::Semantic(SemanticRef::Type(&field.ty))),
                );
            }
            CloneWork::Arrow => {
                let effects = rows.pop().expect("cloned arrow effects");
                let to = types.pop().expect("cloned arrow result");
                let from = types.pop().expect("cloned arrow parameter");
                types.push(Type::Arrow(Box::new(from), Box::new(to), effects));
            }
            CloneWork::Struct => {
                types.push(Type::Struct(rows.pop().expect("cloned struct row")));
            }
            CloneWork::Sum => {
                types.push(Type::Sum(rows.pop().expect("cloned sum row")));
            }
            CloneWork::Named { name, count } => {
                let split = types.len() - count;
                let args = types.drain(split..).collect();
                types.push(Type::Named { name, args });
            }
            CloneWork::FinishRow { labels, rest } => {
                let mut cloned = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    cloned.push((
                        name,
                        RowField {
                            presence,
                            ty: types.pop().expect("cloned row field"),
                        },
                    ));
                }
                cloned.reverse();
                let rest = match rest {
                    Some(rest) => rest,
                    None => Rest::More(Box::new(rows.pop().expect("cloned row rest"))),
                };
                rows.push(Row {
                    labels: cloned,
                    rest,
                });
            }
        }
    }
    (types, rows)
}

impl Clone for Type {
    fn clone(&self) -> Self {
        let (mut types, rows) = clone_semantic(SemanticRef::Type(self));
        debug_assert!(rows.is_empty());
        types.pop().expect("cloned type")
    }
}

impl Clone for Row {
    fn clone(&self) -> Self {
        let (types, mut rows) = clone_semantic(SemanticRef::Row(self));
        debug_assert!(types.is_empty());
        rows.pop().expect("cloned row")
    }
}

enum SemanticOwned {
    Type(Type),
    Row(Row),
}

fn empty_row() -> Row {
    Row {
        labels: Vec::new(),
        rest: Rest::Closed,
    }
}

fn drain_type(value: &mut Type, pending: &mut Vec<SemanticOwned>) {
    match value {
        Type::Arrow(from, to, effects) => {
            pending.push(SemanticOwned::Type(std::mem::replace(
                from.as_mut(),
                Type::Undecided,
            )));
            pending.push(SemanticOwned::Type(std::mem::replace(
                to.as_mut(),
                Type::Undecided,
            )));
            pending.push(SemanticOwned::Row(std::mem::replace(effects, empty_row())));
        }
        Type::Struct(row) | Type::Sum(row) => {
            pending.push(SemanticOwned::Row(std::mem::replace(row, empty_row())));
        }
        Type::Named { args, .. } => {
            pending.extend(std::mem::take(args).into_iter().map(SemanticOwned::Type))
        }
        Type::Nat
        | Type::Int
        | Type::Real
        | Type::String
        | Type::Boolean
        | Type::Var(_)
        | Type::Bound(_)
        | Type::Rigid { .. }
        | Type::Undecided => {}
    }
}

fn drain_row(value: &mut Row, pending: &mut Vec<SemanticOwned>) {
    pending.extend(
        std::mem::take(&mut value.labels)
            .into_iter()
            .map(|(_, field)| SemanticOwned::Type(field.ty)),
    );
    if let Rest::More(more) = &mut value.rest {
        pending.push(SemanticOwned::Row(std::mem::replace(
            more.as_mut(),
            empty_row(),
        )));
    }
}

fn discard_semantic(root: SemanticRefMut<'_>) {
    let mut pending = Vec::new();
    match root {
        SemanticRefMut::Type(value) => drain_type(value, &mut pending),
        SemanticRefMut::Row(value) => drain_row(value, &mut pending),
    }
    while let Some(mut value) = pending.pop() {
        match &mut value {
            SemanticOwned::Type(value) => drain_type(value, &mut pending),
            SemanticOwned::Row(value) => drain_row(value, &mut pending),
        }
    }
}

enum SemanticRefMut<'a> {
    Type(&'a mut Type),
    Row(&'a mut Row),
}

impl Drop for Type {
    fn drop(&mut self) {
        discard_semantic(SemanticRefMut::Type(self));
    }
}

impl Drop for Row {
    fn drop(&mut self) {
        discard_semantic(SemanticRefMut::Row(self));
    }
}

impl Clone for Formula {
    fn clone(&self) -> Self {
        enum Work<'a> {
            Formula(&'a Formula),
            Not,
            Pair(u8),
        }

        let mut work = vec![Work::Formula(self)];
        let mut out = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Formula(Formula::True) => out.push(Formula::True),
                Work::Formula(Formula::False) => out.push(Formula::False),
                Work::Formula(Formula::Var(value)) => out.push(Formula::Var(*value)),
                Work::Formula(Formula::Bound(value)) => out.push(Formula::Bound(*value)),
                Work::Formula(Formula::Not(inner)) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    work.push(Work::Pair(0));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    work.push(Work::Pair(1));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    work.push(Work::Pair(2));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    work.push(Work::Pair(3));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Not => {
                    let inner = out.pop().expect("cloned formula operand");
                    out.push(Formula::Not(Box::new(inner)));
                }
                Work::Pair(kind) => {
                    let right = out.pop().expect("cloned right formula operand");
                    let left = out.pop().expect("cloned left formula operand");
                    out.push(match kind {
                        0 => Formula::And(Box::new(left), Box::new(right)),
                        1 => Formula::Or(Box::new(left), Box::new(right)),
                        2 => Formula::Iff(Box::new(left), Box::new(right)),
                        _ => Formula::Xor(Box::new(left), Box::new(right)),
                    });
                }
            }
        }
        out.pop().expect("cloned formula")
    }
}

impl Drop for Formula {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        drain_formula(self, &mut pending);
        while let Some(mut value) = pending.pop() {
            drain_formula(&mut value, &mut pending);
        }
    }
}

fn drain_formula(value: &mut Formula, pending: &mut Vec<Formula>) {
    match value {
        Formula::Not(inner) => pending.push(std::mem::replace(inner.as_mut(), Formula::True)),
        Formula::And(left, right)
        | Formula::Or(left, right)
        | Formula::Iff(left, right)
        | Formula::Xor(left, right) => {
            pending.push(std::mem::replace(left.as_mut(), Formula::True));
            pending.push(std::mem::replace(right.as_mut(), Formula::True));
        }
        Formula::True | Formula::False | Formula::Var(_) | Formula::Bound(_) => {}
    }
}

/// The span-free LIR portion of an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lir {
    /// Target-provided values, in source order for an unlinked artifact and
    /// dependency-first artifact order after linking.
    pub externs: Vec<Extern>,
    pub functions: Vec<Function>,
    pub globals: Vec<Global>,
}

/// One target-provided value in a backend import table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extern {
    /// The qualified Ruddy name used by [`Op::Global`] references.
    pub name: QualifiedName,
    /// The nonempty target namespace path, whose nonempty segments are joined
    /// with dots by a backend (for example, `["console", "log"]`).
    pub target: Vec<String>,
    pub rep: Rep,
}

/// A lifted LIR function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Block,
}

/// An LIR parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub temp: u32,
    pub rep: Rep,
}

/// A top-level initializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub name: QualifiedName,
    pub body: Block,
}

/// An ordered instruction list and one terminator.
#[derive(Debug)]
pub struct Block {
    pub instrs: Vec<Instr>,
    pub end: End,
}

/// A value-producing instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instr {
    pub temp: u32,
    pub rep: Rep,
    pub op: Op,
}

/// A typed key in a span-free LIR record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKey {
    Named(String),
    UnnamedOperation,
}

/// A span-free LIR operation.
#[derive(Debug)]
pub enum Op {
    Const(Literal),
    Neg(u32),
    Not(u32),
    And {
        left: u32,
        right: u32,
    },
    Or {
        left: u32,
        right: u32,
    },
    Xor {
        left: u32,
        right: u32,
    },
    Add {
        left: u32,
        right: u32,
    },
    Sub {
        left: u32,
        right: u32,
    },
    Mul {
        left: u32,
        right: u32,
    },
    Div {
        left: u32,
        right: u32,
    },
    Struct(Vec<(FieldKey, u32)>),
    Merge(Vec<u32>),
    Project {
        base: u32,
        field: FieldKey,
    },
    Tag {
        name: String,
        payload: Option<u32>,
    },
    Payload(u32),
    Closure {
        /// Index into [`Lir::functions`], fixed-width on the artifact boundary.
        func: u64,
        captures: Vec<u32>,
    },
    Call {
        callee: Callee,
        args: Vec<u32>,
    },
    /// A foreign ABI call containing only host-visible arguments.
    RawCall {
        callee: u32,
        args: Vec<u32>,
    },
    Extern {
        target: QualifiedName,
    },
    Global {
        target: QualifiedName,
    },
    NewTag,
    Catch {
        tag: u32,
        body: Box<Block>,
    },
    SwitchTag {
        on: u32,
        cases: Vec<TagCase>,
        fallback: Option<Box<Block>>,
    },
    SwitchPrim {
        on: u32,
        cases: Vec<PrimCase>,
        fallback: Option<Box<Block>>,
    },
    SwitchPresence {
        on: u32,
        field: String,
        present: Box<Block>,
        absent: Box<Block>,
    },
    SwitchRest {
        on: u32,
        fields: Vec<String>,
        none: Box<Block>,
        some: Box<Block>,
    },
}

/// The target of an LIR call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// Index into [`Lir::functions`], fixed-width on the artifact boundary.
    Direct(u64),
    Indirect(u32),
}

/// One tag-dispatch branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCase {
    pub name: String,
    pub block: Block,
}

/// One primitive-dispatch branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimCase {
    pub value: Literal,
    pub block: Block,
}

/// A block terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    Ret(u32),
    Yield(u32),
    Throw { tag: u32, value: u32 },
}

/// A literal.  Reals retain their bit representation, including NaNs and signed
/// zero, so equality does not accidentally change program data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Natural(u64),
    Integer(i64),
    Real(u64),
    String(String),
    Boolean(bool),
}

/// The machine representation retained by LIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rep {
    Nat,
    Int,
    Real,
    String,
    Boolean,
    Unit,
    Struct,
    Sum,
    Fn,
    Any,
}

// `Block` and `Op` form a mutually recursive ownership tree. They are public
// artifact values in their own right, so their ordinary ownership operations
// must not depend on an enclosing `Artifact` to provide a safe traversal.
enum LirCloneWork<'a> {
    Block(&'a Block),
    Op(&'a Op),
    FinishBlock {
        instrs: Vec<(u32, Rep)>,
        end: End,
    },
    FinishCatch(u32),
    FinishSwitchTag {
        on: u32,
        names: Vec<String>,
        fallback: bool,
    },
    FinishSwitchPrim {
        on: u32,
        values: Vec<Literal>,
        fallback: bool,
    },
    FinishSwitchPresence {
        on: u32,
        field: String,
    },
    FinishSwitchRest {
        on: u32,
        fields: Vec<String>,
    },
}

fn clone_lir_tree(root: LirCloneWork<'_>) -> (Vec<Block>, Vec<Op>) {
    let mut work = vec![root];
    let mut blocks = Vec::new();
    let mut ops = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            LirCloneWork::Block(block) => {
                work.push(LirCloneWork::FinishBlock {
                    instrs: block
                        .instrs
                        .iter()
                        .map(|instr| (instr.temp, instr.rep))
                        .collect(),
                    end: block.end.clone(),
                });
                work.extend(
                    block
                        .instrs
                        .iter()
                        .rev()
                        .map(|instr| LirCloneWork::Op(&instr.op)),
                );
            }
            LirCloneWork::Op(op) => match op {
                Op::Const(value) => ops.push(Op::Const(value.clone())),
                Op::Neg(value) => ops.push(Op::Neg(*value)),
                Op::Not(value) => ops.push(Op::Not(*value)),
                Op::And { left, right } => ops.push(Op::And {
                    left: *left,
                    right: *right,
                }),
                Op::Or { left, right } => ops.push(Op::Or {
                    left: *left,
                    right: *right,
                }),
                Op::Xor { left, right } => ops.push(Op::Xor {
                    left: *left,
                    right: *right,
                }),
                Op::Add { left, right } => ops.push(Op::Add {
                    left: *left,
                    right: *right,
                }),
                Op::Sub { left, right } => ops.push(Op::Sub {
                    left: *left,
                    right: *right,
                }),
                Op::Mul { left, right } => ops.push(Op::Mul {
                    left: *left,
                    right: *right,
                }),
                Op::Div { left, right } => ops.push(Op::Div {
                    left: *left,
                    right: *right,
                }),
                Op::Struct(fields) => ops.push(Op::Struct(fields.clone())),
                Op::Merge(values) => ops.push(Op::Merge(values.clone())),
                Op::Project { base, field } => ops.push(Op::Project {
                    base: *base,
                    field: field.clone(),
                }),
                Op::Tag { name, payload } => ops.push(Op::Tag {
                    name: name.clone(),
                    payload: *payload,
                }),
                Op::Payload(value) => ops.push(Op::Payload(*value)),
                Op::Closure { func, captures } => ops.push(Op::Closure {
                    func: *func,
                    captures: captures.clone(),
                }),
                Op::Call { callee, args } => ops.push(Op::Call {
                    callee: callee.clone(),
                    args: args.clone(),
                }),
                Op::RawCall { callee, args } => ops.push(Op::RawCall {
                    callee: *callee,
                    args: args.clone(),
                }),
                Op::Extern { target } => ops.push(Op::Extern {
                    target: target.clone(),
                }),
                Op::Global { target } => ops.push(Op::Global {
                    target: target.clone(),
                }),
                Op::NewTag => ops.push(Op::NewTag),
                Op::Catch { tag, body } => {
                    work.push(LirCloneWork::FinishCatch(*tag));
                    work.push(LirCloneWork::Block(body));
                }
                Op::SwitchTag {
                    on,
                    cases,
                    fallback,
                } => {
                    work.push(LirCloneWork::FinishSwitchTag {
                        on: *on,
                        names: cases.iter().map(|case| case.name.clone()).collect(),
                        fallback: fallback.is_some(),
                    });
                    if let Some(fallback) = fallback {
                        work.push(LirCloneWork::Block(fallback));
                    }
                    work.extend(
                        cases
                            .iter()
                            .rev()
                            .map(|case| LirCloneWork::Block(&case.block)),
                    );
                }
                Op::SwitchPrim {
                    on,
                    cases,
                    fallback,
                } => {
                    work.push(LirCloneWork::FinishSwitchPrim {
                        on: *on,
                        values: cases.iter().map(|case| case.value.clone()).collect(),
                        fallback: fallback.is_some(),
                    });
                    if let Some(fallback) = fallback {
                        work.push(LirCloneWork::Block(fallback));
                    }
                    work.extend(
                        cases
                            .iter()
                            .rev()
                            .map(|case| LirCloneWork::Block(&case.block)),
                    );
                }
                Op::SwitchPresence {
                    on,
                    field,
                    present,
                    absent,
                } => {
                    work.push(LirCloneWork::FinishSwitchPresence {
                        on: *on,
                        field: field.clone(),
                    });
                    work.push(LirCloneWork::Block(absent));
                    work.push(LirCloneWork::Block(present));
                }
                Op::SwitchRest {
                    on,
                    fields,
                    none,
                    some,
                } => {
                    work.push(LirCloneWork::FinishSwitchRest {
                        on: *on,
                        fields: fields.clone(),
                    });
                    work.push(LirCloneWork::Block(some));
                    work.push(LirCloneWork::Block(none));
                }
            },
            LirCloneWork::FinishBlock { instrs, end } => {
                let split = ops.len() - instrs.len();
                let instrs = instrs
                    .into_iter()
                    .zip(ops.drain(split..))
                    .map(|((temp, rep), op)| Instr { temp, rep, op })
                    .collect();
                blocks.push(Block { instrs, end });
            }
            LirCloneWork::FinishCatch(tag) => {
                ops.push(Op::Catch {
                    tag,
                    body: Box::new(blocks.pop().expect("cloned catch body")),
                });
            }
            LirCloneWork::FinishSwitchTag {
                on,
                names,
                fallback,
            } => {
                let fallback =
                    fallback.then(|| Box::new(blocks.pop().expect("cloned tag fallback")));
                let split = blocks.len() - names.len();
                let cases = names
                    .into_iter()
                    .zip(blocks.drain(split..))
                    .map(|(name, block)| TagCase { name, block })
                    .collect();
                ops.push(Op::SwitchTag {
                    on,
                    cases,
                    fallback,
                });
            }
            LirCloneWork::FinishSwitchPrim {
                on,
                values,
                fallback,
            } => {
                let fallback =
                    fallback.then(|| Box::new(blocks.pop().expect("cloned primitive fallback")));
                let split = blocks.len() - values.len();
                let cases = values
                    .into_iter()
                    .zip(blocks.drain(split..))
                    .map(|(value, block)| PrimCase { value, block })
                    .collect();
                ops.push(Op::SwitchPrim {
                    on,
                    cases,
                    fallback,
                });
            }
            LirCloneWork::FinishSwitchPresence { on, field } => {
                let absent = Box::new(blocks.pop().expect("cloned absent branch"));
                let present = Box::new(blocks.pop().expect("cloned present branch"));
                ops.push(Op::SwitchPresence {
                    on,
                    field,
                    present,
                    absent,
                });
            }
            LirCloneWork::FinishSwitchRest { on, fields } => {
                let some = Box::new(blocks.pop().expect("cloned nonempty-rest branch"));
                let none = Box::new(blocks.pop().expect("cloned empty-rest branch"));
                ops.push(Op::SwitchRest {
                    on,
                    fields,
                    none,
                    some,
                });
            }
        }
    }
    (blocks, ops)
}

impl Clone for Block {
    fn clone(&self) -> Self {
        let (mut blocks, ops) = clone_lir_tree(LirCloneWork::Block(self));
        debug_assert!(ops.is_empty());
        blocks.pop().expect("cloned block")
    }
}

impl Clone for Op {
    fn clone(&self) -> Self {
        let (blocks, mut ops) = clone_lir_tree(LirCloneWork::Op(self));
        debug_assert!(blocks.is_empty());
        ops.pop().expect("cloned operation")
    }
}

#[derive(PartialEq)]
enum OpHead<'a> {
    Const(&'a Literal),
    Unary(u8, u32),
    Binary(u8, u32, u32),
    Struct(&'a [(FieldKey, u32)]),
    Merge(&'a [u32]),
    Project(u32, &'a FieldKey),
    Tag(&'a str, Option<u32>),
    Closure(u64, &'a [u32]),
    Call(&'a Callee, &'a [u32]),
    RawCall(u32, &'a [u32]),
    Extern(&'a str),
    Global(&'a str),
    NewTag,
    Catch(u32),
    SwitchTag(u32, Vec<&'a str>, bool),
    SwitchPrim(u32, Vec<&'a Literal>, bool),
    SwitchPresence(u32, &'a str),
    SwitchRest(u32, &'a [String]),
}

impl<'a> From<&'a Op> for OpHead<'a> {
    fn from(op: &'a Op) -> Self {
        match op {
            Op::Const(value) => Self::Const(value),
            Op::Neg(value) => Self::Unary(0, *value),
            Op::Not(value) => Self::Unary(1, *value),
            Op::And { left, right } => Self::Binary(0, *left, *right),
            Op::Or { left, right } => Self::Binary(1, *left, *right),
            Op::Xor { left, right } => Self::Binary(2, *left, *right),
            Op::Add { left, right } => Self::Binary(3, *left, *right),
            Op::Sub { left, right } => Self::Binary(4, *left, *right),
            Op::Mul { left, right } => Self::Binary(5, *left, *right),
            Op::Div { left, right } => Self::Binary(6, *left, *right),
            Op::Struct(fields) => Self::Struct(fields),
            Op::Merge(values) => Self::Merge(values),
            Op::Project { base, field } => Self::Project(*base, field),
            Op::Tag { name, payload } => Self::Tag(name, *payload),
            Op::Payload(value) => Self::Unary(2, *value),
            Op::Closure { func, captures } => Self::Closure(*func, captures),
            Op::Call { callee, args } => Self::Call(callee, args),
            Op::RawCall { callee, args } => Self::RawCall(*callee, args),
            Op::Extern { target } => Self::Extern(target),
            Op::Global { target } => Self::Global(target),
            Op::NewTag => Self::NewTag,
            Op::Catch { tag, .. } => Self::Catch(*tag),
            Op::SwitchTag {
                on,
                cases,
                fallback,
            } => Self::SwitchTag(
                *on,
                cases.iter().map(|case| case.name.as_str()).collect(),
                fallback.is_some(),
            ),
            Op::SwitchPrim {
                on,
                cases,
                fallback,
            } => Self::SwitchPrim(
                *on,
                cases.iter().map(|case| &case.value).collect(),
                fallback.is_some(),
            ),
            Op::SwitchPresence { on, field, .. } => Self::SwitchPresence(*on, field),
            Op::SwitchRest { on, fields, .. } => Self::SwitchRest(*on, fields),
        }
    }
}

enum LirPair<'a> {
    Block(&'a Block, &'a Block),
    Op(&'a Op, &'a Op),
}

fn lir_tree_eq(root: LirPair<'_>) -> bool {
    let mut work = vec![root];
    while let Some(part) = work.pop() {
        match part {
            LirPair::Block(left, right) => {
                let left_head = (
                    &left.end,
                    left.instrs
                        .iter()
                        .map(|instr| (instr.temp, instr.rep))
                        .collect::<Vec<_>>(),
                );
                let right_head = (
                    &right.end,
                    right
                        .instrs
                        .iter()
                        .map(|instr| (instr.temp, instr.rep))
                        .collect::<Vec<_>>(),
                );
                if left_head != right_head {
                    return false;
                }
                work.extend(
                    left.instrs
                        .iter()
                        .zip(&right.instrs)
                        .map(|(left, right)| LirPair::Op(&left.op, &right.op)),
                );
            }
            LirPair::Op(left, right) => {
                if OpHead::from(left) != OpHead::from(right) {
                    return false;
                }
                match (left, right) {
                    (Op::Catch { body: left, .. }, Op::Catch { body: right, .. }) => {
                        work.push(LirPair::Block(left, right));
                    }
                    (
                        Op::SwitchTag {
                            cases: left_cases,
                            fallback: left_fallback,
                            ..
                        },
                        Op::SwitchTag {
                            cases: right_cases,
                            fallback: right_fallback,
                            ..
                        },
                    ) => {
                        work.extend(
                            left_cases
                                .iter()
                                .zip(right_cases)
                                .map(|(left, right)| LirPair::Block(&left.block, &right.block)),
                        );
                        work.extend(
                            left_fallback
                                .iter()
                                .zip(right_fallback)
                                .map(|(left, right)| LirPair::Block(left, right)),
                        );
                    }
                    (
                        Op::SwitchPrim {
                            cases: left_cases,
                            fallback: left_fallback,
                            ..
                        },
                        Op::SwitchPrim {
                            cases: right_cases,
                            fallback: right_fallback,
                            ..
                        },
                    ) => {
                        work.extend(
                            left_cases
                                .iter()
                                .zip(right_cases)
                                .map(|(left, right)| LirPair::Block(&left.block, &right.block)),
                        );
                        work.extend(
                            left_fallback
                                .iter()
                                .zip(right_fallback)
                                .map(|(left, right)| LirPair::Block(left, right)),
                        );
                    }
                    (
                        Op::SwitchPresence {
                            present: left_present,
                            absent: left_absent,
                            ..
                        },
                        Op::SwitchPresence {
                            present: right_present,
                            absent: right_absent,
                            ..
                        },
                    ) => {
                        work.push(LirPair::Block(left_absent, right_absent));
                        work.push(LirPair::Block(left_present, right_present));
                    }
                    (
                        Op::SwitchRest {
                            none: left_none,
                            some: left_some,
                            ..
                        },
                        Op::SwitchRest {
                            none: right_none,
                            some: right_some,
                            ..
                        },
                    ) => {
                        work.push(LirPair::Block(left_some, right_some));
                        work.push(LirPair::Block(left_none, right_none));
                    }
                    _ => {}
                }
            }
        }
    }
    true
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        lir_tree_eq(LirPair::Block(self, other))
    }
}

impl Eq for Block {}

impl PartialEq for Op {
    fn eq(&self, other: &Self) -> bool {
        lir_tree_eq(LirPair::Op(self, other))
    }
}

impl Eq for Op {}

fn empty_block() -> Block {
    Block {
        instrs: Vec::new(),
        end: End::Ret(0),
    }
}

fn drain_lir_op(op: &mut Op, pending: &mut Vec<Block>) {
    match op {
        Op::Catch { body, .. } => pending.push(std::mem::replace(body.as_mut(), empty_block())),
        Op::SwitchTag {
            cases, fallback, ..
        } => {
            pending.extend(
                cases
                    .iter_mut()
                    .map(|case| std::mem::replace(&mut case.block, empty_block())),
            );
            if let Some(block) = fallback.take() {
                pending.push(*block);
            }
        }
        Op::SwitchPrim {
            cases, fallback, ..
        } => {
            pending.extend(
                cases
                    .iter_mut()
                    .map(|case| std::mem::replace(&mut case.block, empty_block())),
            );
            if let Some(block) = fallback.take() {
                pending.push(*block);
            }
        }
        Op::SwitchPresence {
            present, absent, ..
        } => {
            pending.push(std::mem::replace(present.as_mut(), empty_block()));
            pending.push(std::mem::replace(absent.as_mut(), empty_block()));
        }
        Op::SwitchRest { none, some, .. } => {
            pending.push(std::mem::replace(none.as_mut(), empty_block()));
            pending.push(std::mem::replace(some.as_mut(), empty_block()));
        }
        Op::Const(_)
        | Op::Neg(_)
        | Op::Not(_)
        | Op::And { .. }
        | Op::Or { .. }
        | Op::Xor { .. }
        | Op::Add { .. }
        | Op::Sub { .. }
        | Op::Mul { .. }
        | Op::Div { .. }
        | Op::Struct(_)
        | Op::Merge(_)
        | Op::Project { .. }
        | Op::Tag { .. }
        | Op::Payload(_)
        | Op::Closure { .. }
        | Op::Call { .. }
        | Op::RawCall { .. }
        | Op::Extern { .. }
        | Op::Global { .. }
        | Op::NewTag => {}
    }
}

fn drain_lir_block(block: &mut Block, pending: &mut Vec<Block>) {
    for instr in &mut block.instrs {
        drain_lir_op(&mut instr.op, pending);
    }
    block.instrs.clear();
}

impl Drop for Block {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        drain_lir_block(self, &mut pending);
        while let Some(mut block) = pending.pop() {
            drain_lir_block(&mut block, &mut pending);
        }
    }
}

impl Drop for Op {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        drain_lir_op(self, &mut pending);
        while let Some(mut block) = pending.pop() {
            drain_lir_block(&mut block, &mut pending);
        }
    }
}

/// Build an artifact after inference and LIR lowering succeeded.
pub fn build(
    mint: &Mint,
    program: &ir::Program,
    inference: &inference::Output,
    lir: &lir::Output,
) -> Artifact {
    build_with_dependencies(mint, program, inference, lir, Vec::new())
}

/// Build an artifact with the dependency identities supplied by its driver.
pub fn build_with_dependencies(
    mint: &Mint,
    program: &ir::Program,
    inference: &inference::Output,
    lir: &lir::Output,
    dependencies: Vec<Dependency>,
) -> Artifact {
    let header = Header {
        identity: Identity {
            name: mint.bundle().name().to_string(),
            version: mint.bundle().version().to_string(),
        },
        dependencies,
        values: program
            .externs
            .keys()
            .map(|symbol| Value {
                name: qualified(mint, *symbol),
                scheme: scheme(mint, &inference.externs[symbol]),
            })
            .chain(
                program
                    .terms
                    .keys()
                    // Fresh top-level definitions implement patterns such as
                    // `let _`; they must be initialized, but have no source
                    // name through which another bundle could import them.
                    .filter(|symbol| !mint.is_local(**symbol))
                    .map(|symbol| Value {
                        name: qualified(mint, *symbol),
                        scheme: scheme(mint, &inference.schemes[symbol]),
                    }),
            )
            .collect(),
        types: program
            .types
            .iter()
            .map(|(symbol, declaration)| DeclaredType {
                name: qualified(mint, *symbol),
                params: declaration
                    .params
                    .iter()
                    .map(|param| Parameter {
                        sense: match &param.kind {
                            types::ParamKind::Type { .. } => Sense::Type,
                            types::ParamKind::Fields { .. } => Sense::Fields,
                            types::ParamKind::Cases { .. } => Sense::Cases,
                            types::ParamKind::Effects { .. } => Sense::Effects,
                        },
                        lacks: param.kind.lacks().iter().cloned().collect(),
                        relevant: param.relevant,
                    })
                    .collect(),
                scheme: scheme(mint, &inference.aliases[symbol]),
            })
            .collect(),
        effects: program
            .effects
            .iter()
            .map(|(symbol, declaration)| DeclaredEffect {
                name: qualified(mint, *symbol),
                identity: program.effect_ids.get(symbol).map(effect_id),
                kind: match &declaration.value {
                    ir::Effect::Operations(operations) => EffectKind::Operations(
                        operations
                            .iter()
                            .map(|(name, _)| {
                                let (from, to) = &inference.operations[&(*symbol, name.clone())];
                                Operation {
                                    selector: match name {
                                        ir::OperationSelector::Unnamed => {
                                            OperationSelector::Unnamed
                                        }
                                        ir::OperationSelector::Named(name) => {
                                            OperationSelector::Named(name.clone())
                                        }
                                    },
                                    from: ty(mint, from),
                                    to: ty(mint, to),
                                }
                            })
                            .collect(),
                    ),
                    ir::Effect::Alias(effects) => EffectKind::Alias(
                        effects
                            .values()
                            .map(|effect| qualified(mint, effect.symbol))
                            .collect(),
                    ),
                },
            })
            .collect(),
    };
    Artifact {
        header,
        lir: lower_lir(mint, lir),
    }
}

impl Artifact {
    /// Build an artifact after inference and LIR lowering succeeded.
    pub fn build(
        mint: &Mint,
        program: &ir::Program,
        inference: &inference::Output,
        lir: &lir::Output,
    ) -> Self {
        build(mint, program, inference, lir)
    }

    /// Canonical textual serialization.
    pub fn print(&self) -> String {
        print(self)
    }
    /// Parse trusted internal artifact text. Malformed input panics.
    pub fn parse(input: &str) -> Self {
        parse(input)
    }

    /// Parse artifact text without panicking on malformed input.
    pub fn try_parse(input: &str) -> Result<Self, ParseError> {
        try_parse(input)
    }
}

/// Print canonical artifact text.
pub fn print(artifact: &Artifact) -> String {
    text::print(artifact)
}
/// Parse trusted internal artifact text. Malformed input panics.
pub fn parse(input: &str) -> Artifact {
    text::parse(input)
}

/// Parse artifact text without panicking on malformed input.
pub fn try_parse(input: &str) -> Result<Artifact, ParseError> {
    text::try_parse(input)
}

fn qualified(mint: &Mint, symbol: Symbol) -> QualifiedName {
    if let Some(qualified) = mint.external(symbol) {
        return qualified.to_owned();
    }
    // Source paths deliberately do not distinguish locals. LIR names must:
    // hidden top-level definitions are fresh local symbols and multiple such
    // globals can coexist. A full canonical mangling is deterministic and
    // injective, while `%` keeps this compiler-only component disjoint from
    // every source identifier.
    if mint.is_local(symbol) {
        return format!(
            "{}@{}::%{}",
            mint.bundle().name(),
            mint.bundle().version(),
            mint.mangle(symbol)
        );
    }
    let path = mint.path(symbol).to_string();
    let prefix = mint.bundle().name();
    let suffix = path
        .strip_prefix(prefix)
        .expect("a mint path starts with its bundle");
    format!(
        "{}@{}{}",
        mint.bundle().name(),
        mint.bundle().version(),
        suffix
    )
}

fn effect_id(id: &types::EffectId) -> EffectIdentity {
    match id {
        types::EffectId::Structural { name, interface } => EffectIdentity {
            name: name.clone(),
            interface: interface.clone(),
        },
        types::EffectId::Pending(_) => {
            panic!("artifact building requires structural effect identities")
        }
    }
}

fn scheme(mint: &Mint, value: &types::Scheme) -> Scheme {
    Scheme {
        count: value.count(),
        presences: value.presences(),
        formula: formula(value.formula()),
        body: ty(mint, value.body()),
    }
}

fn ty(mint: &Mint, value: &types::Ty) -> Type {
    enum Work<'a> {
        Ty(&'a types::Ty),
        Row(&'a types::Row),
        Arrow,
        Struct,
        Sum,
        Named {
            name: QualifiedName,
            count: usize,
        },
        FinishRow {
            labels: Vec<(String, Presence)>,
            rest: Option<Rest>,
        },
    }

    let presence = |value: &types::Presence| match value {
        types::Presence::Present => Presence::Present,
        types::Presence::Absent => Presence::Absent,
        types::Presence::Var(value) => Presence::Var(*value),
        types::Presence::Bound(value) => Presence::Bound(*value),
        types::Presence::Recovered(_) | types::Presence::Undecided => Presence::Undecided,
    };
    let rest = |value: &types::Rest| match value {
        types::Rest::Closed => Some(Rest::Closed),
        types::Rest::Var(value) => Some(Rest::Var(*value)),
        types::Rest::Bound(value) => Some(Rest::Bound(*value)),
        types::Rest::Rigid { id, name } => Some(Rest::Rigid {
            id: *id,
            name: name.to_string(),
        }),
        types::Rest::Undecided => Some(Rest::Undecided),
        types::Rest::More(_) => None,
    };

    let mut work = vec![Work::Ty(value)];
    let mut tys = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(value) => match value {
                types::Ty::Nat => tys.push(Type::Nat),
                types::Ty::Int => tys.push(Type::Int),
                types::Ty::Real => tys.push(Type::Real),
                types::Ty::String => tys.push(Type::String),
                types::Ty::Boolean => tys.push(Type::Boolean),
                types::Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects));
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                types::Ty::Struct(row) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(row));
                }
                types::Ty::Sum(row) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(row));
                }
                types::Ty::Var(value) => tys.push(Type::Var(*value)),
                types::Ty::Bound(value) => tys.push(Type::Bound(*value)),
                types::Ty::Rigid { id, name } => tys.push(Type::Rigid {
                    id: *id,
                    name: name.to_string(),
                }),
                types::Ty::Named { symbol, args, .. } => {
                    work.push(Work::Named {
                        name: qualified(mint, *symbol),
                        count: args.len(),
                    });
                    work.extend(args.iter().rev().map(|arg| Work::Ty(arg)));
                }
                types::Ty::Undecided => tys.push(Type::Undecided),
            },
            Work::Row(value) => {
                let labels = value
                    .labels
                    .iter()
                    .map(|(name, field)| (name.clone(), presence(&field.presence)))
                    .collect();
                work.push(Work::FinishRow {
                    labels,
                    rest: rest(&value.rest),
                });
                if let types::Rest::More(more) = &value.rest {
                    work.push(Work::Row(more));
                }
                work.extend(value.labels.values().rev().filter_map(|field| {
                    (!matches!(field.presence, types::Presence::Absent))
                        .then_some(Work::Ty(&field.ty))
                }));
            }
            Work::Arrow => {
                let effects = rows.pop().expect("artifact arrow effects");
                let to = tys.pop().expect("artifact arrow result");
                let from = tys.pop().expect("artifact arrow parameter");
                tys.push(Type::Arrow(Box::new(from), Box::new(to), effects));
            }
            Work::Struct => {
                let row = rows.pop().expect("artifact struct row");
                tys.push(Type::Struct(row));
            }
            Work::Sum => {
                let row = rows.pop().expect("artifact sum row");
                tys.push(Type::Sum(row));
            }
            Work::Named { name, count } => {
                let split = tys.len() - count;
                let args = tys.drain(split..).collect();
                tys.push(Type::Named { name, args });
            }
            Work::FinishRow { labels, rest } => {
                let mut labels_out = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    let ty = match presence {
                        Presence::Absent => Type::Undecided,
                        _ => tys.pop().expect("artifact field payload"),
                    };
                    labels_out.push((name, RowField { presence, ty }));
                }
                labels_out.reverse();
                let labels = labels_out;
                let rest = match rest {
                    Some(rest) => rest,
                    None => Rest::More(Box::new(rows.pop().expect("artifact nested row"))),
                };
                rows.push(Row { labels, rest });
            }
        }
    }
    tys.pop().expect("artifact type result")
}

fn formula(value: &types::Formula) -> Formula {
    enum Work<'a> {
        Formula(&'a types::Formula),
        Not,
        Pair(u8),
    }

    let mut work = vec![Work::Formula(value)];
    let mut out = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Formula(types::Formula::True) => out.push(Formula::True),
            Work::Formula(types::Formula::False) => out.push(Formula::False),
            Work::Formula(types::Formula::Atom(types::Atom::Var(value))) => {
                out.push(Formula::Var(*value))
            }
            Work::Formula(types::Formula::Atom(types::Atom::Bound(value))) => {
                out.push(Formula::Bound(*value))
            }
            Work::Formula(types::Formula::Not(inner)) => {
                work.push(Work::Not);
                work.push(Work::Formula(inner));
            }
            Work::Formula(types::Formula::And(left, right)) => {
                work.push(Work::Pair(0));
                work.push(Work::Formula(right));
                work.push(Work::Formula(left));
            }
            Work::Formula(types::Formula::Or(left, right)) => {
                work.push(Work::Pair(1));
                work.push(Work::Formula(right));
                work.push(Work::Formula(left));
            }
            Work::Formula(types::Formula::Iff(left, right)) => {
                work.push(Work::Pair(2));
                work.push(Work::Formula(right));
                work.push(Work::Formula(left));
            }
            Work::Formula(types::Formula::Xor(left, right)) => {
                work.push(Work::Pair(3));
                work.push(Work::Formula(right));
                work.push(Work::Formula(left));
            }
            Work::Not => {
                let inner = out.pop().expect("formula conversion postorder");
                out.push(Formula::Not(Box::new(inner)));
            }
            Work::Pair(kind) => {
                let right = Box::new(out.pop().expect("right formula conversion postorder"));
                let left = Box::new(out.pop().expect("left formula conversion postorder"));
                out.push(match kind {
                    0 => Formula::And(left, right),
                    1 => Formula::Or(left, right),
                    2 => Formula::Iff(left, right),
                    _ => Formula::Xor(left, right),
                });
            }
        }
    }
    out.pop().expect("a converted formula")
}

fn lower_lir(mint: &Mint, output: &lir::Output) -> Lir {
    Lir {
        externs: output
            .externs
            .iter()
            .map(|external| Extern {
                name: qualified(mint, external.symbol),
                target: external.target.clone(),
                rep: rep(external.rep),
            })
            .collect(),
        functions: output
            .functions
            .iter()
            .map(|function| Function {
                name: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| Param {
                        temp: param.temp,
                        rep: rep(param.rep),
                    })
                    .collect(),
                body: block(mint, &function.body),
            })
            .collect(),
        globals: output
            .globals
            .iter()
            .map(|global| Global {
                name: qualified(mint, global.symbol),
                body: block(mint, &global.body),
            })
            .collect(),
    }
}

fn block(mint: &Mint, value: &lir::Block) -> Block {
    Block {
        instrs: value
            .instrs
            .iter()
            .map(|instr| Instr {
                temp: instr.temp,
                rep: rep(instr.rep),
                op: op(mint, &instr.op),
            })
            .collect(),
        end: end(value.end.kind),
    }
}

fn field_key(value: &lir::FieldKey) -> FieldKey {
    match value {
        lir::FieldKey::Named(name) => FieldKey::Named(name.clone()),
        lir::FieldKey::UnnamedOperation => FieldKey::UnnamedOperation,
    }
}

fn op(mint: &Mint, value: &lir::Op) -> Op {
    use lir::Op as Source;
    match value {
        Source::Const(value) => Op::Const(literal(value)),
        Source::Neg(value) => Op::Neg(*value),
        Source::Not(value) => Op::Not(*value),
        Source::And { left, right } => Op::And {
            left: *left,
            right: *right,
        },
        Source::Or { left, right } => Op::Or {
            left: *left,
            right: *right,
        },
        Source::Xor { left, right } => Op::Xor {
            left: *left,
            right: *right,
        },
        Source::Add { left, right } => Op::Add {
            left: *left,
            right: *right,
        },
        Source::Sub { left, right } => Op::Sub {
            left: *left,
            right: *right,
        },
        Source::Mul { left, right } => Op::Mul {
            left: *left,
            right: *right,
        },
        Source::Div { left, right } => Op::Div {
            left: *left,
            right: *right,
        },
        Source::Struct(fields) => Op::Struct(
            fields
                .iter()
                .map(|(field, temp)| (field_key(field), *temp))
                .collect(),
        ),
        Source::Merge(values) => Op::Merge(values.clone()),
        Source::Project { base, field } => Op::Project {
            base: *base,
            field: field_key(field),
        },
        Source::Tag { name, payload } => Op::Tag {
            name: name.clone(),
            payload: *payload,
        },
        Source::Payload(value) => Op::Payload(*value),
        Source::Closure { func, captures } => Op::Closure {
            func: u64::try_from(*func).expect("LIR function index does not fit artifact format"),
            captures: captures.clone(),
        },
        Source::Call { callee, args } => Op::Call {
            callee: match callee {
                lir::Callee::Direct(value) => Callee::Direct(
                    u64::try_from(*value).expect("LIR function index does not fit artifact format"),
                ),
                lir::Callee::Indirect(value) => Callee::Indirect(*value),
            },
            args: args.clone(),
        },
        Source::RawCall { callee, args } => Op::RawCall {
            callee: *callee,
            args: args.clone(),
        },
        Source::Extern { symbol, .. } => Op::Extern {
            target: qualified(mint, *symbol),
        },
        Source::Global { symbol, .. } => Op::Global {
            target: qualified(mint, *symbol),
        },
        Source::NewTag => Op::NewTag,
        Source::Catch { tag, body } => Op::Catch {
            tag: *tag,
            body: Box::new(block(mint, body)),
        },
        Source::SwitchTag {
            on,
            cases,
            fallback,
        } => Op::SwitchTag {
            on: *on,
            cases: cases
                .iter()
                .map(|case| TagCase {
                    name: case.name.clone(),
                    block: block(mint, &case.block),
                })
                .collect(),
            fallback: fallback
                .as_ref()
                .map(|block_| Box::new(block(mint, block_))),
        },
        Source::SwitchPrim {
            on,
            cases,
            fallback,
        } => Op::SwitchPrim {
            on: *on,
            cases: cases
                .iter()
                .map(|case| PrimCase {
                    value: literal(&case.value),
                    block: block(mint, &case.block),
                })
                .collect(),
            fallback: fallback
                .as_ref()
                .map(|block_| Box::new(block(mint, block_))),
        },
        Source::SwitchPresence {
            on,
            field,
            present,
            absent,
        } => Op::SwitchPresence {
            on: *on,
            field: field.clone(),
            present: Box::new(block(mint, present)),
            absent: Box::new(block(mint, absent)),
        },
        Source::SwitchRest {
            on,
            fields,
            none,
            some,
        } => Op::SwitchRest {
            on: *on,
            fields: fields.clone(),
            none: Box::new(block(mint, none)),
            some: Box::new(block(mint, some)),
        },
    }
}

fn literal(value: &ir::Literal) -> Literal {
    match value {
        ir::Literal::Natural(value) => Literal::Natural(*value),
        ir::Literal::Integer(value) => Literal::Integer(*value),
        ir::Literal::Real(value) => Literal::Real(value.to_bits()),
        ir::Literal::String(value) => Literal::String(value.clone()),
        ir::Literal::Boolean(value) => Literal::Boolean(*value),
    }
}
fn end(value: lir::End) -> End {
    match value {
        lir::End::Ret(value) => End::Ret(value),
        lir::End::Yield(value) => End::Yield(value),
        lir::End::Throw { tag, value } => End::Throw { tag, value },
    }
}
fn rep(value: lir::Rep) -> Rep {
    match value {
        lir::Rep::Nat => Rep::Nat,
        lir::Rep::Int => Rep::Int,
        lir::Rep::Real => Rep::Real,
        lir::Rep::String => Rep::String,
        lir::Rep::Boolean => Rep::Boolean,
        lir::Rep::Unit => Rep::Unit,
        lir::Rep::Struct => Rep::Struct,
        lir::Rep::Sum => Rep::Sum,
        lir::Rep::Fn => Rep::Fn,
        lir::Rep::Any => Rep::Any,
    }
}

/// Canonical text helpers. The S-expression grammar is deliberately explicit:
/// every type, formula, operation, nested block, and ordered map has a distinct
/// tag. The writer uses a fixed-width pretty layout; strings are quoted, and
/// the parser accepts trusted output only.
pub mod text {
    use super::*;
    use pretty::RcDoc;

    /// The fixed width of canonical artifact text. Keeping this here rather
    /// than at the call site makes line breaking part of the format.
    const WIDTH: usize = 80;
    #[derive(Debug, Clone)]
    enum S {
        Atom(String),
        Str(String),
        /// Already-canonical deep syntax. Pretty-printing a 30,000-level list
        /// would build and render an equally deep document (and quadratic
        /// indentation); compact syntax remains canonical and stack safe.
        Raw(String),
        List(Vec<S>),
    }

    // Parsed S-expressions can be arbitrarily deep, including in malformed
    // structural positions that the reader discards. Rust's derived drop walk
    // would recurse through every nested `List`; drain descendants onto an
    // explicit heap stack instead. This applies to every owned `S` (roots,
    // parser stacks, reader task stacks, and truncated extras), so error paths
    // are no less safe than successful decoding.
    impl Drop for S {
        fn drop(&mut self) {
            let mut pending = Vec::new();
            if let S::List(children) = self {
                pending.append(children);
            }
            while let Some(mut value) = pending.pop() {
                if let S::List(children) = &mut value {
                    pending.append(children);
                }
                // `value` now has no owned descendants, so its own Drop is
                // constant-depth.
            }
        }
    }

    use S::{Atom as A, List as L, Raw, Str as Q};

    /// Print one artifact as canonical text, pretty-printed at a fixed width
    /// and always ending in one newline.
    pub fn print(value: &Artifact) -> String {
        let mut out = Vec::new();
        doc(&artifact(value))
            .render(WIDTH, &mut out)
            .expect("writing an artifact to memory cannot fail");
        out.push(b'\n');
        String::from_utf8(out).expect("artifact text is UTF-8")
    }
    /// Parse canonical trusted text; malformed text panics.
    pub fn parse(input: &str) -> Artifact {
        try_parse(input).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Parse canonical text without panicking on malformed input.
    pub fn try_parse(input: &str) -> Result<Artifact, ParseError> {
        let mut parser = Parser { input, at: 0 };
        let value = parser.value()?;
        parser.space();
        if parser.at != input.len() {
            return Err(ParseError::syntax("trailing artifact text", parser.at));
        }
        Reader::new().artifact(value)
    }

    fn artifact(value: &Artifact) -> S {
        L(vec![
            A("artifact".into()),
            header(&value.header),
            lir(&value.lir),
        ])
    }
    fn header(value: &Header) -> S {
        L(vec![
            A("header".into()),
            L(vec![
                A("identity".into()),
                Q(value.identity.name.clone()),
                Q(value.identity.version.clone()),
            ]),
            L(std::iter::once(A("dependencies".into()))
                .chain(value.dependencies.iter().map(dependency))
                .collect()),
            L(std::iter::once(A("values".into()))
                .chain(value.values.iter().map(value_))
                .collect()),
            L(std::iter::once(A("types".into()))
                .chain(value.types.iter().map(declared_type))
                .collect()),
            L(std::iter::once(A("effects".into()))
                .chain(value.effects.iter().map(effect))
                .collect()),
        ])
    }
    fn dependency(value: &Dependency) -> S {
        L(vec![
            A("dependency".into()),
            Q(value.name.clone()),
            Q(value.version.clone()),
        ])
    }
    fn value_(value: &Value) -> S {
        L(vec![
            A("value".into()),
            Q(value.name.clone()),
            scheme(&value.scheme),
        ])
    }
    fn declared_type(value: &DeclaredType) -> S {
        L(vec![
            A("type".into()),
            Q(value.name.clone()),
            L(std::iter::once(A("params".into()))
                .chain(value.params.iter().map(parameter))
                .collect()),
            scheme(&value.scheme),
        ])
    }
    fn parameter(value: &Parameter) -> S {
        L(vec![
            A("param".into()),
            A(match value.sense {
                Sense::Type => "type",
                Sense::Fields => "fields",
                Sense::Cases => "cases",
                Sense::Effects => "effects",
            }
            .into()),
            A(value.relevant.to_string()),
            L(std::iter::once(A("lacks".into()))
                .chain(value.lacks.iter().cloned().map(Q))
                .collect()),
        ])
    }
    fn effect(value: &DeclaredEffect) -> S {
        L(vec![
            A("effect".into()),
            Q(value.name.clone()),
            match &value.identity {
                Some(identity) => L(vec![
                    A("identity".into()),
                    Q(identity.name.clone()),
                    Q(identity.interface.clone()),
                ]),
                None => L(vec![A("identity".into()), A("none".into())]),
            },
            match &value.kind {
                EffectKind::Operations(values) => L(std::iter::once(A("operations".into()))
                    .chain(values.iter().map(operation))
                    .collect()),
                EffectKind::Alias(values) => L(std::iter::once(A("alias".into()))
                    .chain(values.iter().cloned().map(Q))
                    .collect()),
            },
        ])
    }
    fn operation(value: &Operation) -> S {
        let selector = match &value.selector {
            OperationSelector::Unnamed => L(vec![A("selector".into()), A("unnamed".into())]),
            OperationSelector::Named(name) => L(vec![
                A("selector".into()),
                A("named".into()),
                Q(name.clone()),
            ]),
        };
        L(vec![
            A("operation".into()),
            selector,
            ty(&value.from),
            ty(&value.to),
        ])
    }
    fn scheme(value: &Scheme) -> S {
        L(vec![
            A("scheme".into()),
            A(value.count.to_string()),
            A(value.presences.to_string()),
            formula(&value.formula),
            ty(&value.body),
        ])
    }
    fn ty(value: &Type) -> S {
        enum Work<'a> {
            Ty(&'a Type),
            Row(&'a Row),
            Rest(&'a Rest),
            Field(&'a RowField),
            Presence(&'a Presence),
            Text(&'static str),
            Owned(String),
            Quoted(&'a str),
        }

        let mut out = String::new();
        let mut work = vec![Work::Ty(value)];
        while let Some(part) = work.pop() {
            match part {
                Work::Text(text) => out.push_str(text),
                Work::Owned(text) => out.push_str(&text),
                Work::Quoted(text) => out.push_str(&quoted(text)),
                Work::Ty(value) => {
                    out.push_str("(ty ");
                    work.push(Work::Text(")"));
                    match value {
                        Type::Nat => work.push(Work::Text("nat")),
                        Type::Int => work.push(Work::Text("int")),
                        Type::Real => work.push(Work::Text("real")),
                        Type::String => work.push(Work::Text("string")),
                        Type::Boolean => work.push(Work::Text("boolean")),
                        Type::Arrow(from, to, row) => {
                            out.push_str("(arrow ");
                            work.push(Work::Text(")"));
                            work.push(Work::Row(row));
                            work.push(Work::Text(" "));
                            work.push(Work::Ty(to));
                            work.push(Work::Text(" "));
                            work.push(Work::Ty(from));
                        }
                        Type::Struct(row) | Type::Sum(row) => {
                            out.push('(');
                            out.push_str(if matches!(value, Type::Struct(_)) {
                                "struct "
                            } else {
                                "sum "
                            });
                            work.push(Work::Text(")"));
                            work.push(Work::Row(row));
                        }
                        Type::Var(value) => out.push_str(&format!("(var {value})")),
                        Type::Bound(value) => out.push_str(&format!("(bound {value})")),
                        Type::Rigid { id, name } => {
                            out.push_str("(rigid ");
                            out.push_str(&id.to_string());
                            out.push(' ');
                            out.push_str(&quoted(name));
                            out.push(')');
                        }
                        Type::Named { name, args } => {
                            out.push_str("(named ");
                            out.push_str(&quoted(name));
                            work.push(Work::Text(")"));
                            for arg in args.iter().rev() {
                                work.push(Work::Ty(arg));
                                work.push(Work::Text(" "));
                            }
                        }
                        Type::Undecided => work.push(Work::Text("undecided")),
                    }
                }
                Work::Row(row) => {
                    out.push_str("(row (labels");
                    work.push(Work::Text(")"));
                    work.push(Work::Rest(&row.rest));
                    work.push(Work::Text(") "));
                    for (name, field) in row.labels.iter().rev() {
                        work.push(Work::Text(")"));
                        work.push(Work::Field(field));
                        work.push(Work::Text(" "));
                        work.push(Work::Quoted(name));
                        work.push(Work::Text(" ("));
                    }
                }
                Work::Rest(rest) => match rest {
                    Rest::Closed => work.push(Work::Text("closed")),
                    Rest::Var(value) => work.push(Work::Owned(format!("(var {value})"))),
                    Rest::Bound(value) => work.push(Work::Owned(format!("(bound {value})"))),
                    Rest::Rigid { id, name } => {
                        work.push(Work::Text(")"));
                        work.push(Work::Quoted(name));
                        work.push(Work::Owned(format!("(rigid {id} ")));
                    }
                    Rest::Undecided => work.push(Work::Text("undecided")),
                    Rest::More(row) => {
                        work.push(Work::Text(")"));
                        work.push(Work::Row(row));
                        work.push(Work::Text("(more "));
                    }
                },
                Work::Field(field) => {
                    work.push(Work::Text(")"));
                    work.push(Work::Ty(&field.ty));
                    work.push(Work::Text(" "));
                    work.push(Work::Presence(&field.presence));
                    work.push(Work::Text("(field "));
                }
                Work::Presence(presence) => match presence {
                    Presence::Present => work.push(Work::Text("present")),
                    Presence::Absent => work.push(Work::Text("absent")),
                    Presence::Var(value) => work.push(Work::Owned(format!("(var {value})"))),
                    Presence::Bound(value) => work.push(Work::Owned(format!("(bound {value})"))),
                    Presence::Undecided => work.push(Work::Text("undecided")),
                },
            }
        }
        Raw(out)
    }
    fn formula(value: &Formula) -> S {
        let mut depth = vec![(value, 1usize)];
        while let Some((formula, at)) = depth.pop() {
            if at > 1_024 {
                return Raw(compact_formula(value));
            }
            match formula {
                Formula::Not(inner) => depth.push((inner, at + 1)),
                Formula::And(left, right)
                | Formula::Or(left, right)
                | Formula::Iff(left, right)
                | Formula::Xor(left, right) => {
                    depth.push((right, at + 1));
                    depth.push((left, at + 1));
                }
                Formula::True | Formula::False | Formula::Var(_) | Formula::Bound(_) => {}
            }
        }

        enum Work<'a> {
            Formula(&'a Formula),
            Not,
            Pair(&'static str),
        }
        let mut work = vec![Work::Formula(value)];
        let mut out = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Formula(Formula::True) => out.push(A("true".into())),
                Work::Formula(Formula::False) => out.push(A("false".into())),
                Work::Formula(Formula::Var(value)) => {
                    out.push(L(vec![A("var".into()), A(value.to_string())]))
                }
                Work::Formula(Formula::Bound(value)) => {
                    out.push(L(vec![A("bound".into()), A(value.to_string())]))
                }
                Work::Formula(Formula::Not(inner)) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    work.push(Work::Pair("and"));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    work.push(Work::Pair("or"));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    work.push(Work::Pair("iff"));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    work.push(Work::Pair("xor"));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Not => {
                    let inner = out.pop().expect("formula text postorder");
                    out.push(L(vec![A("not".into()), inner]));
                }
                Work::Pair(tag) => {
                    let right = out.pop().expect("right formula text postorder");
                    let left = out.pop().expect("left formula text postorder");
                    out.push(L(vec![A(tag.into()), left, right]));
                }
            }
        }
        out.pop().expect("formula text")
    }

    fn compact_formula(root: &Formula) -> String {
        enum Work<'a> {
            Formula(&'a Formula),
            Text(&'static str),
        }
        let mut out = String::new();
        let mut work = vec![Work::Formula(root)];
        while let Some(part) = work.pop() {
            match part {
                Work::Text(text) => out.push_str(text),
                Work::Formula(Formula::True) => out.push_str("true"),
                Work::Formula(Formula::False) => out.push_str("false"),
                Work::Formula(Formula::Var(value)) => {
                    out.push_str("(var ");
                    out.push_str(&value.to_string());
                    out.push(')');
                }
                Work::Formula(Formula::Bound(value)) => {
                    out.push_str("(bound ");
                    out.push_str(&value.to_string());
                    out.push(')');
                }
                Work::Formula(Formula::Not(inner)) => {
                    out.push_str("(not ");
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    out.push_str("(and ");
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(right));
                    work.push(Work::Text(" "));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    out.push_str("(or ");
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(right));
                    work.push(Work::Text(" "));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    out.push_str("(iff ");
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(right));
                    work.push(Work::Text(" "));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    out.push_str("(xor ");
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(right));
                    work.push(Work::Text(" "));
                    work.push(Work::Formula(left));
                }
            }
        }
        out
    }

    fn lir(value: &Lir) -> S {
        L(vec![
            A("lir".into()),
            L(std::iter::once(A("externs".into()))
                .chain(value.externs.iter().map(extern_))
                .collect()),
            L(std::iter::once(A("functions".into()))
                .chain(value.functions.iter().map(function))
                .collect()),
            L(std::iter::once(A("globals".into()))
                .chain(value.globals.iter().map(global))
                .collect()),
        ])
    }
    fn extern_(value: &Extern) -> S {
        L(vec![
            A("extern".into()),
            Q(value.name.clone()),
            L(std::iter::once(A("target".into()))
                .chain(value.target.iter().cloned().map(Q))
                .collect()),
            A(rep_name(value.rep).into()),
        ])
    }
    fn function(value: &Function) -> S {
        L(vec![
            A("function".into()),
            Q(value.name.clone()),
            L(std::iter::once(A("params".into()))
                .chain(value.params.iter().map(param))
                .collect()),
            block(&value.body),
        ])
    }
    fn param(value: &Param) -> S {
        L(vec![
            A("param".into()),
            A(value.temp.to_string()),
            A(rep_name(value.rep).into()),
        ])
    }
    fn global(value: &Global) -> S {
        L(vec![
            A("global".into()),
            Q(value.name.clone()),
            block(&value.body),
        ])
    }
    fn block(value: &Block) -> S {
        L(vec![
            A("block".into()),
            L(std::iter::once(A("instrs".into()))
                .chain(value.instrs.iter().map(instr))
                .collect()),
            end(&value.end),
        ])
    }
    fn instr(value: &Instr) -> S {
        L(vec![
            A("instr".into()),
            A(value.temp.to_string()),
            A(rep_name(value.rep).into()),
            op(&value.op),
        ])
    }
    fn field_key(value: &FieldKey) -> S {
        match value {
            FieldKey::Named(name) => L(vec![
                A("field-key".into()),
                A("named".into()),
                Q(name.clone()),
            ]),
            FieldKey::UnnamedOperation => {
                L(vec![A("field-key".into()), A("unnamed-operation".into())])
            }
        }
    }
    fn op(value: &Op) -> S {
        match value {
            Op::Const(value) => L(vec![A("const".into()), literal(value)]),
            Op::Neg(value) => unary("neg", *value),
            Op::Not(value) => unary("not", *value),
            Op::And { left, right } => binary("and", *left, *right),
            Op::Or { left, right } => binary("or", *left, *right),
            Op::Xor { left, right } => binary("xor", *left, *right),
            Op::Add { left, right } => binary("add", *left, *right),
            Op::Sub { left, right } => binary("sub", *left, *right),
            Op::Mul { left, right } => binary("mul", *left, *right),
            Op::Div { left, right } => binary("div", *left, *right),
            Op::Struct(fields) => L(std::iter::once(A("struct".into()))
                .chain(
                    fields
                        .iter()
                        .map(|(field, value)| L(vec![field_key(field), A(value.to_string())])),
                )
                .collect()),
            Op::Merge(values) => L(std::iter::once(A("merge".into()))
                .chain(values.iter().map(|value| A(value.to_string())))
                .collect()),
            Op::Project { base, field } => L(vec![
                A("project".into()),
                A(base.to_string()),
                field_key(field),
            ]),
            Op::Tag { name, payload } => L(std::iter::once(A("tag".into()))
                .chain(std::iter::once(Q(name.clone())))
                .chain(payload.iter().map(|value| A(value.to_string())))
                .collect()),
            Op::Payload(value) => unary("payload", *value),
            Op::Closure { func, captures } => L(vec![
                A("closure".into()),
                A(func.to_string()),
                L(std::iter::once(A("captures".into()))
                    .chain(captures.iter().map(|value| A(value.to_string())))
                    .collect()),
            ]),
            Op::Call { callee, args } => L(vec![
                A("call".into()),
                match callee {
                    Callee::Direct(value) => L(vec![A("direct".into()), A(value.to_string())]),
                    Callee::Indirect(value) => L(vec![A("indirect".into()), A(value.to_string())]),
                },
                L(std::iter::once(A("args".into()))
                    .chain(args.iter().map(|value| A(value.to_string())))
                    .collect()),
            ]),
            Op::RawCall { callee, args } => L(vec![
                A("raw-call".into()),
                A(callee.to_string()),
                L(std::iter::once(A("args".into()))
                    .chain(args.iter().map(|value| A(value.to_string())))
                    .collect()),
            ]),
            Op::Extern { target } => L(vec![A("extern".into()), Q(target.clone())]),
            Op::Global { target } => L(vec![A("global".into()), Q(target.clone())]),
            Op::NewTag => A("new-tag".into()),
            Op::Catch { tag, body } => L(vec![A("catch".into()), A(tag.to_string()), block(body)]),
            Op::SwitchTag {
                on,
                cases,
                fallback,
            } => L(vec![
                A("switch-tag".into()),
                A(on.to_string()),
                L(std::iter::once(A("cases".into()))
                    .chain(
                        cases
                            .iter()
                            .map(|case| L(vec![Q(case.name.clone()), block(&case.block)])),
                    )
                    .collect()),
                optional_block("fallback", fallback),
            ]),
            Op::SwitchPrim {
                on,
                cases,
                fallback,
            } => L(vec![
                A("switch-prim".into()),
                A(on.to_string()),
                L(std::iter::once(A("cases".into()))
                    .chain(
                        cases
                            .iter()
                            .map(|case| L(vec![literal(&case.value), block(&case.block)])),
                    )
                    .collect()),
                optional_block("fallback", fallback),
            ]),
            Op::SwitchPresence {
                on,
                field,
                present,
                absent,
            } => L(vec![
                A("switch-presence".into()),
                A(on.to_string()),
                Q(field.clone()),
                block(present),
                block(absent),
            ]),
            Op::SwitchRest {
                on,
                fields,
                none,
                some,
            } => L(vec![
                A("switch-rest".into()),
                A(on.to_string()),
                L(std::iter::once(A("fields".into()))
                    .chain(fields.iter().cloned().map(Q))
                    .collect()),
                block(none),
                block(some),
            ]),
        }
    }
    fn unary(tag: &str, value: u32) -> S {
        L(vec![A(tag.into()), A(value.to_string())])
    }
    fn binary(tag: &str, left: u32, right: u32) -> S {
        L(vec![
            A(tag.into()),
            A(left.to_string()),
            A(right.to_string()),
        ])
    }
    fn optional_block(tag: &str, value: &Option<Box<Block>>) -> S {
        L(std::iter::once(A(tag.into()))
            .chain(value.iter().map(|value| block(value)))
            .collect())
    }
    fn end(value: &End) -> S {
        match value {
            End::Ret(value) => unary("ret", *value),
            End::Yield(value) => unary("yield", *value),
            End::Throw { tag, value } => L(vec![
                A("throw".into()),
                A(tag.to_string()),
                A(value.to_string()),
            ]),
        }
    }
    fn literal(value: &Literal) -> S {
        match value {
            Literal::Natural(value) => L(vec![A("nat".into()), A(value.to_string())]),
            Literal::Integer(value) => L(vec![A("int".into()), A(value.to_string())]),
            Literal::Real(value) => L(vec![A("real".into()), A(value.to_string())]),
            Literal::String(value) => L(vec![A("string".into()), Q(value.clone())]),
            Literal::Boolean(value) => L(vec![A("bool".into()), A(value.to_string())]),
        }
    }
    fn rep_name(value: Rep) -> &'static str {
        match value {
            Rep::Nat => "nat",
            Rep::Int => "int",
            Rep::Real => "real",
            Rep::String => "string",
            Rep::Boolean => "boolean",
            Rep::Unit => "unit",
            Rep::Struct => "struct",
            Rep::Sum => "sum",
            Rep::Fn => "fn",
            Rep::Any => "any",
        }
    }

    fn doc(value: &S) -> RcDoc<'_, ()> {
        enum Work<'a> {
            Value(&'a S),
            List(usize),
        }
        let mut work = vec![Work::Value(value)];
        let mut out = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Value(A(value)) | Work::Value(Raw(value)) => {
                    out.push(RcDoc::text(value.as_str()))
                }
                Work::Value(Q(value)) => out.push(RcDoc::text(quoted(value))),
                Work::Value(L(values)) => {
                    work.push(Work::List(values.len()));
                    work.extend(values.iter().rev().map(Work::Value));
                }
                Work::List(len) => {
                    let at = out.len() - len;
                    let children = out.split_off(at);
                    out.push(
                        RcDoc::text("(")
                            .append(RcDoc::intersperse(children, RcDoc::line()).nest(2))
                            .append(")")
                            .group(),
                    );
                }
            }
        }
        out.pop().expect("one document root")
    }

    fn quoted(value: &str) -> String {
        let mut out = String::from("\"");
        for c in value.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    struct Parser<'a> {
        input: &'a str,
        at: usize,
    }
    impl<'a> Parser<'a> {
        fn space(&mut self) {
            while let Some(character) = self.peek() {
                if !character.is_whitespace() {
                    break;
                }
                self.at += character.len_utf8();
            }
        }
        fn value(&mut self) -> Result<S, ParseError> {
            // S-expression nesting is data, not control flow. Keeping open
            // lists on the heap lets a valid artifact be as deep as memory
            // permits and lets malformed deep input fail normally.
            let mut lists: Vec<Vec<S>> = Vec::new();
            loop {
                self.space();
                match self.peek() {
                    Some('(') => {
                        self.at += 1;
                        lists.push(Vec::new());
                    }
                    Some(')') => {
                        let close_at = self.at;
                        self.at += 1;
                        let Some(values) = lists.pop() else {
                            return Err(ParseError::syntax("unexpected `)`", close_at));
                        };
                        let value = L(values);
                        if let Some(parent) = lists.last_mut() {
                            parent.push(value);
                        } else {
                            return Ok(value);
                        }
                    }
                    Some('"') => {
                        let value = self.string()?;
                        if let Some(parent) = lists.last_mut() {
                            parent.push(value);
                        } else {
                            return Ok(value);
                        }
                    }
                    Some(_) => {
                        let value = self.atom();
                        if let Some(parent) = lists.last_mut() {
                            parent.push(value);
                        } else {
                            return Ok(value);
                        }
                    }
                    None if lists.is_empty() => {
                        return Err(ParseError::syntax("truncated artifact text", self.at));
                    }
                    None => {
                        return Err(ParseError::syntax("unterminated artifact list", self.at));
                    }
                }
            }
        }
        fn peek(&self) -> Option<char> {
            self.input[self.at..].chars().next()
        }
        fn atom(&mut self) -> S {
            let start = self.at;
            while let Some(c) = self.peek() {
                if c.is_whitespace() || matches!(c, '(' | ')') {
                    break;
                }
                self.at += c.len_utf8();
            }
            A(self.input[start..self.at].to_string())
        }
        fn string(&mut self) -> Result<S, ParseError> {
            self.at += 1;
            let mut out = String::new();
            loop {
                let Some(c) = self.peek() else {
                    return Err(ParseError::syntax("unterminated artifact string", self.at));
                };
                self.at += c.len_utf8();
                match c {
                    '"' => break,
                    '\\' => {
                        let Some(escape) = self.peek() else {
                            return Err(ParseError::syntax("truncated artifact escape", self.at));
                        };
                        self.at += escape.len_utf8();
                        out.push(match escape {
                            '\\' => '\\',
                            '"' => '"',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            'u' => self.control_escape()?,
                            _ => {
                                return Err(ParseError::syntax(
                                    "invalid artifact escape",
                                    self.at - escape.len_utf8(),
                                ));
                            }
                        });
                    }
                    c if c.is_control() => {
                        return Err(ParseError::syntax(
                            "unescaped control in artifact string",
                            self.at - c.len_utf8(),
                        ));
                    }
                    c => out.push(c),
                }
            }
            Ok(Q(out))
        }
        fn control_escape(&mut self) -> Result<char, ParseError> {
            let start = self.at;
            let mut value = 0;
            for _ in 0..4 {
                let Some(digit) = self.peek() else {
                    return Err(ParseError::syntax(
                        "truncated artifact control escape",
                        self.at,
                    ));
                };
                self.at += digit.len_utf8();
                let Some(value_digit) = digit.to_digit(16) else {
                    return Err(ParseError::syntax(
                        "invalid artifact control escape",
                        self.at - digit.len_utf8(),
                    ));
                };
                value = value * 16 + value_digit;
            }
            let Some(control) = char::from_u32(value) else {
                return Err(ParseError::syntax("invalid artifact control escape", start));
            };
            if !control.is_control() || matches!(control, '\n' | '\r' | '\t') {
                return Err(ParseError::syntax("invalid artifact control escape", start));
            }
            Ok(control)
        }
    }

    fn list_contents(mut value: S) -> Option<Vec<S>> {
        match &mut value {
            L(values) => Some(std::mem::take(values)),
            _ => None,
        }
    }

    /// Destroy recursive artifact contents without following their ownership
    /// on the call stack. Draining first also makes the artifact's subsequent
    /// field destruction constant-depth.
    pub(super) fn discard_artifact(artifact: &mut Artifact) {
        fn discard_formula(formula: Formula) {
            drop(formula);
        }

        fn discard_type(ty: Type) {
            drop(ty);
        }

        fn discard_scheme(scheme: Scheme) {
            discard_formula(scheme.formula);
            discard_type(scheme.body);
        }

        fn discard_block(block: Block) {
            drop(block);
        }

        for value in artifact.header.values.drain(..) {
            discard_scheme(value.scheme);
        }
        for declared in artifact.header.types.drain(..) {
            discard_scheme(declared.scheme);
        }
        for effect in artifact.header.effects.drain(..) {
            if let EffectKind::Operations(operations) = effect.kind {
                for operation in operations {
                    discard_type(operation.from);
                    discard_type(operation.to);
                }
            }
        }
        for function in artifact.lir.functions.drain(..) {
            discard_block(function.body);
        }
        for global in artifact.lir.globals.drain(..) {
            discard_block(global.body);
        }
    }

    struct Reader {
        error: std::cell::RefCell<Option<ParseError>>,
    }

    impl Reader {
        fn new() -> Self {
            Self {
                error: std::cell::RefCell::new(None),
            }
        }

        fn artifact(self, value: S) -> Result<Artifact, ParseError> {
            let mut artifact = self.read_artifact(value);
            match self.error.into_inner() {
                Some(error) => {
                    discard_artifact(&mut artifact);
                    Err(error)
                }
                None => Ok(artifact),
            }
        }

        fn fail(&self, message: impl Into<String>) {
            let mut error = self.error.borrow_mut();
            if error.is_none() {
                *error = Some(ParseError::structure(message));
            }
        }

        fn invalid<T>(&self, message: impl Into<String>, fallback: T) -> T {
            self.fail(message);
            fallback
        }

        fn take(&self, values: &mut Vec<S>) -> S {
            if values.is_empty() {
                self.fail("missing artifact value");
                A(String::new())
            } else {
                values.remove(0)
            }
        }

        fn list(&self, mut value: S, tag: &str) -> Vec<S> {
            match &mut value {
                L(values) => {
                    if !matches!(values.first(), Some(A(found)) if found == tag) {
                        self.fail(format!("expected `{tag}`"));
                    }
                    self.take(values);
                    std::mem::take(values)
                }
                _ => {
                    self.fail(format!("expected `{tag}` list"));
                    Vec::new()
                }
            }
        }

        fn atom(&self, mut value: S) -> String {
            match &mut value {
                A(value) => std::mem::take(value),
                _ => {
                    self.fail("expected artifact atom");
                    String::new()
                }
            }
        }

        fn string(&self, mut value: S) -> String {
            match &mut value {
                Q(value) => std::mem::take(value),
                _ => {
                    self.fail("expected artifact string");
                    String::new()
                }
            }
        }

        fn exact(&self, mut values: Vec<S>, count: usize, tag: &str) -> Vec<S> {
            if values.len() != count {
                self.fail(format!("bad `{tag}` arity"));
                values.resize_with(count, || A(String::new()));
                values.truncate(count);
            }
            values
        }

        fn number<T: std::str::FromStr + Default>(&self, value: S) -> T {
            match self.atom(value).parse() {
                Ok(value) => value,
                Err(_) => {
                    self.fail("invalid artifact number");
                    T::default()
                }
            }
        }

        fn boolean(&self, value: S) -> bool {
            match self.atom(value).as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    self.fail("invalid artifact boolean");
                    false
                }
            }
        }

        fn many(&self, value: S, tag: &str) -> Vec<S> {
            self.list(value, tag)
        }

        fn read_artifact(&self, value: S) -> Artifact {
            let mut values = self.exact(self.list(value, "artifact"), 2, "artifact");
            Artifact {
                header: self.read_header(self.take(&mut values)),
                lir: self.read_lir(self.take(&mut values)),
            }
        }
        fn read_header(&self, value: S) -> Header {
            let mut values = self.exact(self.list(value, "header"), 5, "header");
            let identity = {
                let mut value =
                    self.exact(self.list(self.take(&mut values), "identity"), 2, "identity");
                Identity {
                    name: self.string(self.take(&mut value)),
                    version: self.string(self.take(&mut value)),
                }
            };
            Header {
                identity,
                dependencies: self
                    .many(self.take(&mut values), "dependencies")
                    .into_iter()
                    .map(|value| self.read_dependency(value))
                    .collect(),
                values: self
                    .many(self.take(&mut values), "values")
                    .into_iter()
                    .map(|value| self.read_value(value))
                    .collect(),
                types: self
                    .many(self.take(&mut values), "types")
                    .into_iter()
                    .map(|value| self.read_declared_type(value))
                    .collect(),
                effects: self
                    .many(self.take(&mut values), "effects")
                    .into_iter()
                    .map(|value| self.read_effect(value))
                    .collect(),
            }
        }
        fn read_dependency(&self, value: S) -> Dependency {
            let mut value = self.exact(self.list(value, "dependency"), 2, "dependency");
            Dependency {
                name: self.string(self.take(&mut value)),
                version: self.string(self.take(&mut value)),
            }
        }
        fn read_value(&self, value: S) -> Value {
            let mut value = self.exact(self.list(value, "value"), 2, "value");
            Value {
                name: self.string(self.take(&mut value)),
                scheme: self.read_scheme(self.take(&mut value)),
            }
        }
        fn read_declared_type(&self, value: S) -> DeclaredType {
            let mut value = self.exact(self.list(value, "type"), 3, "type");
            DeclaredType {
                name: self.string(self.take(&mut value)),
                params: self
                    .many(self.take(&mut value), "params")
                    .into_iter()
                    .map(|value| self.read_parameter(value))
                    .collect(),
                scheme: self.read_scheme(self.take(&mut value)),
            }
        }
        fn read_parameter(&self, value: S) -> Parameter {
            let mut value = self.exact(self.list(value, "param"), 3, "param");
            Parameter {
                sense: match self.atom(self.take(&mut value)).as_str() {
                    "type" => Sense::Type,
                    "fields" => Sense::Fields,
                    "cases" => Sense::Cases,
                    "effects" => Sense::Effects,
                    _ => self.invalid("invalid parameter sense", Sense::Type),
                },
                relevant: self.boolean(self.take(&mut value)),
                lacks: self
                    .many(self.take(&mut value), "lacks")
                    .into_iter()
                    .map(|value| self.string(value))
                    .collect(),
            }
        }
        fn read_effect(&self, value: S) -> DeclaredEffect {
            let mut value = self.exact(self.list(value, "effect"), 3, "effect");
            let name = self.string(self.take(&mut value));
            let id = self.list(self.take(&mut value), "identity");
            let identity = match id.as_slice() {
                [A(none)] if none == "none" => None,
                _ => {
                    let mut id = self.exact(id, 2, "identity");
                    Some(EffectIdentity {
                        name: self.string(self.take(&mut id)),
                        interface: self.string(self.take(&mut id)),
                    })
                }
            };
            let kind = match list_contents(self.take(&mut value)) {
                Some(mut values) => {
                    let tag = self.atom(self.take(&mut values));
                    match tag.as_str() {
                        "operations" => EffectKind::Operations(
                            values
                                .into_iter()
                                .map(|value| self.read_operation(value))
                                .collect(),
                        ),
                        "alias" => EffectKind::Alias(
                            values.into_iter().map(|value| self.string(value)).collect(),
                        ),
                        _ => self.invalid("invalid effect kind", EffectKind::Alias(Vec::new())),
                    }
                }
                _ => self.invalid("invalid effect kind", EffectKind::Alias(Vec::new())),
            };
            DeclaredEffect {
                name,
                identity,
                kind,
            }
        }
        fn read_operation(&self, value: S) -> Operation {
            let mut value = self.exact(self.list(value, "operation"), 3, "operation");
            let selector = self.take(&mut value);
            let mut selector = self.list(selector, "selector");
            let tag = self.atom(self.take(&mut selector));
            let selector = match tag.as_str() {
                "unnamed" => {
                    self.exact(selector, 0, "selector unnamed");
                    OperationSelector::Unnamed
                }
                "named" => {
                    let mut selector = self.exact(selector, 1, "selector named");
                    OperationSelector::Named(self.string(self.take(&mut selector)))
                }
                _ => self.invalid("invalid operation selector", OperationSelector::Unnamed),
            };
            Operation {
                selector,
                from: self.read_ty(self.take(&mut value)),
                to: self.read_ty(self.take(&mut value)),
            }
        }
        fn read_scheme(&self, value: S) -> Scheme {
            let mut value = self.exact(self.list(value, "scheme"), 4, "scheme");
            Scheme {
                count: self.number(self.take(&mut value)),
                presences: self.number(self.take(&mut value)),
                formula: self.read_formula(self.take(&mut value)),
                body: self.read_ty(self.take(&mut value)),
            }
        }
        fn read_ty(&self, value: S) -> Type {
            enum Task {
                Ty(S),
                Row(S),
                Rest(S),
                Field(S),
                BuildArrow,
                BuildStruct,
                BuildSum,
                BuildNamed { name: String, count: usize },
                BuildRow { labels: Vec<String> },
                BuildMore,
                BuildField(Presence),
            }
            let mut tasks = vec![Task::Ty(value)];
            let (mut tys, mut rows, mut rests, mut fields_out) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            while let Some(task) = tasks.pop() {
                match task {
                    Task::Ty(value) => {
                        let mut wrapper = self.exact(self.list(value, "ty"), 1, "ty");
                        let mut value = self.take(&mut wrapper);
                        match &mut value {
                            A(value) => tys.push(match value.as_str() {
                                "nat" => Type::Nat,
                                "int" => Type::Int,
                                "real" => Type::Real,
                                "string" => Type::String,
                                "boolean" => Type::Boolean,
                                "undecided" => Type::Undecided,
                                _ => self.invalid("invalid type", Type::Undecided),
                            }),
                            L(values) => {
                                let mut values = std::mem::take(values);
                                match self.atom(self.take(&mut values)).as_str() {
                                    "arrow" => {
                                        let mut values = self.exact(values, 3, "arrow");
                                        let from = self.take(&mut values);
                                        let to = self.take(&mut values);
                                        let effects = self.take(&mut values);
                                        tasks.push(Task::BuildArrow);
                                        tasks.push(Task::Row(effects));
                                        tasks.push(Task::Ty(to));
                                        tasks.push(Task::Ty(from));
                                    }
                                    "struct" => {
                                        let row = self.exact(values, 1, "struct").remove(0);
                                        tasks.push(Task::BuildStruct);
                                        tasks.push(Task::Row(row));
                                    }
                                    "sum" => {
                                        let row = self.exact(values, 1, "sum").remove(0);
                                        tasks.push(Task::BuildSum);
                                        tasks.push(Task::Row(row));
                                    }
                                    "var" => tys.push(Type::Var(
                                        self.number(self.exact(values, 1, "var").remove(0)),
                                    )),
                                    "bound" => tys.push(Type::Bound(
                                        self.number(self.exact(values, 1, "bound").remove(0)),
                                    )),
                                    "rigid" => {
                                        let mut values = self.exact(values, 2, "rigid");
                                        let id = self.number(self.take(&mut values));
                                        let name = self.string(self.take(&mut values));
                                        tys.push(Type::Rigid { id, name });
                                    }
                                    "named" => {
                                        if values.is_empty() {
                                            self.fail("named type is missing name");
                                        }
                                        let name = self.string(self.take(&mut values));
                                        let count = values.len();
                                        tasks.push(Task::BuildNamed { name, count });
                                        for value in values.into_iter().rev() {
                                            tasks.push(Task::Ty(value));
                                        }
                                    }
                                    _ => tys.push(self.invalid("invalid type", Type::Undecided)),
                                }
                            }
                            _ => tys.push(self.invalid("invalid type", Type::Undecided)),
                        }
                    }
                    Task::Row(value) => {
                        let mut values = self.exact(self.list(value, "row"), 2, "row");
                        let labels = self.many(self.take(&mut values), "labels");
                        let rest = self.take(&mut values);
                        let mut names = Vec::with_capacity(labels.len());
                        let mut fields = Vec::with_capacity(labels.len());
                        for label in labels {
                            let values = list_contents(label)
                                .unwrap_or_else(|| self.invalid("bad row label", Vec::new()));
                            let mut values = self.exact(values, 2, "row label");
                            names.push(self.string(self.take(&mut values)));
                            fields.push(self.take(&mut values));
                        }
                        tasks.push(Task::BuildRow { labels: names });
                        tasks.push(Task::Rest(rest));
                        for value in fields.into_iter().rev() {
                            tasks.push(Task::Field(value));
                        }
                    }
                    Task::Rest(mut value) => match &mut value {
                        A(value) if value == "closed" => rests.push(Rest::Closed),
                        A(value) if value == "undecided" => rests.push(Rest::Undecided),
                        L(values) => {
                            let mut values = std::mem::take(values);
                            match self.atom(self.take(&mut values)).as_str() {
                                "var" => rests.push(Rest::Var(
                                    self.number(self.exact(values, 1, "rest var").remove(0)),
                                )),
                                "bound" => rests.push(Rest::Bound(
                                    self.number(self.exact(values, 1, "rest bound").remove(0)),
                                )),
                                "rigid" => {
                                    let mut values = self.exact(values, 2, "rest rigid");
                                    let id = self.number(self.take(&mut values));
                                    let name = self.string(self.take(&mut values));
                                    rests.push(Rest::Rigid { id, name });
                                }
                                "more" => {
                                    let value = self.exact(values, 1, "more").remove(0);
                                    tasks.push(Task::BuildMore);
                                    tasks.push(Task::Row(value));
                                }
                                _ => rests.push(self.invalid("invalid row rest", Rest::Undecided)),
                            }
                        }
                        _ => rests.push(self.invalid("invalid row rest", Rest::Undecided)),
                    },
                    Task::Field(value) => {
                        let mut values = self.exact(self.list(value, "field"), 2, "field");
                        let presence = self.read_presence(self.take(&mut values));
                        let ty = self.take(&mut values);
                        tasks.push(Task::BuildField(presence));
                        tasks.push(Task::Ty(ty));
                    }
                    Task::BuildField(presence) => {
                        let ty = tys.pop().expect("field type");
                        fields_out.push(RowField { presence, ty });
                    }
                    Task::BuildArrow => {
                        let effects = rows.pop().expect("effects");
                        let to = tys.pop().expect("to");
                        let from = tys.pop().expect("from");
                        tys.push(Type::Arrow(Box::new(from), Box::new(to), effects));
                    }
                    Task::BuildStruct => {
                        let row = rows.pop().expect("struct row");
                        tys.push(Type::Struct(row));
                    }
                    Task::BuildSum => {
                        let row = rows.pop().expect("sum row");
                        tys.push(Type::Sum(row));
                    }
                    Task::BuildNamed { name, count } => {
                        let split = tys.len() - count;
                        let args = tys.split_off(split);
                        tys.push(Type::Named { name, args });
                    }
                    Task::BuildRow { labels } => {
                        let rest = rests.pop().expect("rest");
                        let split = fields_out.len() - labels.len();
                        let fields = fields_out.split_off(split);
                        rows.push(Row {
                            labels: labels.into_iter().zip(fields).collect(),
                            rest,
                        });
                    }
                    Task::BuildMore => {
                        let row = rows.pop().expect("more row");
                        rests.push(Rest::More(Box::new(row)));
                    }
                }
            }
            tys.pop().expect("root type")
        }
        fn read_presence(&self, mut value: S) -> Presence {
            match &mut value {
                A(value) if value == "present" => Presence::Present,
                A(value) if value == "absent" => Presence::Absent,
                A(value) if value == "undecided" => Presence::Undecided,
                L(values) => {
                    let mut values = std::mem::take(values);
                    match self.atom(self.take(&mut values)).as_str() {
                        "var" => Presence::Var(
                            self.number(self.exact(values, 1, "presence var").remove(0)),
                        ),
                        "bound" => Presence::Bound(
                            self.number(self.exact(values, 1, "presence bound").remove(0)),
                        ),
                        _ => self.invalid("invalid presence", Presence::Undecided),
                    }
                }
                _ => self.invalid("invalid presence", Presence::Undecided),
            }
        }
        fn read_formula(&self, value: S) -> Formula {
            enum Task {
                Read(S),
                Not,
                Pair(fn(Box<Formula>, Box<Formula>) -> Formula),
            }
            let mut tasks = vec![Task::Read(value)];
            let mut out = Vec::new();
            while let Some(task) = tasks.pop() {
                match task {
                    Task::Not => {
                        let value = out.pop().unwrap_or(Formula::False);
                        out.push(Formula::Not(Box::new(value)));
                    }
                    Task::Pair(make) => {
                        let right = out.pop().unwrap_or(Formula::False);
                        let left = out.pop().unwrap_or(Formula::False);
                        out.push(make(Box::new(left), Box::new(right)));
                    }
                    Task::Read(mut value) => match &mut value {
                        A(value) if value == "true" => out.push(Formula::True),
                        A(value) if value == "false" => out.push(Formula::False),
                        L(values) => {
                            let mut values = std::mem::take(values);
                            let tag = self.atom(self.take(&mut values));
                            match tag.as_str() {
                                "var" => out.push(Formula::Var(
                                    self.number(self.exact(values, 1, "formula var").remove(0)),
                                )),
                                "bound" => out.push(Formula::Bound(
                                    self.number(self.exact(values, 1, "formula bound").remove(0)),
                                )),
                                "not" => {
                                    let value = self.exact(values, 1, "not").remove(0);
                                    tasks.push(Task::Not);
                                    tasks.push(Task::Read(value));
                                }
                                tag @ ("and" | "or" | "iff" | "xor") => {
                                    let make = match tag {
                                        "and" => Formula::And,
                                        "or" => Formula::Or,
                                        "iff" => Formula::Iff,
                                        _ => Formula::Xor,
                                    };
                                    let mut values = self.exact(values, 2, tag);
                                    let left = self.take(&mut values);
                                    let right = self.take(&mut values);
                                    tasks.push(Task::Pair(make));
                                    tasks.push(Task::Read(right));
                                    tasks.push(Task::Read(left));
                                }
                                _ => out.push(self.invalid("invalid formula", Formula::False)),
                            }
                        }
                        _ => out.push(self.invalid("invalid formula", Formula::False)),
                    },
                }
            }
            out.pop().unwrap_or(Formula::False)
        }

        fn read_lir(&self, value: S) -> Lir {
            let mut value = self.exact(self.list(value, "lir"), 3, "lir");
            Lir {
                externs: self
                    .many(self.take(&mut value), "externs")
                    .into_iter()
                    .map(|value| self.read_extern(value))
                    .collect(),
                functions: self
                    .many(self.take(&mut value), "functions")
                    .into_iter()
                    .map(|value| self.read_function(value))
                    .collect(),
                globals: self
                    .many(self.take(&mut value), "globals")
                    .into_iter()
                    .map(|value| self.read_global(value))
                    .collect(),
            }
        }
        fn read_extern(&self, value: S) -> Extern {
            let mut value = self.exact(self.list(value, "extern"), 3, "extern");
            let name = self.string(self.take(&mut value));
            let target: Vec<String> = self
                .many(self.take(&mut value), "target")
                .into_iter()
                .map(|value| self.string(value))
                .collect();
            if target.is_empty() || target.iter().any(String::is_empty) {
                self.fail("extern target must contain nonempty path segments");
            }
            Extern {
                name,
                target,
                rep: self.read_rep(self.take(&mut value)),
            }
        }
        fn read_function(&self, value: S) -> Function {
            let mut value = self.exact(self.list(value, "function"), 3, "function");
            Function {
                name: self.string(self.take(&mut value)),
                params: self
                    .many(self.take(&mut value), "params")
                    .into_iter()
                    .map(|value| self.read_param(value))
                    .collect(),
                body: self.read_block(self.take(&mut value)),
            }
        }
        fn read_param(&self, value: S) -> Param {
            let mut value = self.exact(self.list(value, "param"), 2, "param");
            Param {
                temp: self.number(self.take(&mut value)),
                rep: self.read_rep(self.take(&mut value)),
            }
        }
        fn read_global(&self, value: S) -> Global {
            let mut value = self.exact(self.list(value, "global"), 2, "global");
            Global {
                name: self.string(self.take(&mut value)),
                body: self.read_block(self.take(&mut value)),
            }
        }
        fn read_block(&self, value: S) -> Block {
            enum Task {
                Block(S),
                Instr(S),
                Op(S),
                BuildBlock {
                    count: usize,
                    end: End,
                },
                BuildInstr {
                    temp: u32,
                    rep: Rep,
                },
                Catch {
                    tag: u32,
                },
                SwitchTag {
                    on: u32,
                    names: Vec<String>,
                    fallback: bool,
                },
                SwitchPrim {
                    on: u32,
                    values: Vec<Literal>,
                    fallback: bool,
                },
                SwitchPresence {
                    on: u32,
                    field: String,
                },
                SwitchRest {
                    on: u32,
                    fields: Vec<String>,
                },
            }
            #[derive(Clone, Copy)]
            enum Recursive {
                Catch,
                SwitchTag,
                SwitchPrim,
                SwitchPresence,
                SwitchRest,
            }
            let mut tasks = vec![Task::Block(value)];
            // As in semantic type decoding, a task's result sort is fixed by
            // the task itself. Separate stacks make an impossible internal
            // sort mismatch unrepresentable as a malformed-input fallback.
            let mut blocks_out = Vec::new();
            let mut instrs_out = Vec::new();
            let mut ops = Vec::new();
            while let Some(task) = tasks.pop() {
                match task {
                    Task::Block(value) => {
                        let mut values = self.exact(self.list(value, "block"), 2, "block");
                        let instrs = self.many(self.take(&mut values), "instrs");
                        let end = self.read_end(self.take(&mut values));
                        tasks.push(Task::BuildBlock {
                            count: instrs.len(),
                            end,
                        });
                        for instr in instrs.into_iter().rev() {
                            tasks.push(Task::Instr(instr));
                        }
                    }
                    Task::Instr(value) => {
                        let mut values = self.exact(self.list(value, "instr"), 3, "instr");
                        let temp = self.number(self.take(&mut values));
                        let rep = self.read_rep(self.take(&mut values));
                        let op = self.take(&mut values);
                        tasks.push(Task::BuildInstr { temp, rep });
                        tasks.push(Task::Op(op));
                    }
                    Task::Op(value) => {
                        let recursive = match &value {
                            L(values) => match values.first() {
                                Some(A(tag)) => match tag.as_str() {
                                    "catch" => Some(Recursive::Catch),
                                    "switch-tag" => Some(Recursive::SwitchTag),
                                    "switch-prim" => Some(Recursive::SwitchPrim),
                                    "switch-presence" => Some(Recursive::SwitchPresence),
                                    "switch-rest" => Some(Recursive::SwitchRest),
                                    _ => None,
                                },
                                _ => None,
                            },
                            _ => None,
                        };
                        let Some(recursive) = recursive else {
                            ops.push(self.read_leaf_op(value));
                            continue;
                        };
                        let mut values = list_contents(value).expect("recursive op is a list");
                        self.take(&mut values);
                        match recursive {
                            Recursive::Catch => {
                                let mut values = self.exact(values, 2, "catch");
                                let tag = self.number(self.take(&mut values));
                                let body = self.take(&mut values);
                                tasks.push(Task::Catch { tag });
                                tasks.push(Task::Block(body));
                            }
                            Recursive::SwitchTag => {
                                let mut values = self.exact(values, 3, "switch-tag");
                                let on = self.number(self.take(&mut values));
                                let cases = self.many(self.take(&mut values), "cases");
                                let fallback = self.list(self.take(&mut values), "fallback");
                                if fallback.len() > 1 {
                                    self.fail("bad optional block");
                                }
                                let fallback = fallback.into_iter().next();
                                let mut names = Vec::with_capacity(cases.len());
                                let mut blocks = Vec::with_capacity(cases.len());
                                for case in cases {
                                    let values = list_contents(case).unwrap_or_else(|| {
                                        self.invalid("bad tag case", Vec::new())
                                    });
                                    let mut values = self.exact(values, 2, "tag case");
                                    names.push(self.string(self.take(&mut values)));
                                    blocks.push(self.take(&mut values));
                                }
                                tasks.push(Task::SwitchTag {
                                    on,
                                    names,
                                    fallback: fallback.is_some(),
                                });
                                if let Some(block) = fallback {
                                    tasks.push(Task::Block(block));
                                }
                                for block in blocks.into_iter().rev() {
                                    tasks.push(Task::Block(block));
                                }
                            }
                            Recursive::SwitchPrim => {
                                let mut values = self.exact(values, 3, "switch-prim");
                                let on = self.number(self.take(&mut values));
                                let cases = self.many(self.take(&mut values), "cases");
                                let fallback = self.list(self.take(&mut values), "fallback");
                                if fallback.len() > 1 {
                                    self.fail("bad optional block");
                                }
                                let fallback = fallback.into_iter().next();
                                let mut literals = Vec::with_capacity(cases.len());
                                let mut blocks = Vec::with_capacity(cases.len());
                                for case in cases {
                                    let values = list_contents(case).unwrap_or_else(|| {
                                        self.invalid("bad primitive case", Vec::new())
                                    });
                                    let mut values = self.exact(values, 2, "primitive case");
                                    literals.push(self.read_literal(self.take(&mut values)));
                                    blocks.push(self.take(&mut values));
                                }
                                tasks.push(Task::SwitchPrim {
                                    on,
                                    values: literals,
                                    fallback: fallback.is_some(),
                                });
                                if let Some(block) = fallback {
                                    tasks.push(Task::Block(block));
                                }
                                for block in blocks.into_iter().rev() {
                                    tasks.push(Task::Block(block));
                                }
                            }
                            Recursive::SwitchPresence => {
                                let mut values = self.exact(values, 4, "switch-presence");
                                let on = self.number(self.take(&mut values));
                                let field = self.string(self.take(&mut values));
                                let present = self.take(&mut values);
                                let absent = self.take(&mut values);
                                tasks.push(Task::SwitchPresence { on, field });
                                tasks.push(Task::Block(absent));
                                tasks.push(Task::Block(present));
                            }
                            Recursive::SwitchRest => {
                                let mut values = self.exact(values, 4, "switch-rest");
                                let on = self.number(self.take(&mut values));
                                let fields = self
                                    .many(self.take(&mut values), "fields")
                                    .into_iter()
                                    .map(|v| self.string(v))
                                    .collect();
                                let none = self.take(&mut values);
                                let some = self.take(&mut values);
                                tasks.push(Task::SwitchRest { on, fields });
                                tasks.push(Task::Block(some));
                                tasks.push(Task::Block(none));
                            }
                        }
                    }
                    Task::BuildInstr { temp, rep } => {
                        let op = ops
                            .pop()
                            .expect("an instruction task produces an operation");
                        instrs_out.push(Instr { temp, rep, op });
                    }
                    Task::BuildBlock { count, end } => {
                        let split = instrs_out.len() - count;
                        let instrs = instrs_out.split_off(split);
                        blocks_out.push(Block { instrs, end });
                    }
                    Task::Catch { tag } => {
                        let body = pop_block(&mut blocks_out);
                        ops.push(Op::Catch {
                            tag,
                            body: Box::new(body),
                        });
                    }
                    Task::SwitchTag {
                        on,
                        names,
                        fallback,
                    } => {
                        let fallback = fallback.then(|| Box::new(pop_block(&mut blocks_out)));
                        let split = blocks_out.len() - names.len();
                        let blocks = blocks_out.split_off(split);
                        ops.push(Op::SwitchTag {
                            on,
                            cases: names
                                .into_iter()
                                .zip(blocks)
                                .map(|(name, block)| TagCase { name, block })
                                .collect(),
                            fallback,
                        });
                    }
                    Task::SwitchPrim {
                        on,
                        values,
                        fallback,
                    } => {
                        let fallback = fallback.then(|| Box::new(pop_block(&mut blocks_out)));
                        let split = blocks_out.len() - values.len();
                        let blocks = blocks_out.split_off(split);
                        ops.push(Op::SwitchPrim {
                            on,
                            cases: values
                                .into_iter()
                                .zip(blocks)
                                .map(|(value, block)| PrimCase { value, block })
                                .collect(),
                            fallback,
                        });
                    }
                    Task::SwitchPresence { on, field } => {
                        let absent = pop_block(&mut blocks_out);
                        let present = pop_block(&mut blocks_out);
                        ops.push(Op::SwitchPresence {
                            on,
                            field,
                            present: Box::new(present),
                            absent: Box::new(absent),
                        });
                    }
                    Task::SwitchRest { on, fields } => {
                        let some = pop_block(&mut blocks_out);
                        let none = pop_block(&mut blocks_out);
                        ops.push(Op::SwitchRest {
                            on,
                            fields,
                            none: Box::new(none),
                            some: Box::new(some),
                        });
                    }
                }
            }
            return pop_block(&mut blocks_out);

            fn pop_block(out: &mut Vec<Block>) -> Block {
                out.pop().expect("a block task produces a block")
            }
        }
        fn read_rep(&self, value: S) -> Rep {
            match self.atom(value).as_str() {
                "nat" => Rep::Nat,
                "int" => Rep::Int,
                "real" => Rep::Real,
                "string" => Rep::String,
                "boolean" => Rep::Boolean,
                "unit" => Rep::Unit,
                "struct" => Rep::Struct,
                "sum" => Rep::Sum,
                "fn" => Rep::Fn,
                "any" => Rep::Any,
                _ => self.invalid("invalid representation", Rep::Any),
            }
        }
        fn read_field_key(&self, value: S) -> FieldKey {
            let Some(values) = list_contents(value) else {
                return self.invalid("invalid field key", FieldKey::UnnamedOperation);
            };
            let mut values = values;
            if self.atom(self.take(&mut values)) != "field-key" {
                return self.invalid("invalid field key", FieldKey::UnnamedOperation);
            }
            match self.atom(self.take(&mut values)).as_str() {
                "named" => {
                    let mut values = self.exact(values, 1, "named field key");
                    FieldKey::Named(self.string(self.take(&mut values)))
                }
                "unnamed-operation" => {
                    self.exact(values, 0, "unnamed operation field key");
                    FieldKey::UnnamedOperation
                }
                _ => self.invalid("invalid field key", FieldKey::UnnamedOperation),
            }
        }
        fn read_leaf_op(&self, value: S) -> Op {
            if matches!(&value, A(atom) if atom == "new-tag") {
                return Op::NewTag;
            }
            let Some(mut values) = list_contents(value) else {
                return self.invalid("invalid operation", Op::NewTag);
            };
            let tag = self.atom(self.take(&mut values));
            match tag.as_str() {
                "const" => Op::Const(self.read_literal(self.exact(values, 1, "const").remove(0))),
                "neg" => Op::Neg(self.number(self.exact(values, 1, "neg").remove(0))),
                "not" => Op::Not(self.number(self.exact(values, 1, "not").remove(0))),
                "and" => self.op_binary(values, |left, right| Op::And { left, right }, "and"),
                "or" => self.op_binary(values, |left, right| Op::Or { left, right }, "or"),
                "xor" => self.op_binary(values, |left, right| Op::Xor { left, right }, "xor"),
                "add" => self.op_binary(values, |left, right| Op::Add { left, right }, "add"),
                "sub" => self.op_binary(values, |left, right| Op::Sub { left, right }, "sub"),
                "mul" => self.op_binary(values, |left, right| Op::Mul { left, right }, "mul"),
                "div" => self.op_binary(values, |left, right| Op::Div { left, right }, "div"),
                "struct" => Op::Struct(
                    values
                        .into_iter()
                        .map(|value| {
                            let value = list_contents(value)
                                .unwrap_or_else(|| self.invalid("bad struct entry", Vec::new()));
                            let mut value = self.exact(value, 2, "struct entry");
                            (
                                self.read_field_key(self.take(&mut value)),
                                self.number(self.take(&mut value)),
                            )
                        })
                        .collect(),
                ),
                "merge" => Op::Merge(values.into_iter().map(|value| self.number(value)).collect()),
                "project" => {
                    let mut values = self.exact(values, 2, "project");
                    Op::Project {
                        base: self.number(self.take(&mut values)),
                        field: self.read_field_key(self.take(&mut values)),
                    }
                }
                "tag" => {
                    if values.is_empty() || values.len() > 2 {
                        self.fail("bad tag");
                        values.truncate(2);
                    }
                    let name = self.string(self.take(&mut values));
                    Op::Tag {
                        name,
                        payload: values.pop().map(|value| self.number(value)),
                    }
                }
                "payload" => Op::Payload(self.number(self.exact(values, 1, "payload").remove(0))),
                "closure" => {
                    let mut values = self.exact(values, 2, "closure");
                    Op::Closure {
                        func: self.number(self.take(&mut values)),
                        captures: self
                            .many(self.take(&mut values), "captures")
                            .into_iter()
                            .map(|value| self.number(value))
                            .collect(),
                    }
                }
                "call" => {
                    let mut values = self.exact(values, 2, "call");
                    let callee = match list_contents(self.take(&mut values)) {
                        Some(target) => {
                            let mut target = self.exact(target, 2, "call target");
                            match self.atom(self.take(&mut target)).as_str() {
                                "direct" => Callee::Direct(self.number(self.take(&mut target))),
                                "indirect" => Callee::Indirect(self.number(self.take(&mut target))),
                                _ => self.invalid("bad call target", Callee::Indirect(0)),
                            }
                        }
                        _ => self.invalid("bad call target", Callee::Indirect(0)),
                    };
                    let args = self
                        .many(self.take(&mut values), "args")
                        .into_iter()
                        .map(|value| self.number(value))
                        .collect();
                    Op::Call { callee, args }
                }
                "raw-call" => {
                    let mut values = self.exact(values, 2, "raw-call");
                    let callee = self.number(self.take(&mut values));
                    let args = self
                        .many(self.take(&mut values), "args")
                        .into_iter()
                        .map(|value| self.number(value))
                        .collect();
                    Op::RawCall { callee, args }
                }
                "extern" => Op::Extern {
                    target: self.string(self.exact(values, 1, "extern").remove(0)),
                },
                "global" => Op::Global {
                    target: self.string(self.exact(values, 1, "global").remove(0)),
                },
                _ => self.invalid("invalid operation", Op::NewTag),
            }
        }
        fn op_binary(&self, values: Vec<S>, make: fn(u32, u32) -> Op, tag: &str) -> Op {
            let mut values = self.exact(values, 2, tag);
            make(
                self.number(self.take(&mut values)),
                self.number(self.take(&mut values)),
            )
        }
        fn read_end(&self, value: S) -> End {
            let mut values = list_contents(value)
                .unwrap_or_else(|| self.invalid("invalid terminator", Vec::new()));
            let tag = self.atom(self.take(&mut values));
            match tag.as_str() {
                "ret" => End::Ret(self.number(self.exact(values, 1, "ret").remove(0))),
                "yield" => End::Yield(self.number(self.exact(values, 1, "yield").remove(0))),
                "throw" => {
                    let mut values = self.exact(values, 2, "throw");
                    End::Throw {
                        tag: self.number(self.take(&mut values)),
                        value: self.number(self.take(&mut values)),
                    }
                }
                _ => self.invalid("invalid terminator", End::Ret(0)),
            }
        }
        fn read_literal(&self, value: S) -> Literal {
            let mut values =
                list_contents(value).unwrap_or_else(|| self.invalid("invalid literal", Vec::new()));
            let tag = self.atom(self.take(&mut values));
            match tag.as_str() {
                "nat" => {
                    Literal::Natural(self.number(self.exact(values, 1, "nat literal").remove(0)))
                }
                "int" => {
                    Literal::Integer(self.number(self.exact(values, 1, "int literal").remove(0)))
                }
                "real" => {
                    Literal::Real(self.number(self.exact(values, 1, "real literal").remove(0)))
                }
                "string" => {
                    Literal::String(self.string(self.exact(values, 1, "string literal").remove(0)))
                }
                "bool" => {
                    Literal::Boolean(self.boolean(self.exact(values, 1, "bool literal").remove(0)))
                }
                _ => self.invalid("invalid literal", Literal::Boolean(false)),
            }
        }
    }
}
