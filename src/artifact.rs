//! A portable, span-free bundle artifact.
//!
//! Artifacts are the compiler's disk boundary.  They intentionally contain no
//! source locations, file paths, or [`crate::symbol::Symbol`]s: external value
//! references are qualified by the identity of the bundle that owns them.
//! [`parse`] accepts only compiler-produced text and deliberately panics for
//! malformed input. Use [`try_parse`] at trust boundaries. This is an internal
//! v1 format, not a compatibility promise.

mod regions;

use std::{collections::HashMap, error::Error, fmt};

use indexmap::IndexMap;

use crate::{
    compile::AcceptedProgram,
    ir, lir,
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

/// Portable artifact data before compiler invariants have been established.
/// Parsers and hand-written producers return this type; consumers must choose
/// strict validation or tolerant recovery explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncheckedArtifact {
    pub header: Header,
    pub lir: Lir,
}

/// A strict semantic-artifact validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    message: String,
}

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for ValidationError {}

/// A repair made while admitting foreign in-memory portable data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryFact {
    /// The dependency did not provide a bundle name, so recovery supplied one.
    IdentityNameReplaced { replacement: String },
    /// The dependency did not provide a bundle version, so recovery supplied one.
    IdentityVersionReplaced { replacement: String },
    /// An exported value lacked a name, so recovery supplied one.
    ValueNameReplaced { index: usize, replacement: String },
    /// A type declaration could not join the validated artifact.
    TypeDiscarded {
        index: usize,
        name: QualifiedName,
        reason: String,
    },
    /// An effect declaration could not join the validated artifact.
    EffectDiscarded {
        index: usize,
        name: QualifiedName,
        reason: String,
    },
    /// A value declaration could not join the validated artifact.
    ValueDiscarded {
        index: usize,
        name: QualifiedName,
        reason: String,
    },
    /// A module declaration could not join the validated artifact.
    ModuleDiscarded {
        index: usize,
        name: QualifiedName,
        reason: String,
    },
    /// Executable data referred outside its local function table and could not
    /// safely be given a meaning during dependency recovery.
    ExecutableDiscarded { reason: String },
}

/// A complete, serializable bundle artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub(crate) header: Header,
    pub(crate) lir: Lir,
}

/// An internal placeholder used only while core compilation establishes the
/// accepted proof that artifact construction consumes. It is never published.
pub(crate) fn empty() -> Artifact {
    Artifact {
        header: Header {
            kind: Kind::Library,
            identity: Identity {
                name: String::new(),
                version: String::new(),
            },
            compiler: Stamp::current(),
            domains: types::Domains::default(),
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: Vec::new(),
            modules: Vec::new(),
        },
        lir: Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
}

impl UncheckedArtifact {
    /// Strictly establish the portable artifact invariants.
    pub fn validate(self) -> Result<Artifact, ValidationError> {
        self.validate_ref()
    }

    /// [`validate`](Self::validate) without giving the data up: the decoder
    /// reads it by reference and answers with a fresh artifact either way, so
    /// recovery can validate first and still have the original to repair.
    fn validate_ref(&self) -> Result<Artifact, ValidationError> {
        if self.header.identity.name.is_empty() || self.header.identity.version.is_empty() {
            return Err(ValidationError::new(
                "artifact identity must name a bundle and version",
            ));
        }
        if self.header.values.iter().any(|value| value.name.is_empty()) {
            return Err(ValidationError::new("artifact value name is empty"));
        }
        validate_executable_relationships(&self.lir)?;
        for value in &self.header.values {
            let Some(global) = self
                .lir
                .globals
                .iter()
                .find(|global| global.name == value.name)
            else {
                continue;
            };
            let nontrivial = |interface: &Option<crate::reification::interface::Interface>| {
                interface.as_ref().is_some_and(|interface| {
                    !matches!(
                        interface.nodes.get(interface.root as usize),
                        Some(crate::reification::interface::Node::Value)
                    )
                })
            };
            if global.type_interface != value.scheme.callable
                && (nontrivial(&global.type_interface) || nontrivial(&value.scheme.callable))
            {
                return Err(ValidationError::new(
                    "exported callable requirements disagree with the lowered evidence layout",
                ));
            }
            let initializer = &self.lir.functions[global.initializer as usize];
            let entry = &initializer.blocks[initializer.entry as usize];
            if let End::Continue {
                value: returned, ..
            } = entry.end
                && let Some(instruction) = entry
                    .instrs
                    .iter()
                    .find(|instruction| instruction.temp == returned)
                && let Op::Closure { func, captures } = &instruction.op
            {
                let parameters = &self.lir.functions[*func as usize].params[captures.len()..];
                let layout = parameters
                    .iter()
                    .take_while(|parameter| parameter.rep == Rep::TypeDescriptor)
                    .count();
                let required = value
                    .scheme
                    .callable
                    .as_ref()
                    .and_then(|interface| interface.nodes.get(interface.root as usize))
                    .map_or(0, |node| match node {
                        crate::reification::interface::Node::Arrow { requirements, .. } => {
                            requirements
                                .iter()
                                .map(|need| need.parameter)
                                .collect::<std::collections::BTreeSet<_>>()
                                .len()
                        }
                        _ => 0,
                    });
                if layout != required {
                    return Err(ValidationError::new(
                        "runtime representation requirements disagree with the exported evidence layout",
                    ));
                }
            }
        }
        // Text decoding is the single semantic translation implementation.
        // Rendering this portable tree and decoding it again deliberately
        // routes hand-built data through the same stack-safe checks as a disk
        // artifact: bounds, formulas, package preorder ownership, rows and
        // absent payloads cannot acquire a second, weaker validation policy.
        // The tree is decoded without laying it out as text: a block nested
        // thirty thousand deep is indented thirty thousand times per line, so
        // the layout alone would be quadratic in the artifact, while the
        // decoder's checks are the same either way.
        text::decode_parts(&self.header, &self.lir)
            .map_err(|error| ValidationError::new(error.to_string()))
    }

    /// Admit a dependency artifact while recording that validation had to be
    /// relaxed. Recovery facts are diagnostics, not source compilation errors.
    pub fn recover(self) -> (Artifact, Vec<RecoveryFact>) {
        match self.validate_ref() {
            Ok(artifact) => (artifact, Vec::new()),
            Err(_) => {
                let UncheckedArtifact { header, lir } = self;
                // Recovery is deliberately local. One malformed spelling must
                // not erase unaffected declarations or executable content.
                let mut header = header;
                let mut facts = Vec::new();
                if header.identity.name.is_empty() {
                    let replacement = String::from("<recovered>");
                    header.identity.name = replacement.clone();
                    facts.push(RecoveryFact::IdentityNameReplaced { replacement });
                }
                if header.identity.version.is_empty() {
                    let replacement = String::from("0");
                    header.identity.version = replacement.clone();
                    facts.push(RecoveryFact::IdentityVersionReplaced { replacement });
                }
                for (index, value) in header.values.iter_mut().enumerate() {
                    if value.name.is_empty() {
                        let replacement = format!("<recovered>::value-{index}");
                        value.name = replacement.clone();
                        facts.push(RecoveryFact::ValueNameReplaced { index, replacement });
                    }
                }
                let (lir, executable_facts) = recover_executable(lir);
                facts.extend(executable_facts);
                let (artifact, discarded) = recover_parts(header, lir);
                facts.extend(discarded);
                (artifact, facts)
            }
        }
    }
}

/// Verify the executable relationships which later consumers rely on without
/// rechecking a target adapter's own syntax or semantics. The traversal is
/// iterative because portable blocks can be arbitrarily deeply nested.
fn validate_executable_relationships(lir: &Lir) -> Result<(), ValidationError> {
    cps::validate(lir).map_err(ValidationError::new)
}

/// An invalid local function reference has no target-neutral repair. Keep the
/// recoverable semantic interface, but remove executable content rather than
/// publishing an `Artifact` that could make a linker or backend dereference an
/// invalid table entry.
fn recover_executable(mut lir: Lir) -> (Lir, Vec<RecoveryFact>) {
    match validate_executable_relationships(&lir) {
        Ok(()) => (lir, Vec::new()),
        Err(error) => {
            lir.functions.clear();
            lir.globals.clear();
            (
                lir,
                vec![RecoveryFact::ExecutableDiscarded {
                    reason: error.to_string(),
                }],
            )
        }
    }
}

/// Retain every independently valid declaration while excluding only portable
/// semantic fragments that cannot establish the artifact invariant.  A valid
/// LIR is carried through every candidate validation, so a bad interface tree
/// never erases executable content that does not depend on it.
fn recover_parts(header: Header, lir: Lir) -> (Artifact, Vec<RecoveryFact>) {
    let mut recovered = Header {
        kind: header.kind,
        identity: header.identity,
        compiler: header.compiler,
        domains: header.domains,
        dependencies: header.dependencies,
        values: Vec::new(),
        types: Vec::new(),
        effects: Vec::new(),
        modules: Vec::new(),
    };

    // Types precede values because an exported value may name a local type.
    // Each candidate takes the same strict route as text input; recovery is
    // selection, never a weaker semantic decoder.
    let mut facts = Vec::new();
    for (index, declaration) in header.types.into_iter().enumerate() {
        let mut candidate = recovered.clone();
        candidate.types.push(declaration.clone());
        match (UncheckedArtifact {
            header: candidate.clone(),
            lir: lir.clone(),
        })
        .validate()
        {
            Ok(_) => recovered = candidate,
            Err(error) => facts.push(RecoveryFact::TypeDiscarded {
                index,
                name: declaration.name,
                reason: error.to_string(),
            }),
        }
    }
    for (index, declaration) in header.effects.into_iter().enumerate() {
        let mut candidate = recovered.clone();
        candidate.effects.push(declaration.clone());
        match (UncheckedArtifact {
            header: candidate.clone(),
            lir: lir.clone(),
        })
        .validate()
        {
            Ok(_) => recovered = candidate,
            Err(error) => facts.push(RecoveryFact::EffectDiscarded {
                index,
                name: declaration.name,
                reason: error.to_string(),
            }),
        }
    }
    for (index, declaration) in header.values.into_iter().enumerate() {
        let mut candidate = recovered.clone();
        candidate.values.push(declaration.clone());
        match (UncheckedArtifact {
            header: candidate.clone(),
            lir: lir.clone(),
        })
        .validate()
        {
            Ok(_) => recovered = candidate,
            Err(error) => facts.push(RecoveryFact::ValueDiscarded {
                index,
                name: declaration.name,
                reason: error.to_string(),
            }),
        }
    }
    // A module carries nothing but its metadata, and nothing else names it,
    // so it is the last thing admitted and its loss costs no other entry.
    for (index, declaration) in header.modules.into_iter().enumerate() {
        let mut candidate = recovered.clone();
        candidate.modules.push(declaration.clone());
        match (UncheckedArtifact {
            header: candidate.clone(),
            lir: lir.clone(),
        })
        .validate()
        {
            Ok(_) => recovered = candidate,
            Err(error) => facts.push(RecoveryFact::ModuleDiscarded {
                index,
                name: declaration.name,
                reason: error.to_string(),
            }),
        }
    }

    let artifact = UncheckedArtifact {
        header: recovered,
        lir,
    }
    .validate()
    .expect("a recovery candidate is admitted only after strict validation");
    (artifact, facts)
}

impl Drop for Artifact {
    fn drop(&mut self) {
        text::discard_artifact(self);
    }
}

/// A bundle's exports and the semantic declarations needed to interpret them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Whether this bundle is importable or provides a program entry point.
    pub kind: Kind,
    pub identity: Identity,
    /// The compiler that wrote this artifact.
    pub compiler: Stamp,
    /// The domains `Nat` and `Int` were bound to. A linked program binds them
    /// once, and an artifact bound to others cannot join it.
    pub domains: types::Domains,
    /// The bundles this artifact depends on.
    pub dependencies: Vec<Dependency>,
    /// Every source-addressable top-level value exported by the bundle. Externs
    /// precede `let`s; each kind retains its source declaration order. Hidden
    /// definitions generated for patterns such as `let _`, private definitions,
    /// and descendants of private modules are not exported.
    pub values: Vec<Value>,
    /// Every declared type, including private semantic support for public
    /// signatures. Only entries marked `exported` are source-addressable.
    pub types: Vec<DeclaredType>,
    /// Every declared effect, including private semantic support for public
    /// signatures. Only entries marked `exported` are source-addressable.
    pub effects: Vec<DeclaredEffect>,
    /// Every exported module, in source declaration order, with its metadata.
    /// A module is otherwise visible only as a segment of the names under it.
    pub modules: Vec<DeclaredModule>,
}

