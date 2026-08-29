//! LIR: the lowered instruction stream a backend consumes.
//!
//! The IR is a tree of nested expressions carrying full structural types — the
//! right shape for inference and diagnostics, the wrong shape for code
//! generation. This phase flattens that tree into a list of top-level functions
//! and globals: every nested expression becomes a temp assignment, every type
//! collapses to a machine [`Rep`]resentation, every `match` becomes a
//! decision tree of tests, every `fn` becomes a lifted function with an explicit
//! capture list, and every effect becomes either an ordinary call through
//! passed-down evidence or a tagged [`End::Throw`].
//!
//! Lowering is infallible. It runs only after lexing, parsing, building,
//! inference and the pattern checks have all reported nothing, so it may assume
//! there is no [`TermKind::Error`], that every [`Term::ty`] is solved, that
//! every match is exhaustive, and that every `raise` sits inside a handler arm.
//! There is no error list here because there is nothing left to complain about.
//!
//! Nothing here is a target. Both planned backends — JS/TS and WebAssembly —
//! have structured control flow and native exceptions, so the shape below maps
//! onto either without a second round of lowering, but no code is generated and
//! no LIR is ever run.

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    inference::{self, unfold},
    ir::{Handler, HandlerArm, Literal, Pattern, PatternKind, Program, Term, TermKind},
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{Formula, Presence, Rest, Row, Ty, same_finite_syntax},
};

/// A value the instruction stream names. Numbered by one program-wide counter,
/// so `%17` is the same temp wherever it appears in the whole listing — which is
/// what lets the debugger cross-highlight one without knowing which function it
/// is looking at. Every value-producing instruction assigns exactly one, and
/// every temp is assigned exactly once.
pub type Temp = u32;

/// A function's position in [`Output::functions`]. Names would do, but a
/// position cannot be misspelled and cannot collide with a definition that
/// happens to be called `map#1`.
pub type FuncId = usize;

/// A field in a lowered record. User/effect names remain ordinary named fields;
/// an unnamed operation has its own unforgeable slot and is never encoded as a
/// private string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldKey {
    Named(String),
    UnnamedOperation,
}

impl FieldKey {
    fn named(name: impl Into<String>) -> Self {
        Self::Named(name.into())
    }
}

fn named_fields(fields: IndexMap<String, Temp>) -> IndexMap<FieldKey, Temp> {
    fields
        .into_iter()
        .map(|(name, value)| (FieldKey::Named(name), value))
        .collect()
}

/// The lowered program: every function, and every global in the order a backend
/// must initialize them.
///
/// No error list. See the module docs: R2 hands this pass a program every
/// earlier phase accepted, and lowering an accepted program cannot fail.
#[derive(Debug, Clone)]
pub struct Output {
    /// Target-provided globals. Unlike [`Global`], these have no initializer:
    /// a backend imports them from their dotted target instead of evaluating a
    /// Ruddy block for them.
    pub externs: Vec<Extern>,
    /// Every lifted `fn`, handler arm and generated wrapper, in the order they
    /// were lifted. Flat: no function nests inside another.
    pub functions: Vec<Function>,
    /// Every top-level definition's initializer, in group order — earliest
    /// group first, and within a group the order the definitions appear in it.
    /// A backend initializes them in exactly this order.
    pub globals: Vec<Global>,
}

/// One target-provided global value, ready for a backend import table.
#[derive(Debug, Clone)]
pub struct Extern {
    pub symbol: Symbol,
    pub name: String,
    pub target: Vec<String>,
    pub span: Span,
    pub rep: Rep,
}

/// One top-level function: what it takes, what it does, and what to call it in a
/// listing.
#[derive(Debug, Clone)]
pub struct Function {
    /// Derived from the enclosing top-level definition: `map` for the function
    /// `map`'s own body, then `map#1`, `map#2` … for everything lifted out of
    /// it, numbered in the order it was lifted.
    pub name: String,
    /// Captures first, then the hidden evidence parameters of each arrow level,
    /// then that level's one visible argument. See [`Rep`] and the evidence
    /// rules in the module docs.
    pub params: Vec<Param>,
    pub body: Block,
    /// Where the source wrote it, or a generated span for a function no one
    /// wrote — a curried wrapper, or an operation used as a value.
    pub span: Span,
}

/// One parameter: the temp it binds and how it is represented.
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub temp: Temp,
    pub rep: Rep,
}

/// One top-level definition's value, as the block that computes it.
#[derive(Debug, Clone)]
pub struct Global {
    pub symbol: Symbol,
    pub name: String,
    pub body: Block,
    pub span: Span,
}

/// An ordered list of instructions and exactly one terminator.
#[derive(Debug, Clone)]
pub struct Block {
    pub instrs: Vec<Instr>,
    pub end: Terminator,
}

/// One instruction: the temp it assigns, how that temp is represented, where the
/// source wrote it, and what it does.
#[derive(Debug, Clone)]
pub struct Instr {
    pub temp: Temp,
    pub rep: Rep,
    pub span: Span,
    pub op: Op,
}

/// What one instruction does.
#[derive(Debug, Clone)]
pub enum Op {
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
    Struct(IndexMap<FieldKey, Temp>),
    /// One record carrying every field of each of these, laid over one another
    /// in order: a later one wins wherever two of them name the same field.
    ///
    /// Evidence plumbing alone, and the only way to build a record out of one
    /// whose fields are not known here — which is what a tail bundle is. Never
    /// fewer than two operands: laying one record over nothing is that record.
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
    /// Pair a function with the current values of its captures.
    Closure {
        func: FuncId,
        captures: Vec<Temp>,
    },
    /// A call. `args` holds each level's evidence records ahead of that level's
    /// visible argument; an indirect call has exactly one visible argument.
    Call {
        callee: Callee,
        args: Vec<Temp>,
    },
    /// Invoke a raw target-provided function. Unlike [`Op::Call`], every entry
    /// in `args` is source-visible at the foreign boundary: effect evidence is
    /// structurally excluded and must be consumed by an adapter before this
    /// instruction is emitted.
    RawCall {
        callee: Temp,
        args: Vec<Temp>,
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
    },
    /// Mint a fresh handler identity, once per dynamic evaluation of a `handle`.
    NewTag,
    /// Run a block, catching only the throws carrying `tag` — a throw with any
    /// other tag keeps unwinding, which is what makes nested and recursive
    /// handlers of one effect correct. A caught value becomes this instruction's
    /// temp directly, bypassing the `return` arm.
    Catch {
        tag: Temp,
        body: Box<Block>,
    },
    /// Dispatch on a sum value's case. `fallback` is present only where some arm
    /// accepts cases the listed ones do not cover.
    SwitchTag {
        on: Temp,
        cases: Vec<TagCase>,
        fallback: Option<Box<Block>>,
    },
    /// Dispatch on a primitive value. The fallback is absent only when the
    /// listed Boolean cases cover both possible values; every other primitive
    /// type has values beyond its listed literals.
    SwitchPrim {
        on: Temp,
        cases: Vec<PrimCase>,
        fallback: Option<Box<Block>>,
    },
    /// Dispatch on whether a struct value has a field, where the solved type
    /// leaves that field's presence undecided at runtime.
    SwitchPresence {
        on: Temp,
        field: String,
        present: Box<Block>,
        absent: Box<Block>,
    },
    /// Dispatch on whether a struct value carries any field beyond the listed
    /// names — what separates an exact pattern from an open one when the
    /// scrutinee's solved type is open.
    SwitchRest {
        on: Temp,
        fields: Vec<String>,
        none: Box<Block>,
        some: Box<Block>,
    },
}

/// What an [`Op::Call`] calls: a function whose identity is known here, or
/// whatever a temp holds.
#[derive(Debug, Clone, Copy)]
pub enum Callee {
    Direct(FuncId),
    Indirect(Temp),
}

/// One case of a [`Op::SwitchTag`].
#[derive(Debug, Clone)]
pub struct TagCase {
    pub name: String,
    pub block: Block,
}

/// One case of a [`Op::SwitchPrim`].
#[derive(Debug, Clone)]
pub struct PrimCase {
    pub value: Literal,
    pub block: Block,
}

/// How a block ends, and where the source that ended it was written.
#[derive(Debug, Clone, Copy)]
pub struct Terminator {
    pub span: Span,
    pub kind: End,
}

/// The three ways a block ends.
#[derive(Debug, Clone, Copy)]
pub enum End {
    /// Return from the function, or finish a global's initializer.
    Ret(Temp),
    /// Finish a child block, making the temp the value of the block-valued
    /// instruction that owns it.
    Yield(Temp),
    /// Unwind to the matching [`Op::Catch`]. Code after a lowered `raise` is
    /// unreachable and is not emitted.
    Throw { tag: Temp, value: Temp },
}

/// How a value is represented, which is the whole of what LIR keeps of a type.
///
/// No structural information survives: no rows, no presences, no effect rows, no
/// field lists, and no [`Ty`] anywhere in the data model. A backend needs to
/// know how to hold a value and nothing else, and monomorphization is deferred,
/// so everything a scheme quantified is [`Rep::Any`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Sum,
    /// A closure: a function paired with its captures.
    Fn,
    /// Anything the solved type does not pin down — a quantified variable, a
    /// rigid, or a type nothing decided.
    Any,
}

/// The hidden evidence one arrow asks for: a record of operation closures per
/// effect the arrow definitely performs, in row order, and then a bundle for
/// whatever the variable part of its row stands for.
///
/// A function is compiled against the shape of the row it was *written* with,
/// and every call has to hand it that same shape. The two need not agree —
/// passing a function that performs `Log` into a parameter declared effect
/// polymorphic turns a record of its own into one entry of a bundle — so where
/// they differ an adapter stands between them; see [`Lower::fitted`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Shape {
    /// The definitely performed effects, in row order: one record each.
    names: Vec<String>,
    /// Whether a bundle keyed by effect name follows those records.
    tail: bool,
}

/// One arrow of a known function's signature: what that level takes, what it
/// gives back, the effects it may perform, and how many hidden evidence
/// parameters those effects come to.
#[derive(Debug, Clone)]
struct Level {
    from: Rc<Ty>,
    to: Rc<Ty>,
    row: Row,
    evidence: usize,
}

/// A top-level definition whose value is a nest of `fn`s: the uncurried function
/// its body became, the curried wrappers that let it be used one argument at a
/// time, and one [`Level`] per arrow.
#[derive(Debug, Clone)]
struct Known {
    lifted: FuncId,
    /// One per level, outermost first: `wrappers[k]` is the function that takes
    /// argument `k` given the `k` before it.
    wrappers: Vec<FuncId>,
    levels: Vec<Level>,
    /// The type the wrappers were built against, which is the type naming the
    /// definition as a value stands for however it is instantiated at the use.
    ty: Rc<Ty>,
}

/// What the variable part of an effect row is, so that a caller can tell whether
/// the callee's tail is the very one it was handed — and so forward its own
/// bundle instead of building a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestKey {
    /// The row variable a scheme quantified, which is the one identity two
    /// rows can be known to share.
    Bound(u32),
    /// A variable part with no identity to share: a closed row one of whose
    /// labels is only *maybe* there. Never forwarded.
    Open,
}

/// One value a function borrows from the scope it was written in: where it came
/// from outside, the parameter it arrives as inside, and how it is held.
#[derive(Debug, Clone, Copy)]
struct Capture {
    outer: Temp,
    inner: Temp,
    rep: Rep,
}

/// A local function whose binding is in scope in its own body.
///
/// Its closure cannot be captured from the surrounding block: that closure is
/// the value the function is still in the middle of producing. Instead, the
/// lifted body reconstructs the same closure from its own function identity
/// and capture parameters. [`Lower::local`] creates that reconstruction lazily
/// when the recursive name is actually used.
struct Recursive {
    symbol: Symbol,
    ty: Rc<Ty>,
    temp: Option<Temp>,
}

/// What one function being built knows: the names in scope, the evidence in
/// scope, the handler a `raise` in it unwinds to, and what it has had to capture
/// so far.
///
/// A stack of these is the whole of closure conversion. Resolving a name, an
/// effect's evidence or a handler tag walks outward until some frame has it and
/// threads it back in through every frame in between, adding a capture at each —
/// so a capture list is exactly the free variables, in order of first use.
#[derive(Default)]
struct Frame {
    locals: IndexMap<Symbol, Temp>,
    /// Present on the frame of a directly let-bound local `fn`. The binding is
    /// recursive at the language level, but costs nothing here unless its body
    /// actually names itself.
    recursive: Option<Recursive>,
    /// Instructions that must run before the written body. A recursive
    /// function's reconstructed self closure is emitted here after its capture
    /// list is known.
    prologue: Vec<Instr>,
    /// Effect name → the record of that effect's operation closures.
    evidence: IndexMap<String, Temp>,
    /// The bundles standing in for the variable part of a row, by which
    /// variable part they stand for.
    tails: Vec<(RestKey, Temp)>,
    /// The handler identity a `raise` written here throws to. Set on the frame
    /// the `handle` is being lowered in, for as long as its arms are.
    raise_tag: Option<Temp>,
    captures: Vec<Capture>,
    caught: HashMap<Temp, Temp>,
}

/// A block under construction: the instructions so far, and the terminator that
/// has already ended it, if one has.
///
/// Everything after a lowered `raise` is unreachable, and the simplest way to
/// not emit it is to accept it and drop it: [`Body::push`] does nothing once the
/// block has ended, so the lowering above can go on returning temps without
/// every caller having to ask whether the value it just built will ever exist.
#[derive(Default)]
struct Body {
    instrs: Vec<Instr>,
    end: Option<Terminator>,
}

/// One application: the node the whole application comes to, and the argument
/// written under it.
///
/// The node is kept rather than just its type because two things are read off
/// it — what the application comes to, which is its own solved type, and where
/// the call was written, which is its span.
#[derive(Debug, Clone, Copy)]
struct Apply<'a> {
    node: &'a Term,
    arg: &'a Term,
}

/// One pattern, with everything that only binds flattened to the wildcard it
/// matches as. [`Cell::Present`] and [`Cell::Absent`] arise from widening a
/// struct column and are written by no pattern.
#[derive(Debug, Clone)]
enum Cell {
    /// Accepts everything, binding the name it carries when it has one.
    Wild(Option<Symbol>),
    Prim(Literal),
    Tag {
        name: String,
        payload: Box<Cell>,
    },
    Struct {
        fields: Vec<(String, Cell)>,
        exact: bool,
    },
    /// The field this presence column stands for must be there, matching what
    /// it carries.
    Present(Box<Cell>),
    /// The field must not be there — or, in a rest column, no field beyond the
    /// listed ones may be, which is what an exact pattern demands.
    Absent,
}

/// One column of the decision matrix: a whole value at a temp, one field's
/// presence, or whatever fields a value carries beyond the ones its type names.
///
/// Each carries its own payload rather than naming fields inline, so that the
/// dispatch in [`Lower::tree`] hands one whole column to whatever decides it
/// and nothing below has to be told what a column is a second time.
#[derive(Debug, Clone)]
enum Col {
    Value(Value),
    Field(Field),
    Beyond(Beyond),
}

