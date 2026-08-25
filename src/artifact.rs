//! A portable, span-free bundle artifact.
//!
//! Artifacts are the compiler's disk boundary.  They intentionally contain no
//! source locations, file paths, or [`crate::symbol::Symbol`]s: external value
//! references are qualified by the identity of the bundle that owns them.
//! [`parse`] accepts only compiler-produced text and deliberately panics for
//! malformed input.  This is an internal v1 format, not a compatibility
//! promise.

use crate::{
    inference, ir, lir,
    symbol::{Mint, Symbol},
    types,
};

/// A complete, serializable bundle artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub header: Header,
    pub lir: Lir,
}

/// The public interface of one bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub identity: Identity,
    /// The bundles this artifact depends on.
    pub dependencies: Vec<Dependency>,
    /// Every top-level `let`, in source declaration order.
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
    pub name: String,
    pub from: Type,
    pub to: Type,
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

/// A normalized semantic type, independent of compiler symbols and spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub core: Core,
    pub fields: Vec<(String, RowField)>,
}

/// A type's core, before its structural fields are laid over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Core {
    Unit,
    Nat,
    Int,
    Real,
    String,
    Boolean,
    Arrow(Box<Type>, Box<Type>, Row),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub labels: Vec<(String, RowField)>,
    pub rest: Rest,
}

