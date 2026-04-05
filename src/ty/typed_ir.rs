//! Type-annotated IR used after HM-style inference.
//!
//! This representation intentionally stays close to [`crate::lower::ir`], but
//! attaches canonical [`TypeId`] values directly to expressions and patterns.

use super::TypeId;
use crate::lower::ir as lir;
use crate::parser::RecordFieldSeparator;
use crate::reporting::TextRange;

/// Fully type-annotated source graph.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Source {
    pub root_module: lir::QualifiedName,
    pub modules: Vec<Module>,
}

/// Fully type-annotated module.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Module {
    pub path: lir::QualifiedName,
    pub source_name: String,
    pub range: TextRange,
    pub statements: Vec<Statement>,
    pub exports: lir::ModuleExports,
}

/// Type-annotated top-level statements.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum Statement {
    ModuleDecl {
        name: String,
        module: lir::QualifiedName,
        range: TextRange,
    },
    Let {
        kind: LetStatementKind,
        range: TextRange,
    },
    Type {
        name: lir::QualifiedName,
        params: Vec<lir::TypeBinder>,
        kind: lir::TypeStatementKind,
        range: TextRange,
    },
    Trait {
        name: lir::QualifiedName,
        params: Vec<lir::TypeBinder>,
        items: Vec<lir::TraitItem>,
        range: TextRange,
    },
    TraitAlias {
        name: lir::QualifiedName,
        target: Option<lir::QualifiedName>,
        range: TextRange,
    },
    Impl {
        trait_ref: Option<lir::QualifiedName>,
        for_types: Vec<lir::TypeExpr>,
        items: Vec<lir::ImplItem>,
        range: TextRange,
    },
    Wasm {
        declarations: Vec<lir::WasmTopLevelDeclaration>,
        range: TextRange,
    },
    Error(lir::ErrorNode),
}

/// Type-annotated top-level `let` statements.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum LetStatementKind {
    PatternBinding {
        pattern: Pattern,
        value: Expr,
    },
    ConstructorAlias {
        alias: Option<lir::QualifiedName>,
        target: Option<lir::QualifiedName>,
    },
}

/// Type-annotated pattern node.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Pattern {
    pub kind: PatternKind,
    pub ty: TypeId,
    pub range: TextRange,
}

/// Structural variants of type-annotated patterns.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum PatternKind {
    Constructor {
        constructor: lir::QualifiedName,
        argument: Box<Pattern>,
    },
    ConstructorName {
        constructor: lir::QualifiedName,
    },
    Binding {
        name: lir::ResolvedName,
    },
    Hole,
    Annotated {
        pattern: Box<Pattern>,
        annotation: TypeId,
    },
    Literal(lir::Literal),
    Tuple {
        elements: Vec<Pattern>,
    },
    Array {
        elements: Vec<ArrayPatternElement>,
    },
    Record {
        fields: Vec<RecordPatternField>,
        open: bool,
    },
    Error(lir::ErrorNode),
}

/// Type-annotated array-pattern element.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ArrayPatternElement {
    Item(Pattern),
    Rest {
        binding: Option<lir::ResolvedName>,
        range: TextRange,
    },
}

/// Type-annotated record-pattern field.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct RecordPatternField {
    pub name: Option<String>,
    pub value: Option<Pattern>,
    pub range: TextRange,
}

/// Type-annotated expression node.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: TypeId,
    pub range: TextRange,
}

/// Structural variants of type-annotated expressions.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ExprKind {
    Let {
        pattern: Pattern,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Function {
        params: Vec<Pattern>,
        body: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Apply {
        callee: Box<Expr>,
        argument: Box<Expr>,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: Option<String>,
    },
    Name(lir::ResolvedName),
    Literal(lir::Literal),
    Unit,
    Tuple {
        elements: Vec<Expr>,
    },
    Array {
        elements: Vec<ArrayElement>,
    },
    Record {
        fields: Vec<RecordField>,
    },
    InlineWasm {
        result_type: TypeId,
        locals: Vec<lir::WasmTypedBinding>,
        instructions: Vec<lir::WasmInstructionNode>,
    },
    Error(lir::ErrorNode),
}

/// Type-annotated match arm.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub range: TextRange,
}

/// Type-annotated array expression element.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub enum ArrayElement {
    Item(Expr),
    Spread { expr: Expr, range: TextRange },
}

/// Type-annotated record field expression.
#[derive(Debug, Clone, PartialEq, salsa::Update)]
pub struct RecordField {
    pub name: Option<String>,
    pub separator: RecordFieldSeparator,
    pub value: Expr,
    pub range: TextRange,
}