/// A whole value, at the temp holding it.
#[derive(Debug, Clone)]
struct Value {
    temp: Temp,
    ty: Rc<Ty>,
}

/// One field of a struct: whether it is there, and what it holds when it is.
#[derive(Debug, Clone)]
struct Field {
    base: Temp,
    base_ty: Rc<Ty>,
    name: String,
    presence: Presence,
    ty: Rc<Ty>,
}

/// Whatever a struct carries past the fields its type names. `open` is whether
/// there can be any.
#[derive(Debug, Clone)]
struct Beyond {
    base: Temp,
    names: Vec<String>,
    open: bool,
}

/// One row of the decision matrix: what the arm tests at each live column, which
/// arm it is, and what the path to here has bound.
#[derive(Debug, Clone)]
struct Line {
    cells: Vec<Cell>,
    arm: usize,
    binds: Vec<(Symbol, Temp)>,
}

/// The decision matrix under one column: the columns still to test and the rows
/// still live. Every row has one cell per column, in the same order, and cell
/// zero is the column being decided right now.
///
/// The two halves travel together everywhere — narrowing the rows and advancing
/// the columns are the same step of one algorithm — so they are one value, and
/// what the algorithm does to them is the methods on it.
#[derive(Debug, Clone)]
struct Matrix {
    cols: Vec<Col>,
    lines: Vec<Line>,
    /// Presence literals selected on the path to this matrix. They let the
    /// decision tree skip combinations the inferred definition promise rules
    /// out, while still emitting ordinary `SwitchPresence` instructions for
    /// the alternatives that can both occur.
    assumed: Formula,
}

/// What every leaf of one match's tree needs: the arms to emit, the
/// representation their value has, what the whole match stands for — which is
/// what every arm's value is fitted to on the way out — and where the match
/// was written.
struct Tree<'a> {
    arms: &'a [(Pattern, Term)],
    rep: Rep,
    ty: Rc<Ty>,
    span: Span,
    allowed: Formula,
}

fn alias_symbols(want: &Rc<Ty>, have: &Rc<Ty>) -> Option<(Symbol, Symbol)> {
    match (&**want, &**have) {
        (Ty::Named { symbol: want, .. }, Ty::Named { symbol: have, .. }) => Some((*want, *have)),
        _ => None,
    }
}

fn same_alias_pair(
    left_want: &Rc<Ty>,
    left_have: &Rc<Ty>,
    right_want: &Rc<Ty>,
    right_have: &Rc<Ty>,
) -> bool {
    same_finite_syntax(left_want, right_want) && same_finite_syntax(left_have, right_have)
}

fn same_row_syntax(left: &Row, right: &Row) -> bool {
    let unit = Rc::new(Ty::unit());
    same_finite_syntax(
        &Rc::new(Ty::Arrow(unit.clone(), unit.clone(), left.clone())),
        &Rc::new(Ty::Arrow(unit.clone(), unit, right.clone())),
    )
}

type TypePair = (Rc<Ty>, Rc<Ty>);
type AliasPairs = HashMap<(Symbol, Symbol), Vec<TypePair>>;
type AdapterAliases = HashMap<(Symbol, Symbol), Vec<(Rc<Ty>, Rc<Ty>, FuncId)>>;
type CachedFit = (Rc<Ty>, Rc<Ty>, bool);

struct OrdinaryExternAdapter {
    ty: Rc<Ty>,
    id: FuncId,
}

struct OrdinaryCallbackAdapter {
    ty: Rc<Ty>,
    available: Row,
    id: FuncId,
}

#[derive(Default)]
struct FitsCache {
    /// Non-aliased arrow pairs keep their allocation identity while a type is
    /// walked. Remembering their answer makes fitting a deep suffix linear
    /// rather than asking the same question again at every enclosing arrow.
    direct: HashMap<(usize, usize), CachedFit>,
}

/// A generated adapter while its two recursive fits are evaluated. Keeping
/// these on the heap is what lets a source type contain an arbitrary number of
/// arrow levels without consuming the Rust call stack.
struct FittedContext {
    want: Rc<Ty>,
    wrapped: Temp,
    params: Vec<Param>,
    body: Body,
    stage: FittedStage,
    reserved: Option<(FuncId, String)>,
}

enum FittedStage {
    Argument {
        args: Vec<Temp>,
        have_to: Rc<Ty>,
        want_to: Rc<Ty>,
    },
    Result,
}

/// The lowering itself: the program being read, everything emitted so far, and
/// the stack of functions currently being built.
struct Lower<'a> {
    mint: &'a Mint,
    program: &'a Program,
    inference: &'a inference::Output,
    temps: u32,
    /// How each temp is held, indexed by the temp. A capture has to be given the
    /// representation of what it captures, and the only place that is written
    /// down is here.
    reps: Vec<Rep>,
    /// The type whose shape each function-valued temp holds — the type it was
    /// produced or fitted at — indexed by the temp. A call reads the callee's
    /// evidence parameters off this, not off the type the use site instantiates
    /// the value at: the two disagree exactly where a polymorphic value reaches
    /// the call through a binding, a parameter or a return. A struct or sum
    /// temp records the type it was built at for the same reason: the
    /// functions inside it are shaped by that type's members, and a read has
    /// to know which type that was. `None` where no producer recorded one, in
    /// which case production and use are the same place and the use's type is
    /// the type held.
    held: Vec<Option<Rc<Ty>>>,
    /// Reserved before anything is emitted, because a definition may call itself
    /// or its neighbour: a slot exists from the moment its name does, and is
    /// filled when its body is built.
    functions: Vec<Option<Function>>,
    labels: Vec<String>,
    known: IndexMap<Symbol, Known>,
    frames: Vec<Frame>,
    /// The top-level definition everything currently being lifted belongs to,
    /// and how many functions it has lifted so far.
    stem: String,
    serial: u32,
    globals: Vec<Global>,
    /// Ordinary foreign adapters currently being generated. Recursive aliases
    /// can return (or accept contravariantly) their own function shape, so the
    /// function slot must be reusable before its body has finished lowering.
    extern_adapters: Vec<OrdinaryExternAdapter>,
    callback_adapters: Vec<OrdinaryCallbackAdapter>,
    /// The top-level definition currently being lowered, whose quantified
    /// presence promise governs every nested match decision tree.
    definition: Option<Symbol>,
    /// Presence literals selected by enclosing match decision trees while an
    /// arm body is lowered. A nested match begins under this path condition.
    assumed: Formula,
}

/// Lower a typed, checked program.
///
/// Runs only when lexing, parsing, building, inference and the pattern checks
/// have all reported nothing; see the module docs for what that buys.
pub fn lower(mint: &Mint, program: &Program, inference: &inference::Output) -> Output {
    let mut low = Lower {
        mint,
        program,
        inference,
        temps: 0,
        reps: Vec::new(),
        held: Vec::new(),
        functions: Vec::new(),
        labels: Vec::new(),
        known: IndexMap::new(),
        frames: Vec::new(),
        stem: String::new(),
        serial: 0,
        globals: Vec::new(),
        extern_adapters: Vec::new(),
        callback_adapters: Vec::new(),
        definition: None,
        assumed: Formula::True,
    };
    let externs = low.lower_externs();
    let order = low.order();
    low.reserve(&order);
    for symbol in &order {
        low.define(*symbol);
    }
    Output {
        externs,
        functions: low
            .functions
            .into_iter()
            .map(|slot| slot.expect("every reserved function slot is filled"))
            .collect(),
        globals: low.globals,
    }
}

/// A sum's or an effect's cases with a tail already decided to be more cases
/// folded in, the outer copy of a label winning.
///
/// A declared sum handed a row at its parameter arrives this way — unfolding
/// `Fallible (#Ok Nat)` splices the argument's cases in behind a tail — so the
/// dispatch that reads the row sees one flat set of cases either way.
fn flat(row: &Row) -> Row {
    let mut labels = row.labels.clone();
    let mut rest = row.rest.clone();
    while let Rest::More(more) = rest {
        for (name, field) in &more.labels {
            labels.entry(name.clone()).or_insert_with(|| field.clone());
        }
        rest = more.rest.clone();
    }
    Row { labels, rest }
}

/// Whether a label is *definitely* there, which is what earns an effect a
/// positional evidence parameter of its own.
fn definite(presence: &Presence) -> bool {
    matches!(presence, Presence::Present)
}

/// Whether a label may be there at all: everything but a settled absence.
fn possible(presence: &Presence) -> bool {
    !matches!(presence, Presence::Absent)
}

/// What the variable part of an effect row stands for, or `None` for a row that
/// has none — a closed row every one of whose labels is settled.
///
/// A label whose own presence is a variable counts: whether that effect is
/// performed is not known here, so its evidence has to travel in the bundle
/// rather than in a parameter that may turn out to stand for nothing.
fn tail_key(row: &Row) -> Option<RestKey> {
    let wobbly = row.labels.values().any(|field| {
        matches!(
            field.presence,
            Presence::Var(_) | Presence::Bound(_) | Presence::Undecided
        )
    });
    match &row.rest {
        Rest::Bound(index) => Some(RestKey::Bound(*index)),
        // Every other rest is one with no identity to share. `More` cannot
        // reach here: every row asked is flattened first. A rest still a solver
        // variable, one an annotation made rigid and one a failure abandoned
        // are all R2's to rule out — it hands this pass solved rows alone — so
        // what is left is the closed row, whose variable part is a label whose
        // own presence is a variable and nothing else.
        _ => wobbly.then_some(RestKey::Open),
    }
}

/// The hidden evidence one arrow's effect row comes to: a record per definitely
/// performed effect, in row order, and a tail bundle where the row has a
/// variable part.
fn shape(row: &Row) -> Shape {
    Shape {
        names: row
            .labels
            .iter()
            .filter(|(_, field)| definite(&field.presence))
            .map(|(name, _)| name.clone())
            .collect(),
        tail: tail_key(row).is_some(),
    }
}

/// The `fn` nest at the head of a term, and the body underneath it. The nest's
/// depth is a function's arity: `fn a => fn b => e` is two levels over `e`.
fn nest(term: &Term) -> (Vec<(&Term, Symbol)>, &Term) {
    let mut fns = Vec::new();
    let mut cur = term;
    while let TermKind::Fn { arg, body } = &cur.kind {
        fns.push((cur, arg.tracked));
        cur = body;
    }
    (fns, cur)
}

/// An application spine, innermost first: the ultimate callee, and every
/// [`Apply`] over it.
fn spine(term: &Term) -> (&Term, Vec<Apply<'_>>) {
    let mut applies = Vec::new();
    let mut cur = term;
    while let TermKind::Apply { func, arg } = &cur.kind {
        applies.push(Apply { node: cur, arg });
        cur = func;
    }
    applies.reverse();
    (cur, applies)
}

/// A pattern as the decision matrix matches it.
///
/// A case written bare is a wildcard rather than the empty exact struct the
/// pattern checks read it as: inference has already pinned such a payload to
/// unit, so there is nothing left here to test and extracting the payload at all
/// would be an instruction with no reader.
fn cell(pattern: &Pattern) -> Cell {
    match &pattern.tracked {
        PatternKind::Bind(name) => Cell::Wild(Some(name.tracked)),
        PatternKind::Wildcard => Cell::Wild(None),
        PatternKind::Natural(value) => Cell::Prim(Literal::Natural(*value)),
        PatternKind::Integer(value) => Cell::Prim(Literal::Integer(*value)),
        PatternKind::Real(value) => Cell::Prim(Literal::Real(*value)),
        PatternKind::String(value) => Cell::Prim(Literal::String(value.clone())),
        PatternKind::Boolean(value) => Cell::Prim(Literal::Boolean(*value)),
        PatternKind::Unit => Cell::Struct {
            fields: Vec::new(),
            exact: true,
        },
        PatternKind::Struct { fields, rest } => Cell::Struct {
            fields: fields
                .iter()
                .map(|(name, field)| (name.clone(), cell(&field.value)))
                .collect(),
            exact: rest.is_none(),
        },
        PatternKind::Tag { name, payload } => Cell::Tag {
            name: name.tracked.clone(),
            payload: Box::new(payload.as_deref().map(cell).unwrap_or(Cell::Wild(None))),
        },
    }
}

/// Whether a cell tests anything at all, which is what decides whether the
/// position it sits at is worth reading out of the value.
fn tests(cell: &Cell) -> bool {
    !matches!(cell, Cell::Wild(None))
}

impl Shape {
    /// How many hidden parameters the shape comes to.
    fn arity(&self) -> usize {
        self.names.len() + usize::from(self.tail)
    }
}

impl RestKey {
    /// Whether a caller holding a bundle for `self` may hand it straight on to a
    /// callee whose variable part is `other`. Only a shared identity may:
    /// [`RestKey::Open`] is two rows each having *some* variable part, which is
    /// no reason to think they have the same one.
    fn forwards(self, other: RestKey) -> bool {
        !matches!(self, RestKey::Open) && self == other
    }
}

impl Line {
    /// This row with the decided column's cell dropped, and whatever that cell
    /// bound bound to the temp the column stood at.
    fn consumed(mut self, temp: Temp) -> Line {
        if let Cell::Wild(Some(symbol)) = &self.cells[0] {
            self.binds.push((*symbol, temp));
        }
        self.cells.remove(0);
        self
    }

    /// The same, where nothing at the column could have bound anything.
    fn dropped(mut self) -> Line {
        self.cells.remove(0);
        self
    }
}

/// Reading the matrix borrows it and narrowing it consumes it, which is the
/// difference between a branch that is one of several — every arm of a switch
/// narrows the same matrix — and a walk that goes on down.
impl Matrix {
    /// Take the column being decided off the front, leaving the matrix under
    /// it, or `None` where every column has been decided and a row has won.
    ///
    /// The rows keep their first cell: it belongs to the column just taken, and
    /// what becomes of it is that column's to say.
    fn split(&mut self) -> Option<Col> {
        match self.cols.is_empty() {
            true => None,
            false => Some(self.cols.remove(0)),
        }
    }

    /// Whether no row tests the column being decided — in which case reading
    /// the value at it would be an instruction with no reader.
    fn untested(&self) -> bool {
        self.lines.iter().all(|line| !tests(&line.cells[0]))
    }

    /// The rows this keeps, under the same columns.
    fn kept(&self, keep: impl Fn(&Line) -> bool) -> Matrix {
        Matrix {
            cols: self.cols.clone(),
            lines: self
                .lines
                .iter()
                .filter(|line| keep(line))
                .cloned()
                .collect(),
            assumed: self.assumed.clone(),
        }
    }

    /// The decided column dropped from every row: it has nothing further to say
    /// about the value.
    fn dropped(self) -> Matrix {
        Matrix {
            cols: self.cols,
            lines: self.lines.into_iter().map(Line::dropped).collect(),
            assumed: self.assumed,
        }
    }

    /// The same, with whatever the column bound bound to the temp it stood at.
    fn consumed(self, temp: Temp) -> Matrix {
        Matrix {
            cols: self.cols,
            lines: self
                .lines
                .into_iter()
                .map(|line| line.consumed(temp))
                .collect(),
            assumed: self.assumed,
        }
    }

