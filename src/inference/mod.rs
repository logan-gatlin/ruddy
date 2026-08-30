//! Assigning a type to every term.
//!
//! Hindley–Milner: unification and `let`-generalization. Types are equated,
//! never ordered — two types either unify or they are an error — so a term's type
//! is the one type it has rather than a bound on it.
//!
//! Each binding group is typed in two passes, and the [`Constraint`] list is
//! all they share.
//!
//! *Generation* ([`Constrain`]) walks the term, mints a variable wherever the
//! type is not yet known, writes one into every [`Term`], and records what has
//! to hold about them. It never inspects the variable table, so the walk reads
//! as a description of the term and nothing else: an arm cannot depend on how
//! much an earlier arm happened to have solved.
//!
//! *Solving* ([`Solve`]) takes that list and nothing else. It unifies, occurs-
//! checks, and reports; it has never seen a [`Term`], so every diagnostic it
//! can produce is one the constraint carried the span for. Every act it
//! performs is also recorded as a [`Step`], so the solve can be replayed one
//! rule at a time rather than only read as its result.
//!
//! Generalization is why the two passes alternate per binding group rather
//! than running over the whole program: `let id = fn x => x` has to become a
//! scheme before a later definition's `id 1` can instantiate it. A group is as
//! small as that alternation can be made — the definitions that name each other
//! have to be solved at once, and everything else gets a scheme of its own. See
//! [`Group`](crate::ir::Group).
//!
//! A nested `let` generalizes inside all that, and cannot be another turn of
//! the same alternation: it sits in the middle of one definition's walk, which
//! is over before anything is solved. So the scoping it needs is written down
//! instead — [`ConstraintKind::Let`] carries the two lists and the order they
//! go in, and the solver does the generalizing where it can. Which is what
//! keeps generation's invariant above true of the one construct that would
//! otherwise have had to break it. See [`Table::levels`] for what decides which
//! variables such a generalization may take.
//!
//! Beside the two passes there is a third thing being decided, and it is not a
//! question about types: which *combinations* of a value's labels are allowed.
//! `{x} | {y}` says exactly one of two fields is there, which no unconstrained
//! type can say, so the demand goes into a [`Store`] of propositional formulas
//! over presence variables instead — grown by a match's coverage, by a
//! constrained scheme's instantiation and by an annotation's `where` clause,
//! and decided by [`sat`].
//!
//! Staging is what keeps that terminating, and it is the whole discipline:
//! unification never consults the solver, matches emit only finite formulas,
//! and SAT runs at generalization boundaries, at the use-site and annotation
//! checks, and — through [`Output::store`] — in the patterns phase. A
//! [`Scheme`] therefore carries a formula as well as a body, which is a
//! constrained scheme in the HM(X) sense: it is what makes a principal type
//! exist for the programs above.
//!
//! Presence refinement keeps the same staging discipline. A qualifying match
//! adds one finite constraint node per written arm. Its ordered guards are
//! built by one fold over those arms, and nesting follows the finite AST.
//! Guarded equality decomposes by the ordinary finite type-size/occurs-check
//! measure; it only replaces a presence binding with one finite implication.
//! SAT is asked only at fixed arm and publishing boundaries, after the formulas
//! involved already exist, and its answer never regenerates constraints or starts another solve
//! pass. Recursive aliases retain the existing finite assumption stack, store
//! growth is bounded by the written arms and structural presence comparisons,
//! and projection retains its explicit cube/minterm budgets. There is therefore
//! no solve → refine → regenerate cycle.
//!
//! Inference runs after lowering and mutates the [`Program`] it is handed:
//! every [`Term`]'s `ty` goes from [`Ty::Undecided`] to what was inferred for
//! it, fully resolved, so nothing downstream ever needs the solver's variable
//! table to read a type. Errors do not stop either pass — a term that failed to
//! type still has a type, [`Ty::Undecided`], which unifies with everything so
//! that one mistake is reported once rather than echoed by every consumer.

mod constrain;
pub mod sat;
mod solve;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    ir::{self, Annotation, Clause, ClauseKind, Program, Tail, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{
        Assigned, Atom, EffectId, Formula, ParamKind, Presence, Rest, Row, RowField, Scheme, Sense,
        Shape, Ty, TyVar, same_finite_syntax,
    },
};
use constrain::Constrain;
use solve::Solve;

#[derive(Debug, Clone)]
pub struct Output {
    /// What each `type` declaration stands for: the semantic type its body
    /// denotes, one step deep. A name inside a body stays a [`Ty::Named`] and
    /// is looked up here again, which is how a declaration that names itself
    /// stays a finite value — and why this map, not the type, is what a
    /// recursive type is made of. See [`unfold`].
    ///
    /// A [`Scheme`] rather than a bare type, because handing a declaration its
    /// arguments is substituting for the [`Ty::Bound`]s standing in for its
    /// parameters — which is what instantiating a scheme already is, down to
    /// the same `open`. A declaration taking no parameters is a scheme binding
    /// nothing, and opening one returns its body unchanged.
    pub aliases: IndexMap<Symbol, Scheme>,
    /// The two sides of every declared operation, keyed by the effect that
    /// declares it and the operation's own name, in declaration order.
    ///
    /// A plain closed arrow, lowered once before any body is walked, because a
    /// signature mentions no variable. Published for the reason
    /// [`Output::aliases`] is: a later phase reads it and has no table to lower
    /// a written type with. [`lir`](crate::lir) is that phase — a handler arm's
    /// binder is the operation's argument, and how a value of it is held is
    /// nowhere else to be found, since the binder has no term of its own to
    /// carry a solved type.
    pub operations: IndexMap<(Symbol, ir::OperationSelector), (Rc<Ty>, Rc<Ty>)>,
    /// The scheme each target-provided top-level value declared. Externs have
    /// no body and are therefore intentionally separate from `schemes`, whose
    /// entries each correspond to a term initializer.
    pub externs: IndexMap<Symbol, Scheme>,
    /// The scheme each top-level term was inferred, or checked, to have.
    pub schemes: IndexMap<Symbol, Scheme>,
    /// The scheme each nested `let` was inferred, in the order the lets were
    /// walked.
    ///
    /// Beside [`Output::schemes`] rather than in it, so that a reader of that
    /// map is still reading the definitions of the file: a local binding is not
    /// a definition, and nothing that consumes the program's exports has any
    /// business seeing one.
    ///
    /// Numbered on its own. A local's scheme may leave an enclosing binder's
    /// variables free, and those are spelled here as letters past its own
    /// quantifiers rather than as the `?3` the solver knew them by — see
    /// [`Table::published`] — so two rows of this map spelling `a` are two
    /// unrelated variables, exactly as two schemes are.
    pub locals: IndexMap<Symbol, Scheme>,
    /// What generation asked of each definition, in the order it asked, and
    /// exactly as it was asked: these are the constraints *before* the solver
    /// ran, so a variable in one prints as the variable it was. Solving is what
    /// the schemes report. Kept so that the pass can be read rather than
    /// inferred from its result — which is what the debugger's tab shows.
    pub constraints: IndexMap<Symbol, Vec<Constraint>>,
    /// Every act of the solver, over the whole program, in the order it
    /// performed them. One flat list rather than one per definition: the
    /// variable table is shared, so replaying the effects in this order — and
    /// only in this order — reconstructs what the solver knew at any point.
    pub steps: Vec<Step>,
    /// What the program requires of its presence variables, in the order it
    /// required it. See [`Store`].
    pub store: Store,
    /// What [`patterns`](crate::patterns) may assume while it walks each
    /// top-level definition: the store's word about *every* presence variable
    /// that definition's zonked terms can name, in the [`Ty::Bound`]
    /// numbering those terms were closed into.
    ///
    /// Not the scheme's `where` clause, which is a strictly smaller thing. A
    /// scheme quantifies only the presences its own type mentions (R8, R12),
    /// but a nested `let` is generalized on its own terms and its presences
    /// still reach the enclosing definition's body — numbered by the same
    /// substitution, and so nameable by the types the walk reads. Promising
    /// only the scheme's clause would leave every such presence unconstrained
    /// and walk both halves of a column the store had already related: the
    /// independence R11 exists to remove.
    ///
    /// Keyed by the top-level symbol, in source order. Empty — [`Formula::True`]
    /// — for a definition generalized once something had flipped the store: a
    /// store with no model entails everything, and asking it would call every
    /// arm of every match unreachable.
    pub promises: IndexMap<Symbol, Formula>,
    /// The branch-local presence assumptions inference used, one report per
    /// arm of every qualifying match, in solve order.
    pub refinements: Vec<Refinement>,
    /// Metadata indexed by `TyVar`, parallel to the solver's private slots.
    pub variables: Vec<VarMeta>,
    /// Append-only reason arena. Speculative nodes remain retired but readable
    /// after rollback; no surviving link can be retargeted by identity reuse.
    pub reasons: Vec<Reason>,
    pub errors: Vec<Error>,
}

/// The propositional constraint store: every formula the program emitted about
/// which combinations of labels may be there, in program order.
///
/// The one piece of inference's state that outlives it, beyond the schemes:
/// [`patterns`](crate::patterns) reads it to decide reachability and
/// exhaustiveness for the columns that qualified, and the debugger's Presence
/// tab renders it whole.
///
/// Order is the whole of what makes a complaint attributable. Conjunction does
/// not care what order it is asked in, but *which batch made the store
/// unsatisfiable* does — so the batches are kept as a sequence, replayed in it,
/// and the first one that flips the verdict owns the error. See [`Batch`].
macro_rules! inference_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn get(self) -> u64 {
                self.0
            }

            #[doc(hidden)]
            pub const fn synthetic(value: u64) -> Self {
                Self(value)
            }

            #[allow(dead_code)]
            fn pending() -> Self {
                Self(u64::MAX)
            }
        }
    };
}

inference_id!(ConstraintId);
inference_id!(StepId);
inference_id!(ErrorId);
inference_id!(BatchId);
inference_id!(ReasonId);

/// The semantic sort of a solver variable. Kept outside [`Ty`] so provenance
/// never changes type equality or user-facing type notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarSort {
    Type,
    Row,
    Presence,
}

/// Immutable information recorded when a solver variable is minted.
#[derive(Debug, Clone)]
pub struct VarMeta {
    pub sort: VarSort,
    pub subject: Subject,
    pub minted_by: ReasonId,
}