/// The part of a sum or effect row beyond its named labels.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The span-free LIR portion of an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lir {
    pub functions: Vec<Function>,
    pub globals: Vec<Global>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A span-free LIR operation.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Struct(Vec<(String, u32)>),
    Merge(Vec<u32>),
    Project {
        base: u32,
        field: String,
    },
    Tag {
        name: String,
        payload: Option<u32>,
    },
    Payload(u32),
    Closure {
        func: usize,
        captures: Vec<u32>,
    },
    Call {
        callee: Callee,
        args: Vec<u32>,
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
    Direct(usize),
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

/// Build an artifact after inference and LIR lowering succeeded.
pub fn build(
    mint: &Mint,
    program: &ir::Program,
    inference: &inference::Output,
    lir: &lir::Output,
) -> Artifact {
    let header = Header {
        identity: Identity {
            name: mint.bundle().name().to_string(),
            version: mint.bundle().version().to_string(),
        },
        dependencies: Vec::new(),
        values: program
            .terms
            .keys()
            .map(|symbol| Value {
                name: qualified(mint, *symbol),
                scheme: scheme(mint, &inference.schemes[symbol]),
            })
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
                        sense: match param.kind.sense() {
                            types::Sense::Type => Sense::Type,
                            types::Sense::Cases => Sense::Cases,
                            types::Sense::Effects => Sense::Effects,
                            types::Sense::Presence => {
                                panic!("a type parameter cannot be a presence")
                            }
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
                                    name: name.clone(),
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
}

/// Print canonical artifact text.
pub fn print(artifact: &Artifact) -> String {
    text::print(artifact)
}
/// Parse trusted internal artifact text. Malformed input panics.
pub fn parse(input: &str) -> Artifact {
    text::parse(input)
}

fn qualified(mint: &Mint, symbol: Symbol) -> QualifiedName {
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
    Type {
        core: match &value.core {
            types::Core::Unit => Core::Unit,
            types::Core::Nat => Core::Nat,
            types::Core::Int => Core::Int,
            types::Core::Real => Core::Real,
            types::Core::String => Core::String,
            types::Core::Boolean => Core::Boolean,
            types::Core::Arrow(from, to, effects) => Core::Arrow(
                Box::new(ty(mint, from)),
                Box::new(ty(mint, to)),
                row(mint, effects),
            ),
            types::Core::Sum(row_) => Core::Sum(row(mint, row_)),
            types::Core::Var(value) => Core::Var(*value),
            types::Core::Bound(value) => Core::Bound(*value),
            types::Core::Rigid { id, name } => Core::Rigid {
                id: *id,
                name: name.to_string(),
            },
            types::Core::Named { symbol, args, .. } => Core::Named {
                name: qualified(mint, *symbol),
                args: args.iter().map(|arg| ty(mint, arg)).collect(),
            },
            types::Core::Undecided => Core::Undecided,
        },
        fields: value
            .fields
            .iter()
            .map(|(name, field)| (name.clone(), row_field(mint, field)))
            .collect(),
    }
}

fn row(mint: &Mint, value: &types::Row) -> Row {
    Row {
        labels: value
            .labels
            .iter()
            .map(|(name, field)| (name.clone(), row_field(mint, field)))
            .collect(),
        rest: match &value.rest {
            types::Rest::Closed => Rest::Closed,
            types::Rest::Var(value) => Rest::Var(*value),
            types::Rest::Bound(value) => Rest::Bound(*value),
            types::Rest::Rigid { id, name } => Rest::Rigid {
                id: *id,
                name: name.to_string(),
            },
            types::Rest::Undecided => Rest::Undecided,
            types::Rest::More(row_) => Rest::More(Box::new(row(mint, row_))),
        },
    }
}

fn row_field(mint: &Mint, value: &types::RowField) -> RowField {
    RowField {
        presence: match &value.presence {
            types::Presence::Present => Presence::Present,
            types::Presence::Absent => Presence::Absent,
            types::Presence::Var(value) => Presence::Var(*value),
            types::Presence::Bound(value) => Presence::Bound(*value),
            types::Presence::Undecided => Presence::Undecided,
        },
        ty: ty(mint, &value.ty),
    }
}

fn formula(value: &types::Formula) -> Formula {
    match value {
        types::Formula::True => Formula::True,
        types::Formula::False => Formula::False,
        types::Formula::Atom(types::Atom::Var(value)) => Formula::Var(*value),
        types::Formula::Atom(types::Atom::Bound(value)) => Formula::Bound(*value),
        types::Formula::Not(value) => Formula::Not(Box::new(formula(value))),
        types::Formula::And(left, right) => {
            Formula::And(Box::new(formula(left)), Box::new(formula(right)))
        }
        types::Formula::Or(left, right) => {
            Formula::Or(Box::new(formula(left)), Box::new(formula(right)))
        }
        types::Formula::Iff(left, right) => {
            Formula::Iff(Box::new(formula(left)), Box::new(formula(right)))
        }
        types::Formula::Xor(left, right) => {
            Formula::Xor(Box::new(formula(left)), Box::new(formula(right)))
        }
    }
}

fn lower_lir(mint: &Mint, output: &lir::Output) -> Lir {
    Lir {
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
                .map(|(name, temp)| (name.clone(), *temp))
                .collect(),
        ),
        Source::Merge(values) => Op::Merge(values.clone()),
        Source::Project { base, field } => Op::Project {
            base: *base,
            field: field.clone(),
        },
        Source::Tag { name, payload } => Op::Tag {
            name: name.clone(),
            payload: *payload,
        },
        Source::Payload(value) => Op::Payload(*value),
        Source::Closure { func, captures } => Op::Closure {
            func: *func,
            captures: captures.clone(),
        },
        Source::Call { callee, args } => Op::Call {
            callee: match callee {
                lir::Callee::Direct(value) => Callee::Direct(*value),
                lir::Callee::Indirect(value) => Callee::Indirect(*value),
            },
            args: args.clone(),
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
        List(Vec<S>),
    }
    use S::{Atom as A, List as L, Str as Q};

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
        let mut parser = Parser { input, at: 0 };
        let value = parser.value();
        parser.space();
        assert!(parser.at == input.len(), "trailing artifact text");
        read_artifact(value)
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
        L(vec![
            A("operation".into()),
            Q(value.name.clone()),
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
        L(vec![
            A("ty".into()),
            core(&value.core),
            L(std::iter::once(A("fields".into()))
                .chain(
                    value
                        .fields
                        .iter()
                        .map(|(name, field)| L(vec![Q(name.clone()), row_field(field)])),
                )
                .collect()),
        ])
    }
    fn core(value: &Core) -> S {
        match value {
            Core::Unit => A("unit".into()),
            Core::Nat => A("nat".into()),
            Core::Int => A("int".into()),
            Core::Real => A("real".into()),
            Core::String => A("string".into()),
            Core::Boolean => A("boolean".into()),
            Core::Arrow(from, to, row_) => L(vec![A("arrow".into()), ty(from), ty(to), row(row_)]),
            Core::Sum(row_) => L(vec![A("sum".into()), row(row_)]),
            Core::Var(value) => L(vec![A("var".into()), A(value.to_string())]),
            Core::Bound(value) => L(vec![A("bound".into()), A(value.to_string())]),
            Core::Rigid { id, name } => {
                L(vec![A("rigid".into()), A(id.to_string()), Q(name.clone())])
            }
            Core::Named { name, args } => L(std::iter::once(A("named".into()))
                .chain(std::iter::once(Q(name.clone())))
                .chain(args.iter().map(ty))
                .collect()),
            Core::Undecided => A("undecided".into()),
        }
    }
    fn row(value: &Row) -> S {
        L(vec![
            A("row".into()),
            L(std::iter::once(A("labels".into()))
                .chain(
                    value
                        .labels
                        .iter()
                        .map(|(name, field)| L(vec![Q(name.clone()), row_field(field)])),
                )
                .collect()),
            rest(&value.rest),
        ])
    }
    fn rest(value: &Rest) -> S {
        match value {
            Rest::Closed => A("closed".into()),
            Rest::Var(value) => L(vec![A("var".into()), A(value.to_string())]),
            Rest::Bound(value) => L(vec![A("bound".into()), A(value.to_string())]),
            Rest::Rigid { id, name } => {
                L(vec![A("rigid".into()), A(id.to_string()), Q(name.clone())])
            }
            Rest::Undecided => A("undecided".into()),
            Rest::More(value) => L(vec![A("more".into()), row(value)]),
        }
    }
    fn row_field(value: &RowField) -> S {
        L(vec![
            A("field".into()),
            presence(&value.presence),
            ty(&value.ty),
        ])
    }
    fn presence(value: &Presence) -> S {
        match value {
            Presence::Present => A("present".into()),
            Presence::Absent => A("absent".into()),
            Presence::Var(value) => L(vec![A("var".into()), A(value.to_string())]),
            Presence::Bound(value) => L(vec![A("bound".into()), A(value.to_string())]),
            Presence::Undecided => A("undecided".into()),
        }
    }
    fn formula(value: &Formula) -> S {
        match value {
            Formula::True => A("true".into()),
            Formula::False => A("false".into()),
            Formula::Var(value) => L(vec![A("var".into()), A(value.to_string())]),
            Formula::Bound(value) => L(vec![A("bound".into()), A(value.to_string())]),
            Formula::Not(value) => L(vec![A("not".into()), formula(value)]),
            Formula::And(left, right) => pair("and", left, right),
            Formula::Or(left, right) => pair("or", left, right),
            Formula::Iff(left, right) => pair("iff", left, right),
            Formula::Xor(left, right) => pair("xor", left, right),
        }
    }
    fn pair(tag: &str, left: &Formula, right: &Formula) -> S {
        L(vec![A(tag.into()), formula(left), formula(right)])
    }

    fn lir(value: &Lir) -> S {
        L(vec![
            A("lir".into()),
            L(std::iter::once(A("functions".into()))
                .chain(value.functions.iter().map(function))
                .collect()),
            L(std::iter::once(A("globals".into()))
                .chain(value.globals.iter().map(global))
                .collect()),
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
                        .map(|(name, value)| L(vec![Q(name.clone()), A(value.to_string())])),
                )
                .collect()),
            Op::Merge(values) => L(std::iter::once(A("merge".into()))
                .chain(values.iter().map(|value| A(value.to_string())))
                .collect()),
            Op::Project { base, field } => L(vec![
                A("project".into()),
                A(base.to_string()),
                Q(field.clone()),
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
        match value {
            A(value) => RcDoc::text(value.as_str()),
            Q(value) => RcDoc::text(quoted(value)),
            L(values) => RcDoc::text("(")
                .append(RcDoc::intersperse(values.iter().map(doc), RcDoc::line()).nest(2))
                .append(")")
                .group(),
        }
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
            while self.input[self.at..].starts_with(char::is_whitespace) {
                self.at += self.input[self.at..].chars().next().unwrap().len_utf8();
            }
        }
        fn value(&mut self) -> S {
            self.space();
            match self.peek() {
                Some('(') => {
                    self.at += 1;
                    let mut values = Vec::new();
                    loop {
                        self.space();
                        match self.peek() {
                            Some(')') => {
                                self.at += 1;
                                break;
                            }
                            Some(_) => values.push(self.value()),
                            None => panic!("unterminated artifact list"),
                        }
                    }
                    L(values)
                }
                Some('"') => self.string(),
                Some(_) => self.atom(),
                None => panic!("truncated artifact text"),
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
            assert!(start != self.at, "expected artifact atom");
            A(self.input[start..self.at].to_string())
        }
        fn string(&mut self) -> S {
            self.at += 1;
            let mut out = String::new();
            loop {
                let c = self.peek().expect("unterminated artifact string");
                self.at += c.len_utf8();
                match c {
                    '"' => break,
                    '\\' => {
                        let escape = self.peek().expect("truncated artifact escape");
                        self.at += escape.len_utf8();
                        out.push(match escape {
                            '\\' => '\\',
                            '"' => '"',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            'u' => self.control_escape(),
                            _ => panic!("invalid artifact escape"),
                        });
                    }
                    c if c.is_control() => panic!("unescaped control in artifact string"),
                    c => out.push(c),
                }
            }
            Q(out)
        }
        fn control_escape(&mut self) -> char {
            let mut value = 0;
            for _ in 0..4 {
                let digit = self.peek().expect("truncated artifact control escape");
                self.at += digit.len_utf8();
                value = value * 16 + digit.to_digit(16).expect("invalid artifact control escape");
            }
            let control = char::from_u32(value).expect("invalid artifact control escape");
            assert!(
                control.is_control() && !matches!(control, '\n' | '\r' | '\t'),
                "invalid artifact control escape"
            );
            control
        }
    }

    fn list(value: S, tag: &str) -> Vec<S> {
        match value {
            L(mut values) => {
                assert!(
                    matches!(values.first(), Some(A(found)) if found == tag),
                    "expected `{tag}`"
                );
                values.remove(0);
                values
            }
            _ => panic!("expected `{tag}` list"),
        }
    }
    fn atom(value: S) -> String {
        match value {
            A(value) => value,
            _ => panic!("expected artifact atom"),
        }
    }
    fn string(value: S) -> String {
        match value {
            Q(value) => value,
            _ => panic!("expected artifact string"),
        }
    }
    fn exact(values: Vec<S>, count: usize, tag: &str) -> Vec<S> {
        assert!(values.len() == count, "bad `{tag}` arity");
        values
    }
    fn number<T: std::str::FromStr>(value: S) -> T {
        atom(value).parse().ok().expect("invalid artifact number")
    }
    fn boolean(value: S) -> bool {
        match atom(value).as_str() {
            "true" => true,
            "false" => false,
            _ => panic!("invalid artifact boolean"),
        }
    }
    fn many(value: S, tag: &str) -> Vec<S> {
        list(value, tag)
    }

    fn read_artifact(value: S) -> Artifact {
        let mut values = exact(list(value, "artifact"), 2, "artifact");
        Artifact {
            header: read_header(values.remove(0)),
            lir: read_lir(values.remove(0)),
        }
    }
    fn read_header(value: S) -> Header {
        let mut values = exact(list(value, "header"), 5, "header");
        let identity = {
            let mut value = exact(list(values.remove(0), "identity"), 2, "identity");
            Identity {
                name: string(value.remove(0)),
                version: string(value.remove(0)),
            }
        };
        Header {
            identity,
            dependencies: many(values.remove(0), "dependencies")
                .into_iter()
                .map(read_dependency)
                .collect(),
            values: many(values.remove(0), "values")
                .into_iter()
                .map(read_value)
                .collect(),
            types: many(values.remove(0), "types")
                .into_iter()
                .map(read_declared_type)
                .collect(),
            effects: many(values.remove(0), "effects")
                .into_iter()
                .map(read_effect)
                .collect(),
        }
    }
    fn read_dependency(value: S) -> Dependency {
        let mut value = exact(list(value, "dependency"), 2, "dependency");
        Dependency {
            name: string(value.remove(0)),
            version: string(value.remove(0)),
        }
    }
    fn read_value(value: S) -> Value {
        let mut value = exact(list(value, "value"), 2, "value");
        Value {
            name: string(value.remove(0)),
            scheme: read_scheme(value.remove(0)),
        }
    }
    fn read_declared_type(value: S) -> DeclaredType {
        let mut value = exact(list(value, "type"), 3, "type");
        DeclaredType {
            name: string(value.remove(0)),
            params: many(value.remove(0), "params")
                .into_iter()
                .map(read_parameter)
                .collect(),
            scheme: read_scheme(value.remove(0)),
        }
    }
    fn read_parameter(value: S) -> Parameter {
        let mut value = exact(list(value, "param"), 3, "param");
        Parameter {
            sense: match atom(value.remove(0)).as_str() {
                "type" => Sense::Type,
                "cases" => Sense::Cases,
                "effects" => Sense::Effects,
                _ => panic!("invalid parameter sense"),
            },
            relevant: boolean(value.remove(0)),
            lacks: many(value.remove(0), "lacks")
                .into_iter()
                .map(string)
                .collect(),
        }
    }
    fn read_effect(value: S) -> DeclaredEffect {
        let mut value = exact(list(value, "effect"), 3, "effect");
        let name = string(value.remove(0));
        let id = list(value.remove(0), "identity");
        let identity = match id.as_slice() {
            [A(none)] if none == "none" => None,
            _ => {
                let mut id = exact(id, 2, "identity");
                Some(EffectIdentity {
                    name: string(id.remove(0)),
                    interface: string(id.remove(0)),
                })
            }
        };
        let kind = match value.remove(0) {
            L(mut values) => {
                let tag = atom(values.remove(0));
                match tag.as_str() {
                    "operations" => {
                        EffectKind::Operations(values.into_iter().map(read_operation).collect())
                    }
                    "alias" => EffectKind::Alias(values.into_iter().map(string).collect()),
                    _ => panic!("invalid effect kind"),
                }
            }
            _ => panic!("invalid effect kind"),
        };
        DeclaredEffect {
            name,
            identity,
            kind,
        }
    }
    fn read_operation(value: S) -> Operation {
        let mut value = exact(list(value, "operation"), 3, "operation");
        Operation {
            name: string(value.remove(0)),
            from: read_ty(value.remove(0)),
            to: read_ty(value.remove(0)),
        }
    }
    fn read_scheme(value: S) -> Scheme {
        let mut value = exact(list(value, "scheme"), 4, "scheme");
        Scheme {
            count: number(value.remove(0)),
            presences: number(value.remove(0)),
            formula: read_formula(value.remove(0)),
            body: read_ty(value.remove(0)),
        }
    }
    fn read_ty(value: S) -> Type {
        let mut value = exact(list(value, "ty"), 2, "ty");
        let core = read_core(value.remove(0));
        let fields = many(value.remove(0), "fields")
            .into_iter()
            .map(|value| {
                let mut value = exact(
                    match value {
                        L(values) => values,
                        _ => panic!("bad type field"),
                    },
                    2,
                    "type field",
                );
                (string(value.remove(0)), read_row_field(value.remove(0)))
            })
            .collect();
        Type { core, fields }
    }
    fn read_core(value: S) -> Core {
        match value {
            A(value) => match value.as_str() {
                "unit" => Core::Unit,
                "nat" => Core::Nat,
                "int" => Core::Int,
                "real" => Core::Real,
                "string" => Core::String,
                "boolean" => Core::Boolean,
                "undecided" => Core::Undecided,
                _ => panic!("invalid type core"),
            },
            L(mut values) => {
                let tag = atom(values.remove(0));
                match tag.as_str() {
                    "arrow" => {
                        let mut values = exact(values, 3, "arrow");
                        Core::Arrow(
                            Box::new(read_ty(values.remove(0))),
                            Box::new(read_ty(values.remove(0))),
                            read_row(values.remove(0)),
                        )
                    }
                    "sum" => Core::Sum(read_row(exact(values, 1, "sum").remove(0))),
                    "var" => Core::Var(number(exact(values, 1, "var").remove(0))),
                    "bound" => Core::Bound(number(exact(values, 1, "bound").remove(0))),
                    "rigid" => {
                        let mut values = exact(values, 2, "rigid");
                        Core::Rigid {
                            id: number(values.remove(0)),
                            name: string(values.remove(0)),
                        }
                    }
                    "named" => {
                        assert!(!values.is_empty(), "named type is missing name");
                        let name = string(values.remove(0));
                        Core::Named {
                            name,
                            args: values.into_iter().map(read_ty).collect(),
                        }
                    }
                    _ => panic!("invalid type core"),
                }
            }
            _ => panic!("invalid type core"),
        }
    }
    fn read_row(value: S) -> Row {
        let mut value = exact(list(value, "row"), 2, "row");
        let labels = many(value.remove(0), "labels")
            .into_iter()
            .map(|value| {
                let value = match value {
                    L(values) => values,
                    _ => panic!("bad row label"),
                };
                let mut value = exact(value, 2, "row label");
                (string(value.remove(0)), read_row_field(value.remove(0)))
            })
            .collect();
        Row {
            labels,
            rest: read_rest(value.remove(0)),
        }
    }
    fn read_rest(value: S) -> Rest {
        match value {
            A(value) if value == "closed" => Rest::Closed,
            A(value) if value == "undecided" => Rest::Undecided,
            L(mut values) => match atom(values.remove(0)).as_str() {
                "var" => Rest::Var(number(exact(values, 1, "rest var").remove(0))),
                "bound" => Rest::Bound(number(exact(values, 1, "rest bound").remove(0))),
                "rigid" => {
                    let mut values = exact(values, 2, "rest rigid");
                    Rest::Rigid {
                        id: number(values.remove(0)),
                        name: string(values.remove(0)),
                    }
                }
                "more" => Rest::More(Box::new(read_row(exact(values, 1, "more").remove(0)))),
                _ => panic!("invalid row rest"),
            },
            _ => panic!("invalid row rest"),
        }
    }
    fn read_row_field(value: S) -> RowField {
        let mut value = exact(list(value, "field"), 2, "field");
        RowField {
            presence: read_presence(value.remove(0)),
            ty: read_ty(value.remove(0)),
        }
    }
    fn read_presence(value: S) -> Presence {
        match value {
            A(value) if value == "present" => Presence::Present,
            A(value) if value == "absent" => Presence::Absent,
            A(value) if value == "undecided" => Presence::Undecided,
            L(mut values) => match atom(values.remove(0)).as_str() {
                "var" => Presence::Var(number(exact(values, 1, "presence var").remove(0))),
                "bound" => Presence::Bound(number(exact(values, 1, "presence bound").remove(0))),
                _ => panic!("invalid presence"),
            },
            _ => panic!("invalid presence"),
        }
    }
    fn read_formula(value: S) -> Formula {
        match value {
            A(value) if value == "true" => Formula::True,
            A(value) if value == "false" => Formula::False,
            L(mut values) => {
                let tag = atom(values.remove(0));
                match tag.as_str() {
                    "var" => Formula::Var(number(exact(values, 1, "formula var").remove(0))),
                    "bound" => Formula::Bound(number(exact(values, 1, "formula bound").remove(0))),
                    "not" => {
                        Formula::Not(Box::new(read_formula(exact(values, 1, "not").remove(0))))
                    }
                    "and" => formula_pair(values, Formula::And, "and"),
                    "or" => formula_pair(values, Formula::Or, "or"),
                    "iff" => formula_pair(values, Formula::Iff, "iff"),
                    "xor" => formula_pair(values, Formula::Xor, "xor"),
                    _ => panic!("invalid formula"),
                }
            }
            _ => panic!("invalid formula"),
        }
    }
    fn formula_pair(
        values: Vec<S>,
        make: fn(Box<Formula>, Box<Formula>) -> Formula,
        tag: &str,
    ) -> Formula {
        let mut values = exact(values, 2, tag);
        make(
            Box::new(read_formula(values.remove(0))),
            Box::new(read_formula(values.remove(0))),
        )
    }

    fn read_lir(value: S) -> Lir {
        let mut value = exact(list(value, "lir"), 2, "lir");
        Lir {
            functions: many(value.remove(0), "functions")
                .into_iter()
                .map(read_function)
                .collect(),
            globals: many(value.remove(0), "globals")
                .into_iter()
                .map(read_global)
                .collect(),
        }
    }
    fn read_function(value: S) -> Function {
        let mut value = exact(list(value, "function"), 3, "function");
        Function {
            name: string(value.remove(0)),
            params: many(value.remove(0), "params")
                .into_iter()
                .map(read_param)
                .collect(),
            body: read_block(value.remove(0)),
        }
    }
    fn read_param(value: S) -> Param {
        let mut value = exact(list(value, "param"), 2, "param");
        Param {
            temp: number(value.remove(0)),
            rep: read_rep(value.remove(0)),
        }
    }
    fn read_global(value: S) -> Global {
        let mut value = exact(list(value, "global"), 2, "global");
        Global {
            name: string(value.remove(0)),
            body: read_block(value.remove(0)),
        }
    }
    fn read_block(value: S) -> Block {
        let mut value = exact(list(value, "block"), 2, "block");
        Block {
            instrs: many(value.remove(0), "instrs")
                .into_iter()
                .map(read_instr)
                .collect(),
            end: read_end(value.remove(0)),
        }
    }
    fn read_instr(value: S) -> Instr {
        let mut value = exact(list(value, "instr"), 3, "instr");
        Instr {
            temp: number(value.remove(0)),
            rep: read_rep(value.remove(0)),
            op: read_op(value.remove(0)),
        }
    }
    fn read_rep(value: S) -> Rep {
        match atom(value).as_str() {
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
            _ => panic!("invalid representation"),
        }
    }
    fn read_op(value: S) -> Op {
        let mut values = match value {
            L(values) => values,
            A(value) if value == "new-tag" => return Op::NewTag,
            _ => panic!("invalid operation"),
        };
        assert!(!values.is_empty(), "empty operation");
        let tag = atom(values.remove(0));
        match tag.as_str() {
            "const" => Op::Const(read_literal(exact(values, 1, "const").remove(0))),
            "neg" => Op::Neg(number(exact(values, 1, "neg").remove(0))),
            "not" => Op::Not(number(exact(values, 1, "not").remove(0))),
            "and" => op_binary(values, |left, right| Op::And { left, right }, "and"),
            "or" => op_binary(values, |left, right| Op::Or { left, right }, "or"),
            "xor" => op_binary(values, |left, right| Op::Xor { left, right }, "xor"),
            "add" => op_binary(values, |left, right| Op::Add { left, right }, "add"),
            "sub" => op_binary(values, |left, right| Op::Sub { left, right }, "sub"),
            "mul" => op_binary(values, |left, right| Op::Mul { left, right }, "mul"),
            "div" => op_binary(values, |left, right| Op::Div { left, right }, "div"),
            "struct" => Op::Struct(
                values
                    .into_iter()
                    .map(|value| {
                        let value = match value {
                            L(values) => values,
                            _ => panic!("bad struct entry"),
                        };
                        let mut value = exact(value, 2, "struct entry");
                        (string(value.remove(0)), number(value.remove(0)))
                    })
                    .collect(),
            ),
            "merge" => Op::Merge(values.into_iter().map(number).collect()),
            "project" => {
                let mut values = exact(values, 2, "project");
                Op::Project {
                    base: number(values.remove(0)),
                    field: string(values.remove(0)),
                }
            }
            "tag" => {
                assert!(values.len() == 1 || values.len() == 2, "bad tag");
                let name = string(values.remove(0));
                Op::Tag {
                    name,
                    payload: values.pop().map(number),
                }
            }
            "payload" => Op::Payload(number(exact(values, 1, "payload").remove(0))),
            "closure" => {
                let mut values = exact(values, 2, "closure");
                Op::Closure {
                    func: number(values.remove(0)),
                    captures: many(values.remove(0), "captures")
                        .into_iter()
                        .map(number)
                        .collect(),
                }
            }
            "call" => {
                let mut values = exact(values, 2, "call");
                let callee = match values.remove(0) {
                    L(mut target) => {
                        assert!(target.len() == 2, "bad call target");
                        match atom(target.remove(0)).as_str() {
                            "direct" => Callee::Direct(number(target.remove(0))),
                            "indirect" => Callee::Indirect(number(target.remove(0))),
                            _ => panic!("bad call target"),
                        }
                    }
                    _ => panic!("bad call target"),
                };
                let args = many(values.remove(0), "args")
                    .into_iter()
                    .map(number)
                    .collect();
                Op::Call { callee, args }
            }
            "global" => Op::Global {
                target: string(exact(values, 1, "global").remove(0)),
            },
            "catch" => {
                let mut values = exact(values, 2, "catch");
                Op::Catch {
                    tag: number(values.remove(0)),
                    body: Box::new(read_block(values.remove(0))),
                }
            }
            "switch-tag" => read_switch_tag(values),
            "switch-prim" => read_switch_prim(values),
            "switch-presence" => {
                let mut values = exact(values, 4, "switch-presence");
                Op::SwitchPresence {
                    on: number(values.remove(0)),
                    field: string(values.remove(0)),
                    present: Box::new(read_block(values.remove(0))),
                    absent: Box::new(read_block(values.remove(0))),
                }
            }
            "switch-rest" => {
                let mut values = exact(values, 4, "switch-rest");
                Op::SwitchRest {
                    on: number(values.remove(0)),
                    fields: many(values.remove(0), "fields")
                        .into_iter()
                        .map(string)
                        .collect(),
                    none: Box::new(read_block(values.remove(0))),
                    some: Box::new(read_block(values.remove(0))),
                }
            }
            _ => panic!("invalid operation"),
        }
    }
    fn op_binary(values: Vec<S>, make: fn(u32, u32) -> Op, tag: &str) -> Op {
        let mut values = exact(values, 2, tag);
        make(number(values.remove(0)), number(values.remove(0)))
    }
    fn read_switch_tag(values: Vec<S>) -> Op {
        let mut values = exact(values, 3, "switch-tag");
        let on = number(values.remove(0));
        let cases = many(values.remove(0), "cases")
            .into_iter()
            .map(|value| {
                let value = match value {
                    L(values) => values,
                    _ => panic!("bad tag case"),
                };
                let mut value = exact(value, 2, "tag case");
                TagCase {
                    name: string(value.remove(0)),
                    block: read_block(value.remove(0)),
                }
            })
            .collect();
        let fallback = optional_block_read(values.remove(0), "fallback");
        Op::SwitchTag {
            on,
            cases,
            fallback,
        }
    }
    fn read_switch_prim(values: Vec<S>) -> Op {
        let mut values = exact(values, 3, "switch-prim");
        let on = number(values.remove(0));
        let cases = many(values.remove(0), "cases")
            .into_iter()
            .map(|value| {
                let value = match value {
                    L(values) => values,
                    _ => panic!("bad primitive case"),
                };
                let mut value = exact(value, 2, "primitive case");
                PrimCase {
                    value: read_literal(value.remove(0)),
                    block: read_block(value.remove(0)),
                }
            })
            .collect();
        let fallback = optional_block_read(values.remove(0), "fallback");
        Op::SwitchPrim {
            on,
            cases,
            fallback,
        }
    }
    fn optional_block_read(value: S, tag: &str) -> Option<Box<Block>> {
        let values = list(value, tag);
        assert!(values.len() <= 1, "bad optional block");
        values
            .into_iter()
            .next()
            .map(|value| Box::new(read_block(value)))
    }
    fn read_end(value: S) -> End {
        let mut values = match value {
            L(values) => values,
            _ => panic!("invalid terminator"),
        };
        let tag = atom(values.remove(0));
        match tag.as_str() {
            "ret" => End::Ret(number(exact(values, 1, "ret").remove(0))),
            "yield" => End::Yield(number(exact(values, 1, "yield").remove(0))),
            "throw" => {
                let mut values = exact(values, 2, "throw");
                End::Throw {
                    tag: number(values.remove(0)),
                    value: number(values.remove(0)),
                }
            }
            _ => panic!("invalid terminator"),
        }
    }
    fn read_literal(value: S) -> Literal {
        let mut values = match value {
            L(values) => values,
            _ => panic!("invalid literal"),
        };
        let tag = atom(values.remove(0));
        match tag.as_str() {
            "nat" => Literal::Natural(number(exact(values, 1, "nat literal").remove(0))),
            "int" => Literal::Integer(number(exact(values, 1, "int literal").remove(0))),
            "real" => Literal::Real(number(exact(values, 1, "real literal").remove(0))),
            "string" => Literal::String(string(exact(values, 1, "string literal").remove(0))),
            "bool" => Literal::Boolean(boolean(exact(values, 1, "bool literal").remove(0))),
            _ => panic!("invalid literal"),
        }
    }
}