    /// Columns put in front of the ones still to test, which is what reading a
    /// value out of another one comes to. The rows already carry the cells they
    /// line up with — a widening put them there, or the column being decided
    /// left its own behind.
    fn under(mut self, mut cols: Vec<Col>) -> Matrix {
        cols.append(&mut self.cols);
        Matrix {
            cols,
            lines: self.lines,
            assumed: self.assumed,
        }
    }

    /// The rows that survive the field being there, with the presence cell
    /// replaced by what the arm asks of the field's value.
    fn present(&self) -> Matrix {
        Matrix {
            cols: self.cols.clone(),
            lines: self
                .lines
                .iter()
                .filter_map(|line| {
                    let mut line = line.clone();
                    line.cells[0] = match &line.cells[0] {
                        Cell::Present(sub) => (**sub).clone(),
                        Cell::Absent => return None,
                        // The widening puts nothing else here.
                        _ => Cell::Wild(None),
                    };
                    Some(line)
                })
                .collect(),
            assumed: self.assumed.clone(),
        }
    }

    /// The rows that survive the field being missing, with the column dropped:
    /// a field that is not there contributes nothing further to the value.
    fn absent(&self) -> Matrix {
        self.kept(|line| matches!(&line.cells[0], Cell::Absent | Cell::Wild(_)))
            .dropped()
    }

    fn assuming(mut self, literal: Formula) -> Matrix {
        self.assumed = self.assumed.and(literal);
        self
    }
}

impl Body {
    fn push(&mut self, instr: Instr) {
        if self.end.is_none() {
            self.instrs.push(instr);
        }
    }

    fn stop(&mut self, end: Terminator) {
        if self.end.is_none() {
            self.end = Some(end);
        }
    }

    /// Close the block, keeping the terminator a `raise` already put on it.
    fn seal(self, end: Terminator) -> Block {
        Block {
            instrs: self.instrs,
            end: self.end.unwrap_or(end),
        }
    }
}