/// One immutable node in the inference reason arena. Parents always name
/// earlier nodes, so consumers can walk the graph iteratively without needing
/// the solver table or risking recursion on deeply nested imported types.
#[derive(Debug, Clone)]
pub struct Reason {
    pub id: ReasonId,
    pub parents: Vec<ReasonId>,
    pub origin: ReasonOrigin,
    /// False when the causal act belonged to speculative work that was rolled
    /// back. IDs remain retired and are never reused.
    pub reachable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonOrigin {
    Variable {
        sort: VarSort,
        subject: Subject,
    },
    Constraint(ConstraintId),
    Batch(BatchId),
    Step(StepId),
    Recovery,
    DefaultBinding {
        var: TyVar,
        kind: DefaultBinding,
        assigned: DefaultAssignment,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultBinding {
    Sat,
    CloseEffects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAssignment {
    Present,
    Absent,
    EmptyRow,
}

impl Output {
    /// Reachable causal slice rooted at `seed`, walked iteratively so debugger
    /// queries remain safe for arbitrarily deep chains of solved aliases.
    pub fn reason_ancestors(&self, seed: ReasonId) -> Vec<ReasonId> {
        let arena: HashMap<_, _> = self
            .reasons
            .iter()
            .map(|reason| (reason.id, reason))
            .collect();
        let mut seen = HashSet::new();
        let mut work = vec![seed];
        let mut out = Vec::new();
        while let Some(id) = work.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(reason) = arena.get(&id).filter(|reason| reason.reachable) else {
                continue;
            };
            out.push(id);
            work.extend(reason.parents.iter().rev().copied());
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct Store {
    pub batches: Vec<Batch>,
}

/// One tagged batch of the store: where the program said it, why, and what it
/// said.
#[derive(Debug, Clone)]
pub struct Batch {
    /// Stable identity allocated when this source-order slot is reserved.
    pub id: BatchId,
    /// Top-level definition whose generation/solve emitted this batch.
    pub definition: Option<Symbol>,
    /// Where the program said it: the match, the use site, or the annotation.
    pub span: Span,
    pub origin: Origin,
    /// Root of this batch's causal explanation.
    pub reason: ReasonId,
    /// What it requires, over the presence variables that existed when it was
    /// emitted. Kept as it was emitted, not as the solve later resolved it —
    /// the debugger shows the pass being read rather than its result, exactly
    /// as [`Output::constraints`] does.
    pub formula: Formula,
    /// Whether conjoining this batch is what made the store unsatisfiable. At
    /// most one batch in a store is, which is the cascade rule: the first flip
    /// owns the single resulting error and everything after it is suppressed.
    pub flipped: bool,
}

/// Why a batch is in the store.
#[derive(Debug, Clone)]
pub enum Origin {
    /// A match's arms, read as the presences they cover. See [`Coverage`].
    Coverage(Coverage),
    /// A use of a constrained scheme: its formula, with fresh variables
    /// substituted for the ones it quantified.
    Instance(Named),
    /// An annotation's own `where` clause.
    Annotation(Named),
    /// A presence relation produced by guarded structural equality.
    Refinement(Named),
    /// An existing obligation made conditional on a branch assumption. The
    /// original origin is retained so patterns and diagnostics keep their
    /// attribution instead of seeing an opaque simplified implication.
    Guarded(GuardedOrigin),
}

/// The metadata retained around an implication emitted in a qualifying arm.
#[derive(Debug, Clone)]
pub struct GuardedOrigin {
    pub premise: Formula,
    pub obligation: Formula,
    pub origin: Box<Origin>,
}

/// What a match-coverage batch carries beyond its formula, so that the two
/// readers who need more than "is this satisfiable" have it.
pub type PresencePath = Vec<String>;

pub fn display_presence_path(path: &[String]) -> String {
    path.iter()
        .map(|segment| crate::ui::label(Shape::Struct, segment))
        .collect::<Vec<_>>()
        .join(".")
}

/// Every reachable structural field path and the presence worn at that path.
/// An absent field contributes its own path but never exposes its meaningless
/// payload.
pub fn structural_presence_paths(ty: &Rc<Ty>) -> Vec<(PresencePath, Presence)> {
    constrain::structural_presence_paths(ty)
}

#[derive(Debug, Clone)]
pub struct Coverage {
    /// One disjunct per written arm, in order: the conjunction of presence
    /// literals that arm covers. The batch's formula is their disjunction; the
    /// patterns phase asks about them one at a time, which is what makes
    /// reachability a SAT query.
    pub arms: Vec<Formula>,
    /// The scrutinee's labels and what decides whether each is there, so a
    /// model of `store ∧ ¬covered` can be written out as a value.
    pub fields: Vec<(String, Presence)>,
    /// Every nested struct path and its presence, for translating the raw
    /// formulas into the zonked alphabet used while walking nested terms.
    pub paths: Vec<(PresencePath, Presence)>,
}

/// Ordered effective arm conditions, in one linear fold.
///
/// If arm `i` covers `C_i`, the returned condition is `C_i` together with the
/// negation of everything written before it. This is the single definition
/// shared by inference and pattern reachability.
pub fn effective_conditions(raw: &[Formula]) -> Vec<Formula> {
    let mut earlier = Formula::False;
    raw.iter()
        .cloned()
        .map(|covered| {
            let effective = covered.clone().and(earlier.clone().not());
            earlier = earlier.clone().or(covered);
            effective
        })
        .collect()
}

/// One arm inside a qualifying match constraint.
#[derive(Debug, Clone)]
pub struct GuardedArm {
    pub span: Span,
    pub raw: Formula,
    pub effective: Formula,
    /// Coverage batch whose arm condition supplies this premise.
    pub premise_reason: ReasonId,
    pub constraints: Vec<Constraint>,
    pub requirements: Vec<DeferredRequirement>,
    /// The guarded equality between this body's type and the match result
    /// family. It is a child constraint because it is solved under this arm's
    /// premise, not as undifferentiated work owned by the enclosing match.
    pub result: Constraint,
}

/// A generation-time store batch held inert in its original source-order slot
/// until the solver reaches the arm that owns it.
#[derive(Debug, Clone)]
pub struct DeferredRequirement {
    pub at: usize,
    pub batch: Batch,
}

/// What guarded solving learned about one qualifying arm.
#[derive(Debug, Clone)]
pub struct Refinement {
    pub definition: Symbol,
    pub match_span: Span,
    pub arm_span: Span,
    pub raw: Formula,
    pub effective: Formula,
    pub reachable: bool,
    pub fields: Vec<(PresencePath, Presence)>,
    pub facts: Vec<RefinementFact>,
    pub obligations: Vec<GuardedObligation>,
}

#[derive(Debug, Clone)]
pub struct RefinementFact {
    pub field: PresencePath,
    pub present: bool,
}

#[derive(Debug, Clone)]
pub struct GuardedObligation {
    pub span: Span,
    pub premise: Formula,
    pub obligation: Formula,
    pub formula: Formula,
}

/// What a batch carries so that a complaint about it can be worded in the
/// reader's own nouns.
///
/// "this value needs `x != y` among its fields" — the reader never wrote the
/// presence variable, only the field it governs. A use site reads the pairs off
/// the type it instantiated, which is the one place both are in hand; an
/// annotation reads them off the `when` clauses it wrote, which is what its own
/// formula was written in.
#[derive(Debug, Clone)]
pub struct Named {
    pub labels: Vec<(String, Presence)>,
    /// The row shape whose labels the formula names. Annotation-only origins
    /// do not need one; required-use diagnostics do, so they can call the
    /// labels fields, cases, or effects without guessing from their spelling.
    pub shape: Option<Shape>,
}

/// One act of the solver: the rule it applied, what it applied it to, and what
/// changed as a result.
///
/// A step is a snapshot of a moment, not of the end: its types are resolved as
/// far as the solver had got, and an [`ErrorKind`] in its effect is worded from
/// what was known then. [`Output::errors`] is the same errors said again with
/// everything the solve went on to learn, which is what a reporter wants and a
/// replay does not.
#[derive(Debug, Clone)]
pub struct Step {
    /// Stable identity in this inference run. Retired, never reused, on rollback.
    pub id: StepId,
    /// The constraint whose solve produced this step.
    pub constraint: Option<ConstraintId>,
    /// Immutable reason node for this act of the solver.
    pub reason: ReasonId,
    /// The error this step emitted, if it failed. Kept separately from the
    /// rendered effect so correlation never depends on error wording.
    pub error: Option<ErrorId>,
    /// The definition being solved. Solving runs per definition, so this is
    /// what divides one solve from the next in the flat list.
    pub definition: Symbol,
    pub span: Span,
    /// How far inside a decomposition: the two halves of an arrow are one
    /// deeper than the arrow that produced them, and follow it immediately.
    pub depth: u32,
    pub rule: Rule,
    /// What the rule was applied to.
    ///
    /// Wider than a [`Constraint`], because the solver asks questions
    /// generation cannot. Every constraint equates two types; taking one apart
    /// reaches a pair of rows — what two tails have to agree on — and a pair of
    /// presences, and neither of those is a question about types. So a goal is
    /// three-sorted where a constraint is one.
    pub goal: Goal,
    pub effect: Effect,
}

/// Two things of one sort the solver is deciding must be equal.
///
/// The sorts are the sorts a variable can have, and for the same reason: a
/// question about a row is not a question about the type the row belongs to,
/// and answering one binds a different kind of variable.
#[derive(Debug, Clone)]
pub enum Goal {
    Type {
        expected: Rc<Ty>,
        actual: Rc<Ty>,
    },
    Row {
        expected: Rc<Row>,
        actual: Rc<Row>,
    },
    Presence {
        expected: Presence,
        actual: Presence,
    },
}

/// The case of the solver that fired. One per arm of [`Solve::unify`] — with
/// the occurs check counting as its own, since it is the arm *not* applying,
/// and the assumption that ends an unfolding likewise — plus the recovery
/// that follows every failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// One side is [`Ty::Undecided`], which unifies with anything.
    Absorb,
    /// Both sides are already the same thing: the same variable, or the same
    /// declared type applied to nothing. Either way there is nothing to take
    /// apart.
    Same,
    /// The same declared type on both sides, applied to arguments: taken apart
    /// into one goal per argument rather than unfolded.
    ///
    /// What [`Rule::Same`] becomes when there is something to compare, and a
    /// shortcut rather than a decision. It is taken only for a declaration
    /// every parameter of which survives unfolding, where the arguments agree
    /// exactly when the bodies do — so it comes before [`Rule::Unfold`] to give
    /// the better complaint, never to give a different answer. See
    /// [`Solve::nominal`].
    Congruent,
    /// A variable against a type: the only rule that grows the solution.
    Bind,
    /// A variable against a type that contains it. The occurs check fired, so
    /// the binding [`Rule::Bind`] would have made was not made — a rule of its
    /// own rather than a `Bind` that failed, because a reader shown "a variable
    /// takes the type it is against" above an effect reading "this type would have
    /// to contain itself" is being told the opposite of what happened.
    Occurs,
    /// A variable standing for the rest of a row, against a row that certainly
    /// has a field the first row already names. The lacks check fired, so —
    /// for the same reason [`Rule::Occurs`] is not a [`Rule::Bind`] — the
    /// refusal is a rule of its own rather than a binding that did not happen.
    ///
    /// Certainly has: a label the row names settled absent is no copy at all,
    /// and one still being decided settles absent at the binding instead — so
    /// neither is this rule, and both bind. See [`Solve::assign`].
    ///
    /// The shape rides along for the wording, as it does on [`Rule::Presence`]:
    /// the rule is one rule, and a reader watching two sums be decided should
    /// still be read to in cases.
    Overlap { shape: Shape },
    /// Two identical primitives.
    Prim,
    /// Two arrows, taken apart into argument, result and effects.
    Arrow,
    /// An application, with the callee's row opened into the ambient the call
    /// was written at — or refused, where the ambient cannot allow one of the
    /// effects the callee certainly performs. The one rule that widens rather
    /// than equates; see [`ConstraintKind::Performs`].
    Performs,
    /// A struct type, taken apart: the fields both sides name against
    /// each other, the fields only one names into what the other side allows
    /// beyond its own, and then the two struct rows.
    ///
    /// Recorded whenever either side carries a label, whatever the constructors and row tails are —
    /// so it is over a struct against a struct, as it always was, and over the
    /// `Nat` carrying an `x` that only a declaration can reach.
    Struct,
    /// Presence equality under an arm premise. No solver variable is bound;
    /// the implication recorded in the store is the effect.
    Refine,
    /// [`Rule::Struct`] about the other shape: two sums, the cases both name
    /// against each other and the cases only one names into what the other's
    /// tail allows.
    ///
    /// One rule in the code and two here, because a reader stepping through a
    /// solve is reading about their program: a line about fields over a goal
    /// about `#Some` and `#None` describes something they never wrote.
    /// See [`Solve::labels`], which is both.
    Sum,
    /// Whether one field is there, decided: present agrees with present and
    /// absent with absent, and a field one side must have while the other side
    /// cannot is where a missing or extra one is discovered.
    ///
    /// One rule in the code and two in the reading, the way [`Rule::Struct`]
    /// and [`Rule::Sum`] are — except that the two differ in one noun rather
    /// than in a sentence, so the shape is carried here and the wording reads
    /// it. A reader watching a sum be decided is told about its cases.
    Presence { shape: Shape },
    /// A declared type replaced by what it stands for, so that a goal about a
    /// name becomes a goal about a shape. What names are for: a type is equal
    /// to another by how it unfolds, never by what it is called.
    Unfold,
    /// The same two types are already being compared further out, so unfolding
    /// them again would ask a question that is already open. A recursive type
    /// equals another when assuming they are equal never leads to a
    /// contradiction, and this is that assumption being used.
    Assume,
    /// Nothing above applied, and the two types cannot be made equal.
    Mismatch,
    /// Pointing what an abandoned goal would have decided at the undecided
    /// value of its own sort
    /// so that one failure is not echoed by everything downstream of it.
    Recover,
}

/// What a step changed. Only [`Effect::Bound`] grows the solution and only
/// [`Effect::Failed`] grows the errors, so replaying a prefix of the steps and
/// collecting those two is the whole state of the solve at that point.
#[derive(Debug, Clone)]
pub enum Effect {
    /// The goal already held, or was put back for later.
    None,
    /// A variable now points at a value of its own sort.
    Bound {
        var: TyVar,
        value: Assigned,
        /// The solver-step reason that made this binding.
        by: ReasonId,
        /// For recovery bindings, the failed/absorbing step that caused the
        /// value to be abandoned. Ordinary bindings have no `because` link.
        because: Option<ReasonId>,
    },
    /// The goal was replaced by the goals that follow it one level deeper —
    /// the halves of an arrow, the fields of a struct, or the same goal asked
    /// again about what a name stands for.
    Decomposed,
    /// A guarded presence equality was recorded as an implication.
    Guarded {
        premise: Formula,
        obligation: Formula,
    },
    /// Reported, and the goal abandoned.
    Failed(ErrorKind),
}

/// One thing that has to be true of a definition's types, and where the program
/// said so.
#[derive(Debug, Clone)]
pub struct Constraint {
    /// Stable identity in generation order, including nested constraints.
    pub id: ConstraintId,
    /// Immutable root reason for this generated requirement.
    pub reason: ReasonId,
    pub span: Span,
    /// The source operation that required this constraint.
    pub origin: ConstraintOrigin,
    /// Source-facing names for the ordered operands carried by `kind`.
    pub subjects: ConstraintSubjects,
    pub kind: ConstraintKind,
}

/// Why generation emitted a constraint. This describes generation rather than
/// the solver rule that eventually handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintOrigin {
    Binding,
    ContextualCheck,
    ApplicationCallee,
    ApplicationArgument,
    ApplicationEffects,
    Raise,
    Projection,
    Match,
    MatchScrutinee,
    MatchArm,
    HandlerArm,
    HandlerReturn,
    HandlerFallback,
    Pattern,
    Instance,
    CallbackBoundary,
}

impl ConstraintOrigin {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::ContextualCheck => "contextual-check",
            Self::ApplicationCallee => "application-callee",
            Self::ApplicationArgument => "application-argument",
            Self::ApplicationEffects => "application-effects",
            Self::Raise => "raise",
            Self::Projection => "projection",
            Self::Match => "match",
            Self::MatchScrutinee => "match-scrutinee",
            Self::MatchArm => "match-arm",
            Self::HandlerArm => "handler-arm",
            Self::HandlerReturn => "handler-return",
            Self::HandlerFallback => "handler-fallback",
            Self::Pattern => "pattern",
            Self::Instance => "instance",
            Self::CallbackBoundary => "callback-boundary",
        }
    }
}

/// Source roles of a constraint's ordered operands. Unary and scoping
/// constraints use only `primary`.
#[derive(Debug, Clone, Copy, Eq)]
pub struct ConstraintSubjects {
    pub primary: Subject,
    pub primary_span: Option<Span>,
    pub secondary: Option<Subject>,
    pub secondary_span: Option<Span>,
}

impl PartialEq for ConstraintSubjects {
    fn eq(&self, other: &Self) -> bool {
        self.primary == other.primary && self.secondary == other.secondary
    }
}

impl ConstraintSubjects {
    pub const fn one(primary: Subject) -> Self {
        Self {
            primary,
            primary_span: None,
            secondary: None,
            secondary_span: None,
        }
    }

    pub const fn pair(primary: Subject, secondary: Subject) -> Self {
        Self {
            primary,
            primary_span: None,
            secondary: Some(secondary),
            secondary_span: None,
        }
    }

    /// Two source roles with the independent ranges that actually wrote them.
    /// `None` means the role is semantic context rather than source syntax and
    /// must not be turned into a diagnostic annotation.
    pub const fn pair_at(
        primary: Subject,
        primary_span: Option<Span>,
        secondary: Subject,
        secondary_span: Option<Span>,
    ) -> Self {
        Self {
            primary,
            primary_span,
            secondary: Some(secondary),
            secondary_span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
    Binding,
    Annotation,
    TopLevelBinding,
    LocalBinding,
    Context,
    Term,
    Callee,
    CallShape,
    Argument,
    Parameter,
    PerformedEffects,
    AmbientEffects,
    RaisedValue,
    RaiseResult,
    HandlerAnswer,
    ProjectionBase,
    ProjectionResult,
    MatchScrutinee,
    PatternDemand,
    MatchResult,
    MatchArm,
    HandlerArm,
    HandlerReturn,
    HandlerBody,
    Scheme,
    Instance,
    CallbackRequired,
    CallbackAvailable,
}

impl Subject {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Annotation => "annotation",
            Self::TopLevelBinding => "top-level-binding",
            Self::LocalBinding => "local-binding",
            Self::Context => "context",
            Self::Term => "term",
            Self::Callee => "callee",
            Self::CallShape => "call-shape",
            Self::Argument => "argument",
            Self::Parameter => "parameter",
            Self::PerformedEffects => "performed-effects",
            Self::AmbientEffects => "ambient-effects",
            Self::RaisedValue => "raised-value",
            Self::RaiseResult => "raise-result",
            Self::HandlerAnswer => "handler-answer",
            Self::ProjectionBase => "projection-base",
            Self::ProjectionResult => "projection-result",
            Self::MatchScrutinee => "match-scrutinee",
            Self::PatternDemand => "pattern-demand",
            Self::MatchResult => "match-result",
            Self::MatchArm => "match-arm",
            Self::HandlerArm => "handler-arm",
            Self::HandlerReturn => "handler-return",
            Self::HandlerBody => "handler-body",
            Self::Scheme => "scheme",
            Self::Instance => "instance",
            Self::CallbackRequired => "callback-required",
            Self::CallbackAvailable => "callback-available",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConstraintKind {
    /// Read one field from a base. The operation stays distinct from ordinary
    /// equality so a known non-struct can be diagnosed at the base.
    Project {
        base: Rc<Ty>,
        field: String,
        result: Rc<Ty>,
        base_span: Span,
    },
    /// Two types the program requires to be the same. `expected` is the side
    /// the context demanded — an annotation, a function's parameter, or the
    /// arrow shape a call site needs of something that is not one — and
    /// `actual` is what the term turned out to be, which is the order a
    /// mismatch is worded in.
    Equal { expected: Rc<Ty>, actual: Rc<Ty> },
    /// A name bound for the length of a body, and generalized before the body
    /// is looked at.
    ///
    /// The scoping a nested `let` needs, said in the constraint language rather
    /// than done by the walk — which is what keeps generation a description of
    /// the term and nothing else. See [`Constrain`].
    Let {
        symbol: Symbol,
        /// What the name stands for while its own value is walked: the
        /// annotation, or a variable standing for whatever the value turns out
        /// to be. Monomorphic there, so a recursive use is the one type being
        /// decided, and the type generalization is taken of once it is.
        bound: Rc<Ty>,
        /// The level the value was walked at. Everything still unbound at or
        /// above it when the value is solved is the value's to quantify.
        level: u32,
        /// The `where` clause the annotation promised, or [`Formula::True`]
        /// where none was written. What the scheme published for `symbol`
        /// requires of its presences: R10 makes the clause the contract, so a
        /// use of the name sees it rather than whatever the value worked out.
        promised: Formula,
        /// The variables the annotation declared, which are the
        /// ones the scheme this publishes may quantify. Empty where none was
        /// written. See [`ErrorKind::RigidEscapes`].
        rigids: Vec<u32>,
        /// What the value requires, including that it match the annotation when
        /// one was written. Solved first, at `level`.
        value: Vec<Constraint>,
        /// What the rest of the term requires, with `symbol` bound to the
        /// scheme generalization produced. Solved at `level - 1`.
        body: Vec<Constraint>,
    },
    /// A use of a let-bound name: `ty` is a fresh copy of whatever scheme the
    /// enclosing [`ConstraintKind::Let`] published for `symbol`.
    Instance {
        symbol: Symbol,
        ty: Rc<Ty>,
        /// Source-order store slot reserved during generation for the scheme
        /// requirement solving may discover.
        requirement: usize,
    },
    /// A qualifying presence-only match. Each arm owns the constraints and
    /// store requirements generated by its body; solving applies its ordered
    /// condition as a premise and constructs one structural result family.
    Match {
        scrutinee: Rc<Ty>,
        result: Rc<Ty>,
        arms: Vec<GuardedArm>,
        /// The first store slot generated after this match. Arm-boundary
        /// reachability ignores later source requirements.
        store_end: usize,
    },
    /// An application: what calling the function may perform, and what the
    /// place it is written in allows.
    ///
    /// The one rule that *opens* a row, and the whole of what makes an effect
    /// row an upper bound rather than a demand on the caller. A callee whose
    /// row is a variable takes whatever the ambient allows; one whose row is
    /// closed requires the ambient to allow at least its labels, and no more
    /// than that. See R12.
    ///
    /// Solved in emission order, after the [`ConstraintKind::Equal`] that
    /// determines the callee's row — so an unresolved bare row variable here is
    /// the variable-tail case, which is the right reading of "the callee
    /// performs whatever the ambient does".
    Performs {
        performed: Row,
        ambient: Row,
        /// Whether a `fn` encloses the application. What tells the two readings
        /// of a failure apart: outside every function the ambient is a
        /// definition's own, so nothing could ever have handled the effect,
        /// and inside one it is that function's row, which does not allow it.
        /// See R11.
        inside: bool,
    },
    /// An effectful callback crossing a foreign boundary must be callable with
    /// the evidence carried by that boundary.
    CallbackCoverage { required: Row, available: Row },
}

#[derive(Debug, Clone)]
pub struct Error {
    /// Stable identity allocated when the error is emitted. Source sorting moves
    /// but never renumbers it, and speculative identities are never reused.
    pub id: ErrorId,
    /// The compiler record that directly emitted this error. A direct error is
    /// produced by a boundary/final check rather than by a solve step or SAT
    /// batch.
    pub cause: ErrorCause,
    pub span: Span,
    pub kind: ErrorKind,
    /// Source-level account extracted from the immutable reason graph. Families
    /// not migrated yet deliberately leave this empty and use their established
    /// diagnostic wording.
    pub explanation: Option<InferenceExplanation>,
}

/// A reporter-independent explanation of an inference contradiction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceExplanation {
    /// Complete source facts in deterministic causal order.
    pub full_facts: Vec<ExplanationFact>,
    /// Indices into `full_facts` selected for the ordinary 2–4 fact view.
    pub abridged: Vec<usize>,
    pub contradiction: Contradiction,
    /// Both the source constraint slice and the unabridged raw reason slice.
    pub cause: ExplanationCause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationFact {
    pub span: Span,
    pub constraint: ConstraintId,
    pub origin: ConstraintOrigin,
    pub subject: Subject,
    pub payload: ExplanationFactPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExplanationFactPayload {
    RequiresType,
    UsedAsFunction,
    SuppliesArgument,
    BranchResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub kind: ContradictionKind,
    pub left: TypeDescription,
    pub right: TypeDescription,
    /// Neutral equality failures always retain both possible repair directions.
    pub repairs: [RepairDirection; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictionKind {
    IncompatibleTypes,
    ValueUsedAsFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDirection {
    ChangeFirstUse,
    ChangeSecondUse,
}

/// Deliberately source-facing and finite: no solver variable, row-tail, or
/// compiler-synthesized arrow can enter migrated diagnostic prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeDescription {
    NaturalNumber,
    Integer,
    RealNumber,
    Text,
    Boolean,
    Function,
    Struct,
    TaggedValue,
    DeclaredType,
    Undecided,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationCause {
    pub error: ErrorId,
    pub seed: Option<ReasonId>,
    pub constraints: Vec<ConstraintId>,
    pub reasons: Vec<ReasonId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCause {
    Step(StepId),
    Batch(BatchId),
    Direct,
}

impl Error {
    /// Construct an error for focused semantic/UI tests. Inference replaces
    /// this pending identity before publishing an [`Output`].
    pub fn new(span: Span, kind: ErrorKind) -> Self {
        Self {
            id: ErrorId::pending(),
            cause: ErrorCause::Direct,
            span,
            kind,
            explanation: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    /// A field was read from a value whose outer type is known not to be a struct.
    NotAStruct { base: Rc<Ty> },
    /// Two types that had to be equal are not. `expected` is the side the
    /// context demanded — an annotation, a function's parameter, or the arrow
    /// shape a call site needs — and `actual` is what the term turned out to
    /// be.
    Mismatch { expected: Rc<Ty>, actual: Rc<Ty> },
    /// The occurs check fired: a variable would have to contain itself, as in
    /// `fn x => x x`. The cycle is reported rather than constructed, so the
    /// type language stays finite trees. A row closes the same cycle when the
    /// only way to make two structs equal would put their shared tail inside
    /// itself.
    Recursive,
    /// A label demanded of a row that does not have it: a field a projection
    /// or a struct type requires, or a case a sum type requires. The name is
    /// carried rather than left to be read back out of the source, so the
    /// message can be written once here instead of once per reporter.
    ///
    /// Which of the two it is, is carried, because `base` no longer answers
    /// it. Only structs have fields *and* may have cases, so a base can be a
    /// sum type that is missing a *field* — `(#A 1).x` is exactly
    /// that, and reading the noun off the base would call it a case. The
    /// solver knows which row it was deciding at the moment it failed, so the
    /// shape is set there. One complaint, because it is one thing gone wrong —
    /// a row was asked for a label it does not have.
    MissingField {
        shape: Shape,
        base: Rc<Ty>,
        field: String,
    },
    /// A label the row has but the type it is against does not allow: a closed
    /// type lists every field — or case — there is, so one more is as wrong as
    /// one missing. Worded from the type rather than the label's own span
    /// because the type is the side that says what is allowed. The shape is
    /// carried for the reason [`ErrorKind::MissingField`]'s is.
    ///
    /// The base need not be a struct, now that only structs carry labels. A
    /// type whose closed struct row allows nothing more and which names none allows no label
    /// at all, so a
    /// projection's demand landing on the *actual* side of a goal against `Nat`
    /// is refused here rather than as a missing field: `let g = fn p => p.x` and
    /// then `let b : Nat -> Nat = g` reads ``extra field `x`: the type `Nat`
    /// lists every field it allows``, because the annotation is the expected side
    /// and what is being checked against it — demand and all — is `g`.
    ///
    /// Which of the two complaints a demand becomes is decided by which side of
    /// the goal it sits on, which is decided by where the annotation is and by
    /// nothing about the projection. Writing the same thing as one definition,
    /// `let b : Nat -> Nat = fn p => p.x`, puts the demand on the expected side —
    /// a projection is a demand on its base — and the annotation's `Nat` on the
    /// actual one, and the very same refusal comes out as ``no field `x` on
    /// `Nat` ``. See [`Solve::absorb`], which says this from the other end.
    ExtraField {
        shape: Shape,
        base: Rc<Ty>,
        field: String,
    },
    /// A body deciding what one of its annotation's variables is.
    ///
    /// `a` stands for whatever the caller picks, so a body that makes it a
    /// `Nat` — or makes it the *other* variable the annotation declared — has
    /// not written the function its signature promises. Refused at the
    /// expression that decided it rather than at the annotation, which is the
    /// whole point of skolemizing: the reader is sent to the line they can
    /// change instead of being told that a type several lines up is somehow
    /// wrong.
    ///
    /// `found` is what the expression turned out to be, `name` is the variable
    /// it was supposed to be, and `declared` is where that variable was
    /// declared — the second place a reporter points at, the way
    /// [`ir::ErrorKind::Duplicate`](crate::ir::ErrorKind) carries one.
    /// `sense` is the sort the variable was declared at, which is what the
    /// wording follows: a type is something the caller picks, a rest and a row
    /// of effects are something the caller allows.
    RigidBroken {
        found: Rc<Ty>,
        name: Rc<str>,
        sense: Sense,
        declared: Span,
    },
    /// A label demanded of a variable: a projection, a match arm, a
    /// struct literal's field set.
    ///
    /// What replaces the lacks bookkeeping for a rigid. An ordinary open row
    /// can be told which labels it may not stand for and then given the rest; a
    /// rigid can never be given anything at all, so every label demanded of one
    /// is refused outright, wherever the demand was written.
    RigidField {
        shape: Shape,
        field: String,
        name: Rc<str>,
        declared: Span,
    },
    /// A variable reaching a type outside the annotation that
    /// declared it.
    ///
    /// A rigid stands for whatever the caller of *that* annotation picks, so it
    /// means nothing anywhere else: a scheme quantifying one would be promising
    /// its own callers something only somebody else's caller decides. Reported
    /// at the declaring name, which is the line that has to change.
    RigidEscapes { name: Rc<str> },
    /// A `..` was decided to stand for a label the row it tails already names.
    /// `{ x: Nat, ..'r }` says "an `x`, plus whatever else `'r` is", so `'r`
    /// standing for anything that certainly has an `x` of its own would name
    /// the field twice — and the two copies could disagree. Certainly has,
    /// because presence is the whole of what a second copy is: an `x` settled
    /// absent is not part of what its type says, and one still being decided
    /// settles absent at the binding, so neither is refused. Only the label
    /// and what kind of row it was found in are carried: those are the two
    /// things both halves of the contradiction have in common, and the rows
    /// themselves are each half a type the reader never wrote down.
    RepeatedField { shape: Shape, field: String },
    /// A use of a definition whose type requires a combination of labels the
    /// value written there cannot have: `p {}` where `p` accepts exactly one of
    /// `x` and `y`.
    ///
    /// The one complaint unification could never make. Each label on its own is
    /// perfectly consistent — `{}` has neither, and neither is required alone —
    /// so nothing goes wrong until the two are asked about together, which is
    /// what the constraint store is for.
    ///
    /// The formula is carried already worded in the reader's own nouns: the
    /// labels their type names, rather than the presence variables the compiler
    /// gave them.
    PresenceRequired {
        formula: String,
        shape: Option<Shape>,
    },
    /// An annotation whose `where` clause nothing can satisfy once the
    /// definition under it has had its say.
    ///
    /// [`ErrorKind::PresenceRequired`]'s twin at the other end: there the
    /// contradiction is between a scheme and a use, here between an annotation
    /// and the body it is written over.
    ///
    /// Only when the clause has a model of its own and something earlier took
    /// it away. A clause with no model at all is
    /// [`ErrorKind::ClauseImpossible`] instead: blaming the definition for it
    /// would be blaming a body that may do nothing with the type whatever.
    PresenceImpossible { formula: String },
    /// An annotation whose `where` clause nothing can satisfy on its own
    /// terms — `where a and not a`, which forbids every value at once.
    ///
    /// [`ErrorKind::PresenceImpossible`]'s other half, split from it because
    /// the two blame different things. There the clause is fine and the
    /// definition is what leaves it without a model; here nothing outside the
    /// clause is involved, and a complaint that mentioned the definition would
    /// be asserting something untrue of it.
    ClauseImpossible { formula: String },
    /// An annotation whose `where` clause allows more than the definition
    /// under it does — `where a or b` over a body that needs `a`.
    ///
    /// The annotation is the contract, and every use of the name sees it, so a
    /// body requiring more than it promises would let a use through that the
    /// definition cannot serve. Refused at the annotation, which is the line
    /// the reader has to change: both formulas are carried so the complaint can
    /// show what was promised beside what is needed.
    AnnotationAllows { allowed: String, required: String },
    /// An effect performed where nothing could ever handle it: a definition's
    /// value is computed outside every handler, so an effect performed there
    /// has no one to answer it.
    ///
    /// [`ErrorKind::NotAllowed`]'s other reading, and the two are split because
    /// they send the reader to different places: here there is no function to
    /// widen and the fix is to wrap the value in one, or in a handler.
    Unhandled { effect: String },
    /// An effect performed inside a function whose own row does not allow it.
    ///
    /// The ordinary refusal, and the reason an effect row is worth having: the
    /// function says what calling it may do, and this would do more.
    NotAllowed { effect: String },
    /// A foreign callback requires evidence the containing host call cannot carry.
    CallbackEffectsNotCovered,
    /// An ordinary foreign boundary leaf has no fixed runtime representation:
    /// an annotation variable there could instantiate to a Ruddy closure.
    PolymorphicExternBoundary,
}

/// What one type variable is known to be. Private to inference, and rightly so:
/// it is the solver's working state rather than part of the type language, and
/// nothing downstream ever sees a [`Ty::Var`] to want a slot for — generalizing
/// and zonking are what make sure of that.
#[derive(Debug, Clone)]
enum Slot {
    Unbound,
    Bound { value: Assigned, by: ReasonId },
}

/// What one variable may not stand for: the labels, and the kind of row the
/// condition came from.
///
/// The shape is stored rather than read back off wherever the variable ends up,
/// because there is no longer anywhere to read it from — a row-tail variable stands
/// for a whole type, and the labels forbidden of it are that type's fields. See
/// [`Table::lacks`].
type Lacks = (Shape, IndexSet<String>);

/// Everything speculative solving can change about a [`Table`]. Taken and put
/// back by [`Rule::Congruent`], which is the one rule that asks a question
/// before it is sure the question is the right one to have asked.
///
/// The levels travel with the slots rather than beside them: the two lists are
/// indexed by the same variable, so putting one back without the other would
/// leave a variable minted since the snapshot with a level and no slot. The
/// existential sets travel with them too: presence aliasing propagates sealed
/// identity, and opening a fresh package records its new variable as both a
/// witness and abstract. The remaining semantic side tables are either read
/// only during unification or have their own congruence rollback in [`Solve`].
struct Known {
    vars: Vec<Slot>,
    var_meta: Vec<VarMeta>,
    levels: Vec<u32>,
    lacks: HashMap<TyVar, Lacks>,
    existential_witnesses: HashSet<TyVar>,
    abstract_existentials: HashSet<TyVar>,
    reason_len: usize,
}

/// What one name in scope means. Private for the same reason as [`Slot`]: a
/// binding exists only while a definition is being walked, and what survives
/// the walk is the [`Scheme`] in [`Output::schemes`].
#[derive(Debug, Clone)]
enum Binding {
    Mono(Rc<Ty>),
    Poly(Scheme),
    /// A name a nested `let` bound, whose scheme only the solver will know.
    ///
    /// Generation cannot name what a use of one is a copy of: the scheme is
    /// what solving the value produces, and generation has solved nothing. So
    /// it says only that this use is a copy — a
    /// [`ConstraintKind::Instance`] — and the solver, which by then has the
    /// scheme, makes it.
    Local,
}

/// Which side of a goal a row's tail sits on. [`Solve::unify`] decomposes
/// without swapping, so an act performed on a tail's behalf — a binding, a
/// field settled absent, a mismatch — has to know which way round to say
/// itself, or a complaint about an annotation would read as one about the
/// term.
#[derive(Debug, Clone, Copy)]
enum Side {
    Expected,
    Actual,
}

/// One member of a binding group as it goes into scope, before anything in the
/// group has been walked.
///
/// Which is the whole of what makes recursion typable here: what the rest of
/// the group — and the definition itself — sees this name as has to exist
/// before any body mentioning it is read. See [`Binding::Mono`].
struct Scoped {
    symbol: Symbol,
    /// What the definition is bound to for the length of the group: its
    /// lowered annotation, or a variable standing for whatever the body turns
    /// out to be.
    bound: Rc<Ty>,
    /// The variables its annotation declared, which are the ones
    /// its scheme is entitled to quantify. Empty for a definition with no
    /// annotation, which is entitled to none. See
    /// [`ErrorKind::RigidEscapes`].
    rigids: Vec<u32>,
    /// What the annotation's `where` clause promised, over the presence
    /// variables the annotation minted. [`Formula::True`] for a definition with
    /// no clause, which promises nothing.
    promised: Formula,
    /// The presence names the annotation bound, and what each one lowered to —
    /// what a complaint about the clause quotes it in, since the reader wrote
    /// `a` and never saw the variable.
    names: Vec<(String, Presence)>,
}

/// One annotation written on a nested `let`, kept aside while the definition it
/// is in is solved.
///
/// The one thing about a nested binding that the constraint language has no
/// room for, and rightly so: whether an annotation's clause allows more than
/// the value under it needs is a question about a *written type*, asked once
/// the solve is over, and the solver has never seen a written anything.
/// [`Scoped::promised`] is the same pair about a definition's own annotation,
/// checked in the same place.
struct Annotated {
    /// The annotation's span, which is the line the reader has to change.
    span: Span,
    /// Ordered qualifying-arm premise in force where it was written.
    guard: Formula,
    /// The `where` clause it promised, or [`Formula::True`] where none was
    /// written. The clause the value under it is held to, exactly as a
    /// definition's own is. See [`Scoped::promised`].
    promised: Formula,
    /// The labels its `when`s named, so a complaint about the clause quotes it
    /// in the nouns the reader wrote. See [`Scoped::names`].
    names: Vec<(String, Presence)>,
}

/// One member of a binding group once it has been walked and solved, waiting
/// for the group to end so that it can be generalized.
struct Solved {
    scoped: Scoped,
    /// The type this definition publishes: its annotation, which is the
    /// contract, or what its body turned out to be.
    ty: Rc<Ty>,
    /// What generation asked of it, kept exactly as it was asked. See
    /// [`Output::constraints`].
    generated: Vec<Constraint>,
    /// Every annotation written on a nested `let` inside it. See [`Annotated`].
    annotated: Vec<Annotated>,
    /// Where the schemes this definition's nested lets published begin in the
    /// shared list, so that each can be numbered for printing once the group is
    /// solved. See [`Table::published`].
    locals: usize,
    /// Where this definition's own complaints begin in the shared list. Each is
    /// resolved against the substitution its own definition ends with, so which
    /// ones are whose has to be marked before the next member's solve appends
    /// to the list.
    reported: usize,
}

/// Every type variable ever minted.
///
/// Both passes hold this: generation mints into it, solving binds in it, and
/// generalization reads it. It is the only state that outlives a pass.
#[derive(Clone)]
struct PackageGuarantee {
    /// Distinct owned conjuncts in stable registration order. A coherent local
    /// package can be instantiated thousands of times; set insertion makes
    /// repeating its identical guarantee idempotent instead of growing an
    /// ever-deeper conjunction (and cloning that conjunction on every use).
    clauses: IndexSet<Formula>,
    /// Nested arrow results are production events and open freshly each time;
    /// a root package denotes one lexical value and keeps one coherent view.
    fresh: bool,
}

#[derive(Default)]
struct Table {
    /// One slot per variable; [`Ty::Var`] indexes into it.
    ///
    /// A group rather than a definition, and the difference is only where the
    /// line falls: two definitions that name each other are solved together, so
    /// a variable one of them minted may still be open while the other is being
    /// walked. It is still nobody's but the group's, because the group is
    /// finished before anything outside it is looked at.
    vars: Vec<Slot>,
    /// Source-semantic metadata parallel to `vars`.
    var_meta: Vec<VarMeta>,
    /// The generalization level each variable was minted at, parallel to
    /// `vars`. A binding group is level 0, a nested `let`'s value is one deeper
    /// than whatever it was written in, and everything still unbound at or
    /// above a level when the thing that owns it is solved is that thing's to
    /// quantify.
    ///
    /// Rémy's levels, and the whole of what a nested `let` had to bring back
    /// with it. Where a definition could not nest there was nothing to decide:
    /// every variable still unsolved when a group ended was minted by that
    /// group. Now `fn p => let q = p.x in q` mints the field's variable inside
    /// the let and must still not quantify it — see [`Table::demote`], which is
    /// what puts it back where it belongs, and why a range of minted variables
    /// is no substitute for this.
    levels: Vec<u32>,
    /// The level being walked, or solved, at. Both passes set it: generation
    /// raises it over a nested `let`'s value, and solving sets it from the
    /// level each [`ConstraintKind::Let`] carries, so a variable minted at
    /// either point belongs to wherever it was written.
    level: u32,
    /// What each variable may not stand for: the labels already written out
    /// beside the open end it sits at.
    ///
    /// `{ x: Nat, ..?3 }` reads "an `x`, and whatever else `?3` is", so a `?3`
    /// standing for a type with an `x` of its own would give the type two
    /// fields of one name. This is now the *only* way a struct's condition is
    /// recorded, and the row-tail variable is the only place left to put it: there
    /// is no tail variable beside it any more, because the constructor is the tail.
    /// Nothing in [`Ty`] can express the side condition, so it is held here,
    /// beside the slots, and enforced at the one place a variable acquires a
    /// value.
    ///
    /// A sum's tail is under the same condition, for the same reason and with
    /// its own labels: `#A Nat | ..?3` reads the same sentence about cases.
    /// So the shape says which of the two a condition came from, and a
    /// [`Shape::Struct`] one is always on a row-tail variable while a
    /// [`Shape::Sum`] one is always on a tail.
    ///
    /// Insertion-ordered, so that a value breaking the rule twice always names
    /// the same label first and the complaint does not depend on a hash.
    lacks: HashMap<TyVar, Lacks>,
    /// What each declaration's parameters stand for, in the declaration's own
    /// order. Read only by [`Table::note_lacks`], and only for the one thing a
    /// [`ParamKind`] carries that a type cannot: the labels the declaration
    /// already names beside that parameter, which whatever is written there may
    /// not name either.
    ///
    /// Held beside the variables rather than looked up through
    /// [`Solve::aliases`] because it is a fact about the *written* declaration
    /// and survives no unfolding: by the time a body has been opened, the tail
    /// its parameter sat in is an ordinary row and the condition has to have
    /// been said already.
    params: HashMap<Symbol, Vec<ParamKind>>,
    /// Least declared-type variance fixpoint, shared with inferred presence
    /// classification so a named argument is never assumed covariant merely
    /// because its representation has not been unfolded here.
    variances: HashMap<(Symbol, u32), u8>,
    /// Producer-owned annotation presences. Unification may establish a
    /// witness equation for these while checking the producer, but must not
    /// overwrite their identity: publication has to conceal that equation.
    existential_witnesses: HashSet<TyVar>,
    /// Sealed producer-owned identities opened from a published scheme. Unlike
    /// witnesses while their producer is checked, these may only be observed:
    /// a consumer may use an equation already guaranteed by the package but
    /// may not add a new equation choosing the hidden presence.
    abstract_existentials: HashSet<TyVar>,
    /// A non-function local binding opens its produced package once. Every use
    /// of that binding must observe the same abstract identity; re-instantiating
    /// its existential slots would let one match refinement leak or disappear
    /// between projections of the same value.
    local_package_instances: HashMap<(Symbol, u32), Presence>,
    /// Guarantees are inert until their exact package is destroyed. The key is
    /// the package's producer-owned presence identities: scheme instantiation
    /// mints these afresh, while cloning and substitution preserve them. Unlike
    /// an `Rc` address this cannot alias after an allocation is released, and
    /// the whole map dies with this inference table.
    package_guarantees: HashMap<Vec<TyVar>, PackageGuarantee>,
    /// What the program has required of its presences so far. Grown by
    /// generation (a match's coverage), by instantiation (a constrained
    /// scheme's formula) and by lowering (an annotation's `where` clause), and
    /// consulted only where R8 allows: at a generalization boundary, at the
    /// use-site and annotation checks, and — through
    /// [`Output::store`] — in the patterns phase. Never by unification.
    store: Store,
    /// Where each a variable variable in the program was declared, by the id
    /// its annotation gave it.
    ///
    /// A [`Ty::Rigid`] carries its spelling but not its span — a type is
    /// printed with no table beside it, and a span is not something a reader
    /// reads — so the second place a rigid complaint points at is looked up
    /// here. Program-global, exactly as the ids are.
    rigids: HashMap<u32, Span>,
    /// The rigids already reported as escaping. One mistake said once: a
    /// variable that reaches two schemes it does not belong to is still one
    /// annotation to rewrite. See [`ErrorKind::RigidEscapes`].
    escaped: HashSet<u32>,
    /// Top-level definition currently being generated or solved, stamped onto
    /// store batches for source-order cascade decisions downstream.
    definition: Option<Symbol>,
    /// Generation-time batch slots currently held inert by qualifying arms.
    /// Nested arms claim their own slots first; an enclosing arm claims only
    /// the still-unclaimed coverage/requirements around them.
    deferred: HashSet<usize>,
    /// Reserved local-instance slots whose eventual scheme required nothing.
    /// They stay as inert internal placeholders until publication, when they
    /// are omitted from the store readers see.
    empty_batches: HashSet<usize>,
    /// Whether some batch has already flipped the store unsatisfiable. The
    /// cascade rule: that batch owns the single resulting error, and every
    /// later SAT-dependent complaint — a use-site violation, an annotation
    /// disagreement, a scheme's `where` clause — is suppressed, because all of
    /// them would be the same contradiction said again.
    unsat: bool,
    /// Append-only identity arenas. These counters are deliberately absent from
    /// `snapshot`/`restore`: speculative records are retired on rollback rather
    /// than letting a later record inherit an observed identity.
    next_constraint_id: u64,
    next_step_id: u64,
    next_error_id: u64,
    next_batch_id: u64,
    /// Immutable append-only causal records. Unlike variable metadata this is
    /// deliberately not restored after speculative congruence.
    reasons: Vec<Reason>,
    next_reason_id: u64,
    /// Binding reasons observed during one solver act. `None` outside a solve
    /// makes publication, generalization, and zonking incapable of leaking
    /// incidental reads into a later step.
    causal_reads: RefCell<Option<Vec<IndexSet<ReasonId>>>>,
    causal_scope_depth: usize,
}

/// Which quantified position each thing a scheme closes over was given, in the
/// one index space a scheme has.
///
/// Three maps rather than one because three different things are being
/// numbered — a solver variable standing for a type or a row, a solver variable
/// standing for a presence, and a rigid, which is no variable at all and is
/// keyed by its own id — and no key of one is ever a key of another. The
/// numbers they hand out come from one pool: the presences take `0..presences`
/// and everything else the rest, which is what [`Scheme`] promises.
#[derive(Debug, Default, Clone)]
struct Subst {
    types: HashMap<TyVar, u32>,
    presences: HashMap<TyVar, u32>,
    /// The variables an annotated definition's own scheme
    /// re-quantifies: rigid while the body was checked, and an ordinary
    /// quantified position once it has been. See [`ErrorKind::RigidEscapes`]
    /// for the one that may not be.
    rigids: HashMap<u32, u32>,
}

impl Subst {
    /// The next position for something that is not a presence. The presences
    /// are numbered first and in full, so what is left is one running count
    /// over the two maps that share the high end of the space.
    fn next(&self) -> u32 {
        (self.presences.len() + self.types.len() + self.rigids.len()) as u32
    }
}

/// Exercise the match-result family builder without constructing a source
/// program. This is public so integration regressions can feed it semantic
/// types whose depth would make source syntax itself the thing under test.
#[doc(hidden)]
pub fn structural_family_for_tests(
    definition: Symbol,
    aliases: &IndexMap<Symbol, Scheme>,
    types: &[Rc<Ty>],
) -> Rc<Ty> {
    let mut table = Table::default();
    let mut errors = Vec::new();
    let mut steps = Vec::new();
    let nominal = HashSet::new();
    let mut locals = IndexMap::new();
    let mut refinements = Vec::new();
    Solve {
        table: &mut table,
        errors: &mut errors,
        steps: &mut steps,
        aliases,
        nominal: &nominal,
        definition,
        depth: 0,
        constraint: None,
        constraint_reason: None,
        assumed: Vec::new(),
        schemes: HashMap::new(),
        locals: &mut locals,
        guard: None,
        active_refinement: None,
        guard_reasons: Vec::new(),
        refinements: &mut refinements,
        generated_end: 0,
    }
    .family_type(types)
}

/// Exercise inferred package placement over semantic rows assembled directly
/// by integration regressions.
#[doc(hidden)]
pub fn package_positive_presences_for_tests(
    body: &Rc<Ty>,
    presences: u32,
    variances: &HashMap<(Symbol, u32), u8>,
) -> (Rc<Ty>, IndexSet<u32>) {
    package_positive_presences(body, presences, &IndexSet::new(), variances)
}

/// Alternately erase transparent packages and unfold transparent names until
/// neither operation exposes another wrapper.
fn expose_packages(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
    let mut exposed = ty.clone();
    loop {
        while let Ty::Package(body) = &*exposed {
            exposed = body.clone();
        }
        let next = unfold(aliases, &exposed);
        if Rc::ptr_eq(&next, &exposed) {
            return exposed;
        }
        exposed = next;
    }
}

/// Find an ordinary boundary leaf whose outer runtime representation remains
/// polymorphic. Such a leaf cannot be adapted once at declaration lowering:
/// in particular, an instantiation to an arrow would otherwise send a Ruddy
/// evidence-taking closure directly to the host.
fn polymorphic_extern_boundary(
    aliases: &IndexMap<Symbol, Scheme>,
    abi: &ir::ExternType,
    ty: &Rc<Ty>,
) -> Option<Span> {
    fn boundary(
        aliases: &IndexMap<Symbol, Scheme>,
        abi: &ir::ExternType,
        ty: &Rc<Ty>,
        seen: &mut Vec<(*const ir::ExternType, Rc<Ty>)>,
    ) -> Option<Span> {
        let state = abi as *const ir::ExternType;
        if seen.iter().any(|(prior_state, prior_ty)| {
            *prior_state == state && same_finite_syntax(prior_ty, ty)
        }) {
            return None;
        }
        seen.push((state, ty.clone()));
        match &abi.tracked {
            ir::ExternTypeKind::Group(inner) => boundary(aliases, inner, ty, seen),
            ir::ExternTypeKind::Function {
                parameters, result, ..
            } => {
                let mut cursor = ty.clone();
                for parameter in parameters {
                    let exposed = expose_packages(aliases, &cursor);
                    let Ty::Arrow(from, to, _) = &*exposed else {
                        return None;
                    };
                    if let Some(span) = boundary(aliases, parameter, from, seen) {
                        return Some(span);
                    }
                    cursor = to.clone();
                }
                // A nullary marked function is represented by the desugared
                // unit arrow, but its unit input is synthetic rather than a
                // boundary leaf written by the reader.
                if parameters.is_empty() {
                    let exposed = expose_packages(aliases, &cursor);
                    let Ty::Arrow(_, to, _) = &*exposed else {
                        return None;
                    };
                    cursor = to.clone();
                }
                boundary(aliases, result, &cursor, seen)
            }
            ir::ExternTypeKind::Ordinary(_) => match &*expose_packages(aliases, ty) {
                Ty::Package(_) => unreachable!("package exposure reaches a fixed point"),
                Ty::Bound(_) | Ty::Var(_) | Ty::Rigid { .. } => Some(abi.span),
                Ty::Arrow(from, to, _) => {
                    boundary(aliases, abi, from, seen).or_else(|| boundary(aliases, abi, to, seen))
                }
                // Primitive and structural outer representations are fixed.
                // Their members do not individually cross the function ABI.
                Ty::Nat
                | Ty::Int
                | Ty::Real
                | Ty::String
                | Ty::Boolean
                | Ty::Struct(_)
                | Ty::Sum(_)
                | Ty::Undecided
                | Ty::Named { .. } => None,
            },
        }
    }

    boundary(aliases, abi, ty, &mut Vec::new())
}

/// Build callback evidence obligations from the resolved semantic type. The
/// ABI tree contributes only host grouping; aliases, row tails and conditional
/// presences all come from the same rows ordinary inference solves.
fn callback_coverage_constraints(
    aliases: &IndexMap<Symbol, Scheme>,
    abi: &ir::ExternType,
    ty: &Rc<Ty>,
) -> (Vec<Constraint>, Formula) {
    fn presence_formula(presence: &Presence) -> Option<Formula> {
        match presence {
            Presence::Present => Some(Formula::True),
            Presence::Absent => Some(Formula::False),
            Presence::Var(var) => Some(Formula::Atom(Atom::Var(*var))),
            Presence::Bound(bound) => Some(Formula::Atom(Atom::Bound(*bound))),
            Presence::Recovered(_) | Presence::Undecided => None,
        }
    }

    fn expose(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
        let mut ty = unfold(aliases, ty);
        while let Ty::Package(body) = &*ty {
            ty = unfold(aliases, body);
        }
        ty
    }

    fn callback_rows(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut cursor = ty.clone();
        let mut seen = Vec::new();
        loop {
            if matches!(&*cursor, Ty::Named { .. }) {
                if seen.iter().any(|prior| same_finite_syntax(prior, &cursor)) {
                    break;
                }
                seen.push(cursor.clone());
            }
            let exposed = expose(aliases, &cursor);
            let Ty::Arrow(_, to, row) = &*exposed else {
                break;
            };
            rows.push(row.clone());
            cursor = to.clone();
        }
        rows
    }

    fn cover(
        aliases: &IndexMap<Symbol, Scheme>,
        span: Span,
        callback: &Rc<Ty>,
        available: &Row,
        out: &mut Vec<Constraint>,
    ) {
        out.extend(
            callback_rows(aliases, callback)
                .into_iter()
                .map(|required| Constraint {
                    id: ConstraintId::pending(),
                    reason: ReasonId::pending(),
                    span,
                    origin: ConstraintOrigin::CallbackBoundary,
                    subjects: ConstraintSubjects::pair(
                        Subject::CallbackRequired,
                        Subject::CallbackAvailable,
                    ),
                    kind: ConstraintKind::CallbackCoverage {
                        required,
                        available: available.clone(),
                    },
                }),
        );
    }

    fn boundary(
        aliases: &IndexMap<Symbol, Scheme>,
        abi: &ir::ExternType,
        ty: &Rc<Ty>,
        out: &mut Vec<Constraint>,
        seen: &mut Vec<(*const ir::ExternType, Rc<Ty>)>,
    ) {
        let state = abi as *const ir::ExternType;
        if seen.iter().any(|(prior_state, prior_ty)| {
            *prior_state == state && same_finite_syntax(prior_ty, ty)
        }) {
            return;
        }
        seen.push((state, ty.clone()));
        match &abi.tracked {
            ir::ExternTypeKind::Group(inner) => boundary(aliases, inner, ty, out, seen),
            ir::ExternTypeKind::Function {
                parameters, result, ..
            } => {
                let mut cursor = ty.clone();
                let mut inputs = Vec::new();
                let mut available = Row::closed();
                for _ in 0..parameters.len().max(1) {
                    let exposed = expose(aliases, &cursor);
                    let Ty::Arrow(from, to, row) = &*exposed else {
                        return;
                    };
                    if !parameters.is_empty() {
                        inputs.push(from.clone());
                    }
                    available = row.clone();
                    cursor = to.clone();
                }
                for (parameter, input) in parameters.iter().zip(inputs) {
                    cover(aliases, parameter.span, &input, &available, out);
                    boundary(aliases, parameter, &input, out, seen);
                }
                boundary(aliases, result, &cursor, out, seen);
            }
            ir::ExternTypeKind::Ordinary(_) => {
                let exposed = expose(aliases, ty);
                let Ty::Arrow(from, to, available) = &*exposed else {
                    return;
                };
                cover(aliases, abi.span, from, available, out);
                // An ordinary leaf may conceal arbitrarily much foreign shape
                // behind aliases. Every arrow remains a unary host boundary.
                boundary(aliases, abi, from, out, seen);
                boundary(aliases, abi, to, out, seen);
            }
        }
    }

    let mut out = Vec::new();
    boundary(aliases, abi, ty, &mut out, &mut Vec::new());
    let conditional = Formula::all(out.iter().flat_map(|constraint| {
        let ConstraintKind::CallbackCoverage {
            required,
            available,
        } = &constraint.kind
        else {
            return None;
        };
        Some(Formula::all(required.labels.iter().filter_map(
            |(name, required)| {
                let required = presence_formula(&required.presence)?;
                let available = match available.labels.get(name) {
                    Some(field) => presence_formula(&field.presence)?,
                    None if matches!(
                        available.rest,
                        Rest::Closed | Rest::Bound(_) | Rest::Rigid { .. }
                    ) =>
                    {
                        Formula::False
                    }
                    None => return None,
                };
                Some(required.not().or(available))
            },
        )))
    }));
    (out, conditional)
}

fn describe_type(ty: &Rc<Ty>) -> TypeDescription {
    let mut ty = ty;
    while let Ty::Package(inner) = &**ty {
        ty = inner;
    }
    match &**ty {
        Ty::Nat => TypeDescription::NaturalNumber,
        Ty::Int => TypeDescription::Integer,
        Ty::Real => TypeDescription::RealNumber,
        Ty::String => TypeDescription::Text,
        Ty::Boolean => TypeDescription::Boolean,
        Ty::Arrow(..) => TypeDescription::Function,
        Ty::Struct(..) => TypeDescription::Struct,
        Ty::Sum(..) => TypeDescription::TaggedValue,
        Ty::Named { .. } => TypeDescription::DeclaredType,
        Ty::Bound(_) | Ty::Var(_) | Ty::Rigid { .. } | Ty::Undecided => TypeDescription::Undecided,
        Ty::Package(_) => unreachable!("packages were removed iteratively"),
    }
}

/// Pick the first incompatible semantic leaf on an explicit stack. The walk
/// follows every payload-bearing type position and unfolds declarations one
/// layer at a time; malformed/missing alias provenance is reported honestly as
/// a declared or undecided type rather than guessed from solver variables.
fn smallest_incompatible(
    aliases: &IndexMap<Symbol, Scheme>,
    left: &Rc<Ty>,
    right: &Rc<Ty>,
) -> (TypeDescription, TypeDescription) {
    let mut work = vec![(left.clone(), right.clone())];
    let mut fallback = (describe_type(left), describe_type(right));
    while let Some((mut left, mut right)) = work.pop() {
        while let Ty::Package(inner) = &*left {
            left = inner.clone();
        }
        while let Ty::Package(inner) = &*right {
            right = inner.clone();
        }

        // Names are source spelling, not semantic leaves. Only unfold when the
        // published declaration is actually available; imported recovery holes
        // retain the honest `DeclaredType` fallback.
        if matches!(&*left, Ty::Named { symbol, .. } if aliases.contains_key(symbol)) {
            left = unfold(aliases, &left);
        }
        if matches!(&*right, Ty::Named { symbol, .. } if aliases.contains_key(symbol)) {
            right = unfold(aliases, &right);
        }

        let descriptions = (describe_type(&left), describe_type(&right));
        fallback = descriptions;
        match (&*left, &*right) {
            (Ty::Arrow(l_from, l_to, l_effects), Ty::Arrow(r_from, r_to, r_effects)) => {
                push_row_payloads(&mut work, l_effects, r_effects);
                // Source order: parameter, result, then effects.
                work.push((l_to.clone(), r_to.clone()));
                work.push((l_from.clone(), r_from.clone()));
            }
            (Ty::Struct(left), Ty::Struct(right)) | (Ty::Sum(left), Ty::Sum(right)) => {
                push_row_payloads(&mut work, left, right);
                if left.labels.keys().ne(right.labels.keys()) {
                    return descriptions;
                }
            }
            (
                Ty::Named {
                    symbol: l_symbol,
                    args: l_args,
                    ..
                },
                Ty::Named {
                    symbol: r_symbol,
                    args: r_args,
                    ..
                },
            ) if l_symbol == r_symbol && l_args.len() == r_args.len() => {
                for (left, right) in l_args.iter().zip(r_args.iter()).rev() {
                    work.push((left.clone(), right.clone()));
                }
            }
            (Ty::Nat, Ty::Nat)
            | (Ty::Int, Ty::Int)
            | (Ty::Real, Ty::Real)
            | (Ty::String, Ty::String)
            | (Ty::Boolean, Ty::Boolean)
            | (Ty::Bound(_), Ty::Bound(_))
            | (Ty::Var(_), Ty::Var(_))
            | (Ty::Rigid { .. }, Ty::Rigid { .. })
            | (Ty::Undecided, Ty::Undecided) => {}
            _ => return descriptions,
        }
    }
    fallback
}

fn push_row_payloads(work: &mut Vec<(Rc<Ty>, Rc<Ty>)>, left: &Row, right: &Row) {
    // Reverse insertion order so the first written common label is visited
    // first by the LIFO work list. Presence/rest incompatibilities have no type
    // leaf; their dedicated row diagnostics remain the truthful fallback.
    let common: Vec<_> = left
        .labels
        .iter()
        .filter_map(|(name, field)| right.labels.get(name).map(|other| (&field.ty, &other.ty)))
        .collect();
    for (left, right) in common.into_iter().rev() {
        work.push((left.clone(), right.clone()));
    }
}

fn all_constraints(
    constraints: &IndexMap<Symbol, Vec<Constraint>>,
) -> HashMap<ConstraintId, &Constraint> {
    let mut out = HashMap::new();
    let mut work: Vec<&Constraint> = constraints.values().flatten().collect();
    while let Some(constraint) = work.pop() {
        if out.insert(constraint.id, constraint).is_some() {
            continue;
        }
        match &constraint.kind {
            ConstraintKind::Let { value, body, .. } => {
                work.extend(value);
                work.extend(body);
            }
            ConstraintKind::Match { arms, .. } => {
                for arm in arms {
                    work.extend(&arm.constraints);
                    work.push(&arm.result);
                }
            }
            _ => {}
        }
    }
    out
}

fn attach_mismatch_explanations(
    errors: &mut [Error],
    constraints: &IndexMap<Symbol, Vec<Constraint>>,
    steps: &[Step],
    reasons: &[Reason],
    aliases: &IndexMap<Symbol, Scheme>,
) {
    let constraints = all_constraints(constraints);
    let steps: HashMap<_, _> = steps.iter().map(|step| (step.id, step)).collect();
    let reasons_by_id: HashMap<_, _> = reasons.iter().map(|reason| (reason.id, reason)).collect();

    for error in errors {
        let ErrorKind::Mismatch { expected, actual } = &error.kind else {
            continue;
        };
        let seed = match error.cause {
            ErrorCause::Step(id) => steps.get(&id).map(|step| step.reason),
            ErrorCause::Batch(_) | ErrorCause::Direct => None,
        };

        // Iterative, parent-order DFS. IDs are immutable and parents precede
        // children, so this is deterministic even when bindings share causes.
        let mut reason_slice = Vec::new();
        let mut constraint_slice = Vec::new();
        let mut seen_reasons = HashSet::new();
        let mut seen_constraints = HashSet::new();
        let mut work: Vec<ReasonId> = seed.into_iter().collect();
        while let Some(id) = work.pop() {
            if !seen_reasons.insert(id) {
                continue;
            }
            let Some(reason) = reasons_by_id.get(&id).filter(|reason| reason.reachable) else {
                continue;
            };
            reason_slice.push(id);
            if let ReasonOrigin::Constraint(id) = reason.origin
                && seen_constraints.insert(id)
            {
                constraint_slice.push(id);
            }
            work.extend(reason.parents.iter().rev().copied());
        }

        let mut full_facts = Vec::new();
        for id in &constraint_slice {
            let Some(constraint) = constraints.get(id) else {
                continue;
            };
            let endpoints = [
                (
                    constraint.subjects.primary,
                    constraint.subjects.primary_span,
                ),
                (
                    constraint
                        .subjects
                        .secondary
                        .unwrap_or(constraint.subjects.primary),
                    constraint.subjects.secondary_span,
                ),
            ];
            for (subject, span) in endpoints {
                let Some(span) = span else { continue };
                let payload = match (constraint.origin, subject) {
                    (ConstraintOrigin::ApplicationCallee, Subject::Callee) => {
                        ExplanationFactPayload::UsedAsFunction
                    }
                    (ConstraintOrigin::ApplicationArgument, _) => {
                        ExplanationFactPayload::SuppliesArgument
                    }
                    (ConstraintOrigin::MatchArm | ConstraintOrigin::Match, _) => {
                        ExplanationFactPayload::BranchResult
                    }
                    _ => ExplanationFactPayload::RequiresType,
                };
                full_facts.push(ExplanationFact {
                    span,
                    constraint: *id,
                    origin: constraint.origin,
                    subject,
                    payload,
                });
            }
        }
        // A reason without a written endpoint cannot support source labels.
        // Keep the established mismatch diagnostic rather than inventing
        // pending contextual facts and claiming they came from the program.
        if full_facts.is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        let mut included = HashSet::new();
        for (at, fact) in full_facts.iter().enumerate() {
            // Repeated roles at different source ranges are different uses.
            // Suppress only literal duplicate annotations from shared paths.
            if included.insert((fact.span, fact.constraint, fact.subject)) {
                candidates.push(at);
            }
        }
        // The failed requirement is first in the reason walk and its oldest
        // conflicting source requirement is last. Keep both even on long paths,
        // filling the bounded ordinary view with nearby causal context.
        let mut abridged: Vec<_> = candidates.iter().take(4).copied().collect();
        if candidates.len() > 4 {
            abridged[3] = *candidates.last().unwrap();
        }
        // Keep the failed source endpoint primary; the other genuinely written
        // causes remain related labels even when their reasons precede it.
        abridged.sort_by_key(|at| full_facts[*at].span != error.span);
        let leaf = smallest_incompatible(aliases, expected, actual);
        let failing_constraint = match error.cause {
            ErrorCause::Step(id) => steps.get(&id).and_then(|step| step.constraint),
            ErrorCause::Batch(_) | ErrorCause::Direct => None,
        };
        let value_used_as_function = failing_constraint
            .and_then(|id| constraints.get(&id))
            .is_some_and(|constraint| {
                constraint.origin == ConstraintOrigin::ApplicationCallee
                    && (leaf.0 == TypeDescription::Function || leaf.1 == TypeDescription::Function)
            });
        error.explanation = Some(InferenceExplanation {
            full_facts,
            abridged,
            contradiction: Contradiction {
                kind: if value_used_as_function {
                    ContradictionKind::ValueUsedAsFunction
                } else {
                    ContradictionKind::IncompatibleTypes
                },
                left: leaf.0,
                right: leaf.1,
                repairs: [
                    RepairDirection::ChangeFirstUse,
                    RepairDirection::ChangeSecondUse,
                ],
            },
            cause: ExplanationCause {
                error: error.id,
                seed,
                constraints: constraint_slice,
                reasons: reason_slice,
            },
        });
    }
}

/// Assign a type to every term in the program, in place, and return the
/// schemes of its top-level definitions.
pub fn infer(mint: &Mint, program: &mut Program) -> Output {
    let mut table = Table::default();
    let mut env = HashMap::new();
    let mut aliases = IndexMap::new();
    let mut errors = Vec::new();

    // What every declaration takes, before anything is lowered: the very first
    // lowering below is of a declaration's body, and a row parameter written
    // inside one already imposes its condition. See [`Table::params`].
    table.params = program
        .types
        .iter()
        .map(|(symbol, decl)| {
            let kinds = decl.params.iter().map(|param| param.kind.clone()).collect();
            (*symbol, kinds)
        })
        .chain(
            program
                .external_types
                .iter()
                .map(|(symbol, declaration)| (*symbol, declaration.params.clone())),
        )
        .collect();
    // And which of them are nominal within themselves: those every parameter of
    // which survives unfolding, so that comparing two applications argument by
    // argument can only ever agree with comparing what they stand for. See
    // [`Solve::nominal`].
    let nominal: HashSet<Symbol> = program
        .types
        .iter()
        .filter(|(_, decl)| decl.params.iter().all(|param| param.relevant))
        .map(|(symbol, _)| *symbol)
        .chain(
            program
                .external_types
                .iter()
                .filter(|(_, declaration)| declaration.relevant.iter().all(|relevant| *relevant))
                .map(|(symbol, _)| *symbol),
        )
        .collect();

    aliases.extend(
        program
            .external_types
            .iter()
            .map(|(symbol, declaration)| (*symbol, declaration.scheme.clone())),
    );

    // Aliases first: annotations refer to them. A name inside a body stays a
    // name, so this pass reads no alias it is still building and the order it
    // runs in decides nothing — which is what lets two declarations refer to
    // each other.
    for (symbol, decl) in &program.types {
        // The parameters are already `Ty::Bound`s by their position, so the
        // scheme is closed by counting them rather than by walking anything.
        let body = lower_type(mint, &mut table, &decl.value);
        aliases.insert(*symbol, Scheme::new(decl.params.len() as u32, body));
    }

    table.variances = semantic_variances(&aliases);

    // And what each operation was declared to be, before any body is walked: a
    // perform site and a handler arm each want the two sides of one, and both
    // come straight off the declaration. Lowered here rather than per use, for
    // the reason the aliases above are: a signature is a plain closed arrow, so
    // it mentions no variable and lowering one twice would only mint two copies
    // of nothing.
    let mut operations = program.external_operations.clone();
    for (symbol, decl) in &program.effects {
        let ir::Effect::Operations(declared) = &decl.value else {
            continue;
        };
        for (name, operation) in declared {
            let from = lower_type(mint, &mut table, &operation.from);
            let to = lower_type(mint, &mut table, &operation.to);
            operations.insert((*symbol, name.clone()), (from, to));
        }
    }

    // An extern has no initializer group to walk. Its written scheme is
    // nevertheless in scope before every group, exactly as a completed earlier
    // definition would be, so normal identifier lookup instantiates it at each
    // use. The annotation is authoritative, including any effect row it
    // declares for calls through the imported value.
    let mut externs = IndexMap::new();
    let mut extern_coverage = Vec::new();
    for (symbol, decl) in &program.externs {
        let annotation = decl
            .annotation
            .as_ref()
            .expect("the parser requires every extern annotation");
        let lowered = lower_annotation(mint, &mut table, annotation);
        // The clause belongs to each opened package, not to the compiler's
        // global store. `lowered.scheme` carries its closed copy; validate an
        // intrinsically impossible contract here without leaking its
        // assumptions into later declarations.
        if !sat::satisfiable(&lowered.formula) {
            errors.push(Error {
                id: table.error_id(),
                cause: ErrorCause::Direct,
                span: annotation.ty.span,
                kind: ErrorKind::ClauseImpossible {
                    formula: crate::ui::in_labels(&lowered.formula, &lowered.names),
                },
                explanation: None,
            });
        }
        if let Some(span) =
            polymorphic_extern_boundary(&aliases, &decl.value.abi, lowered.scheme.body())
        {
            errors.push(Error {
                id: table.error_id(),
                cause: ErrorCause::Direct,
                span,
                kind: ErrorKind::PolymorphicExternBoundary,
                explanation: None,
            });
        } else {
            let (mut coverage, conditional) =
                callback_coverage_constraints(&aliases, &decl.value.abi, &lowered.ty);
            for constraint in &mut coverage {
                constraint.id = table.constraint_id();
                constraint.reason = table.constraint_reason(constraint.id);
            }
            if sat::entails(&lowered.formula, &conditional) {
                extern_coverage.push((*symbol, coverage));
            } else {
                errors.push(Error {
                    id: table.error_id(),
                    cause: ErrorCause::Direct,
                    span: decl.value.abi.span,
                    kind: ErrorKind::CallbackEffectsNotCovered,
                    explanation: None,
                });
            }
        }
        env.insert(*symbol, Binding::Poly(lowered.scheme.clone()));
        externs.insert(*symbol, lowered.scheme);
    }

    let mut schemes = IndexMap::new();
    env.extend(
        program
            .external_schemes
            .iter()
            .map(|(symbol, scheme)| (*symbol, Binding::Poly(scheme.clone()))),
    );
    let mut locals = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut promises = IndexMap::new();
    let mut steps = Vec::new();
    let mut refinements = Vec::new();
    for (symbol, coverage) in extern_coverage {
        // Callback coverage is solver input just like generated definition
        // constraints. Keep it in the published arena so every step's direct
        // constraint identity remains resolvable by debugger consumers.
        constraints.insert(symbol, coverage.clone());
        Solve {
            table: &mut table,
            errors: &mut errors,
            steps: &mut steps,
            aliases: &aliases,
            nominal: &nominal,
            definition: symbol,
            depth: 0,
            constraint: None,
            constraint_reason: None,
            assumed: Vec::new(),
            schemes: HashMap::new(),
            locals: &mut locals,
            guard: None,
            active_refinement: None,
            guard_reasons: Vec::new(),
            refinements: &mut refinements,
            generated_end: 0,
        }
        .run(&coverage);
        report_flip(&mut table, &mut errors);
    }
    // The groups are read out before anything is solved: solving mutates the
    // definitions they name, and which definitions have to be typed together is
    // a fact about the lowered program that nothing here changes.
    let groups: Vec<Vec<Symbol>> = program
        .groups
        .iter()
        .map(|group| group.members.clone())
        .collect();
    for members in groups {
        // Every member is in scope before any of them is walked. That is the
        // whole of what makes recursion typable: a use of a group member inside
        // the group is either the one type the group is deciding or a copy of
        // what its annotation already promised, rather than a copy of a scheme
        // that does not exist yet. A use of a definition in an earlier group is
        // a [`Binding::Poly`] and instantiates as it always has, which is what
        // keeps let-polymorphism.
        //
        // A member with an annotation is bound to *it* rather than to a fresh
        // variable. A variable would never be tied to what was written, so the
        // recursive uses the annotation exists for would be checked against
        // nothing at all.
        //
        // And it is bound to the annotation's *scheme* rather than to the type
        // itself, so that a recursive use instantiates what the annotation
        // declared and shares what it left to inference. That is R29, and it is
        // what makes polymorphic recursion over a declared variable type: the
        // body sees a fresh copy of `r` at every mention, while the holes stay
        // the one thing the group is deciding.
        let scoped: Vec<Scoped> = members
            .iter()
            .map(|symbol| {
                table.definition = Some(*symbol);
                // The annotation is the contract: the body is checked against
                // it, and it — not whatever the body's constraints worked out
                // along the way — is what the definition means to everyone
                // downstream.
                //
                // Which is honest because the variables it declared are rigid
                // while the body is checked. A variable is a promise
                // about every caller, so nothing the body does may decide it —
                // and a body that tries is refused at the expression that
                // tried, which is the line the reader can change. What is left
                // open with a `..`, a `when _` or a `_` is the opposite: those
                // are holes, and a body deciding one is exactly what a hole is
                // for.
                let lowered = program.terms[symbol].annotation.as_ref().map(|annotation| {
                    let lowered = lower_annotation(mint, &mut table, annotation);
                    // The clause goes into the store as its own batch, so
                    // that it is what a use of this name sees and what the
                    // body is held to.
                    if !lowered.formula.is_true() {
                        let origin = Origin::Annotation(Named {
                            labels: lowered.names.clone(),
                            shape: None,
                        });
                        // A true placeholder preserves source/debug ordering
                        // for a wholly package-owned clause without making its
                        // guarantee globally active.
                        table.require(annotation.ty.span, origin, lowered.assumptions.clone());
                    }
                    lowered
                });
                let promised = lowered
                    .as_ref()
                    .map_or(Formula::True, |lowered| lowered.formula.clone());
                let names = lowered
                    .as_ref()
                    .map_or_else(Vec::new, |lowered| lowered.names.clone());
                let rigids = lowered
                    .as_ref()
                    .map_or_else(Vec::new, |lowered| lowered.rigids.clone());
                let bound = match &lowered {
                    Some(lowered) => {
                        env.insert(*symbol, Binding::Poly(lowered.scheme.clone()));
                        lowered.ty.clone()
                    }
                    None => {
                        let bound = table.fresh_type_for(Subject::TopLevelBinding);
                        env.insert(*symbol, Binding::Mono(bound.clone()));
                        bound
                    }
                };
                Scoped {
                    symbol: *symbol,
                    bound,
                    rigids,
                    promised,
                    names,
                }
            })
            .collect();

        // One walk and one solve per member, in source order, over the shared
        // table — which decides the same things solving the union would, since
        // unification does not care what order it is asked in, and keeps a
        // [`Step`] able to name the definition it came from.
        let mut solved: Vec<Solved> = Vec::with_capacity(scoped.len());
        // Where the group's clauses end and its bodies begin. Every member's
        // annotation is lowered before any member's body is walked, so the
        // store falls into two stretches, and the R10 check below wants the
        // second one — a clause is the promise being checked rather than any
        // part of what the group needs. Which member's body a batch is in does
        // not matter to that check and must not: a group is monomorphic, so a
        // presence one member's match constrains is the same variable in
        // another's type, and a clause that ignored what a fellow member did to
        // it would be a contract the group cannot keep. The batches that say
        // nothing about a clause's own presences project away to `true`, so
        // members with nothing to do with each other cost nothing but the walk.
        let bodies = table.store.batches.len();

        for scoped in scoped {
            table.definition = Some(scoped.symbol);
            let decl = &mut program.terms[&scoped.symbol];
            let mut constrain = Constrain {
                table: &mut table,
                mint,
                env: &mut env,
                aliases: &aliases,
                out: Vec::new(),
                annotated: Vec::new(),
                operations: &operations,
                effect_ids: &program.effect_ids,
                // A definition's value is computed where no handler can reach
                // it, so it is walked at the empty closed row and outside every
                // function — which is what makes performing an effect at the
                // top level an error rather than a silently discarded effect.
                ambient: constrain::Ambient {
                    row: Row::closed(),
                    inside: false,
                },
                answer: None,
                presence_guard: Formula::True,
            };
            // Checked against exactly what the rest of the group sees this
            // definition as. For an annotated one that is the annotation, as it
            // has always been; for the rest it is the variable standing in for
            // the definition, and checking against a bare variable is inferring
            // and equating — see [`Constrain::check_term`]. The equation is what
            // ties the name the body used to the type the body has.
            let expected_subject = if decl.annotation.is_some() {
                Subject::Annotation
            } else {
                Subject::TopLevelBinding
            };
            let expected_span = decl
                .annotation
                .as_ref()
                .map(|annotation| annotation.ty.span);
            constrain.check_term(
                &mut decl.value,
                &scoped.bound,
                expected_subject,
                expected_span,
            );
            let generated = constrain.out;
            let annotated = constrain.annotated;
            let ty = match &decl.annotation {
                Some(_) => scoped.bound.clone(),
                None => decl.value.ty.clone(),
            };
            let reported = errors.len();
            let published = locals.len();

            let generated_end = table.store.batches.len();
            Solve {
                table: &mut table,
                errors: &mut errors,
                steps: &mut steps,
                aliases: &aliases,
                nominal: &nominal,
                definition: scoped.symbol,
                depth: 0,
                constraint: None,
                constraint_reason: None,
                assumed: Vec::new(),
                schemes: HashMap::new(),
                locals: &mut locals,
                guard: None,
                active_refinement: None,
                guard_reasons: Vec::new(),
                refinements: &mut refinements,
                generated_end,
            }
            .run(&generated);

            // The store's verdict, once this definition's constraints are in:
            // the first batch that leaves it with no model owns the single
            // resulting error, and everything after it is suppressed. Asked
            // here rather than at the end of the group so that the batch is
            // named while the definition it came from is still the one being
            // read.
            report_flip(&mut table, &mut errors);

            solved.push(Solved {
                scoped,
                ty,
                generated,
                annotated,
                reported,
                locals: published,
            });
        }

        // And where they end, now that every member has been walked.
        let bodies = bodies..table.store.batches.len();

        // Where each member's complaints end, which is where the next one's
        // begin — and, for the last, where the group left the list.
        let mut bounds: Vec<usize> = solved.iter().map(|member| member.reported).collect();
        bounds.push(errors.len());
        // The same, for the schemes each member's nested lets published.
        let mut published: Vec<usize> = solved.iter().map(|member| member.locals).collect();
        published.push(locals.len());

        // Generalization, once the whole group is solved and not before: a
        // member's type is not settled while another member of its own group
        // can still constrain it. Each is then quantified into a scheme of its
        // own. Two members can share a variable and each quantify it
        // separately, which loses the sharing — and there is no scope outside a
        // group for that to matter to, a group being the outermost thing there
        // is. A nested `let` is inside one and so keeps the sharing: what its
        // level leaves free is a [`Ty::Var`] the enclosing binder still owns.
        for (at, member) in solved.into_iter().enumerate() {
            let symbol = member.scoped.symbol;
            let (from, to) = (bounds[at], bounds[at + 1]);
            let decl = &mut program.terms[&symbol];

            // Said here, past every member's solve, rather than beside the
            // list it is about, so that it lands after all of them: a member's
            // own range was fixed while the group was being solved, and a
            // complaint written into the middle of it would move everybody
            // else's.
            let told = errors.len();
            // A variable stands for whatever *this* annotation's
            // caller picks, so it means nothing in anybody else's type. A
            // scheme that would quantify one it did not declare is refused at
            // the declaration, which is the line the reader has to change.
            table.escapes(&member.ty, &member.scoped.rigids, &mut errors);
            // An annotation's `where` clause is the contract, so the body may
            // not need more of its presences than the clause allows: a use of
            // the name sees the clause and nothing of the body, and one that
            // the clause admits but the body cannot serve would be let through.
            // Silent once something has already flipped the store, and once
            // this definition has already failed some other way — both for the
            // reason the check above is.
            //
            // Read against the group's bodies and no clause of anyone's: a
            // clause is a promise rather than a requirement, and leaving one in
            // the range would let the message say the definition requires what
            // an annotation asked for. See `bodies` above.
            if let Some(annotation) = &decl.annotation
                && annotation.clause.is_some()
                && from == to
                && !table.unsat
                && let Some((allowed, required)) = table.disagreement(
                    &member.scoped.promised,
                    &member.scoped.names,
                    &Formula::True,
                    bodies.clone(),
                )
            {
                errors.push(Error {
                    id: table.error_id(),
                    cause: ErrorCause::Direct,
                    span: annotation.ty.span,
                    kind: ErrorKind::AnnotationAllows { allowed, required },
                    explanation: None,
                });
            }
            // An annotation on a nested binding is the same promise about a
            // smaller scope, and is kept or broken on the same terms — so it is
            // checked here, beside the definition's own, rather than by the
            // solver, which has never seen a written type. Held back by the
            // definition's own silence for the reason the one above is: a
            // complaint about the fallout of a failure is one mistake said
            // twice.
            for annotated in &member.annotated {
                // Its clause is a promise about a smaller scope on the same
                // terms. Read against the same stretch, which needs no narrower
                // bookkeeping: a batch that says nothing about the clause's
                // variables projects away to `true`, so everything outside the
                // nested value costs nothing but the walk over it.
                if from == to
                    && !table.unsat
                    && let Some((allowed, required)) = table.disagreement(
                        &annotated.promised,
                        &annotated.names,
                        &annotated.guard,
                        bodies.clone(),
                    )
                {
                    errors.push(Error {
                        id: table.error_id(),
                        cause: ErrorCause::Direct,
                        span: annotated.span,
                        kind: ErrorKind::AnnotationAllows { allowed, required },
                        explanation: None,
                    });
                }
            }

            // What its nested lets came to, numbered for a reader now that
            // nothing can bind their variables again. See [`Table::published`].
            for at in published[at]..published[at + 1] {
                let (_, scheme) = locals
                    .get_index_mut(at)
                    .expect("the range is this member's own");
                *scheme = table.published(scheme);
            }

            // R23's closing rule, before anything is quantified: an effect
            // variable the solve learned nothing about links nothing, so it is
            // the empty row rather than a `..'b` the caller gets to choose.
            table.close_effects(&member.ty, 0);
            // Fold-back, next of everything generalization does: a presence
            // the store has already decided is no variable at all, so it is
            // settled here rather than quantified and printed as one.
            table.fold_back(&member.ty);
            // What the scheme requires of what is left. An annotated definition
            // publishes its *annotation's* clause rather than what its body
            // worked out — the annotation is the contract, and R10 has already
            // held the body to it.
            let required = match decl.annotation.as_ref().map(|_| &member.scoped.promised) {
                // Silent once something has flipped the store: the cascade rule
                // reaches an annotated definition's clause exactly as it
                // reaches an inferred one's.
                Some(promised) if !promised.is_true() && !table.unsat => table.resolved(promised),
                Some(_) if table.unsat => Formula::True,
                _ => table.required(&member.ty),
            };
            let (scheme, mut subst) = table.generalize(&member.ty, 0, required);
            // With the substitution in hand, resolve every type the walk wrote
            // into the body, so a term's type and its definition's scheme spell
            // the same variable the same way.
            table.zonk_term(&mut decl.value, &mut subst);
            // With every presence the body can name now numbered, the store's
            // word about all of them, for the patterns walk to assume. The
            // scheme's own clause is deliberately not this: see
            // [`Output::promises`].
            promises.insert(symbol, table.promised(&subst));
            // And the same for what it complained about, which is why this
            // waits until the group is solved rather than running where the
            // error was reported: a variable in a payload may have been solved
            // after the fact, and the later knowledge reads better. Nothing
            // past here can touch this definition's variables, so this is the
            // last word on them. Its own two stretches of the list: the range
            // its solve wrote, and whatever was just added past the end.
            for at in (from..to).chain(told..errors.len()) {
                let zonked = table.zonk_error(&errors[at].kind, &mut subst);
                errors[at].kind = zonked;
            }
            env.insert(symbol, Binding::Poly(scheme.clone()));
            schemes.insert(symbol, scheme);
            constraints.insert(symbol, member.generated);
        }
    }

    // Both maps are keyed in source order, whatever order the groups were
    // solved in: a reader of either is reading the file, and which definition
    // had to be solved first is the solver's business rather than theirs.
    // [`Output::steps`] is that business exactly, and stays in solve order.
    let position: HashMap<Symbol, usize> = program
        .externs
        .keys()
        .chain(program.terms.keys())
        .enumerate()
        .map(|(at, symbol)| (*symbol, at))
        .collect();
    schemes.sort_by(|one, _, other, _| position[one].cmp(&position[other]));
    constraints.sort_by(|one, _, other, _| position[one].cmp(&position[other]));
    promises.sort_by(|one, _, other, _| position[one].cmp(&position[other]));

    attach_mismatch_explanations(&mut errors, &constraints, &steps, &table.reasons, &aliases);

    // Constraints are solved in the order the walk emitted them, which is not
    // quite the order anyone reads a file in — a body's demands come before
    // the annotation's on its result. Sorting by position puts that back; the
    // sort is stable, so two complaints about one span keep the order the
    // solver found them in.
    errors.sort_by_key(|error| error.span.start);

    // The store as the finished solve reads it: every variable followed to what
    // it was decided to be, so that what leaves inference can be reasoned about
    // without the variable table that is about to go away. The batches keep
    // their order, their origins and their flip marks; only what they *say* is
    // brought up to date, which is the difference between `a != b` as it was
    // emitted and the contradiction it turned out to be.
    let store = table.settled();
    let mut refinements: Vec<Refinement> = refinements
        .iter()
        .map(|refinement| table.settled_refinement(refinement))
        .collect();
    refinements.sort_by_key(|refinement| {
        (
            position[&refinement.definition],
            refinement.match_span.start,
            refinement.arm_span.start,
        )
    });

    Output {
        aliases,
        operations,
        externs,
        schemes,
        locals,
        constraints,
        steps,
        store,
        promises,
        refinements,
        variables: table.var_meta.clone(),
        reasons: table.reasons.clone(),
        errors,
    }
}

/// Record the first batch that left the store without a model, and say so where
/// the batch's own origin asks it to be said.
///
/// A match's coverage says nothing here: the complaint belongs to the patterns
/// phase, which words it as the unhandled values it is and reads a witness off a
/// model. The other two are inference's own, and both are about a contradiction
/// unification could never have found — each half is consistent, and only the
/// two together are not.
enum UnguardedOrigin<'a> {
    Coverage,
    Annotation(&'a Named),
    Required(&'a Named),
}

fn unguarded_origin(origin: &Origin) -> UnguardedOrigin<'_> {
    match origin {
        Origin::Coverage(_) => UnguardedOrigin::Coverage,
        Origin::Annotation(named) => UnguardedOrigin::Annotation(named),
        Origin::Instance(named) | Origin::Refinement(named) => UnguardedOrigin::Required(named),
        Origin::Guarded(guarded) => unguarded_origin(&guarded.origin),
    }
}

fn unguarded_formula<'a>(origin: &'a Origin, formula: &'a Formula) -> &'a Formula {
    match origin {
        Origin::Guarded(guarded) => unguarded_formula(&guarded.origin, &guarded.obligation),
        _ => formula,
    }
}

fn report_flip(table: &mut Table, errors: &mut Vec<Error>) {
    if table.unsat {
        return;
    }
    let Some(at) = table.flip() else {
        return;
    };
    table.unsat = true;
    table.store.batches[at].flipped = true;
    let batch = table.store.batches[at].clone();
    let base = unguarded_origin(&batch.origin);
    let kind = match base {
        UnguardedOrigin::Coverage => return,
        UnguardedOrigin::Annotation(named) => {
            // Which of the two annotation complaints it is: a clause with no
            // model of its own is wrong by itself, and the body under it —
            // which may do nothing with the type at all — has no part in it.
            // Asked of the formula as written rather than of
            // [`Table::resolved`], which is the whole distinction: resolving
            // substitutes what the solve decided, so a clause the definition
            // ruled out and one that rules itself out would come back the
            // same, and every clause would blame the clause. An annotation
            // check is one of the fixed boundaries where SAT may run.
            let alone = sat::satisfiable(unguarded_formula(&batch.origin, &batch.formula));
            let formula = crate::ui::in_labels(&batch.formula, &named.labels);
            match alone {
                true => ErrorKind::PresenceImpossible { formula },
                false => ErrorKind::ClauseImpossible { formula },
            }
        }
        UnguardedOrigin::Required(named) => {
            let formula = crate::ui::in_labels(&batch.formula, &named.labels);
            ErrorKind::PresenceRequired {
                formula,
                shape: named.shape,
            }
        }
    };
    errors.push(Error {
        id: table.error_id(),
        cause: ErrorCause::Batch(batch.id),
        span: batch.span,
        kind,
        explanation: None,
    });
}

impl Table {
    fn constraint_id(&mut self) -> ConstraintId {
        let id = ConstraintId(self.next_constraint_id);
        self.next_constraint_id += 1;
        id
    }

    fn reason(&mut self, origin: ReasonOrigin, parents: Vec<ReasonId>) -> ReasonId {
        let id = ReasonId(self.next_reason_id);
        self.next_reason_id += 1;
        self.reasons.push(Reason {
            id,
            parents,
            origin,
            reachable: true,
        });
        id
    }

    fn constraint_reason(&mut self, id: ConstraintId) -> ReasonId {
        self.reason(ReasonOrigin::Constraint(id), Vec::new())
    }

    fn step_id(&mut self) -> StepId {
        let id = StepId(self.next_step_id);
        self.next_step_id += 1;
        id
    }

    fn error_id(&mut self) -> ErrorId {
        let id = ErrorId(self.next_error_id);
        self.next_error_id += 1;
        id
    }

    fn batch_id(&mut self) -> BatchId {
        let id = BatchId(self.next_batch_id);
        self.next_batch_id += 1;
        id
    }

    /// What is known now, to be handed back to [`restore`](Self::restore) if
    /// what follows turns out not to have been asked.
    ///
    /// Copied rather than journalled. A trail of undo records would be the
    /// cheaper thing and a second representation of the solution to keep
    /// honest; this is one line, and the one caller takes it once for the
    /// outermost open congruence rather than per binding or nominal depth.
    fn snapshot(&self) -> Known {
        Known {
            vars: self.vars.clone(),
            var_meta: self.var_meta.clone(),
            levels: self.levels.clone(),
            lacks: self.lacks.clone(),
            existential_witnesses: self.existential_witnesses.clone(),
            abstract_existentials: self.abstract_existentials.clone(),
            reason_len: self.reasons.len(),
        }
    }

    /// Put back what [`snapshot`](Self::snapshot) took.
    ///
    /// The variables minted since go with it. Nothing can still be pointing at
    /// one: a fresh variable reaches the rest of the solve only by being bound
    /// into something, and every binding made since is being undone here too.
    fn restore(&mut self, known: Known) {
        self.vars = known.vars;
        self.var_meta = known.var_meta;
        self.levels = known.levels;
        self.lacks = known.lacks;
        self.existential_witnesses = known.existential_witnesses;
        self.abstract_existentials = known.abstract_existentials;
        for reason in &mut self.reasons[known.reason_len..] {
            reason.reachable = false;
        }
        if let Some(captures) = self.causal_reads.borrow_mut().as_mut()
            && let Some(reads) = captures.last_mut()
        {
            reads.clear();
        }
    }

    fn enter_solver_scope(&mut self) {
        if self.causal_scope_depth == 0 {
            let previous = self.causal_reads.replace(Some(Vec::new()));
            assert!(
                previous.is_none(),
                "causal reads active outside a solver scope"
            );
        }
        self.causal_scope_depth += 1;
    }

    fn leave_solver_scope(&mut self) {
        self.causal_scope_depth = self
            .causal_scope_depth
            .checked_sub(1)
            .expect("solver scope");
        if self.causal_scope_depth == 0 {
            let _discarded = self
                .causal_reads
                .replace(None)
                .expect("solver reads active");
            assert!(self.causal_reads.borrow().is_none());
        }
    }

    /// Start an independent boundary such as one generated constraint. Any
    /// unfinished reads in the enclosing act are discarded: crossing a nested
    /// solve cannot make them causes of work performed after that solve.
    fn begin_solver_act(&self) {
        let mut captures = self.causal_reads.borrow_mut();
        let captures = captures.as_mut().expect("solver act outside solver scope");
        if let Some(enclosing) = captures.last_mut() {
            enclosing.clear();
        }
        captures.push(IndexSet::new());
    }

    /// Start one rule, handing it reads made while exposing its goal. A rule
    /// which records no step discards those reads at completion rather than
    /// allowing the next rule to claim them.
    fn begin_solver_rule(&self) {
        let mut captures = self.causal_reads.borrow_mut();
        let captures = captures.as_mut().expect("solver rule outside solver scope");
        let inherited = captures.last_mut().map(std::mem::take).unwrap_or_default();
        captures.push(inherited);
    }

    fn end_solver_act(&self) {
        self.causal_reads
            .borrow_mut()
            .as_mut()
            .expect("solver act outside solver scope")
            .pop()
            .expect("solver act capture");
    }

    fn note_binding_read(&self, reason: ReasonId) {
        if let Some(captures) = self.causal_reads.borrow_mut().as_mut()
            && let Some(reads) = captures.last_mut()
        {
            reads.insert(reason);
        }
    }

    fn take_binding_reads(&self) -> Vec<ReasonId> {
        self.causal_reads
            .borrow_mut()
            .as_mut()
            .expect("causal reads consumed outside a solver act")
            .last_mut()
            .expect("causal reads consumed without an active act")
            .drain(..)
            .collect()
    }

    fn binding_parents(&self, var: TyVar, root: Option<ReasonId>) -> Vec<ReasonId> {
        let mut parents = Vec::new();
        if let Some(root) = root {
            parents.push(root);
        }
        parents.push(self.var_meta[var as usize].minted_by);
        for reason in self.take_binding_reads() {
            if !parents.contains(&reason) {
                parents.push(reason);
            }
        }
        parents
    }

    fn default_bind(
        &mut self,
        var: TyVar,
        value: Assigned,
        kind: DefaultBinding,
        assigned: DefaultAssignment,
        mut causes: Vec<ReasonId>,
    ) -> ReasonId {
        causes.insert(0, self.var_meta[var as usize].minted_by);
        causes.dedup();
        let reason = self.reason(
            ReasonOrigin::DefaultBinding {
                var,
                kind,
                assigned,
            },
            causes,
        );
        self.vars[var as usize] = Slot::Bound { value, by: reason };
        reason
    }

    /// One more variable, of no sort yet, at the level being walked. A
    /// variable's sort is fixed by the position it was minted for, and the four
    /// functions below are those positions; nothing else may call this.
    fn mint(&mut self, sort: VarSort, subject: Subject) -> TyVar {
        let var = self.vars.len() as TyVar;
        let minted_by = self.reason(ReasonOrigin::Variable { sort, subject }, Vec::new());
        self.vars.push(Slot::Unbound);
        self.var_meta.push(VarMeta {
            sort,
            subject,
            minted_by,
        });
        self.levels.push(self.level);
        var
    }

    /// A variable standing for a whole type: an unconstrained type
    /// of its own, so that binding it takes whatever it is against entire.
    fn fresh_type_for(&mut self, subject: Subject) -> Rc<Ty> {
        let var = self.mint(VarSort::Type, subject);
        Rc::new(Ty::plain(Ty::Var(var)))
    }

    /// A variable standing for the rest of a row.
    fn fresh_row(&mut self) -> Rest {
        self.fresh_row_for(Subject::Term)
    }

    fn fresh_row_for(&mut self, subject: Subject) -> Rest {
        Rest::Var(self.mint(VarSort::Row, subject))
    }

    /// A variable standing for whether one label is there.
    fn fresh_presence(&mut self) -> Presence {
        self.fresh_presence_for(Subject::Term)
    }

    fn fresh_presence_for(&mut self, subject: Subject) -> Presence {
        Presence::Var(self.mint(VarSort::Presence, subject))
    }

    fn fresh_instance_type(&mut self) -> Rc<Ty> {
        self.fresh_type_for(Subject::Instance)
    }

    fn fresh_instance_presence(&mut self) -> Presence {
        self.fresh_presence_for(Subject::Instance)
    }

    fn fresh_match_family_type(&mut self) -> Rc<Ty> {
        self.fresh_type_for(Subject::MatchResult)
    }

    fn fresh_match_family_row(&mut self) -> Rest {
        self.fresh_row_for(Subject::MatchResult)
    }

    fn fresh_match_family_presence(&mut self) -> Presence {
        self.fresh_presence_for(Subject::MatchResult)
    }

    /// Follow bound variables until reaching something that is not one. Only
    /// the head is resolved; a composite's children still need their own
    /// resolution, which is what [`zonk`](Self::zonk) does exhaustively.
    ///
    /// This follows only whole-type variables. Struct-row-tail variables are
    /// [`Rest::Var`] values and are flattened separately by [`Table::canon`].
    ///
    /// What guarantees this terminates is the occurs check, not anything here:
    /// [`assign`](Solve::assign) refuses every binding that would put a
    /// variable inside its own type, so a chain of bindings can never close a
    /// cycle and following one strictly shrinks what is left to follow. This is
    /// the solver's hottest path — every rule resolves both its sides — so it
    /// pays for no bookkeeping of its own.
    ///
    /// The budget is not that guarantee restated; it is a bound on what a bug
    /// in the occurs check would cost. A chain that follows more bindings than
    /// there are variables has visited one of them twice, so an off counter is
    /// a panic the debugger renders rather than a hang that says nothing.
    fn resolve(&self, ty: &Rc<Ty>) -> Rc<Ty> {
        let mut ty = ty.clone();
        let mut budget = self.vars.len();
        while let Ty::Var(v) = &*ty {
            let Slot::Bound {
                value: Assigned::Ty(inner),
                by,
            } = &self.vars[*v as usize]
            else {
                break;
            };
            self.note_binding_read(*by);
            budget = budget.checked_sub(1).expect("bound type cycle");
            ty = inner.clone();
        }
        ty
    }

    /// A sum's cases flattened: its own labels joined with every label its tail
    /// has already accumulated, and what remains of the tail — an unbound
    /// variable, [`Rest::Closed`] or [`Rest::Undecided`] — as far as the solver
    /// has got.
    ///
    /// A sum's tails and nothing else, now that a struct's `..` is its row tail and
    /// [`resolve`](Self::resolve) is the splice that settles one.
    ///
    /// A read and nothing else, and one an [`IndexMap`] settles: the outer
    /// row's own labels are inserted first, so a label its tail also names is
    /// the label the row wrote out. A row whose tail certainly has one of its
    /// labels is not a type, and there is no way left to reach one — lowering
    /// refuses a declaration handed a row that repeats a label it already
    /// names, and [`Solve::assign`]'s lacks check refuses every route through
    /// a variable that would bring the label in present, settling one still
    /// undecided absent instead. So the one copy a tail can carry here is a
    /// label settled absent — no case a value could be — and keeping the outer
    /// label is the flattening agreeing with the type, never a choice between
    /// two copies that could differ.
    ///
    /// The budget is [`resolve`](Self::resolve)'s, kept for its reason: it
    /// bounds what a bug in the occurs check would cost. Only a step through a
    /// bound variable counts — a [`Rest::More`] link unwraps a finite tree and
    /// can close no cycle — and following more bound variables than the store
    /// holds has visited one of them twice, so an off counter is a panic the
    /// debugger renders rather than a hang that says nothing.
    fn canon(&self, row: &Row) -> Row {
        let mut labels: IndexMap<String, RowField> = IndexMap::new();
        let mut row = row.clone();
        let mut budget = self.vars.len();
        loop {
            for (name, field) in &row.labels {
                labels.entry(name.clone()).or_insert_with(|| field.clone());
            }
            let deeper = match &row.rest {
                Rest::More(more) => (**more).clone(),
                Rest::Var(var) => match &self.vars[*var as usize] {
                    Slot::Bound {
                        value: Assigned::Row(bound),
                        by,
                    } => {
                        self.note_binding_read(*by);
                        budget = budget.checked_sub(1).expect(
                            "a chain of bound row variables closed a cycle the occurs check should refuse",
                        );
                        (**bound).clone()
                    }
                    _ => {
                        return Row {
                            labels,
                            rest: std::mem::take(&mut row.rest),
                        };
                    }
                },
                _ => {
                    return Row {
                        labels,
                        rest: std::mem::take(&mut row.rest),
                    };
                }
            };
            row = deeper;
        }
    }

    /// Where the variable behind one rigid was written: the second
    /// place a rigid complaint points at.
    ///
    /// Indexed rather than looked up: every rigid in a type was minted by
    /// [`lower_annotation`], which records the span as it mints.
    fn declared(&self, id: u32) -> Span {
        self.rigids[&id]
    }

    /// Follow a presence variable to what it stands for.
    ///
    /// The budget is [`resolve`](Self::resolve)'s, kept for its reason: a
    /// chain longer than the store has visited a variable twice, so a bug in
    /// the occurs check is a panic rather than a hang.
    fn presence_of(&self, presence: &Presence) -> Presence {
        let mut presence = presence.clone();
        let mut budget = self.vars.len();
        while let Presence::Var(var) = presence {
            let Slot::Bound {
                value: Assigned::Presence(inner),
                by,
            } = &self.vars[var as usize]
            else {
                break;
            };
            self.note_binding_read(*by);
            budget = budget.checked_sub(1).expect(
                "a chain of bound presence variables closed a cycle the occurs check should refuse",
            );
            presence = inner.clone();
        }
        presence
    }

    /// Whether two types are the same type as far as anything already decided
    /// can tell — every variable followed to what it stands for, and then term
    /// against term.
    ///
    /// Not a rule of the solve and never a reason to accept a program: solving
    /// is what decides whether two types *can be made* equal, and this only
    /// answers whether they already are. Its one caller is [`Solve::unfold`],
    /// which needs to recognize a goal it is already in the middle of.
    ///
    /// Read at the moment of the question rather than recorded when the types
    /// were first seen, which is the whole reason it resolves: a variable bound
    /// since then is part of what the older goal now says, and comparing what it
    /// said before would be comparing something the solver has stopped
    /// believing.
    ///
    /// Says no where it cannot tell. Answering no to a question that is really
    /// yes costs a repeated goal; answering yes to one that is really no would
    /// accept two types that differ, so the one-sided error is the one to make.
    fn alike(&self, a: &Rc<Ty>, b: &Rc<Ty>) -> bool {
        enum Work {
            Ty(Rc<Ty>, Rc<Ty>),
            Row(Row, Row),
        }

        let mut same = true;
        let mut work = vec![Work::Ty(a.clone(), b.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(a, b) => {
                    let (a, b) = (self.resolve(&a), self.resolve(&b));
                    match (&*a, &*b) {
                        (Ty::Nat, Ty::Nat)
                        | (Ty::Int, Ty::Int)
                        | (Ty::Real, Ty::Real)
                        | (Ty::String, Ty::String)
                        | (Ty::Boolean, Ty::Boolean)
                        | (Ty::Undecided, Ty::Undecided) => {}
                        (Ty::Var(x), Ty::Var(y)) => same &= x == y,
                        (Ty::Rigid { id: x, .. }, Ty::Rigid { id: y, .. }) => same &= x == y,
                        (Ty::Arrow(from, to, effects), Ty::Arrow(other, result, performs)) => {
                            work.push(Work::Row(effects.clone(), performs.clone()));
                            work.push(Work::Ty(to.clone(), result.clone()));
                            work.push(Work::Ty(from.clone(), other.clone()));
                        }
                        (Ty::Struct(a), Ty::Struct(b)) | (Ty::Sum(a), Ty::Sum(b)) => {
                            work.push(Work::Row(a.clone(), b.clone()));
                        }
                        (
                            Ty::Named {
                                symbol: a,
                                args: xs,
                                ..
                            },
                            Ty::Named {
                                symbol: b,
                                args: ys,
                                ..
                            },
                        ) => {
                            // Both parts are independent shape facts. Evaluate
                            // both without short-circuiting so malformed arity
                            // cannot hide behind a different declaration.
                            if (a != b) | (xs.len() != ys.len()) {
                                return false;
                            }
                            work.extend(
                                xs.iter()
                                    .zip(ys.iter())
                                    .rev()
                                    .map(|(x, y)| Work::Ty(x.clone(), y.clone())),
                            );
                        }
                        _ => return false,
                    }
                }
                Work::Row(a, b) => {
                    let (a, b) = (self.canon(&a), self.canon(&b));
                    if a.labels.len() != b.labels.len() {
                        return false;
                    }
                    for (name, field) in &a.labels {
                        let Some(other) = b.labels.get(name) else {
                            return false;
                        };
                        let left = self.presence_of(&field.presence);
                        let right = self.presence_of(&other.presence);
                        if left != right {
                            return false;
                        }
                        // An absent slot denotes no payload. Imported recovery
                        // artifacts may put arbitrary, even recursive, trees in
                        // it; those trees are not part of row equality.
                        work.extend(
                            (!matches!((&left, &right), (Presence::Absent, Presence::Absent)))
                                .then(|| Work::Ty(field.ty.clone(), other.ty.clone())),
                        );
                    }
                    let same_rest = match (&a.rest, &b.rest) {
                        (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => true,
                        (Rest::Var(x), Rest::Var(y)) => x == y,
                        (Rest::Rigid { id: x, .. }, Rest::Rigid { id: y, .. }) => x == y,
                        _ => false,
                    };
                    if !same_rest {
                        return false;
                    }
                }
            }
        }
        same
    }

    /// Whether `var` occurs in what it is about to be bound to, whichever sort
    /// that is. One variable space, so a row-tail variable hiding inside a row is
    /// as much a cycle as one hiding inside a type.
    ///
    /// Asked by listing every variable the value mentions and then looking for
    /// this one, rather than by asking "is this it?" at each position in turn.
    /// The question is one question, and asking it once is what keeps the walk
    /// a walk: the positions differ in where they look, not in what they are
    /// looking for.
    ///
    /// Which is also the only form of it this codebase can hold itself to. Asked
    /// per position, the comparison at a row's tail and the one at a presence
    /// could never answer yes — the paragraph below is why — so two of the
    /// walk's own branches would be unreachable, and a rule with a branch nobody
    /// can exercise is a rule nobody can rely on. One comparison, at the end, is
    /// a comparison both answers of which a program can produce. Walking the
    /// whole value to reach it is what that costs, and this is not the place the
    /// solver's time goes.
    ///
    /// It is answered yes only about a row-tail variable, and that is a fact about
    /// the solver rather than a hole in the walk. Two reasons, and between them
    /// they cover every route here:
    ///
    /// - A variable's sort is fixed where it was minted and never changes, so a
    ///   presence variable is never the same variable as a row or type one. Most
    ///   of what this walk turns up is therefore of the wrong sort to be the one
    ///   being bound, and no comparison across two sorts can say yes.
    /// - The same-sort cases are turned away before a binding is ever proposed.
    ///   Two sides that share an open end and differ in labels either way round
    ///   are refused in [`Solve::labels`], where the complaint can name both
    ///   types instead of one variable; two tails that flatten to the same
    ///   variable are [`Rule::Same`] in [`Solve::rests`]; two presences that are
    ///   the same variable are [`Rule::Same`] in [`Solve::presences`], and both
    ///   of its callers — [`Solve::field`] and [`Solve::absorb`] — put each
    ///   presence through [`Table::presence_of`] first, so no chain of aliases
    ///   arrives back at the variable being bound.
    ///
    /// The walk stays whole regardless. Those interceptions are where they are
    /// because they word a better complaint, not because this cannot answer;
    /// a rule with a hole in it is a rule nobody can rely on, and the next
    /// person to move one of them should find this check already correct.
    fn occurs(&self, var: TyVar, value: &Assigned) -> bool {
        let mut mentioned = Vec::new();
        self.mentions(value, &mut mentioned);
        mentioned.contains(&var)
    }

    /// Lower the level of everything `value` mentions to no more than `var`'s
    /// own, which is what binding a variable does to the variables inside what
    /// it takes.
    ///
    /// The whole of why generalization at a level is right. A variable stands
    /// for a type the binder that minted `var` may see, so everything inside
    /// that type is as old as `var` is however recently it was written:
    /// `fn p => let q = p.x in q` mints the field's variable inside the let and
    /// then binds `p`'s — minted outside it — to a type carrying the field, at
    /// which point the field is the lambda's and not the let's. Generalizing
    /// the variables minted inside a let would quantify it; generalizing the
    /// ones still at or above the let's level does not.
    ///
    /// Walked the way the occurs check is walked, and by the same walk: the two
    /// ask about the variables one value mentions, and asking once per position
    /// is what would make either of them a rule with a branch nobody can
    /// exercise. See [`Table::occurs`].
    fn demote(&mut self, var: TyVar, value: &Assigned) {
        let level = self.levels[var as usize];
        let mut mentioned = Vec::new();
        self.mentions(value, &mut mentioned);
        for var in mentioned {
            let at = &mut self.levels[var as usize];
            *at = (*at).min(level);
        }
    }

    /// Collect every variable `value` mentions, whichever sort it is.
    fn mentions(&self, value: &Assigned, found: &mut Vec<TyVar>) {
        match value {
            Assigned::Ty(ty) => self.mentions_ty(ty, found),
            Assigned::Row(row) => self.mentions_row(row, found),
            Assigned::Presence(presence) => self.mentions_presence(presence, found),
        }
    }

    /// Every variable `ty` mentions — in its constructor, and in the fields it carries.
    fn mentions_ty(&self, ty: &Rc<Ty>, found: &mut Vec<TyVar>) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Var(var) => found.push(*var),
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        Ty::Nat
                        | Ty::Int
                        | Ty::Real
                        | Ty::String
                        | Ty::Boolean
                        | Ty::Bound(_)
                        | Ty::Rigid { .. }
                        | Ty::Undecided => {}
                    }
                }
                Work::Row(row) => {
                    let row = self.canon(&row);
                    if let Rest::Var(var) = row.rest {
                        found.push(var);
                    }
                    for field in row.labels.values().rev() {
                        let presence = self.presence_of(&field.presence);
                        if let Presence::Var(var) = presence {
                            found.push(var);
                        }
                        if !matches!(presence, Presence::Absent) {
                            work.push(Work::Ty(field.ty.clone()));
                        }
                    }
                }
            }
        }
    }

    /// Every variable a sum's cases mention. Every slot a row has, in one
    /// sequence: a label's presence is as much a place a variable can hide as
    /// its type is, and the tail is another, and none of the three is a
    /// different question.
    fn mentions_row(&self, row: &Row, found: &mut Vec<TyVar>) {
        let row = self.canon(row);
        if let Rest::Var(var) = row.rest {
            found.push(var);
        }
        self.mentions_labels(&row.labels, found);
    }

    /// [`mentions_row`](Self::mentions_row) about a label map, which has no
    /// tail: each label's presence, and what it holds.
    fn mentions_labels(&self, labels: &IndexMap<String, RowField>, found: &mut Vec<TyVar>) {
        for field in labels.values() {
            let presence = self.presence_of(&field.presence);
            self.mentions_presence(&presence, found);
            if !matches!(presence, Presence::Absent) {
                self.mentions_ty(&field.ty, found);
            }
        }
    }

    /// The variable a presence stands for, if it still stands for one.
    fn mentions_presence(&self, presence: &Presence, found: &mut Vec<TyVar>) {
        if let Presence::Var(var) = self.presence_of(presence) {
            found.push(var);
        }
    }

    /// Record what the row variables inside `ty` may not stand for: every row
    /// in it puts the names it writes out onto the variable its tail resolves
    /// to. See [`Table::lacks`].
    ///
    /// Called on every type that enters the solver's world already mentioning
    /// row variables — a lowered annotation, an instantiated scheme, the
    /// demand a projection makes — because the condition is carried beside the
    /// variables rather than inside the type, and so is lost by any step that
    /// rebuilds one. A scheme is the clearest case:
    /// `{ x: Nat, ..'b } -> { ..'b }` quantifies a tail that must lack `x`, and
    /// each instantiation has to say so again of its own fresh copy.
    ///
    /// The tail chain is followed to its end, so a row whose tail is already
    /// bound to another row puts the names of both onto whatever is still open
    /// past them — the flattening [`Table::canon`] does, for the same reason.
    fn note_lacks(&mut self, ty: &Rc<Ty>) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row, Shape),
        }

        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone(), Shape::Effect));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Struct(row) => work.push(Work::Row(row.clone(), Shape::Struct)),
                        Ty::Sum(row) => work.push(Work::Row(row.clone(), Shape::Sum)),
                        Ty::Named { symbol, args, .. } => {
                            let symbol = *symbol;
                            for (at, arg) in args.iter().enumerate().rev() {
                                let demand = self.params.get(&symbol).and_then(|kinds| {
                                    kinds.get(at).and_then(|kind| {
                                        kind.row().map(|(shape, labels)| (shape, labels.clone()))
                                    })
                                });
                                if let Some((shape, labels)) = demand {
                                    // Fields are carried only by structs. Using
                                    // `cases` here erased precisely the row a
                                    // Fields parameter's declaration forbade.
                                    let resolved = self.resolve(arg);
                                    let row = match (shape, &*resolved) {
                                        (Shape::Struct, Ty::Struct(row))
                                        | (Shape::Sum | Shape::Effect, Ty::Sum(row)) => row.clone(),
                                        _ => Row::of(Rest::Undecided),
                                    };
                                    self.forbid(&row, shape, &labels);
                                }
                                work.push(Work::Ty(arg.clone()));
                            }
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
                }
                Work::Row(row, shape) => {
                    let flat = self.canon(&row);
                    let labels: IndexSet<String> = flat.labels.keys().cloned().collect();
                    work.extend(
                        flat.labels
                            .values()
                            .rev()
                            .filter(|field| {
                                !matches!(self.presence_of(&field.presence), Presence::Absent)
                            })
                            .map(|field| Work::Ty(field.ty.clone())),
                    );
                    if let Rest::Var(var) = flat.rest {
                        self.forbidden(var, shape, labels);
                    }
                }
            }
        }
    }

    /// Record one row's lacks facts and walk its payloads. Kept as the row
    /// entry point for effect extension and generalization; each payload walk
    /// is itself iterative.
    fn note_lacks_row(&mut self, row: &Row, shape: Shape) {
        let flat = self.canon(row);
        let labels: IndexSet<String> = flat.labels.keys().cloned().collect();
        for field in flat.labels.values() {
            if !matches!(self.presence_of(&field.presence), Presence::Absent) {
                self.note_lacks(&field.ty);
            }
        }
        if let Rest::Var(var) = flat.rest {
            self.forbidden(var, shape, labels);
        }
    }

    /// Record that whatever is still open past a sum's cases may not stand for
    /// any of `labels`: the tail chain followed to its end, and the condition
    /// put on the variable it lands at.
    ///
    /// Nothing to do when it lands anywhere else. A closed row has no room for
    /// the labels to arrive in, and a [`Rest::Bound`] is a declaration's own
    /// parameter, whose condition is a fact about the declaration that
    /// [`ir::kinds`](crate::ir) already worked out and this table never sees a
    /// variable for.
    fn forbid(&mut self, row: &Row, shape: Shape, labels: &IndexSet<String>) {
        if let Rest::Var(var) = self.canon(row).rest {
            self.forbidden(var, shape, labels.iter().cloned());
        }
    }

    /// Put one condition on one variable. The shape is the row the condition
    /// came from, and the first one recorded stands: a variable sits at the
    /// open end of one row, so every condition on it is about the same shape.
    ///
    /// A condition forbidding nothing is not recorded at all. It would say
    /// nothing about what the variable may stand for and would fix the shape
    /// every later condition on it is read in — so a tail carried across a
    /// binding by a row that happened to name no labels would leave a sum's
    /// tail being complained about in fields.
    fn forbidden(&mut self, var: TyVar, shape: Shape, labels: impl IntoIterator<Item = String>) {
        let mut labels = labels.into_iter().peekable();
        if labels.peek().is_none() {
            return;
        }
        let (_, recorded) = self
            .lacks
            .entry(var)
            .or_insert_with(|| (shape, IndexSet::new()));
        recorded.extend(labels);
    }

    /// Record one more thing the program requires of its presences.
    ///
    /// A batch that requires nothing is still recorded when it carries an
    /// origin the readers downstream need — a match's coverage is what the
    /// patterns phase asks its reachability questions of, whether or not the
    /// arms happened to relate anything.
    fn require(&mut self, span: Span, origin: Origin, formula: Formula) -> ReasonId {
        self.require_because(span, origin, formula, None)
    }

    fn require_because(
        &mut self,
        span: Span,
        origin: Origin,
        formula: Formula,
        because: Option<ReasonId>,
    ) -> ReasonId {
        let id = self.batch_id();
        let reason = self.reason(ReasonOrigin::Batch(id), because.into_iter().collect());
        self.store.batches.push(Batch {
            id,
            definition: self.definition,
            span,
            origin,
            reason,
            formula,
            flipped: false,
        });
        reason
    }

    /// Point the use-site batches emitted since `from`, and still carrying
    /// `at`, at `span` instead. See [`Constrain::infer_term`]'s application
    /// arm, which is the one caller and the whole of the rule.
    fn aim(&mut self, from: usize, at: Span, span: Span) {
        for batch in &mut self.store.batches[from..] {
            match batch {
                Batch {
                    origin: Origin::Instance(_),
                    span: batch_span,
                    ..
                } if *batch_span == at => *batch_span = span,
                _ => {}
            }
        }
    }

    /// One formula with every variable followed to what the solve decided it
    /// stands for.
    ///
    /// The bridge between the two halves of the answer. A batch is written
    /// about the variables that existed when it was emitted; unification then
    /// decides some of them outright and ties others together, and neither of
    /// those facts is in the store. Reading a batch through this is what puts
    /// them back: `p {}` conjoins `a != b` and then binds both to absent, and
    /// the contradiction only exists once the two are read together.
    fn resolved(&self, formula: &Formula) -> Formula {
        formula.rename(&|var| self.presence_of(&Presence::Var(var)).formula())
    }

    /// The store with everything the solve decided folded into it — what
    /// inference publishes. See the comment at the end of [`infer`].
    fn settled(&self) -> Store {
        let batches = self
            .store
            .batches
            .iter()
            .enumerate()
            .filter(|(at, _)| !self.empty_batches.contains(at))
            .map(|(_, batch)| Batch {
                id: batch.id,
                definition: batch.definition,
                span: batch.span,
                origin: self.settled_origin(&batch.origin),
                reason: batch.reason,
                formula: self.resolved(&batch.formula),
                flipped: batch.flipped,
            })
            .collect();
        Store { batches }
    }

    /// One origin with every presence alias followed, retaining guarded
    /// attribution recursively.
    fn settled_origin(&self, origin: &Origin) -> Origin {
        match origin {
            Origin::Coverage(coverage) => Origin::Coverage(Coverage {
                arms: coverage.arms.iter().map(|arm| self.resolved(arm)).collect(),
                fields: coverage
                    .fields
                    .iter()
                    .map(|(name, presence)| (name.clone(), self.presence_of(presence)))
                    .collect(),
                paths: coverage
                    .paths
                    .iter()
                    .map(|(name, presence)| (name.clone(), self.presence_of(presence)))
                    .collect(),
            }),
            Origin::Instance(named) => Origin::Instance(self.settled_names(named)),
            Origin::Annotation(named) => Origin::Annotation(self.settled_names(named)),
            Origin::Refinement(named) => Origin::Refinement(self.settled_names(named)),
            Origin::Guarded(guarded) => Origin::Guarded(GuardedOrigin {
                premise: self.resolved(&guarded.premise),
                obligation: self.resolved(&guarded.obligation),
                origin: Box::new(self.settled_origin(&guarded.origin)),
            }),
        }
    }

    fn settled_refinement(&self, refinement: &Refinement) -> Refinement {
        Refinement {
            definition: refinement.definition,
            match_span: refinement.match_span,
            arm_span: refinement.arm_span,
            raw: self.resolved(&refinement.raw),
            effective: self.resolved(&refinement.effective),
            reachable: refinement.reachable,
            fields: refinement
                .fields
                .iter()
                .map(|(name, presence)| (name.clone(), self.presence_of(presence)))
                .collect(),
            facts: refinement.facts.clone(),
            obligations: refinement
                .obligations
                .iter()
                .map(|obligation| GuardedObligation {
                    span: obligation.span,
                    premise: self.resolved(&obligation.premise),
                    obligation: self.resolved(&obligation.obligation),
                    formula: self.resolved(&obligation.formula),
                })
                .collect(),
        }
    }

    /// One batch's label pairs with every presence followed to what the solve
    /// decided it is.
    fn settled_names(&self, named: &Named) -> Named {
        Named {
            labels: named
                .labels
                .iter()
                .map(|(name, presence)| (name.clone(), self.presence_of(presence)))
                .collect(),
            shape: named.shape,
        }
    }

    /// Everything the store says, read as the solve now stands.
    fn known(&self) -> Formula {
        Formula::all(
            self.store
                .batches
                .iter()
                .map(|batch| self.resolved(&batch.formula)),
        )
    }

    /// The longest satisfiable prefix of the store. Arm-boundary queries use
    /// this rather than an already contradictory whole: the first flip owns the
    /// one error, and treating every later arm as unreachable would cascade it
    /// into unrelated typing decisions before [`report_flip`] can mark it.
    fn consistent_known(&self, upto: usize, runtime_from: usize) -> Formula {
        let mut known = Formula::True;
        for batch in self.store.batches[..upto]
            .iter()
            .chain(self.store.batches[runtime_from..].iter())
        {
            let next = known.clone().and(self.resolved(&batch.formula));
            if !sat::satisfiable(&next) {
                break;
            }
            known = next;
        }
        known
    }

    /// The first batch conjoining which leaves the store with no model, if the
    /// store has none.
    ///
    /// Replayed from the start every time rather than accumulated, because what
    /// a batch *says* changes as the solve goes: an earlier batch can become
    /// the flipping one once a variable it names is decided. The formulas are
    /// tiny and there is one batch per match, use and annotation, so replaying
    /// is cheaper than keeping a second copy of the solve honest.
    fn flip(&self) -> Option<usize> {
        let mut accumulated = Formula::True;
        for (at, batch) in self.store.batches.iter().enumerate() {
            accumulated = accumulated.and(self.resolved(&batch.formula));
            if !sat::satisfiable(&accumulated) {
                return Some(at);
            }
        }
        None
    }

    /// Everything the store says about `atoms` and about whatever those reach:
    /// the batches that mention one of them, the batches that mention *those*
    /// variables, and so on until nothing more joins.
    ///
    /// The whole store would be sound and useless: a definition's scheme would
    /// carry every other definition's constraints, all of them about variables
    /// its type never mentions. Following the connections instead keeps a
    /// scheme's formula about the scheme.
    fn component(&self, atoms: &[Atom]) -> Formula {
        let mut wanted: IndexSet<Atom> = atoms.iter().copied().collect();
        let mut taken = vec![false; self.store.batches.len()];
        let mut grew = true;
        while grew {
            grew = false;
            for (at, batch) in self.store.batches.iter().enumerate() {
                if taken[at] {
                    continue;
                }
                let formula = self.resolved(&batch.formula);
                let mut named = Vec::new();
                formula.atoms(&mut named);
                if !named.iter().any(|atom| wanted.contains(atom)) {
                    continue;
                }
                taken[at] = true;
                grew = true;
                wanted.extend(named);
            }
        }
        Formula::all(
            self.store
                .batches
                .iter()
                .enumerate()
                .filter(|(at, _)| taken[*at])
                .map(|(_, batch)| self.resolved(&batch.formula)),
        )
    }

    /// Whether an annotation's `where` clause allows more than the definition
    /// under it does, and — when it does — the two formulas to quote.
    ///
    /// The clause is the contract, so it has to *entail* what the body needs:
    /// every combination the clause admits must be one the definition serves. A
    /// body needing more is the disagreement, and it is reported at the
    /// annotation because that is the line the reader changes — the body is
    /// doing what it says.
    ///
    /// The body's side is every batch in `batches`, whatever its origin: a body
    /// acquires a presence requirement as readily by *using* another
    /// constrained definition — an [`Origin::Instance`] batch — as by matching
    /// on its own argument, and a clause that does not cover the one covers
    /// nothing.
    ///
    /// Which is why the caller hands over the batches the bodies of the group
    /// put in the store and no clause of anyone's. A clause is the promise
    /// rather than the body, and leaving one in would let the message say the
    /// definition requires what an annotation asked for. It changes no verdict —
    /// the premise of the entailment already carries this definition's own
    /// clause — only who the second half of the complaint is about.
    ///
    /// The body's side is projected onto the clause's variables and any
    /// enclosing arm premise. Keeping the latter until after entailment is what
    /// prevents `E -> Q` from becoming `true` merely because `E` belongs to the
    /// enclosing value rather than the local annotation.
    fn disagreement(
        &self,
        promised: &Formula,
        names: &[(String, Presence)],
        guard: &Formula,
        batches: Range<usize>,
    ) -> Option<(String, String)> {
        let promised = self.resolved(promised);
        let guard = self.resolved(guard);
        let allowed = guard.clone().and(promised.clone());
        let body = Formula::all(
            self.store.batches[batches]
                .iter()
                .map(|batch| self.resolved(&batch.formula)),
        );
        let mut promised_atoms = Vec::new();
        promised.atoms(&mut promised_atoms);
        // Producer-owned presences are witnesses, not inputs the body may
        // demand from every caller. Eliminate them from the body's side before
        // checking the universal contract; their package formula remains in
        // `allowed`, where the producer's witness equations are checked for
        // consistency without publishing those equations.
        let mut check_atoms: Vec<Atom> = promised_atoms
            .iter()
            .copied()
            .filter(|atom| match atom {
                Atom::Var(var) => !self.existential_witnesses.contains(var),
                Atom::Bound(_) => true,
            })
            .collect();
        let mut guard_atoms = Vec::new();
        guard.atoms(&mut guard_atoms);
        // A local annotation mints its own presences, disjoint from the
        // enclosing arm's. Both sets stay in the entailment alphabet.
        check_atoms.extend(guard_atoms);
        // The clause has two quantifier polarities. Caller-owned presences are
        // inputs, while producer-owned presences are witnesses selected by the
        // body. Consequently a mixed contract is not the ordinary implication
        // `promised -> project(body)`: that loses the correlation between an
        // input and its witness. For every admitted caller assignment, some
        // witness must satisfy both the advertised guarantee and what the body
        // actually produces.
        //
        //     (exists E. promised(U, E))
        //       -> (exists E, locals. promised(U, E) and body(U, E, locals))
        //
        // Projection supplies those existential quantifiers. With no producer
        // witnesses this reduces to the old universal contract check.
        let admitted = sat::project(&allowed, &check_atoms);
        let realized = sat::project(&allowed.clone().and(body.clone()), &check_atoms);
        if sat::entails(&admitted, &realized) {
            return None;
        }
        let required = sat::project(&guard.and(body), &promised_atoms);
        // Both halves are quoted in the names the reader wrote, which means
        // looking each one up by the presence it decides — as the solve now has
        // it, not as the annotation minted it. A variable unified with another
        // is spelled by whichever of the two the store kept, and a lookup that
        // asked for the other would find nothing and print the number.
        let named = self.settled_names(&Named {
            labels: names.to_vec(),
            shape: None,
        });
        Some((
            crate::ui::in_labels(&sat::project(&promised, &promised_atoms), &named.labels),
            crate::ui::in_labels(&required, &named.labels),
        ))
    }

    /// Settle every presence in `ty` the store has already decided.
    ///
    /// The fold-back R8 asks for. A variable whose literal the store entails is
    /// not a variable at all — every model of the program agrees about it — so
    /// it is bound here, once, at the boundary where SAT is allowed to run. The
    /// binding is permanent and sound: the store only ever grows, and
    /// entailment survives conjunction.
    ///
    /// What this buys is the whole of the `h` example: a `let` pattern forces
    /// `x` present, the match's `a != b` then forces `y` absent, and the
    /// definition generalizes to `{x: a} -> {}` with no variables and no
    /// `where` clause left over.
    fn fold_back(&mut self, ty: &Rc<Ty>) {
        let known = self.known();
        if !sat::satisfiable(&known) {
            return;
        }
        let mut found = IndexSet::new();
        self.presences_in(ty, &mut found);
        for var in found {
            let literal = Formula::var(var);
            let settled = if sat::entails(&known, &literal) {
                Presence::Present
            } else if sat::entails(&known, &literal.clone().not()) {
                Presence::Absent
            } else {
                continue;
            };
            let assigned = match settled {
                Presence::Present => DefaultAssignment::Present,
                Presence::Absent => DefaultAssignment::Absent,
                _ => unreachable!("SAT only settles literals"),
            };
            let mut causes = Vec::new();
            let mut wanted = IndexSet::from([Atom::Var(var)]);
            let mut taken = vec![false; self.store.batches.len()];
            let mut grew = true;
            while grew {
                grew = false;
                for (at, batch) in self.store.batches.iter().enumerate() {
                    if taken[at] {
                        continue;
                    }
                    let formula = self.resolved(&batch.formula);
                    let mut atoms = Vec::new();
                    formula.atoms(&mut atoms);
                    if atoms.iter().any(|atom| wanted.contains(atom)) {
                        taken[at] = true;
                        grew = true;
                        wanted.extend(atoms);
                        causes.push(batch.reason);
                    }
                }
            }
            self.default_bind(
                var,
                Assigned::Presence(settled),
                DefaultBinding::Sat,
                assigned,
                causes,
            );
        }
    }

    /// Every presence variable a type still mentions, in the order it mentions
    /// them — which is the order the printed alphabet follows.
    ///
    /// A set, because a type may mention one variable twice — two labels
    /// sharing a presence is the whole of what a `when` name buys — and the
    /// order is the first mention's.
    fn presences_in(&self, ty: &Rc<Ty>, found: &mut IndexSet<TyVar>) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row),
            Presence(Presence),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
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
                }
                Work::Row(row) => {
                    let row = self.canon(&row);
                    for field in row.labels.values().rev() {
                        let presence = self.presence_of(&field.presence);
                        if !matches!(presence, Presence::Absent) {
                            work.push(Work::Ty(field.ty.clone()));
                        }
                        work.push(Work::Presence(presence));
                    }
                }
                Work::Presence(presence) => {
                    if let Presence::Var(var) = self.presence_of(&presence) {
                        found.insert(var);
                    }
                }
            }
        }
    }

    /// One fresh copy of a scheme, minted at the level the table is being
    /// walked or solved at.
    ///
    /// Two callers, and they are the two ways a name can stand for a scheme:
    /// generation opening an earlier definition's, and the solver opening the
    /// one a nested `let` published — which generation could not have opened,
    /// there being nothing to open until the value is solved. One function
    /// because it is one act.
    /// Open a nested value binding. Only an explicit producer package is
    /// coherent for the binding; arrow shape is no longer used as a proxy.
    /// Packages nested in an arrow remain wrapped until result destruction.
    fn instantiate_local(&mut self, span: Span, symbol: Symbol, scheme: &Scheme) -> Rc<Ty> {
        // Only slots occurring directly in the root package are coherent for a
        // lexical value. Nested package nodes are separate production
        // boundaries and must remain fresh even when the same scheme also owns
        // an arrow-value effect at its root.
        enum Work {
            Ty(Rc<Ty>),
            Row(Row),
        }
        let mut root_slots = IndexSet::new();
        let mut work = match &**scheme.body() {
            Ty::Package(body) => vec![Work::Ty(body.clone())],
            _ => Vec::new(),
        };
        while let Some(next) = work.pop() {
            match next {
                Work::Ty(ty) => match &*ty {
                    Ty::Package(_) => {} // a nested owner
                    Ty::Arrow(from, to, effects) => {
                        work.push(Work::Row(effects.clone()));
                        work.push(Work::Ty(to.clone()));
                        work.push(Work::Ty(from.clone()));
                    }
                    Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                    Ty::Named { args, .. } => work.extend(args.iter().cloned().map(Work::Ty)),
                    _ => {}
                },
                Work::Row(row) => {
                    for field in row.labels.values() {
                        if let Presence::Bound(index) = field.presence
                            && scheme.is_existential(index)
                        {
                            root_slots.insert(index);
                        }
                        work.push(Work::Ty(field.ty.clone()));
                    }
                    if let Rest::More(more) = &row.rest {
                        work.push(Work::Row((**more).clone()));
                    }
                }
            }
        }
        let coherent = (!root_slots.is_empty()).then_some((symbol, root_slots));
        let instantiated = self.instantiate_scoped(span, scheme, coherent);
        match &*instantiated {
            Ty::Package(_) => self.open_package(span, &instantiated),
            _ => instantiated,
        }
    }

    fn instantiate_scoped(
        &mut self,
        span: Span,
        scheme: &Scheme,
        coherent: Option<(Symbol, IndexSet<u32>)>,
    ) -> Rc<Ty> {
        // One fresh variable per position the scheme bound, handed over as a
        // bare type: which sort each one is, is decided where it lands, since
        // that is what a scheme records. See [`Assigned::as_row`]. A presence
        // is minted in its own alphabet, for the reason [`Scheme`] gives.
        let fresh: Vec<Assigned> = (0..scheme.count())
            .map(|at| match at < scheme.presences() {
                true => {
                    let key = coherent
                        .as_ref()
                        .filter(|(_, slots)| slots.contains(&at))
                        .map(|(symbol, _)| (*symbol, at));
                    let presence = key
                        .and_then(|key| self.local_package_instances.get(&key).cloned())
                        .unwrap_or_else(|| {
                            let fresh = self.fresh_instance_presence();
                            if let Some(key) = key {
                                self.local_package_instances.insert(key, fresh.clone());
                            }
                            fresh
                        });
                    if scheme.is_existential(at)
                        && let Presence::Var(var) = &presence
                    {
                        self.existential_witnesses.insert(*var);
                        self.abstract_existentials.insert(*var);
                    }
                    Assigned::Presence(presence)
                }
                false => Assigned::Ty(self.fresh_instance_type()),
            })
            .collect();
        let ty = scheme.body().open(&fresh);
        // A scheme's body says which of its rows a quantified tail is the tail
        // of, but the condition that follows from that is not part of the
        // body: it lived beside the variables the scheme closed over, and this
        // copy's variables are new. Said again, of them.
        self.note_lacks(&ty);
        // And so is what it requires of its presences. Per instance, never per
        // scheme: two uses of one definition with different field sets are both
        // legal exactly when each instance's formula is separately satisfiable.
        let formula = scheme.formula().open(&fresh);
        let immediate = self.register_package_guarantees(&ty, formula);
        if !immediate.is_true() {
            let mut labels = IndexMap::new();
            let mut shape = None;
            self.labels_in(&ty, &mut labels, &mut shape, &immediate);
            let labels = labels.into_values().collect();
            self.require(span, Origin::Instance(Named { labels, shape }), immediate);
        }
        ty
    }

    /// Attach each owned conjunct to the package allocation named by its
    /// structural preorder owner. Unowned/mixed conjuncts remain immediate.
    fn register_package_guarantees(&mut self, ty: &Rc<Ty>, formula: Formula) -> Formula {
        enum Work {
            Ty(Rc<Ty>, bool),
            Row(Row),
        }
        let mut packages = Vec::new();
        let mut work = vec![Work::Ty(ty.clone(), true)];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty, root) => match &*ty {
                    Ty::Package(body) => {
                        let key: Vec<_> =
                            collect_owned_existentials(body, &self.abstract_existentials)
                                .into_iter()
                                .collect();
                        packages.push((key, !root));
                        work.push(Work::Ty(body.clone(), false));
                    }
                    Ty::Arrow(from, to, effects) => {
                        work.push(Work::Row(effects.clone()));
                        work.push(Work::Ty(to.clone(), false));
                        work.push(Work::Ty(from.clone(), false));
                    }
                    Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                    Ty::Named { args, .. } => {
                        work.extend(args.iter().rev().cloned().map(|ty| Work::Ty(ty, false)));
                    }
                    _ => {}
                },
                Work::Row(row) => {
                    if let Rest::More(more) = &row.rest {
                        work.push(Work::Row((**more).clone()));
                    }
                    work.extend(
                        row.labels
                            .values()
                            .rev()
                            .map(|field| Work::Ty(field.ty.clone(), false)),
                    );
                }
            }
        }

        // Opening behavior belongs to the package node even when its scheme has
        // no nontrivial owned clause. In particular, an unconstrained fresh
        // result still has to alpha-rename its hidden witnesses on every call.
        for (key, fresh) in &packages {
            self.package_guarantees
                .entry(key.clone())
                .or_insert(PackageGuarantee {
                    clauses: IndexSet::new(),
                    fresh: *fresh,
                });
        }

        let mut pending = vec![formula];
        let mut immediate = Vec::new();
        while let Some(part) = pending.pop() {
            match &part {
                Formula::And(left, right) => {
                    pending.push((**right).clone());
                    pending.push((**left).clone());
                }
                Formula::Owned(owner, inner) => {
                    if let Some((key, fresh)) = packages.get(*owner as usize) {
                        self.package_guarantees
                            .entry(key.clone())
                            .and_modify(|before| {
                                before.clauses.insert((**inner).clone());
                            })
                            .or_insert_with(|| PackageGuarantee {
                                clauses: std::iter::once((**inner).clone()).collect(),
                                fresh: *fresh,
                            });
                    }
                }
                _ => immediate.push(part),
            }
        }
        Formula::all(immediate)
    }

    /// Destroy one semantic package. Nested result packages alpha-rename their
    /// abstract presences on every destruction; root lexical packages retain
    /// the coherent identity selected by `instantiate_local` (including R16's
    /// fresh identity for a separately bound alias).
    fn open_package(&mut self, span: Span, package: &Rc<Ty>) -> Rc<Ty> {
        let Ty::Package(body) = &**package else {
            return package.clone();
        };
        let key: Vec<_> = collect_owned_existentials(body, &self.abstract_existentials)
            .into_iter()
            .collect();
        let guarantee = self.package_guarantees.get(&key).cloned();
        let mut renames = HashMap::new();
        if guarantee.as_ref().is_some_and(|guarantee| guarantee.fresh) {
            for var in collect_owned_existentials(body, &self.abstract_existentials) {
                let fresh = self.fresh_instance_presence();
                if let Presence::Var(fresh_var) = fresh {
                    // The alpha-renamed identity is just as sealed as the
                    // scheme witness it replaces. Otherwise a consumer could
                    // choose a fresh result merely because it was opened twice.
                    self.existential_witnesses.insert(fresh_var);
                    self.abstract_existentials.insert(fresh_var);
                    renames.insert(var, Presence::Var(fresh_var));
                }
            }
        }
        let opened = if renames.is_empty() {
            body.clone()
        } else {
            substitute_presence_vars(body, &renames)
        };
        if let Some(guarantee) = guarantee {
            let formula = Formula::all(guarantee.clauses.iter().cloned()).rename(&|var| {
                renames
                    .get(&var)
                    .cloned()
                    .unwrap_or(Presence::Var(var))
                    .formula()
            });
            if !formula.is_true() {
                let mut labels = IndexMap::new();
                let mut shape = None;
                self.labels_in(&opened, &mut labels, &mut shape, &formula);
                self.require(
                    span,
                    Origin::Instance(Named {
                        labels: labels.into_values().collect(),
                        shape,
                    }),
                    formula,
                );
            }
        }
        opened
    }

    /// Every nested struct-field path a match demand names and the presence
    /// deciding it. Unlike diagnostic labels, paths retain their parents so
    /// `x.a` and `y.a` remain two readable facts in a refinement trace. Match
    /// demands are structural, so a declared name cannot reach this walk.
    fn presence_paths(
        &self,
        ty: &Rc<Ty>,
        prefix: &mut PresencePath,
        found: &mut IndexMap<PresencePath, Presence>,
    ) {
        let ty = self.resolve(ty);
        if let Ty::Struct(row) = &*ty {
            for (name, field) in &self.canon(row).labels {
                prefix.push(name.clone());
                found
                    .entry(prefix.clone())
                    .or_insert_with(|| self.presence_of(&field.presence));
                if !matches!(self.presence_of(&field.presence), Presence::Absent) {
                    self.presence_paths(&field.ty, prefix, found);
                }
                prefix.pop();
            }
        }
    }

    /// Every label a type names and what decides whether it is there, in the
    /// order the type names them — what a use-site complaint quotes its formula
    /// in. The first spelling of a label wins, which is the one a reader
    /// reading the type left to right meets. A formula gets a shape only when
    /// all of its named presences belong to that shape; a relation spanning
    /// nested shapes must use the neutral diagnostic vocabulary.
    fn labels_in(
        &self,
        ty: &Rc<Ty>,
        found: &mut IndexMap<String, (String, Presence)>,
        formula_shape: &mut Option<Shape>,
        formula: &Formula,
    ) {
        enum Work {
            Ty(Rc<Ty>),
            Field(Shape, String, RowField),
            Effects(Row),
        }

        let mut atoms = Vec::new();
        formula.atoms(&mut atoms);
        let names_atom = |presence: &Presence| match presence {
            Presence::Var(var) => atoms.contains(&Atom::Var(*var)),
            Presence::Bound(index) => atoms.contains(&Atom::Bound(*index)),
            _ => false,
        };
        let mut matched_shape = None;
        let mut mixed_shapes = false;
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Arrow(a, b, effects) => {
                            work.push(Work::Effects(effects.clone()));
                            work.push(Work::Ty(b.clone()));
                            work.push(Work::Ty(a.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => {
                            let shape = match &*ty {
                                Ty::Struct(_) => Shape::Struct,
                                Ty::Sum(_) => Shape::Sum,
                                _ => unreachable!(),
                            };
                            let (labels, _) = self.canon(row).into_parts();
                            work.extend(
                                labels
                                    .into_iter()
                                    .rev()
                                    .map(|(name, field)| Work::Field(shape, name, field)),
                            );
                        }
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        _ => {}
                    }
                }
                Work::Field(shape, name, field) => {
                    let presence = self.presence_of(&field.presence);
                    if names_atom(&presence) {
                        match matched_shape {
                            Some(found) if found != shape => mixed_shapes = true,
                            None => matched_shape = Some(shape),
                            _ => {}
                        }
                    }
                    found
                        .entry(name.clone())
                        .or_insert_with(|| (name, presence.clone()));
                    match presence {
                        Presence::Absent => {}
                        _ => work.push(Work::Ty(field.ty)),
                    }
                }
                Work::Effects(row) => {
                    let (labels, _) = self.canon(&row).into_parts();
                    for (name, field) in labels {
                        let presence = self.presence_of(&field.presence);
                        if names_atom(&presence) {
                            match matched_shape {
                                Some(Shape::Effect) => {}
                                Some(_) => mixed_shapes = true,
                                None => matched_shape = Some(Shape::Effect),
                            }
                        }
                        found.entry(name.clone()).or_insert_with(|| {
                            let shown =
                                EffectId::parse_row_key(&name).map_or(name.as_str(), |x| x.0);
                            (shown.to_string(), presence)
                        });
                    }
                }
            }
        }
        *formula_shape = (!mixed_shapes).then_some(matched_shape).flatten();
    }

    /// [`unfold`] with the row conditions its result implies recorded against
    /// this table's variables.
    ///
    /// A declaration's body says which of its rows a `..` parameter is the tail
    /// of, but the condition that follows — `type WithX 'r = { x: Nat, ..'r }`
    /// says `'r` has no `x` — is not part of the body. It lived beside variables
    /// the declaration never had, and this use site's are new, so it has to be
    /// said again of them. Exactly what [`Constrain::instantiate`] does for a
    /// scheme, for exactly the same reason.
    ///
    /// Only when there was an unfolding, which is what the identity check is:
    /// [`unfold`] hands back the very type it was given unless the type was a
    /// name, and both callers ask on every application and every checked term,
    /// where a name is the rare case. Walking a type that did not change would
    /// record nothing that is not already recorded — everything reaching the
    /// solver goes through [`lower_type`] or [`Constrain::instantiate`] first,
    /// and both note what they built — at the cost of resolving every variable
    /// and allocating a set per row, on nearly every constraint.
    fn unfolded(&mut self, aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
        // A package may conceal a name and an alias may unfold to another
        // package. Alternate the two transparent operations until a real shape
        // is exposed; doing either only once leaves `Package (Alias
        // (Package ...))` opaque to callers such as ABI validation.
        let mut exposed = ty.clone();
        loop {
            while let Ty::Package(body) = &*exposed {
                exposed = body.clone();
            }
            let next = unfold(aliases, &exposed);
            if Rc::ptr_eq(&next, &exposed) {
                break;
            }
            exposed = next;
        }
        if !Rc::ptr_eq(&exposed, ty) {
            self.note_lacks(&exposed);
        }
        exposed
    }

    /// Every a variable variable a type still mentions that the scheme being
    /// generalized into may not quantify, reported at the declaration.
    ///
    /// A walk rather than a level or a rank discipline on variables, and that
    /// is enough here because the language has one annotation scope per
    /// binding: a rigid belongs to exactly one annotation, `owned` is the list
    /// that annotation declared, and anything else in the type came from
    /// somewhere it cannot mean anything.
    ///
    /// Said once per variable across the whole program. A rigid that reaches
    /// two schemes it does not belong to is still one annotation to rewrite,
    /// and a reader sent to the same line twice learns nothing the second time.
    fn escapes(&mut self, ty: &Rc<Ty>, owned: &[u32], errors: &mut Vec<Error>) {
        let mut found = IndexMap::new();
        self.rigids_in(ty, &mut found);
        for (id, name) in found {
            if owned.contains(&id) || !self.escaped.insert(id) {
                continue;
            }
            let span = self.rigids[&id];
            errors.push(Error {
                id: self.error_id(),
                cause: ErrorCause::Direct,
                span,
                kind: ErrorKind::RigidEscapes { name },
                explanation: None,
            });
        }
    }

    /// Every rigid a type mentions, by id, with the spelling it prints as, in
    /// the order the type mentions them. A leaf wherever it appears, so this is
    /// the ordinary walk with one arm that collects — and two rows a rigid can
    /// tail, a sum's cases and an arrow's effects, read the same way.
    fn rigids_in(&self, ty: &Rc<Ty>, found: &mut IndexMap<u32, Rc<str>>) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Rigid { id, name } => {
                            found.entry(*id).or_insert_with(|| name.clone());
                        }
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        _ => {}
                    }
                }
                Work::Row(row) => {
                    let row = self.canon(&row);
                    work.extend(
                        row.labels
                            .values()
                            .rev()
                            .filter(|field| {
                                !matches!(self.presence_of(&field.presence), Presence::Absent)
                            })
                            .map(|field| Work::Ty(field.ty.clone())),
                    );
                    if let Rest::Rigid { id, name } = &row.rest {
                        found.entry(*id).or_insert_with(|| name.clone());
                    }
                }
            }
        }
    }

    /// The labels `value` names that `var` may not stand for, each with what
    /// its presence has resolved to, and the kind of row the condition came
    /// from: what the lacks check reads, for [`Solve::assign`] to rule on.
    ///
    /// Read here and ruled on there, because the ruling depends on presence
    /// and one of its three answers is an act only the solver can perform. A
    /// label certainly there is the contradiction the condition exists for; a
    /// label settled absent is not part of what the type says, and no copy of
    /// anything; and a label still being decided has just met the thing that
    /// decides it, which is a binding the table cannot make on its own.
    ///
    /// In the row's own order rather than in the order the condition was
    /// recorded, so that a complaint names the label a reader would reach
    /// first reading the type left to right. The shape comes off the recorded
    /// condition, since a row-tail variable's labels are the fields of whatever it
    /// is being bound to and there is no row here to read a shape from.
    fn lacked(&self, var: TyVar, value: &Assigned) -> Option<(Shape, Vec<(String, Presence)>)> {
        let (shape, lacks) = self.lacks.get(&var)?;
        // Whatever labels the value would bring with it, read at the shape the
        // condition was recorded in: the fields of a whole type, or the cases a
        // sum's rest stands for. A presence brings neither, which
        // [`Assigned::as_row`] says by answering with a row that names nothing
        // and [`Assigned::as_ty`] by answering with a type that carries none.
        let labels: IndexMap<String, RowField> = match shape {
            Shape::Struct | Shape::Sum | Shape::Effect => {
                self.canon(&value.as_row()).into_parts().0
            }
        };
        let named = labels
            .iter()
            .filter(|(name, _)| lacks.contains(*name))
            .map(|(name, field)| (name.clone(), self.presence_of(&field.presence)))
            .collect();
        Some((*shape, named))
    }

    /// Carry the lacks condition across a binding. What `var` may not stand
    /// for, whatever is still open past `value` may not stand for either — and
    /// `value`'s own rows impose their names on their own tails, which is
    /// [`note_lacks`](Self::note_lacks).
    ///
    /// Without this a condition would survive exactly one binding: `..'r`
    /// absorbing a `y` continues as a fresh tail, and that tail is where the
    /// next field to conflict would arrive.
    fn inherit_lacks(&mut self, var: TyVar, value: &Assigned) {
        self.note_lacks_value(value, self.lacks_shape(var));
        let Some((shape, labels)) = self.lacks.get(&var).cloned() else {
            return;
        };
        let row = value.as_row();
        self.forbid(&row, shape, &labels);
    }

    /// [`note_lacks`](Self::note_lacks) about a value of any sort. `shape` is
    /// what the variable it is being bound to sits at the open end of, since a
    /// row on its own has no shape to be read in.
    fn note_lacks_value(&mut self, value: &Assigned, shape: Shape) {
        match value {
            Assigned::Ty(ty) => {
                let ty = ty.clone();
                self.note_lacks(&ty);
            }
            Assigned::Row(row) => {
                let row = (**row).clone();
                self.note_lacks_row(&row, shape);
            }
            // Whether a label is there says nothing about which labels there
            // are.
            Assigned::Presence(_) => {}
        }
    }

    /// The kind of row a variable sits at the open end of, or [`Shape::Struct`]
    /// for one nothing has said anything about yet. Only the reading a
    /// complaint would be worded in depends on it, and a tail with no condition
    /// recorded has nothing to complain about.
    fn lacks_shape(&self, var: TyVar) -> Shape {
        match self.lacks.get(&var) {
            Some((shape, _)) => *shape,
            None => Shape::Struct,
        }
    }

    /// Close every effect row variable this type mentions exactly once.
    ///
    /// R23. An effect variable the solver learned nothing about links nothing:
    /// it sits on one arrow and no other, so quantifying it would publish a
    /// scheme whose caller may choose what the definition performs — which no
    /// definition means. Closing it to the empty row is what makes
    /// `fn x => x` read `'a -> 'a` rather than `'a -> 'a + ..'b`, and one that
    /// genuinely does link two arrows occurs twice and is quantified.
    ///
    /// Only variables inference minted, which after a variable is what an
    /// effect tail a reader kept is *not*: a named `..'e` is declared and lowers
    /// to a rigid, which is no solver variable and is never counted here. What
    /// the reader left anonymous — a bare `+ ..`, or no row at all — is
    /// inference's to decide, and this is the deciding.
    ///
    /// And only what this scheme would quantify: a variable below `level`
    /// belongs to a binder further out, which may still put an effect in it.
    fn close_effects(&mut self, ty: &Rc<Ty>, level: u32) {
        let mut counted: IndexMap<TyVar, usize> = IndexMap::new();
        self.count_effects(ty, &mut counted);
        for (var, count) in counted {
            if count != 1 || self.levels[var as usize] < level {
                continue;
            }
            self.default_bind(
                var,
                Assigned::Row(Rc::new(Row::closed())),
                DefaultBinding::CloseEffects,
                DefaultAssignment::EmptyRow,
                Vec::new(),
            );
        }
    }

    /// How often each still-open effect row variable appears in the effect
    /// position of an arrow inside `ty`.
    ///
    /// Effect positions alone, which is what makes the count well defined: a
    /// variable's sort is fixed where it was minted, and nothing can unify a
    /// sum's tail with an arrow's effects, so a variable found here is found
    /// nowhere but here.
    fn count_effects(&self, ty: &Rc<Ty>, found: &mut IndexMap<TyVar, usize>) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row, bool),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone(), true));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => {
                            work.push(Work::Row(row.clone(), false));
                        }
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        _ => {}
                    }
                }
                Work::Row(row, effects) => {
                    let row = self.canon(&row);
                    if effects && let Rest::Var(var) = row.rest {
                        *found.entry(var).or_default() += 1;
                    }
                    work.extend(
                        row.labels
                            .values()
                            .rev()
                            .filter(|field| {
                                !matches!(self.presence_of(&field.presence), Presence::Absent)
                            })
                            .map(|field| Work::Ty(field.ty.clone())),
                    );
                }
            }
        }
    }

    /// Quantify everything in `ty` still unsolved at or above `level`.
    /// Returns the scheme and the substitution that built it, so the caller can
    /// spell the same variables the same way elsewhere.
    ///
    /// A variable below the level is left where it stands, as a [`Ty::Var`]
    /// in the scheme's body: it belongs to an enclosing binder, so every use of
    /// this scheme is to share it rather than get a copy. That is the whole of
    /// what makes `fn p => let q = p.x in q` one type rather than two — see
    /// [`Table::demote`] — and it is why a scheme published here may still
    /// mention the table. A definition's own scheme never does: its level is 0,
    /// and nothing is below that.
    fn generalize(&self, ty: &Rc<Ty>, level: u32, formula: Formula) -> (Scheme, Subst) {
        let mut subst = Subst::default();
        self.quantify(ty, &mut subst, level);
        let body = self.zonk(ty, &subst);
        let formula = quantify_formula(&formula, &subst);
        let mut existentials: IndexSet<u32> = subst
            .presences
            .iter()
            .filter_map(|(var, index)| self.existential_witnesses.contains(var).then_some(*index))
            .collect();
        let (body, inferred) = package_positive_presences(
            &body,
            subst.presences.len() as u32,
            &existentials,
            &self.variances,
        );
        existentials.extend(inferred);
        (
            Scheme::existential(
                subst.next(),
                subst.presences.len() as u32,
                existentials,
                body,
                formula,
            ),
            subst,
        )
    }

    /// What the scheme of a type generalized here should require of its
    /// presences: everything the store says about the presences the type still
    /// mentions, with every other variable existentially eliminated, in the
    /// canonical form R12 prints.
    ///
    /// Silent once something has already flipped the store: the cascade rule.
    /// A `where false` on every scheme downstream of one contradiction is the
    /// same mistake said in as many places as the program has definitions.
    fn required(&self, ty: &Rc<Ty>) -> Formula {
        self.required_given(ty, &Formula::True)
    }

    /// What a type generalized inside a reachable arm requires while that arm's
    /// premise holds. Conjoining the premise before projection prevents
    /// existential elimination from turning `E -> Q(local)` into `true`.
    fn required_given(&self, ty: &Rc<Ty>, premise: &Formula) -> Formula {
        if self.unsat {
            return Formula::True;
        }
        let mut presences = IndexSet::new();
        self.presences_in(ty, &mut presences);
        let atoms: Vec<Atom> = presences.into_iter().map(Atom::Var).collect();
        let needed = self.component(&atoms).and(self.resolved(premise));
        sat::project(&needed, &atoms)
    }

    /// What the store says about every presence variable `subst` numbered, in
    /// that numbering: the formula [`patterns`](crate::patterns) walks a
    /// definition under. See [`Output::promises`].
    ///
    /// Beside [`required`](Self::required) rather than in it, and deliberately
    /// unlike it in two ways. The atoms are the substitution's rather than the
    /// type's, so a presence only a nested binding's type mentions is spoken
    /// for; and nothing is projected away, because the walk asks only whether
    /// an assignment exists — a variable left free answers that as well as a
    /// quantified one, and eliminating it would cost a walk of the projection
    /// for nothing.
    ///
    /// Silent once something has already flipped the store, for the reason
    /// [`required`](Self::required) is.
    fn promised(&self, subst: &Subst) -> Formula {
        if self.unsat {
            return Formula::True;
        }
        // By the number each was given, so that what the walk is handed does
        // not depend on how a hash map happened to order itself.
        let mut numbered: Vec<(TyVar, u32)> = subst
            .presences
            .iter()
            .map(|(var, at)| (*var, *at))
            .collect();
        numbered.sort_by_key(|(_, at)| *at);
        let atoms: Vec<Atom> = numbered
            .into_iter()
            .map(|(var, _)| Atom::Var(var))
            .collect();
        quantify_formula(&self.component(&atoms), subst)
    }

    /// A local binding's scheme as it is published: everything it left free
    /// numbered on past its own quantifiers of the same sort, so that a reader
    /// is shown letters rather than the solver's `?3` and the scheme still says
    /// truthfully which of its positions are presences.
    ///
    /// Only for [`Output::locals`], and only once the solve that could still
    /// bind those variables is over. The scheme the solver instantiates is the
    /// one [`generalize`](Self::generalize) produced, free variables and all;
    /// numbering them would hand each use a copy of a variable it is meant to
    /// share.
    fn published(&self, scheme: &Scheme) -> Scheme {
        let mut subst = Subst::default();
        self.quantify(scheme.body(), &mut subst, 0);
        // Each sort numbers on from the end of the scheme's own quantifiers of
        // that sort, rather than past all of them at once: the low positions
        // are the presences and the rest are types and rows, and a scheme that
        // said otherwise about itself would be lying to everything that reads
        // [`Scheme::presences`]. So a free presence lands directly above the
        // scheme's own presences, and the scheme's own types and rows move up
        // to make room — which is what `shift` below is for.
        let free = subst.presences.len() as u32;
        let base = scheme.count();
        let shifted = Subst {
            types: subst
                .types
                .into_iter()
                .map(|(var, at)| (var, base + at))
                .collect(),
            presences: subst
                .presences
                .into_iter()
                .map(|(var, at)| (var, scheme.presences() + at))
                .collect(),
            // A scheme's body holds no rigid: generalization numbered every
            // one into a [`Ty::Bound`] before it published anything, so
            // there is nothing here left to shift.
            rigids: HashMap::new(),
        };
        let mut existentials = scheme.existentials().clone();
        existentials.extend(
            shifted.presences.iter().filter_map(|(var, index)| {
                self.existential_witnesses.contains(var).then_some(*index)
            }),
        );
        Scheme::existential(
            base + shifted.next(),
            scheme.presences() + free,
            existentials,
            self.zonk(&shift(scheme.body(), free), &shifted),
            quantify_formula(scheme.formula(), &shifted),
        )
    }

    /// Number everything `ty` quantifies, in first-occurrence order within each
    /// of the two passes — which is what makes the leftmost variable of each
    /// print as the earlier letter.
    ///
    /// The presences go first and in full, because they take the low positions
    /// of the one index space a scheme has: what a variable lists is every
    /// letter in index order, and a numbering that interleaved the two sorts
    /// would still be correct but would put the letters in an order nobody
    /// reading the type left to right could predict. Whichever pass runs, a
    /// variable can only be numbered by one of them — a variable's sort is
    /// fixed where it was minted — so the two can never number one twice.
    fn quantify(&self, ty: &Rc<Ty>, subst: &mut Subst, level: u32) {
        self.quantify_walk(ty, subst, level, true);
        self.quantify_walk(ty, subst, level, false);
    }

    /// One numbering pass over one type: over everything but the presence
    /// slots, or — when `presences` — over only them. A variable can only be one
    /// or the other, so the two passes cannot number one twice. Either way the
    /// descent is the same: the fields first and then the constructor they sit on,
    /// which is what makes `{ x: 'a, ..'b }` number left to right.
    ///
    /// Fields first because the constructor is what the `..` prints, and a tail is read
    /// last. The rule used to be the other way round, when the type and row tail were separate
    /// a type had and its tail was another; now the constructor *is* the tail, so
    /// numbering it first would call the rightmost thing on the line `a`. No
    /// special case for it either way — it is descended into exactly where it
    /// sits.
    fn quantify_walk(&self, ty: &Rc<Ty>, subst: &mut Subst, level: u32, presences: bool) {
        enum Work {
            Ty(Rc<Ty>),
            Row(Row),
            Presence(Presence),
            Tail(Rest),
        }

        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Var(var) if !presences => self.quantify_var(*var, subst, level),
                        Ty::Rigid { id, .. } if !presences => {
                            let next = subst.next();
                            subst.rigids.entry(*id).or_insert(next);
                        }
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        _ => {}
                    }
                }
                Work::Row(row) => {
                    let row = self.canon(&row);
                    if !presences {
                        work.push(Work::Tail(row.rest.clone()));
                    }
                    for field in row.labels.values().rev() {
                        let presence = self.presence_of(&field.presence);
                        if !matches!(presence, Presence::Absent) {
                            work.push(Work::Ty(field.ty.clone()));
                        }
                        if presences {
                            work.push(Work::Presence(presence));
                        }
                    }
                }
                Work::Presence(presence) => {
                    if let Presence::Var(var) = self.presence_of(&presence) {
                        self.quantify_presence(var, subst, level);
                    }
                }
                Work::Tail(Rest::Var(var)) => self.quantify_var(var, subst, level),
                Work::Tail(Rest::Rigid { id, .. }) => {
                    let next = subst.next();
                    subst.rigids.entry(id).or_insert(next);
                }
                Work::Tail(Rest::Closed | Rest::Bound(_) | Rest::Undecided | Rest::More(_)) => {}
            }
        }
    }

    /// Number one variable, unless it already has a number or belongs to a
    /// binder further out. See [`Table::levels`].
    fn quantify_var(&self, var: TyVar, subst: &mut Subst, level: u32) {
        if self.levels[var as usize] < level {
            return;
        }
        let next = subst.next();
        subst.types.entry(var).or_insert(next);
    }

    /// [`quantify_var`](Self::quantify_var) at the low end of the space, which
    /// the presences have to themselves. See [`Scheme`].
    fn quantify_presence(&self, var: TyVar, subst: &mut Subst, level: u32) {
        if self.levels[var as usize] < level {
            return;
        }
        let next = subst.presences.len() as u32;
        subst.presences.entry(var).or_insert(next);
    }

    /// Resolve a type all the way down, replacing each variable in `subst`
    /// with its quantified stand-in. What comes back mentions the variable
    /// table only where the caller chose to leave it mentioning one — see
    /// [`Table::generalize`], whose scheme keeps an enclosing binder's
    /// variables free — so everything published at level 0 outlives the solver.
    fn zonk(&self, ty: &Rc<Ty>, subst: &Subst) -> Rc<Ty> {
        enum Work {
            Ty(Rc<Ty>),
            Arrow,
            Package,
            Struct,
            Sum,
            Named {
                symbol: Symbol,
                name: Rc<str>,
                args: usize,
            },
            Row(Row),
            BuiltRow {
                labels: Vec<(String, Presence)>,
                rest: Rest,
            },
        }

        let mut work = vec![Work::Ty(ty.clone())];
        let mut types = Vec::new();
        let mut rows = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Var(var) => types.push(Rc::new(
                            subst
                                .types
                                .get(var)
                                .map_or(Ty::Var(*var), |at| Ty::Bound(*at)),
                        )),
                        Ty::Rigid { id, .. } => types.push(Rc::new(Ty::Bound(subst.rigids[id]))),
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Arrow);
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Ty(to.clone()));
                            work.push(Work::Ty(from.clone()));
                        }
                        Ty::Package(body) => {
                            work.push(Work::Package);
                            work.push(Work::Ty(body.clone()));
                        }
                        Ty::Struct(row) => {
                            work.push(Work::Struct);
                            work.push(Work::Row(row.clone()));
                        }
                        Ty::Sum(row) => {
                            work.push(Work::Sum);
                            work.push(Work::Row(row.clone()));
                        }
                        Ty::Named { symbol, name, args } => {
                            work.push(Work::Named {
                                symbol: *symbol,
                                name: name.clone(),
                                args: args.len(),
                            });
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        other => types.push(Rc::new(other.clone())),
                    }
                }
                Work::Row(row) => {
                    let row = self.canon(&row);
                    let rest = match &row.rest {
                        Rest::Var(var) => subst
                            .types
                            .get(var)
                            .map_or(Rest::Var(*var), |at| Rest::Bound(*at)),
                        Rest::Rigid { id, .. } => Rest::Bound(subst.rigids[id]),
                        rest => rest.clone(),
                    };
                    let labels: Vec<_> = row
                        .labels
                        .iter()
                        .map(|(name, field)| {
                            let presence = match self.presence_of(&field.presence) {
                                Presence::Var(var) => subst
                                    .presences
                                    .get(&var)
                                    .map_or(Presence::Var(var), |at| Presence::Bound(*at)),
                                decided => decided,
                            };
                            (name.clone(), presence)
                        })
                        .collect();
                    work.push(Work::BuiltRow {
                        labels: labels.clone(),
                        rest,
                    });
                    work.extend(row.labels.values().zip(labels).rev().filter_map(
                        |(field, (_, presence))| {
                            (!matches!(presence, Presence::Absent))
                                .then(|| Work::Ty(field.ty.clone()))
                        },
                    ));
                }
                Work::Arrow => {
                    let effects = rows.pop().expect("zonked effects row");
                    let to = types.pop().expect("zonked arrow result");
                    let from = types.pop().expect("zonked arrow argument");
                    types.push(Rc::new(Ty::Arrow(from, to, effects)));
                }
                Work::Package => {
                    let body = types.pop().expect("zonked package body");
                    types.push(Rc::new(Ty::Package(body)));
                }
                Work::Struct => {
                    let row = rows.pop().expect("zonked struct row");
                    types.push(Rc::new(Ty::Struct(row)));
                }
                Work::Sum => {
                    let row = rows.pop().expect("zonked sum row");
                    types.push(Rc::new(Ty::Sum(row)));
                }
                Work::Named { symbol, name, args } => {
                    let mut opened = Vec::with_capacity(args);
                    for _ in 0..args {
                        opened.push(types.pop().expect("zonked named argument"));
                    }
                    opened.reverse();
                    types.push(Rc::new(Ty::Named {
                        symbol,
                        name,
                        args: opened.into(),
                    }));
                }
                Work::BuiltRow { labels, rest } => {
                    let mut built = Vec::with_capacity(labels.len());
                    for (name, presence) in labels.into_iter().rev() {
                        let ty = match presence {
                            Presence::Absent => Rc::new(Ty::Undecided),
                            _ => types.pop().expect("zonked field payload"),
                        };
                        built.push((name, RowField { presence, ty }));
                    }
                    built.reverse();
                    rows.push(Row {
                        labels: built.into_iter().collect(),
                        rest,
                    });
                }
            }
        }
        types.pop().expect("a zonked type")
    }

    /// [`zonk`](Self::zonk) applied to every type the walk wrote into a
    /// definition's body, once the definition is solved.
    ///
    /// The substitution grows as the walk goes, because the body is a larger
    /// domain than the definition's own type: in `let a = k 1 (fn z => z)` the
    /// argument is typed `?5 -> ?5`, which `a : Nat` never mentions and
    /// generalization therefore never numbered. A variable like that is
    /// unconstrained rather than unknown, so it is quantified here and
    /// numbered on from the scheme's — which leaves no [`Ty::Var`] anywhere in
    /// the program for a consumer to have to resolve, and still spells a
    /// variable the scheme does name the way the scheme names it.
    /// Resolve `ty` and give a name to whatever is still unsolved in it,
    /// numbering on from `subst` so that a variable the scheme already named
    /// keeps that name. The one rule everything outliving the solver goes
    /// through, so no two of them can spell one variable differently.
    /// At level 0, because this is the last word: whatever a term or a
    /// complaint still mentions when the group is done belongs to nobody
    /// further out, so every variable left in it is named rather than left for
    /// a reader to resolve.
    fn close(&self, ty: &Rc<Ty>, subst: &mut Subst) -> Rc<Ty> {
        self.quantify(ty, subst, 0);
        self.zonk(ty, subst)
    }

    fn zonk_term(&self, term: &mut Term, subst: &mut Subst) {
        term.ty = self.close(&term.ty, subst);
        match &mut term.kind {
            TermKind::Unary { value, .. } => self.zonk_term(value, subst),
            TermKind::Binary { left, right, .. } => {
                self.zonk_term(left, subst);
                self.zonk_term(right, subst);
            }
            TermKind::Apply { func, arg } => {
                self.zonk_term(func, subst);
                self.zonk_term(arg, subst);
            }
            TermKind::Fn { body, .. } => self.zonk_term(body, subst),
            TermKind::Let { value, body, .. } => {
                self.zonk_term(value, subst);
                self.zonk_term(body, subst);
            }
            TermKind::Struct(fields) => {
                for field in fields.values_mut() {
                    self.zonk_term(&mut field.value, subst);
                }
            }
            TermKind::Project { base, .. } => self.zonk_term(base, subst),
            TermKind::Tag { payload, .. } => {
                if let Some(payload) = payload {
                    self.zonk_term(payload, subst);
                }
            }
            TermKind::Match { scrutinee, arms } => {
                self.zonk_term(scrutinee, subst);
                for (_, body) in arms {
                    self.zonk_term(body, subst);
                }
            }
            TermKind::Handle { body, handler } => {
                self.zonk_term(body, subst);
                for arm in &mut handler.arms {
                    self.zonk_term(&mut arm.body, subst);
                }
                if let Some(ret) = &mut handler.ret {
                    self.zonk_term(&mut ret.body, subst);
                }
            }
            TermKind::Raise(value) => self.zonk_term(value, subst),
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

    /// [`close`](Self::close) applied to the types one complaint carries.
    ///
    /// Only the types move: which complaint this *is* was settled where it was
    /// reported, by the code that knew why, so nothing here can reword one by
    /// re-reading a payload the solve went on to change.
    ///
    /// A payload can name a variable neither the scheme nor the body does —
    /// the parameter of something that turned out not to be a function belongs
    /// to no term — so this quantifies as it goes rather than reading a
    /// finished substitution. Left alone, such a variable would reach the
    /// reader as `?7`, which names the solver's bookkeeping rather than
    /// anything they wrote, and would spell as `?7` a type the scheme beside
    /// it spells `a`.
    fn zonk_error(&self, kind: &ErrorKind, subst: &mut Subst) -> ErrorKind {
        match kind {
            ErrorKind::NotAStruct { base } => ErrorKind::NotAStruct {
                base: self.close(base, subst),
            },
            ErrorKind::Mismatch { expected, actual } => ErrorKind::Mismatch {
                expected: self.close(expected, subst),
                actual: self.close(actual, subst),
            },
            ErrorKind::Recursive => ErrorKind::Recursive,
            ErrorKind::MissingField { shape, base, field } => ErrorKind::MissingField {
                shape: *shape,
                base: self.close(base, subst),
                field: field.clone(),
            },
            ErrorKind::ExtraField { shape, base, field } => ErrorKind::ExtraField {
                shape: *shape,
                base: self.close(base, subst),
                field: field.clone(),
            },
            ErrorKind::RigidBroken {
                found,
                name,
                sense,
                declared,
            } => ErrorKind::RigidBroken {
                found: self.close(found, subst),
                name: name.clone(),
                sense: *sense,
                declared: *declared,
            },
            // The label and the variable are both spellings, and the span is a
            // place: nothing here is a type for a later substitution to improve.
            ErrorKind::RigidField {
                shape,
                field,
                name,
                declared,
            } => ErrorKind::RigidField {
                shape: *shape,
                field: field.clone(),
                name: name.clone(),
                declared: *declared,
            },
            ErrorKind::RigidEscapes { name } => ErrorKind::RigidEscapes { name: name.clone() },
            // The presence complaints carry prose rather than types:
            // their formulas were already worded, at the moment the variables
            // in them still had labels to be named by. There is nothing here
            // for a later substitution to improve.
            ErrorKind::PresenceRequired { formula, shape } => ErrorKind::PresenceRequired {
                formula: formula.clone(),
                shape: *shape,
            },
            ErrorKind::PresenceImpossible { formula } => ErrorKind::PresenceImpossible {
                formula: formula.clone(),
            },
            ErrorKind::ClauseImpossible { formula } => ErrorKind::ClauseImpossible {
                formula: formula.clone(),
            },
            ErrorKind::AnnotationAllows { allowed, required } => ErrorKind::AnnotationAllows {
                allowed: allowed.clone(),
                required: required.clone(),
            },
            ErrorKind::RepeatedField { shape, field } => ErrorKind::RepeatedField {
                shape: *shape,
                field: field.clone(),
            },
            // The two effect complaints carry a label rather than a type, for
            // the reason the presence ones carry prose: what the reader can
            // change is the signature, and the effect's name is the whole of
            // what it says.
            ErrorKind::Unhandled { effect } => ErrorKind::Unhandled {
                effect: effect.clone(),
            },
            ErrorKind::NotAllowed { effect } => ErrorKind::NotAllowed {
                effect: effect.clone(),
            },
            ErrorKind::CallbackEffectsNotCovered => ErrorKind::CallbackEffectsNotCovered,
            ErrorKind::PolymorphicExternBoundary => ErrorKind::PolymorphicExternBoundary,
        }
    }
}

/// One formula with each variable a scheme quantified replaced by the position
/// it was given — [`Table::zonk`] about a formula rather than about a type.
///
/// A variable with no number is left standing, for the same reason a
/// [`Ty::Var`] is: it belongs to a binder further out, and a conjunct linking
/// a quantified presence to an outer one moves into the scheme with the outer
/// one kept free. That is standard HM(X), and the level machinery is what
/// decided the entitlement.
fn quantify_formula(formula: &Formula, subst: &Subst) -> Formula {
    formula.rename(&|var| match subst.presences.get(&var) {
        Some(at) => Formula::bound(*at),
        None => Formula::var(var),
    })
}

fn immediate_formula(formula: Formula) -> Formula {
    let mut pending = vec![formula];
    let mut parts = Vec::new();
    while let Some(part) = pending.pop() {
        match &part {
            Formula::Owned(..) => {}
            Formula::And(left, right) => {
                pending.push((**right).clone());
                pending.push((**left).clone());
            }
            _ => parts.push(part),
        }
    }
    Formula::all(parts)
}

fn collect_owned_existentials(body: &Rc<Ty>, abstract_: &HashSet<TyVar>) -> IndexSet<TyVar> {
    enum Work {
        Ty(Rc<Ty>),
        Row(Row),
    }
    let mut found = IndexSet::new();
    let mut work = vec![Work::Ty(body.clone())];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &*ty {
                Ty::Package(_) => {} // belongs to the nested package
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Row(effects.clone()));
                    work.push(Work::Ty(to.clone()));
                    work.push(Work::Ty(from.clone()));
                }
                Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                Ty::Named { args, .. } => work.extend(args.iter().rev().cloned().map(Work::Ty)),
                _ => {}
            },
            Work::Row(row) => {
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row((**more).clone()));
                }
                for field in row.labels.values().rev() {
                    if let Presence::Var(var) = field.presence
                        && abstract_.contains(&var)
                    {
                        found.insert(var);
                    }
                    work.push(Work::Ty(field.ty.clone()));
                }
            }
        }
    }
    found
}

