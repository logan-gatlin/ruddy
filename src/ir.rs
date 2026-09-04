use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    artifact,
    parse::{self, Expr, ExprKind, Stmt, StmtKind},
    symbol::{Mint, Module, Namespace, Symbol},
    tracking::{Span, Tracked, TrackedString},
    types::{EffectId, ParamKind, Presence, Prim, Rest, Scheme, Sense, Shape, Ty},
};

#[derive(Debug, Clone)]
pub struct Program {
    /// Target-provided values. They bind in the term namespace but have no
    /// initializer, so recursive initializer grouping never sees them.
    pub externs: IndexMap<Symbol, Decl<Extern>>,
    pub terms: IndexMap<Symbol, Decl<Term>>,
    pub types: IndexMap<Symbol, Decl<Type>>,
    /// Dependency declarations are not definitions of this bundle, but their
    /// semantic interfaces participate in checking exactly as in-bundle
    /// declarations do.
    pub external_names: IndexMap<Symbol, artifact::QualifiedName>,
    pub external_schemes: IndexMap<Symbol, Scheme>,
    pub external_types: IndexMap<Symbol, ExternalType>,
    pub external_operations: IndexMap<(Symbol, OperationSelector), (Rc<Ty>, Rc<Ty>)>,
    /// What each effect's parameters stand for, local and imported alike, in
    /// the order they are applied. An operation reference or a handler mints
    /// one fresh argument per entry; an effect declared without parameters
    /// has an empty list.
    pub effect_params: IndexMap<Symbol, Vec<ParamKind>>,
    /// The effects declared, in the order they were written, each with the
    /// operations it declares or the effects it stands for.
    pub effects: IndexMap<Symbol, Decl<Effect>>,
    /// The structural identity of each operation-declaring effect. Source
    /// symbols remain on labels for resolution and navigation; this map is
    /// what gives rows their module-independent semantic identity.
    pub effect_ids: IndexMap<Symbol, EffectId>,
    /// The definitions split into the smallest sets that have to be typed
    /// together, earliest first. See [`Group`] and [`grouping`].
    pub groups: Vec<Group>,
}

/// What one `effect` declaration says: the operations it declares, or the
/// effects it stands for.
///
/// The two forms of R3 and R4, told apart by whether the cases carry a `:` and
/// kept apart from there on: an alias declares no operations, so
/// `!Console.write` has nothing to resolve to, and an operation declaration
/// stands for itself alone.
#[derive(Debug, Clone)]
pub enum Effect {
    /// The operations the effect declares, in the order written.
    Operations(IndexMap<OperationSelector, Operation>),
    /// A way of writing a row rather than an effect of its own. See [`Alias`].
    Alias(Alias),
}

/// What an alias stands for, as written and as it comes to.
///
/// An alias is a checked way of writing a row: it applies effects to its own
/// parameters and may end in one of them, and every row that names it gets
/// the effects it names with the arguments substituted. Nothing is performed
/// through one and no row is keyed by one.
#[derive(Debug, Clone)]
pub struct Alias {
    /// The row as the declaration wrote it: applications, alias or concrete,
    /// and at most one tail naming a parameter.
    pub body: AliasBody,
    /// The row it comes to with its own parameters as the arguments: every
    /// alias it names expanded, so every label is a concrete effect. What the
    /// parameter fixpoint and the argument checks read, since it says of the
    /// parameters exactly what a use of the alias says of the arguments.
    /// Always a [`TypeKind::Effects`].
    pub expanded: Type,
}

/// An alias body: the applications it names and the tail it ends in.
#[derive(Debug, Clone, Default)]
pub struct AliasBody {
    pub span: Span,
    pub cases: Vec<AliasCase>,
    pub tail: Option<Tail>,
}

/// One application an alias body names: `!Ask 'a`, or `Sys::!Log`.
#[derive(Debug, Clone)]
pub struct AliasCase {
    pub name_span: Span,
    pub symbol: Symbol,
    pub args: Vec<Type>,
}

/// One operation of an effect: the plain closed arrow performing it has.
///
/// The two sides rather than one arrow node, because that is what the arm rule
/// of R16 wants: a handler arm for `op : A -> B` binds its binder at `A`
/// and has body type `B`, and neither half is ever read as a whole arrow.
#[derive(Debug, Clone)]
pub struct Operation {
    /// Where the selector (or the unnamed signature) was written.
    pub name_span: Span,
    pub from: Type,
    pub to: Type,
}

/// An operation's source-visible selector. Empty and named interfaces remain
/// distinct from unnamed singleton effects throughout the compiler.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OperationSelector {
    Unnamed,
    Named(String),
}

impl std::fmt::Display for OperationSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unnamed => Ok(()),
            Self::Named(name) => write!(f, ".{name}"),
        }
    }
}

impl OperationSelector {
    pub fn source_name(&self) -> String {
        match self {
            Self::Unnamed => "<unnamed>".to_string(),
            Self::Named(name) => name.clone(),
        }
    }

    fn canonical(&self) -> String {
        match self {
            Self::Unnamed => "u".to_string(),
            Self::Named(name) => format!("n{}:{name}", name.len()),
        }
    }
}

/// One effect named by another: the symbol it resolved to, and where it was
/// written. What an alias's cases are, and what the labels of an effect row
/// resolve through.
#[derive(Debug, Clone)]
pub struct Named {
    pub name_span: Span,
    pub symbol: Symbol,
}

/// A set of definitions that all have to be typed at once, because each of them
/// is reachable from every other through the names their values mention.
///
/// This is what buys recursion without costing let-polymorphism. Every
/// definition in one group is monomorphic to every other while the group is
/// being solved — that is what lets a definition name itself — and each is
/// generalized once the group is done, so a definition that shares its group
/// with nothing keeps exactly the scheme it would have had if nothing here
/// existed. Treating the whole file as one group would type-check every
/// recursion and destroy the polymorphism of everything else.
/// The imported semantic interface of a declared type.
#[derive(Debug, Clone)]
pub struct ExternalType {
    pub params: Vec<ParamKind>,
    pub relevant: Vec<bool>,
    pub scheme: Scheme,
    /// Qualified identity retained when a direct-only interface references a
    /// type whose transitive declaration was not supplied.
    pub unresolved: Option<artifact::QualifiedName>,
}

#[derive(Debug, Clone)]
pub struct Group {
    /// The definitions in this group, in source order.
    pub members: Vec<Symbol>,
    /// Whether anything in the group refers back into it.
    ///
    /// Every group of two or more is one, since being a group at all is being
    /// mutually reachable. A group of one is only recursive when the definition
    /// names itself, and nothing about the members can say which — hence the
    /// flag, rather than a reader of this working it out again from a graph
    /// this is the published form of.
    pub recursive: bool,
}

/// A top-level definition. The symbol is the map key rather than part of the
/// value, so only the span the name was written at is field_summary here — the same
/// split [`Field`] uses.
#[derive(Debug, Clone)]
pub struct Decl<T> {
    pub name_span: Span,
    /// The written type the definition is to be checked against, when it was
    /// ascribed one. Always `None` for a `type` declaration: that *is* a type,
    /// so there is nothing to check it against.
    pub annotation: Option<Annotation>,
    /// The parameters a `type` declaration binds, in order. Always empty for a
    /// term, which binds none of its own — a lambda's argument is bound inside
    /// its body rather than by the definition.
    pub params: Vec<Param>,
    pub value: T,
}

/// One parameter of a `type` declaration.
#[derive(Debug, Clone)]
pub struct Param {
    /// Where the name was written, so a repeat can point at what it repeats.
    pub span: Span,
    pub symbol: Symbol,
    /// What it stands for. Not known while the body is being lowered — it
    /// follows from how the body uses it — so it is [`ParamKind::Type`] until
    /// the kinds are worked out, once every body is in.
    pub kind: ParamKind,
    /// Whether the argument written here survives unfolding: whether it reaches
    /// a position of what the declaration stands for, rather than being handed
    /// to something that throws it away.
    ///
    /// `'a` in `type Box 'a = { it: 'a }` does, and `'a` in `type Ptr 'a = Nat`
    /// does not. The difference is the whole of what
    /// [`Rule::Congruent`](crate::inference::Rule) may be taken on: comparing
    /// two applications argument by argument agrees with comparing what they
    /// stand for exactly when every argument reaches the body, so a
    /// declaration with an argument that does not is compared by unfolding
    /// like any other type. See [`relevance`] for how it is worked out and
    /// [`Ty::Named`] for what rests on it.
    ///
    /// Not known while the body is being lowered either — it follows from what
    /// every *other* declaration does with what it is handed — so it is `false`
    /// until the fixpoint has run, which is the reading that decides nothing.
    pub relevant: bool,
}

/// A target-provided global value. The decoded target source is retained
/// unchanged; target-independent lowering does not interpret it.
#[derive(Debug, Clone)]
pub struct Extern {
    pub target: TrackedString,
    /// This declaration matched one exact compiler-reserved array intrinsic
    /// signature during lowering. Source-controlled names and targets alone
    /// are not authority to bypass ordinary host-boundary checks.
    pub array_intrinsic: bool,
    /// The resolved foreign boundary spelling. This is kept separately from
    /// the annotation's ordinary curried type so lowering can distinguish one
    /// n-ary host call from a chain of Ruddy calls without resolving names (or
    /// annotation variables) a second time.
    pub abi: ExternType,
}

/// A resolved extern boundary type. Every leaf contains the corresponding
/// part of the already-resolved annotation, while marked functions retain
/// their host arity and callback nesting.
pub type ExternType = Tracked<ExternTypeKind>;

#[derive(Debug, Clone)]
pub enum ExternTypeKind {
    Ordinary(Type),
    Function {
        parameters: Vec<ExternType>,
        result: Box<ExternType>,
        /// Effects on the final arrow of the desugared marked function.
        effects: Box<EffectRow>,
    },
    Group(Box<ExternType>),
}

/// Attach resolved annotation nodes to the parser's ABI skeleton. The parser
/// constructs the ordinary annotation and ABI tree together, so a marked
/// function corresponds to one arrow per parameter (or one unit arrow when it
/// is nullary). Invalid annotation pieces are deliberately retained as error
/// leaves rather than making ABI retention depend on successful resolution.
fn resolved_extern_type(written: parse::ExternType, resolved: &Type) -> ExternType {
    let span = written.span;
    match written.tracked {
        parse::ExternTypeKind::Ordinary(_) => {
            span.track(ExternTypeKind::Ordinary(resolved.clone()))
        }
        parse::ExternTypeKind::Group(inner) => span.track(ExternTypeKind::Group(Box::new(
            resolved_extern_type(*inner, resolved),
        ))),
        parse::ExternTypeKind::Function {
            parameters, result, ..
        } => {
            let mut cursor = resolved.clone();
            let mut resolved_parameters = Vec::with_capacity(parameters.len());
            let arrow_count = parameters.len().max(1);
            let mut effects = Box::new(EffectRow::default());
            for at in 0..arrow_count {
                match cursor.tracked.clone() {
                    TypeKind::Arrow {
                        from,
                        to,
                        effects: arrow_effects,
                    } => {
                        if at < parameters.len() {
                            resolved_parameters.push(*from);
                        }
                        cursor = *to;
                        if at + 1 == arrow_count {
                            effects = arrow_effects;
                        }
                    }
                    _ => {
                        // Resolution can absorb a bad arrow into Error. Keep
                        // the ABI tree total so diagnostics remain the only
                        // observable consequence of the malformed annotation.
                        if at < parameters.len() {
                            resolved_parameters.push(span.track(TypeKind::Error));
                        }
                        cursor = span.track(TypeKind::Error);
                    }
                }
            }
            let parameters = parameters
                .into_iter()
                .zip(resolved_parameters.iter())
                .map(|(parameter, resolved)| resolved_extern_type(parameter, resolved))
                .collect();
            let result = Box::new(resolved_extern_type(*result, &cursor));
            span.track(ExternTypeKind::Function {
                parameters,
                result,
                effects,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct Term {
    /// What the term was inferred to be. Lowering runs before inference, so
    /// until then this is [`Ty::default`], the undecided type — see
    /// [`TermKind::with_span`].
    pub ty: Rc<Ty>,
    pub span: Span,
    pub kind: TermKind,
}

#[derive(Debug, Clone)]
pub enum TermKind {
    Unary {
        op: UnaryOp,
        value: Box<Term>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Term>,
        right: Box<Term>,
    },
    Apply {
        func: Box<Term>,
        arg: Box<Term>,
    },
    Fn {
        arg: Tracked<Symbol>,
        body: Box<Term>,
    },
    /// A name given a value for the length of a body.
    ///
    /// Kept as a node of its own rather than desugared into a lambda applied
    /// to the value: an application is monomorphic, and a nested binding gets a
    /// scheme — see [`inference`](crate::inference) — so the two are not the
    /// same term. It is also what the reader wrote, which is what the IR is for.
    Let {
        /// The name this binds, and where it was written.
        name: Tracked<Symbol>,
        /// The written type, lowered, when the binding was ascribed one.
        ///
        /// Boxed because an annotation is the largest thing a term can carry
        /// and the rarest, so inlining one would grow every node of every
        /// tree to the size of the few that have one.
        annotation: Option<Box<Annotation>>,
        value: Box<Term>,
        body: Box<Term>,
    },
    /// A struct literal: the fields it names, and — with the `..` — one more
    /// value every field of which it takes on, less the ones it names itself.
    /// The spread stays a node of its own rather than being expanded: which
    /// fields it brings is inference's to find out, and the tree is what was
    /// written.
    Struct {
        fields: IndexMap<String, Field<Term>>,
        spread: Option<Spread>,
    },
    /// An immutable homogeneous array literal, item by item: a value of its
    /// own, or — with the `..` — an array spread into the literal in place.
    Array(Vec<ArrayItem>),
    /// `#Some 1` — one case of a sum, with what it carries.
    ///
    /// The name stays a string for the reason [`Field`]'s keys do: it is a
    /// label scoped to whichever sum turns out to be on the other side, not a
    /// path anything can refer to, so there is no symbol to resolve it to and
    /// nothing here can fail to resolve.
    ///
    /// A case field_summaries nothing keeps its `None` rather than being handed a
    /// `{}` here. `()` and `{}` are two spellings of one *written* type and so
    /// meet in this pass; `#None` writes no type at all, and inventing one
    /// would put a node on the page the reader never wrote. What it means is
    /// unit all the same, which [`inference`](crate::inference) says where it
    /// builds the type rather than the tree.
    Tag {
        name: TrackedString,
        payload: Option<Box<Term>>,
    },
    /// Reading one field out of a struct. The name stays a string for the
    /// reason [`Field`]'s keys do: it is a label scoped to whichever struct
    /// turns out to be on the left, not a path anything can refer to, so there
    /// is no symbol to resolve it to and nothing here can fail to resolve.
    Project {
        base: Box<Term>,
        field: TrackedString,
    },
    /// Dispatch on what a value is: the written match, one arm per written
    /// arm, each field_summaries its pattern normalized — names resolved, puns
    /// expanded, grouping gone — and its body exactly once.
    ///
    /// The matrix of arms carries the whole meaning: first-match, top to
    /// bottom. Nothing here says how a machine would dispatch them — typing is
    /// defined on the written match, and compiling it to a decision tree is a
    /// post-typing phase for a future evaluator. Every written arm is here:
    /// which arms a value can reach, and whether any value goes unhandled, are
    /// questions about solved types, asked by [`patterns`](crate::patterns)
    /// after inference rather than here.
    Match {
        scrutinee: Box<Term>,
        arms: Vec<(Pattern, Term)>,
    },
    /// `handle <expr> with <arms> end` — the effects the arms fully cover,
    /// discharged.
    ///
    /// Which effects those are is settled here, before inference: R15 holds a
    /// handler to covering every operation of every effect it names, so the set
    /// is a fact about the written arms rather than something the solver works
    /// out. See [`Handler`].
    Handle {
        body: Box<Term>,
        handler: Handler,
    },
    /// `raise <expr>` — abort to the handler around this arm.
    ///
    /// That an arm encloses it, with no `fn` in between, is checked here rather
    /// than typed: `raise` is bound to one particular handler, so a closure
    /// field_summaries one could outlive the `handle` and be called with nothing on
    /// the stack. See [`ErrorKind::RaiseInFunction`].
    Raise(Box<Term>),
    /// `!Log.write` — one operation of an effect, as an ordinary value.
    ///
    /// The operation's name stays a string for the reason [`Field`]'s keys do:
    /// it is scoped to the declaration that declares it, and two effects may
    /// each declare a `write`. What it resolved to is the effect beside
    /// it, and lowering has already checked that the name is one of that
    /// effect's.
    Operation {
        effect: Tracked<Symbol>,
        selector: Tracked<OperationSelector>,
    },
    Ident(Symbol),
    /// Numeric literals carry no symbol: a literal names nothing, so there is
    /// nothing for the mint to hand out.
    Natural(u64),
    Integer(i64),
    Real(f64),
    String(String),
    Boolean(bool),
    /// A name that did not resolve. Lowering stays total so that one typo
    /// produces one error rather than a cascade from a dropped definition.
    Error,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
}

/// The arms of one `handle`, and the effects they discharge between them.
///
/// `discharges` is what R15's coverage check bought: every effect an arm names
/// is fully covered, so the set of effects the body may perform beyond its
/// context is known before a single constraint is generated.
#[derive(Debug, Clone)]
pub struct Handler {
    /// The operation arms, in the order they were written.
    pub arms: Vec<HandlerArm>,
    /// The `return` arm, when one was written. At most one, in any position;
    /// where it was written is not kept, because nothing depends on it.
    pub ret: Option<ReturnArm>,
    /// The effects fully covered, in the order the arms first name them.
    pub discharges: Vec<Tracked<Symbol>>,
}

/// One operation arm: the operation it answers, the name it binds the payload
/// to, and the expression it gives back.
///
/// The arm's value *is* the operation's result — an arm resumes with it — so
/// both halves of its type come off the declaration and `Ans` appears in
/// neither.
#[derive(Debug, Clone)]
pub struct HandlerArm {
    pub effect: Tracked<Symbol>,
    pub selector: Tracked<OperationSelector>,
    /// The binder, as a symbol. A `_` gets a fresh one nothing can name, the
    /// way a `fn` header's wildcard does.
    pub binder: Tracked<Symbol>,
    pub body: Term,
}

/// The `return` arm: the name it binds the handled expression's value to, and
/// the answer it gives back.
#[derive(Debug, Clone)]
pub struct ReturnArm {
    /// Where the `return` was written, so a second one can point at the first.
    pub span: Span,
    pub binder: Tracked<Symbol>,
    /// Boxed where an operation arm's is not: those sit in a `Vec`, which is
    /// already a step away, and there is at most one of these.
    pub body: Box<Term>,
}

/// A surface pattern, normalized: every binder a resolved [`Symbol`], puns
/// expanded, grouping parentheses gone. What a [`TermKind::Match`] arm keeps
/// of what the reader wrote — the structure survives, only the surface
/// conveniences are erased.
pub type Pattern = Tracked<PatternKind>;

// spans field_summary per node as the IR's other types do
/// One scalar value that can be written both as an expression and a pattern.
/// Real equality is by representation so the compiler and eventual backends
/// agree even for signed zero.
#[derive(Debug, Clone)]
pub enum Literal {
    Natural(u64),
    Integer(i64),
    Real(f64),
    String(String),
    Boolean(bool),
}

impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Natural(a), Self::Natural(b)) => a == b,
            (Self::Integer(a), Self::Integer(b)) => a == b,
            (Self::Real(a), Self::Real(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Boolean(a), Self::Boolean(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for Literal {}

impl Hash for Literal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Natural(value) => value.hash(state),
            Self::Integer(value) => value.hash(state),
            // Equality compares a real's representation, including the sign
            // of zero, so hashing must do the same.
            Self::Real(value) => value.to_bits().hash(state),
            Self::String(value) => value.hash(state),
            Self::Boolean(value) => value.hash(state),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PatternKind {
    /// An identifier: binds the whole value at this position.
    Bind(Tracked<Symbol>),
    /// `_`: accepts the whole value at this position and binds nothing. A
    /// variant of its own rather than a fresh [`Bind`](PatternKind::Bind), so
    /// the tree still says what was written: printing gives back the `_`, the
    /// debugger shows one, and no invented name has to be kept out of every
    /// duplicate check by hand — there is none.
    Wildcard,
    /// Field name → sub-pattern. Puns are expanded here: surface `{x}`
    /// arrives as field "x" → Bind(x's symbol).
    Struct {
        fields: IndexMap<String, Field<Pattern>>,
        /// The `..` the pattern was written with, when it was written open.
        /// Without it the pattern is exact — it matches only values with
        /// exactly the fields it names — which is what the column rule in
        /// inference and the typed checks in [`patterns`](crate::patterns)
        /// both read off this marker.
        rest: Option<Span>,
    },
    /// Payload `None` means "written bare": constrains the payload to
    /// unit, binding nothing — the same convention TermKind::Tag keeps.
    Tag {
        name: TrackedString,
        payload: Option<Box<Pattern>>,
    },
    Natural(u64),
    Integer(i64),
    Real(f64),
    String(String),
    Boolean(bool),
    Unit,
    /// `[a, ..rest, b]`: the elements before the rest counted from the front,
    /// the ones after it from the back, and the rest — when written — the
    /// array of everything between. Without a rest the pattern is exact: it
    /// matches arrays of exactly as many elements as it names.
    Array {
        before: Vec<Pattern>,
        rest: Option<ArrayRest>,
        after: Vec<Pattern>,
    },
}

pub type Type = Tracked<TypeKind>;

#[derive(Debug, Clone)]
pub enum TypeKind {
    /// The type of an immutable homogeneous array.
    Array(Box<Type>),
    Struct {
        fields: IndexMap<String, TypeField>,
        /// The `..` tail, when the struct type was written open. Inside a
        /// `type` declaration this is `Some` only for a tail naming a row
        /// parameter — see [`ErrorKind::OpenDeclaredType`].
        tail: Option<Tail>,
    },
    /// `#Some T | #None` — a sum type, as the cases it allows.
    ///
    /// The struct's twin down to the [`Tail`]: what a `..` stands for, and
    /// what it may be inside a declaration, is the same question about cases
    /// that it is about fields.
    Sum {
        cases: IndexMap<String, SumCase>,
        tail: Option<Tail>,
    },
    Arrow {
        from: Box<Type>,
        to: Box<Type>,
        /// The effects calling it may perform, with every alias already
        /// expanded to the effects it names. Empty and closed for a bare
        /// `A -> B`, which is pure.
        effects: Box<EffectRow>,
    },
    Ident(Symbol),
    /// A declared type applied to arguments.
    ///
    /// The head is a symbol rather than a type: lowering is where "only a
    /// declared type may be applied" is said, so by the time one of these
    /// exists the head has already been one. The spine arrives flat from the
    /// parser and stays flat, because a declaration is applied to everything it
    /// takes at once.
    Apply {
        head: Symbol,
        /// Where the head was written, for the complaints and the rows that are
        /// about the name alone: the growth [`grows`] finds is a property of the
        /// declaration being mentioned, and the debugger gives the head a row of
        /// its own to cross-highlight against what it names.
        ///
        /// An arity complaint is deliberately *not* one of those. A wrong count
        /// is about the whole application — counting the arguments is the thing
        /// the reader has to do, and underlining four characters of a name says
        /// nothing about how many follow it. See [`Builder::apply`].
        head_span: Span,
        args: Vec<Type>,
    },
    /// A parameter of the declaration this type is the body of.
    ///
    /// Both the symbol and the position, because the two readers want
    /// different things: the debugger names it and cross-highlights it, and
    /// inference substitutes for it by position — which is [`Ty::Bound`]
    /// exactly, so lowering one is a rename rather than a translation.
    Param {
        symbol: Symbol,
        index: u32,
    },
    Prim(Prim),
    /// `!Log + !IO` — a row of effects handed to a declaration as an argument,
    /// resolved and with every alias expanded, exactly as an arrow's own row is.
    ///
    /// The one position a row reaches without an arrow around it. Everywhere
    /// else it is [`ErrorKind::EffectsOutsideRow`], so a reader of the tree can
    /// take a row here as being spliced into whatever the parameter is used as.
    Effects(Box<EffectRow>),
    /// A variable the annotation's an annotation introduced, used in a type
    /// position.
    ///
    /// The name stays a string for the reason [`Row::Named`] does: it is scoped
    /// to the one annotation that declares it, so nothing outside can refer to
    /// it and there is no symbol for it to resolve to. Which variable it is, is
    /// [`Annotation::variables`]'s to say — that list carries the sort and the
    /// identity, so inference does not re-derive from the lowered type what
    /// lowering already worked out.
    Var(String),
    /// A type position left entirely to the solver: lowers to a fresh
    /// variable, so whatever meets it decides it.
    ///
    /// Written `_`, and written by the compiler too: the pattern desugar's
    /// exact struct demand names which fields are there and nothing about what
    /// they hold, so each of its field types is one of these. The two are the
    /// same thing and are deliberately not told apart — a hole is *there to be
    /// decided*, whoever wrote it.
    Hole,
    Error,
}

/// One field of a struct type: the [`Field`] split of spans, plus the `when`
/// clause it may wear — there or not, as the named presence decides — or the
/// `\` that says it is definitely not there, with no type at all.
#[derive(Debug, Clone)]
pub enum TypeField {
    /// `name [when a]: T`, as written.
    Written {
        name_span: Span,
        when: Option<Box<When>>,
        value: Type,
    },
    /// `\name` — the label is explicitly absent, so there is no type here to
    /// carry: what the entry lowers to is [`Presence::Absent`](crate::types::Presence)
    /// with its type deliberately unconstrained. `name_span` covers the whole
    /// `\name`, which is where a complaint about the entry points.
    Absent { name_span: Span },
}

/// The `when` clause on one label, lowered.
///
/// The name stays a string for the reason [`Row::Named`] does: it is scoped to
/// the one written type it appears in, resolved by inference into the presence
/// variable the type's labels share, and nothing outside that type can refer to
/// it. `None` is `when _`, the anonymous presence — a variable minted like any
/// other and named by nothing, so no [`Clause`] can mention it.
#[derive(Debug, Clone)]
pub struct When {
    pub span: Span,
    pub name: Option<String>,
    /// Program-unique even for `when _`; anonymous occurrences must never
    /// accidentally share an inferred package slot.
    pub id: u32,
}

/// A `where` clause, lowered: a formula over the names the written type's
/// `when` clauses bound.
///
/// Every name in it is one the type binds — lowering refuses the rest, which is
/// what [`ErrorKind::UnboundPresence`] is — so inference can resolve one
/// without a lookup that could fail. Kept as the tree it was written as rather
/// than as a normal form, because an annotation is a contract and prints back
/// as itself; normalizing is generalization's, once.
pub type Clause = Tracked<ClauseKind>;

#[derive(Debug, Clone)]
pub enum ClauseKind {
    Name(String),
    Not(Box<Clause>),
    And(Box<Clause>, Box<Clause>),
    Or(Box<Clause>, Box<Clause>),
    /// `a = b` — both there or neither.
    Equal(Box<Clause>, Box<Clause>),
    /// `a != b` — exactly one of them there.
    NotEqual(Box<Clause>, Box<Clause>),
}

/// A written type and the `where` clause that followed it, lowered together.
///
/// One value rather than two fields wherever an ascription is kept, because the
/// clause is meaningless without the type whose variables it is written about:
/// the two are one contract, and a reader of either half alone is reading half
/// of what the definition promised.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub ty: Type,
    /// What the a variable statements declared, in the order they were
    /// written, each with the sort its uses gave it.
    ///
    /// Stored rather than read back out of the lowered type, because lowering
    /// is what worked it out: a variable's sort follows from
    /// where the type uses it, and inference should not re-derive a fact this
    /// pass already settled. Empty for an annotation that declares nothing,
    /// which is every annotation the language had before this.
    pub variables: Vec<Variable>,
    /// Anonymous positive-only occurrences and their exact producer boundary.
    /// Each `when _` has its own id even when several share a boundary.
    pub anonymous_existentials: Vec<(u32, Span)>,
    pub clause: Option<Clause>,
}

/// One variable a variable statement declared.
#[derive(Debug, Clone)]
pub struct Variable {
    /// Whether this presence is selected by callers or hidden by the value
    /// which produces it. Non-presence variables are always `Universal`.
    ///
    /// This is computed from the complete lowered annotation rather than at a
    /// `when` occurrence, because arrows can reverse polarity more than once.
    pub ownership: PresenceOwnership,
    /// Where the name was written, so a complaint about what the body did with
    /// it can point back at the promise it broke.
    pub span: Span,
    pub name: String,
    /// What the type beside it uses the name as. Worked out from the uses, the
    /// way a declaration parameter's [`ParamKind`] is; a variable used nowhere
    /// is refused rather than given a default, so this is always what something
    /// actually said.
    pub sense: Sense,
    /// Which variable this is, across the whole program. Two annotations that
    /// each write `a` declare two variables, and this is what keeps them apart
    /// wherever both are in hand at once.
    pub id: u32,
}

/// Ownership inferred for an annotation variable from its type polarity.
///
/// Existential ownership is semantic metadata only: it adds no source syntax
/// and has no runtime representation. `boundary` identifies the result/value
/// node which owns the hidden choice, allowing later lowering to preserve
/// nested result scopes instead of prenexing every presence into the scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceOwnership {
    Universal,
    Existential { boundary: Span },
}

#[derive(Debug, Clone, Default)]
struct PresenceOccurrences {
    positive: u32,
    negative: u32,
    owners: Vec<Span>,
}

impl PresenceOccurrences {
    /// The exact written production boundary shared by every positive
    /// occurrence. Containing boundaries are deliberately not unified: an
    /// invocation result makes a fresh choice, so its witness cannot also be
    /// the witness of the enclosing value (or of a sibling invocation).
    fn owner(&self) -> Result<Span, (Span, Span)> {
        let owner = self
            .owners
            .first()
            .copied()
            .expect("a positive presence occurrence has an owner");
        self.owners
            .iter()
            .copied()
            .find(|candidate| *candidate != owner)
            .map_or(Ok(owner), |second| Err((owner, second)))
    }
}

/// Compute presence polarity and every positive production boundary used by a
/// name. Formula uses are deliberately absent: a formula relates
/// presences established by labels but does not decide their ownership.
fn presence_polarities(
    ty: &Type,
    variances: &HashMap<Slot, u8>,
) -> HashMap<u32, PresenceOccurrences> {
    fn note(
        out: &mut HashMap<u32, PresenceOccurrences>,
        when: &Option<Box<When>>,
        positive: bool,
        owner: Span,
    ) {
        let Some(when) = when.as_ref() else { return };
        let occurrence = out.entry(when.id).or_default();
        if positive {
            occurrence.positive += 1;
            occurrence.owners.push(owner);
        } else {
            occurrence.negative += 1;
        }
    }

    enum Work<'a> {
        Ty(&'a Type, bool, Span),
        Effects(&'a EffectRow, bool, Span),
    }

    // An explicit worklist is important here: annotations and imported alias
    // applications are recovery input and may be far deeper than Rust's call
    // stack. Push in reverse source order so first-occurrence diagnostics stay
    // deterministic.
    let mut out = HashMap::new();
    let mut seen: HashMap<(usize, Span), u8> = HashMap::new();
    let mut work = vec![Work::Ty(ty, true, ty.span)];
    while let Some(part) = work.pop() {
        match part {
            Work::Effects(row, positive, owner) => {
                for effect in row.effects.values() {
                    if let EffectLabel::Written { when, .. } = effect {
                        note(&mut out, when, positive, owner);
                    }
                }
            }
            Work::Ty(ty, positive, owner) => {
                let bit = if positive { COVARIANT } else { CONTRAVARIANT };
                let visited = seen.entry((ty as *const Type as usize, owner)).or_default();
                if *visited & bit != 0 {
                    continue;
                }
                *visited |= bit;
                match &ty.tracked {
                    TypeKind::Arrow { from, to, effects } => {
                        work.push(Work::Effects(effects, positive, owner));
                        let result_owner = if positive { to.span } else { owner };
                        work.push(Work::Ty(to, positive, result_owner));
                        work.push(Work::Ty(from, !positive, owner));
                    }
                    TypeKind::Array(element) => work.push(Work::Ty(element, positive, owner)),
                    TypeKind::Struct { fields, .. } => {
                        for field in fields.values().rev() {
                            if let TypeField::Written { when, value, .. } = field {
                                note(&mut out, when, positive, owner);
                                work.push(Work::Ty(value, positive, owner));
                            }
                        }
                    }
                    TypeKind::Sum { cases, .. } => {
                        for case in cases.values().rev() {
                            if let SumCase::Written { when, payload, .. } = case {
                                note(&mut out, when, positive, owner);
                                if let Some(payload) = payload {
                                    work.push(Work::Ty(payload, positive, owner));
                                }
                            }
                        }
                    }
                    TypeKind::Effects(effects) => {
                        work.push(Work::Effects(effects, positive, owner))
                    }
                    TypeKind::Apply { head, args, .. } => {
                        for (at, arg) in args.iter().enumerate().rev() {
                            let variance = variances.get(&(*head, at as u32)).copied().unwrap_or(3);
                            if variance & 2 != 0 {
                                work.push(Work::Ty(arg, !positive, owner));
                            }
                            if variance & 1 != 0 {
                                work.push(Work::Ty(arg, positive, owner));
                            }
                        }
                    }
                    TypeKind::Ident(_)
                    | TypeKind::Param { .. }
                    | TypeKind::Prim(_)
                    | TypeKind::Var(_)
                    | TypeKind::Hole
                    | TypeKind::Error => {}
                }
            }
        }
    }
    out
}

const COVARIANT: u8 = 1;
const CONTRAVARIANT: u8 = 2;

/// Declared aliases are representation-transparent, so their parameter
/// variance is the least solution of the uses in every local and imported body.
/// The finite two-bit lattice makes recursive and mutually-recursive forwarding
/// terminate without a depth cap; a parameter may be covariant,
/// contravariant, invariant (both bits), or erased (no bits).
fn declaration_variances(
    local: &IndexMap<Symbol, Decl<Type>>,
    imported: &IndexMap<Symbol, ExternalType>,
) -> HashMap<Slot, u8> {
    let mut out = HashMap::new();
    for (symbol, decl) in local {
        for at in 0..decl.params.len() {
            out.insert((*symbol, at as u32), 0);
        }
    }
    for (symbol, decl) in imported {
        for at in 0..decl.params.len() {
            out.insert((*symbol, at as u32), 0);
        }
    }

    loop {
        let before = out.clone();
        for (owner, decl) in local {
            let mut seen: HashMap<usize, u8> = HashMap::new();
            let mut work = vec![(&decl.value, true)];
            while let Some((ty, positive)) = work.pop() {
                let bit = if positive { COVARIANT } else { CONTRAVARIANT };
                let visited = seen.entry(ty as *const Type as usize).or_default();
                if *visited & bit != 0 {
                    continue;
                }
                *visited |= bit;
                match &ty.tracked {
                    TypeKind::Param { index, .. } => {
                        *out.entry((*owner, *index)).or_default() |= bit;
                    }
                    TypeKind::Arrow { from, to, effects } => {
                        work.push((from, !positive));
                        work.push((to, positive));
                        if let Some(Tail {
                            of: Row::Param { index, .. },
                            ..
                        }) = &effects.tail
                        {
                            *out.entry((*owner, *index)).or_default() |= bit;
                        }
                    }
                    TypeKind::Array(element) => work.push((element, positive)),
                    TypeKind::Struct { fields, tail } => {
                        work.extend(fields.values().filter_map(|field| match field {
                            TypeField::Written { value, .. } => Some((value, positive)),
                            TypeField::Absent { .. } => None,
                        }));
                        if let Some(Tail {
                            of: Row::Param { index, .. },
                            ..
                        }) = tail
                        {
                            *out.entry((*owner, *index)).or_default() |= bit;
                        }
                    }
                    TypeKind::Sum { cases, tail } => {
                        work.extend(cases.values().filter_map(|case| match case {
                            SumCase::Written {
                                payload: Some(value),
                                ..
                            } => Some((value, positive)),
                            _ => None,
                        }));
                        if let Some(Tail {
                            of: Row::Param { index, .. },
                            ..
                        }) = tail
                        {
                            *out.entry((*owner, *index)).or_default() |= bit;
                        }
                    }
                    TypeKind::Effects(effects) => {
                        if let Some(Tail {
                            of: Row::Param { index, .. },
                            ..
                        }) = &effects.tail
                        {
                            *out.entry((*owner, *index)).or_default() |= bit;
                        }
                    }
                    TypeKind::Apply { head, args, .. } => {
                        for (at, arg) in args.iter().enumerate() {
                            let variance = before.get(&(*head, at as u32)).copied().unwrap_or(3);
                            if variance & COVARIANT != 0 {
                                work.push((arg, positive));
                            }
                            if variance & CONTRAVARIANT != 0 {
                                work.push((arg, !positive));
                            }
                        }
                    }
                    TypeKind::Ident(_)
                    | TypeKind::Prim(_)
                    | TypeKind::Var(_)
                    | TypeKind::Hole
                    | TypeKind::Error => {}
                }
            }
        }

        enum Semantic<'a> {
            Ty(&'a Ty, bool),
            Row(&'a crate::types::Row, bool),
        }
        for (owner, decl) in imported {
            let mut seen: HashMap<usize, u8> = HashMap::new();
            let mut work = vec![Semantic::Ty(decl.scheme.body(), true)];
            while let Some(item) = work.pop() {
                match item {
                    Semantic::Ty(ty, positive) => {
                        let bit = if positive { COVARIANT } else { CONTRAVARIANT };
                        let visited = seen.entry(ty as *const Ty as usize).or_default();
                        if *visited & bit != 0 {
                            continue;
                        }
                        *visited |= bit;
                        match ty {
                            Ty::Bound(index) if (*index as usize) < decl.params.len() => {
                                *out.entry((*owner, *index)).or_default() |= bit;
                            }
                            Ty::Arrow(from, to, effects) => {
                                work.push(Semantic::Ty(from, !positive));
                                work.push(Semantic::Ty(to, positive));
                                work.push(Semantic::Row(effects, positive));
                            }
                            Ty::Package(body) => work.push(Semantic::Ty(body, positive)),
                            Ty::Array(element) => work.push(Semantic::Ty(element, positive)),
                            Ty::Struct(row) | Ty::Sum(row) => {
                                work.push(Semantic::Row(row, positive))
                            }
                            Ty::Named { symbol, args, .. } => {
                                for (at, arg) in args.iter().enumerate() {
                                    let variance =
                                        before.get(&(*symbol, at as u32)).copied().unwrap_or(3);
                                    if variance & COVARIANT != 0 {
                                        work.push(Semantic::Ty(arg, positive));
                                    }
                                    if variance & CONTRAVARIANT != 0 {
                                        work.push(Semantic::Ty(arg, !positive));
                                    }
                                }
                            }
                            Ty::Var(_)
                            | Ty::Rigid { .. }
                            | Ty::Bound(_)
                            | Ty::Undecided
                            | Ty::Nat
                            | Ty::Int
                            | Ty::Real
                            | Ty::String
                            | Ty::Boolean => {}
                        }
                    }
                    Semantic::Row(row, positive) => {
                        let bit = if positive { COVARIANT } else { CONTRAVARIANT };
                        work.extend(
                            row.labels
                                .values()
                                .map(|field| Semantic::Ty(&field.ty, positive)),
                        );
                        if let Rest::Bound(index) = row.rest
                            && (index as usize) < decl.params.len()
                        {
                            *out.entry((*owner, index)).or_default() |= bit;
                        }
                        if let Rest::More(more) = &row.rest {
                            work.push(Semantic::Row(more, positive));
                        }
                    }
                }
            }
        }
        if out == before {
            break;
        }
    }
    out
}

/// One case of a sum type: the [`Field`] split of spans, the `when` clause it
/// may wear — a case a value may or may not be — and what it carries; or the
/// `\` that says the case is definitely absent, [`TypeField::Absent`]'s twin.
///
/// `payload` keeps the `None` the parser gave it, for the reason
/// [`TermKind::Tag`] does: a case written bare means unit, and saying so here
/// would be this pass writing a type nobody wrote into the tree the debugger
/// shows.
#[derive(Debug, Clone)]
pub enum SumCase {
    /// `#Name [(when a)] [T]`, as written.
    Written {
        name_span: Span,
        when: Option<Box<When>>,
        payload: Option<Type>,
    },
    /// `\#Name` — the case is explicitly absent, field_summaries nothing.
    /// `name_span` covers the whole `\#Name`.
    Absent { name_span: Span },
}

/// The effects an arrow may perform, lowered: the effects it names, and what is
/// known about the ones it does not.
///
/// Aliases are gone by the time one of these exists — `!Console` lowers to
/// the labels it stands for — so nothing downstream has an alias to look
/// through, and a printed type shows the effects rather than the name. See
/// [`Builder::expansions`].
///
/// `written` is `false` for the row a bare `A -> B` has, which is the empty
/// closed one. Kept so that the debugger's AST and IR tabs can print an arrow
/// back as it stood: `A -> B + |` writes a row and `A -> B` writes none, and
/// both mean this.
#[derive(Debug, Clone, Default)]
pub struct EffectRow {
    pub span: Span,
    pub written: bool,
    pub effects: IndexMap<EffectId, EffectLabel>,
    pub tail: Option<Tail>,
}

/// One effect a row names: performed — as its `when` clause says, where it
/// wears one — or definitely not.
///
/// [`SumCase`]'s twin, whose payload is the arguments the effect is applied
/// to: `!Ask Nat` hands `Ask`'s one parameter `Nat`, and a label holds those
/// beside what a complaint about it needs and the symbol the name resolved
/// to. An effect declared without parameters carries none.
#[derive(Debug, Clone)]
pub enum EffectLabel {
    /// `!Log`, `!Ask Nat`, or `!Log (when a)`.
    Written {
        name_span: Span,
        symbol: Symbol,
        /// The arguments, one per parameter the effect declares, in order.
        args: Vec<Type>,
        /// Whether this label came out of an alias rather than being written.
        ///
        /// `!Console` stands for `!Log` and `!IO`, and neither of
        /// those names is anywhere on the page — `name_span` is the alias's.
        /// What reads this is the debugger, which may paint a span in the
        /// editor as a use of a name only where the name really is.
        expanded: bool,
        when: Option<Box<When>>,
    },
    /// `\!Log` — definitely not performed. `name_span` covers the whole
    /// `\!Log`.
    Absent {
        name_span: Span,
        symbol: Symbol,
        args: Vec<Type>,
        expanded: bool,
    },
}

/// The `..` tail of a struct type: what is said about the fields not named.
#[derive(Debug, Clone)]
pub struct Tail {
    pub span: Span,
    pub of: Row,
}

/// What a `..` tail stands for.
#[derive(Debug, Clone)]
pub enum Row {
    /// `..` — any fields at all. Only an annotation may say this; a
    /// declaration holds for every definition and so cannot leave the question
    /// open. See [`ErrorKind::OpenDeclaredType`].
    Anything,
    /// `..'r` in an annotation, naming one of the variables its a variable
    /// declared: a name scoped to that one annotation, staying a string for the
    /// reason [`Field`]'s keys do — it is not a path anything can refer to, so
    /// there is no symbol to resolve it to. Two `..'r` in one annotation stand
    /// for one rest; another annotation's `'r` is unrelated.
    ///
    /// A use rather than a binder. `r` has to have been declared, or the
    /// annotation is refused with [`ErrorKind::Undefined`] — which is what
    /// makes a rest a promise the body can be held to instead of a variable the
    /// body may quietly decide.
    Named(String),
    /// `..'r` naming a row parameter of the declaration being lowered. This is
    /// the one tail a declaration may have, and the only way a declared type
    /// can be left open: what it stands for is supplied at every use rather
    /// than decided once here, so the body still mentions no solver variable.
    Param { symbol: Symbol, index: u32 },
}

/// A struct field. The name is the map key rather than part of the value, so
/// that a field can be looked up by name alone; only the span the name was
/// written at is field_summary here. `value` keeps its own span as usual.
///
/// Field names stay strings: they are labels scoped to their own struct, not
/// paths anything can refer to, so they have no place in a module tree.
#[derive(Debug, Clone)]
pub struct Field<T> {
    pub name_span: Span,
    pub value: T,
}

/// The `..` of a struct literal, normalized: the same dots-and-value the
/// surface carries, the value lowered.
#[derive(Debug, Clone)]
pub struct Spread {
    pub span: Span,
    pub value: Box<Term>,
}

/// One item of an array literal, the `..` kept as the span it was written at
/// when the item spreads an array rather than supplying one value.
#[derive(Debug, Clone)]
pub struct ArrayItem {
    pub spread: Option<Span>,
    pub value: Term,
}

/// The `..` of an array pattern, normalized: the same dots-and-name the
/// surface carries, the name resolved to the symbol it binds.
#[derive(Debug, Clone)]
pub struct ArrayRest {
    pub span: Span,
    pub name: Option<Tracked<Symbol>>,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    /// Arrays have a private persistent representation and cannot cross a
    /// user-authored host boundary in this release.
    ArrayInExtern,
    /// A dependency alias cannot be written as a source path component.
    InvalidDependencyAlias {
        alias: String,
    },
    /// The same source-visible dependency alias was supplied more than once.
    DuplicateDependencyAlias {
        alias: String,
    },
    /// The same artifact identity was supplied more than once.
    DuplicateDependency {
        name: String,
        version: String,
    },
    /// A name with no definition in scope at the point it was written.
    Undefined {
        name: String,
        namespace: Namespace,
    },
    /// A second definition of a name. The first one is the one that stands.
    Duplicate {
        name: String,
        namespace: Namespace,
        previous: Span,
    },
    DuplicateField {
        name: String,
        previous: Span,
    },
    /// A second case of a name in one sum, one effect row, or one alias — all
    /// three being a set of labels a name may appear in once.
    DuplicateCase {
        shape: Shape,
        name: String,
        previous: Span,
    },
    /// An explicitly absent label in a composite with no `..` tail, as in
    /// `{ a: Nat, \y }` or `#A | \#B`.
    ///
    /// A `\` says the `..` beside it may not stand for the label, and a type
    /// with no `..` already says that of every label it does not name — so
    /// there is nothing here for the mark to rule out. Refused rather than
    /// dropped: the mark says something about a tail that is not there, and a
    /// reader who wrote it meant one of the two to change.
    ///
    /// The shape and the label are field_summary for the wording alone, the way
    /// [`ErrorKind::RepeatedRowField`] carries them: the complaint quotes the
    /// label the way it was written, `#` and all for a case.
    AbsentInClosed {
        shape: Shape,
        label: String,
    },
    /// A definition given as a name that leads back to itself with nothing in
    /// between: `type t = t`, `let x = x`, or a pair each given as the other.
    ///
    /// This is not the same complaint as a type that contains itself. A type
    /// may name itself as much as it likes through a struct or an arrow —
    /// that is what makes recursive types writable — because unfolding such a
    /// type reaches a shape one step in. A chain of bare names never reaches
    /// one, so there is nothing for the declaration to mean.
    ///
    /// A declaration that stands for one of its own arguments is a link in
    /// such a chain rather than an end to it: `type A 'a = 'a` hands back
    /// whatever it was given, so `type B = A B` leads back to `B` with only a
    /// hand-off in between and reaches no shape either. What closes the loop
    /// is the whole chain, so the loop is looked for by following what each
    /// declaration stands for rather than by reading any one body.
    ///
    /// One rule about both namespaces, worded twice. A term is the same
    /// mistake made about values: `let f = fn n => f n` names itself through a
    /// shape and is an ordinary recursive function, and `let x = x` reaches no
    /// shape at all and so says nothing about what `x` is. The namespace is
    /// field_summary for the wording, the way [`ErrorKind::Undefined`] carries one.
    Circular {
        namespace: Namespace,
    },
    /// A `when` clause inside a `type` declaration, or a `..` tail there that
    /// does not name one of the declaration's own parameters — as in
    /// `type t = { x: Nat, .. }`.
    ///
    /// What a declaration stands for is lowered once, before any definition,
    /// and holds for all of them; a `when` or a bare `..` stands for something
    /// a definition gets to decide, so there is nothing for one to mean here.
    ///
    /// A tail naming a row parameter is the exception, and the reason the rule
    /// is worth stating this precisely rather than as "a declaration is
    /// closed". What such a tail stands for is not decided here either — it is
    /// supplied at every use — so it lowers to a [`Ty::Bound`], not to a
    /// variable, and the property inference leans on survives untouched: a
    /// declaration's body mentions no solver variable, which is what lets every
    /// walk stop at a name instead of descending into what it stands for.
    ///
    /// The shape is field_summary for the wording alone: `type X = #A (when a) Nat`
    /// is the same mistake made about cases, and a complaint that said "fields"
    /// to someone who wrote `#`s would be describing a type they never
    /// wrote.
    OpenDeclaredType {
        shape: Shape,
    },
    /// A `where` clause on a `type` declaration's body, as in
    /// `type T = { x: Nat } where a`.
    ///
    /// [`ErrorKind::OpenDeclaredType`]'s sibling and refused for the same
    /// reason: a declaration holds for every definition, so it has no presence
    /// of its own left open for a formula to relate. It is a complaint of its
    /// own rather than that one because a clause sits beside the type rather
    /// than inside a row, so there is no shape to word it in.
    ClauseInDeclaration,
    /// A variable written in a `type` declaration's body that its header does
    /// not bind, or one written in an operation's signature at all, as in
    /// `type Bad = { x: 'a }`.
    ///
    /// [`ErrorKind::ClauseInDeclaration`]'s sibling, and refused for a reason
    /// of its own rather than for that one's: a variable is something the
    /// caller picks, and a declaration says the same thing wherever it is used,
    /// so its own variables are its parameters and they are written in its
    /// header. Reported at each name, so a body with several reports each.
    ///
    /// The fix is the header, which is why this is one complaint rather than
    /// two: `type Bad = { x: Nat, ..'r }` left a row open by naming something
    /// nothing binds, and writing `'r` beside the name is what closes it.
    VariableInDeclaration {
        name: String,
    },
    /// A `_` in a `type` declaration's body, as in `type Bad = { x: _ }`.
    ///
    /// [`ErrorKind::OpenDeclaredType`]'s sibling: a hole is a position left for
    /// a definition to decide, and a declaration says the same thing wherever
    /// it is used, so there is nothing here for one to leave open. A complaint
    /// of its own rather than that one because a hole is not a row's openness —
    /// there is no shape to word it in.
    HoleInDeclaration,
    /// A `_` in an operation signature. Operation signatures are fixed
    /// interfaces just like declared types, but this context is kept distinct
    /// so diagnostics do not describe the hole as part of a type declaration.
    HoleInOperation,
    /// A `where` clause naming something no `when` in the same type gives a
    /// label to, as in `{ x when 'a: Nat } -> Nat where 'a = 'c`.
    ///
    /// A formula is written about presences, and a presence is what a `when`
    /// puts on a label — so a name the type never wears is a name the formula
    /// has nothing to say about. Refused rather than read as a fresh variable:
    /// a formula about a presence no label carries constrains nothing, and a
    /// reader who wrote one meant a name they also wrote on a label.
    ///
    /// Both halves of that reach here: a name the type never writes at all, and
    /// one it writes somewhere other than on a label. The reader's fix is the
    /// same either way — put the name on a label — so the complaint is too.
    UnboundPresence {
        name: String,
    },
    /// One producer-chosen presence was written at more than one production
    /// boundary. Each result invocation and enclosing value owns a distinct
    /// hidden choice, so one source variable cannot identify their witnesses.
    IncompatiblePresenceOwnership {
        name: String,
        /// The other actionable production boundary.
        previous: Span,
    },
    /// A type given a different number of arguments than it takes, including a
    /// name written bare that takes some.
    ///
    /// There is no partial application: a declaration takes what it takes
    /// wherever it is written, so a name short of its arguments is the same
    /// complaint as one given too many.
    Arity {
        name: String,
        expected: usize,
        found: usize,
    },
    /// An effect applied to the wrong number of arguments: `!Ask` or
    /// `!Ask Nat Nat` where `effect Ask 'a` takes one. Counted at the label,
    /// the way [`ErrorKind::Arity`] counts a type application, and the label
    /// is dropped rather than paired up by guesswork.
    EffectArity {
        name: String,
        expected: usize,
        found: usize,
    },
    /// A ring of aliases each standing for the next. Every one is refused,
    /// since expanding it would never end; `growing` says the ring adds to the
    /// row on the way round rather than only handing it on, which is a
    /// different mistake to look for.
    AliasCycle {
        name: String,
        growing: bool,
    },
    /// An alias written absent or under a `when` whose expansion keeps an open
    /// tail: the modifier distributes to every effect the alias stands for,
    /// and a tail nobody has named yet is not one of them.
    ModifiedOpenAlias {
        name: String,
    },
    /// A row given two tails: one it wrote, and one an alias it names brought
    /// with it.
    TwoTails {
        previous: Span,
    },
    /// Something that is not a declared type, applied: a primitive, a struct,
    /// a parenthesized arrow.
    NotAConstructor,
    /// A parameter used as the head of an application, as in
    /// `type Flip 'f 'a = 'f 'a`.
    ///
    /// A parameter stands for one type, never for something still waiting for
    /// types of its own. Refusing this is what keeps every declaration's
    /// parameters plain — each one a type, and nothing higher — so that
    /// checking an application is counting rather than a language of its own.
    ParameterApplied {
        name: String,
    },
    /// One declaration binding a name twice: `type Pair 'A 'A = ...`.
    DuplicateParameter {
        name: String,
        previous: Span,
    },
    /// A type that leads back to itself having been given an argument built out
    /// of what it takes, as in `type T 'a = { next: T { x: 'a } }`.
    ///
    /// One reason, and it is growth. Unfolding that declaration hands on
    /// `{ x: 'a }`, then `{ x: { x: 'a } }`, and so on: the argument is bigger
    /// every time and never comes back round, so there is no finite answer to
    /// whether two of them are the same type.
    ///
    /// What does come back round is allowed, and that is the whole of the rule.
    /// A parameter handed straight on is whatever came in. An argument
    /// mentioning no parameter is written out in the program and is the same
    /// type every round, so `type Forest = { head: Tree Nat }` inside `Tree`'s
    /// own group is an ordinary declaration and is accepted. See [`grows`] for
    /// the condition and [`Solve::unfold`](crate::inference) for what rests on
    /// it.
    GrowingRecursion,
    /// One a variable variable used at two sorts in one annotation, as in
    /// `{ x: Nat, ..'r } -> (#A Nat | ..'r)` or
    /// `{ x when 'a: Nat } -> 'a`.
    ///
    /// A declared variable stands for one thing, and there are three things it
    /// could be: a whole type — which is what a struct's `..` is, since the
    /// rest of a struct is the type its fields sit on — the cases a sum does
    /// not write out, or whether one label is there. Which of them was meant is
    /// the writer's to say, so this is reported at the second use, the one that
    /// brought the two together, the way a mixed parameter is reported at the
    /// parameter.
    ///
    /// A name in a `where` clause's formula counts as a presence use, since a
    /// formula is written about presences and nothing else.
    ///
    /// The row absorbs, for the reason every other row mistake does: left
    /// standing, the tail would be shared anyway, and a field would come back
    /// out of the solve as a case with the reader never told why.
    ///
    /// Where the name was first used is field_summary the way a repeat carries the
    /// definition it repeats — see [`ErrorKind::Duplicate`] — because half of
    /// what went wrong is somewhere else on the page, and a reader shown only
    /// the second `..` has to hunt for the first one themselves.
    MixedTail {
        first: Sense,
        second: Sense,
        previous: Span,
    },
    /// A parameter used as both of the things a parameter can be — a whole type
    /// and the rest of a sum — as in `type M 'r = { g: (#A | ..'r), f: 'r }`.
    ///
    /// `type W 'r = { f: 'r, ..'r }` is *not* one. The rest of a struct is a
    /// whole type, so both uses say the same thing about `'r` and the
    /// declaration is well-formed; only a sum's rest is a second reading to
    /// disagree with.
    ///
    /// A parameter is written as a name and nothing more, so what it stands
    /// for is read off its uses. Two uses that disagree leave nothing to read,
    /// and neither of them is the wrong one — it is the declaration that has to
    /// say which it meant.
    ///
    /// The two readings can meet across declarations, when one hands its
    /// parameter to another and uses it as a type as well. The declaration
    /// told is the one that brought them together: one that merely hands the
    /// parameter on to a declaration already broken says one thing about its
    /// own and is right about it, however long the chain of them is.
    ///
    /// Every declaration the clash reaches is erased all the same, told or not.
    /// What such a body would stand for is exactly what could not be worked
    /// out, and leaving it standing would put whatever a use site handed it
    /// into a row — so the declaration absorbs, and nothing is asked of the
    /// arguments written at it either.
    ///
    /// The two readings are field_summary so the complaint can name them. A
    /// parameter read all three ways still names two: there is one thing to
    /// fix, and a sentence listing every way the declaration is wrong is not a
    /// better instruction for fixing it.
    MixedParameter {
        first: Sense,
        second: Sense,
    },
    /// Something that cannot stand for the rest of a sum's cases, written where
    /// a sum's row parameter goes: `Or Nat` against
    /// `type Or 'r = #A | ..'r`.
    ///
    /// A sum can stand for one, and so can another sum's row parameter. A struct
    /// cannot, and neither can a declared name, though the latter looks as
    /// though it should: a tail holding a name would have to be unfolded by the
    /// walks that flatten rows, and neither does.
    ///
    /// Never about a struct: a struct's `..` is its row tail, and its tail accepts only a field row
    /// type at all, so `WithX Nat` is well-formed. A sum's rest and an arrow's
    /// effects are the two that are spliced into a row, so the reading is
    /// field_summary to say which of them the reader was asked for. The name stays
    /// because the code is stable and renaming it would churn a code and a test
    /// file for no gain.
    ///
    /// The argument absorbs, so this is said once. Left standing it would be
    /// substituted into the tail all the same, and the reader would be told a
    /// second time in words about a row they never wrote.
    NotARow {
        sense: Sense,
    },
    /// An argument naming a label the declaration it is handed to already
    /// names: `WithX { x: Nat }` against `type WithX 'r = { x: Nat, ..'r }`,
    /// and `Or (#A)` against `type Or 'r = #A | ..'r`.
    ///
    /// A `..` covers the labels its own row does not write out, so what is
    /// spliced in may not write out any of them: the type would name the label
    /// twice, and the two copies could disagree. Which labels those are is part
    /// of what the parameter stands for — see [`ParamKind`] — so it is known
    /// here, at the argument, rather than only wherever something later happened
    /// to flatten the row.
    ///
    /// A struct's argument is looked at through names as well as at what it
    /// writes out, since a `..` handed a declared type ends up field_summaries whatever
    /// *that* carries: `WithX (WithX Nat)` names `x` twice as much as
    /// `WithX { x: Nat }` does. See [`row_summaries`].
    ///
    /// The argument absorbs, for the reason [`ErrorKind::NotARow`] does: left
    /// standing it would be substituted in all the same, and the reader would be
    /// told a second time about a type nobody wrote.
    RepeatedRowField {
        shape: Shape,
        field: String,
    },
    /// A declaration whose fields never run out: `type T = WithX T` against
    /// `type WithX 'r = { x: Nat, ..'r }`, or a pair reaching each other the
    /// same way.
    ///
    /// A struct's `..` is the struct-row tail, so a declaration written at one is a
    /// declaration in the constructor position of what the first stands for. Following
    /// those positions round to the declaration itself means each unfolding adds
    /// the fields written in the `..` and finds the same `..` again: `T` has
    /// an `x`, and past it a `T`, which has an `x`, and there is no finite set of
    /// fields for `T` to have.
    ///
    /// [`ErrorKind::Circular`]'s sibling and told apart from it by one step: a
    /// loop with no such `..` on it reaches no shape at all and is `Circular`,
    /// and one with a `..` on it reaches a shape every time round and is this.
    /// `type List = { next: List }` is neither — the recursion is in a field's
    /// type and the constructor is unit.
    ///
    /// Carries nothing. The span is the body, and every declaration on the loop
    /// is reported, so there is nothing further to say — the same choice
    /// [`ErrorKind::Circular`] makes.
    EndlessFields,
    /// A pattern that can fail, written on a `let` — `let #Some x = opt`. A
    /// binding has no arms to fall through to, so it has to accept every
    /// value, and a tag or primitive literal anywhere in the pattern is a
    /// value it would not.
    ///
    /// Lowering stays total: every name the pattern would have bound is still
    /// bound, to [`TermKind::Error`] values, so downstream uses resolve and
    /// one mistake makes one complaint. What made the pattern able to fail is
    /// field_summary so the complaint can quote it.
    RefutableBinding {
        found: Refuter,
    },
    /// One pattern binding the same name twice — `{x, x}`, `{a: x, b: x}`, or
    /// across any nesting in one pattern. Reported at the repeat; the first
    /// binding is the one the body sees. Distinct patterns — two different
    /// arms — may of course bind the same name.
    DuplicateBinding {
        name: String,
        previous: Span,
    },
    /// A second operation of a name in one effect:
    /// `effect Log = { write: Nat -> (), write: () -> () }`.
    ///
    /// [`ErrorKind::DuplicateCase`]'s twin, and scoped the same way: an
    /// operation belongs to its own declaration, so two effects may each
    /// declare a `write` and two `write` in one may not.
    DuplicateOperation {
        name: String,
        previous: Span,
    },
    /// An operation whose signature is not a function, retained after the
    /// parser rejects `effect Log = { write: Nat }`.
    ///
    /// Performing an operation is applying it, so an operation that is not an
    /// arrow would be performed by mention — there would be nowhere for the
    /// perform site to be. Refused rather than read as a nullary operation,
    /// which is a larger language than this one has.
    NotAnOperation {
        /// The operation's name, so the complaint can show what to write
        /// instead.
        name: String,
    },
    /// The part of an operation signature that prevents it from being one
    /// fixed function type. The reason is retained so the diagnostic can name
    /// the exact construct to change rather than listing every possible one.
    ImpureOperation {
        found: OperationTypeProblem,
    },
    /// A row of effects written where a type goes: `let x : !Log = 1`.
    ///
    /// A row is not a type. The one place one may be written without an arrow
    /// to carry it is an argument — `Runner (!Log)` hands a declaration the
    /// effects its own `..` stands for — and everywhere else there is nothing
    /// for a set of effects to mean.
    ///
    /// Whether the parameter it was handed to is used as effects at all is a
    /// separate question, asked where every other row argument's is; see
    /// [`ErrorKind::NotARow`].
    EffectsOutsideRow,
    /// `!Console.write` where `Console` is an alias.
    ///
    /// An alias is a name for a set of effects and declares nothing of its own,
    /// so there is no operation here to refer to. The name is field_summary for the
    /// wording; the span points at the head.
    OperationOnAlias {
        effect: String,
    },
    /// An operation an effect does not declare: `!Log.writ`.
    ///
    /// [`ErrorKind::Undefined`]'s cousin, and not it: an operation is a label
    /// scoped to its own declaration rather than a name in a namespace, so what
    /// went wrong names the effect as well as the operation.
    UnknownOperation {
        effect: String,
        op: String,
    },
    /// Bare access to an empty or named-only effect.
    BareOperationUnavailable {
        effect: String,
        suggestion: Option<String>,
    },
    /// Dotted access to an unnamed singleton effect.
    NamedOperationOnUnnamed {
        effect: String,
        op: String,
    },
    /// A handler naming an effect it does not fully cover, as in
    /// `handle p () with | !Log.write s => () end` where `Log` also
    /// declares `flush`.
    ///
    /// Which effects a handler discharges has to be known before inference —
    /// the body is checked at an ambient the discharged effects extend — and a
    /// half-covered effect leaves that set undecidable. The operations with no
    /// arm are field_summary so the complaint can name them.
    PartialHandler {
        effect: String,
        missing: Vec<String>,
    },
    /// Two arms for one operation in a handler. The first is the one that
    /// stands, the way a duplicate anything else is.
    DuplicateArm {
        effect: String,
        selector: OperationSelector,
        previous: Span,
    },
    /// Two `return` arms in one handler. Reported at the second; the first is
    /// the one that stands.
    DuplicateReturn {
        previous: Span,
    },
    /// A `raise` with no handler arm enclosing it.
    RaiseOutsideArm,
    /// A `raise` written inside a `fn` that sits inside a handler arm.
    ///
    /// The rule that keeps the feature sound. A `raise` answers one particular
    /// handler rather than an effect, so a closure field_summaries one could be
    /// returned by the computation, outlive the `handle`, and be called with no
    /// handler on the stack. The mirror case needs no rule, because the row
    /// already tracks it.
    RaiseInFunction {
        function: Span,
    },
}

/// What prevents an operation signature from being one fixed function type.
#[derive(Debug, Clone)]
pub enum OperationTypeProblem {
    /// An explicit `+ effects` on an operation arrow.
    Effects,
    /// A `..` tail or `when` condition that leaves part of the signature open.
    OpenPart,
    /// A variable that no operation declaration can bind.
    Variable(String),
}

/// What made a binding's pattern able to fail: the first tag or literal found
/// in it, field_summary so the complaint can quote what the reader wrote. See
/// [`ErrorKind::RefutableBinding`].
#[derive(Debug, Clone)]
pub enum Refuter {
    /// A tag pattern: a value here might not be this case.
    Case(String),
    /// A primitive literal: a value here might be a different value of this
    /// primitive type.
    Literal(Literal),
    /// An array pattern naming elements: a value here might have a different
    /// number of them, length being no part of an array's type.
    Length,
}

/// A concrete example of a value a match leaves unhandled, in the shape of the
/// value rather than in words, so the complaint can write it in source syntax.
/// See [`patterns::ErrorKind::UnhandledValues`](crate::patterns::ErrorKind).
#[derive(Debug, Clone)]
pub enum Witness {
    /// Any value at all: a position the arms never test, so no example is
    /// more instructive than another.
    Any,
    /// This number.
    Natural(u64),
    /// This non-natural primitive value.
    Literal(Literal),
    /// This case. `None` says any payload serves — rendered bare, the way a
    /// case field_summaries unit is written.
    Tag {
        name: String,
        payload: Option<Box<Witness>>,
    },
    /// A value field_summaries these fields. A field held to be *present* is named
    /// even when any value serves for what it holds — under exactness the
    /// presence is the information, and such a field prints pun-style, as
    /// `{a}` — while a field the example does not need is left out.
    Struct(IndexMap<String, Witness>),
    /// A value that is none of the listed cases — what an open position's
    /// "anything else" is written as.
    Other(Vec<String>),
    /// An array of these elements — and, when `open`, of any number more:
    /// the example stands for every length past the ones the arms name.
    Array { elements: Vec<Witness>, open: bool },
}

#[derive(Debug, Clone)]
pub struct Output {
    pub program: Program,
    pub errors: Vec<Error>,
}

/// The namespaces a `let`, a `type` and an `effect` declaration write into.
///
/// [`Namespace`] has a fourth — modules — which `module` writes into and which
/// nothing else can reach: a module is declared by one statement and named only
/// by a path's segments, so [`Builder::declare`] is told which of these three
/// rather than asked to rule out a value it can never be handed. See
/// [`Builder::declare_module`], which is the fourth's own door.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Terms,
    Types,
    Effects,
}

/// What resolving a path found, when it did not find a symbol.
///
/// The two halves of R19, told apart because only one of them is the caller's
/// to word: a segment naming no module has already been reported where it was
/// written, and a name the named module does not declare is reported in the
/// namespace the position demanded — which the caller is the one that knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// A segment named no module. Reported already, at that segment.
    Segment,
    /// The path's own name is not declared where the segments led. Nothing has
    /// been reported: a bare name in a type may still be a primitive.
    Name,
}

impl From<Scope> for Namespace {
    fn from(scope: Scope) -> Self {
        match scope {
            Scope::Terms => Namespace::Terms,
            Scope::Types => Namespace::Types,
            Scope::Effects => Namespace::Effects,
        }
    }
}

struct Builder<'a> {
    mint: &'a mut Mint,
    /// The module being lowered into: `None` at the top level of the bundle,
    /// which is a real position in the tree rather than a missing one. Every
    /// symbol minted while it holds a module is minted under that module, and
    /// every unqualified name is resolved starting from it.
    module: Option<Module>,
    errors: Vec<Error>,
    /// The locals in scope, innermost last — a lambda's arguments, a nested
    /// `let`'s name, a handler arm's binder — and nothing else. Where a global
    /// lives is decided by its module rather than by how far down a stack it
    /// sits, so globals are in [`Builder::globals`] instead.
    ///
    /// Only terms have any: a type's parameters are [`Builder::params`] and
    /// wear a `'`, and nothing binds an effect locally.
    terms: Names,
    /// Every global declaration in the bundle, by the module it was written in,
    /// the namespace it lives in and the name it was written under, with where
    /// that name was written so a repeat can point back at it.
    ///
    /// Filled by the hoist before any body is lowered, so a name may be reached
    /// from above the declaration that binds it and from another file. R9's
    /// walk reads it once per enclosing module, outward to the bundle root.
    globals: HashMap<(Option<Module>, Namespace, String), (Symbol, Span)>,
    /// The modules themselves, which are their own namespace: `module Pair`,
    /// `type Pair`, `let Pair` and `effect Pair` may all coexist in one scope.
    /// Kept apart from [`Builder::globals`] because a [`Module`] is a symbol
    /// already known to be one, and passing a term where a containing module
    /// goes is what that newtype exists to rule out.
    modules: HashMap<(Option<Module>, String), (Module, Span)>,
    /// The immediate `prelude` module of the direct dependency imported under
    /// the source alias `std`. Bare lookup consults its direct members only,
    /// after every ordinary user scope has failed.
    std_prelude: Option<Module>,
    /// What each declared effect stands for: itself, for one that declares
    /// operations, and the effects it names — transitively — for an alias.
    ///
    /// Filled before any type body or annotation is lowered, which is what lets
    /// an effect row expand an alias wherever it is written, including above the
    /// declaration it names. See [`Builder::expansions`].
    expanded: HashMap<Symbol, IndexMap<String, Symbol>>,
    /// The operations each effect *declares*, in the order it declares them.
    /// An alias has no entry at all, which is what tells `!Console.write`
    /// from `!Log.writ`: one names something that declares no operations,
    /// and the other an operation the effect does not have.
    operations: HashMap<Symbol, IndexSet<OperationSelector>>,
    /// Which handler arm, if any, lexically encloses the term being lowered.
    /// What R17's `raise` placement check reads; see [`Answering`].
    answering: Answering,
    /// How many arguments each declared type takes. Filled before any body is
    /// lowered, so an application can be counted wherever it appears —
    /// including above the declaration it names, which the hoist allows.
    ///
    /// The count is the parameters the declaration bound, not the names it
    /// wrote: a repeated name binds nothing, so there is nothing in the body
    /// that could name the argument it would ask for.
    arities: HashMap<Symbol, usize>,
    /// Alias declarations not yet lowered, by symbol. An alias body is read
    /// the first time anything names it — which may be another declaration
    /// above it — so the bodies are kept as written until then. See
    /// [`Builder::alias_body`].
    pending_aliases: HashMap<Symbol, PendingAlias>,
    /// Every alias body lowered so far, local or imported.
    alias_bodies: HashMap<Symbol, AliasBody>,
    /// The aliases being expanded, innermost last: an alias met again while it
    /// is on this stack is a cycle.
    expanding: Vec<Symbol>,
    /// The aliases a cycle was reported at, which expand to nothing from then
    /// on so the one complaint stands alone.
    cyclic: HashSet<Symbol>,
    /// The aliases a dependency declared. A cycle through these is the
    /// dependency's mistake, recovered silently rather than reported at a
    /// declaration the reader cannot see.
    imported_aliases: HashSet<Symbol>,
    /// What each imported effect's parameters stand for, from its header.
    imported_effect_params: HashMap<Symbol, Vec<ParamKind>>,
    /// The parameters of the declaration being lowered, by the name they were
    /// written under, with the symbol each was minted as and where it sits in
    /// the list. Empty outside a `type` body, which is what makes a parameter
    /// unwritable in an annotation — a `'a` written there is a variable
    /// instead, which is the same name meaning the one other thing it can.
    ///
    /// Keyed by name rather than by symbol, and kept out of [`Builder::types`]:
    /// a parameter wears the `'` an annotation's variable does, so it is not a
    /// name a bare word in a type could reach and has no business in the scope
    /// bare words are resolved against.
    params: HashMap<String, (Symbol, u32)>,
    /// What the a variable statements of the annotation being lowered
    /// declared, in the order they were written. Cleared for every written
    /// type, which is the whole scope of a declared variable: `..'r` twice in
    /// one annotation stands for one rest, and another annotation's `'r` is
    /// unrelated.
    ///
    /// The one scope a `where` clause establishes, and so the one table a
    /// name written in a type, in a `..`, in a `when` or in a formula is
    /// resolved against. One variable stands for one thing, and this is what
    /// says so. See [`ErrorKind::MixedTail`].
    vars: IndexMap<String, Declared>,
    /// How many variables have been declared anywhere in the program so far,
    /// which is what the next one's id is. Program-global, so two annotations
    /// that each write `a` never collide however alike they look — see
    /// [`Variable::id`].
    rigids: u32,
    /// Positive/negative uses of every declared-type parameter after the
    /// transparent alias graph reaches its least fixpoint. Bit 0 is covariant,
    /// bit 1 contravariant; both is invariant and neither is erased.
    variances: HashMap<Slot, u8>,
}

/// Every declaration in the bundle, split by what it declares and paired with
/// the module it was written in.
///
/// R12 in one value. Where a declaration is written — and which file it is in —
/// decides nothing about what can see it, so the tree is taken apart into three
/// lists before any name is bound, and each list is hoisted over the whole
/// bundle rather than over one file or one module. Each keeps the order it was
/// written in, depth-first through the modules, so a repeat is still reported
/// against the first and the reports come in the order a reader would meet
/// them.
#[derive(Default)]
struct Flat {
    types: Vec<(Option<Module>, TrackedString, Vec<TrackedString>, Annotated)>,
    effects: Vec<(
        Option<Module>,
        TrackedString,
        Vec<TrackedString>,
        EffectBody,
    )>,
    terms: Vec<Defined>,
    externs: Vec<External>,
    /// Shared term/extern source order, which preserves duplicate precedence.
    values: Vec<FlatValue>,
}

/// An alias declaration as written, waiting to be lowered on first use.
struct PendingAlias {
    module: Option<Module>,
    params: Vec<Param>,
    row: parse::EffectRow,
}

/// What an alias application comes to: the labels it stands for, each a
/// concrete effect applied to substituted arguments, and the tail it ends in.
#[derive(Default)]
struct Expansion {
    labels: Vec<EffectLabel>,
    tail: Option<Tail>,
}

#[derive(Debug, Clone, Copy)]
enum FlatValue {
    Term(usize),
    Extern(usize),
}

/// One `let`, as the parser read it and with the module it was written in: the
/// pattern it binds, the annotation it may wear, and its value. Named because
/// the tuple is the widest of the three and reads as noise inline.
type Defined = (Option<Module>, Box<parse::Pattern>, Option<Annotated>, Body);

/// One extern declaration, paired with the module it belongs to.
type External = (
    Option<Module>,
    TrackedString,
    Annotated,
    parse::ExternType,
    TrackedString,
);

enum DeclaredValue {
    Term {
        at: usize,
        symbols: Vec<Option<Symbol>>,
    },
    Extern {
        at: usize,
        symbol: Option<Symbol>,
    },
}

/// A written annotation, as the parser read it. Named so [`Flat`]'s rows fit on
/// a line apiece.
type Annotated = parse::Annotation;

/// One `effect` declaration's cases, as the parser read them.
type EffectBody = parse::EffectBody;

/// One definition's value, as the parser read it.
type Body = Tracked<Expr>;

/// One name a variable statement declared, while its own annotation is being
/// lowered.
#[derive(Debug)]
struct Declared {
    /// Where the name was written, which is what a repeat points back at and
    /// what an unused declaration is reported at.
    span: Span,
    id: u32,
    /// What the first use read it as, and where that use was. `None` until
    /// something uses it, which is what [`ErrorKind::UnusedVariable`] refuses.
    sense: Option<(Sense, Span)>,
    /// Whether a `when` in the type ever wore this name. A formula is written
    /// about presences a label carries, so a variable read as a presence by a
    /// clause alone is one no label gives a meaning to — see
    /// [`ErrorKind::UnboundPresence`].
    labelled: bool,
}

/// Which symbol a local name means.
///
/// Name resolution lives here rather than in the mint: the mint's job is to
/// make symbols unique, and this decides which name refers to which of them.
///
/// Locals only. A global is reached through the module it was declared in, and
/// which modules a name may be looked for in is a walk outward rather than a
/// position on a stack — see [`Builder::globals`].
#[derive(Debug, Default)]
struct Names {
    /// Most recent binding last. A lambda's arguments are pushed for the length
    /// of its body and released after it, which is the whole of the stack.
    bindings: Vec<Binding>,
}

#[derive(Debug)]
struct Binding {
    name: String,
    symbol: Symbol,
}

/// Where a written type is being lowered from: the body of a `type`
/// declaration, an operation's signature, or an annotation on a definition.
/// Only an annotation may be open; see [`ErrorKind::OpenDeclaredType`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    Declaration,
    /// An operation's signature inside an `effect` declaration. Held closed
    /// like a declaration's body, and told about it in its own words, because
    /// the `+` an operation may not carry is refused here and nowhere else.
    Operation,
    Annotation,
}

/// Which handler arm a `raise` written here would answer.
///
/// R17 in three values. A `raise` answers the innermost arm that lexically
/// encloses it, so lowering carries the answer down: an arm's body sets
/// [`Answering::Arm`], a `fn` body puts whatever it had behind it — a closure
/// can outlive the `handle` it was written in — and everything else passes it
/// on unchanged. Which is why a `raise` in the handled expression of a nested
/// `handle` inside an arm is legal: the outer handler is still on the stack
/// while the inner one runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answering {
    /// No handler arm encloses this point at all.
    Nowhere,
    /// An arm encloses it, and nothing has come between.
    Arm,
    /// An arm encloses it, but a `fn` lies between the two.
    UnderFn(Span),
}

/// Where one parameter sits: the declaration that binds it, and its position
/// in that declaration's list. The unit everything in [`kinds`] and
/// [`relevance`] is said about, since a parameter's own symbol says nothing
/// about what is handed to it.
type Slot = (Symbol, u32);

/// One thing a declaration's body says about one of its own parameters. See
/// [`constrain`], which reads them off a body, and [`kinds`], which resolves
/// the three into a kind apiece.
enum Fact {
    /// The parameter is used as this kind here. The kind carries the labels the
    /// row it tails writes out beside it, which are what an argument
    /// substituted for it may not name.
    Says(u32, ParamKind),
    /// The parameter is handed straight on to another declaration's slot, so
    /// it stands for whatever that slot stands for — and may not name whatever
    /// that slot may not name.
    Hands(u32, Slot),
    /// The parameter is the tail of a row written out as an argument, as `s` is
    /// in `WithX { y: Nat, ..'s }`. The row goes where the callee's own tail
    /// sat, so this tail inherits the callee's obligation as well as its own
    /// row's — but the *argument* is a row rather than the parameter, so this
    /// says nothing about which of the two readings anything has.
    Tails(u32, Slot),
}

/// Everything one body says about one of its own parameters, gathered before
/// anything is resolved. [`Fact::Hands`] is what crosses declarations and lives
/// in an edge map instead; these are what a slot says of itself.
#[derive(Debug, Default)]
struct Reading {
    /// Every way the parameter was read: as a whole type, or as the rest of a
    /// sum. One of them is a parameter that means something; both is the clash
    /// [`ErrorKind::MixedParameter`] reports, and the set is what lets the
    /// complaint name which two.
    ///
    /// Insertion-ordered, so the reading a body states first is the one a
    /// mixed parameter is displayed as and the one a complaint names first.
    senses: IndexSet<Sense>,
    /// The labels the rows it tails write out beside it.
    lacks: IndexSet<String>,
}

/// What [`kinds`] worked out for the whole table.
struct Kinds {
    /// A kind per parameter, in the declaration's own order.
    kinds: HashMap<Symbol, Vec<ParamKind>>,
    /// Every declaration with a parameter read two ways. Its body is erased
    /// and nothing is asked of the arguments written at it: what a mixed
    /// parameter means is exactly what could not be worked out, and checking a
    /// use site against a reading nobody could settle is the second complaint
    /// about the first mistake.
    mixed: IndexSet<Symbol>,
    /// One complaint per declaration that has to say which reading it meant —
    /// which is fewer than the declarations in `mixed`, since a declaration
    /// that merely hands a broken parameter on has nothing to fix.
    errors: Vec<Error>,
}

/// What a body turns out to be once every name in the way has been followed: a
/// shape one step in, one of the declaration's own parameters, or a loop that
/// reaches neither.
///
/// [`Stands::Param`] is a declaration's answer alone. A definition binds no
/// parameters, so [`Chain`] only ever reaches the other two.
#[derive(Debug, Clone, Copy)]
enum Stands {
    Shape,
    /// One of the declaration's own parameters, and whether reaching it went
    /// through a struct's fields.
    ///
    /// `type Id 'a = 'a` stands for its parameter outright, and
    /// `type WithX 'r = { x: Nat, ..'r }` stands for its parameter *with an `x`
    /// in front of it* — the fields are written beside the `..`, and the `..` is
    /// the constructor, so what the declaration stands for is the argument field_summaries
    /// them. That is the one step that tells [`ErrorKind::EndlessFields`] from
    /// [`ErrorKind::Circular`]: a loop with such a step on it adds a field every
    /// time round.
    Param {
        index: u32,
        fields: bool,
    },
    Loop,
}

/// One explicit evaluator step in [`Follow::decl`]. The recursion classifier
/// follows one chain, but imported forwarding chains can be arbitrarily deep,
/// so the chain lives here rather than on the native stack.
enum FollowWork<'a> {
    Decl(Symbol),
    Written(&'a Type),
    Semantic(&'a Rc<Ty>),
    SelectWritten(&'a [Type]),
    SelectSemantic(&'a [Rc<Ty>]),
    FinishSelect { fields: bool },
    FinishDecl(Symbol),
}

/// Following what every declaration stands for, once, remembering the loops
/// closed on the way. See [`ErrorKind::Circular`] for what a loop costs and
/// [`ErrorKind::EndlessFields`] for what the fielded kind costs.
struct Follow<'a> {
    types: &'a IndexMap<Symbol, Decl<Type>>,
    /// Imported declarations, whose semantic bodies can forward a local
    /// recursive occurrence through one of their arguments just as a local
    /// declaration can.
    external: &'a IndexMap<Symbol, ExternalType>,
    /// What each declaration was found to stand for. [`Stands::Loop`] is
    /// absorbing — anything that reduces into a loop never reaches a shape
    /// either — so a result reached under an open assumption is still the
    /// right one to keep.
    done: HashMap<Symbol, Stands>,
    /// The declarations being followed, outermost first. Meeting one again is
    /// the loop, and everything from it inwards is on that loop.
    open: Vec<Symbol>,
    /// How many fielded steps had been taken when each open declaration was
    /// pushed. The walk is one chain — a struct answers without descending, and
    /// an application descends into exactly one argument — so every step counted
    /// since a frame was pushed is a step on the path from it, and a loop is
    /// endless exactly when that count moved.
    ///
    /// Counted rather than flagged per frame because the count only ever grows:
    /// nothing is unwound, so the mark taken at the push is the whole of what a
    /// frame has to remember.
    marks: Vec<u32>,
    /// How many fielded steps the walk has taken. See [`Follow::marks`].
    fielded: u32,
    /// Every declaration found to be on a loop, in the order they were found.
    looping: IndexSet<Symbol>,
    /// Those of them whose loop had a fielded step on it, which is the loop that
    /// never runs out of fields rather than the one that reaches no shape.
    endless: IndexSet<Symbol>,
}

/// Every declaration on a loop, split by what kind of loop it is. See
/// [`looping`].
struct Loops {
    /// Every declaration on a loop of any kind, in the order the loops were
    /// found.
    looping: IndexSet<Symbol>,
    /// Those on a loop with a fielded step on it. A subset of `looping`, and the
    /// ones told about [`ErrorKind::EndlessFields`] rather than
    /// [`ErrorKind::Circular`].
    endless: IndexSet<Symbol>,
}

/// A pattern that cannot fail: a bare name, `()`, or a struct pattern all of
/// whose fields cannot fail either. What a `let` accepts — read off a
/// normalized [`Pattern`] by [`calm`], which answers the refutability question
/// in the type: a walk over one of these never meets a test it would have to
/// call unreachable.
#[derive(Debug, Clone)]
enum Calm {
    /// Binds the whole value.
    Bind(Tracked<Symbol>),
    /// `_`: binds nothing and constrains nothing. The value is still held —
    /// by a fresh definition nothing can name — so it is still typechecked;
    /// see [`Builder::destructure_stmt`] and R5/R6 of the wildcard spec.
    Wildcard(Span),
    /// Binds nothing, and constrains the value to unit. What `()` reads as,
    /// and `{}` too: the two are one pattern — exactly no fields.
    Unit(Span),
    /// Reaches into fields, each with a pattern that cannot fail. Exact
    /// without the `..` — the value has these fields and no others, a demand
    /// [`Builder::destructure`] writes as an annotation — and open with it,
    /// asking only for the fields it names.
    Struct {
        span: Span,
        fields: Vec<(TrackedString, Calm)>,
        rest: Option<Span>,
    },
    /// `[..r]` or `[..]`: constrains the value to an array and binds the whole
    /// of it to the name, when one was written. The one array pattern that
    /// cannot fail, since it names no element for a length to fall short of.
    ArrayRest {
        span: Span,
        name: Option<Tracked<Symbol>>,
    },
}

/// One step from a position of the scrutinee to a position inside it: into a
/// struct's field, or into the payload of one tag. A path of these names a
/// *position* of a match — the unit R7 types column-wise and the matrix
/// algorithm draws values from — and the payload step carries its tag because
/// different cases carry different payloads: `#A`'s payload and `#B`'s
/// are two positions, not one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Step {
    Field(String),
    Payload(String),
    /// Into an array's elements — every one of them at once. One position
    /// rather than one per index, because an array's elements are of one
    /// type, and a pattern's third element from the front and another's
    /// second from the back may be the same element of some array: nothing
    /// syntactic tells them apart, so nothing here pretends to.
    Element,
}

/// What one match's written arms say, position by position: the tags and
/// primitive literals tested at each, and where the arms stop testing. The
/// universes the matrix checks draw values from — usefulness in the sense of
/// Maranget (JFP 2007), with each tag position's universe being its listed
/// cases when no arm is irrefutable there, its listed cases plus "anything
/// else" when one is, Boolean's universe being `false` and `true`, and every
/// other primitive's universe being infinite.
///
/// Syntactic throughout: built from the normalized patterns and consulting no
/// types. What inference reads: whether a position's row stays open
/// ([`Matrix::open`]) and whether earlier arms fully handle a case at a
/// position ([`Matrix::handled`], which is what refines a binder's view). The
/// typed exhaustiveness and reachability checks moved to
/// [`patterns`](crate::patterns), which reads the solved types this analysis
/// shaped — one implementation of the closure rule, read from two ends.
pub(crate) struct Matrix {
    /// The tags and primitive literals tested at each position, over every
    /// arm.
    tests: HashMap<Vec<Step>, Tests>,
    /// Every position an identifier pattern sits at. A position is open —
    /// its universe holds "anything else" — exactly when one of these is a
    /// prefix of it: an arm irrefutable at a position is one whose walk meets
    /// a binder at or before it.
    binds: Vec<Vec<Step>>,
}

/// The tests one position has seen, over every arm. Both kinds are collected —
/// a mixed position types as the mismatch it is, and the universes here have
/// to hold whatever was written.
#[derive(Debug, Default)]
struct Tests {
    tags: IndexSet<String>,
    literals: IndexSet<Literal>,
}

/// One cell of the matrix the usefulness walk works on: a [`Pattern`] with
/// everything that only binds flattened to the one wildcard it matches as.
/// `()` and a bare name accept every value alike — what they demand of the
/// type is typing's business, and the check is syntactic — and a bare tag's
/// payload arrives as [`Mat::Wild`] the same way.
///
/// Owned rather than borrowed, because the walk builds rows no pattern wrote:
/// specializing a row appends its payload or its fields, and the question
/// [`Matrix::handled`] asks arrives as a synthetic row built from a path.
#[derive(Debug, Clone)]
enum Mat {
    /// Accepts everything: a binder, `()`, a bare tag's payload.
    Wild,
    Tag {
        name: String,
        payload: Box<Mat>,
    },
    Literal(Literal),
    /// A struct pattern with at least one field. `{}` tests nothing and
    /// arrives as [`Mat::Wild`] instead, so a struct cell always has a field
    /// for the widening step to find.
    Struct(Vec<(String, Mat)>),
    /// An array pattern naming an element: a test of the value's length that
    /// this syntactic walk does not reason about. The cell matches no
    /// specialization and so never counts toward handling a case, which errs
    /// toward leaving a binder's view unrefined — the sound side. `[..r]`
    /// tests nothing and arrives as [`Mat::Wild`].
    Array,
}

/// Where a lowered pattern's binder symbols come from.
///
/// A pattern in an expression mints its own locals and puts them in scope for
/// the body being lowered. A pattern on a top-level `let` binds names the
/// declare pass already declared — declared before any body was read, which is
/// what hoists them — so lowering consumes that list instead, in the same
/// order the declare pass walked.
enum Binders {
    /// Mint a local per name; bind everything that is not a repeat.
    Local,
    /// The declare pass's symbols, in pattern order. `None` is a name that
    /// bound nothing — a repeat — and lowering mints an unbound stand-in so
    /// the walk stays total.
    Declared(std::vec::IntoIter<Option<Symbol>>),
}

/// [`Follow`] about definitions: following what each one's value stands for,
/// once, remembering the loops closed on the way.
///
/// The twin rather than the same walk, because the two languages are different
/// enough that one walk over both would be a match on which it was in every
/// arm. What they share is the shape: a memo, a stack of what is still open,
/// and the set of everything found on a loop.
struct Chain<'a> {
    terms: &'a IndexMap<Symbol, Decl<Term>>,
    /// The value each nested `let` in the program binds its name to.
    ///
    /// A local symbol is no definition, so the table above has no key for it —
    /// and a nested `let` given as itself is the same mistake a definition
    /// given as itself is, so the walk has to be able to follow one. Collected
    /// over the whole program before anything is followed, for the reason
    /// definitions are hoisted: which of the two maps a name comes out of is
    /// not something the walk should have to reach the binder to know.
    locals: HashMap<Symbol, &'a Term>,
    /// What each definition was found to stand for; [`Stands::Loop`] absorbs,
    /// exactly as it does for [`Follow`].
    done: HashMap<Symbol, Stands>,
    /// The definitions being followed, outermost first.
    open: Vec<Symbol>,
    /// Every definition found to be on a loop, in the order they were found.
    looping: IndexSet<Symbol>,
}

impl TypeField {
    /// Where the label was written: the name of an ordinary field, the whole
    /// `\name` of an absent one.
    pub fn name_span(&self) -> Span {
        match self {
            TypeField::Written { name_span, .. } | TypeField::Absent { name_span } => *name_span,
        }
    }

    /// The written type, when the field has one. An absent label writes none,
    /// which is what lets every walk over a struct's fields visit exactly the
    /// types that are there.
    pub fn value(&self) -> Option<&Type> {
        match self {
            TypeField::Written { value, .. } => Some(value),
            TypeField::Absent { .. } => None,
        }
    }
}

impl EffectLabel {
    /// Where the label was written: the name of a performed effect, the whole
    /// `\!Log` of one written absent.
    pub fn name_span(&self) -> Span {
        match self {
            EffectLabel::Written { name_span, .. } | EffectLabel::Absent { name_span, .. } => {
                *name_span
            }
        }
    }

    /// The effect this label resolved to. Always one: a name that resolved to
    /// nothing is dropped from the row where it was written, so nothing here
    /// can be a dangling symbol.
    pub fn symbol(&self) -> Symbol {
        match self {
            EffectLabel::Written { symbol, .. } | EffectLabel::Absent { symbol, .. } => *symbol,
        }
    }

    /// Whether an alias put this label here rather than the reader. See
    /// [`EffectLabel::Written::expanded`].
    pub fn expanded(&self) -> bool {
        match self {
            EffectLabel::Written { expanded, .. } | EffectLabel::Absent { expanded, .. } => {
                *expanded
            }
        }
    }

    /// The arguments the effect is applied to, one per parameter it declares.
    pub fn args(&self) -> &[Type] {
        match self {
            EffectLabel::Written { args, .. } | EffectLabel::Absent { args, .. } => args,
        }
    }

    /// [`args`](Self::args), to substitute into or re-key in place.
    pub fn args_mut(&mut self) -> &mut Vec<Type> {
        match self {
            EffectLabel::Written { args, .. } | EffectLabel::Absent { args, .. } => args,
        }
    }

    /// The `when` clause the label wears, when it wears one. An absent label
    /// never does: `\!Log` says the effect is not performed outright.
    pub fn when(&self) -> Option<&When> {
        match self {
            EffectLabel::Written { when, .. } => when.as_deref(),
            EffectLabel::Absent { .. } => None,
        }
    }
}

impl SumCase {
    /// Where the label was written: the [`TypeField::name_span`] of a case.
    pub fn name_span(&self) -> Span {
        match self {
            SumCase::Written { name_span, .. } | SumCase::Absent { name_span } => *name_span,
        }
    }

    /// The written payload, when the case has one — which an absent case never
    /// does, any more than a case written bare.
    pub fn payload(&self) -> Option<&Type> {
        match self {
            SumCase::Written { payload, .. } => payload.as_ref(),
            SumCase::Absent { .. } => None,
        }
    }
}

impl TermKind {
    fn with_span(self, span: Span) -> Term {
        Term {
            ty: Default::default(),
            span,
            kind: self,
        }
    }
}

pub fn build(mint: &mut Mint, stmts: Vec<Stmt>) -> Output {
    build_with_dependencies(mint, stmts, &[])
}

/// Build IR with direct dependency artifact headers visible as root-qualified
/// source namespaces. Artifact bodies remain linked implementation details;
/// only declarations in these direct headers are installed in the resolver.
pub fn build_with_dependencies(
    mint: &mut Mint,
    stmts: Vec<Stmt>,
    dependencies: &[artifact::Artifact],
) -> Output {
    build_with_dependency_graph(mint, stmts, dependencies, &[])
}

/// A direct artifact and the valid source identifier used to qualify it.
///
/// The alias is deliberately separate from the artifact identity: bundle names
/// may contain `-`, while source path components may not.
#[derive(Debug, Clone, Copy)]
pub struct DependencyImport<'a> {
    pub alias: &'a str,
    pub artifact: &'a artifact::Artifact,
}

/// Build against explicitly aliased direct dependencies and linked interfaces.
pub fn build_with_dependency_imports(
    mint: &mut Mint,
    stmts: Vec<Stmt>,
    dependencies: &[DependencyImport<'_>],
    linked: &[artifact::Artifact],
) -> Output {
    build_with_dependency_imports_inner(mint, stmts, dependencies, linked)
}

/// Build against direct source-visible dependencies and additional linked
/// implementation interfaces referenced by those dependencies.
pub fn build_with_dependency_graph(
    mint: &mut Mint,
    stmts: Vec<Stmt>,
    dependencies: &[artifact::Artifact],
    linked: &[artifact::Artifact],
) -> Output {
    let imports: Vec<_> = dependencies
        .iter()
        .map(|artifact| DependencyImport {
            alias: &artifact.header().identity.name,
            artifact,
        })
        .collect();
    build_with_dependency_imports_inner(mint, stmts, &imports, linked)
}

fn build_with_dependency_imports_inner(
    mint: &mut Mint,
    stmts: Vec<Stmt>,
    dependencies: &[DependencyImport<'_>],
    linked: &[artifact::Artifact],
) -> Output {
    let mut b = Builder {
        mint,
        module: None,
        errors: Vec::new(),
        terms: Names::default(),
        globals: HashMap::new(),
        modules: HashMap::new(),
        std_prelude: None,
        expanded: HashMap::new(),
        operations: HashMap::new(),
        answering: Answering::Nowhere,
        arities: HashMap::new(),
        pending_aliases: HashMap::new(),
        alias_bodies: HashMap::new(),
        expanding: Vec::new(),
        cyclic: HashSet::new(),
        imported_aliases: HashSet::new(),
        imported_effect_params: HashMap::new(),
        params: HashMap::new(),
        vars: IndexMap::new(),
        rigids: 0,
        variances: HashMap::new(),
    };
    let mut program = Program {
        externs: IndexMap::new(),
        terms: IndexMap::new(),
        types: IndexMap::new(),
        external_names: IndexMap::new(),
        external_schemes: IndexMap::new(),
        external_types: IndexMap::new(),
        external_operations: IndexMap::new(),
        effect_params: IndexMap::new(),
        effects: IndexMap::new(),
        effect_ids: IndexMap::new(),
        groups: Vec::new(),
    };
    b.import_dependencies(dependencies, linked, &mut program);
    // The whole tree flattened before anything is declared: every module is
    // minted, over every file, and what each remaining statement is written in
    // is recorded beside it. Nothing is resolved on the way, so a module may be
    // named from above its own declaration and from another file.
    let mut flat = Flat::default();
    b.flatten(stmts, &mut flat);
    // Every type's name is bound before any type's body is read, so a type can
    // name itself and two types can name each other. That is the whole of what
    // makes a recursive type writable: nothing downstream ties the knot, and
    // nothing downstream can, so a type is recursive exactly when a
    // declaration says it is. Names are bound in the order they were written,
    // so a repeated one is still reported against the first.
    let declared: Vec<_> = flat
        .types
        .iter()
        .map(|(module, name, _, _)| {
            b.module = *module;
            b.declare(Scope::Types, name)
        })
        .collect();
    // Every declaration's parameters, minted before any body is read, and how
    // many arguments each declaration therefore takes. Knowing the count above
    // the declaration itself is what makes a forward reference applicable:
    // `type A = B Nat` above `type B 'x = ...` has to be counted, and counting
    // it cannot wait for `B` to be lowered.
    let bound: Vec<Vec<Param>> = flat
        .types
        .iter()
        .map(|(module, _, params, _)| {
            b.module = *module;
            b.declare_params(params)
        })
        .collect();
    b.arities.extend(
        declared
            .iter()
            .zip(&bound)
            .filter_map(|(symbol, params)| Some(((*symbol)?, params.len()))),
    );
    // Effects next, and before any type body: a `type` declaration may name one
    // in an arrow it writes, and an alias written anywhere has to be expandable
    // wherever a row mentions it. Their names are bound before any of their own
    // cases are read, for the reason a type's is — two effects may name each
    // other, and an operation's signature may name a type declared below it.
    let named: Vec<_> = flat
        .effects
        .iter()
        .map(|(module, name, params, _)| {
            b.module = *module;
            let symbol = b.declare(Scope::Effects, name);
            // The parameters are bound before any body is lowered, so that an
            // application can be counted wherever it appears — in an
            // operation's signature above the declaration it names included.
            let params = b.declare_params(params);
            if let Some(symbol) = symbol {
                b.arities.insert(symbol, params.len());
            }
            (symbol, params)
        })
        .collect();
    let mut aliases = Vec::new();
    for ((symbol, params), (module, name, _, cases)) in named.into_iter().zip(flat.effects) {
        b.module = module;
        let value = match cases {
            // An alias body is read when something first names it, which
            // may be an operation signature above it. Its declaration takes
            // its place in the table now, so the table keeps the order the
            // effects were written in, and is filled in below.
            parse::EffectBody::Alias(row) => {
                if let Some(symbol) = symbol {
                    b.pending_aliases.insert(
                        symbol,
                        PendingAlias {
                            module,
                            params: params.clone(),
                            row,
                        },
                    );
                    aliases.push(symbol);
                }
                Effect::Alias(Alias {
                    body: AliasBody::default(),
                    expanded: name.span.track(TypeKind::Error),
                })
            }
            cases => {
                // In scope for the length of the body, the way a type's are.
                b.scope_params(&params);
                let value = b.effect(cases);
                b.params.clear();
                value
            }
        };
        if let Some(symbol) = symbol {
            program.effects.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation: None,
                    params,
                    value,
                },
            );
        }
    }
    // Every alias body, lowered if nothing has asked for it yet, and expanded
    // over its own parameters: what the alias says of them is what any use
    // says of the arguments, so that is the row its parameters are read off.
    for symbol in aliases {
        let params = program.effects[&symbol].params.clone();
        let value = b.declared_alias(symbol, &params);
        program.effects[&symbol].value = Effect::Alias(value);
    }
    // What each effect name stands for, once every declaration is in: itself,
    // or — for an alias — the effects it names, through however many aliases it
    // takes to reach them.
    b.expanded = expansions(b.mint, &program.effects, &b.expanded, &b.cyclic);
    b.operations.extend(
        program
            .effects
            .iter()
            .filter_map(|(symbol, decl)| match &decl.value {
                Effect::Operations(operations) => {
                    Some((*symbol, operations.keys().cloned().collect()))
                }
                Effect::Alias(_) => None,
            }),
    );
    for ((symbol, (module, name, _, body)), params) in
        declared.into_iter().zip(flat.types).zip(bound)
    {
        b.module = module;
        // The parameters are in scope for the length of the body and released
        // after it, the way a lambda's argument is — this is the type
        // language's only binder, and its only scope.
        b.scope_params(&params);
        // A declaration's body is read as an annotation so that a `where`
        // written there can be refused rather than misparsed; what survives is
        // the type, since a refused clause is dropped.
        let value = b.written(body, Place::Declaration).ty;
        b.params.clear();
        if let Some(symbol) = symbol {
            program.types.insert(
                symbol,
                Decl {
                    name_span: name.span,
                    annotation: None,
                    params,
                    value,
                },
            );
        }
    }
    // A loop of bare names is the one recursion that cannot be allowed, and it
    // is what mutual visibility just made writable. See [`ErrorKind::Circular`]
    // for why it means nothing, and [`Solve::unify`](crate::inference) for what
    // it would cost the solver to be handed one.
    // Read back off the table in declaration order, so the reports come in the
    // order the reader wrote them rather than the order the loops were found.
    let Loops { looping, endless } = looping(&program.types, &program.external_types);
    let circular: Vec<_> = program
        .types
        .keys()
        .copied()
        .filter(|symbol| looping.contains(symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        // Two loops and one erasure. A loop with a struct's `..` on it reaches a
        // shape every time round and adds a field doing it, which is a different
        // thing gone wrong from a loop that reaches no shape at all. See
        // [`ErrorKind::EndlessFields`].
        let kind = match endless.contains(&symbol) {
            true => ErrorKind::EndlessFields,
            false => ErrorKind::Circular {
                namespace: Namespace::Types,
            },
        };
        b.error(span, kind);
    }
    // The other recursion the solver cannot be handed: one that builds a bigger
    // argument on the way round. See [`ErrorKind::GrowingRecursion`].
    for (symbol, at) in growing(&program.types) {
        let decl = &mut program.types[&symbol];
        let span = decl.value.span;
        decl.value = span.track(TypeKind::Error);
        b.error(at, ErrorKind::GrowingRecursion);
    }
    // What each effect's parameters stand for is part of the effect's
    // identity, and the lacks a parameter carries are spelled with identities:
    // so the readings are taken once before identities exist, with rows keyed
    // provisionally, and the whole fixpoint runs again below once they do.
    // Only the senses are kept from this pass; its complaints are the second
    // pass's to make.
    let senses = kinds(&program.types, &program.effects, &program.external_types);
    for (symbol, decl) in program.effects.iter_mut() {
        if let Some(kinds) = senses.kinds.get(symbol) {
            for (param, kind) in decl.params.iter_mut().zip(kinds) {
                param.kind = kind.clone();
            }
        }
    }
    // Type declarations are complete, so their effect rows can now be keyed
    // by normalized operation interfaces before parameter-kind analysis reads
    // their lacks sets. Terms are re-keyed after they are lowered below.
    structuralize_effects(&mut program, b.mint, &b.expanded, &mut b.errors);
    // Alias applications are transparent for annotation polarity. Compute the
    // least variance fixpoint only after every local/imported body is available;
    // this terminates for recursive and mutually-recursive aliases because each
    // slot can gain only the positive and negative bits.
    b.variances = declaration_variances(&program.types, &program.external_types);
    // What each parameter stands for, which only the finished bodies can say: a
    // parameter handed straight on to another declaration takes its kind from
    // there, so no one body decides its own.
    let Kinds {
        mut kinds,
        mixed,
        errors: clashes,
    } = kinds(&program.types, &program.effects, &program.external_types);
    b.errors.extend(clashes);
    for (symbol, kinds) in &kinds {
        let params = match program.types.get_mut(symbol) {
            Some(decl) => &mut decl.params,
            None => &mut program.effects[symbol].params,
        };
        for (param, kind) in params.iter_mut().zip(kinds) {
            param.kind = kind.clone();
        }
    }
    // A declaration whose parameter could not be read one way is erased, the
    // way a circular one is, and dropped from the table the use sites are
    // checked against. Both halves are the absorbing: a body left standing
    // would put whatever a use site handed it into a tail, which is the one
    // thing nothing downstream can recover from, and a slot left in the table
    // would complain at every ordinary type written at it — three complaints
    // about code the reader got right, for one mistake somewhere else.
    // An effect read two ways keeps its operations — a handler still has to
    // cover them — and is dropped from the table the same way, so its
    // applications are not checked against a reading it never had.
    for symbol in &mixed {
        if let Some(decl) = program.types.get_mut(symbol) {
            let span = decl.value.span;
            decl.value = span.track(TypeKind::Error);
        }
        kinds.remove(symbol);
    }
    // What each effect's parameters stand for, for inference to mint fresh
    // arguments from. Imported effects declare their kinds in their headers;
    // until they carry any, they take none.
    for (symbol, decl) in &program.effects {
        program.effect_params.insert(
            *symbol,
            decl.params.iter().map(|param| param.kind.clone()).collect(),
        );
    }
    for (symbol, kinds) in &b.imported_effect_params {
        program.effect_params.insert(*symbol, kinds.clone());
    }
    for symbol in program.effect_ids.keys() {
        program.effect_params.entry(*symbol).or_default();
    }
    // Imported constructors impose exactly the same row-shape and lacks
    // conditions at a use site as local constructors. Their kinds came from
    // the artifact header rather than the local fixpoint above.
    kinds.extend(
        program
            .external_types
            .iter()
            .map(|(symbol, declaration)| (*symbol, declaration.params.clone())),
    );
    // Every definition's name is bound before any definition's body is read —
    // the hoist the `type` half above already gets, and for the same reason.
    // That is the whole of what makes a definition able to name itself and two
    // definitions able to name each other; nothing downstream ties the knot,
    // so a definition is recursive exactly when its body says so. Names are
    // bound in the order they were written, so a repeated one is still
    // reported against the first, and the first is still the one that stands.
    //
    // A pattern declares every name it binds, in the order the pattern walk
    // meets them, so the names a struct-pattern `let` takes apart are hoisted
    // like any other definition. A name a pattern repeats binds nothing here —
    // the lowering walk reports it as the pattern mistake it is, not as a
    // second definition.
    // Terms and externs share the same declaration pass, in written order:
    // a duplicate belongs to whichever one appeared first, just as two lets do.
    let declared: Vec<DeclaredValue> = flat
        .values
        .iter()
        .map(|value| match value {
            FlatValue::Term(at) => {
                let (module, pattern, _, _) = &flat.terms[*at];
                b.module = *module;
                let mut names = Vec::new();
                pattern_names(pattern, &mut names);
                let mut seen: Vec<String> = Vec::new();
                DeclaredValue::Term {
                    at: *at,
                    symbols: names
                        .iter()
                        .map(|name| {
                            if seen.contains(&name.tracked) {
                                return None;
                            }
                            seen.push(name.tracked.clone());
                            b.declare(Scope::Terms, name)
                        })
                        .collect(),
                }
            }
            FlatValue::Extern(at) => {
                let (module, name, _, _, _) = &flat.externs[*at];
                b.module = *module;
                DeclaredValue::Extern {
                    at: *at,
                    symbol: b.declare(Scope::Terms, name),
                }
            }
        })
        .collect();
    for value in declared {
        match value {
            DeclaredValue::Extern { at, symbol } => {
                let (module, name, annotation, abi, target) = flat.externs[at].clone();
                b.module = module;
                let annotation = b.written(annotation, Place::Annotation);
                let runtime_array_primitive = array_intrinsic_signature(
                    target.tracked.as_str(),
                    &annotation.ty,
                    &program.types,
                );
                if !runtime_array_primitive
                    && type_contains_array(&annotation.ty, &program.types, &program.external_types)
                {
                    b.error(annotation.ty.span, ErrorKind::ArrayInExtern);
                }
                // Resolve the ABI against the annotation we just lowered. Its
                // leaves are clones of the same parsed types, so lowering them
                // independently would duplicate diagnostics and, more subtly,
                // mint different annotation-variable identities.
                let abi = resolved_extern_type(abi, &annotation.ty);
                if let Some(symbol) = symbol {
                    program.externs.insert(
                        symbol,
                        Decl {
                            name_span: name.span,
                            annotation: Some(annotation),
                            params: Vec::new(),
                            value: Extern {
                                target,
                                abi,
                                array_intrinsic: runtime_array_primitive,
                            },
                        },
                    );
                }
            }
            DeclaredValue::Term {
                at,
                symbols: declared,
            } => {
                let (module, pattern, ty, body) = flat.terms[at].clone();
                b.module = module;
                match pattern.tracked {
                    parse::PatternKind::Ident { name } => {
                        let symbol = declared
                            .into_iter()
                            .next()
                            .expect("a bare name declares one symbol");
                        let annotation = ty.map(|ty| b.written(ty, Place::Annotation));
                        let value = b.term(body.tracked);
                        if let Some(symbol) = symbol {
                            program.terms.insert(
                                symbol,
                                Decl {
                                    name_span: name.span,
                                    annotation,
                                    params: Vec::new(),
                                    value,
                                },
                            );
                        }
                    }
                    tracked => {
                        let pspan = pattern.span;
                        let pattern = pspan.track(tracked);
                        let annotation = ty.map(|ty| b.written(ty, Place::Annotation));
                        let value = b.term(body.tracked);
                        let mut binders = Binders::Declared(declared.into_iter());
                        let mut seen = Vec::new();
                        let mut dropped = Vec::new();
                        let pattern = b.pattern(pattern, &mut seen, &mut binders, &mut dropped);
                        match calm(&pattern) {
                            Some(calm) => {
                                b.destructure_stmt(calm, annotation, value, &mut program.terms)
                            }
                            None => {
                                let (at, found) = refuter(&pattern)
                                    .expect("a pattern that is not calm names what refutes it");
                                b.error(at, ErrorKind::RefutableBinding { found });
                                let held = b.fresh("%value", pspan);
                                program.terms.insert(
                                    held.tracked,
                                    Decl {
                                        name_span: pspan,
                                        annotation,
                                        params: Vec::new(),
                                        value,
                                    },
                                );
                                let mut names = Vec::new();
                                pattern_binders(&pattern, &mut names);
                                dropped.extend(names);
                            }
                        }
                        for name in dropped {
                            program.terms.insert(
                                name.tracked,
                                Decl {
                                    name_span: name.span,
                                    annotation: None,
                                    params: Vec::new(),
                                    value: TermKind::Error.with_span(name.span),
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    // The term half of the loop refused above, and refused for the same
    // reason: a value given as a name that leads back to itself is never given
    // one. Read back off the table in definition order, so the reports come in
    // the order the reader wrote them.
    let circling = circling(&program.terms);
    // The nested bindings on a loop first, since a definition that is on one
    // too has its whole value erased below and would take them with it. Each
    // is reported where the top-level walk reports one: at the value's span.
    let mut nested_loops = Vec::new();
    for decl in program.terms.values_mut() {
        erase_circular(&mut decl.value, &circling, &mut nested_loops);
    }
    for span in nested_loops {
        b.error(
            span,
            ErrorKind::Circular {
                namespace: Namespace::Terms,
            },
        );
    }
    let circular: Vec<_> = program
        .terms
        .keys()
        .copied()
        .filter(|symbol| circling.contains(symbol))
        .collect();
    for symbol in circular {
        let decl = &mut program.terms[&symbol];
        let span = decl.value.span;
        decl.value = TermKind::Error.with_span(span);
        b.error(
            span,
            ErrorKind::Circular {
                namespace: Namespace::Terms,
            },
        );
    }
    // Terms were lowered after the type declarations above; give their rows
    // the identities already computed before inference sees them. Handler
    // coverage is finalized here too, now that nominal declarations can be
    // grouped by those identities.
    // Keep coverage keyed by the same structural identities as effect rows.
    // `Builder::operations` is the complete selector registry assembled while
    // resolving terms: imports install their artifact selectors there before
    // local declarations extend it. Borrow it rather than reconstructing a
    // local-only view from `Program::effects`; this also leaves ownership of
    // the builder and its error accumulator explicit during rekeying below.
    //
    // Seed every known identity so indexing remains total even for recovery
    // identities which declare no usable operations. Structurally equivalent
    // declarations merge their selector sets, whether they came from source,
    // dependencies, or both.
    let mut operation_sets: HashMap<EffectId, IndexSet<OperationSelector>> = program
        .effect_ids
        .values()
        .cloned()
        .map(|identity| (identity, IndexSet::new()))
        .collect();
    // Walk the identity table rather than the hash-based selector registry so
    // a partial-handler diagnostic keeps declaration order. Imported identities
    // precede locals, and each selector set itself preserves interface order.
    for (symbol, identity) in &program.effect_ids {
        if let Some(selectors) = b.operations.get(symbol) {
            operation_sets
                .entry(identity.clone())
                .or_default()
                .extend(selectors.iter().cloned());
        }
    }
    for decl in program.terms.values_mut() {
        if let Some(annotation) = &mut decl.annotation {
            rekey_type(&mut annotation.ty, &program.effect_ids, &mut b.errors);
        }
        rekey_term(
            &mut decl.value,
            &program.effect_ids,
            &operation_sets,
            &mut b.errors,
        );
    }
    for decl in program.externs.values_mut() {
        let annotation = decl
            .annotation
            .as_mut()
            .expect("an extern always has a written annotation");
        rekey_type(&mut annotation.ty, &program.effect_ids, &mut b.errors);
    }
    // Which definitions have to be typed together, read off the values as they
    // finally stand — so a refused loop is a group of one naming nobody rather
    // than the group its erased value used to describe.
    program.groups = grouping(&program.terms);
    // What the arguments handed to a row parameter are allowed to be. Last of
    // all, because an annotation is as much a place to write one as a
    // declaration's body is, and annotations are only just lowered.
    b.errors.extend(row_arguments(&mut program, &kinds));
    // Which parameters survive unfolding, read off the bodies as they finally
    // stand: every erasure above is a position a parameter no longer reaches,
    // and calling one relevant that nothing keeps would let the solver decide
    // by a name what unfolding decides otherwise.
    let relevant = relevance(&program.types);
    for (symbol, decl) in program.types.iter_mut() {
        for (index, param) in decl.params.iter_mut().enumerate() {
            param.relevant = relevant.contains(&(*symbol, index as u32));
        }
    }
    // Every complaint in the order the reader would meet it. The passes above
    // do not run in source order and cannot — what a parameter stands for is
    // not known until every body is in, and what a row argument may be is not
    // known until every annotation is — so the whole list is put back in the
    // one place that has all of it. The sort is stable, so two complaints about
    // one span keep the order the passes found them in.
    //
    // Said here rather than left to each reporter: the debugger sorts its
    // diagnostics for its own reasons, and a driver that prints them in the
    // order they arrive should not be the only one telling the reader about
    // line 3 before line 2.
    b.errors.sort_by_key(|error| error.span.start);
    Output {
        program,
        errors: b.errors,
    }
}

/// The only array-bearing host signatures accepted by lowering. This runs
/// after name resolution, so primitive and structural identities—not
/// shadowable source spellings—decide whether the declaration is intrinsic.
fn array_intrinsic_signature(
    target: &str,
    root: &Type,
    declarations: &IndexMap<Symbol, Decl<Type>>,
) -> bool {
    fn arrow(ty: &Type) -> Option<(&Type, &Type)> {
        match &ty.tracked {
            TypeKind::Arrow { from, to, effects }
                if effects.effects.is_empty() && effects.tail.is_none() =>
            {
                Some((from, to))
            }
            _ => None,
        }
    }

    fn variable(ty: &Type) -> Option<&str> {
        match &ty.tracked {
            TypeKind::Var(name) => Some(name.as_str()),
            _ => None,
        }
    }

    fn array_variable(ty: &Type) -> Option<&str> {
        match &ty.tracked {
            TypeKind::Array(element) => variable(element),
            _ => None,
        }
    }

    fn natural(ty: &Type) -> bool {
        matches!(ty.tracked, TypeKind::Prim(crate::types::Prim::Nat))
    }

    /// The two halves of a written pair `(A, B)`: the closed positional
    /// struct a tuple type lowers to, with no `when` on either field.
    fn pair(ty: &Type) -> Option<(&Type, &Type)> {
        let TypeKind::Struct { fields, tail: None } = &ty.tracked else {
            return None;
        };
        match (fields.len(), fields.get("0"), fields.get("1")) {
            (
                2,
                Some(TypeField::Written {
                    when: None,
                    value: first,
                    ..
                }),
                Some(TypeField::Written {
                    when: None,
                    value: second,
                    ..
                }),
            ) => Some((first, second)),
            _ => None,
        }
    }

    fn option<'a>(ty: &'a Type, declarations: &IndexMap<Symbol, Decl<Type>>) -> Option<&'a Type> {
        let TypeKind::Apply { head, args, .. } = &ty.tracked else {
            return None;
        };
        let declaration = declarations.get(head)?;
        let TypeKind::Sum { cases, tail: None } = &declaration.value.tracked else {
            return None;
        };
        let some = cases.get("Some")?;
        let none = cases.get("None")?;
        let option_shape = declaration.params.len() == 1
            && cases.len() == 2
            && matches!(
                some,
                SumCase::Written {
                    when: None,
                    payload: Some(payload),
                    ..
                } if matches!(payload.tracked, TypeKind::Param { index: 0, .. })
            )
            && matches!(
                none,
                SumCase::Written {
                    when: None,
                    payload: None,
                    ..
                }
            );
        (args.len() == 1 && option_shape).then(|| &args[0])
    }

    let Some((array, rest)) = arrow(root) else {
        return false;
    };
    let Some(element) = array_variable(array) else {
        return false;
    };
    match target {
        "$arrayLen" => natural(rest),
        "$arrayGet" => {
            let Some((index, result)) = arrow(rest) else {
                return false;
            };
            natural(index) && option(result, declarations).and_then(variable) == Some(element)
        }
        "$arraySet" => {
            let Some((index, rest)) = arrow(rest) else {
                return false;
            };
            let Some((value, result)) = arrow(rest) else {
                return false;
            };
            natural(index)
                && variable(value) == Some(element)
                && option(result, declarations).and_then(array_variable) == Some(element)
        }
        "$arrayPush" | "$arrayPrepend" => {
            let Some((value, result)) = arrow(rest) else {
                return false;
            };
            variable(value) == Some(element) && array_variable(result) == Some(element)
        }
        "$arrayConcat" => {
            let Some((other, result)) = arrow(rest) else {
                return false;
            };
            array_variable(other) == Some(element) && array_variable(result) == Some(element)
        }
        "$arraySlice" => {
            let Some((from, rest)) = arrow(rest) else {
                return false;
            };
            let Some((to, result)) = arrow(rest) else {
                return false;
            };
            natural(from)
                && natural(to)
                && option(result, declarations).and_then(array_variable) == Some(element)
        }
        "$arrayPop" => {
            option(rest, declarations)
                .and_then(pair)
                .is_some_and(|(last, remaining)| {
                    variable(last) == Some(element) && array_variable(remaining) == Some(element)
                })
        }
        _ => false,
    }
}

fn type_contains_array(
    root: &Type,
    declarations: &IndexMap<Symbol, Decl<Type>>,
    external: &IndexMap<Symbol, ExternalType>,
) -> bool {
    enum Work<'a> {
        Ir(&'a Type),
        Semantic(Rc<Ty>),
        SemanticRow(Rc<crate::types::Row>),
    }

    fn push_alias<'a>(
        symbol: Symbol,
        declarations: &'a IndexMap<Symbol, Decl<Type>>,
        external: &IndexMap<Symbol, ExternalType>,
        expanded: &mut HashSet<Symbol>,
        work: &mut Vec<Work<'a>>,
    ) {
        if !expanded.insert(symbol) {
            return;
        }
        if let Some(declaration) = declarations.get(&symbol) {
            work.push(Work::Ir(&declaration.value));
        } else if let Some(declaration) = external.get(&symbol) {
            work.push(Work::Semantic(declaration.scheme.body().clone()));
        }
    }

    let mut work = vec![Work::Ir(root)];
    let mut expanded = HashSet::new();
    let mut expanded_rows = HashSet::new();
    // Rows cloned out of by-value semantic constructors need to stay alive:
    // otherwise the allocator may reuse an address and make the cycle guard
    // mistake a later sibling for a row it has already visited.
    let mut visited_rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ir(ty) => match &ty.tracked {
                TypeKind::Array(_) => return true,
                TypeKind::Arrow { from, to, .. } => {
                    work.push(Work::Ir(to));
                    work.push(Work::Ir(from));
                }
                TypeKind::Apply { head, args, .. } => {
                    work.extend(args.iter().map(Work::Ir));
                    push_alias(*head, declarations, external, &mut expanded, &mut work);
                }
                TypeKind::Ident(symbol) => {
                    push_alias(*symbol, declarations, external, &mut expanded, &mut work);
                }
                TypeKind::Struct { fields, .. } => {
                    work.extend(fields.values().filter_map(|field| match field {
                        TypeField::Written { value, .. } => Some(Work::Ir(value)),
                        TypeField::Absent { .. } => None,
                    }));
                }
                TypeKind::Sum { cases, .. } => {
                    work.extend(cases.values().filter_map(|case| match case {
                        SumCase::Written { payload, .. } => payload.as_ref().map(Work::Ir),
                        SumCase::Absent { .. } => None,
                    }));
                }
                TypeKind::Param { .. }
                | TypeKind::Prim(_)
                | TypeKind::Effects(_)
                | TypeKind::Var(_)
                | TypeKind::Hole
                | TypeKind::Error => {}
            },
            Work::Semantic(ty) => match &*ty {
                Ty::Array(_) => return true,
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::SemanticRow(Rc::new(effects.clone())));
                    work.push(Work::Semantic(to.clone()));
                    work.push(Work::Semantic(from.clone()));
                }
                Ty::Package(body) => work.push(Work::Semantic(body.clone())),
                Ty::Struct(row) | Ty::Sum(row) => {
                    work.push(Work::SemanticRow(Rc::new(row.clone())))
                }
                Ty::Named { symbol, args, .. } => {
                    work.extend(args.iter().cloned().map(Work::Semantic));
                    push_alias(*symbol, declarations, external, &mut expanded, &mut work);
                }
                Ty::Nat
                | Ty::Int
                | Ty::Real
                | Ty::String
                | Ty::Boolean
                | Ty::Var(_)
                | Ty::Bound(_)
                | Ty::Rigid { .. }
                | Ty::Undecided => {}
            },
            Work::SemanticRow(row) => {
                if !expanded_rows.insert(Rc::as_ptr(&row) as usize) {
                    continue;
                }
                visited_rows.push(row.clone());
                work.extend(
                    row.labels
                        .values()
                        .map(|field| Work::Semantic(field.ty.clone())),
                );
                if let Rest::More(more) = &row.rest {
                    work.push(Work::SemanticRow(more.clone()));
                }
            }
        }
    }
    false
}

fn dependency_path(dependency: &artifact::Artifact, qualified: &str) -> Option<Vec<String>> {
    let prefix = format!(
        "{}@{}::",
        dependency.header().identity.name,
        dependency.header().identity.version
    );
    let path = qualified.strip_prefix(&prefix)?;
    let parts: Vec<String> = path.split("::").map(str::to_owned).collect();
    // `split` always returns at least one part, including for the empty input.
    parts.iter().all(|part| !part.is_empty()).then_some(parts)
}

fn imported_symbol(
    mint: &mut Mint,
    namespace: Namespace,
    qualified: &str,
    symbols: &mut HashMap<(Namespace, String), Symbol>,
    names: &mut IndexMap<Symbol, artifact::QualifiedName>,
) -> Symbol {
    let key = (namespace, qualified.to_owned());
    if let Some(&symbol) = symbols.get(&key) {
        return symbol;
    }
    // References in an interface may target transitive dependencies. They get
    // semantic symbols so inference can follow them, but are deliberately not
    // installed in Builder::globals and therefore cannot be named by source.
    let symbol = mint.local(None, namespace, qualified);
    symbols.insert(key, symbol);
    mint.register_external(symbol, qualified);
    names.insert(symbol, qualified.to_owned());
    symbol
}

/// Whether a published type is the empty closed struct: what an effects
/// argument's arrow goes from and to.
fn is_unit(ty: &artifact::Type) -> bool {
    matches!(ty, artifact::Type::Struct(row) if row.labels.is_empty() && matches!(row.rest, artifact::Rest::Closed))
}

/// A published type read back as the written type it stands for, so that an
/// imported alias body can be substituted into and spliced exactly as a local
/// one is. Bound positions become the alias's parameters, named types the
/// imported declarations, and effect labels the imported effects; what has
/// no written form — a solver variable, a rigid, an undecided type — absorbs
/// as the error type. Bounded in depth, since an imported tree is data.
fn imported_syntax(
    mint: &mut Mint,
    ty: &artifact::Type,
    params: &[Symbol],
    symbols: &mut HashMap<(Namespace, String), Symbol>,
    names: &mut IndexMap<Symbol, artifact::QualifiedName>,
    effect_rows: &ImportedEffectRows,
    depth: usize,
) -> Type {
    let span = Span::default();
    if depth > 256 {
        return span.track(TypeKind::Error);
    }
    let tail_of = |rest: &artifact::Rest| match rest {
        artifact::Rest::Closed => None,
        artifact::Rest::Bound(index) => params.get(*index as usize).map(|symbol| Tail {
            span,
            of: Row::Param {
                symbol: *symbol,
                index: *index,
            },
        }),
        _ => Some(Tail {
            span,
            of: Row::Anything,
        }),
    };
    let tracked = match ty {
        artifact::Type::Nat => TypeKind::Prim(Prim::Nat),
        artifact::Type::Int => TypeKind::Prim(Prim::Int),
        artifact::Type::Real => TypeKind::Prim(Prim::Real),
        artifact::Type::String => TypeKind::Prim(Prim::String),
        artifact::Type::Boolean => TypeKind::Prim(Prim::Boolean),
        artifact::Type::Bound(index) => match params.get(*index as usize) {
            Some(symbol) => TypeKind::Param {
                symbol: *symbol,
                index: *index,
            },
            None => TypeKind::Error,
        },
        artifact::Type::Package(body) => {
            return imported_syntax(mint, body, params, symbols, names, effect_rows, depth + 1);
        }
        artifact::Type::Array(element) => TypeKind::Array(Box::new(imported_syntax(
            mint,
            element,
            params,
            symbols,
            names,
            effect_rows,
            depth + 1,
        ))),
        artifact::Type::Named { name, args } => {
            let head = imported_symbol(mint, Namespace::Types, name, symbols, names);
            match args.is_empty() {
                true => TypeKind::Ident(head),
                false => TypeKind::Apply {
                    head,
                    head_span: span,
                    args: args
                        .iter()
                        .map(|arg| {
                            imported_syntax(
                                mint,
                                arg,
                                params,
                                symbols,
                                names,
                                effect_rows,
                                depth + 1,
                            )
                        })
                        .collect(),
                },
            }
        }
        artifact::Type::Struct(row) => {
            let mut fields = IndexMap::new();
            for (name, field) in &row.labels {
                let field = match field.presence {
                    artifact::Presence::Present => TypeField::Written {
                        name_span: span,
                        when: None,
                        value: imported_syntax(
                            mint,
                            &field.ty,
                            params,
                            symbols,
                            names,
                            effect_rows,
                            depth + 1,
                        ),
                    },
                    artifact::Presence::Absent => TypeField::Absent { name_span: span },
                    _ => return span.track(TypeKind::Error),
                };
                fields.insert(name.clone(), field);
            }
            TypeKind::Struct {
                fields,
                tail: tail_of(&row.rest),
            }
        }
        artifact::Type::Sum(row) => {
            let mut cases = IndexMap::new();
            for (name, field) in &row.labels {
                let case = match field.presence {
                    artifact::Presence::Present => SumCase::Written {
                        name_span: span,
                        when: None,
                        payload: Some(imported_syntax(
                            mint,
                            &field.ty,
                            params,
                            symbols,
                            names,
                            effect_rows,
                            depth + 1,
                        )),
                    },
                    artifact::Presence::Absent => SumCase::Absent { name_span: span },
                    _ => return span.track(TypeKind::Error),
                };
                cases.insert(name.clone(), case);
            }
            TypeKind::Sum {
                cases,
                tail: tail_of(&row.rest),
            }
        }
        artifact::Type::Arrow(from, to, row) => {
            let mut effects = IndexMap::new();
            for (label, field) in &row.labels {
                let key = effect_rows.label(label);
                let Some(symbol) = effect_rows.symbol(&key, symbols) else {
                    continue;
                };
                let args = match &field.ty {
                    artifact::Type::Struct(payload) => payload
                        .labels
                        .iter()
                        .map(|(_, arg)| match &arg.ty {
                            // An effects argument: the row its arrow carries,
                            // read back as the row of effects it was.
                            artifact::Type::Arrow(from, to, _) if is_unit(from) && is_unit(to) => {
                                let arrow = imported_syntax(
                                    mint,
                                    &arg.ty,
                                    params,
                                    symbols,
                                    names,
                                    effect_rows,
                                    depth + 1,
                                );
                                match arrow.tracked {
                                    TypeKind::Arrow { effects, .. } => {
                                        span.track(TypeKind::Effects(effects))
                                    }
                                    other => span.track(other),
                                }
                            }
                            _ => imported_syntax(
                                mint,
                                &arg.ty,
                                params,
                                symbols,
                                names,
                                effect_rows,
                                depth + 1,
                            ),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let lowered = match field.presence {
                    artifact::Presence::Absent => EffectLabel::Absent {
                        name_span: span,
                        symbol,
                        args,
                        expanded: false,
                    },
                    _ => EffectLabel::Written {
                        name_span: span,
                        symbol,
                        args,
                        expanded: false,
                        when: None,
                    },
                };
                effects.insert(EffectId::pending(symbol), lowered);
            }
            let written = !effects.is_empty() || !matches!(row.rest, artifact::Rest::Closed);
            TypeKind::Arrow {
                from: Box::new(imported_syntax(
                    mint,
                    from,
                    params,
                    symbols,
                    names,
                    effect_rows,
                    depth + 1,
                )),
                to: Box::new(imported_syntax(
                    mint,
                    to,
                    params,
                    symbols,
                    names,
                    effect_rows,
                    depth + 1,
                )),
                effects: Box::new(EffectRow {
                    span,
                    written,
                    effects,
                    tail: tail_of(&row.rest),
                }),
            }
        }
        artifact::Type::Var(_) | artifact::Type::Rigid { .. } | artifact::Type::Undecided => {
            TypeKind::Error
        }
    };
    span.track(tracked)
}

fn import_scheme(
    mint: &mut Mint,
    scheme: &artifact::Scheme,
    symbols: &mut HashMap<(Namespace, String), Symbol>,
    names: &mut IndexMap<Symbol, artifact::QualifiedName>,
    effect_rows: &ImportedEffectRows,
) -> Scheme {
    // Published values own their quantifier metadata. Clamp inconsistent
    // presence counts and every bound use before the trusted opening paths can
    // index the fresh-variable vector.
    let count = scheme.count;
    let presences = scheme.presences.min(count);
    let body = import_type(mint, &scheme.body, symbols, names, effect_rows);
    let body = clamp_bounds(body, count as usize, presences as usize);
    let (formula, sanitized) = import_formula(&scheme.formula);
    let formula = match sanitized && formula_bounds_valid(&formula, presences) {
        true => formula,
        false => {
            drop_formula_iterative(formula);
            crate::types::Formula::True
        }
    };
    let existentials = scheme
        .existentials
        .iter()
        .copied()
        .filter(|index| *index < presences)
        .collect();
    Scheme::existential(count, presences, existentials, body, formula)
}

/// Replace bound positions a malformed imported interface did not declare with
/// undecided recovery nodes. Presence slots occupy `0..presences`; type and row
/// slots occupy the remainder. Foreign solver-local variables and rigids are
/// recovery input as well. The explicit postorder stack keeps arbitrarily deep
/// types, field payloads, and `Rest::More` rows safe to import.
fn clamp_bounds(ty: Rc<Ty>, count: usize, presences: usize) -> Rc<Ty> {
    enum Work<'a> {
        Ty(&'a Ty),
        Row(&'a crate::types::Row),
        Arrow,
        Package,
        Array,
        Struct,
        Sum,
        Named {
            symbol: Symbol,
            name: Rc<str>,
            args: usize,
        },
        BuiltRow {
            labels: Vec<(&'a String, &'a crate::types::RowField)>,
            rest: Rest,
            composed: bool,
        },
    }
    let mut types = Vec::new();
    let mut rows = Vec::new();

    let mut work = vec![Work::Ty(&ty)];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(value) => match value {
                Ty::Nat => types.push(Rc::new(Ty::Nat)),
                Ty::Int => types.push(Rc::new(Ty::Int)),
                Ty::Real => types.push(Rc::new(Ty::Real)),
                Ty::String => types.push(Rc::new(Ty::String)),
                Ty::Boolean => types.push(Rc::new(Ty::Boolean)),
                Ty::Bound(index) if (*index as usize) < count && (*index as usize) >= presences => {
                    types.push(Rc::new(Ty::Bound(*index)))
                }
                Ty::Bound(_) | Ty::Var(_) | Ty::Rigid { .. } | Ty::Undecided => {
                    types.push(Rc::new(Ty::Undecided))
                }
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects));
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                Ty::Package(body) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(body));
                }
                Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element));
                }
                Ty::Struct(fields) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(fields));
                }
                Ty::Sum(cases) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(cases));
                }
                Ty::Named { symbol, name, args } => {
                    work.push(Work::Named {
                        symbol: *symbol,
                        name: name.clone(),
                        args: args.len(),
                    });
                    work.extend(args.iter().rev().map(|arg| Work::Ty(arg)));
                }
            },
            Work::Row(value) => {
                let composed = matches!(value.rest, Rest::More(_));
                let mut labels = Vec::new();
                let mut claimed = HashSet::new();
                let mut row = value;
                let rest = loop {
                    for (name, field) in &row.labels {
                        if claimed.insert(name.as_str()) {
                            labels.push((name, field));
                        }
                    }
                    match &row.rest {
                        Rest::More(more) => row = more,
                        Rest::Closed => break Rest::Closed,
                        Rest::Bound(index)
                            if (*index as usize) < count && (*index as usize) >= presences =>
                        {
                            break Rest::Bound(*index);
                        }
                        Rest::Bound(_) | Rest::Var(_) | Rest::Rigid { .. } | Rest::Undecided => {
                            break Rest::Undecided;
                        }
                    }
                };
                work.push(Work::BuiltRow {
                    labels: labels.clone(),
                    rest,
                    composed,
                });
                work.extend(labels.into_iter().rev().filter_map(|(_, field)| {
                    (!matches!(field.presence, Presence::Absent)).then_some(Work::Ty(&field.ty))
                }));
            }
            Work::Arrow => {
                let effects = rows.pop().expect("row postorder stays balanced");
                let to = types.pop().expect("type postorder stays balanced");
                let from = types.pop().expect("type postorder stays balanced");
                types.push(Rc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("package postorder stays balanced");
                types.push(Rc::new(Ty::Package(body)));
            }
            Work::Array => {
                let element = types.pop().expect("array postorder stays balanced");
                types.push(Rc::new(Ty::Array(element)));
            }
            Work::Struct => {
                let fields = rows.pop().expect("row postorder stays balanced");
                types.push(Rc::new(Ty::Struct(fields)));
            }
            Work::Sum => {
                let cases = rows.pop().expect("row postorder stays balanced");
                types.push(Rc::new(Ty::Sum(cases)));
            }
            Work::Named { symbol, name, args } => {
                let mut imported = Vec::with_capacity(args);
                for _ in 0..args {
                    imported.push(types.pop().expect("type postorder stays balanced"));
                }
                imported.reverse();
                types.push(Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: imported.into(),
                }));
            }
            Work::BuiltRow {
                labels,
                rest,
                composed,
            } => {
                let mut fields = Vec::with_capacity(labels.len());
                for (name, field) in labels.into_iter().rev() {
                    let presence = match field.presence {
                        Presence::Present => Presence::Present,
                        Presence::Absent => Presence::Absent,
                        // Preserve presence correlation for canonical recovery.
                        // `open_type` bounds-checks malformed positions and
                        // turns them into Undecided before solving.
                        Presence::Bound(index) => Presence::Bound(index),
                        Presence::Recovered(index) => Presence::Recovered(index),
                        Presence::Var(_) | Presence::Undecided => Presence::Undecided,
                    };
                    let ty = match presence {
                        Presence::Absent => Rc::new(Ty::Undecided),
                        _ => types.pop().expect("type postorder stays balanced"),
                    };
                    fields.push((name.clone(), crate::types::RowField { presence, ty }));
                }
                fields.reverse();
                let row = crate::types::Row {
                    labels: fields.into_iter().collect(),
                    rest,
                };
                rows.push(match composed {
                    true => crate::types::Row::of(Rest::More(Rc::new(row))),
                    false => row,
                });
            }
        }
    }
    let clamped = types.pop().expect("type postorder stays balanced");
    drop_type_iterative(ty);
    clamped
}

/// Destroy a possibly deep temporary semantic type with an explicit stack.
/// Importing and clamping intentionally build two independent trees; letting
/// Rust recursively release the first one would undo the stack-safe walk.
fn drop_type_iterative(root: Rc<Ty>) {
    enum Work {
        Ty(Rc<Ty>),
        Row(Rc<crate::types::Row>),
    }

    fn row(row: &crate::types::Row, work: &mut Vec<Work>) {
        work.extend(row.labels.values().map(|field| Work::Ty(field.ty.clone())));
        if let Rest::More(more) = &row.rest {
            work.push(Work::Row(more.clone()));
        }
    }

    let mut work = vec![Work::Ty(root)];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => {
                match &*ty {
                    Ty::Arrow(from, to, effects) => {
                        work.push(Work::Ty(from.clone()));
                        work.push(Work::Ty(to.clone()));
                        row(effects, &mut work);
                    }
                    Ty::Package(body) => work.push(Work::Ty(body.clone())),
                    Ty::Array(element) => work.push(Work::Ty(element.clone())),
                    Ty::Struct(fields) | Ty::Sum(fields) => row(fields, &mut work),
                    Ty::Named { args, .. } => {
                        work.extend(args.iter().cloned().map(Work::Ty));
                    }
                    Ty::Nat
                    | Ty::Int
                    | Ty::Real
                    | Ty::String
                    | Ty::Boolean
                    | Ty::Var(_)
                    | Ty::Bound(_)
                    | Ty::Rigid { .. }
                    | Ty::Undecided => {}
                }
                drop(ty);
            }
            Work::Row(row_) => {
                row(&row_, &mut work);
                drop(row_);
            }
        }
    }
}

fn drop_formula_iterative(root: crate::types::Formula) {
    use crate::types::Formula;

    fn children(formula: &Formula, work: &mut Vec<Rc<Formula>>) {
        match formula {
            Formula::Owned(_, inner) | Formula::Not(inner) => work.push(inner.clone()),
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Iff(left, right)
            | Formula::Xor(left, right) => {
                work.push(left.clone());
                work.push(right.clone());
            }
            Formula::True | Formula::False | Formula::Atom(_) => {}
        }
    }

    let mut work = Vec::new();
    children(&root, &mut work);
    drop(root);
    while let Some(formula) = work.pop() {
        children(&formula, &mut work);
        drop(formula);
    }
}

fn formula_bounds_valid(formula: &crate::types::Formula, presences: u32) -> bool {
    use crate::types::{Atom, Formula};
    let mut work = vec![formula];
    while let Some(formula) = work.pop() {
        match formula {
            Formula::True | Formula::False => {}
            Formula::Atom(Atom::Bound(index)) if *index < presences => {}
            Formula::Atom(_) => return false,
            Formula::Owned(_, inner) | Formula::Not(inner) => work.push(inner),
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Iff(left, right)
            | Formula::Xor(left, right) => {
                work.push(right);
                work.push(left);
            }
        }
    }
    true
}

/// Imported effect identities are canonicalized independently of the semantic
/// types which refer to them. Generated row labels still contain the published
/// interface text, so carry the corresponding key rewrite and the published
/// senses of named arguments through the stack-safe type importer.
#[derive(Default)]
struct ImportedEffectRows {
    labels: HashMap<String, String>,
    /// A leaf name with exactly one known structural declaration. This closes
    /// stale generated keys whose published interface text no longer exactly
    /// matches that declaration; ambiguous same-named effects still require an
    /// exact old key and are never guessed.
    names: HashMap<String, Option<String>>,
    identities: HashMap<artifact::QualifiedName, EffectId>,
    arguments: HashMap<artifact::QualifiedName, Vec<artifact::Sense>>,
}

impl ImportedEffectRows {
    fn insert_identity(&mut self, qualified: &str, identity: EffectId) {
        let name = identity.name().to_string();
        let canonical = identity.row_key();
        self.identities.insert(qualified.to_string(), identity);
        match self.names.entry(name.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(canonical.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != Some(&canonical) {
                    entry.insert(None);
                }
            }
        }
    }

    /// The imported effect an identity's row key names, when this bundle
    /// imported one with that identity.
    fn symbol(&self, key: &str, symbols: &HashMap<(Namespace, String), Symbol>) -> Option<Symbol> {
        self.identities
            .iter()
            .find(|(_, identity)| identity.row_key() == key)
            .and_then(|(qualified, _)| symbols.get(&(Namespace::Effects, qualified.clone())))
            .copied()
    }

    fn label(&self, label: &str) -> String {
        if let Some(canonical) = self.labels.get(label) {
            return canonical.clone();
        }
        let Some((name, interface)) = EffectId::parse_row_key(label) else {
            return label.to_string();
        };
        if let Some(Some(canonical)) = self.names.get(name) {
            return canonical.clone();
        }
        EffectId::structural(name.to_string(), canonical_effect_interface(interface)).row_key()
    }

    fn argument_is_effects(&self, name: &str, index: usize) -> bool {
        matches!(
            self.arguments
                .get(name)
                .and_then(|senses| senses.get(index)),
            Some(artifact::Sense::Effects)
        )
    }
}

/// Convert an artifact type without using the host call stack. Artifact types
/// are untrusted and may be tens of thousands of constructors deep. The boolean
/// on type and row jobs is a shape, not a spelling heuristic: only an arrow's
/// row or an argument published with `Sense::Effects` may have effect keys
/// rewritten. In particular, an ordinary sum label containing the separator is
/// still an ordinary sum label.
fn import_type(
    mint: &mut Mint,
    value: &artifact::Type,
    symbols: &mut HashMap<(Namespace, String), Symbol>,
    names: &mut IndexMap<Symbol, artifact::QualifiedName>,
    effect_rows: &ImportedEffectRows,
) -> Rc<Ty> {
    enum Work<'a> {
        Ty(&'a artifact::Type, bool),
        Row(&'a artifact::Row, bool),
        Arrow,
        Package,
        Array,
        Struct,
        Sum,
        Named {
            symbol: Symbol,
            name: Rc<str>,
            args: usize,
        },
        BuiltRow(&'a artifact::Row, bool),
    }
    let mut types = Vec::new();
    let mut rows = Vec::new();

    let mut work = vec![Work::Ty(value, false)];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(value, is_effect_row) => match value {
                artifact::Type::Nat => types.push(Rc::new(Ty::Nat)),
                artifact::Type::Int => types.push(Rc::new(Ty::Int)),
                artifact::Type::Real => types.push(Rc::new(Ty::Real)),
                artifact::Type::String => types.push(Rc::new(Ty::String)),
                artifact::Type::Boolean => types.push(Rc::new(Ty::Boolean)),
                artifact::Type::Bound(index) => types.push(Rc::new(Ty::Bound(*index))),
                artifact::Type::Var(_)
                | artifact::Type::Rigid { .. }
                | artifact::Type::Undecided => types.push(Rc::new(Ty::Undecided)),
                artifact::Type::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects, true));
                    work.push(Work::Ty(to, false));
                    work.push(Work::Ty(from, false));
                }
                artifact::Type::Package(body) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(body, is_effect_row));
                }
                artifact::Type::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element, false));
                }
                artifact::Type::Struct(fields) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(fields, false));
                }
                artifact::Type::Sum(cases) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(cases, is_effect_row));
                }
                artifact::Type::Named { name, args } => {
                    let symbol = imported_symbol(mint, Namespace::Types, name, symbols, names);
                    work.push(Work::Named {
                        symbol,
                        name: Rc::from(name.as_str()),
                        args: args.len(),
                    });
                    work.extend(args.iter().enumerate().rev().map(|(index, arg)| {
                        Work::Ty(arg, effect_rows.argument_is_effects(name, index))
                    }));
                }
            },
            Work::Row(value, is_effect_row) => {
                work.push(Work::BuiltRow(value, is_effect_row));
                if let artifact::Rest::More(more) = &value.rest {
                    work.push(Work::Row(more, is_effect_row));
                }
                work.extend(value.labels.iter().rev().filter_map(|(_, field)| {
                    (!matches!(field.presence, artifact::Presence::Absent))
                        .then_some(Work::Ty(&field.ty, false))
                }));
            }
            Work::Arrow => {
                let effects = rows.pop().expect("row postorder stays balanced");
                let to = types.pop().expect("type postorder stays balanced");
                let from = types.pop().expect("type postorder stays balanced");
                types.push(Rc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("package postorder stays balanced");
                types.push(Rc::new(Ty::Package(body)));
            }
            Work::Array => {
                let element = types.pop().expect("array postorder stays balanced");
                types.push(Rc::new(Ty::Array(element)));
            }
            Work::Struct => {
                let fields = rows.pop().expect("row postorder stays balanced");
                types.push(Rc::new(Ty::Struct(fields)));
            }
            Work::Sum => {
                let cases = rows.pop().expect("row postorder stays balanced");
                types.push(Rc::new(Ty::Sum(cases)));
            }
            Work::Named { symbol, name, args } => {
                let mut imported = Vec::with_capacity(args);
                for _ in 0..args {
                    imported.push(types.pop().expect("type postorder stays balanced"));
                }
                imported.reverse();
                types.push(Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: imported.into(),
                }));
            }
            Work::BuiltRow(value, is_effect_row) => {
                let rest = match &value.rest {
                    artifact::Rest::Closed => Rest::Closed,
                    artifact::Rest::Bound(index) => Rest::Bound(*index),
                    artifact::Rest::More(_) => {
                        Rest::More(Rc::new(rows.pop().expect("row postorder stays balanced")))
                    }
                    artifact::Rest::Var(_)
                    | artifact::Rest::Rigid { .. }
                    | artifact::Rest::Undecided => Rest::Undecided,
                };
                let mut fields = Vec::with_capacity(value.labels.len());
                for (name, field) in value.labels.iter().rev() {
                    let presence = match field.presence {
                        artifact::Presence::Present => Presence::Present,
                        artifact::Presence::Absent => Presence::Absent,
                        artifact::Presence::Bound(index) => Presence::Bound(index),
                        // Foreign solver variables cannot enter this solver's
                        // ID space. Keep their correlation in an out-of-band
                        // recovery namespace which cannot collide with any
                        // valid (or malformed) scheme-bound index.
                        artifact::Presence::Var(index) => Presence::Recovered(index),
                        artifact::Presence::Undecided => Presence::Undecided,
                    };
                    let ty = match presence {
                        Presence::Absent => Rc::new(Ty::Undecided),
                        _ => types.pop().expect("type postorder stays balanced"),
                    };
                    let name = match is_effect_row {
                        true => effect_rows.label(name),
                        false => name.clone(),
                    };
                    fields.push((name, crate::types::RowField { presence, ty }));
                }
                fields.reverse();
                rows.push(crate::types::Row {
                    labels: fields.into_iter().collect(),
                    rest,
                });
            }
        }
    }
    types.pop().expect("type postorder stays balanced")
}

/// Import a formula iteratively. A solver-local atom has no meaning across the
/// artifact boundary; `false` in the second result asks the scheme importer to
/// recover the whole constraint to `true` rather than retaining a foreign ID.
fn import_formula(value: &artifact::Formula) -> (crate::types::Formula, bool) {
    use crate::types::{Atom, Formula};

    #[derive(Clone, Copy)]
    enum Binary {
        And,
        Or,
        Iff,
        Xor,
    }
    enum Work<'a> {
        Formula(&'a artifact::Formula),
        Owned(u32),
        Not,
        Binary(Binary),
    }

    let mut valid = true;
    let mut work = vec![Work::Formula(value)];
    let mut values = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Formula(value) => match value {
                artifact::Formula::True => values.push(Formula::True),
                artifact::Formula::False => values.push(Formula::False),
                artifact::Formula::Bound(index) => values.push(Formula::Atom(Atom::Bound(*index))),
                artifact::Formula::Var(_) => {
                    valid = false;
                    values.push(Formula::True);
                }
                artifact::Formula::Owned(owner, inner) => {
                    work.push(Work::Owned(*owner));
                    work.push(Work::Formula(inner));
                }
                artifact::Formula::Not(inner) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                artifact::Formula::And(left, right) => {
                    work.push(Work::Binary(Binary::And));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                artifact::Formula::Or(left, right) => {
                    work.push(Work::Binary(Binary::Or));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                artifact::Formula::Iff(left, right) => {
                    work.push(Work::Binary(Binary::Iff));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                artifact::Formula::Xor(left, right) => {
                    work.push(Work::Binary(Binary::Xor));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
            },
            Work::Owned(owner) => {
                let inner = values.pop().expect("owned visits one operand");
                values.push(Formula::owned(owner, inner));
            }
            Work::Not => {
                let inner = values.pop().expect("not visits one operand");
                values.push(Formula::Not(Rc::new(inner)));
            }
            Work::Binary(operator) => {
                let right = values
                    .pop()
                    .expect("a binary formula visits its right operand");
                let left = values
                    .pop()
                    .expect("a binary formula visits its left operand");
                values.push(match operator {
                    Binary::And => Formula::And(Rc::new(left), Rc::new(right)),
                    Binary::Or => Formula::Or(Rc::new(left), Rc::new(right)),
                    Binary::Iff => Formula::Iff(Rc::new(left), Rc::new(right)),
                    Binary::Xor => Formula::Xor(Rc::new(left), Rc::new(right)),
                });
            }
        }
    }
    (
        values.pop().expect("a visited formula produces a value"),
        valid,
    )
}

/// Assign structural identities to effects and replace the provisional source
/// symbols in every lowered effect row. Symbols remain on each label for
/// operation lookup and editor navigation; the key is only row semantics.
fn structuralize_effects(
    program: &mut Program,
    mint: &Mint,
    expansions: &HashMap<Symbol, IndexMap<String, Symbol>>,
    errors: &mut Vec<Error>,
) {
    // Imported rows were lowered with imported symbols before local effect
    // identities could be computed. Keep those identities in the rekeying map:
    // replacing the map with local-only identities silently dropped every
    // dependency effect from local declared types.
    let mut ids = program.effect_ids.clone();
    let operation_effects: Vec<_> = program
        .effects
        .iter()
        .filter_map(|(symbol, decl)| match &decl.value {
            Effect::Operations(operations) => Some((*symbol, operations)),
            Effect::Alias(_) => None,
        })
        .collect();
    let local_ids = {
        let mut canonical = EffectCanonicalizer::new(
            &program.types,
            &program.effects,
            &program.external_types,
            &program.effect_ids,
            mint,
        );
        operation_effects
            .into_iter()
            .map(|(symbol, operations)| {
                let interface =
                    canonical.canonical_interface(&program.effects[&symbol].params, operations);
                (
                    symbol,
                    EffectId::structural(mint.name(symbol).to_string(), interface),
                )
            })
            .collect::<Vec<_>>()
    };
    ids.extend(local_ids);
    program.effect_ids = ids.clone();
    for decl in program.effects.values_mut() {
        match &mut decl.value {
            Effect::Operations(operations) => {
                for operation in operations.values_mut() {
                    rekey_type(&mut operation.from, &ids, errors);
                    rekey_type(&mut operation.to, &ids, errors);
                }
            }
            Effect::Alias(alias) => {
                // The row an alias comes to is keyed like any other, but two
                // of its labels meeting at one identity is the overlap
                // reported below, at the case that brought the second.
                rekey_type(&mut alias.expanded, &ids, &mut Vec::new());
                for case in &mut alias.body.cases {
                    for arg in &mut case.args {
                        rekey_type(arg, &ids, errors);
                    }
                }
                // Diagnose overlap after closing aliases transitively. Each
                // written case gets its own expansion, so two differently
                // named aliases that reach the same structural effect are not
                // silently collapsed by the expansion map.
                let mut seen: HashMap<EffectId, Span> = HashMap::new();
                for item in &alias.body.cases {
                    let mut duplicate = None;
                    for concrete in expansions
                        .get(&item.symbol)
                        .into_iter()
                        .flat_map(IndexMap::values)
                    {
                        if let Some(id) = ids.get(concrete) {
                            if let Some(previous) = seen.get(id) {
                                duplicate.get_or_insert_with(|| (id.name().to_string(), *previous));
                            } else {
                                seen.insert(id.clone(), item.name_span);
                            }
                        }
                    }
                    if let Some((name, previous)) = duplicate {
                        errors.push(Error {
                            span: item.name_span,
                            kind: ErrorKind::DuplicateCase {
                                shape: Shape::Effect,
                                name,
                                previous,
                            },
                        });
                    }
                }
            }
        }
    }
    for decl in program.types.values_mut() {
        rekey_type(&mut decl.value, &ids, errors);
    }
}

/// An operation signature's structural spelling. The operation rule currently
/// rejects effect rows and open tails, but this walk deliberately preserves
/// them: recovery still gives an erroneous signature a semantic identity, and
/// a future relaxation of that rule must not silently make effectful arrows
/// compare as pure ones. Recursive types and effects use graph backreferences,
/// so equivalent dependency cycles neither acquire a module path nor depend on
/// a finite unrolling depth.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RegularLabel {
    Ordinary(String),
    /// Recovery for text which is not a graph encoding. This is graph data,
    /// rather than a magic ordinary atom an accepted encoding could forge.
    OpaqueInterface(String),
    /// Recovery for an effect-row key with no interface component.
    UnknownInterface,
}

impl Default for RegularLabel {
    fn default() -> Self {
        Self::Ordinary(String::new())
    }
}

#[derive(Clone, Default)]
struct RegularNode {
    label: RegularLabel,
    edges: Vec<(String, usize)>,
}

/// One exact, compact presentation shared by operation interfaces and every
/// type/effect graph they reach. Node numbers are only construction details;
/// `encode` bisimulation-minimizes the reachable graph and assigns deterministic
/// backreference numbers before an identity crosses the artifact boundary.
type EncodedGraph = Vec<(usize, RegularLabel, Vec<(String, usize)>)>;

#[derive(Default)]
struct CanonicalArena {
    nodes: Vec<RegularNode>,
}

impl CanonicalArena {
    fn labelled_node(&mut self, label: RegularLabel, edges: Vec<(String, usize)>) -> usize {
        let id = self.nodes.len();
        self.nodes.push(RegularNode { label, edges });
        id
    }

    fn node(&mut self, label: impl Into<String>, edges: Vec<(String, usize)>) -> usize {
        self.labelled_node(RegularLabel::Ordinary(label.into()), edges)
    }

    fn atom(&mut self, label: impl Into<String>) -> usize {
        self.node(label, Vec::new())
    }

    /// Decode an interface emitted by `encode`. Imported generated effect keys
    /// therefore join the same graph as local interfaces instead of becoming a
    /// large opaque label. Untrusted or older artifact text remains an exact,
    /// distinct recovery atom.
    fn import(&mut self, encoded: &str) -> usize {
        fn number(bytes: &[u8], at: &mut usize, delimiter: u8) -> Option<usize> {
            let start = *at;
            while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
                *at += 1;
            }
            if start == *at || bytes.get(*at).copied()? != delimiter {
                return None;
            }
            let value = std::str::from_utf8(&bytes[start..*at]).ok()?.parse().ok()?;
            *at += 1;
            Some(value)
        }
        fn bare_number(bytes: &[u8], at: &mut usize) -> Option<usize> {
            let start = *at;
            while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
                *at += 1;
            }
            (start != *at)
                .then(|| std::str::from_utf8(&bytes[start..*at]).ok()?.parse().ok())
                .flatten()
        }
        fn parse(encoded: &str) -> Option<EncodedGraph> {
            let bytes = encoded.as_bytes();
            let mut at = 0;
            let mut parsed = Vec::new();
            while at < bytes.len() {
                let id = number(bytes, &mut at, b'#')?;
                let label = match bytes.get(at).copied() {
                    Some(b'o') => {
                        at += 1;
                        let length = number(bytes, &mut at, b':')?;
                        let end = at.checked_add(length)?;
                        let value = std::str::from_utf8(bytes.get(at..end)?).ok()?.to_string();
                        at = end;
                        RegularLabel::OpaqueInterface(value)
                    }
                    Some(b'u') => {
                        at += 1;
                        RegularLabel::UnknownInterface
                    }
                    _ => {
                        let length = number(bytes, &mut at, b':')?;
                        let end = at.checked_add(length)?;
                        let value = std::str::from_utf8(bytes.get(at..end)?).ok()?.to_string();
                        at = end;
                        RegularLabel::Ordinary(value)
                    }
                };
                let mut edges = Vec::new();
                while bytes.get(at).copied() == Some(b'|') {
                    at += 1;
                    let length = number(bytes, &mut at, b':')?;
                    let end = at.checked_add(length)?;
                    let edge = std::str::from_utf8(bytes.get(at..end)?).ok()?.to_string();
                    at = end;
                    (bytes.get(at).copied() == Some(b'>')).then_some(())?;
                    at += 1;
                    edges.push((edge, bare_number(bytes, &mut at)?));
                }
                (bytes.get(at).copied() == Some(b';')).then_some(())?;
                at += 1;
                parsed.push((id, label, edges));
            }
            (!parsed.is_empty()
                && !parsed.iter().enumerate().any(|(expected, (id, _, edges))| {
                    *id != expected || edges.iter().any(|(_, child)| *child >= parsed.len())
                }))
            .then_some(parsed)
        }

        let Some(parsed) = parse(encoded).filter(|parsed| {
            // Graph identities have one wire spelling. Besides excluding
            // unreachable records, this rejects padded numbers, construction
            // numbering, unsorted edges and every other presentation which
            // decodes to a canonical graph but is not itself that encoding.
            // Recovery labels (`#o` and `#u`) go through the same check and
            // remain accepted when canonically spelled.
            let nodes: Vec<_> = parsed
                .iter()
                .map(|(_, label, edges)| RegularNode {
                    label: label.clone(),
                    edges: edges.clone(),
                })
                .collect();
            encode_dense_graph(&nodes, 0) == encoded
        }) else {
            return self.labelled_node(
                RegularLabel::OpaqueInterface(encoded.to_string()),
                Vec::new(),
            );
        };
        let base = self.nodes.len();
        self.nodes
            .extend((0..parsed.len()).map(|_| RegularNode::default()));
        for (id, label, edges) in parsed {
            self.nodes[base + id] = RegularNode {
                label,
                edges: edges
                    .into_iter()
                    .map(|(label, child)| (label, base + child))
                    .collect(),
            };
        }
        base
    }

    fn effect_key(&mut self, key: &str) -> usize {
        let (name, interface) = EffectId::parse_row_key(key)
            .map_or((key, None), |(name, interface)| (name, Some(interface)));
        let interface = interface
            .map(|interface| self.import(interface))
            .unwrap_or_else(|| self.labelled_node(RegularLabel::UnknownInterface, Vec::new()));
        self.node(
            format!("effect:{name}"),
            vec![("interface".into(), interface)],
        )
    }

    fn encode(&self, root: usize) -> String {
        encode_regular_graph(&self.nodes, root)
    }
}

/// Imported identities are compared and stored only in graph-canonical form.
/// Malformed text becomes the canonical opaque recovery node, which also makes
/// an explicitly encoded opaque identity for that exact text the same row key.
fn canonical_effect_interface(interface: &str) -> String {
    let mut arena = CanonicalArena::default();
    let root = arena.import(interface);
    arena.encode(root)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SemanticPresenceKey {
    Bound { instantiation: usize, index: u32 },
    Recovered { instantiation: usize, id: u32 },
}

/// The alpha alphabet of one operation signature. Its input and output share
/// this scope, while every imported scheme instantiation receives a child
/// namespace for its bound presences.
#[derive(Default)]
struct CanonicalPresenceScope {
    written: HashMap<String, usize>,
    semantic: HashMap<SemanticPresenceKey, usize>,
    next_alpha: usize,
    next_instantiation: usize,
}

impl CanonicalPresenceScope {
    fn instantiation(&mut self) -> usize {
        let id = self.next_instantiation;
        self.next_instantiation += 1;
        id
    }
}

/// A finite presentation of a regular type tree. Named declarations are
/// memoized as graph nodes and then minimized by bisimulation before they are
/// spelled. This deliberately canonicalizes the *infinite unfolding*, rather
/// than the particular collection of aliases which happened to present it.
struct RegularType<'a> {
    types: &'a IndexMap<Symbol, Decl<Type>>,
    external_types: &'a IndexMap<Symbol, ExternalType>,
    effect_interfaces: &'a HashMap<Symbol, usize>,
    arena: &'a mut CanonicalArena,
    created: Vec<usize>,
    /// Hash-consing makes semantic argument identity independent of the path
    /// which built it. In particular, `T Nat` reached around a recursive
    /// imported declaration meets the same memo entry instead of rebuilding
    /// and re-encoding an isomorphic argument graph on every turn.
    interned: HashMap<(String, Vec<(String, usize)>), usize>,
    named: HashMap<(Symbol, Vec<usize>, bool), usize>,
}

impl RegularType<'_> {
    fn node(&mut self, label: impl Into<String>, edges: Vec<(String, usize)>) -> usize {
        let label = label.into();
        let key = (label.clone(), edges.clone());
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.arena.node(label, edges);
        self.created.push(id);
        self.interned.insert(key, id);
        id
    }

    /// A mutable graph back edge. Placeholders are deliberately not interned:
    /// their label is replaced once the declaration body has been visited.
    fn placeholder(&mut self) -> usize {
        let id = self.arena.atom("unproductive-cycle");
        self.created.push(id);
        id
    }

    fn atom(&mut self, value: impl Into<String>) -> usize {
        self.node(value, Vec::new())
    }

    /// The nodes an effect's own parameters stand for in its operation types:
    /// one position each, and nothing about the name.
    fn parameters(&mut self, count: usize) -> Vec<usize> {
        (0..count)
            .map(|index| self.atom(format!("param:{index}")))
            .collect()
    }

    /// Select one applied parameter, recovering malformed imported arity with
    /// the ordinary unknown node. Source applications are already arity-checked;
    /// sharing this lookup keeps both presentations bounds-safe.
    fn argument(&mut self, args: &[usize], index: u32) -> usize {
        args.get(index as usize)
            .copied()
            .unwrap_or_else(|| self.atom("?"))
    }

    fn with_fields(&mut self, core: usize, fields: Vec<(String, String, usize)>) -> usize {
        if fields.is_empty() {
            return core;
        }
        let mut edges = Vec::with_capacity(fields.len() + 1);
        edges.push(("core".into(), core));
        edges.extend(
            fields
                .into_iter()
                .map(|(name, presence, ty)| (format!("field:{name}:{presence}"), ty)),
        );
        self.node("fields", edges)
    }

    /// Build source syntax, local named declarations, and effect rows with one
    /// continuation stack. Effect spellings have already been resolved by the
    /// outer dependency work list, so crossing type/effect/type boundaries
    /// never re-enters this walk on the native stack.
    fn source(
        &mut self,
        ty: &Type,
        args: &[usize],
        presence_scope: &mut CanonicalPresenceScope,
    ) -> usize {
        enum Work<'a> {
            Type(&'a Type, Vec<usize>),
            CoreTail(&'a Option<Tail>, Vec<usize>),
            RowTail(&'a Option<Tail>, Vec<usize>),
            EffectRow(&'a EffectRow, Vec<usize>),
            Named(Symbol, Vec<usize>),
            Apply(Symbol, usize),
            FinishNamed(usize),
            Make(String, Vec<String>),
            Fields(Vec<(String, String)>),
            EffectCase(String, usize),
            Canonical(usize),
            Atom(String),
        }

        let mut work = vec![Work::Type(ty, args.to_vec())];
        let mut values = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Atom(label) => values.push(self.atom(label)),
                Work::Canonical(node) => values.push(node),
                Work::EffectCase(presence, arity) => {
                    let start = values.len() - arity;
                    let args = values.split_off(start);
                    let payload = values.pop().expect("an effect case has a payload");
                    let identity = values.pop().expect("an effect case has an identity");
                    let edges = [("identity".into(), identity), ("payload".into(), payload)]
                        .into_iter()
                        .chain(
                            args.into_iter()
                                .enumerate()
                                .map(|(at, arg)| (format!("arg:{at}"), arg)),
                        )
                        .collect();
                    values.push(self.node(format!("effect-case:{presence}"), edges));
                }
                Work::Make(label, edge_labels) => {
                    let start = values.len() - edge_labels.len();
                    let children = values.split_off(start);
                    values.push(self.node(label, edge_labels.into_iter().zip(children).collect()));
                }
                Work::Fields(fields) => {
                    let start = values.len() - fields.len() - 1;
                    let mut children = values.split_off(start).into_iter();
                    let core = children.next().expect("fields have a core");
                    let fields = fields
                        .into_iter()
                        .zip(children)
                        .map(|((name, presence), ty)| (name, presence, ty))
                        .collect();
                    values.push(self.with_fields(core, fields));
                }
                Work::Type(ty, args) => match &ty.tracked {
                    TypeKind::Array(element) => {
                        work.push(Work::Make("array".into(), vec!["element".into()]));
                        work.push(Work::Type(element, args));
                    }
                    TypeKind::Struct { fields, tail } => {
                        let labels = fields
                            .iter()
                            .map(|(name, field)| {
                                let presence = match field {
                                    TypeField::Written { when, .. } => {
                                        canonical_written_presence(when, presence_scope)
                                    }
                                    TypeField::Absent { .. } => "\\".into(),
                                };
                                (name.clone(), presence)
                            })
                            .collect();
                        work.push(Work::Fields(labels));
                        for field in fields.values().rev() {
                            match field {
                                TypeField::Written { value, .. } => {
                                    work.push(Work::Type(value, args.clone()))
                                }
                                TypeField::Absent { .. } => work.push(Work::Atom("?".into())),
                            }
                        }
                        work.push(Work::CoreTail(tail, args));
                    }
                    TypeKind::Sum { cases, tail } => {
                        let labels = cases
                            .iter()
                            .map(|(name, case)| {
                                let presence = match case {
                                    SumCase::Written { when, .. } => {
                                        canonical_written_presence(when, presence_scope)
                                    }
                                    SumCase::Absent { .. } => "\\".into(),
                                };
                                format!("label:{name}:{presence}")
                            })
                            .chain(std::iter::once("tail".into()))
                            .collect();
                        work.push(Work::Make("sum".into(), labels));
                        work.push(Work::RowTail(tail, args.clone()));
                        for case in cases.values().rev() {
                            match case {
                                SumCase::Written {
                                    payload: Some(payload),
                                    ..
                                } => work.push(Work::Type(payload, args.clone())),
                                SumCase::Written { payload: None, .. } => {
                                    work.push(Work::Atom("Unit".into()))
                                }
                                SumCase::Absent { .. } => work.push(Work::Atom("?".into())),
                            }
                        }
                    }
                    TypeKind::Arrow { from, to, effects } => {
                        work.push(Work::Make(
                            "arrow".into(),
                            vec!["from".into(), "to".into(), "effects".into()],
                        ));
                        work.push(Work::EffectRow(effects, args.clone()));
                        work.push(Work::Type(to, args.clone()));
                        work.push(Work::Type(from, args));
                    }
                    TypeKind::Ident(symbol) => work.push(Work::Named(*symbol, Vec::new())),
                    TypeKind::Apply {
                        head,
                        args: applied,
                        ..
                    } => {
                        work.push(Work::Apply(*head, applied.len()));
                        work.extend(
                            applied
                                .iter()
                                .rev()
                                .map(|argument| Work::Type(argument, args.clone())),
                        );
                    }
                    TypeKind::Param { index, .. } => {
                        values.push(self.argument(&args, *index));
                    }
                    TypeKind::Prim(prim) => values.push(self.atom(format!("{prim:?}"))),
                    TypeKind::Effects(row) => work.push(Work::EffectRow(row, args)),
                    TypeKind::Var(_) | TypeKind::Hole | TypeKind::Error => {
                        values.push(self.atom("?"));
                    }
                },
                Work::CoreTail(tail, args) => {
                    let value = match tail.as_ref().map(|tail| &tail.of) {
                        None => self.atom("Unit"),
                        Some(Row::Param { index, .. }) => self.argument(&args, *index),
                        Some(Row::Anything | Row::Named(_)) => self.atom("?"),
                    };
                    values.push(value);
                }
                Work::RowTail(tail, args) => {
                    let value = match tail.as_ref().map(|tail| &tail.of) {
                        None => self.atom("closed"),
                        Some(Row::Param { index, .. }) => self.argument(&args, *index),
                        Some(Row::Anything | Row::Named(_)) => self.atom("?"),
                    };
                    values.push(value);
                }
                Work::EffectRow(row, args) => {
                    let labels = std::iter::repeat_n("effect".to_string(), row.effects.len())
                        .chain(std::iter::once("tail".into()))
                        .collect();
                    work.push(Work::Make("effects".into(), labels));
                    work.push(Work::RowTail(&row.tail, args.clone()));
                    for label in row.effects.values().rev() {
                        let presence = match label {
                            EffectLabel::Written { when, .. } => {
                                canonical_written_presence(when, presence_scope)
                            }
                            EffectLabel::Absent { .. } => "\\".into(),
                        };
                        work.push(Work::EffectCase(presence, label.args().len()));
                        // The arguments follow the identity and payload, in
                        // order, each a type in the scope the row is in.
                        work.extend(
                            label
                                .args()
                                .iter()
                                .rev()
                                .map(|arg| Work::Type(arg, args.clone())),
                        );
                        work.push(Work::Atom(
                            if matches!(label, EffectLabel::Absent { .. }) {
                                "?"
                            } else {
                                "Unit"
                            }
                            .into(),
                        ));
                        work.push(Work::Canonical(self.effect_interfaces[&label.symbol()]));
                    }
                }
                Work::Apply(symbol, count) => {
                    let start = values.len() - count;
                    let args = values.split_off(start);
                    work.push(Work::Named(symbol, args));
                }
                Work::Named(symbol, args) => {
                    let key = (symbol, args.clone(), false);
                    if let Some(id) = self.named.get(&key) {
                        values.push(*id);
                    } else if let Some(decl) = self.types.get(&symbol) {
                        let id = self.placeholder();
                        self.named.insert(key, id);
                        work.push(Work::FinishNamed(id));
                        work.push(Work::Type(&decl.value, args));
                    } else {
                        // Imported semantic trees have their own iterative
                        // continuation walk and share this graph's named memo.
                        let id = self.semantic_work(symbol, args, presence_scope);
                        values.push(id);
                    }
                }
                Work::FinishNamed(id) => {
                    let body = values.pop().expect("named bodies produce one graph node");
                    // Bare aliases were rejected during recursion
                    // classification, so a local declaration always reaches a
                    // distinct body node.
                    self.arena.nodes[id] = self.arena.nodes[body].clone();
                    values.push(id);
                }
            }
        }
        values.pop().expect("a source type has one graph root")
    }

    /// Whether `needle` is structurally retained below `root`. Substitution
    /// puts an argument's existing graph node below every constructor wrapped
    /// around it, so this exact reachability check recognizes growth without a
    /// depth bound or a recursive host-stack walk.
    fn retains(nodes: &[RegularNode], needle: usize, root: usize) -> bool {
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        while let Some(node) = pending.pop() {
            if node == needle {
                return true;
            }
            if seen.insert(node) {
                pending.extend(nodes[node].edges.iter().map(|(_, child)| *child));
            }
        }
        false
    }

    /// A recursive application grows when every argument from an earlier
    /// instantiation remains at its corresponding position and at least one is
    /// now below a constructor in the SCC quotient. Distinct roots of one
    /// recursive component mutually retain each other and are therefore equal
    /// at this level: finite permutations of such roots must reach their exact
    /// memo key instead of being absorbed as growth.
    ///
    fn grows(nodes: &[RegularNode], before: &[usize], after: &[usize]) -> bool {
        before.len() == after.len()
            && before
                .iter()
                .zip(after)
                .all(|(before, after)| RegularType::retains(nodes, *before, *after))
            && before
                .iter()
                .zip(after)
                .any(|(before, after)| !RegularType::retains(nodes, *after, *before))
    }

    /// Build imported semantic types without borrowing the host stack. The
    /// imported interface is untrusted: besides ordinary deep arrows and rows,
    /// it may contain a growing recursive application. Hash-consed argument
    /// nodes make finite regular applications meet `named`; structural
    /// embedding rejects only an active constructor-growing application.
    fn semantic_work(
        &mut self,
        symbol: Symbol,
        args: Vec<usize>,
        presence_scope: &mut CanonicalPresenceScope,
    ) -> usize {
        use crate::types::{Presence, Rest};

        enum Work<'a> {
            Type(&'a Ty, Vec<usize>, bool, usize),
            Row(&'a crate::types::Row, Vec<usize>, bool, bool, usize),
            Named(Symbol, Vec<usize>, bool),
            Apply(Symbol, usize, bool),
            FinishNamed(usize, Symbol),
            Make(String, Vec<String>),
            EffectCase(String, usize),
            Canonical(usize),
            Atom(String),
            Argument(Vec<usize>, u32),
        }

        let mut work = vec![Work::Named(symbol, args, false)];
        let mut values = Vec::new();
        let mut active_instantiations: HashMap<Symbol, Vec<Vec<usize>>> = HashMap::new();
        // A placeholder only has to survive when an in-progress recursive edge
        // actually observed it. Pure forwarding aliases can otherwise publish
        // their body's interned node directly, so a fresh `Id a = a`
        // placeholder does not make `Loop (Id a)` a new instantiation forever.
        let mut referenced_placeholders = HashSet::new();

        while let Some(part) = work.pop() {
            match part {
                Work::Atom(label) => values.push(self.atom(label)),
                Work::Canonical(node) => values.push(node),
                Work::Argument(args, index) => values.push(self.argument(&args, index)),
                Work::EffectCase(presence, arity) => {
                    let start = values.len() - arity;
                    let args = values.split_off(start);
                    let payload = values.pop().expect("an effect case has a payload");
                    let identity = values.pop().expect("an effect case has an identity");
                    let edges = [("identity".into(), identity), ("payload".into(), payload)]
                        .into_iter()
                        .chain(
                            args.into_iter()
                                .enumerate()
                                .map(|(at, arg)| (format!("arg:{at}"), arg)),
                        )
                        .collect();
                    values.push(self.node(format!("effect-case:{presence}"), edges));
                }
                Work::Make(label, edge_labels) => {
                    let mut children = Vec::with_capacity(edge_labels.len());
                    for _ in 0..edge_labels.len() {
                        children.push(values.pop().expect("semantic graph postorder is balanced"));
                    }
                    children.reverse();
                    values.push(self.node(label, edge_labels.into_iter().zip(children).collect()));
                }
                Work::Type(ty, args, supplied_as_effects, instantiation) => match ty {
                    Ty::Nat => values.push(self.atom("Nat")),
                    Ty::Int => values.push(self.atom("Int")),
                    Ty::Real => values.push(self.atom("Real")),
                    Ty::String => values.push(self.atom("String")),
                    Ty::Boolean => values.push(self.atom("Boolean")),
                    Ty::Bound(index) => values.push(self.argument(&args, *index)),
                    Ty::Var(_) | Ty::Rigid { .. } | Ty::Undecided => values.push(self.atom("?")),
                    Ty::Package(body) => {
                        work.push(Work::Type(body, args, supplied_as_effects, instantiation));
                    }
                    Ty::Array(element) => {
                        work.push(Work::Make("array".into(), vec!["element".into()]));
                        work.push(Work::Type(
                            element,
                            args,
                            supplied_as_effects,
                            instantiation,
                        ));
                    }
                    Ty::Arrow(from, to, effects) => {
                        work.push(Work::Make(
                            "arrow".into(),
                            vec!["from".into(), "to".into(), "effects".into()],
                        ));
                        work.push(Work::Row(effects, args.clone(), true, false, instantiation));
                        work.push(Work::Type(to, args.clone(), false, instantiation));
                        work.push(Work::Type(from, args, false, instantiation));
                    }
                    Ty::Struct(row)
                        if row.labels.is_empty() && matches!(row.rest, Rest::Closed) =>
                    {
                        values.push(self.atom("Unit"));
                    }
                    Ty::Struct(row) => work.push(Work::Row(row, args, false, true, instantiation)),
                    Ty::Sum(row) => work.push(Work::Row(
                        row,
                        args,
                        supplied_as_effects,
                        false,
                        instantiation,
                    )),
                    Ty::Named {
                        symbol,
                        args: applied,
                        ..
                    } => {
                        work.push(Work::Apply(*symbol, applied.len(), supplied_as_effects));
                        work.extend(applied.iter().enumerate().rev().map(|(index, argument)| {
                            let effects = self
                                .external_types
                                .get(symbol)
                                .and_then(|declaration| declaration.params.get(index))
                                .is_some_and(|kind| matches!(kind, ParamKind::Effects { .. }));
                            Work::Type(argument, args.clone(), effects, instantiation)
                        }));
                    }
                },
                Work::Apply(symbol, count, supplied_as_effects) => {
                    let mut args = Vec::with_capacity(count);
                    for _ in 0..count {
                        args.push(values.pop().expect("application postorder is balanced"));
                    }
                    args.reverse();
                    work.push(Work::Named(symbol, args, supplied_as_effects));
                }
                Work::Named(symbol, args, supplied_as_effects) => {
                    let key = (symbol, args.clone(), supplied_as_effects);
                    if let Some(id) = self.named.get(&key) {
                        referenced_placeholders.insert(*id);
                        values.push(*id);
                        continue;
                    }
                    if active_instantiations.get(&symbol).is_some_and(|active| {
                        active
                            .iter()
                            .any(|ancestor| Self::grows(&self.arena.nodes, ancestor, &args))
                    }) {
                        values.push(self.atom("?"));
                        continue;
                    }
                    active_instantiations
                        .entry(symbol)
                        .or_default()
                        .push(args.clone());
                    let id = self.placeholder();
                    self.named.insert(key, id);
                    // Import installs a real or qualified recovery declaration
                    // for every type symbol reachable from an artifact.
                    let decl = &self.external_types[&symbol];
                    if let Some(name) = &decl.unresolved {
                        let edges = args
                            .iter()
                            .copied()
                            .enumerate()
                            .map(|(index, argument)| (format!("arg:{index}"), argument))
                            .collect();
                        let body = self.node(format!("unresolved:{name}"), edges);
                        self.arena.nodes[id] = self.arena.nodes[body].clone();
                        active_instantiations
                            .get_mut(&symbol)
                            .expect("the unresolved instantiation is active")
                            .pop();
                        values.push(id);
                    } else {
                        work.push(Work::FinishNamed(id, symbol));
                        let instantiation = presence_scope.instantiation();
                        work.push(Work::Type(
                            decl.scheme.body(),
                            args,
                            supplied_as_effects,
                            instantiation,
                        ));
                    }
                }
                Work::FinishNamed(id, symbol) => {
                    let body = values.pop().expect("named body postorder is balanced");
                    let completed: Vec<_> = self
                        .named
                        .iter()
                        .filter_map(|(key, node)| (*node == id).then_some(key.clone()))
                        .collect();
                    let result = if body != id && !referenced_placeholders.contains(&id) {
                        for named in self.named.values_mut() {
                            if *named == id {
                                *named = body;
                            }
                        }
                        body
                    } else {
                        if body != id {
                            self.arena.nodes[id] = self.arena.nodes[body].clone();
                        }
                        id
                    };
                    for key in completed {
                        self.named.remove(&key);
                    }
                    active_instantiations
                        .get_mut(&symbol)
                        .expect("the finished instantiation is active")
                        .pop();
                    values.push(result);
                }
                Work::Row(row, args, effects, fields, instantiation) => {
                    let labels = row
                        .labels
                        .iter()
                        .map(|(name, field)| {
                            if effects {
                                "effect".to_string()
                            } else {
                                let presence = canonical_semantic_presence(
                                    &field.presence,
                                    instantiation,
                                    presence_scope,
                                );
                                format!(
                                    "{}:{name}:{presence}",
                                    if fields { "field" } else { "label" }
                                )
                            }
                        })
                        .chain(std::iter::once(if fields {
                            "core".into()
                        } else {
                            "tail".into()
                        }))
                        .collect();
                    work.push(Work::Make(
                        if fields {
                            "fields".into()
                        } else if effects {
                            "effects".into()
                        } else {
                            "sum".into()
                        },
                        labels,
                    ));
                    match &row.rest {
                        Rest::Closed => {
                            work.push(Work::Atom(if fields { "Unit" } else { "closed" }.into()))
                        }
                        Rest::Bound(index) => {
                            work.push(Work::Argument(args.clone(), *index));
                        }
                        Rest::Var(_) | Rest::Rigid { .. } | Rest::Undecided => {
                            work.push(Work::Atom("?".into()))
                        }
                        Rest::More(more) => work.push(Work::Row(
                            more,
                            args.clone(),
                            effects,
                            fields,
                            instantiation,
                        )),
                    }
                    for (name, field) in row.labels.iter().rev() {
                        if effects {
                            let presence = canonical_semantic_presence(
                                &field.presence,
                                instantiation,
                                presence_scope,
                            );
                            let identity = self.arena.effect_key(name);
                            work.push(Work::EffectCase(presence, 0));
                            if matches!(field.presence, Presence::Absent) {
                                work.push(Work::Atom("?".into()));
                            } else {
                                work.push(Work::Type(
                                    &field.ty,
                                    args.clone(),
                                    false,
                                    instantiation,
                                ));
                            }
                            work.push(Work::Canonical(identity));
                        } else if matches!(field.presence, Presence::Absent) {
                            work.push(Work::Atom("?".into()));
                        } else {
                            work.push(Work::Type(&field.ty, args.clone(), false, instantiation));
                        }
                    }
                }
            }
        }
        values.pop().expect("a semantic graph has one root")
    }

    /// Eliminate row-composition edges after the complete regular graph has
    /// been built. A named tail may have been a placeholder when its caller
    /// was visited, so doing this while lowering would make normalization
    /// depend on declaration order. Epsilon closure also makes recursive row
    /// graphs finite: each row node contributes its labels at most once.
    fn flatten_rows(&mut self) {
        let original = self.arena.nodes.clone();
        let mut effect_keys = HashMap::new();
        for root in self.created.iter().copied() {
            let (kind, join) = match &original[root].label {
                RegularLabel::Ordinary(label) if label == "fields" => ("fields", "core"),
                RegularLabel::Ordinary(label) if label == "sum" => ("sum", "tail"),
                RegularLabel::Ordinary(label) if label == "effects" => ("effects", "tail"),
                _ => continue,
            };
            let mut pending = vec![root];
            let mut seen = HashSet::new();
            let mut claimed = HashSet::new();
            let mut edges = Vec::new();
            while let Some(node) = pending.pop() {
                if !seen.insert(node) {
                    continue;
                }
                for (label, child) in &original[node].edges {
                    if label == join
                        && original[*child].label == RegularLabel::Ordinary(kind.into())
                    {
                        pending.push(*child);
                    } else if label == join {
                        edges.push((label.clone(), *child));
                    } else {
                        // Presence is the final colon-delimited component.
                        // Claiming the semantic label before inspecting it is
                        // the row rule: an outer absent edge masks an inner
                        // present one just as surely as an outer present does.
                        let key = if kind == "effects" {
                            let identity = original[*child]
                                .edges
                                .iter()
                                .find_map(|(edge, identity)| {
                                    (edge == "identity").then_some(*identity)
                                })
                                .expect("effect cases have an identity");
                            effect_keys
                                .entry(identity)
                                .or_insert_with(|| encode_regular_graph(&original, identity))
                                .clone()
                        } else {
                            label
                                .rsplit_once(':')
                                .map_or(label.as_str(), |(key, _)| key)
                                .to_string()
                        };
                        if claimed.insert(key) {
                            edges.push((label.clone(), *child));
                        }
                    }
                }
            }
            // Graph encoding is order independent, but stable storage keeps
            // duplicate labels and multiple distinct exits deterministic too.
            edges.sort();
            self.arena.nodes[root].edges = edges;
        }
    }

    fn finish(&mut self, root: usize) -> usize {
        self.flatten_rows();
        root
    }
}

fn encode_regular_graph(nodes: &[RegularNode], root: usize) -> String {
    let mut reachable = Vec::new();
    let mut ids = HashMap::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if ids.contains_key(&node) {
            continue;
        }
        ids.insert(node, reachable.len());
        reachable.push(node);
        pending.extend(nodes[node].edges.iter().rev().map(|(_, child)| *child));
    }
    let dense: Vec<_> = reachable
        .iter()
        .map(|node| RegularNode {
            label: nodes[*node].label.clone(),
            edges: nodes[*node]
                .edges
                .iter()
                .map(|(label, child)| (label.clone(), ids[child]))
                .collect(),
        })
        .collect();
    encode_dense_graph(&dense, ids[&root])
}

fn encode_dense_graph(nodes: &[RegularNode], root: usize) -> String {
    // Acyclic imported types are by far the common deep case. Classify
    // those bottom-up in one pass; repeated whole-graph refinement would
    // take quadratic time on a 30,000-arrow signature. Regular recursive
    // graphs retain the bisimulation refinement below.
    let mut remaining: Vec<_> = nodes.iter().map(|node| node.edges.len()).collect();
    let mut parents = vec![Vec::new(); nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for (_, child) in &node.edges {
            parents[*child].push(parent);
        }
    }
    let mut pending: Vec<_> = remaining
        .iter()
        .enumerate()
        .filter_map(|(node, count)| (*count == 0).then_some(node))
        .collect();
    let mut dag_colors = vec![usize::MAX; nodes.len()];
    let mut color_base = 0;
    let mut classified = 0;
    while !pending.is_empty() {
        let wave = std::mem::take(&mut pending);
        let signatures: Vec<_> = wave
            .iter()
            .map(|node| {
                let mut edges: Vec<_> = nodes[*node]
                    .edges
                    .iter()
                    .map(|(label, child)| (label.clone(), dag_colors[*child]))
                    .collect();
                edges.sort();
                (nodes[*node].label.clone(), edges)
            })
            .collect();
        let wave_colors = ranks(signatures);
        let classes = wave_colors
            .iter()
            .copied()
            .max()
            .map_or(0, |color| color + 1);
        for (node, color) in wave.into_iter().zip(wave_colors) {
            dag_colors[node] = color_base + color;
            classified += 1;
            for parent in &parents[node] {
                remaining[*parent] -= 1;
                if remaining[*parent] == 0 {
                    pending.push(*parent);
                }
            }
        }
        color_base += classes;
    }

    let mut colors = if classified == nodes.len() {
        dag_colors
    } else {
        ranks(
            nodes
                .iter()
                .map(|node| (node.label.clone(), Vec::<(String, usize)>::new()))
                .collect(),
        )
    };
    if classified != nodes.len() {
        loop {
            let signatures: Vec<_> = nodes
                .iter()
                .enumerate()
                .map(|(id, node)| {
                    let mut edges: Vec<_> = node
                        .edges
                        .iter()
                        .map(|(label, to)| (label.clone(), colors[*to]))
                        .collect();
                    edges.sort();
                    (colors[id], node.label.clone(), edges)
                })
                .collect();
            let next = ranks(signatures);
            let old_classes = colors.iter().copied().max().map_or(0, |n| n + 1);
            let new_classes = next.iter().copied().max().map_or(0, |n| n + 1);
            colors = next;
            if old_classes == new_classes {
                break;
            }
        }
    }
    let mut representatives = vec![0; colors.iter().copied().max().map_or(0, |n| n + 1)];
    for (node, color) in colors.iter().copied().enumerate() {
        representatives[color] = node;
    }
    let mut ids = HashMap::new();
    let mut order = Vec::new();
    let mut pending = vec![colors[root]];
    while let Some(color) = pending.pop() {
        if ids.contains_key(&color) {
            continue;
        }
        ids.insert(color, order.len());
        order.push(color);
        let mut edges: Vec<_> = nodes[representatives[color]]
            .edges
            .iter()
            .map(|(label, to)| (label, colors[*to]))
            .collect();
        edges.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        pending.extend(edges.into_iter().rev().map(|(_, child)| child));
    }
    let mut out = String::new();
    for color in order {
        let node = &nodes[representatives[color]];
        out.push_str(&format!("{}#", ids[&color]));
        match &node.label {
            RegularLabel::Ordinary(label) => {
                out.push_str(&format!("{}:{}", label.len(), label));
            }
            RegularLabel::OpaqueInterface(value) => {
                out.push_str(&format!("o{}:{}", value.len(), value));
            }
            RegularLabel::UnknownInterface => out.push('u'),
        }
        let mut edges: Vec<_> = node
            .edges
            .iter()
            .map(|(label, to)| (label, colors[*to]))
            .collect();
        edges.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (label, child) in edges {
            out.push_str(&format!("|{}:{}>{}", label.len(), label, ids[&child]));
        }
        out.push(';');
    }
    out
}

fn ranks<T: Ord + Clone>(values: Vec<T>) -> Vec<usize> {
    let mut sorted = values.clone();
    sorted.sort();
    sorted.dedup();
    values
        .iter()
        .map(|value| sorted.binary_search(value).expect("ranked value"))
        .collect()
}

fn canonical_semantic_presence(
    presence: &Presence,
    instantiation: usize,
    scope: &mut CanonicalPresenceScope,
) -> String {
    let key = match presence {
        Presence::Present => return "+".into(),
        Presence::Absent => return "\\".into(),
        Presence::Undecided => return "?".into(),
        Presence::Bound(index) => SemanticPresenceKey::Bound {
            instantiation,
            index: *index,
        },
        Presence::Var(id) | Presence::Recovered(id) => SemanticPresenceKey::Recovered {
            instantiation,
            id: *id,
        },
    };
    let index = *scope.semantic.entry(key).or_insert_with(|| {
        let index = scope.next_alpha;
        scope.next_alpha += 1;
        index
    });
    format!("?{index}")
}

fn canonical_written_presence(
    when: &Option<Box<When>>,
    scope: &mut CanonicalPresenceScope,
) -> String {
    let Some(when) = when else {
        return "+".to_string();
    };
    let index = match &when.name {
        Some(name) => *scope.written.entry(name.clone()).or_insert_with(|| {
            let index = scope.next_alpha;
            scope.next_alpha += 1;
            index
        }),
        None => {
            let index = scope.next_alpha;
            scope.next_alpha += 1;
            index
        }
    };
    format!("?{index}")
}

/// Collect all source effect labels reached by a type's regular local named
/// graph. A label does not depend on type arguments, so each declaration body
/// needs visiting only once even when applications form a cycle.
fn type_effect_dependencies<'a>(
    root: &'a Type,
    types: &'a IndexMap<Symbol, Decl<Type>>,
) -> Vec<Symbol> {
    let mut pending = vec![root];
    let mut named_seen = HashSet::new();
    let mut dependencies = Vec::new();
    while let Some(ty) = pending.pop() {
        match &ty.tracked {
            TypeKind::Array(element) => pending.push(element),
            TypeKind::Struct { fields, .. } => {
                pending.extend(fields.values().rev().filter_map(|field| match field {
                    TypeField::Written { value, .. } => Some(value),
                    TypeField::Absent { .. } => None,
                }));
            }
            TypeKind::Sum { cases, .. } => {
                pending.extend(cases.values().rev().filter_map(|case| match case {
                    SumCase::Written {
                        payload: Some(payload),
                        ..
                    } => Some(payload),
                    SumCase::Written { payload: None, .. } | SumCase::Absent { .. } => None,
                }));
            }
            TypeKind::Arrow { from, to, effects } => {
                dependencies.extend(effects.effects.values().map(EffectLabel::symbol));
                pending.extend(
                    effects
                        .effects
                        .values()
                        .flat_map(|label| label.args().iter().rev()),
                );
                pending.push(to);
                pending.push(from);
            }
            TypeKind::Ident(symbol) => {
                if named_seen.insert(*symbol)
                    && let Some(decl) = types.get(symbol)
                {
                    pending.push(&decl.value);
                }
            }
            TypeKind::Apply {
                head,
                args: applied,
                ..
            } => {
                if named_seen.insert(*head)
                    && let Some(decl) = types.get(head)
                {
                    pending.push(&decl.value);
                }
                pending.extend(applied.iter().rev());
            }
            TypeKind::Effects(effects) => {
                dependencies.extend(effects.effects.values().map(EffectLabel::symbol));
                pending.extend(
                    effects
                        .effects
                        .values()
                        .flat_map(|label| label.args().iter().rev()),
                );
            }
            TypeKind::Param { .. }
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole
            | TypeKind::Error => {}
        }
    }
    dependencies
}

#[derive(Clone, Copy)]
struct CanonicalValue {
    node: usize,
}

/// Canonicalize the complete effect/type/effect dependency graph with one
/// shared exact graph. Effect placeholders are real graph backreferences, so
/// recursive identity does not depend on a finite unrolling depth.
struct EffectCanonicalizer<'a> {
    types: &'a IndexMap<Symbol, Decl<Type>>,
    effects: &'a IndexMap<Symbol, Decl<Effect>>,
    external_types: &'a IndexMap<Symbol, ExternalType>,
    effect_ids: &'a IndexMap<Symbol, EffectId>,
    mint: &'a Mint,
    active: HashMap<Symbol, usize>,
    cache: HashMap<Symbol, CanonicalValue>,
    arena: CanonicalArena,
    presence_scopes: HashMap<usize, CanonicalPresenceScope>,
    next_presence_scope: usize,
}

impl<'a> EffectCanonicalizer<'a> {
    fn new(
        types: &'a IndexMap<Symbol, Decl<Type>>,
        effects: &'a IndexMap<Symbol, Decl<Effect>>,
        external_types: &'a IndexMap<Symbol, ExternalType>,
        effect_ids: &'a IndexMap<Symbol, EffectId>,
        mint: &'a Mint,
    ) -> Self {
        Self {
            types,
            effects,
            external_types,
            effect_ids,
            mint,
            active: HashMap::new(),
            cache: HashMap::new(),
            arena: CanonicalArena::default(),
            presence_scopes: HashMap::new(),
            next_presence_scope: 0,
        }
    }

    fn name(&self, symbol: Symbol) -> &str {
        match self.effect_ids.get(&symbol) {
            Some(EffectId::Structural { name, .. }) => name,
            _ => self.mint.name(symbol),
        }
    }

    fn canonical_interface(
        &mut self,
        params: &'a [Param],
        operations: &'a IndexMap<OperationSelector, Operation>,
    ) -> String {
        let root = self.run(CanonicalWork::Interface(params, operations)).node;
        self.arena.encode(root)
    }

    /// The edges an interface node carries for its parameters: how many, and
    /// what each stands for. Names are nowhere in it; positions are, through
    /// the `param:{index}` atoms the operation types are built over.
    fn parameter_edges(&mut self, params: &[Param]) -> Vec<(String, usize)> {
        std::iter::once((
            "arity".to_string(),
            self.arena.atom(params.len().to_string()),
        ))
        .chain(params.iter().enumerate().map(|(index, param)| {
            (
                format!("param:{index}"),
                self.arena.atom(format!("{:?}", param.kind.sense())),
            )
        }))
        .collect()
    }

    fn run(&mut self, root: CanonicalWork<'a>) -> CanonicalValue {
        let mut work = vec![root];
        let mut values: Vec<CanonicalValue> = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                CanonicalWork::Interface(params, operations) => {
                    let names = operations.keys().map(|name| name.canonical()).collect();
                    let edges = self.parameter_edges(params);
                    work.push(CanonicalWork::FinishInterface(edges, names));
                    let count = params.len();
                    work.extend(
                        operations
                            .values()
                            .rev()
                            .map(|operation| CanonicalWork::Operation(operation, count)),
                    );
                }
                CanonicalWork::Operation(operation, count) => {
                    let scope = self.next_presence_scope;
                    self.next_presence_scope += 1;
                    self.presence_scopes
                        .insert(scope, CanonicalPresenceScope::default());
                    work.push(CanonicalWork::Type(&operation.to, scope, count));
                    work.push(CanonicalWork::Type(&operation.from, scope, count));
                }
                CanonicalWork::FinishInterface(mut edges, names) => {
                    let start = values.len() - names.len() * 2;
                    let mut types = values.split_off(start).into_iter();
                    for name in names {
                        let from = types.next().expect("an operation has an input");
                        let to = types.next().expect("an operation has an output");
                        edges.push((format!("operation:{name}:from"), from.node));
                        edges.push((format!("operation:{name}:to"), to.node));
                    }
                    values.push(CanonicalValue {
                        node: self.arena.node("interface", edges),
                    });
                }
                CanonicalWork::Type(ty, scope, count) => {
                    let dependencies = type_effect_dependencies(ty, self.types);
                    work.push(CanonicalWork::FinishType(
                        ty,
                        dependencies.clone(),
                        scope,
                        count,
                    ));
                    work.extend(dependencies.into_iter().rev().map(CanonicalWork::Effect));
                }
                CanonicalWork::FinishType(ty, dependencies, scope, count) => {
                    let start = values.len() - dependencies.len();
                    let resolved_values = values.split_off(start);
                    let resolved = dependencies
                        .into_iter()
                        .zip(resolved_values)
                        .map(|(symbol, value)| (symbol, value.node))
                        .collect();
                    let mut graph = RegularType {
                        types: self.types,
                        external_types: self.external_types,
                        effect_interfaces: &resolved,
                        arena: &mut self.arena,
                        created: Vec::new(),
                        interned: HashMap::new(),
                        named: HashMap::new(),
                    };
                    let params = graph.parameters(count);
                    let root = graph.source(
                        ty,
                        &params,
                        self.presence_scopes
                            .get_mut(&scope)
                            .expect("an operation presence scope remains live"),
                    );
                    values.push(CanonicalValue {
                        node: graph.finish(root),
                    });
                }
                CanonicalWork::Effect(symbol) => {
                    let name = self.name(symbol).to_string();
                    if let Some(node) = self.active.get(&symbol) {
                        values.push(CanonicalValue { node: *node });
                    } else if let Some(value) = self.cache.get(&symbol) {
                        values.push(*value);
                    } else {
                        let placeholder = self.arena.atom("pending-effect");
                        self.active.insert(symbol, placeholder);
                        match self
                            .effects
                            .get(&symbol)
                            .map(|decl| (&decl.params, &decl.value))
                        {
                            Some((params, Effect::Operations(operations))) => {
                                let names =
                                    operations.keys().map(|name| name.canonical()).collect();
                                let edges = self.parameter_edges(params);
                                work.push(CanonicalWork::FinishOperations(
                                    symbol, name, edges, names,
                                ));
                                let count = params.len();
                                work.extend(
                                    operations.values().rev().map(|operation| {
                                        CanonicalWork::Operation(operation, count)
                                    }),
                                );
                            }
                            Some((_, Effect::Alias(alias))) => {
                                let cases = &alias.body.cases;
                                work.push(CanonicalWork::FinishAlias(symbol, name, cases.len()));
                                work.extend(
                                    cases
                                        .iter()
                                        .rev()
                                        .map(|case| CanonicalWork::Effect(case.symbol)),
                                );
                            }
                            None => {
                                let interface = match self.effect_ids.get(&symbol) {
                                    Some(EffectId::Structural { interface, .. }) => interface,
                                    _ => "?",
                                };
                                let interface = self.arena.import(interface);
                                self.finish_effect(
                                    symbol,
                                    name,
                                    CanonicalValue { node: interface },
                                    &mut values,
                                );
                            }
                        }
                    }
                }
                CanonicalWork::FinishOperations(symbol, name, mut edges, names) => {
                    let start = values.len() - names.len() * 2;
                    let mut types = values.split_off(start).into_iter();
                    for name in names {
                        let from = types.next().expect("an operation has an input");
                        let to = types.next().expect("an operation has an output");
                        edges.push((format!("operation:{name}:from"), from.node));
                        edges.push((format!("operation:{name}:to"), to.node));
                    }
                    let interface = CanonicalValue {
                        node: self.arena.node("interface", edges),
                    };
                    self.finish_effect(symbol, name, interface, &mut values);
                }
                CanonicalWork::FinishAlias(symbol, name, count) => {
                    let start = values.len() - count;
                    let named = values.split_off(start);
                    let interface = CanonicalValue {
                        node: self.arena.node(
                            "alias-interface",
                            named
                                .into_iter()
                                .map(|value| ("effect".into(), value.node))
                                .collect(),
                        ),
                    };
                    self.finish_effect(symbol, name, interface, &mut values);
                }
            }
        }
        values.pop().expect("canonical work produces one value")
    }

    fn finish_effect(
        &mut self,
        symbol: Symbol,
        name: String,
        interface: CanonicalValue,
        values: &mut Vec<CanonicalValue>,
    ) {
        let node = self
            .active
            .remove(&symbol)
            .expect("finishing effects have a placeholder");
        self.arena.nodes[node] = RegularNode {
            label: RegularLabel::Ordinary(format!("effect:{name}")),
            edges: vec![("interface".into(), interface.node)],
        };
        let value = CanonicalValue { node };
        self.cache.insert(symbol, value);
        values.push(value);
    }
}

enum CanonicalWork<'a> {
    Interface(&'a [Param], &'a IndexMap<OperationSelector, Operation>),
    FinishInterface(Vec<(String, usize)>, Vec<String>),
    /// An operation, with how many parameters its effect binds.
    Operation(&'a Operation, usize),
    Type(&'a Type, usize, usize),
    Effect(Symbol),
    FinishType(&'a Type, Vec<Symbol>, usize, usize),
    FinishOperations(Symbol, String, Vec<(String, usize)>, Vec<String>),
    FinishAlias(Symbol, String, usize),
}

fn rekey_row(row: &mut EffectRow, ids: &IndexMap<Symbol, EffectId>, errors: &mut Vec<Error>) {
    let old = std::mem::take(&mut row.effects);
    for (_, mut label) in old {
        // An argument is a written type like any other, rows included.
        for arg in label.args_mut() {
            rekey_type(arg, ids, errors);
        }
        let Some(id) = ids.get(&label.symbol()) else {
            continue;
        }; // aliases never survive expansion
        if let Some(previous) = row.effects.get(id) {
            errors.push(Error {
                span: label.name_span(),
                kind: ErrorKind::DuplicateCase {
                    shape: Shape::Effect,
                    name: id.name().to_string(),
                    previous: previous.name_span(),
                },
            });
        } else {
            row.effects.insert(id.clone(), label);
        }
    }
}

fn rekey_type(ty: &mut Type, ids: &IndexMap<Symbol, EffectId>, errors: &mut Vec<Error>) {
    match &mut ty.tracked {
        TypeKind::Struct { fields, .. } => {
            for field in fields.values_mut() {
                if let TypeField::Written { value, .. } = field {
                    rekey_type(value, ids, errors);
                }
            }
        }
        TypeKind::Sum { cases, .. } => {
            for case in cases.values_mut() {
                if let SumCase::Written {
                    payload: Some(payload),
                    ..
                } = case
                {
                    rekey_type(payload, ids, errors);
                }
            }
        }
        TypeKind::Arrow { from, to, effects } => {
            rekey_type(from, ids, errors);
            rekey_type(to, ids, errors);
            rekey_row(effects, ids, errors);
        }
        TypeKind::Apply { args, .. } => {
            for arg in args {
                rekey_type(arg, ids, errors);
            }
        }
        TypeKind::Array(element) => rekey_type(element, ids, errors),
        TypeKind::Effects(effects) => rekey_row(effects, ids, errors),
        TypeKind::Ident(_)
        | TypeKind::Param { .. }
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => {}
    }
}

fn rekey_term(
    term: &mut Term,
    ids: &IndexMap<Symbol, EffectId>,
    operations: &HashMap<EffectId, IndexSet<OperationSelector>>,
    errors: &mut Vec<Error>,
) {
    match &mut term.kind {
        TermKind::Unary { value, .. } => rekey_term(value, ids, operations, errors),
        TermKind::Binary { left, right, .. } => {
            rekey_term(left, ids, operations, errors);
            rekey_term(right, ids, operations, errors);
        }
        TermKind::Apply { func, arg } => {
            rekey_term(func, ids, operations, errors);
            rekey_term(arg, ids, operations, errors);
        }
        TermKind::Fn { body, .. } | TermKind::Raise(body) => {
            rekey_term(body, ids, operations, errors)
        }
        TermKind::Let {
            annotation,
            value,
            body,
            ..
        } => {
            if let Some(annotation) = annotation {
                rekey_type(&mut annotation.ty, ids, errors);
            }
            rekey_term(value, ids, operations, errors);
            rekey_term(body, ids, operations, errors);
        }
        TermKind::Struct { fields, spread } => {
            for field in fields.values_mut() {
                rekey_term(&mut field.value, ids, operations, errors);
            }
            if let Some(spread) = spread {
                rekey_term(&mut spread.value, ids, operations, errors);
            }
        }
        TermKind::Array(items) => {
            for item in items {
                rekey_term(&mut item.value, ids, operations, errors);
            }
        }
        TermKind::Tag {
            payload: Some(payload),
            ..
        } => rekey_term(payload, ids, operations, errors),
        TermKind::Project { base, .. } => rekey_term(base, ids, operations, errors),
        TermKind::Match { scrutinee, arms } => {
            rekey_term(scrutinee, ids, operations, errors);
            for (_, body) in arms {
                rekey_term(body, ids, operations, errors);
            }
        }
        TermKind::Handle { body, handler } => {
            rekey_term(body, ids, operations, errors);
            for arm in &mut handler.arms {
                rekey_term(&mut arm.body, ids, operations, errors);
            }
            if let Some(ret) = &mut handler.ret {
                rekey_term(&mut ret.body, ids, operations, errors);
            }

            // Coverage is structural, not nominal. Equivalent declarations may
            // contribute different selectors to one evidence record, while a
            // repeated selector across either spelling is still one duplicate.
            let mut covered: IndexMap<
                EffectId,
                (Tracked<Symbol>, IndexMap<OperationSelector, Span>),
            > = IndexMap::new();
            let mut unique = Vec::new();
            for arm in std::mem::take(&mut handler.arms) {
                let effect = ids[&arm.effect.tracked].clone();
                let group = covered
                    .entry(effect.clone())
                    .or_insert_with(|| (arm.effect, IndexMap::new()));
                if let Some(previous) = group.1.get(&arm.selector.tracked) {
                    errors.push(Error {
                        span: arm.selector.span,
                        kind: ErrorKind::DuplicateArm {
                            effect: effect.name().to_string(),
                            selector: arm.selector.tracked,
                            previous: *previous,
                        },
                    });
                } else {
                    group
                        .1
                        .insert(arm.selector.tracked.clone(), arm.selector.span);
                    unique.push(arm);
                }
            }
            handler.arms = unique;
            handler.discharges.clear();
            for (effect, (representative, arms)) in covered {
                let missing: Vec<String> = operations[&effect]
                    .iter()
                    .filter(|selector| !arms.contains_key(*selector))
                    .map(OperationSelector::source_name)
                    .collect();
                if missing.is_empty() {
                    handler.discharges.push(representative);
                } else {
                    errors.push(Error {
                        span: representative.span,
                        kind: ErrorKind::PartialHandler {
                            effect: effect.name().to_string(),
                            missing,
                        },
                    });
                }
            }
        }
        TermKind::Tag { payload: None, .. }
        | TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// The old nominal spelling used only while aliases are expanded during IR
/// construction. It is never allowed into a semantic row.
fn source_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !matches!(
            name,
            "_" | "let"
                | "do"
                | "return"
                | "type"
                | "end"
                | "with"
                | "match"
                | "fn"
                | "effect"
                | "handle"
                | "raise"
                | "and"
                | "or"
                | "xor"
                | "not"
                | "module"
                | "true"
                | "false"
        )
}

fn effect_key(mint: &Mint, symbol: Symbol) -> String {
    let mut names = vec![mint.name(symbol).to_string()];
    let mut parent = mint.parent(symbol);
    while let Some(module) = parent {
        names.push(mint.name(module.symbol()).to_string());
        parent = mint.parent(module.symbol());
    }
    names.reverse();
    names.join("::")
}

/// What each declared effect stands for, as the concrete effects it names.
///
/// An effect that declares operations stands for itself. An alias stands for
/// the effects its cases name, and for whatever *those* stand for — which makes
/// this a fixpoint over the alias graph rather than a read of one declaration,
/// since aliases are hoisted and may name each other.
///
/// An alias that leads back to itself contributes nothing on the way round,
/// which is what keeps this finite: the sets only grow, they are bounded by the
/// effects in the program, and a loop of pure aliases simply comes to the union
/// of what its members reach outside itself. There is nothing to refuse — a
/// circular alias stands for a perfectly good set of effects, unlike a circular
/// *type*, which stands for nothing at all.
///
/// Insertion-ordered, so a row that expands an alias always names its effects
/// in the same order and a printed type reads the same on every run.
fn expansions(
    mint: &Mint,
    effects: &IndexMap<Symbol, Decl<Effect>>,
    imported: &HashMap<Symbol, IndexMap<String, Symbol>>,
    cyclic: &HashSet<Symbol>,
) -> HashMap<Symbol, IndexMap<String, Symbol>> {
    // A declaration with operations is the effect itself, whatever it declares
    // — including the empty one, which declares nothing and is still an effect
    // a row may name. An alias starts from nothing and grows below. Imported
    // expansions seed the same fixpoint so a local alias can reach through a
    // dependency alias or operation.
    let mut out = imported.clone();
    out.extend(effects.iter().map(|(symbol, decl)| {
        let stands = match &decl.value {
            Effect::Operations(_) => [(effect_key(mint, *symbol), *symbol)].into_iter().collect(),
            Effect::Alias(_) => IndexMap::new(),
        };
        (*symbol, stands)
    }));
    loop {
        let mut grew = false;
        for (symbol, decl) in effects {
            let Effect::Alias(alias) = &decl.value else {
                continue;
            };
            // An alias a cycle was reported at stands for nothing.
            if cyclic.contains(symbol) {
                continue;
            }
            let mut found: IndexMap<String, Symbol> = IndexMap::new();
            for case in &alias.body.cases {
                for (name, reached) in out.get(&case.symbol).into_iter().flatten() {
                    found.insert(name.clone(), *reached);
                }
            }
            let entry = out.get_mut(symbol).expect("every effect was seeded");
            for (name, reached) in found {
                grew |= entry.insert(name, reached).is_none();
            }
        }
        if !grew {
            return out;
        }
    }
}

/// Every declaration that leads back to itself through nothing but struct-row-tail
/// positions, in the order the loops were found, split by whether the loop adds
/// fields on the way round.
///
/// Only what a declaration stands for is followed. A type with any structure
/// to it — `type t = { next: t }`, `type t = t -> Nat` — says what it is one
/// step in, and the loop through it is the recursion this language is for; it
/// is a name standing for a name standing for the first that never says
/// anything. A name, or a parameter, which is the argument written for it:
/// `type A 'a = 'a` says no more about `type B = A B` than a bare name would,
/// because what `A` stands for is whatever it was handed.
///
/// A struct whose `..` names a parameter is such a hand-off too, and that is the
/// one thing this walk had to learn: `type WithX 'r = { x: Nat, ..'r }` stands
/// for its argument with an `x` in front of it, because the `..` is the struct-row tail.
/// So `type T = WithX T` reaches `T` again with a field added, which is a loop
/// like any other and is [`ErrorKind::EndlessFields`] rather than
/// [`ErrorKind::Circular`]. See [`Stands::Param`].
///
/// Each declaration is followed once and remembered, which is what keeps a
/// legal nesting from looking like a loop as much as it is what makes this
/// terminate: `Id (Id Nat)` never finds `Id` still open, because the first was
/// finished before the second was reached. Only the declarations *on* a loop
/// are named — one that merely leads into one has nothing to fix.
fn looping(
    types: &IndexMap<Symbol, Decl<Type>>,
    external: &IndexMap<Symbol, ExternalType>,
) -> Loops {
    let mut follow = Follow {
        types,
        external,
        done: HashMap::new(),
        open: Vec::new(),
        marks: Vec::new(),
        fielded: 0,
        looping: IndexSet::new(),
        endless: IndexSet::new(),
    };
    for symbol in types.keys() {
        follow.decl(*symbol);
    }
    Loops {
        looping: follow.looping,
        endless: follow.endless,
    }
}

impl<'a> Follow<'a> {
    /// What one declaration stands for, followed once and remembered.
    fn decl(&mut self, symbol: Symbol) -> Stands {
        let mut work = vec![FollowWork::Decl(symbol)];
        let mut answer = None;
        while let Some(next) = work.pop() {
            match next {
                FollowWork::Decl(symbol) => {
                    if let Some(stands) = self.done.get(&symbol) {
                        answer = Some(*stands);
                        continue;
                    }
                    // Meeting a declaration that is still being followed is
                    // the loop, and everything pushed since is on it. Whether a
                    // field was added is whether the count moved since its mark.
                    if let Some(at) = self.open.iter().position(|open| *open == symbol) {
                        self.looping.extend(self.open[at..].iter().copied());
                        if self.fielded > self.marks[at] {
                            self.endless.extend(self.open[at..].iter().copied());
                        }
                        answer = Some(Stands::Loop);
                        continue;
                    }
                    self.open.push(symbol);
                    self.marks.push(self.fielded);
                    work.push(FollowWork::FinishDecl(symbol));
                    match self.types.get(&symbol) {
                        Some(decl) => work.push(FollowWork::Written(&decl.value)),
                        None => {
                            let decl = self
                                .external
                                .get(&symbol)
                                .expect("every resolved imported type has a recovery interface");
                            work.push(FollowWork::Semantic(decl.scheme.body()));
                        }
                    }
                }
                FollowWork::FinishDecl(symbol) => {
                    let stands = answer.expect("a followed declaration has an answer");
                    self.open.pop();
                    self.marks.pop();
                    self.done.insert(symbol, stands);
                    answer = Some(stands);
                }
                FollowWork::Written(ty) => match &ty.tracked {
                    // A struct whose `..` names a parameter stands for that
                    // parameter, with any fields written in front of it.
                    TypeKind::Struct {
                        fields,
                        tail:
                            Some(Tail {
                                of: Row::Param { index, .. },
                                ..
                            }),
                        ..
                    } => {
                        answer = Some(Stands::Param {
                            index: *index,
                            fields: fields
                                .values()
                                .any(|field| !matches!(field, TypeField::Absent { .. })),
                        });
                    }
                    TypeKind::Struct { .. }
                    | TypeKind::Array(_)
                    | TypeKind::Sum { .. }
                    | TypeKind::Arrow { .. }
                    | TypeKind::Effects(_)
                    | TypeKind::Prim(_)
                    | TypeKind::Var(_)
                    | TypeKind::Hole
                    | TypeKind::Error => answer = Some(Stands::Shape),
                    TypeKind::Param { index, .. } => {
                        answer = Some(Stands::Param {
                            index: *index,
                            fields: false,
                        });
                    }
                    TypeKind::Ident(symbol) => work.push(FollowWork::Decl(*symbol)),
                    TypeKind::Apply { head, args, .. } => {
                        work.push(FollowWork::SelectWritten(args));
                        work.push(FollowWork::Decl(*head));
                    }
                },
                FollowWork::Semantic(ty) => match &**ty {
                    Ty::Package(body) => work.push(FollowWork::Semantic(body)),
                    Ty::Bound(index) => {
                        answer = Some(Stands::Param {
                            index: *index,
                            fields: false,
                        });
                    }
                    Ty::Struct(row) => {
                        let mut row = row;
                        let mut fields = false;
                        let mut seen = HashSet::new();
                        loop {
                            // A spliced row is flattened with outer-wins
                            // shadowing. An outer absence therefore forbids an
                            // inner presence from counting as field addition.
                            fields |= row.labels.iter().any(|(name, field)| {
                                match (
                                    seen.insert(name.clone()),
                                    matches!(field.presence, Presence::Absent),
                                ) {
                                    (true, false) => true,
                                    (true, true) | (false, true) | (false, false) => false,
                                }
                            });
                            match &row.rest {
                                Rest::Bound(index) => {
                                    answer = Some(Stands::Param {
                                        index: *index,
                                        fields,
                                    });
                                    break;
                                }
                                Rest::More(more) => row = more,
                                Rest::Closed
                                | Rest::Var(_)
                                | Rest::Rigid { .. }
                                | Rest::Undecided => {
                                    answer = Some(Stands::Shape);
                                    break;
                                }
                            }
                        }
                    }
                    Ty::Named { symbol, args, .. } => {
                        work.push(FollowWork::SelectSemantic(args));
                        work.push(FollowWork::Decl(*symbol));
                    }
                    Ty::Nat
                    | Ty::Int
                    | Ty::Real
                    | Ty::String
                    | Ty::Boolean
                    | Ty::Arrow(..)
                    | Ty::Array(_)
                    | Ty::Sum(_)
                    | Ty::Var(_)
                    | Ty::Rigid { .. }
                    | Ty::Undecided => answer = Some(Stands::Shape),
                },
                FollowWork::SelectWritten(args) => {
                    let stands = answer.expect("a forwarding head has an answer");
                    if let Stands::Param { index, fields } = stands {
                        if fields {
                            self.fielded += 1;
                        }
                        // Written applications have already passed the arity
                        // check, and imported forwarding slots are clamped to
                        // their published arity while their interface is read.
                        let arg = &args[index as usize];
                        work.push(FollowWork::FinishSelect { fields });
                        work.push(FollowWork::Written(arg));
                    }
                }
                FollowWork::SelectSemantic(args) => {
                    let stands = answer.expect("a forwarding head has an answer");
                    if let Stands::Param { index, fields } = stands {
                        if fields {
                            self.fielded += 1;
                        }
                        match args.get(index as usize) {
                            Some(arg) => {
                                work.push(FollowWork::FinishSelect { fields });
                                work.push(FollowWork::Semantic(arg));
                            }
                            None => answer = Some(Stands::Shape),
                        }
                    }
                }
                FollowWork::FinishSelect { fields } => {
                    answer = Some(match answer.expect("a selected argument has an answer") {
                        Stands::Param {
                            index,
                            fields: below,
                        } => Stands::Param {
                            index,
                            fields: fields || below,
                        },
                        stands => stands,
                    });
                }
            }
        }
        answer.expect("every declaration has a recursion-classification answer")
    }
}

/// Every definition whose value leads back to itself through nothing but names,
/// in the order the loops were found. [`looping`] about terms.
///
/// Only what a definition's value stands for is followed, and a value with any
/// structure to it stands for itself: `fn`, a struct, a tag, a call, a literal
/// all say what the definition is one step in, and a name that leads to one of
/// them has been given a value through it. `let f = fn n => f n` is the shape
/// of recursion this language is for, and it ends the chain at the `fn`. What
/// never says anything is a name standing for a name standing for the first.
///
/// A projection is a shape too, though it looks like a hand-off. `let a = b.x`
/// reads a field out of whatever `b` is, so it is not `b`; a loop closing
/// through one still describes a value, and whether it can be typed is
/// [`inference`](crate::inference)'s occurs check to answer.
///
/// A block's binding is followed too, and by the same rules: `do let x = x return x end`
/// says no more about what `x` is than `let x = x` at the top level does. What
/// a `let` expression stands for is what its *body* stands for, since a body
/// that is a bare name is a value given as that name.
fn circling(terms: &IndexMap<Symbol, Decl<Term>>) -> IndexSet<Symbol> {
    let mut locals = HashMap::new();
    for decl in terms.values() {
        nested(&decl.value, &mut locals);
    }
    // Every nested binding is followed from itself as well as from the
    // definitions, because a definition's value stops at the first `fn` it
    // meets: without this, a loop written inside a lambda would be found only
    // if something outside it happened to lead there.
    let bound: Vec<Symbol> = locals.keys().copied().collect();
    let mut chain = Chain {
        terms,
        locals,
        done: HashMap::new(),
        open: Vec::new(),
        looping: IndexSet::new(),
    };
    for symbol in terms.keys() {
        chain.def(*symbol);
    }
    for symbol in bound {
        chain.def(symbol);
    }
    chain.looping
}

/// Erase the value of every nested `let` on a loop, and collect the span each
/// is to be reported at.
///
/// [`build`]'s treatment of a circular definition, said about a binding written
/// inside one: the value is replaced by [`TermKind::Error`], which absorbs, and
/// the span reported is the value's — the same span, and the same complaint,
/// that `let x = x` gets at the top level.
///
/// Innermost first, because a loop can close through a binding written inside
/// another binding's value — `do let x = do let y = x return y end return x end` is two bindings on
/// one loop — and erasing the outer one takes the inner one's value out of the
/// program with it. Every binding on the loop is told, which is what the
/// top-level walk already does.
fn erase_circular(term: &mut Term, looping: &IndexSet<Symbol>, out: &mut Vec<Span>) {
    match &mut term.kind {
        TermKind::Let {
            name, value, body, ..
        } => {
            erase_circular(value, looping, out);
            if looping.contains(&name.tracked) {
                out.push(value.span);
                **value = TermKind::Error.with_span(value.span);
            }
            erase_circular(body, looping, out);
        }
        TermKind::Unary { value, .. } => erase_circular(value, looping, out),
        TermKind::Binary { left, right, .. } => {
            erase_circular(left, looping, out);
            erase_circular(right, looping, out);
        }
        TermKind::Apply { func, arg } => {
            erase_circular(func, looping, out);
            erase_circular(arg, looping, out);
        }
        TermKind::Fn { body, .. } => erase_circular(body, looping, out),
        TermKind::Struct { fields, spread } => {
            for field in fields.values_mut() {
                erase_circular(&mut field.value, looping, out);
            }
            if let Some(spread) = spread {
                erase_circular(&mut spread.value, looping, out);
            }
        }
        TermKind::Array(items) => {
            for item in items {
                erase_circular(&mut item.value, looping, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                erase_circular(payload, looping, out);
            }
        }
        TermKind::Project { base, .. } => erase_circular(base, looping, out),
        TermKind::Match { scrutinee, arms } => {
            erase_circular(scrutinee, looping, out);
            for (_, body) in arms {
                erase_circular(body, looping, out);
            }
        }
        TermKind::Handle { body, handler } => {
            erase_circular(body, looping, out);
            for arm in &mut handler.arms {
                erase_circular(&mut arm.body, looping, out);
            }
            if let Some(ret) = &mut handler.ret {
                erase_circular(&mut ret.body, looping, out);
            }
        }
        TermKind::Raise(value) => erase_circular(value, looping, out),
        // An operation names an effect rather than a definition, so there is
        // no value here for a loop to close through.
        TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// Every nested `let` in a term, as the symbol it binds and the value it binds
/// it to. See [`Chain::locals`].
fn nested<'a>(term: &'a Term, out: &mut HashMap<Symbol, &'a Term>) {
    match &term.kind {
        TermKind::Let {
            name, value, body, ..
        } => {
            out.insert(name.tracked, value);
            nested(value, out);
            nested(body, out);
        }
        TermKind::Unary { value, .. } => nested(value, out),
        TermKind::Binary { left, right, .. } => {
            nested(left, out);
            nested(right, out);
        }
        TermKind::Apply { func, arg } => {
            nested(func, out);
            nested(arg, out);
        }
        TermKind::Fn { body, .. } => nested(body, out),
        TermKind::Struct { fields, spread } => {
            for field in fields.values() {
                nested(&field.value, out);
            }
            if let Some(spread) = spread {
                nested(&spread.value, out);
            }
        }
        TermKind::Array(items) => {
            for item in items {
                nested(&item.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                nested(payload, out);
            }
        }
        TermKind::Project { base, .. } => nested(base, out),
        // A match binds through its patterns' binders, which — like a lambda's
        // argument — are handed values at no `let` and so have nothing here to
        // collect; only what sits inside is walked, each arm body once.
        TermKind::Match { scrutinee, arms } => {
            nested(scrutinee, out);
            for (_, body) in arms {
                nested(body, out);
            }
        }
        // A handler arm's binder is a lambda argument's twin, so — as with a
        // match — only what sits inside is walked.
        TermKind::Handle { body, handler } => {
            nested(body, out);
            for arm in &handler.arms {
                nested(&arm.body, out);
            }
            if let Some(ret) = &handler.ret {
                nested(&ret.body, out);
            }
        }
        TermKind::Raise(value) => nested(value, out),
        TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

impl Chain<'_> {
    /// What one definition stands for, followed once and remembered.
    fn def(&mut self, symbol: Symbol) -> Stands {
        if let Some(stands) = self.done.get(&symbol) {
            return *stands;
        }
        // Meeting a definition that is still being followed is the loop, and
        // everything pushed since is on it with them.
        if let Some(at) = self.open.iter().position(|open| *open == symbol) {
            self.looping.extend(self.open[at..].iter().copied());
            return Stands::Loop;
        }
        // A name written bare is a definition, a nested binding, or a lambda's
        // argument, and only the first two have a value here to follow. An
        // argument is handed one at every call site rather than given one where
        // it is written, so it ends the chain the way a shape does — there is
        // nothing about it that could lead back round.
        let value = match self.terms.get(&symbol) {
            Some(decl) => &decl.value,
            None => match self.locals.get(&symbol) {
                Some(value) => value,
                None => return Stands::Shape,
            },
        };
        self.open.push(symbol);
        let stands = self.value(value);
        self.open.pop();
        self.done.insert(symbol, stands);
        stands
    }

    /// What one value stands for. Every kind but a bare name and a nested
    /// `let` is a shape, which is the whole rule.
    fn value(&mut self, term: &Term) -> Stands {
        match &term.kind {
            TermKind::Ident(symbol) => self.def(*symbol),
            // A `let` is whatever its body is: the value it binds is beside the
            // question, and a body written as a bare name is a value given as
            // that name.
            TermKind::Let { body, .. } => self.value(body),
            // A match is a shape too, though nothing has run yet to pick an
            // arm: `let x = match x with ... end` asks something of `x` the
            // way a projection does, so the loop through it still describes a
            // value and is inference's to judge.
            TermKind::Unary { .. }
            | TermKind::Binary { .. }
            | TermKind::Apply { .. }
            | TermKind::Fn { .. }
            | TermKind::Struct { .. }
            | TermKind::Array(_)
            | TermKind::Tag { .. }
            | TermKind::Project { .. }
            | TermKind::Match { .. }
            // A handler is a shape for the reason a match is: it asks
            // something of what it handles rather than handing it on. A
            // `raise` and an operation are shapes outright — neither is a
            // name that could lead anywhere.
            | TermKind::Handle { .. }
            | TermKind::Raise(_)
            | TermKind::Operation { .. }
            | TermKind::Natural(_) | TermKind::Integer(_) | TermKind::Real(_) | TermKind::String(_) | TermKind::Boolean(_)
            | TermKind::Error => Stands::Shape,
        }
    }
}

/// The first tag or primitive literal in a normalized pattern — what a
/// complaint about a binding that can fail quotes, and where it points. `None`
/// exactly for an irrefutable pattern, which is the syntactic rule of R3: a
/// pattern is refutable iff it contains a tag or literal anywhere inside it.
fn refuter(pattern: &Pattern) -> Option<(Span, Refuter)> {
    let literal = |value| Some((pattern.span, Refuter::Literal(value)));
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Wildcard | PatternKind::Unit => None,
        PatternKind::Tag { name, .. } => Some((name.span, Refuter::Case(name.tracked.clone()))),
        PatternKind::Struct { fields, .. } => {
            fields.values().find_map(|field| refuter(&field.value))
        }
        PatternKind::Natural(value) => literal(Literal::Natural(*value)),
        PatternKind::Integer(value) => literal(Literal::Integer(*value)),
        PatternKind::Real(value) => literal(Literal::Real(*value)),
        PatternKind::String(value) => literal(Literal::String(value.clone())),
        PatternKind::Boolean(value) => literal(Literal::Boolean(*value)),
        // Length is no part of an array's type, so naming any element is a
        // test a value can fail; the lone rest names none and cannot.
        PatternKind::Array {
            before,
            rest,
            after,
        } => match before.is_empty() && after.is_empty() && rest.is_some() {
            true => None,
            false => Some((pattern.span, Refuter::Length)),
        },
    }
}

/// A normalized pattern read as one that cannot fail, or `None` when it can.
/// The `let` walk goes through this rather than matching the pattern itself,
/// so destructuring takes a value that cannot fail and meets no test it would
/// have to call unreachable.
fn calm(pattern: &Pattern) -> Option<Calm> {
    match &pattern.tracked {
        PatternKind::Bind(name) => Some(Calm::Bind(*name)),
        PatternKind::Wildcard => Some(Calm::Wildcard(pattern.span)),
        PatternKind::Unit => Some(Calm::Unit(pattern.span)),
        // `{}` and `()` are one pattern — exactly no fields — so the exact
        // empty struct reads as the unit it is and the two lower alike.
        PatternKind::Struct { fields, rest: None } if fields.is_empty() => {
            Some(Calm::Unit(pattern.span))
        }
        PatternKind::Struct { fields, rest } => {
            let mut lowered = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                lowered.push((field.name_span.track(name.clone()), calm(&field.value)?));
            }
            Some(Calm::Struct {
                span: pattern.span,
                fields: lowered,
                rest: *rest,
            })
        }
        PatternKind::Array {
            before,
            rest: Some(rest),
            after,
        } if before.is_empty() && after.is_empty() => Some(Calm::ArrayRest {
            span: pattern.span,
            name: rest.name,
        }),
        PatternKind::Tag { .. }
        | PatternKind::Array { .. }
        | PatternKind::Natural(_)
        | PatternKind::Integer(_)
        | PatternKind::Real(_)
        | PatternKind::String(_)
        | PatternKind::Boolean(_) => None,
    }
}

/// Every name a normalized pattern binds, in the order the pattern walk met
/// them. What a refused binding still has to bind — to error values — so
/// downstream uses resolve.
fn pattern_binders(pattern: &Pattern, out: &mut Vec<Tracked<Symbol>>) {
    match &pattern.tracked {
        PatternKind::Bind(name) => out.push(*name),
        PatternKind::Wildcard
        | PatternKind::Unit
        | PatternKind::Natural(_)
        | PatternKind::Integer(_)
        | PatternKind::Real(_)
        | PatternKind::String(_)
        | PatternKind::Boolean(_) => {}
        PatternKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                pattern_binders(payload, out);
            }
        }
        PatternKind::Struct { fields, .. } => {
            for field in fields.values() {
                pattern_binders(&field.value, out);
            }
        }
        PatternKind::Array {
            before,
            rest,
            after,
        } => {
            for element in before {
                pattern_binders(element, out);
            }
            if let Some(name) = rest.as_ref().and_then(|rest| rest.name) {
                out.push(name);
            }
            for element in after {
                pattern_binders(element, out);
            }
        }
    }
}

/// Bind each name to an error value around `body`, innermost last, so the
/// names resolve and absorb rather than cascade. What a refused binding's
/// names get, and what the binders of a struct pattern's dropped duplicate
/// field get: the value they would have named is no longer part of the
/// program, and one mistake should make one complaint.
fn bound_to_errors(names: Vec<Tracked<Symbol>>, body: Term) -> Term {
    let mut inner = body;
    for name in names.into_iter().rev() {
        let error = TermKind::Error.with_span(name.span);
        let at = name.span.merge(inner.span);
        inner = TermKind::Let {
            name,
            annotation: None,
            value: Box::new(error),
            body: Box::new(inner),
        }
        .with_span(at);
    }
    inner
}

/// The demand an exact struct pattern makes of the value a `let` binds it to,
/// written as an annotation: exactly the named fields — the list closed, each
/// field present — with a hole where each field's type goes, since the pattern
/// says which fields are there and nothing about what they hold. Spanned at
/// the pattern, so the solver's ordinary mismatch points at what was written.
///
/// An annotation because that is the channel a binding's demand already
/// travels: the unit pattern's `()` goes the same way, and inference needs no
/// new rule to enforce this one. The holes are [`TypeKind::Hole`], which is now
/// the reader's spelling of the same thing — a hole is there to be decided
/// whoever wrote it, so the desugar needs no mark of its own to be exempt from
/// anything.
fn exact_demand(span: Span, fields: &[(TrackedString, Calm)]) -> Annotation {
    let fields = fields
        .iter()
        .map(|(name, _)| {
            let field = TypeField::Written {
                name_span: name.span,
                when: None,
                value: name.span.track(TypeKind::Hole),
            };
            (name.tracked.clone(), field)
        })
        .collect();
    demand(span.track(TypeKind::Struct { fields, tail: None }))
}

/// The demand a lone-rest array pattern makes of the value a `let` binds it
/// to: an array, of elements the pattern says nothing about — a hole, as an
/// exact struct pattern's fields are.
fn array_demand(span: Span) -> Annotation {
    demand(span.track(TypeKind::Array(Box::new(span.track(TypeKind::Hole)))))
}

/// A type the compiler wrote itself, as the annotation it travels in.
///
/// Never a `where` clause and never a declared variable: a demand is about
/// which labels are there, one at a time, and the compiler has nothing to say
/// about their combinations that the pattern it came from did not already say
/// by naming them.
fn demand(ty: Type) -> Annotation {
    Annotation {
        ty,
        variables: Vec::new(),
        anonymous_existentials: Vec::new(),
        clause: None,
    }
}

/// A normalized pattern as the matrix walk matches it: everything that only
/// binds is the one wildcard, a bare tag carries a wildcard payload, and `{}`
/// — which reaches into nothing — is a wildcard too.
fn mat(pattern: &Pattern) -> Mat {
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Wildcard | PatternKind::Unit => Mat::Wild,
        PatternKind::Tag { name, payload } => Mat::Tag {
            name: name.tracked.clone(),
            payload: Box::new(payload.as_deref().map(mat).unwrap_or(Mat::Wild)),
        },
        PatternKind::Struct { fields, .. } if fields.is_empty() => Mat::Wild,
        PatternKind::Struct { fields, .. } => Mat::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), mat(&field.value)))
                .collect(),
        ),
        PatternKind::Natural(value) => Mat::Literal(Literal::Natural(*value)),
        PatternKind::Integer(value) => Mat::Literal(Literal::Integer(*value)),
        PatternKind::Real(value) => Mat::Literal(Literal::Real(*value)),
        PatternKind::String(value) => Mat::Literal(Literal::String(value.clone())),
        PatternKind::Boolean(value) => Mat::Literal(Literal::Boolean(*value)),
        PatternKind::Array {
            before,
            rest,
            after,
        } => match before.is_empty() && after.is_empty() && rest.is_some() {
            true => Mat::Wild,
            false => Mat::Array,
        },
    }
}

impl Matrix {
    /// Read the tests and the binder positions off one match's arms. The
    /// universes are a property of the whole written match — R7 closes a row
    /// over every arm's tests, so the checks that mirror it must draw from
    /// the same set — which is why they are collected once here rather than
    /// re-read from whatever rows a recursive step still holds.
    pub(crate) fn new<'a>(patterns: impl IntoIterator<Item = &'a Pattern>) -> Self {
        let mut matrix = Matrix {
            tests: HashMap::new(),
            binds: Vec::new(),
        };
        let mut path = Vec::new();
        for pattern in patterns {
            // A catch-all arm is irrefutable at every position, whichever
            // spelling it is — a bare name, `()`, a struct of binders — so it
            // opens the root, and the root is a prefix of everything.
            if refuter(pattern).is_none() {
                matrix.binds.push(Vec::new());
            }
            matrix.collect(pattern, &mut path);
        }
        matrix
    }

    /// One arm's contribution: its tests by position, and where it binds. A
    /// wildcard is a binder minus the name, and the name is no part of what
    /// this reads: the position is open either way.
    fn collect(&mut self, pattern: &Pattern, path: &mut Vec<Step>) {
        match &pattern.tracked {
            PatternKind::Bind(_) | PatternKind::Wildcard => self.binds.push(path.clone()),
            PatternKind::Unit => {}
            PatternKind::Tag { name, payload } => {
                self.tests
                    .entry(path.clone())
                    .or_default()
                    .tags
                    .insert(name.tracked.clone());
                if let Some(payload) = payload {
                    path.push(Step::Payload(name.tracked.clone()));
                    self.collect(payload, path);
                    path.pop();
                }
            }
            PatternKind::Struct { fields, .. } => {
                for (name, field) in fields {
                    path.push(Step::Field(name.clone()));
                    self.collect(&field.value, path);
                    path.pop();
                }
            }
            PatternKind::Natural(value) => self.collect_literal(path, Literal::Natural(*value)),
            PatternKind::Integer(value) => self.collect_literal(path, Literal::Integer(*value)),
            PatternKind::Real(value) => self.collect_literal(path, Literal::Real(*value)),
            PatternKind::String(value) => {
                self.collect_literal(path, Literal::String(value.clone()))
            }
            PatternKind::Boolean(value) => self.collect_literal(path, Literal::Boolean(*value)),
            // Every element, from either end, is the one element position;
            // see [`Step::Element`]. The rest binds the array between the
            // elements rather than any element, so it opens nothing here.
            PatternKind::Array { before, after, .. } => {
                path.push(Step::Element);
                for element in before.iter().chain(after) {
                    self.collect(element, path);
                }
                path.pop();
            }
        }
    }

    fn collect_literal(&mut self, path: &[Step], literal: Literal) {
        self.tests
            .entry(path.to_vec())
            .or_default()
            .literals
            .insert(literal);
    }

    /// Whether the earlier arms alone leave no unhandled value field_summaries this
    /// case at this position — what lets a later binder's view mark the case
    /// absent. The same usefulness question, asked of a synthetic row that is
    /// wildcards everywhere except the path down to the case.
    pub(crate) fn handled(&self, earlier: &[&Pattern], path: &[Step], case: &str) -> bool {
        let rows: Vec<Vec<Mat>> = earlier.iter().map(|pattern| vec![mat(pattern)]).collect();
        let mut forced = Mat::Tag {
            name: case.to_string(),
            payload: Box::new(Mat::Wild),
        };
        for step in path.iter().rev() {
            forced = match step {
                Step::Field(name) => Mat::Struct(vec![(name.clone(), forced)]),
                Step::Payload(name) => Mat::Tag {
                    name: name.clone(),
                    payload: Box::new(forced),
                },
                // The element position stands for every index at once, and
                // an arm testing the third element says nothing about the
                // fourth: no arm handles a case there, whatever it wrote.
                Step::Element => return false,
            };
        }
        !self.useful(&rows, &[Vec::new()], &[forced])
    }

    /// Maranget's usefulness: whether some value matches `q` and no row of
    /// `rows`. `cols` names the position each column stands for, which is
    /// where the universes come from; the three are always the same width.
    ///
    /// No witness comes back any more: the checks that show a reader an
    /// example are typed and live in [`patterns`](crate::patterns), and the
    /// refinement this serves asks only whether anything escapes.
    fn useful(&self, rows: &[Vec<Mat>], cols: &[Vec<Step>], q: &[Mat]) -> bool {
        // No columns left: every value matching q matches every row, so q is
        // useful exactly when no row is left to cover it.
        let Some((pos, later)) = cols.split_first() else {
            return rows.is_empty();
        };
        // A column with a struct pattern in it is widened first: one column
        // per field any of its structs names, plus the position itself for the
        // tags and literals, so the rest of the walk only ever sees flat cells.
        // A struct reaches into its fields and says nothing about the constructor, so
        // it widens to a wildcard beside its fields; everything else says
        // nothing about the fields and widens to wildcards beside itself.
        let named: IndexSet<&String> = std::iter::once(&q[0])
            .chain(rows.iter().map(|row| &row[0]))
            .filter_map(|cell| match cell {
                Mat::Struct(fields) => Some(fields),
                _ => None,
            })
            .flat_map(|fields| fields.iter().map(|(name, _)| name))
            .collect();
        if !named.is_empty() {
            let widen = |cell: &Mat, rest: &[Mat]| -> Vec<Mat> {
                let (core, fields) = match cell {
                    Mat::Struct(fields) => (
                        Mat::Wild,
                        named
                            .iter()
                            .map(|name| {
                                fields
                                    .iter()
                                    .find(|(field, _)| field == *name)
                                    .map(|(_, sub)| sub.clone())
                                    .unwrap_or(Mat::Wild)
                            })
                            .collect::<Vec<Mat>>(),
                    ),
                    cell => (cell.clone(), vec![Mat::Wild; named.len()]),
                };
                std::iter::once(core)
                    .chain(fields)
                    .chain(rest.iter().cloned())
                    .collect()
            };
            let wide_cols: Vec<Vec<Step>> = std::iter::once(pos.clone())
                .chain(named.iter().map(|name| {
                    let mut sub = pos.clone();
                    sub.push(Step::Field((*name).clone()));
                    sub
                }))
                .chain(later.iter().cloned())
                .collect();
            let wide_rows: Vec<Vec<Mat>> =
                rows.iter().map(|row| widen(&row[0], &row[1..])).collect();
            let wide_q = widen(&q[0], &q[1..]);
            return self.useful(&wide_rows, &wide_cols, &wide_q);
        }
        match &q[0] {
            // q tests a tag: only the rows that could take the same values
            // matter, and the question moves into the payload.
            Mat::Tag { name, payload } => {
                let rows = specialize_tag(rows, name);
                let cols = payload_cols(pos, name, later);
                let q: Vec<Mat> = std::iter::once((**payload).clone())
                    .chain(q[1..].iter().cloned())
                    .collect();
                self.useful(&rows, &cols, &q)
            }
            // q accepts everything here — [`handled`](Self::handled)'s forced
            // rows are tags and structs down to a wildcard, so the only
            // non-tag query that reaches this point is a wildcard: widening
            // above has already consumed every struct. It is useful if any
            // value of the position's universe escapes the rows. Tags draw
            // from their listed cases; scalar literals draw from their listed
            // values and, except for booleans, an unlisted value; and an open
            // tag position also has an "anything else" value.
            _ => {
                let empty = Tests::default();
                let tests = self.tests.get(pos).unwrap_or(&empty);
                if !tests.tags.is_empty() {
                    tests.tags.iter().any(|name| {
                        let rows = specialize_tag(rows, name);
                        let cols = payload_cols(pos, name, later);
                        let q: Vec<Mat> = std::iter::once(Mat::Wild)
                            .chain(q[1..].iter().cloned())
                            .collect();
                        self.useful(&rows, &cols, &q)
                    }) || (self.open(pos) && self.useful(&defaults(rows), later, &q[1..]))
                } else if !tests.literals.is_empty() {
                    if tests
                        .literals
                        .iter()
                        .all(|literal| matches!(literal, Literal::Boolean(_)))
                    {
                        // Boolean is the one finite scalar universe. Asking
                        // both values — not merely the values written in an
                        // arm — is what makes `false | true` total and a
                        // single boolean literal partial.
                        [false, true].into_iter().any(|value| {
                            self.useful(
                                &specialize_literal(rows, &Literal::Boolean(value)),
                                later,
                                &q[1..],
                            )
                        })
                    } else {
                        // Every other primitive has values beyond a finite
                        // list of literal patterns.
                        tests.literals.iter().any(|value| {
                            self.useful(&specialize_literal(rows, value), later, &q[1..])
                        }) || self.useful(&defaults(rows), later, &q[1..])
                    }
                } else {
                    // Nothing tests the position at all, so every cell in the
                    // column is a wildcard and any value serves.
                    self.useful(&defaults(rows), later, &q[1..])
                }
            }
        }
    }

    /// Whether a position's universe holds values beyond the listed cases:
    /// whether any arm is irrefutable at it, which is a binder at or before
    /// it — or a catch-all arm, irrefutable everywhere. What decides, in
    /// inference, whether the position's row closes over its listed cases;
    /// the matrix checks here read the same answer, which is what keeps them
    /// mirrors of one rule.
    pub(crate) fn open(&self, pos: &[Step]) -> bool {
        self.binds.iter().any(|bind| pos.starts_with(bind))
    }
}

/// The rows still in the running once the value is known to be this case,
/// with the payload column opened up: a row testing the same tag matches on
/// through its payload, a wildcard matches whatever the payload is, and a row
/// testing anything else can never take the value.
fn specialize_tag(rows: &[Vec<Mat>], name: &str) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| {
            let payload = match &row[0] {
                Mat::Tag { name: tag, payload } if tag == name => (**payload).clone(),
                Mat::Wild => Mat::Wild,
                _ => return None,
            };
            Some(
                std::iter::once(payload)
                    .chain(row[1..].iter().cloned())
                    .collect(),
            )
        })
        .collect()
}

/// [`specialize_tag`] about a primitive literal, which carries nothing: the
/// column is consumed rather than replaced.
fn specialize_literal(rows: &[Vec<Mat>], value: &Literal) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| match &row[0] {
            Mat::Literal(literal) if literal == value => Some(row[1..].to_vec()),
            Mat::Wild => Some(row[1..].to_vec()),
            _ => None,
        })
        .collect()
}

/// The rows still in the running once the value is known to escape every
/// listed test: exactly the ones that accept everything at the column.
fn defaults(rows: &[Vec<Mat>]) -> Vec<Vec<Mat>> {
    rows.iter()
        .filter_map(|row| match &row[0] {
            Mat::Wild => Some(row[1..].to_vec()),
            _ => None,
        })
        .collect()
}

/// The columns after a tag specialization: the payload's position, then
/// whatever columns were already waiting.
fn payload_cols(pos: &[Step], name: &str, later: &[Vec<Step>]) -> Vec<Vec<Step>> {
    let mut payload = pos.to_vec();
    payload.push(Step::Payload(name.to_string()));
    std::iter::once(payload)
        .chain(later.iter().cloned())
        .collect()
}

/// Every name a surface pattern binds, in the order the lowering walk meets
/// them — repeats included, so the declare pass and [`Builder::pattern`] agree
/// position for position. See [`Binders::Declared`].
fn pattern_names(pattern: &parse::Pattern, out: &mut Vec<TrackedString>) {
    match &pattern.tracked {
        parse::PatternKind::Ident { name } => out.push(name.clone()),
        // A wildcard binds nothing, so there is nothing here to declare — and
        // nothing for a duplicate check, anywhere, to ever meet.
        parse::PatternKind::Wildcard
        | parse::PatternKind::Natural(_)
        | parse::PatternKind::Integer(_)
        | parse::PatternKind::Real(_)
        | parse::PatternKind::String(_)
        | parse::PatternKind::Boolean(_)
        | parse::PatternKind::Unit => {}
        parse::PatternKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                pattern_names(payload, out);
            }
        }
        parse::PatternKind::Struct { fields, .. } => {
            for (name, sub) in fields {
                match sub {
                    Some(sub) => pattern_names(sub, out),
                    None => out.push(name.clone()),
                }
            }
        }
        parse::PatternKind::Tuple(elements) => {
            for element in elements {
                pattern_names(element, out);
            }
        }
        // The rest's name binds between the elements, in the order it was
        // written among them, which is the order the lowering walk keeps.
        parse::PatternKind::Array {
            before,
            rest,
            after,
        } => {
            for element in before {
                pattern_names(element, out);
            }
            if let Some(name) = rest.as_ref().and_then(|rest| rest.name.as_ref()) {
                out.push(name.clone());
            }
            for element in after {
                pattern_names(element, out);
            }
        }
    }
}

/// Which definitions have to be typed together, and in which order.
///
/// A group is a set of definitions each of which is reachable from every other
/// through the names their values mention — the strongly connected components
/// of the reference graph — and the groups come back in dependency order, so
/// that a definition is always solved after everything it names outside its own
/// group. That is what lets an earlier group's definition be instantiated at a
/// use site rather than shared with it. See [`Group`].
///
/// Only the definitions in the table are nodes. A lambda's argument is not one,
/// being no definition at all, and a name that failed to resolve became
/// [`TermKind::Error`] and mentions nobody.
///
/// Deterministic throughout, and deliberately so: the groups decide the order
/// inference runs in, so a hash anywhere in here would be a program that
/// type-checks on one run and not the next.
fn grouping(terms: &IndexMap<Symbol, Decl<Term>>) -> Vec<Group> {
    // Who each definition names, and then everything each one leads to. The
    // same closure the type half takes for the same question; a pair being
    // mutually reachable is the whole of what a group is.
    let mentions: IndexMap<Symbol, Vec<Symbol>> = terms
        .iter()
        .map(|(symbol, decl)| {
            let mut out = Vec::new();
            references(&decl.value, &mut out);
            out.retain(|named| terms.contains_key(named));
            (*symbol, out)
        })
        .collect();
    let reachable = closure(&mentions);

    // The groups, and which one each definition landed in. Walked in source
    // order, so the first definition of a group reached is its earliest and the
    // members it collects are in source order too. A definition leading back to
    // itself is what makes its group recursive — including a group of one,
    // which is the case the flag exists for, and including every group of two
    // or more, where being a group at all makes it so.
    let mut of: HashMap<Symbol, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for symbol in terms.keys() {
        if of.contains_key(symbol) {
            continue;
        }
        let reaches = &reachable[symbol];
        let members: Vec<Symbol> = terms
            .keys()
            .copied()
            .filter(|other| {
                other == symbol || (reaches.contains(other) && reachable[other].contains(symbol))
            })
            .collect();
        for member in &members {
            of.insert(*member, groups.len());
        }
        groups.push(Group {
            members,
            recursive: reaches.contains(symbol),
        });
    }

    // Which groups each group has to wait for: the ones its members name and
    // are not in. A group never waits for itself, which is what makes this a
    // graph with no loops in it — everything mutually reachable is already one
    // node here.
    let mut needs: Vec<IndexSet<usize>> = vec![IndexSet::new(); groups.len()];
    for (at, group) in groups.iter().enumerate() {
        for named in group.members.iter().flat_map(|member| &mentions[member]) {
            let other = of[named];
            if other != at {
                needs[at].insert(other);
            }
        }
    }

    // Dependency order, taking the earliest group that is ready at every step.
    // `groups` is already in order of earliest member, so the lowest index that
    // is ready is the earliest one, and two groups with nothing between them
    // come out in the order they were written.
    //
    // Every group is placed. Each round places one unless nothing is ready, and
    // nothing being ready in a graph with no loops means nothing is left.
    let mut placed = vec![false; groups.len()];
    let mut order = Vec::with_capacity(groups.len());
    while let Some(next) =
        (0..groups.len()).find(|at| !placed[*at] && needs[*at].iter().all(|need| placed[*need]))
    {
        placed[next] = true;
        order.push(next);
    }
    order.into_iter().map(|at| groups[at].clone()).collect()
}

/// Every definition a value names, at any depth, in the order it names them. A
/// lambda's argument is not one: it is a local, and no definition answers to it.
///
/// [`mentioned`] about terms, down to what it is for — the edges of the graph
/// [`grouping`] closes.
fn references(term: &Term, out: &mut Vec<Symbol>) {
    match &term.kind {
        TermKind::Ident(symbol) => out.push(*symbol),
        TermKind::Unary { value, .. } => references(value, out),
        TermKind::Binary { left, right, .. } => {
            references(left, out);
            references(right, out);
        }
        TermKind::Apply { func, arg } => {
            references(func, out);
            references(arg, out);
        }
        // The binder names nothing here; only the body can name anything, and
        // a use of the binder inside it is a symbol this walk pushes and
        // [`grouping`] then drops, since it is in no definition table.
        TermKind::Fn { body, .. } => references(body, out),
        // Both halves, and no filtering of its own: the name a nested `let`
        // binds is a local, so a use of it is pushed here and dropped by
        // [`grouping`] along with a lambda argument's, and a top-level name
        // mentioned from inside one still lands in the right group.
        TermKind::Let { value, body, .. } => {
            references(value, out);
            references(body, out);
        }
        TermKind::Struct { fields, spread } => {
            for field in fields.values() {
                references(&field.value, out);
            }
            if let Some(spread) = spread {
                references(&spread.value, out);
            }
        }
        TermKind::Array(items) => {
            for item in items {
                references(&item.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                references(payload, out);
            }
        }
        TermKind::Project { base, .. } => references(base, out),
        TermKind::Match { scrutinee, arms } => {
            references(scrutinee, out);
            for (_, body) in arms {
                references(body, out);
            }
        }
        TermKind::Handle { body, handler } => {
            references(body, out);
            for arm in &handler.arms {
                references(&arm.body, out);
            }
            if let Some(ret) = &handler.ret {
                references(&ret.body, out);
            }
        }
        TermKind::Raise(value) => references(value, out),
        // An operation names an effect, which is no definition and so no node
        // of the graph a group is read off.
        TermKind::Operation { .. }
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// What each parameter of each declaration stands for, worked out from how the
/// bodies use them, and every parameter used both ways.
///
/// A parameter is written as a name and nothing more, so its kind is read off
/// its uses: a name in a `..` tail stands for a row, a name anywhere else stands
/// for a type, and a name handed straight on to another declaration stands for
/// whatever that declaration's parameter in that position stands for. The third
/// is what makes this an inference rather than a scan — declarations are hoisted
/// and may name each other, so `type A 'x = B 'x` and `type B 'y = A 'y`
/// constrain each other in
/// a circle.
///
/// Which is why the answer is a reachability question and not an assignment: a
/// slot stands for whatever it says of itself together with whatever every slot
/// it hands itself on to says. Reachability needs no order, so no declaration
/// can win a race by being written first, and it has a direction, so a
/// disagreement is reported against the parameter that reached both readings
/// rather than against whichever of the two a symmetric answer would have had
/// to guess between.
///
/// Handing a parameter on is a demand on the *argument*, never on the callee. A
/// use site writing `WithX Nat` says nothing about what `WithX` takes; it is
/// checked against what `WithX` takes, by [`row_arguments`]. A declaration says
/// what it takes, and a use site is not the declaration.
///
/// A slot nothing said anything about stands for a type: `type Ghost 'a = Nat`
/// takes a type, because that is what a reader writing `Ghost Nat` will expect
/// and there is nothing to contradict it.
///
/// A row parameter carries one thing more than the reading: the labels an
/// argument written at it may not name. Those join the same way — a slot may
/// not name what it writes out beside itself, nor what any slot it hands itself
/// on to may not name — so the set falls out of the same closure with union in
/// place of a boolean or. It is finite because the labels in a program are, so
/// no amount of handing on can make it grow forever.
fn kinds(
    types: &IndexMap<Symbol, Decl<Type>>,
    effects: &IndexMap<Symbol, Decl<Effect>>,
    external: &IndexMap<Symbol, ExternalType>,
) -> Kinds {
    // What each body says of its own parameters, which slots each one hands
    // itself on to, and which ones sit in the tail of a row handed on. All
    // gathered over the whole table before anything is resolved, so the walk
    // needs nothing from the answer and the answer needs nothing from the
    // order. Imported parameter readings are already normalized in their
    // interfaces, but are nodes in the same graph: a local parameter handed to
    // one must inherit its reading and lacks just as it does from a local slot.
    let mut said: HashMap<Slot, Reading> = external
        .iter()
        .flat_map(|(symbol, declaration)| {
            declaration.params.iter().enumerate().map(|(index, kind)| {
                (
                    (*symbol, index as u32),
                    Reading {
                        senses: std::iter::once(kind.sense()).collect(),
                        lacks: kind.lacks().clone(),
                    },
                )
            })
        })
        .collect();
    let summaries: HashMap<Shape, HashMap<Symbol, RowSummary>> =
        [Shape::Struct, Shape::Sum, Shape::Effect]
            .into_iter()
            .map(|shape| (shape, row_summaries(types, external, shape)))
            .collect();
    // Every body that binds parameters, with the types it reads them in: a
    // type declaration's one body, and an effect's operation signatures. An
    // effect is a declaration with parameters like any other, and its
    // operations are where the parameters are used, so both feed one graph.
    let declarations: Vec<(Symbol, &[Param], Vec<&Type>)> = types
        .iter()
        .map(|(symbol, decl)| (*symbol, decl.params.as_slice(), vec![&decl.value]))
        .chain(effects.iter().map(|(symbol, decl)| {
            let bodies = match &decl.value {
                Effect::Operations(operations) => operations
                    .values()
                    .flat_map(|operation| [&operation.from, &operation.to])
                    .collect(),
                Effect::Alias(alias) => vec![&alias.expanded],
            };
            (*symbol, decl.params.as_slice(), bodies)
        }))
        .collect();
    let mut handed: IndexMap<Slot, Vec<Slot>> = IndexMap::new();
    let mut tails: IndexMap<Slot, Vec<Slot>> = IndexMap::new();
    for (symbol, _, bodies) in &declarations {
        for body in bodies {
            constrain(body, &summaries, &mut |fact| match fact {
                Fact::Says(index, kind) => {
                    let entry = said.entry((*symbol, index)).or_default();
                    entry.senses.insert(kind.sense());
                    entry.lacks.extend(kind.lacks().iter().cloned());
                }
                Fact::Hands(index, to) => handed.entry((*symbol, index)).or_default().push(to),
                Fact::Tails(index, to) => tails.entry((*symbol, index)).or_default().push(to),
            });
        }
    }
    // Two closures over two edge sets, because the two questions travel
    // differently. Which reading a slot has travels only along a parameter
    // handed straight on: a row written out as an argument is a row whatever
    // the callee is, so letting that edge carry the reading would call every
    // tail inside an ordinary argument a mixed parameter. What a slot may not
    // name travels along both, since either way the argument lands where the
    // callee's tail sat.
    let reads = closure(&handed);
    let mut edges = handed.clone();
    for (slot, to) in tails {
        edges.entry(slot).or_default().extend(to);
    }
    let carries = closure(&edges);

    let mut out = HashMap::new();
    let mut mixed = IndexSet::new();
    let mut told = Vec::new();
    for (symbol, params, _) in &declarations {
        let symbol = *symbol;
        let mut kinds = Vec::with_capacity(params.len());
        for (index, param) in params.iter().enumerate() {
            let slot = (symbol, index as u32);
            let read_as = senses(&reads, &said, slot);
            let mut lacks = IndexSet::new();
            for demand in reached(&carries, slot) {
                if let Some(reading) = said.get(&demand) {
                    lacks.extend(reading.lacks.iter().cloned());
                }
            }
            // A parameter used more than one way is reported against the
            // parameter rather than against any one use: no use is wrong on its
            // own, and it is the declaration that has to say which it meant. The
            // declaration told is the one that brought the readings
            // together — a declaration that merely hands the parameter on to
            // one already broken is right about its own and has nothing to
            // change, which is what the second condition says. A slot leading
            // back to itself is not below itself, so a pair that disagree
            // around a circle are each told rather than neither.
            //
            // The first two readings are the ones named. A parameter read all
            // three ways has one mistake to point at like any other, and the
            // sentence that names two of them says enough to find it.
            if read_as.len() > 1 {
                mixed.insert(symbol);
                let below = handed.get(&slot).into_iter().flatten().any(|to| {
                    senses(&reads, &said, *to).len() > 1
                        && !reached(&carries, *to).any(|at| at == slot)
                });
                if !below {
                    told.push(Error {
                        span: param.span,
                        kind: ErrorKind::MixedParameter {
                            first: read_as[0],
                            second: read_as[1],
                        },
                    });
                }
            }
            // A parameter read more than one way is still taken as the sum's
            // rest among them, so that the debugger and the Types tab show what
            // the body actually said of it. Nothing is enforced against it —
            // the declaration is a write-off and `mixed` says so — but calling
            // it a type would be this pass reporting one thing and displaying
            // another.
            // A parameter read more than one way is shown as the row reading
            // among them, for the reason below — and a sum's rest wins over an
            // arrow's effects only because one of the two has to, the
            // declaration being a write-off either way.
            kinds.push(if read_as.contains(&Sense::Fields) {
                ParamKind::Fields { lacks }
            } else if read_as.contains(&Sense::Cases) {
                ParamKind::Cases { lacks }
            } else if read_as.contains(&Sense::Effects) {
                ParamKind::Effects { lacks }
            } else {
                ParamKind::Type { lacks }
            });
        }
        out.insert(symbol, kinds);
    }
    Kinds {
        kinds: out,
        mixed,
        errors: told,
    }
}

/// What a row of effects says about the parameter its `..` names: that the
/// parameter is used as effects, and may not name what the row already does.
///
/// One function because there are two rows now — an arrow's own, and the one an
/// argument may be — and what each says of its tail is the same sentence.
fn says_effects(
    effects: &EffectRow,
    summaries: &HashMap<Shape, HashMap<Symbol, RowSummary>>,
    out: &mut impl FnMut(Fact),
) {
    if let Some(Tail {
        of: Row::Param { index, .. },
        ..
    }) = effects.tail
    {
        let lacks = effects.effects.keys().map(EffectId::label_key).collect();
        out(Fact::Says(index, ParamKind::Effects { lacks }));
    }
    // An applied effect is an application: what each argument says is what an
    // argument of a type application says, about the effect's slot instead.
    for label in effects.effects.values() {
        arguments(label.symbol(), label.args(), summaries, out);
    }
}

/// What the arguments of one application say, whichever declaration is
/// applied: a parameter handed straight on stands for whatever the slot does,
/// and any other argument's own tails land where the slot's tail sat.
fn arguments(
    head: Symbol,
    args: &[Type],
    summaries: &HashMap<Shape, HashMap<Symbol, RowSummary>>,
    out: &mut impl FnMut(Fact),
) {
    for (at, arg) in args.iter().enumerate() {
        let slot = (head, at as u32);
        match &arg.tracked {
            // A parameter handed straight on stands for whatever it is
            // handed to. This is the statement that crosses
            // declarations, and the only one that needs resolving
            // rather than reading.
            //
            // Recorded here and *not* descended into: argument position
            // is not type position, and walking in would say the
            // parameter stands for a type — which is how a row handed
            // straight on came to look like a parameter used both ways.
            TypeKind::Param { index, .. } => out(Fact::Hands(*index, slot)),
            // Anything else says nothing about what the head takes.
            // Writing `WithX Nat` is a claim about `Nat`, not about
            // `WithX` — the argument is checked against the kind the
            // declaration was read to have, by [`row_arguments`], and
            // letting a use site vote here is what made that kind
            // depend on which declaration was written first. What is
            // inside the argument still speaks for itself, so this
            // descends.
            //
            // A row written out here is the one thing that carries
            // something back across the same edge: its own tail ends up
            // where the callee's tail sat, so it inherits what the
            // callee may not name. That is an obligation and not a
            // reading, which is why it is a [`Fact::Tails`] rather than
            // a second [`Fact::Hands`].
            _ => {
                // Every parameter whose row reaches this argument's
                // outer row lands where the callee's tail sat. Keep the
                // three senses separate: a struct alias carries only
                // field obligations, a sum alias only case obligations,
                // and an effects row only effect obligations.
                for shape in [Shape::Struct, Shape::Sum, Shape::Effect] {
                    let declarations = summaries
                        .get(&shape)
                        .expect("every row sense has summaries");
                    for index in row_summary(arg, declarations, shape).slots {
                        out(Fact::Tails(index, slot));
                    }
                }
                constrain(arg, summaries, out);
            }
        }
    }
}

/// A slot together with everything it leads to, which is the set every question
/// in [`kinds`] is answered over: a slot stands for whatever it says of itself
/// joined with whatever every slot it hands itself on to says.
fn reached(closed: &IndexMap<Slot, IndexSet<Slot>>, slot: Slot) -> impl Iterator<Item = Slot> + '_ {
    std::iter::once(slot).chain(closed.get(&slot).into_iter().flatten().copied())
}

/// Every way one slot is read: what it says of itself joined with what every
/// slot it hands itself on to says. More than one is the clash, so this is both
/// what [`kinds`] decides a parameter's kind from and what it asks of a slot's
/// successors to decide whether the clash it found is already somebody else's
/// to fix.
///
/// Insertion-ordered, and the order is the order the bodies were read in, so a
/// complaint naming two readings names the same two every run.
fn senses(
    reads: &IndexMap<Slot, IndexSet<Slot>>,
    said: &HashMap<Slot, Reading>,
    slot: Slot,
) -> IndexSet<Sense> {
    let mut senses = IndexSet::new();
    for demand in reached(reads, slot) {
        if let Some(reading) = said.get(&demand) {
            senses.extend(reading.senses.iter().copied());
        }
    }
    senses
}

/// Everything one declaration's body says about its own parameters. See
/// [`Fact`] for the three, and [`kinds`], which resolves them into a kind
/// apiece.
fn constrain(
    ty: &Type,
    summaries: &HashMap<Shape, HashMap<Symbol, RowSummary>>,
    out: &mut impl FnMut(Fact),
) {
    match &ty.tracked {
        // A name reached as a type is one: this walk only descends through
        // positions a type goes in, so arriving here at all is the statement.
        TypeKind::Param { index, .. } => out(Fact::Says(
            *index,
            ParamKind::Type {
                lacks: IndexSet::new(),
            },
        )),
        TypeKind::Array(element) => constrain(element, summaries, out),
        TypeKind::Struct { fields, tail } => {
            for field in fields.values() {
                if let Some(value) = field.value() {
                    constrain(value, summaries, out);
                }
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                // A struct's `..` is the struct-row tail, so the parameter stands
                // for a whole type — the same reading a parameter written
                // anywhere else has, which is why `type W 'r = { f: 'r, ..'r }` is
                // well-formed.
                //
                // The fields written in it are exactly what it may not
                // name: they are already named here, and a `..` covers what is
                // not. An absent label is named as surely as a written one —
                // `\y` says the tail has no `y`, which is the same sentence a
                // field named `y` makes it say — and sits in the same map, so
                // the keys are the whole set.
                let lacks = fields.keys().cloned().collect();
                out(Fact::Says(*index, ParamKind::Fields { lacks }));
            }
        }
        // The struct arm again, about cases: a payload is a type position, and
        // the cases written beside a tail — absent ones included — are what it
        // may not name.
        TypeKind::Sum { cases, tail } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
                    constrain(payload, summaries, out);
                }
            }
            if let Some(Tail {
                of: Row::Param { index, .. },
                ..
            }) = tail
            {
                let lacks = cases.keys().cloned().collect();
                out(Fact::Says(*index, ParamKind::Cases { lacks }));
            }
        }
        // The sum's arm a third time, about effects: an arrow's two sides are
        // type positions, and the effects written beside its tail — absent ones
        // included — are what that tail may not name.
        TypeKind::Arrow { from, to, effects } => {
            constrain(from, summaries, out);
            constrain(to, summaries, out);
            says_effects(effects, summaries, out);
        }
        // A row written as an argument says of its own tail exactly what an
        // arrow's row says: whatever the tail names is used as effects, and may
        // not name what the row already does.
        TypeKind::Effects(effects) => says_effects(effects, summaries, out),
        TypeKind::Apply { head, args, .. } => arguments(*head, args, summaries, out),
        TypeKind::Ident(_)
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => {}
    }
}

/// Every argument written where a parameter's conditions refuse it, erased where
/// it stood.
///
/// Two ways to fail, and which of them a parameter can be failed by is what
/// [`ParamKind`] says. A [`ParamKind::Cases`] parameter has both: a sum's rest is
/// spliced into a row, so the argument has to be something a row can hold —
/// [`row_shaped`] — and it has to name none of the cases the declaration already
/// names, since a `..` covers only what its row leaves out. A
/// [`ParamKind::Type`] parameter has only the second. A struct's `..` requires
/// a struct row. [`row_summary`] reads the labels an argument brings through
/// local and imported aliases for each row sense independently.
///
/// The kinds themselves are a well-formedness check and nothing more — an
/// argument lowers to exactly the type it would anywhere else, and substitution
/// puts it wherever the parameter sat. What this refuses is the argument that
/// would leave a row holding something no row can hold, which is the invariant
/// [`Rest`](crate::types::Rest) documents and nothing else enforces, and the
/// argument that would leave a row or a type naming a label twice, which nothing
/// downstream can recover from either.
///
/// Refusing it is not enough on its own: an argument left standing is
/// substituted where the parameter sat anyway, and the reader is told a second
/// time in words about a type nobody wrote. So the argument absorbs, the way
/// [`ErrorKind::Circular`] and [`ErrorKind::OpenDeclaredType`] do — the
/// argument and not the whole application, because the mistake is the argument
/// and `WithX { x: Nat } -> Nat` is half correct. [`row_shaped`] already reads
/// [`TypeKind::Error`] as row-shaped and [`row_summary`] reads it as naming
/// nothing, so nothing complains about the erasure, and it lowers to the
/// undecided type, which row-tail checks already allow.
fn row_arguments(program: &mut Program, kinds: &HashMap<Symbol, Vec<ParamKind>>) -> Vec<Error> {
    fn walk(
        ty: &mut Type,
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        carries: &HashMap<Shape, HashMap<Symbol, RowSummary>>,
        rows: &HashMap<Symbol, Sense>,
        out: &mut Vec<Error>,
    ) {
        match &mut ty.tracked {
            TypeKind::Apply { head, args, .. } => {
                applied(*head, args, kinds, carries, rows, out);
            }
            // An effect row is a row of applications: each label's arguments
            // are held to the effect's parameters exactly as a type
            // application's are to the type's.
            TypeKind::Arrow { from, to, effects } => {
                walk(from, kinds, carries, rows, out);
                walk(to, kinds, carries, rows, out);
                for label in effects.effects.values_mut() {
                    applied(label.symbol(), label.args_mut(), kinds, carries, rows, out);
                }
            }
            TypeKind::Effects(effects) => {
                for label in effects.effects.values_mut() {
                    applied(label.symbol(), label.args_mut(), kinds, carries, rows, out);
                }
            }
            TypeKind::Array(element) => walk(element, kinds, carries, rows, out),
            TypeKind::Struct { fields, .. } => {
                for field in fields.values_mut() {
                    if let TypeField::Written { value, .. } = field {
                        walk(value, kinds, carries, rows, out);
                    }
                }
            }
            TypeKind::Sum { cases, .. } => {
                for case in cases.values_mut() {
                    if let SumCase::Written {
                        payload: Some(payload),
                        ..
                    } = case
                    {
                        walk(payload, kinds, carries, rows, out);
                    }
                }
            }
            TypeKind::Ident(_)
            | TypeKind::Param { .. }
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole
            | TypeKind::Error => {}
        }
    }

    /// The arguments of one application, whichever declaration is applied,
    /// each checked against the parameter it lands at and erased where it
    /// breaks the parameter's conditions.
    fn applied(
        head: Symbol,
        args: &mut [Type],
        kinds: &HashMap<Symbol, Vec<ParamKind>>,
        carries: &HashMap<Shape, HashMap<Symbol, RowSummary>>,
        rows: &HashMap<Symbol, Sense>,
        out: &mut Vec<Error>,
    ) {
        for (at, arg) in args.iter_mut().enumerate() {
            let kind = kinds.get(&head).and_then(|kinds| kinds.get(at));
            // The empty row is written `|` whichever row it is, and
            // the parser reads a bare `|` as the sum with no cases.
            // Where a row of effects is asked for, that is the row of
            // effects with none — which is what a printed pure
            // effects argument reads back as.
            if matches!(kind, Some(kind) if kind.row().is_some_and(|(shape, _)| shape == Shape::Effect))
                && matches!(&arg.tracked, TypeKind::Sum { cases, tail: None } if cases.is_empty())
            {
                let span = arg.span;
                *arg = span.track(TypeKind::Effects(Box::new(EffectRow {
                    span,
                    written: true,
                    effects: IndexMap::new(),
                    tail: None,
                })));
            }
            let refused = match kind {
                // A sum's rest and an arrow's effects are both
                // spliced into a row, so only a row can go there —
                // and only one naming none of the labels the
                // declaration already names. The two conditions are
                // one condition; only the noun a complaint is
                // worded in differs.
                Some(kind) if kind.row().is_some() => {
                    let (shape, lacks) = kind.row().expect("the arm just asked for one");
                    let summaries = carries.get(&shape).expect("every row sense has summaries");
                    match row_shaped(arg, rows, shape, summaries) {
                        false => Some(ErrorKind::NotARow {
                            sense: kind.sense(),
                        }),
                        true => row_summary(arg, summaries, shape)
                            .labels
                            .into_iter()
                            .find(|name| lacks.contains(name))
                            .map(|field| ErrorKind::RepeatedRowField { shape, field }),
                    }
                }
                _ => None,
            };
            if let Some(kind) = refused {
                let span = arg.span;
                out.push(Error { span, kind });
                // Nothing left inside to walk: what it was made of is no
                // longer part of the program.
                *arg = span.track(TypeKind::Error);
                continue;
            }
            walk(arg, kinds, carries, rows, out);
        }
    }

    // What each declaration's fields come to, read once over the whole table
    // before anything is walked: an argument written at a struct's `..` carries
    // whatever the declaration it names carries, which is what the repeated
    // field check is asked against. See [`row_summaries`].
    let carries: HashMap<Shape, HashMap<Symbol, RowSummary>> =
        [Shape::Struct, Shape::Sum, Shape::Effect]
            .into_iter()
            .map(|shape| {
                (
                    shape,
                    row_summaries(&program.types, &program.external_types, shape),
                )
            })
            .collect();
    let mut out = Vec::new();
    for decl in program.types.values_mut() {
        // Which of this declaration's own parameters are a sum's rest, so one
        // handed straight on is recognised as one. Read out first, so that
        // walking the body borrows nothing the kinds are still held in.
        //
        // Nothing more than the set is wanted: a parameter handed straight on
        // has already collected everything the slot it goes to may not name —
        // that is what [`kinds`] closed the sets over — so there is no second
        // condition here for it to fail.
        let rows: HashMap<Symbol, Sense> = decl
            .params
            .iter()
            .filter(|param| param.kind.row().is_some())
            .map(|param| (param.symbol, param.kind.sense()))
            .collect();
        walk(&mut decl.value, kinds, &carries, &rows, &mut out);
    }
    // An annotation binds no parameters, so nothing in one can be a sum's rest
    // by being a parameter — but it is every bit as much a place to apply a
    // declaration, and was the way this check was first written round.
    for decl in program.terms.values_mut() {
        if let Some(annotation) = decl.annotation.as_mut().map(|it| &mut it.ty) {
            walk(annotation, kinds, &carries, &HashMap::new(), &mut out);
        }
        // And so is a nested binding's, which is a place to write one as much
        // as a definition's is. An annotation this walk never reaches is a
        // [`ErrorKind::RepeatedRowField`] never reported.
        annotations(&mut decl.value, &mut |annotation| {
            walk(annotation, kinds, &carries, &HashMap::new(), &mut out);
        });
    }
    for decl in program.externs.values_mut() {
        let annotation = &mut decl
            .annotation
            .as_mut()
            .expect("an extern always has a written annotation")
            .ty;
        walk(annotation, kinds, &carries, &HashMap::new(), &mut out);
    }
    // Operation inputs and outputs are source-written types too. They are not
    // annotations on terms, so reach them explicitly rather than letting this
    // well-formedness check silently omit one of the language's type-bearing
    // positions.
    for decl in program.effects.values_mut() {
        let rows: HashMap<Symbol, Sense> = decl
            .params
            .iter()
            .filter(|param| param.kind.row().is_some())
            .map(|param| (param.symbol, param.kind.sense()))
            .collect();
        match &mut decl.value {
            Effect::Operations(operations) => {
                for operation in operations.values_mut() {
                    walk(&mut operation.from, kinds, &carries, &rows, &mut out);
                    walk(&mut operation.to, kinds, &carries, &rows, &mut out);
                }
            }
            Effect::Alias(alias) => walk(&mut alias.expanded, kinds, &carries, &rows, &mut out),
        }
    }
    out
}

/// Every annotation written inside a term, in the order they were written.
///
/// A nested binding is the one place a written type appears anywhere but at the
/// top level of a definition, so this is where the passes that ask something of
/// every written type reach the ones inside a body.
fn annotations(term: &mut Term, out: &mut impl FnMut(&mut Type)) {
    match &mut term.kind {
        TermKind::Let {
            annotation,
            value,
            body,
            ..
        } => {
            if let Some(annotation) = annotation {
                out(&mut annotation.ty);
            }
            annotations(value, out);
            annotations(body, out);
        }
        TermKind::Unary { value, .. } => annotations(value, out),
        TermKind::Binary { left, right, .. } => {
            annotations(left, out);
            annotations(right, out);
        }
        TermKind::Apply { func, arg } => {
            annotations(func, out);
            annotations(arg, out);
        }
        TermKind::Fn { body, .. } => annotations(body, out),
        TermKind::Struct { fields, spread } => {
            for field in fields.values_mut() {
                annotations(&mut field.value, out);
            }
            if let Some(spread) = spread {
                annotations(&mut spread.value, out);
            }
        }
        TermKind::Array(items) => {
            for item in items {
                annotations(&mut item.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                annotations(payload, out);
            }
        }
        TermKind::Project { base, .. } => annotations(base, out),
        TermKind::Match { scrutinee, arms } => {
            annotations(scrutinee, out);
            for (_, body) in arms {
                annotations(body, out);
            }
        }
        TermKind::Handle { body, handler } => {
            annotations(body, out);
            for arm in &mut handler.arms {
                annotations(&mut arm.body, out);
            }
            if let Some(ret) = &mut handler.ret {
                annotations(&mut ret.body, out);
            }
        }
        TermKind::Raise(value) => annotations(value, out),
        TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// Whether a written type can supply a row of the requested sense.
///
/// Concrete rows answer directly. Parameters answer from their inferred sense,
/// and declared names/applications answer from the normalized summary that
/// follows local and imported forwarding aliases without recursion.
fn row_shaped(
    ty: &Type,
    rows: &HashMap<Symbol, Sense>,
    shape: Shape,
    summaries: &HashMap<Symbol, RowSummary>,
) -> bool {
    fn outer(
        ty: &Type,
        rows: &HashMap<Symbol, Sense>,
        shape: Shape,
        summaries: &HashMap<Symbol, RowSummary>,
    ) -> bool {
        match &ty.tracked {
            TypeKind::Error => true,
            TypeKind::Struct { .. } => shape == Shape::Struct,
            TypeKind::Sum { .. } => shape == Shape::Sum,
            TypeKind::Effects(_) => shape == Shape::Effect,
            TypeKind::Param { symbol, .. } => rows.get(symbol).copied() == Some(sense(shape)),
            TypeKind::Ident(symbol) => {
                let summary = summaries.get(symbol).cloned().unwrap_or_default();
                summary.shaped
            }
            TypeKind::Apply {
                head,
                args: supplied,
                ..
            } => {
                let summary = summaries.get(head).cloned().unwrap_or_default();
                summary.shaped
                    || summary.slots.into_iter().any(|index| {
                        supplied
                            .get(index as usize)
                            .is_some_and(|argument| outer(argument, rows, shape, summaries))
                    })
            }
            TypeKind::Array(_)
            | TypeKind::Arrow { .. }
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole => false,
        }
    }

    outer(ty, rows, shape, summaries)
}

/// A written type's normalized outer row for one sense: whether it reaches a
/// concrete row, the labels it names, and the parameters it forwards there.
/// A separate summary is built for fields, cases, and effects.
#[derive(Debug, Clone, Default)]
struct RowSummary {
    /// Whether normalization reaches a concrete row of this sense. A pure
    /// forwarding parameter has slots but no shape until its argument is read.
    shaped: bool,
    /// Insertion-ordered, so a complaint about an argument breaking the rule
    /// twice always names the same label first — the one a reader would reach
    /// first reading the argument left to right, which is the rule
    /// [`Table::repeated`](crate::inference) states for the same complaint
    /// reached through a variable.
    labels: IndexSet<String>,
    /// The parameters used as row tails, whose arguments therefore add to this
    /// summary.
    slots: IndexSet<u32>,
}

/// What each local and imported declaration carries at its outer row for one
/// requested sense.
///
/// Imported declarations are semantic [`Ty`] trees rather than written
/// [`Type`] trees, but forwarding means the same thing in both: the head's
/// labels are followed by whatever is supplied at each parameter used as the
/// outer row. Resolving both forms through one summary table makes kind/lacks
/// inference and argument checking agree across an import boundary.
///
/// Resolution is depth-first to retain that source order, but uses an explicit
/// work stack rather than the call stack. Completed declarations are memoized
/// by symbol. An active declaration contributes nothing when met again, making
/// malformed imported alias cycles and unresolved names absorbing rather than
/// recursive. Local unproductive cycles have already been diagnosed and
/// erased, but the guard keeps this normalization total independently of that
/// ordering.
fn row_summaries(
    types: &IndexMap<Symbol, Decl<Type>>,
    external: &IndexMap<Symbol, ExternalType>,
    shape: Shape,
) -> HashMap<Symbol, RowSummary> {
    enum Work<'a> {
        Named(Symbol),
        FinishNamed(Symbol),
        Semantic(&'a Ty),
        Artifact(&'a Type),
        SemanticApply(&'a [Rc<Ty>]),
        ArtifactApply(&'a [Type]),
        FinishApply {
            shaped: bool,
            labels: IndexSet<String>,
            arguments: usize,
            crossed_cycle: bool,
        },
        Value(RowSummary),
    }

    fn semantic_row(mut row: &crate::types::Row) -> RowSummary {
        let mut out = RowSummary {
            shaped: true,
            ..RowSummary::default()
        };
        loop {
            out.labels.extend(row.labels.keys().cloned());
            match &row.rest {
                Rest::Bound(index) => {
                    out.slots.insert(*index);
                    return out;
                }
                Rest::More(more) => row = more,
                Rest::Closed | Rest::Var(_) | Rest::Rigid { .. } | Rest::Undecided => {
                    return out;
                }
            }
        }
    }

    fn written_row_summary(
        labels: impl IntoIterator<Item = String>,
        tail: &Option<Tail>,
    ) -> RowSummary {
        let mut slots = IndexSet::new();
        if let Some(Tail {
            of: Row::Param { index, .. },
            ..
        }) = tail
        {
            slots.insert(*index);
        }
        RowSummary {
            shaped: true,
            labels: labels.into_iter().collect(),
            slots,
        }
    }

    fn resolve<'a>(
        root: Symbol,
        types: &'a IndexMap<Symbol, Decl<Type>>,
        external: &'a IndexMap<Symbol, ExternalType>,
        shape: Shape,
        done: &mut HashMap<Symbol, RowSummary>,
    ) {
        let mut active = HashSet::new();
        let mut work = vec![Work::Named(root)];
        let mut values: Vec<(RowSummary, bool)> = Vec::new();
        while let Some(next) = work.pop() {
            match next {
                Work::Named(symbol) => {
                    if let Some(summary) = done.get(&symbol) {
                        values.push((summary.clone(), false));
                    } else if !active.insert(symbol) {
                        values.push((RowSummary::default(), true));
                    } else {
                        work.push(Work::FinishNamed(symbol));
                        match types.get(&symbol) {
                            Some(declaration) => work.push(Work::Artifact(&declaration.value)),
                            None => {
                                let declaration = external.get(&symbol).expect(
                                    "every semantic name has an imported recovery interface",
                                );
                                work.push(Work::Semantic(declaration.scheme.body()));
                            }
                        }
                    }
                }
                Work::FinishNamed(symbol) => {
                    let (mut summary, crossed_cycle) =
                        values.pop().expect("a declaration leaves one summary");
                    active.remove(&symbol);
                    // Summary slots are relative to the declaration whose body
                    // produced them. Malformed semantic interfaces sometimes
                    // contain a bound row index beyond their published arity;
                    // it is not a slot in whichever local declaration happens
                    // to mention the bare alias.
                    let arity = types
                        .get(&symbol)
                        .map(|declaration| declaration.params.len())
                        .or_else(|| {
                            external
                                .get(&symbol)
                                .map(|declaration| declaration.params.len())
                        })
                        .unwrap_or(0);
                    summary.slots.retain(|index| (*index as usize) < arity);
                    // A nested result reached under an active-cycle assumption
                    // is valid only for that walk. The root is valid for its
                    // own symbol; caching an intermediate assumption would make
                    // a cycle's summary depend on declaration order.
                    if !crossed_cycle || symbol == root {
                        done.insert(symbol, summary.clone());
                    }
                    values.push((summary, crossed_cycle));
                }
                Work::Semantic(ty) => match ty {
                    Ty::Package(body) => work.push(Work::Semantic(body)),
                    Ty::Bound(index) => work.push(Work::Value(RowSummary {
                        shaped: false,
                        labels: IndexSet::new(),
                        slots: std::iter::once(*index).collect(),
                    })),
                    Ty::Struct(row) if shape == Shape::Struct => {
                        work.push(Work::Value(semantic_row(row)))
                    }
                    Ty::Sum(row) if shape == Shape::Sum => {
                        work.push(Work::Value(semantic_row(row)))
                    }
                    Ty::Named {
                        symbol,
                        args: supplied,
                        ..
                    } => {
                        work.push(Work::SemanticApply(supplied));
                        work.push(Work::Named(*symbol));
                    }
                    Ty::Nat
                    | Ty::Int
                    | Ty::Real
                    | Ty::String
                    | Ty::Boolean
                    | Ty::Arrow(..)
                    | Ty::Array(_)
                    | Ty::Struct(_)
                    | Ty::Sum(_)
                    | Ty::Var(_)
                    | Ty::Rigid { .. } => work.push(Work::Value(RowSummary::default())),
                    Ty::Undecided => work.push(Work::Value(RowSummary {
                        shaped: true,
                        ..RowSummary::default()
                    })),
                },
                Work::Artifact(ty) => match &ty.tracked {
                    TypeKind::Struct { fields, tail } if shape == Shape::Struct => {
                        work.push(Work::Value(written_row_summary(
                            fields.keys().cloned(),
                            tail,
                        )));
                    }
                    TypeKind::Sum { cases, tail } if shape == Shape::Sum => {
                        work.push(Work::Value(written_row_summary(
                            cases.keys().cloned(),
                            tail,
                        )));
                    }
                    TypeKind::Param { index, .. } => work.push(Work::Value(RowSummary {
                        shaped: false,
                        labels: IndexSet::new(),
                        slots: std::iter::once(*index).collect(),
                    })),
                    TypeKind::Ident(symbol) => work.push(Work::Named(*symbol)),
                    TypeKind::Apply {
                        head,
                        args: supplied,
                        ..
                    } => {
                        work.push(Work::ArtifactApply(supplied));
                        work.push(Work::Named(*head));
                    }
                    TypeKind::Struct { .. }
                    | TypeKind::Array(_)
                    | TypeKind::Sum { .. }
                    | TypeKind::Arrow { .. }
                    | TypeKind::Effects(_)
                    | TypeKind::Prim(_)
                    | TypeKind::Var(_)
                    | TypeKind::Hole
                    | TypeKind::Error => work.push(Work::Value(RowSummary::default())),
                },
                Work::SemanticApply(supplied) => {
                    let (head, crossed_cycle) = values
                        .pop()
                        .expect("an application head leaves one summary");
                    let arguments: Vec<_> = head
                        .slots
                        .into_iter()
                        .filter_map(|index| supplied.get(index as usize))
                        .collect();
                    work.push(Work::FinishApply {
                        shaped: head.shaped,
                        labels: head.labels,
                        arguments: arguments.len(),
                        crossed_cycle,
                    });
                    for argument in arguments.into_iter().rev() {
                        work.push(Work::Semantic(argument));
                    }
                }
                Work::ArtifactApply(supplied) => {
                    let (head, crossed_cycle) = values
                        .pop()
                        .expect("an application head leaves one summary");
                    let arguments: Vec<_> = head
                        .slots
                        .into_iter()
                        .filter_map(|index| supplied.get(index as usize))
                        .collect();
                    work.push(Work::FinishApply {
                        shaped: head.shaped,
                        labels: head.labels,
                        arguments: arguments.len(),
                        crossed_cycle,
                    });
                    for argument in arguments.into_iter().rev() {
                        work.push(Work::Artifact(argument));
                    }
                }
                Work::FinishApply {
                    shaped,
                    labels,
                    arguments,
                    mut crossed_cycle,
                } => {
                    let at = values
                        .len()
                        .checked_sub(arguments)
                        .expect("every selected argument leaves one summary");
                    let mut out = RowSummary {
                        shaped,
                        labels,
                        slots: IndexSet::new(),
                    };
                    for (argument, cycle) in values.drain(at..) {
                        out.shaped |= argument.shaped;
                        out.labels.extend(argument.labels);
                        out.slots.extend(argument.slots);
                        crossed_cycle |= cycle;
                    }
                    values.push((out, crossed_cycle));
                }
                Work::Value(summary) => values.push((summary, false)),
            }
        }
        let (summary, _) = values.pop().expect("the root leaves one summary");
        debug_assert!(values.is_empty());
        debug_assert_eq!(
            done.get(&root).map(|value| value.labels.len()),
            Some(summary.labels.len())
        );
    }

    let mut done = HashMap::new();
    for symbol in types.keys().chain(external.keys()) {
        if !done.contains_key(symbol) {
            resolve(*symbol, types, external, shape, &mut done);
        }
    }
    done.retain(|symbol, _| types.contains_key(symbol) || external.contains_key(symbol));
    done
}

/// One written type's row summary for a requested sense, composing declaration
/// summaries with whichever arguments their forwarding slots select.
/// Concrete rows of another sense contribute nothing.
fn written_summary(labels: impl IntoIterator<Item = String>, tail: &Option<Tail>) -> RowSummary {
    let mut slots = IndexSet::new();
    if let Some(Tail {
        of: Row::Param { index, .. },
        ..
    }) = tail
    {
        slots.insert(*index);
    }
    RowSummary {
        shaped: true,
        labels: labels.into_iter().collect(),
        slots,
    }
}

fn row_summary(ty: &Type, decls: &HashMap<Symbol, RowSummary>, shape: Shape) -> RowSummary {
    match &ty.tracked {
        TypeKind::Struct { fields, tail } if shape == Shape::Struct => {
            written_summary(fields.keys().cloned(), tail)
        }
        TypeKind::Sum { cases, tail } if shape == Shape::Sum => {
            written_summary(cases.keys().cloned(), tail)
        }
        TypeKind::Effects(effects) if shape == Shape::Effect => written_summary(
            effects.effects.keys().map(EffectId::label_key),
            &effects.tail,
        ),
        // The body is the parameter, as in `type Id 'a = 'a`: whatever is written
        // there is the whole of what the declaration stands for, fields
        // included.
        TypeKind::Param { index, .. } => RowSummary {
            shaped: false,
            labels: IndexSet::new(),
            slots: std::iter::once(*index).collect(),
        },
        // A name written bare takes no arguments — the arity check saw to that —
        // so it names no slot this scope could fill.
        TypeKind::Ident(symbol) => decls.get(symbol).cloned().unwrap_or_default(),
        TypeKind::Apply { head, args, .. } => {
            let head = decls.get(head).cloned().unwrap_or_default();
            let mut out = RowSummary {
                shaped: head.shaped,
                labels: head.labels,
                slots: IndexSet::new(),
            };
            // The head's slots are its own parameters, and the arguments are in
            // the scope of whoever wrote this — so what comes back from one is
            // said about *this* declaration's parameters, which is what makes
            // the slots compose.
            //
            // A local declaration's arity has already been checked. Imported
            // semantic interfaces may still be malformed, so an impossible
            // forwarding slot absorbs instead of indexing outside its supplied
            // arguments.
            for index in head.slots {
                let inner = args
                    .get(index as usize)
                    .map(|arg| row_summary(arg, decls, shape))
                    .unwrap_or_default();
                out.shaped |= inner.shaped;
                out.labels.extend(inner.labels);
                out.slots.extend(inner.slots);
            }
            out
        }
        TypeKind::Struct { .. }
        | TypeKind::Array(_)
        | TypeKind::Sum { .. }
        | TypeKind::Arrow { .. }
        | TypeKind::Effects(_)
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => RowSummary::default(),
    }
}

/// Every place a declaration leads back to itself with an argument that gets
/// bigger, as the declaration it was found in and the span to report at.
///
/// Two declarations are in one group when each leads to the other, and inside a
/// group every mention of a member must hand it arguments that cannot grow —
/// see [`grows`] for what that allows. Across groups nothing is restricted:
/// `type Rose 'a = { kids: List (Rose 'a) }` is fine because `List` is somebody
/// else's group, and only the `Rose 'a` inside it is the group's business.
///
/// See [`ErrorKind::GrowingRecursion`] for why the restriction is here, and
/// [`Solve::unfold`](crate::inference) for what rests on it.
fn growing(types: &IndexMap<Symbol, Decl<Type>>) -> Vec<(Symbol, Span)> {
    // Who each declaration mentions, directly, and then everything each one
    // leads to. Closed once rather than walked per pair: the table is one file
    // long, and a pair being mutually reachable is the whole of what a group
    // is.
    let mentions: IndexMap<Symbol, Vec<Symbol>> = types
        .iter()
        .map(|(symbol, decl)| {
            let mut out = Vec::new();
            mentioned(&decl.value, &mut out);
            (*symbol, out)
        })
        .collect();
    let reachable = closure(&mentions);

    // Which group each declaration is in, worked out once for the group rather
    // than once per member. A declaration reaching itself is what puts it in
    // one at all; everything mutually reachable with it is in the same one, and
    // shares the answer — so the members are collected from the first of them
    // reached and handed to the rest, and a declaration on no loop has no entry.
    let mut groups: HashMap<Symbol, Rc<[Symbol]>> = HashMap::new();
    for (symbol, reaches) in &reachable {
        if groups.contains_key(symbol) || !reaches.contains(symbol) {
            continue;
        }
        let group: Rc<[Symbol]> = reaches
            .iter()
            .copied()
            .filter(|other| {
                reachable
                    .get(other)
                    .is_some_and(|back| back.contains(symbol))
            })
            .collect();
        for member in group.iter() {
            groups.insert(*member, group.clone());
        }
    }

    let mut out = Vec::new();
    for (symbol, decl) in types {
        let Some(group) = groups.get(symbol) else {
            continue;
        };
        grows(&decl.value, group, &mut |at| out.push((*symbol, at)));
    }
    out
}

/// Everything each node leads to through one edge or more. A node leads to
/// itself exactly when it is on a loop, which is why the set holds no node for
/// free: two nodes each leading to the other is the whole of what a group of
/// mutually recursive declarations is, and one node leading to itself is what
/// makes a lone declaration one.
///
/// One walk per node, with a set that answers in one look — the pairwise
/// question is asked about every ordered pair, and asking it that way walked
/// the table twice per pair. Generic over the node because the same closure
/// answers "which parameter slots does this one hand itself on to", in
/// [`kinds`]; insertion-ordered so that nothing downstream depends on a hash.
fn closure<T: Copy + Eq + Hash>(edges: &IndexMap<T, Vec<T>>) -> IndexMap<T, IndexSet<T>> {
    edges
        .keys()
        .map(|from| {
            let mut seen = IndexSet::new();
            let mut stack = vec![*from];
            while let Some(at) = stack.pop() {
                for next in edges.get(&at).into_iter().flatten() {
                    if seen.insert(*next) {
                        stack.push(*next);
                    }
                }
            }
            (*from, seen)
        })
        .collect()
}

/// Which parameters survive unfolding: which slots the argument written at them
/// reaches a position of what the declaration stands for.
///
/// A parameter written anywhere in the body reaches one, except inside an
/// argument to another declaration — there it reaches one only if *that* slot
/// does, because unfolding the head is what decides whether the argument is
/// kept or thrown away. `'a` in `type Box 'a = { it: 'a }` reaches one; `'a` in
/// `type Ptr 'a = Nat` never appears and so reaches none; `'a` in
/// `type Alias 'a = Ptr 'a` appears only where `Ptr` discards it, so it reaches
/// none either.
///
/// Which makes this a fixpoint over the declaration graph for the same reason
/// [`kinds`] is one: declarations are hoisted and may name each other, so no
/// one body decides its own answer. Each occurrence contributes the slots it
/// sits inside, all of which must survive for the occurrence to; a slot
/// survives if any one of its occurrences does. Both are monotone in a set that
/// only grows and is bounded by the parameters in the program, so the loop
/// stops. Starting from nothing is the safe start: a slot wrongly called
/// irrelevant costs an unfolding, and a slot wrongly called relevant would let
/// [`Rule::Congruent`](crate::inference::Rule) decide something unfolding
/// disagrees with.
///
/// A `..'r` tail counts as an occurrence, being the one place a parameter is not
/// written as a [`TypeKind::Param`]: what is spliced in there is as much part
/// of what the declaration stands for as a field is.
fn relevance(types: &IndexMap<Symbol, Decl<Type>>) -> HashSet<Slot> {
    /// Every occurrence of a parameter in one body, as the slots it is nested
    /// inside — innermost order does not matter, since all of them have to
    /// survive.
    fn occurrences(ty: &Type, under: &mut Vec<Slot>, out: &mut impl FnMut(u32, &[Slot])) {
        match &ty.tracked {
            TypeKind::Param { index, .. } => out(*index, under),
            TypeKind::Array(element) => occurrences(element, under, out),
            TypeKind::Struct { fields, tail } => {
                for field in fields.values() {
                    if let Some(value) = field.value() {
                        occurrences(value, under, out);
                    }
                }
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = tail
                {
                    out(*index, under);
                }
            }
            TypeKind::Sum { cases, tail } => {
                for case in cases.values() {
                    if let Some(payload) = case.payload() {
                        occurrences(payload, under, out);
                    }
                }
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = tail
                {
                    out(*index, under);
                }
            }
            // A row of effects is its tail, as far as this is concerned: the
            // labels are names and only what is spliced in can grow.
            TypeKind::Effects(effects) => {
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = effects.tail
                {
                    out(index, under);
                }
            }
            TypeKind::Arrow { from, to, effects } => {
                occurrences(from, under, out);
                occurrences(to, under, out);
                // The effect tail counts, for the reason a struct's and a
                // sum's do: what is spliced in there is as much part of what
                // the declaration stands for as a field is.
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = effects.tail
                {
                    out(index, under);
                }
            }
            TypeKind::Apply { head, args, .. } => {
                for (at, arg) in args.iter().enumerate() {
                    under.push((*head, at as u32));
                    occurrences(arg, under, out);
                    under.pop();
                }
            }
            TypeKind::Ident(_)
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole
            | TypeKind::Error => {}
        }
    }

    let mut demands: IndexMap<Slot, Vec<Vec<Slot>>> = IndexMap::new();
    for (symbol, decl) in types {
        let mut under = Vec::new();
        occurrences(&decl.value, &mut under, &mut |index, guards| {
            demands
                .entry((*symbol, index))
                .or_default()
                .push(guards.to_vec());
        });
    }

    let mut relevant: HashSet<Slot> = HashSet::new();
    loop {
        let mut grew = false;
        for (slot, occurrences) in &demands {
            if relevant.contains(slot) {
                continue;
            }
            // An occurrence nested inside nothing is a position of the body
            // outright, which is the base case the empty `all` gives for free.
            if occurrences
                .iter()
                .any(|guards| guards.iter().all(|slot| relevant.contains(slot)))
            {
                relevant.insert(*slot);
                grew = true;
            }
        }
        if !grew {
            return relevant;
        }
    }
}

/// Every declaration a type mentions, at any depth. A parameter is not one: it
/// is a local, and no declaration answers to it.
fn mentioned(ty: &Type, out: &mut Vec<Symbol>) {
    match &ty.tracked {
        TypeKind::Ident(symbol) => out.push(*symbol),
        TypeKind::Array(element) => mentioned(element, out),
        TypeKind::Apply { head, args, .. } => {
            out.push(*head);
            for arg in args {
                mentioned(arg, out);
            }
        }
        // An effect row names effects and no declared types, so there is
        // nothing in one to mention.
        TypeKind::Arrow { from, to, .. } => {
            mentioned(from, out);
            mentioned(to, out);
        }
        // An effect is not a declared type, so a row of them names none.
        TypeKind::Effects(_) => {}
        TypeKind::Struct { fields, .. } => {
            for field in fields.values() {
                if let Some(value) = field.value() {
                    mentioned(value, out);
                }
            }
        }
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
                    mentioned(payload, out);
                }
            }
        }
        TypeKind::Param { .. }
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => {}
    }
}

/// Report every mention of a `group` member in `ty` that hands it an argument
/// which could get bigger, in order.
///
/// An argument is safe when it is one of two things:
///
/// - one of the mentioning declaration's own parameters, written alone — it is
///   then whatever came in, passed straight through;
/// - a type mentioning no parameter at all — it is then written out in full in
///   the program and is the same type every time round, however many names or
///   applications it is built from.
///
/// Anything else is a type built *out of* a parameter, and that is what grows:
/// `type T 'a = { next: T { x: 'a } }` hands on `{ x: 'a }`, then
/// `{ x: { x: 'a } }`, and never comes back round.
///
/// So the arguments a group can reach are drawn from the arguments it was given
/// at the use site plus the finitely many param-free types written inside it —
/// a finite set, and therefore finitely many argument lists, which is what makes
/// the solver's assumption repeat. Order and repetition are free:
/// `type A 'a 'b = { x: B 'b 'a }` only ever permutes what it was handed. See
/// [`ErrorKind::GrowingRecursion`] and [`Solve::unfold`](crate::inference).
fn grows(ty: &Type, group: &[Symbol], report: &mut impl FnMut(Span)) {
    match &ty.tracked {
        // A group member written bare hands on nothing — and if it takes
        // something, the arity check has already spoken and this would be a
        // second complaint about one mistake.
        TypeKind::Ident(_) => {}
        TypeKind::Array(element) => grows(element, group, report),
        TypeKind::Apply {
            head,
            head_span,
            args,
        } => {
            let safe = |arg: &Type| {
                matches!(arg.tracked, TypeKind::Param { .. }) || !mentions_a_parameter(arg)
            };
            if group.contains(head) && !args.iter().all(safe) {
                report(*head_span);
            }
            // The arguments are still walked: a member hidden inside one is as
            // much a way round as a member at the top.
            for arg in args {
                grows(arg, group, report);
            }
        }
        TypeKind::Arrow { from, to, .. } => {
            grows(from, group, report);
            grows(to, group, report);
        }
        TypeKind::Effects(_) => {}
        TypeKind::Struct { fields, .. } => {
            for field in fields.values() {
                if let Some(value) = field.value() {
                    grows(value, group, report);
                }
            }
        }
        TypeKind::Sum { cases, .. } => {
            for case in cases.values() {
                if let Some(payload) = case.payload() {
                    grows(payload, group, report);
                }
            }
        }
        TypeKind::Param { .. }
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => {}
    }
}

/// Whether a written type is built out of any of the parameters of the
/// declaration whose body it is in. Every [`TypeKind::Param`] in a body is one
/// of them, since a parameter is scoped to the declaration that binds it, so
/// this asks about parameters at all rather than about a particular list.
///
/// The `..'r` tail counts. It is the one place a parameter appears without being
/// a [`TypeKind::Param`] node, and a type handed on with a parameter in its tail
/// grows exactly as one with a parameter in a field does.
fn mentions_a_parameter(ty: &Type) -> bool {
    match &ty.tracked {
        TypeKind::Param { .. } => true,
        TypeKind::Array(element) => mentions_a_parameter(element),
        TypeKind::Apply { args, .. } => args.iter().any(mentions_a_parameter),
        TypeKind::Arrow { from, to, effects } => {
            tails_a_parameter(&effects.tail)
                || mentions_a_parameter(from)
                || mentions_a_parameter(to)
        }
        TypeKind::Effects(effects) => tails_a_parameter(&effects.tail),
        TypeKind::Struct { fields, tail } => {
            tails_a_parameter(tail)
                || fields
                    .values()
                    .any(|field| field.value().is_some_and(mentions_a_parameter))
        }
        TypeKind::Sum { cases, tail } => {
            tails_a_parameter(tail)
                || cases
                    .values()
                    .any(|case| case.payload().is_some_and(mentions_a_parameter))
        }
        TypeKind::Ident(_)
        | TypeKind::Prim(_)
        | TypeKind::Var(_)
        | TypeKind::Hole
        | TypeKind::Error => false,
    }
}

impl Names {
    /// Searched innermost first, so a lambda argument hides a definition of the
    /// same name.
    fn get(&self, name: &str) -> Option<Symbol> {
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.name == name)
            .map(|binding| binding.symbol)
    }

    fn bind(&mut self, name: String, symbol: Symbol) {
        self.bindings.push(Binding { name, symbol });
    }

    fn mark(&self) -> usize {
        self.bindings.len()
    }

    fn release(&mut self, mark: usize) {
        self.bindings.truncate(mark);
    }
}

impl Builder<'_> {
    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.errors.push(Error { span, kind });
    }

    /// Mint every module in one statement list, depth-first, and record what
    /// each remaining statement was written in.
    ///
    /// Modules go first over the whole tree because everything else is declared
    /// *into* one: a type cannot be minted under `A::B` before `A::B` is a
    /// symbol. Nothing is resolved here — a path's segments are looked up when
    /// the body naming them is lowered, by which time every module in the
    /// bundle exists.
    ///
    /// A module whose body was never spliced in contributes an empty one, so a
    /// file the loader could not read costs its own complaint and no other.
    fn flatten(&mut self, stmts: Vec<Stmt>, flat: &mut Flat) {
        let outer = self.module;
        for stmt in stmts {
            match stmt.tracked {
                StmtKind::Type { name, params, body } => {
                    flat.types.push((outer, name, params, body))
                }
                StmtKind::Effect { name, params, body } => {
                    flat.effects.push((outer, name, params, body))
                }
                StmtKind::Let { pattern, ty, body } => {
                    let at = flat.terms.len();
                    flat.terms.push((outer, pattern, ty, body));
                    flat.values.push(FlatValue::Term(at));
                }
                StmtKind::Extern {
                    name,
                    ty,
                    abi,
                    target,
                } => {
                    let at = flat.externs.len();
                    flat.externs.push((outer, name, ty, abi, target));
                    flat.values.push(FlatValue::Extern(at));
                }
                StmtKind::Module { name, body } => {
                    self.module = Some(self.declare_module(&name));
                    self.flatten(body.unwrap_or_default(), flat);
                    self.module = outer;
                }
            }
        }
    }

    /// Declare a global in the module being lowered into. `None` when that
    /// module already declares the name in that namespace: the first
    /// declaration is the one that stands, and the repeat is reported against
    /// it.
    ///
    /// Only against the same module's own declarations. A name that shadows one
    /// an enclosing module declares is what a scope is for, not a repeat.
    fn declare(&mut self, scope: Scope, name: &TrackedString) -> Option<Symbol> {
        let namespace = Namespace::from(scope);
        let key = (self.module, namespace, name.tracked.clone());
        if let Some(&(_, previous)) = self.globals.get(&key) {
            self.error(
                name.span,
                ErrorKind::Duplicate {
                    name: name.tracked.clone(),
                    namespace,
                    previous,
                },
            );
            return None;
        }
        let symbol = self
            .mint
            .global(self.module, namespace, &name.tracked)
            .expect("the name table already ruled out a repeat");
        self.globals.insert(key, (symbol, name.span));
        Some(symbol)
    }

    /// Install direct dependency headers before local declarations are
    /// flattened. The dependency bundle name is an ordinary root module, so
    /// the existing strict path walk handles nested modules without a second
    /// resolver.
    fn import_dependencies(
        &mut self,
        dependencies: &[DependencyImport<'_>],
        linked: &[artifact::Artifact],
        program: &mut Program,
    ) {
        let mut symbols: HashMap<(Namespace, String), Symbol> = HashMap::new();
        let mut identities = HashSet::new();
        let mut aliases = HashSet::new();
        let mut valid = Vec::with_capacity(dependencies.len());
        for import in dependencies {
            let identity = (
                import.artifact.header().identity.name.clone(),
                import.artifact.header().identity.version.clone(),
            );
            if !source_identifier(import.alias) {
                self.errors.push(Error {
                    span: Span::default(),
                    kind: ErrorKind::InvalidDependencyAlias {
                        alias: import.alias.to_string(),
                    },
                });
            } else if identities.contains(&identity) {
                self.errors.push(Error {
                    span: Span::default(),
                    kind: ErrorKind::DuplicateDependency {
                        name: import.artifact.header().identity.name.clone(),
                        version: import.artifact.header().identity.version.clone(),
                    },
                });
            } else if aliases.contains(import.alias) {
                self.errors.push(Error {
                    span: Span::default(),
                    kind: ErrorKind::DuplicateDependencyAlias {
                        alias: import.alias.to_string(),
                    },
                });
            } else {
                identities.insert(identity);
                aliases.insert(import.alias);
                valid.push(*import);
            }
        }

        // Identities and named parameter senses are published independently of
        // the schemes which contain their generated row keys. Gather both
        // tables before converting any semantic type so forward references and
        // transitive aliases receive the same shape-aware normalization.
        let mut effect_rows = ImportedEffectRows::default();
        for dependency in linked
            .iter()
            .chain(valid.iter().map(|import| import.artifact))
        {
            for declaration in &dependency.header().effects {
                if dependency_path(dependency, &declaration.name).is_none() {
                    continue;
                }
                let (identity, published_interface) = match &declaration.identity {
                    Some(identity) => (
                        EffectId::structural(
                            identity.name.clone(),
                            canonical_effect_interface(&identity.interface),
                        ),
                        identity.interface.clone(),
                    ),
                    None if matches!(declaration.kind, artifact::EffectKind::Operations(_)) => {
                        let name = declaration
                            .name
                            .rsplit("::")
                            .next()
                            .unwrap_or(&declaration.name)
                            .to_string();
                        (
                            EffectId::structural(
                                name,
                                canonical_effect_interface(&format!(
                                    "unresolved:{}",
                                    declaration.name
                                )),
                            ),
                            format!("unresolved:{}", declaration.name),
                        )
                    }
                    None => continue,
                };
                let canonical = identity.row_key();
                let name = identity.name();
                effect_rows.labels.insert(
                    format!("{name}\u{1f}{published_interface}"),
                    canonical.clone(),
                );
                effect_rows.labels.insert(
                    EffectId::structural(name.to_string(), published_interface).row_key(),
                    canonical,
                );
                effect_rows.insert_identity(&declaration.name, identity);
            }
            for declaration in &dependency.header().types {
                if dependency_path(dependency, &declaration.name).is_none() {
                    continue;
                }
                effect_rows.arguments.insert(
                    declaration.name.clone(),
                    declaration.params.iter().map(|param| param.sense).collect(),
                );
            }
        }

        // Every linked declaration gets a semantic symbol, including
        // transitive implementation dependencies. Only the direct pass below
        // installs those symbols into source resolution tables.
        for dependency in linked
            .iter()
            .chain(valid.iter().map(|import| import.artifact))
        {
            for (namespace, qualified) in dependency
                .header
                .values
                .iter()
                .map(|value| (Namespace::Terms, &value.name))
                .chain(
                    dependency
                        .header
                        .types
                        .iter()
                        .map(|value| (Namespace::Types, &value.name)),
                )
                .chain(
                    dependency
                        .header
                        .effects
                        .iter()
                        .map(|value| (Namespace::Effects, &value.name)),
                )
            {
                if dependency_path(dependency, qualified).is_none() {
                    continue;
                }
                imported_symbol(
                    self.mint,
                    namespace,
                    qualified,
                    &mut symbols,
                    &mut program.external_names,
                );
            }
        }

        // Declare every source-visible name first. Semantic interfaces may
        // refer forward, sideways, or through a transitive implementation
        // dependency, so conversion happens in the second pass.
        for import in &valid {
            let dependency = import.artifact;
            let root_name = import.alias;
            // Valid dependency aliases are unique, and imports are installed
            // before source modules are flattened, so this root is necessarily
            // new. Modeling an existing branch here only hid that invariant.
            let root = self
                .mint
                .module(None, root_name)
                .expect("a dependency root was checked before minting");
            self.modules
                .insert((None, root_name.to_string()), (root, Span::default()));
            for (namespace, qualified) in dependency
                .header
                .values
                .iter()
                .map(|value| (Namespace::Terms, &value.name))
                .chain(
                    dependency
                        .header
                        .types
                        .iter()
                        .map(|value| (Namespace::Types, &value.name)),
                )
                .chain(
                    dependency
                        .header
                        .effects
                        .iter()
                        .map(|value| (Namespace::Effects, &value.name)),
                )
            {
                let Some(parts) = dependency_path(dependency, qualified) else {
                    continue;
                };
                let (modules, name) = parts.split_at(parts.len() - 1);
                let mut parent = root;
                for segment in modules {
                    parent = match self.modules.get(&(Some(parent), segment.clone())) {
                        Some(&(module, _)) => module,
                        None => {
                            let module = self
                                .mint
                                .module(Some(parent), segment)
                                .expect("an imported module was checked before minting");
                            self.modules
                                .insert((Some(parent), segment.clone()), (module, Span::default()));
                            module
                        }
                    };
                }
                let key = (Some(parent), namespace, name[0].clone());
                if self.globals.contains_key(&key) {
                    continue;
                }
                let symbol = symbols[&(namespace, qualified.clone())];
                self.globals.insert(key, (symbol, Span::default()));
            }
            if import.alias == "std" {
                self.std_prelude = self
                    .modules
                    .get(&(Some(root), "prelude".to_string()))
                    .map(|&(module, _)| module);
            }
        }

        for dependency in linked
            .iter()
            .chain(valid.iter().map(|import| import.artifact))
        {
            for value in &dependency.header().values {
                let Some(&symbol) = symbols.get(&(Namespace::Terms, value.name.clone())) else {
                    continue;
                };
                let scheme = import_scheme(
                    self.mint,
                    &value.scheme,
                    &mut symbols,
                    &mut program.external_names,
                    &effect_rows,
                );
                program.external_schemes.insert(symbol, scheme);
            }
            for declaration in &dependency.header().types {
                let Some(&symbol) = symbols.get(&(Namespace::Types, declaration.name.clone()))
                else {
                    continue;
                };
                let params: Vec<ParamKind> = declaration
                    .params
                    .iter()
                    .map(|param| {
                        let lacks = match param.sense {
                            artifact::Sense::Effects => param
                                .lacks
                                .iter()
                                .map(|label| effect_rows.label(label))
                                .collect(),
                            artifact::Sense::Type
                            | artifact::Sense::Fields
                            | artifact::Sense::Cases => param.lacks.iter().cloned().collect(),
                        };
                        match param.sense {
                            artifact::Sense::Type => ParamKind::Type { lacks },
                            artifact::Sense::Fields => ParamKind::Fields { lacks },
                            artifact::Sense::Cases => ParamKind::Cases { lacks },
                            artifact::Sense::Effects => ParamKind::Effects { lacks },
                        }
                    })
                    .collect();
                // A declared type's parameter table is authoritative. Unlike a
                // value scheme it cannot quantify presences or carry a formula;
                // malformed artifact metadata must not make opening index a
                // differently sized argument list.
                let body = import_type(
                    self.mint,
                    &declaration.scheme.body,
                    &mut symbols,
                    &mut program.external_names,
                    &effect_rows,
                );
                let body = clamp_bounds(body, params.len(), 0);
                let scheme = Scheme::new(params.len() as u32, body);
                self.arities.insert(symbol, params.len());
                program.external_types.insert(
                    symbol,
                    ExternalType {
                        params,
                        relevant: declaration
                            .params
                            .iter()
                            .map(|param| param.relevant)
                            .collect(),
                        scheme,
                        unresolved: None,
                    },
                );
            }
            for declaration in &dependency.header().effects {
                let Some(&symbol) = symbols.get(&(Namespace::Effects, declaration.name.clone()))
                else {
                    continue;
                };
                if let Some(identity) = effect_rows.identities.get(&declaration.name) {
                    program.effect_ids.insert(symbol, identity.clone());
                }
                // The parameters the effect binds, read as the header says
                // them: an application in this bundle is counted against
                // them, and inference mints one fresh argument per entry.
                let kinds: Vec<ParamKind> = declaration
                    .params
                    .iter()
                    .map(|param| {
                        let lacks: IndexSet<String> = param
                            .lacks
                            .iter()
                            .map(|label| match param.sense {
                                artifact::Sense::Effects => effect_rows.label(label),
                                _ => label.clone(),
                            })
                            .collect();
                        match param.sense {
                            artifact::Sense::Type => ParamKind::Type { lacks },
                            artifact::Sense::Fields => ParamKind::Fields { lacks },
                            artifact::Sense::Cases => ParamKind::Cases { lacks },
                            artifact::Sense::Effects => ParamKind::Effects { lacks },
                        }
                    })
                    .collect();
                let count = kinds.len();
                self.arities.insert(symbol, count);
                self.imported_effect_params.insert(symbol, kinds);
                match &declaration.kind {
                    artifact::EffectKind::Operations(operations) => {
                        // Trusted artifacts always carry the structural identity
                        // of an operation declaration. Keep malformed or older
                        // direct-only interfaces total as well: an absent
                        // identity must never make every use of the effect pure.
                        // The normalization table above installs this for
                        // every operation declaration, including legacy
                        // artifacts with no published identity.
                        debug_assert!(program.effect_ids.contains_key(&symbol));
                        self.expanded.insert(
                            symbol,
                            [(declaration.name.clone(), symbol)].into_iter().collect(),
                        );
                        self.operations.insert(
                            symbol,
                            operations
                                .iter()
                                .map(|operation| match &operation.selector {
                                    artifact::OperationSelector::Unnamed => {
                                        OperationSelector::Unnamed
                                    }
                                    artifact::OperationSelector::Named(name) => {
                                        OperationSelector::Named(name.clone())
                                    }
                                })
                                .collect(),
                        );
                        for operation in operations {
                            let from = import_type(
                                self.mint,
                                &operation.from,
                                &mut symbols,
                                &mut program.external_names,
                                &effect_rows,
                            );
                            let from = clamp_bounds(from, count, 0);
                            let to = import_type(
                                self.mint,
                                &operation.to,
                                &mut symbols,
                                &mut program.external_names,
                                &effect_rows,
                            );
                            let to = clamp_bounds(to, count, 0);
                            let selector = match &operation.selector {
                                artifact::OperationSelector::Unnamed => OperationSelector::Unnamed,
                                artifact::OperationSelector::Named(name) => {
                                    OperationSelector::Named(name.clone())
                                }
                            };
                            program
                                .external_operations
                                .insert((symbol, selector), (from, to));
                        }
                    }
                    artifact::EffectKind::Alias(row) => {
                        // The alias's own parameters, as the positions its
                        // row refers to; each becomes a parameter symbol the
                        // syntactic row can name, so the alias expands at a
                        // use exactly as a local one does.
                        let params: Vec<Symbol> = (0..count)
                            .map(|index| {
                                self.mint.local(
                                    None,
                                    Namespace::Types,
                                    &format!("{}'{index}", declaration.name),
                                )
                            })
                            .collect();
                        let mut cases = Vec::new();
                        let mut expansion = IndexMap::new();
                        for case in &row.cases {
                            let target = imported_symbol(
                                self.mint,
                                Namespace::Effects,
                                &case.name,
                                &mut symbols,
                                &mut program.external_names,
                            );
                            expansion.insert(case.name.clone(), target);
                            let args = case
                                .args
                                .iter()
                                .map(|arg| {
                                    imported_syntax(
                                        self.mint,
                                        arg,
                                        &params,
                                        &mut symbols,
                                        &mut program.external_names,
                                        &effect_rows,
                                        0,
                                    )
                                })
                                .collect();
                            cases.push(AliasCase {
                                name_span: Span::default(),
                                symbol: target,
                                args,
                            });
                        }
                        let tail = row.tail.and_then(|index| {
                            params.get(index as usize).map(|symbol| Tail {
                                span: Span::default(),
                                of: Row::Param {
                                    symbol: *symbol,
                                    index,
                                },
                            })
                        });
                        self.expanded.insert(symbol, expansion);
                        self.imported_aliases.insert(symbol);
                        self.alias_bodies.insert(
                            symbol,
                            AliasBody {
                                span: Span::default(),
                                cases,
                                tail,
                            },
                        );
                    }
                }
            }
        }

        // Imported aliases can point forward and through any number of linked
        // headers. Close the expansion map over itself after every header has
        // been installed; the one-pass insertion above otherwise leaves
        // `Alias -> Alias -> Effect` ending at an identity-less alias, which is
        // subsequently dropped while rows are rekeyed.
        loop {
            let previous = self.expanded.clone();
            let mut grew = false;
            for expansion in self.expanded.values_mut() {
                let reached: Vec<_> = expansion.values().copied().collect();
                for target in reached {
                    for (name, concrete) in previous.get(&target).into_iter().flatten() {
                        grew |= expansion.insert(name.clone(), *concrete).is_none();
                    }
                }
            }
            if !grew {
                break;
            }
        }

        // A direct-only caller may hand us an alias that reaches a transitive
        // effect without its defining header. Give that leaf a stable recovery
        // identity before rows are rekeyed: dropping it would turn an annotated
        // effect row into a pure one, while one shared sentinel would make
        // unrelated missing effects equal. Graph-aware callers install the real
        // identity above and therefore never take this path.
        let unresolved_effects: Vec<_> = program
            .external_names
            .iter()
            .filter(|(symbol, _)| {
                self.mint.namespace(**symbol) == Namespace::Effects
                    && !program.effect_ids.contains_key(*symbol)
                    && !self.expanded.contains_key(*symbol)
            })
            .map(|(symbol, qualified)| (*symbol, qualified.clone()))
            .collect();
        for (symbol, qualified) in unresolved_effects {
            let name = qualified
                .rsplit("::")
                .next()
                .unwrap_or(qualified.as_str())
                .to_string();
            program.effect_ids.insert(
                symbol,
                EffectId::structural(name, format!("unresolved:{qualified}")),
            );
            self.expanded
                .entry(symbol)
                .or_insert_with(|| [(qualified, symbol)].into_iter().collect());
        }

        // A direct-only caller may hand us an interface containing a named
        // transitive type without its defining header. Keep inference total by
        // installing an undecided recovery declaration. Graph-aware callers
        // replace these with the real schemes during the import pass above.
        let unresolved: Vec<_> = program
            .external_names
            .keys()
            .copied()
            .filter(|symbol| {
                self.mint.namespace(*symbol) == Namespace::Types
                    && !program.external_types.contains_key(symbol)
            })
            .collect();
        for symbol in unresolved {
            program.external_types.insert(
                symbol,
                ExternalType {
                    params: Vec::new(),
                    relevant: Vec::new(),
                    scheme: Scheme::new(0, Rc::new(Ty::default())),
                    unresolved: program.external_names.get(&symbol).cloned(),
                },
            );
        }
    }

    /// [`declare`](Self::declare) in the module namespace, which has a door of
    /// its own because a module is what everything else is declared *into*.
    ///
    /// A repeat is reported and the module already there is handed back, so the
    /// declarations under the second `module A` join the first one's rather
    /// than being lost — one complaint, and every name in either body still
    /// resolves.
    fn declare_module(&mut self, name: &TrackedString) -> Module {
        let key = (self.module, name.tracked.clone());
        if let Some(&(module, previous)) = self.modules.get(&key) {
            self.error(
                name.span,
                ErrorKind::Duplicate {
                    name: name.tracked.clone(),
                    namespace: Namespace::Modules,
                    previous,
                },
            );
            return module;
        }
        let module = self
            .mint
            .module(self.module, &name.tracked)
            .expect("the name table already ruled out a repeat");
        self.modules.insert(key, (module, name.span));
        module
    }

    /// The global `name` declares in `module` exactly — no outward walk.
    fn global_in(
        &self,
        module: Option<Module>,
        namespace: Namespace,
        name: &str,
    ) -> Option<Symbol> {
        self.globals
            .get(&(module, namespace, name.to_owned()))
            .map(|&(symbol, _)| symbol)
    }

    /// R9's walk for a global: the module being lowered into, then each
    /// enclosing module in turn, then the bundle root. The first match wins.
    /// If the user tree has no match, direct members of the configured std
    /// prelude provide the final fallback.
    fn outward(&self, namespace: Namespace, name: &str) -> Option<Symbol> {
        let mut at = self.module;
        loop {
            if let Some(symbol) = self.global_in(at, namespace, name) {
                return Some(symbol);
            }
            let Some(module) = at else {
                break;
            };
            at = self.mint.parent(module.symbol());
        }
        self.std_prelude
            .and_then(|prelude| self.global_in(Some(prelude), namespace, name))
    }

    /// [`outward`](Self::outward) about modules, which is how a path's first
    /// segment is resolved. A direct child module of the configured std
    /// prelude is the final fallback.
    fn module_outward(&self, name: &str) -> Option<Module> {
        let mut at = self.module;
        loop {
            if let Some(&(module, _)) = self.modules.get(&(at, name.to_owned())) {
                return Some(module);
            }
            let Some(module) = at else {
                break;
            };
            at = self.mint.parent(module.symbol());
        }
        self.std_prelude.and_then(|prelude| {
            self.modules
                .get(&(Some(prelude), name.to_owned()))
                .map(|&(module, _)| module)
        })
    }

    /// Which module a path's segments name: `Some(None)` for a bare name, whose
    /// segments are none at all.
    ///
    /// R10 in one loop. The first segment resolves by the R9 walk; every later
    /// one strictly inside the module the previous one named, with no outward
    /// step, because a path says where to look and a walk would let it mean
    /// somewhere else.
    fn segments(&mut self, path: &parse::Path) -> Option<Option<Module>> {
        let mut at: Option<Module> = None;
        for (index, segment) in path.modules.iter().enumerate() {
            let found = match index {
                0 => self.module_outward(&segment.tracked),
                _ => self
                    .modules
                    .get(&(at, segment.tracked.clone()))
                    .map(|&(module, _)| module),
            };
            let Some(module) = found else {
                self.error(
                    segment.span,
                    ErrorKind::Undefined {
                        name: segment.tracked.clone(),
                        namespace: Namespace::Modules,
                    },
                );
                return None;
            };
            at = Some(module);
        }
        Some(at)
    }

    /// The symbol `path` names in `namespace`, or why it names none.
    ///
    /// A bare name is looked for among the locals first and then by R9's walk;
    /// a qualified one strictly inside the module its segments named. Only
    /// terms have locals, so only they are asked about them.
    fn find(&mut self, path: &parse::Path, namespace: Namespace) -> Result<Symbol, Missing> {
        let Some(module) = self.segments(path) else {
            return Err(Missing::Segment);
        };
        let found = match module {
            Some(module) => self.global_in(Some(module), namespace, &path.name.tracked),
            None => self
                .local(namespace, &path.name.tracked)
                .or_else(|| self.outward(namespace, &path.name.tracked)),
        };
        found.ok_or(Missing::Name)
    }

    /// The local `name` means here, if the namespace has locals at all.
    fn local(&self, namespace: Namespace, name: &str) -> Option<Symbol> {
        match namespace {
            Namespace::Terms => self.terms.get(name),
            Namespace::Types | Namespace::Effects | Namespace::Modules => None,
        }
    }

    /// [`find`](Self::find) with the complaint attached: a name the path's
    /// module does not declare is reported at the name, in the namespace the
    /// position it was written at demands.
    fn resolve(&mut self, path: &parse::Path, namespace: Namespace) -> Option<Symbol> {
        match self.find(path, namespace) {
            Ok(symbol) => Some(symbol),
            Err(Missing::Segment) => None,
            Err(Missing::Name) => {
                self.error(
                    path.name.span,
                    ErrorKind::Undefined {
                        name: path.name.tracked.clone(),
                        namespace,
                    },
                );
                None
            }
        }
    }

    /// Mint one declaration's parameters, in the order they were written.
    ///
    /// Minted as locals, the way a lambda's arguments are, so a parameter is a
    /// symbol like any other and the debugger lists and cross-highlights it
    /// with no special case. Nothing is put in scope here: the parameters of
    /// every declaration are minted before any body is read, and only the one
    /// declaration's own are in scope while its body is.
    ///
    /// A repeat is reported against the earlier name in this same list, and
    /// binds nothing — which is what makes the list this returns the number of
    /// arguments the declaration takes. Only against this declaration's own
    /// parameters: a parameter shadowing a declared type is what a scope is
    /// for, not a repeat.
    fn declare_params(&mut self, params: &[TrackedString]) -> Vec<Param> {
        let mut bound = Vec::new();
        let mut seen: Vec<(&str, Span)> = Vec::new();
        for name in params {
            if let Some(&(_, previous)) = seen.iter().find(|(seen, _)| *seen == name.tracked) {
                self.error(
                    name.span,
                    ErrorKind::DuplicateParameter {
                        name: name.tracked.clone(),
                        previous,
                    },
                );
                continue;
            }
            seen.push((&name.tracked, name.span));
            let symbol = self
                .mint
                .local(self.module, Namespace::Types, &name.tracked);
            bound.push(Param {
                span: name.span,
                symbol,
                // Both read off the bodies once every body is in; see [`kinds`]
                // and [`relevance`].
                kind: ParamKind::Type {
                    lacks: IndexSet::new(),
                },
                relevant: false,
            });
        }
        bound
    }

    /// Put one declaration's parameters in scope for the length of its body,
    /// as the variable scope [`ty`](Self::ty) resolves against. The caller
    /// releases them; see [`build`].
    ///
    /// No stack and no marks, unlike [`Names`]: a declaration's body is the
    /// whole scope of its parameters and no declaration nests inside another,
    /// so the table holds one declaration's worth at a time and emptying it is
    /// the whole of releasing them.
    fn scope_params(&mut self, params: &[Param]) {
        for (index, param) in params.iter().enumerate() {
            let name = self.mint.name(param.symbol).to_string();
            self.params.insert(name, (param.symbol, index as u32));
        }
    }

    /// Lower one of the four disjoint effect declaration bodies.
    fn effect(&mut self, body: EffectBody) -> Effect {
        match body {
            parse::EffectBody::Empty => Effect::Operations(IndexMap::new()),
            // Alias bodies are lowered when first named; see [`build`].
            parse::EffectBody::Alias(row) => Effect::Alias(Alias {
                body: AliasBody::default(),
                expanded: row.span.track(TypeKind::Error),
            }),
            parse::EffectBody::Unnamed { signature } => {
                let span = signature.span;
                let selector = OperationSelector::Unnamed;
                let operation = self.operation(&span.track("<unnamed>".to_string()), *signature);
                Effect::Operations([(selector, operation)].into_iter().collect())
            }
            parse::EffectBody::Named(fields) => {
                let mut operations: IndexMap<OperationSelector, Operation> = IndexMap::new();
                for (name, signature) in fields {
                    let selector = OperationSelector::Named(name.tracked.clone());
                    let operation = self.operation(&name, *signature);
                    if let Some(previous) = operations.get(&selector) {
                        self.error(
                            name.span,
                            ErrorKind::DuplicateOperation {
                                name: name.tracked,
                                previous: previous.name_span,
                            },
                        );
                        continue;
                    }
                    operations.insert(selector, operation);
                }
                Effect::Operations(operations)
            }
        }
    }

    /// Whether a name resolved in the effect namespace is an alias — local,
    /// lowered or not yet, or imported.
    fn is_alias(&self, symbol: Symbol) -> bool {
        self.pending_aliases.contains_key(&symbol) || self.alias_bodies.contains_key(&symbol)
    }

    /// One alias body, lowered the first time it is asked for.
    ///
    /// Lowered in the alias's own module with its own parameters in scope,
    /// whatever was being lowered when it was named, and put back exactly as
    /// found. A body asked for while it is itself being lowered — an alias
    /// naming itself through an argument's row — is the empty one: the cycle
    /// is [`expand_alias`](Self::expand_alias)'s to report.
    fn alias_body(&mut self, symbol: Symbol) -> AliasBody {
        if let Some(body) = self.alias_bodies.get(&symbol) {
            return body.clone();
        }
        let Some(pending) = self.pending_aliases.remove(&symbol) else {
            return AliasBody::default();
        };
        let module = std::mem::replace(&mut self.module, pending.module);
        let params = std::mem::take(&mut self.params);
        let vars = std::mem::take(&mut self.vars);
        self.scope_params(&pending.params);
        let body = self.alias_row(pending.row);
        self.module = module;
        self.params = params;
        self.vars = vars;
        self.alias_bodies.insert(symbol, body.clone());
        body
    }

    /// Lower the row an alias declaration wrote: each application resolved,
    /// counted and its arguments lowered, each effect named once, and a tail
    /// that names one of the declaration's parameters.
    fn alias_row(&mut self, row: parse::EffectRow) -> AliasBody {
        let mut cases: Vec<AliasCase> = Vec::new();
        for (name, label) in row.effects {
            let at = name.span();
            let Some(symbol) = self.resolve(&name, Namespace::Effects) else {
                continue;
            };
            let args = match label {
                parse::EffectLabel::Written { args, .. } | parse::EffectLabel::Absent { args } => {
                    args
                }
            };
            let expected = self.arity(symbol);
            if args.len() != expected {
                self.error(
                    at,
                    ErrorKind::EffectArity {
                        name: self.mint.name(symbol).to_string(),
                        expected,
                        found: args.len(),
                    },
                );
                continue;
            }
            if let Some(previous) = cases.iter().find(|case| case.symbol == symbol) {
                self.error(
                    at,
                    ErrorKind::DuplicateCase {
                        shape: Shape::Effect,
                        name: effect_key(self.mint, symbol),
                        previous: previous.name_span,
                    },
                );
                continue;
            }
            let args = args
                .into_iter()
                .map(|arg| self.argument(arg, Place::Declaration))
                .collect();
            cases.push(AliasCase {
                name_span: at,
                symbol,
                args,
            });
        }
        let tail = self
            .tail(row.tail, Place::Declaration, Shape::Effect)
            .unwrap_or_default();
        AliasBody {
            span: row.span,
            cases,
            tail,
        }
    }

    /// An alias declaration's value: its body, and the row it comes to over
    /// its own parameters.
    fn declared_alias(&mut self, symbol: Symbol, params: &[Param]) -> Alias {
        let args: Vec<Type> = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                param.span.track(TypeKind::Param {
                    symbol: param.symbol,
                    index: index as u32,
                })
            })
            .collect();
        let body = self.alias_body(symbol);
        let at = body.span;
        let expansion = self.expand_alias(symbol, &args, at);
        let mut effects: IndexMap<EffectId, EffectLabel> = IndexMap::new();
        for label in expansion.labels {
            effects
                .entry(EffectId::pending(label.symbol()))
                .or_insert(label);
        }
        let expanded = at.track(TypeKind::Effects(Box::new(EffectRow {
            span: at,
            written: true,
            effects,
            tail: expansion.tail,
        })));
        Alias { body, expanded }
    }

    /// What one application of an effect stands for.
    ///
    /// A concrete effect stands for itself, applied as written. An alias
    /// stands for the effects its body names with the arguments substituted
    /// for its parameters, each expanded in turn, and for the row its tail's
    /// argument writes. An alias met again while it is being expanded is a
    /// cycle: reported once, at the application that closes the ring, after
    /// which it stands for nothing — the row that named it still stands for
    /// whatever else it named.
    ///
    /// The aliases being expanded are frames on a stack of this function's
    /// own, since a chain of aliases is as long as a bundle cares to make it
    /// and must not be a chain of native frames.
    fn expand_alias(&mut self, symbol: Symbol, args: &[Type], at: Span) -> Expansion {
        struct Frame {
            body: AliasBody,
            args: Vec<Type>,
            next: usize,
            expansion: Expansion,
        }
        if !self.is_alias(symbol) {
            return Expansion {
                labels: vec![EffectLabel::Written {
                    name_span: at,
                    symbol,
                    args: args.to_vec(),
                    expanded: false,
                    when: None,
                }],
                tail: None,
            };
        }
        let mut frames: Vec<Frame> = Vec::new();
        let open = |this: &mut Self,
                    symbol: Symbol,
                    args: Vec<Type>,
                    at: Span,
                    frames: &mut Vec<Frame>| {
            if this.opened_alias(symbol, at) {
                let body = this.alias_body(symbol);
                frames.push(Frame {
                    body,
                    args,
                    next: 0,
                    expansion: Expansion::default(),
                });
            }
        };
        open(self, symbol, args.to_vec(), at, &mut frames);
        loop {
            let Some(frame) = frames.last_mut() else {
                // The root was a ring or stands for nothing.
                return Expansion::default();
            };
            if let Some(case) = frame.body.cases.get(frame.next).cloned() {
                frame.next += 1;
                let frame_args = frame.args.clone();
                let case_args: Vec<Type> = case
                    .args
                    .iter()
                    .map(|arg| self.substituted(arg, &frame_args))
                    .collect();
                if self.is_alias(case.symbol) {
                    open(self, case.symbol, case_args, case.name_span, &mut frames);
                } else {
                    frames
                        .last_mut()
                        .expect("the frame is still open")
                        .expansion
                        .labels
                        .push(EffectLabel::Written {
                            name_span: case.name_span,
                            symbol: case.symbol,
                            args: case_args,
                            expanded: true,
                            when: None,
                        });
                }
                continue;
            }
            // Every case is expanded: the tail, then what this alias stands
            // for joins the alias that named it, or is the answer.
            let mut frame = frames.pop().expect("the frame is still open");
            self.splice_tail(&mut frame.expansion, &frame.body, &frame.args);
            self.expanding.pop();
            match frames.last_mut() {
                Some(parent) => {
                    for mut label in frame.expansion.labels {
                        let (EffectLabel::Written { expanded, .. }
                        | EffectLabel::Absent { expanded, .. }) = &mut label;
                        *expanded = true;
                        parent.expansion.labels.push(label);
                    }
                    if let Some(tail) = frame.expansion.tail {
                        self.adopt_tail(&mut parent.expansion.tail, tail);
                    }
                }
                None => return frame.expansion,
            }
        }
    }

    /// Begin expanding one alias: `true` when it may be, with the alias now
    /// on the stack of those being expanded; `false` for one a ring was
    /// already reported at, or one met again on the way down — which is the
    /// ring, reported here at the application that closes it.
    fn opened_alias(&mut self, symbol: Symbol, at: Span) -> bool {
        if self.cyclic.contains(&symbol) {
            return false;
        }
        if let Some(start) = self.expanding.iter().position(|open| *open == symbol) {
            // A ring is a forwarding one when every alias on it does nothing
            // but hand its parameters to the next: a body of one application
            // whose arguments are all bare parameters, with no tail. Anything
            // else adds to the row every time round.
            let growing = !self.expanding[start..].iter().all(|open| {
                self.alias_bodies.get(open).is_some_and(|body| {
                    body.tail.is_none()
                        && matches!(body.cases.as_slice(), [case] if case
                            .args
                            .iter()
                            .all(|arg| matches!(arg.tracked, TypeKind::Param { .. })))
                })
            });
            if !self.imported_aliases.contains(&symbol) {
                let name = self.mint.name(symbol).to_string();
                self.error(at, ErrorKind::AliasCycle { name, growing });
            }
            self.cyclic.insert(symbol);
            return false;
        }
        self.expanding.push(symbol);
        true
    }

    /// Splice the row an alias's tail argument writes into its expansion:
    /// the argument's labels, which may not name what the alias already
    /// supplies, and its tail.
    fn splice_tail(&mut self, expansion: &mut Expansion, body: &AliasBody, args: &[Type]) {
        let Some(Tail {
            span,
            of: Row::Param { index, .. },
        }) = &body.tail
        else {
            return;
        };
        match args.get(*index as usize).map(|arg| &arg.tracked) {
            Some(TypeKind::Effects(row)) => {
                // The row spliced in may not name what the alias already
                // supplies: the parameter's lacks, said where the argument
                // was written. Whichever arguments the two carry, one
                // constructor stands once in a row.
                let arg_span = args[*index as usize].span;
                for label in row.effects.values() {
                    let supplied = expansion
                        .labels
                        .iter()
                        .any(|previous| previous.symbol() == label.symbol());
                    if supplied {
                        self.error(
                            arg_span,
                            ErrorKind::RepeatedRowField {
                                shape: Shape::Effect,
                                field: effect_key(self.mint, label.symbol()),
                            },
                        );
                        continue;
                    }
                    expansion.labels.push(label.clone());
                }
                if let Some(tail) = row.tail.clone() {
                    self.adopt_tail(&mut expansion.tail, tail);
                }
            }
            Some(TypeKind::Param {
                symbol: param,
                index: outer,
            }) => {
                let tail = Tail {
                    span: *span,
                    of: Row::Param {
                        symbol: *param,
                        index: *outer,
                    },
                };
                self.adopt_tail(&mut expansion.tail, tail);
            }
            Some(TypeKind::Error) | None => {}
            Some(_) => {
                let span = args[*index as usize].span;
                self.error(
                    span,
                    ErrorKind::NotARow {
                        sense: Sense::Effects,
                    },
                );
            }
        }
    }

    /// Give a row the tail an expansion brought, or refuse the second one.
    fn adopt_tail(&mut self, into: &mut Option<Tail>, tail: Tail) {
        match into {
            Some(previous) => {
                let previous = previous.span;
                self.error(tail.span, ErrorKind::TwoTails { previous });
            }
            None => *into = Some(tail),
        }
    }

    /// One alias's written type with the alias's parameters replaced by the
    /// arguments it was applied to. A parameter standing in a row's tail is
    /// spliced: the argument's own labels join the row, and its tail becomes
    /// the row's.
    fn substituted(&mut self, ty: &Type, args: &[Type]) -> Type {
        let span = ty.span;
        let tracked = match &ty.tracked {
            TypeKind::Param { index, .. } => {
                return args
                    .get(*index as usize)
                    .cloned()
                    .unwrap_or_else(|| span.track(TypeKind::Error));
            }
            TypeKind::Struct { fields, tail } => {
                let mut fields: IndexMap<String, TypeField> = fields
                    .iter()
                    .map(|(name, field)| {
                        let field = match field {
                            TypeField::Written {
                                name_span,
                                when,
                                value,
                            } => TypeField::Written {
                                name_span: *name_span,
                                when: when.clone(),
                                value: self.substituted(value, args),
                            },
                            absent => absent.clone(),
                        };
                        (name.clone(), field)
                    })
                    .collect();
                let mut tail = tail.clone();
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = &tail
                {
                    match args.get(*index as usize).map(|arg| &arg.tracked) {
                        Some(TypeKind::Struct {
                            fields: more,
                            tail: rest,
                        }) => {
                            for (name, field) in more {
                                fields.entry(name.clone()).or_insert_with(|| field.clone());
                            }
                            tail = rest.clone();
                        }
                        Some(TypeKind::Param { symbol, index }) => {
                            tail = tail.map(|tail| Tail {
                                span: tail.span,
                                of: Row::Param {
                                    symbol: *symbol,
                                    index: *index,
                                },
                            });
                        }
                        Some(TypeKind::Error) | None => tail = None,
                        Some(_) => {
                            self.error(
                                args[*index as usize].span,
                                ErrorKind::NotARow {
                                    sense: Sense::Fields,
                                },
                            );
                            tail = None;
                        }
                    }
                }
                TypeKind::Struct { fields, tail }
            }
            TypeKind::Sum { cases, tail } => {
                let mut cases: IndexMap<String, SumCase> = cases
                    .iter()
                    .map(|(name, case)| {
                        let case = match case {
                            SumCase::Written {
                                name_span,
                                when,
                                payload,
                            } => SumCase::Written {
                                name_span: *name_span,
                                when: when.clone(),
                                payload: payload
                                    .as_ref()
                                    .map(|payload| self.substituted(payload, args)),
                            },
                            absent => absent.clone(),
                        };
                        (name.clone(), case)
                    })
                    .collect();
                let mut tail = tail.clone();
                if let Some(Tail {
                    of: Row::Param { index, .. },
                    ..
                }) = &tail
                {
                    match args.get(*index as usize).map(|arg| &arg.tracked) {
                        Some(TypeKind::Sum {
                            cases: more,
                            tail: rest,
                        }) => {
                            for (name, case) in more {
                                cases.entry(name.clone()).or_insert_with(|| case.clone());
                            }
                            tail = rest.clone();
                        }
                        Some(TypeKind::Param { symbol, index }) => {
                            tail = tail.map(|tail| Tail {
                                span: tail.span,
                                of: Row::Param {
                                    symbol: *symbol,
                                    index: *index,
                                },
                            });
                        }
                        Some(TypeKind::Error) | None => tail = None,
                        Some(_) => {
                            self.error(
                                args[*index as usize].span,
                                ErrorKind::NotARow {
                                    sense: Sense::Cases,
                                },
                            );
                            tail = None;
                        }
                    }
                }
                TypeKind::Sum { cases, tail }
            }
            TypeKind::Arrow { from, to, effects } => TypeKind::Arrow {
                from: Box::new(self.substituted(from, args)),
                to: Box::new(self.substituted(to, args)),
                effects: Box::new(self.substituted_row(effects, args)),
            },
            TypeKind::Effects(effects) => {
                TypeKind::Effects(Box::new(self.substituted_row(effects, args)))
            }
            TypeKind::Array(element) => TypeKind::Array(Box::new(self.substituted(element, args))),
            TypeKind::Apply {
                head,
                head_span,
                args: applied,
            } => TypeKind::Apply {
                head: *head,
                head_span: *head_span,
                args: applied
                    .iter()
                    .map(|arg| self.substituted(arg, args))
                    .collect(),
            },
            TypeKind::Ident(_)
            | TypeKind::Prim(_)
            | TypeKind::Var(_)
            | TypeKind::Hole
            | TypeKind::Error => ty.tracked.clone(),
        };
        span.track(tracked)
    }

    /// [`substituted`](Self::substituted) for a row of effects: the arguments
    /// of each label substituted, and a parameter in the tail spliced.
    fn substituted_row(&mut self, row: &EffectRow, args: &[Type]) -> EffectRow {
        let mut effects: IndexMap<EffectId, EffectLabel> = IndexMap::new();
        for (key, label) in &row.effects {
            let mut label = label.clone();
            for arg in label.args_mut() {
                *arg = self.substituted(arg, args);
            }
            effects.insert(key.clone(), label);
        }
        let mut tail = row.tail.clone();
        if let Some(Tail {
            of: Row::Param { index, .. },
            ..
        }) = &tail
        {
            match args.get(*index as usize).map(|arg| &arg.tracked) {
                Some(TypeKind::Effects(more)) => {
                    for (key, label) in &more.effects {
                        effects.entry(key.clone()).or_insert_with(|| label.clone());
                    }
                    tail = more.tail.clone();
                }
                Some(TypeKind::Param { symbol, index }) => {
                    tail = tail.map(|tail| Tail {
                        span: tail.span,
                        of: Row::Param {
                            symbol: *symbol,
                            index: *index,
                        },
                    });
                }
                Some(TypeKind::Error) | None => tail = None,
                Some(_) => {
                    self.error(
                        args[*index as usize].span,
                        ErrorKind::NotARow {
                            sense: Sense::Effects,
                        },
                    );
                    tail = None;
                }
            }
        }
        EffectRow {
            span: row.span,
            written: row.written,
            effects,
            tail,
        }
    }

    /// Lower one operation's signature: the plain closed arrow performing it
    /// has, taken apart into what it takes and what it gives back.
    ///
    /// An operation that is not an arrow is refused — a perform site is always
    /// an application, so there would be nowhere for one to be — and the two
    /// sides fall back to the error type, which absorbs.
    fn operation(&mut self, name: &TrackedString, signature: parse::Type) -> Operation {
        // A signature is a whole written type, so it is a scope of its own for
        // the reason an annotation is — even though it has no a variable of
        // its own to put anything in it.
        self.vars.clear();
        let span = signature.span;
        let lowered = self.ty(signature, Place::Operation);
        let (from, to) = match lowered.tracked {
            TypeKind::Arrow { from, to, effects } => {
                // The outer arrow is the one an operation performs through,
                // and performing it introduces the declaring effect and
                // nothing else. An arrow nested inside either side may carry
                // effects like any other type written there.
                if effects.written {
                    self.error(
                        effects.span,
                        ErrorKind::ImpureOperation {
                            found: OperationTypeProblem::Effects,
                        },
                    );
                }
                (*from, *to)
            }
            written => {
                // Only when the signature lowered to something in the first
                // place: a type that was already refused is the error type, and
                // saying it is not an arrow would be one mistake told twice.
                if !matches!(written, TypeKind::Error) {
                    self.error(
                        span,
                        ErrorKind::NotAnOperation {
                            name: name.tracked.clone(),
                        },
                    );
                }
                (span.track(TypeKind::Error), span.track(TypeKind::Error))
            }
        };
        Operation {
            name_span: name.span,
            from,
            to,
        }
    }

    /// What a `..` tail stands for, and the one place a declaration is allowed
    /// to be open.
    ///
    /// A name is looked for among the parameters first, because that is the
    /// only reading under which a declaration's tail means anything: what it
    /// stands for is supplied at every use rather than decided here, so the
    /// body still mentions no solver variable and every walk can still stop at
    /// a name. Anything else in a declaration — a bare `..`, or a variable its
    /// header did not bind — is a question the declaration cannot leave open,
    /// and is reported here rather than by the caller, since only here is it
    /// known which of the three a tail turned out to be.
    fn row(&mut self, span: Span, of: parse::Rest, place: Place, shape: Shape) -> Option<Row> {
        let name = match of {
            parse::Rest::Anything => {
                if let Some(kind) = openness(place, shape) {
                    self.error(span, kind);
                }
                return Some(Row::Anything);
            }
            parse::Rest::Variable(name) => name,
        };
        // The parameter of the declaration this is the body of, if it is one,
        // and a declaration's only tail is one of those.
        if let Some(&(symbol, index)) = self.params.get(&name.tracked) {
            return Some(Row::Param { symbol, index });
        }
        // Otherwise it is an annotation's variable, and neither a declaration
        // nor an operation's signature has one: each says the same thing
        // wherever it is used. Reported and dropped, the way a refused `..` is —
        // the type beside it stands.
        if let Some(kind) = wherever(place, &name.tracked) {
            self.error(name.span, kind);
            return Some(Row::Anything);
        }
        // And one variable stands for one thing. A struct's `..` is the type its
        // fields sit on and a sum's is the cases it does not write out, so the
        // two are a whole type and the rest of a sum — which is what a name
        // given both would have to be at once. `None` says the row absorbs; see
        // [`ErrorKind::MixedTail`].
        match self.variable(&name, sense(shape)) {
            true => Some(Row::Named(name.tracked)),
            false => None,
        }
    }

    /// Read one variable of the annotation being lowered at `sense`, minting it
    /// if this is the first time it has been written, and say whether the
    /// reading stands.
    ///
    /// A variable is introduced by being used: `'a` is a variable wherever it
    /// is written, and the span kept is the first use's, which is where a
    /// complaint about what the rest of the annotation did with it points.
    fn variable(&mut self, name: &TrackedString, sense: Sense) -> bool {
        if !self.vars.contains_key(&name.tracked) {
            let id = self.rigids;
            self.rigids += 1;
            self.vars.insert(
                name.tracked.clone(),
                Declared {
                    span: name.span,
                    id,
                    sense: None,
                    labelled: false,
                },
            );
        }
        self.used(name, sense)
    }

    /// Read one variable already minted at `sense`, and say whether that
    /// reading stands.
    ///
    /// The first use decides what the variable is; every use after it is held
    /// to that. A use that disagrees is reported where it was written — the one
    /// that brought the two readings together — and answers `false`, which is
    /// whatever asked absorbing rather than being lowered as one of two things.
    fn used(&mut self, name: &TrackedString, sense: Sense) -> bool {
        let declared = self
            .vars
            .get_mut(&name.tracked)
            .expect("the caller found the declaration");
        match declared.sense {
            Some((first, previous)) if first != sense => {
                self.error(
                    name.span,
                    ErrorKind::MixedTail {
                        first,
                        second: sense,
                        previous,
                    },
                );
                false
            }
            Some(_) => true,
            None => {
                declared.sense = Some((sense, name.span));
                true
            }
        }
    }

    /// Lower one whole written type and the `where` clause that followed it — a
    /// declaration's body, or a definition's annotation — which is the scope a
    /// declared variable lives in, and so the scope this clears. Every other
    /// caller of [`ty`](Self::ty) is inside one of these and shares its scope,
    /// which is exactly what makes two `..'r` in one annotation stand for one
    /// rest and two `when a` for one presence.
    ///
    /// Two passes, in this order and no other. The type first, since a variable
    /// is introduced by being written in it and its sort is read off its uses.
    /// Then the constraints, because a formula is written about presences and
    /// which variables are presences is a thing only the type's `when`s can
    /// have said.
    fn written(&mut self, written: parse::Annotation, place: Place) -> Annotation {
        self.vars.clear();
        let clauses = written
            .clause
            .map_or_else(Vec::new, |clause| clause.clauses);
        let ty = self.ty(written.ty, place);
        // Then the constraints, which are resolved against what the type just
        // read: a formula is written about presences, and which variables are
        // presences is a thing only the `when`s can have said.
        let mut clause: Option<Clause> = None;
        let mut absorbed = false;
        for written in clauses {
            // A declaration says the same thing wherever it is used, so it has
            // no presence of its own for a formula to relate.
            if place == Place::Declaration {
                self.error(written.span, ErrorKind::ClauseInDeclaration);
                continue;
            }
            // Every statement is lowered before any is judged, so a clause with
            // two bad statements reports both — the precedent [`ty`](Self::ty)
            // sets for an open declared type.
            let Some(lowered) = self.clause(written) else {
                absorbed = true;
                continue;
            };
            // Several constraint statements are conjoined in written order:
            // `where a; b` says what `where a and b` says.
            clause = Some(match clause {
                None => lowered,
                Some(before) => {
                    let span = before.span.merge(lowered.span);
                    span.track(ClauseKind::And(Box::new(before), Box::new(lowered)))
                }
            });
        }
        let polarity = presence_polarities(&ty, &self.variances);
        let named_ids: HashSet<u32> = self.vars.values().map(|declared| declared.id).collect();
        let anonymous_existentials = polarity
            .iter()
            .filter(|(id, occurrences)| {
                !named_ids.contains(id) && occurrences.negative == 0 && occurrences.positive > 0
            })
            .filter_map(|(id, occurrences)| occurrences.owner().ok().map(|owner| (*id, owner)))
            .collect();
        let mut ownership_errors = Vec::new();
        let variables = self
            .vars
            .iter()
            .map(|(name, declared)| {
                let sense = declared
                    .sense
                    .expect("an annotation variable was minted by a typed use")
                    .0;
                let ownership = match (sense, polarity.get(&declared.id)) {
                    (Sense::Presence, Some(occurrences))
                        if occurrences.negative == 0 && occurrences.positive > 0 =>
                    {
                        match occurrences.owner() {
                            Ok(boundary) => PresenceOwnership::Existential { boundary },
                            Err((previous, second)) => {
                                ownership_errors.push(Error {
                                    span: second,
                                    kind: ErrorKind::IncompatiblePresenceOwnership {
                                        name: name.clone(),
                                        previous,
                                    },
                                });
                                // Lowering continues only to accumulate independent errors;
                                // this annotation is rejected and never reaches inference.
                                PresenceOwnership::Universal
                            }
                        }
                    }
                    _ => PresenceOwnership::Universal,
                };
                Variable {
                    ownership,
                    span: declared.span,
                    name: name.clone(),
                    sense,
                    id: declared.id,
                }
            })
            .collect();
        self.errors.extend(ownership_errors);
        Annotation {
            ty,
            variables,
            anonymous_existentials,
            // The clause absorbs whole rather than keeping the statements that
            // resolved, for the reason one bad name absorbs a formula: a
            // contract missing one of its conjuncts is a contract nobody wrote.
            clause: match absorbed {
                true => None,
                false => clause,
            },
        }
    }

    /// Lower one constraint statement, resolving each name against the
    /// variables the `where` clause declared and the `when`s the type wore.
    ///
    /// `None` when any name failed to resolve: the statement absorbs whole
    /// rather than keeping the half that resolved, for the reason every other
    /// row mistake absorbs — a formula missing one of its conjuncts is a
    /// contract nobody wrote, and holding the definition to it would be a
    /// second complaint about the first one.
    ///
    /// A name here is a *presence* use, since a formula is written about
    /// presences and nothing else. So a declared variable the type already read
    /// another way is [`ErrorKind::MixedTail`], and one no `when` ever wore is
    /// [`ErrorKind::UnboundPresence`] — the same complaint a name nothing
    /// declared gets, because the reader's fix is the same either way.
    fn clause(&mut self, clause: parse::Clause) -> Option<Clause> {
        let span = clause.span;
        let kind = match clause.tracked {
            parse::ClauseKind::Name(name) => {
                let named = span.track(name.clone());
                let declared = self.vars.get(&name).is_some();
                if declared && !self.used(&named, Sense::Presence) {
                    return None;
                }
                if !self.vars.get(&name).is_some_and(|var| var.labelled) {
                    self.error(span, ErrorKind::UnboundPresence { name });
                    return None;
                }
                ClauseKind::Name(name)
            }
            parse::ClauseKind::Not(inner) => ClauseKind::Not(Box::new(self.clause(*inner)?)),
            parse::ClauseKind::And(left, right) => {
                // Both sides are lowered before either is judged, so a clause
                // naming two unbound presences reports both: the precedent
                // [`ty`](Self::ty) sets for an open declared type.
                let (left, right) = (self.clause(*left), self.clause(*right));
                ClauseKind::And(Box::new(left?), Box::new(right?))
            }
            parse::ClauseKind::Or(left, right) => {
                let (left, right) = (self.clause(*left), self.clause(*right));
                ClauseKind::Or(Box::new(left?), Box::new(right?))
            }
            parse::ClauseKind::Equal(left, right) => {
                let (left, right) = (self.clause(*left), self.clause(*right));
                ClauseKind::Equal(Box::new(left?), Box::new(right?))
            }
            parse::ClauseKind::NotEqual(left, right) => {
                let (left, right) = (self.clause(*left), self.clause(*right));
                ClauseKind::NotEqual(Box::new(left?), Box::new(right?))
            }
        };
        Some(span.track(kind))
    }

    /// Lower one label's `when` clause.
    ///
    /// A named `when` is a use of a variable like any other, and introduces one
    /// where it is the first: `when 'a` says the label is there when `'a` says
    /// so. A name the type already read another way leaves the label wearing
    /// the anonymous presence instead, which is the `when _` the reader could
    /// have written and which nothing can then name.
    ///
    /// None of that in a declaration or an operation's signature, where the
    /// clause is refused for existing wherever it is written: `place` is what
    /// says so, and a second complaint about a name that could never have been
    /// declared there would be the first one said again in different words.
    fn when(&mut self, when: Option<Box<parse::When>>, place: Place) -> Option<Box<When>> {
        let when = when?;
        if place != Place::Annotation {
            let id = self.rigids;
            self.rigids += 1;
            return Some(Box::new(When {
                span: when.span,
                name: when.name.map(|name| name.tracked),
                id,
            }));
        }
        let mut id = None;
        let name = when.name.and_then(|name| {
            if !self.variable(&name, Sense::Presence) {
                return None;
            }
            // Worn by a label, which is what a formula needs of a name before
            // it can say anything about it.
            let declared = self
                .vars
                .get_mut(&name.tracked)
                .expect("the variable was just minted");
            declared.labelled = true;
            id = Some(declared.id);
            Some(name.tracked)
        });
        let id = id.unwrap_or_else(|| {
            let id = self.rigids;
            self.rigids += 1;
            id
        });
        Some(Box::new(When {
            span: when.span,
            name,
            id,
        }))
    }

    /// The `..` a row was written with, lowered, or `Err` when it named a rest
    /// the written type already gave another shape to — which is the row
    /// absorbing, the way [`ErrorKind::OpenDeclaredType`] makes one absorb.
    fn tail(
        &mut self,
        tail: Option<parse::Tail>,
        place: Place,
        shape: Shape,
    ) -> Result<Option<Tail>, ()> {
        let Some(tail) = tail else {
            return Ok(None);
        };
        let span = tail.span;
        match self.row(span, tail.of, place, shape) {
            Some(of) => Ok(Some(Tail { span, of })),
            None => Err(()),
        }
    }

    /// Whether a row written here may stand as it is: a declaration must be
    /// closed, except through a parameter, and everything else may be as open
    /// as it likes. See [`ErrorKind::OpenDeclaredType`].
    ///
    /// `marks` is where each `when` was written — every one is its own report,
    /// since every one is a label the reader can close — and `tail` is the `..`
    /// the row ended with, which has already reported for itself in
    /// [`row`](Self::row), because only there is it known which of the three a
    /// tail turned out to be. A row refused in a declaration lowers to the
    /// error type. An operation keeps its invalid row as recovery syntax after
    /// reporting it, so structural effect identity cannot mistake it for a
    /// closed row.
    ///
    /// One rule for both shapes, because it is one rule. The two arms of
    /// [`ty`](Self::ty) that call it differ in the nouns they are written about
    /// and in nothing else, and a rule about openness kept in two places is one
    /// a sum can quietly stop obeying.
    fn closed(
        &mut self,
        place: Place,
        shape: Shape,
        marks: impl IntoIterator<Item = Span>,
        tail: &Option<Tail>,
    ) -> bool {
        let Some(kind) = openness(place, shape) else {
            return true;
        };
        let marks: Vec<Span> = marks.into_iter().collect();
        let open_tail = matches!(
            tail,
            Some(Tail {
                of: Row::Anything | Row::Named(_),
                ..
            })
        );
        if marks.is_empty() && !open_tail {
            return true;
        }
        for span in marks {
            self.error(span, kind.clone());
        }
        place == Place::Operation
    }

    /// Whether a row's explicit absences have a `..` to speak about. A `\`
    /// says the tail beside it may not stand for the label, and a row with no
    /// tail already says that of every label it does not name — so each one in
    /// a closed row is reported where it was written, and the row absorbs. See
    /// [`ErrorKind::AbsentInClosed`].
    ///
    /// [`closed`](Self::closed)'s sibling, and one rule for both shapes for
    /// the same reason: the two arms of [`ty`](Self::ty) that call it differ
    /// in the nouns they are written about and in nothing else.
    fn tailed(
        &mut self,
        shape: Shape,
        absences: impl IntoIterator<Item = (String, Span)>,
        tail: &Option<Tail>,
    ) -> bool {
        if tail.is_some() {
            return true;
        }
        let mut tailed = true;
        for (label, span) in absences {
            self.error(span, ErrorKind::AbsentInClosed { shape, label });
            tailed = false;
        }
        tailed
    }

    /// How many arguments a declared type takes. A symbol with no entry is one
    /// whose declaration was refused, and counting against it would be a second
    /// complaint about the first thing that went wrong.
    fn arity(&self, symbol: Symbol) -> usize {
        self.arities.get(&symbol).copied().unwrap_or(0)
    }

    /// Lower `<head> <arg>...`.
    ///
    /// The arguments are lowered before the head is judged, so a bad name
    /// inside an application nobody could have applied is still reported — the
    /// precedent [`ty`](Self::ty) already sets for an open declared type. The
    /// head goes the same way: something that is not a name cannot be applied,
    /// but it is still a written type, and `{ x: Bogus } Nat` should tell the
    /// reader about `Bogus` rather than make fixing the application the price
    /// of hearing about it. So it is lowered for its complaints and the result
    /// dropped, since there is nothing here for a head to be part of.
    fn apply(
        &mut self,
        span: Span,
        head: parse::Type,
        args: Vec<parse::Type>,
        place: Place,
    ) -> Type {
        let found = args.len();
        let args: Vec<Type> = args
            .into_iter()
            .map(|arg| self.argument(arg, place))
            .collect();
        let head_span = head.span;
        let name = match head.tracked {
            parse::TypeKind::Ident { name } => name,
            // A declaration's parameter stands for one type outright, and so
            // does an annotation's variable, so there is nothing here to give
            // arguments to either way. A variable is read as a type all the
            // same, so the one thing gone wrong is not joined by a complaint
            // about the name itself; a parameter is already known and needs no
            // reading.
            parse::TypeKind::Variable { name } => {
                if !self.params.contains_key(&name.tracked) {
                    self.variable(&name, Sense::Type);
                }
                self.error(
                    head_span,
                    ErrorKind::ParameterApplied { name: name.tracked },
                );
                return span.track(TypeKind::Error);
            }
            // Something that is not a name cannot be applied, but it is still a
            // written type: it is lowered for its own complaints and the result
            // dropped, since there is nothing here for a head to be part of.
            written => {
                self.ty(head_span.track(written), place);
                self.error(head_span, ErrorKind::NotAConstructor);
                return span.track(TypeKind::Error);
            }
        };
        let symbol = match self.find(&name, Namespace::Types) {
            Ok(symbol) => symbol,
            // A segment named no module; the complaint is already at it.
            Err(Missing::Segment) => return span.track(TypeKind::Error),
            Err(Missing::Name) => {
                // A primitive takes nothing, so applying one is an arity
                // complaint rather than a "not a constructor": the reader wrote
                // a type that exists and gave it too much. Only a bare name can
                // be one — a primitive lives in no module, so a path can never
                // reach it.
                if name.modules.is_empty() && Prim::from_name(&name.name.tracked).is_some() {
                    self.error(
                        span,
                        ErrorKind::Arity {
                            name: name.name.tracked.clone(),
                            expected: 0,
                            found,
                        },
                    );
                } else {
                    self.error(
                        name.name.span,
                        ErrorKind::Undefined {
                            name: name.name.tracked.clone(),
                            namespace: Namespace::Types,
                        },
                    );
                }
                return span.track(TypeKind::Error);
            }
        };
        let expected = self.arity(symbol);
        if expected != found {
            // Once, at the application — the whole of it, head and arguments
            // together, because counting them is what the reader has to do and
            // a span around the name alone shows none of what was counted. Then
            // the whole thing absorbs: a wrong count makes every position after
            // the first guesswork, and pairing them up to say more would be
            // inventing what was meant. See [`TypeKind::Apply::head_span`],
            // which exists for the complaints that *are* about the name.
            self.error(
                span,
                ErrorKind::Arity {
                    name: name.name.tracked.clone(),
                    expected,
                    found,
                },
            );
            return span.track(TypeKind::Error);
        }
        span.track(TypeKind::Apply {
            head: symbol,
            head_span,
            args,
        })
    }

    /// Lower a surface expression into an IR term. Multi-argument functions are
    /// curried into nested single-argument [`TermKind::Fn`]s; the parser
    /// guarantees every function binds at least one argument, so the fold is
    /// never empty.
    fn term(&mut self, expr: Expr) -> Term {
        let span = expr.span;
        match expr.tracked {
            // `()` is the empty struct rather than a form of its own, so it is
            // erased here instead of surviving into the IR; semantic unit is
            // the explicit closed struct row with no labels.
            ExprKind::Unit => TermKind::Struct {
                fields: Default::default(),
                spread: None,
            }
            .with_span(span),
            ExprKind::Ident { name } => match self.resolve(&name, Namespace::Terms) {
                Some(symbol) => TermKind::Ident(symbol).with_span(span),
                None => TermKind::Error.with_span(span),
            },
            ExprKind::Natural(value) => TermKind::Natural(value).with_span(span),
            ExprKind::Integer(value) => TermKind::Integer(value).with_span(span),
            ExprKind::Real(value) => TermKind::Real(value).with_span(span),
            ExprKind::String(value) => TermKind::String(value).with_span(span),
            ExprKind::Boolean(value) => TermKind::Boolean(value).with_span(span),
            ExprKind::Unary { op, value } => TermKind::Unary {
                op: match op {
                    parse::UnaryOp::Neg => UnaryOp::Neg,
                    parse::UnaryOp::Not => UnaryOp::Not,
                },
                value: Box::new(self.term(*value)),
            }
            .with_span(span),
            ExprKind::Binary { op, left, right } => TermKind::Binary {
                op: match op {
                    parse::BinaryOp::Add => BinaryOp::Add,
                    parse::BinaryOp::Sub => BinaryOp::Sub,
                    parse::BinaryOp::Mul => BinaryOp::Mul,
                    parse::BinaryOp::Div => BinaryOp::Div,
                    parse::BinaryOp::And => BinaryOp::And,
                    parse::BinaryOp::Or => BinaryOp::Or,
                    parse::BinaryOp::Xor => BinaryOp::Xor,
                },
                left: Box::new(self.term(*left)),
                right: Box::new(self.term(*right)),
            }
            .with_span(span),
            ExprKind::Pipe { value, function } => {
                let func = self.term(*function);
                let arg = self.term(*value);
                TermKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                }
                .with_span(span)
            }
            ExprKind::Apply { func, arg } => {
                let func = self.term(*func);
                let arg = self.term(*arg);
                TermKind::Apply {
                    func: Box::new(func),
                    arg: Box::new(arg),
                }
                .with_span(span)
            }
            ExprKind::Function { args, body } => {
                let mark = self.terms.mark();
                let mut bound = Vec::with_capacity(args.len());
                for arg in args {
                    let span = arg.span;
                    // A name is bound over the body; a `_` gets a fresh symbol
                    // that goes into no scope, so the argument is typechecked —
                    // the arrow still has a domain — and nothing can name it.
                    // The node keeps its shape either way.
                    let symbol = match arg.tracked {
                        parse::ArgKind::Name(name) => {
                            let symbol = self.mint.local(self.module, Namespace::Terms, &name);
                            self.terms.bind(name, symbol);
                            symbol
                        }
                        parse::ArgKind::Wildcard => self.fresh("%discard", span).tracked,
                    };
                    bound.push(span.track(symbol));
                }
                // A closure can be returned by the computation and outlive the
                // `handle` it was written in, so a `raise` inside one answers
                // nothing that is certain to still be on the stack. See R17.
                let inner = match self.answering {
                    Answering::Nowhere => Answering::Nowhere,
                    Answering::Arm | Answering::UnderFn(_) => Answering::UnderFn(span),
                };
                let outer = std::mem::replace(&mut self.answering, inner);
                let body = self.term(*body);
                self.answering = outer;
                self.terms.release(mark);
                bound.into_iter().rev().fold(body, |body, arg| {
                    let span = arg.span.merge(body.span);
                    TermKind::Fn {
                        arg,
                        body: Box::new(body),
                    }
                    .with_span(span)
                })
            }
            // A block is a spelling of the nested bindings it holds, one term
            // per `let`; see [`Builder::block`].
            ExprKind::Do { stmts, result } => self.block(span, stmts.into_iter(), result),
            ExprKind::Match { scrutinee, arms } => self.match_term(span, *scrutinee, arms),
            // A conditional is surface syntax for the ordinary exhaustive
            // Boolean match. Keeping the desugaring here means inference,
            // coverage checking, pattern compilation, and LIR all use their
            // existing match paths; no conditional reaches the IR.
            ExprKind::If {
                predicate,
                consequent,
                alternative,
            } => {
                let pattern_span = predicate.span;
                let arms = vec![
                    parse::Arm {
                        pattern: pattern_span.track(parse::PatternKind::Boolean(true)),
                        body: *consequent,
                    },
                    parse::Arm {
                        pattern: pattern_span.track(parse::PatternKind::Boolean(false)),
                        body: *alternative,
                    },
                ];
                self.match_term(span, *predicate, arms)
            }
            // The fields first and the spread after them, which is the order
            // they were written in and the order they will run in; a repeat
            // among the fields is the complaint it always was, and a field
            // the spread also carries is not one — that is what a spread is
            // for, and which value wins is inference's to say.
            ExprKind::Struct { fields, spread } => {
                let fields = self.fields(fields, |b, value| b.term(value));
                let spread = spread.map(|spread| Spread {
                    span: spread.span,
                    value: Box::new(self.term(*spread.value)),
                });
                TermKind::Struct { fields, spread }.with_span(span)
            }
            ExprKind::Array(items) => TermKind::Array(
                items
                    .into_iter()
                    .map(|item| ArrayItem {
                        spread: item.spread,
                        value: self.term(item.value),
                    })
                    .collect(),
            )
            .with_span(span),
            // Tuples are positional structs at the IR boundary. Decimal keys
            // are canonical and zero-based; the element's own span stands in
            // for the generated field name, since no label was written.
            ExprKind::Tuple(elements) => {
                let fields = elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        let name_span = element.span;
                        (
                            index.to_string(),
                            Field {
                                name_span,
                                value: self.term(element),
                            },
                        )
                    })
                    .collect();
                TermKind::Struct {
                    fields,
                    spread: None,
                }
                .with_span(span)
            }
            ExprKind::Tag { name, payload } => {
                let payload = payload.map(|payload| Box::new(self.term(*payload)));
                TermKind::Tag { name, payload }.with_span(span)
            }
            ExprKind::Project { base, field } => {
                let base = self.term(*base);
                TermKind::Project {
                    base: Box::new(base),
                    field,
                }
                .with_span(span)
            }
            // An operation is an ordinary value: what it resolves to is the
            // effect and the label, and the type it gets is the declared
            // signature with the effect's own label on its outermost arrow.
            ExprKind::Operation { effect, selector } => {
                let selector = selector.span.track(match selector.tracked {
                    parse::OperationSelector::Unnamed => OperationSelector::Unnamed,
                    parse::OperationSelector::Named(name) => OperationSelector::Named(name),
                });
                match self.operation_of(&effect, &selector) {
                    Some(symbol) => TermKind::Operation {
                        effect: effect.span().track(symbol),
                        selector,
                    }
                    .with_span(span),
                    None => TermKind::Error.with_span(span),
                }
            }
            ExprKind::Handle { body, arms } => self.handle_term(span, *body, arms),
            // The check R17 is: a `raise` answers the innermost arm around it,
            // and a `fn` between the two is a closure that could outlive the
            // handler. The value is lowered either way, so its own mistakes
            // are still its own complaints.
            ExprKind::Raise(value) => {
                let kind = match self.answering {
                    Answering::Arm => None,
                    Answering::UnderFn(function) => Some(ErrorKind::RaiseInFunction { function }),
                    Answering::Nowhere => Some(ErrorKind::RaiseOutsideArm),
                };
                if let Some(kind) = kind {
                    self.error(span, kind);
                }
                TermKind::Raise(Box::new(self.term(*value))).with_span(span)
            }
        }
    }

    /// Which effect declares the operation `!Eff.op` names, or `None` when
    /// nothing does.
    ///
    /// Three ways to fail and a complaint apiece: the head names no effect at
    /// all, the head is an alias — which declares no operations, so there is
    /// nothing there to perform — or the effect declares no such operation.
    fn operation_of(
        &mut self,
        effect: &parse::Path,
        selector: &Tracked<OperationSelector>,
    ) -> Option<Symbol> {
        let symbol = self.resolve(effect, Namespace::Effects)?;
        match self.operations.get(&symbol) {
            Some(operations) if operations.contains(&selector.tracked) => Some(symbol),
            Some(operations) => {
                let effect_name = self.mint.name(symbol).to_string();
                let kind = match &selector.tracked {
                    OperationSelector::Unnamed => ErrorKind::BareOperationUnavailable {
                        effect: effect_name,
                        suggestion: operations.iter().next().map(OperationSelector::source_name),
                    },
                    OperationSelector::Named(op)
                        if operations.contains(&OperationSelector::Unnamed) =>
                    {
                        ErrorKind::NamedOperationOnUnnamed {
                            effect: effect_name,
                            op: op.clone(),
                        }
                    }
                    OperationSelector::Named(op) => ErrorKind::UnknownOperation {
                        effect: effect_name,
                        op: op.clone(),
                    },
                };
                self.error(selector.span, kind);
                None
            }
            None => {
                self.error(
                    effect.span(),
                    ErrorKind::OperationOnAlias {
                        effect: self.mint.name(symbol).to_string(),
                    },
                );
                None
            }
        }
    }

    /// Lower `handle <expr> with <arms> end`.
    ///
    /// The handled expression first, then the arms, each with its binder in
    /// scope for its own body and released after it — the shape a `match`
    /// keeps. R15's coverage is decided by this phase rather than inference,
    /// but only after structural effect identities have been computed: arms
    /// using equivalent declarations can then contribute to one interface.
    /// The resulting discharges are still known before constraints are
    /// generated, and are what R16 hands the body as a larger ambient.
    fn handle_term(&mut self, span: Span, body: Expr, arms: Vec<parse::HandlerArm>) -> Term {
        let body = self.term(body);
        // An arm's body runs where the `handle` was written rather than inside
        // the computation, so a `raise` in one answers the handler around
        // *that* — which is this one.
        let outer = std::mem::replace(&mut self.answering, Answering::Arm);
        let mut lowered: Vec<HandlerArm> = Vec::new();
        let mut ret: Option<ReturnArm> = None;
        for arm in arms {
            match arm.head {
                parse::ArmHead::Return { span: at } => {
                    if let Some(first) = &ret {
                        self.error(
                            at,
                            ErrorKind::DuplicateReturn {
                                previous: first.span,
                            },
                        );
                    }
                    let (binder, body) = self.arm_body(arm.binder, arm.body);
                    // The first `return` arm is the one that stands, the way a
                    // repeated definition is.
                    if ret.is_none() {
                        ret = Some(ReturnArm {
                            span: at,
                            binder,
                            body: Box::new(body),
                        });
                    }
                }
                parse::ArmHead::Operation { effect, selector } => {
                    let selector = selector.span.track(match selector.tracked {
                        parse::OperationSelector::Unnamed => OperationSelector::Unnamed,
                        parse::OperationSelector::Named(name) => OperationSelector::Named(name),
                    });
                    let symbol = self.operation_of(&effect, &selector);
                    let (binder, body) = self.arm_body(arm.binder, arm.body);
                    let Some(symbol) = symbol else {
                        continue;
                    };
                    lowered.push(HandlerArm {
                        effect: effect.span().track(symbol),
                        selector,
                        binder,
                        body,
                    });
                }
            }
        }
        self.answering = outer;
        // Coverage and duplicate selection are finalized after structural
        // effect identities have been computed. Until then, source symbols do
        // not tell us which arms belong to one semantic interface.
        TermKind::Handle {
            body: Box::new(body),
            handler: Handler {
                arms: lowered,
                ret,
                discharges: Vec::new(),
            },
        }
        .with_span(span)
    }

    /// One handler arm's binder and body: the name bound for the length of the
    /// body and released after it, the way a `fn`'s argument is.
    fn arm_body(&mut self, binder: parse::Arg, body: Expr) -> (Tracked<Symbol>, Term) {
        let mark = self.terms.mark();
        let at = binder.span;
        let symbol = match binder.tracked {
            parse::ArgKind::Name(name) => {
                let symbol = self.mint.local(self.module, Namespace::Terms, &name);
                self.terms.bind(name, symbol);
                symbol
            }
            // A `_` gets a symbol nothing can name, so the payload is still
            // typed and the body still cannot reach it — a `fn` header's rule.
            parse::ArgKind::Wildcard => self.fresh("%discard", at).tracked,
        };
        let body = self.term(body);
        self.terms.release(mark);
        (at.track(symbol), body)
    }

    /// Lower a surface type into an IR type, mirroring [`term`](Self::term).
    ///
    /// The scope this resolves against is pushed by the caller rather than
    /// here, because a declaration's parameters are bound for the whole of its
    /// body and this is called once per node in it. So every name reaching
    /// here is a parameter of the declaration being lowered, a top-level
    /// declaration, or a primitive — looked for in that order, since a
    /// parameter is meant to hide a declaration of the same name.
    ///
    /// A tail's name may or may not be a binder, and which it is decides what
    /// the tail means: naming a row parameter it is that parameter, and
    /// anything else is scoped to its annotation and resolved by inference, so
    /// it passes through here as the string it was written as. See
    /// [`row`](Self::row).
    /// [`ty`](Self::ty) at the one position a row of effects may be written
    /// without an arrow to carry it: an argument.
    ///
    /// Whether the parameter it lands at is used as effects is asked later, by
    /// [`row_arguments`], which asks it of a sum's row too. What is decided
    /// here is only that a row was allowed to be written at all.
    fn argument(&mut self, ty: parse::Type, place: Place) -> Type {
        let span = ty.span;
        match ty.tracked {
            parse::TypeKind::Effects(written) => {
                match self.effect_row(span, Some(*written), place) {
                    Some(row) => span.track(TypeKind::Effects(Box::new(row))),
                    None => span.track(TypeKind::Error),
                }
            }
            written => self.ty(span.track(written), place),
        }
    }

    fn ty(&mut self, ty: parse::Type, place: Place) -> Type {
        let span = ty.span;
        match ty.tracked {
            // A row where a type goes. Lowered all the same and the result
            // dropped, the way a head that cannot be applied is: the effects it
            // names are still names, and a reader who wrote one wrong should be
            // told about that too.
            parse::TypeKind::Effects(written) => {
                self.effect_row(span, Some(*written), place);
                self.error(span, ErrorKind::EffectsOutsideRow);
                span.track(TypeKind::Error)
            }
            // As in [`term`](Self::term): the two surface spellings of the
            // empty struct, `()` and `{}`, meet here as a closed empty row.
            parse::TypeKind::Unit => span.track(TypeKind::Struct {
                fields: Default::default(),
                tail: None,
            }),
            // A hole is a position left for inference to decide — except in a
            // declaration or an operation's signature, which say the same
            // thing wherever they are used and so have nothing to leave open.
            // Refused there and absorbed, the way an open row written there is.
            parse::TypeKind::Hole => match place {
                Place::Declaration => {
                    self.error(span, ErrorKind::HoleInDeclaration);
                    span.track(TypeKind::Error)
                }
                Place::Operation => {
                    self.error(span, ErrorKind::HoleInOperation);
                    span.track(TypeKind::Error)
                }
                Place::Annotation => span.track(TypeKind::Hole),
            },
            // Two things a `'a` can be, in this order: the parameter of the
            // declaration this is the body of, or a variable of the annotation
            // it is written in. A declaration binds its parameters in its
            // header and an annotation binds nothing, so the two never both
            // apply — which is what lets one sigil serve both.
            parse::TypeKind::Variable { name } => {
                // A parameter stands for one type outright, so the name alone
                // is the whole of writing one and there is nothing to count.
                if let Some(&(symbol, index)) = self.params.get(&name.tracked) {
                    return span.track(TypeKind::Param { symbol, index });
                }
                // A declaration and an operation's signature each say the same
                // thing wherever they are used, so neither has anything for a
                // caller to pick — and each says so in its own words.
                if let Some(kind) = wherever(place, &name.tracked) {
                    self.error(name.span, kind);
                    return span.track(TypeKind::Error);
                }
                match self.variable(&name, Sense::Type) {
                    true => span.track(TypeKind::Var(name.tracked)),
                    false => span.track(TypeKind::Error),
                }
            }
            // Two things a bare name can be, in this order: a declared type or
            // a primitive.
            //
            // A declaration is looked for first, so a `type Nat` of one's own
            // shadows the built-in rather than colliding with a declaration
            // nobody wrote. Types being hoisted, every term sees such a
            // declaration wherever it was written; a type sees only the ones
            // above it, and reaches the built-in otherwise.
            parse::TypeKind::Ident { name } => match self.find(&name, Namespace::Types) {
                // A declaration written bare is applied to nothing, which is
                // only enough if it takes nothing. See [`ErrorKind::Arity`].
                Ok(symbol) => match self.arity(symbol) {
                    0 => span.track(TypeKind::Ident(symbol)),
                    expected => {
                        self.error(
                            name.span(),
                            ErrorKind::Arity {
                                name: name.name.tracked.clone(),
                                expected,
                                found: 0,
                            },
                        );
                        span.track(TypeKind::Error)
                    }
                },
                // A segment named no module; the complaint is already at it.
                Err(Missing::Segment) => span.track(TypeKind::Error),
                // A primitive lives in no module, so only a bare name can
                // reach one.
                Err(Missing::Name) => {
                    let prim = name
                        .modules
                        .is_empty()
                        .then(|| Prim::from_name(&name.name.tracked))
                        .flatten();
                    match prim {
                        Some(prim) => span.track(TypeKind::Prim(prim)),
                        None => {
                            self.error(
                                name.name.span,
                                ErrorKind::Undefined {
                                    name: name.name.tracked.clone(),
                                    namespace: Namespace::Types,
                                },
                            );
                            span.track(TypeKind::Error)
                        }
                    }
                }
            },
            parse::TypeKind::Apply { head, args } => self.apply(span, *head, args, place),
            parse::TypeKind::Array(element) => {
                let element = self.ty(*element, place);
                span.track(TypeKind::Array(Box::new(element)))
            }
            // A tuple type is a closed struct with unconditional, zero-based
            // decimal fields. As with tuple terms, an element's span is the
            // best source location for its generated label.
            parse::TypeKind::Tuple(elements) => {
                let fields = elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        let name_span = element.span;
                        let value = self.ty(element, place);
                        (
                            index.to_string(),
                            TypeField::Written {
                                name_span,
                                when: None,
                                value,
                            },
                        )
                    })
                    .collect();
                span.track(TypeKind::Struct { fields, tail: None })
            }
            parse::TypeKind::Struct { fields, tail } => {
                // The values are lowered before openness is judged, so a bad
                // name inside an open declared type is still reported: the
                // reader should not have to fix the `..` to be told about it.
                //
                // A type's field has a `when` and a `\` to it that [`Field`] has
                // no room for, so the marks ride down with the value and are
                // taken apart again here. Re-keying and repeats are the same
                // question they are for a struct literal, and are asked in the
                // one place that answers it.
                let lowered: IndexMap<String, TypeField> = self
                    .fields(fields, |b, field| match field {
                        parse::TypeField::Written { when, value } => {
                            // The clause before the value, so that a presence
                            // name is bound in the order the reader wrote it.
                            let when = b.when(when, place);
                            Some((when, b.ty(value, place)))
                        }
                        parse::TypeField::Absent => None,
                    })
                    .into_iter()
                    .map(|(name, field)| {
                        let lowered = match field.value {
                            Some((when, value)) => TypeField::Written {
                                name_span: field.name_span,
                                when,
                                value,
                            },
                            None => TypeField::Absent {
                                name_span: field.name_span,
                            },
                        };
                        (name, lowered)
                    })
                    .collect();
                let tail = match self.tail(tail, place, Shape::Struct) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                // Where a declaration is held to being closed, and where a `\`
                // is held to having a `..` to speak about; see
                // [`closed`](Self::closed) and [`tailed`](Self::tailed), the
                // same two checks the sum arm below makes. Both run, so a row
                // wrong both ways is told about both.
                let marks = lowered.values().filter_map(|field| match field {
                    TypeField::Written {
                        when: Some(when), ..
                    } => Some(when.span),
                    _ => None,
                });
                let closed = self.closed(place, Shape::Struct, marks, &tail);
                let absences = lowered.iter().filter_map(|(name, field)| match field {
                    TypeField::Absent { name_span } => Some((name.clone(), *name_span)),
                    TypeField::Written { .. } => None,
                });
                let tailed = self.tailed(Shape::Struct, absences, &tail);
                if !closed || !tailed {
                    return span.track(TypeKind::Error);
                }
                span.track(TypeKind::Struct {
                    fields: lowered,
                    tail,
                })
            }
            // The struct arm again, about cases. Everything it says holds
            // here — the payloads are lowered before openness is judged,
            // re-keying is the same question, and openness is literally the
            // same check — so what differs is the two nouns and the payload a
            // case may not have.
            parse::TypeKind::Sum { cases, tail } => {
                let lowered: IndexMap<String, SumCase> = self
                    .labels(
                        cases,
                        |name, previous| ErrorKind::DuplicateCase {
                            shape: Shape::Sum,
                            name,
                            previous,
                        },
                        |b, case| match case {
                            parse::SumCase::Written { when, payload } => {
                                let when = b.when(when, place);
                                Some((when, payload.map(|payload| b.ty(payload, place))))
                            }
                            parse::SumCase::Absent => None,
                        },
                    )
                    .into_iter()
                    .map(|(name, case)| {
                        let lowered = match case.value {
                            Some((when, payload)) => SumCase::Written {
                                name_span: case.name_span,
                                when,
                                payload,
                            },
                            None => SumCase::Absent {
                                name_span: case.name_span,
                            },
                        };
                        (name, lowered)
                    })
                    .collect();
                let tail = match self.tail(tail, place, Shape::Sum) {
                    Ok(tail) => tail,
                    Err(()) => return span.track(TypeKind::Error),
                };
                let marks = lowered.values().filter_map(|case| match case {
                    SumCase::Written {
                        when: Some(when), ..
                    } => Some(when.span),
                    _ => None,
                });
                let closed = self.closed(place, Shape::Sum, marks, &tail);
                let absences = lowered.iter().filter_map(|(name, case)| match case {
                    SumCase::Absent { name_span } => Some((name.clone(), *name_span)),
                    SumCase::Written { .. } => None,
                });
                let tailed = self.tailed(Shape::Sum, absences, &tail);
                if !closed || !tailed {
                    return span.track(TypeKind::Error);
                }
                span.track(TypeKind::Sum {
                    cases: lowered,
                    tail,
                })
            }
            parse::TypeKind::Arrow { from, to, effects } => {
                let from = self.ty(*from, place);
                let to = self.ty(*to, place);
                let Some(effects) = self.effect_row(span, effects.map(|row| *row), place) else {
                    return span.track(TypeKind::Error);
                };
                span.track(TypeKind::Arrow {
                    from: Box::new(from),
                    to: Box::new(to),
                    effects: Box::new(effects),
                })
            }
        }
    }

    /// Lower the `+` clause on one arrow: resolve every effect it names,
    /// expand every alias among them, and hold the row to being closed where
    /// the position demands it.
    ///
    /// `None` says the row absorbs and the arrow with it — the same answer a
    /// refused struct or sum gives, and for the same reason: what the arrow
    /// would stand for is exactly what could not be worked out.
    ///
    /// A bare `A -> B` writes no clause and is the empty closed row, which is
    /// what pure means. An operation's signature may write none at all: the `+`
    /// is refused there, once, and the row falls back to the pure one so the
    /// arrow itself still stands.
    fn effect_row(
        &mut self,
        span: Span,
        written: Option<parse::EffectRow>,
        place: Place,
    ) -> Option<EffectRow> {
        let Some(written) = written else {
            return Some(EffectRow {
                span,
                ..EffectRow::default()
            });
        };
        // Every label, expanded: a name declaring operations stands for itself,
        // and an alias for the effects it reaches. So no alias survives into
        // what a definition is checked against, and a printed type shows the
        // effects rather than the name.
        let mut effects: IndexMap<EffectId, EffectLabel> = IndexMap::new();
        // Which written label each effect came from, so an alias standing for
        // one effect twice — refused at its declaration — is not refused
        // again here, while two written labels reaching one effect are.
        let mut origins: HashMap<Symbol, usize> = HashMap::new();
        // The tail an alias application brought with it, if one did.
        let mut brought: Option<Tail> = None;
        for (origin, (name, label)) in written.effects.into_iter().enumerate() {
            let at = name.span();
            let Some(symbol) = self.resolve(&name, Namespace::Effects) else {
                continue;
            };
            // The clause is lowered once, before the expansion, so that a name
            // a `where` beside it can use is bound exactly once however many
            // effects an alias stands for.
            let absent = matches!(label, parse::EffectLabel::Absent { .. });
            let (written_args, when) = match label {
                parse::EffectLabel::Written { args, when } => (args, self.when(when, place)),
                parse::EffectLabel::Absent { args } => (args, None),
            };
            // Counted at the label, the whole of it, as a type application
            // is: a wrong count makes every position guesswork, so the label
            // is dropped rather than paired up.
            let expected = self.arity(symbol);
            if written_args.len() != expected {
                self.error(
                    at,
                    ErrorKind::EffectArity {
                        name: self.mint.name(symbol).to_string(),
                        expected,
                        found: written_args.len(),
                    },
                );
                continue;
            }
            let args: Vec<Type> = written_args
                .into_iter()
                .map(|arg| self.argument(arg, place))
                .collect();
            let expansion = self.expand_alias(symbol, &args, at);
            // A `\` or a `when` on an alias distributes to every effect it
            // stands for, which is only a row when the expansion is closed:
            // an open tail is nobody's to mark.
            if (absent || when.is_some()) && expansion.tail.is_some() {
                self.error(
                    at,
                    ErrorKind::ModifiedOpenAlias {
                        name: self.mint.name(symbol).to_string(),
                    },
                );
                continue;
            }
            if let Some(tail) = expansion.tail {
                self.adopt_tail(&mut brought, tail);
            }
            for label in expansion.labels {
                // Whether the reader wrote *this* effect's name here, or an
                // alias standing for it among others. A label's own mark
                // stands; the application's mark goes on the rest.
                let label_symbol = label.symbol();
                let expanded = label_symbol != symbol;
                let lowered = match (absent, label) {
                    (true, label) => EffectLabel::Absent {
                        name_span: at,
                        symbol: label_symbol,
                        args: label.args().to_vec(),
                        expanded,
                    },
                    (false, EffectLabel::Absent { args, .. }) => EffectLabel::Absent {
                        name_span: at,
                        symbol: label_symbol,
                        args,
                        expanded,
                    },
                    (
                        false,
                        EffectLabel::Written {
                            args, when: inner, ..
                        },
                    ) => EffectLabel::Written {
                        name_span: at,
                        symbol: label_symbol,
                        args,
                        expanded,
                        when: inner.or_else(|| when.clone()),
                    },
                };
                let label_key = EffectId::pending(label_symbol);
                if let Some(previous) = effects.get(&label_key) {
                    // An alias standing for one effect twice was refused at
                    // its declaration; here it stands for the effect once.
                    if expanded && origins.get(&label_symbol) == Some(&origin) {
                        continue;
                    }
                    self.error(
                        at,
                        ErrorKind::DuplicateCase {
                            shape: Shape::Effect,
                            name: effect_key(self.mint, label_symbol),
                            previous: previous.name_span(),
                        },
                    );
                    continue;
                }
                origins.insert(label_symbol, origin);
                effects.insert(label_key, lowered);
            }
        }
        let tail = match self.tail(written.tail, place, Shape::Effect) {
            Ok(tail) => tail,
            Err(()) => return None,
        };
        let tail = match (tail, brought) {
            (Some(written), Some(brought)) => {
                self.error(
                    brought.span,
                    ErrorKind::TwoTails {
                        previous: written.span,
                    },
                );
                Some(written)
            }
            (written, brought) => written.or(brought),
        };
        // The same two checks a struct and a sum make, in the effect reading:
        // a position that holds for every definition may leave nothing open,
        // and a `\` needs a `..` beside it to speak about.
        let marks = effects
            .values()
            .filter_map(|label| Some(label.when()?.span));
        let closed = self.closed(place, Shape::Effect, marks, &tail);
        let absences: Vec<_> = effects
            .values()
            .filter_map(|label| match label {
                EffectLabel::Absent {
                    name_span, symbol, ..
                } => Some((self.mint.name(*symbol).to_string(), *name_span)),
                EffectLabel::Written { .. } => None,
            })
            .collect();
        let tailed = self.tailed(Shape::Effect, absences, &tail);
        match closed && tailed {
            true => Some(EffectRow {
                span: written.span,
                written: true,
                effects,
                tail,
            }),
            false => None,
        }
    }

    /// [`labels`](Self::labels) for a struct's fields, which is every caller
    /// but one.
    fn fields<S, T>(
        &mut self,
        fields: IndexMap<TrackedString, S>,
        lower: impl Fn(&mut Self, S) -> T,
    ) -> IndexMap<String, Field<T>> {
        self.labels(
            fields,
            |name, previous| ErrorKind::DuplicateField { name, previous },
            lower,
        )
    }

    /// Re-key surface labels by name, lowering each value with `lower`.
    /// The surface syntax tolerates a name appearing twice; the IR does not, so
    /// a repeat is reported at the offending name — as `repeat`, since a field
    /// and a case are two complaints — and the first occurrence is the one that
    /// survives.
    fn labels<S, T>(
        &mut self,
        labels: IndexMap<TrackedString, S>,
        repeat: impl Fn(String, Span) -> ErrorKind,
        lower: impl Fn(&mut Self, S) -> T,
    ) -> IndexMap<String, Field<T>> {
        let mut lowered: IndexMap<String, Field<T>> = IndexMap::new();
        for (name, value) in labels {
            let name_span = name.span;
            let value = lower(self, value);
            if let Some(previous) = lowered.get(&name.tracked) {
                self.error(name_span, repeat(name.tracked.clone(), previous.name_span));
                continue;
            }
            lowered.insert(name.tracked, Field { name_span, value });
        }
        lowered
    }

    /// `do <stmt>* [return <expr>] end`, lowered to the nested `Let` terms it
    /// is a spelling of: the first statement's binding holds the rest of the
    /// block as its body, and the innermost body is the `return`'s value or,
    /// when there is none, a unit literal spanning the block. Each term keeps
    /// the span of its own statement, so a complaint about a binding points
    /// at the line that wrote it.
    ///
    /// Written recursively because scope runs outward-in while the tree is
    /// built inside-out: a statement's name has to be bound before the rest
    /// of the block is lowered, and released after.
    fn block(
        &mut self,
        span: Span,
        mut stmts: std::vec::IntoIter<Stmt>,
        result: Option<Box<Expr>>,
    ) -> Term {
        let Some(stmt) = stmts.next() else {
            return match result {
                Some(result) => self.term(*result),
                None => TermKind::Struct {
                    fields: Default::default(),
                    spread: None,
                }
                .with_span(span),
            };
        };
        let StmtKind::Let { pattern, ty, body } = stmt.tracked else {
            // The parser refuses every other kind at its keyword and drops
            // it. One that got through binds nothing, so the block goes on
            // without it.
            return self.block(span, stmts, result);
        };
        self.binding(stmt.span, *pattern, ty, body.tracked, |b| {
            b.block(span, stmts, result)
        })
    }

    /// One binding of a block: the name or pattern, what it is bound to, and
    /// the term it is in scope for, lowered by `body` once the name is bound.
    ///
    /// A bare name is bound before the value is lowered, so a binding may
    /// name itself the way a definition may; and released after the body, so
    /// nothing written past the block can see it. Bound rather than declared,
    /// which is what makes it shadow silently: two definitions of one name
    /// are a repeat, and a scope inside one is not.
    ///
    /// A pattern is the other way round: the value is lowered first — the
    /// temporary is bound before any of the pattern's names, so none of them
    /// is in scope in it — and then the binding desugars to that temporary
    /// and a projection per name. A pattern that could fail is refused; see
    /// [`ErrorKind::RefutableBinding`].
    fn binding(
        &mut self,
        span: Span,
        pattern: parse::Pattern,
        ty: Option<parse::Annotation>,
        value: Expr,
        body: impl FnOnce(&mut Self) -> Term,
    ) -> Term {
        match pattern.tracked {
            parse::PatternKind::Ident { name } => {
                let annotation = ty.map(|ty| Box::new(self.written(ty, Place::Annotation)));
                let mark = self.terms.mark();
                let symbol = self
                    .mint
                    .local(self.module, Namespace::Terms, &name.tracked);
                self.terms.bind(name.tracked, symbol);
                let value = self.term(value);
                let body = body(self);
                self.terms.release(mark);
                TermKind::Let {
                    name: name.span.track(symbol),
                    annotation,
                    value: Box::new(value),
                    body: Box::new(body),
                }
                .with_span(span)
            }
            tracked => {
                let pspan = pattern.span;
                let pattern = pspan.track(tracked);
                let annotation = ty.map(|ty| Box::new(self.written(ty, Place::Annotation)));
                let value = self.term(value);
                let mark = self.terms.mark();
                let mut seen = Vec::new();
                let mut dropped = Vec::new();
                let pattern = self.pattern(pattern, &mut seen, &mut Binders::Local, &mut dropped);
                let body = body(self);
                self.terms.release(mark);
                let body = bound_to_errors(dropped, body);
                match calm(&pattern) {
                    Some(calm) => {
                        let mut term = self.destructure(calm, value, annotation, body);
                        term.span = span;
                        term
                    }
                    // The binding has to accept every value, and this
                    // pattern would not. Every name it would have bound is
                    // still bound — to error values, which absorb — and
                    // the value keeps its place, so its own mistakes are
                    // still its own complaints.
                    None => {
                        let (at, found) = refuter(&pattern)
                            .expect("a pattern that is not calm names what refutes it");
                        self.error(at, ErrorKind::RefutableBinding { found });
                        let mut names = Vec::new();
                        pattern_binders(&pattern, &mut names);
                        let inner = bound_to_errors(names, body);
                        let held = self.fresh("%value", pspan);
                        TermKind::Let {
                            name: held,
                            annotation,
                            value: Box::new(value),
                            body: Box::new(inner),
                        }
                        .with_span(span)
                    }
                }
            }
        }
    }

    /// A fresh symbol no source name can reach: minted like a local, never
    /// bound into any scope, so nothing written can name or capture it. The
    /// name starts with `%`, which no identifier can, so the debugger shows it
    /// recognizably as the compiler's own.
    fn fresh(&mut self, name: &str, span: Span) -> Tracked<Symbol> {
        span.track(self.mint.local(self.module, Namespace::Terms, name))
    }

    /// One name a pattern binds. `seen` is every name the whole pattern has
    /// bound so far — one pattern binds a name once, however deep the nesting,
    /// and the repeat is reported here, pointing at itself. See
    /// [`Binders`] for where the symbol comes from.
    fn bound(
        &mut self,
        name: TrackedString,
        seen: &mut Vec<(String, Span)>,
        binders: &mut Binders,
    ) -> Tracked<Symbol> {
        let previous = seen
            .iter()
            .find(|(seen, _)| *seen == name.tracked)
            .map(|(_, span)| *span);
        let repeat = previous.is_some();
        if let Some(previous) = previous {
            self.error(
                name.span,
                ErrorKind::DuplicateBinding {
                    name: name.tracked.clone(),
                    previous,
                },
            );
        } else {
            seen.push((name.tracked.clone(), name.span));
        }
        match binders {
            Binders::Local => {
                let symbol = self
                    .mint
                    .local(self.module, Namespace::Terms, &name.tracked);
                // The repeat binds nothing: the first binding is the one the
                // body sees, the way a repeated definition stands.
                if !repeat {
                    self.terms.bind(name.tracked.clone(), symbol);
                }
                name.span.track(symbol)
            }
            Binders::Declared(declared) => {
                let declared = declared
                    .next()
                    .expect("the declare pass walked this same pattern");
                let symbol = match declared {
                    Some(symbol) => symbol,
                    // A name that bound nothing — a repeat, within the pattern
                    // or of an earlier definition — still gets a stand-in, so
                    // the walk stays total.
                    None => self
                        .mint
                        .local(self.module, Namespace::Terms, &name.tracked),
                };
                name.span.track(symbol)
            }
        }
    }

    /// Lower one surface pattern into the normalized [`Pattern`]: resolve its
    /// binders, expand punning, discard grouping — the parser already did —
    /// and keep everything else as written. The duplicate-binding and
    /// duplicate-field complaints are made here, at the repeats.
    ///
    /// `dropped` collects the binders of a duplicate field's sub-pattern: the
    /// field the normalized map keeps is the first, so the repeat's names have
    /// no position left to be bound at, and the caller binds them to error
    /// values instead — the way a refused binding's names are bound — so
    /// downstream uses resolve.
    fn pattern(
        &mut self,
        pattern: parse::Pattern,
        seen: &mut Vec<(String, Span)>,
        binders: &mut Binders,
        dropped: &mut Vec<Tracked<Symbol>>,
    ) -> Pattern {
        let span = pattern.span;
        match pattern.tracked {
            parse::PatternKind::Ident { name } => {
                span.track(PatternKind::Bind(self.bound(name, seen, binders)))
            }
            // Nothing to resolve and nothing to repeat: a wildcard never goes
            // through [`bound`](Self::bound), which is the whole of how it
            // stays out of the duplicate-binder check.
            parse::PatternKind::Wildcard => span.track(PatternKind::Wildcard),
            parse::PatternKind::Natural(value) => span.track(PatternKind::Natural(value)),
            parse::PatternKind::Integer(value) => span.track(PatternKind::Integer(value)),
            parse::PatternKind::Real(value) => span.track(PatternKind::Real(value)),
            parse::PatternKind::String(value) => span.track(PatternKind::String(value)),
            parse::PatternKind::Boolean(value) => span.track(PatternKind::Boolean(value)),
            parse::PatternKind::Unit => span.track(PatternKind::Unit),
            // A bare tag keeps its `None`: what it constrains the payload to —
            // unit — is said where the type is built rather than written into
            // a tree node the reader never wrote, the convention
            // [`TermKind::Tag`] keeps.
            parse::PatternKind::Tag { name, payload } => {
                let payload =
                    payload.map(|payload| Box::new(self.pattern(*payload, seen, binders, dropped)));
                span.track(PatternKind::Tag { name, payload })
            }
            // Tuple patterns are exact positional struct patterns. Lowering
            // each element through this same walk preserves binder order and
            // duplicate-binding diagnostics across arbitrary nesting.
            parse::PatternKind::Tuple(elements) => {
                let fields = elements
                    .into_iter()
                    .enumerate()
                    .map(|(index, element)| {
                        let name_span = element.span;
                        let value = self.pattern(element, seen, binders, dropped);
                        (index.to_string(), Field { name_span, value })
                    })
                    .collect();
                span.track(PatternKind::Struct { fields, rest: None })
            }
            parse::PatternKind::Struct {
                fields: entries,
                rest,
            } => {
                let mut named: Vec<(String, bool, Span)> = Vec::new();
                let mut fields: IndexMap<String, Field<Pattern>> = IndexMap::new();
                for (name, sub) in entries {
                    let pun = sub.is_none();
                    let keep = match named.iter().find(|(seen, _, _)| *seen == name.tracked) {
                        // Two puns of one name are `{x, x}`: the same name
                        // bound twice, which the binder walk below words
                        // better than a complaint about the field would.
                        Some((_, earlier, previous)) if !(pun && *earlier) => {
                            self.error(
                                name.span,
                                ErrorKind::DuplicateField {
                                    name: name.tracked.clone(),
                                    previous: *previous,
                                },
                            );
                            false
                        }
                        Some(_) => false,
                        None => {
                            named.push((name.tracked.clone(), pun, name.span));
                            true
                        }
                    };
                    // The repeat's sub-pattern is lowered all the same — its
                    // own mistakes are still its own complaints, and the
                    // declare pass walked the same names — and then dropped:
                    // the first field is the one that stands.
                    let sub = match sub {
                        Some(sub) => self.pattern(sub, seen, binders, dropped),
                        None => {
                            let bound = self.bound(name.clone(), seen, binders);
                            name.span.track(PatternKind::Bind(bound))
                        }
                    };
                    match keep {
                        true => {
                            fields.insert(
                                name.tracked,
                                Field {
                                    name_span: name.span,
                                    value: sub,
                                },
                            );
                        }
                        false => pattern_binders(&sub, dropped),
                    }
                }
                span.track(PatternKind::Struct { fields, rest })
            }
            parse::PatternKind::Array {
                before,
                rest,
                after,
            } => {
                let before = before
                    .into_iter()
                    .map(|element| self.pattern(element, seen, binders, dropped))
                    .collect();
                let rest = rest.map(|rest| ArrayRest {
                    span: rest.span,
                    name: rest.name.map(|name| self.bound(name, seen, binders)),
                });
                let after = after
                    .into_iter()
                    .map(|element| self.pattern(element, seen, binders, dropped))
                    .collect();
                span.track(PatternKind::Array {
                    before,
                    rest,
                    after,
                })
            }
        }
    }

    /// Wrap `inner` in the bindings a calm pattern makes against `value`: a
    /// name is one `let`, `()` is a fresh binding annotated unit — the
    /// pattern's whole demand — and a struct pattern is R6's chain, a fresh
    /// temporary and one binding per field, in written order.
    fn destructure(
        &mut self,
        calm: Calm,
        value: Term,
        annotation: Option<Box<Annotation>>,
        inner: Term,
    ) -> Term {
        match calm {
            Calm::Bind(name) => {
                let span = name.span.merge(inner.span);
                TermKind::Let {
                    name,
                    annotation,
                    value: Box::new(value),
                    body: Box::new(inner),
                }
                .with_span(span)
            }
            // The binding a name would have made, made to a name nothing can
            // write: the value keeps its place — typechecked, its mistakes
            // still its own complaints, the annotation still its contract —
            // and the body cannot reach it.
            Calm::Wildcard(span) => {
                let held = self.fresh("%discard", span);
                let at = span.merge(inner.span);
                TermKind::Let {
                    name: held,
                    annotation,
                    value: Box::new(value),
                    body: Box::new(inner),
                }
                .with_span(at)
            }
            Calm::Unit(span) => match annotation {
                // The written type is the contract on the whole value, and the
                // pattern's own demand — unit — goes on a second binding of
                // it, so both are said and neither displaces the other.
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    let constrained = self.destructure(Calm::Unit(span), again, None, inner);
                    let at = span.merge(constrained.span);
                    TermKind::Let {
                        name: held,
                        annotation: Some(annotation),
                        value: Box::new(value),
                        body: Box::new(constrained),
                    }
                    .with_span(at)
                }
                None => {
                    let unit = demand(span.track(TypeKind::Struct {
                        fields: IndexMap::new(),
                        tail: None,
                    }));
                    let name = self.fresh("%unit", span);
                    let at = span.merge(inner.span);
                    TermKind::Let {
                        name,
                        annotation: Some(Box::new(unit)),
                        value: Box::new(value),
                        body: Box::new(inner),
                    }
                    .with_span(at)
                }
            },
            // The unit pattern's split again: a written annotation holds the
            // whole value on a binding of its own, and the pattern's demand —
            // an array, see [`array_demand`] — goes on the binding the rest
            // makes, named as written or as nothing can name.
            Calm::ArrayRest { span, name } => match annotation {
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    let constrained =
                        self.destructure(Calm::ArrayRest { span, name }, again, None, inner);
                    let at = span.merge(constrained.span);
                    TermKind::Let {
                        name: held,
                        annotation: Some(annotation),
                        value: Box::new(value),
                        body: Box::new(constrained),
                    }
                    .with_span(at)
                }
                None => {
                    let name = name.unwrap_or_else(|| self.fresh("%array", span));
                    let at = span.merge(inner.span);
                    TermKind::Let {
                        name,
                        annotation: Some(Box::new(array_demand(span))),
                        value: Box::new(value),
                        body: Box::new(inner),
                    }
                    .with_span(at)
                }
            },
            // Without the `..` the pattern is exact, and the demand — the
            // value has exactly the named fields — is an annotation of the
            // pattern's own making; see [`exact_demand`]. A written annotation
            // is the contract on the whole value, so it goes on a binding of
            // its own and the demand on a second, the way a unit pattern's
            // does. With the `..` the pattern asks only for the fields it
            // names, which is the demand the projections already make.
            Calm::Struct { span, fields, rest } => match (annotation, rest) {
                (Some(annotation), None) => {
                    let held = self.fresh("%value", span);
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    let calm = Calm::Struct { span, fields, rest };
                    let constrained = self.destructure(calm, again, None, inner);
                    let at = span.merge(constrained.span);
                    TermKind::Let {
                        name: held,
                        annotation: Some(annotation),
                        value: Box::new(value),
                        body: Box::new(constrained),
                    }
                    .with_span(at)
                }
                (annotation, rest) => {
                    let annotation = match rest {
                        None => Some(Box::new(exact_demand(span, &fields))),
                        Some(_) => annotation,
                    };
                    let held = self.fresh("%struct", span);
                    let mut inner = inner;
                    for (name, sub) in fields.into_iter().rev() {
                        let base = TermKind::Ident(held.tracked).with_span(name.span);
                        let field = TermKind::Project {
                            base: Box::new(base),
                            field: name.clone(),
                        }
                        .with_span(name.span);
                        inner = self.destructure(sub, field, None, inner);
                    }
                    let at = span.merge(inner.span);
                    TermKind::Let {
                        name: held,
                        annotation,
                        value: Box::new(value),
                        body: Box::new(inner),
                    }
                    .with_span(at)
                }
            },
        }
    }

    /// R6's statement half: a calm pattern on a top-level `let` becomes
    /// ordinary top-level definitions — a fresh one holding the value, with
    /// the written annotation, then one per name, fields in written order.
    fn destructure_stmt(
        &mut self,
        calm: Calm,
        annotation: Option<Annotation>,
        value: Term,
        out: &mut IndexMap<Symbol, Decl<Term>>,
    ) {
        match calm {
            Calm::Bind(name) => {
                out.insert(
                    name.tracked,
                    Decl {
                        name_span: name.span,
                        annotation,
                        params: Vec::new(),
                        value,
                    },
                );
            }
            // The definition a name would have made, made under a name nothing
            // can write. The value is still an ordinary definition — inferred,
            // checked against the annotation when one was written — so
            // `let _ : T = e` is a type assertion; and the name being fresh is
            // what lets any number of `let _` stand side by side.
            Calm::Wildcard(span) => {
                let held = self.fresh("%discard", span);
                out.insert(
                    held.tracked,
                    Decl {
                        name_span: span,
                        annotation,
                        params: Vec::new(),
                        value,
                    },
                );
            }
            Calm::Unit(span) => match annotation {
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    out.insert(
                        held.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(annotation),
                            params: Vec::new(),
                            value,
                        },
                    );
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    self.destructure_stmt(Calm::Unit(span), None, again, out);
                }
                None => {
                    let unit = demand(span.track(TypeKind::Struct {
                        fields: IndexMap::new(),
                        tail: None,
                    }));
                    let name = self.fresh("%unit", span);
                    out.insert(
                        name.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(unit),
                            params: Vec::new(),
                            value,
                        },
                    );
                }
            },
            Calm::ArrayRest { span, name } => match annotation {
                Some(annotation) => {
                    let held = self.fresh("%value", span);
                    out.insert(
                        held.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(annotation),
                            params: Vec::new(),
                            value,
                        },
                    );
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    self.destructure_stmt(Calm::ArrayRest { span, name }, None, again, out);
                }
                None => {
                    let name = name.unwrap_or_else(|| self.fresh("%array", span));
                    out.insert(
                        name.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(array_demand(span)),
                            params: Vec::new(),
                            value,
                        },
                    );
                }
            },
            // The expression form's split, said about definitions: a written
            // annotation holds the whole value on a binding of its own, and
            // the exact pattern's demand — see [`exact_demand`] — rides on the
            // `%struct` the projections read from.
            Calm::Struct { span, fields, rest } => match (annotation, rest) {
                (Some(annotation), None) => {
                    let held = self.fresh("%value", span);
                    out.insert(
                        held.tracked,
                        Decl {
                            name_span: span,
                            annotation: Some(annotation),
                            params: Vec::new(),
                            value,
                        },
                    );
                    let again = TermKind::Ident(held.tracked).with_span(span);
                    let calm = Calm::Struct { span, fields, rest };
                    self.destructure_stmt(calm, None, again, out);
                }
                (annotation, rest) => {
                    let annotation = match rest {
                        None => Some(exact_demand(span, &fields)),
                        Some(_) => annotation,
                    };
                    let held = self.fresh("%struct", span);
                    out.insert(
                        held.tracked,
                        Decl {
                            name_span: span,
                            annotation,
                            params: Vec::new(),
                            value,
                        },
                    );
                    for (name, sub) in fields {
                        let base = TermKind::Ident(held.tracked).with_span(name.span);
                        let field = TermKind::Project {
                            base: Box::new(base),
                            field: name.clone(),
                        }
                        .with_span(name.span);
                        self.destructure_stmt(sub, None, field, out);
                    }
                }
            },
        }
    }

    /// Lower `match <expr> with <arms> end`: the scrutinee, then each arm's
    /// pattern and body — the pattern's names in scope for its own body and
    /// released after it — into the one [`TermKind::Match`] node, arms in
    /// written order. Every written arm is lowered and kept, so every one is
    /// typechecked: which arms a value can reach, where a catch-all belongs,
    /// and whether anything goes unhandled are questions about solved types,
    /// asked by [`patterns`](crate::patterns) once inference has answered
    /// them. What stays here is the pattern's own to get wrong — a name bound
    /// twice — reported by the pattern walk itself.
    fn match_term(&mut self, span: Span, scrutinee: Expr, arms: Vec<parse::Arm>) -> Term {
        let scrutinee = self.term(scrutinee);
        let mut lowered: Vec<(Pattern, Term)> = Vec::new();
        for arm in arms {
            let mark = self.terms.mark();
            let mut seen = Vec::new();
            let mut dropped = Vec::new();
            let pattern = self.pattern(arm.pattern, &mut seen, &mut Binders::Local, &mut dropped);
            let body = self.term(arm.body);
            self.terms.release(mark);
            let body = bound_to_errors(dropped, body);
            lowered.push((pattern, body));
        }
        TermKind::Match {
            scrutinee: Box::new(scrutinee),
            arms: lowered,
        }
        .with_span(span)
    }
}

/// What a `..` written on a row of this shape stands for: a struct's is the
/// whole type its fields sit on, and a sum's is the cases it does not write out.
/// The one place the two shapes still have different answers, and the reason
/// [`Sense`] has two variants rather than one. See [`ErrorKind::MixedTail`].
fn sense(shape: Shape) -> Sense {
    match shape {
        Shape::Struct => Sense::Fields,
        Shape::Sum => Sense::Cases,
        Shape::Effect => Sense::Effects,
    }
}

/// What a variable written here is told, or `None` where one may be written.
///
/// The two positions that hold for every use of what they describe refuse one,
/// and each in its own words: a declaration's variables are its parameters, and
/// an operation's signature is the one place the same sentence is worded about
/// the `+`, the `..` and the `when` beside it.
fn wherever(place: Place, name: &str) -> Option<ErrorKind> {
    match place {
        Place::Annotation => None,
        Place::Declaration => Some(ErrorKind::VariableInDeclaration {
            name: name.to_string(),
        }),
        Place::Operation => Some(ErrorKind::ImpureOperation {
            found: OperationTypeProblem::Variable(name.to_string()),
        }),
    }
}

/// What a row written here is told when it is left open, or `None` where being
/// open is allowed.
///
/// One question in one place, because the answer travels: a `when` and a `..`
/// each stand for something a definition gets to decide, and the two positions
/// that hold for every definition — a `type` declaration's body and an
/// operation's signature — therefore refuse both. They differ only in the words:
/// an operation's signature refuses the `+` beside them, which a declaration's
/// body allows, so a reader who wrote one is told about all three at once.
fn openness(place: Place, shape: Shape) -> Option<ErrorKind> {
    match place {
        Place::Annotation => None,
        Place::Declaration => Some(ErrorKind::OpenDeclaredType { shape }),
        Place::Operation => Some(ErrorKind::ImpureOperation {
            found: OperationTypeProblem::OpenPart,
        }),
    }
}

/// Whether a row's `..` names one of its declaration's parameters. The one
/// place a parameter appears without being a [`TypeKind::Param`] node, so every
/// walk that asks about parameters has to ask about this too — and both shapes
/// of row have one.
fn tails_a_parameter(tail: &Option<Tail>) -> bool {
    matches!(
        tail,
        Some(Tail {
            of: Row::Param { .. },
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parse,
        symbol::{Bundle, Version},
        token,
        tracking::FileID,
    };

    #[test]
    fn extern_target_text_and_literal_span_survive_lowering() {
        let source = r#"extern value : String = "globalThis.answer""#;
        let lexed = token::lex(source, FileID::GENERATED);
        assert!(lexed.errors.is_empty());
        let parsed = parse::parse(lexed.tokens);
        assert!(parsed.errors.is_empty());

        let bundle = Bundle::new("test", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let lowered = build(&mut mint, parsed.stmts);
        assert!(lowered.errors.is_empty(), "IR errors: {:?}", lowered.errors);

        let external = lowered.program.externs.values().next().unwrap();
        assert_eq!(external.value.target.tracked, "globalThis.answer");
        let start = source.find('"').unwrap();
        assert_eq!(external.value.target.span.start, start);
        assert_eq!(external.value.target.span.width, source.len() - start);
    }
}
