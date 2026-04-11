use crate::lower::Namespace;
use crate::parser::RecordFieldSeparator;
use crate::reporting::TextRange;
use crate::wasm;

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct LoweredSource {
    pub root_module: QualifiedName,
    pub modules: Vec<LoweredModule>,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct LoweredModule {
    pub path: QualifiedName,
    pub source_name: String,
    pub range: TextRange,
    pub statements: Vec<Statement>,
    pub exports: ModuleExports,
}

#[derive(Debug, Clone, PartialEq, Default, salsa::Update)]
pub struct ModuleExports {
    pub modules: Vec<NamedPath>,
    pub types: Vec<NamedPath>,
    pub traits: Vec<NamedPath>,
    pub terms: Vec<NamedPath>,
    pub constructors: Vec<NamedPath>,
}

impl ModuleExports {
    pub fn get(&self, namespace: Namespace, name: &str) -> Option<QualifiedName> {
        let entries = match namespace {
            Namespace::Module => &self.modules,
            Namespace::Type => &self.types,
            Namespace::Trait => &self.traits,
            Namespace::Term => &self.terms,
            Namespace::Constructor => &self.constructors,
        };

        entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.path.clone())
    }
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct NamedPath {
    pub name: String,
    pub path: QualifiedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct QualifiedName {
    pub segments: Vec<String>,
    pub range: TextRange,
}

impl QualifiedName {
    pub fn from_text(path: &str, range: TextRange) -> Self {
        let segments = path
            .split("::")
            .filter(|segment| !segment.is_empty())
            .map(ToOwned::to_owned)
            .collect();

        Self { segments, range }
    }

    pub fn text(&self) -> String {
        self.segments.join("::")
    }
    pub fn extend(mut self, segs: impl IntoIterator<Item = String>) -> Self {
        self.segments.extend(segs);
        self
    }
    pub fn range(mut self, range: TextRange) -> Self {
        self.range = range;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct TypeBinder {
    pub id: LocalId,
    pub name: String,
    pub kind_annotation: Option<KindExpr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum KindExpr {
    Type {
        range: TextRange,
    },
    Row {
        range: TextRange,
    },
    Arrow {
        param: Box<KindExpr>,
        result: Box<KindExpr>,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ResolvedName {
    Global(QualifiedName),
    Local {
        id: LocalId,
        name: String,
        range: TextRange,
    },
    Error {
        name: String,
        range: TextRange,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct ErrorNode {
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum Statement {
    ModuleDecl {
        name: String,
        module: QualifiedName,
        range: TextRange,
    },
    Let {
        kind: LetStatementKind,
        range: TextRange,
    },
    Type {
        name: QualifiedName,
        declared_kind: Option<KindExpr>,
        kind: TypeStatementKind,
        range: TextRange,
    },
    Trait {
        name: QualifiedName,
        params: Vec<TypeBinder>,
        items: Vec<TraitItem>,
        range: TextRange,
    },
    TraitAlias {
        name: QualifiedName,
        target: Option<QualifiedName>,
        range: TextRange,
    },
    Impl {
        trait_ref: Option<QualifiedName>,
        for_types: Vec<TypeExpr>,
        items: Vec<ImplItem>,
        range: TextRange,
    },
    Wasm {
        declarations: Vec<WasmTopLevelDeclaration>,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum LetStatementKind {
    PatternBinding {
        pattern: Pattern,
        value: Expr,
    },
    ConstructorAlias {
        alias: Option<QualifiedName>,
        target: Option<QualifiedName>,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeStatementKind {
    Alias { value: TypeExpr },
    Nominal { definition: TypeDefinition },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeDefinition {
    Lambda {
        params: Vec<TypeBinder>,
        body: Box<TypeDefinition>,
        range: TextRange,
    },
    Struct {
        members: Vec<RecordTypeMember>,
        range: TextRange,
    },
    Sum {
        variants: Vec<SumVariant>,
        range: TextRange,
    },
    Opaque {
        representation: TypeExpr,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum RecordTypeMember {
    Field {
        name: Option<String>,
        ty: TypeExpr,
        range: TextRange,
    },
    Spread {
        ty: TypeExpr,
        range: TextRange,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct SumVariant {
    pub name: Option<QualifiedName>,
    pub argument: Option<TypeExpr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TraitItem {
    Method {
        name: Option<QualifiedName>,
        ty: TypeExpr,
        range: TextRange,
    },
    Type {
        name: Option<QualifiedName>,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ImplItem {
    Method {
        name: Option<QualifiedName>,
        value: Expr,
        range: TextRange,
    },
    Type {
        name: Option<QualifiedName>,
        value: TypeExpr,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum TypeExpr {
    Forall {
        params: Vec<TypeBinder>,
        body: Box<TypeExpr>,
        constraints: Vec<TraitConstraint>,
        range: TextRange,
    },
    Lambda {
        params: Vec<TypeBinder>,
        body: Box<TypeExpr>,
        range: TextRange,
    },
    Function {
        param: Box<TypeExpr>,
        result: Box<TypeExpr>,
        range: TextRange,
    },
    Apply {
        callee: Box<TypeExpr>,
        argument: Box<TypeExpr>,
        range: TextRange,
    },
    Record {
        members: Vec<RecordTypeMember>,
        range: TextRange,
    },
    Name {
        name: ResolvedName,
    },
    Hole {
        range: TextRange,
    },
    Tuple {
        elements: Vec<TypeExpr>,
        range: TextRange,
    },
    Unit {
        range: TextRange,
    },
    Array {
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct TraitConstraint {
    pub trait_ref: Option<QualifiedName>,
    pub args: Vec<TypeExpr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum Pattern {
    Constructor {
        constructor: QualifiedName,
        argument: Box<Pattern>,
        range: TextRange,
    },
    ConstructorName {
        constructor: QualifiedName,
        range: TextRange,
    },
    Binding {
        name: ResolvedName,
        range: TextRange,
    },
    Hole {
        range: TextRange,
    },
    Annotated {
        pattern: Box<Pattern>,
        ty: TypeExpr,
        range: TextRange,
    },
    Literal(Literal),
    Tuple {
        elements: Vec<Pattern>,
        range: TextRange,
    },
    Array {
        elements: Vec<ArrayPatternElement>,
        range: TextRange,
    },
    Record {
        fields: Vec<RecordPatternField>,
        open: bool,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ArrayPatternElement {
    Item(Pattern),
    Rest {
        binding: Option<ResolvedName>,
        range: TextRange,
    },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct RecordPatternField {
    pub name: Option<String>,
    pub value: Option<Pattern>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum Expr {
    Let {
        pattern: Pattern,
        value: Box<Expr>,
        body: Box<Expr>,
        range: TextRange,
    },
    Function {
        params: Vec<Pattern>,
        body: Box<Expr>,
        range: TextRange,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        range: TextRange,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        range: TextRange,
    },
    Apply {
        callee: Box<Expr>,
        argument: Box<Expr>,
        range: TextRange,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: Option<String>,
        range: TextRange,
    },
    Name(ResolvedName),
    Literal(Literal),
    Unit {
        range: TextRange,
    },
    Tuple {
        elements: Vec<Expr>,
        range: TextRange,
    },
    Array {
        elements: Vec<ArrayElement>,
        range: TextRange,
    },
    Record {
        fields: Vec<RecordField>,
        range: TextRange,
    },
    InlineWasm {
        locals: Vec<WasmTypedBinding>,
        instructions: Vec<WasmInstructionNode>,
        range: TextRange,
    },
    Error(ErrorNode),
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ArrayElement {
    Item(Expr),
    Spread { expr: Expr, range: TextRange },
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct RecordField {
    pub name: Option<String>,
    pub separator: RecordFieldSeparator,
    pub value: Expr,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Literal {
    pub value: LiteralValue,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum LiteralValue {
    Integer(i64),
    Natural(u64),
    Real(RealLiteral),
    String(String),
    Glyph(String),
    FormatString(Vec<FormatStringSegment>),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum FormatStringSegment {
    Text(String),
    Placeholder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct RealLiteral(pub u64);

impl RealLiteral {
    pub fn new(value: f64) -> Self {
        Self(value.to_bits())
    }

    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum WasmSymbolNamespace {
    Local,
    Function,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct WasmResolvedSymbol {
    pub namespace: WasmSymbolNamespace,
    pub symbol: String,
    pub resolved_index: u32,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct WasmBinding {
    pub name: String,
    pub index: u32,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct WasmTypedBinding {
    pub binding: WasmBinding,
    pub ty: wasm::ValueType,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct WasmInstructionNode {
    pub instruction: wasm::Instruction,
    pub range: TextRange,
    pub symbols: Vec<WasmResolvedSymbol>,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct WasmFunctionDecl {
    pub binding: Option<WasmBinding>,
    pub index: u32,
    pub function: wasm::Function,
    pub params: Vec<WasmTypedBinding>,
    pub locals: Vec<WasmTypedBinding>,
    pub instructions: Vec<WasmInstructionNode>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct WasmGlobalDecl {
    pub binding: Option<WasmBinding>,
    pub index: u32,
    pub global: wasm::Global,
    pub instructions: Vec<WasmInstructionNode>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum WasmTopLevelDeclaration {
    Function(WasmFunctionDecl),
    Global(WasmGlobalDecl),
    Error(ErrorNode),
}