/// A bundle's role, independent of its output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Library,
    Executable,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Library => "library",
            Self::Executable => "executable",
        })
    }
}

/// The identity that owns an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub version: String,
}

/// The digest of the compiler that wrote an artifact: a hash of the
/// compiler's own source, taken when it was built, rather than a version
/// number anyone has to remember to bump. Two compilers with the same stamp
/// write the same artifact for the same source, so a cached artifact whose
/// stamp is this compiler's can be trusted without being recompiled — and one
/// whose stamp is not can only be recompiled, since nothing says what has
/// changed in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp(String);

/// The stamp of the compiler this is, computed by the build script.
pub const COMPILER_HASH: &str = env!("RUDDY_COMPILER_HASH");

impl Stamp {
    /// The stamp of the compiler this is.
    pub fn current() -> Self {
        Self(COMPILER_HASH.to_string())
    }

    /// A stamp read back from an artifact, whatever compiler wrote it.
    pub fn recorded(text: String) -> Self {
        Self(text)
    }

    /// Whether the compiler that wrote the artifact is this one.
    pub fn is_current(&self) -> bool {
        self.0 == COMPILER_HASH
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    pub metadata: Metadata,
}

/// A declared type and the semantics of its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredType {
    pub name: QualifiedName,
    /// Whether dependent source may name this declaration. Private entries
    /// remain available solely to interpret structural types and effects.
    pub exported: bool,
    pub params: Vec<Parameter>,
    pub scheme: Scheme,
    pub metadata: Metadata,
}

/// A declared module: its qualified name and the metadata in front of its
/// declaration. The one entry of the header that says nothing semantic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredModule {
    pub name: QualifiedName,
    pub metadata: Metadata,
}

/// A declaration's metadata: literal data by key, in written order, and empty
/// when no attribute was written. `private` controls source export visibility;
/// other keys are uninterpreted data for tools.
pub type Metadata = IndexMap<String, Data>;

/// A metadata value, span-free: the lowered [`ir::DataKind`] with nothing
/// about where it was written. Unit is the empty struct and a tuple is the
/// struct numbered `0`, `1`, …, as they are after lowering.
///
/// Compared by representation — a real by its bits, so signed zero is told
/// apart — so that two artifacts are equal exactly when they print the same.
/// Nested at most [`ir::METADATA_DEPTH_LIMIT`] deep, which the reader
/// enforces before building one, so the derived traits may recurse.
#[derive(Debug, Clone)]
pub enum Data {
    Natural(u64),
    Integer(i64),
    Fixed(crate::types::FixedLiteral),
    Real(f64),
    String(String),
    Bool(bool),
    Array(Vec<Data>),
    Struct(IndexMap<String, Data>),
    Tag { name: String, payload: Box<Data> },
}

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Natural(a), Self::Natural(b)) => a == b,
            (Self::Fixed(a), Self::Fixed(b)) => a == b,
            (Self::Integer(a), Self::Integer(b)) => a == b,
            (Self::Real(a), Self::Real(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Struct(a), Self::Struct(b)) => a == b,
            (
                Self::Tag { name, payload },
                Self::Tag {
                    name: other_name,
                    payload: other_payload,
                },
            ) => name == other_name && payload == other_payload,
            _ => false,
        }
    }
}
impl Eq for Data {}

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
    Region,
    Type,
    Row,
    Effects,
}

/// A declared effect and its semantic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredEffect {
    pub name: QualifiedName,
    /// Whether dependent source may name this declaration. Private entries
    /// remain available solely to interpret structural types and effects.
    pub exported: bool,
    /// The parameters the effect binds, in the order it is applied to them.
    /// Operation signatures refer to them by position through
    /// [`Type::Bound`] and [`Rest::Bound`]; an alias row does the same.
    pub params: Vec<Parameter>,
    /// `None` for an alias: aliases expand to other effects and do not name a
    /// row label of their own.
    pub identity: Option<EffectIdentity>,
    pub kind: EffectKind,
    pub metadata: Metadata,
}

/// The structural identity used by semantic effect rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIdentity {
    pub name: String,
    pub interface: String,
}

/// An effect's operations, or the row an alias writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectKind {
    Operations(Vec<Operation>),
    Alias(AliasRow),
}

/// The row an alias stands for, unexpanded: the effects it applies, each to
/// arguments over the alias's own parameters, and the parameter it ends in.
/// Kept as written because an open alias is expanded at each use with that
/// use's arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRow {
    pub cases: Vec<AliasCase>,
    /// The parameter position spliced as the row's tail, if the alias ends in
    /// one.
    pub tail: Option<u32>,
}

impl AliasRow {
    /// A closed row naming effects that take nothing.
    pub fn naming(names: Vec<QualifiedName>) -> Self {
        AliasRow {
            cases: names
                .into_iter()
                .map(|name| AliasCase {
                    name,
                    args: Vec::new(),
                })
                .collect(),
            tail: None,
        }
    }
}

/// One application an alias row names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasCase {
    pub name: QualifiedName,
    /// One argument per parameter the named effect declares, over the
    /// alias's own parameters.
    pub args: Vec<Type>,
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
    pub callable: Option<crate::reification::interface::Interface>,
    pub representations: Vec<u32>,
    pub count: u32,
    pub presences: u32,
    /// Producer-owned presence positions. Each index is in `0..presences`.
    /// The sorted canonical representation survives import and linking so an
    /// imported consumer cannot accidentally reopen a witness as universal.
    pub existentials: Vec<u32>,
    pub formula: Formula,
    pub body: Type,
}