impl Lower<'_> {
    /// Import raw target values and install Ruddy-facing globals in front of
    /// ordinary definitions. Function imports are always adapters: they accept
    /// internal evidence parameters but raw calls contain visible arguments
    /// only. Marked ABI functions accumulate their whole host argument group;
    /// ordinary arrows retain the legacy unary host-curried convention.
    fn lower_externs(&mut self) -> Vec<Extern> {
        let declarations: Vec<_> = self
            .program
            .externs
            .iter()
            .map(|(symbol, decl)| (*symbol, decl.clone()))
            .collect();
        let mut externs = Vec::with_capacity(declarations.len());
        for (symbol, decl) in declarations {
            let name = self.mint.name(symbol).to_string();
            let ty = self.inference.externs[&symbol].body().clone();
            externs.push(Extern {
                symbol,
                name: name.clone(),
                target: decl
                    .value
                    .target
                    .segments
                    .iter()
                    .map(|segment| segment.tracked.clone())
                    .collect(),
                span: decl.value.target.span(),
                rep: self.rep(&ty),
            });

            // Non-function imports already have the Ruddy representation and
            // remain direct globals. Only functions need an initialized
            // adapter in front of the raw host value.
            if self.rep(&ty) == Rep::Fn {
                self.stem = format!("{name}#extern");
                self.serial = 0;
                let mut body = Body::default();
                let raw = self.emit(
                    &mut body,
                    decl.value.target.span(),
                    self.rep(&ty),
                    Op::Extern {
                        symbol,
                        name: name.clone(),
                    },
                );
                let value = self.host_to_ruddy(&decl.value.abi, &ty, raw, &mut body);
                self.globals.push(Global {
                    symbol,
                    name,
                    body: body.seal(Terminator {
                        span: decl.value.target.span(),
                        kind: End::Ret(value),
                    }),
                    span: decl.name_span,
                });
            }
        }
        externs
    }

    fn host_to_ruddy(
        &mut self,
        abi: &crate::ir::ExternType,
        ty: &Rc<Ty>,
        raw: Temp,
        body: &mut Body,
    ) -> Temp {
        use crate::ir::ExternTypeKind;
        match &abi.tracked {
            ExternTypeKind::Group(inner) => self.host_to_ruddy(inner, ty, raw, body),
            ExternTypeKind::Function {
                parameters, result, ..
            } => {
                let arity = parameters.len().max(1);
                let id = self.marked_extern_level(
                    raw,
                    Vec::new(),
                    Vec::new(),
                    ty.clone(),
                    arity,
                    parameters.is_empty(),
                    parameters.clone(),
                    result.as_ref().clone(),
                    0,
                );
                self.emit(
                    body,
                    abi.span,
                    Rep::Fn,
                    Op::Closure {
                        func: id,
                        captures: vec![raw],
                    },
                )
            }
            ExternTypeKind::Ordinary(_) if self.rep(ty) == Rep::Fn => {
                let id = self.ordinary_extern_level(ty.clone());
                self.emit(
                    body,
                    abi.span,
                    Rep::Fn,
                    Op::Closure {
                        func: id,
                        captures: vec![raw],
                    },
                )
            }
            ExternTypeKind::Ordinary(_) => raw,
        }
    }

    /// One legacy host-curried arrow. Evidence is accepted as part of the
    /// Ruddy closure ABI and intentionally omitted from the raw unary call.
    fn ordinary_extern_level(&mut self, ty: Rc<Ty>) -> FuncId {
        if matches!(&*ty, Ty::Named { .. })
            && let Some(adapter) = self
                .extern_adapters
                .iter()
                .rev()
                .find(|adapter| same_finite_syntax(&adapter.ty, &ty))
        {
            return adapter.id;
        }
        let id = self.slot(format!("{}#{}", self.stem, self.serial));
        self.serial += 1;
        let recursive = matches!(&*ty, Ty::Named { .. });
        if recursive {
            self.extern_adapters
                .push(OrdinaryExternAdapter { ty: ty.clone(), id });
        }
        let raw = self.fresh(Rep::Fn);
        let (from, to, row) = self.arrow(&ty);
        let mut params = vec![Param {
            temp: raw,
            rep: Rep::Fn,
        }];
        self.frames.push(Frame::default());
        self.evidence_params(&row, &mut params);
        let argument = self.fresh(self.rep(&from));
        params.push(Param {
            temp: argument,
            rep: self.rep(&from),
        });
        let mut body = Body::default();
        let ordinary = Span::default().track(crate::ir::ExternTypeKind::Ordinary(
            Span::default().track(crate::ir::TypeKind::Error),
        ));
        let argument = self.ruddy_to_host(&ordinary, &from, argument, &row, &mut body);
        let result = self.emit(
            &mut body,
            Span::default(),
            self.rep(&to),
            Op::RawCall {
                callee: raw,
                args: vec![argument],
            },
        );
        let result = self.host_to_ruddy(&ordinary, &to, result, &mut body);
        self.frames.pop().expect("the extern frame just pushed");
        self.fill(
            id,
            Function {
                name: self.labels[id].clone(),
                params,
                body: body.seal(Terminator {
                    span: Span::default(),
                    kind: End::Ret(result),
                }),
                span: Span::default(),
            },
        );
        if recursive {
            self.extern_adapters
                .pop()
                .expect("the ordinary extern adapter just completed");
        }
        id
    }

    /// The least evidence a host-facing callback and the callbacks it returns
    /// can ask for. Walking the semantic result spine (rather than the written
    /// ABI leaves) sees through aliases; remembering alias applications keeps
    /// regular recursive callback types finite.
    fn callback_evidence(&self, ty: &Rc<Ty>, available: &Row) -> Row {
        let mut required = Row::closed();
        let mut cursor = ty.clone();
        let mut seen = Vec::new();
        let mut variable = false;
        loop {
            if matches!(&*cursor, Ty::Named { .. }) {
                if seen.iter().any(|prior| same_finite_syntax(prior, &cursor)) {
                    break;
                }
                seen.push(cursor.clone());
            }
            let exposed = unfold(&self.inference.aliases, &cursor);
            let Ty::Arrow(_, to, row) = &*exposed else {
                break;
            };
            let row = flat(row);
            for (name, field) in &row.labels {
                if !possible(&field.presence) {
                    continue;
                }
                match required.labels.entry(name.clone()) {
                    indexmap::map::Entry::Vacant(entry) => {
                        entry.insert(field.clone());
                    }
                    indexmap::map::Entry::Occupied(mut entry) => {
                        // The callback may perform the same effect at several
                        // returned arrows. Its requirement is their union: a
                        // definite occurrence dominates either traversal
                        // order, while conditional occurrences remain one
                        // possible bundle entry (their particular formula is
                        // immaterial to the evidence representation).
                        if definite(&field.presence) {
                            entry.get_mut().presence = Presence::Present;
                        }
                    }
                }
            }
            // Conditional labels need a bundle, but they do not make the
            // callback effect-polymorphic. Only a semantic row tail may inherit
            // the containing arrow's tail identity.
            variable |= !matches!(row.rest, Rest::Closed);
            cursor = to.clone();
        }
        if variable {
            // Preserve the containing row's tail identity when it has one. A
            // callback with conditional labels already gives `required` an
            // anonymous variable part of its own.
            if tail_key(available).is_some() {
                required.rest = available.rest.clone();
            }
        }
        required
    }

    /// Convert a Ruddy closure passed to the host into the ABI written at that
    /// direct boundary position. These wrappers are called by the host, so
    /// their parameter list deliberately has no Ruddy evidence slots.
    fn ruddy_to_host(
        &mut self,
        abi: &crate::ir::ExternType,
        ty: &Rc<Ty>,
        value: Temp,
        available: &Row,
        body: &mut Body,
    ) -> Temp {
        use crate::ir::ExternTypeKind;
        match &abi.tracked {
            ExternTypeKind::Group(inner) => self.ruddy_to_host(inner, ty, value, available, body),
            ExternTypeKind::Function {
                parameters, result, ..
            } => {
                let required = self.callback_evidence(ty, available);
                let evidence = self.evidence_args(&required, available, body);
                let id =
                    self.marked_callback(ty.clone(), parameters.clone(), *result.clone(), required);
                let mut captures = vec![value];
                captures.extend(evidence);
                self.emit(body, abi.span, Rep::Fn, Op::Closure { func: id, captures })
            }
            ExternTypeKind::Ordinary(_) if self.rep(ty) == Rep::Fn => {
                let required = self.callback_evidence(ty, available);
                let evidence = self.evidence_args(&required, available, body);
                let id = self.ordinary_callback(ty.clone(), required);
                let mut captures = vec![value];
                captures.extend(evidence);
                self.emit(body, abi.span, Rep::Fn, Op::Closure { func: id, captures })
            }
            ExternTypeKind::Ordinary(_) => value,
        }
    }

    fn ordinary_callback(&mut self, ty: Rc<Ty>, available: Row) -> FuncId {
        if matches!(&*ty, Ty::Named { .. })
            && let Some(adapter) = self.callback_adapters.iter().rev().find(|adapter| {
                same_finite_syntax(&adapter.ty, &ty)
                    && same_row_syntax(&adapter.available, &available)
            })
        {
            return adapter.id;
        }
        let id = self.slot(format!("{}#callback#{}", self.stem, self.serial));
        self.serial += 1;
        let recursive = matches!(&*ty, Ty::Named { .. });
        if recursive {
            self.callback_adapters.push(OrdinaryCallbackAdapter {
                ty: ty.clone(),
                available: available.clone(),
                id,
            });
        }
        let closure = self.fresh(Rep::Fn);
        let (from, to, row) = self.arrow(&ty);
        let raw_argument = self.fresh(self.rep(&from));
        let mut params = vec![Param {
            temp: closure,
            rep: Rep::Fn,
        }];
        self.frames.push(Frame::default());
        self.evidence_params(&available, &mut params);
        params.push(Param {
            temp: raw_argument,
            rep: self.rep(&from),
        });
        let ordinary = Span::default().track(crate::ir::ExternTypeKind::Ordinary(
            Span::default().track(crate::ir::TypeKind::Error),
        ));
        let mut body = Body::default();
        let argument = self.host_to_ruddy(&ordinary, &from, raw_argument, &mut body);
        let mut args = self.evidence_args(&row, &row, &mut body);
        args.push(argument);
        let result = self.emit(
            &mut body,
            Span::default(),
            self.rep(&to),
            Op::Call {
                callee: Callee::Indirect(closure),
                args,
            },
        );
        let result = self.ruddy_to_host(&ordinary, &to, result, &available, &mut body);
        self.frames.pop().expect("the callback frame just pushed");
        self.fill(
            id,
            Function {
                name: self.labels[id].clone(),
                params,
                body: body.seal(Terminator {
                    span: Span::default(),
                    kind: End::Ret(result),
                }),
                span: Span::default(),
            },
        );
        if recursive {
            self.callback_adapters
                .pop()
                .expect("the ordinary callback adapter just completed");
        }
        id
    }

    fn marked_callback(
        &mut self,
        ty: Rc<Ty>,
        parameter_abis: Vec<crate::ir::ExternType>,
        result_abi: crate::ir::ExternType,
        available: Row,
    ) -> FuncId {
        let id = self.slot(format!("{}#callback#{}", self.stem, self.serial));
        self.serial += 1;
        let closure = self.fresh(Rep::Fn);
        let mut params = vec![Param {
            temp: closure,
            rep: Rep::Fn,
        }];
        self.frames.push(Frame::default());
        self.evidence_params(&available, &mut params);
        let mut raw_parameters = Vec::new();
        let mut arrows = Vec::new();
        let mut cursor = ty;
        for _ in 0..parameter_abis.len().max(1) {
            let (from, to, row) = self.arrow(&cursor);
            if !parameter_abis.is_empty() {
                let temp = self.fresh(self.rep(&from));
                params.push(Param {
                    temp,
                    rep: self.rep(&from),
                });
                raw_parameters.push(temp);
            }
            arrows.push((from, to.clone(), row));
            cursor = to;
        }

        let mut body = Body::default();
        let mut current = closure;
        if parameter_abis.is_empty() {
            let unit = self.emit(
                &mut body,
                Span::default(),
                Rep::Unit,
                Op::Struct(IndexMap::new()),
            );
            let mut args = self.evidence_args(&arrows[0].2, &arrows[0].2, &mut body);
            args.push(unit);
            current = self.emit(
                &mut body,
                Span::default(),
                self.rep(&cursor),
                Op::Call {
                    callee: Callee::Indirect(current),
                    args,
                },
            );
        } else {
            for ((abi, raw), (from, to, row)) in
                parameter_abis.iter().zip(raw_parameters).zip(arrows.iter())
            {
                let argument = self.host_to_ruddy(abi, from, raw, &mut body);
                let mut args = self.evidence_args(row, row, &mut body);
                args.push(argument);
                current = self.emit(
                    &mut body,
                    Span::default(),
                    self.rep(to),
                    Op::Call {
                        callee: Callee::Indirect(current),
                        args,
                    },
                );
            }
        }
        let result = self.ruddy_to_host(&result_abi, &cursor, current, &available, &mut body);
        self.frames.pop().expect("the callback frame just pushed");
        self.fill(
            id,
            Function {
                name: self.labels[id].clone(),
                params,
                body: body.seal(Terminator {
                    span: Span::default(),
                    kind: End::Ret(result),
                }),
                span: Span::default(),
            },
        );
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn marked_extern_level(
        &mut self,
        _outer_raw: Temp,
        carried: Vec<Rep>,
        carried_types: Vec<Rc<Ty>>,
        ty: Rc<Ty>,
        arity: usize,
        nullary: bool,
        parameter_abis: Vec<crate::ir::ExternType>,
        result_abi: crate::ir::ExternType,
        step: usize,
    ) -> FuncId {
        let id = self.slot(format!("{}#{}", self.stem, self.serial));
        self.serial += 1;
        let raw = self.fresh(Rep::Fn);
        let mut params = vec![Param {
            temp: raw,
            rep: Rep::Fn,
        }];
        let mut gathered = Vec::with_capacity(carried.len());
        for rep in &carried {
            let temp = self.fresh(*rep);
            params.push(Param { temp, rep: *rep });
            gathered.push(temp);
        }
        let (from, to, row) = self.arrow(&ty);
        self.frames.push(Frame::default());
        self.evidence_params(&row, &mut params);
        let argument_rep = self.rep(&from);
        let argument = self.fresh(argument_rep);
        params.push(Param {
            temp: argument,
            rep: argument_rep,
        });
        let mut body = Body::default();
        if !nullary {
            gathered.push(argument);
        }

        let value = if step + 1 == arity {
            let gathered = gathered
                .into_iter()
                .zip(carried_types.iter().chain(std::iter::once(&from)))
                .zip(parameter_abis.iter())
                .map(|((argument, ty), abi)| self.ruddy_to_host(abi, ty, argument, &row, &mut body))
                .collect();
            let result = self.emit(
                &mut body,
                Span::default(),
                self.rep(&to),
                Op::RawCall {
                    callee: raw,
                    args: gathered,
                },
            );
            self.host_to_ruddy(&result_abi, &to, result, &mut body)
        } else {
            let mut next_carried = carried;
            next_carried.push(argument_rep);
            let mut next_types = carried_types;
            next_types.push(from);
            let next = self.marked_extern_level(
                raw,
                next_carried,
                next_types,
                to.clone(),
                arity,
                false,
                parameter_abis,
                result_abi,
                step + 1,
            );
            let mut captures = vec![raw];
            captures.extend(gathered);
            self.emit(
                &mut body,
                Span::default(),
                Rep::Fn,
                Op::Closure {
                    func: next,
                    captures,
                },
            )
        };
        self.frames.pop().expect("the extern frame just pushed");
        self.fill(
            id,
            Function {
                name: self.labels[id].clone(),
                params,
                body: body.seal(Terminator {
                    span: Span::default(),
                    kind: End::Ret(value),
                }),
                span: Span::default(),
            },
        );
        id
    }

    /// The definitions in the order a backend initializes them: group order,
    /// earliest first, and source order within a group.
    fn order(&self) -> Vec<Symbol> {
        // Every definition is in exactly one group — grouping is worked out
        // from the very map being read here — so this is a reordering rather
        // than a filter, and nothing can fall out of it.
        self.program
            .groups
            .iter()
            .flat_map(|group| group.members.iter().copied())
            .collect()
    }

    /// Give every definition whose value is a nest of `fn`s its function slots
    /// before any body is built. A definition may call itself, or its
    /// neighbour, so the identity of the function it calls has to exist before
    /// the call does.
    fn reserve(&mut self, order: &[Symbol]) {
        let program = self.program;
        for symbol in order {
            let decl = &program.terms[symbol];
            let (fns, _) = nest(&decl.value);
            if fns.is_empty() {
                continue;
            }
            let levels: Vec<Level> = fns
                .iter()
                .map(|(node, _)| {
                    let (from, to, row) = self.arrow(&node.ty);
                    Level {
                        evidence: shape(&row).arity(),
                        from,
                        to,
                        row,
                    }
                })
                .collect();
            let name = self.mint.name(*symbol).to_string();
            let lifted = self.slot(name.clone());
            let wrappers = (1..=levels.len())
                .map(|step| self.slot(format!("{name}#{step}")))
                .collect();
            self.known.insert(
                *symbol,
                Known {
                    lifted,
                    wrappers,
                    levels,
                    ty: decl.value.ty.clone(),
                },
            );
        }
    }

    /// Reserve one function slot under a name, so that a call to it can be
    /// written before its body is.
    fn slot(&mut self, name: String) -> FuncId {
        self.functions.push(None);
        self.labels.push(name);
        self.functions.len() - 1
    }

    fn fill(&mut self, id: FuncId, function: Function) {
        self.functions[id] = Some(function);
    }

    /// A fresh temp, held the given way.
    fn fresh(&mut self, rep: Rep) -> Temp {
        let temp = self.temps;
        self.temps += 1;
        self.reps.push(rep);
        self.held.push(None);
        temp
    }

    /// Record the type whose shape a temp holds. A function value is the one
    /// thing a caller must match; containers are [`Lower::contain`]'s to note.
    fn hold(&mut self, temp: Temp, ty: &Rc<Ty>) {
        if self.rep(ty) == Rep::Fn {
            self.held[temp as usize] = Some(ty.clone());
        }
    }

    /// Record the type a container was built at. A struct or sum may carry
    /// function values, each stored at the shape the container's own type
    /// gives its member — so a read out of the container has to know that
    /// type, not whatever type a use instantiates the container to.
    fn contain(&mut self, temp: Temp, ty: &Rc<Ty>) {
        if matches!(self.rep(ty), Rep::Struct | Rep::Sum) {
            self.held[temp as usize] = Some(ty.clone());
        }
    }

    /// The type whose shape a temp's value actually holds: what its producer
    /// recorded, or the type the value is being read at where none did — in
    /// which case production and use are the same place and the two agree.
    fn holding(&self, temp: Temp, ty: &Rc<Ty>) -> Rc<Ty> {
        self.held[temp as usize]
            .clone()
            .unwrap_or_else(|| ty.clone())
    }

    /// Emit one value-producing instruction and hand back the temp it assigns.
    fn emit(&mut self, body: &mut Body, span: Span, rep: Rep, op: Op) -> Temp {
        let temp = self.fresh(rep);
        body.push(Instr {
            temp,
            rep,
            span,
            op,
        });
        temp
    }

    /// Build one child block, ending it by yielding whatever the walk came to —
    /// unless a `raise` inside it has already ended it another way.
    fn child(&mut self, span: Span, build: impl FnOnce(&mut Self, &mut Body) -> Temp) -> Block {
        let mut body = Body::default();
        let value = build(self, &mut body);
        body.seal(Terminator {
            span,
            kind: End::Yield(value),
        })
    }

    fn top(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("a term is only lowered inside a function or a global")
    }

    /// How a value of this type is held. The one place a [`Ty`] becomes a
    /// [`Rep`], and the last place a type is read at all.
    ///
    /// A declared name is looked through first: what a value of `Boxed` is held
    /// as is what a value of what `Boxed` stands for is held as. Everything a
    /// scheme quantified, everything an annotation made rigid, and everything
    /// nothing decided is [`Rep::Any`] — monomorphization is deferred, so there
    /// is no narrower answer to give.
    fn rep(&self, ty: &Rc<Ty>) -> Rep {
        let ty = unfold(&self.inference.aliases, ty);
        match &*ty {
            Ty::Nat => Rep::Nat,
            Ty::Int => Rep::Int,
            Ty::Real => Rep::Real,
            Ty::String => Rep::String,
            Ty::Boolean => Rep::Boolean,
            Ty::Arrow(..) => Rep::Fn,
            Ty::Sum(_) => Rep::Sum,
            Ty::Struct(row) => {
                let row = flat(row);
                if row
                    .labels
                    .values()
                    .all(|field| matches!(field.presence, Presence::Absent))
                    && matches!(row.rest, Rest::Closed)
                {
                    Rep::Unit
                } else {
                    Rep::Struct
                }
            }
            _ => Rep::Any,
        }
    }

    /// The three halves of an arrow, with a declared name looked through and the
    /// effect row flattened.
    ///
    /// Every position that asks holds a term inference gave an arrow to: a `fn`
    /// is built as one, an operation's type is one, a value handed to a
    /// parameter has the type that parameter was declared with, and a callee
    /// that is none of those is a program inference refused — which R2 says
    /// this pass is never handed.
    fn arrow(&self, ty: &Rc<Ty>) -> (Rc<Ty>, Rc<Ty>, Row) {
        let ty = unfold(&self.inference.aliases, ty);
        let Ty::Arrow(from, to, row) = &*ty else {
            panic!("LIR runs only on programs with no errors");
        };
        (from.clone(), to.clone(), flat(row))
    }

    /// The type reached by walking `steps` arrows down from `ty`, which is what
    /// a value applied that many times short of its arity stands for.
    fn walked(&self, ty: &Rc<Ty>, steps: usize) -> Rc<Ty> {
        let mut ty = ty.clone();
        for _ in 0..steps {
            let (_, to, _) = self.arrow(&ty);
            ty = to;
        }
        ty
    }

    /// What a container's type says one of its members holds: one field of a
    /// struct, or the payload of one sum case. `None` where the type does not
    /// name the member — a use may dispatch on a case the production type
    /// never listed — or does not pin its shape down, in which case the use
    /// site's own reading is all there is to go on.
    fn member_of(&self, ty: &Rc<Ty>, name: &str) -> Option<Rc<Ty>> {
        let ty = unfold(&self.inference.aliases, ty);
        let member = match &*ty {
            Ty::Sum(row) | Ty::Struct(row) => {
                flat(row).labels.get(name).map(|field| field.ty.clone())
            }
            _ => None,
        };
        member.filter(|member| self.rep(member) != Rep::Any)
    }

    /// Whether a value whose evidence was built for `have` can be called where
    /// `want` was declared: the same evidence, in the same order, at every level
    /// a call walks down.
    ///
    /// A side neither type pins down to a function is a side with no shape to
    /// compare — a quantified variable, a rigid, or a type nothing decided —
    /// and there is nothing to build an adapter out of either. Both sides are
    /// asked, not just the declaration: a value reaches a call through a
    /// position its own type left open as often as a declaration does, and the
    /// walk down the levels reaches such a position even where the two types
    /// started out as arrows. Where a side is open, lowering passes the value
    /// through as it stands, which is the same assumption the whole pass makes
    /// of every polymorphic position: what a scheme quantified is carried, not
    /// repacked.
    ///
    /// Iterative evidence-shape comparison. Recursive declarations are regular
    /// trees: revisiting the same pair of alias applications after inspecting
    /// an arrow is a guarded back edge, and therefore a successful end to this
    /// path rather than another recursive Rust call.
    fn fits(&self, want: &Rc<Ty>, have: &Rc<Ty>, cache: &mut FitsCache) -> bool {
        let mut want = want.clone();
        let mut have = have.clone();
        let mut direct = Vec::new();
        let mut aliases = AliasPairs::new();

        let answer = loop {
            let direct_key = (Rc::as_ptr(&want) as usize, Rc::as_ptr(&have) as usize);
            if !matches!((&*want, &*have), (Ty::Named { .. }, Ty::Named { .. })) {
                if let Some((_, _, answer)) = cache.direct.get(&direct_key) {
                    break *answer;
                }
                // Retain the nodes as well as their addresses. Alias opening
                // may allocate a finite arrow prefix; without these owners a
                // later allocation could reuse an address during this fit.
                direct.push((direct_key, want.clone(), have.clone()));
            }

            if let Some((want_symbol, have_symbol)) = alias_symbols(&want, &have) {
                let states = aliases.entry((want_symbol, have_symbol)).or_default();
                if states.iter().any(|(seen_want, seen_have)| {
                    same_alias_pair(seen_want, seen_have, &want, &have)
                }) {
                    break true;
                }
                states.push((want.clone(), have.clone()));
            }

            if self.rep(&want) != Rep::Fn || self.rep(&have) != Rep::Fn {
                break true;
            }
            let (_, want_to, want_row) = self.arrow(&want);
            let (_, have_to, have_row) = self.arrow(&have);
            if shape(&want_row) != shape(&have_row) {
                break false;
            }
            want = want_to;
            have = have_to;
        };

        for (key, want, have) in direct {
            cache.direct.insert(key, (want, have, answer));
        }
        answer
    }

    /// Lower one top-level definition: its functions, and the global that gives
    /// its value.
    fn define(&mut self, symbol: Symbol) {
        self.definition = Some(symbol);
        let program = self.program;
        let decl = &program.terms[&symbol];
        let name = self.mint.name(symbol).to_string();
        self.stem = name.clone();
        self.serial = self.known.get(&symbol).map_or(0, |known| {
            // The lifted function takes the bare name and each wrapper one
            // number, so anything lifted out of the body carries on from there.
            known.wrappers.len() as u32
        });

        let body = match self.known.get(&symbol).cloned() {
            Some(known) => {
                self.uncurried(&known, &decl.value);
                self.curried(&known);
                // Referencing the definition as a value is the outermost
                // wrapper's closure, so a name and a call reach the same
                // function by two routes that cannot disagree.
                let mut body = Body::default();
                let temp = self.emit(
                    &mut body,
                    decl.value.span,
                    Rep::Fn,
                    Op::Closure {
                        func: known.wrappers[0],
                        captures: Vec::new(),
                    },
                );
                body.seal(Terminator {
                    span: decl.value.span,
                    kind: End::Ret(temp),
                })
            }
            None => {
                self.frames.push(Frame::default());
                let mut body = Body::default();
                let temp = self.term(&decl.value, &mut body);
                self.frames.pop();
                body.seal(Terminator {
                    span: decl.value.span,
                    kind: End::Ret(temp),
                })
            }
        };
        self.globals.push(Global {
            symbol,
            name,
            body,
            span: decl.name_span,
        });
    }

    /// The n-ary function a nest of `fn`s becomes: every level's evidence and
    /// argument in one parameter list, and the innermost body as the whole of
    /// what it does.
    fn uncurried(&mut self, known: &Known, value: &Term) {
        let (fns, inner) = nest(value);
        self.frames.push(Frame::default());
        let mut params = Vec::new();
        for (level, (_, arg)) in known.levels.iter().zip(&fns) {
            self.evidence_params(&level.row, &mut params);
            let rep = self.rep(&level.from);
            let temp = self.fresh(rep);
            self.hold(temp, &level.from);
            self.top().locals.insert(*arg, temp);
            params.push(Param { temp, rep });
        }
        let mut body = Body::default();
        let temp = self.term(inner, &mut body);
        let frame = self.frames.pop().expect("the frame just pushed");
        let body = body.seal(Terminator {
            span: inner.span,
            kind: End::Ret(temp),
        });
        // A top-level `fn` closes over nothing but top-level definitions, which
        // are `global` reads rather than captures — so this is empty, and is
        // written out anyway rather than asserted about.
        let params = Self::with_captures(&frame, params);
        self.fill(
            known.lifted,
            Function {
                name: self.labels[known.lifted].clone(),
                params,
                body,
                span: value.span,
            },
        );
    }

    /// The unary wrappers that let an n-ary function be applied one argument at
    /// a time.
    ///
    /// Each takes the arguments and evidence gathered so far as captures, its own
    /// level's evidence and argument as parameters, and hands the lot to the next
    /// wrapper — or, at the last level, to the uncurried function itself. So the
    /// captures of one wrapper are exactly as many values, held exactly as, the
    /// parameters of the one before it, which is what lets a partial application
    /// at any depth build the right closure without knowing how the chain is
    /// wired.
    ///
    /// What travels from one step to the next is the representations, not the
    /// temps: a temp is assigned once in the whole listing, so the wrapper that
    /// receives a captured value names it with a temp of its own, exactly as
    /// [`Lower::thread`] does for the captures of a lifted `fn`.
    fn curried(&mut self, known: &Known) {
        let last = known.levels.len() - 1;
        let mut carried: Vec<Rep> = Vec::new();
        for (step, level) in known.levels.iter().enumerate() {
            let mut params: Vec<Param> = Vec::new();
            for rep in &carried {
                let temp = self.fresh(*rep);
                params.push(Param { temp, rep: *rep });
            }
            for _ in 0..level.evidence {
                let temp = self.fresh(Rep::Struct);
                params.push(Param {
                    temp,
                    rep: Rep::Struct,
                });
            }
            let rep = self.rep(&level.from);
            let temp = self.fresh(rep);
            params.push(Param { temp, rep });

            let args: Vec<Temp> = params.iter().map(|param| param.temp).collect();
            let mut body = Body::default();
            let (rep, op) = match step == last {
                true => (
                    self.rep(&level.to),
                    Op::Call {
                        callee: Callee::Direct(known.lifted),
                        args,
                    },
                ),
                false => (
                    Rep::Fn,
                    Op::Closure {
                        func: known.wrappers[step + 1],
                        captures: args,
                    },
                ),
            };
            let value = self.emit(&mut body, Span::default(), rep, op);
            let body = body.seal(Terminator {
                span: Span::default(),
                kind: End::Ret(value),
            });
            let id = known.wrappers[step];
            carried = params.iter().map(|param| param.rep).collect();
            self.fill(
                id,
                Function {
                    name: self.labels[id].clone(),
                    params,
                    body,
                    span: Span::default(),
                },
            );
        }
    }

    /// A parameter list with the function's captures put in front of it, which
    /// is where a lifted function takes them.
    fn with_captures(frame: &Frame, params: Vec<Param>) -> Vec<Param> {
        frame
            .captures
            .iter()
            .map(|capture| Param {
                temp: capture.inner,
                rep: capture.rep,
            })
            .chain(params)
            .collect()
    }

    /// The next name for something lifted out of the definition being lowered.
    fn lifted_name(&mut self) -> String {
        self.serial += 1;
        format!("{}#{}", self.stem, self.serial)
    }

    /// Declare one arrow's hidden evidence parameters into the frame being
    /// built: one per definitely performed effect, in row order, then the tail
    /// bundle where the row has a variable part.
    fn evidence_params(&mut self, row: &Row, params: &mut Vec<Param>) {
        for name in shape(row).names {
            let temp = self.fresh(Rep::Struct);
            params.push(Param {
                temp,
                rep: Rep::Struct,
            });
            self.top().evidence.insert(name, temp);
        }
        if let Some(key) = tail_key(row) {
            let temp = self.fresh(Rep::Struct);
            params.push(Param {
                temp,
                rep: Rep::Struct,
            });
            self.top().tails.push((key, temp));
        }
    }

    /// The evidence one call site has to hand a callee: what is in scope for
    /// each definitely performed effect, and a bundle for the variable part.
    ///
    /// Two rows decide it. The callee's parameter list was built from the row it
    /// was *declared* with, so that row says which records go across and whether
    /// there is a tail parameter at all. What goes *in* the tail is the row at
    /// this *use*: a row variable is only the caller's own where the use says so,
    /// since a variable's index is its place in its own scheme and two unrelated
    /// schemes both quantify a first one. Where the use has no variable part to
    /// share, [`RestKey::Open`] stands for it and never forwards, so the bundle
    /// is built here out of what the scope can handle.
    fn evidence_args(&mut self, declared: &Row, used: &Row, body: &mut Body) -> Vec<Temp> {
        let mut args = Vec::new();
        for name in shape(declared).names {
            args.push(self.evidence_of(&name));
        }
        if tail_key(declared).is_some() {
            let key = tail_key(used).unwrap_or(RestKey::Open);
            // Conditional labels live in the bundle even when a genuine row
            // tail follows them. A shared tail cannot simply be forwarded in
            // that case: a use may have promoted one of those labels to a
            // definite evidence parameter, so overlay its record on the tail.
            // Conversely, do not rebuild the whole bundle from ambient named
            // evidence, which would leak unrelated effects into the callback.
            let conditional: Vec<String> = declared
                .labels
                .iter()
                .filter(|(_, field)| possible(&field.presence) && !definite(&field.presence))
                .map(|(name, _)| name.clone())
                .collect();
            if conditional.is_empty() {
                args.push(self.bundle(key, None, true, body));
            } else {
                let needs_tails = conditional.iter().any(|name| {
                    !used
                        .labels
                        .get(name)
                        .is_some_and(|field| definite(&field.presence))
                });
                if matches!(declared.rest, Rest::Closed) {
                    args.push(self.bundle(key, Some(&conditional), needs_tails, body));
                } else {
                    // Thread the shared tail first. Building the overlay may
                    // project from that same capture, and must refer to the
                    // already-established inner temp rather than introduce a
                    // later capture before its declaration.
                    let tail = self.bundle(key, None, true, body);
                    let named = self.bundle(key, Some(&conditional), needs_tails, body);
                    args.push(self.emit(
                        body,
                        Span::default(),
                        Rep::Struct,
                        Op::Merge(vec![tail, named]),
                    ));
                }
            }
        }
        args
    }

    /// One visible argument, fitted to the evidence the position it goes into
    /// was compiled to hand over.
    ///
    /// A function takes the evidence of the row it was *written* with, and the
    /// call passes the evidence of the row the position it fills was
    /// *declared* with. Those name the same effects and need not put them the
    /// same way round: handing a function that performs `Log` to a parameter
    /// polymorphic in its effects makes that function's own record one entry of
    /// a bundle, and handing an effect-polymorphic function to a parameter that
    /// names `Log` does the reverse. Where the two shapes differ an adapter
    /// stands between them, taking what the declaration passes, rebuilding out
    /// of it what the value expects, and calling the value — which is code
    /// nobody wrote, so all of it is generated.
    fn fitted(&mut self, want: &Rc<Ty>, have: &Rc<Ty>, temp: Temp, body: &mut Body) -> Temp {
        let mut cache = FitsCache::default();
        let mut contexts: Vec<FittedContext> = Vec::new();
        let mut adapters = AdapterAliases::new();
        let mut root = std::mem::take(body);
        let mut request = Some((want.clone(), have.clone(), temp));
        let mut result = None;

        loop {
            if let Some((want, have, temp)) = request.take() {
                if self.fits(&want, &have, &mut cache) {
                    self.hold(temp, &want);
                    result = Some(temp);
                    continue;
                }

                // Re-entering a guarded recursive pair closes over the newly
                // returned function with the adapter already being built. The
                // generated function is a reusable schema: only that capture
                // changes at each trip around the regular tree.
                let reused = alias_symbols(&want, &have).and_then(|symbols| {
                    adapters.get(&symbols).and_then(|states| {
                        states.iter().find_map(|(seen_want, seen_have, id)| {
                            same_alias_pair(seen_want, seen_have, &want, &have).then_some(*id)
                        })
                    })
                });
                if let Some(id) = reused {
                    let current = contexts
                        .last_mut()
                        .map(|context| &mut context.body)
                        .unwrap_or(&mut root);
                    let adapted = self.emit(
                        current,
                        Span::default(),
                        Rep::Fn,
                        Op::Closure {
                            func: id,
                            captures: vec![temp],
                        },
                    );
                    self.hold(adapted, &want);
                    result = Some(adapted);
                    continue;
                }

                let (want_from, want_to, want_row) = self.arrow(&want);
                let (have_from, have_to, have_row) = self.arrow(&have);
                let want_shape = shape(&want_row);
                let have_shape = shape(&have_row);

                self.frames.push(Frame::default());
                let wrapped = self.thread(self.frames.len() - 2, temp);
                let mut params = Vec::new();
                let mut records: IndexMap<String, Temp> = IndexMap::new();
                for name in &want_shape.names {
                    let record = self.fresh(Rep::Struct);
                    params.push(Param {
                        temp: record,
                        rep: Rep::Struct,
                    });
                    records.insert(name.clone(), record);
                }
                let mut carried = None;
                if want_shape.tail {
                    let bundle = self.fresh(Rep::Struct);
                    params.push(Param {
                        temp: bundle,
                        rep: Rep::Struct,
                    });
                    carried = Some(bundle);
                }
                let rep = self.rep(&want_from);
                let arg = self.fresh(rep);
                params.push(Param { temp: arg, rep });

                let mut lifted = Body::default();
                let mut args = Vec::new();
                for name in &have_shape.names {
                    let record = match records.get(name) {
                        Some(record) => *record,
                        None => {
                            let bundle = carried.expect(
                                "the position a value fills accounts for every effect it performs",
                            );
                            self.emit(
                                &mut lifted,
                                Span::default(),
                                Rep::Struct,
                                Op::Project {
                                    base: bundle,
                                    field: FieldKey::named(name.clone()),
                                },
                            )
                        }
                    };
                    args.push(record);
                }
                if have_shape.tail {
                    let bundle = match (carried, records.is_empty()) {
                        (Some(bundle), true) => bundle,
                        (Some(bundle), false) => {
                            let named = self.emit(
                                &mut lifted,
                                Span::default(),
                                Rep::Struct,
                                Op::Struct(named_fields(records)),
                            );
                            self.emit(
                                &mut lifted,
                                Span::default(),
                                Rep::Struct,
                                Op::Merge(vec![bundle, named]),
                            )
                        }
                        (None, _) => self.emit(
                            &mut lifted,
                            Span::default(),
                            Rep::Struct,
                            Op::Struct(named_fields(records)),
                        ),
                    };
                    args.push(bundle);
                }

                // Recursive alias adapters need their slot before their result
                // is fitted, because that result may close over this very
                // function. Ordinary adapters retain the established postorder
                // numbering used by listings and the debugger.
                let reserved = alias_symbols(&want, &have).map(|symbols| {
                    let name = self.lifted_name();
                    let id = self.slot(name.clone());
                    adapters
                        .entry(symbols)
                        .or_default()
                        .push((want.clone(), have.clone(), id));
                    (id, name)
                });
                contexts.push(FittedContext {
                    want,
                    wrapped,
                    params,
                    body: lifted,
                    stage: FittedStage::Argument {
                        args,
                        have_to,
                        want_to,
                    },
                    reserved,
                });
                request = Some((have_from, want_from, arg));
                continue;
            }

            let value = result.take().expect("a fitted request produced a value");
            let Some(context) = contexts.last_mut() else {
                *body = root;
                return value;
            };
            match &mut context.stage {
                FittedStage::Argument {
                    args,
                    have_to,
                    want_to,
                } => {
                    args.push(value);
                    let called = self.emit(
                        &mut context.body,
                        Span::default(),
                        self.rep(have_to),
                        Op::Call {
                            callee: Callee::Indirect(context.wrapped),
                            args: std::mem::take(args),
                        },
                    );
                    let next_want = want_to.clone();
                    let next_have = have_to.clone();
                    context.stage = FittedStage::Result;
                    request = Some((next_want, next_have, called));
                }
                FittedStage::Result => {
                    let context = contexts.pop().expect("the current fitted adapter");
                    let lifted = context.body.seal(Terminator {
                        span: Span::default(),
                        kind: End::Ret(value),
                    });
                    let frame = self.frames.pop().expect("the frame just pushed");
                    let (id, name) = context.reserved.unwrap_or_else(|| {
                        let name = self.lifted_name();
                        let id = self.slot(name.clone());
                        (id, name)
                    });
                    let captures: Vec<Temp> =
                        frame.captures.iter().map(|capture| capture.outer).collect();
                    let params = Self::with_captures(&frame, context.params);
                    self.fill(
                        id,
                        Function {
                            name,
                            params,
                            body: lifted,
                            span: Span::default(),
                        },
                    );
                    let parent = contexts
                        .last_mut()
                        .map(|context| &mut context.body)
                        .unwrap_or(&mut root);
                    let adapted = self.emit(
                        parent,
                        Span::default(),
                        Rep::Fn,
                        Op::Closure { func: id, captures },
                    );
                    self.hold(adapted, &context.want);
                    result = Some(adapted);
                }
            }
        }
    }

    /// The record of one effect's operations, as this scope has it — captured
    /// into the function being built if it came from further out.
    ///
    /// Never missing: an effect performed where nothing can handle it is what
    /// inference calls an unhandled effect, and this pass runs only on programs
    /// it accepted.
    fn evidence_of(&mut self, name: &str) -> Temp {
        let at = self
            .frames
            .iter()
            .rposition(|frame| frame.evidence.contains_key(name))
            .expect("inference refuses an effect nothing in scope can handle");
        let temp = self.frames[at].evidence[name];
        self.thread(at, temp)
    }

    /// The bundle standing in for a row's variable part: the caller's own, when
    /// caller and callee share the very same variable part, and otherwise a
    /// fresh record of everything the scope can hand over.
    ///
    /// What the scope can hand over is two things, not one. The effects it names
    /// outright are a record built here. The effects it holds only inside a
    /// bundle of its own — because its own row has a variable part, and what
    /// that part stands for is known by name to nobody — cannot be named at all,
    /// so every such bundle is laid underneath. A bundle that shares no identity
    /// with the callee's variable part still carries evidence the callee may
    /// reach for, and a key the callee never reads costs nothing.
    ///
    /// A record built here is evidence plumbing rather than anything the reader
    /// wrote, so it carries no span and the debugger marks it generated.
    fn bundle(
        &mut self,
        key: RestKey,
        only: Option<&[String]>,
        include_tails: bool,
        body: &mut Body,
    ) -> Temp {
        // A restricted conditional bundle is a projection, never a forwarded
        // ambient tail. Rebuild exactly the requested keys: named evidence wins
        // as usual, and a key known only through an opaque tail is projected
        // from the layered tails. Forwarding or merging those tails themselves
        // would expose every unrelated effect they happen to contain.
        if let Some(names) = only {
            let held: Vec<(usize, Temp)> = if include_tails {
                // Prefer the innermost tail, and its matching identity where
                // that frame holds more than one. In particular, callback
                // adapters have an explicit bundle parameter in their current
                // frame; reaching through it to ambient construction-time
                // frames would create undeclared captures and combine evidence
                // outside the callback row.
                self.frames
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(at, frame)| {
                        frame
                            .tails
                            .iter()
                            .rev()
                            .find(|(held, _)| held.forwards(key))
                            .or_else(|| frame.tails.last())
                            .map(|(_, temp)| vec![(at, *temp)])
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let tail_owner = held.iter().map(|(at, _)| *at).max();
            let mut tails: Vec<Temp> = held
                .into_iter()
                .map(|(at, temp)| self.thread(at, temp))
                .collect();
            let tail = match tails.as_slice() {
                [] => None,
                [tail] => Some(*tail),
                _ => Some(self.emit(
                    body,
                    Span::default(),
                    Rep::Struct,
                    Op::Merge(std::mem::take(&mut tails)),
                )),
            };
            let entries: IndexMap<String, Temp> = names
                .iter()
                .map(|name| {
                    let evidence_owner = self
                        .frames
                        .iter()
                        .rposition(|frame| frame.evidence.contains_key(name));
                    let value = if evidence_owner.is_some() && evidence_owner >= tail_owner {
                        self.evidence_of(name)
                    } else {
                        self.emit(
                            body,
                            Span::default(),
                            Rep::Struct,
                            Op::Project {
                                base: tail.expect(
                                    "accepted conditional evidence is named or supplied by a tail",
                                ),
                                field: FieldKey::named(name.clone()),
                            },
                        )
                    };
                    (name.clone(), value)
                })
                .collect();
            return self.emit(
                body,
                Span::default(),
                Rep::Struct,
                Op::Struct(named_fields(entries)),
            );
        }

        let found = include_tails
            .then(|| {
                self.frames
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(at, frame)| {
                        frame
                            .tails
                            .iter()
                            .find(|(held, _)| held.forwards(key))
                            .map(|(_, temp)| (at, *temp))
                    })
            })
            .flatten();
        if let Some((at, temp)) = found {
            return self.thread(at, temp);
        }
        // Outermost first, so the record reads in the order the handlers were
        // written; the innermost frame that has a name still wins, since that is
        // what `evidence_of` answers with. The bundles go in the same order and
        // for the same reason, and under the record: a name the scope has
        // outright is the one to hand over where a bundle holds it too.
        let held: Vec<(usize, Temp)> = if include_tails {
            self.frames
                .iter()
                .enumerate()
                .flat_map(|(at, frame)| frame.tails.iter().map(move |(_, temp)| (at, *temp)))
                .collect()
        } else {
            Vec::new()
        };
        let mut layers: Vec<Temp> = held
            .into_iter()
            .map(|(at, temp)| self.thread(at, temp))
            .collect();
        let mut names: Vec<String> = Vec::new();
        for frame in &self.frames {
            for name in frame.evidence.keys() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        // The record is worth building for its own sake, or to lay over the
        // bundles — but not to lay nothing over them.
        if layers.is_empty() || !names.is_empty() {
            let entries: IndexMap<String, Temp> = names
                .into_iter()
                .map(|name| {
                    let temp = self.evidence_of(&name);
                    (name, temp)
                })
                .collect();
            let record = self.emit(
                body,
                Span::default(),
                Rep::Struct,
                Op::Struct(named_fields(entries)),
            );
            layers.push(record);
        }
        match layers.as_slice() {
            [only] => *only,
            _ => self.emit(body, Span::default(), Rep::Struct, Op::Merge(layers)),
        }
    }

    /// The handler identity a `raise` here unwinds to, captured in if the arm it
    /// is written in has been lifted away from the `handle`.
    fn raise_tag(&mut self) -> Temp {
        let at = self
            .frames
            .iter()
            .rposition(|frame| frame.raise_tag.is_some())
            .expect("lowering refuses a `raise` with no handler arm around it");
        let temp = self.frames[at]
            .raise_tag
            .expect("the frame found is the one that has it");
        self.thread(at, temp)
    }

    /// Carry one temp from the frame that owns it into the frame being built,
    /// adding a capture to every function in between.
    fn thread(&mut self, owner: usize, temp: Temp) -> Temp {
        let mut carried = temp;
        for at in owner + 1..self.frames.len() {
            carried = match self.frames[at].caught.get(&carried).copied() {
                // Captured already: one parameter however often the body uses
                // what it stands for.
                Some(known) => known,
                None => {
                    let rep = self.reps[carried as usize];
                    let shape = self.held[carried as usize].clone();
                    let inner = self.fresh(rep);
                    // A capture is the same value under a new name, so it holds
                    // the very shape the value it stands for does.
                    self.held[inner as usize] = shape;
                    let frame = &mut self.frames[at];
                    frame.caught.insert(carried, inner);
                    frame.captures.push(Capture {
                        outer: carried,
                        inner,
                        rep,
                    });
                    inner
                }
            };
        }
        carried
    }

    /// A local name's temp: the frame that binds it, threaded in.
    ///
    /// A recursive local function has no outer temp to capture while its value
    /// is being built. On its first use, mint a temp in the function's own
    /// frame for a closure over the lifted function and that frame's eventual
    /// capture parameters. The instruction is held in the frame's prologue;
    /// [`Lower::lambda`] fills in the function and captures once body lowering
    /// has discovered both.
    fn local(&mut self, symbol: Symbol) -> Option<Temp> {
        let at = self.frames.iter().rposition(|frame| {
            frame.locals.contains_key(&symbol)
                || frame
                    .recursive
                    .as_ref()
                    .is_some_and(|recursive| recursive.symbol == symbol)
        })?;
        let temp = match self.frames[at].locals.get(&symbol).copied() {
            Some(temp) => temp,
            None => match self.frames[at]
                .recursive
                .as_ref()
                .and_then(|recursive| recursive.temp)
            {
                Some(temp) => temp,
                None => {
                    let ty = self.frames[at]
                        .recursive
                        .as_ref()
                        .expect("the matching frame has a recursive binding")
                        .ty
                        .clone();
                    let temp = self.fresh(Rep::Fn);
                    self.hold(temp, &ty);
                    let frame = &mut self.frames[at];
                    frame
                        .recursive
                        .as_mut()
                        .expect("the matching frame has a recursive binding")
                        .temp = Some(temp);
                    frame.prologue.push(Instr {
                        temp,
                        rep: Rep::Fn,
                        span: Span::default(),
                        // Patched before this frame becomes a [`Function`].
                        op: Op::Closure {
                            func: FuncId::MAX,
                            captures: Vec::new(),
                        },
                    });
                    temp
                }
            },
        };
        Some(self.thread(at, temp))
    }

    /// Lower one term into the block being built, and hand back the temp its
    /// value lands in.
    fn term(&mut self, term: &Term, body: &mut Body) -> Temp {
        let span = term.span;
        let rep = self.rep(&term.ty);
        match &term.kind {
            TermKind::Natural(value) => {
                self.emit(body, span, rep, Op::Const(Literal::Natural(*value)))
            }
            TermKind::Integer(value) => {
                self.emit(body, span, rep, Op::Const(Literal::Integer(*value)))
            }
            TermKind::Real(value) => self.emit(body, span, rep, Op::Const(Literal::Real(*value))),
            TermKind::String(value) => {
                self.emit(body, span, rep, Op::Const(Literal::String(value.clone())))
            }
            TermKind::Boolean(value) => {
                self.emit(body, span, rep, Op::Const(Literal::Boolean(*value)))
            }
            TermKind::Unary { op, value } => {
                let value = self.term(value, body);
                let op = match op {
                    crate::ir::UnaryOp::Neg => Op::Neg(value),
                    crate::ir::UnaryOp::Not => Op::Not(value),
                };
                self.emit(body, span, rep, op)
            }
            TermKind::Binary { op, left, right } => {
                let left = self.term(left, body);
                let right = self.term(right, body);
                let op = match op {
                    crate::ir::BinaryOp::Add => Op::Add { left, right },
                    crate::ir::BinaryOp::Sub => Op::Sub { left, right },
                    crate::ir::BinaryOp::Mul => Op::Mul { left, right },
                    crate::ir::BinaryOp::Div => Op::Div { left, right },
                    crate::ir::BinaryOp::And => Op::And { left, right },
                    crate::ir::BinaryOp::Or => Op::Or { left, right },
                    crate::ir::BinaryOp::Xor => Op::Xor { left, right },
                };
                self.emit(body, span, rep, op)
            }
            // A container honestly contains values shaped by its own type:
            // each member is fitted from the shape it holds to the shape the
            // container's type gives it going in, and the container records
            // that type so a read coming out can trust it.
            TermKind::Struct(fields) => {
                let mut entries = IndexMap::new();
                for (name, field) in fields {
                    let temp = self.term(&field.value, body);
                    let want = self
                        .member_of(&term.ty, name)
                        .unwrap_or_else(|| field.value.ty.clone());
                    let have = self.holding(temp, &field.value.ty);
                    let temp = self.fitted(&want, &have, temp, body);
                    entries.insert(FieldKey::named(name.clone()), temp);
                }
                let temp = self.emit(body, span, rep, Op::Struct(entries));
                self.contain(temp, &term.ty);
                temp
            }
            // The field's shape is the base container's to declare: the type
            // the container was built at says what the stored value holds,
            // however this use instantiates the container, and the read is
            // fitted from that to what this projection stands for.
            TermKind::Project { base, field } => {
                let temp = self.term(base, body);
                let authority = self.holding(temp, &base.ty);
                let have = self
                    .member_of(&authority, &field.tracked)
                    .unwrap_or_else(|| term.ty.clone());
                let read = self.emit(
                    body,
                    span,
                    self.rep(&have),
                    Op::Project {
                        base: temp,
                        field: FieldKey::named(field.tracked.clone()),
                    },
                );
                self.contain(read, &have);
                self.fitted(&term.ty, &have, read, body)
            }
            TermKind::Tag { name, payload } => {
                let payload = payload.as_ref().map(|written| {
                    let temp = self.term(written, body);
                    let want = self
                        .member_of(&term.ty, &name.tracked)
                        .unwrap_or_else(|| written.ty.clone());
                    let have = self.holding(temp, &written.ty);
                    self.fitted(&want, &have, temp, body)
                });
                let temp = self.emit(
                    body,
                    span,
                    rep,
                    Op::Tag {
                        name: name.tracked.clone(),
                        payload,
                    },
                );
                self.contain(temp, &term.ty);
                temp
            }
            // A binding is a name for a temp and nothing else: the value is
            // lowered where it was written, the name maps to it, and the
            // annotation beside it has nothing left to say.
            TermKind::Let {
                name,
                value,
                body: rest,
                ..
            } => {
                // A local function's name is in scope in its own value. Its
                // closure does not exist yet, so [`Lower::lambda`] gives the
                // lifted body a way to reconstruct it instead of letting the
                // name fall through to a bogus global read.
                let temp = match &value.kind {
                    TermKind::Fn { arg, body: inner } => {
                        self.lambda(value, arg.tracked, inner, body, Some(name.tracked))
                    }
                    _ => self.term(value, body),
                };
                self.top().locals.insert(name.tracked, temp);
                self.term(rest, body)
            }
            // A definition is compiled once, against the row it was written
            // with, so what a `global` read hands back is that shape however
            // the use instantiated it. Fitting it here — where the value is
            // produced rather than wherever it is eventually passed — is what
            // makes every temp hold a value shaped by its own term's type, so
            // that a value travelling through a binding or a return arrives
            // callable. A container is not refitted — its contents were built
            // at the definition's type — so the read records that type for
            // whatever is later projected out.
            TermKind::Ident(symbol) => match self.local(*symbol) {
                Some(temp) => temp,
                None => {
                    let temp = self.emit(
                        body,
                        span,
                        rep,
                        Op::Global {
                            symbol: *symbol,
                            name: self.mint.name(*symbol).to_string(),
                        },
                    );
                    // A defined or dependency global was compiled at its
                    // declared type. An extern is supplied directly at this
                    // instantiated use type, since it has no Ruddy initializer
                    // with another shape to preserve.
                    let have = self
                        .program
                        .terms
                        .get(symbol)
                        .map(|decl| decl.value.ty.clone())
                        .or_else(|| {
                            self.program
                                .external_schemes
                                .get(symbol)
                                .map(|scheme| scheme.body().clone())
                        })
                        .unwrap_or_else(|| {
                            debug_assert!(self.program.externs.contains_key(symbol));
                            term.ty.clone()
                        });
                    self.contain(temp, &have);
                    self.fitted(&term.ty, &have, temp, body)
                }
            },
            TermKind::Fn { arg, body: inner } => self.lambda(term, arg.tracked, inner, body, None),
            TermKind::Apply { .. } => self.apply(term, body),
            TermKind::Operation { selector, .. } => {
                self.operation_value(term, Self::operation_slot(&selector.tracked), body)
            }
            TermKind::Match { scrutinee, arms } => self.matched(term, scrutinee, arms, body),
            TermKind::Handle {
                body: handled,
                handler,
            } => self.handle(term, handled, handler, body),
            TermKind::Raise(value) => {
                let temp = self.term(value, body);
                let tag = self.raise_tag();
                body.stop(Terminator {
                    span,
                    kind: End::Throw { tag, value: temp },
                });
                temp
            }
            // R2 keeps this out of reach: a name that did not resolve is one of
            // lowering's own errors, and LIR runs only where there were none.
            TermKind::Error => panic!("LIR runs only on programs with no errors"),
        }
    }

    /// One source `fn`, lifted: a top-level function taking its captures, then
    /// its own arrow's evidence, then its one visible argument, and a `closure`
    /// here pairing the two.
    ///
    /// `recursive` is the name of a directly enclosing local `let`, when there
    /// is one. A use of it is reconstructed in the lifted body's prologue from
    /// this function's identity and inner capture parameters; the outer closure
    /// is still built here from the corresponding outer temps.
    fn lambda(
        &mut self,
        term: &Term,
        arg: Symbol,
        inner: &Term,
        body: &mut Body,
        recursive: Option<Symbol>,
    ) -> Temp {
        let (from, _, row) = self.arrow(&term.ty);

        self.frames.push(Frame {
            recursive: recursive.map(|symbol| Recursive {
                symbol,
                ty: term.ty.clone(),
                temp: None,
            }),
            ..Frame::default()
        });
        let mut params = Vec::new();
        self.evidence_params(&row, &mut params);
        let rep = self.rep(&from);
        let temp = self.fresh(rep);
        self.hold(temp, &from);
        self.top().locals.insert(arg, temp);
        params.push(Param { temp, rep });

        let mut lifted = Body::default();
        let value = self.term(inner, &mut lifted);
        let mut frame = self.frames.pop().expect("the frame just pushed");
        let name = self.lifted_name();
        let id = self.slot(name.clone());
        if frame
            .recursive
            .as_ref()
            .is_some_and(|recursive| recursive.temp.is_some())
        {
            let captures: Vec<Temp> = frame.captures.iter().map(|capture| capture.inner).collect();
            for self_closure in &mut frame.prologue {
                self_closure.op = Op::Closure {
                    func: id,
                    captures: captures.clone(),
                };
            }
        }
        frame.prologue.append(&mut lifted.instrs);
        lifted.instrs = std::mem::take(&mut frame.prologue);
        let lifted = lifted.seal(Terminator {
            span: inner.span,
            kind: End::Ret(value),
        });
        let captures: Vec<Temp> = frame.captures.iter().map(|capture| capture.outer).collect();
        let params = Self::with_captures(&frame, params);
        self.fill(
            id,
            Function {
                name,
                params,
                body: lifted,
                span: term.span,
            },
        );
        let closure = self.emit(body, term.span, Rep::Fn, Op::Closure { func: id, captures });
        // The closure holds the shape of the very type the `fn` was compiled
        // against, wherever a binding or a return carries it from here.
        self.hold(closure, &term.ty);
        closure
    }

    /// An operation used as a value: a wrapper that takes the effect's evidence
    /// and the payload, reads the operation out of the record and calls it.
    ///
    /// Its own evidence parameter is what puts the effect in the wrapper's arrow
    /// row, exactly as the inferred type `From -> To + !E` says it should be.
    fn operation_slot(selector: &crate::ir::OperationSelector) -> FieldKey {
        match selector {
            crate::ir::OperationSelector::Unnamed => FieldKey::UnnamedOperation,
            crate::ir::OperationSelector::Named(name) => FieldKey::named(name.clone()),
        }
    }

    fn operation_value(&mut self, term: &Term, op: FieldKey, body: &mut Body) -> Temp {
        let (from, to, _) = self.arrow(&term.ty);
        let evidence = self.fresh(Rep::Struct);
        let rep = self.rep(&from);
        let arg = self.fresh(rep);
        let mut wrapped = Body::default();
        let held = self.emit(
            &mut wrapped,
            Span::default(),
            Rep::Fn,
            Op::Project {
                base: evidence,
                field: op,
            },
        );
        let value = self.emit(
            &mut wrapped,
            Span::default(),
            self.rep(&to),
            Op::Call {
                callee: Callee::Indirect(held),
                args: vec![arg],
            },
        );
        let wrapped = wrapped.seal(Terminator {
            span: Span::default(),
            kind: End::Ret(value),
        });
        let name = self.lifted_name();
        let id = self.slot(name.clone());
        self.fill(
            id,
            Function {
                name,
                params: vec![
                    Param {
                        temp: evidence,
                        rep: Rep::Struct,
                    },
                    Param { temp: arg, rep },
                ],
                body: wrapped,
                span: Span::default(),
            },
        );
        self.emit(
            body,
            term.span,
            Rep::Fn,
            Op::Closure {
                func: id,
                captures: Vec::new(),
            },
        )
    }

    /// An application spine.
    ///
    /// Three shapes, in order of how much is known. Performing an operation is a
    /// projection out of the evidence record and a call. Applying a definition
    /// whose value is a nest of `fn`s is one direct call of the uncurried
    /// function, taking as many arguments as it has levels, with whatever is left
    /// applied one at a time; short of a full application it is a wrapper
    /// closure. Everything else is an ordinary indirect call per argument.
    fn apply(&mut self, term: &Term, body: &mut Body) -> Temp {
        let (head, applies) = spine(term);
        if let TermKind::Operation { effect, selector } = &head.kind {
            return self.perform(
                head,
                *effect,
                Self::operation_slot(&selector.tracked),
                &applies,
                body,
            );
        }
        if let TermKind::Ident(symbol) = &head.kind
            && let Some(known) = self.known.get(symbol).cloned()
        {
            return self.call_known(&known, &head.ty, &applies, body);
        }
        let callee = self.term(head, body);
        self.spun(callee, &head.ty, &applies, body)
    }

    /// Whatever is left of a spine once the head has taken all it can: one
    /// indirect call per argument, with what each application comes to becoming
    /// the type the next is made at.
    ///
    /// All three shapes end here. An ordinary application takes the whole
    /// spine, performing an operation takes everything past the payload, and a
    /// known chain takes everything past its arity — over-applying a definition
    /// is calling what its last level gave back, which is a value like any
    /// other.
    fn spun(&mut self, head: Temp, ty: &Rc<Ty>, applies: &[Apply], body: &mut Body) -> Temp {
        let mut value = head;
        let mut carrying = ty.clone();
        for apply in applies {
            value = self.indirect(&carrying, value, *apply, body);
            carrying = apply.node.ty.clone();
        }
        value
    }

    /// One indirect call: the argument, the evidence the callee's own arrow asks
    /// for, and then the call — the argument fitted to what that arrow declares
    /// of it.
    ///
    /// The arrow asked is the one the callee value *holds*, not the one this
    /// use instantiates it at: a value reaching the call through a binding, a
    /// parameter or a return was compiled against its own row, and calling it
    /// takes exactly the parameters that row came to. The row at this use still
    /// says what goes in the tail bundle, and the result is fitted back to what
    /// this node stands for, so the temp handed on holds its own term's shape.
    fn indirect(
        &mut self,
        callee_ty: &Rc<Ty>,
        callee: Temp,
        apply: Apply,
        body: &mut Body,
    ) -> Temp {
        // The argument is lowered first, ahead of any evidence: the reader
        // wrote it, and everything else here is this pass's own plumbing.
        let arg = self.term(apply.arg, body);
        let shape = match &self.held[callee as usize] {
            Some(ty) => ty.clone(),
            // No producer recorded a shape, so production and use are the same
            // place and the type at this use is the type the value holds.
            None => callee_ty.clone(),
        };
        let (from, to, declared) = self.arrow(&shape);
        let (_, _, used) = self.arrow(callee_ty);
        let mut args = self.evidence_args(&declared, &used, body);
        args.push(self.fitted(&from, &apply.arg.ty, arg, body));
        let rep = self.rep(&apply.node.ty);
        let value = self.emit(
            body,
            apply.node.span,
            rep,
            Op::Call {
                callee: Callee::Indirect(callee),
                args,
            },
        );
        // Containers, like functions, retain the production type of the
        // callee's result. A later projection must read members at the stored
        // ABI before adapting them to this use's instantiation.
        self.contain(value, &to);
        // What comes back is shaped by the callee's own next level. Where that
        // level pins a function shape down, fit it to what this node stands
        // for; where it does not — an `any` nothing decided — the value is
        // taken as this use reads it, which is all there is to go on.
        match self.rep(&to) == Rep::Fn {
            true => self.fitted(&apply.node.ty, &to, value, body),
            false => value,
        }
    }

    /// Performing an operation: read the implementation the handler passed down
    /// out of the evidence record, and call it.
    fn perform(
        &mut self,
        head: &Term,
        effect: crate::tracking::Tracked<Symbol>,
        op: FieldKey,
        applies: &[Apply],
        body: &mut Body,
    ) -> Temp {
        let name = self.program.effect_ids[&effect.tracked].row_key();
        let record = self.evidence_of(&name);
        let held = self.emit(
            body,
            head.span,
            Rep::Fn,
            Op::Project {
                base: record,
                field: op,
            },
        );
        let first = applies[0];
        let arg = self.term(first.arg, body);
        let rep = self.rep(&first.node.ty);
        // The implementation the record holds is an ordinary closure of the
        // operation's declared arrow, so no evidence goes with the call.
        let value = self.emit(
            body,
            first.node.span,
            rep,
            Op::Call {
                callee: Callee::Indirect(held),
                args: vec![arg],
            },
        );
        self.spun(value, &first.node.ty, &applies[1..], body)
    }

    /// Applying a definition whose arity is known.
    ///
    /// `used` is the type the head has *here*, which is the definition's type
    /// however this use instantiated it. The call takes its parameter shape from
    /// the definition and its tail bundles from `used`, per [`Self::evidence_args`].
    fn call_known(
        &mut self,
        known: &Known,
        used: &Rc<Ty>,
        applies: &[Apply],
        body: &mut Body,
    ) -> Temp {
        let arity = known.levels.len();
        let taken = applies.len().min(arity);
        let mut args: Vec<Temp> = Vec::new();
        for apply in &applies[..taken] {
            args.push(self.term(apply.arg, body));
        }
        // The visible arguments are evaluated where they were written; the
        // evidence around them is this pass's own, and goes in afterwards.
        let span = applies[0].node.span;
        let mut full: Vec<Temp> = Vec::new();
        let mut here = used.clone();
        for ((level, apply), arg) in known.levels.iter().zip(applies).zip(&args) {
            let (_, rest, row) = self.arrow(&here);
            full.extend(self.evidence_args(&level.row, &row, body));
            full.push(self.fitted(&level.from, &apply.arg.ty, *arg, body));
            here = rest;
        }

        let value = match taken == arity {
            true => {
                let node = applies[arity - 1].node;
                let rep = self.rep(&node.ty);
                let temp = self.emit(
                    body,
                    node.span,
                    rep,
                    Op::Call {
                        callee: Callee::Direct(known.lifted),
                        args: full,
                    },
                );
                // What a full application gives back is shaped by the
                // definition's own last arrow, not by what this use
                // instantiated it to — and it carries that shape wherever a
                // binding or a further application takes it.
                let to = known.levels[arity - 1].to.clone();
                self.hold(temp, &to);
                self.contain(temp, &to);
                temp
            }
            // Short of a full application, the arguments so far become the
            // captures of the wrapper that takes the next one. What that
            // wrapper takes is the definition's own shape walked down one
            // arrow per argument given, so the closure is fitted to what this
            // use asked for before it goes anywhere.
            false => {
                let node = applies[taken - 1].node;
                let rep = self.rep(&node.ty);
                let temp = self.emit(
                    body,
                    span,
                    rep,
                    Op::Closure {
                        func: known.wrappers[taken],
                        captures: full,
                    },
                );
                let have = self.walked(&known.ty, taken);
                self.fitted(&node.ty, &have, temp, body)
            }
        };
        self.spun(value, &applies[taken - 1].node.ty, &applies[taken..], body)
    }

    /// `handle body with arms end`, inline in the function around it: a fresh
    /// identity, one evidence record per discharged effect, and a `catch` over
    /// the body.
    fn handle(&mut self, term: &Term, handled: &Term, handler: &Handler, body: &mut Body) -> Temp {
        let span = term.span;
        let tag = self.emit(body, Span::default(), Rep::Any, Op::NewTag);
        let held_tag = self.top().raise_tag.replace(tag);
        let mut records: Vec<(String, Temp)> = Vec::new();
        for effect in &handler.discharges {
            let name = self.program.effect_ids[&effect.tracked].row_key();
            let mut entries: IndexMap<FieldKey, Temp> = IndexMap::new();
            let identity = &self.program.effect_ids[&effect.tracked];
            let arms: Vec<&HandlerArm> = handler
                .arms
                .iter()
                .filter(|arm| self.program.effect_ids[&arm.effect.tracked] == *identity)
                .collect();
            for arm in arms {
                let closure = self.arm(arm, body);
                entries.insert(Self::operation_slot(&arm.selector.tracked), closure);
            }
            // The record itself is evidence plumbing: the arms in it are the
            // reader's, the record holding them is this pass's own.
            let record = self.emit(body, Span::default(), Rep::Struct, Op::Struct(entries));
            records.push((name, record));
        }
        self.top().raise_tag = held_tag;

        let held: Vec<(String, Option<Temp>)> = records
            .iter()
            .map(|(name, record)| {
                let held = self.top().evidence.insert(name.clone(), *record);
                (name.clone(), held)
            })
            .collect();
        let ret = handler.ret.as_ref();
        let block = self.child(handled.span, |low, inner| {
            let value = low.term(handled, inner);
            match ret {
                Some(ret) => {
                    low.top().locals.insert(ret.binder.tracked, value);
                    low.term(&ret.body, inner)
                }
                None => value,
            }
        });
        for (name, previous) in held {
            match previous {
                Some(temp) => {
                    self.top().evidence.insert(name, temp);
                }
                None => {
                    self.top().evidence.shift_remove(&name);
                }
            }
        }

        let rep = self.rep(&term.ty);
        self.emit(
            body,
            span,
            rep,
            Op::Catch {
                tag,
                body: Box::new(block),
            },
        )
    }

    /// One operation arm, lifted: a function taking its captures and the
    /// operation's payload, and giving back the value the perform site resumes
    /// with.
    fn arm(&mut self, arm: &HandlerArm, body: &mut Body) -> Temp {
        let payload = self
            .inference
            .operations
            .get(&(arm.effect.tracked, arm.selector.tracked.clone()))
            .map(|(from, _)| from.clone())
            .unwrap_or_default();
        self.frames.push(Frame::default());
        let rep = self.rep(&payload);
        let temp = self.fresh(rep);
        self.top().locals.insert(arm.binder.tracked, temp);
        let mut lifted = Body::default();
        let value = self.term(&arm.body, &mut lifted);
        let frame = self.frames.pop().expect("the frame just pushed");
        let lifted = lifted.seal(Terminator {
            span: arm.body.span,
            kind: End::Ret(value),
        });
        let name = self.lifted_name();
        let id = self.slot(name.clone());
        let captures: Vec<Temp> = frame.captures.iter().map(|capture| capture.outer).collect();
        let params = Self::with_captures(&frame, vec![Param { temp, rep }]);
        self.fill(
            id,
            Function {
                name,
                params,
                body: lifted,
                span: arm.body.span,
            },
        );
        self.emit(
            body,
            arm.selector.span,
            Rep::Fn,
            Op::Closure { func: id, captures },
        )
    }

    /// One `match`, as a decision tree over the scrutinee.
    fn matched(
        &mut self,
        term: &Term,
        scrutinee: &Term,
        arms: &[(Pattern, Term)],
        body: &mut Body,
    ) -> Temp {
        let temp = self.term(scrutinee, body);
        let rep = self.rep(&term.ty);
        // A match with no arms constrained its scrutinee to the empty sum, which
        // has no values: a dispatch with nowhere to go, and nothing to test.
        if arms.is_empty() {
            return self.emit(
                body,
                term.span,
                rep,
                Op::SwitchTag {
                    on: temp,
                    cases: Vec::new(),
                    fallback: None,
                },
            );
        }
        let allowed = self
            .definition
            .and_then(|symbol| self.inference.promises.get(&symbol).cloned())
            .unwrap_or(Formula::True);
        let tree = Tree {
            arms,
            rep,
            ty: term.ty.clone(),
            span: term.span,
            allowed,
        };
        let matrix = Matrix {
            cols: vec![Col::Value(Value {
                temp,
                ty: scrutinee.ty.clone(),
            })],
            lines: arms
                .iter()
                .enumerate()
                .map(|(at, (pattern, _))| Line {
                    cells: vec![cell(pattern)],
                    arm: at,
                    binds: Vec::new(),
                })
                .collect(),
            assumed: self.assumed.clone(),
        };
        self.tree(matrix, &tree, body)
    }

    /// The decision tree itself: test the first column that anything tests, and
    /// recur on what each answer leaves.
    fn tree(&mut self, mut matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        let Some(col) = matrix.split() else {
            let line = matrix
                .lines
                .into_iter()
                .next()
                .expect("the pattern checks proved the match exhaustive");
            return self.leaf(line, matrix.assumed, tree, body);
        };
        match col {
            Col::Value(col) => self.column(&col, matrix, tree, body),
            Col::Field(col) => self.presence(&col, matrix, tree, body),
            Col::Beyond(col) => self.remainder(&col, matrix, tree, body),
        }
    }

    /// A leaf: bind what the path here matched and emit the arm's body.
    ///
    /// An arm reachable from several leaves is emitted at each of them, so its
    /// binders are bound afresh every time — which is why they are written into the
    /// frame here rather than once for the whole match.
    fn leaf(&mut self, line: Line, assumed: Formula, tree: &Tree, body: &mut Body) -> Temp {
        for (symbol, temp) in &line.binds {
            self.top().locals.insert(*symbol, *temp);
        }
        // Every leaf yields to the one temp the whole match stands at, so a
        // function value is fitted from the shape it holds to the match's own
        // type — which is the shape everything downstream reads the temp at.
        let arm = &tree.arms[line.arm].1;
        let outer = std::mem::replace(&mut self.assumed, assumed);
        let value = self.term(arm, body);
        self.assumed = outer;
        let have = self.holding(value, &arm.ty);
        self.fitted(&tree.ty, &have, value, body)
    }

    /// One whole position. A column any arm reaches into fields at is widened
    /// first — one presence column per field the type names, the constructor, and then
    /// whatever fields lie beyond — so everything below only ever sees a flat
    /// cell.
    fn column(&mut self, col: &Value, matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        let ty = unfold(&self.inference.aliases, &col.ty);
        if matrix
            .lines
            .iter()
            .any(|line| matches!(line.cells[0], Cell::Struct { .. }))
        {
            return self.widen(col.temp, &ty, matrix, tree, body);
        }
        let primitives = matrix
            .lines
            .iter()
            .any(|line| matches!(line.cells[0], Cell::Prim(_)));
        let tags = matrix
            .lines
            .iter()
            .any(|line| matches!(line.cells[0], Cell::Tag { .. }));
        match &*ty {
            Ty::Nat | Ty::Int | Ty::Real | Ty::String | Ty::Boolean if primitives => {
                self.switch_prim(col.temp, matrix, tree, body)
            }
            Ty::Sum(row) if tags => {
                let row = flat(row);
                self.switch_tag(col.temp, &row, matrix, tree, body)
            }
            // Nothing here to test: a unit, an arrow, a variable, or a position
            // every arm accepts whole. The column is consumed and whatever binds
            // at it binds the value itself.
            _ => self.tree(matrix.consumed(col.temp), tree, body),
        }
    }

    /// A primitive position: one case per literal written, and the rest. Only
    /// Boolean has a finite universe, so only a switch listing both `false`
    /// and `true` can omit its fallback.
    fn switch_prim(&mut self, temp: Temp, matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        let mut listed: Vec<Literal> = Vec::new();
        for line in &matrix.lines {
            if let Cell::Prim(value) = &line.cells[0]
                && !listed.contains(value)
            {
                listed.push(value.clone());
            }
        }
        let narrow = |value: Option<&Literal>| -> Matrix {
            matrix
                .kept(|line| match (&line.cells[0], value) {
                    (Cell::Prim(written), Some(value)) => written == value,
                    (Cell::Wild(_), _) => true,
                    _ => false,
                })
                .consumed(temp)
        };
        let cases: Vec<PrimCase> = listed
            .iter()
            .map(|value| {
                let kept = narrow(Some(value));
                PrimCase {
                    value: value.clone(),
                    block: self.child(tree.span, |low, inner| low.tree(kept, tree, inner)),
                }
            })
            .collect();
        let wilds = narrow(None);
        let boolean_complete =
            listed.contains(&Literal::Boolean(false)) && listed.contains(&Literal::Boolean(true));
        let fallback = if boolean_complete {
            // The wildcard rows remain in each listed case for tests in later
            // columns, but there is no Boolean value left for a default arm.
            None
        } else {
            assert!(
                !wilds.lines.is_empty(),
                "the pattern checks proved the match exhaustive"
            );
            Some(Box::new(
                self.child(tree.span, |low, inner| low.tree(wilds, tree, inner)),
            ))
        };
        self.emit(
            body,
            tree.span,
            tree.rep,
            Op::SwitchPrim {
                on: temp,
                cases,
                fallback,
            },
        )
    }

    /// A sum position: one case per tag written, each reading the payload out
    /// once where anything below it tests one, and a fallback only where some
    /// arm accepts a case the listed ones do not.
    fn switch_tag(
        &mut self,
        temp: Temp,
        row: &Row,
        matrix: Matrix,
        tree: &Tree,
        body: &mut Body,
    ) -> Temp {
        let mut listed: Vec<String> = Vec::new();
        for line in &matrix.lines {
            if let Cell::Tag { name, .. } = &line.cells[0]
                && !listed.contains(name)
            {
                listed.push(name.clone());
            }
        }
        let cases: Vec<TagCase> = listed
            .iter()
            .map(|name| {
                let payload_ty = row
                    .labels
                    .get(name)
                    .map(|case| case.ty.clone())
                    .unwrap_or_default();
                let kept = Matrix {
                    cols: matrix.cols.clone(),
                    lines: matrix
                        .lines
                        .iter()
                        .filter_map(|line| {
                            let mut line = line.clone();
                            let payload = match &line.cells[0] {
                                Cell::Tag { name: tag, payload } if tag == name => {
                                    (**payload).clone()
                                }
                                Cell::Wild(bound) => {
                                    if let Some(symbol) = bound {
                                        line.binds.push((*symbol, temp));
                                    }
                                    Cell::Wild(None)
                                }
                                _ => return None,
                            };
                            line.cells[0] = payload;
                            Some(line)
                        })
                        .collect(),
                    assumed: matrix.assumed.clone(),
                };
                let reads = !kept.untested();
                let block = self.child(tree.span, |low, inner| match reads {
                    true => {
                        // What shape the payload holds is the scrutinee's
                        // container type to declare, where one was recorded —
                        // the case was stored at that type's word, not at
                        // whatever this use instantiates the sum to.
                        let have = low.held[temp as usize]
                            .clone()
                            .and_then(|authority| low.member_of(&authority, name))
                            .unwrap_or_else(|| payload_ty.clone());
                        let rep = low.rep(&have);
                        let read = low.emit(inner, tree.span, rep, Op::Payload(temp));
                        low.contain(read, &have);
                        let held = low.fitted(&payload_ty, &have, read, inner);
                        let cols = vec![Col::Value(Value {
                            temp: held,
                            ty: payload_ty.clone(),
                        })];
                        low.tree(kept.under(cols), tree, inner)
                    }
                    false => low.tree(kept.dropped(), tree, inner),
                });
                TagCase {
                    name: name.clone(),
                    block,
                }
            })
            .collect();

        let uncovered = !matches!(row.rest, Rest::Closed)
            || row
                .labels
                .iter()
                .any(|(name, case)| possible(&case.presence) && !listed.contains(name));
        let wilds = matrix
            .kept(|line| matches!(line.cells[0], Cell::Wild(_)))
            .consumed(temp);
        let fallback = match uncovered && !wilds.lines.is_empty() {
            true => Some(Box::new(
                self.child(tree.span, |low, inner| low.tree(wilds, tree, inner)),
            )),
            false => None,
        };
        self.emit(
            body,
            tree.span,
            tree.rep,
            Op::SwitchTag {
                on: temp,
                cases,
                fallback,
            },
        )
    }

    /// Widen a struct position: one presence column per field the solved type
    /// names, then the constructor, then whatever fields lie beyond the named ones.
    fn widen(
        &mut self,
        temp: Temp,
        ty: &Rc<Ty>,
        matrix: Matrix,
        tree: &Tree,
        body: &mut Body,
    ) -> Temp {
        let exposed = unfold(&self.inference.aliases, ty);
        let row = match &*exposed {
            Ty::Struct(row) => flat(row),
            // Imported recovery types can disagree with the already-recovered
            // pattern matrix. Treat them as an open unknown row: the fields the
            // pattern itself names are added below, and lowering remains total.
            _ => Row::of(Rest::Undecided),
        };
        let mut named: Vec<(String, Presence, Rc<Ty>)> = row
            .labels
            .iter()
            .map(|(name, field)| (name.clone(), field.presence.clone(), field.ty.clone()))
            .collect();
        // A field an arm names that the type does not is provably absent — the
        // pattern checks let it through over the fieldless unit alone — and
        // still needs a column, with the one-value universe absence is.
        for line in &matrix.lines {
            if let Cell::Struct { fields, .. } = &line.cells[0] {
                for (name, _) in fields {
                    if !named.iter().any(|(known, _, _)| known == name) {
                        named.push((name.clone(), Presence::Absent, Rc::new(Ty::default())));
                    }
                }
            }
        }
        let widen = |cell: &Cell| -> Vec<Cell> {
            let mut wide = Vec::with_capacity(named.len() + 1);
            match cell {
                Cell::Struct { fields, exact } => {
                    for (name, _, _) in &named {
                        let sub = fields.iter().find(|(field, _)| field == name);
                        wide.push(match (sub, exact) {
                            (Some((_, sub)), _) => Cell::Present(Box::new(sub.clone())),
                            (None, true) => Cell::Absent,
                            (None, false) => Cell::Wild(None),
                        });
                    }
                    wide.push(match exact {
                        true => Cell::Absent,
                        false => Cell::Wild(None),
                    });
                }
                _ => {
                    wide.extend(std::iter::repeat_n(Cell::Wild(None), named.len()));
                    wide.push(Cell::Wild(None));
                }
            }
            wide
        };
        let mut cols: Vec<Col> = named
            .iter()
            .map(|(name, presence, field_ty)| {
                Col::Field(Field {
                    base: temp,
                    base_ty: ty.clone(),
                    name: name.clone(),
                    presence: presence.clone(),
                    ty: field_ty.clone(),
                })
            })
            .collect();
        cols.push(Col::Beyond(Beyond {
            base: temp,
            names: named.iter().map(|(name, _, _)| name.clone()).collect(),
            open: !matches!(row.rest, Rest::Closed),
        }));
        let widened = Matrix {
            cols: matrix.cols,
            lines: matrix
                .lines
                .into_iter()
                .map(|line| {
                    let mut cells = widen(&line.cells[0]);
                    cells.extend(line.cells[1..].iter().cloned());
                    Line { cells, ..line }
                })
                .collect(),
            assumed: matrix.assumed,
        };
        self.tree(widened.under(cols), tree, body)
    }

    /// One field's presence. A field the type proves present is read straight
    /// out; one it proves absent tests nothing and starves the arms demanding
    /// it; one still open is the `switch_presence` the optional fields exist
    /// for.
    fn presence(&mut self, col: &Field, matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        // Nothing anywhere in the column asks about this field, so neither the
        // presence nor the value is worth reading.
        if matrix.untested() {
            return self.tree(matrix.dropped(), tree, body);
        }
        match col.presence {
            Presence::Present => self.field(col, matrix.present(), tree, body),
            Presence::Absent => self.tree(matrix.absent(), tree, body),
            _ => {
                let literal = match &col.presence {
                    Presence::Var(_) | Presence::Bound(_) => Some(col.presence.formula()),
                    Presence::Present
                    | Presence::Absent
                    | Presence::Recovered(_)
                    | Presence::Undecided => None,
                };
                let path = tree.allowed.clone().and(matrix.assumed.clone());
                let can_present = literal.as_ref().is_none_or(|literal| {
                    inference::sat::satisfiable(&path.clone().and(literal.clone()))
                });
                let can_absent = literal.as_ref().is_none_or(|literal| {
                    inference::sat::satisfiable(&path.clone().and(literal.clone().not()))
                });
                match (can_present, can_absent) {
                    (true, false) => {
                        let kept = matrix
                            .present()
                            .assuming(literal.expect("a decided branch has a literal"));
                        self.field(col, kept, tree, body)
                    }
                    (false, true) => {
                        let missing = matrix
                            .absent()
                            .assuming(literal.expect("a decided branch has a literal").not());
                        self.tree(missing, tree, body)
                    }
                    (true, true) => {
                        let kept = match &literal {
                            Some(literal) => matrix.present().assuming(literal.clone()),
                            None => matrix.present(),
                        };
                        let present =
                            self.child(tree.span, |low, inner| low.field(col, kept, tree, inner));
                        let missing = match &literal {
                            Some(literal) => matrix.absent().assuming(literal.clone().not()),
                            None => matrix.absent(),
                        };
                        let absent =
                            self.child(tree.span, |low, inner| low.tree(missing, tree, inner));
                        self.emit(
                            body,
                            tree.span,
                            tree.rep,
                            Op::SwitchPresence {
                                on: col.base,
                                field: col.name.clone(),
                                present: Box::new(present),
                                absent: Box::new(absent),
                            },
                        )
                    }
                    (false, false) => {
                        unreachable!(
                            "the inferred definition promise has a model on every emitted path"
                        )
                    }
                }
            }
        }
    }

    /// Read one field out and go on into what the arms ask of it.
    fn field(&mut self, col: &Field, matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        if matrix.untested() {
            return self.tree(matrix.dropped(), tree, body);
        }
        // As with expression projection, the container's production type is
        // authoritative about the representation (including a function's
        // effect-evidence ABI) actually stored in this slot. Read at that shape,
        // record it on the projected temp, then adapt to the pattern's
        // instantiated field type.
        let authority = self.holding(col.base, &col.base_ty);
        let have = self
            .member_of(&authority, &col.name)
            .unwrap_or_else(|| col.ty.clone());
        let temp = self.emit(
            body,
            tree.span,
            self.rep(&have),
            Op::Project {
                base: col.base,
                field: FieldKey::named(col.name.clone()),
            },
        );
        self.contain(temp, &have);
        let temp = self.fitted(&col.ty, &have, temp, body);
        let read = vec![Col::Value(Value {
            temp,
            ty: col.ty.clone(),
        })];
        self.tree(matrix.under(read), tree, body)
    }

    /// Whatever fields a value carries beyond the ones its type names. Only a
    /// pattern written exact asks about them, and only an open type can have
    /// any, so the test exists exactly where both are true.
    fn remainder(&mut self, col: &Beyond, matrix: Matrix, tree: &Tree, body: &mut Body) -> Temp {
        let exacting = matrix
            .lines
            .iter()
            .any(|line| matches!(line.cells[0], Cell::Absent));
        if !exacting || !col.open {
            return self.tree(matrix.dropped(), tree, body);
        }
        let some = matrix
            .kept(|line| matches!(line.cells[0], Cell::Wild(_)))
            .dropped();
        let none = matrix.dropped();
        let none = self.child(tree.span, |low, inner| low.tree(none, tree, inner));
        let some = self.child(tree.span, |low, inner| low.tree(some, tree, inner));
        self.emit(
            body,
            tree.span,
            tree.rep,
            Op::SwitchRest {
                on: col.base,
                fields: col.names.clone(),
                none: Box::new(none),
                some: Box::new(some),
            },
        )
    }
}
