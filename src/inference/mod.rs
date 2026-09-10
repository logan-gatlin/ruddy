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
//! checks, and — through [`Semantics::store`] — in the patterns phase. A
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

mod flow;
mod queries;
pub use queries::Session;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    ops::Range,
    sync::{Arc, Weak},
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    ir::{
        self, Annotation, Clause, ClauseKind, Decl, Program, Tail, Term, TermKind, Type, TypeKind,
    },
    symbol::{Mint, Symbol},
    tracking::{Anchor, Order},
    types::{
        Assigned, Atom, EffectId, Formula, ParamKind, Presence, Rest, Row, RowField, Scheme, Sense,
        Shape, Ty, TyVar, same_finite_syntax, same_finite_syntax_metered,
    },
};
use constrain::Constrain;
use solve::Solve;

/// Whether inference keeps its complete solver replay data.
///
/// Structured errors and their causal explanations are always kept, so a
/// reporter reads the same rich account in either mode. What the setting
/// decides is whether the constraints, solver steps, reasons, variables and
/// refinements the explanations were built from stay published afterwards —
/// which only a debugger stepping the solve wants, and which ordinary
/// compilation would otherwise carry for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trace {
    /// Keep errors and their explanations only.
    #[default]
    Off,
    /// Also keep every constraint, step, reason, variable and refinement.
    Complete,
}

/// What inference publishes: the semantic facts every later phase reads, and
/// the diagnostic account of how it reached them.
///
/// The two are published separately because they are consumed separately. A
/// lowering reads schemes and never a solver step; a debugger reads steps and
/// must not be able to rewrite a scheme on the way. Neither half is mutable
/// from outside inference, so a program that came out of one `infer` call
/// cannot be paired with facts that came out of another.
#[derive(Debug, Clone)]
pub struct Output {
    semantics: Semantics,
    diagnostics: Diagnostics,
}

/// The two sides of every declared operation, keyed by the effect that declares
/// it and the operation's own name. See [`Semantics::operations`].
pub type Operations = IndexMap<(Symbol, ir::OperationSelector), (Arc<Ty>, Arc<Ty>)>;

/// The accepted semantic facts of one inference run, read-only.
///
/// Everything a later phase needs and nothing about how it was found: the
/// meaning of each declaration, the scheme of each definition, and the presence
/// store the pattern checks ask. Numbering, ordering, zonking and publication
/// all happened before this was built, and none of them can be redone through
/// it.
#[derive(Debug, Clone)]
pub struct Semantics {
    reification: crate::reification::Analysis,
    pub(crate) aliases: IndexMap<Symbol, Scheme>,
    pub(crate) operations: Operations,
    /// Each effect alias's row over its own parameters, unexpanded, for the
    /// artifact to publish: an open alias is expanded at each use with that
    /// use's arguments, so what is published is what it was declared as.
    pub(crate) effect_aliases: IndexMap<Symbol, EffectAliasRow>,
    pub(crate) externs: IndexMap<Symbol, Scheme>,
    pub(crate) reviewed_externs: IndexMap<Symbol, ReviewedExtern>,
    pub(crate) schemes: IndexMap<Symbol, Scheme>,
    pub(crate) locals: IndexMap<Symbol, Scheme>,
    pub(crate) store: Store,
    pub(crate) promises: IndexMap<Symbol, Formula>,
    /// Every definition, typed: the program's declarations with what the
    /// solve decided written into their terms. Inference reads the program it
    /// is handed and writes here instead, so the program stays what lowering
    /// made it.
    pub(crate) typed: IndexMap<Symbol, Decl<Term>>,
}

/// An effect alias's row as declared, semantically: the effects it applies,
/// each to arguments over the alias's parameters ([`Ty::Bound`]), and the
/// parameter position it ends in.
#[derive(Debug, Clone)]
pub struct EffectAliasRow {
    /// The effects the alias applies, each with its arguments in order.
    pub cases: Vec<(Symbol, Vec<Arc<Ty>>)>,
    /// The parameter position the alias ends in, if it ends in one.
    pub tail: Option<u32>,
}

/// Complete target-neutral facts inference accepted about an extern
/// declaration.  This is the hand-off to extern planning: later phases do not
/// reinterpret a raw IR declaration to rebuild facts inference already
/// reviewed.
#[derive(Debug, Clone)]
pub(crate) struct ReviewedExtern {
    pub(crate) scheme: Scheme,
    pub(crate) abi: ir::ExternType,
    pub(crate) target: String,
    pub(crate) target_span: Anchor,
    pub(crate) declaration_span: Anchor,
}

/// The diagnostic account of one inference run: its errors, and — when a
/// complete [`Trace`] was asked for — everything the solver did to find them.
#[derive(Debug, Clone)]
pub(crate) struct Diagnostics {
    pub(crate) trace: Trace,
    pub(crate) errors: Vec<Error>,
    pub(crate) constraints: IndexMap<Symbol, Vec<Constraint>>,
    pub(crate) steps: Vec<Step>,
    pub(crate) refinements: Vec<Refinement>,
    /// The variables each table minted, by the table's scope: a variable's
    /// number means nothing outside the table that minted it.
    pub(crate) variables: IndexMap<Symbol, Vec<VarMeta>>,
    pub(crate) reasons: Vec<Reason>,
    pub(crate) recovery_facts: Vec<crate::artifact::RecoveryFact>,
}

/// The supported read-only view of an inference run's diagnostics.
///
/// Every accessor returns what was published, and nothing here can change it.
/// The replay data — constraints, steps, reasons, variables and refinements —
/// is empty unless the run was asked for a complete [`Trace`]; the errors and
/// their explanations are there in either mode.
#[derive(Clone, Copy)]
pub struct DiagnosticView<'a> {
    inner: &'a Diagnostics,
}

impl Output {
    /// Publish inferred representation requirements before the accepted artifact
    /// and its callers fix their evidence convention.
    pub(crate) fn reify(&mut self, reification: &crate::reification::Analysis) {
        let aliases = self.semantics.aliases.clone();
        let requirements = reification.callables.graph.solve();
        let semantics = &mut self.semantics;
        for schemes in [
            &mut semantics.schemes,
            &mut semantics.externs,
            &mut semantics.locals,
        ] {
            for (symbol, scheme) in schemes {
                if let Some(binding) = reification.callables.bindings.get(symbol) {
                    let substitution =
                        crate::reification::instantiate(&binding.ty, scheme.body(), &aliases);
                    let parameters = substitution
                        .iter()
                        .filter_map(|(index, ty)| {
                            crate::reification::parameter_index(ty).map(|p| (*index, p))
                        })
                        .collect();
                    let callable = crate::reification::interface::Interface::export(
                        &reification.callables.graph,
                        binding.value,
                        &parameters,
                        &requirements,
                    );
                    *scheme = scheme.clone().with_callable(Some(callable));
                }
                if let Some(binding) = reification.bindings.get(symbol) {
                    let substitution =
                        crate::reification::instantiate(&binding.ty, scheme.body(), &aliases);
                    let parameters = binding
                        .parameters
                        .iter()
                        .filter_map(|parameter| {
                            substitution
                                .get(parameter)
                                .and_then(|ty| crate::reification::parameter_index(ty))
                        })
                        .collect();
                    *scheme = scheme.clone().with_representations(parameters);
                }
            }
        }
        for (symbol, reviewed) in &mut semantics.reviewed_externs {
            reviewed.scheme = semantics.externs[symbol].clone();
        }
    }

    /// The accepted semantic facts.
    pub fn semantics(&self) -> &Semantics {
        &self.semantics
    }

    /// The diagnostic account.
    pub fn diagnostics(&self) -> DiagnosticView<'_> {
        DiagnosticView {
            inner: &self.diagnostics,
        }
    }

    /// Every error, in source order. The same slice
    /// [`DiagnosticView::errors`] returns, here because it is the one
    /// diagnostic every caller reads.
    pub fn errors(&self) -> &[Error] {
        &self.diagnostics.errors
    }

    /// Publish non-fatal repairs made while admitting dependency artifacts.
    /// Core compilation is the only caller: source inference does not invent
    /// artifact recovery facts, but its diagnostic view is their supported
    /// publication seam.
    pub(crate) fn publish_recovery_facts(
        &mut self,
        facts: impl IntoIterator<Item = crate::artifact::RecoveryFact>,
    ) {
        self.diagnostics.recovery_facts.extend(facts);
    }

    /// Crate-private mutable access, for tests that deliberately corrupt an
    /// otherwise coherent publication to reach a defensive branch.
    #[cfg(test)]
    pub(crate) fn semantics_mut(&mut self) -> &mut Semantics {
        &mut self.semantics
    }

    /// Crate-private mutable access, for tests that synthesize diagnostic
    /// records beyond what any real solve would publish.
    #[cfg(test)]
    pub(crate) fn diagnostics_mut(&mut self) -> &mut Diagnostics {
        &mut self.diagnostics
    }
}

impl Semantics {
    pub fn reification(&self) -> &crate::reification::Analysis {
        &self.reification
    }
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
    pub fn aliases(&self) -> &IndexMap<Symbol, Scheme> {
        &self.aliases
    }

    /// The two sides of every declared operation, keyed by the effect that
    /// declares it and the operation's own name, in declaration order.
    ///
    /// A plain closed arrow, lowered once before any body is walked, because a
    /// signature mentions no variable. Published for the reason
    /// [`Semantics::aliases`] is: a later phase reads it and has no table to
    /// lower a written type with. [`lir`](crate::lir) is that phase — a handler
    /// arm's binder is the operation's argument, and how a value of it is held
    /// is nowhere else to be found, since the binder has no term of its own to
    /// carry a solved type.
    pub fn operations(&self) -> &Operations {
        &self.operations
    }

    /// Each effect alias's row over its own parameters, unexpanded. See
    /// [`EffectAliasRow`].
    pub fn effect_aliases(&self) -> &IndexMap<Symbol, EffectAliasRow> {
        &self.effect_aliases
    }

    /// The scheme each target-provided top-level value declared. Externs have
    /// no body and are therefore intentionally separate from
    /// [`Semantics::schemes`], whose entries each correspond to a term
    /// initializer.
    pub fn externs(&self) -> &IndexMap<Symbol, Scheme> {
        &self.externs
    }

    /// The complete reviewed target-neutral facts for each extern.
    ///
    /// This is the private hand-off to post-inference planning. Public callers
    /// consume the resulting [`ExternPlan`](crate::externs::ExternPlan) from
    /// an [`AcceptedProgram`](crate::compile::AcceptedProgram), rather than
    /// reinterpreting the reviewed ABI facts themselves.
    pub(crate) fn reviewed_externs(&self) -> &IndexMap<Symbol, ReviewedExtern> {
        &self.reviewed_externs
    }

    /// The scheme each top-level term was inferred, or checked, to have.
    pub fn schemes(&self) -> &IndexMap<Symbol, Scheme> {
        &self.schemes
    }

    /// The scheme each nested `let` was inferred, in the order the lets were
    /// walked.
    ///
    /// Beside [`Semantics::schemes`] rather than in it, so that a reader of
    /// that map is still reading the definitions of the file: a local binding
    /// is not a definition, and nothing that consumes the program's exports has
    /// any business seeing one.
    ///
    /// Numbered on its own. A local's scheme may leave an enclosing binder's
    /// variables free, and those are spelled here as letters past its own
    /// quantifiers rather than as the `?3` the solver knew them by — see
    /// [`Table::published`] — so two rows of this map spelling `a` are two
    /// unrelated variables, exactly as two schemes are.
    pub fn locals(&self) -> &IndexMap<Symbol, Scheme> {
        &self.locals
    }

    /// What the program requires of its presence variables, in the order it
    /// required it. See [`Store`].
    pub fn store(&self) -> &Store {
        &self.store
    }

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
    pub fn promises(&self) -> &IndexMap<Symbol, Formula> {
        &self.promises
    }

    /// Every definition with what the solve decided written into its terms,
    /// keyed by the top-level symbol, in source order.
    pub fn typed(&self) -> &IndexMap<Symbol, Decl<Term>> {
        &self.typed
    }
}

impl Output {
    /// Write the inferred types into `program`'s declarations: what a later
    /// phase reads when it wants the program as inference left it, kept out
    /// of inference itself so that the program it reads is never the one it
    /// writes.
    pub fn apply_types(&self, program: &mut Program) {
        for (symbol, decl) in &self.semantics.typed {
            program.terms.insert(*symbol, decl.clone());
        }
    }
}

impl<'a> DiagnosticView<'a> {
    /// Which [`Trace`] the run was asked for.
    pub fn trace(self) -> Trace {
        self.inner.trace
    }

    /// Every error, in source order, each carrying its causal explanation
    /// where its family has one.
    pub fn errors(self) -> &'a [Error] {
        &self.inner.errors
    }

    /// Repairs made while accepting portable dependency data. These are facts
    /// for tooling, never source compilation errors.
    pub fn recovery_facts(self) -> &'a [crate::artifact::RecoveryFact] {
        &self.inner.recovery_facts
    }

    /// What generation asked of each definition, in the order it asked, and
    /// exactly as it was asked: these are the constraints *before* the solver
    /// ran, so a variable in one prints as the variable it was. Solving is what
    /// the schemes report. Kept so that the pass can be read rather than
    /// inferred from its result — which is what the debugger's tab shows.
    ///
    /// Empty unless the trace is complete.
    pub fn constraints(self) -> &'a IndexMap<Symbol, Vec<Constraint>> {
        &self.inner.constraints
    }

    /// Every act of the solver, over the whole program, in the order it
    /// performed them: group by group, and within a group in solve order. A
    /// group's variable table is its own, so replaying the effects of one
    /// scope's steps in this order — and only in this order — reconstructs
    /// what that group's solver knew at any point; a step's scope is its
    /// id's.
    ///
    /// Empty unless the trace is complete.
    pub fn steps(self) -> &'a [Step] {
        &self.inner.steps
    }

    /// The branch-local presence assumptions inference used, one report per
    /// arm of every qualifying match, in solve order.
    ///
    /// Empty unless the trace is complete.
    pub fn refinements(self) -> &'a [Refinement] {
        &self.inner.refinements
    }

    /// Metadata indexed by `TyVar` within each table's scope, parallel to
    /// that table's private slots. A variable's number is only meaningful
    /// beside the scope of the record that mentions it: a step's is its id's.
    ///
    /// Empty unless the trace is complete.
    pub fn variables(self) -> &'a IndexMap<Symbol, Vec<VarMeta>> {
        &self.inner.variables
    }

    /// The metadata of `var` as the table scoped to `scope` minted it.
    pub fn variable(self, scope: Symbol, var: TyVar) -> Option<&'a VarMeta> {
        self.inner.variables.get(&scope)?.get(var as usize)
    }

    /// Append-only reason arena. Speculative nodes remain retired but readable
    /// after rollback; no surviving link can be retargeted by identity reuse.
    ///
    /// Empty unless the trace is complete.
    pub fn reasons(self) -> &'a [Reason] {
        &self.inner.reasons
    }

    /// Reachable causal slice rooted at `seed`, walked iteratively so debugger
    /// queries remain safe for arbitrarily deep chains of solved aliases.
    pub fn reason_ancestors(self, seed: ReasonId) -> Vec<ReasonId> {
        let arena: HashMap<_, _> = self
            .inner
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
///
/// Each group solves in a store of its own, and what is published is those
/// stores one after another: a batch's id says which group's it is, its
/// presence variables are numbered by that group, and a flip is that group's
/// alone. A reader conjoins batches of one scope and never across two.
/// An identity minted by one inference table: the scope the table solved —
/// a group, named by its first member, or [`Symbol::GENERATED`] for the
/// declarations solved before any group — and a count within it.
///
/// Scoped rather than counted across the program so that a group's records
/// are the same values whatever was solved before it, which is what lets one
/// group's result be reused while another's is recomputed. Ordered by scope
/// and then by count, which is minting order within a scope and no order in
/// particular across scopes.
macro_rules! inference_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            scope: Symbol,
            index: u32,
        }

        impl $name {
            /// The table this was minted in: a group's first member, or
            /// [`Symbol::GENERATED`] for the declarations solved before the
            /// groups.
            pub fn scope(self) -> Symbol {
                self.scope
            }

            /// Where in its scope's minting order this came.
            pub fn index(self) -> u32 {
                self.index
            }

            /// A fabricated identity, for crate-private tests that build
            /// diagnostic records no real solve would publish.
            #[allow(dead_code)]
            pub(crate) const fn synthetic(value: u64) -> Self {
                Self {
                    scope: Symbol::GENERATED,
                    index: value as u32,
                }
            }

            #[allow(dead_code)]
            fn pending() -> Self {
                Self {
                    scope: Symbol::GENERATED,
                    index: u32::MAX,
                }
            }
        }

        /// The scope's fingerprint and the count, as one token the debugger
        /// can key on: unique across the program, and the same in every run.
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:x}.{}", self.scope.bits(), self.index)
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
    Region,
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
    /// A written annotation reopened as an authoritative scheme contract.
    Contract(ConstraintId),
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
    pub at: Anchor,
    pub origin: Origin,
    /// Root of this batch's causal explanation.
    pub reason: ReasonId,
    /// What it requires, over the presence variables that existed when it was
    /// emitted. Kept as it was emitted, not as the solve later resolved it —
    /// the debugger shows the pass being read rather than its result, exactly
    /// as [`DiagnosticView::constraints`] does.
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
pub fn structural_presence_paths(ty: &Arc<Ty>) -> Vec<(PresencePath, Presence)> {
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
    pub at: Anchor,
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
    pub match_at: Anchor,
    pub arm_at: Anchor,
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
    pub at: Anchor,
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
    pub at: Anchor,
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
    /// Exact structural route selected by the occurs walk, captured before
    /// recovery rewrites the graph. `None` for every non-recursive step.
    pub recursive: Option<RecursiveCycleShape>,
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
        expected: Arc<Ty>,
        actual: Arc<Ty>,
    },
    Row {
        expected: Arc<Row>,
        actual: Arc<Row>,
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
    /// Two homogeneous arrays: compare their element types.
    Array,
    Mut,
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
    Overlap {
        shape: Shape,
    },
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
    Presence {
        shape: Shape,
    },
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
    pub at: Anchor,
    /// The source operation that required this constraint.
    pub origin: ConstraintOrigin,
    /// Source-facing names for the ordered operands carried by `kind`.
    pub subjects: ConstraintSubjects,
    pub kind: ConstraintKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticPivot {
    FunctionInput(Symbol),
    BranchResult(Anchor),
}

/// Why generation emitted a constraint. This describes generation rather than
/// the solver rule that eventually handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstraintOrigin {
    Binding,
    ContextualCheck,
    ApplicationCallee,
    ApplicationArgument,
    ApplicationEffects,
    Raise,
    Projection,
    StructSpread,
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
            Self::StructSpread => "struct-spread",
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
    pub semantic_pivot: Option<SemanticPivot>,
    pub primary: Subject,
    pub primary_span: Option<Anchor>,
    pub secondary: Option<Subject>,
    pub secondary_span: Option<Anchor>,
}

impl PartialEq for ConstraintSubjects {
    fn eq(&self, other: &Self) -> bool {
        self.primary == other.primary && self.secondary == other.secondary
    }
}

impl ConstraintSubjects {
    pub const fn one(primary: Subject) -> Self {
        Self {
            semantic_pivot: None,
            primary,
            primary_span: None,
            secondary: None,
            secondary_span: None,
        }
    }

    pub const fn pair(primary: Subject, secondary: Subject) -> Self {
        Self {
            semantic_pivot: None,
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
        primary_span: Option<Anchor>,
        secondary: Subject,
        secondary_span: Option<Anchor>,
    ) -> Self {
        Self {
            semantic_pivot: None,
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
    EffectDeclaration,
    /// The `..` of an array literal: what it spreads has to be an array of
    /// the literal's own type.
    Spread,
    /// The `..` of a struct literal: what it spreads has to be a struct, and
    /// the literal keeps every field of it that it does not name itself.
    StructSpread,
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
            Self::EffectDeclaration => "effect-declaration",
            Self::Spread => "spread",
            Self::StructSpread => "struct-spread",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConstraintKind {
    /// Publish a function's effects after checking whether its state is private.
    Isolate {
        /// Operations and callees contributing to this body's effect boundary.
        effect_origins: Vec<EffectSource>,
        input: Arc<Ty>,
        output: Arc<Ty>,
        internal: Row,
        external: Row,
        level: u32,
    },
    /// Read one field from a base. The operation stays distinct from ordinary
    /// equality so a known non-struct can be diagnosed at the base.
    Project {
        base: Arc<Ty>,
        field: String,
        result: Arc<Ty>,
        base_span: Anchor,
    },
    /// Spread one value's fields into a struct literal. Distinct from
    /// ordinary equality for the reason a projection is: a known non-struct
    /// is diagnosed at the operand, as the value that has no fields to
    /// spread, rather than as a mismatch against the row it was asked for.
    Spread {
        /// What was written after the `..`.
        operand: Arc<Ty>,
        /// What the literal asks of it: a struct that may or may not have
        /// each field the literal names — holding anything where it does,
        /// since the literal's own value replaces it — and past those, the
        /// rest the result keeps.
        demand: Arc<Ty>,
        /// The literal's own type — its named fields, certainly there, over
        /// the same rest — abandoned when the operand is no struct.
        result: Arc<Ty>,
        operand_span: Anchor,
    },
    /// Two types the program requires to be the same. `expected` is the side
    /// the context demanded — an annotation, a function's parameter, or the
    /// arrow shape a call site needs of something that is not one — and
    /// `actual` is what the term turned out to be, which is the order a
    /// mismatch is worded in.
    Equal { expected: Arc<Ty>, actual: Arc<Ty> },
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
        bound: Arc<Ty>,
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
        initializer_effects: Row,
        ambient: Row,
        inside: bool,
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
        ty: Arc<Ty>,
        /// Source-order store slot reserved during generation for the scheme
        /// requirement solving may discover.
        requirement: usize,
    },
    /// A qualifying presence-only match. Each arm owns the constraints and
    /// store requirements generated by its body; solving applies its ordered
    /// condition as a premise and constructs one structural result family.
    Match {
        scrutinee: Arc<Ty>,
        result: Arc<Ty>,
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
        /// The exact declaration selected by a directly written operation.
        /// This is diagnostic provenance, not row identity: structurally equal
        /// declarations may coalesce in `performed` while retaining distinct
        /// source origins through calls and generalized aliases.
        effect_origins: Vec<EffectSource>,
        /// Source labels of handlers which extend `ambient`.
        ambient_label_spans: IndexMap<String, Anchor>,
        /// Whether a `fn` encloses the application. What tells the two readings
        /// of a failure apart: outside every function the ambient is a
        /// definition's own, so nothing could ever have handled the effect,
        /// and inside one it is that function's row, which does not allow it.
        /// See R11.
        inside: bool,
    },
    /// An effectful callback crossing a foreign boundary must be callable with
    /// the evidence carried by that boundary.
    CallbackCoverage {
        required: Row,
        available: Row,
        boundary: CallbackBoundary,
    },
}

/// Source context retained on a foreign callback obligation.  Rows deliberately
/// contain no declaration or path information, so keeping this beside the
/// constraint is what lets a failed solve still explain the written boundary.
#[derive(Debug, Clone)]
pub struct CallbackBoundary {
    pub callback_at: Anchor,
    pub callback_path: String,
    pub callback_type: String,
    /// One exact required-presence => available-presence implication per
    /// semantic effect label. Keeping the labels separate lets declaration
    /// validation name every failed implication instead of collapsing a row to
    /// its definitely-present subset.
    pub effects: Vec<CallbackEffectCondition>,
    /// Their conjunction, retained for debugger consumers and source rendering.
    pub condition: Formula,
    /// The exact terminal relation after every `Rest::More` segment is
    /// flattened. `None` means the callback row is closed.
    pub tail: Option<CallbackTailRelation>,
    pub extern_name: String,
    pub extern_at: Anchor,
    pub capability_at: Anchor,
}

#[derive(Debug, Clone)]
pub struct CallbackEffectCondition {
    pub effect: String,
    pub condition: Formula,
}

#[derive(Debug, Clone)]
pub struct CallbackTailRelation {
    /// Source spelling of the callback remainder, including its `..` sigil.
    pub required: String,
    /// Source spelling of the extern remainder, or `None` for a closed row.
    pub available: Option<String>,
}

/// Source identity of an effect use, kept separate from structural row
/// equality so equivalent interfaces cannot overwrite one another's evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectOrigin {
    pub symbol: Symbol,
    pub interface: crate::types::EffectId,
    pub declaration_at: Anchor,
}

/// Either an exact operation declaration or a callable supplied through a
/// function parameter. Parameter sources are substituted as values flow; only
/// exact origins ever reach a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectSource {
    Origin(EffectOrigin),
    Parameter {
        symbol: Symbol,
        path: Vec<EffectPathStep>,
    },
}

/// One value-flow step from a symbolic higher-order parameter. Kept beside
/// provenance rather than in [`Ty`] so paths do not affect type identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectPathStep {
    CallResult,
    Field(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectParameter {
    symbol: Symbol,
    path: Vec<EffectPathStep>,
}

/// Shared value-flow provenance. Children are immutable DAG edges: substituting
/// one argument into several fields therefore installs the same node at every
/// occurrence instead of recursively cloning its complete replacement tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectProvenance(Arc<EffectProvenanceNode>);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EffectProvenanceNode {
    callable: Vec<EffectSource>,
    value_parameter: Option<EffectParameter>,
    argument: Option<Symbol>,
    result: Option<EffectProvenance>,
    fields: IndexMap<String, EffectProvenance>,
    /// A small, argument-independent cache used when a bounded rewrite has to
    /// cut this node off.  In particular, declarations below the cutoff are
    /// not lost merely because their structural route was too expensive.
    exact_origins: Vec<EffectOrigin>,
}

impl Default for EffectProvenance {
    fn default() -> Self {
        Self(Arc::new(EffectProvenanceNode::default()))
    }
}

impl Drop for EffectProvenance {
    fn drop(&mut self) {
        // Arc normally destroys a uniquely-owned chain recursively. Provenance
        // paths are generated data and can be much deeper than the machine
        // stack, so detach unique children breadth-first before Arc drops them.
        let mut work = Vec::new();
        if let Some(node) = Arc::get_mut(&mut self.0) {
            work.extend(node.result.take());
            work.extend(std::mem::take(&mut node.fields).into_values());
        }
        while let Some(mut child) = work.pop() {
            if let Some(node) = Arc::get_mut(&mut child.0) {
                work.extend(node.result.take());
                work.extend(std::mem::take(&mut node.fields).into_values());
            }
        }
    }
}

impl std::ops::Deref for EffectProvenance {
    type Target = EffectProvenanceNode;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Copy)]
struct ProvenanceMeter {
    left: usize,
}

impl ProvenanceMeter {
    fn spend(&mut self, amount: usize) -> bool {
        if let Some(left) = self.left.checked_sub(amount) {
            self.left = left;
            true
        } else {
            self.left = 0;
            false
        }
    }
}

impl EffectProvenance {
    const PATH_BUDGET: usize = 256;
    const NODE_BUDGET: usize = 4096;
    const SUMMARY_BUDGET: usize = 16;

    fn node(mut node: EffectProvenanceNode) -> Self {
        if node.exact_origins.is_empty() {
            for source in &node.callable {
                if let EffectSource::Origin(origin) = source
                    && !node.exact_origins.contains(origin)
                {
                    node.exact_origins.push(origin.clone());
                    if node.exact_origins.len() == Self::SUMMARY_BUDGET {
                        break;
                    }
                }
            }
            for child in node.result.iter().chain(node.fields.values()) {
                for origin in &child.exact_origins {
                    if !node.exact_origins.contains(origin) {
                        node.exact_origins.push(origin.clone());
                        if node.exact_origins.len() == Self::SUMMARY_BUDGET {
                            break;
                        }
                    }
                }
                if node.exact_origins.len() == Self::SUMMARY_BUDGET {
                    break;
                }
            }
        }
        Self::node_summarized(node)
    }

    /// Construct a node whose summary was already assembled by metered work.
    fn node_summarized(node: EffectProvenanceNode) -> Self {
        if node.callable.is_empty()
            && node.value_parameter.is_none()
            && node.argument.is_none()
            && node.result.is_none()
            && node.fields.is_empty()
        {
            Self::default()
        } else {
            Self(Arc::new(node))
        }
    }

    fn parameter(symbol: Symbol) -> Self {
        Self::node(EffectProvenanceNode {
            value_parameter: Some(EffectParameter {
                symbol,
                path: Vec::new(),
            }),
            ..Default::default()
        })
    }

    fn function(callable: Vec<EffectSource>, argument: Symbol, result: Self) -> Self {
        Self::node(EffectProvenanceNode {
            callable: deduplicate(callable),
            argument: Some(argument),
            result: (result != Self::default()).then_some(result),
            ..Default::default()
        })
    }

    fn origin(origin: EffectOrigin) -> Self {
        Self::node(EffectProvenanceNode {
            callable: vec![EffectSource::Origin(origin)],
            ..Default::default()
        })
    }

    fn from_fields(fields: IndexMap<String, Self>) -> Self {
        Self::node(EffectProvenanceNode {
            fields: fields
                .into_iter()
                .filter(|(_, p)| *p != Self::default())
                .collect(),
            ..Default::default()
        })
    }

    fn select_metered(
        &self,
        path: &[EffectPathStep],
        meter: &mut ProvenanceMeter,
    ) -> Result<Option<Self>, Self> {
        // Missing structure and a bounded traversal cutoff have different
        // meanings. A genuinely missing route contributes nothing; a cutoff
        // conservatively contributes this node's cached declaration summary.
        if path.len() > Self::PATH_BUDGET || !meter.spend(1) {
            return Err(self.exact_summary());
        }
        let mut work: Vec<_> = path.iter().enumerate().rev().collect();
        let mut selected = self;
        while let Some((at, step)) = work.pop() {
            // One path step and one traversed edge (including a missing edge).
            if !meter.spend(2) {
                return Err(selected.exact_summary());
            }
            let known = match step {
                EffectPathStep::CallResult => selected.result.as_ref(),
                EffectPathStep::Field(field) => selected.fields.get(field),
            };
            if let Some(known) = known {
                selected = known;
                if !meter.spend(1) {
                    return Err(selected.exact_summary());
                } // selected node
                continue;
            }
            let Some(mut parameter) = selected.value_parameter.clone() else {
                return Ok(None);
            };
            let tail = &path[at..];
            if parameter.path.len() + tail.len() > Self::PATH_BUDGET || !meter.spend(tail.len()) {
                return Err(selected.exact_summary());
            }
            parameter.path.extend_from_slice(tail);
            return Ok(Some(Self::node(EffectProvenanceNode {
                value_parameter: Some(parameter),
                ..Default::default()
            })));
        }
        Ok(Some(selected.clone()))
    }

    fn select(&self, path: &[EffectPathStep]) -> Option<Self> {
        let mut meter = ProvenanceMeter {
            left: Self::NODE_BUDGET,
        };
        match self.select_metered(path, &mut meter) {
            Ok(selected) => selected,
            Err(summary) => Some(summary),
        }
    }

    fn projected(&self, field: String) -> Option<Self> {
        self.select(&[EffectPathStep::Field(field)])
    }

    fn call_result(&self) -> Option<Self> {
        self.select(&[EffectPathStep::CallResult])
    }

    fn substitute(&self, symbol: Symbol, value: &Self) -> Self {
        let mut memo = HashMap::new();
        let mut budget = Self::NODE_BUDGET;
        self.substitute_bounded(symbol, value, &mut memo, &mut budget)
    }

    fn exact_summary(&self) -> Self {
        Self::node_summarized(EffectProvenanceNode {
            callable: self
                .exact_origins
                .iter()
                .cloned()
                .map(EffectSource::Origin)
                .collect(),
            exact_origins: self.exact_origins.clone(),
            ..Default::default()
        })
    }

    fn exhausted_with(&self, symbol: Symbol, extra: &[EffectOrigin]) -> Self {
        let mut exact_origins = self.exact_origins.clone();
        for origin in extra {
            if exact_origins.len() == Self::SUMMARY_BUDGET {
                break;
            }
            if !exact_origins.contains(origin) {
                exact_origins.push(origin.clone());
            }
        }
        let callable = exact_origins
            .iter()
            .cloned()
            .map(EffectSource::Origin)
            .collect();
        Self::node_summarized(EffectProvenanceNode {
            callable,
            // Never leave a source naming the argument we just claimed to
            // substitute. An unresolved projection is omission, not stale data.
            value_parameter: self.value_parameter.clone().filter(|p| p.symbol != symbol),
            argument: self.argument,
            exact_origins,
            ..Default::default()
        })
    }

    fn exhausted(&self, symbol: Symbol) -> Self {
        self.exhausted_with(symbol, &[])
    }

    fn substitute_bounded(
        &self,
        symbol: Symbol,
        value: &Self,
        memo: &mut HashMap<*const EffectProvenanceNode, Self>,
        budget: &mut usize,
    ) -> Self {
        #[derive(Clone, Copy)]
        enum Stage {
            ResultStart,
            ResultFinish,
            FieldStart(usize),
            FieldFinish(usize),
            Callable(usize),
            Finish,
        }
        struct Frame {
            original: EffectProvenance,
            stage: Stage,
            result: Option<EffectProvenance>,
            fields: IndexMap<String, EffectProvenance>,
            callable: Vec<EffectSource>,
            exact: Vec<EffectOrigin>,
        }
        enum Task {
            Visit(EffectProvenance),
            Resume(Frame),
        }

        fn add_unique<T: PartialEq>(
            out: &mut Vec<T>,
            item: T,
            meter: &mut ProvenanceMeter,
        ) -> bool {
            let found = out.iter().position(|v| v == &item);
            let comparisons = found.map_or(out.len(), |at| at + 1);
            if !meter.spend(comparisons) {
                return false;
            }
            if found.is_none() {
                out.push(item);
            }
            true
        }
        fn add_exact(
            frame: &mut Frame,
            origins: &[EffectOrigin],
            meter: &mut ProvenanceMeter,
        ) -> bool {
            for origin in origins
                .iter()
                .take(EffectProvenance::SUMMARY_BUDGET - frame.exact.len())
            {
                if !meter.spend(1) || !add_unique(&mut frame.exact, origin.clone(), meter) {
                    return false;
                }
            }
            true
        }

        let mut meter = ProvenanceMeter { left: *budget };
        let root = Arc::as_ptr(&self.0);
        let mut work = vec![Task::Visit(self.clone())];
        while let Some(task) = work.pop() {
            match task {
                Task::Visit(original) => {
                    let key = Arc::as_ptr(&original.0);
                    if memo.contains_key(&key) {
                        continue;
                    }
                    if !meter.spend(1) {
                        memo.insert(key, original.exhausted(symbol));
                        continue;
                    }
                    if let Some(parameter) = &original.value_parameter
                        && parameter.symbol == symbol
                    {
                        let selected = match value.select_metered(&parameter.path, &mut meter) {
                            Ok(Some(selected)) => selected,
                            Ok(None) => Self::default(),
                            Err(summary) => summary,
                        };
                        memo.insert(key, selected);
                        continue;
                    }
                    work.push(Task::Resume(Frame {
                        original,
                        stage: Stage::ResultStart,
                        result: None,
                        fields: IndexMap::new(),
                        callable: Vec::new(),
                        exact: Vec::new(),
                    }));
                }
                Task::Resume(mut frame) => {
                    let key = Arc::as_ptr(&frame.original.0);
                    let failed = match frame.stage {
                        Stage::ResultStart => {
                            if let Some(child) = frame.original.result.clone() {
                                if !meter.spend(1) {
                                    true
                                } else {
                                    frame.stage = Stage::ResultFinish;
                                    work.push(Task::Resume(frame));
                                    work.push(Task::Visit(child));
                                    continue;
                                }
                            } else {
                                frame.stage = Stage::FieldStart(0);
                                work.push(Task::Resume(frame));
                                continue;
                            }
                        }
                        Stage::ResultFinish => {
                            let child = frame
                                .original
                                .result
                                .as_ref()
                                .and_then(|c| memo.get(&Arc::as_ptr(&c.0)))
                                .cloned()
                                .unwrap_or_default();
                            if !add_exact(&mut frame, &child.exact_origins, &mut meter) {
                                true
                            } else {
                                frame.result = (child != Self::default()).then_some(child);
                                frame.stage = Stage::FieldStart(0);
                                work.push(Task::Resume(frame));
                                continue;
                            }
                        }
                        Stage::FieldStart(at) => {
                            if let Some(child) = frame
                                .original
                                .fields
                                .get_index(at)
                                .map(|(_, child)| child.clone())
                            {
                                if !meter.spend(1) {
                                    true
                                } else {
                                    frame.stage = Stage::FieldFinish(at);
                                    work.push(Task::Resume(frame));
                                    work.push(Task::Visit(child));
                                    continue;
                                }
                            } else {
                                frame.stage = Stage::Callable(0);
                                work.push(Task::Resume(frame));
                                continue;
                            }
                        }
                        Stage::FieldFinish(at) => {
                            let (name, old) = frame
                                .original
                                .fields
                                .get_index(at)
                                .map(|(name, old)| (name.clone(), old.clone()))
                                .expect("field frame");
                            let child = memo.get(&Arc::as_ptr(&old.0)).cloned().unwrap_or_default();
                            if !add_exact(&mut frame, &child.exact_origins, &mut meter) {
                                true
                            } else {
                                if child != Self::default() {
                                    frame.fields.insert(name, child);
                                }
                                frame.stage = Stage::FieldStart(at + 1);
                                work.push(Task::Resume(frame));
                                continue;
                            }
                        }
                        Stage::Callable(at) => {
                            if let Some(source) = frame.original.callable.get(at) {
                                if !meter.spend(1) {
                                    true
                                } else {
                                    let mut ok = true;
                                    match source {
                                        EffectSource::Parameter { symbol: p, path }
                                            if *p == symbol =>
                                        {
                                            let replacement =
                                                match value.select_metered(path, &mut meter) {
                                                    Ok(Some(replacement)) => Some(replacement),
                                                    Ok(None) => None,
                                                    Err(summary) => Some(summary),
                                                };
                                            if let Some(replacement) = replacement {
                                                // Preserve the bounded endpoint cache before
                                                // walking entries. If admission exhausts midway,
                                                // the fallback still names declarations introduced
                                                // by this substitution.
                                                ok = add_exact(
                                                    &mut frame,
                                                    &replacement.exact_origins,
                                                    &mut meter,
                                                );
                                                if ok {
                                                    // Do not collect or clone the replacement list:
                                                    // every entry is metered before it is admitted.
                                                    for replacement_source in &replacement.callable
                                                    {
                                                        let entry_work = match replacement_source {
                                                            EffectSource::Origin(_) => 1,
                                                            EffectSource::Parameter {
                                                                path,
                                                                ..
                                                            } => path.len() + 1,
                                                        };
                                                        if entry_work > Self::PATH_BUDGET + 1
                                                            || !meter.spend(entry_work)
                                                            || !add_unique(
                                                                &mut frame.callable,
                                                                replacement_source.clone(),
                                                                &mut meter,
                                                            )
                                                        {
                                                            ok = false;
                                                            break;
                                                        }
                                                    }
                                                }
                                                if ok
                                                    && let Some(parameter) =
                                                        &replacement.value_parameter
                                                    && (parameter.path.len() > Self::PATH_BUDGET
                                                        || !meter.spend(parameter.path.len() + 1)
                                                        || !add_unique(
                                                            &mut frame.callable,
                                                            EffectSource::Parameter {
                                                                symbol: parameter.symbol,
                                                                path: parameter.path.clone(),
                                                            },
                                                            &mut meter,
                                                        ))
                                                {
                                                    ok = false;
                                                }
                                            }
                                        }
                                        source => {
                                            let path_work = match source {
                                                EffectSource::Origin(_) => 1,
                                                EffectSource::Parameter { path, .. } => {
                                                    path.len() + 1
                                                }
                                            };
                                            if path_work > Self::PATH_BUDGET + 1
                                                || !meter.spend(path_work)
                                            {
                                                ok = false;
                                            } else {
                                                // Clone only this admitted entry, after charging
                                                // for its path-sized allocation.
                                                let source = source.clone();
                                                if let EffectSource::Origin(origin) = &source
                                                    && !add_exact(
                                                        &mut frame,
                                                        std::slice::from_ref(origin),
                                                        &mut meter,
                                                    )
                                                {
                                                    ok = false;
                                                }
                                                if ok
                                                    && !add_unique(
                                                        &mut frame.callable,
                                                        source,
                                                        &mut meter,
                                                    )
                                                {
                                                    ok = false;
                                                }
                                            }
                                        }
                                    }
                                    if !ok {
                                        true
                                    } else {
                                        frame.stage = Stage::Callable(at + 1);
                                        work.push(Task::Resume(frame));
                                        continue;
                                    }
                                }
                            } else {
                                frame.stage = Stage::Finish;
                                work.push(Task::Resume(frame));
                                continue;
                            }
                        }
                        Stage::Finish => {
                            let out = Self::node_summarized(EffectProvenanceNode {
                                callable: frame.callable,
                                value_parameter: frame.original.value_parameter.clone(),
                                argument: frame.original.argument,
                                result: frame.result,
                                fields: frame.fields,
                                exact_origins: frame.exact,
                            });
                            memo.insert(key, out);
                            continue;
                        }
                    };
                    if failed {
                        memo.insert(key, frame.original.exhausted_with(symbol, &frame.exact));
                    }
                }
            }
        }
        *budget = meter.left;
        memo.get(&root)
            .cloned()
            .unwrap_or_else(|| self.exhausted(symbol))
    }
}
fn deduplicate<T: PartialEq>(values: Vec<T>) -> Vec<T> {
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

#[cfg(test)]
mod effect_provenance_tests {
    use super::*;
    use crate::symbol::{Bundle, Mint, Namespace};
    use semver::Version;

    fn symbol(mint: &mut Mint, name: &str) -> Symbol {
        mint.local(None, Namespace::Terms, name)
    }

    fn unique_nodes(root: &EffectProvenance) -> usize {
        let mut seen = std::collections::HashSet::new();
        let mut work = vec![root];
        while let Some(node) = work.pop() {
            if !seen.insert(Arc::as_ptr(&node.0)) {
                continue;
            }
            work.extend(node.fields.values());
            work.extend(node.result.iter());
        }
        seen.len()
    }

    #[test]
    fn duplicate_substitution_is_a_bounded_shared_dag() {
        const DEPTH: usize = 40;
        let bundle = Bundle::new("provenance-test", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let parameter = symbol(&mut mint, "x");
        let effect = symbol(&mut mint, "Log");
        let parameter_node = EffectProvenance::parameter(parameter);
        let duplicated = EffectProvenance::from_fields(
            [
                ("a".into(), parameter_node.clone()),
                ("b".into(), parameter_node),
            ]
            .into_iter()
            .collect(),
        );
        let duplicate = EffectProvenance::function(Vec::new(), parameter, duplicated);

        let mut operations = 0;
        let mut empty = EffectProvenance::default();
        for _ in 0..DEPTH {
            let mut memo = HashMap::new();
            let mut budget = EffectProvenance::NODE_BUDGET;
            empty = duplicate
                .substitute_bounded(parameter, &empty, &mut memo, &mut budget)
                .call_result()
                .unwrap_or_default();
            operations += EffectProvenance::NODE_BUDGET - budget;
        }
        assert_eq!(
            empty,
            EffectProvenance::default(),
            "empty branches are pruned"
        );
        assert!(
            operations <= DEPTH * 32,
            "empty substitution work is linear"
        );

        let origin = EffectOrigin {
            symbol: effect,
            interface: crate::types::EffectId::structural("Log".into(), "test".into()),
            declaration_at: Anchor::GENERATED,
        };
        let mut operations = 0;
        let mut real = EffectProvenance::origin(origin.clone());
        for _ in 0..DEPTH {
            let mut memo = HashMap::new();
            let mut budget = EffectProvenance::NODE_BUDGET;
            real = duplicate
                .substitute_bounded(parameter, &real, &mut memo, &mut budget)
                .call_result()
                .expect("duplicate has a result");
            operations += EffectProvenance::NODE_BUDGET - budget;
        }
        assert!(operations <= DEPTH * 32, "real substitution work is linear");
        assert!(
            unique_nodes(&real) <= DEPTH + 1,
            "forty binary duplications must retain a linear number of nodes"
        );
        let mut selected = real;
        for _ in 0..DEPTH {
            let a = selected.fields.get("a").expect("duplicated a");
            let b = selected.fields.get("b").expect("duplicated b");
            assert!(Arc::ptr_eq(&a.0, &b.0), "identical branches share one node");
            selected = selected.projected("a".into()).expect("projected a");
        }
        assert_eq!(selected.callable, [EffectSource::Origin(origin)]);
    }

    #[test]
    fn deep_substitution_and_destruction_fit_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let bundle = Bundle::new("deep-provenance", Version::new(0, 0, 0)).unwrap();
                let mut mint = Mint::new(bundle);
                let target = symbol(&mut mint, "target");
                let other = symbol(&mut mint, "other");
                let mut provenance = EffectProvenance::parameter(target);
                for _ in 0..5000 {
                    provenance = EffectProvenance::from_fields(
                        [("next".into(), provenance)].into_iter().collect(),
                    );
                }
                let rewritten = provenance.substitute(other, &EffectProvenance::default());
                assert!(!rewritten.exact_origins.iter().any(|_| false));
                drop(rewritten);
                drop(provenance);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn wide_graph_work_is_bounded_and_shared_fallback_is_memoized() {
        let bundle = Bundle::new("wide-provenance", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let target = symbol(&mut mint, "target");
        let shared = EffectProvenance::parameter(target);
        let wide = EffectProvenance::from_fields(
            (0..10_000)
                .map(|n| (format!("f{n:05}"), shared.clone()))
                .collect(),
        );
        let mut memo = HashMap::new();
        let mut budget = EffectProvenance::NODE_BUDGET;
        let rewritten =
            wide.substitute_bounded(target, &EffectProvenance::default(), &mut memo, &mut budget);
        assert_eq!(budget, 0);
        assert!(unique_nodes(&rewritten) <= EffectProvenance::NODE_BUDGET);
    }

    #[test]
    fn huge_callable_replacement_is_admitted_with_bounded_work_and_allocation() {
        let bundle = Bundle::new("huge-callable", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let target = symbol(&mut mint, "target");
        let effect = symbol(&mut mint, "Log");
        let origin = EffectOrigin {
            symbol: effect,
            interface: crate::types::EffectId::structural("Log".into(), "test".into()),
            declaration_at: Anchor::GENERATED,
        };
        // Construct directly: function() deliberately deduplicates its normal,
        // source-sized input, while this test models an adversarial replacement.
        let replacement = EffectProvenance::node(EffectProvenanceNode {
            callable: vec![EffectSource::Origin(origin.clone()); 100_000],
            ..Default::default()
        });
        let source = EffectProvenance::function(
            vec![EffectSource::Parameter {
                symbol: target,
                path: Vec::new(),
            }],
            target,
            EffectProvenance::default(),
        );
        let mut memo = HashMap::new();
        let mut budget = EffectProvenance::NODE_BUDGET;
        let rewritten = source.substitute_bounded(target, &replacement, &mut memo, &mut budget);
        assert_eq!(budget, 0, "the traversal must stop at its work budget");
        assert_eq!(rewritten.callable, [EffectSource::Origin(origin)]);
        assert!(
            rewritten.callable.capacity() <= EffectProvenance::SUMMARY_BUDGET,
            "fallback allocation is bounded rather than sized to the replacement"
        );
    }

    #[test]
    fn selection_exhaustion_retains_exact_origin_but_missing_path_does_not() {
        let bundle = Bundle::new("selection-exhaustion", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let target = symbol(&mut mint, "target");
        let effect = symbol(&mut mint, "Log");
        let origin = EffectOrigin {
            symbol: effect,
            interface: crate::types::EffectId::structural("Log".into(), "test".into()),
            declaration_at: Anchor::GENERATED,
        };
        let replacement = EffectProvenance::origin(origin.clone());
        let too_long = vec![EffectPathStep::CallResult; EffectProvenance::PATH_BUDGET + 1];

        let mut meter = ProvenanceMeter { left: usize::MAX };
        let Err(summary) = replacement.select_metered(&too_long, &mut meter) else {
            panic!("PATH_BUDGET exhaustion must be distinct from a missing path");
        };
        assert_eq!(summary.callable, [EffectSource::Origin(origin.clone())]);
        let mut meter = ProvenanceMeter { left: usize::MAX };
        assert!(matches!(
            replacement.select_metered(&[EffectPathStep::CallResult], &mut meter),
            Ok(None)
        ));

        let source = EffectProvenance::function(
            vec![EffectSource::Parameter {
                symbol: target,
                path: too_long,
            }],
            target,
            EffectProvenance::default(),
        );
        let rewritten = source.substitute(target, &replacement);
        assert_eq!(rewritten.callable, [EffectSource::Origin(origin)]);
    }

    #[test]
    fn exhausted_rewrite_retains_deep_exact_origin_without_stale_parameter() {
        let bundle = Bundle::new("exhausted-provenance", Version::new(0, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let target = symbol(&mut mint, "target");
        let effect = symbol(&mut mint, "Log");
        let origin = EffectOrigin {
            symbol: effect,
            interface: crate::types::EffectId::structural("Log".into(), "test".into()),
            declaration_at: Anchor::GENERATED,
        };
        let mut deep = EffectProvenance::origin(origin.clone());
        for _ in 0..100 {
            deep = EffectProvenance::from_fields([("next".into(), deep)].into_iter().collect());
        }
        let source = EffectProvenance::function(
            vec![EffectSource::Parameter {
                symbol: target,
                path: vec![],
            }],
            target,
            deep,
        );
        let mut memo = HashMap::new();
        let mut budget = 0;
        let rewritten =
            source.substitute_bounded(target, &EffectProvenance::default(), &mut memo, &mut budget);
        assert!(rewritten.callable.contains(&EffectSource::Origin(origin)));
        assert!(!rewritten.callable.iter().any(
            |source| matches!(source, EffectSource::Parameter { symbol, .. } if *symbol == target)
        ));
        assert!(
            rewritten
                .value_parameter
                .as_ref()
                .is_none_or(|p| p.symbol != target)
        );
    }
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
    pub at: Anchor,
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
    /// Indices into `full_facts` selected for the ordinary source-ordered view.
    pub abridged: Vec<usize>,
    /// The one source-facing name, if any, which ties displayed facts together.
    /// It is presentation metadata, never a solver-variable name.
    pub pivot: Option<ExplanationPivot>,
    /// Grounded facts retained by the full path but left out of `abridged`.
    pub omitted_facts: usize,
    pub contradiction: Contradiction,
    /// Both the source constraint slice and the unabridged raw reason slice.
    pub cause: ExplanationCause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationFact {
    pub at: Anchor,
    /// True for source facts established by a direct boundary check.
    pub direct: bool,
    pub constraint: ConstraintId,
    pub origin: ConstraintOrigin,
    pub subject: Subject,
    pub payload: ExplanationFactPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationPivot {
    /// Explanatory spelling chosen without exposing or consulting solver IDs.
    pub name: String,
    pub kind: ExplanationPivotKind,
    /// Displayed facts which refer to this pivot, as indices into `full_facts`.
    pub references: Vec<usize>,
    /// The one fact whose label introduces the spelling.
    pub introduced_at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplanationPivotKind {
    WrittenValue,
    FunctionInput,
    BranchResult,
    ProjectedField,
    FunctionEffects,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExplanationFactPayload {
    RequiresType,
    UsedAsFunction,
    SuppliesArgument,
    BranchResult,
    /// The written `.name` / pattern operation that demands a label.
    LabelDemand,
    /// A source use that fixes or closes the other side of a row.
    ClosedRow,
    /// The source use which actually introduces the repeated label.
    LabelIntroduction,
    /// The source row remainder whose adjacent label forbids that introduction.
    LabelForbidden,
    /// The annotation spelling which leaves this choice to each caller.
    CallerChoiceDeclaration,
    /// The body operation which fixes or assumes part of the caller's choice.
    CallerChoiceUse,
    /// The binding whose published type would carry another annotation's choice.
    CallerChoiceDestination,
    /// A written operation application, including calls which propagate an
    /// operation performed deeper in their callee.
    EffectUse,
    /// The function or top-level computation whose effect boundary refuses it.
    EffectBoundary,
    /// The source declaration which gives the effect its name.
    EffectDeclaration,
    /// The callback type at the exact host parameter path which requires effects.
    CallbackRequirement,
    /// The containing extern call's written effect capability.
    ExternCapability,
    /// The declaration which owns a foreign boundary.
    ExternDeclaration,
    /// The generic leaf whose runtime representation is not fixed.
    PolymorphicExternLeaf,
    /// The host input/result path which needs a fixed representation.
    ExternPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub kind: ContradictionKind,
    pub left: TypeDescription,
    pub right: TypeDescription,
    /// Present for row-shaped contradictions. The label is retained as source
    /// syntax rather than reconstructed from a rendered solver row.
    pub row: Option<RowContradiction>,
    /// Source shape of a recursive cycle, used to keep repair advice specific.
    pub recursive: Option<RecursiveCycleShape>,
    /// Neutral failures always retain both possible repair directions.
    pub repairs: [RepairDirection; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowContradiction {
    pub shape: Shape,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictionKind {
    IncompatibleTypes,
    ValueUsedAsFunction,
    /// A finite value would have to contain itself, or a callable value would
    /// have to accept itself as one of its own inputs.
    RecursiveValue,
    ProjectionOnNonStruct,
    LabelUnavailable,
    RepeatedLabel,
    /// A body fixed or inspected a choice its annotation leaves to callers.
    CallerChoice,
    /// A binding would export a choice owned by another annotation.
    CallerChoiceEscape,
    /// An operation reaches a top-level computation with no enclosing handler.
    UnhandledEffect,
    /// An operation reaches an enclosing function whose type does not list it.
    EffectNotAllowed,
    CallbackEffectsNotCovered,
    PolymorphicExternBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDirection {
    ChangeFirstUse,
    ChangeSecondUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursiveCycleShape {
    CallInput,
    Containment,
    Neutral,
}

/// Deliberately source-facing and finite: no solver variable, row-tail, or
/// compiler-synthesized arrow can enter migrated diagnostic prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeDescription {
    NaturalNumber,
    Integer,
    RealNumber,
    Text,
    Bool,
    Any,
    ForeignValue,
    Function,
    Struct,
    TaggedValue,
    Array,
    Mut,
    DeclaredType,
    Undecided,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationCause {
    pub error: ErrorId,
    pub seed: Option<ReasonId>,
    pub constraints: Vec<ConstraintId>,
    pub reasons: Vec<ReasonId>,
    /// Reachable reason nodes not visited because the source-facing full view
    /// hit its work budget. Nonzero means the slice is explicitly abridged.
    pub omitted_reasons: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCause {
    Step(StepId),
    Batch(BatchId),
    Direct,
}

impl Error {
    /// A direct, not-yet-explained error. Inference replaces the pending
    /// identity before publishing an [`Output`].
    pub fn new(span: Anchor, kind: ErrorKind) -> Self {
        Self {
            id: ErrorId::pending(),
            cause: ErrorCause::Direct,
            at: span,
            kind,
            explanation: None,
        }
    }
}

/// What asked a value to be a struct, for the wording of
/// [`ErrorKind::NotAStruct`]: the complaint is the same, and what the reader
/// wrote — and so what they might change — is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructDemand {
    /// `v.x` — a field read off the value.
    Projection,
    /// `{ ..v }` — the value's fields spread into a struct literal.
    Spread,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    RuntimeTypeInformation {
        message: String,
    },
    /// A value whose outer type is known not to be a struct was asked for its
    /// fields — read one at a time, or spread all at once.
    NotAStruct {
        base: Arc<Ty>,
        demand: StructDemand,
    },
    /// Two types that had to be equal are not. `expected` is the side the
    /// context demanded — an annotation, a function's parameter, or the arrow
    /// shape a call site needs — and `actual` is what the term turned out to
    /// be.
    Mismatch {
        expected: Arc<Ty>,
        actual: Arc<Ty>,
    },
    /// Two applications of one effect met and their arguments at one
    /// position could not be made equal: one computation would be using
    /// incompatible versions of the effect. The failure the arguments met
    /// with is kept as the cause, so the reader is told which requirement on
    /// the argument clashed with which.
    EffectArgument {
        effect: String,
        position: u32,
        cause: Box<ErrorKind>,
    },
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
        base: Arc<Ty>,
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
        base: Arc<Ty>,
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
        found: Arc<Ty>,
        name: Arc<str>,
        sense: Sense,
        declared: Anchor,
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
        name: Arc<str>,
        declared: Anchor,
    },
    /// A variable reaching a type outside the annotation that
    /// declared it.
    ///
    /// A rigid stands for whatever the caller of *that* annotation picks, so it
    /// means nothing anywhere else: a scheme quantifying one would be promising
    /// its own callers something only somebody else's caller decides. Reported
    /// at the declaring name, which is the line that has to change.
    RigidEscapes {
        name: Arc<str>,
        declared: Anchor,
        destination: Arc<Ty>,
        destination_name: Arc<str>,
        destination_span: Anchor,
    },
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
    RepeatedField {
        shape: Shape,
        field: String,
        introduction: Option<RowFactOrigin>,
        forbidden: Option<RowFactOrigin>,
    },
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
    PresenceImpossible {
        formula: String,
    },
    /// An annotation whose `where` clause nothing can satisfy on its own
    /// terms — `where a and not a`, which forbids every value at once.
    ///
    /// [`ErrorKind::PresenceImpossible`]'s other half, split from it because
    /// the two blame different things. There the clause is fine and the
    /// definition is what leaves it without a model; here nothing outside the
    /// clause is involved, and a complaint that mentioned the definition would
    /// be asserting something untrue of it.
    ClauseImpossible {
        formula: String,
    },
    /// An annotation whose `where` clause allows more than the definition
    /// under it does — `where a or b` over a body that needs `a`.
    ///
    /// The annotation is the contract, and every use of the name sees it, so a
    /// body requiring more than it promises would let a use through that the
    /// definition cannot serve. Refused at the annotation, which is the line
    /// the reader has to change: both formulas are carried so the complaint can
    /// show what was promised beside what is needed.
    AnnotationAllows {
        allowed: String,
        required: String,
    },
    /// An effect performed where nothing could ever handle it: a definition's
    /// value is computed outside every handler, so an effect performed there
    /// has no one to answer it.
    ///
    /// [`ErrorKind::NotAllowed`]'s other reading, and the two are split because
    /// they send the reader to different places: here there is no function to
    /// widen and the fix is to wrap the value in one, or in a handler.
    Unhandled {
        effect: String,
    },
    /// An effect performed inside a function whose own row does not allow it.
    ///
    /// The ordinary refusal, and the reason an effect row is worth having: the
    /// function says what calling it may do, and this would do more.
    NotAllowed {
        effect: String,
    },
    /// A foreign callback requires evidence the containing host call cannot carry.
    CallbackEffectsNotCovered {
        missing_effects: Vec<String>,
        extern_effects: Vec<String>,
        callback_path: String,
        callback_type: String,
        extern_name: String,
        /// Every offending callback path in this declaration. The scalar
        /// fields above retain the first issue for compatibility and prose.
        issues: Vec<ExternCallbackIssue>,
    },
    /// An ordinary foreign boundary leaf has no fixed runtime representation:
    /// an annotation variable there could instantiate to a Ruddy closure.
    PolymorphicExternBoundary {
        variable: String,
        variable_kind: ExternVariableKind,
        position: String,
        extern_name: String,
        /// All representation-varying leaves in source/path order.
        leaves: Vec<PolymorphicExternLeaf>,
        /// Callback evidence is retained even when a polymorphic leaf is the
        /// declaration's primary existing diagnostic kind.
        callback_issues: Vec<ExternCallbackIssue>,
    },
}

impl ErrorKind {
    /// The failure this one reports, looking through an effect-argument
    /// clash to the failure its arguments met with.
    pub fn cause(&self) -> &ErrorKind {
        match self {
            ErrorKind::EffectArgument { cause, .. } => cause,
            other => other,
        }
    }

    /// This failure, reported as a clash between two applications of
    /// `effect` at argument `position` that it descends from.
    pub(crate) fn as_effect_argument(&mut self, effect: String, position: u32) {
        let cause = Box::new(std::mem::replace(self, ErrorKind::Recursive));
        *self = ErrorKind::EffectArgument {
            effect,
            position,
            cause,
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternVariableKind {
    Type,
    Row,
    Presence,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFactOrigin {
    pub constraint: ConstraintId,
    pub reason: ReasonId,
    pub origin: ConstraintOrigin,
    pub subject: Subject,
    pub at: Anchor,
}

#[derive(Debug, Clone)]
struct LacksEntry {
    shape: Shape,
    label: String,
    origin: Option<RowFactOrigin>,
}

type Lacks = IndexMap<String, LacksEntry>;

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
    handler_presences: HashSet<TyVar>,
    handler_absences: HashSet<TyVar>,
    vars: Vec<Slot>,
    /// [`Table::var_meta`] is append-only, so its snapshot is its length and
    /// putting it back is a truncate.
    var_meta_len: usize,
    levels: Vec<u32>,
    lacks: HashMap<TyVar, Lacks>,
    active_lacks_origin: Option<RowFactOrigin>,
    existential_witnesses: HashSet<TyVar>,
    abstract_existentials: HashSet<TyVar>,
    reason_len: usize,
}

/// What one name in scope means. Private for the same reason as [`Slot`]: a
/// binding exists only while a definition is being walked, and what survives
/// the walk is the [`Scheme`] in [`Semantics::schemes`].
#[derive(Debug, Clone)]
enum Binding {
    Mono(Arc<Ty>),
    Poly(ExplainedScheme),
    /// A name a nested `let` bound, whose scheme only the solver will know.
    ///
    /// Generation cannot name what a use of one is a copy of: the scheme is
    /// what solving the value produces, and generation has solved nothing. So
    /// it says only that this use is a copy — a
    /// [`ConstraintKind::Instance`] — and the solver, which by then has the
    /// scheme, makes it.
    Local,
}

/// A semantic scheme paired with the causal roots that produced its shape.
///
/// This deliberately stays outside [`Scheme`]: spans and inference identity are
/// compilation-local evidence, not part of type equality or an exported type.
/// Opening the scheme opens this compact skeleton at the same time, attaching
/// its shared roots to the freshly instantiated variables (and to the opened
/// root for closed contracts).
#[derive(Debug, Clone)]
struct ExplainedScheme {
    scheme: Scheme,
    provenance: SchemeProvenance,
}

/// A compact causal skeleton keyed like the zonked scheme body. Child edges,
/// rather than incidental traversal order, align evidence when a scheme is
/// opened; this matters when packages disappear and absent payloads are
/// pruned.
#[derive(Debug, Clone, Default)]
struct SchemeProvenance {
    nodes: Vec<ProvenanceNode>,
    quantified: Vec<QuantifiedProvenance>,
}

#[derive(Debug, Clone)]
struct QuantifiedProvenance {
    sort: VarSort,
    roots: Vec<ReasonId>,
    omitted: usize,
}

#[derive(Debug, Clone)]
struct ProvenanceNode {
    shape: ProvenanceShape,
    roots: Vec<ReasonId>,
    omitted: usize,
    children: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProvenanceShape {
    Ty(ProvenanceTy),
    Row(Vec<(String, bool)>),
    Presence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvenanceTy {
    Leaf,
    Array,
    Mut,
    Arrow,
    Package,
    Struct,
    Sum,
    Named(usize),
}

impl ExplainedScheme {
    /// An imported contract is authoritative but has no definition span in
    /// this compilation, so only its local use can become a displayed fact.
    fn imported(scheme: Scheme) -> Self {
        Self {
            scheme,
            provenance: SchemeProvenance::default(),
        }
    }

    fn local(scheme: Scheme, provenance: SchemeProvenance) -> Self {
        Self { scheme, provenance }
    }
}

/// Which side of a goal a row's tail sits on. [`Solve::unify`] decomposes
/// without swapping, so an act performed on a tail's behalf — a binding, a
/// field settled absent, a mismatch — has to know which way round to say
/// itself, or a complaint about an annotation would read as one about the
/// term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    bound: Arc<Ty>,
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
    span: Anchor,
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
    ty: Arc<Ty>,
    /// What generation asked of it, kept exactly as it was asked. See
    /// [`DiagnosticView::constraints`].
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

/// What every group reads and none writes: the declarations, lowered once.
/// Shared by reference with every table, so a group's table carries no copy
/// of the program's aliases.
#[derive(Debug, Clone, Default)]
struct Signatures {
    effect_kinds: HashMap<String, Vec<ParamKind>>,
    /// Source spellings for bindings whose nested generalization may report
    /// an escape.
    binding_names: HashMap<Symbol, Arc<str>>,
    /// What each declaration's parameters stand for, in the declaration's
    /// own order. Read only by [`Table::note_lacks`], and only for the one
    /// thing a [`ParamKind`] carries that a type cannot: the labels the
    /// declaration already names beside that parameter, which whatever is
    /// written there may not name either.
    ///
    /// A fact about the *written* declaration that survives no unfolding: by
    /// the time a body has been opened, the tail its parameter sat in is an
    /// ordinary row and the condition has to have been said already.
    params: HashMap<Symbol, Vec<ParamKind>>,
    /// Which declared types are nominal within themselves: those every
    /// parameter of which survives unfolding, so that comparing two
    /// applications argument by argument can only ever agree with comparing
    /// what they stand for. See [`Solve::nominal`].
    nominal: HashSet<Symbol>,
    /// Least declared-type variance fixpoint, shared with inferred presence
    /// classification so a named argument is never assumed covariant merely
    /// because its representation has not been unfolded here.
    variances: HashMap<(Symbol, u32), u8>,
    aliases: IndexMap<Symbol, Scheme>,
    operations: Operations,
    effect_aliases: IndexMap<Symbol, EffectAliasRow>,
    effect_declaration_spans: IndexMap<Symbol, Anchor>,
    externs: IndexMap<Symbol, Scheme>,
    /// The environment every group starts from: the externs and the imported
    /// schemes.
    env: HashMap<Symbol, Binding>,
}

/// What a group's walk may name: everything solved before it, read through
/// the shared map, and what the group itself binds, kept apart so that the
/// group leaves nothing behind in what the next one reads.
struct Env {
    outer: Arc<HashMap<Symbol, Binding>>,
    own: HashMap<Symbol, Binding>,
}

/// What one table's solve produced, handed to [`Table::finish`] to be settled
/// against the table before the table goes away.
struct GroupParts {
    typed: IndexMap<Symbol, Decl<Term>>,
    published: IndexMap<Symbol, ExplainedScheme>,
    schemes: IndexMap<Symbol, Scheme>,
    locals: IndexMap<Symbol, Scheme>,
    promises: IndexMap<Symbol, Formula>,
    constraints: IndexMap<Symbol, Vec<Constraint>>,
    errors: Vec<Error>,
    steps: Vec<Step>,
    refinements: Vec<Refinement>,
}

/// One table's result, complete in itself: the declaration stage's, or one
/// group's. A value with no variable in it that the table it came from would
/// have to be asked about, so that [`assemble`] needs no table at all — and
/// so that a query can hand it back for a group whose inputs did not
/// change.
#[derive(Clone)]
struct GroupResult {
    /// The scope every identity in here was minted under.
    scope: Symbol,
    /// The group's declarations, typed.
    typed: IndexMap<Symbol, Decl<Term>>,
    /// What the group publishes for later groups to instantiate.
    published: IndexMap<Symbol, ExplainedScheme>,
    schemes: IndexMap<Symbol, Scheme>,
    locals: IndexMap<Symbol, Scheme>,
    promises: IndexMap<Symbol, Formula>,
    constraints: IndexMap<Symbol, Vec<Constraint>>,
    errors: Vec<Error>,
    steps: Vec<Step>,
    refinements: Vec<Refinement>,
    /// The group's store, settled: every variable followed to what it was
    /// decided to be.
    store: Store,
    variables: Vec<VarMeta>,
    reasons: Vec<Reason>,
    omitted_reason_parents: HashMap<ReasonId, usize>,
    effect_argument_reasons: HashMap<ReasonId, (String, u32)>,
}

/// Canonical bytes used to compare a query projection: what a solve reads,
/// hashed once they are all written. Types and formulas are walked without
/// the stack, since an imported one may be deeper than any stack; what is
/// written by hand, and the provenance beside a scheme, is taken as it
/// prints.
#[derive(Default)]
struct Fingerprint {
    text: Vec<u8>,
}

#[derive(Default)]
struct Table {
    /// Optional ambient labels introduced during handler constraint generation.
    /// An unused allowance is closed at generalization, like an unshared tail.
    handler_presences: HashSet<TyVar>,
    /// Closed handler allowances disappear, unlike written negative labels.
    handler_absences: HashSet<TyVar>,
    /// What every identity minted here is scoped to. See [`inference_id!`].
    scope: Symbol,
    /// The declarations, shared with every other table. See [`Signatures`].
    signatures: Arc<Signatures>,
    /// Source spellings for the nested bindings walked here, whose
    /// generalization may report an escape. The top-level bindings' are in
    /// the signatures; see [`Table::binding_name`].
    local_names: HashMap<Symbol, Arc<str>>,
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
    /// group. Now `fn p => do let q = p.x return q end` mints the field's variable inside
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
    /// an `Arc` address this cannot alias after an allocation is released, and
    /// the whole map dies with this inference table.
    package_guarantees: HashMap<Vec<TyVar>, PackageGuarantee>,
    /// What the program has required of its presences so far. Grown by
    /// generation (a match's coverage), by instantiation (a constrained
    /// scheme's formula) and by lowering (an annotation's `where` clause), and
    /// consulted only where R8 allows: at a generalization boundary, at the
    /// use-site and annotation checks, and — through
    /// [`Semantics::store`] — in the patterns phase. Never by unification.
    store: Store,
    /// Where each rigid variable this table lowered was declared, by the id
    /// its annotation gave it.
    ///
    /// A [`Ty::Rigid`] carries its spelling but not its span — a type is
    /// printed with no table beside it, and a span is not something a reader
    /// reads — so the second place a rigid complaint points at is looked up
    /// here. A rigid belongs to one annotation, and an annotation to one
    /// definition, so a table only ever meets its own.
    rigids: HashMap<u32, Anchor>,
    region_rigids: HashSet<u32>,
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
    next_constraint_id: u32,
    next_step_id: u32,
    next_error_id: u32,
    next_batch_id: u32,
    /// Immutable append-only causal records. Unlike variable metadata this is
    /// deliberately not restored after speculative congruence.
    reasons: Vec<Reason>,
    /// Publication roots omitted before a marker entered the public arena.
    omitted_reason_parents: HashMap<ReasonId, usize>,
    /// Reasons descended from recovery, retained for debugging but excluded
    /// from publishable scheme evidence in O(1).
    unpublishable_reasons: HashSet<ReasonId>,
    /// The reasons minted while two applications of one effect had their
    /// arguments made equal, by the effect and the argument's position. A
    /// failure whose cause runs through one of these is a clash between two
    /// uses of the effect, however far from the meeting it surfaced.
    effect_argument_reasons: HashMap<ReasonId, (String, u32)>,
    next_reason_id: u32,
    /// Binding reasons observed during one solver act. `None` outside a solve
    /// makes publication, generalization, and zonking incapable of leaking
    /// incidental reads into a later step.
    causal_reads: RefCell<Option<Vec<IndexSet<ReasonId>>>>,
    causal_scope_depth: usize,
    /// Source cause currently introducing row syntax. Lacks facts copy this
    /// value when they are created and retain it across every tail binding.
    active_lacks_origin: Option<RowFactOrigin>,
    /// Opened scheme roots keyed by the root `Arc<Ty>` identity. This covers
    /// closed schemes, which mint no variable on which to hang provenance.
    /// Entries live only for this inference run and never affect semantics.
    opened_provenance: HashMap<usize, (Weak<Ty>, Vec<ReasonId>)>,
    /// Presence-position evidence follows the opened presence variable itself.
    /// It must not ride on the field payload's `Arc`: distinct sibling fields
    /// may deliberately share a payload node without sharing presence causes.
    opened_presence_provenance: HashMap<TyVar, (ReasonId, Vec<ReasonId>)>,
    /// Nested bindings whose written contracts, rather than implementation
    /// evidence, are authoritative when their schemes are published.
    authoritative_bindings: HashSet<Symbol>,
    /// Binding-specific annotation identity used when publishing local schemes.
    authoritative_spans: HashMap<Symbol, Anchor>,
    /// The store, as one solver that has already been told every batch. See
    /// [`Table::store_assumptions`].
    sat: RefCell<StoreSat>,
}

/// The store's batches encoded once each into a solver kept for the run.
///
/// A batch is encoded as it was emitted, over the variables it named then,
/// and what the solve has since decided those variables to be is said afresh
/// as assumptions at every question — which is what makes the encoding
/// survive both a variable being bound after its batch was added and a
/// binding being rolled back. See [`sat::Incremental`].
#[derive(Default)]
struct StoreSat {
    solver: sat::Incremental,
    /// The guard of each batch's encoding and a digest of the formula it
    /// encoded, by the batch's position in the store. Kept no longer than
    /// the store, and re-encoded where the store put a different batch in a
    /// slot: a reserved slot is filled in once its local use is solved. A
    /// batch rolled back or replaced leaves its clauses in the solver under
    /// a guard nothing assumes again.
    guards: Vec<(u64, sat::Lit)>,
    /// The guard of each `atom = atom` said so far, so that a variable that
    /// resolved to another is related to it by one pair of clauses however
    /// many questions are asked while the binding stands.
    equivalences: HashMap<(Atom, Atom), sat::Lit>,
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
    /// A rigid may occur as a whole type or as a row tail. Keep that semantic
    /// sort when the annotation's rigid is reopened as an ordinary variable.
    rigid_sorts: HashMap<u32, VarSort>,
}

impl Subst {
    /// The next position for something that is not a presence. The presences
    /// are numbered first and in full, so what is left is one running count
    /// over the two maps that share the high end of the space.
    fn next(&self) -> u32 {
        (self.presences.len() + self.types.len() + self.rigids.len()) as u32
    }
}

/// Run a real failing equality over a compact, exponentially expanding type
/// DAG. Integration regressions use this to cover recovery independently of
/// source parsing, which cannot express shared Arc identity.
#[doc(hidden)]
pub fn compact_dag_failure_for_tests(definition: Symbol, depth: usize) -> (Vec<Error>, Vec<Step>) {
    let mut table = Table::default();
    let leaf = table.fresh_type_for(Subject::Term);
    let mut dag = leaf;
    for _ in 0..depth {
        dag = Arc::new(Ty::Arrow(dag.clone(), dag, Row::closed()));
    }
    let constraint_id = ConstraintId::synthetic(0);
    let reason = table.reason(ReasonOrigin::Constraint(constraint_id), Vec::new());
    let constraints = [Constraint {
        id: constraint_id,
        reason,
        at: Anchor::GENERATED,
        origin: ConstraintOrigin::ContextualCheck,
        subjects: ConstraintSubjects::pair(Subject::Context, Subject::Term),
        kind: ConstraintKind::Equal {
            expected: Arc::new(Ty::Nat),
            actual: dag,
        },
    }];
    let mut errors = Vec::new();
    let mut steps = Vec::new();
    let aliases = IndexMap::new();
    let nominal = HashSet::new();
    let mut locals = IndexMap::new();
    let mut refinements = Vec::new();
    Solve {
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
        generated_end: constraints.len(),
    }
    .run(&constraints);
    (errors, steps)
}

/// Exercise the match-result family builder without constructing a source
/// program. This is public so integration regressions can feed it semantic
/// types whose depth would make source syntax itself the thing under test.
#[doc(hidden)]
pub fn structural_family_for_tests(
    definition: Symbol,
    aliases: &IndexMap<Symbol, Scheme>,
    types: &[Arc<Ty>],
) -> Arc<Ty> {
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
    body: &Arc<Ty>,
    presences: u32,
    variances: &HashMap<(Symbol, u32), u8>,
) -> (Arc<Ty>, IndexSet<u32>) {
    package_positive_presences(body, presences, &IndexSet::new(), variances)
}

/// Alternately erase transparent packages and unfold transparent names until
/// neither operation exposes another wrapper.
fn expose_packages(aliases: &IndexMap<Symbol, Scheme>, ty: &Arc<Ty>) -> Arc<Ty> {
    let mut exposed = ty.clone();
    loop {
        while let Ty::Package(body) = &*exposed {
            exposed = body.clone();
        }
        let next = unfold(aliases, &exposed);
        if Arc::ptr_eq(&next, &exposed) {
            return exposed;
        }
        exposed = next;
    }
}

/// Find an ordinary boundary leaf whose outer runtime representation remains
/// polymorphic. Such a leaf cannot be adapted once at declaration lowering:
/// in particular, an instantiation to an arrow would otherwise send a Ruddy
/// evidence-taking closure directly to the host.
#[derive(Debug, Clone)]
pub struct PolymorphicExternLeaf {
    pub at: Anchor,
    pub variable: String,
    pub kind: ExternVariableKind,
    pub position: String,
}

#[derive(Debug, Clone)]
pub struct ExternCallbackIssue {
    pub callback_at: Anchor,
    pub callback_path: String,
    pub callback_type: String,
    pub missing_effects: Vec<String>,
    pub extern_effects: Vec<String>,
    /// Effect/condition pairs retain which symbolic requirement belongs to
    /// which effect after several callback result arrows share one path.
    pub requirements: Vec<ExternCallbackRequirement>,
    /// Every open callback remainder not covered by the extern remainder.
    pub missing_tails: Vec<String>,
    /// Every distinct source-level implication the declaration fails to
    /// guarantee at this callback path.
    pub conditions: Vec<String>,
    /// Conjunction of this issue's conditions. Retained as the compatibility
    /// scalar for structured consumers; new presentation uses `requirements`.
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternCallbackRequirement {
    pub effect: String,
    /// `None` means the effect is required unconditionally.
    pub condition: Option<String>,
}

#[derive(Clone, Copy)]
struct ExternPath(usize);

struct ExternPathNode {
    parent: Option<usize>,
    segment: String,
}

fn root_path(paths: &mut Vec<ExternPathNode>) -> ExternPath {
    paths.push(ExternPathNode {
        parent: None,
        segment: "extern".into(),
    });
    ExternPath(0)
}

fn child_path(
    paths: &mut Vec<ExternPathNode>,
    parent: ExternPath,
    segment: impl Into<String>,
) -> ExternPath {
    let at = paths.len();
    paths.push(ExternPathNode {
        parent: Some(parent.0),
        segment: segment.into(),
    });
    ExternPath(at)
}

fn render_path(paths: &[ExternPathNode], path: ExternPath) -> String {
    let mut parts = Vec::new();
    let mut cursor = Some(path.0);
    while let Some(at) = cursor {
        parts.push(paths[at].segment.as_str());
        cursor = paths[at].parent;
    }
    parts.reverse();
    parts.join(" ")
}

fn render_child_path(
    paths: &mut Vec<ExternPathNode>,
    parent: ExternPath,
    segment: impl Into<String>,
) -> String {
    let child = child_path(paths, parent, segment);
    render_path(paths, child)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ExternVariableIdentity {
    TypeVar(TyVar),
    TypeRigid(u32),
    TypeBound(u32),
    RowVar(TyVar),
    RowRigid(u32),
    RowBound(u32),
    PresenceVar(TyVar),
    PresenceBound(u32),
}

#[derive(Debug, Clone)]
struct ExternVariableSource {
    span: Anchor,
    name: String,
    hidden: bool,
}

/// Identity-indexed source provenance for one annotation boundary. Alias
/// expansion substitutes the same semantic identities into local and imported
/// bodies (including `Rest::More` row splices), so this map remains exact where
/// rescanning an alias application cannot possibly recover the originating
/// quantifier.
#[derive(Debug, Clone, Default)]
struct ExternSourceMap {
    variables: HashMap<ExternVariableIdentity, ExternVariableSource>,
}

impl ExternSourceMap {
    fn insert(&mut self, identity: ExternVariableIdentity, span: Anchor, name: impl Into<String>) {
        self.insert_with_ownership(identity, span, name, false);
    }

    fn insert_with_ownership(
        &mut self,
        identity: ExternVariableIdentity,
        span: Anchor,
        name: impl Into<String>,
        hidden: bool,
    ) {
        self.variables
            .entry(identity)
            .or_insert(ExternVariableSource {
                span,
                name: name.into(),
                hidden,
            });
    }

    fn get(&self, identity: ExternVariableIdentity) -> Option<&ExternVariableSource> {
        self.variables.get(&identity)
    }
}

fn type_variable_identity(ty: &Ty) -> Option<ExternVariableIdentity> {
    match ty {
        Ty::Var(var) => Some(ExternVariableIdentity::TypeVar(*var)),
        Ty::Rigid { id, .. } => Some(ExternVariableIdentity::TypeRigid(*id)),
        Ty::Bound(index) => Some(ExternVariableIdentity::TypeBound(*index)),
        _ => None,
    }
}

fn row_variable_identity(rest: &Rest) -> Option<ExternVariableIdentity> {
    match rest {
        Rest::Var(var) => Some(ExternVariableIdentity::RowVar(*var)),
        Rest::Rigid { id, .. } => Some(ExternVariableIdentity::RowRigid(*id)),
        Rest::Bound(index) => Some(ExternVariableIdentity::RowBound(*index)),
        Rest::Closed | Rest::Undecided | Rest::More(_) => None,
    }
}

fn presence_variable_identity(presence: &Presence) -> Option<ExternVariableIdentity> {
    match presence {
        Presence::Var(var) => Some(ExternVariableIdentity::PresenceVar(*var)),
        Presence::Bound(index) => Some(ExternVariableIdentity::PresenceBound(*index)),
        Presence::Present | Presence::Absent | Presence::Recovered(_) | Presence::Undecided => None,
    }
}

fn flatten_row(row: &Row) -> Row {
    let mut labels = IndexMap::new();
    let mut segment = row;
    loop {
        for (name, field) in &segment.labels {
            labels.entry(name.clone()).or_insert_with(|| field.clone());
        }
        match &segment.rest {
            Rest::More(more) => segment = more,
            rest => {
                return Row {
                    labels,
                    rest: rest.clone(),
                };
            }
        }
    }
}

fn presence_formula(presence: &Presence) -> Option<Formula> {
    match presence {
        Presence::Present => Some(Formula::True),
        Presence::Absent => Some(Formula::False),
        Presence::Var(var) => Some(Formula::Atom(Atom::Var(*var))),
        Presence::Bound(bound) => Some(Formula::Atom(Atom::Bound(*bound))),
        Presence::Recovered(_) | Presence::Undecided => None,
    }
}

fn callback_rows(aliases: &IndexMap<Symbol, Scheme>, ty: &Arc<Ty>) -> Vec<Row> {
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
        let exposed = expose_packages(aliases, &cursor);
        let Ty::Arrow(_, to, row) = &*exposed else {
            break;
        };
        rows.push(row.clone());
        cursor = to.clone();
    }
    rows
}

fn callback_tail_spelling(rest: &Rest, sources: &ExternSourceMap) -> Option<String> {
    if matches!(rest, Rest::Closed | Rest::Undecided) {
        return None;
    }
    if let Some(source) = row_variable_identity(rest).and_then(|identity| sources.get(identity)) {
        return Some(match source.name.as_str() {
            "generic row" => "`..`".into(),
            name => format!("`..{name}`"),
        });
    }
    Some(match rest {
        Rest::Var(var) => format!("open callback tail `?{var}`"),
        Rest::Bound(index) => format!("open callback tail `bound {index}`"),
        Rest::Rigid { name, .. } => format!("`..'{name}`"),
        Rest::More(_) => unreachable!("callback rows are flattened"),
        Rest::Closed | Rest::Undecided => return None,
    })
}

fn callback_effect_conditions(required: &Row, available: &Row) -> Vec<CallbackEffectCondition> {
    required
        .labels
        .iter()
        .map(|(name, required)| {
            let required = presence_formula(&required.presence).unwrap_or(Formula::True);
            let available = match available.labels.get(name) {
                Some(field) => presence_formula(&field.presence).unwrap_or(Formula::True),
                // A flexible row hole can acquire this exact conditional
                // label. Fixed abstract tails cannot: their lacks promise
                // excludes every label written beside them.
                None if matches!(available.rest, Rest::Var(_) | Rest::Undecided) => Formula::True,
                None => Formula::False,
            };
            CallbackEffectCondition {
                effect: name.clone(),
                condition: required.not().or(available),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn push_callback_coverage(
    aliases: &IndexMap<Symbol, Scheme>,
    callback: &Arc<Ty>,
    available: &Row,
    sources: &ExternSourceMap,
    callback_span: Anchor,
    callback_path: String,
    extern_name: &str,
    extern_span: Anchor,
    capability_span: Anchor,
    out: &mut Vec<Constraint>,
) {
    let available = flatten_row(available);
    for required in callback_rows(aliases, callback) {
        let required = flatten_row(&required);
        let effects = callback_effect_conditions(&required, &available);
        let condition = Formula::all(effects.iter().map(|effect| effect.condition.clone()));
        let tail =
            callback_tail_spelling(&required.rest, sources).map(|required| CallbackTailRelation {
                required,
                available: callback_tail_spelling(&available.rest, sources),
            });
        let boundary = CallbackBoundary {
            callback_at: callback_span,
            callback_path: callback_path.clone(),
            callback_type: callback.to_string(),
            effects,
            condition,
            tail,
            extern_name: extern_name.into(),
            extern_at: extern_span,
            capability_at: capability_span,
        };
        out.push(Constraint {
            id: ConstraintId::pending(),
            reason: ReasonId::pending(),
            at: callback_span,
            origin: ConstraintOrigin::CallbackBoundary,
            subjects: ConstraintSubjects::pair(
                Subject::CallbackRequired,
                Subject::CallbackAvailable,
            ),
            kind: ConstraintKind::CallbackCoverage {
                required,
                available: available.clone(),
                boundary,
            },
        });
    }
}

#[derive(Default)]
struct ExternBoundaryReview {
    leaves: Vec<PolymorphicExternLeaf>,
    coverage: Vec<Constraint>,
}

/// Review one extern's semantic boundary once. The ABI contributes only host
/// grouping and source spans; the lowered type supplies representation leaves,
/// callback formulas, and composed rows. Exact lowered identities recover the
/// source variable which survived alias substitution and row expansion.
fn extern_boundary_review(
    aliases: &IndexMap<Symbol, Scheme>,
    root_abi: &ir::ExternType,
    root_ty: &Arc<Ty>,
    sources: &ExternSourceMap,
    extern_name: &str,
    extern_span: Anchor,
) -> ExternBoundaryReview {
    // This is intentionally an explicit work list. Extern annotations are user
    // input and generated annotations can contain tens of thousands of nested
    // arrows; diagnostics must not consume the Rust call stack.
    let mut review = ExternBoundaryReview::default();
    let mut paths = Vec::new();
    let root = root_path(&mut paths);
    let mut work = vec![(root_abi, root_ty.clone(), root, Vec::<Arc<Ty>>::new(), true)];
    let mut seen: HashSet<(*const ir::ExternType, *const Ty)> = HashSet::new();
    while let Some((abi, mut ty, path, mut active_aliases, mut report_row_leaves)) = work.pop() {
        // A package fixes the representation choices it encloses, but callbacks
        // inside it still cross the host boundary and need effect coverage.
        while let Ty::Package(inner) = &*ty {
            report_row_leaves = false;
            ty = inner.clone();
        }
        let original = ty.clone();
        // Lowered semantic types are DAGs. An invariant alias reuses the same
        // argument for both sides of every arrow, so walking occurrences rather
        // than nodes turns depth N into 2^N work. The ABI node is part of the
        // key so distinct written parameters still retain distinct paths.
        if !seen.insert((abi as *const ir::ExternType, Arc::as_ptr(&original))) {
            continue;
        }
        // Transparent ABI wrappers have not traversed the alias yet. Recording
        // it here would make the inner node look like a recursive occurrence
        // and skip its representation and callback-effect checks.
        if matches!(&*original, Ty::Named { .. })
            && !matches!(
                &abi.anchored,
                ir::ExternTypeKind::Group(_) | ir::ExternTypeKind::Annotated { .. }
            )
        {
            if active_aliases
                .iter()
                .any(|prior| same_finite_syntax(prior, &original))
            {
                continue;
            }
            active_aliases.push(original.clone());
        }

        match &abi.anchored {
            ir::ExternTypeKind::Group(inner) | ir::ExternTypeKind::Annotated { inner, .. } => {
                work.push((inner, ty, path, active_aliases, report_row_leaves))
            }
            ir::ExternTypeKind::Function {
                parameters, result, ..
            } => {
                let mut cursor = ty;
                let mut children = Vec::new();
                let mut available = Row::closed();
                for at in 0..parameters.len().max(1) {
                    let exposed = expose_packages(aliases, &cursor);
                    let Ty::Arrow(from, to, row) = &*exposed else {
                        break;
                    };
                    if let Some(parameter) = parameters.get(at) {
                        children.push((
                            parameter,
                            from.clone(),
                            child_path(&mut paths, path, format!("parameter {}", at + 1)),
                            active_aliases.clone(),
                        ));
                    }
                    available = row.clone();
                    cursor = to.clone();
                }

                // Preserve source callback order in the boundary evidence. The
                // traversal stack below is reversed separately so leaves also
                // remain in source/path order.
                for (parameter, input, child, _) in &children {
                    if matches!(&*expose_packages(aliases, input), Ty::Arrow(..)) {
                        push_callback_coverage(
                            aliases,
                            input,
                            &available,
                            sources,
                            parameter.at,
                            render_path(&paths, *child),
                            extern_name,
                            extern_span,
                            abi.at,
                            &mut review.coverage,
                        );
                    }
                }

                work.push((
                    result,
                    cursor,
                    child_path(&mut paths, path, "result"),
                    active_aliases,
                    report_row_leaves,
                ));
                work.extend(
                    children.into_iter().rev().map(|(abi, ty, path, aliases)| {
                        (abi, ty, path, aliases, report_row_leaves)
                    }),
                );
            }
            ir::ExternTypeKind::Ordinary(written) => {
                // An alias may expose an existential package even when the
                // work item itself is still nominal. Preserve that boundary
                // instead of looking through its hidden witness.
                let unfolded = unfold(aliases, &original);
                if matches!(&*unfolded, Ty::Package(_)) {
                    continue;
                }
                let exposed = expose_packages(aliases, &unfolded);
                match &*exposed {
                    Ty::Bound(_) | Ty::Var(_) | Ty::Rigid { .. } => {
                        let source = type_variable_identity(&exposed)
                            .and_then(|identity| sources.get(identity));
                        if let Some(source) = source.filter(|source| !source.hidden) {
                            review.leaves.push(PolymorphicExternLeaf {
                                at: source.span,
                                variable: source.name.clone(),
                                kind: ExternVariableKind::Type,
                                position: render_path(&paths, path),
                            });
                        }
                    }
                    Ty::Arrow(from, to, available) => {
                        let callback_path = child_path(&mut paths, path, "callback input");
                        if matches!(&*expose_packages(aliases, from), Ty::Arrow(..)) {
                            push_callback_coverage(
                                aliases,
                                from,
                                available,
                                sources,
                                abi.at,
                                render_path(&paths, callback_path),
                                extern_name,
                                extern_span,
                                abi.at,
                                &mut review.coverage,
                            );
                        }
                        // Effect-row variables are evidence relations rather
                        // than runtime representation choices. Coverage above
                        // retains their exact tails and label formulas.
                        // With legacy ordinary-arrow spelling, the reused ABI
                        // node cannot identify a nested row token. Keep its
                        // established representation behavior instead of
                        // attributing an unrelated outer presence or tail.
                        let child_row_leaves = report_row_leaves
                            && !matches!(written.anchored, ir::TypeKind::Arrow { .. });
                        work.push((
                            abi,
                            to.clone(),
                            child_path(&mut paths, path, "callback result"),
                            active_aliases.clone(),
                            child_row_leaves,
                        ));
                        work.push((
                            abi,
                            from.clone(),
                            callback_path,
                            active_aliases,
                            child_row_leaves,
                        ));
                    }
                    Ty::Struct(semantic_row) | Ty::Sum(semantic_row) => {
                        let row = flatten_row(semantic_row);
                        if report_row_leaves
                            && let Some(source) = row_variable_identity(&row.rest)
                                .and_then(|identity| sources.get(identity))
                                .filter(|source| !source.hidden)
                        {
                            review.leaves.push(PolymorphicExternLeaf {
                                at: source.span,
                                variable: source.name.clone(),
                                kind: ExternVariableKind::Row,
                                position: render_child_path(&mut paths, path, "row tail"),
                            });
                        }
                        for (label, field) in row.labels.iter().rev() {
                            if report_row_leaves
                                && let Some(source) = presence_variable_identity(&field.presence)
                                    .and_then(|identity| sources.get(identity))
                                    .filter(|source| !source.hidden)
                            {
                                review.leaves.push(PolymorphicExternLeaf {
                                    at: source.span,
                                    variable: source.name.clone(),
                                    kind: ExternVariableKind::Presence,
                                    position: render_child_path(
                                        &mut paths,
                                        path,
                                        format!("`{label}` presence"),
                                    ),
                                });
                            }
                            // Payload polymorphism has a fixed outer host
                            // representation supplied by the surrounding row;
                            // only the row's own tail/presence can vary it.
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    review
}

#[cfg(test)]
mod extern_boundary_depth_tests {
    use super::*;
    use crate::{tracking::Anchored, types::Prim};

    #[test]
    fn thirty_thousand_arrow_boundary_walk_is_stack_safe() {
        let span = Anchor::GENERATED;
        let written = Anchored {
            anchored: ir::TypeKind::Prim(Prim::Nat),
            at: span,
        };
        let abi = Anchored {
            anchored: ir::ExternTypeKind::Ordinary(written),
            at: span,
        };
        let mut ty = Arc::new(Ty::Nat);
        for _ in 0..30_000 {
            ty = Arc::new(Ty::Arrow(Arc::new(Ty::Nat), ty, Row::closed()));
        }
        let aliases = IndexMap::new();
        let sources = ExternSourceMap::default();
        let review = extern_boundary_review(&aliases, &abi, &ty, &sources, "deep", span);
        assert!(review.leaves.is_empty());
        assert!(review.coverage.is_empty());
        // A 30k recursively represented test value also needs iterative
        // destruction; leaking this one synthetic value keeps the test focused
        // on the boundary walkers rather than `Arc<Ty>`'s destructor.
        std::mem::forget(ty);
    }
}

fn describe_type(ty: &Arc<Ty>) -> TypeDescription {
    let mut ty = ty;
    while let Ty::Package(inner) = &**ty {
        ty = inner;
    }
    match &**ty {
        Ty::Nat => TypeDescription::NaturalNumber,
        Ty::Int => TypeDescription::Integer,
        Ty::Fixed(kind) => {
            if kind.signed() {
                TypeDescription::Integer
            } else {
                TypeDescription::NaturalNumber
            }
        }
        Ty::Real => TypeDescription::RealNumber,
        Ty::String => TypeDescription::Text,
        Ty::Bool => TypeDescription::Bool,
        Ty::Any => TypeDescription::Any,
        Ty::ForeignValue => TypeDescription::ForeignValue,
        Ty::Arrow(..) => TypeDescription::Function,
        Ty::Struct(..) => TypeDescription::Struct,
        Ty::Sum(..) => TypeDescription::TaggedValue,
        Ty::Array(..) => TypeDescription::Array,
        Ty::Mut(..) => TypeDescription::Mut,
        Ty::Named { .. } => TypeDescription::DeclaredType,
        Ty::Bound(_) | Ty::Var(_) | Ty::Rigid { .. } | Ty::Undecided => TypeDescription::Undecided,
        Ty::Package(_) => unreachable!("packages were removed iteratively"),
    }
}

struct MismatchWork {
    left: Arc<Ty>,
    right: Arc<Ty>,
    alias_path: Option<usize>,
    alias_work: usize,
    root: bool,
}

struct AliasGoal {
    left_symbol: Symbol,
    left_args: Arc<[Arc<Ty>]>,
    right_symbol: Symbol,
    right_args: Arc<[Arc<Ty>]>,
    parent: Option<usize>,
    depth: usize,
    fingerprint: u64,
}

/// Memoized, stack-safe hashes used only to reject unequal alias goals cheaply.
/// Hash matches are still checked with metered exact finite-syntax equality, so
/// collisions can cost bounded work but cannot alter a diagnostic.
struct MismatchFingerprints {
    types: HashMap<*const Ty, (Arc<Ty>, u64)>,
    argument_hash_mask: u64,
}

impl Default for MismatchFingerprints {
    fn default() -> Self {
        Self {
            types: HashMap::new(),
            argument_hash_mask: u64::MAX,
        }
    }
}

enum MismatchFingerprintWork {
    Type(Arc<Ty>),
    Finish(*const Ty, Arc<Ty>, u8, usize, Option<Symbol>),
    Row(Row),
    FinishRow(Vec<(String, Presence, bool)>, Rest, usize),
}

impl MismatchFingerprints {
    fn arguments(&mut self, args: &[Arc<Ty>], work_left: &mut usize) -> Option<u64> {
        fn tagged(tag: u8, parts: impl IntoIterator<Item = u64>) -> u64 {
            let mut hash = DefaultHasher::new();
            tag.hash(&mut hash);
            for part in parts {
                part.hash(&mut hash);
            }
            hash.finish()
        }
        fn presence_hash(p: &Presence) -> u64 {
            match p {
                Presence::Present => tagged(0, []),
                Presence::Absent => tagged(1, []),
                Presence::Undecided => tagged(2, []),
                Presence::Var(id) => tagged(3, [u64::from(*id)]),
                Presence::Bound(id) => tagged(4, [u64::from(*id)]),
                Presence::Recovered(id) => tagged(5, [u64::from(*id)]),
            }
        }
        fn rest_leaf_hash(rest: &Rest) -> Option<u64> {
            Some(match rest {
                Rest::Closed => tagged(0, []),
                Rest::Undecided => tagged(1, []),
                Rest::Var(id) => tagged(2, [u64::from(*id)]),
                Rest::Bound(id) => tagged(3, [u64::from(*id)]),
                Rest::Rigid { id, .. } => tagged(4, [u64::from(*id)]),
                Rest::More(_) => return None,
            })
        }

        let mut pending: Vec<_> = args
            .iter()
            .rev()
            .cloned()
            .map(MismatchFingerprintWork::Type)
            .collect();
        let mut values = Vec::new();
        while let Some(next) = pending.pop() {
            if *work_left == 0 {
                return None;
            }
            *work_left -= 1;
            match next {
                MismatchFingerprintWork::Type(ty) => {
                    let key = Arc::as_ptr(&ty);
                    if let Some((_, hash)) = self.types.get(&key) {
                        values.push(*hash);
                        continue;
                    }
                    match &*ty {
                        Ty::Nat => values.push(tagged(0, [])),
                        Ty::Int => values.push(tagged(1, [])),
                        Ty::Fixed(kind) => values.push(tagged(30, [*kind as u64])),
                        Ty::Real => values.push(tagged(2, [])),
                        Ty::String => values.push(tagged(3, [])),
                        Ty::Bool => values.push(tagged(4, [])),
                        Ty::Any => values.push(tagged(50, [])),
                        Ty::ForeignValue => values.push(tagged(51, [])),
                        Ty::Var(id) => values.push(tagged(5, [u64::from(*id)])),
                        Ty::Bound(id) => values.push(tagged(6, [u64::from(*id)])),
                        Ty::Rigid { id, .. } => values.push(tagged(7, [u64::from(*id)])),
                        Ty::Undecided => values.push(tagged(8, [])),
                        Ty::Arrow(from, to, row) => {
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                9,
                                3,
                                None,
                            ));
                            pending.push(MismatchFingerprintWork::Row(row.clone()));
                            pending.push(MismatchFingerprintWork::Type(to.clone()));
                            pending.push(MismatchFingerprintWork::Type(from.clone()));
                            continue;
                        }
                        Ty::Package(inner) => {
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                10,
                                1,
                                None,
                            ));
                            pending.push(MismatchFingerprintWork::Type(inner.clone()));
                            continue;
                        }
                        Ty::Mut(region, element) => {
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                15,
                                2,
                                None,
                            ));
                            pending.push(MismatchFingerprintWork::Type(element.clone()));
                            pending.push(MismatchFingerprintWork::Type(region.clone()));
                            continue;
                        }
                        Ty::Array(element) => {
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                14,
                                1,
                                None,
                            ));
                            pending.push(MismatchFingerprintWork::Type(element.clone()));
                            continue;
                        }
                        Ty::Struct(row) | Ty::Sum(row) => {
                            let tag = if matches!(&*ty, Ty::Struct(_)) {
                                11
                            } else {
                                12
                            };
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                tag,
                                1,
                                None,
                            ));
                            pending.push(MismatchFingerprintWork::Row(row.clone()));
                            continue;
                        }
                        Ty::Named { symbol, args, .. } => {
                            pending.push(MismatchFingerprintWork::Finish(
                                key,
                                ty.clone(),
                                13,
                                args.len(),
                                Some(*symbol),
                            ));
                            pending.extend(
                                args.iter()
                                    .rev()
                                    .cloned()
                                    .map(MismatchFingerprintWork::Type),
                            );
                            continue;
                        }
                    }
                    self.types.insert(key, (ty, *values.last().unwrap()));
                }
                MismatchFingerprintWork::Finish(key, ty, tag, count, symbol) => {
                    let parts = values.split_off(values.len() - count);
                    let mut hash = DefaultHasher::new();
                    tag.hash(&mut hash);
                    symbol.hash(&mut hash);
                    parts.hash(&mut hash);
                    let hash = hash.finish();
                    self.types.insert(key, (ty, hash));
                    values.push(hash);
                }
                MismatchFingerprintWork::Row(row) => {
                    // Flatten composed rows exactly as solver canon does: an
                    // outer spelling wins over a duplicate in its tail.
                    let mut flattened = IndexMap::new();
                    let mut segment = &row;
                    let rest = loop {
                        for (name, field) in &segment.labels {
                            if *work_left == 0 {
                                return None;
                            }
                            *work_left -= 1;
                            flattened
                                .entry(name.clone())
                                .or_insert_with(|| field.clone());
                        }
                        match &segment.rest {
                            Rest::More(more) => segment = more,
                            rest => break rest.clone(),
                        }
                    };
                    let mut fields: Vec<_> = flattened
                        .into_iter()
                        .map(|(name, field)| {
                            let payload = matches!(field.presence, Presence::Present);
                            (name, field.presence, payload, field.ty)
                        })
                        .collect();
                    // Charge each comparison performed by the sort, not merely
                    // the eventual label hashes.
                    let mut exhausted = false;
                    fields.sort_unstable_by(|left, right| {
                        if let Some(next) = work_left.checked_sub(1) {
                            *work_left = next;
                        } else {
                            exhausted = true;
                        }
                        left.0.cmp(&right.0)
                    });
                    if exhausted {
                        return None;
                    }
                    let payloads = fields.iter().filter(|field| field.2).count();
                    let completion = fields
                        .iter()
                        .map(|(n, p, has, _)| (n.clone(), p.clone(), *has))
                        .collect();
                    pending.push(MismatchFingerprintWork::FinishRow(
                        completion, rest, payloads,
                    ));
                    pending.extend(
                        fields
                            .into_iter()
                            .rev()
                            .filter(|field| field.2)
                            .map(|field| MismatchFingerprintWork::Type(field.3)),
                    );
                }
                MismatchFingerprintWork::FinishRow(fields, rest, payloads) => {
                    let mut parts = values.split_off(values.len() - payloads).into_iter();
                    let mut hash = DefaultHasher::new();
                    fields.len().hash(&mut hash);
                    for (name, presence, payload) in fields {
                        if *work_left == 0 {
                            return None;
                        }
                        *work_left -= 1;
                        name.hash(&mut hash);
                        presence_hash(&presence).hash(&mut hash);
                        if payload {
                            parts.next().unwrap().hash(&mut hash);
                        }
                    }
                    rest_leaf_hash(&rest).unwrap().hash(&mut hash);
                    values.push(hash.finish());
                }
            }
        }
        let parts = values.split_off(values.len() - args.len());
        Some(tagged(14, parts) & self.argument_hash_mask)
    }
}

/// Pick the first incompatible semantic leaf on an explicit stack. The walk
/// follows every payload-bearing type position and unfolds declarations one
/// layer at a time; malformed/missing alias provenance is reported honestly as
/// a declared or undecided type rather than guessed from solver variables.
fn smallest_incompatible(
    aliases: &IndexMap<Symbol, Scheme>,
    left: &Arc<Ty>,
    right: &Arc<Ty>,
) -> (TypeDescription, TypeDescription) {
    let mut operations = 0;
    smallest_incompatible_counted(aliases, left, right, &mut operations)
}

fn smallest_incompatible_counted(
    aliases: &IndexMap<Symbol, Scheme>,
    left: &Arc<Ty>,
    right: &Arc<Ty>,
    operations: &mut usize,
) -> (TypeDescription, TypeDescription) {
    smallest_incompatible_counted_with_mask(aliases, left, right, operations, u64::MAX)
}

fn smallest_incompatible_counted_with_mask(
    aliases: &IndexMap<Symbol, Scheme>,
    left: &Arc<Ty>,
    right: &Arc<Ty>,
    operations: &mut usize,
    argument_hash_mask: u64,
) -> (TypeDescription, TypeDescription) {
    const MAX_ALIAS_WORK: usize = 1_048_576;
    let mut work = vec![MismatchWork {
        left: left.clone(),
        right: right.clone(),
        alias_path: None,
        alias_work: MAX_ALIAS_WORK,
        root: true,
    }];
    let fallback = (describe_type(left), describe_type(right));
    let mut unfolded_fallback = None;
    let mut incomplete_alias_walk = false;
    // Recursive declarations return to the same applications. Arguments are
    // part of the goal: the same pair of names can occur at independent
    // substitutions in sibling fields, and suppressing one must not suppress
    // the other. Keep the finite syntax itself because opening can rebuild a
    // semantically identical substitution at a different allocation. Arena
    // indices make paths persistent without recursive destruction or cloning.
    let mut alias_goals: Vec<AliasGoal> = Vec::new();
    let mut forwarding = Forwarding::default();
    let mut fingerprints = MismatchFingerprints {
        argument_hash_mask,
        ..MismatchFingerprints::default()
    };
    // Malformed imported declarations can manufacture fresh, ever larger
    // substitutions. Charge fingerprint nodes and ancestor probes to a strict
    // branch-local budget; queued siblings retain the budget at their fork.
    while let Some(MismatchWork {
        mut left,
        mut right,
        mut alias_path,
        mut alias_work,
        root,
    }) = work.pop()
    {
        while let Ty::Package(inner) = &*left {
            left = inner.clone();
        }
        while let Ty::Package(inner) = &*right {
            right = inner.clone();
        }

        // Names are source spelling, not semantic leaves. Only unfold when the
        // published declaration is actually available; imported recovery holes
        // retain the honest `DeclaredType` fallback. Record a pair before
        // unfolding it so a recursive field is skipped without abandoning the
        // other fields already on the explicit DFS stack.
        if let (
            Ty::Named {
                symbol: left_symbol,
                args: left_args,
                ..
            },
            Ty::Named {
                symbol: right_symbol,
                args: right_args,
                ..
            },
        ) = (&*left, &*right)
            && aliases.contains_key(left_symbol)
            && aliases.contains_key(right_symbol)
        {
            // Forwarding spellings do not change a substitution. Without
            // removing them, `Stream (Id 'a)` appears successively as
            // `Stream (Id 'a)`, `Stream (Id (Id 'a))`, ... and defeats the
            // coinductive key. The forwarding classifier is the same semantic
            // machinery used by alias opening; it is iterative and also marks
            // mutually-forwarding recovery cycles.
            let left_args: Arc<[Arc<Ty>]> = left_args
                .iter()
                .map(|arg| canonical_alias_argument(aliases, &mut forwarding, arg))
                .collect::<Vec<_>>()
                .into();
            let right_args: Arc<[Arc<Ty>]> = right_args
                .iter()
                .map(|arg| canonical_alias_argument(aliases, &mut forwarding, arg))
                .collect::<Vec<_>>()
                .into();
            let Some(left_fingerprint) = fingerprints.arguments(&left_args, &mut alias_work) else {
                *operations += MAX_ALIAS_WORK - alias_work;
                incomplete_alias_walk = true;
                continue;
            };
            let Some(right_fingerprint) = fingerprints.arguments(&right_args, &mut alias_work)
            else {
                *operations += MAX_ALIAS_WORK - alias_work;
                incomplete_alias_walk = true;
                continue;
            };
            let mut goal_hash = DefaultHasher::new();
            left_symbol.hash(&mut goal_hash);
            right_symbol.hash(&mut goal_hash);
            left_fingerprint.hash(&mut goal_hash);
            right_fingerprint.hash(&mut goal_hash);
            let fingerprint = goal_hash.finish();
            let mut ancestor = alias_path;
            let mut seen = false;
            while let Some(index) = ancestor {
                if alias_work == 0 {
                    break;
                }
                // The ancestor probe itself and every exact collision-check
                // node share one allowance. A hostile hash collision therefore
                // cannot smuggle an unbounded structural walk past the meter.
                alias_work -= 1;
                let goal = &alias_goals[index];
                let mut same_arguments = |known: &[Arc<Ty>], current: &[Arc<Ty>]| {
                    if known.len() != current.len() {
                        return Some(false);
                    }
                    for (known, current) in known.iter().zip(current) {
                        if !same_finite_syntax_metered(known, current, &mut alias_work)? {
                            return Some(false);
                        }
                    }
                    Some(true)
                };
                if goal.fingerprint == fingerprint
                    && goal.left_symbol == *left_symbol
                    && goal.right_symbol == *right_symbol
                    && same_arguments(&goal.left_args, &left_args) == Some(true)
                    && same_arguments(&goal.right_args, &right_args) == Some(true)
                {
                    seen = true;
                    break;
                }
                ancestor = goal.parent;
            }
            let depth = alias_path.map_or(0, |index| alias_goals[index].depth);
            if seen || alias_work == 0 {
                *operations += MAX_ALIAS_WORK - alias_work;
                incomplete_alias_walk = true;
                continue;
            }
            alias_goals.push(AliasGoal {
                left_symbol: *left_symbol,
                left_args,
                right_symbol: *right_symbol,
                right_args,
                parent: alias_path,
                depth: depth + 1,
                fingerprint,
            });
            alias_path = Some(alias_goals.len() - 1);
        }
        if matches!(&*left, Ty::Named { symbol, .. } if aliases.contains_key(symbol)) {
            left = unfold(aliases, &left);
        }
        if matches!(&*right, Ty::Named { symbol, .. } if aliases.contains_key(symbol)) {
            right = unfold(aliases, &right);
        }
        // An alias may publish a packaged container. Packages are transparent
        // to mismatch structure, including when they only become visible after
        // opening the declaration.
        while let Ty::Package(inner) = &*left {
            left = inner.clone();
        }
        while let Ty::Package(inner) = &*right {
            right = inner.clone();
        }

        let descriptions = (describe_type(&left), describe_type(&right));
        // The input pair is known to be incompatible. Once its declarations
        // have been opened, their semantic container is the honest fallback if
        // payload walking finds no narrower leaf (for example, an arrow whose
        // only disagreement is its effect row). Child jobs are not themselves
        // known to be incompatible, so they must not replace this root fallback
        // merely because an equal child also happens to be an alias.
        if root
            && matches!(
                descriptions,
                (
                    TypeDescription::Function
                        | TypeDescription::Array
                        | TypeDescription::Struct
                        | TypeDescription::TaggedValue,
                    TypeDescription::Function
                        | TypeDescription::Array
                        | TypeDescription::Struct
                        | TypeDescription::TaggedValue
                )
            )
        {
            unfolded_fallback = Some(descriptions);
        }
        // A name left here has no declaration we can honestly inspect. Even
        // when both sides spell the same name, its arguments are not known to
        // be the declaration's semantic parameters, so they cannot supply a
        // more specific mismatch.
        if matches!(&*left, Ty::Named { .. }) || matches!(&*right, Ty::Named { .. }) {
            return descriptions;
        }
        match (&*left, &*right) {
            (Ty::Array(left), Ty::Array(right)) => work.push(MismatchWork {
                left: left.clone(),
                right: right.clone(),
                alias_path,
                alias_work,
                root: false,
            }),
            (Ty::Arrow(l_from, l_to, l_effects), Ty::Arrow(r_from, r_to, r_effects)) => {
                // An incompatible effect row contributes no payload jobs. Keep
                // walking parameter and result first; if neither has a leaf,
                // the enclosing function fallback remains the honest answer.
                let mut effect_alias_work = alias_work;
                push_row_payloads(
                    &mut work,
                    l_effects,
                    r_effects,
                    alias_path,
                    &mut effect_alias_work,
                );
                // Source order: parameter, result, then effects. Effect-row
                // work is its own queued branch and cannot starve these siblings.
                work.push(MismatchWork {
                    left: l_to.clone(),
                    right: r_to.clone(),
                    alias_path,
                    alias_work,
                    root: false,
                });
                work.push(MismatchWork {
                    left: l_from.clone(),
                    right: r_from.clone(),
                    alias_path,
                    alias_work,
                    root: false,
                });
            }
            (Ty::Struct(left), Ty::Struct(right)) | (Ty::Sum(left), Ty::Sum(right)) => {
                if !push_row_payloads(&mut work, left, right, alias_path, &mut alias_work) {
                    return descriptions;
                }
            }
            (Ty::Fixed(left), Ty::Fixed(right)) if left == right => {}
            (Ty::Nat, Ty::Nat)
            | (Ty::Int, Ty::Int)
            | (Ty::Real, Ty::Real)
            | (Ty::String, Ty::String)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Any, Ty::Any)
            | (Ty::ForeignValue, Ty::ForeignValue)
            | (Ty::Bound(_), Ty::Bound(_))
            | (Ty::Var(_), Ty::Var(_))
            | (Ty::Rigid { .. }, Ty::Rigid { .. })
            | (Ty::Undecided, Ty::Undecided) => {}
            _ => return descriptions,
        }
    }
    if incomplete_alias_walk {
        fallback
    } else {
        unfolded_fallback.unwrap_or(fallback)
    }
}

/// Remove head aliases that only select one of their arguments. This consumes
/// the finite argument syntax rather than rebuilding it. A forwarding cycle
/// has no observable body in recovery input, so all its applications share the
/// same undecided canonical form regardless of the ignored arguments.
fn canonical_alias_argument(
    aliases: &IndexMap<Symbol, Scheme>,
    forwarding: &mut Forwarding,
    argument: &Arc<Ty>,
) -> Arc<Ty> {
    let mut argument = argument.clone();
    loop {
        let Ty::Named { symbol, args, .. } = &*argument else {
            return argument;
        };
        let Some(scheme) = aliases.get(symbol) else {
            return argument;
        };
        match forwarding.projection(aliases, scheme.body()) {
            Some(index) => match args.get(index as usize) {
                Some(selected) => argument = selected.clone(),
                None => return Arc::new(Ty::Undecided),
            },
            None if forwarding.cycles.contains(symbol) => return Arc::new(Ty::Undecided),
            None => return argument,
        }
    }
}

fn push_row_payloads(
    work: &mut Vec<MismatchWork>,
    left: &Row,
    right: &Row,
    alias_path: Option<usize>,
    alias_work: &mut usize,
) -> bool {
    fn flatten<'a>(
        mut row: &'a Row,
        work_left: &mut usize,
    ) -> Option<(IndexMap<&'a str, &'a RowField>, &'a Rest)> {
        let mut labels = IndexMap::new();
        loop {
            for (name, field) in &row.labels {
                *work_left = work_left.checked_sub(1)?;
                labels.entry(name.as_str()).or_insert(field);
            }
            match &row.rest {
                Rest::More(more) => row = more,
                rest => return Some((labels, rest)),
            }
        }
    }

    // Compare canonical flattened maps rather than segment boundaries. This is
    // solver canon's outer-label precedence and makes a segmented row equal to
    // the same finite row represented in one segment.
    let Some((left_labels, left_rest)) = flatten(left, alias_work) else {
        return false;
    };
    let Some((right_labels, right_rest)) = flatten(right, alias_work) else {
        return false;
    };
    if left_labels.len() != right_labels.len() {
        return false;
    }
    let mut payloads = Vec::new();
    for (name, left_field) in left_labels {
        let Some(next) = alias_work.checked_sub(1) else {
            return false;
        };
        *alias_work = next;
        let Some(right_field) = right_labels.get(name) else {
            return false;
        };
        if left_field.presence != right_field.presence {
            return false;
        }
        if matches!(left_field.presence, Presence::Present) {
            payloads.push((left_field.ty.clone(), right_field.ty.clone()));
        }
    }
    let same_rest = match (left_rest, right_rest) {
        (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => true,
        (Rest::Var(left), Rest::Var(right)) | (Rest::Bound(left), Rest::Bound(right)) => {
            left == right
        }
        (Rest::Rigid { id: left, .. }, Rest::Rigid { id: right, .. }) => left == right,
        _ => false,
    };
    if !same_rest {
        return false;
    }
    for (left, right) in payloads.into_iter().rev() {
        work.push(MismatchWork {
            left,
            right,
            alias_path,
            alias_work: *alias_work,
            root: false,
        });
    }
    true
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PivotKey {
    FunctionInput(PivotAnchor),
    BranchResult(Anchor),
    ProjectedField { base: Anchor, field: String },
    FunctionEffects(Anchor),
    WrittenBinder(Anchor),
    Anonymous { span: Anchor, origin: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PivotAnchor {
    Binder(Symbol),
    Anchor(Anchor),
}

impl PivotKey {
    fn kind(&self) -> ExplanationPivotKind {
        match self {
            Self::FunctionInput(_) => ExplanationPivotKind::FunctionInput,
            Self::BranchResult(_) => ExplanationPivotKind::BranchResult,
            Self::ProjectedField { .. } => ExplanationPivotKind::ProjectedField,
            Self::FunctionEffects(_) => ExplanationPivotKind::FunctionEffects,
            Self::WrittenBinder(_) => ExplanationPivotKind::WrittenValue,
            Self::Anonymous { .. } => ExplanationPivotKind::Value,
        }
    }

    fn priority(&self) -> u8 {
        match self {
            Self::FunctionInput(_) => 0,
            Self::BranchResult(_) => 1,
            Self::ProjectedField { .. } => 2,
            Self::FunctionEffects(_) => 3,
            Self::WrittenBinder(_) => 4,
            Self::Anonymous { .. } => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PivotContext {
    branch: Option<Anchor>,
    binder: Option<Anchor>,
}

fn pivot_contexts(
    constraints: &IndexMap<Symbol, Vec<Constraint>>,
) -> HashMap<ConstraintId, PivotContext> {
    let mut contexts = HashMap::new();
    let mut work: Vec<_> = constraints
        .values()
        .flatten()
        .map(|constraint| (constraint, PivotContext::default()))
        .collect();
    while let Some((constraint, inherited)) = work.pop() {
        if contexts.insert(constraint.id, inherited).is_some() {
            continue;
        }
        match &constraint.kind {
            ConstraintKind::Let { value, body, .. } => {
                let nested = PivotContext {
                    binder: Some(constraint.at),
                    ..inherited
                };
                work.extend(value.iter().map(|child| (child, nested)));
                work.extend(body.iter().map(|child| (child, inherited)));
            }
            ConstraintKind::Match { arms, .. } => {
                let nested = PivotContext {
                    branch: Some(constraint.at),
                    ..inherited
                };
                for arm in arms {
                    work.extend(arm.constraints.iter().map(|child| (child, nested)));
                    work.push((&arm.result, nested));
                }
            }
            _ => {}
        }
    }
    contexts
}

fn pivot_key(
    fact: &ExplanationFact,
    constraint: &Constraint,
    context: PivotContext,
    instance_binders: &HashMap<Anchor, Symbol>,
) -> PivotKey {
    if constraint.origin == ConstraintOrigin::ApplicationArgument {
        let call = constraint.subjects.primary_span.unwrap_or(constraint.at);
        return PivotKey::FunctionInput(match constraint.subjects.semantic_pivot {
            Some(SemanticPivot::FunctionInput(symbol)) => PivotAnchor::Binder(symbol),
            _ => instance_binders
                .get(&call)
                .copied()
                .map_or(PivotAnchor::Anchor(call), PivotAnchor::Binder),
        });
    }
    if matches!(
        constraint.origin,
        ConstraintOrigin::Match | ConstraintOrigin::MatchArm
    ) {
        return PivotKey::BranchResult(match constraint.subjects.semantic_pivot {
            Some(SemanticPivot::BranchResult(span)) => span,
            _ => context.branch.unwrap_or(constraint.at),
        });
    }
    if let ConstraintKind::Project {
        field, base_span, ..
    } = &constraint.kind
    {
        return PivotKey::ProjectedField {
            base: *base_span,
            field: field.clone(),
        };
    }
    if matches!(constraint.kind, ConstraintKind::Performs { .. }) {
        return PivotKey::FunctionEffects(constraint.at);
    }
    if matches!(
        fact.subject,
        Subject::Binding | Subject::TopLevelBinding | Subject::LocalBinding
    ) {
        return PivotKey::WrittenBinder(context.binder.unwrap_or(fact.at));
    }
    PivotKey::Anonymous {
        span: constraint.at,
        origin: constraint.origin.code(),
    }
}

fn explanatory_name(base: &str, visible_names: &HashSet<String>) -> String {
    if !visible_names.contains(base) {
        return base.into();
    }
    for suffix in 'A'..='Z' {
        let candidate = format!("{base} {suffix}");
        if !visible_names.contains(&candidate) {
            return candidate;
        }
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base} {suffix}");
        if !visible_names.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Keep the earliest and latest entries of an ordered list, dropping the
/// middle, so a bounded slice preserves both defining endpoints. Returns how
/// many entries were dropped.
fn keep_endpoints(items: &mut Vec<ReasonId>, limit: usize) -> usize {
    if items.len() <= limit {
        return 0;
    }
    let dropped = items.len() - limit;
    let low = limit / 2;
    let high = limit - low;
    let tail_start = items.len() - high;
    items.drain(low..tail_start);
    dropped
}

fn budget_reason_slice(mut reasons: Vec<ReasonId>) -> (Vec<ReasonId>, usize) {
    const MAX_EXPLANATION_REASONS: usize = 16_384;
    reasons.sort_unstable();
    let omitted = keep_endpoints(&mut reasons, MAX_EXPLANATION_REASONS);
    (reasons, omitted)
}

struct ExplanationSources<'a> {
    aliases: &'a IndexMap<Symbol, Scheme>,
    /// See [`Table::effect_argument_reasons`].
    effect_arguments: &'a HashMap<ReasonId, (String, u32)>,
    /// Where the definitions stand, for putting facts in source order.
    order: &'a Order,
}

fn attach_ordinary_explanations(
    mint: &Mint,
    errors: &mut [Error],
    constraints: &IndexMap<Symbol, Vec<Constraint>>,
    steps: &[Step],
    reasons: &[Reason],
    omitted_reason_parents: &HashMap<ReasonId, usize>,
    sources: ExplanationSources<'_>,
) {
    if errors.is_empty() {
        return;
    }
    let ExplanationSources {
        aliases,
        effect_arguments,
        order,
    } = sources;
    let pivot_contexts = pivot_contexts(constraints);
    let all = all_constraints(constraints);
    // `all` iterates in hash order, so when desugaring reuses one span for
    // several instances the earliest constraint wins explicitly — collecting
    // directly would print a different binder from run to run.
    let mut instances: Vec<(ConstraintId, Anchor, Symbol)> = all
        .values()
        .filter_map(|constraint| match constraint.kind {
            ConstraintKind::Instance { symbol, .. } => Some((constraint.id, constraint.at, symbol)),
            _ => None,
        })
        .collect();
    instances.sort_unstable_by_key(|(id, ..)| *id);
    let mut instance_binders: HashMap<Anchor, Symbol> = HashMap::new();
    for (_, span, symbol) in instances {
        instance_binders.entry(span).or_insert(symbol);
    }
    let visible_names: HashSet<String> = constraints
        .keys()
        .copied()
        .chain(all.values().filter_map(|constraint| match constraint.kind {
            ConstraintKind::Let { symbol, .. } | ConstraintKind::Instance { symbol, .. } => {
                Some(symbol)
            }
            _ => match constraint.subjects.semantic_pivot {
                Some(SemanticPivot::FunctionInput(symbol)) => Some(symbol),
                _ => None,
            },
        }))
        .map(|symbol| mint.name(symbol).to_owned())
        .collect();
    let constraints = all;
    let steps: HashMap<_, _> = steps.iter().map(|step| (step.id, step)).collect();
    let reasons_by_id: HashMap<_, _> = reasons.iter().map(|reason| (reason.id, reason)).collect();

    for error in errors {
        // An effect-argument clash is explained by the failure its arguments
        // met with: the facts that led there are the ones the reader can act
        // on, and the effect and position are the title's to say.
        let (left, right, kind, row) = match error.kind.cause() {
            ErrorKind::Mismatch { expected, actual } => {
                (describe_type(expected), describe_type(actual), None, None)
            }
            ErrorKind::NotAStruct { base, .. } => (
                describe_type(base),
                TypeDescription::Struct,
                Some(ContradictionKind::ProjectionOnNonStruct),
                None,
            ),
            ErrorKind::MissingField { shape, field, .. }
            | ErrorKind::ExtraField { shape, field, .. } => (
                match shape {
                    Shape::Struct => TypeDescription::Struct,
                    Shape::Sum => TypeDescription::TaggedValue,
                    Shape::Effect => TypeDescription::Function,
                },
                TypeDescription::Undecided,
                Some(ContradictionKind::LabelUnavailable),
                Some(RowContradiction {
                    shape: *shape,
                    label: field.clone(),
                }),
            ),
            ErrorKind::RepeatedField { shape, field, .. } => (
                match shape {
                    Shape::Struct => TypeDescription::Struct,
                    Shape::Sum => TypeDescription::TaggedValue,
                    Shape::Effect => TypeDescription::Function,
                },
                TypeDescription::Undecided,
                Some(ContradictionKind::RepeatedLabel),
                Some(RowContradiction {
                    shape: *shape,
                    label: field.clone(),
                }),
            ),
            ErrorKind::Recursive => (
                TypeDescription::Undecided,
                TypeDescription::Undecided,
                Some(ContradictionKind::RecursiveValue),
                None,
            ),
            ErrorKind::RigidBroken { found, .. } => (
                describe_type(found),
                TypeDescription::Undecided,
                Some(ContradictionKind::CallerChoice),
                None,
            ),
            ErrorKind::RigidField { shape, field, .. } => (
                match shape {
                    Shape::Struct => TypeDescription::Struct,
                    Shape::Sum => TypeDescription::TaggedValue,
                    Shape::Effect => TypeDescription::Function,
                },
                TypeDescription::Undecided,
                Some(ContradictionKind::CallerChoice),
                Some(RowContradiction {
                    shape: *shape,
                    label: field.clone(),
                }),
            ),
            ErrorKind::RigidEscapes { .. } => continue,
            ErrorKind::Unhandled { effect } => (
                TypeDescription::Function,
                TypeDescription::Undecided,
                Some(ContradictionKind::UnhandledEffect),
                Some(RowContradiction {
                    shape: Shape::Effect,
                    label: effect.clone(),
                }),
            ),
            ErrorKind::NotAllowed { effect } => (
                TypeDescription::Function,
                TypeDescription::Undecided,
                Some(ContradictionKind::EffectNotAllowed),
                Some(RowContradiction {
                    shape: Shape::Effect,
                    label: effect.clone(),
                }),
            ),
            _ => continue,
        };
        // Presence failures retain which side supplied the required label after
        // solving. Source roles must follow that evidence rather than assuming,
        // for example, that every application argument is the demand.
        let required_side = match error.kind {
            ErrorKind::MissingField { .. } => Some(Side::Expected),
            ErrorKind::ExtraField { .. } => Some(Side::Actual),
            _ => None,
        };
        let seed = match error.cause {
            ErrorCause::Step(id) => steps.get(&id).map(|step| step.reason),
            ErrorCause::Batch(_) | ErrorCause::Direct => None,
        };
        let repeated_origins = match &error.kind {
            ErrorKind::RepeatedField {
                introduction,
                forbidden,
                ..
            } => Some((introduction.as_ref(), forbidden.as_ref())),
            _ => None,
        };

        // Iterative, parent-order DFS. IDs are immutable and parents precede
        // children, so this is deterministic even when bindings share causes.
        let mut walked_reasons = Vec::new();
        let mut omitted_by_roots = 0usize;
        let mut seen_reasons = HashSet::new();
        let mut work: Vec<ReasonId> = seed.into_iter().collect();
        while let Some(id) = work.pop() {
            if !seen_reasons.insert(id) {
                continue;
            }
            let Some(reason) = reasons_by_id.get(&id).filter(|reason| reason.reachable) else {
                continue;
            };
            walked_reasons.push(id);
            omitted_by_roots = omitted_by_roots
                .saturating_add(omitted_reason_parents.get(&id).copied().unwrap_or(0));
            work.extend(reason.parents.iter().rev().copied());
        }
        // A failure that descends from making two applications of one effect
        // agree is a clash between those uses, wherever it surfaced: the
        // annotation that finally refused an argument was refusing what the
        // other use had already decided.
        let clash = match error.kind {
            ErrorKind::EffectArgument { .. } => None,
            _ => walked_reasons
                .iter()
                .find_map(|id| effect_arguments.get(id))
                .cloned(),
        };
        // Preserve both semantic endpoints: the newest nodes contain the
        // failing act, while the oldest tail contains the defining fact. A
        // bounded full view says exactly how much middle was omitted.
        let (reason_slice, omitted_reasons) = budget_reason_slice(walked_reasons);
        let omitted_reasons = omitted_reasons.saturating_add(omitted_by_roots);
        let mut seen_constraints = HashSet::new();
        let mut authoritative_constraints = HashSet::new();
        let mut constraint_slice: Vec<_> = reason_slice
            .iter()
            .filter_map(|id| reasons_by_id.get(id))
            .filter_map(|reason| match reason.origin {
                ReasonOrigin::Constraint(id) if seen_constraints.insert(id) => Some(id),
                ReasonOrigin::Contract(id) => {
                    authoritative_constraints.insert(id);
                    seen_constraints.insert(id).then_some(id)
                }
                _ => None,
            })
            .collect();
        constraint_slice.sort_by_key(|id| {
            constraints.get(id).map_or(
                (true, order.key(Anchor::GENERATED), "", "", ""),
                |constraint| {
                    (
                        constraint.at.is_generated(),
                        order.key(constraint.at),
                        constraint.origin.code(),
                        constraint.subjects.primary.code(),
                        constraint.subjects.secondary.map_or("", Subject::code),
                    )
                },
            )
        });

        let mut full_facts = Vec::new();
        for id in &constraint_slice {
            let Some(constraint) = constraints.get(id) else {
                continue;
            };
            let mut endpoints = vec![
                (
                    constraint.subjects.primary,
                    constraint.subjects.primary_span,
                    Some(Side::Expected),
                ),
                (
                    constraint
                        .subjects
                        .secondary
                        .unwrap_or(constraint.subjects.primary),
                    constraint.subjects.secondary_span,
                    Some(Side::Actual),
                ),
            ];
            // A projection's two source ranges live in its structured payload:
            // the field token is the demand and `base_span` is the value which
            // supplies the other side. Do not invent spans for its fresh result.
            if let ConstraintKind::Project { base_span, .. } = &constraint.kind {
                endpoints = vec![
                    (Subject::ProjectionBase, Some(*base_span), None),
                    (Subject::PatternDemand, Some(constraint.at), None),
                ];
            } else if let ConstraintKind::Performs {
                ambient_label_spans,
                ..
            } = &constraint.kind
                && kind == Some(ContradictionKind::RepeatedLabel)
            {
                endpoints = vec![(Subject::PerformedEffects, Some(constraint.at), None)];
                if let Some(ambient_span) = row
                    .as_ref()
                    .and_then(|row| ambient_label_spans.get(&row.label))
                {
                    endpoints.push((Subject::AmbientEffects, Some(*ambient_span), None));
                }
            } else if matches!(constraint.kind, ConstraintKind::Performs { .. })
                && !matches!(
                    kind,
                    Some(ContradictionKind::UnhandledEffect | ContradictionKind::EffectNotAllowed)
                )
            {
                // The ambient owner is effect-boundary provenance only. Other
                // contradiction families retain the operation endpoint they
                // had before boundaries became structured.
                endpoints = vec![(Subject::PerformedEffects, Some(constraint.at), None)];
            } else if kind == Some(ContradictionKind::RecursiveValue) {
                // A recursive cycle needs both source concepts even when one
                // operand is semantic context (most notably a definition's
                // pre-minted result variable). The closing constraint's own
                // written range grounds that role; no solver-variable range is
                // invented. Keeping both subjects also makes a one-edge cycle
                // a useful two-fact explanation rather than a lone headline.
                for (_, span, _) in &mut endpoints {
                    if span.is_none() {
                        *span = Some(constraint.at);
                    }
                }
                if constraint.subjects.secondary.is_none() {
                    endpoints[1].0 = Subject::Context;
                }
            } else if kind.is_some() && endpoints.iter().all(|(_, span, _)| span.is_none()) {
                // Unary/scoping constraints still own their written operation's
                // range even when no independently written second operand exists.
                endpoints[0].1 = Some(constraint.at);
            }
            for &(subject, span, side) in &endpoints {
                let Some(span) = span else { continue };
                if authoritative_constraints.contains(id) && subject != Subject::Annotation {
                    continue;
                }
                let payload = if kind == Some(ContradictionKind::RepeatedLabel) {
                    // A same-label effects application can be an intermediate in
                    // another repeated-effect path. Assign endpoint roles only
                    // when every available piece of row-fact provenance agrees;
                    // label membership alone is not causal evidence.
                    let matches = |origin: &RowFactOrigin| {
                        origin.constraint == *id
                            && origin.reason == constraint.reason
                            && origin.origin == constraint.origin
                            && origin.subject == subject
                            && origin.at == span
                    };
                    match repeated_origins {
                        Some((Some(introduction), _)) if matches(introduction) => {
                            ExplanationFactPayload::LabelIntroduction
                        }
                        Some((_, Some(forbidden))) if matches(forbidden) => {
                            ExplanationFactPayload::LabelForbidden
                        }
                        _ => ExplanationFactPayload::RequiresType,
                    }
                } else {
                    match (&constraint.kind, constraint.origin, subject, side) {
                        // A projection contributes two views of one requirement:
                        // the written access and the base it demands the field of.
                        // It never supplies evidence that a row is closed.
                        (ConstraintKind::Project { .. }, ConstraintOrigin::Projection, _, _)
                            if kind.is_some() =>
                        {
                            ExplanationFactPayload::LabelDemand
                        }
                        (ConstraintKind::Equal { .. }, _, _, Some(side))
                            if matches!(kind, Some(ContradictionKind::LabelUnavailable))
                                && required_side.is_some() =>
                        {
                            if Some(side) == required_side {
                                ExplanationFactPayload::LabelDemand
                            } else {
                                ExplanationFactPayload::ClosedRow
                            }
                        }
                        (_, ConstraintOrigin::Pattern, Subject::PatternDemand, _)
                            if matches!(kind, Some(ContradictionKind::LabelUnavailable)) =>
                        {
                            ExplanationFactPayload::LabelDemand
                        }
                        (
                            ConstraintKind::Performs { .. },
                            ConstraintOrigin::ApplicationEffects,
                            Subject::AmbientEffects,
                            _,
                        ) if matches!(
                            kind,
                            Some(
                                ContradictionKind::UnhandledEffect
                                    | ContradictionKind::EffectNotAllowed
                            )
                        ) =>
                        {
                            ExplanationFactPayload::EffectBoundary
                        }
                        (
                            ConstraintKind::Performs { .. },
                            ConstraintOrigin::ApplicationEffects,
                            _,
                            _,
                        ) if matches!(
                            kind,
                            Some(
                                ContradictionKind::UnhandledEffect
                                    | ContradictionKind::EffectNotAllowed
                            )
                        ) =>
                        {
                            ExplanationFactPayload::EffectUse
                        }
                        (_, ConstraintOrigin::ApplicationCallee, Subject::Callee, _) => {
                            ExplanationFactPayload::UsedAsFunction
                        }
                        (_, ConstraintOrigin::ApplicationArgument, _, _) => {
                            ExplanationFactPayload::SuppliesArgument
                        }
                        (_, ConstraintOrigin::MatchArm | ConstraintOrigin::Match, _, _) => {
                            ExplanationFactPayload::BranchResult
                        }
                        _ => ExplanationFactPayload::RequiresType,
                    }
                };
                full_facts.push(ExplanationFact {
                    at: span,
                    direct: false,
                    constraint: *id,
                    origin: constraint.origin,
                    subject,
                    payload,
                });
            }
        }
        if let Some(declared) = match &error.kind {
            ErrorKind::RigidBroken { declared, .. } | ErrorKind::RigidField { declared, .. } => {
                Some(*declared)
            }
            _ => None,
        } {
            for fact in &mut full_facts {
                if fact.at == error.at {
                    fact.payload = ExplanationFactPayload::CallerChoiceUse;
                }
                if fact.at == declared {
                    fact.payload = ExplanationFactPayload::CallerChoiceDeclaration;
                }
            }
            if !full_facts.iter().any(|fact| fact.at == declared)
                && let Some(constraint) = constraint_slice.first().copied()
            {
                full_facts.push(ExplanationFact {
                    direct: false,
                    at: declared,
                    constraint,
                    origin: ConstraintOrigin::ContextualCheck,
                    subject: Subject::Annotation,
                    payload: ExplanationFactPayload::CallerChoiceDeclaration,
                });
            }
        }
        if matches!(
            kind,
            Some(ContradictionKind::UnhandledEffect | ContradictionKind::EffectNotAllowed)
        ) && let Some(row) = &row
            && let Some((constraint, origin)) = constraint_slice.iter().rev().find_map(|id| {
                let effect_origins = match &constraints.get(id)?.kind {
                    ConstraintKind::Performs { effect_origins, .. }
                    | ConstraintKind::Isolate { effect_origins, .. } => effect_origins,
                    _ => return None,
                };
                effect_origins.iter().find_map(|source| {
                    let EffectSource::Origin(origin) = source else {
                        return None;
                    };
                    (origin.interface.row_key() == row.label).then_some((*id, origin))
                })
            })
        {
            // Declaration evidence follows the resolved operation identity, not
            // a structural row-key lookup. Same-named equivalent interfaces may
            // share a row while still naming different declarations here.
            full_facts.push(ExplanationFact {
                direct: false,
                at: origin.declaration_at,
                constraint,
                origin: ConstraintOrigin::ContextualCheck,
                subject: Subject::EffectDeclaration,
                payload: ExplanationFactPayload::EffectDeclaration,
            });
        }
        if let ErrorKind::RepeatedField {
            introduction: Some(introduction),
            forbidden: Some(forbidden),
            ..
        } = &error.kind
            && introduction.at != forbidden.at
        {
            // Keep the complete causal slice. Exact row-fact provenance either
            // corrects the corresponding sliced fact in place or adds a missing
            // endpoint; it must never replace the intermediate path.
            for (origin, payload) in [
                (introduction, ExplanationFactPayload::LabelIntroduction),
                (forbidden, ExplanationFactPayload::LabelForbidden),
            ] {
                if let Some(at) = full_facts.iter().position(|fact| {
                    fact.at == origin.at
                        && fact.constraint == origin.constraint
                        && fact.origin == origin.origin
                        && fact.subject == origin.subject
                }) {
                    full_facts[at].payload = payload;
                } else {
                    full_facts.push(ExplanationFact {
                        direct: false,
                        at: origin.at,
                        constraint: origin.constraint,
                        origin: origin.origin,
                        subject: origin.subject,
                        payload,
                    });
                }
            }
        }
        // Overlays can append or change roles. Restore source order afterwards;
        // no diagnostic order is allowed to inherit a solver identity.
        full_facts.sort_by_key(|fact| {
            (
                fact.at.is_generated(),
                order.key(fact.at),
                fact.origin.code(),
                fact.subject.code(),
                fact.payload as u8,
            )
        });
        // A reason without a written endpoint cannot support source labels.
        // Keep the established diagnostic rather than inventing context.
        if full_facts.is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        let mut included = HashSet::new();
        for (at, fact) in full_facts.iter().enumerate() {
            // Constraint IDs distinguish solver records, not source facts.
            // Shared paths may repeat one written fact through several records.
            if included.insert((fact.at, fact.origin, fact.subject, fact.payload)) {
                candidates.push(at);
            }
        }
        let opposing_roles = match kind {
            Some(ContradictionKind::RepeatedLabel) => Some((
                ExplanationFactPayload::LabelIntroduction,
                ExplanationFactPayload::LabelForbidden,
            )),
            Some(
                ContradictionKind::LabelUnavailable | ContradictionKind::ProjectionOnNonStruct,
            ) => Some((
                ExplanationFactPayload::LabelDemand,
                ExplanationFactPayload::ClosedRow,
            )),
            _ => None,
        };
        let caller_endpoints = (kind == Some(ContradictionKind::CallerChoice))
            .then(|| {
                let declared = candidates.iter().copied().find(|at| {
                    full_facts[*at].payload == ExplanationFactPayload::CallerChoiceDeclaration
                })?;
                let used = candidates.iter().rev().copied().find(|at| {
                    full_facts[*at].payload == ExplanationFactPayload::CallerChoiceUse
                })?;
                Some([declared, used])
            })
            .flatten();
        let opposing_endpoints = opposing_roles.and_then(|(first, second)| {
            let first = candidates
                .iter()
                .rev()
                .copied()
                .find(|at| full_facts[*at].payload == first)?;
            let second = candidates.iter().rev().copied().find(|at| {
                full_facts[*at].payload == second && full_facts[*at].at != full_facts[first].at
            })?;
            Some([first, second])
        });
        // Row prose is causal only when independently grounded evidence names
        // both the demand/introduction and the use which limits/forbids it.
        if opposing_roles.is_some() && opposing_endpoints.is_none() {
            continue;
        }

        // Keep the two contradiction endpoints and, at most, one useful
        // semantic handoff between them. The complete path above is untouched;
        // this only chooses the ordinary reading of it.
        let mut shared_keys: Vec<(PivotKey, Vec<usize>)> = Vec::new();
        let mut keyed: HashMap<PivotKey, Vec<usize>> = HashMap::new();
        for at in candidates.iter().copied() {
            let fact = &full_facts[at];
            let Some(constraint) = constraints.get(&fact.constraint) else {
                continue;
            };
            let key = pivot_key(
                fact,
                constraint,
                pivot_contexts
                    .get(&fact.constraint)
                    .copied()
                    .unwrap_or_default(),
                &instance_binders,
            );
            keyed.entry(key).or_default().push(at);
        }
        shared_keys.extend(
            keyed
                .into_iter()
                .filter(|(_, references)| references.len() >= 2),
        );
        shared_keys.sort_by_key(|(key, references)| (key.priority(), key.clone(), references[0]));
        let endpoint_family = shared_keys.first().map(|(key, matching)| {
            let preferred: Vec<_> = matching
                .iter()
                .copied()
                .filter(|at| match key {
                    PivotKey::FunctionInput(_) => full_facts[*at].subject == Subject::Argument,
                    PivotKey::BranchResult(_) => full_facts[*at].subject == Subject::MatchArm,
                    _ => true,
                })
                .collect();
            let matching = if preferred.len() >= 2 {
                &preferred
            } else {
                matching
            };
            [matching[0], *matching.last().expect("shared pivot key")]
        });
        let effect_endpoints = matches!(
            kind,
            Some(ContradictionKind::UnhandledEffect | ContradictionKind::EffectNotAllowed)
        )
        .then(|| {
            let use_at = candidates
                .iter()
                .rev()
                .copied()
                .find(|at| full_facts[*at].payload == ExplanationFactPayload::EffectUse)?;
            let boundary = candidates
                .iter()
                .copied()
                .find(|at| full_facts[*at].payload == ExplanationFactPayload::EffectBoundary)?;
            Some(match kind {
                Some(ContradictionKind::UnhandledEffect) => [use_at, boundary],
                Some(ContradictionKind::EffectNotAllowed) => [boundary, use_at],
                _ => unreachable!("effect endpoints require an effect contradiction"),
            })
        })
        .flatten();
        let endpoints = caller_endpoints
            .or(opposing_endpoints)
            .or(effect_endpoints)
            .or(endpoint_family)
            .unwrap_or_else(|| {
                [
                    candidates[0],
                    *candidates.last().expect("nonempty candidates"),
                ]
            });
        let mut abridged = vec![endpoints[0], endpoints[1]];
        if effect_endpoints.is_some() {
            if let Some(declaration) = candidates
                .iter()
                .copied()
                .find(|at| full_facts[*at].payload == ExplanationFactPayload::EffectDeclaration)
            {
                abridged.push(declaration);
            }
            if let Some(first_use) = candidates.iter().copied().find(|at| {
                full_facts[*at].payload == ExplanationFactPayload::EffectUse && *at != endpoints[0]
            }) {
                abridged.push(first_use);
            }
        }
        let lo = endpoints[0].min(endpoints[1]);
        let hi = endpoints[0].max(endpoints[1]);
        if opposing_endpoints.is_none()
            && caller_endpoints.is_none()
            && effect_endpoints.is_none()
            && let Some(handoff) = candidates.iter().copied().find(|at| {
                lo < *at
                    && *at < hi
                    && matches!(
                        full_facts[*at].subject,
                        Subject::Parameter
                            | Subject::MatchResult
                            | Subject::ProjectionResult
                            | Subject::AmbientEffects
                    )
            })
        {
            abridged.push(handoff);
        }
        if effect_endpoints.is_some() {
            // Effect diagnostics have a semantic primary: the call which makes
            // a top-level escape unavoidable, or the function boundary which
            // refuses it. Source sorting must not promote a declaration fact.
            let mut seen = HashSet::new();
            abridged.retain(|at| seen.insert(*at));
        } else {
            abridged.sort_unstable();
            abridged.dedup();
        }

        let pivot_candidate = shared_keys.iter().find_map(|(key, all_references)| {
            if matches!(
                kind,
                Some(
                    ContradictionKind::CallerChoice
                        | ContradictionKind::UnhandledEffect
                        | ContradictionKind::EffectNotAllowed
                )
            ) || opposing_endpoints.is_some() && matches!(key, PivotKey::Anonymous { .. })
            {
                return None;
            }
            let references: Vec<_> = all_references
                .iter()
                .copied()
                .filter(|at| abridged.contains(at))
                .collect();
            (references.len() >= 2).then_some((key.kind(), references))
        });
        let pivot = pivot_candidate.map(|(kind, references)| {
            let base = match kind {
                ExplanationPivotKind::FunctionInput => "Input",
                ExplanationPivotKind::BranchResult => "Result",
                ExplanationPivotKind::ProjectedField => "Field",
                ExplanationPivotKind::FunctionEffects => "Effects",
                ExplanationPivotKind::WrittenValue | ExplanationPivotKind::Value => "Value",
            };
            ExplanationPivot {
                name: explanatory_name(base, &visible_names),
                kind,
                introduced_at: references[0],
                references,
            }
        });
        let omitted_facts = candidates.len().saturating_sub(abridged.len());
        let leaf = match error.kind.cause() {
            ErrorKind::Mismatch { expected, actual } => {
                smallest_incompatible(aliases, expected, actual)
            }
            _ => (left, right),
        };
        let failing_constraint = match error.cause {
            ErrorCause::Step(id) => steps.get(&id).and_then(|step| step.constraint),
            ErrorCause::Batch(_) | ErrorCause::Direct => None,
        };
        let value_used_as_function = kind.is_none()
            && failing_constraint
                .and_then(|id| constraints.get(&id))
                .is_some_and(|constraint| {
                    constraint.origin == ConstraintOrigin::ApplicationCallee
                        && (leaf.0 == TypeDescription::Function
                            || leaf.1 == TypeDescription::Function)
                });
        let recursive =
            (kind == Some(ContradictionKind::RecursiveValue)).then(|| match error.cause {
                ErrorCause::Step(id) => steps
                    .get(&id)
                    .and_then(|step| step.recursive)
                    .unwrap_or(RecursiveCycleShape::Neutral),
                ErrorCause::Batch(_) | ErrorCause::Direct => RecursiveCycleShape::Neutral,
            });
        error.explanation = Some(InferenceExplanation {
            full_facts,
            abridged,
            pivot,
            omitted_facts,
            contradiction: Contradiction {
                kind: kind.unwrap_or(if value_used_as_function {
                    ContradictionKind::ValueUsedAsFunction
                } else {
                    ContradictionKind::IncompatibleTypes
                }),
                left: leaf.0,
                right: leaf.1,
                row,
                recursive,
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
                omitted_reasons,
            },
        });
        if let Some((effect, position)) = clash {
            error.kind.as_effect_argument(effect, position);
        }
    }
}

/// Assign a type to every term in the program, in place, and return the
/// schemes of its top-level definitions.
fn direct_extern_explanation(
    error: ErrorId,
    facts: Vec<(Anchor, ConstraintOrigin, Subject, ExplanationFactPayload)>,
    kind: ContradictionKind,
    row: Option<RowContradiction>,
) -> InferenceExplanation {
    // Direct source facts deliberately do not claim a forgeable arena
    // identity. `pending` is never published in the cause's constraint slice.
    let constraint = ConstraintId::pending();
    let mut full_facts = Vec::new();
    for (span, origin, subject, payload) in facts {
        if full_facts.iter().any(|fact: &ExplanationFact| {
            fact.at == span
                && fact.origin == origin
                && fact.subject == subject
                && fact.payload == payload
        }) {
            continue;
        }
        full_facts.push(ExplanationFact {
            at: span,
            direct: true,
            constraint,
            origin,
            subject,
            payload,
        });
    }
    InferenceExplanation {
        abridged: (0..full_facts.len()).collect(),
        full_facts,
        pivot: None,
        omitted_facts: 0,
        contradiction: Contradiction {
            kind,
            left: TypeDescription::Function,
            right: TypeDescription::Undecided,
            row,
            recursive: None,
            repairs: [
                RepairDirection::ChangeFirstUse,
                RepairDirection::ChangeSecondUse,
            ],
        },
        cause: ExplanationCause {
            error,
            seed: None,
            constraints: Vec::new(),
            reasons: Vec::new(),
            omitted_reasons: 0,
        },
    }
}

fn listed_effects(row: &Row) -> Vec<String> {
    flatten_row(row)
        .into_parts()
        .0
        .into_iter()
        .filter(|(_, field)| !matches!(field.presence, Presence::Absent))
        .map(|(name, _)| name)
        .collect()
}

fn same_callback_tail(required: &Rest, available: &Rest) -> bool {
    match (required, available) {
        (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => true,
        (Rest::Var(left), Rest::Var(right)) | (Rest::Bound(left), Rest::Bound(right)) => {
            left == right
        }
        (Rest::Rigid { id: left, .. }, Rest::Rigid { id: right, .. }) => left == right,
        _ => false,
    }
}

/// A fixed abstract callback remainder cannot be supplied by a closed or
/// differently-quantified extern remainder. Flexible holes are intentionally
/// left for the published solver constraint: constraining such a hole is valid
/// inference, not a declaration error by itself.
fn callback_tail_definitely_missing(required: &Row, available: &Row) -> bool {
    let required = flatten_row(required);
    let available = flatten_row(available);
    if matches!(required.rest, Rest::Closed | Rest::Undecided)
        || same_callback_tail(&required.rest, &available.rest)
    {
        return false;
    }
    matches!(required.rest, Rest::Bound(_) | Rest::Rigid { .. })
        && matches!(
            available.rest,
            Rest::Closed | Rest::Bound(_) | Rest::Rigid { .. }
        )
}

fn tail_issue(relation: &CallbackTailRelation) -> String {
    match &relation.available {
        Some(available) => format!(
            "callback remainder {} is not covered by extern remainder {available}",
            relation.required
        ),
        None => format!(
            "callback remainder {} is not covered by the extern declaration's fixed effects",
            relation.required
        ),
    }
}

fn push_distinct(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn aggregate_callback_issues(
    issues: impl IntoIterator<Item = ExternCallbackIssue>,
) -> Vec<ExternCallbackIssue> {
    let mut merged: IndexMap<String, ExternCallbackIssue> = IndexMap::new();
    for issue in issues {
        let entry = merged
            .entry(issue.callback_path.clone())
            .or_insert_with(|| ExternCallbackIssue {
                callback_at: issue.callback_at,
                callback_path: issue.callback_path.clone(),
                callback_type: issue.callback_type.clone(),
                missing_effects: Vec::new(),
                extern_effects: Vec::new(),
                requirements: Vec::new(),
                missing_tails: Vec::new(),
                conditions: Vec::new(),
                condition: String::new(),
            });
        for effect in issue.missing_effects {
            push_distinct(&mut entry.missing_effects, effect);
        }
        for effect in issue.extern_effects {
            push_distinct(&mut entry.extern_effects, effect);
        }
        for requirement in issue.requirements {
            if !entry.requirements.contains(&requirement) {
                entry.requirements.push(requirement);
            }
        }
        for tail in issue.missing_tails {
            push_distinct(&mut entry.missing_tails, tail);
        }
        for condition in issue.conditions {
            push_distinct(&mut entry.conditions, condition);
        }
        entry.condition = match entry.conditions.as_slice() {
            [] => "true".into(),
            [condition] => condition.clone(),
            conditions => conditions
                .iter()
                .map(|condition| format!("({condition})"))
                .collect::<Vec<_>>()
                .join(" and "),
        };
    }
    merged.into_values().collect()
}

fn set_callback_issues(error: &mut Error, additional: Vec<ExternCallbackIssue>) {
    match &mut error.kind {
        ErrorKind::CallbackEffectsNotCovered {
            missing_effects,
            extern_effects,
            callback_path,
            callback_type,
            issues,
            ..
        } => {
            let merged = aggregate_callback_issues(issues.iter().cloned().chain(additional));
            if let Some(first) = merged.first() {
                *missing_effects = first.missing_effects.clone();
                *extern_effects = first.extern_effects.clone();
                *callback_path = first.callback_path.clone();
                *callback_type = first.callback_type.clone();
            }
            *issues = merged;
        }
        ErrorKind::PolymorphicExternBoundary {
            callback_issues, ..
        } => {
            *callback_issues =
                aggregate_callback_issues(callback_issues.iter().cloned().chain(additional));
        }
        _ => {}
    }
}

fn deduplicate_extern_facts(facts: Vec<ExplanationFact>) -> Vec<ExplanationFact> {
    let mut found = Vec::new();
    for fact in facts {
        if let Some(at) = found.iter().position(|prior: &ExplanationFact| {
            prior.at == fact.at
                && prior.origin == fact.origin
                && prior.subject == fact.subject
                && prior.payload == fact.payload
        }) {
            // A solved endpoint carries a published constraint identity. Prefer
            // it over an otherwise equivalent direct declaration-time fact.
            if found[at].direct && !fact.direct {
                found[at] = fact;
            }
            continue;
        }
        found.push(fact);
    }
    found
}

fn direct_callback_issue(
    constraint: &Constraint,
    promised: &Formula,
    names: &[(String, Presence)],
) -> Option<ExternCallbackIssue> {
    let ConstraintKind::CallbackCoverage {
        required,
        available,
        boundary,
    } = &constraint.kind
    else {
        return None;
    };
    let failed: Vec<_> = boundary
        .effects
        .iter()
        .filter(|effect| !sat::entails(promised, &effect.condition))
        .collect();
    let missing_tail = callback_tail_definitely_missing(required, available);
    if failed.is_empty() && !missing_tail {
        return None;
    }
    let requirements: Vec<_> = failed
        .iter()
        .map(|effect| {
            let mut atoms = Vec::new();
            effect.condition.atoms(&mut atoms);
            ExternCallbackRequirement {
                effect: effect.effect.clone(),
                condition: (!atoms.is_empty())
                    .then(|| crate::ui::in_labels(&effect.condition, names)),
            }
        })
        .collect();
    let conditions: Vec<_> = requirements
        .iter()
        .filter_map(|requirement| requirement.condition.clone())
        .collect();
    let condition = crate::ui::in_labels(
        &Formula::all(failed.iter().map(|effect| effect.condition.clone())),
        names,
    );
    Some(ExternCallbackIssue {
        callback_at: boundary.callback_at,
        callback_path: boundary.callback_path.clone(),
        callback_type: boundary.callback_type.clone(),
        missing_effects: failed.iter().map(|effect| effect.effect.clone()).collect(),
        extern_effects: listed_effects(available),
        requirements,
        missing_tails: missing_tail
            .then(|| boundary.tail.as_ref().map(tail_issue))
            .flatten()
            .into_iter()
            .collect(),
        conditions,
        condition,
    })
}

pub fn infer(mint: &Mint, program: &Program, trace: Trace) -> Output {
    Session::default().infer(mint, program, trace)
}

impl Fingerprint {
    fn declarations(&mut self, program: &Program) {
        let fingerprint = self;
        fingerprint.debug(&program.externs);
        fingerprint.debug(&program.types);
        fingerprint.debug(&program.external_names);
        for (symbol, scheme) in &program.external_schemes {
            fingerprint.debug(symbol);
            fingerprint.scheme(scheme);
        }
        for (symbol, declaration) in &program.external_types {
            fingerprint.debug(symbol);
            fingerprint.debug(&declaration.params);
            fingerprint.debug(&declaration.relevant);
            fingerprint.debug(&declaration.unresolved);
            fingerprint.scheme(&declaration.scheme);
        }
        for ((symbol, selector), (from, to)) in &program.external_operations {
            fingerprint.debug(symbol);
            fingerprint.debug(selector);
            fingerprint.ty(from);
            fingerprint.ty(to);
        }
        fingerprint.debug(&program.effect_params);
        fingerprint.debug(&program.effects);
        fingerprint.debug(&program.effect_ids);
    }

    fn word(&mut self, word: u64) {
        self.text.extend_from_slice(&word.to_le_bytes());
    }

    /// What `value` prints as, for the records whose depth is a reader's:
    /// declarations as written, and provenance.
    fn debug(&mut self, value: &impl std::fmt::Debug) {
        use std::io::Write as _;
        writeln!(self.text, "{value:?}").expect("writing to a buffer");
    }

    fn scheme(&mut self, scheme: &Scheme) {
        self.word(scheme.count() as u64);
        self.word(scheme.presences() as u64);
        self.word(scheme.existentials().len() as u64);
        for existential in scheme.existentials() {
            self.word(*existential as u64);
        }
        self.debug(&scheme.representations());
        self.debug(&scheme.callable());
        self.ty(scheme.body());
        self.formula(scheme.formula());
    }

    /// A type's structure, walked with a list rather than the stack: an
    /// imported type may be as deep as an artifact cares to make it.
    fn ty(&mut self, ty: &Arc<Ty>) {
        let mut work: Vec<&Ty> = vec![ty];
        let mut rows: Vec<&Row> = Vec::new();
        loop {
            if let Some(ty) = work.pop() {
                match ty {
                    Ty::Nat => self.word(0x01),
                    Ty::Int => self.word(0x02),
                    Ty::Fixed(kind) => {
                        self.word(0x20);
                        self.word(*kind as u64);
                    }
                    Ty::Real => self.word(0x03),
                    Ty::String => self.word(0x04),
                    Ty::Bool => self.word(0x05),
                    Ty::Any => self.word(0x30),
                    Ty::ForeignValue => self.word(0x31),
                    Ty::Arrow(from, to, effects) => {
                        self.word(0x06);
                        work.push(to);
                        work.push(from);
                        rows.push(effects);
                    }
                    Ty::Package(inner) => {
                        self.word(0x07);
                        work.push(inner);
                    }
                    Ty::Mut(region, element) => {
                        self.word(0x10);
                        work.push(element);
                        work.push(region);
                    }
                    Ty::Array(inner) => {
                        self.word(0x08);
                        work.push(inner);
                    }
                    Ty::Struct(row) => {
                        self.word(0x09);
                        rows.push(row);
                    }
                    Ty::Sum(row) => {
                        self.word(0x0a);
                        rows.push(row);
                    }
                    Ty::Var(var) => {
                        self.word(0x0b);
                        self.word(*var as u64);
                    }
                    Ty::Bound(index) => {
                        self.word(0x0c);
                        self.word(*index as u64);
                    }
                    Ty::Rigid { id, name } => {
                        self.word(0x0d);
                        self.word(*id as u64);
                        self.debug(name);
                    }
                    Ty::Named { symbol, name, args } => {
                        self.word(0x0e);
                        self.debug(symbol);
                        self.debug(name);
                        self.word(args.len() as u64);
                        work.extend(args.iter().rev().map(|arg| &**arg));
                    }
                    Ty::Undecided => self.word(0x0f),
                }
                continue;
            }
            let Some(row) = rows.pop() else { break };
            self.word(0x52);
            let mut row = row;
            loop {
                self.word(row.labels.len() as u64);
                let mut labels: Vec<_> = row.labels.iter().collect();
                labels.sort_unstable_by_key(|(label, _)| *label);
                for (label, field) in labels {
                    self.debug(label);
                    self.debug(&field.presence);
                    work.push(&field.ty);
                }
                match &row.rest {
                    Rest::Closed => self.word(0x60),
                    Rest::Var(var) => {
                        self.word(0x61);
                        self.word(*var as u64);
                    }
                    Rest::Bound(index) => {
                        self.word(0x62);
                        self.word(*index as u64);
                    }
                    Rest::Rigid { id, name } => {
                        self.word(0x63);
                        self.word(*id as u64);
                        self.debug(name);
                    }
                    Rest::Undecided => self.word(0x64),
                    Rest::More(more) => {
                        self.word(0x65);
                        row = more;
                        continue;
                    }
                }
                break;
            }
        }
    }

    /// A formula's structure, walked with a list for the reason a type's is.
    fn formula(&mut self, formula: &Formula) {
        let mut work = vec![formula];
        while let Some(formula) = work.pop() {
            match formula {
                Formula::True => self.word(0x20),
                Formula::False => self.word(0x21),
                Formula::Atom(atom) => {
                    self.word(0x22);
                    self.debug(atom);
                }
                Formula::Owned(owner, inner) => {
                    self.word(0x23);
                    self.word(*owner as u64);
                    work.push(inner);
                }
                Formula::Not(inner) => {
                    self.word(0x24);
                    work.push(inner);
                }
                Formula::And(left, right) => {
                    self.word(0x25);
                    work.push(right);
                    work.push(left);
                }
                Formula::Or(left, right) => {
                    self.word(0x26);
                    work.push(right);
                    work.push(left);
                }
                Formula::Iff(left, right) => {
                    self.word(0x27);
                    work.push(right);
                    work.push(left);
                }
                Formula::Xor(left, right) => {
                    self.word(0x28);
                    work.push(right);
                    work.push(left);
                }
            }
        }
    }
}

/// Lower and review every declaration: the aliases, operations, effect
/// aliases and externs that every group reads and none writes. What comes
/// back is the signatures, and the declaration stage's own result — the
/// extern reviews' errors, constraints and steps, which are solved in a table
/// of their own the way a group's are.
fn declarations(mint: &Mint, program: &Program) -> (Arc<Signatures>, GroupResult) {
    let binding_names = program
        .terms
        .keys()
        .copied()
        .map(|symbol| (symbol, Arc::from(mint.name(symbol))))
        .collect();
    let mut env = HashMap::new();
    let mut aliases = IndexMap::new();
    let mut errors = Vec::new();

    // What every declaration takes, before anything is lowered: the very first
    // lowering below is of a declaration's body, and a row parameter written
    // inside one already imposes its condition. See [`Table::params`].
    let params: HashMap<Symbol, Vec<ParamKind>> = program
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

    // The declarations' own table: what the aliases and externs are lowered
    // and reviewed in, before any group exists. Lowering reads the parameter
    // kinds, so those are in place first; the rest of the signatures are
    // filled in below as they are made.
    let mut table = Table::new(
        Symbol::GENERATED,
        Arc::new(Signatures {
            effect_kinds: program
                .effect_ids
                .iter()
                .map(|(symbol, id)| {
                    (
                        id.row_key(),
                        program
                            .effect_params
                            .get(symbol)
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            binding_names,
            params,
            nominal,
            ..Default::default()
        }),
    );
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

    let variances = semantic_variances(&aliases);

    // And what each operation was declared to be, before any body is walked: a
    // perform site and a handler arm each want the two sides of one, and both
    // come straight off the declaration. Lowered here rather than per use, for
    // the reason the aliases above are: a signature is a plain closed arrow, so
    // it mentions no variable and lowering one twice would only mint two copies
    // of nothing.
    let effect_declaration_spans: IndexMap<_, _> = program
        .effects
        .iter()
        .map(|(symbol, declaration)| (*symbol, declaration.name_at))
        .collect();
    let mut operations = program.external_operations.clone();
    let mut effect_aliases = IndexMap::new();
    for (symbol, decl) in &program.effects {
        match &decl.value {
            ir::Effect::Operations(declared) => {
                for (name, operation) in declared {
                    let from = lower_type(mint, &mut table, &operation.from);
                    let to = lower_type(mint, &mut table, &operation.to);
                    operations.insert((*symbol, name.clone()), (from, to));
                }
            }
            ir::Effect::Alias(alias) => {
                let cases = alias
                    .body
                    .cases
                    .iter()
                    .map(|case| {
                        let args = case
                            .args
                            .iter()
                            .map(|arg| lower_type(mint, &mut table, arg))
                            .collect();
                        (case.symbol, args)
                    })
                    .collect();
                let tail = match &alias.body.tail {
                    Some(ir::Tail {
                        of: ir::Row::Param { index, .. },
                        ..
                    }) => Some(*index),
                    _ => None,
                };
                effect_aliases.insert(*symbol, EffectAliasRow { cases, tail });
            }
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
        let clause_valid = sat::satisfiable(&lowered.formula);
        if !clause_valid {
            errors.push(Error {
                id: table.error_id(),
                cause: ErrorCause::Direct,
                at: annotation.ty.at,
                kind: ErrorKind::ClauseImpossible {
                    formula: crate::ui::in_labels(&lowered.formula, &lowered.names),
                },
                explanation: None,
            });
        }
        let extern_name = mint.name(*symbol).to_string();
        // Reserved array runtime targets are compiler intrinsics rather than
        // user host boundaries. Lowering accepts them only with one of the
        // four exact intrinsic signatures, so the element variable is safely
        // representation-polymorphic here regardless of the declaring
        // bundle's source-controlled name.
        let runtime_array_primitive = decl.value.array_intrinsic
            || crate::reification::Intrinsic::recognize(
                &decl.value.target.anchored,
                lowered.scheme.body(),
                &aliases,
            )
            .is_some();
        let ExternBoundaryReview {
            leaves: _,
            mut coverage,
        } = if clause_valid && !runtime_array_primitive {
            extern_boundary_review(
                &aliases,
                &decl.value.abi,
                &lowered.ty,
                &lowered.extern_sources,
                &extern_name,
                decl.name_at,
            )
        } else {
            ExternBoundaryReview::default()
        };
        let leaves: Vec<PolymorphicExternLeaf> = Vec::new();
        for constraint in &mut coverage {
            constraint.id = table.constraint_id();
            constraint.reason = table.constraint_reason(constraint.id);
        }
        let callback_issues = aggregate_callback_issues(coverage.iter().filter_map(|constraint| {
            direct_callback_issue(constraint, &lowered.formula, &lowered.names)
        }));

        let baseline_error = if let Some(first) = leaves.first().cloned() {
            let id = table.error_id();
            let mut facts = Vec::new();
            for leaf in &leaves {
                facts.push((
                    leaf.at,
                    ConstraintOrigin::ContextualCheck,
                    Subject::Annotation,
                    ExplanationFactPayload::PolymorphicExternLeaf,
                ));
            }
            for issue in &callback_issues {
                facts.push((
                    issue.callback_at,
                    ConstraintOrigin::CallbackBoundary,
                    Subject::CallbackRequired,
                    ExplanationFactPayload::CallbackRequirement,
                ));
            }
            facts.push((
                decl.name_at,
                ConstraintOrigin::Binding,
                Subject::Binding,
                ExplanationFactPayload::ExternPosition,
            ));
            facts.push((
                decl.name_at,
                ConstraintOrigin::Binding,
                Subject::Binding,
                ExplanationFactPayload::ExternDeclaration,
            ));
            let explanation = direct_extern_explanation(
                id,
                facts,
                ContradictionKind::PolymorphicExternBoundary,
                None,
            );
            errors.push(Error {
                id,
                cause: ErrorCause::Direct,
                at: first.at,
                kind: ErrorKind::PolymorphicExternBoundary {
                    variable: first.variable.clone(),
                    variable_kind: first.kind,
                    position: first.position.clone(),
                    extern_name,
                    leaves,
                    callback_issues,
                },
                explanation: Some(explanation),
            });
            Some(id)
        } else if let Some(first) = callback_issues
            .iter()
            .find(|issue| !issue.missing_effects.is_empty())
            .cloned()
        {
            // Label implications are declaration-level SAT failures and do
            // not bind caller choices in the solver. A tail-only issue is left
            // to its real solver failure so the diagnostic retains the failed
            // step which established that structural relation.
            let id = table.error_id();
            let mut facts = Vec::new();
            for issue in &callback_issues {
                facts.push((
                    issue.callback_at,
                    ConstraintOrigin::CallbackBoundary,
                    Subject::CallbackRequired,
                    ExplanationFactPayload::CallbackRequirement,
                ));
            }
            facts.push((
                decl.value.abi.at,
                ConstraintOrigin::CallbackBoundary,
                Subject::CallbackAvailable,
                ExplanationFactPayload::ExternCapability,
            ));
            facts.push((
                decl.name_at,
                ConstraintOrigin::Binding,
                Subject::Binding,
                ExplanationFactPayload::ExternDeclaration,
            ));
            let explanation = direct_extern_explanation(
                id,
                facts,
                ContradictionKind::CallbackEffectsNotCovered,
                first
                    .missing_effects
                    .first()
                    .map(|effect| RowContradiction {
                        shape: Shape::Effect,
                        label: effect.clone(),
                    }),
            );
            errors.push(Error {
                id,
                cause: ErrorCause::Direct,
                at: first.callback_at,
                kind: ErrorKind::CallbackEffectsNotCovered {
                    missing_effects: first.missing_effects.clone(),
                    extern_effects: first.extern_effects.clone(),
                    callback_path: first.callback_path.clone(),
                    callback_type: first.callback_type.clone(),
                    extern_name,
                    issues: callback_issues,
                },
                explanation: Some(explanation),
            });
            Some(id)
        } else {
            None
        };
        // Coverage remains real solver evidence even when another boundary
        // defect already owns this declaration's one diagnostic. Static SAT
        // checks above decide written symbolic labels; solving retains labels
        // in anonymous row holes, evaluates exact open-tail relations, and
        // preserves every published constraint id.
        extern_coverage.push((*symbol, coverage, baseline_error));
        env.insert(
            *symbol,
            Binding::Poly(ExplainedScheme::imported(lowered.scheme.clone())),
        );
        externs.insert(*symbol, lowered.scheme);
    }

    env.extend(program.external_schemes.iter().map(|(symbol, scheme)| {
        (
            *symbol,
            Binding::Poly(ExplainedScheme::imported(scheme.clone())),
        )
    }));
    let mut locals = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut steps = Vec::new();
    let mut refinements = Vec::new();
    let signatures = table.signatures.clone();
    for (symbol, coverage, baseline_error) in extern_coverage {
        let error_start = errors.len();
        // Callback coverage is solver input just like generated definition
        // constraints. Keep it in the published arena so every step's direct
        // constraint identity remains resolvable by debugger consumers.
        constraints.insert(symbol, coverage.clone());
        Solve {
            table: &mut table,
            errors: &mut errors,
            steps: &mut steps,
            aliases: &aliases,
            nominal: &signatures.nominal,
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
        let flipped_now = report_flip(&mut table, &mut errors);
        if errors.len() == error_start {
            // A coverage-origin flip reports no error of its own. Leaving the
            // store marked unsatisfiable with nothing on screen would silently
            // suppress every later definition's presence diagnostics, so the
            // mark rolls back; a genuine contradiction resurfaces during the
            // definition solves, where the flip owns a surviving error.
            if let Some(at) = flipped_now {
                table.unsat = false;
                table.store.batches[at].flipped = false;
            }
            continue;
        }

        let generated_issues = errors[error_start..]
            .iter()
            .flat_map(|error| match &error.kind {
                ErrorKind::CallbackEffectsNotCovered { issues, .. } => issues.clone(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        let generated_facts = errors[error_start..]
            .iter()
            .filter_map(|error| error.explanation.as_ref())
            .flat_map(|explanation| explanation.full_facts.iter().cloned())
            .collect::<Vec<_>>();
        let generated_constraints = errors[error_start..]
            .iter()
            .filter_map(|error| error.explanation.as_ref())
            .flat_map(|explanation| explanation.cause.constraints.iter().copied())
            .collect::<Vec<_>>();
        let generated_reasons = errors[error_start..]
            .iter()
            .filter_map(|error| error.explanation.as_ref())
            .flat_map(|explanation| explanation.cause.reasons.iter().copied())
            .collect::<Vec<_>>();

        // One extern declaration remains one diagnostic. A representation
        // defect may already own it; otherwise the first solved tail failure
        // does. In either case merge every callback path and every real causal
        // id, while emitting shared capability/declaration facts only once.
        let primary_at = baseline_error
            .and_then(|id| {
                errors[..error_start]
                    .iter()
                    .position(|error| error.id == id)
            })
            .unwrap_or(error_start);
        let primary_id = errors[primary_at].id;
        set_callback_issues(&mut errors[primary_at], generated_issues);
        if let Some(explanation) = &mut errors[primary_at].explanation {
            let mut facts = explanation.full_facts.clone();
            facts.extend(generated_facts);
            explanation.full_facts = deduplicate_extern_facts(facts);
            explanation.abridged = (0..explanation.full_facts.len()).collect();
            explanation.cause.constraints = deduplicate(
                explanation
                    .cause
                    .constraints
                    .iter()
                    .copied()
                    .chain(generated_constraints)
                    .collect(),
            );
            explanation.cause.reasons = deduplicate(
                explanation
                    .cause
                    .reasons
                    .iter()
                    .copied()
                    .chain(generated_reasons)
                    .collect(),
            );
        }
        for step in &mut steps {
            if step.definition == symbol && step.error.is_some() {
                step.error = Some(primary_id);
            }
        }
        match baseline_error {
            Some(_) => errors.truncate(error_start),
            None => errors.truncate(error_start + 1),
        }
        // The merge keeps one extern diagnostic and discards the rest of the
        // window, which may include the flip error just reported. The cascade
        // rule suppresses later presence diagnostics only while the flip's
        // own error is on screen, so when the merge dropped that error the
        // store's mark rolls back with it.
        if let Some(at) = flipped_now {
            let cause = ErrorCause::Batch(table.store.batches[at].id);
            if !errors.iter().any(|error| error.cause == cause) {
                table.unsat = false;
                table.store.batches[at].flipped = false;
            }
        }
    }
    // The groups are read out before anything is solved: solving mutates the
    // definitions they name, and which definitions have to be typed together is
    // a fact about the lowered program that nothing here changes.
    drop(signatures);
    let Signatures {
        effect_kinds,
        binding_names,
        params,
        nominal,
        ..
    } = Arc::try_unwrap(std::mem::take(&mut table.signatures))
        .unwrap_or_else(|shared| (*shared).clone());
    let signatures = Arc::new(Signatures {
        effect_kinds,
        binding_names,
        params,
        nominal,
        variances,
        aliases,
        operations,
        effect_aliases,
        effect_declaration_spans,
        externs,
        env,
    });
    table.signatures = signatures.clone();
    let result = table.finish(GroupParts {
        typed: IndexMap::new(),
        published: IndexMap::new(),
        schemes: IndexMap::new(),
        locals,
        promises: IndexMap::new(),
        constraints,
        errors,
        steps,
        refinements,
    });
    (signatures, result)
}

/// Solve one group in a table of its own, reading the signatures and the
/// environment and writing nothing anywhere else: the same members with the
/// same signatures and the same bindings for what they name give the same
/// result, whatever else the program holds and whatever was solved before.
///
/// The declarations are copied before they are typed, so the program handed
/// in is read only; the typed copies come back in the result.
fn infer_group(
    mint: &Mint,
    program: &Program,
    signatures: &Arc<Signatures>,
    outer: Arc<HashMap<Symbol, Binding>>,
    members: &[Symbol],
) -> GroupResult {
    let scope = *members.first().expect("a group has a member");
    let mut table = Table::new(scope, signatures.clone());
    let mut env = Env {
        outer,
        own: HashMap::new(),
    };
    let mut typed: IndexMap<Symbol, Decl<Term>> = members
        .iter()
        .map(|symbol| (*symbol, program.terms[symbol].clone()))
        .collect();
    let mut errors = Vec::new();
    let mut steps = Vec::new();
    let mut locals = IndexMap::new();
    let mut refinements = Vec::new();
    let mut promises = IndexMap::new();
    let mut schemes = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut published_schemes = IndexMap::new();
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
            let lowered = typed[symbol].annotation.as_ref().map(|annotation| {
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
                    table.require(annotation.ty.at, origin, lowered.assumptions.clone());
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
                    env.insert(
                        *symbol,
                        Binding::Poly(ExplainedScheme::imported(lowered.scheme.clone())),
                    );
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
        let decl = &mut typed[&scoped.symbol];
        let mut constrain = Constrain {
            table: &mut table,
            mint,
            env: &mut env,
            aliases: &signatures.aliases,
            out: Vec::new(),
            annotated: Vec::new(),
            operations: &signatures.operations,
            effect_ids: &program.effect_ids,
            effect_params: &program.effect_params,
            // A definition's value is computed where no handler can reach
            // it, so it is walked at the empty closed row and outside every
            // function — which is what makes performing an effect at the
            // top level an error rather than a silently discarded effect.
            ambient: constrain::Ambient {
                row: Row::closed(),
                inside: false,
                boundary_at: decl.name_at,
                label_spans: IndexMap::new(),
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
        let expected_span = decl.annotation.as_ref().map(|annotation| annotation.ty.at);
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
            aliases: &signatures.aliases,
            nominal: &signatures.nominal,
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
        let decl = &mut typed[&symbol];

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
        table.escapes(
            &member.ty,
            &member.scoped.rigids,
            Arc::from(mint.name(symbol)),
            decl.name_at,
            &mut errors,
        );
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
                at: annotation.ty.at,
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
                    at: annotated.span,
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
        table.close_handler_presences(&member.ty, 0);
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
        // [`Semantics::promises`].
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
        // Written contracts are authoritative: their implementation and
        // any recovery used to finish it must not leak into consumers.
        // Likewise a failed definition publishes no causal evidence.
        let failed = from != to || told != errors.len();
        let provenance = if failed {
            SchemeProvenance::default()
        } else if decl.annotation.is_some() {
            table.authoritative_provenance(
                &member.ty,
                scheme.body(),
                &subst,
                scheme.count(),
                &member.generated,
                decl.annotation.as_ref().expect("annotated").ty.at,
            )
        } else {
            table.scheme_provenance(&member.ty, scheme.body(), &subst, scheme.count())
        };
        let explained = ExplainedScheme::local(scheme.clone(), provenance);
        env.insert(symbol, Binding::Poly(explained.clone()));
        published_schemes.insert(symbol, explained);
        schemes.insert(symbol, scheme);
        constraints.insert(symbol, member.generated);
    }
    table.finish(GroupParts {
        typed,
        published: published_schemes,
        schemes,
        locals,
        promises,
        constraints,
        errors,
        steps,
        refinements,
    })
}

/// Every table's result, in the order they were solved, made into one output
/// in the order a reader meets it.
fn assemble(
    mint: &Mint,
    program: &Program,
    signatures: Arc<Signatures>,
    results: Vec<&GroupResult>,
    trace: Trace,
) -> Output {
    let mut typed = IndexMap::new();
    let mut schemes = IndexMap::new();
    let mut locals = IndexMap::new();
    let mut promises = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut errors = Vec::new();
    let mut steps = Vec::new();
    let mut refinements = Vec::new();
    let mut batches = Vec::new();
    let mut variables = IndexMap::new();
    let mut reasons = Vec::new();
    let mut omitted_reason_parents = HashMap::new();
    let mut effect_argument_reasons = HashMap::new();
    let evidence =
        trace == Trace::Complete || results.iter().any(|result| !result.errors.is_empty());
    for result in results {
        typed.extend(
            result
                .typed
                .iter()
                .map(|(symbol, decl)| (*symbol, decl.clone())),
        );
        schemes.extend(
            result
                .schemes
                .iter()
                .map(|(symbol, scheme)| (*symbol, scheme.clone())),
        );
        locals.extend(
            result
                .locals
                .iter()
                .map(|(symbol, scheme)| (*symbol, scheme.clone())),
        );
        promises.extend(
            result
                .promises
                .iter()
                .map(|(symbol, promise)| (*symbol, promise.clone())),
        );
        errors.extend(result.errors.iter().cloned());
        batches.extend(result.store.batches.iter().cloned());
        if evidence {
            constraints.extend(
                result
                    .constraints
                    .iter()
                    .map(|(symbol, constraints)| (*symbol, constraints.clone())),
            );
            steps.extend(result.steps.iter().cloned());
            reasons.extend(result.reasons.iter().cloned());
            omitted_reason_parents.extend(
                result
                    .omitted_reason_parents
                    .iter()
                    .map(|(id, count)| (*id, *count)),
            );
            effect_argument_reasons.extend(
                result
                    .effect_argument_reasons
                    .iter()
                    .map(|(id, reason)| (*id, reason.clone())),
            );
        }
        if trace == Trace::Complete {
            refinements.extend(result.refinements.iter().cloned());
            variables.insert(result.scope, result.variables.clone());
        }
    }
    let store = Store { batches };

    // Both maps are keyed in source order, whatever order the groups were
    // solved in: a reader of either is reading the file, and which definition
    // had to be solved first is the solver's business rather than theirs.
    // [`DiagnosticView::steps`] is that business exactly, and stays in solve order.
    let order = program.order();
    schemes.sort_by(|one, _, other, _| order.rank(*one).cmp(&order.rank(*other)));
    constraints.sort_by(|one, _, other, _| order.rank(*one).cmp(&order.rank(*other)));
    promises.sort_by(|one, _, other, _| order.rank(*one).cmp(&order.rank(*other)));
    typed.sort_by(|one, _, other, _| order.rank(*one).cmp(&order.rank(*other)));

    if evidence {
        flow::refresh(program, &signatures, &mut constraints);
    }

    attach_ordinary_explanations(
        mint,
        &mut errors,
        &constraints,
        &steps,
        &reasons,
        &omitted_reason_parents,
        ExplanationSources {
            aliases: &signatures.aliases,
            effect_arguments: &effect_argument_reasons,
            order: &order,
        },
    );

    // Constraints are solved in the order the walk emitted them, which is not
    // quite the order anyone reads a file in — a body's demands come before
    // the annotation's on its result. Sorting by position puts that back; the
    // sort is stable, so two complaints about one span keep the order the
    // solver found them in.
    errors.sort_by_key(|error| order.key(error.at));
    refinements.sort_by_key(|refinement| {
        (
            order.rank(refinement.definition),
            order.key(refinement.match_at),
            order.key(refinement.arm_at),
        )
    });

    let Signatures {
        aliases,
        operations,
        effect_aliases,
        externs,
        ..
    } = Arc::try_unwrap(signatures).unwrap_or_else(|shared| (*shared).clone());
    let reviewed_externs = program
        .externs
        .iter()
        .map(|(symbol, declaration)| {
            (
                *symbol,
                ReviewedExtern {
                    scheme: externs[symbol].clone(),
                    abi: declaration.value.abi.clone(),
                    target: declaration.value.target.anchored.clone(),
                    target_span: declaration.value.target.at,
                    declaration_span: declaration.name_at,
                },
            )
        })
        .collect();
    let semantics = Semantics {
        reification: crate::reification::Analysis::default(),
        aliases,
        operations,
        effect_aliases,
        externs,
        reviewed_externs,
        schemes,
        locals,
        store,
        promises,
        typed,
    };
    let diagnostics = match trace {
        Trace::Complete => Diagnostics {
            trace,
            errors,
            constraints,
            steps,
            refinements,
            variables,
            reasons,
            recovery_facts: Vec::new(),
        },
        Trace::Off => Diagnostics {
            trace,
            errors,
            constraints: IndexMap::new(),
            steps: Vec::new(),
            refinements: Vec::new(),
            variables: IndexMap::new(),
            reasons: Vec::new(),
            recovery_facts: Vec::new(),
        },
    };
    let mut output = Output {
        semantics,
        diagnostics,
    };
    if output.errors().is_empty() {
        let requirements = crate::reification::Analysis::infer(program, output.semantics());
        output.reify(&requirements);
        output.semantics.reification = requirements.clone();
        for (index, error) in requirements
            .review(program, output.semantics())
            .into_iter()
            .enumerate()
        {
            let ir::ErrorKind::RuntimeTypeInformation { message } = error.kind else {
                unreachable!("representation review reports representation errors")
            };
            output.diagnostics.errors.push(Error {
                id: ErrorId {
                    scope: Symbol::GENERATED,
                    index: index as u32,
                },
                cause: ErrorCause::Direct,
                at: error.at,
                kind: ErrorKind::RuntimeTypeInformation { message },
                explanation: None,
            });
        }
    }
    output
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

/// A digest of a formula's structure, for telling whether the batch in a
/// store slot is still the one the solver encoded. Iterative, since a formula
/// can be as deep as the match that wrote it.
fn formula_digest(formula: &Formula) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut work = vec![formula];
    while let Some(formula) = work.pop() {
        std::mem::discriminant(formula).hash(&mut hasher);
        match formula {
            Formula::True | Formula::False => {}
            Formula::Atom(atom) => atom.hash(&mut hasher),
            Formula::Owned(owner, inner) => {
                owner.hash(&mut hasher);
                work.push(inner);
            }
            Formula::Not(inner) => work.push(inner),
            Formula::And(left, right)
            | Formula::Or(left, right)
            | Formula::Iff(left, right)
            | Formula::Xor(left, right) => {
                work.push(right);
                work.push(left);
            }
        }
    }
    hasher.finish()
}

/// Returns the index of the batch this call newly marked as the flip, so a
/// caller which then discards the flip's error can roll the mark back.
fn report_flip(table: &mut Table, errors: &mut Vec<Error>) -> Option<usize> {
    if table.unsat {
        return None;
    }
    let at = table.flip()?;
    table.unsat = true;
    table.store.batches[at].flipped = true;
    let batch = table.store.batches[at].clone();
    let base = unguarded_origin(&batch.origin);
    let kind = match base {
        UnguardedOrigin::Coverage => return Some(at),
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
        at: batch.at,
        kind,
        explanation: None,
    });
    Some(at)
}

impl Env {
    fn get(&self, symbol: Symbol) -> Option<&Binding> {
        self.own.get(&symbol).or_else(|| self.outer.get(&symbol))
    }

    fn insert(&mut self, symbol: Symbol, binding: Binding) {
        self.own.insert(symbol, binding);
    }
}

impl Table {
    fn new(scope: Symbol, signatures: Arc<Signatures>) -> Self {
        Self {
            scope,
            signatures,
            ..Default::default()
        }
    }

    /// How `symbol` was spelled, for an escape complaint: a top-level
    /// binding's from the signatures, a nested one's from this table.
    fn binding_name(&self, symbol: Symbol) -> Option<Arc<str>> {
        self.local_names
            .get(&symbol)
            .or_else(|| self.signatures.binding_names.get(&symbol))
            .cloned()
    }

    /// Settle what was solved here against this table, and hand it back as a
    /// value that needs no table: the store with every variable followed to
    /// what it was decided to be, the refinements likewise, and the arenas
    /// the explanations read.
    fn finish(self, parts: GroupParts) -> GroupResult {
        let store = self.settled();
        let refinements = parts
            .refinements
            .iter()
            .map(|refinement| self.settled_refinement(refinement))
            .collect();
        GroupResult {
            scope: self.scope,
            typed: parts.typed,
            published: parts.published,
            schemes: parts.schemes,
            locals: parts.locals,
            promises: parts.promises,
            constraints: parts.constraints,
            errors: parts.errors,
            steps: parts.steps,
            refinements,
            store,
            variables: self.var_meta,
            reasons: self.reasons,
            omitted_reason_parents: self.omitted_reason_parents,
            effect_argument_reasons: self.effect_argument_reasons,
        }
    }

    fn constraint_id(&mut self) -> ConstraintId {
        let id = ConstraintId {
            scope: self.scope,
            index: self.next_constraint_id,
        };
        self.next_constraint_id += 1;
        id
    }

    fn reason(&mut self, origin: ReasonOrigin, parents: Vec<ReasonId>) -> ReasonId {
        self.reason_with_omissions(origin, parents, 0)
    }

    fn reason_with_omissions(
        &mut self,
        origin: ReasonOrigin,
        parents: Vec<ReasonId>,
        omitted_parents: usize,
    ) -> ReasonId {
        let id = ReasonId {
            scope: self.scope,
            index: self.next_reason_id,
        };
        self.next_reason_id += 1;
        if origin == ReasonOrigin::Recovery
            || parents
                .iter()
                .any(|parent| self.unpublishable_reasons.contains(parent))
        {
            self.unpublishable_reasons.insert(id);
        }
        if omitted_parents != 0 {
            self.omitted_reason_parents.insert(id, omitted_parents);
        }
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

    fn note_opened_type(&self, ty: &Arc<Ty>) {
        if let Some((opened, reasons)) = self.opened_provenance.get(&(Arc::as_ptr(ty) as usize))
            && opened
                .upgrade()
                .is_some_and(|opened| Arc::ptr_eq(&opened, ty))
        {
            for reason in reasons {
                self.note_binding_read(*reason);
            }
        }
    }

    fn constraint_reason_for(&mut self, id: ConstraintId, kind: &ConstraintKind) -> ReasonId {
        let roots: Vec<&Arc<Ty>> = match kind {
            ConstraintKind::Isolate { input, output, .. } => vec![input, output],
            ConstraintKind::Project { base, result, .. } => vec![base, result],
            ConstraintKind::Spread {
                operand, result, ..
            } => vec![operand, result],
            ConstraintKind::Equal { expected, actual } => vec![expected, actual],
            ConstraintKind::Let { bound, .. } => vec![bound],
            ConstraintKind::Instance { ty, .. } => vec![ty],
            ConstraintKind::Match {
                scrutinee, result, ..
            } => vec![scrutinee, result],
            ConstraintKind::Performs { .. } | ConstraintKind::CallbackCoverage { .. } => Vec::new(),
        };
        let mut parents = Vec::new();
        for ty in roots {
            if let Some((opened, opened_parents)) =
                self.opened_provenance.get(&(Arc::as_ptr(ty) as usize))
                && opened
                    .upgrade()
                    .is_some_and(|opened| Arc::ptr_eq(&opened, ty))
            {
                for parent in opened_parents {
                    if !parents.contains(parent) {
                        parents.push(*parent);
                    }
                }
            }
        }
        self.reason(ReasonOrigin::Constraint(id), parents)
    }

    fn step_id(&mut self) -> StepId {
        let id = StepId {
            scope: self.scope,
            index: self.next_step_id,
        };
        self.next_step_id += 1;
        id
    }

    fn error_id(&mut self) -> ErrorId {
        let id = ErrorId {
            scope: self.scope,
            index: self.next_error_id,
        };
        self.next_error_id += 1;
        id
    }

    fn batch_id(&mut self) -> BatchId {
        let id = BatchId {
            scope: self.scope,
            index: self.next_batch_id,
        };
        self.next_batch_id += 1;
        id
    }

    /// The reason that would be minted next, so a caller can tell which
    /// reasons an act it is about to perform minted.
    fn next_reason(&self) -> ReasonId {
        ReasonId {
            scope: self.scope,
            index: self.next_reason_id,
        }
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
            handler_presences: self.handler_presences.clone(),
            handler_absences: self.handler_absences.clone(),
            vars: self.vars.clone(),
            var_meta_len: self.var_meta.len(),
            levels: self.levels.clone(),
            lacks: self.lacks.clone(),
            active_lacks_origin: self.active_lacks_origin.clone(),
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
        self.handler_presences = known.handler_presences;
        self.handler_absences = known.handler_absences;
        self.vars = known.vars;
        self.var_meta.truncate(known.var_meta_len);
        self.levels = known.levels;
        self.lacks = known.lacks;
        self.active_lacks_origin = known.active_lacks_origin;
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
        self.mint_from(sort, subject, Vec::new())
    }

    fn mint_from(&mut self, sort: VarSort, subject: Subject, parents: Vec<ReasonId>) -> TyVar {
        self.mint_from_budgeted(sort, subject, parents, 0)
    }

    fn mint_from_budgeted(
        &mut self,
        sort: VarSort,
        subject: Subject,
        parents: Vec<ReasonId>,
        omitted: usize,
    ) -> TyVar {
        let var = self.vars.len() as TyVar;
        let minted_by =
            self.reason_with_omissions(ReasonOrigin::Variable { sort, subject }, parents, omitted);
        self.vars.push(Slot::Unbound);
        self.var_meta.push(VarMeta {
            sort,
            subject,
            minted_by,
        });
        self.levels.push(self.level);
        var
    }

    /// An inferred identity used by cell types and mutation effects.
    fn fresh_region(&mut self) -> Arc<Ty> {
        Arc::new(Ty::Var(self.mint(VarSort::Region, Subject::Term)))
    }

    /// A variable standing for a whole type: an unconstrained type
    /// of its own, so that binding it takes whatever it is against entire.
    fn fresh_type_for(&mut self, subject: Subject) -> Arc<Ty> {
        let var = self.mint(VarSort::Type, subject);
        Arc::new(Ty::plain(Ty::Var(var)))
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

    fn fresh_handler_presence(&mut self) -> Presence {
        let presence = self.fresh_presence_for(Subject::AmbientEffects);
        if let Presence::Var(var) = presence {
            self.handler_presences.insert(var);
        }
        presence
    }

    /// Guarded assignments and match families give a presence fresh views.
    /// Keep track of views originating in an inferred handler allowance so
    /// generalization can minimize them together without minimizing a caller's
    /// written or instantiated presences.
    fn inherit_handler_presence<'a>(
        &mut self,
        target: &Presence,
        sources: impl IntoIterator<Item = &'a Presence>,
    ) {
        if let Presence::Var(target) = target
            && sources.into_iter().any(|source| {
                matches!(self.presence_of(source), Presence::Var(var) if self.handler_presences.contains(&var))
            })
        {
            self.handler_presences.insert(*target);
        }
    }

    fn fresh_presence_for(&mut self, subject: Subject) -> Presence {
        Presence::Var(self.mint(VarSort::Presence, subject))
    }

    fn fresh_instance_presence(&mut self) -> Presence {
        self.fresh_presence_for(Subject::Instance)
    }

    fn fresh_match_family_type(&mut self) -> Arc<Ty> {
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
    fn resolve(&self, ty: &Arc<Ty>) -> Arc<Ty> {
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
                if self.unused_handler_presence(&field.presence) {
                    continue;
                }
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
    fn declared(&self, id: u32) -> Anchor {
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
            if let Some((identity, reasons)) = self.opened_presence_provenance.get(&var)
                && self.var_meta[var as usize].minted_by == *identity
            {
                for reason in reasons {
                    self.note_binding_read(*reason);
                }
            }
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

    /// Follow aliases to distinguish a closed, inferred handler allowance
    /// from an explicit negative label. Only the former disappears from rows.
    fn unused_handler_presence(&self, presence: &Presence) -> bool {
        if self.handler_absences.is_empty() {
            return false;
        }
        let mut presence = presence;
        let mut budget = self.vars.len();
        while let Presence::Var(var) = presence {
            let Slot::Bound {
                value: Assigned::Presence(inner),
                by,
            } = &self.vars[*var as usize]
            else {
                return false;
            };
            self.note_binding_read(*by);
            if self.handler_absences.contains(var) {
                return true;
            }
            budget = budget.checked_sub(1).expect("a cycle in presence bindings");
            presence = inner;
        }
        false
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
    fn alike(&self, a: &Arc<Ty>, b: &Arc<Ty>) -> bool {
        enum Work {
            Ty(Arc<Ty>, Arc<Ty>),
            Row(Row, Row),
        }

        let mut same = true;
        let mut work = vec![Work::Ty(a.clone(), b.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(a, b) => {
                    let (a, b) = (self.resolve(&a), self.resolve(&b));
                    match (&*a, &*b) {
                        (Ty::Fixed(left), Ty::Fixed(right)) if left == right => {}
                        (Ty::Nat, Ty::Nat)
                        | (Ty::Int, Ty::Int)
                        | (Ty::Real, Ty::Real)
                        | (Ty::String, Ty::String)
                        | (Ty::Bool, Ty::Bool)
                        | (Ty::Any, Ty::Any)
                        | (Ty::ForeignValue, Ty::ForeignValue)
                        | (Ty::Undecided, Ty::Undecided) => {}
                        (Ty::Var(x), Ty::Var(y)) => same &= x == y,
                        (Ty::Rigid { id: x, .. }, Ty::Rigid { id: y, .. }) => same &= x == y,
                        (Ty::Arrow(from, to, effects), Ty::Arrow(other, result, performs)) => {
                            work.push(Work::Row(effects.clone(), performs.clone()));
                            work.push(Work::Ty(to.clone(), result.clone()));
                            work.push(Work::Ty(from.clone(), other.clone()));
                        }
                        (Ty::Mut(a, b), Ty::Mut(c, d)) => {
                            work.push(Work::Ty(a.clone(), c.clone()));
                            work.push(Work::Ty(b.clone(), d.clone()));
                        }
                        (Ty::Array(element), Ty::Array(other)) => {
                            work.push(Work::Ty(element.clone(), other.clone()));
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
    /// The walk deliberately follows the raw tree instead of calling
    /// [`resolve`](Self::resolve) or [`canon`](Self::canon). Those readers are
    /// right for equality but flatten away the bound-variable edges which form
    /// the causal route around a cycle. On the first route back to `var`, this
    /// returns that route's source-facing shape and records exactly its binding
    /// reasons. [`Solve::fail`] joins them to the active constraint reason, so
    /// the explanation keeps both the iterative path and the source operation
    /// which closed it.
    ///
    /// The explicit work and trace stacks keep deeply nested types bounded by
    /// heap space. A field whose presence resolves absent, or whose imported,
    /// quantified, or recovered presence makes its payload unavailable, is
    /// skipped: that payload denotes nothing locally and cannot participate in
    /// a real cycle. Shared
    /// row-tail cycles with no intervening binding are caught earlier by
    /// [`Solve::labels`]; their closing constraint still enters the same
    /// structured explanation path.
    fn occurs(&self, var: TyVar, value: &Assigned) -> Option<RecursiveCycleShape> {
        // Do not flatten the value before searching it. Flattening is useful
        // for equality, but it loses which bound-variable edges led back to
        // `var` and records reads from innocent sibling branches. This raw,
        // iterative walk follows one concrete route and retains precisely the
        // binding reasons on that route in the active solver rule. The failed
        // step then joins those reasons to the current constraint reason,
        // preserving both the cycle path and its source-level closing edge.
        #[derive(Clone)]
        enum Part {
            Ty(Arc<Ty>),
            Row(Arc<Row>),
            Presence(Presence),
            Field(Arc<Ty>, Presence),
        }
        struct Trace {
            reason: ReasonId,
            parent: Option<usize>,
        }

        fn assigned_part(value: &Assigned) -> Part {
            match value {
                Assigned::Ty(ty) => Part::Ty(ty.clone()),
                Assigned::Row(row) => Part::Row(row.clone()),
                Assigned::Presence(presence) => Part::Presence(presence.clone()),
            }
        }

        #[derive(Clone, Copy, Default)]
        struct Route {
            call_input: bool,
            containment: bool,
        }

        let mut work = vec![(assigned_part(value), None, Route::default())];
        let mut traces: Vec<Trace> = Vec::new();
        let mut seen_tys = HashSet::new();
        let mut seen_rows = HashSet::new();
        // Keep visited allocations alive: inline rows are wrapped for this
        // walk, and otherwise an allocator could reuse a recorded address.
        let mut visited_tys = Vec::new();
        let mut visited_rows = Vec::new();
        let mut seen_vars = HashSet::new();
        // Presence resolution in a field has a continuation: two fields may
        // share one presence variable but carry different payloads.
        let mut seen_field_presences = HashSet::new();
        while let Some((part, trace, route)) = work.pop() {
            let found_target = match part {
                Part::Ty(ty) => {
                    if let Ty::Var(found) = &*ty {
                        if *found == var {
                            true
                        } else if !seen_vars.insert(*found) {
                            false
                        } else if let Slot::Bound { value, by } = &self.vars[*found as usize] {
                            let next = traces.len();
                            traces.push(Trace {
                                reason: *by,
                                parent: trace,
                            });
                            work.push((assigned_part(value), Some(next), route));
                            false
                        } else {
                            false
                        }
                    } else if !seen_tys.insert(Arc::as_ptr(&ty) as usize) {
                        false
                    } else {
                        visited_tys.push(ty.clone());
                        match &*ty {
                            Ty::Package(body) => work.push((Part::Ty(body.clone()), trace, route)),
                            Ty::Mut(region, element) => {
                                for child in [element, region] {
                                    work.push((
                                        Part::Ty(child.clone()),
                                        trace,
                                        Route {
                                            containment: true,
                                            ..route
                                        },
                                    ));
                                }
                            }
                            Ty::Array(element) => work.push((
                                Part::Ty(element.clone()),
                                trace,
                                Route {
                                    containment: true,
                                    ..route
                                },
                            )),
                            Ty::Arrow(from, to, effects) => {
                                // Reverse pushes preserve written order: input,
                                // output, then effects. This ordering is semantic:
                                // the first exact route wins, not a union of sibling routes.
                                work.push((
                                    Part::Row(Arc::new(effects.clone())),
                                    trace,
                                    Route {
                                        containment: true,
                                        ..route
                                    },
                                ));
                                work.push((Part::Ty(to.clone()), trace, route));
                                work.push((
                                    Part::Ty(from.clone()),
                                    trace,
                                    Route {
                                        call_input: true,
                                        ..route
                                    },
                                ));
                            }
                            Ty::Struct(row) | Ty::Sum(row) => {
                                work.push((
                                    Part::Row(Arc::new(row.clone())),
                                    trace,
                                    Route {
                                        containment: true,
                                        ..route
                                    },
                                ));
                            }
                            Ty::Named { args, .. } => work.extend(
                                args.iter()
                                    .rev()
                                    .cloned()
                                    .map(|ty| (Part::Ty(ty), trace, route)),
                            ),
                            Ty::Var(_)
                            | Ty::Nat
                            | Ty::Int
                            | Ty::Fixed(_)
                            | Ty::Real
                            | Ty::String
                            | Ty::Bool
                            | Ty::Any
                            | Ty::ForeignValue
                            | Ty::Bound(_)
                            | Ty::Rigid { .. }
                            | Ty::Undecided => {}
                        }
                        false
                    }
                }
                Part::Row(row) => {
                    if !seen_rows.insert(Arc::as_ptr(&row) as usize) {
                        false
                    } else {
                        visited_rows.push(row.clone());
                        // Push payloads first so the tail is visited first. A
                        // later sibling therefore cannot pollute the retained
                        // binding route.
                        for field in row.labels.values().rev() {
                            work.push((
                                Part::Field(field.ty.clone(), field.presence.clone()),
                                trace,
                                Route {
                                    containment: true,
                                    ..route
                                },
                            ));
                        }
                        if matches!(&row.rest, Rest::Var(found) if *found == var) {
                            true
                        } else {
                            match &row.rest {
                                Rest::More(more) => {
                                    work.push((Part::Row(more.clone()), trace, route))
                                }
                                Rest::Var(found) if seen_vars.insert(*found) => {
                                    if let Slot::Bound {
                                        value: Assigned::Row(row),
                                        by,
                                    } = &self.vars[*found as usize]
                                    {
                                        let next = traces.len();
                                        traces.push(Trace {
                                            reason: *by,
                                            parent: trace,
                                        });
                                        work.push((Part::Row(row.clone()), Some(next), route));
                                    }
                                }
                                Rest::Var(_)
                                | Rest::Closed
                                | Rest::Undecided
                                | Rest::Bound(_)
                                | Rest::Rigid { .. } => {}
                            }
                            false
                        }
                    }
                }
                Part::Presence(presence) => {
                    if let Presence::Var(found) = presence {
                        if found == var {
                            true
                        } else if seen_vars.insert(found)
                            && let Slot::Bound {
                                value: Assigned::Presence(presence),
                                by,
                            } = &self.vars[found as usize]
                        {
                            let next = traces.len();
                            traces.push(Trace {
                                reason: *by,
                                parent: trace,
                            });
                            work.push((Part::Presence(presence.clone()), Some(next), route));
                            false
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                Part::Field(ty, presence) => match presence {
                    Presence::Absent => false,
                    Presence::Present => {
                        work.push((Part::Ty(ty), trace, route));
                        false
                    }
                    // These presences do not make a payload locally available.
                    // In particular, recovery placeholders must not turn an
                    // abandoned sibling into a real occurs route.
                    Presence::Undecided | Presence::Recovered(_) | Presence::Bound(_) => false,
                    Presence::Var(found) if found == var => true,
                    Presence::Var(found)
                        if seen_field_presences.insert((Arc::as_ptr(&ty) as usize, found)) =>
                    {
                        if let Slot::Bound {
                            value: Assigned::Presence(next_presence),
                            by,
                        } = &self.vars[found as usize]
                        {
                            let next = traces.len();
                            traces.push(Trace {
                                reason: *by,
                                parent: trace,
                            });
                            work.push((Part::Field(ty, next_presence.clone()), Some(next), route));
                        } else {
                            work.push((Part::Ty(ty), trace, route));
                        }
                        false
                    }
                    Presence::Var(_) => false,
                },
            };
            if found_target {
                let mut path = Vec::new();
                let mut at = trace;
                while let Some(index) = at {
                    path.push(traces[index].reason);
                    at = traces[index].parent;
                }
                for reason in path.into_iter().rev() {
                    self.note_binding_read(reason);
                }
                return Some(if route.containment {
                    RecursiveCycleShape::Containment
                } else if route.call_input {
                    RecursiveCycleShape::CallInput
                } else {
                    RecursiveCycleShape::Neutral
                });
            }
        }
        None
    }

    /// Lower the level of everything `value` mentions to no more than `var`'s
    /// own, which is what binding a variable does to the variables inside what
    /// it takes.
    ///
    /// The whole of why generalization at a level is right. A variable stands
    /// for a type the binder that minted `var` may see, so everything inside
    /// that type is as old as `var` is however recently it was written:
    /// `fn p => do let q = p.x return q end` mints the field's variable inside the let and
    /// then binds `p`'s — minted outside it — to a type carrying the field, at
    /// which point the field is the lambda's and not the let's. Generalizing
    /// the variables minted inside a let would quantify it; generalizing the
    /// ones still at or above the let's level does not.
    ///
    /// Unlike [`Table::occurs`], this needs every mentioned variable rather
    /// than one exact causal route, so the exhaustive mention collector remains
    /// the appropriate walk here.
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
    fn mentions_ty(&self, ty: &Arc<Ty>, found: &mut Vec<TyVar>) {
        enum Work {
            Ty(Arc<Ty>),
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
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
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
                        Ty::Nat
                        | Ty::Int
                        | Ty::Fixed(_)
                        | Ty::Real
                        | Ty::String
                        | Ty::Bool
                        | Ty::Any
                        | Ty::ForeignValue
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
    fn note_lacks(&mut self, ty: &Arc<Ty>) {
        enum Work {
            Ty(Arc<Ty>),
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
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
                        Ty::Struct(row) => work.push(Work::Row(row.clone(), Shape::Struct)),
                        Ty::Sum(row) => work.push(Work::Row(row.clone(), Shape::Sum)),
                        Ty::Named { symbol, args, .. } => {
                            let symbol = *symbol;
                            for (at, arg) in args.iter().enumerate().rev() {
                                let demand =
                                    self.signatures.params.get(&symbol).and_then(|kinds| {
                                        kinds.get(at).and_then(|kind| {
                                            kind.row()
                                                .map(|(shape, labels)| (shape, labels.clone()))
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
                        | Ty::Fixed(_)
                        | Ty::Real
                        | Ty::String
                        | Ty::Bool
                        | Ty::Any
                        | Ty::ForeignValue
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
                    self.forbid(&flat, shape, &labels);
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
        self.forbid(&flat, shape, &labels);
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
        let origin = self.active_lacks_origin.clone();
        let entries = labels.iter().cloned().map(|label| LacksEntry {
            shape,
            label,
            origin: origin.clone(),
        });
        self.forbid_entries(row, entries);
    }

    fn forbid_entries(&mut self, row: &Row, entries: impl IntoIterator<Item = LacksEntry>) {
        if let Rest::Var(var) = self.canon(row).rest {
            self.forbidden(var, entries);
        }
    }

    /// Put conditions on one variable without losing where each label was
    /// introduced. Existing entries win: they are the earliest grounded
    /// introduction and remain the causal parent through arbitrarily many
    /// tail bindings.
    fn forbidden_labels(
        &mut self,
        var: TyVar,
        shape: Shape,
        labels: impl IntoIterator<Item = String>,
    ) {
        let origin = self.active_lacks_origin.clone();
        self.forbidden(
            var,
            labels.into_iter().map(|label| LacksEntry {
                shape,
                label,
                origin: origin.clone(),
            }),
        );
    }

    fn forbidden(&mut self, var: TyVar, entries: impl IntoIterator<Item = LacksEntry>) {
        let mut entries = entries.into_iter().peekable();
        if entries.peek().is_none() {
            return;
        }
        let recorded = self.lacks.entry(var).or_default();
        for entry in entries {
            match recorded.entry(entry.label.clone()) {
                indexmap::map::Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
                indexmap::map::Entry::Occupied(mut slot)
                    if slot.get().origin.is_none() && entry.origin.is_some() =>
                {
                    slot.insert(entry);
                }
                _ => {}
            }
        }
    }

    fn with_lacks_origin<T>(
        &mut self,
        origin: Option<RowFactOrigin>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = std::mem::replace(&mut self.active_lacks_origin, origin);
        let result = f(self);
        self.active_lacks_origin = previous;
        result
    }

    /// Record one more thing the program requires of its presences.
    ///
    /// A batch that requires nothing is still recorded when it carries an
    /// origin the readers downstream need — a match's coverage is what the
    /// patterns phase asks its reachability questions of, whether or not the
    /// arms happened to relate anything.
    fn require(&mut self, span: Anchor, origin: Origin, formula: Formula) -> ReasonId {
        self.require_because(span, origin, formula, None)
    }

    fn require_because(
        &mut self,
        span: Anchor,
        origin: Origin,
        formula: Formula,
        because: Option<ReasonId>,
    ) -> ReasonId {
        let id = self.batch_id();
        let reason = self.reason(ReasonOrigin::Batch(id), because.into_iter().collect());
        self.store.batches.push(Batch {
            id,
            definition: self.definition,
            at: span,
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
    fn aim(&mut self, from: usize, at: Anchor, span: Anchor) {
        for batch in &mut self.store.batches[from..] {
            match batch {
                Batch {
                    origin: Origin::Instance(_),
                    at: batch_span,
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
                at: batch.at,
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
            match_at: refinement.match_at,
            arm_at: refinement.arm_at,
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
                    at: obligation.at,
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
        let count = self.store.batches.len();
        if self.store_prefix_satisfiable(count) {
            return None;
        }
        // Satisfiability of a prefix only ever goes one way as it lengthens,
        // so the shortest unsatisfiable prefix is found by halving, each half
        // one question to the solver that already holds every batch.
        let (mut low, mut high) = (1, count);
        while low < high {
            let middle = low + (high - low) / 2;
            if self.store_prefix_satisfiable(middle) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Some(low - 1)
    }

    /// The assumptions under which the solver in [`Table::sat`] is the first
    /// `prefix` batches of the store as the solve has resolved them so far:
    /// those batches' guards, and for every variable the solver knows, what
    /// it has been decided to be — a constant assumed as its literal, another
    /// variable as the guard of the pair's equivalence.
    ///
    /// Batches the store has since gained are encoded here first, and ones it
    /// has rolled back are forgotten, so the solver always mirrors the store.
    fn store_assumptions(&self, prefix: usize) -> Vec<sat::Lit> {
        let mut sat = self.sat.borrow_mut();
        let sat = &mut *sat;
        sat.guards.truncate(self.store.batches.len());
        for (at, batch) in self.store.batches.iter().enumerate() {
            let digest = formula_digest(&batch.formula);
            if sat
                .guards
                .get(at)
                .is_some_and(|(encoded, _)| *encoded == digest)
            {
                continue;
            }
            let guard = sat.solver.add_guarded(&batch.formula);
            match sat.guards.get_mut(at) {
                Some(slot) => *slot = (digest, guard),
                None => sat.guards.push((digest, guard)),
            }
        }
        let mut assumptions: Vec<sat::Lit> = sat.guards[..prefix]
            .iter()
            .map(|(_, guard)| *guard)
            .collect();
        let atoms: Vec<Atom> = sat.solver.atoms().collect();
        for atom in atoms {
            let Atom::Var(var) = atom else { continue };
            let resolved = match self.presence_of(&Presence::Var(var)).formula() {
                Formula::True => Some(true),
                Formula::False => Some(false),
                Formula::Atom(other) if other != atom => {
                    let guard = *sat
                        .equivalences
                        .entry((atom, other))
                        .or_insert_with(|| sat.solver.add_equivalence(atom, other));
                    assumptions.push(guard);
                    None
                }
                _ => None,
            };
            if let Some(value) = resolved {
                let literal = sat.solver.atom(atom);
                assumptions.push(if value { literal } else { !literal });
            }
        }
        assumptions
    }

    /// Whether the first `prefix` batches of the store have a model.
    fn store_prefix_satisfiable(&self, prefix: usize) -> bool {
        let assumptions = self.store_assumptions(prefix);
        self.sat.borrow_mut().solver.satisfiable(&assumptions)
    }

    /// Whether the whole store has a model.
    fn store_satisfiable(&self) -> bool {
        self.store_prefix_satisfiable(self.store.batches.len())
    }

    /// Whether the store, assumed satisfiable, forces `var` to `value`.
    fn store_settles(&self, var: TyVar, value: bool) -> bool {
        let mut assumptions = self.store_assumptions(self.store.batches.len());
        let mut sat = self.sat.borrow_mut();
        let literal = sat.solver.atom(Atom::Var(var));
        assumptions.push(if value { !literal } else { literal });
        !sat.solver.satisfiable(&assumptions)
    }

    /// Whether the store, assumed satisfiable, forces `left` and `right` to
    /// agree: neither can be there without the other.
    fn store_equates(&self, left: TyVar, right: TyVar) -> bool {
        let assumptions = self.store_assumptions(self.store.batches.len());
        let mut sat = self.sat.borrow_mut();
        let (left, right) = (
            sat.solver.atom(Atom::Var(left)),
            sat.solver.atom(Atom::Var(right)),
        );
        let mut one_way = assumptions.clone();
        one_way.extend([left, !right]);
        let mut other_way = assumptions;
        other_way.extend([!left, right]);
        !sat.solver.satisfiable(&one_way) && !sat.solver.satisfiable(&other_way)
    }

    /// The variables some batch of the store, as resolved so far, still
    /// names: the only ones it could settle. A variable no batch reaches is
    /// not asked about, which for most definitions is every variable.
    fn store_mentions(&self) -> HashSet<TyVar> {
        let atoms: Vec<Atom> = self.sat.borrow().solver.atoms().collect();
        atoms
            .into_iter()
            .filter_map(|atom| match atom {
                Atom::Var(var) => match self.presence_of(&Presence::Var(var)) {
                    Presence::Var(var) => Some(var),
                    _ => None,
                },
                Atom::Bound(_) => None,
            })
            .collect()
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
    fn fold_back(&mut self, ty: &Arc<Ty>) {
        if !self.store_satisfiable() {
            return;
        }
        let mut found = IndexSet::new();
        self.presences_in(ty, &mut found);
        let mentioned = self.store_mentions();
        for var in found {
            if !mentioned.contains(&var) {
                continue;
            }
            let settled = if self.store_settles(var, true) {
                Presence::Present
            } else if self.store_settles(var, false) {
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
    fn presences_in(&self, ty: &Arc<Ty>, found: &mut IndexSet<TyVar>) {
        enum Work {
            Ty(Arc<Ty>),
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
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Ty));
                        }
                        Ty::Nat
                        | Ty::Int
                        | Ty::Fixed(_)
                        | Ty::Real
                        | Ty::String
                        | Ty::Bool
                        | Ty::Any
                        | Ty::ForeignValue
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
    fn instantiate_local(
        &mut self,
        span: Anchor,
        symbol: Symbol,
        explained: &ExplainedScheme,
    ) -> Arc<Ty> {
        let scheme = &explained.scheme;
        // Only slots occurring directly in the root package are coherent for a
        // lexical value. Nested package nodes are separate production
        // boundaries and must remain fresh even when the same scheme also owns
        // an arrow-value effect at its root.
        enum Work {
            Ty(Arc<Ty>),
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
                    Ty::Array(element) => work.push(Work::Ty(element.clone())),
                    Ty::Mut(region, element) => {
                        work.push(Work::Ty(element.clone()));
                        work.push(Work::Ty(region.clone()));
                    }
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
        let instantiated = self.instantiate_scoped(span, explained, coherent);
        let opened = match &*instantiated {
            Ty::Package(_) => self.open_package(span, &instantiated),
            _ => instantiated,
        };
        self.mark_opened_type(&opened, &explained.provenance);
        opened
    }

    /// Replay a scheme's semantic skeleton over the exact opened structure.
    /// A marker is minted only for a node with contributors of its own; sibling
    /// fields and branches therefore cannot inherit one another's body facts.
    fn mark_opened_type(&mut self, root: &Arc<Ty>, provenance: &SchemeProvenance) {
        enum Opened {
            Ty(Arc<Ty>),
            Row(Row, Arc<Ty>),
            Presence(Presence),
        }
        let Some(mut root_node) = (!provenance.nodes.is_empty()).then_some(0) else {
            return;
        };
        // A lexical package is destructed before it is returned. Follow the
        // explicit body edge rather than shifting an unlabelled preorder.
        if matches!(
            provenance.nodes[root_node].shape,
            ProvenanceShape::Ty(ProvenanceTy::Package)
        ) && !matches!(&**root, Ty::Package(_))
        {
            let Some(body) = provenance.nodes[root_node].children.first() else {
                return;
            };
            root_node = *body;
        }
        let mut work = vec![(Opened::Ty(root.clone()), root_node)];
        while let Some((opened, at)) = work.pop() {
            let Some(node) = provenance.nodes.get(at) else {
                continue;
            };
            let attach = |table: &mut Self, owner: &Arc<Ty>| {
                if node.roots.is_empty() && node.omitted == 0 {
                    return;
                }
                let marker = table.reason_with_omissions(
                    ReasonOrigin::Variable {
                        sort: match node.shape {
                            ProvenanceShape::Ty(_) => VarSort::Type,
                            ProvenanceShape::Row(_) => VarSort::Row,
                            ProvenanceShape::Presence => VarSort::Presence,
                        },
                        subject: Subject::Scheme,
                    },
                    node.roots.clone(),
                    node.omitted,
                );
                let entry = table
                    .opened_provenance
                    .entry(Arc::as_ptr(owner) as usize)
                    .or_insert_with(|| (Arc::downgrade(owner), Vec::new()));
                if entry
                    .0
                    .upgrade()
                    .is_some_and(|opened| Arc::ptr_eq(&opened, owner))
                {
                    entry.1.push(marker);
                } else {
                    *entry = (Arc::downgrade(owner), vec![marker]);
                }
            };
            match opened {
                Opened::Ty(ty) => {
                    let shape = match &*ty {
                        Ty::Arrow(..) => ProvenanceTy::Arrow,
                        Ty::Package(..) => ProvenanceTy::Package,
                        Ty::Array(..) => ProvenanceTy::Array,
                        Ty::Mut(..) => ProvenanceTy::Mut,
                        Ty::Struct(..) => ProvenanceTy::Struct,
                        Ty::Sum(..) => ProvenanceTy::Sum,
                        Ty::Named { args, .. } => ProvenanceTy::Named(args.len()),
                        _ => ProvenanceTy::Leaf,
                    };
                    if node.shape != ProvenanceShape::Ty(shape) {
                        continue;
                    }
                    attach(self, &ty);
                    let children: Vec<Opened> = match &*ty {
                        Ty::Arrow(from, to, effects) => vec![
                            Opened::Ty(from.clone()),
                            Opened::Ty(to.clone()),
                            Opened::Row(effects.clone(), ty.clone()),
                        ],
                        Ty::Package(body) => vec![Opened::Ty(body.clone())],
                        Ty::Mut(region, element) => {
                            vec![Opened::Ty(region.clone()), Opened::Ty(element.clone())]
                        }
                        Ty::Array(element) => vec![Opened::Ty(element.clone())],
                        Ty::Struct(row) | Ty::Sum(row) => {
                            vec![Opened::Row(self.canon(row), ty.clone())]
                        }
                        Ty::Named { args, .. } => args.iter().cloned().map(Opened::Ty).collect(),
                        _ => Vec::new(),
                    };
                    if children.len() != node.children.len() {
                        continue;
                    }
                    work.extend(
                        children
                            .into_iter()
                            .zip(node.children.iter().copied())
                            .rev(),
                    );
                }
                Opened::Row(row, owner) => {
                    let row = self.canon(&row);
                    let layout: Vec<_> = row
                        .labels
                        .iter()
                        .map(|(name, field)| {
                            (
                                name.clone(),
                                !matches!(self.presence_of(&field.presence), Presence::Absent),
                            )
                        })
                        .collect();
                    if node.shape != ProvenanceShape::Row(layout) {
                        continue;
                    }
                    attach(self, &owner);
                    let mut children = Vec::new();
                    for field in row.labels.values() {
                        let presence = self.presence_of(&field.presence);
                        children.push(Opened::Presence(presence.clone()));
                        if !matches!(presence, Presence::Absent) {
                            children.push(Opened::Ty(field.ty.clone()));
                        }
                    }
                    if children.len() != node.children.len() {
                        continue;
                    }
                    work.extend(
                        children
                            .into_iter()
                            .zip(node.children.iter().copied())
                            .rev(),
                    );
                }
                Opened::Presence(presence) => {
                    if node.shape != ProvenanceShape::Presence {
                        continue;
                    }
                    if let Presence::Var(var) = presence
                        && (!node.roots.is_empty() || node.omitted != 0)
                    {
                        let marker = self.reason_with_omissions(
                            ReasonOrigin::Variable {
                                sort: VarSort::Presence,
                                subject: Subject::Scheme,
                            },
                            node.roots.clone(),
                            node.omitted,
                        );
                        let identity = self.var_meta[var as usize].minted_by;
                        self.opened_presence_provenance
                            .entry(var)
                            .and_modify(|entry| {
                                if entry.0 == identity {
                                    entry.1.push(marker);
                                } else {
                                    *entry = (identity, vec![marker]);
                                }
                            })
                            .or_insert_with(|| (identity, vec![marker]));
                    }
                }
            }
        }
    }

    fn region_bounds(&self, root: &Arc<Ty>) -> HashSet<u32> {
        enum Work<'a> {
            Ty(&'a Ty),
            Region(&'a Ty),
            Row(&'a Row, bool),
        }
        let mut regions = HashSet::new();
        let mut work = vec![Work::Ty(root)];
        while let Some(part) = work.pop() {
            match part {
                Work::Region(Ty::Bound(index)) => {
                    regions.insert(*index);
                }
                Work::Region(_) => {}
                Work::Ty(ty) => match ty {
                    Ty::Mut(region, element) => {
                        work.push(Work::Region(region));
                        work.push(Work::Ty(element));
                    }
                    Ty::Package(body) | Ty::Array(body) => work.push(Work::Ty(body)),
                    Ty::Arrow(from, to, row) => {
                        work.push(Work::Ty(from));
                        work.push(Work::Ty(to));
                        work.push(Work::Row(row, true));
                    }
                    Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row, false)),
                    Ty::Named { symbol, args, .. } => {
                        for (index, arg) in args.iter().enumerate() {
                            if self
                                .signatures
                                .params
                                .get(symbol)
                                .and_then(|kinds| kinds.get(index))
                                .is_some_and(|kind| kind.sense() == Sense::Region)
                            {
                                work.push(Work::Region(arg));
                            } else {
                                work.push(Work::Ty(arg));
                            }
                        }
                    }
                    _ => {}
                },
                Work::Row(row, effects) => {
                    if let Rest::More(more) = &row.rest {
                        work.push(Work::Row(more, effects));
                    }
                    for (name, field) in &row.labels {
                        if matches!(field.presence, Presence::Absent) {
                            continue;
                        }
                        if effects && let Ty::Struct(args) = &*field.ty {
                            for (index, arg) in args.labels.values().enumerate() {
                                let region = name == &crate::types::mutation_effect().row_key()
                                    || self
                                        .signatures
                                        .effect_kinds
                                        .get(name)
                                        .and_then(|kinds| kinds.get(index))
                                        .is_some_and(|kind| kind.sense() == Sense::Region);
                                work.push(if region {
                                    Work::Region(&arg.ty)
                                } else {
                                    Work::Ty(&arg.ty)
                                });
                            }
                        } else {
                            work.push(Work::Ty(&field.ty));
                        }
                    }
                }
            }
        }
        regions
    }

    fn instantiate_scoped(
        &mut self,
        span: Anchor,
        explained: &ExplainedScheme,
        coherent: Option<(Symbol, IndexSet<u32>)>,
    ) -> Arc<Ty> {
        let scheme = &explained.scheme;
        let regions = self.region_bounds(scheme.body());
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
                            let fresh = Presence::Var(
                                self.mint_from_budgeted(
                                    VarSort::Presence,
                                    Subject::Instance,
                                    explained
                                        .provenance
                                        .quantified
                                        .get(at as usize)
                                        .map(|slot| slot.roots.clone())
                                        .unwrap_or_default(),
                                    explained
                                        .provenance
                                        .quantified
                                        .get(at as usize)
                                        .map_or(0, |slot| slot.omitted),
                                ),
                            );
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
                false => Assigned::Ty(Arc::new(Ty::plain(Ty::Var(
                    self.mint_from_budgeted(
                        explained
                            .provenance
                            .quantified
                            .get(at as usize)
                            .map_or_else(
                                || {
                                    if regions.contains(&at) {
                                        VarSort::Region
                                    } else {
                                        VarSort::Type
                                    }
                                },
                                |slot| slot.sort,
                            ),
                        Subject::Instance,
                        explained
                            .provenance
                            .quantified
                            .get(at as usize)
                            .map(|slot| slot.roots.clone())
                            .unwrap_or_default(),
                        explained
                            .provenance
                            .quantified
                            .get(at as usize)
                            .map_or(0, |slot| slot.omitted),
                    ),
                )))),
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
    fn register_package_guarantees(&mut self, ty: &Arc<Ty>, formula: Formula) -> Formula {
        enum Work {
            Ty(Arc<Ty>, bool),
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
                    Ty::Array(element) => work.push(Work::Ty(element.clone(), false)),
                    Ty::Mut(region, element) => {
                        work.push(Work::Ty(element.clone(), false));
                        work.push(Work::Ty(region.clone(), false));
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
    fn open_package(&mut self, span: Anchor, package: &Arc<Ty>) -> Arc<Ty> {
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
        ty: &Arc<Ty>,
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
        ty: &Arc<Ty>,
        found: &mut IndexMap<String, (String, Presence)>,
        formula_shape: &mut Option<Shape>,
        formula: &Formula,
    ) {
        enum Work {
            Ty(Arc<Ty>),
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
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
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
    fn unfolded(&mut self, aliases: &IndexMap<Symbol, Scheme>, ty: &Arc<Ty>) -> Arc<Ty> {
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
            if Arc::ptr_eq(&next, &exposed) {
                break;
            }
            exposed = next;
        }
        if !Arc::ptr_eq(&exposed, ty) {
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
    /// Said once per variable across the group. A rigid that reaches
    /// two schemes it does not belong to is still one annotation to rewrite,
    /// and a reader sent to the same line twice learns nothing the second time.
    fn escapes(
        &mut self,
        ty: &Arc<Ty>,
        owned: &[u32],
        destination_name: Arc<str>,
        destination_span: Anchor,
        errors: &mut Vec<Error>,
    ) {
        // Lowering represents `_ = value` as a synthetic let so that `value`
        // is still checked and sequenced. Nothing can refer to that let, so it
        // publishes no type for a caller-owned choice to escape through.
        if &*destination_name == "%discard" {
            return;
        }

        let mut found = IndexMap::new();
        self.rigids_in(ty, &mut found);
        for (id, name) in found {
            if owned.contains(&id) || !self.escaped.insert(id) {
                continue;
            }
            let declared = self.rigids[&id];
            let error_id = self.error_id();
            let constraint = ConstraintId {
                scope: error_id.scope,
                index: error_id.index,
            };
            let full_facts = vec![
                ExplanationFact {
                    direct: false,
                    at: declared,
                    constraint,
                    origin: ConstraintOrigin::ContextualCheck,
                    subject: Subject::Annotation,
                    payload: ExplanationFactPayload::CallerChoiceDeclaration,
                },
                ExplanationFact {
                    direct: false,
                    at: destination_span,
                    constraint,
                    origin: ConstraintOrigin::Binding,
                    subject: Subject::Binding,
                    payload: ExplanationFactPayload::CallerChoiceDestination,
                },
            ];
            errors.push(Error {
                id: error_id,
                cause: ErrorCause::Direct,
                at: declared,
                kind: ErrorKind::RigidEscapes {
                    name,
                    declared,
                    destination: ty.clone(),
                    destination_name: destination_name.clone(),
                    destination_span,
                },
                explanation: Some(InferenceExplanation {
                    full_facts,
                    abridged: vec![0, 1],
                    pivot: None,
                    omitted_facts: 0,
                    contradiction: Contradiction {
                        kind: ContradictionKind::CallerChoiceEscape,
                        left: TypeDescription::Undecided,
                        right: describe_type(ty),
                        row: None,
                        recursive: None,
                        repairs: [
                            RepairDirection::ChangeFirstUse,
                            RepairDirection::ChangeSecondUse,
                        ],
                    },
                    cause: ExplanationCause {
                        error: error_id,
                        seed: None,
                        constraints: vec![constraint],
                        reasons: Vec::new(),
                        omitted_reasons: 0,
                    },
                }),
            });
        }
    }

    /// Every rigid a type mentions, by id, with the spelling it prints as, in
    /// the order the type mentions them. A leaf wherever it appears, so this is
    /// the ordinary walk with one arm that collects — and two rows a rigid can
    /// tail, a sum's cases and an arrow's effects, read the same way.
    fn rigids_in(&self, ty: &Arc<Ty>, found: &mut IndexMap<u32, Arc<str>>) {
        enum Work {
            Ty(Arc<Ty>),
            Row(Row),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
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
    fn lacked(
        &self,
        var: TyVar,
        value: &Assigned,
    ) -> Option<Vec<(LacksEntry, Presence, Option<RowFactOrigin>)>> {
        let lacks = self.lacks.get(&var)?;
        let row = self.canon(&value.as_row());
        let introduction = match row.rest {
            Rest::Var(tail) => self.lacks.get(&tail),
            _ => None,
        };
        let named = row
            .labels
            .iter()
            .filter_map(|(name, field)| {
                let lacked = lacks.get(name)?.clone();
                let introduced = introduction
                    .and_then(|entries| entries.get(name))
                    .and_then(|entry| entry.origin.clone())
                    .or_else(|| self.active_lacks_origin.clone());
                Some((lacked, self.presence_of(&field.presence), introduced))
            })
            .collect();
        Some(named)
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
        let Some(entries) = self.lacks.get(&var).cloned() else {
            return;
        };
        let row = value.as_row();
        self.forbid_entries(&row, entries.into_values());
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
        self.lacks
            .get(&var)
            .and_then(|entries| entries.first().map(|(_, entry)| entry.shape))
            .unwrap_or(Shape::Struct)
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
    fn close_effects(&mut self, ty: &Arc<Ty>, level: u32) {
        let mut counted: IndexMap<TyVar, usize> = IndexMap::new();
        self.count_effects(ty, &mut counted);
        for (var, count) in counted {
            if count != 1 || self.levels[var as usize] < level {
                continue;
            }
            self.default_bind(
                var,
                Assigned::Row(Arc::new(Row::closed())),
                DefaultBinding::CloseEffects,
                DefaultAssignment::EmptyRow,
                Vec::new(),
            );
        }
    }

    /// A handler's optional outer label is an allowance, not an inferred
    /// operation. Minimize unshared allowances and their guarded views while
    /// preserving every admitted assignment of caller-owned presences. A
    /// conditional rethrow therefore survives; a guarded swallowing handler
    /// does not publish a phantom effect just because its views have formulas.
    fn close_handler_presences(&mut self, ty: &Arc<Ty>, level: u32) {
        if self.handler_presences.is_empty() {
            return;
        }
        let mut mentions = Vec::new();
        self.mentions_ty(ty, &mut mentions);
        let mut counted = IndexMap::<TyVar, usize>::new();
        for var in mentions {
            *counted.entry(var).or_default() += 1;
        }
        let mentioned = self.store_mentions();
        let mut protected = HashSet::new();
        // A presence in a callable's input belongs to its caller even if a
        // match family gave it a fresh identity separate from the output.
        let mut inputs = IndexSet::new();
        let mut work = vec![ty.clone()];
        while let Some(ty) = work.pop() {
            let ty = self.resolve(&ty);
            let row = match &*ty {
                Ty::Arrow(from, to, row) => {
                    self.presences_in(from, &mut inputs);
                    work.push(to.clone());
                    Some(row)
                }
                Ty::Struct(row) | Ty::Sum(row) => Some(row),
                Ty::Package(ty) | Ty::Array(ty) => {
                    work.push(ty.clone());
                    None
                }
                Ty::Mut(region, ty) => {
                    work.extend([region.clone(), ty.clone()]);
                    None
                }
                Ty::Named { args, .. } => {
                    work.extend(args.iter().cloned());
                    None
                }
                _ => None,
            };
            if let Some(row) = row {
                work.extend(
                    self.canon(row)
                        .labels
                        .values()
                        .filter(|field| {
                            !matches!(self.presence_of(&field.presence), Presence::Absent)
                        })
                        .map(|field| field.ty.clone()),
                );
            }
        }
        protected.extend(inputs);
        for (var, meta) in self.var_meta.iter().enumerate() {
            let var = var as TyVar;
            if meta.sort == VarSort::Presence
                && !self.handler_presences.contains(&var)
                && let Presence::Var(root) = self.presence_of(&Presence::Var(var))
            {
                protected.insert(root);
            }
        }
        let mut candidates: Vec<_> = self
            .handler_presences
            .iter()
            .copied()
            .filter(|var| {
                let count = counted.get(var).copied().unwrap_or_default();
                count <= 1
                    && (count == 1 || mentioned.contains(var))
                    && matches!(self.vars[*var as usize], Slot::Unbound)
                    && self.levels[*var as usize] >= level
                    && !protected.contains(var)
            })
            .collect();
        candidates.sort_unstable();
        // Only close this binding's allowances and their formula-connected
        // views. An earlier sibling's returned callback may still be waiting
        // for its own generalization at the same level.
        let eligible: HashSet<_> = candidates.iter().copied().collect();
        let mut reachable: HashSet<_> = candidates
            .iter()
            .copied()
            .filter(|var| counted.get(var) == Some(&1))
            .collect();
        let connections: Vec<Vec<_>> = self
            .store
            .batches
            .iter()
            .map(|batch| {
                let mut atoms = Vec::new();
                self.resolved(&batch.formula).atoms(&mut atoms);
                atoms
                    .into_iter()
                    .filter_map(|atom| match atom {
                        Atom::Var(var) if eligible.contains(&var) => Some(var),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        loop {
            let before = reachable.len();
            for connection in &connections {
                if connection.iter().any(|var| reachable.contains(var)) {
                    reachable.extend(connection.iter().copied());
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        candidates.retain(|var| reachable.contains(var));
        if candidates.is_empty() {
            return;
        }
        let mut known = self.known();
        let mut keep = Vec::new();
        known.atoms(&mut keep);
        keep.retain(
            |atom| !matches!(atom, Atom::Var(var) if candidates.binary_search(var).is_ok()),
        );
        let Some(admitted) = sat::project_exact(&known, &keep) else {
            return;
        };
        for var in candidates {
            let absent = known.substitute(&|atom| {
                if atom == Atom::Var(var) {
                    Formula::False
                } else {
                    Formula::Atom(atom)
                }
            });
            if let Some(realized) = sat::project_exact(&absent, &keep)
                && sat::entails(&admitted, &realized)
            {
                self.default_bind(
                    var,
                    Assigned::Presence(Presence::Absent),
                    DefaultBinding::CloseEffects,
                    DefaultAssignment::Absent,
                    Vec::new(),
                );
                self.handler_absences.insert(var);
                known = absent;
            }
        }
    }

    /// How often each still-open effect row variable appears in the effect
    /// position of an arrow inside `ty`.
    ///
    /// Effect positions alone, which is what makes the count well defined: a
    /// variable's sort is fixed where it was minted, and nothing can unify a
    /// sum's tail with an arrow's effects, so a variable found here is found
    /// nowhere but here.
    fn count_effects(&self, ty: &Arc<Ty>, found: &mut IndexMap<TyVar, usize>) {
        enum Work {
            Ty(Arc<Ty>),
            Row(Row, bool),
        }
        let mut work = vec![Work::Ty(ty.clone())];
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty) => {
                    let ty = self.resolve(&ty);
                    match &*ty {
                        Ty::Package(body) => work.push(Work::Ty(body.clone())),
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
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
    /// what makes `fn p => do let q = p.x return q end` one type rather than two — see
    /// [`Table::demote`] — and it is why a scheme published here may still
    /// mention the table. A definition's own scheme never does: its level is 0,
    /// and nothing is below that.
    fn generalize(&self, ty: &Arc<Ty>, level: u32, formula: Formula) -> (Scheme, Subst) {
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
            &self.signatures.variances,
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

    /// Capture only causal bindings which survive in the published semantic
    /// shape. This preorder is replayed over the opened scheme, bounding work
    /// by semantic nodes rather than by the size of the defining body.
    fn scheme_provenance(
        &self,
        ty: &Arc<Ty>,
        published: &Arc<Ty>,
        subst: &Subst,
        count: u32,
    ) -> SchemeProvenance {
        enum Work {
            Ty(Arc<Ty>, Option<usize>),
            Row(Row, Option<usize>),
            Presence(Presence, Option<usize>),
        }
        let mut quantified = (0..count)
            .map(|_| QuantifiedProvenance {
                sort: VarSort::Type,
                roots: Vec::new(),
                omitted: 0,
            })
            .collect::<Vec<_>>();
        for (var, at) in &subst.types {
            if let Some(slot) = quantified.get_mut(*at as usize) {
                slot.sort = self.var_meta[*var as usize].sort;
                slot.roots.push(self.var_meta[*var as usize].minted_by);
            }
        }
        for (var, at) in &subst.presences {
            if let Some(slot) = quantified.get_mut(*at as usize) {
                slot.sort = VarSort::Presence;
                slot.roots.push(self.var_meta[*var as usize].minted_by);
            }
        }
        for (id, at) in &subst.rigids {
            if let Some(slot) = quantified.get_mut(*at as usize) {
                slot.sort = subst.rigid_sorts.get(id).copied().unwrap_or(VarSort::Type);
            }
        }
        let mut nodes = Vec::<ProvenanceNode>::new();
        let mut work = vec![Work::Ty(ty.clone(), None)];
        while let Some(item) = work.pop() {
            let parent = match &item {
                Work::Ty(_, p) | Work::Row(_, p) | Work::Presence(_, p) => *p,
            };
            let (shape, mut roots, children): (ProvenanceShape, Vec<ReasonId>, Vec<Work>) =
                match item {
                    Work::Ty(mut ty, _) => {
                        let mut roots = Vec::new();
                        if let Some((opened, reasons)) =
                            self.opened_provenance.get(&(Arc::as_ptr(&ty) as usize))
                            && opened
                                .upgrade()
                                .is_some_and(|opened| Arc::ptr_eq(&opened, &ty))
                        {
                            roots.extend(reasons);
                        }
                        let mut seen = HashSet::new();
                        while let Ty::Var(var) = &*ty {
                            if !seen.insert(*var) {
                                break;
                            }
                            match &self.vars[*var as usize] {
                                Slot::Bound {
                                    value: Assigned::Ty(next),
                                    by,
                                } => {
                                    roots.push(*by);
                                    ty = next.clone();
                                }
                                _ => break,
                            }
                        }
                        let (tag, children) = match &*ty {
                            Ty::Arrow(from, to, effects) => (
                                ProvenanceTy::Arrow,
                                vec![
                                    Work::Ty(from.clone(), None),
                                    Work::Ty(to.clone(), None),
                                    Work::Row(effects.clone(), None),
                                ],
                            ),
                            Ty::Package(body) => {
                                (ProvenanceTy::Package, vec![Work::Ty(body.clone(), None)])
                            }
                            Ty::Mut(region, element) => (
                                ProvenanceTy::Mut,
                                vec![
                                    Work::Ty(region.clone(), None),
                                    Work::Ty(element.clone(), None),
                                ],
                            ),
                            Ty::Array(element) => {
                                (ProvenanceTy::Array, vec![Work::Ty(element.clone(), None)])
                            }
                            Ty::Struct(row) => {
                                (ProvenanceTy::Struct, vec![Work::Row(row.clone(), None)])
                            }
                            Ty::Sum(row) => (ProvenanceTy::Sum, vec![Work::Row(row.clone(), None)]),
                            Ty::Named { args, .. } => (
                                ProvenanceTy::Named(args.len()),
                                args.iter().cloned().map(|ty| Work::Ty(ty, None)).collect(),
                            ),
                            _ => (ProvenanceTy::Leaf, Vec::new()),
                        };
                        (ProvenanceShape::Ty(tag), roots, children)
                    }
                    Work::Row(row, _) => {
                        let canonical = self.canon(&row);
                        let mut roots = Vec::new();
                        let mut rest = row.rest.clone();
                        let mut seen = HashSet::new();
                        while let Rest::Var(var) = rest {
                            if !seen.insert(var) {
                                break;
                            }
                            match &self.vars[var as usize] {
                                Slot::Bound {
                                    value: Assigned::Row(next),
                                    by,
                                } => {
                                    roots.push(*by);
                                    rest = next.rest.clone();
                                }
                                _ => break,
                            }
                        }
                        let mut layout = Vec::new();
                        let mut children = Vec::new();
                        for (name, field) in &canonical.labels {
                            let presence = self.presence_of(&field.presence);
                            let payload = !matches!(presence, Presence::Absent);
                            layout.push((name.clone(), payload));
                            children.push(Work::Presence(presence, None));
                            if payload {
                                children.push(Work::Ty(field.ty.clone(), None));
                            }
                        }
                        (ProvenanceShape::Row(layout), roots, children)
                    }
                    Work::Presence(mut presence, _) => {
                        let mut roots = Vec::new();
                        let mut seen = HashSet::new();
                        while let Presence::Var(var) = presence {
                            if !seen.insert(var) {
                                break;
                            }
                            match &self.vars[var as usize] {
                                Slot::Bound {
                                    value: Assigned::Presence(next),
                                    by,
                                } => {
                                    roots.push(*by);
                                    presence = next.clone();
                                }
                                _ => break,
                            }
                        }
                        (ProvenanceShape::Presence, roots, Vec::new())
                    }
                };
            roots.retain(|root| self.publishable_reason(*root));
            roots.sort_unstable();
            roots.dedup();
            let id = nodes.len();
            nodes.push(ProvenanceNode {
                shape,
                roots,
                omitted: 0,
                children: Vec::new(),
            });
            if let Some(parent) = parent {
                nodes[parent].children.push(id);
            }
            for child in children.into_iter().rev() {
                let child = match child {
                    Work::Ty(x, _) => Work::Ty(x, Some(id)),
                    Work::Row(x, _) => Work::Row(x, Some(id)),
                    Work::Presence(x, _) => Work::Presence(x, Some(id)),
                };
                work.push(child);
            }
        }
        // Positive-presence packaging runs after zonking and may synthesize a
        // package at any covariant structural position. Rebuild the skeleton
        // from the exact body that is actually published, carrying evidence
        // through matching nodes and inserting evidence-free package wrappers
        // in lockstep. Opening can therefore replay nested synthesized packages
        // without guessing from the pre-packaging body.
        enum Published {
            Ty(Arc<Ty>, Option<usize>, Option<usize>),
            Row(Row, Option<usize>, Option<usize>),
            Presence(Option<usize>, Option<usize>),
        }
        let before = nodes;
        let mut exact = Vec::<ProvenanceNode>::new();
        let mut work = vec![Published::Ty(published.clone(), None, Some(0))];
        while let Some(item) = work.pop() {
            let (shape, parent, old, children): (_, _, _, Vec<Published>) = match item {
                Published::Ty(ty, parent, old) => {
                    let (tag, child_types): (ProvenanceTy, Vec<Published>) = match &*ty {
                        Ty::Arrow(from, to, effects) => (
                            ProvenanceTy::Arrow,
                            vec![
                                Published::Ty(from.clone(), None, None),
                                Published::Ty(to.clone(), None, None),
                                Published::Row(effects.clone(), None, None),
                            ],
                        ),
                        Ty::Package(body) => (
                            ProvenanceTy::Package,
                            vec![Published::Ty(body.clone(), None, None)],
                        ),
                        Ty::Mut(region, element) => (
                            ProvenanceTy::Mut,
                            vec![
                                Published::Ty(region.clone(), None, None),
                                Published::Ty(element.clone(), None, None),
                            ],
                        ),
                        Ty::Array(element) => (
                            ProvenanceTy::Array,
                            vec![Published::Ty(element.clone(), None, None)],
                        ),
                        Ty::Struct(row) => (
                            ProvenanceTy::Struct,
                            vec![Published::Row(row.clone(), None, None)],
                        ),
                        Ty::Sum(row) => (
                            ProvenanceTy::Sum,
                            vec![Published::Row(row.clone(), None, None)],
                        ),
                        Ty::Named { args, .. } => (
                            ProvenanceTy::Named(args.len()),
                            args.iter()
                                .cloned()
                                .map(|arg| Published::Ty(arg, None, None))
                                .collect(),
                        ),
                        _ => (ProvenanceTy::Leaf, Vec::new()),
                    };
                    let shape = ProvenanceShape::Ty(tag);
                    let synthesized = matches!(shape, ProvenanceShape::Ty(ProvenanceTy::Package))
                        && old.is_some_and(|at| before.get(at).is_some_and(|n| n.shape != shape));
                    let old_children = if synthesized {
                        vec![old]
                    } else {
                        old.and_then(|at| before.get(at))
                            .filter(|node| node.shape == shape)
                            .map(|node| node.children.iter().copied().map(Some).collect())
                            .unwrap_or_default()
                    };
                    let children = child_types
                        .into_iter()
                        .enumerate()
                        .map(|(at, child)| match child {
                            Published::Ty(ty, _, _) => {
                                Published::Ty(ty, None, old_children.get(at).copied().flatten())
                            }
                            Published::Row(row, _, _) => {
                                Published::Row(row, None, old_children.get(at).copied().flatten())
                            }
                            Published::Presence(_, _) => unreachable!(),
                        })
                        .collect();
                    (
                        shape,
                        parent,
                        (!synthesized).then_some(old).flatten(),
                        children,
                    )
                }
                Published::Row(row, parent, old) => {
                    let layout: Vec<_> = row
                        .labels
                        .iter()
                        .map(|(name, field)| {
                            (name.clone(), !matches!(field.presence, Presence::Absent))
                        })
                        .collect();
                    let shape = ProvenanceShape::Row(layout);
                    let old_children = old
                        .and_then(|at| before.get(at))
                        .filter(|node| node.shape == shape)
                        .map(|node| node.children.clone())
                        .unwrap_or_default();
                    let mut children = Vec::new();
                    let mut at = 0;
                    for field in row.labels.values() {
                        children.push(Published::Presence(None, old_children.get(at).copied()));
                        at += 1;
                        if !matches!(field.presence, Presence::Absent) {
                            children.push(Published::Ty(
                                field.ty.clone(),
                                None,
                                old_children.get(at).copied(),
                            ));
                            at += 1;
                        }
                    }
                    (shape, parent, old, children)
                }
                Published::Presence(parent, old) => {
                    (ProvenanceShape::Presence, parent, old, Vec::new())
                }
            };
            let id = exact.len();
            let (roots, omitted) = old
                .and_then(|at| before.get(at))
                .filter(|node| node.shape == shape)
                .map(|node| (node.roots.clone(), node.omitted))
                .unwrap_or_default();
            exact.push(ProvenanceNode {
                shape,
                roots,
                omitted,
                children: Vec::new(),
            });
            if let Some(parent) = parent {
                exact[parent].children.push(id);
            }
            for child in children.into_iter().rev() {
                work.push(match child {
                    Published::Ty(ty, _, old) => Published::Ty(ty, Some(id), old),
                    Published::Row(row, _, old) => Published::Row(row, Some(id), old),
                    Published::Presence(_, old) => Published::Presence(Some(id), old),
                });
            }
        }
        let mut nodes = exact;
        for slot in &mut quantified {
            slot.roots.sort_unstable();
            slot.roots.dedup();
            slot.roots.retain(|root| self.publishable_reason(*root));
        }
        // Bound each position and then the whole publication, preserving the
        // earliest defining and latest relevant endpoints at both levels.
        const PER_POSITION: usize = 64;
        const TOTAL: usize = 256;
        for node in &mut nodes {
            node.omitted += keep_endpoints(&mut node.roots, PER_POSITION);
        }
        for slot in &mut quantified {
            slot.omitted += keep_endpoints(&mut slot.roots, PER_POSITION);
        }
        let mut all = nodes
            .iter()
            .flat_map(|n| n.roots.iter().copied())
            .chain(quantified.iter().flat_map(|q| q.roots.iter().copied()))
            .collect::<Vec<_>>();
        all.sort_unstable();
        all.dedup();
        if keep_endpoints(&mut all, TOTAL) > 0 {
            let keep: HashSet<_> = all.iter().copied().collect();
            for node in &mut nodes {
                let old = node.roots.len();
                node.roots.retain(|root| keep.contains(root));
                node.omitted += old - node.roots.len();
            }
            for slot in &mut quantified {
                let old = slot.roots.len();
                slot.roots.retain(|root| keep.contains(root));
                slot.omitted += old - slot.roots.len();
            }
        }
        SchemeProvenance { nodes, quantified }
    }

    /// Preserve a written contract as the sole defining contributor. Its body
    /// is walked only to reproduce the compact semantic skeleton; recovered or
    /// incidental implementation facts are never published through it.
    fn authoritative_provenance(
        &mut self,
        ty: &Arc<Ty>,
        published: &Arc<Ty>,
        subst: &Subst,
        count: u32,
        constraints: &[Constraint],
        annotation_span: Anchor,
    ) -> SchemeProvenance {
        let mut provenance = self.scheme_provenance(ty, published, subst, count);
        for node in &mut provenance.nodes {
            node.roots.clear();
            node.omitted = 0;
        }
        for slot in &mut provenance.quantified {
            slot.roots.clear();
            slot.omitted = 0;
        }
        let mut work: Vec<_> = constraints.iter().rev().collect();
        let mut contract = None;
        while let Some(constraint) = work.pop() {
            let exact = (constraint.subjects.primary == Subject::Annotation
                && constraint.subjects.primary_span == Some(annotation_span))
                || (constraint.subjects.secondary == Some(Subject::Annotation)
                    && constraint.subjects.secondary_span == Some(annotation_span));
            if constraint.origin == ConstraintOrigin::ContextualCheck && exact {
                contract = Some(self.reason(ReasonOrigin::Contract(constraint.id), Vec::new()));
                break;
            }
            match &constraint.kind {
                ConstraintKind::Let { value, body, .. } => {
                    work.extend(body.iter().rev());
                    work.extend(value.iter().rev());
                }
                ConstraintKind::Match { arms, .. } => {
                    for arm in arms.iter().rev() {
                        work.push(&arm.result);
                        work.extend(arm.constraints.iter().rev());
                    }
                }
                _ => {}
            }
        }
        if let Some(contract) = contract {
            // Structural decomposition of a written contract remains the same
            // contract at each corresponding semantic position.
            for node in &mut provenance.nodes {
                node.roots.push(contract);
            }
        }
        provenance
    }

    /// Recovery nodes and rolled-back work can settle the compiler enough to
    /// continue, but are not facts a later consumer may attribute to a scheme.
    ///
    /// A reason from another scope reached this table through a published
    /// scheme, and its own table decided its publishability before publishing
    /// it: it is taken as read.
    fn publishable_reason(&self, seed: ReasonId) -> bool {
        if seed.scope != self.scope {
            return true;
        }
        self.reasons
            .get(seed.index as usize)
            .is_some_and(|reason| reason.reachable)
            && !self.unpublishable_reasons.contains(&seed)
    }

    /// What the scheme of a type generalized here should require of its
    /// presences: everything the store says about the presences the type still
    /// mentions, with every other variable existentially eliminated, in the
    /// canonical form R12 prints.
    ///
    /// Silent once something has already flipped the store: the cascade rule.
    /// A `where false` on every scheme downstream of one contradiction is the
    /// same mistake said in as many places as the program has definitions.
    fn required(&self, ty: &Arc<Ty>) -> Formula {
        self.required_given(ty, &Formula::True)
    }

    /// What a type generalized inside a reachable arm requires while that arm's
    /// premise holds. Conjoining the premise before projection prevents
    /// existential elimination from turning `E -> Q(local)` into `true`.
    fn required_given(&self, ty: &Arc<Ty>, premise: &Formula) -> Formula {
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
    /// definition under. See [`Semantics::promises`].
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
    /// Only for [`Semantics::locals`], and only once the solve that could still
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
            rigid_sorts: HashMap::new(),
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
    fn quantify(&self, ty: &Arc<Ty>, subst: &mut Subst, level: u32) {
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
    fn quantify_walk(&self, ty: &Arc<Ty>, subst: &mut Subst, level: u32, presences: bool) {
        enum Work {
            Ty(Arc<Ty>),
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
                        Ty::Array(element) => work.push(Work::Ty(element.clone())),
                        Ty::Mut(region, element) => {
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
                        }
                        Ty::Var(var) if !presences => self.quantify_var(*var, subst, level),
                        Ty::Rigid { id, .. } if !presences => {
                            let next = subst.next();
                            subst.rigids.entry(*id).or_insert(next);
                            subst.rigid_sorts.entry(*id).or_insert(
                                if self.region_rigids.contains(id) {
                                    VarSort::Region
                                } else {
                                    VarSort::Type
                                },
                            );
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
                    subst.rigid_sorts.entry(id).or_insert(VarSort::Row);
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
    fn zonk(&self, ty: &Arc<Ty>, subst: &Subst) -> Arc<Ty> {
        enum Work {
            Ty(Arc<Ty>),
            Arrow,
            Package,
            Array,
            Mut,
            Struct,
            Sum,
            Named {
                symbol: Symbol,
                name: Arc<str>,
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
                        Ty::Var(var) => types.push(Arc::new(
                            subst
                                .types
                                .get(var)
                                .map_or(Ty::Var(*var), |at| Ty::Bound(*at)),
                        )),
                        Ty::Rigid { id, .. } => types.push(match subst.rigids.get(id) {
                            Some(index) => Arc::new(Ty::Bound(*index)),
                            None if self.region_rigids.contains(id) => ty.clone(),
                            None => panic!("a quantified type rigid must have a binder"),
                        }),
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
                        Ty::Array(element) => {
                            work.push(Work::Array);
                            work.push(Work::Ty(element.clone()));
                        }
                        Ty::Mut(region, element) => {
                            work.push(Work::Mut);
                            work.push(Work::Ty(element.clone()));
                            work.push(Work::Ty(region.clone()));
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
                        other => types.push(Arc::new(other.clone())),
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
                    types.push(Arc::new(Ty::Arrow(from, to, effects)));
                }
                Work::Package => {
                    let body = types.pop().expect("zonked package body");
                    types.push(Arc::new(Ty::Package(body)));
                }
                Work::Array => {
                    let element = types.pop().expect("zonked array element");
                    types.push(Arc::new(Ty::Array(element)));
                }
                Work::Mut => {
                    let element = types.pop().expect("cell element");
                    let region = types.pop().expect("cell region");
                    types.push(Arc::new(Ty::Mut(region, element)));
                }
                Work::Struct => {
                    let row = rows.pop().expect("zonked struct row");
                    types.push(Arc::new(Ty::Struct(row)));
                }
                Work::Sum => {
                    let row = rows.pop().expect("zonked sum row");
                    types.push(Arc::new(Ty::Sum(row)));
                }
                Work::Named { symbol, name, args } => {
                    let mut opened = Vec::with_capacity(args);
                    for _ in 0..args {
                        opened.push(types.pop().expect("zonked named argument"));
                    }
                    opened.reverse();
                    types.push(Arc::new(Ty::Named {
                        symbol,
                        name,
                        args: opened.into(),
                    }));
                }
                Work::BuiltRow { labels, rest } => {
                    let mut built = Vec::with_capacity(labels.len());
                    for (name, presence) in labels.into_iter().rev() {
                        let ty = match presence {
                            Presence::Absent => Arc::new(Ty::Undecided),
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
    fn close(&self, ty: &Arc<Ty>, subst: &mut Subst) -> Arc<Ty> {
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
            TermKind::Struct { fields, spread } => {
                for field in fields.values_mut() {
                    self.zonk_term(&mut field.value, subst);
                }
                if let Some(spread) = spread {
                    self.zonk_term(&mut spread.value, subst);
                }
            }
            TermKind::Array(items) => {
                for item in items {
                    self.zonk_term(&mut item.value, subst);
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
            | TermKind::Fixed(_)
            | TermKind::Integer(_)
            | TermKind::Real(_)
            | TermKind::String(_)
            | TermKind::Bool(_)
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
            ErrorKind::RuntimeTypeInformation { message } => ErrorKind::RuntimeTypeInformation {
                message: message.clone(),
            },
            ErrorKind::NotAStruct { base, demand } => ErrorKind::NotAStruct {
                base: self.close(base, subst),
                demand: *demand,
            },
            ErrorKind::Mismatch { expected, actual } => ErrorKind::Mismatch {
                expected: self.close(expected, subst),
                actual: self.close(actual, subst),
            },
            ErrorKind::EffectArgument {
                effect,
                position,
                cause,
            } => ErrorKind::EffectArgument {
                effect: effect.clone(),
                position: *position,
                cause: Box::new(self.zonk_error(cause, subst)),
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
            ErrorKind::RigidEscapes {
                name,
                declared,
                destination,
                destination_name,
                destination_span,
            } => ErrorKind::RigidEscapes {
                name: name.clone(),
                declared: *declared,
                destination: self.close(destination, subst),
                destination_name: destination_name.clone(),
                destination_span: *destination_span,
            },
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
            ErrorKind::RepeatedField {
                shape,
                field,
                introduction,
                forbidden,
            } => ErrorKind::RepeatedField {
                shape: *shape,
                field: field.clone(),
                introduction: introduction.clone(),
                forbidden: forbidden.clone(),
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
            ErrorKind::CallbackEffectsNotCovered {
                missing_effects,
                extern_effects,
                callback_path,
                callback_type,
                extern_name,
                issues,
            } => ErrorKind::CallbackEffectsNotCovered {
                missing_effects: missing_effects.clone(),
                extern_effects: extern_effects.clone(),
                callback_path: callback_path.clone(),
                callback_type: callback_type.clone(),
                extern_name: extern_name.clone(),
                issues: issues.clone(),
            },
            ErrorKind::PolymorphicExternBoundary {
                variable,
                variable_kind,
                position,
                extern_name,
                leaves,
                callback_issues,
            } => ErrorKind::PolymorphicExternBoundary {
                variable: variable.clone(),
                variable_kind: *variable_kind,
                position: position.clone(),
                extern_name: extern_name.clone(),
                leaves: leaves.clone(),
                callback_issues: callback_issues.clone(),
            },
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

fn collect_owned_existentials(body: &Arc<Ty>, abstract_: &HashSet<TyVar>) -> IndexSet<TyVar> {
    enum Work {
        Ty(Arc<Ty>),
        Row(Row),
    }
    let mut found = IndexSet::new();
    let mut work = vec![Work::Ty(body.clone())];
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &*ty {
                Ty::Package(_) => {} // belongs to the nested package
                Ty::Array(element) => work.push(Work::Ty(element.clone())),
                Ty::Mut(region, element) => {
                    work.push(Work::Ty(element.clone()));
                    work.push(Work::Ty(region.clone()));
                }
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
fn substitute_presence_vars(root: &Arc<Ty>, renames: &HashMap<TyVar, Presence>) -> Arc<Ty> {
    enum Work {
        Ty(Arc<Ty>),
        Row(Row),
        Arrow,
        Package,
        Array,
        Mut,
        Struct,
        Sum,
        Named(Symbol, Arc<str>, usize),
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
                Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element.clone()));
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Mut);
                    work.push(Work::Ty(element.clone()));
                    work.push(Work::Ty(region.clone()));
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
                other => types.push(Arc::new(other.clone())),
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
                types.push(Arc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().unwrap();
                types.push(Arc::new(Ty::Package(body)));
            }
            Work::Array => {
                let element = types.pop().unwrap();
                types.push(Arc::new(Ty::Array(element)));
            }
            Work::Mut => {
                let element = types.pop().expect("cell element");
                let region = types.pop().expect("cell region");
                types.push(Arc::new(Ty::Mut(region, element)));
            }
            Work::Struct => types.push(Arc::new(Ty::Struct(rows.pop().unwrap()))),
            Work::Sum => types.push(Arc::new(Ty::Sum(rows.pop().unwrap()))),
            Work::Named(symbol, name, count) => {
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(types.pop().unwrap());
                }
                args.reverse();
                types.push(Arc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                }));
            }
            Work::BuiltRow(labels, rest) => {
                let rest = match rest {
                    Rest::More(_) => Rest::More(Arc::new(rows.pop().unwrap())),
                    rest => rest,
                };
                let mut built = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    let ty = if matches!(presence, Presence::Absent) {
                        Arc::new(Ty::Undecided)
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
        Ty(Arc<Ty>, bool),
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
                        let visited = seen.entry(Arc::as_ptr(&ty) as usize).or_default();
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
                            Ty::Array(element) => work.push(Work::Ty(element.clone(), positive)),
                            Ty::Mut(region, element) => {
                                work.push(Work::Ty(element.clone(), positive));
                                work.push(Work::Ty(region.clone(), positive));
                                work.push(Work::Ty(element.clone(), !positive));
                                work.push(Work::Ty(region.clone(), !positive));
                            }
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
    body: &Arc<Ty>,
    presences: u32,
    already: &IndexSet<u32>,
    variances: &HashMap<(Symbol, u32), u8>,
) -> (Arc<Ty>, IndexSet<u32>) {
    #[derive(Default)]
    struct Uses {
        positive: u32,
        negative: u32,
        owners: IndexSet<usize>,
    }
    enum Scan {
        Ty(Arc<Ty>, bool, usize),
        Row(Row, bool, usize),
    }
    let root = Arc::as_ptr(body) as usize;
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
                let visited = seen.entry((owner, Arc::as_ptr(&ty) as usize)).or_default();
                if *visited & bit != 0 {
                    continue;
                }
                *visited |= bit;
                match &*ty {
                    Ty::Arrow(from, to, effects) => {
                        work.push(Scan::Ty(from.clone(), !positive, owner));
                        let result_owner = if positive {
                            Arc::as_ptr(to) as usize
                        } else {
                            owner
                        };
                        work.push(Scan::Ty(to.clone(), positive, result_owner));
                        work.push(Scan::Row(effects.clone(), positive, owner));
                    }
                    Ty::Package(inner) => work.push(Scan::Ty(inner.clone(), positive, owner)),
                    Ty::Array(element) => work.push(Scan::Ty(element.clone(), positive, owner)),
                    Ty::Mut(region, element) => {
                        work.push(Scan::Ty(element.clone(), positive, owner));
                        work.push(Scan::Ty(region.clone(), positive, owner));
                        work.push(Scan::Ty(element.clone(), !positive, owner));
                        work.push(Scan::Ty(region.clone(), !positive, owner));
                    }
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
        Ty(Arc<Ty>),
        Arrow(bool),
        Package(bool),
        Array(bool),
        Mut(bool),
        Struct(bool),
        Sum(bool),
        Named(bool, Symbol, Arc<str>, usize),
        Row(Row),
        FinishRow(Vec<(String, Presence)>, Rest),
    }
    let mut work = vec![Build::Ty(body.clone())];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(item) = work.pop() {
        match item {
            Build::Ty(ty) => {
                let wrap = owners.contains(&(Arc::as_ptr(&ty) as usize));
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
                    Ty::Mut(region, element) => {
                        work.push(Build::Mut(wrap));
                        work.push(Build::Ty(element.clone()));
                        work.push(Build::Ty(region.clone()));
                    }
                    Ty::Array(element) => {
                        work.push(Build::Array(wrap));
                        work.push(Build::Ty(element.clone()));
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
                        let value = Arc::new(other.clone());
                        types.push(if wrap {
                            Arc::new(Ty::Package(value))
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
                        Rest::More(Arc::new(rows.pop().expect("packaged composed row tail")))
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
                let value = Arc::new(Ty::Arrow(from, to, effects));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Package(wrap) => {
                let value = Arc::new(Ty::Package(types.pop().unwrap()));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Mut(wrap) => {
                let element = types.pop().unwrap();
                let region = types.pop().unwrap();
                let value = Arc::new(Ty::Mut(region, element));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Array(wrap) => {
                let value = Arc::new(Ty::Array(types.pop().unwrap()));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Struct(wrap) => {
                let value = Arc::new(Ty::Struct(rows.pop().unwrap()));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
                } else {
                    value
                });
            }
            Build::Sum(wrap) => {
                let value = Arc::new(Ty::Sum(rows.pop().unwrap()));
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
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
                let value = Arc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                });
                types.push(if wrap {
                    Arc::new(Ty::Package(value))
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
fn shift(ty: &Arc<Ty>, by: u32) -> Arc<Ty> {
    enum Work {
        Ty(Arc<Ty>),
        Row(Row),
        Arrow,
        Package,
        Array,
        Mut,
        Struct,
        Sum,
        Named(Symbol, Arc<str>, usize),
        BuiltRow(Vec<(String, Presence)>, Rest),
    }
    let mut work = vec![Work::Ty(ty.clone())];
    let mut types = Vec::new();
    let mut rows = Vec::new();
    while let Some(part) = work.pop() {
        match part {
            Work::Ty(ty) => match &*ty {
                Ty::Bound(at) => types.push(Arc::new(Ty::Bound(at + by))),
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
                Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element.clone()));
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Mut);
                    work.push(Work::Ty(element.clone()));
                    work.push(Work::Ty(region.clone()));
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
                other => types.push(Arc::new(other.clone())),
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
                types.push(Arc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("shifted package");
                types.push(Arc::new(Ty::Package(body)));
            }
            Work::Array => {
                let element = types.pop().expect("shifted array element");
                types.push(Arc::new(Ty::Array(element)));
            }
            Work::Mut => {
                let element = types.pop().expect("cell element");
                let region = types.pop().expect("cell region");
                types.push(Arc::new(Ty::Mut(region, element)));
            }
            Work::Struct => types.push(Arc::new(Ty::Struct(rows.pop().expect("shifted struct")))),
            Work::Sum => types.push(Arc::new(Ty::Sum(rows.pop().expect("shifted sum")))),
            Work::Named(symbol, name, count) => {
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(types.pop().expect("shifted named argument"));
                }
                args.reverse();
                types.push(Arc::new(Ty::Named {
                    symbol,
                    name,
                    args: args.into(),
                }));
            }
            Work::BuiltRow(labels, rest) => {
                let rest = match rest {
                    Rest::Bound(at) => Rest::Bound(at + by),
                    Rest::More(_) => {
                        Rest::More(Arc::new(rows.pop().expect("shifted composed row")))
                    }
                    rest => rest,
                };
                let mut built = Vec::with_capacity(labels.len());
                for (name, presence) in labels.into_iter().rev() {
                    let ty = match presence {
                        Presence::Absent => Arc::new(Ty::Undecided),
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
fn lower_type(mint: &Mint, table: &mut Table, ty: &Type) -> Arc<Ty> {
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

fn extern_annotation_sources(
    annotation: &Annotation,
    tails: &Tails,
    semantic: &Arc<Ty>,
) -> ExternSourceMap {
    fn source_when(
        sources: &mut ExternSourceMap,
        when: &Option<Box<ir::When>>,
        presence: &Presence,
    ) {
        let Some(when) = when else { return };
        if when.name.is_none()
            && let Some(identity) = presence_variable_identity(presence)
        {
            // Anonymous presence holes are declaration-local inference sites,
            // not caller-chosen polymorphism at the host boundary. This is true
            // whether polarity later packages them existentially or leaves the
            // one monomorphic hole to inference.
            sources.insert_with_ownership(identity, when.at, "conditional presence", true);
        }
    }

    fn source_tail(sources: &mut ExternSourceMap, tail: &Option<ir::Tail>, rest: &Rest) {
        let Some(tail) = tail else { return };
        if matches!(tail.of, ir::Row::Anything)
            && let Some(identity) = row_variable_identity(rest)
        {
            sources.insert(identity, tail.at, "generic row");
        }
    }

    let mut sources = ExternSourceMap::default();
    for variable in &annotation.variables {
        let name = format!("'{}", variable.name);
        match variable.sense {
            Sense::Type | Sense::Region => {
                if let Some(ty) = tails.types.get(&variable.name)
                    && let Some(identity) = type_variable_identity(ty)
                {
                    sources.insert(identity, variable.at, name);
                }
            }
            Sense::Fields | Sense::Cases | Sense::Effects => {
                if let Some(rest) = tails.rows.get(&variable.name)
                    && let Some(identity) = row_variable_identity(rest)
                {
                    sources.insert(identity, variable.at, name);
                }
            }
            Sense::Presence => {
                if let Some(presence) = tails.presences.get(&variable.name)
                    && let Some(identity) = presence_variable_identity(presence)
                {
                    sources.insert_with_ownership(
                        identity,
                        variable.at,
                        name,
                        matches!(
                            variable.ownership,
                            crate::ir::PresenceOwnership::Existential { .. }
                        ),
                    );
                }
            }
        }
    }

    // Walk the still-written annotation and its unexpanded semantic spelling in
    // lockstep. This records anonymous holes at their exact occurrence while
    // named variables above deliberately retain their declaration span. Alias
    // arguments are visited here before unfolding; their semantic identities
    // are what later survive substitution into local or imported bodies.
    let mut work = vec![(&annotation.ty, semantic.clone())];
    while let Some((written, mut semantic)) = work.pop() {
        while let Ty::Package(inner) = &*semantic {
            semantic = inner.clone();
        }
        match (&written.anchored, &*semantic) {
            (ir::TypeKind::Hole, Ty::Var(var)) => sources.insert(
                ExternVariableIdentity::TypeVar(*var),
                written.at,
                "generic type",
            ),
            (ir::TypeKind::Apply { args, .. }, Ty::Named { args: lowered, .. }) => {
                work.extend(
                    args.iter()
                        .zip(lowered.iter())
                        .rev()
                        .map(|(written, semantic)| (written, semantic.clone())),
                );
            }
            (ir::TypeKind::Array(element), Ty::Array(lowered)) => {
                work.push((element, lowered.clone()));
            }
            (ir::TypeKind::Mut(region, element), Ty::Mut(lowered_region, lowered_element)) => {
                work.push((region, lowered_region.clone()));
                work.push((element, lowered_element.clone()));
            }
            (
                ir::TypeKind::Arrow { from, to, effects },
                Ty::Arrow(lowered_from, lowered_to, lowered_effects),
            ) => {
                let lowered_effects = flatten_row(lowered_effects);
                source_tail(&mut sources, &effects.tail, &lowered_effects.rest);
                for (effect, written) in &effects.effects {
                    let ir::EffectLabel::Written { when, .. } = written else {
                        continue;
                    };
                    if let Some(field) = lowered_effects.labels.get(&effect.row_key()) {
                        source_when(&mut sources, when, &field.presence);
                    }
                }
                work.push((to, lowered_to.clone()));
                work.push((from, lowered_from.clone()));
            }
            (ir::TypeKind::Struct { fields, tail }, Ty::Struct(row)) => {
                let row = flatten_row(row);
                source_tail(&mut sources, tail, &row.rest);
                for (name, written) in fields.iter().rev() {
                    let ir::TypeField::Written { when, value, .. } = written else {
                        continue;
                    };
                    if let Some(field) = row.labels.get(name) {
                        source_when(&mut sources, when, &field.presence);
                        work.push((value, field.ty.clone()));
                    }
                }
            }
            (ir::TypeKind::Sum { cases, tail }, Ty::Sum(row)) => {
                let row = flatten_row(row);
                source_tail(&mut sources, tail, &row.rest);
                for (name, written) in cases.iter().rev() {
                    let ir::SumCase::Written { when, payload, .. } = written else {
                        continue;
                    };
                    if let Some(field) = row.labels.get(name) {
                        source_when(&mut sources, when, &field.presence);
                        if let Some(payload) = payload {
                            work.push((payload, field.ty.clone()));
                        }
                    }
                }
            }
            (ir::TypeKind::Effects(effects), Ty::Sum(row)) => {
                let row = flatten_row(row);
                source_tail(&mut sources, &effects.tail, &row.rest);
                for (effect, written) in &effects.effects {
                    let ir::EffectLabel::Written { when, .. } = written else {
                        continue;
                    };
                    if let Some(field) = row.labels.get(&effect.row_key()) {
                        source_when(&mut sources, when, &field.presence);
                    }
                }
            }
            _ => {}
        }
    }
    sources
}

/// Everything one lowered annotation says: the type its body is checked
/// against, the scheme a use of its own name inside that body instantiates, and
/// what it promised about its presences.
struct Lowered {
    /// The skolemized type. Its variables are rigids and its holes
    /// are ordinary solver variables, which is the whole of the difference
    /// between what the annotation promises and what it leaves to inference.
    ty: Arc<Ty>,
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
    /// Exact source provenance for variables which may reach a host boundary.
    extern_sources: ExternSourceMap,
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
        table.rigids.insert(variable.id, variable.at);
        if variable.sense == Sense::Region {
            table.region_rigids.insert(variable.id);
        }
        let name: Arc<str> = variable.name.as_str().into();
        match variable.sense {
            Sense::Type | Sense::Region => {
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
    let boundaries: HashSet<Anchor> = annotation
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
    let extern_sources = extern_annotation_sources(annotation, &tails, &ty);
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
        rigid_sorts: annotation
            .variables
            .iter()
            .filter_map(|variable| {
                let sort = match variable.sense {
                    Sense::Type => VarSort::Type,
                    Sense::Region => VarSort::Region,
                    Sense::Fields | Sense::Cases | Sense::Effects => VarSort::Row,
                    Sense::Presence => return None,
                };
                Some((variable.id, sort))
            })
            .collect(),
    };
    let scheme = Scheme::existential(
        presences + at.len() as u32,
        presences,
        existential_slots,
        table.zonk(&ty, &subst),
        quantify_formula(&formula, &subst),
    );
    let mut original = vec![Assigned::Ty(Arc::new(Ty::Undecided)); scheme.count() as usize];
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
        extern_sources,
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
    match &clause.anchored {
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
fn lower(mint: &Mint, table: &mut Table, tails: &mut Tails, ty: &Type) -> Arc<Ty> {
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
    boundaries: Option<&HashSet<Anchor>>,
) -> Arc<Ty> {
    let lowered = match &ty.anchored {
        TypeKind::Prim(prim) => (*prim).into(),
        TypeKind::Ident(symbol) => Ty::Named {
            symbol: *symbol,
            name: mint.name(*symbol).into(),
            // Lowering counted the arguments, so a name that reaches here bare
            // is one that takes none.
            args: Arc::from([]),
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
            effect_row(mint, table, tails, effects, boundaries),
        ),
        TypeKind::Mut(region, element) => Ty::Mut(
            lower_scoped(mint, table, tails, region, boundaries),
            lower_scoped(mint, table, tails, element, boundaries),
        ),
        TypeKind::Array(element) => {
            Ty::Array(lower_scoped(mint, table, tails, element, boundaries))
        }
        // A row handed to a declaration as an argument, which is the one place
        // a row arrives without an arrow around it. It lowers to the row it is
        // — [`Ty::Sum`] is a set of labels and a rest, which is what a row of
        // effects is too — and the position it is spliced into is what says
        // which of the three it is being read as. See
        // [`types::Shape`](crate::types::Shape).
        TypeKind::Effects(effects) => Ty::Sum(effect_row(mint, table, tails, effects, boundaries)),
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
                        ty: Arc::new(Ty::default()),
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
                            None => Arc::new(Ty::unit()),
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
                        ty: Arc::new(Ty::default()),
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
    let body = Arc::new(lowered);
    match boundaries.is_some_and(|boundaries| boundaries.contains(&ty.at)) {
        true => Arc::new(Ty::Package(body)),
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
///
/// Each label's arguments are lowered in the scope of the type around it and
/// become the label's payload, as the positional struct a tuple is; a label
/// written absent carries them too, so a printed row can still say which
/// application is not performed.
fn effect_row(
    mint: &Mint,
    table: &mut Table,
    tails: &mut Tails,
    effects: &ir::EffectRow,
    boundaries: Option<&HashSet<Anchor>>,
) -> Row {
    let mut labels = IndexMap::new();
    for (name, label) in &effects.effects {
        let args: Vec<Assigned> = label
            .args()
            .iter()
            .map(|arg| {
                Assigned::Ty(match &arg.anchored {
                    // A row of effects written as an argument travels as an
                    // effects argument, not as the sum a declaration's row
                    // argument lowers to: see [`Ty::effects_argument`].
                    TypeKind::Effects(row) => Arc::new(Ty::effects_argument(effect_row(
                        mint, table, tails, row, boundaries,
                    ))),
                    _ => lower_scoped(mint, table, tails, arg, boundaries),
                })
            })
            .collect();
        let ty = constrain::argument_tuple(&args);
        let lowered = match label {
            ir::EffectLabel::Written { when, .. } => RowField {
                presence: presence(table, tails, when),
                ty,
            },
            ir::EffectLabel::Absent { .. } => RowField {
                presence: Presence::Absent,
                ty,
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
pub fn unfold(aliases: &IndexMap<Symbol, Scheme>, ty: &Arc<Ty>) -> Arc<Ty> {
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
    fn ty(&mut self, ty: &Arc<Ty>) -> Arc<Ty> {
        let mut ty = ty.clone();
        let mut entered = Vec::new();
        let result = loop {
            let Ty::Named { symbol, args, .. } = &*ty else {
                break ty;
            };
            let Some(scheme) = self.aliases.get(symbol) else {
                break Arc::new(Ty::Undecided);
            };
            let body = scheme.body().clone();
            let grows = self.forwarding.projection(self.aliases, &body).is_none();
            // A head-alias cycle that only grows the forwarded argument can be
            // rejected before materializing any cumulative row at all. The
            // forwarding classifier has already walked that declaration graph
            // and marked every alias on the cycle-reaching path.
            if grows && adds_argument_structure(&body) && self.forwarding.cycles.contains(symbol) {
                break Arc::new(Ty::Undecided);
            }
            // A forwarding declaration selects an existing argument and thus
            // strictly consumes the finite application tree; it needs no
            // cumulative identity for every argument it passes through.
            // Structure-adding paths are bounded by declaration identity
            // instead. This keeps an N-alias malformed growth cycle at O(N)
            // retained graph memory rather than storing N flattened maps of
            // sizes 1 through N.
            if grows && !self.growing.insert(*symbol) {
                break Arc::new(Ty::Undecided);
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
    fn projection(&mut self, aliases: &IndexMap<Symbol, Scheme>, root: &Arc<Ty>) -> Option<u32> {
        enum Work {
            Expression(Arc<Ty>),
            Alias(Symbol),
            AfterAlias(Arc<[Arc<Ty>]>),
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
                    let address = Arc::as_ptr(&ty) as usize;
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
    fn open_alias(&self, fresh: &[Assigned], unfold: &mut Unfold<'_>) -> Arc<Ty> {
        substitute_type(
            self,
            fresh,
            |index| {
                fresh
                    .get(index as usize)
                    .map(Assigned::as_ty)
                    .unwrap_or_else(|| Arc::new(Ty::Undecided))
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
    pub fn open(&self, fresh: &[Assigned]) -> Arc<Ty> {
        substitute_type(
            self,
            fresh,
            |index| {
                fresh
                    .get(index as usize)
                    .map(Assigned::as_ty)
                    .unwrap_or_else(|| Arc::new(Ty::Undecided))
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
    mut bound_ty: impl FnMut(u32) -> Arc<Ty>,
    mut bound_row: impl FnMut(u32) -> Row,
) -> Arc<Ty> {
    enum Work<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
        Arrow,
        Package,
        Array,
        Mut,
        Struct,
        Sum,
        Named {
            symbol: Symbol,
            name: Arc<str>,
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
                Ty::Array(element) => {
                    work.push(Work::Array);
                    work.push(Work::Ty(element));
                }
                Ty::Mut(region, element) => {
                    work.push(Work::Mut);
                    work.push(Work::Ty(element));
                    work.push(Work::Ty(region));
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
                other => types.push(Arc::new(other.clone())),
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
                types.push(Arc::new(Ty::Arrow(from, to, effects)));
            }
            Work::Package => {
                let body = types.pop().expect("package substitution postorder");
                types.push(Arc::new(Ty::Package(body)));
            }
            Work::Array => {
                let element = types.pop().expect("array substitution postorder");
                types.push(Arc::new(Ty::Array(element)));
            }
            Work::Mut => {
                let element = types.pop().expect("cell element");
                let region = types.pop().expect("cell region");
                types.push(Arc::new(Ty::Mut(region, element)));
            }
            Work::Struct => {
                let row = rows.pop().expect("struct substitution postorder");
                types.push(Arc::new(Ty::Struct(row)));
            }
            Work::Sum => {
                let row = rows.pop().expect("sum substitution postorder");
                types.push(Arc::new(Ty::Sum(row)));
            }
            Work::Named { symbol, name, args } => {
                let mut opened = Vec::with_capacity(args);
                for _ in 0..args {
                    opened.push(types.pop().expect("named substitution postorder"));
                }
                opened.reverse();
                types.push(Arc::new(Ty::Named {
                    symbol,
                    name,
                    args: opened.into(),
                }));
            }
            Work::BuiltRow(row) => {
                let rest = match &row.rest {
                    Rest::Bound(index) => Rest::More(Arc::new(bound_row(*index))),
                    Rest::More(_) => Rest::More(Arc::new(
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
                        Presence::Absent => Arc::new(Ty::Undecided),
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
        let body = Arc::new(Ty::Struct(Row {
            labels: [(
                "hidden".into(),
                RowField {
                    presence: Presence::Var(hidden),
                    ty: Arc::new(Ty::Nat),
                },
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let package = Arc::new(Ty::Package(body));
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
            Arc::new(Ty::Struct(Row {
                labels: [(
                    "hidden".into(),
                    RowField {
                        presence,
                        ty: Arc::new(Ty::Nat),
                    },
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }))
        };
        let named = |args: Vec<Arc<Ty>>| {
            Arc::new(Ty::Named {
                symbol: wrapper,
                name: Arc::from("Wrapper"),
                args: args.into(),
            })
        };
        // The first argument speculatively aliases `alias` to the abstract
        // identity. The second fails, forcing congruence to discard that work.
        let expected = named(vec![presence_arg(Presence::Var(hidden)), Arc::new(Ty::Nat)]);
        let actual = named(vec![
            presence_arg(Presence::Var(alias)),
            Arc::new(Ty::String),
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
                at: Anchor::GENERATED,
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
        assert!(failed.id.index() > 0);
        assert!(errors[0].id.index() > 0);
    }

    #[test]
    fn mismatch_leaf_walks_nested_rows_effect_payloads_and_alias_arguments() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("leaf-test", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let wrapper = mint.global(None, Namespace::Types, "Wrapper").unwrap();
        let unit = Arc::new(Ty::unit());
        let effect = Row {
            labels: [("effect".into(), RowField::present(Arc::new(Ty::Bound(0))))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        };
        let arrow = Arc::new(Ty::Arrow(unit.clone(), unit, effect));
        let sum = Arc::new(Ty::Sum(Row {
            labels: [("Case".into(), RowField::present(arrow))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let body = Arc::new(Ty::Struct(Row {
            labels: [("field".into(), RowField::present(sum))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [(wrapper, Scheme::new(1, body))].into_iter().collect();
        let named = |argument| {
            Arc::new(Ty::Named {
                symbol: wrapper,
                name: "Wrapper".into(),
                args: Arc::from([Arc::new(argument)]),
            })
        };
        assert_eq!(
            smallest_incompatible(&aliases, &named(Ty::Nat), &named(Ty::Bool)),
            (TypeDescription::NaturalNumber, TypeDescription::Bool)
        );

        let missing_left = mint.global(None, Namespace::Types, "MissingLeft").unwrap();
        let missing_right = mint.global(None, Namespace::Types, "MissingRight").unwrap();
        let missing = |symbol, name: &'static str, args: Arc<[Arc<Ty>]>| {
            Arc::new(Ty::Named {
                symbol,
                name: name.into(),
                args,
            })
        };
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &missing(missing_left, "MissingLeft", Arc::from([])),
                &missing(missing_right, "MissingRight", Arc::from([])),
            ),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType),
            "missing semantic definitions must keep the declared-type fallback"
        );
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &missing(missing_left, "MissingLeft", Arc::from([Arc::new(Ty::Nat)])),
                &missing(missing_left, "MissingLeft", Arc::from([Arc::new(Ty::Bool)]),),
            ),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType),
            "arguments of the same unavailable alias have no honest semantics"
        );
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &missing(missing_left, "MissingLeft", Arc::from([Arc::new(Ty::Nat)])),
                &Arc::new(Ty::Bool),
            ),
            (TypeDescription::DeclaredType, TypeDescription::Bool),
            "one unavailable alias must remain declared without hiding the other side"
        );
    }

    #[test]
    fn mismatch_alias_function_keeps_unfolded_fallback_for_effect_disagreement() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("function-fallback", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let left_name = mint.global(None, Namespace::Types, "Left").unwrap();
        let right_name = mint.global(None, Namespace::Types, "Right").unwrap();
        let unit = Arc::new(Ty::unit());
        let arrow = |effect: &'static str| {
            Arc::new(Ty::Arrow(
                unit.clone(),
                unit.clone(),
                Row {
                    labels: [(effect.into(), RowField::present(Arc::new(Ty::unit())))]
                        .into_iter()
                        .collect(),
                    rest: Rest::Closed,
                },
            ))
        };
        let aliases = [
            (left_name, Scheme::new(0, arrow("read"))),
            (right_name, Scheme::new(0, arrow("write"))),
        ]
        .into_iter()
        .collect();
        let named = |symbol, name: &'static str| {
            Arc::new(Ty::Named {
                symbol,
                name: name.into(),
                args: Arc::from([]),
            })
        };

        assert_eq!(
            smallest_incompatible(
                &aliases,
                &named(left_name, "Left"),
                &named(right_name, "Right"),
            ),
            (TypeDescription::Function, TypeDescription::Function),
            "an effect-only disagreement must not fall back to the aliases' spelling"
        );
    }

    #[test]
    fn mismatch_alias_containers_keep_unfolded_fallback_without_payload_leaf() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("container-fallback", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let struct_left = mint.global(None, Namespace::Types, "StructLeft").unwrap();
        let struct_right = mint.global(None, Namespace::Types, "StructRight").unwrap();
        let sum_left = mint.global(None, Namespace::Types, "SumLeft").unwrap();
        let sum_right = mint.global(None, Namespace::Types, "SumRight").unwrap();
        let hidden_row = |presence, ty| Row {
            labels: [(
                "hidden".into(),
                RowField {
                    presence,
                    ty: Arc::new(ty),
                },
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        };
        let aliases = [
            (
                struct_left,
                Scheme::new(
                    0,
                    Arc::new(Ty::Struct(hidden_row(Presence::Absent, Ty::Nat))),
                ),
            ),
            (
                struct_right,
                Scheme::new(
                    0,
                    Arc::new(Ty::Struct(hidden_row(Presence::Absent, Ty::Bool))),
                ),
            ),
            (
                sum_left,
                Scheme::new(
                    0,
                    Arc::new(Ty::Package(Arc::new(Ty::Sum(hidden_row(
                        Presence::Recovered(1),
                        Ty::Nat,
                    ))))),
                ),
            ),
            (
                sum_right,
                Scheme::new(
                    0,
                    Arc::new(Ty::Package(Arc::new(Ty::Sum(hidden_row(
                        Presence::Recovered(1),
                        Ty::Bool,
                    ))))),
                ),
            ),
        ]
        .into_iter()
        .collect();
        let named = |symbol, name: &'static str| {
            Arc::new(Ty::Named {
                symbol,
                name: name.into(),
                args: Arc::from([]),
            })
        };

        assert_eq!(
            smallest_incompatible(
                &aliases,
                &named(struct_left, "StructLeft"),
                &named(struct_right, "StructRight"),
            ),
            (TypeDescription::Struct, TypeDescription::Struct)
        );
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &named(sum_left, "SumLeft"),
                &named(sum_right, "SumRight"),
            ),
            (TypeDescription::TaggedValue, TypeDescription::TaggedValue),
            "packages exposed by aliases must remain transparent while preserving the sum fallback"
        );
    }

    #[test]
    fn mismatch_rows_ignore_label_order_and_walk_composed_tail_payloads() {
        let field = |ty| RowField::present(Arc::new(ty));
        let left = Arc::new(Ty::Struct(Row {
            labels: [
                ("first".into(), field(Ty::unit())),
                ("second".into(), field(Ty::unit())),
            ]
            .into_iter()
            .collect(),
            rest: Rest::More(Arc::new(Row {
                labels: [("tail".into(), field(Ty::Nat))].into_iter().collect(),
                rest: Rest::Closed,
            })),
        }));
        let right = Arc::new(Ty::Struct(Row {
            labels: [
                ("second".into(), field(Ty::unit())),
                ("first".into(), field(Ty::unit())),
            ]
            .into_iter()
            .collect(),
            rest: Rest::More(Arc::new(Row {
                labels: [("tail".into(), field(Ty::Bool))].into_iter().collect(),
                rest: Rest::Closed,
            })),
        }));
        assert_eq!(
            smallest_incompatible(&IndexMap::new(), &left, &right),
            (TypeDescription::NaturalNumber, TypeDescription::Bool),
            "reordered map keys must not hide a mismatch in a composed tail"
        );
    }

    #[test]
    fn mismatch_rows_compare_segmented_and_flat_canonical_shapes() {
        let field = |ty| RowField::present(Arc::new(ty));
        let left = Arc::new(Ty::Struct(Row {
            labels: [("outer".into(), field(Ty::unit()))].into_iter().collect(),
            rest: Rest::More(Arc::new(Row {
                labels: [("leaf".into(), field(Ty::Nat))].into_iter().collect(),
                rest: Rest::Closed,
            })),
        }));
        let right = Arc::new(Ty::Struct(Row {
            labels: [
                ("leaf".into(), field(Ty::Bool)),
                ("outer".into(), field(Ty::unit())),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        assert_eq!(
            smallest_incompatible(&IndexMap::new(), &left, &right),
            (TypeDescription::NaturalNumber, TypeDescription::Bool)
        );
    }

    #[test]
    fn mismatch_rows_give_duplicate_outer_labels_precedence() {
        let field = |ty| RowField::present(Arc::new(ty));
        let segmented = |outer, hidden, leaf| {
            Arc::new(Ty::Struct(Row {
                labels: [("duplicate".into(), field(outer))].into_iter().collect(),
                rest: Rest::More(Arc::new(Row {
                    labels: [
                        ("duplicate".into(), field(hidden)),
                        ("leaf".into(), field(leaf)),
                    ]
                    .into_iter()
                    .collect(),
                    rest: Rest::Closed,
                })),
            }))
        };
        assert_eq!(
            smallest_incompatible(
                &IndexMap::new(),
                &segmented(Ty::unit(), Ty::Nat, Ty::Nat),
                &segmented(Ty::unit(), Ty::Bool, Ty::Bool),
            ),
            (TypeDescription::NaturalNumber, TypeDescription::Bool),
            "the hidden duplicate payload must not become the first mismatch"
        );
    }

    #[test]
    fn mismatch_row_fingerprints_meter_unavailable_labels_and_ignore_their_payloads() {
        let unavailable = |presence: Presence, payload: Ty| {
            let labels = (0..10_000)
                .map(|at| {
                    (
                        format!("f{at}"),
                        RowField {
                            presence: presence.clone(),
                            ty: Arc::new(payload.clone()),
                        },
                    )
                })
                .collect();
            Arc::new(Ty::Struct(Row {
                labels,
                rest: Rest::Closed,
            }))
        };
        for presence in [Presence::Absent, Presence::Recovered(7)] {
            let left = unavailable(presence.clone(), Ty::Nat);
            let right = unavailable(presence, Ty::Bool);
            let mut compare_budget = 128;
            assert_eq!(
                same_finite_syntax_metered(&left, &right, &mut compare_budget),
                None
            );
            assert_eq!(
                compare_budget, 0,
                "each canonical row-label comparison must be metered"
            );

            let mut fingerprint_budget = 128;
            assert!(
                MismatchFingerprints::default()
                    .arguments(std::slice::from_ref(&left), &mut fingerprint_budget)
                    .is_none()
            );
            assert_eq!(
                fingerprint_budget, 0,
                "each row-label fingerprint operation must be metered"
            );

            let mut left_fingerprints = MismatchFingerprints::default();
            let mut right_fingerprints = MismatchFingerprints::default();
            let mut left_budget = usize::MAX;
            let mut right_budget = usize::MAX;
            assert_eq!(
                left_fingerprints.arguments(&[left], &mut left_budget),
                right_fingerprints.arguments(&[right], &mut right_budget),
                "unavailable recovery payloads must not enter fingerprints"
            );
        }
    }

    #[test]
    fn mismatch_rows_do_not_read_semantically_unavailable_payloads() {
        let row = |presence, ty| {
            Arc::new(Ty::Struct(Row {
                labels: [(
                    "x".into(),
                    RowField {
                        presence,
                        ty: Arc::new(ty),
                    },
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }))
        };
        for (left_presence, right_presence) in [
            (Presence::Absent, Presence::Absent),
            (Presence::Recovered(7), Presence::Recovered(7)),
            (Presence::Present, Presence::Absent),
        ] {
            assert_eq!(
                smallest_incompatible(
                    &IndexMap::new(),
                    &row(left_presence, Ty::Nat),
                    &row(right_presence, Ty::Bool),
                ),
                (TypeDescription::Struct, TypeDescription::Struct),
                "absent, recovery, and incompatible presences require the honest row fallback"
            );
        }
    }

    #[test]
    fn recursive_alias_mismatch_skips_cycle_and_finds_sibling_leaf() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("recursive-leaf", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let stream = mint.global(None, Namespace::Types, "Stream").unwrap();
        // Put the recursive field first: the DFS must encounter and suppress
        // that cycle, then continue to the value field rather than returning a
        // container fallback or looping forever.
        let recursive = Arc::new(Ty::Named {
            symbol: stream,
            name: "Stream".into(),
            args: Arc::from([Arc::new(Ty::Bound(0))]),
        });
        let body = Arc::new(Ty::Struct(Row {
            labels: [
                ("tail".into(), RowField::present(recursive)),
                ("value".into(), RowField::present(Arc::new(Ty::Bound(0)))),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [(stream, Scheme::new(1, body))].into_iter().collect();
        let named = |argument| {
            Arc::new(Ty::Named {
                symbol: stream,
                name: "Stream".into(),
                args: Arc::from([argument]),
            })
        };
        let left = named(Arc::new(Ty::Nat));
        let right = named(Arc::new(Ty::Bool));
        for _ in 0..32 {
            assert_eq!(
                smallest_incompatible(&aliases, &left, &right),
                (TypeDescription::NaturalNumber, TypeDescription::Bool),
                "the recursive walk must be deterministic"
            );
        }
    }

    #[test]
    fn growing_alias_branch_does_not_starve_later_sibling() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("growing-branch-leaf", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let growth: Vec<_> = (0..4_200)
            .map(|at| {
                mint.global(None, Namespace::Types, &format!("Growth{at}"))
                    .unwrap()
            })
            .collect();
        let wrapper = mint.global(None, Namespace::Types, "Wrapper").unwrap();
        let mut aliases = IndexMap::new();
        for (at, symbols) in growth.windows(2).enumerate() {
            let argument = Arc::new(Ty::Struct(Row {
                labels: [("x".into(), RowField::present(Arc::new(Ty::Bound(0))))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }));
            aliases.insert(
                symbols[0],
                Scheme::new(
                    1,
                    Arc::new(Ty::Named {
                        symbol: symbols[1],
                        name: format!("Growth{}", at + 1).into(),
                        args: Arc::from([argument]),
                    }),
                ),
            );
        }
        aliases.insert(wrapper, Scheme::new(1, Arc::new(Ty::Bound(0))));
        let named = |symbol, name: String, argument| {
            Arc::new(Ty::Named {
                symbol,
                name: name.into(),
                args: Arc::from([argument]),
            })
        };
        let row = |bad, good| {
            Arc::new(Ty::Struct(Row {
                labels: [
                    ("bad".into(), RowField::present(bad)),
                    ("good".into(), RowField::present(good)),
                ]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }))
        };
        let left = row(
            named(growth[0], "Growth0".into(), Arc::new(Ty::Nat)),
            named(wrapper, "Wrapper".into(), Arc::new(Ty::Nat)),
        );
        let right = row(
            named(growth[0], "Growth0".into(), Arc::new(Ty::Nat)),
            named(wrapper, "Wrapper".into(), Arc::new(Ty::Bool)),
        );

        assert_eq!(
            smallest_incompatible(&aliases, &left, &right),
            (TypeDescription::NaturalNumber, TypeDescription::Bool),
            "a growing first branch must not spend its sibling's alias depth"
        );
    }

    #[test]
    fn pure_alias_growth_has_a_bounded_declared_fallback() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("pure-growth-leaf", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let growth: Vec<_> = (0..30_000)
            .map(|at| {
                mint.global(None, Namespace::Types, &format!("Growth{at}"))
                    .unwrap()
            })
            .collect();
        let mut aliases = IndexMap::new();
        for (at, symbols) in growth.windows(2).enumerate() {
            let argument = Arc::new(Ty::Struct(Row {
                labels: [("x".into(), RowField::present(Arc::new(Ty::Bound(0))))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }));
            aliases.insert(
                symbols[0],
                Scheme::new(
                    1,
                    Arc::new(Ty::Named {
                        symbol: symbols[1],
                        name: format!("Growth{}", at + 1).into(),
                        args: Arc::from([argument]),
                    }),
                ),
            );
        }
        let named = Arc::new(Ty::Named {
            symbol: growth[0],
            name: "Growth0".into(),
            args: Arc::from([Arc::new(Ty::Nat)]),
        });
        assert_eq!(
            smallest_incompatible(&aliases, &named, &named),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType),
            "pure growth must stop at the per-branch bound"
        );
    }

    #[test]
    fn fingerprint_collisions_in_self_growth_have_a_strict_operation_bound() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("self-growing-leaf", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let grow = mint.global(None, Namespace::Types, "Grow").unwrap();
        // Recovery input equivalent to `type Grow 'a = Grow { x: 'a }`.
        // Every opening has a distinct, one-node-larger argument, so an
        // ancestor-by-ancestor deep equality walk is superlinear.
        let larger = Arc::new(Ty::Struct(Row {
            labels: [("x".into(), RowField::present(Arc::new(Ty::Bound(0))))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let recursive = Arc::new(Ty::Named {
            symbol: grow,
            name: "Grow".into(),
            args: Arc::from([larger]),
        });
        let body = Arc::new(Ty::Struct(Row {
            labels: [("next".into(), RowField::present(recursive))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [(grow, Scheme::new(1, body))].into_iter().collect();
        let named = Arc::new(Ty::Named {
            symbol: grow,
            name: "Grow".into(),
            args: Arc::from([Arc::new(Ty::Nat)]),
        });
        let mut operations = 0;
        assert_eq!(
            // Mask every argument fingerprint to zero. Every ancestor is now
            // an intentional collision and exact verification must still be
            // charged to the same bounded node-work allowance.
            smallest_incompatible_counted_with_mask(&aliases, &named, &named, &mut operations, 0,),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType)
        );
        assert_eq!(
            operations, 1_048_576,
            "self-growth must stop exactly at the branch budget"
        );
    }

    #[test]
    fn recursive_alias_mismatch_canonicalizes_forwarding_arguments() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("forwarding-recursive-leaf", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let stream = mint.global(None, Namespace::Types, "Stream").unwrap();
        let id = mint.global(None, Namespace::Types, "Id").unwrap();
        let forward = mint.global(None, Namespace::Types, "Forward").unwrap();
        let pass = mint.global(None, Namespace::Types, "Pass").unwrap();
        let id_body = Arc::new(Ty::Bound(0));
        // Malformed imported interfaces can also contain a mutually-forwarding
        // cycle. It has no observable body and canonicalizes deterministically
        // to recovery instead of retaining an ever-growing argument spelling.
        let forward_body = Arc::new(Ty::Named {
            symbol: pass,
            name: "Pass".into(),
            args: Arc::from([Arc::new(Ty::Bound(0))]),
        });
        let pass_body = Arc::new(Ty::Named {
            symbol: forward,
            name: "Forward".into(),
            args: Arc::from([Arc::new(Ty::Bound(0))]),
        });
        let recursive = Arc::new(Ty::Named {
            symbol: stream,
            name: "Stream".into(),
            args: Arc::from([Arc::new(Ty::Named {
                symbol: id,
                name: "Id".into(),
                args: Arc::from([Arc::new(Ty::Bound(0))]),
            })]),
        });
        let stream_body = Arc::new(Ty::Struct(Row {
            labels: [
                ("tail".into(), RowField::present(recursive)),
                ("value".into(), RowField::present(Arc::new(Ty::Bound(0)))),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [
            (stream, Scheme::new(1, stream_body)),
            (id, Scheme::new(1, id_body)),
            (forward, Scheme::new(1, forward_body)),
            (pass, Scheme::new(1, pass_body)),
        ]
        .into_iter()
        .collect();
        let named = |argument| {
            Arc::new(Ty::Named {
                symbol: stream,
                name: "Stream".into(),
                args: Arc::from([argument]),
            })
        };
        assert_eq!(
            smallest_incompatible(
                &aliases,
                &named(Arc::new(Ty::Nat)),
                &named(Arc::new(Ty::Bool)),
            ),
            (TypeDescription::NaturalNumber, TypeDescription::Bool)
        );
        let cyclic = Arc::new(Ty::Named {
            symbol: forward,
            name: "Forward".into(),
            args: Arc::from([Arc::new(Ty::Nat)]),
        });
        for _ in 0..32 {
            assert!(matches!(
                &*canonical_alias_argument(&aliases, &mut Forwarding::default(), &cyclic),
                Ty::Undecided
            ));
        }
    }

    #[test]
    fn recursive_alias_forwarding_argument_is_deep_stack_safe() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        std::thread::Builder::new()
            .name("deep-forwarding-recursive-leaf".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let bundle = Bundle::new("deep-forwarding-leaf", Version::new(1, 0, 0)).unwrap();
                let mut mint = Mint::new(bundle);
                let stream = mint.global(None, Namespace::Types, "Stream").unwrap();
                let forwards: Vec<_> = (0..30_000)
                    .map(|at| {
                        mint.global(None, Namespace::Types, &format!("Forward{at}"))
                            .unwrap()
                    })
                    .collect();
                let mut aliases = IndexMap::new();
                for (at, symbol) in forwards.iter().copied().enumerate() {
                    let body = match forwards.get(at + 1) {
                        Some(next) => Arc::new(Ty::Named {
                            symbol: *next,
                            name: format!("Forward{}", at + 1).into(),
                            args: Arc::from([Arc::new(Ty::Bound(0))]),
                        }),
                        None => Arc::new(Ty::Bound(0)),
                    };
                    aliases.insert(symbol, Scheme::new(1, body));
                }
                let recursive = Arc::new(Ty::Named {
                    symbol: stream,
                    name: "Stream".into(),
                    args: Arc::from([Arc::new(Ty::Named {
                        symbol: forwards[0],
                        name: "Forward0".into(),
                        args: Arc::from([Arc::new(Ty::Bound(0))]),
                    })]),
                });
                aliases.insert(
                    stream,
                    Scheme::new(
                        1,
                        Arc::new(Ty::Struct(Row {
                            labels: [
                                ("tail".into(), RowField::present(recursive)),
                                ("value".into(), RowField::present(Arc::new(Ty::Bound(0)))),
                            ]
                            .into_iter()
                            .collect(),
                            rest: Rest::Closed,
                        })),
                    ),
                );
                let named = |argument| {
                    Arc::new(Ty::Named {
                        symbol: stream,
                        name: "Stream".into(),
                        args: Arc::from([argument]),
                    })
                };
                assert_eq!(
                    smallest_incompatible(
                        &aliases,
                        &named(Arc::new(Ty::Nat)),
                        &named(Arc::new(Ty::Bool)),
                    ),
                    (TypeDescription::NaturalNumber, TypeDescription::Bool)
                );
            })
            .unwrap()
            .join()
            .expect("forwarding normalization stays on the explicit stack");
    }

    #[test]
    fn recursive_alias_pure_cycle_keeps_declared_fallback() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        let bundle = Bundle::new("recursive-fallback", Version::new(1, 0, 0)).unwrap();
        let mut mint = Mint::new(bundle);
        let cycle = mint.global(None, Namespace::Types, "Cycle").unwrap();
        let named = || {
            Arc::new(Ty::Named {
                symbol: cycle,
                name: "Cycle".into(),
                args: Arc::from([]),
            })
        };
        let body = Arc::new(Ty::Struct(Row {
            labels: [("next".into(), RowField::present(named()))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }));
        let aliases = [(cycle, Scheme::new(0, body))].into_iter().collect();
        assert_eq!(
            smallest_incompatible(&aliases, &named(), &named()),
            (TypeDescription::DeclaredType, TypeDescription::DeclaredType)
        );
    }

    #[test]
    fn recursive_alias_mismatch_is_deep_stack_safe() {
        use crate::symbol::{Bundle, Mint, Namespace, Version};

        std::thread::Builder::new()
            .name("deep-recursive-mismatch-leaf".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let bundle = Bundle::new("deep-recursive-leaf", Version::new(1, 0, 0)).unwrap();
                let mut mint = Mint::new(bundle);
                let stream = mint.global(None, Namespace::Types, "Stream").unwrap();
                let recursive = Arc::new(Ty::Named {
                    symbol: stream,
                    name: "Stream".into(),
                    args: Arc::from([Arc::new(Ty::Bound(0))]),
                });
                let body = Arc::new(Ty::Struct(Row {
                    labels: [
                        ("tail".into(), RowField::present(recursive)),
                        ("value".into(), RowField::present(Arc::new(Ty::Bound(0)))),
                    ]
                    .into_iter()
                    .collect(),
                    rest: Rest::Closed,
                }));
                let aliases: IndexMap<_, _> =
                    [(stream, Scheme::new(1, body))].into_iter().collect();
                let mut left_argument = Arc::new(Ty::Nat);
                let mut right_argument = Arc::new(Ty::Bool);
                for _ in 0..30_000 {
                    left_argument = Arc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(left_argument))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                    right_argument = Arc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(right_argument))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                }
                let named = |argument| {
                    Arc::new(Ty::Named {
                        symbol: stream,
                        name: "Stream".into(),
                        args: Arc::from([argument]),
                    })
                };
                let left = named(left_argument);
                let right = named(right_argument);
                assert_eq!(
                    smallest_incompatible(&aliases, &left, &right),
                    (TypeDescription::NaturalNumber, TypeDescription::Bool)
                );
                std::mem::forget(left);
                std::mem::forget(right);
            })
            .unwrap()
            .join()
            .expect("recursive mismatch leaves stay on the explicit stack");
    }

    #[test]
    fn smallest_mismatch_leaf_is_deep_stack_safe() {
        std::thread::Builder::new()
            .name("deep-mismatch-leaf".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let mut left = Arc::new(Ty::Nat);
                let mut right = Arc::new(Ty::Bool);
                for _ in 0..30_000 {
                    left = Arc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(left))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                    right = Arc::new(Ty::Struct(Row {
                        labels: [("x".into(), RowField::present(right))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                }
                assert_eq!(
                    smallest_incompatible(&IndexMap::new(), &left, &right),
                    (TypeDescription::NaturalNumber, TypeDescription::Bool)
                );
                // Deep Arc destruction is unrelated to the iterative reader.
                std::mem::forget(left);
                std::mem::forget(right);
            })
            .unwrap()
            .join()
            .expect("smallest mismatch leaf stays on its explicit stack");
    }

    #[test]
    fn recursive_cycle_retains_only_the_exact_binding_path() {
        let mut table = Table::default();
        let target = table.mint(VarSort::Type, Subject::Term);
        let first = table.mint(VarSort::Type, Subject::Term);
        let second = table.mint(VarSort::Type, Subject::Term);
        let first_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        let second_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[first as usize] = Slot::Bound {
            value: Assigned::Ty(Arc::new(Ty::plain(Ty::Var(second)))),
            by: first_reason,
        };
        table.vars[second as usize] = Slot::Bound {
            value: Assigned::Ty(Arc::new(Ty::plain(Ty::Var(target)))),
            by: second_reason,
        };
        // The input closes the first exact route. A containment route in the
        // result is an innocent sibling and must not override call advice.
        let candidate = Assigned::Ty(Arc::new(Ty::Arrow(
            Arc::new(Ty::plain(Ty::Var(first))),
            Arc::new(Ty::Struct(Row {
                labels: [(
                    "sibling".into(),
                    RowField::present(Arc::new(Ty::Var(target))),
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            })),
            Row::closed(),
        )));

        table.enter_solver_scope();
        table.begin_solver_act();
        assert_eq!(
            table.occurs(target, &candidate),
            Some(RecursiveCycleShape::CallInput)
        );
        assert_eq!(table.take_binding_reads(), [first_reason, second_reason]);

        // An absent call-input sibling has no payload. The later present
        // containment route is therefore the exact route returned.
        let absent_sibling = Assigned::Ty(Arc::new(Ty::Arrow(
            Arc::new(Ty::Struct(Row {
                labels: [(
                    "gone".into(),
                    RowField {
                        ty: Arc::new(Ty::Var(target)),
                        presence: Presence::Absent,
                    },
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            })),
            Arc::new(Ty::Struct(Row {
                labels: [("kept".into(), RowField::present(Arc::new(Ty::Var(target))))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            })),
            Row::closed(),
        )));
        assert_eq!(
            table.occurs(target, &absent_sibling),
            Some(RecursiveCycleShape::Containment)
        );
        assert!(table.take_binding_reads().is_empty());
        table.end_solver_act();
        table.leave_solver_scope();
    }

    #[test]
    fn recursive_cycle_walk_is_linear_over_a_compact_exponential_dag() {
        let mut table = Table::default();
        let target = table.mint(VarSort::Type, Subject::Term);
        let mut shared = Arc::new(Ty::Nat);
        for _ in 0..28 {
            shared = Arc::new(Ty::Arrow(shared.clone(), shared.clone(), Row::closed()));
        }

        table.enter_solver_scope();
        table.begin_solver_act();
        assert!(table.occurs(target, &Assigned::Ty(shared)).is_none());
        assert!(table.take_binding_reads().is_empty());
        table.end_solver_act();
        table.leave_solver_scope();
    }

    #[test]
    fn unrelated_bound_variable_cycle_is_bounded_and_does_not_leak_reads() {
        let mut table = Table::default();
        let target = table.mint(VarSort::Type, Subject::Term);
        let first = table.mint(VarSort::Type, Subject::Term);
        let second = table.mint(VarSort::Type, Subject::Term);
        let first_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        let second_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[first as usize] = Slot::Bound {
            value: Assigned::Ty(Arc::new(Ty::Var(second))),
            by: first_reason,
        };
        table.vars[second as usize] = Slot::Bound {
            value: Assigned::Ty(Arc::new(Ty::Var(first))),
            by: second_reason,
        };

        table.enter_solver_scope();
        table.begin_solver_act();
        assert!(
            table
                .occurs(target, &Assigned::Ty(Arc::new(Ty::Var(first))))
                .is_none()
        );
        assert!(table.take_binding_reads().is_empty());
        // A failed search must not suppress or pollute the next independent
        // search in the same solver act.
        let direct = Assigned::Ty(Arc::new(Ty::Var(target)));
        assert_eq!(
            table.occurs(target, &direct),
            Some(RecursiveCycleShape::Neutral)
        );
        assert!(table.take_binding_reads().is_empty());
        table.end_solver_act();
        table.leave_solver_scope();
    }

    #[test]
    fn recursive_cycle_paths_cover_row_tails_presences_and_first_shared_sibling() {
        let mut table = Table::default();
        let row_target = table.mint(VarSort::Row, Subject::Term);
        let row_first = table.mint(VarSort::Row, Subject::Term);
        let row_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[row_first as usize] = Slot::Bound {
            value: Assigned::Row(Arc::new(Row::of(Rest::Var(row_target)))),
            by: row_reason,
        };

        let presence_target = table.mint(VarSort::Presence, Subject::Term);
        let presence_first = table.mint(VarSort::Presence, Subject::Term);
        let presence_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[presence_first as usize] = Slot::Bound {
            value: Assigned::Presence(Presence::Var(presence_target)),
            by: presence_reason,
        };

        let ty_target = table.mint(VarSort::Type, Subject::Term);
        let left = table.mint(VarSort::Type, Subject::Term);
        let right = table.mint(VarSort::Type, Subject::Term);
        let left_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        let right_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        let shared_target = Arc::new(Ty::Var(ty_target));
        table.vars[left as usize] = Slot::Bound {
            value: Assigned::Ty(shared_target.clone()),
            by: left_reason,
        };
        table.vars[right as usize] = Slot::Bound {
            value: Assigned::Ty(shared_target),
            by: right_reason,
        };
        let siblings = Assigned::Ty(Arc::new(Ty::Arrow(
            Arc::new(Ty::Var(left)),
            Arc::new(Ty::Var(right)),
            Row::closed(),
        )));

        table.enter_solver_scope();
        table.begin_solver_act();
        assert_eq!(
            table.occurs(
                row_target,
                &Assigned::Row(Arc::new(Row::of(Rest::Var(row_first))))
            ),
            Some(RecursiveCycleShape::Neutral)
        );
        assert_eq!(table.take_binding_reads(), [row_reason]);
        assert_eq!(
            table.occurs(
                presence_target,
                &Assigned::Presence(Presence::Var(presence_first))
            ),
            Some(RecursiveCycleShape::Neutral)
        );
        assert_eq!(table.take_binding_reads(), [presence_reason]);
        assert_eq!(
            table.occurs(ty_target, &siblings),
            Some(RecursiveCycleShape::CallInput)
        );
        assert_eq!(table.take_binding_reads(), [left_reason]);

        let shared_presence = table.mint(VarSort::Presence, Subject::Term);
        let shared_presence_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[shared_presence as usize] = Slot::Bound {
            value: Assigned::Presence(Presence::Present),
            by: shared_presence_reason,
        };
        let shared_fields = Assigned::Row(Arc::new(Row {
            labels: [
                (
                    "first".into(),
                    RowField {
                        ty: Arc::new(Ty::Nat),
                        presence: Presence::Var(shared_presence),
                    },
                ),
                (
                    "second".into(),
                    RowField {
                        ty: Arc::new(Ty::Var(ty_target)),
                        presence: Presence::Var(shared_presence),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        assert_eq!(
            table.occurs(ty_target, &shared_fields),
            Some(RecursiveCycleShape::Containment)
        );
        assert_eq!(table.take_binding_reads(), [shared_presence_reason]);

        let absent_presence = table.mint(VarSort::Presence, Subject::Term);
        let absent_reason = table.reason(ReasonOrigin::Recovery, Vec::new());
        table.vars[absent_presence as usize] = Slot::Bound {
            value: Assigned::Presence(Presence::Absent),
            by: absent_reason,
        };
        for unavailable in [
            Presence::Var(absent_presence),
            Presence::Recovered(77),
            Presence::Bound(0),
            Presence::Undecided,
        ] {
            let unavailable_payload = Assigned::Row(Arc::new(Row {
                labels: [(
                    "gone".into(),
                    RowField {
                        ty: Arc::new(Ty::Var(ty_target)),
                        presence: unavailable,
                    },
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }));
            assert!(
                table.occurs(ty_target, &unavailable_payload).is_none(),
                "an unavailable field payload is not an occurs route"
            );
            assert!(table.take_binding_reads().is_empty());
        }
        table.end_solver_act();
        table.leave_solver_scope();
    }

    #[test]
    fn recursive_cycle_detection_is_stack_safe_at_thirty_thousand_fields() {
        std::thread::Builder::new()
            .name("deep-recursive-cycle".into())
            .stack_size(512 * 1024)
            .spawn(|| {
                let mut table = Table::default();
                let target = table.fresh_type_for(Subject::Term);
                let Ty::Var(var) = &*target else {
                    unreachable!()
                };
                let var = *var;
                let mut nested = target;
                for at in 0..30_000 {
                    nested = Arc::new(Ty::Struct(Row {
                        labels: [(format!("f{at}"), RowField::present(nested))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    }));
                }
                assert!(table.occurs(var, &Assigned::Ty(nested.clone())).is_some());
                // The property under test is the explicit walk, not recursive
                // destruction of a deliberately pathological Arc tree.
                std::mem::forget(nested);
            })
            .unwrap()
            .join()
            .expect("recursive-cycle detection stays on its explicit stack");
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
                            ty: Arc::new(Ty::Bound(3)),
                        },
                    );
                    row = Row {
                        labels,
                        rest: Rest::More(Arc::new(row)),
                    };
                }
                let shifted = shift(&Arc::new(Ty::Struct(row)), 4);
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
    use super::{
        ConstraintId, ExplainedScheme, ProvenanceNode, ProvenanceShape, ProvenanceTy,
        QuantifiedProvenance, ReasonId, ReasonOrigin, SchemeProvenance, Subject, Table, VarSort,
        budget_reason_slice,
    };
    use crate::tracking::Anchor;
    use crate::types::{Presence, Rest, Row, RowField, Scheme, Ty};
    use indexmap::IndexMap;
    use std::sync::Arc;

    #[test]
    fn full_reason_budget_preserves_oldest_and_failure_endpoints_and_counts_omissions() {
        let chain: Vec<_> = (0..17_000).rev().map(ReasonId::synthetic).collect();
        let (kept, omitted) = budget_reason_slice(chain);
        assert_eq!(kept.len(), 16_384);
        assert_eq!(omitted, 616);
        assert_eq!(kept.first().copied(), Some(ReasonId::synthetic(0)));
        assert_eq!(kept.last().copied(), Some(ReasonId::synthetic(16_999)));
    }

    #[test]
    fn package_and_absent_payload_provenance_follow_zonked_structure() {
        let table = Table::default();
        let absent_payload = Arc::new(Ty::Arrow(
            Arc::new(Ty::Nat),
            Arc::new(Ty::Bool),
            Row::closed(),
        ));
        let mut labels = IndexMap::new();
        labels.insert(
            "hidden".into(),
            RowField {
                presence: Presence::Absent,
                ty: absent_payload,
            },
        );
        labels.insert("shown".into(), RowField::present(Arc::new(Ty::Nat)));
        let root = Arc::new(Ty::Package(Arc::new(Ty::Struct(Row {
            labels,
            rest: Rest::Closed,
        }))));
        let provenance = table.scheme_provenance(&root, &root, &Default::default(), 0);
        assert_eq!(provenance.nodes.len(), 6);
        assert!(matches!(
            provenance.nodes[0].shape,
            super::ProvenanceShape::Ty(super::ProvenanceTy::Package)
        ));
        assert_eq!(provenance.nodes[0].children, [1]);
        let super::ProvenanceShape::Row(layout) = &provenance.nodes[2].shape else {
            panic!()
        };
        assert_eq!(layout, &[("hidden".into(), false), ("shown".into(), true)]);
        assert_eq!(provenance.nodes[2].children.len(), 3);
    }

    #[test]
    fn synthesized_nested_packages_rebuild_the_published_provenance_body() {
        let table = Table::default();
        let result = Arc::new(Ty::Struct(Row {
            labels: [(
                "hidden".into(),
                RowField {
                    presence: Presence::Bound(0),
                    ty: Arc::new(Ty::Nat),
                },
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let source = Arc::new(Ty::Arrow(Arc::new(Ty::Nat), result.clone(), Row::closed()));
        let published = Arc::new(Ty::Arrow(
            Arc::new(Ty::Nat),
            Arc::new(Ty::Package(result)),
            Row::closed(),
        ));

        let provenance = table.scheme_provenance(&source, &published, &Default::default(), 1);
        assert_eq!(
            provenance.nodes[provenance.nodes[0].children[1]].shape,
            ProvenanceShape::Ty(ProvenanceTy::Package)
        );
        assert_eq!(
            provenance.nodes[provenance.nodes[0].children[1]]
                .children
                .len(),
            1
        );
    }

    #[test]
    fn shared_payload_rc_does_not_share_sibling_presence_provenance() {
        let mut table = Table::default();
        let left_root = table.constraint_reason(ConstraintId::synthetic(10));
        let right_root = table.constraint_reason(ConstraintId::synthetic(20));
        let left = table.fresh_instance_presence();
        let right = table.fresh_instance_presence();
        let shared = Arc::new(Ty::Nat);
        let root = Arc::new(Ty::Struct(Row {
            labels: [
                (
                    "left".into(),
                    RowField {
                        presence: left.clone(),
                        ty: shared.clone(),
                    },
                ),
                (
                    "right".into(),
                    RowField {
                        presence: right.clone(),
                        ty: shared,
                    },
                ),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }));
        let provenance = SchemeProvenance {
            nodes: vec![
                ProvenanceNode {
                    shape: ProvenanceShape::Ty(ProvenanceTy::Struct),
                    roots: vec![],
                    omitted: 0,
                    children: vec![1],
                },
                ProvenanceNode {
                    shape: ProvenanceShape::Row(vec![
                        ("left".into(), true),
                        ("right".into(), true),
                    ]),
                    roots: vec![],
                    omitted: 0,
                    children: vec![2, 3, 4, 5],
                },
                ProvenanceNode {
                    shape: ProvenanceShape::Presence,
                    roots: vec![left_root],
                    omitted: 0,
                    children: vec![],
                },
                ProvenanceNode {
                    shape: ProvenanceShape::Ty(ProvenanceTy::Leaf),
                    roots: vec![],
                    omitted: 0,
                    children: vec![],
                },
                ProvenanceNode {
                    shape: ProvenanceShape::Presence,
                    roots: vec![right_root],
                    omitted: 0,
                    children: vec![],
                },
                ProvenanceNode {
                    shape: ProvenanceShape::Ty(ProvenanceTy::Leaf),
                    roots: vec![],
                    omitted: 0,
                    children: vec![],
                },
            ],
            quantified: vec![],
        };
        table.mark_opened_type(&root, &provenance);
        table.enter_solver_scope();

        table.begin_solver_act();
        table.presence_of(&left);
        let left_reads = table.take_binding_reads();
        table.end_solver_act();
        table.begin_solver_act();
        table.presence_of(&right);
        let right_reads = table.take_binding_reads();
        table.end_solver_act();
        table.leave_solver_scope();

        assert_eq!(left_reads.len(), 1);
        assert_eq!(right_reads.len(), 1);
        assert_eq!(
            table.reasons[left_reads[0].index() as usize].parents,
            [left_root]
        );
        assert_eq!(
            table.reasons[right_reads[0].index() as usize].parents,
            [right_root]
        );
    }

    #[test]
    fn quantified_row_positions_keep_their_sort_when_opened() {
        let mut table = Table::default();
        let explained = ExplainedScheme {
            scheme: Scheme::new(1, Arc::new(Ty::Struct(Row::of(Rest::Bound(0))))),
            provenance: SchemeProvenance {
                nodes: Vec::new(),
                quantified: vec![QuantifiedProvenance {
                    sort: VarSort::Row,
                    roots: Vec::new(),
                    omitted: 0,
                }],
            },
        };
        table.instantiate_scoped(Anchor::GENERATED, &explained, None);
        assert_eq!(table.var_meta.last().unwrap().sort, VarSort::Row);
        assert!(matches!(
            table.reasons[table.var_meta.last().unwrap().minted_by.index() as usize].origin,
            ReasonOrigin::Variable {
                sort: VarSort::Row,
                ..
            }
        ));
    }

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

#[cfg(test)]
mod publication_tests {
    //! Tests that fabricate diagnostic records no real solve would publish.

    use super::*;
    use crate::test_support;

    #[test]
    fn deep_reason_slices_are_iterative() {
        let (_, _, mut output) = test_support::inferred("let x = 1n", Trace::Complete);
        let base = output.diagnostics().reasons().len() as u64 + 100;
        let depth = 30_000u64;
        for at in 0..depth {
            output.diagnostics_mut().reasons.push(Reason {
                id: ReasonId::synthetic(base + at),
                parents: (at > 0)
                    .then(|| ReasonId::synthetic(base + at - 1))
                    .into_iter()
                    .collect(),
                origin: ReasonOrigin::Recovery,
                reachable: true,
            });
        }
        let slice = output
            .diagnostics()
            .reason_ancestors(ReasonId::synthetic(base + depth - 1));
        assert_eq!(slice.len(), depth as usize);
    }
}