/// A normalized semantic type. Structural fields are representable only by
/// the `Struct` constructor.
#[derive(Debug)]
pub enum Type {
    Nat,
    Int,
    Fixed(crate::types::FixedInt),
    Real,
    String,
    Bool,
    ForeignValue,
    Arrow(Box<Type>, Box<Type>, Row),
    Package(Box<Type>),
    /// `hide 'a => T`, with the occurrences of its variable inside the body
    /// as [`Type::HiddenVar`]s naming the same binder.
    Hidden {
        binder: u32,
        name: String,
        body: Box<Type>,
    },
    HiddenVar {
        binder: u32,
        name: String,
    },
    Array(Box<Type>),
    Mirror(Box<Type>),
    Mut(Box<Type>, Box<Type>),
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
    /// Constraint owned by the package at this preorder in the scheme body.
    Owned(u32, Box<Formula>),
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
                (Type::Fixed(left), Type::Fixed(right)) if left == right => {}
                (Type::Nat, Type::Nat)
                | (Type::Int, Type::Int)
                | (Type::Real, Type::Real)
                | (Type::String, Type::String)
                | (Type::Bool, Type::Bool)
                | (Type::ForeignValue, Type::ForeignValue)
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
                (Type::Package(left), Type::Package(right)) => {
                    pending.push(SemanticPair::Type(left, right));
                }
                (
                    Type::Hidden {
                        binder: left_binder,
                        name: left_name,
                        body: left,
                    },
                    Type::Hidden {
                        binder: right_binder,
                        name: right_name,
                        body: right,
                    },
                ) if left_binder == right_binder && left_name == right_name => {
                    pending.push(SemanticPair::Type(left, right));
                }
                (
                    Type::HiddenVar {
                        binder: left_binder,
                        name: left_name,
                    },
                    Type::HiddenVar {
                        binder: right_binder,
                        name: right_name,
                    },
                ) if left_binder == right_binder && left_name == right_name => {}
                (Type::Mut(a, b), Type::Mut(c, d)) => {
                    pending.push(SemanticPair::Type(a, c));
                    pending.push(SemanticPair::Type(b, d));
                }
                (Type::Array(left), Type::Array(right))
                | (Type::Mirror(left), Type::Mirror(right)) => {
                    pending.push(SemanticPair::Type(left, right));
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
                (Formula::Owned(left_owner, left), Formula::Owned(right_owner, right))
                    if left_owner == right_owner =>
                {
                    pending.push((left, right))
                }
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
    Package,
    Hidden(u32, String),
    Array,
    Mirror,
    Mut,
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
                Type::Fixed(kind) => types.push(Type::Fixed(*kind)),
                Type::Real => types.push(Type::Real),
                Type::String => types.push(Type::String),
                Type::Bool => types.push(Type::Bool),
                Type::ForeignValue => types.push(Type::ForeignValue),
                Type::Arrow(from, to, effects) => {
                    work.push(CloneWork::Arrow);
                    work.push(CloneWork::Semantic(SemanticRef::Row(effects)));
                    work.push(CloneWork::Semantic(SemanticRef::Type(to)));
                    work.push(CloneWork::Semantic(SemanticRef::Type(from)));
                }
                Type::Package(body) => {
                    work.push(CloneWork::Package);
                    work.push(CloneWork::Semantic(SemanticRef::Type(body)));
                }
                Type::Hidden { binder, name, body } => {
                    work.push(CloneWork::Hidden(*binder, name.clone()));
                    work.push(CloneWork::Semantic(SemanticRef::Type(body)));
                }
                Type::HiddenVar { binder, name } => types.push(Type::HiddenVar {
                    binder: *binder,
                    name: name.clone(),
                }),
                Type::Array(element) => {
                    work.push(CloneWork::Array);
                    work.push(CloneWork::Semantic(SemanticRef::Type(element)));
                }
                Type::Mirror(element) => {
                    work.push(CloneWork::Mirror);
                    work.push(CloneWork::Semantic(SemanticRef::Type(element)));
                }
                Type::Mut(region, element) => {
                    work.push(CloneWork::Mut);
                    work.push(CloneWork::Semantic(SemanticRef::Type(element)));
                    work.push(CloneWork::Semantic(SemanticRef::Type(region)));
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
            CloneWork::Package => {
                let body = types.pop().expect("cloned package body");
                types.push(Type::Package(Box::new(body)));
            }
            CloneWork::Hidden(binder, name) => {
                let body = types.pop().expect("cloned hidden body");
                types.push(Type::Hidden {
                    binder,
                    name,
                    body: Box::new(body),
                });
            }
            CloneWork::Array => {
                let element = types.pop().expect("cloned array element");
                types.push(Type::Array(Box::new(element)));
            }
            CloneWork::Mirror => {
                let element = types.pop().expect("cloned mirror type");
                types.push(Type::Mirror(Box::new(element)));
            }
            CloneWork::Mut => {
                let element = types.pop().expect("cell element");
                let region = types.pop().expect("cell region");
                types.push(Type::Mut(Box::new(region), Box::new(element)));
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
        Type::Hidden { body, .. } => pending.push(SemanticOwned::Type(std::mem::replace(
            body,
            Type::Undecided,
        ))),
        Type::Package(body) => pending.push(SemanticOwned::Type(std::mem::replace(
            body.as_mut(),
            Type::Undecided,
        ))),
        Type::Mut(region, element) => {
            pending.push(SemanticOwned::Type(std::mem::replace(
                region.as_mut(),
                Type::Undecided,
            )));
            pending.push(SemanticOwned::Type(std::mem::replace(
                element.as_mut(),
                Type::Undecided,
            )));
        }
        Type::Array(element) | Type::Mirror(element) => pending.push(SemanticOwned::Type(
            std::mem::replace(element.as_mut(), Type::Undecided),
        )),
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
        | Type::Fixed(_)
        | Type::Real
        | Type::String
        | Type::Bool
        | Type::ForeignValue
        | Type::Var(_)
        | Type::Bound(_)
        | Type::Rigid { .. }
        | Type::HiddenVar { .. }
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
            Owned(u32),
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
                Work::Formula(Formula::Owned(owner, inner)) => {
                    work.push(Work::Owned(*owner));
                    work.push(Work::Formula(inner));
                }
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
                Work::Owned(owner) => {
                    let inner = out.pop().expect("cloned owned formula operand");
                    out.push(Formula::Owned(owner, Box::new(inner)));
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
        Formula::Owned(_, inner) | Formula::Not(inner) => {
            pending.push(std::mem::replace(inner.as_mut(), Formula::True))
        }
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

mod cps;
pub use cps::*;

/// Build an artifact after inference and LIR lowering succeeded.
pub fn build(accepted: &AcceptedProgram) -> Artifact {
    build_with_dependencies(accepted, Vec::new())
}

/// Build an artifact with the dependency identities supplied by its driver.
pub fn build_with_dependencies(
    accepted: &AcceptedProgram,
    dependencies: Vec<Dependency>,
) -> Artifact {
    let lir = accepted.lower();
    build_lowered(accepted, dependencies, &lir)
}

pub(crate) fn build_lowered(
    accepted: &AcceptedProgram,
    dependencies: Vec<Dependency>,
    lir: &lir::Output,
) -> Artifact {
    Artifact {
        header: interface(
            accepted.mint(),
            accepted.ir(),
            accepted.semantics(),
            dependencies,
            accepted.domains(),
        ),
        lir: lower_lir(accepted.mint(), lir),
    }
}

/// Publish the current semantic interface without lowering or constructing an
/// executable artifact. Recovery types remain explicit in editor interfaces.
pub fn interface(
    mint: &Mint,
    program: &ir::Program,
    inference: &crate::inference::Semantics,
    dependencies: Vec<Dependency>,
    domains: types::Domains,
) -> Header {
    Header {
        kind: Kind::Library,
        identity: Identity {
            name: mint.bundle().name().to_string(),
            version: mint.bundle().version().to_string(),
        },
        compiler: Stamp::current(),
        domains,
        dependencies,
        values: program
            .externs
            .iter()
            .filter(|(symbol, declaration)| {
                is_exported(mint, program, **symbol, &declaration.metadata)
            })
            .map(|(symbol, declaration)| Value {
                name: qualified(mint, *symbol),
                scheme: scheme(mint, &inference.externs()[symbol]),
                metadata: metadata(&declaration.metadata),
            })
            .chain(
                program
                    .terms
                    .iter()
                    // Fresh top-level definitions implement patterns such as
                    // `let _`; they must be initialized, but have no source
                    // name through which another bundle could import them.
                    .filter(|(symbol, declaration)| {
                        inference.schemes().contains_key(*symbol)
                            && is_exported(mint, program, **symbol, &declaration.metadata)
                    })
                    .map(|(symbol, declaration)| Value {
                        name: qualified(mint, *symbol),
                        scheme: scheme(mint, &inference.schemes()[symbol]),
                        metadata: metadata(&declaration.metadata),
                    }),
            )
            .collect(),
        types: program
            .types
            .iter()
            .map(|(symbol, declaration)| DeclaredType {
                name: qualified(mint, *symbol),
                exported: is_exported(mint, program, *symbol, &declaration.metadata),
                params: declaration
                    .params
                    .iter()
                    .map(|param| Parameter {
                        sense: match &param.kind {
                            types::ParamKind::Region { .. } => Sense::Region,
                            types::ParamKind::Type { .. } => Sense::Type,
                            types::ParamKind::Row { .. } => Sense::Row,
                            types::ParamKind::Effects { .. } => Sense::Effects,
                        },
                        lacks: param.kind.lacks().iter().cloned().collect(),
                        relevant: param.relevant,
                    })
                    .collect(),
                scheme: scheme(mint, &inference.aliases()[symbol]),
                metadata: metadata(&declaration.metadata),
            })
            .collect(),
        effects: program
            .effects
            .iter()
            .map(|(symbol, declaration)| DeclaredEffect {
                name: qualified(mint, *symbol),
                exported: is_exported(mint, program, *symbol, &declaration.metadata),
                params: declaration
                    .params
                    .iter()
                    .map(|param| Parameter {
                        sense: match param.kind {
                            types::ParamKind::Region { .. } => Sense::Region,
                            types::ParamKind::Type { .. } => Sense::Type,
                            types::ParamKind::Row { .. } => Sense::Row,
                            types::ParamKind::Effects { .. } => Sense::Effects,
                        },
                        lacks: param.kind.lacks().iter().cloned().collect(),
                        // Every effect parameter counts: even one no operation
                        // mentions tells two applications apart.
                        relevant: true,
                    })
                    .collect(),
                identity: program.effect_ids.get(symbol).map(effect_id),
                kind: match &declaration.value {
                    ir::Effect::Operations(operations) => EffectKind::Operations(
                        operations
                            .iter()
                            .map(|(name, _)| {
                                let (from, to) = &inference.operations()[&(*symbol, name.clone())];
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
                    ir::Effect::Alias(_) => {
                        let row = &inference.effect_aliases()[symbol];
                        EffectKind::Alias(AliasRow {
                            cases: row
                                .cases
                                .iter()
                                .map(|(symbol, args)| AliasCase {
                                    name: qualified(mint, *symbol),
                                    args: args.iter().map(|arg| ty(mint, arg)).collect(),
                                })
                                .collect(),
                            tail: row.tail,
                        })
                    }
                },
                metadata: metadata(&declaration.metadata),
            })
            .collect(),
        modules: program
            .modules
            .iter()
            .filter(|(symbol, declaration)| {
                is_exported(mint, program, **symbol, &declaration.metadata)
            })
            .map(|(symbol, declaration)| DeclaredModule {
                name: qualified(mint, *symbol),
                metadata: metadata(&declaration.metadata),
            })
            .collect(),
    }
}

/// The private test interface, before ordinary export filtering.
pub(crate) fn test_values(
    mint: &Mint,
    program: &ir::Program,
    inference: &crate::inference::Semantics,
) -> Vec<Value> {
    program
        .terms
        .iter()
        .filter(|(_, declaration)| declaration.metadata.contains_key("test"))
        .filter_map(|(symbol, declaration)| {
            inference.schemes().get(symbol).map(|ty| Value {
                name: qualified(mint, *symbol),
                scheme: {
                    let mut result = scheme(mint, ty);
                    let body = crate::inference::unfold(inference.aliases(), ty.body());
                    result.body = self::ty(mint, &body);
                    if let (crate::types::Ty::Arrow(from, to, _), Type::Arrow(input, output, _)) =
                        (&*body, &mut result.body)
                    {
                        **input =
                            self::ty(mint, &crate::inference::unfold(inference.aliases(), from));
                        **output =
                            self::ty(mint, &crate::inference::unfold(inference.aliases(), to));
                    }
                    result
                },
                metadata: metadata(&declaration.metadata),
            })
        })
        .collect()
}

/// A source name is exported only when it and every enclosing module are public.
/// This does not affect resolution or initialization inside the declaring bundle.
fn is_exported(
    mint: &Mint,
    program: &ir::Program,
    symbol: Symbol,
    metadata: &ir::Metadata,
) -> bool {
    if mint.is_local(symbol) || metadata.contains_key("private") {
        return false;
    }
    let mut parent = mint.parent(symbol);
    while let Some(module) = parent {
        if program
            .modules
            .get(&module.symbol())
            .is_some_and(|declaration| declaration.metadata.contains_key("private"))
        {
            return false;
        }
        parent = mint.parent(module.symbol());
    }
    true
}

/// A declaration's lowered metadata, with its spans left behind.
fn metadata(value: &ir::Metadata) -> Metadata {
    value
        .iter()
        .map(|(key, attribute)| (key.clone(), data(&attribute.value)))
        .collect()
}

/// One lowered metadata value, with its spans left behind.
fn data(value: &ir::Data) -> Data {
    match &value.anchored {
        ir::DataKind::Natural(value) => Data::Natural(*value),
        ir::DataKind::Fixed(value) => Data::Fixed(*value),
        ir::DataKind::Integer(value) => Data::Integer(*value),
        ir::DataKind::Real(value) => Data::Real(*value),
        ir::DataKind::String(value) => Data::String(value.clone()),
        ir::DataKind::Bool(value) => Data::Bool(*value),
        ir::DataKind::Array(items) => Data::Array(items.iter().map(data).collect()),
        ir::DataKind::Struct(fields) => Data::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), data(&field.value)))
                .collect(),
        ),
        ir::DataKind::Tag { name, payload } => Data::Tag {
            name: name.clone(),
            payload: Box::new(data(payload)),
        },
    }
}

impl Artifact {
    /// The validated public interface of this artifact.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The validated executable representation of this artifact.
    pub fn lir(&self) -> &Lir {
        &self.lir
    }

    /// Copy this validated artifact back to portable data for a caller that
    /// needs to pass it through a dependency-admission boundary.
    pub fn to_unchecked(&self) -> UncheckedArtifact {
        UncheckedArtifact {
            header: self.header.clone(),
            lir: self.lir.clone(),
        }
    }

    pub(crate) fn from_validated_parts(header: Header, lir: Lir) -> Self {
        Self { header, lir }
    }
    /// Build an artifact after inference and LIR lowering succeeded.
    pub fn build(accepted: &AcceptedProgram) -> Self {
        build(accepted)
    }

    /// Canonical textual serialization.
    pub fn print(&self) -> String {
        print(self)
    }
    /// Parse trusted internal artifact text. Malformed input panics.
    pub fn parse(input: &str) -> UncheckedArtifact {
        parse(input)
    }

    /// Parse artifact text without panicking on malformed input.
    pub fn try_parse(input: &str) -> Result<UncheckedArtifact, ParseError> {
        try_parse(input)
    }
}

/// Print canonical artifact text.
pub fn print(artifact: &Artifact) -> String {
    text::print(artifact)
}
/// Parse trusted internal artifact text. Malformed input panics.
pub fn parse(input: &str) -> UncheckedArtifact {
    let artifact = text::parse(input);
    UncheckedArtifact {
        header: artifact.header.clone(),
        lir: artifact.lir.clone(),
    }
}

/// Parse artifact text without panicking on malformed input.
pub fn try_parse(input: &str) -> Result<UncheckedArtifact, ParseError> {
    text::try_parse(input).map(|artifact| UncheckedArtifact {
        header: artifact.header.clone(),
        lir: artifact.lir.clone(),
    })
}

pub fn qualified(mint: &Mint, symbol: Symbol) -> QualifiedName {
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

/// Export one semantic scheme as portable artifact data, naming every declared
/// type by its qualified name in `mint`. The same translation
/// [`build_with_dependencies`] uses for every exported value.
pub fn export_scheme(mint: &Mint, value: &types::Scheme) -> Scheme {
    scheme(mint, value)
}

fn scheme(mint: &Mint, value: &types::Scheme) -> Scheme {
    let mut existentials: Vec<_> = value.existentials().iter().copied().collect();
    existentials.sort_unstable();
    Scheme {
        callable: value.callable().cloned(),
        representations: value.representations().to_vec(),
        count: value.count(),
        presences: value.presences(),
        existentials,
        formula: formula(value.formula()),
        body: ty(mint, value.body()),
    }
}

fn ty(mint: &Mint, value: &types::Ty) -> Type {
    enum Work<'a> {
        Ty(&'a types::Ty),
        Row(&'a types::Row),
        Arrow,
        Package,
        Hidden(u32, String),
        Array,
        Mirror,
        Mut,
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
                types::Ty::Fixed(kind) => tys.push(Type::Fixed(*kind)),
                types::Ty::Real => tys.push(Type::Real),
                types::Ty::String => tys.push(Type::String),
                types::Ty::Bool => tys.push(Type::Bool),
                types::Ty::ForeignValue => tys.push(Type::ForeignValue),
                types::Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects));
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                types::Ty::Package(body) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(body));
                }
                types::Ty::Hidden { binder, name, body } => {
                    work.push(Work::Hidden(*binder, name.to_string()));
                    work.push(Work::Ty(body));
                }
                types::Ty::HiddenVar { binder, name } => tys.push(Type::HiddenVar {
                    binder: *binder,
                    name: name.to_string(),
                }),
                types::Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element));
                }
                types::Ty::Mirror(element) => {
                    work.push(Work::Mirror);
                    work.push(Work::Ty(element));
                }
                types::Ty::Mut(region, element) => {
                    work.push(Work::Mut);
                    work.push(Work::Ty(element));
                    work.push(Work::Ty(region));
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
            Work::Package => {
                let body = tys.pop().expect("artifact package body");
                tys.push(Type::Package(Box::new(body)));
            }
            Work::Hidden(binder, name) => {
                let body = tys.pop().expect("hidden body");
                tys.push(Type::Hidden {
                    binder,
                    name,
                    body: Box::new(body),
                });
            }
            Work::Array => {
                let element = tys.pop().expect("artifact array element");
                tys.push(Type::Array(Box::new(element)));
            }
            Work::Mirror => {
                let element = tys.pop().expect("artifact mirror type");
                tys.push(Type::Mirror(Box::new(element)));
            }
            Work::Mut => {
                let element = tys.pop().expect("cell element");
                let region = tys.pop().expect("cell region");
                tys.push(Type::Mut(Box::new(region), Box::new(element)));
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
        Owned(u32),
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
            Work::Formula(types::Formula::Owned(owner, inner)) => {
                work.push(Work::Owned(*owner));
                work.push(Work::Formula(inner));
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
            Work::Owned(owner) => {
                let inner = out.pop().expect("owned formula conversion postorder");
                out.push(Formula::Owned(owner, Box::new(inner)));
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
            .map(|e| Extern {
                name: qualified(mint, e.symbol),
                target: e.target.clone(),
                rep: rep(e.rep),
            })
            .collect(),
        globals: output
            .globals
            .iter()
            .map(|g| Global {
                type_interface: g.type_interface.clone(),
                adapter: g.adapter,
                callable: g.callable,
                name: qualified(mint, g.symbol),
                initializer: g.initializer as u64,
            })
            .collect(),
        functions: output
            .functions
            .iter()
            .map(|f| Function {
                suspension: f.suspension,
                name: f.name.clone(),
                params: f.params.iter().map(param).collect(),
                continuation: f.continuation,
                entry: f.entry as u64,
                blocks: f
                    .blocks
                    .iter()
                    .map(|b| Block {
                        params: b.params.iter().map(param).collect(),
                        result: b.result,
                        instrs: b
                            .instrs
                            .iter()
                            .map(|i| Instr {
                                temp: i.temp,
                                rep: rep(i.rep),
                                op: op(mint, &i.op),
                            })
                            .collect(),
                        end: end(&b.end.kind),
                    })
                    .collect(),
            })
            .collect(),
    }
}
fn param(p: &lir::Param) -> Param {
    Param {
        temp: p.temp,
        rep: rep(p.rep),
    }
}
fn edge(e: &lir::Edge) -> Edge {
    Edge {
        block: e.block as u64,
        args: e.args.clone(),
    }
}
fn test(t: &lir::Test) -> Test {
    match t {
        lir::Test::Tag { on, name } => Test::Tag {
            on: *on,
            name: name.clone(),
        },
        lir::Test::Literal { on, value } => Test::Literal {
            on: *on,
            value: literal(value),
        },
        lir::Test::Presence { on, field } => Test::Presence {
            on: *on,
            field: field.clone(),
        },
        lir::Test::Rest { on, fields } => Test::Rest {
            on: *on,
            fields: fields.clone(),
        },
        lir::Test::Length { on, length } => Test::Length {
            on: *on,
            length: *length as u64,
        },
    }
}
fn callee(c: lir::Callee) -> Callee {
    match c {
        lir::Callee::Direct(f) => Callee::Direct(f as u64),
        lir::Callee::Indirect(t) => Callee::Indirect(t),
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
        Source::Callback { value, mode } => Op::Callback {
            value: *value,
            mode: *mode,
        },
        Source::Const(value) => Op::Const(literal(value)),
        Source::Neg(value) => Op::Neg(*value),
        Source::Not(value) => Op::Not(*value),
        Source::Allocate(value) => Op::Allocate(*value),
        Source::Read(value) => Op::Read(*value),
        Source::Write { left, right } => Op::Write {
            left: *left,
            right: *right,
        },
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
        Source::Array(values) => Op::Array(values.clone()),
        Source::Merge(values) => Op::Merge(values.clone()),
        Source::Concat(values) => Op::Concat(values.clone()),
        Source::Project { base, field } => Op::Project {
            base: *base,
            field: field_key(field),
        },
        Source::Tag { name, payload } => Op::Tag {
            name: name.clone(),
            payload: *payload,
        },
        Source::Payload(value) => Op::Payload(*value),
        Source::Nth { base, index } => Op::Nth {
            base: *base,
            index: *index as u64,
        },
        Source::NthBack { base, index } => Op::NthBack {
            base: *base,
            index: *index as u64,
        },
        Source::Slice { base, start, drop } => Op::Slice {
            base: *base,
            start: *start as u64,
            drop: *drop as u64,
        },
        Source::Closure { func, captures } => Op::Closure {
            func: u64::try_from(*func).expect("LIR function index does not fit artifact format"),
            captures: captures.clone(),
        },
        Source::Continuation { code, captures } => Op::Continuation {
            code: CodeRef {
                function: code.function as u64,
                block: code.block as u64,
            },
            captures: captures.clone(),
        },
        Source::Extern { symbol, .. } => Op::Extern {
            target: qualified(mint, *symbol),
        },
        Source::Global {
            symbol, callable, ..
        } => Op::Global {
            callable: *callable,
            target: qualified(mint, *symbol),
        },
        Source::TypeProjection { descriptor, path } => Op::TypeProjection {
            descriptor: *descriptor,
            path: path.clone(),
        },
        Source::TypeDescriptor {
            template,
            arguments,
        } => Op::TypeDescriptor {
            template: template.clone(),
            arguments: arguments.clone(),
        },
        Source::NativePlan {
            template,
            arguments,
        } => Op::NativePlan {
            template: template.clone(),
            arguments: arguments.clone(),
        },
        Source::Reflect {
            kind,
            descriptor,
            value,
        } => Op::Reflect {
            kind: *kind,
            descriptor: *descriptor,
            value: *value,
        },
        Source::Convert {
            descriptor,
            value,
            direction,
        } => Op::Convert {
            descriptor: *descriptor,
            value: *value,
            direction: *direction,
        },
        Source::NewTag => Op::NewTag,
    }
}

fn literal(value: &ir::Literal) -> Literal {
    match value {
        ir::Literal::Natural(value) => Literal::Natural(*value),
        ir::Literal::Fixed(value) => Literal::Fixed(*value),
        ir::Literal::Integer(value) => Literal::Integer(*value),
        ir::Literal::Real(value) => Literal::Real(value.to_bits()),
        ir::Literal::String(value) => Literal::String(value.clone()),
        ir::Literal::Bool(value) => Literal::Bool(*value),
    }
}
fn end(e: &lir::End) -> End {
    match e {
        lir::End::Continue {
            continuation,
            value,
        } => End::Continue {
            continuation: *continuation,
            value: *value,
        },
        lir::End::Jump(e) => End::Jump(edge(e)),
        lir::End::Branch { test: t, yes, no } => End::Branch {
            test: test(t),
            yes: edge(yes),
            no: edge(no),
        },
        lir::End::Call {
            callee: c,
            args,
            continuation,
        } => End::Call {
            callee: callee(*c),
            args: args.clone(),
            continuation: *continuation,
        },
        lir::End::RawCall {
            callee,
            args,
            continuation,
            completion,
        } => End::RawCall {
            callee: *callee,
            args: args.clone(),
            continuation: *continuation,
            completion: *completion,
        },
        lir::End::Enter {
            tag,
            body,
            continuation,
        } => End::Enter {
            tag: *tag,
            body: edge(body),
            continuation: *continuation,
        },
        lir::End::Leave { tag, value } => End::Leave {
            tag: *tag,
            value: *value,
        },
        lir::End::Abort { tag, value } => End::Abort {
            tag: *tag,
            value: *value,
        },
        lir::End::Unreachable => End::Unreachable,
    }
}

fn rep(value: lir::Rep) -> Rep {
    match value {
        lir::Rep::Nat => Rep::Nat,
        lir::Rep::Int => Rep::Int,
        lir::Rep::Fixed(kind) => Rep::Fixed(kind),
        lir::Rep::Real => Rep::Real,
        lir::Rep::String => Rep::String,
        lir::Rep::Bool => Rep::Bool,
        lir::Rep::TypeDescriptor => Rep::TypeDescriptor,
        lir::Rep::NativePlan => Rep::NativePlan,
        lir::Rep::HostValue => Rep::HostValue,
        lir::Rep::Unit => Rep::Unit,
        lir::Rep::Struct => Rep::Struct,
        lir::Rep::Array => Rep::Array,
        lir::Rep::Sum => Rep::Sum,
        lir::Rep::Fn => Rep::Fn,
        lir::Rep::Any => Rep::Any,
        lir::Rep::Cont => Rep::Cont,
        lir::Rep::Handler => Rep::Handler,
    }
}

/// Canonical text helpers. The S-expression grammar is deliberately explicit:
/// every type, formula, operation, nested block, and ordered map has a distinct
/// tag. The writer uses a fixed-width pretty layout; strings are quoted, and
/// the parser accepts trusted output only.
pub mod text {
    use super::*;
    use unicode_width::UnicodeWidthStr;

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
        let mut out = layout(&artifact(value));
        out.push('\n');
        out
    }
    /// Decode an artifact's own portable tree through the reader, which is
    /// what parsing its canonical text would do minus the text.
    pub(crate) fn decode_parts(header: &Header, lir: &Lir) -> Result<Artifact, ParseError> {
        Reader::new().artifact(expanded(L(vec![
            A("artifact".into()),
            self::header(header),
            self::lir(lir),
        ]))?)
    }
    /// Replace every compact type spelling in a tree with the list it reads
    /// as, so the tree is exactly what parsing its text would have produced.
    /// Iterative, since the tree is as deep as the artifact.
    fn expanded(root: S) -> Result<S, ParseError> {
        enum Work {
            Value(S),
            Close(usize),
        }
        let mut work = vec![Work::Value(root)];
        let mut out: Vec<S> = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                // Taken out rather than matched out: the tree has a drop of
                // its own, so its parts are moved through `take`.
                Work::Value(mut value) => match &mut value {
                    Raw(text) => {
                        let text = std::mem::take(text);
                        let mut parser = Parser {
                            input: &text,
                            at: 0,
                        };
                        out.push(parser.value()?);
                    }
                    L(values) => {
                        let values = std::mem::take(values);
                        work.push(Work::Close(values.len()));
                        work.extend(values.into_iter().rev().map(Work::Value));
                    }
                    A(_) | Q(_) => out.push(value),
                },
                Work::Close(count) => {
                    let at = out.len() - count;
                    let children = out.split_off(at);
                    out.push(L(children));
                }
            }
        }
        Ok(out.pop().expect("a tree expands to one value"))
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
            L(vec![A("kind".into()), A(value.kind.to_string())]),
            L(vec![
                A("identity".into()),
                Q(value.identity.name.clone()),
                Q(value.identity.version.clone()),
            ]),
            L(vec![
                A("compiler".into()),
                Q(value.compiler.as_str().to_string()),
            ]),
            L(vec![A("domains".into()), A(value.domains.name().into())]),
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
            L(std::iter::once(A("modules".into()))
                .chain(value.modules.iter().map(module))
                .collect()),
        ])
    }
    fn module(value: &DeclaredModule) -> S {
        L(vec![
            A("module".into()),
            Q(value.name.clone()),
            metadata(&value.metadata),
        ])
    }
    /// `(metadata (entry "key" <data>) ...)` — a declaration's metadata, in
    /// the order it was written.
    fn metadata(value: &Metadata) -> S {
        L(std::iter::once(A("metadata".into()))
            .chain(
                value
                    .iter()
                    .map(|(key, value)| L(vec![A("entry".into()), Q(key.clone()), data(value)])),
            )
            .collect())
    }
    /// One metadata value. Recursive, since the reader bounds the depth
    /// before any tree this deep can exist.
    fn data(value: &Data) -> S {
        match value {
            Data::Natural(value) => L(vec![A("nat".into()), A(value.to_string())]),
            Data::Fixed(value) => L(vec![
                A("fixed".into()),
                A(value.kind().suffix().into()),
                A(value.value().to_string()),
            ]),
            Data::Integer(value) => L(vec![A("int".into()), A(value.to_string())]),
            Data::Real(value) => L(vec![A("real".into()), A(value.to_string())]),
            Data::String(value) => L(vec![A("string".into()), Q(value.clone())]),
            Data::Bool(value) => L(vec![A("bool".into()), A(value.to_string())]),
            Data::Array(items) => L(std::iter::once(A("array".into()))
                .chain(items.iter().map(data))
                .collect()),
            Data::Struct(fields) => {
                L(std::iter::once(A("struct".into()))
                    .chain(fields.iter().map(|(name, value)| {
                        L(vec![A("field".into()), Q(name.clone()), data(value)])
                    }))
                    .collect())
            }
            Data::Tag { name, payload } => L(vec![A("tag".into()), Q(name.clone()), data(payload)]),
        }
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
            metadata(&value.metadata),
        ])
    }
    fn declared_type(value: &DeclaredType) -> S {
        L(vec![
            A("type".into()),
            Q(value.name.clone()),
            A(value.exported.to_string()),
            L(std::iter::once(A("params".into()))
                .chain(value.params.iter().map(parameter))
                .collect()),
            scheme(&value.scheme),
            metadata(&value.metadata),
        ])
    }
    fn parameter(value: &Parameter) -> S {
        L(vec![
            A("param".into()),
            A(match value.sense {
                Sense::Region => "region",
                Sense::Type => "type",
                Sense::Row => "row",
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
            A(value.exported.to_string()),
            L(std::iter::once(A("params".into()))
                .chain(value.params.iter().map(parameter))
                .collect()),
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
                EffectKind::Alias(row) => L(vec![
                    A("alias".into()),
                    L(std::iter::once(A("cases".into()))
                        .chain(row.cases.iter().map(|case| {
                            L(std::iter::once(A("case".into()))
                                .chain(std::iter::once(Q(case.name.clone())))
                                .chain(case.args.iter().map(ty))
                                .collect())
                        }))
                        .collect()),
                    L(vec![
                        A("tail".into()),
                        match row.tail {
                            Some(index) => A(index.to_string()),
                            None => A("none".into()),
                        },
                    ]),
                ]),
            },
            metadata(&value.metadata),
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
        let mut parts = vec![
            A("scheme".into()),
            A(value.count.to_string()),
            A(value.presences.to_string()),
            L(std::iter::once(A("existentials".into()))
                .chain(value.existentials.iter().map(|index| A(index.to_string())))
                .collect()),
            formula(&value.formula),
            ty(&value.body),
        ];
        if !value.representations.is_empty() {
            parts.insert(
                4,
                L(std::iter::once(A("representations".into()))
                    .chain(
                        value
                            .representations
                            .iter()
                            .map(|index| A(index.to_string())),
                    )
                    .collect()),
            );
        }
        if let Some(callable) = &value.callable {
            parts.push(L(vec![
                A("callable".into()),
                Q(serde_json::to_string(callable).expect("serializable callable interface")),
            ]));
        }
        L(parts)
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
                        Type::Fixed(kind) => work.push(Work::Text(kind.suffix())),
                        Type::Real => work.push(Work::Text("real")),
                        Type::String => work.push(Work::Text("string")),
                        Type::Bool => work.push(Work::Text("boolean")),
                        Type::ForeignValue => work.push(Work::Text("foreign-value")),
                        Type::Arrow(from, to, row) => {
                            out.push_str("(arrow ");
                            work.push(Work::Text(")"));
                            work.push(Work::Row(row));
                            work.push(Work::Text(" "));
                            work.push(Work::Ty(to));
                            work.push(Work::Text(" "));
                            work.push(Work::Ty(from));
                        }
                        Type::Package(body) => {
                            out.push_str("(package ");
                            work.push(Work::Text(")"));
                            work.push(Work::Ty(body));
                        }
                        Type::Hidden { binder, name, body } => {
                            out.push_str("(hidden ");
                            out.push_str(&binder.to_string());
                            out.push(' ');
                            out.push_str(&quoted(name));
                            out.push(' ');
                            work.push(Work::Text(")"));
                            work.push(Work::Ty(body));
                        }
                        Type::HiddenVar { binder, name } => {
                            out.push_str("(hidden-var ");
                            out.push_str(&binder.to_string());
                            out.push(' ');
                            out.push_str(&quoted(name));
                            out.push(')');
                        }
                        Type::Mut(region, element) => {
                            out.push_str("(mut ");
                            work.push(Work::Text(")"));
                            work.push(Work::Ty(element));
                            work.push(Work::Text(" "));
                            work.push(Work::Ty(region));
                        }
                        Type::Array(element) => {
                            out.push_str("(array ");
                            work.push(Work::Text(")"));
                            work.push(Work::Ty(element));
                        }
                        Type::Mirror(element) => {
                            out.push_str("(mirror ");
                            work.push(Work::Text(")"));
                            work.push(Work::Ty(element));
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
                Formula::Owned(_, inner) | Formula::Not(inner) => depth.push((inner, at + 1)),
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
            Owned(u32),
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
                Work::Formula(Formula::Owned(owner, inner)) => {
                    work.push(Work::Owned(*owner));
                    work.push(Work::Formula(inner));
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
                Work::Owned(owner) => {
                    let inner = out.pop().expect("owned formula text postorder");
                    out.push(L(vec![A("owned".into()), A(owner.to_string()), inner]));
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
                Work::Formula(Formula::Owned(owner, inner)) => {
                    out.push_str("(owned ");
                    out.push_str(&owner.to_string());
                    out.push(' ');
                    work.push(Work::Text(")"));
                    work.push(Work::Formula(inner));
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
            A("cps-lir".into()),
            Q(serde_json::to_string(value).expect("portable CPS data serializes")),
        ])
    }

    /// One step of laying a tree out: a value to write at an indentation,
    /// flat or broken, the text of a bracket, or the separator between two
    /// values of a list, which is a space when the list is flat and a line
    /// break to the list's indentation otherwise.
    enum Cmd<'a> {
        Value(&'a S, usize, bool),
        Text(&'static str),
        Line(usize, bool),
    }

    /// Lay a tree out at [`WIDTH`], with every list either on one line or
    /// with each of its values on a line of its own, indented two past the
    /// list.
    ///
    /// A list goes on one line when it fits there together with whatever
    /// follows it on that line — the brackets closing its ancestors — which
    /// is the rule the Wadler-style printers apply, and the one this text
    /// was canonical under before it was laid out here. Both the layout and
    /// its fitting check run on explicit stacks: a list nested as deeply as
    /// an artifact's blocks can be costs memory, not stack, and the check
    /// stops the moment a line is overrun, so it reads at most a line's worth
    /// of text however large the list.
    fn layout(root: &S) -> String {
        let mut out = String::new();
        let mut column = 0;
        let mut stack = vec![Cmd::Value(root, 0, false)];
        while let Some(cmd) = stack.pop() {
            match cmd {
                Cmd::Text(text) => {
                    out.push_str(text);
                    column += text.len();
                }
                Cmd::Line(_, true) => {
                    out.push(' ');
                    column += 1;
                }
                Cmd::Line(indent, false) => {
                    out.push('\n');
                    out.extend(std::iter::repeat_n(' ', indent));
                    column = indent;
                }
                Cmd::Value(L(values), indent, flat) => {
                    let flat = flat || fits(values, &stack, WIDTH.saturating_sub(column));
                    stack.push(Cmd::Text(")"));
                    for (at, value) in values.iter().enumerate().rev() {
                        stack.push(Cmd::Value(value, indent + 2, flat));
                        if at > 0 {
                            stack.push(Cmd::Line(indent + 2, flat));
                        }
                    }
                    stack.push(Cmd::Text("("));
                }
                Cmd::Value(value, _, _) => {
                    let text = leaf(value);
                    column += text.width();
                    out.push_str(&text);
                }
            }
        }
        out
    }

    /// Whether a list laid out flat fits in `remaining` columns along with
    /// what the pending commands put after it on the same line: text up to
    /// the first line break that is not itself flat.
    fn fits(values: &[S], pending: &[Cmd<'_>], remaining: usize) -> bool {
        let mut remaining = remaining as isize;
        // The list itself, flat: its brackets, its values and the spaces
        // between them, in order.
        let mut work: Vec<Cmd<'_>> = vec![Cmd::Text(")")];
        for (at, value) in values.iter().enumerate().rev() {
            work.push(Cmd::Value(value, 0, true));
            if at > 0 {
                work.push(Cmd::Line(0, true));
            }
        }
        work.push(Cmd::Text("("));
        let mut after = pending.iter().rev();
        loop {
            let cmd = match work.pop() {
                Some(cmd) => cmd,
                None => match after.next() {
                    // Nothing follows on the line, and everything fit.
                    None => return true,
                    Some(Cmd::Value(value, indent, flat)) => Cmd::Value(value, *indent, *flat),
                    Some(Cmd::Text(text)) => Cmd::Text(text),
                    Some(Cmd::Line(indent, flat)) => Cmd::Line(*indent, *flat),
                },
            };
            match cmd {
                Cmd::Line(_, false) => return true,
                Cmd::Line(_, true) => remaining -= 1,
                Cmd::Text(text) => remaining -= text.len() as isize,
                Cmd::Value(L(values), _, flat) => {
                    work.push(Cmd::Text(")"));
                    for (at, value) in values.iter().enumerate().rev() {
                        work.push(Cmd::Value(value, 0, flat));
                        if at > 0 {
                            work.push(Cmd::Line(0, flat));
                        }
                    }
                    work.push(Cmd::Text("("));
                }
                Cmd::Value(value, _, _) => remaining -= leaf(value).width() as isize,
            }
            if remaining < 0 {
                return false;
            }
        }
    }

    /// The text of a value that is not a list.
    fn leaf(value: &S) -> std::borrow::Cow<'_, str> {
        match value {
            A(text) | Raw(text) => std::borrow::Cow::Borrowed(text),
            Q(text) => std::borrow::Cow::Owned(quoted(text)),
            L(_) => unreachable!("lists are laid out, not written"),
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
        artifact.lir.functions.clear();
        artifact.lir.globals.clear();
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
            if let Err(error) = super::regions::validate(&artifact.header) {
                self.fail(error);
            }
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
            let mut values = self.exact(self.list(value, "header"), 9, "header");
            let mut kind = self.exact(self.list(self.take(&mut values), "kind"), 1, "kind");
            let kind = match self.atom(self.take(&mut kind)).as_str() {
                "library" => Kind::Library,
                "executable" => Kind::Executable,
                _ => {
                    self.fail("invalid bundle kind");
                    Kind::Library
                }
            };
            let identity = {
                let mut value =
                    self.exact(self.list(self.take(&mut values), "identity"), 2, "identity");
                Identity {
                    name: self.string(self.take(&mut value)),
                    version: self.string(self.take(&mut value)),
                }
            };
            let compiler = {
                let mut value =
                    self.exact(self.list(self.take(&mut values), "compiler"), 1, "compiler");
                Stamp::recorded(self.string(self.take(&mut value)))
            };
            let domains = {
                let mut value =
                    self.exact(self.list(self.take(&mut values), "domains"), 1, "domains");
                match types::Domains::from_name(&self.atom(self.take(&mut value))) {
                    Some(domains) => domains,
                    None => {
                        self.fail("invalid target domains");
                        types::Domains::default()
                    }
                }
            };
            Header {
                kind,
                identity,
                compiler,
                domains,
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
                effects: {
                    let effects: Vec<DeclaredEffect> = self
                        .many(self.take(&mut values), "effects")
                        .into_iter()
                        .map(|value| self.read_effect(value))
                        .collect();
                    self.check_effects(&effects);
                    effects
                },
                modules: self
                    .many(self.take(&mut values), "modules")
                    .into_iter()
                    .map(|value| self.read_module(value))
                    .collect(),
            }
        }
        fn read_module(&self, value: S) -> DeclaredModule {
            let mut value = self.exact(self.list(value, "module"), 2, "module");
            DeclaredModule {
                name: self.string(self.take(&mut value)),
                metadata: self.read_metadata(self.take(&mut value)),
            }
        }
        fn read_metadata(&self, value: S) -> Metadata {
            let mut metadata = Metadata::new();
            for entry in self.many(value, "metadata") {
                let mut entry = self.exact(self.list(entry, "entry"), 2, "entry");
                let key = self.string(self.take(&mut entry));
                let data = self.read_data(self.take(&mut entry), 1);
                if metadata.insert(key, data).is_some() {
                    self.fail("repeated metadata key");
                }
            }
            metadata
        }
        /// One metadata value, `depth` values down from the top of its entry.
        /// Recursive, and safe to be: nothing below the depth limit is read,
        /// so the stack this uses is bounded by the limit and not by the file.
        fn read_data(&self, value: S, depth: usize) -> Data {
            if depth > ir::METADATA_DEPTH_LIMIT {
                return self.invalid("metadata nested too deeply", Data::Struct(IndexMap::new()));
            }
            let mut values = list_contents(value)
                .unwrap_or_else(|| self.invalid("invalid metadata value", Vec::new()));
            let tag = self.atom(self.take(&mut values));
            match tag.as_str() {
                "nat" => Data::Natural(self.number(self.exact(values, 1, "nat").remove(0))),
                "fixed" => {
                    let mut values = self.exact(values, 2, "fixed");
                    let suffix = self.atom(self.take(&mut values));
                    let number = self.number(self.take(&mut values));
                    let literal = crate::types::FixedInt::ALL
                        .into_iter()
                        .find(|kind| kind.suffix() == suffix)
                        .and_then(|kind| crate::types::FixedLiteral::new(kind, number));
                    match literal {
                        Some(value) => Data::Fixed(value),
                        None => self.invalid("invalid fixed-width literal", Data::Integer(0)),
                    }
                }
                "int" => Data::Integer(self.number(self.exact(values, 1, "int").remove(0))),
                "real" => Data::Real(self.number(self.exact(values, 1, "real").remove(0))),
                "string" => Data::String(self.string(self.exact(values, 1, "string").remove(0))),
                "bool" => Data::Bool(self.boolean(self.exact(values, 1, "bool").remove(0))),
                "array" => Data::Array(
                    values
                        .into_iter()
                        .map(|item| self.read_data(item, depth + 1))
                        .collect(),
                ),
                "struct" => {
                    let mut fields = IndexMap::new();
                    for field in values {
                        let mut field = self.exact(self.list(field, "field"), 2, "field");
                        let name = self.string(self.take(&mut field));
                        let value = self.read_data(self.take(&mut field), depth + 1);
                        if fields.insert(name, value).is_some() {
                            self.fail("repeated metadata field");
                        }
                    }
                    Data::Struct(fields)
                }
                "tag" => {
                    let mut values = self.exact(values, 2, "tag");
                    Data::Tag {
                        name: self.string(self.take(&mut values)),
                        payload: Box::new(self.read_data(self.take(&mut values), depth + 1)),
                    }
                }
                _ => self.invalid("invalid metadata value", Data::Struct(IndexMap::new())),
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
            let mut value = self.exact(self.list(value, "value"), 3, "value");
            Value {
                name: self.string(self.take(&mut value)),
                scheme: self.read_scheme(self.take(&mut value)),
                metadata: self.read_metadata(self.take(&mut value)),
            }
        }
        fn read_declared_type(&self, value: S) -> DeclaredType {
            let mut value = self.exact(self.list(value, "type"), 5, "type");
            DeclaredType {
                name: self.string(self.take(&mut value)),
                exported: self.boolean(self.take(&mut value)),
                params: self
                    .many(self.take(&mut value), "params")
                    .into_iter()
                    .map(|value| self.read_parameter(value))
                    .collect(),
                scheme: self.read_scheme(self.take(&mut value)),
                metadata: self.read_metadata(self.take(&mut value)),
            }
        }
        fn read_parameter(&self, value: S) -> Parameter {
            let mut value = self.exact(self.list(value, "param"), 3, "param");
            Parameter {
                sense: match self.atom(self.take(&mut value)).as_str() {
                    "region" => Sense::Region,
                    "type" => Sense::Type,
                    "row" | "fields" | "cases" => Sense::Row,
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
            let mut value = self.exact(self.list(value, "effect"), 6, "effect");
            let name = self.string(self.take(&mut value));
            let exported = self.boolean(self.take(&mut value));
            let params: Vec<Parameter> = self
                .many(self.take(&mut value), "params")
                .into_iter()
                .map(|value| self.read_parameter(value))
                .collect();
            let count = params.len() as u32;
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
                                .map(|value| {
                                    let operation = self.read_operation(value);
                                    // A signature refers to the effect's own
                                    // parameters and to nothing else that is
                                    // bound: no presence is quantified there.
                                    self.check_bounds(&operation.from, count, 0);
                                    self.check_bounds(&operation.to, count, 0);
                                    operation
                                })
                                .collect(),
                        ),
                        "alias" => {
                            let mut values = self.exact(values, 2, "alias");
                            let cases = self
                                .many(self.take(&mut values), "cases")
                                .into_iter()
                                .map(|value| {
                                    let mut value = self.list(value, "case");
                                    if value.is_empty() {
                                        return self.invalid(
                                            "alias case names no effect",
                                            AliasCase {
                                                name: String::new(),
                                                args: Vec::new(),
                                            },
                                        );
                                    }
                                    let name = self.string(self.take(&mut value));
                                    let args: Vec<Type> = value
                                        .into_iter()
                                        .map(|value| self.read_ty(value))
                                        .collect();
                                    for arg in &args {
                                        self.check_bounds(arg, count, 0);
                                    }
                                    AliasCase { name, args }
                                })
                                .collect();
                            let mut tail =
                                self.exact(self.list(self.take(&mut values), "tail"), 1, "tail");
                            let tail = match self.take(&mut tail) {
                                A(ref none) if none == "none" => None,
                                value => {
                                    let index = self.number(value);
                                    if index >= count {
                                        self.fail("alias tail is outside the effect's parameters");
                                    }
                                    Some(index)
                                }
                            };
                            EffectKind::Alias(AliasRow { cases, tail })
                        }
                        _ => self.invalid(
                            "invalid effect kind",
                            EffectKind::Alias(AliasRow::naming(Vec::new())),
                        ),
                    }
                }
                _ => self.invalid(
                    "invalid effect kind",
                    EffectKind::Alias(AliasRow::naming(Vec::new())),
                ),
            };
            let metadata = self.read_metadata(self.take(&mut value));
            DeclaredEffect {
                name,
                exported,
                params,
                identity,
                kind,
                metadata,
            }
        }

        /// Every bound position in a type refers inside the quantifier space
        /// it is read in: `count` type and row positions, of which the first
        /// `presences` are presences.
        fn check_bounds(&self, root: &Type, count: u32, presences: u32) {
            enum Part<'a> {
                Ty(&'a Type),
                Row(&'a Row),
            }
            let mut parts = vec![Part::Ty(root)];
            while let Some(part) = parts.pop() {
                match part {
                    Part::Ty(ty) => match ty {
                        Type::Bound(index) => {
                            if *index < presences || *index >= count {
                                self.fail("type bound is outside the type quantifier space");
                            }
                        }
                        Type::Package(inner)
                        | Type::Array(inner)
                        | Type::Mirror(inner)
                        | Type::Hidden { body: inner, .. } => parts.push(Part::Ty(inner)),
                        Type::Mut(region, element) => {
                            parts.push(Part::Ty(region));
                            parts.push(Part::Ty(element));
                        }
                        Type::Arrow(from, to, row) => {
                            parts.push(Part::Row(row));
                            parts.push(Part::Ty(to));
                            parts.push(Part::Ty(from));
                        }
                        Type::Struct(row) | Type::Sum(row) => parts.push(Part::Row(row)),
                        Type::Named { args, .. } => {
                            parts.extend(args.iter().rev().map(Part::Ty));
                        }
                        _ => {}
                    },
                    Part::Row(row) => {
                        if let Rest::Bound(index) = row.rest
                            && (index < presences || index >= count)
                        {
                            self.fail("row bound is outside the row quantifier space");
                        }
                        if let Rest::More(more) = &row.rest {
                            parts.push(Part::Row(more));
                        }
                        for (_, field) in row.labels.iter().rev() {
                            if let Presence::Bound(index) = field.presence
                                && index >= presences
                            {
                                self.fail(
                                    "presence bound is outside the presence quantifier space",
                                );
                            }
                            parts.push(Part::Ty(&field.ty));
                        }
                    }
                }
            }
        }

        /// What one header's effects say about each other: an alias applies an
        /// effect of the same header to as many arguments as it takes, and no
        /// ring of aliases stands for itself.
        fn check_effects(&self, effects: &[DeclaredEffect]) {
            let arity: HashMap<&str, usize> = effects
                .iter()
                .map(|effect| (effect.name.as_str(), effect.params.len()))
                .collect();
            let aliases: HashMap<&str, &AliasRow> = effects
                .iter()
                .filter_map(|effect| match &effect.kind {
                    EffectKind::Alias(row) => Some((effect.name.as_str(), row)),
                    EffectKind::Operations(_) => None,
                })
                .collect();
            for row in aliases.values() {
                for case in &row.cases {
                    if let Some(expected) = arity.get(case.name.as_str())
                        && *expected != case.args.len()
                    {
                        self.fail("alias applies an effect to the wrong number of arguments");
                    }
                }
            }
            // Every alias, walked through the aliases it names; one met again
            // on the way down is a ring. An alias walked to the end once is
            // done for good, so a diamond of aliases is walked once rather
            // than once per path to it.
            let mut done: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for start in aliases.keys() {
                if done.contains(start) {
                    continue;
                }
                let mut stack = vec![(*start, 0usize)];
                let mut path = vec![*start];
                while let Some((name, at)) = stack.pop() {
                    let Some(row) = aliases.get(name) else {
                        continue;
                    };
                    let Some(case) = row.cases.get(at) else {
                        path.pop();
                        done.insert(name);
                        continue;
                    };
                    stack.push((name, at + 1));
                    let next = case.name.as_str();
                    if !aliases.contains_key(next) || done.contains(next) {
                        continue;
                    }
                    if path.contains(&next) {
                        self.fail("alias effects stand for themselves");
                        return;
                    }
                    path.push(next);
                    stack.push((next, 0));
                }
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
            let mut value = self.list(value, "scheme");
            let callable = if value.last().is_some_and(|value| matches!(value, S::List(parts) if matches!(parts.first(), Some(S::Atom(tag)) if tag == "callable"))) {
                let mut encoded = self.exact(self.list(value.pop().unwrap(), "callable"), 1, "callable");
                let encoded = self.string(self.take(&mut encoded));
                match serde_json::from_str::<crate::reification::interface::Interface>(&encoded) {
                    Ok(callable) => Some(callable),
                    Err(_) => { self.fail("invalid callable interface encoding"); None }
                }
            } else { None };
            let has_representations = value.len() == 6;
            let mut value = self.exact(value, if has_representations { 6 } else { 5 }, "scheme");
            let count = self.number(self.take(&mut value));
            let presences = self.number(self.take(&mut value));
            let mut encoded = self.list(self.take(&mut value), "existentials");
            let mut existentials = Vec::new();
            while !encoded.is_empty() {
                existentials.push(self.number(self.take(&mut encoded)));
            }
            if presences > count {
                self.fail("scheme presence count exceeds quantifier count");
            }
            if existentials.iter().any(|index| *index >= presences)
                || existentials.windows(2).any(|pair| pair[0] >= pair[1])
            {
                self.fail("invalid existential presence positions");
            }
            let mut representations = Vec::new();
            if has_representations {
                let mut encoded = self.list(self.take(&mut value), "representations");
                while !encoded.is_empty() {
                    representations.push(self.number(self.take(&mut encoded)));
                }
                if representations
                    .iter()
                    .any(|index| *index < presences || *index >= count)
                    || representations.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    self.fail("invalid runtime representation positions");
                }
            }
            if let Some(callable) = &callable
                && let Err(message) = callable.validate(count, presences)
            {
                self.fail(&message);
            }
            let formula = self.read_formula(self.take(&mut value));
            let body = self.read_ty(self.take(&mut value));
            // Package owners are exact structural preorder positions. Validate
            // them against the decoded body rather than guessing from arrow
            // shape or from which existential slots happen to occur nearby.
            let mut package_count = 0u32;
            let mut slot_owners = HashMap::new();
            enum Part<'a> {
                Ty(&'a Type, Option<u32>),
                Row(&'a Row, Option<u32>),
            }
            let mut parts = vec![Part::Ty(&body, None)];
            while let Some(part) = parts.pop() {
                match part {
                    Part::Ty(ty, owner) => match ty {
                        Type::Bound(index) => {
                            if *index < presences || *index >= count {
                                self.fail("type bound is outside the type quantifier space");
                            }
                        }
                        Type::Package(inner) => {
                            let here = package_count;
                            package_count += 1;
                            parts.push(Part::Ty(inner, Some(here)));
                        }
                        Type::Hidden { body, .. } => parts.push(Part::Ty(body, owner)),
                        Type::Arrow(from, to, row) => {
                            parts.push(Part::Row(row, owner));
                            parts.push(Part::Ty(to, owner));
                            parts.push(Part::Ty(from, owner));
                        }
                        Type::Mut(region, element) => {
                            parts.push(Part::Ty(region, owner));
                            parts.push(Part::Ty(element, owner));
                        }
                        Type::Array(inner) | Type::Mirror(inner) => {
                            parts.push(Part::Ty(inner, owner))
                        }
                        Type::Struct(row) | Type::Sum(row) => {
                            parts.push(Part::Row(row, owner));
                        }
                        Type::Named { args, .. } => {
                            parts.extend(args.iter().rev().map(|ty| Part::Ty(ty, owner)))
                        }
                        _ => {}
                    },
                    Part::Row(row, owner) => {
                        if let Rest::Bound(index) = row.rest
                            && (index < presences || index >= count)
                        {
                            self.fail("row bound is outside the row quantifier space");
                        }
                        if let Rest::More(more) = &row.rest {
                            parts.push(Part::Row(more, owner));
                        }
                        for (_, field) in row.labels.iter().rev() {
                            if let Presence::Bound(index) = field.presence {
                                if index >= presences {
                                    self.fail(
                                        "presence bound is outside the presence quantifier space",
                                    );
                                }
                                if existentials.binary_search(&index).is_ok() {
                                    let Some(owner) = owner else {
                                        self.fail("existential presence occurs outside a package");
                                        continue;
                                    };
                                    if slot_owners
                                        .insert(index, owner)
                                        .is_some_and(|before| before != owner)
                                    {
                                        self.fail("existential presence crosses package owners");
                                    }
                                }
                            }
                            parts.push(Part::Ty(&field.ty, owner));
                        }
                    }
                }
            }
            if existentials
                .iter()
                .any(|index| !slot_owners.contains_key(index))
            {
                self.fail("existential presence has no package-owned occurrence");
            }

            // `Owned` is a canonical wrapper around one top-level conjunct.
            // Its owner is determined by the existential atoms it mentions;
            // universal atoms may occur in the same indivisible proposition and
            // remain scoped by this scheme. This rejects incomplete metadata
            // without rejecting an input-to-result guarantee that necessarily
            // relates a universal input to a package-owned result.
            let mut conjuncts = vec![&formula];
            while let Some(part) = conjuncts.pop() {
                if let Formula::And(left, right) = part {
                    conjuncts.push(right);
                    conjuncts.push(left);
                    continue;
                }
                let (claimed, inner) = match part {
                    Formula::Owned(owner, inner) => {
                        if *owner >= package_count {
                            self.fail("formula package owner is outside scheme body");
                        }
                        (Some(*owner), &**inner)
                    }
                    _ => (None, part),
                };
                // Only a conjunction immediately inside `Owned` is
                // partitionable into separate top-level conjuncts. An `And`
                // below `Or`, `Iff`, `Xor`, or `Not` is part of one indivisible
                // proposition and must retain the package wrapper.
                if matches!(inner, Formula::And(..)) {
                    self.fail("owned conjunction is not canonical");
                }
                let mut atoms = Vec::new();
                let mut formulas = vec![inner];
                while let Some(formula) = formulas.pop() {
                    match formula {
                        Formula::Bound(index) => {
                            if *index >= presences {
                                self.fail("formula bound is outside the presence quantifier space");
                            }
                            atoms.push(*index);
                        }
                        // A nested local scheme can retain a captured presence
                        // as an open atom. It scopes the mixed proposition like
                        // a universal bound; existential bounds alone infer its
                        // exact package owner.
                        Formula::Owned(_, _) => {
                            self.fail("nested formula package owners are not canonical");
                        }
                        Formula::Not(inner) => formulas.push(inner),
                        Formula::And(left, right)
                        | Formula::Or(left, right)
                        | Formula::Iff(left, right)
                        | Formula::Xor(left, right) => {
                            formulas.push(right);
                            formulas.push(left);
                        }
                        Formula::True | Formula::False | Formula::Var(_) => {}
                    }
                }
                let mut inferred = None;
                for index in atoms
                    .iter()
                    .copied()
                    .filter(|index| existentials.binary_search(index).is_ok())
                {
                    let owner = slot_owners.get(&index).copied();
                    match (inferred, owner) {
                        (None, owner) => inferred = owner,
                        (Some(before), Some(owner)) if before == owner => {}
                        _ => {
                            self.fail("formula existential atoms cross package owners");
                            inferred = None;
                            break;
                        }
                    }
                }
                if claimed != inferred {
                    self.fail("formula package ownership is incomplete or non-canonical");
                }
            }
            Scheme {
                callable,
                representations,
                count,
                presences,
                existentials,
                formula,
                body,
            }
        }
        fn read_ty(&self, value: S) -> Type {
            enum Task {
                Ty(S),
                Row(S),
                Rest(S),
                Field(S),
                BuildArrow,
                BuildPackage,
                BuildHidden(u32, String),
                BuildMirror,
                BuildArray,
                BuildMut,
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
                                "n8" => Type::Fixed(crate::types::FixedInt::Nat8),
                                "n16" => Type::Fixed(crate::types::FixedInt::Nat16),
                                "n32" => Type::Fixed(crate::types::FixedInt::Nat32),
                                "n64" => Type::Fixed(crate::types::FixedInt::Nat64),
                                "i8" => Type::Fixed(crate::types::FixedInt::Int8),
                                "i16" => Type::Fixed(crate::types::FixedInt::Int16),
                                "i32" => Type::Fixed(crate::types::FixedInt::Int32),
                                "i64" => Type::Fixed(crate::types::FixedInt::Int64),
                                "real" => Type::Real,
                                "string" => Type::String,
                                "boolean" => Type::Bool,
                                "foreign-value" => Type::ForeignValue,
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
                                    "package" => {
                                        let body = self.exact(values, 1, "package").remove(0);
                                        tasks.push(Task::BuildPackage);
                                        tasks.push(Task::Ty(body));
                                    }
                                    "hidden" => {
                                        let mut values = self.exact(values, 3, "hidden");
                                        let binder = self.number(self.take(&mut values));
                                        let name = self.string(self.take(&mut values));
                                        let body = self.take(&mut values);
                                        tasks.push(Task::BuildHidden(binder, name));
                                        tasks.push(Task::Ty(body));
                                    }
                                    "hidden-var" => {
                                        let mut values = self.exact(values, 2, "hidden-var");
                                        let binder = self.number(self.take(&mut values));
                                        let name = self.string(self.take(&mut values));
                                        tys.push(Type::HiddenVar { binder, name });
                                    }
                                    "mut" => {
                                        let mut values = self.exact(values, 2, "mut");
                                        let region = self.take(&mut values);
                                        let element = self.take(&mut values);
                                        tasks.push(Task::BuildMut);
                                        tasks.push(Task::Ty(element));
                                        tasks.push(Task::Ty(region));
                                    }
                                    "array" => {
                                        let element = self.exact(values, 1, "array").remove(0);
                                        tasks.push(Task::BuildArray);
                                        tasks.push(Task::Ty(element));
                                    }
                                    "mirror" => {
                                        let element = self.exact(values, 1, "mirror").remove(0);
                                        tasks.push(Task::BuildMirror);
                                        tasks.push(Task::Ty(element));
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
                    Task::BuildPackage => {
                        let body = tys.pop().expect("package body");
                        tys.push(Type::Package(Box::new(body)));
                    }
                    Task::BuildHidden(binder, name) => {
                        let body = tys.pop().expect("hidden body");
                        tys.push(Type::Hidden {
                            binder,
                            name,
                            body: Box::new(body),
                        });
                    }
                    Task::BuildMut => {
                        let element = tys.pop().expect("cell element");
                        let region = tys.pop().expect("cell region");
                        tys.push(Type::Mut(Box::new(region), Box::new(element)));
                    }
                    Task::BuildArray => {
                        let element = tys.pop().expect("array element");
                        tys.push(Type::Array(Box::new(element)));
                    }
                    Task::BuildMirror => {
                        let element = tys.pop().expect("mirror type");
                        tys.push(Type::Mirror(Box::new(element)));
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
                Owned(u32),
                Not,
                Pair(fn(Box<Formula>, Box<Formula>) -> Formula),
            }
            let mut tasks = vec![Task::Read(value)];
            let mut out = Vec::new();
            while let Some(task) = tasks.pop() {
                match task {
                    Task::Owned(owner) => {
                        let value = out.pop().unwrap_or(Formula::False);
                        out.push(Formula::Owned(owner, Box::new(value)));
                    }
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
                                "owned" => {
                                    let mut values = self.exact(values, 2, "owned");
                                    let owner = self.number(self.take(&mut values));
                                    let value = self.take(&mut values);
                                    tasks.push(Task::Owned(owner));
                                    tasks.push(Task::Read(value));
                                }
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
            let mut values = self.exact(self.list(value, "cps-lir"), 1, "cps-lir");
            let encoded = self.string(self.take(&mut values));
            serde_json::from_str(&encoded).unwrap_or_else(|_| {
                self.invalid(
                    "invalid CPS LIR",
                    Lir {
                        externs: vec![],
                        functions: vec![],
                        globals: vec![],
                    },
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_with_target(target: &str) -> Artifact {
        Artifact {
            header: Header {
                kind: Kind::Library,
                identity: Identity {
                    name: "test".into(),
                    version: "1".into(),
                },
                compiler: Stamp::current(),
                domains: types::Domains::default(),
                dependencies: Vec::new(),
                values: Vec::new(),
                types: Vec::new(),
                effects: Vec::new(),
                modules: Vec::new(),
            },
            lir: Lir {
                externs: vec![Extern {
                    name: "test@1::Main::foreign".into(),
                    target: target.into(),
                    rep: Rep::Any,
                }],
                functions: Vec::new(),
                globals: Vec::new(),
            },
        }
    }

    #[test]
    fn extern_target_uses_one_canonical_string() {
        for target in ["", "console.log", "(value) => value\n+ 1", "a \"quote\""] {
            let artifact = artifact_with_target(target);
            let printed = print(&artifact);
            assert_eq!(try_parse(&printed).unwrap().validate().unwrap(), artifact);
            assert_eq!(try_parse(&printed).unwrap().lir.externs[0].target, target);
        }
    }

    #[test]
    fn extern_target_rejects_old_path_encoding() {
        let printed = print(&artifact_with_target("console.log"));
        let old = printed.replace(
            r#"\"target\":\"console.log\""#,
            r#"\"target\":[\"console\",\"log\"]"#,
        );
        assert_ne!(old, printed);
        assert_eq!(try_parse(&old).unwrap_err().message(), "invalid CPS LIR");
    }

    #[test]
    fn validation_rejects_out_of_range_function_references() {
        let mut unchecked = artifact_with_target("host.value").to_unchecked();
        unchecked.lir.globals.push(Global {
            type_interface: None,
            adapter: None,
            callable: None,
            name: "test@1::value".into(),
            initializer: 1,
        });
        assert_eq!(
            unchecked.validate().unwrap_err().message(),
            "initializer outside function table"
        );
    }
    #[test]
    fn recovery_discards_unrepairable_executable_data() {
        let mut unchecked = artifact_with_target("host.value").to_unchecked();
        unchecked.lir.globals.push(Global {
            type_interface: None,
            adapter: None,
            callable: None,
            name: "test@1::value".into(),
            initializer: 1,
        });
        let (recovered, facts) = unchecked.recover();
        assert!(recovered.lir().functions.is_empty());
        assert!(recovered.lir().globals.is_empty());
        assert!(matches!(
            facts.as_slice(),
            [RecoveryFact::ExecutableDiscarded { .. }]
        ));
    }
}
