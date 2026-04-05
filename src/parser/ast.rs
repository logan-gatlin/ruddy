use crate::reporting::TextRange;

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct AstFile {
    /// `Some` if this is a bundle root
    pub bundle_name: Option<Identifier>,
    pub range: TextRange,
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct ErrorNode {
    pub range: TextRange,
}

impl ErrorNode {
    pub fn new(range: TextRange) -> Self {
        Self { range }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum IdentifierKind {
    Bare,
    Bracketed,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct Identifier {
    pub kind: IdentifierKind,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum PathRoot {
    Relative,
    Root,
    Bundle,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct Path {
    pub root: PathRoot,
    pub segments: Vec<Identifier>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum NameRef {
    Identifier(Identifier),
    Path(Path),
}

impl NameRef {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Identifier(identifier) => identifier.range,
            Self::Path(path) => path.range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum LiteralKind {
    Integer,
    Natural,
    Real,
    String,
    Glyph,
    FormatString,
    BoolTrue,
    BoolFalse,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct Literal {
    pub kind: LiteralKind,
    pub range: TextRange,
}

impl Literal {
    pub fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Statement {
    Bundle {
        name: Option<Identifier>,
        range: TextRange,
    },
    Module {
        name: Option<Identifier>,
        body: Vec<Statement>,
        range: TextRange,
    },
    ModuleRef {
        name: Option<Identifier>,
        in_loc: Option<Literal>,
        range: TextRange,
    },
    Let {
        kind: LetStatementKind,
        range: TextRange,
    },
    Do {
        expr: Expr,
        range: TextRange,
    },
    Use {
        target: Option<NameRef>,
        alias: Option<Identifier>,
        range: TextRange,
    },
    Type {
        name: Option<Identifier>,
        params: Vec<Identifier>,
        kind: TypeStatementKind,
        range: TextRange,
    },
    Trait {
        name: Option<Identifier>,
        params: Vec<Identifier>,
        items: Vec<TraitItem>,
        range: TextRange,
    },
    TraitAlias {
        name: Option<Identifier>,
        target: Option<NameRef>,
        range: TextRange,
    },
    Impl {
        trait_ref: Option<NameRef>,
        for_types: Vec<TypeExpr>,
        items: Vec<ImplItem>,
        range: TextRange,
    },
    Wasm {
        declarations: Vec<SExpr>,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl Statement {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Bundle { range, .. }
            | Self::Module { range, .. }
            | Self::ModuleRef { range, .. }
            | Self::Let { range, .. }
            | Self::Do { range, .. }
            | Self::Use { range, .. }
            | Self::Type { range, .. }
            | Self::Trait { range, .. }
            | Self::TraitAlias { range, .. }
            | Self::Impl { range, .. }
            | Self::Wasm { range, .. } => *range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum LetStatementKind {
    PatternBinding {
        pattern: Pattern,
        value: Expr,
    },
    ConstructorAlias {
        alias: Option<Identifier>,
        target: Option<NameRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum TypeStatementKind {
    Alias { value: TypeExpr },
    Nominal { definition: TypeDefinition },
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum TypeDefinition {
    Struct {
        members: Vec<RecordTypeMember>,
        range: TextRange,
    },
    Sum {
        variants: Vec<SumVariant>,
        range: TextRange,
    },
    Expr(TypeExpr),
}

impl TypeDefinition {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Struct { range, .. } | Self::Sum { range, .. } => *range,
            Self::Expr(expr) => expr.range(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum RecordTypeMember {
    Field {
        name: Option<Identifier>,
        ty: TypeExpr,
        range: TextRange,
    },
    Spread {
        ty: TypeExpr,
        range: TextRange,
    },
}

impl RecordTypeMember {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Field { range, .. } | Self::Spread { range, .. } => *range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct SumVariant {
    pub name: Option<Identifier>,
    pub argument: Option<TypeExpr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum TraitItem {
    Method {
        name: Option<Identifier>,
        ty: TypeExpr,
        range: TextRange,
    },
    Type {
        name: Option<Identifier>,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl TraitItem {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Method { range, .. } | Self::Type { range, .. } => *range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum ImplItem {
    Method {
        name: Option<Identifier>,
        value: Expr,
        range: TextRange,
    },
    Type {
        name: Option<Identifier>,
        value: TypeExpr,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl ImplItem {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Method { range, .. } | Self::Type { range, .. } => *range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum TypeExpr {
    Forall {
        params: Vec<Identifier>,
        body: Box<TypeExpr>,
        constraints: Vec<TraitConstraint>,
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
    Name(NameRef),
    Grouped {
        inner: Box<TypeExpr>,
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

impl TypeExpr {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Forall { range, .. }
            | Self::Function { range, .. }
            | Self::Apply { range, .. }
            | Self::Grouped { range, .. }
            | Self::Tuple { range, .. }
            | Self::Unit { range }
            | Self::Array { range } => *range,
            Self::Name(name) => name.range(),
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct TraitConstraint {
    pub trait_ref: Option<NameRef>,
    pub args: Vec<TypeExpr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Pattern {
    Constructor {
        constructor: NameRef,
        argument: Box<Pattern>,
        range: TextRange,
    },
    Annotated {
        pattern: Box<Pattern>,
        ty: TypeExpr,
        range: TextRange,
    },
    Name(NameRef),
    Literal(Literal),
    Grouped {
        inner: Box<Pattern>,
        range: TextRange,
    },
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

impl Pattern {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Constructor { range, .. }
            | Self::Annotated { range, .. }
            | Self::Grouped { range, .. }
            | Self::Tuple { range, .. }
            | Self::Array { range, .. }
            | Self::Record { range, .. } => *range,
            Self::Name(name) => name.range(),
            Self::Literal(literal) => literal.range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum ArrayPatternElement {
    Item(Pattern),
    Rest {
        binding: Option<Identifier>,
        range: TextRange,
    },
}

impl ArrayPatternElement {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Item(pattern) => pattern.range(),
            Self::Rest { range, .. } => *range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct RecordPatternField {
    pub name: Option<Identifier>,
    pub value: Option<Pattern>,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum BinaryOperator {
    Sequence,
    And,
    Or,
    PipeRight,
    PlusPipe,
    StarPipe,
    Xor,
    ShiftRight,
    ShiftLeft,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl std::fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Sequence => "[;]",
                Self::And => "[and]",
                Self::Or => "[or]",
                Self::PipeRight => "[|>]",
                Self::PlusPipe => "[+>]",
                Self::StarPipe => "[*>]",
                Self::Xor => "[xor]",
                Self::ShiftRight => "[>>]",
                Self::ShiftLeft => "[<<]",
                Self::Add => "[+]",
                Self::Subtract => "[-]",
                Self::Multiply => "[*]",
                Self::Divide => "[/]",
                Self::Modulo => "[mod]",
                Self::Equal => "[==]",
                Self::NotEqual => "[!=]",
                Self::Less => "[<]",
                Self::LessEqual => "[<=]",
                Self::Greater => "[>]",
                Self::GreaterEqual => "[>=]",
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum UnaryOperator {
    Not,
    Negate,
}

impl std::fmt::Display for UnaryOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Not => "[not]",
                Self::Negate => "[~]",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Expr {
    Let {
        pattern: Pattern,
        value: Box<Expr>,
        body: Box<Expr>,
        range: TextRange,
    },
    Use {
        target: Option<NameRef>,
        alias: Option<Identifier>,
        body: Box<Expr>,
        range: TextRange,
    },
    Function {
        params: Vec<Parameter>,
        body: FunctionBody,
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
    Binary {
        op: BinaryOperator,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        range: TextRange,
    },
    Unary {
        op: UnaryOperator,
        expr: Box<Expr>,
        range: TextRange,
    },
    Apply {
        callee: Box<Expr>,
        argument: Box<Expr>,
        range: TextRange,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: Option<Identifier>,
        range: TextRange,
    },
    Name(NameRef),
    Literal(Literal),
    Grouped {
        inner: Box<Expr>,
        range: TextRange,
    },
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
        result_type: TypeExpr,
        body: Option<SExpr>,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl Expr {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Let { range, .. }
            | Self::Use { range, .. }
            | Self::Function { range, .. }
            | Self::If { range, .. }
            | Self::Match { range, .. }
            | Self::Binary { range, .. }
            | Self::Unary { range, .. }
            | Self::Apply { range, .. }
            | Self::FieldAccess { range, .. }
            | Self::Grouped { range, .. }
            | Self::Unit { range }
            | Self::Tuple { range, .. }
            | Self::Array { range, .. }
            | Self::Record { range, .. }
            | Self::InlineWasm { range, .. } => *range,
            Self::Name(name) => name.range(),
            Self::Literal(literal) => literal.range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum FunctionBody {
    Expr(Box<Expr>),
    MatchArms(Vec<MatchArm>),
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum Parameter {
    Named(Identifier),
    Typed {
        name: Option<Identifier>,
        ty: TypeExpr,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl Parameter {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Named(identifier) => identifier.range,
            Self::Typed { range, .. } => *range,
            Self::Error(error) => error.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum ArrayElement {
    Item(Expr),
    Spread { expr: Expr, range: TextRange },
}

impl ArrayElement {
    pub fn range(&self) -> TextRange {
        match self {
            Self::Item(expr) => expr.range(),
            Self::Spread { range, .. } => *range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum RecordFieldSeparator {
    Equals,
    Colon,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct RecordField {
    pub name: Option<Identifier>,
    pub separator: RecordFieldSeparator,
    pub value: Expr,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum SExprAtomKind {
    Path,
    Ident,
    String,
    Integer,
    Natural,
    Real,
    BoolTrue,
    BoolFalse,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum SExpr {
    List {
        items: Vec<SExpr>,
        range: TextRange,
    },
    Atom {
        kind: SExprAtomKind,
        range: TextRange,
    },
    Error(ErrorNode),
}

impl SExpr {
    pub fn range(&self) -> TextRange {
        match self {
            Self::List { range, .. } | Self::Atom { range, .. } => *range,
            Self::Error(error) => error.range,
        }
    }
}

pub struct AstVisitor<'a> {
    visit_statement: Box<dyn FnMut(&Statement) + 'a>,
    visit_expr: Box<dyn FnMut(&Expr) + 'a>,
    visit_pattern: Box<dyn FnMut(&Pattern) + 'a>,
    visit_type_expr: Box<dyn FnMut(&TypeExpr) + 'a>,
    visit_type_def: Box<dyn FnMut(&TypeDefinition) + 'a>,
    visit_sexpr: Box<dyn FnMut(&SExpr) + 'a>,
    enter_module: Box<dyn FnMut(&Identifier) + 'a>,
    leave_module: Box<dyn FnMut() + 'a>,
}

impl Default for AstVisitor<'_> {
    fn default() -> Self {
        Self {
            visit_statement: Box::new(|_| {}),
            visit_expr: Box::new(|_| {}),
            visit_pattern: Box::new(|_| {}),
            visit_type_expr: Box::new(|_| {}),
            visit_type_def: Box::new(|_| {}),
            visit_sexpr: Box::new(|_| {}),
            enter_module: Box::new(|_| {}),
            leave_module: Box::new(|| {}),
        }
    }
}

impl<'a> AstVisitor<'a> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn statement(mut self, stmt: impl FnMut(&Statement) + 'a) -> Self {
        self.visit_statement = Box::new(stmt);
        self
    }
    pub fn expr(mut self, expr: impl FnMut(&Expr) + 'a) -> Self {
        self.visit_expr = Box::new(expr);
        self
    }
    pub fn pattern(mut self, pattern: impl FnMut(&Pattern) + 'a) -> Self {
        self.visit_pattern = Box::new(pattern);
        self
    }
    pub fn type_expr(mut self, type_expr: impl FnMut(&TypeExpr) + 'a) -> Self {
        self.visit_type_expr = Box::new(type_expr);
        self
    }
    pub fn type_def(mut self, type_def: impl FnMut(&TypeDefinition) + 'a) -> Self {
        self.visit_type_def = Box::new(type_def);
        self
    }
    pub fn sexpr(mut self, sexpr: impl FnMut(&SExpr) + 'a) -> Self {
        self.visit_sexpr = Box::new(sexpr);
        self
    }
    pub fn enter_module(mut self, enter: impl FnMut(&Identifier) + 'a) -> Self {
        self.enter_module = Box::new(enter);
        self
    }
    pub fn leave_module(mut self, leave: impl FnMut() + 'a) -> Self {
        self.leave_module = Box::new(leave);
        self
    }
}

impl AstFile {
    pub fn walk(&self, visitor: &mut AstVisitor) {
        for statement in &self.statements {
            walk_statement(statement, visitor);
        }
    }
}

pub fn walk_statement(statement: &Statement, visitor: &mut AstVisitor) {
    (visitor.visit_statement)(statement);

    match statement {
        Statement::Bundle { .. } | Statement::TraitAlias { .. } | Statement::ModuleRef { .. } => {}
        Statement::Module { name, body, .. } => {
            if let Some(name) = name {
                (visitor.enter_module)(name);
            }
            for nested in body {
                walk_statement(nested, visitor);
            }
            if name.is_some() {
                (visitor.leave_module)();
            }
        }
        Statement::Let { kind, .. } => match kind {
            LetStatementKind::PatternBinding { pattern, value } => {
                walk_pattern(pattern, visitor);
                walk_expr(value, visitor);
            }
            LetStatementKind::ConstructorAlias { .. } => {}
        },
        Statement::Do { expr, .. } => walk_expr(expr, visitor),
        Statement::Use { .. } => {}
        Statement::Type { kind, .. } => match kind {
            TypeStatementKind::Alias { value } => walk_type_expr(value, visitor),
            TypeStatementKind::Nominal { definition } => walk_type_definition(definition, visitor),
        },
        Statement::Trait { items, .. } => {
            for item in items {
                match item {
                    TraitItem::Method { ty, .. } => walk_type_expr(ty, visitor),
                    TraitItem::Type { .. } | TraitItem::Error(_) => {}
                }
            }
        }
        Statement::Impl {
            for_types, items, ..
        } => {
            for type_expr in for_types {
                walk_type_expr(type_expr, visitor);
            }

            for item in items {
                match item {
                    ImplItem::Method { value, .. } => walk_expr(value, visitor),
                    ImplItem::Type { value, .. } => walk_type_expr(value, visitor),
                    ImplItem::Error(_) => {}
                }
            }
        }
        Statement::Wasm { declarations, .. } => {
            for declaration in declarations {
                walk_sexpr(declaration, visitor);
            }
        }
        Statement::Error(_) => {}
    }
}

pub fn walk_pattern(pattern: &Pattern, visitor: &mut AstVisitor) {
    (visitor.visit_pattern)(pattern);

    match pattern {
        Pattern::Constructor { argument, .. }
        | Pattern::Grouped {
            inner: argument, ..
        } => walk_pattern(argument, visitor),
        Pattern::Annotated { pattern, ty, .. } => {
            walk_pattern(pattern, visitor);
            walk_type_expr(ty, visitor);
        }
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                walk_pattern(element, visitor);
            }
        }
        Pattern::Array { elements, .. } => {
            for element in elements {
                if let ArrayPatternElement::Item(item) = element {
                    walk_pattern(item, visitor);
                }
            }
        }
        Pattern::Record { fields, .. } => {
            for field in fields {
                if let Some(value) = &field.value {
                    walk_pattern(value, visitor);
                }
            }
        }
        Pattern::Name(_) | Pattern::Literal(_) | Pattern::Error(_) => {}
    }
}

pub fn walk_type_expr(type_expr: &TypeExpr, visitor: &mut AstVisitor) {
    (visitor.visit_type_expr)(type_expr);

    match type_expr {
        TypeExpr::Forall {
            body, constraints, ..
        } => {
            walk_type_expr(body, visitor);
            for constraint in constraints {
                for arg in &constraint.args {
                    walk_type_expr(arg, visitor);
                }
            }
        }
        TypeExpr::Function { param, result, .. } => {
            walk_type_expr(param, visitor);
            walk_type_expr(result, visitor);
        }
        TypeExpr::Apply {
            callee, argument, ..
        } => {
            walk_type_expr(callee, visitor);
            walk_type_expr(argument, visitor);
        }
        TypeExpr::Grouped { inner, .. } => walk_type_expr(inner, visitor),
        TypeExpr::Tuple { elements, .. } => {
            for element in elements {
                walk_type_expr(element, visitor);
            }
        }
        TypeExpr::Name(_) | TypeExpr::Unit { .. } | TypeExpr::Array { .. } | TypeExpr::Error(_) => {
        }
    }
}

pub fn walk_expr(expr: &Expr, visitor: &mut AstVisitor) {
    (visitor.visit_expr)(expr);

    match expr {
        Expr::Let {
            pattern,
            value,
            body,
            ..
        } => {
            walk_pattern(pattern, visitor);
            walk_expr(value, visitor);
            walk_expr(body, visitor);
        }
        Expr::Use { body, .. } => walk_expr(body, visitor),
        Expr::Function { params, body, .. } => {
            for param in params {
                if let Parameter::Typed { ty, .. } = param {
                    walk_type_expr(ty, visitor);
                }
            }

            match body {
                FunctionBody::Expr(body) => walk_expr(body, visitor),
                FunctionBody::MatchArms(arms) => {
                    for arm in arms {
                        walk_pattern(&arm.pattern, visitor);
                        walk_expr(&arm.body, visitor);
                    }
                }
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            walk_expr(condition, visitor);
            walk_expr(then_branch, visitor);
            walk_expr(else_branch, visitor);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, visitor);
            for arm in arms {
                walk_pattern(&arm.pattern, visitor);
                walk_expr(&arm.body, visitor);
            }
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Apply {
            callee: lhs,
            argument: rhs,
            ..
        } => {
            walk_expr(lhs, visitor);
            walk_expr(rhs, visitor);
        }
        Expr::Unary { expr, .. } | Expr::Grouped { inner: expr, .. } => walk_expr(expr, visitor),
        Expr::FieldAccess { expr, .. } => walk_expr(expr, visitor),
        Expr::Tuple { elements, .. } => {
            for element in elements {
                walk_expr(element, visitor);
            }
        }
        Expr::Array { elements, .. } => {
            for element in elements {
                match element {
                    ArrayElement::Item(item) => walk_expr(item, visitor),
                    ArrayElement::Spread { expr, .. } => walk_expr(expr, visitor),
                }
            }
        }
        Expr::Record { fields, .. } => {
            for field in fields {
                walk_expr(&field.value, visitor);
            }
        }
        Expr::InlineWasm {
            result_type, body, ..
        } => {
            walk_type_expr(result_type, visitor);
            if let Some(body) = body {
                walk_sexpr(body, visitor);
            }
        }
        Expr::Name(_) | Expr::Literal(_) | Expr::Unit { .. } | Expr::Error(_) => {}
    }
}

pub fn walk_sexpr(sexpr: &SExpr, visitor: &mut AstVisitor) {
    (visitor.visit_sexpr)(sexpr);

    if let SExpr::List { items, .. } = sexpr {
        for item in items {
            walk_sexpr(item, visitor);
        }
    }
}

fn walk_type_definition(definition: &TypeDefinition, visitor: &mut AstVisitor) {
    (visitor.visit_type_def)(definition);
    match definition {
        TypeDefinition::Expr(expr) => walk_type_expr(expr, visitor),
        TypeDefinition::Struct { members, .. } => {
            for member in members {
                match member {
                    RecordTypeMember::Field { ty, .. } | RecordTypeMember::Spread { ty, .. } => {
                        walk_type_expr(ty, visitor)
                    }
                }
            }
        }
        TypeDefinition::Sum { variants, .. } => {
            for variant in variants {
                if let Some(argument) = &variant.argument {
                    walk_type_expr(argument, visitor);
                }
            }
        }
    }
}