/// Alpha-rename presence variables throughout one package without using the
/// native stack; payloads and composed rows can be adversarially deep.
fn substitute_presence_vars(root: &Rc<Ty>, renames: &HashMap<TyVar, Presence>) -> Rc<Ty> {
    enum Work {
        Ty(Rc<Ty>),
        Row(Row),
        Arrow,
        Package,
        Struct,
        Sum,
        Named(Symbol, Rc<str>, usize),
        BuiltRow(Vec<(String, Presence)>, Rest),
    }
    let mut work = vec![Work::Ty(root.clone())];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &*ty {
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects.clone()));
                    work.push(Work::Ty(to.clone()));
                    work.push(Work::Ty(from.clone()));
                }
                Ty::Package(body) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(body.clone()));
                }
                Ty::Struct(row) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(row.clone()));
                }
                Ty::Sum(row) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(row.clone()));
                }
                Ty::Named { symbol, name, args } => {
                    work.push(Work::Named(*symbol, name.clone(), args.len()));
                    work.extend(args.iter().rev().cloned().map(Work::Ty));
                }
                other => types.push(Rc::new(other.clone())),
            },
            Work::Row(row) => {
                let labels = row
                    .labels
                    .iter()
                    .map(|(name, field)| {
                        let presence = match field.presence {
                            Presence::Var(var) => {
                                renames.get(&var).cloned().unwrap_or(Presence::Var(var))
                            }
                            _ => field.presence.clone(),
                        };
                        (name.clone(), presence)
                    })
                    .collect();
                work.push(Work::BuiltRow(labels, row.rest.clone()));
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row((**more).clone()));
                }
                work.extend(
                    row.labels
                        .values()
                        .rev()
                        .filter(|field| !matches!(field.presence, Presence::Absent))
                        .map(|field| Work::Ty(field.ty.clone())),
                );
            }
            Work::Arrow => {
                let effects = rows.pop().unwrap();
                let to = types.pop().unwrap();
                let from = types.pop().unwrap();
                types.push(Rc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().unwrap();
                types.push(Rc::new(Ty::Package(body)));
            }
            Work::Struct => types.push(Rc::new(Ty::Struct(rows.pop().unwrap()))),
            Work::Sum => types.push(Rc::new(Ty::Sum(rows.pop().unwrap()))),
            Work::Named(symbol, name, count) => {
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(types.pop().unwrap());
                }
                args.reverse();
                types.push(Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                }));
            }
            Work::BuiltRow(labels, rest) => {
                let rest = match rest {
                    Rest::More(_) => Rest::More(Rc::new(rows.pop().unwrap())),
                    rest => rest,
                };
                let mut built = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    let ty = if matches!(presence, Presence::Absent) {
                        Rc::new(Ty::Undecided)
                    } else {
                        types.pop().unwrap()
                    };
                    built.push((name, RowField { presence, ty }));
                }
                built.reverse();
                rows.push(Row {
                    labels: built.into_iter().collect(),
                    rest,
                });
            }
        }
    }
    types.pop().unwrap()
}

/// Least variance solution for semantic declared bodies. Alias schemes are
/// closed and use their low bound positions as declaration parameters. The
/// finite two-bit lattice terminates for recursive/imported graphs and records
/// erased, covariant, contravariant and invariant parameters exactly.
fn semantic_variances(aliases: &IndexMap<Symbol, Scheme>) -> HashMap<(Symbol, u32), u8> {
    enum Work {
        Ty(Rc<Ty>, bool),
        Row(Row, bool),
    }
    let mut out = HashMap::new();
    for (symbol, scheme) in aliases {
        for at in 0..scheme.count() {
            out.insert((*symbol, at), 0);
        }
    }
    loop {
        let before = out.clone();
        for (owner, scheme) in aliases {
            // Invariant named parameters enqueue an argument at both
            // polarities. Join those states at each shared semantic node so a
            // deep nest is visited at most twice, rather than branching 2^n.
            let mut seen: HashMap<usize, u8> = HashMap::new();
            let mut work = vec![Work::Ty(scheme.body().clone(), true)];
            while let Some(part) = work.pop() {
                match part {
                    Work::Ty(ty, positive) => {
                        let bit = if positive { 1 } else { 2 };
                        let visited = seen.entry(Rc::as_ptr(&ty) as usize).or_default();
                        if *visited & bit != 0 {
                            continue;
                        }
                        *visited |= bit;
                        match &*ty {
                            Ty::Bound(index) if *index < scheme.count() => {
                                *out.entry((*owner, *index)).or_default() |=
                                    if positive { 1 } else { 2 };
                            }
                            Ty::Arrow(from, to, effects) => {
                                work.push(Work::Row(effects.clone(), positive));
                                work.push(Work::Ty(to.clone(), positive));
                                work.push(Work::Ty(from.clone(), !positive));
                            }
                            Ty::Package(body) => work.push(Work::Ty(body.clone(), positive)),
                            Ty::Struct(row) | Ty::Sum(row) => {
                                work.push(Work::Row(row.clone(), positive))
                            }
                            Ty::Named { symbol, args, .. } => {
                                for (at, arg) in args.iter().enumerate() {
                                    let variance =
                                        before.get(&(*symbol, at as u32)).copied().unwrap_or(3);
                                    if variance & 1 != 0 {
                                        work.push(Work::Ty(arg.clone(), positive));
                                    }
                                    if variance & 2 != 0 {
                                        work.push(Work::Ty(arg.clone(), !positive));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Work::Row(row, positive) => {
                        work.extend(
                            row.labels
                                .values()
                                .map(|field| Work::Ty(field.ty.clone(), positive)),
                        );
                        match &row.rest {
                            Rest::Bound(index) if *index < scheme.count() => {
                                *out.entry((*owner, *index)).or_default() |=
                                    if positive { 1 } else { 2 };
                            }
                            Rest::More(more) => work.push(Work::Row((**more).clone(), positive)),
                            _ => {}
                        }
                    }
                }
            }
        }
        if out == before {
            return out;
        }
    }
}

/// Classify solver-inferred presence quantifiers by polarity and seal every
/// positive-only class at its exact semantic production node. Explicit
/// annotation variables are rigids rather than these low presence positions,
/// so this cannot turn a caller-chosen annotation variable existential.
fn package_positive_presences(
    body: &Rc<Ty>,
    presences: u32,
    already: &IndexSet<u32>,
    variances: &HashMap<(Symbol, u32), u8>,
) -> (Rc<Ty>, IndexSet<u32>) {
    #[derive(Default)]
    struct Uses {
        positive: u32,
        negative: u32,
        owners: IndexSet<usize>,
    }
    enum Scan {
        Ty(Rc<Ty>, bool, usize),
        Row(Row, bool, usize),
    }
    let root = Rc::as_ptr(body) as usize;
    let mut uses: HashMap<u32, Uses> = HashMap::new();
    // A named invariant argument is traversed at both polarities. Memoize the
    // joined polarity state per package owner and semantic allocation so deep
    // alias nests stay linear in their expanded path rather than exponential.
    let mut seen: HashMap<(usize, usize), u8> = HashMap::new();
    let mut work = vec![Scan::Ty(body.clone(), true, root)];
    while let Some(item) = work.pop() {
        match item {
            Scan::Ty(ty, positive, owner) => {
                let bit = if positive { 1 } else { 2 };
                let visited = seen.entry((owner, Rc::as_ptr(&ty) as usize)).or_default();
                if *visited & bit != 0 {
                    continue;
                }
                *visited |= bit;
                match &*ty {
                    Ty::Arrow(from, to, effects) => {
                        work.push(Scan::Ty(from.clone(), !positive, owner));
                        let result_owner = if positive {
                            Rc::as_ptr(to) as usize
                        } else {
                            owner
                        };
                        work.push(Scan::Ty(to.clone(), positive, result_owner));
                        work.push(Scan::Row(effects.clone(), positive, owner));
                    }
                    Ty::Package(inner) => work.push(Scan::Ty(inner.clone(), positive, owner)),
                    Ty::Struct(row) | Ty::Sum(row) => {
                        work.push(Scan::Row(row.clone(), positive, owner))
                    }
                    Ty::Named { symbol, args, .. } => {
                        for (at, arg) in args.iter().enumerate() {
                            let variance =
                                variances.get(&(*symbol, at as u32)).copied().unwrap_or(3);
                            if variance & 1 != 0 {
                                work.push(Scan::Ty(arg.clone(), positive, owner));
                            }
                            if variance & 2 != 0 {
                                work.push(Scan::Ty(arg.clone(), !positive, owner));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Scan::Row(row, positive, owner) => {
                for field in row.labels.values() {
                    if let Presence::Bound(index) = field.presence
                        && index < presences
                        && !already.contains(&index)
                    {
                        let use_ = uses.entry(index).or_default();
                        if positive {
                            use_.positive += 1
                        } else {
                            use_.negative += 1
                        }
                        if positive {
                            use_.owners.insert(owner);
                        }
                    }
                    work.push(Scan::Ty(field.ty.clone(), positive, owner));
                }
                if let Rest::More(more) = &row.rest {
                    work.push(Scan::Row((**more).clone(), positive, owner));
                }
            }
        }
    }
    let inferred: IndexSet<u32> = uses
        .iter()
        .filter_map(|(index, use_)| {
            (use_.positive > 0 && use_.negative == 0 && use_.owners.len() == 1).then_some(*index)
        })
        .collect();
    let owners: HashSet<usize> = uses
        .iter()
        .filter(|(index, _)| inferred.contains(*index))
        .map(|(_, use_)| *use_.owners.first().expect("positive presence has owner"))
        .collect();
    if owners.is_empty() {
        return (body.clone(), inferred);
    }

    enum Build {
        Ty(Rc<Ty>),
        Arrow(bool),
        Package(bool),
        Struct(bool),
        Sum(bool),
        Named(bool, Symbol, Rc<str>, usize),
        Row(Row),
        FinishRow(Vec<(String, Presence)>, Rest),
    }
    let mut work = vec![Build::Ty(body.clone())];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(item) = work.pop() {
        match item {
            Build::Ty(ty) => {
                let wrap = owners.contains(&(Rc::as_ptr(&ty) as usize));
                match &*ty {
                    Ty::Arrow(from, to, effects) => {
                        work.push(Build::Arrow(wrap));
                        work.push(Build::Row(effects.clone()));
                        work.push(Build::Ty(to.clone()));
                        work.push(Build::Ty(from.clone()));
                    }
                    Ty::Package(inner) => {
                        work.push(Build::Package(wrap));
                        work.push(Build::Ty(inner.clone()));
                    }
                    Ty::Struct(row) => {
                        work.push(Build::Struct(wrap));
                        work.push(Build::Row(row.clone()));
                    }
                    Ty::Sum(row) => {
                        work.push(Build::Sum(wrap));
                        work.push(Build::Row(row.clone()));
                    }
                    Ty::Named { symbol, name, args } => {
                        work.push(Build::Named(wrap, *symbol, name.clone(), args.len()));
                        work.extend(args.iter().rev().cloned().map(Build::Ty));
                    }
                    other => {
                        let value = Rc::new(other.clone());
                        types.push(if wrap {
                            Rc::new(Ty::Package(value))
                        } else {
                            value
                        });
                    }
                }
            }
            Build::Row(row) => {
                let labels: Vec<_> = row
                    .labels
                    .iter()
                    .map(|(name, field)| (name.clone(), field.presence.clone()))
                    .collect();
                work.push(Build::FinishRow(labels, row.rest.clone()));
                if let Rest::More(more) = &row.rest {
                    work.push(Build::Row((**more).clone()));
                }
                work.extend(
                    row.labels
                        .values()
                        .rev()
                        .map(|field| Build::Ty(field.ty.clone())),
                );
            }
            Build::FinishRow(labels, rest) => {
                let mut built = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    built.push((
                        name,
                        RowField {
                            presence,
                            ty: types.pop().expect("packaged row payload"),
                        },
                    ));
                }
                built.reverse();
                let rest = match rest {
                    Rest::More(_) => {
                        Rest::More(Rc::new(rows.pop().expect("packaged composed row tail")))
                    }
                    rest => rest,
                };
                rows.push(Row {
                    labels: built.into_iter().collect(),
                    rest,
                });
            }
            Build::Arrow(wrap) => {
                let effects = rows.pop().unwrap();
                let to = types.pop().unwrap();
                let from = types.pop().unwrap();
                let value = Rc::new(Ty::Arrow(from, to, effects));
                types.push(if wrap {
                    Rc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Package(wrap) => {
                let value = Rc::new(Ty::Package(types.pop().unwrap()));
                types.push(if wrap {
                    Rc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Struct(wrap) => {
                let value = Rc::new(Ty::Struct(rows.pop().unwrap()));
                types.push(if wrap {
                    Rc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Sum(wrap) => {
                let value = Rc::new(Ty::Sum(rows.pop().unwrap()));
                types.push(if wrap {
                    Rc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Named(wrap, symbol, name, count) => {
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(types.pop().unwrap());
                }
                args.reverse();
                let value = Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                });
                types.push(if wrap {
                    Rc::new(Ty::Package(value))
                } else {
                    value
                });
            }
        }
    }
    (types.pop().expect("packaged type"), inferred)
}

/// One scheme body with every type and row position it quantifies moved up by
/// `by`, to make room below them for that many more presences.
///
/// Only [`Table::published`] needs this, and only because a scheme's presences
/// are promised the low positions of its index space: numbering what a local
/// left free means fitting free presences in beside the scheme's own, and
/// everything above the presences has to move for them. A presence position is
/// below the ones that move and is left exactly where it is, which is why
/// neither [`Presence::Bound`] nor the formula's [`Atom::Bound`] appears here.
///
/// Unconditional over [`Ty::Bound`] and [`Rest::Bound`]: every one of those is
/// a type or a row position by construction, since a presence is only ever
/// written as a [`Presence::Bound`].
fn shift(ty: &Rc<Ty>, by: u32) -> Rc<Ty> {
    enum Work {
        Ty(Rc<Ty>),
        Row(Row),
        Arrow,
        Package,
        Struct,
        Sum,
        Named(Symbol, Rc<str>, usize),
        BuiltRow(Vec<(String, Presence)>, Rest),
    }
    let mut work = vec![Work::Ty(ty.clone())];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &*ty {
                Ty::Bound(at) => types.push(Rc::new(Ty::Bound(at + by))),
                Ty::Arrow(from, to, effects) => {
                    work.push(Work::Arrow);
                    work.push(Work::Row(effects.clone()));
                    work.push(Work::Ty(to.clone()));
                    work.push(Work::Ty(from.clone()));
                }
                Ty::Package(body) => {
                    work.push(Work::Package);
                    work.push(Work::Ty(body.clone()));
                }
                Ty::Struct(row) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(row.clone()));
                }
                Ty::Sum(row) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(row.clone()));
                }
                Ty::Named { symbol, name, args } => {
                    work.push(Work::Named(*symbol, name.clone(), args.len()));
                    work.extend(args.iter().rev().cloned().map(Work::Ty));
                }
                other => types.push(Rc::new(other.clone())),
            },
            Work::Row(row) => {
                let labels = row
                    .labels
                    .iter()
                    .map(|(name, field)| (name.clone(), field.presence.clone()))
                    .collect();
                work.push(Work::BuiltRow(labels, row.rest.clone()));
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row((**more).clone()));
                }
                work.extend(
                    row.labels
                        .values()
                        .rev()
                        .filter(|field| !matches!(field.presence, Presence::Absent))
                        .map(|field| Work::Ty(field.ty.clone())),
                );
            }
            Work::Arrow => {
                let effects = rows.pop().expect("shifted effects");
                let to = types.pop().expect("shifted result");
                let from = types.pop().expect("shifted parameter");
                types.push(Rc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("shifted package");
                types.push(Rc::new(Ty::Package(body)));
            }
            Work::Struct => types.push(Rc::new(Ty::Struct(rows.pop().expect("shifted struct")))),
            Work::Sum => types.push(Rc::new(Ty::Sum(rows.pop().expect("shifted sum")))),
            Work::Named(symbol, name, count) => {
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(types.pop().expect("shifted named argument"));
                }
                args.reverse();
                types.push(Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                }));
            }
            Work::BuiltRow(labels, rest) => {
                let rest = match rest {
                    Rest::Bound(at) => Rest::Bound(at + by),
                    Rest::More(_) => Rest::More(Rc::new(rows.pop().expect("shifted composed row"))),
                    rest => rest,
                };
                let mut built = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    let ty = match presence {
                        Presence::Absent => Rc::new(Ty::Undecided),
                        _ => types.pop().expect("shifted field payload"),
                    };
                    built.push((name, RowField { presence, ty }));
                }
                built.reverse();
                rows.push(Row {
                    labels: built.into_iter().collect(),
                    rest,
                });
            }
        }
    }
    types.pop().expect("shifted type")
}

/// The semantic type a written type denotes. A declared type stays the name it
/// was written as — `Endo` stays `Endo`, and what it stands for is looked up
/// where a shape is actually needed — and a type that failed to lower becomes
/// [`Ty::Undecided`], which absorbs rather than cascades.
///
/// Keeping the name is what makes a recursive declaration lowerable at all: a
/// body that named its own meaning would have to contain it, and nothing
/// finite does. It is also what a reader gets told, since a type prints as
/// itself; the mint is here only to spell it, never to decide anything.
///
/// The table is here for the parts of an annotation that stand for something
/// the definition gets to decide: a `..` tail, a `..'r` tail, and a `when` field
/// each lower to a fresh variable, minted at the level the annotation is
/// lowered at so that what stays unconstrained is quantified with the
/// definition. One call is one annotation, which is the whole scope of a
/// tail's name: every `..'r` in it shares one variable, and no other
/// annotation's `'r` can reach it.
///
/// A `type` declaration's body reaches none of that. A `when` and a bare `..` are
/// refused there outright, and the one tail it may have names a row parameter,
/// which lowers to a [`Ty::Bound`] — a leaf whose value comes from the use
/// site, not a variable this table has to solve. So a declaration still lowers
/// to something with no [`Ty::Var`] anywhere in it, which several walks here
/// rely on: it is why they may stop at a name rather than descend into what it
/// stands for.
fn lower_type(mint: &Mint, table: &mut Table, ty: &Type) -> Rc<Ty> {
    let mut tails = Tails::default();
    let lowered = lower(mint, table, &mut tails, ty);
    // A tail stands for the fields its row did not write out, and this is
    // where that is first true of a written one: `{ x: Nat, ..'r }` says `'r`
    // has no `x`. See [`Table::lacks`].
    table.note_lacks(&lowered);
    lowered
}

/// What one annotation's an annotation introduced, ready for the type beside it to
/// be lowered against.
///
/// One map per sort a variable can have, because the three are three sorts of
/// value — lowering has already refused a name used at two, so no name is ever
/// in more than one. A declared type or row variable is a rigid, minted before
/// the type is walked because a use may come first; a declared presence is an
/// ordinary solver variable, exactly as `when a` has always minted one, for the
/// reason the spec's assumption gives: presences remain governed by the SAT
/// store rather than skolemized.
///
/// Empty for a `type` declaration's body, which declares nothing and whose
/// tails are its parameters.
#[derive(Default)]
struct Tails {
    types: HashMap<String, Ty>,
    rows: HashMap<String, Rest>,
    /// The presences, by the names their `when`s wear. Two labels wearing one
    /// name share one variable, which is how a type says two fields are there
    /// together, and what the `where` clause beside them resolves against.
    presences: HashMap<String, Presence>,
    anonymous: HashMap<u32, Presence>,
}

/// Everything one lowered annotation says: the type its body is checked
/// against, the scheme a use of its own name inside that body instantiates, and
/// what it promised about its presences.
struct Lowered {
    /// The skolemized type. Its variables are rigids and its holes
    /// are ordinary solver variables, which is the whole of the difference
    /// between what the annotation promises and what it leaves to inference.
    ty: Rc<Ty>,
    /// [`Lowered::ty`] with its rigids quantified and nothing else — what a
    /// recursive use of the name is a copy of.
    ///
    /// Quantifying the rigids is what makes polymorphic recursion over a
    /// declared variable typable: each mention gets its own fresh copy of `r`,
    /// so `depth n.kids` handing a closed record where the annotation wrote an
    /// open one is no conflict. Leaving the holes alone is what keeps the rest
    /// monomorphic: they are the one thing the group is deciding, and a copy of
    /// one would decide nothing.
    scheme: Scheme,
    /// What the `where` clause requires, over the presence variables the
    /// annotation minted.
    formula: Formula,
    /// Scheme-wide assumptions only. Package-owned guarantees stay inert until
    /// `open_package` destroys their exact boundary.
    assumptions: Formula,
    /// The presence names it bound and what each lowered to — what a complaint
    /// about the clause quotes it in, since the reader wrote `a` and never saw
    /// the variable.
    names: Vec<(String, Presence)>,
    /// The ids of the type and row variables it declared, which are the ones
    /// its own scheme may quantify. See [`Table::escapes`].
    rigids: Vec<u32>,
}

/// The semantic type a written annotation denotes, with everything else the
/// annotation settles.
///
/// The declarations first, because a use may be written before the statement
/// that declares it and every one of them has to resolve. Then the type. Then
/// the clause, whatever order a reader's eye takes the two in, because there is
/// nothing to resolve a formula's names against until the labels that wear them
/// are lowered.
fn lower_annotation(mint: &Mint, table: &mut Table, annotation: &Annotation) -> Lowered {
    let mut tails = Tails::default();
    let mut at: HashMap<u32, u32> = HashMap::new();
    let mut rigids = Vec::new();
    for variable in &annotation.variables {
        table.rigids.insert(variable.id, variable.span);
        let name: Rc<str> = variable.name.as_str().into();
        match variable.sense {
            Sense::Type => {
                at.insert(variable.id, at.len() as u32);
                rigids.push(variable.id);
                tails.types.insert(
                    variable.name.clone(),
                    Ty::Rigid {
                        id: variable.id,
                        name,
                    },
                );
            }
            // Every row sort has an explicit rest.
            // from the same map: which row a use splices into was fixed where
            // the variable's sense was, so the two senses lower the same way.
            Sense::Fields | Sense::Cases | Sense::Effects => {
                at.insert(variable.id, at.len() as u32);
                rigids.push(variable.id);
                let rest = Rest::Rigid {
                    id: variable.id,
                    name,
                };
                tails.rows.insert(variable.name.clone(), rest);
            }
            // A presence is a solver variable like any other `when`'s: the
            // clause beside it and the SAT store are what govern it, and
            // nothing here is skolemized.
            Sense::Presence => {
                let presence = table.fresh_presence_for(Subject::Annotation);
                if matches!(
                    variable.ownership,
                    crate::ir::PresenceOwnership::Existential { .. }
                ) && let Presence::Var(var) = &presence
                {
                    table.existential_witnesses.insert(*var);
                }
                tails.presences.insert(variable.name.clone(), presence);
            }
        }
    }
    let formula = match &annotation.clause {
        Some(clause) => clause_formula(&tails, clause),
        None => Formula::True,
    };
    let boundaries: HashSet<Span> = annotation
        .variables
        .iter()
        .filter_map(|variable| match variable.ownership {
            crate::ir::PresenceOwnership::Existential { boundary } => Some(boundary),
            crate::ir::PresenceOwnership::Universal => None,
        })
        .chain(
            annotation
                .anonymous_existentials
                .iter()
                .map(|(_, boundary)| *boundary),
        )
        .collect();
    let ty = lower_scoped(mint, table, &mut tails, &annotation.ty, Some(&boundaries));
    for (id, _) in &annotation.anonymous_existentials {
        if let Some(Presence::Var(var)) = tails.anonymous.get(id) {
            table.existential_witnesses.insert(*var);
        }
    }
    // A tail stands for the fields its row did not write out, and this is where
    // that is first true of a written one: `{ x: Nat, ..h }` says the hole `h`
    // has no `x`. A rigid needs none of it — nothing can ever be bound to one,
    // so every label demanded of one is refused outright instead. See
    // [`Table::lacks`] and [`ErrorKind::RigidField`].
    table.note_lacks(&ty);
    // Recursive and external uses see a genuinely closed annotation
    // interface. Every named presence belongs to that published interface:
    // producer-owned slots open as fresh hidden witnesses, while universal
    // slots open as fresh caller choices. Leaving the latter free made all
    // recursive invocations share one solver variable and let one invocation
    // choose the supposedly universal presence for its siblings. Anonymous
    // holes remain the shared decisions of the surrounding definition.
    let mut presence_subst = HashMap::new();
    let mut existential_slots = IndexSet::new();
    for variable in &annotation.variables {
        if variable.sense != Sense::Presence {
            continue;
        }
        let Presence::Var(var) = tails.presences[&variable.name] else {
            continue;
        };
        let index = presence_subst.len() as u32;
        presence_subst.insert(var, index);
        if matches!(
            variable.ownership,
            crate::ir::PresenceOwnership::Existential { .. }
        ) {
            existential_slots.insert(index);
        }
    }
    for (id, _) in &annotation.anonymous_existentials {
        let Some(Presence::Var(var)) = tails.anonymous.get(id) else {
            continue;
        };
        let index = presence_subst.len() as u32;
        presence_subst.insert(*var, index);
        existential_slots.insert(index);
    }
    let presences = presence_subst.len() as u32;
    let subst = Subst {
        types: HashMap::new(),
        presences: presence_subst,
        rigids: at
            .iter()
            .map(|(id, index)| (*id, presences + index))
            .collect(),
    };
    let scheme = Scheme::existential(
        presences + at.len() as u32,
        presences,
        existential_slots,
        table.zonk(&ty, &subst),
        quantify_formula(&formula, &subst),
    );
    let mut original = vec![Assigned::Ty(Rc::new(Ty::Undecided)); scheme.count() as usize];
    for (var, index) in &subst.presences {
        original[*index as usize] = Assigned::Presence(Presence::Var(*var));
    }
    let assumptions = if sat::satisfiable(&formula) {
        immediate_formula(scheme.formula().open(&original))
    } else {
        // An intrinsically impossible contract is invalid before any package
        // can be produced; retain it for the ordinary annotation diagnostic.
        formula.clone()
    };
    // In one fixed order, because they come out of a hash map and the order it
    // hands them over is nobody's: a complaint about the clause names its
    // presences the same way on every run, which alphabetical is enough for.
    // Spelled with the sigil they were written with: a complaint about the
    // clause quotes what the reader can find on the page.
    let mut names: Vec<(String, Presence)> = tails
        .presences
        .into_iter()
        .map(|(name, presence)| (format!("'{name}"), presence))
        .collect();
    names.sort_by(|(one, _), (other, _)| one.cmp(other));
    Lowered {
        ty,
        scheme,
        formula,
        assumptions,
        names,
        rigids,
    }
}

/// One lowered `where` clause as the formula it denotes, over the presence
/// variables the annotation's `when`s minted.
///
/// A name with no variable behind it is one whose label did not survive
/// lowering — a row refused for some other reason lowers to the error type, and
/// the labels inside it go with it — so it claims nothing rather than being
/// looked up and found missing. The first complaint already speaks.
fn clause_formula(tails: &Tails, clause: &Clause) -> Formula {
    match &clause.tracked {
        ClauseKind::Name(name) => tails
            .presences
            .get(name)
            .map_or(Formula::True, Presence::formula),
        ClauseKind::Not(inner) => clause_formula(tails, inner).not(),
        ClauseKind::And(left, right) => {
            clause_formula(tails, left).and(clause_formula(tails, right))
        }
        ClauseKind::Or(left, right) => clause_formula(tails, left).or(clause_formula(tails, right)),
        ClauseKind::Equal(left, right) => {
            clause_formula(tails, left).iff(clause_formula(tails, right))
        }
        ClauseKind::NotEqual(left, right) => {
            clause_formula(tails, left).xor(clause_formula(tails, right))
        }
    }
}

/// The recursion inside [`lower_type`], carrying the annotation's named-tail
/// scope.
fn lower(mint: &Mint, table: &mut Table, tails: &mut Tails, ty: &Type) -> Rc<Ty> {
    lower_scoped(mint, table, tails, ty, None)
}

/// Lower with producer package boundaries supplied by the IR polarity pass.
/// Spans identify exact result nodes, so nested arrows need no root-shape
/// heuristic and sibling production scopes remain distinct.
fn lower_scoped(
    mint: &Mint,
    table: &mut Table,
    tails: &mut Tails,
    ty: &Type,
    boundaries: Option<&HashSet<Span>>,
) -> Rc<Ty> {
    let lowered = match &ty.tracked {
        TypeKind::Prim(prim) => (*prim).into(),
        TypeKind::Ident(symbol) => Ty::Named {
            symbol: *symbol,
            name: mint.name(*symbol).into(),
            // Lowering counted the arguments, so a name that reaches here bare
            // is one that takes none.
            args: Rc::from([]),
        },
        TypeKind::Apply { head, args, .. } => Ty::Named {
            symbol: *head,
            name: mint.name(*head).into(),
            args: args
                .iter()
                .map(|arg| lower_scoped(mint, table, tails, arg, boundaries))
                .collect(),
        },
        // A parameter is the position it was declared at, which is what
        // unfolding hands an argument to. See [`Ty::Bound`].
        TypeKind::Param { index, .. } => Ty::Bound(*index),
        TypeKind::Arrow { from, to, effects } => Ty::Arrow(
            lower_scoped(mint, table, tails, from, boundaries),
            lower_scoped(mint, table, tails, to, boundaries),
            effect_row(table, tails, effects),
        ),
        // A row handed to a declaration as an argument, which is the one place
        // a row arrives without an arrow around it. It lowers to the row it is
        // — [`Ty::Sum`] is a set of labels and a rest, which is what a row of
        // effects is too — and the position it is spliced into is what says
        // which of the three it is being read as. See
        // [`types::Shape`](crate::types::Shape).
        TypeKind::Effects(effects) => Ty::Sum(effect_row(table, tails, effects)),
        // A written struct is unit carrying the fields that were written: the
        // fields are what the type says, and there is nothing else to it. A
        // field written `\name` is [`Presence::Absent`] in the position it was
        // written, its type deliberately unconstrained — a field that is not
        // there has nothing to have a type.
        TypeKind::Struct { fields, tail } => {
            let mut labels = IndexMap::new();
            for (name, field) in fields {
                let lowered = match field {
                    ir::TypeField::Written { when, value, .. } => RowField {
                        presence: presence(table, tails, when),
                        ty: lower_scoped(mint, table, tails, value, boundaries),
                    },
                    ir::TypeField::Absent { .. } => RowField {
                        presence: Presence::Absent,
                        ty: Rc::new(Ty::default()),
                    },
                };
                labels.insert(name.clone(), lowered);
            }
            Ty::Struct(row(table, tails, labels, tail))
        }
        // The struct arm again, about cases — except that a sum's cases are its
        // constructor, distinct from struct fields. The one other difference is
        // the payload a case may not have written, which is unit
        // — the same type `()` is, built here rather than in the tree so that
        // what the reader wrote and what the compiler means stay two separate
        // things. See [`ir::TermKind::Tag`](crate::ir::TermKind::Tag).
        TypeKind::Sum { cases, tail } => {
            let mut labels = IndexMap::new();
            for (name, case) in cases {
                let lowered = match case {
                    ir::SumCase::Written { when, payload, .. } => {
                        let presence = presence(table, tails, when);
                        let carried = match payload {
                            Some(payload) => lower_scoped(mint, table, tails, payload, boundaries),
                            None => Rc::new(Ty::unit()),
                        };
                        RowField {
                            presence,
                            ty: carried,
                        }
                    }
                    // The struct's absent field again: a case a value can
                    // never be carries nothing worth constraining.
                    ir::SumCase::Absent { .. } => RowField {
                        presence: Presence::Absent,
                        ty: Rc::new(Ty::default()),
                    },
                };
                labels.insert(name.clone(), lowered);
            }
            Ty::Sum(row(table, tails, labels, tail))
        }
        // A hole the solver fills: a fresh variable, so whatever the position
        // meets decides it. `_` written by a reader and the field types of the
        // pattern desugar's exact demand are the same thing — a position left
        // to be decided, whoever left it — and the variable is what lets a
        // projection of a field and the demand for it share one answer.
        TypeKind::Hole => Ty::Var(table.mint(VarSort::Type, Subject::Annotation)),
        // A variable in a type position: the rigid its declaration
        // minted, shared by every mention of the name in this one annotation.
        //
        // Indexed rather than looked up: lowering refused a name nothing
        // declared, and a name read at another sort absorbed into
        // [`TypeKind::Error`] rather than reaching here.
        TypeKind::Var(name) => tails.types[name].clone(),
        TypeKind::Error => Ty::Undecided,
    };
    let body = Rc::new(lowered);
    match boundaries.is_some_and(|boundaries| boundaries.contains(&ty.span)) {
        true => Rc::new(Ty::Package(body)),
        false => body,
    }
}

/// Whether one label is there, as its `when` clause says it: no clause means it
/// is simply there, a named one is the variable the annotation's own
/// declaration minted for that name, and `when _` is a variable of its own that
/// nothing can name.
///
/// Indexed rather than looked up: a named `when` is a *use*, so lowering has
/// already refused a name nothing declared and erased one read at another sort,
/// and [`lower_annotation`] minted a variable for every name that survived.
fn presence(table: &mut Table, tails: &mut Tails, when: &Option<Box<ir::When>>) -> Presence {
    let Some(when) = when else {
        return Presence::Present;
    };
    let Some(name) = &when.name else {
        return tails
            .anonymous
            .entry(when.id)
            .or_insert_with(|| table.fresh_presence_for(Subject::Annotation))
            .clone();
    };
    tails.presences[name].clone()
}

/// The cases of a written sum and what its `..` stands for, assembled into the
/// row. `..` is a variable this definition may decide, `..'r` is that variable
/// shared across one annotation, and `..'r` naming a parameter is a position an
/// argument is handed to.
fn row(
    table: &mut Table,
    tails: &mut Tails,
    labels: IndexMap<String, RowField>,
    tail: &Option<Tail>,
) -> Row {
    let rest = match tail.as_ref().map(|tail| &tail.of) {
        None => Rest::Closed,
        Some(ir::Row::Anything) => table.fresh_row_for(Subject::Annotation),
        // The rigid its declaration minted, shared by every `..'r` in this one
        // annotation. Indexed for the reason [`presence`] is.
        Some(ir::Row::Named(name)) => tails.rows[name].clone(),
        // A row parameter is its position, the same as a type one: what it
        // stands for is spliced in where this sits, by the flattening
        // [`Table::canon`] already does for a tail bound to a row.
        Some(ir::Row::Param { index, .. }) => Rest::Bound(*index),
    };
    Row { labels, rest }
}

/// The effects a written arrow says it may perform, as the [`Row`] they denote.
///
/// [`row`] about the third shape, and shorter for it: an effect carries
/// nothing, so every label holds [`Ty::unit`] and no walk ever looks at it. The
/// tail goes the same four ways a sum's does — no `..` is [`Rest::Closed`],
/// which is what a pure arrow has; a bare `..` is a variable this definition
/// may decide; `..'e` is that variable shared across one annotation; and `..'e`
/// naming a parameter is the position an argument is handed to.
///
/// The variables a bare `..` mints are ordinary solver variables, which is what
/// makes them R23's to close: what a reader left anonymous is inference's to
/// decide ([`Table::close_effects`]). A tail the reader *named* is out of R23's
/// reach by construction — `..'e` lowers to a rigid, which is no solver variable
/// and is never counted.
fn effect_row(table: &mut Table, tails: &mut Tails, effects: &ir::EffectRow) -> Row {
    let mut labels = IndexMap::new();
    for (name, label) in &effects.effects {
        let lowered = match label {
            ir::EffectLabel::Written { when, .. } => RowField {
                presence: presence(table, tails, when),
                ty: Rc::new(Ty::unit()),
            },
            // The struct's absent field again: an effect that is definitely
            // not performed carries nothing worth constraining.
            ir::EffectLabel::Absent { .. } => RowField {
                presence: Presence::Absent,
                ty: Rc::new(Ty::default()),
            },
        };
        labels.insert(name.row_key(), lowered);
    }
    row(table, tails, labels, &effects.tail)
}

/// What a declared type stands for: [`Ty::Named`] replaced by the body it was
/// declared with, holding the arguments it was applied to, and again for as
/// long as that is another name.
///
/// Substituting the arguments is opening the declaration's [`Scheme`], which is
/// the same `open` that instantiates a definition's — a declaration's
/// parameters and a scheme's quantified variables are both [`Ty::Bound`], and
/// both are handed their values from outside. One taking no arguments opens to
/// its body unchanged, which is what this did before there were any.
///
/// The one place a name is looked through, and always by one caller that needs
/// a shape rather than a name — never as a normalization pass. A type that
/// names itself unfolds forever if asked to, so nothing here asks: what comes
/// back is one shape deep, and the names inside it are still names.
///
/// Source recursion checking proves the ordinary walk finite. Imported
/// interfaces are recovery input, though, and can contain forwarding and
/// growing cycles this compiler never checked. Forwarding classification walks
/// the declaration graph without opening its arguments; structure-adding paths
/// are then bounded by the declarations active on that path, and a cycle that
/// cumulatively adds fields is rejected before those prefixes are materialized.
/// Direct type and empty-row forwarding do not count because they add no
/// semantic structure, preserving valid nesting independently of how many
/// aliases happen to exist. Independent sibling substitutions have independent
/// active paths.
///
/// A name with no declaration behind it is [`Ty::Undecided`]: the only way to
/// write one is to repeat a type's name, which was already reported.
pub fn unfold(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
    Unfold {
        aliases,
        growing: HashSet::new(),
        forwarding: Forwarding::default(),
    }
    .ty(ty)
}

/// One unfolding path. Row-bound forwarding can ask for the shape of an
/// argument while its outer alias is still opening, so active growing
/// declarations are shared through that nested request rather than reset at
/// the row.
struct Unfold<'a> {
    aliases: &'a IndexMap<Symbol, Scheme>,
    /// Non-forwarding declarations on the active path. Re-entering one is
    /// malformed structural growth; unrelated declarations do not buy fuel.
    /// Nested row opening is part of the path, while a completed sibling is not.
    growing: HashSet<Symbol>,
    forwarding: Forwarding,
}

impl Unfold<'_> {
    fn ty(&mut self, ty: &Rc<Ty>) -> Rc<Ty> {
        let mut ty = ty.clone();
        let mut entered = Vec::new();
        let result = loop {
            let Ty::Named { symbol, args, .. } = &*ty else {
                break ty;
            };
            let Some(scheme) = self.aliases.get(symbol) else {
                break Rc::new(Ty::Undecided);
            };
            let body = scheme.body().clone();
            let grows = self.forwarding.projection(self.aliases, &body).is_none();
            // A head-alias cycle that only grows the forwarded argument can be
            // rejected before materializing any cumulative row at all. The
            // forwarding classifier has already walked that declaration graph
            // and marked every alias on the cycle-reaching path.
            if grows && adds_argument_structure(&body) && self.forwarding.cycles.contains(symbol) {
                break Rc::new(Ty::Undecided);
            }
            // A forwarding declaration selects an existing argument and thus
            // strictly consumes the finite application tree; it needs no
            // cumulative identity for every argument it passes through.
            // Structure-adding paths are bounded by declaration identity
            // instead. This keeps an N-alias malformed growth cycle at O(N)
            // retained graph memory rather than storing N flattened maps of
            // sizes 1 through N.
            if grows && !self.growing.insert(*symbol) {
                break Rc::new(Ty::Undecided);
            }
            entered.push(grows.then_some(*symbol));

            let fresh: Vec<_> = args.iter().map(|a| Assigned::Ty(a.clone())).collect();
            ty = body.open_alias(&fresh, self);
        };
        for symbol in entered.into_iter().flatten() {
            self.growing.remove(&symbol);
        }
        result
    }
}

/// Whether a head alias adds a constructor around one of the arguments it
/// passes on. For a cycle of distinct aliases this is the cumulative-growth
/// case that can be rejected before materializing all N row prefixes. A plain
/// recursive argument still opens one layer: row forwarding uses that layer to
/// preserve the outer `Rest::More` and recover only its recursive tail.
fn adds_argument_structure(ty: &Ty) -> bool {
    let Ty::Named { args, .. } = ty else {
        return false;
    };
    args.iter().any(|arg| match &**arg {
        Ty::Struct(row) | Ty::Sum(row) => {
            let mut row = row;
            loop {
                if !row.labels.is_empty() {
                    break true;
                }
                match &row.rest {
                    Rest::More(more) => row = more,
                    _ => break false,
                }
            }
        }
        _ => false,
    })
}

/// Which parameter an expression forwards unchanged, if it is only a chain of
/// applications of other forwarding declarations. The cache is allocation
/// keyed and the evaluator is an explicit stack: imported interfaces can put
/// tens of thousands of aliases between a body and its parameter.
///
/// A named node is not growth merely because it is named. `F a = Id a` adds no
/// constructor at all when `Id a = a`, and counting the spelling as growth lets
/// `F (F Nat)` consume the malformed-growth guard before it can reach `Nat`.
/// Conversely, a cycle with no eventual parameter and an application that
/// wraps the selected argument are not projections, so malformed growing
/// imports retain the termination guard.
#[derive(Default)]
struct Forwarding {
    expressions: HashMap<usize, Option<u32>>,
    aliases: HashMap<Symbol, Option<u32>>,
    cycles: HashSet<Symbol>,
}

impl Forwarding {
    fn projection(&mut self, aliases: &IndexMap<Symbol, Scheme>, root: &Rc<Ty>) -> Option<u32> {
        enum Work {
            Expression(Rc<Ty>),
            Alias(Symbol),
            AfterAlias(Rc<[Rc<Ty>]>),
            FinishExpression(usize),
            FinishAlias(Symbol),
            Value(Option<u32>),
        }

        let mut active = HashSet::new();
        let mut values = Vec::new();
        let mut work = vec![Work::Expression(root.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Value(value) => values.push(value),
                Work::Expression(ty) => {
                    let address = Rc::as_ptr(&ty) as usize;
                    if let Some(value) = self.expressions.get(&address) {
                        values.push(*value);
                    } else {
                        work.push(Work::FinishExpression(address));
                        match &*ty {
                            Ty::Bound(index) => work.push(Work::Value(Some(*index))),
                            Ty::Named { symbol, args, .. } => {
                                work.push(Work::AfterAlias(args.clone()));
                                work.push(Work::Alias(*symbol));
                            }
                            Ty::Struct(row) | Ty::Sum(row) => {
                                let mut row = row;
                                while row.labels.is_empty() {
                                    match &row.rest {
                                        Rest::More(more) => row = more,
                                        Rest::Bound(index) => {
                                            work.push(Work::Value(Some(*index)));
                                            break;
                                        }
                                        _ => {
                                            work.push(Work::Value(None));
                                            break;
                                        }
                                    }
                                }
                                if !row.labels.is_empty() {
                                    work.push(Work::Value(None));
                                }
                            }
                            _ => work.push(Work::Value(None)),
                        }
                    }
                }
                Work::Alias(symbol) => {
                    if let Some(value) = self.aliases.get(&symbol) {
                        values.push(*value);
                    } else if !active.insert(symbol) {
                        self.cycles.extend(active.iter().copied());
                        self.cycles.insert(symbol);
                        values.push(None);
                    } else {
                        work.push(Work::FinishAlias(symbol));
                        match aliases.get(&symbol) {
                            Some(scheme) => work.push(Work::Expression(scheme.body().clone())),
                            None => work.push(Work::Value(None)),
                        }
                    }
                }
                Work::AfterAlias(args) => {
                    let selected = values.pop().expect("forwarding alias result");
                    match selected.and_then(|index| args.get(index as usize)) {
                        Some(arg) => work.push(Work::Expression(arg.clone())),
                        None => work.push(Work::Value(None)),
                    }
                }
                Work::FinishExpression(address) => {
                    let value = values.pop().expect("forwarding expression result");
                    self.expressions.insert(address, value);
                    values.push(value);
                }
                Work::FinishAlias(symbol) => {
                    let value = values.pop().expect("forwarding declaration result");
                    active.remove(&symbol);
                    self.aliases.insert(symbol, value);
                    values.push(value);
                }
            }
        }
        values.pop().expect("forwarding root result")
    }
}

impl Ty {
    /// Open a declared alias body. A bound row rest is allowed to receive a
    /// named row alias, so look through that argument before converting it to
    /// the row spliced at [`Rest::More`]. Ordinary bound type positions keep
    /// the written named type intact.
    fn open_alias(&self, fresh: &[Assigned], unfold: &mut Unfold<'_>) -> Rc<Ty> {
        substitute_type(
            self,
            fresh,
            |index| {
                fresh
                    .get(index as usize)
                    .map(Assigned::as_ty)
                    .unwrap_or_else(|| Rc::new(Ty::Undecided))
            },
            |index| {
                fresh.get(index as usize).map(Assigned::as_ty).map_or_else(
                    || Row::of(Rest::Undecided),
                    |ty| match &*unfold.ty(&ty) {
                        Ty::Struct(row) | Ty::Sum(row) => row.clone(),
                        Ty::Var(var) => Row::of(Rest::Var(*var)),
                        _ => Row::of(Rest::Undecided),
                    },
                )
            },
        )
    }

    /// Replace each bound variable with what it was opened to.
    ///
    /// Two callers and one rule. Instantiating a definition's scheme hands each
    /// position a fresh variable; unfolding a declaration hands each position
    /// the argument written at the use site. Which sort a position needs is
    /// decided here rather than by the caller, because the position is what
    /// knows: see [`Assigned::as_row`]. The worklist keeps imported schemes of
    /// arbitrary arrow, named-argument, row, and payload depth stack safe.
    pub fn open(&self, fresh: &[Assigned]) -> Rc<Ty> {
        substitute_type(
            self,
            fresh,
            |index| {
                fresh
                    .get(index as usize)
                    .map(Assigned::as_ty)
                    .unwrap_or_else(|| Rc::new(Ty::Undecided))
            },
            |index| {
                fresh
                    .get(index as usize)
                    .map(Assigned::as_row)
                    .unwrap_or_else(|| Row::of(Rest::Undecided))
            },
        )
    }
}

/// Iterative substitution shared by scheme instantiation and alias exposure.
fn substitute_type(
    root: &Ty,
    fresh: &[Assigned],
    mut bound_ty: impl FnMut(u32) -> Rc<Ty>,
    mut bound_row: impl FnMut(u32) -> Row,
) -> Rc<Ty> {
    enum Work<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
        Arrow,
        Package,
        Struct,
        Sum,
        Named {
            symbol: Symbol,
            name: Rc<str>,
            args: usize,
        },
        BuiltRow(&'a Row),
    }

    let mut work = vec![Work::Ty(root)];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match ty {
                Ty::Bound(index) => types.push(bound_ty(*index)),
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
                Ty::Struct(row) => {
                    work.push(Work::Struct);
                    work.push(Work::Row(row));
                }
                Ty::Sum(row) => {
                    work.push(Work::Sum);
                    work.push(Work::Row(row));
                }
                Ty::Named { symbol, name, args } => {
                    work.push(Work::Named {
                        symbol: *symbol,
                        name: name.clone(),
                        args: args.len(),
                    });
                    work.extend(args.iter().rev().map(|arg| Work::Ty(arg)));
                }
                other => types.push(Rc::new(other.clone())),
            },
            Work::Row(row) => {
                work.push(Work::BuiltRow(row));
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row(more));
                }
                work.extend(row.labels.values().rev().filter_map(|field| {
                    let presence = match &field.presence {
                        Presence::Bound(index) => fresh
                            .get(*index as usize)
                            .map(Assigned::presence)
                            .unwrap_or(Presence::Undecided),
                        Presence::Recovered(_) => Presence::Undecided,
                        presence => presence.clone(),
                    };
                    (!matches!(presence, Presence::Absent)).then_some(Work::Ty(&field.ty))
                }));
            }
            Work::Arrow => {
                let effects = rows.pop().expect("row substitution postorder");
                let to = types.pop().expect("result substitution postorder");
                let from = types.pop().expect("argument substitution postorder");
                types.push(Rc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("package substitution postorder");
                types.push(Rc::new(Ty::Package(body)));
            }
            Work::Struct => {
                let row = rows.pop().expect("struct substitution postorder");
                types.push(Rc::new(Ty::Struct(row)));
            }
            Work::Sum => {
                let row = rows.pop().expect("sum substitution postorder");
                types.push(Rc::new(Ty::Sum(row)));
            }
            Work::Named { symbol, name, args } => {
                let mut opened = Vec::with_capacity(args);
                for _ in 0..args {
                    opened.push(types.pop().expect("named substitution postorder"));
                }
                opened.reverse();
                types.push(Rc::new(Ty::Named {
                    symbol,
                    name,
                    args: opened.into(),
                }));
            }
            Work::BuiltRow(row) => {
                let rest = match &row.rest {
                    Rest::Bound(index) => Rest::More(Rc::new(bound_row(*index))),
                    Rest::More(_) => Rest::More(Rc::new(
                        rows.pop().expect("nested row substitution postorder"),
                    )),
                    rest => rest.clone(),
                };
                let mut labels = Vec::with_capacity(row.labels.len());
                for (name, field) in row.labels.iter().rev() {
                    let presence = match &field.presence {
                        Presence::Bound(index) => fresh
                            .get(*index as usize)
                            .map(Assigned::presence)
                            .unwrap_or(Presence::Undecided),
                        Presence::Recovered(_) => Presence::Undecided,
                        presence => presence.clone(),
                    };
                    let ty = match presence {
                        Presence::Absent => Rc::new(Ty::Undecided),
                        _ => types.pop().expect("field substitution postorder"),
                    };
                    labels.push((name.clone(), RowField { presence, ty }));
                }
                labels.reverse();
                rows.push(Row {
                    labels: labels.into_iter().collect(),
                    rest,
                });
            }
        }
    }
    types.pop().expect("a type substitution result")
}

/// Whether two field maps carry exactly the same names, in whatever order.
///
/// Structs are records: `{ x: Nat, y: Nat }` written either way round is one
/// type, so what decides whether two of them line up is the set of names and
/// nothing else. Generic because its one caller compares a struct literal's
/// fields against a written type's, which map their names to different
/// things.
///
/// This is a gate, not the rule. Two structs that name different fields can
/// still be one type — that is what the constructor beside them decides, in
/// [`Solve::labels`] — so failing here only means checking cannot push the
/// expected fields in
/// one by one and the literal is inferred and equated instead.
fn same_field_set<A, B>(want: &IndexMap<String, A>, have: &IndexMap<String, B>) -> bool {
    want.len() == have.len() && want.keys().all(|name| have.contains_key(name))
}

#[cfg(test)]
mod existential_regressions {
    use super::*;

    #[test]
    fn coherent_package_guarantees_are_registered_idempotently() {
        let mut table = Table::default();
        let Presence::Var(hidden) = table.fresh_presence() else {
            unreachable!()
        };
        table.abstract_existentials.insert(hidden);
        let body = Rc::new(Ty::Struct(Row {
            labels: [(
                "hidden".into(),
                RowField {
                    presence: Presence::Var(hidden),
                    ty: Rc::new(Ty::Nat),
                },
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let package = Rc::new(Ty::Package(body));
        let clause = Formula::owned(0, Formula::var(hidden));

        for _ in 0..5_000 {
            assert!(
                table
                    .register_package_guarantees(&package, clause.clone())
                    .is_true()
            );
        }

        let guarantee = table.package_guarantees.values().next().unwrap();
        assert_eq!(guarantee.clauses.len(), 1);

        // Idempotence must not collapse a genuinely different proposition for
        // the same package allocation.
        table.register_package_guarantees(&package, Formula::owned(0, Formula::var(hidden).not()));
        let guarantee = table.package_guarantees.values().next().unwrap();
        assert_eq!(guarantee.clauses.len(), 2);
    }

    #[test]
    fn failed_nominal_congruence_restores_abstract_presence_aliasing() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("rollback-test", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let wrapper = mint.global(None, Namespace::Types, "Wrapper").unwrap();
        let definition = mint.global(None, Namespace::Terms, "definition").unwrap();

        let mut table = Table::default();
        let Presence::Var(hidden) = table.fresh_presence() else {
            unreachable!()
        };
        let Presence::Var(alias) = table.fresh_presence() else {
            unreachable!()
        };
        // This is the state after a package has been opened: its witness is a
        // sealed identity, while the ordinary variable beside it is flexible.
        table.existential_witnesses.insert(hidden);
        table.abstract_existentials.insert(hidden);

        let presence_arg = |presence| {
            Rc::new(Ty::Struct(Row {
                labels: [(
                    "hidden".into(),
                    RowField {
                        presence,
                        ty: Rc::new(Ty::Nat),
                    },
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }))
        };
        let named = |args: Vec<Rc<Ty>>| {
            Rc::new(Ty::Named {
                symbol: wrapper,
                name: Rc::from("Wrapper"),
                args: args.into(),
            })
        };
        // The first argument speculatively aliases `alias` to the abstract
        // identity. The second fails, forcing congruence to discard that work.
        let expected = named(vec![presence_arg(Presence::Var(hidden)), Rc::new(Ty::Nat)]);
        let actual = named(vec![
            presence_arg(Presence::Var(alias)),
            Rc::new(Ty::String),
        ]);
        let aliases = IndexMap::new();
        let nominal = [wrapper].into_iter().collect();
        let constraint = table.constraint_id();
        let reason = table.constraint_reason(constraint);
        let mut errors = Vec::new();
        let mut steps = Vec::new();
        let mut locals = IndexMap::new();
        let mut refinements = Vec::new();
        {
            let mut solve = Solve {
                table: &mut table,
                errors: &mut errors,
                steps: &mut steps,
                aliases: &aliases,
                nominal: &nominal,
                definition,
                depth: 0,
                constraint: None,
                constraint_reason: None,
                assumed: Vec::new(),
                schemes: HashMap::new(),
                locals: &mut locals,
                guard: None,
                active_refinement: None,
                guard_reasons: Vec::new(),
                refinements: &mut refinements,
                generated_end: 0,
            };
            solve.run(&[Constraint {
                id: constraint,
                reason,
                span: Span::default(),
                origin: ConstraintOrigin::ContextualCheck,
                subjects: ConstraintSubjects::pair(Subject::Context, Subject::Term),
                kind: ConstraintKind::Equal { expected, actual },
            }]);
        }

        assert!(table.abstract_existentials.contains(&hidden));
        assert!(!table.abstract_existentials.contains(&alias));
        assert!(table.existential_witnesses.contains(&hidden));
        assert!(!table.existential_witnesses.contains(&alias));

        assert_eq!(errors.len(), 1);
        let ErrorCause::Step(cause) = errors[0].cause else {
            panic!("failed congruence must retain its direct solve cause")
        };
        let failed = steps.iter().find(|step| step.id == cause).unwrap();
        assert_eq!(failed.error, Some(errors[0].id));
        assert_eq!(failed.constraint, Some(constraint));
        // The discarded congruence, binding, and inner failure consumed their
        // identities. The surviving failure must not reuse any of them.
        assert!(failed.id.get() > 0);
        assert!(errors[0].id.get() > 0);
    }

    #[test]
    fn mismatch_leaf_walks_nested_rows_effect_payloads_and_alias_arguments() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("leaf-test", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let wrapper = mint.global(None, Namespace::Types, "Wrapper").unwrap();
        let unit = Rc::new(Ty::unit());
        let effect = Row {
            labels: [("effect".into(), RowField::present(Rc::new(Ty::Bound(0))))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        };
        let arrow = Rc::new(Ty::Arrow(unit.clone(), unit, effect));
        let sum = Rc::new(Ty::Sum(Row {
            labels: [("Case".into(), RowField::present(arrow))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let body = Rc::new(Ty::Struct(Row {
            labels: [("field".into(), RowField::present(sum))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [(wrapper, Scheme::new(1, body))].into_iter().collect();
        let named = |argument| {
            Rc::new(Ty::Named {
                symbol: wrapper,
                name: "Wrapper".into(),
                args: Rc::from([Rc::new(argument)]),
            })
        };
        assert_eq!(
            smallest_incompatible(&aliases, &named(Ty::Nat), &named(Ty::Boolean)),
            (TypeDescription::NaturalNumber, TypeDescription::Boolean)
        );

        let missing_left = mint.global(None, Namespace::Types, "MissingLeft").unwrap();
        let missing_right = mint.global(None, Namespace::Types, "MissingRight").unwrap();
        let missing = |symbol, name: &'static str| {
            Rc::new(Ty::Named {
                symbol,
                name: name.into(),
                args: Rc::from([]),
            })
        };
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &missing(missing_left, "MissingLeft"),
                &missing(missing_right, "MissingRight"),
            ),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType),
            "missing semantic definitions must keep the declared-type fallback"
        );
    }

    #[test]
    fn smallest_mismatch_leaf_is_deep_stack_safe() {
        std::thread::Builder::new()
            .name("deep-mismatch-leaf".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let mut left = Rc::new(Ty::Nat);
                let mut right = Rc::new(Ty::Boolean);
                for _ in 0..30_000 {
                    left = Rc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(left))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                    right = Rc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(right))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                }
                assert_eq!(
                    smallest_incompatible(&IndexMap::new(), &left, &right),
                    (TypeDescription::NaturalNumber, TypeDescription::Boolean)
                );
                // Deep Rc destruction is unrelated to the iterative reader.
                std::mem::forget(left);
                std::mem::forget(right);
            })
            .unwrap()
            .join()
            .expect("smallest mismatch leaf stays on its explicit stack");
    }

    #[test]
    fn publication_shift_captures_deep_composed_rows_without_recursing() {
        std::thread::Builder::new()
            .name("deep-publication-shift".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let mut row = Row::of(Rest::Bound(2));
                for at in 0..20_000 {
                    let mut labels = IndexMap::new();
                    labels.insert(
                        format!("f{at}"),
                        RowField {
                            presence: Presence::Bound(0),
                            ty: Rc::new(Ty::Bound(3)),
                        },
                    );
                    row = Row {
                        labels,
                        rest: Rest::More(Rc::new(row)),
                    };
                }
                let shifted = shift(&Rc::new(Ty::Struct(row)), 4);
                let Ty::Struct(row) = &*shifted else {
                    unreachable!()
                };
                let mut row = row.clone();
                let mut depth = 0;
                loop {
                    if row.labels.is_empty() {
                        assert!(matches!(row.rest, Rest::Bound(6)));
                        break;
                    }
                    let field = row.labels.values().next().unwrap();
                    assert_eq!(field.presence, Presence::Bound(0));
                    assert!(matches!(&*field.ty, Ty::Bound(7)));
                    depth += 1;
                    match &row.rest {
                        Rest::More(more) => row = (**more).clone(),
                        _ => panic!("unexpected shifted tail"),
                    }
                }
                assert_eq!(depth, 20_000);
            })
            .unwrap()
            .join()
            .expect("publication shift stays on its explicit stack");
    }
}

#[cfg(test)]
mod identity_tests {
    use super::{ReasonOrigin, Subject, Table, VarSort};

    #[test]
    fn rollback_retires_allocated_identities_instead_of_reusing_them() {
        let mut table = Table::default();
        let known = table.snapshot();
        let abandoned_step = table.step_id();
        let abandoned_batch = table.batch_id();
        let abandoned_var = table.mint(VarSort::Type, Subject::Term);
        let abandoned_reason = table.var_meta[abandoned_var as usize].minted_by;

        table.restore(known);

        assert_ne!(table.step_id(), abandoned_step);
        assert_ne!(table.batch_id(), abandoned_batch);
        assert!(table.var_meta.is_empty());
        assert!(table.reasons.iter().any(|reason| {
            reason.id == abandoned_reason
                && !reason.reachable
                && matches!(reason.origin, ReasonOrigin::Variable { .. })
        }));
        let surviving = table.mint(VarSort::Type, Subject::Term);
        assert_eq!(surviving, abandoned_var);
        assert_ne!(
            table.var_meta[surviving as usize].minted_by,
            abandoned_reason
        );
    }
}
