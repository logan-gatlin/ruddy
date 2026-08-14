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
//! Inference runs after lowering and mutates the [`Program`] it is handed:
//! every [`Term`]'s `ty` goes from [`Core::Undecided`] to what was inferred for
//! it, fully resolved, so nothing downstream ever needs the solver's variable
//! table to read a type. Errors do not stop either pass — a term that failed to
//! type still has a type, [`Core::Undecided`], which unifies with everything so
//! that one mistake is reported once rather than echoed by every consumer.

mod constrain;
pub mod sat;
mod solve;

use std::{
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
        Assigned, Atom, Core, Formula, ParamKind, Presence, Rest, Row, RowField, Scheme, Shape, Ty,
        TyVar,
    },
};
use constrain::Constrain;
use solve::Solve;

#[derive(Debug, Clone)]
pub struct Output {
    /// What each `type` declaration stands for: the semantic type its body
    /// denotes, one step deep. A name inside a body stays a [`Core::Named`] and
    /// is looked up here again, which is how a declaration that names itself
    /// stays a finite value — and why this map, not the type, is what a
    /// recursive type is made of. See [`unfold`].
    ///
    /// A [`Scheme`] rather than a bare type, because handing a declaration its
    /// arguments is substituting for the [`Core::Bound`]s standing in for its
    /// parameters — which is what instantiating a scheme already is, down to
    /// the same `open`. A declaration taking no parameters is a scheme binding
    /// nothing, and opening one returns its body unchanged.
    pub aliases: IndexMap<Symbol, Scheme>,
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
    /// [`Table::published`] — so two rows of this map spelling `'a` are two
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
    /// that definition's zonked terms can name, in the [`Core::Bound`]
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
#[derive(Debug, Clone, Default)]
pub struct Store {
    pub batches: Vec<Batch>,
}

/// One tagged batch of the store: where the program said it, why, and what it
/// said.
#[derive(Debug, Clone)]
pub struct Batch {
    /// Where the program said it: the match, the use site, or the annotation.
    pub span: Span,
    pub origin: Origin,
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
}

/// What a match-coverage batch carries beyond its formula, so that the two
/// readers who need more than "is this satisfiable" have it.
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
    /// One side is [`Core::Undecided`], which unifies with anything.
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
    /// A type carrying fields, taken apart: the fields both sides name against
    /// each other, the fields only one names into what the other side allows
    /// beyond its own, and then the two cores.
    ///
    /// Recorded whenever either side carries a label, whatever the cores are —
    /// so it is over a struct against a struct, as it always was, and over the
    /// `Nat` carrying an `x` that only a declaration can reach.
    Struct,
    /// [`Rule::Struct`] about the other shape: two sums, the cases both name
    /// against each other and the cases only one names into what the other's
    /// tail allows.
    ///
    /// One rule in the code and two here, because a reader stepping through a
    /// solve is reading about their program: a line about fields over a goal
    /// about `` `Some `` and `` `None `` describes something they never wrote.
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
    Bound { var: TyVar, value: Assigned },
    /// The goal was replaced by the goals that follow it one level deeper —
    /// the halves of an arrow, the fields of a struct, or the same goal asked
    /// again about what a name stands for.
    Decomposed,
    /// Reported, and the goal abandoned.
    Failed(ErrorKind),
}

/// One thing that has to be true of a definition's types, and where the program
/// said so.
#[derive(Debug, Clone)]
pub struct Constraint {
    pub span: Span,
    pub kind: ConstraintKind,
}

#[derive(Debug, Clone)]
pub enum ConstraintKind {
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
        /// The variables the annotation on it minted, which R23's closing rule
        /// may not touch: an effect tail the reader left open is the reader's
        /// however few times it occurs. Empty for a binding with no annotation,
        /// which leaves nothing open to protect.
        opened: Range<TyVar>,
        /// The `where` clause the annotation promised, or [`Formula::True`]
        /// where none was written. What the scheme published for `symbol`
        /// requires of its presences: R10 makes the clause the contract, so a
        /// use of the name sees it rather than whatever the value worked out.
        promised: Formula,
        /// What the value requires, including that it match the annotation when
        /// one was written. Solved first, at `level`.
        value: Vec<Constraint>,
        /// What the rest of the term requires, with `symbol` bound to the
        /// scheme generalization produced. Solved at `level - 1`.
        body: Vec<Constraint>,
    },
    /// A use of a let-bound name: `ty` is a fresh copy of whatever scheme the
    /// enclosing [`ConstraintKind::Let`] published for `symbol`.
    Instance { symbol: Symbol, ty: Rc<Ty> },
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
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
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
    /// it. Every type has fields *and* may have cases, so a base can be a
    /// sum-cored type that is missing a *field* — `` (`A 1).x `` is exactly
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
    /// The base need not be a struct, now that every type carries labels. A
    /// type whose core allows nothing more and which names none allows no label
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
    /// An annotation left something open with `..` or `when` that the definition
    /// then decided. The type the definition was checked against is not the
    /// type it has, so the contract the annotation looked like — works for any
    /// rest, works whether or not the field is there — was never checked and
    /// does not hold.
    ///
    /// No payload: the complaint is about the written type, which the span
    /// already points at, and the type it should have said instead is the
    /// scheme printed beside it.
    AnnotationTooOpen,
    /// A `..` was decided to stand for a label the row it tails already names.
    /// `{ x: Nat, ..r }` says "an `x`, plus whatever else `r` is", so `r`
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
    PresenceRequired { formula: String },
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
}

/// What one type variable is known to be. Private to inference, and rightly so:
/// it is the solver's working state rather than part of the type language, and
/// nothing downstream ever sees a [`Core::Var`] to want a slot for — generalizing
/// and zonking are what make sure of that.
#[derive(Debug, Clone)]
enum Slot {
    Unbound,
    Bound(Assigned),
}

/// What one variable may not stand for: the labels, and the kind of row the
/// condition came from.
///
/// The shape is stored rather than read back off wherever the variable ends up,
/// because there is no longer anywhere to read it from — a core variable stands
/// for a whole type, and the labels forbidden of it are that type's fields. See
/// [`Table::lacks`].
type Lacks = (Shape, IndexSet<String>);

/// Everything a solve can change about a [`Table`]: what the variables are
/// known to be, which level each belongs to, and what they may not stand for.
/// Taken and put back by [`Rule::Congruent`], which is the one rule that asks a
/// question before it is sure the question is the right one to have asked.
///
/// The levels travel with the slots rather than beside them: the two lists are
/// indexed by the same variable, so putting one back without the other would
/// leave a variable minted since the snapshot with a level and no slot.
type Known = (Vec<Slot>, Vec<u32>, HashMap<TyVar, Lacks>);

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

/// What a variable an annotation left open came back as, once the group it was
/// in is solved. See [`Table::residue`].
#[derive(Debug, Clone, Copy)]
enum Residue {
    /// Still a variable of its own sort with nothing attached, and which one:
    /// the annotation kept its promise about this slot, whatever the variable
    /// ended up being called. Named rather than a bare yes, because two slots
    /// that came back as one variable are the case a bare yes cannot see.
    Open(TyVar),
    /// The undecided value of its own sort: abandoned by a failure rather than
    /// solved by the definition. Something already went wrong and pointed it
    /// here, and a complaint about the annotation would be that failure said
    /// again in words about the wrong line.
    Nothing,
    /// Something the reader could have written instead. The annotation said
    /// this slot was open and the definition closed it.
    Decided,
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
    /// The variables the annotation minted, which are the ones it left for the
    /// definition to decide. Empty for a definition with no annotation. See
    /// [`Table::narrowed`].
    opened: Range<TyVar>,
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
/// room for, and rightly so: whether an annotation promised more than the value
/// kept is a question about a *written type*, asked once the solve is over, and
/// the solver has never seen a written anything. [`Scoped::opened`] is the same
/// pair about a definition's own annotation, checked in the same place.
struct Annotated {
    /// The annotation's span, which is the line the reader has to change.
    span: Span,
    /// The variables it minted, which are the ones it left for the value to
    /// decide. See [`Table::narrowed`].
    opened: Range<TyVar>,
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
#[derive(Default)]
struct Table {
    /// One slot per variable; [`Core::Var`] indexes into it.
    ///
    /// A group rather than a definition, and the difference is only where the
    /// line falls: two definitions that name each other are solved together, so
    /// a variable one of them minted may still be open while the other is being
    /// walked. It is still nobody's but the group's, because the group is
    /// finished before anything outside it is looked at.
    vars: Vec<Slot>,
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
    /// recorded, and the core variable is the only place left to put it: there
    /// is no tail variable beside it any more, because the core is the tail.
    /// Nothing in [`Ty`] can express the side condition, so it is held here,
    /// beside the slots, and enforced at the one place a variable acquires a
    /// value.
    ///
    /// A sum's tail is under the same condition, for the same reason and with
    /// its own labels: `` `A Nat | ..?3 `` reads the same sentence about cases.
    /// So the shape says which of the two a condition came from, and a
    /// [`Shape::Struct`] one is always on a core variable while a
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
    /// What the program has required of its presences so far. Grown by
    /// generation (a match's coverage), by instantiation (a constrained
    /// scheme's formula) and by lowering (an annotation's `where` clause), and
    /// consulted only where R8 allows: at a generalization boundary, at the
    /// use-site and annotation checks, and — through
    /// [`Output::store`] — in the patterns phase. Never by unification.
    store: Store,
    /// Whether some batch has already flipped the store unsatisfiable. The
    /// cascade rule: that batch owns the single resulting error, and every
    /// later SAT-dependent complaint — a use-site violation, an annotation
    /// disagreement, a scheme's `where` clause — is suppressed, because all of
    /// them would be the same contradiction said again.
    unsat: bool,
}

/// Which quantified position each variable was given, in the two alphabets a
/// scheme has: `'a` for a type or a row, and the bare `a` of a `when` clause
/// for a presence.
///
/// Two maps rather than one, for the reason [`Scheme`] gives: one numbering
/// would leave a visible gap in whichever alphabet came second. A variable's
/// sort is fixed where it was minted, so no variable is ever in both.
#[derive(Debug, Default, Clone)]
struct Subst {
    types: HashMap<TyVar, u32>,
    presences: HashMap<TyVar, u32>,
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
        .collect();

    // Aliases first: annotations refer to them. A name inside a body stays a
    // name, so this pass reads no alias it is still building and the order it
    // runs in decides nothing — which is what lets two declarations refer to
    // each other.
    for (symbol, decl) in &program.types {
        // The parameters are already `Core::Bound`s by their position, so the
        // scheme is closed by counting them rather than by walking anything.
        let body = lower_type(mint, &mut table, &decl.value);
        aliases.insert(*symbol, Scheme::new(decl.params.len() as u32, body));
    }

    // And what each operation was declared to be, before any body is walked: a
    // perform site and a handler arm each want the two sides of one, and both
    // come straight off the declaration. Lowered here rather than per use, for
    // the reason the aliases above are: a signature is a plain closed arrow, so
    // it mentions no variable and lowering one twice would only mint two copies
    // of nothing.
    let mut operations = constrain::Operations::new();
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

    let mut schemes = IndexMap::new();
    let mut locals = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut promises = IndexMap::new();
    let mut steps = Vec::new();
    // The groups are read out before anything is solved: solving mutates the
    // definitions they name, and which definitions have to be typed together is
    // a fact about the lowered program that nothing here changes.
    let groups: Vec<Vec<Symbol>> = program
        .groups
        .iter()
        .map(|group| group.members.clone())
        .collect();
    for members in groups {
        // Every member is in scope, monomorphically, before any of them is
        // walked. That is the whole of what makes recursion typable: a use of a
        // group member inside the group is the one type the group is deciding,
        // rather than a copy of a scheme that does not exist yet. A use of a
        // definition in an earlier group is a [`Binding::Poly`] and instantiates
        // as it always has, which is what keeps let-polymorphism.
        //
        // A member with an annotation is bound to *it* rather than to a fresh
        // variable. A variable would never be tied to what was written, so the
        // recursive uses the annotation exists for would be checked against
        // nothing at all.
        let scoped: Vec<Scoped> = members
            .iter()
            .map(|symbol| {
                // The annotation is the contract: the body is checked against
                // it, and it — not whatever the body's constraints worked out
                // along the way — is what the definition means to everyone
                // downstream.
                //
                // Which is only honest while the body cannot decide the parts
                // of it the annotation left open. Nothing stops it: a `..`, a
                // `..r` and a `when` each lower to an ordinary variable, and a
                // variable is something the solve is free to bind. So the
                // variables this lowering mints are counted off here and
                // checked once the group is solved — see [`Table::narrowed`].
                // The alternative is to make them unbindable, and that is a
                // larger language than this one has: a variable that refuses to
                // be bound needs a rule saying what happens when something
                // tries, and the answer has to travel all the way back to the
                // annotation anyway. Checking says the same thing, at the one
                // place that knows which variables came from where.
                let from = table.vars.len() as TyVar;
                let annotated = program.terms[symbol].annotation.as_ref().map(|annotation| {
                    let (ty, formula, names) = lower_annotation(mint, &mut table, annotation);
                    // The clause goes into the store as its own batch, so that
                    // it is what a use of this name sees and what the body is
                    // held to. See R10.
                    if !formula.is_true() {
                        let origin = Origin::Annotation(Named {
                            labels: names.clone(),
                        });
                        table.require(annotation.ty.span, origin, formula.clone());
                    }
                    (ty, formula, names)
                });
                let opened = from..table.vars.len() as TyVar;
                let promised = annotated
                    .as_ref()
                    .map_or(Formula::True, |(_, formula, _)| formula.clone());
                let names = annotated
                    .as_ref()
                    .map_or_else(Vec::new, |(_, _, names)| names.clone());
                let bound = annotated.map_or_else(|| table.fresh_type(), |(ty, _, _)| ty);
                env.insert(*symbol, Binding::Mono(bound.clone()));
                Scoped {
                    symbol: *symbol,
                    bound,
                    opened,
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
            let decl = &mut program.terms[&scoped.symbol];
            let mut constrain = Constrain {
                table: &mut table,
                mint,
                env: &mut env,
                aliases: &aliases,
                out: Vec::new(),
                annotated: Vec::new(),
                operations: &operations,
                // A definition's value is computed where no handler can reach
                // it, so it is walked at the empty closed row and outside every
                // function — which is what makes performing an effect at the
                // top level an error rather than a silently discarded effect.
                ambient: constrain::Ambient {
                    row: Row::closed(),
                    inside: false,
                },
                answer: None,
            };
            // Checked against exactly what the rest of the group sees this
            // definition as. For an annotated one that is the annotation, as it
            // has always been; for the rest it is the variable standing in for
            // the definition, and checking against a bare variable is inferring
            // and equating — see [`Constrain::check_term`]. The equation is what
            // ties the name the body used to the type the body has.
            constrain.check_term(&mut decl.value, &scoped.bound);
            let generated = constrain.out;
            let annotated = constrain.annotated;
            let ty = match &decl.annotation {
                Some(_) => scoped.bound.clone(),
                None => decl.value.ty.clone(),
            };
            let reported = errors.len();
            let published = locals.len();

            Solve {
                table: &mut table,
                errors: &mut errors,
                steps: &mut steps,
                aliases: &aliases,
                nominal: &nominal,
                definition: scoped.symbol,
                depth: 0,
                assumed: Vec::new(),
                schemes: HashMap::new(),
                locals: &mut locals,
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
        // level leaves free is a [`Core::Var`] the enclosing binder still owns.
        for (at, member) in solved.into_iter().enumerate() {
            let symbol = member.scoped.symbol;
            let (from, to) = (bounds[at], bounds[at + 1]);
            let decl = &mut program.terms[&symbol];

            // One complaint per definition, not per variable: an annotation
            // that is too open is one thing to rewrite however many of its
            // `..`s and `when`s the body pinned down, and the reader is being sent
            // to the same line either way. Held back when *this* definition
            // already said something, because everything a failure abandons it
            // also decides — pointing it at the undecided type is a binding like
            // any other — and a second complaint about the fallout of the first
            // is one mistake said twice. Its own range and nobody else's: a
            // definition that shares a group with a broken one is not the one
            // with something to rewrite.
            //
            // Said here, past every member's solve, rather than beside the
            // list it is about, so that it lands after all of them: a member's
            // own range was fixed while the group was being solved, and a
            // complaint written into the middle of it would move everybody
            // else's.
            let told = errors.len();
            if let Some(annotation) = &decl.annotation
                // A synthetic annotation — a pattern's exact demand — is meant
                // to be decided, so there is no promise here to hold anyone to.
                && !synthetic(&annotation.ty)
                && from == to
                && table.narrowed(member.scoped.opened.clone())
            {
                errors.push(Error {
                    span: annotation.ty.span,
                    kind: ErrorKind::AnnotationTooOpen,
                });
            }
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
                    bodies.clone(),
                )
            {
                errors.push(Error {
                    span: annotation.ty.span,
                    kind: ErrorKind::AnnotationAllows { allowed, required },
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
                if from == to && table.narrowed(annotated.opened.clone()) {
                    errors.push(Error {
                        span: annotated.span,
                        kind: ErrorKind::AnnotationTooOpen,
                    });
                }
                // And its clause is a promise about a smaller scope on the same
                // terms. Read against the same stretch, which needs no narrower
                // bookkeeping: a batch that says nothing about the clause's
                // variables projects away to `true`, so everything outside the
                // nested value costs nothing but the walk over it.
                if from == to
                    && !table.unsat
                    && let Some((allowed, required)) =
                        table.disagreement(&annotated.promised, &annotated.names, bodies.clone())
                {
                    errors.push(Error {
                        span: annotated.span,
                        kind: ErrorKind::AnnotationAllows { allowed, required },
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
            table.close_effects(&member.ty, 0, &member.scoped.opened);
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
        .terms
        .keys()
        .enumerate()
        .map(|(at, symbol)| (*symbol, at))
        .collect();
    schemes.sort_by(|one, _, other, _| position[one].cmp(&position[other]));
    constraints.sort_by(|one, _, other, _| position[one].cmp(&position[other]));
    promises.sort_by(|one, _, other, _| position[one].cmp(&position[other]));

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

    Output {
        aliases,
        schemes,
        locals,
        constraints,
        steps,
        store,
        promises,
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
    let kind = match &batch.origin {
        Origin::Coverage(_) => return,
        Origin::Instance(named) | Origin::Annotation(named) => {
            // Which of the two annotation complaints it is: a clause with no
            // model of its own is wrong by itself, and the body under it —
            // which may do nothing with the type at all — has no part in it.
            // Asked of the formula as written rather than of
            // [`Table::resolved`], which is the whole distinction: resolving
            // substitutes what the solve decided, so a clause the definition
            // ruled out and one that rules itself out would come back the
            // same, and every clause would blame the clause. An annotation
            // check is one of the three places R8 lets the solver run, so
            // this widens nothing.
            let alone = sat::satisfiable(&batch.formula);
            let formula = crate::ui::in_labels(&batch.formula, &named.labels);
            match &batch.origin {
                Origin::Annotation(_) if alone => ErrorKind::PresenceImpossible { formula },
                Origin::Annotation(_) => ErrorKind::ClauseImpossible { formula },
                _ => ErrorKind::PresenceRequired { formula },
            }
        }
    };
    errors.push(Error {
        span: batch.span,
        kind,
    });
}

impl Table {
    /// What is known now, to be handed back to [`restore`](Self::restore) if
    /// what follows turns out not to have been asked.
    ///
    /// Copied rather than journalled. A trail of undo records would be the
    /// cheaper thing and a second representation of the solution to keep
    /// honest; this is one line, and the one caller takes it per congruence
    /// between two applications rather than per binding.
    fn snapshot(&self) -> Known {
        (self.vars.clone(), self.levels.clone(), self.lacks.clone())
    }

    /// Put back what [`snapshot`](Self::snapshot) took.
    ///
    /// The variables minted since go with it. Nothing can still be pointing at
    /// one: a fresh variable reaches the rest of the solve only by being bound
    /// into something, and every binding made since is being undone here too.
    fn restore(&mut self, (vars, levels, lacks): Known) {
        self.vars = vars;
        self.levels = levels;
        self.lacks = lacks;
    }

    /// One more variable, of no sort yet, at the level being walked. A
    /// variable's sort is fixed by the position it was minted for, and the four
    /// functions below are those positions; nothing else may call this.
    fn mint(&mut self) -> TyVar {
        let var = self.vars.len() as TyVar;
        self.vars.push(Slot::Unbound);
        self.levels.push(self.level);
        var
    }

    /// A variable standing for a whole type: a bare core, carrying no fields
    /// of its own, so that binding it takes whatever it is against entire.
    fn fresh_type(&mut self) -> Rc<Ty> {
        let var = self.mint();
        Rc::new(Ty::plain(Core::Var(var)))
    }

    /// A variable standing for the core of a type whose fields are written
    /// beside it — the projection's demand, and nothing else so far.
    ///
    /// The same sort as [`fresh_type`](Self::fresh_type) and a different
    /// position, which is why it is its own function. A fresh *type* is bare, so
    /// binding it takes whatever it meets entire, fields and all; a fresh
    /// *core* has labels standing next to it, so binding it takes only what the
    /// other side is under those labels — and the labels beside it are a
    /// condition on it, which is what [`Table::note_lacks`] then records. It
    /// hands back the variable rather than a type because its caller has a row
    /// to build around it.
    fn fresh_core(&mut self) -> TyVar {
        self.mint()
    }

    /// A variable standing for the rest of a row.
    fn fresh_row(&mut self) -> Rest {
        Rest::Var(self.mint())
    }

    /// A variable standing for whether one label is there.
    fn fresh_presence(&mut self) -> Presence {
        Presence::Var(self.mint())
    }

    /// Follow bound variables until reaching something that is not one. Only
    /// the head is resolved; a composite's children still need their own
    /// resolution, which is what [`zonk`](Self::zonk) does exhaustively.
    ///
    /// The splice is what makes this more than a lookup, and it is now the only
    /// splice a struct has. A core variable stands for a whole type, so
    /// `{ x: Nat, ..?3 }` becomes, once `?3` is known, that type's core carrying
    /// both its own labels and the `x` — the outer labels winning, the way
    /// [`Table::canon`] settles a sum tail's. Every reader of a type goes
    /// through here, so no reader has to know that a core can stand for
    /// something with fields of its own.
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
        while let Core::Var(v) = &ty.core {
            let Slot::Bound(Assigned::Ty(inner)) = &self.vars[*v as usize] else {
                break;
            };
            budget = budget
                .checked_sub(1)
                .expect("a chain of bound variables closed a cycle the occurs check should refuse");
            ty = splice(&ty.fields, inner);
        }
        ty
    }

    /// A sum's cases flattened: its own labels joined with every label its tail
    /// has already accumulated, and what remains of the tail — an unbound
    /// variable, [`Rest::Closed`] or [`Rest::Undecided`] — as far as the solver
    /// has got.
    ///
    /// A sum's tails and nothing else, now that a struct's `..` is its core and
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
    fn canon(&self, row: &Row) -> Row {
        let mut labels: IndexMap<String, RowField> = IndexMap::new();
        let mut row = row.clone();
        loop {
            for (name, field) in &row.labels {
                labels.entry(name.clone()).or_insert_with(|| field.clone());
            }
            let deeper = match &row.rest {
                Rest::More(more) => (**more).clone(),
                Rest::Var(var) => match &self.vars[*var as usize] {
                    Slot::Bound(Assigned::Row(bound)) => (**bound).clone(),
                    _ => {
                        return Row {
                            labels,
                            rest: row.rest,
                        };
                    }
                },
                _ => {
                    return Row {
                        labels,
                        rest: row.rest,
                    };
                }
            };
            row = deeper;
        }
    }

    /// Follow a presence variable to what it stands for.
    fn presence_of(&self, presence: &Presence) -> Presence {
        let mut presence = presence.clone();
        while let Presence::Var(var) = presence {
            let Slot::Bound(Assigned::Presence(inner)) = &self.vars[var as usize] else {
                break;
            };
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
        let (a, b) = (self.resolve(a), self.resolve(b));
        self.alike_labels(&a.fields, &b.fields) && self.alike_core(&a.core, &b.core)
    }

    /// [`alike`](Self::alike) about two label maps: name against name, since a
    /// type's fields have no tail to agree about. The order the labels were
    /// written in decides nothing.
    fn alike_labels(&self, a: &IndexMap<String, RowField>, b: &IndexMap<String, RowField>) -> bool {
        a.len() == b.len()
            && a.iter().all(|(name, field)| {
                b.get(name).is_some_and(|other| {
                    self.alike_presence(&field.presence, &other.presence)
                        && self.alike(&field.ty, &other.ty)
                })
            })
    }

    /// [`alike`](Self::alike) about two cores.
    fn alike_core(&self, a: &Core, b: &Core) -> bool {
        match (a, b) {
            (Core::Unit, Core::Unit)
            | (Core::Nat, Core::Nat)
            | (Core::Undecided, Core::Undecided) => true,
            (Core::Var(x), Core::Var(y)) => x == y,
            // A quantified variable is not written out, on purpose: nothing
            // reaches here holding one — an argument was lowered from what
            // somebody wrote at a use site, and a scheme is opened before it is
            // ever compared — and a variant this cannot tell about belongs in
            // the arm below, whose answer is the safe one.
            (Core::Arrow(from, to, effects), Core::Arrow(other_from, other_to, others)) => {
                self.alike(from, other_from)
                    && self.alike(to, other_to)
                    && self.alike_row(effects, others)
            }
            (Core::Sum(cases), Core::Sum(others)) => self.alike_row(cases, others),
            // The name and the arguments, and nothing of the body. Two
            // applications of one declaration with equal arguments are
            // certainly the same type, which is all this has to be right
            // about — it says no where it cannot tell, and two applications
            // that differ only in an argument the declaration discards are one
            // of the places it cannot. Arity belongs to the declaration, so
            // equal symbols mean equal lengths and the zip drops nothing.
            (
                Core::Named { symbol, args, .. },
                Core::Named {
                    symbol: other,
                    args: other_args,
                    ..
                },
            ) => {
                symbol == other
                    && args
                        .iter()
                        .zip(other_args.iter())
                        .all(|(arg, other)| self.alike(arg, other))
            }
            // Two different shapes, and — since this is the one arm that is not
            // written out — anything a later variant is put against. Both are
            // the same answer, and it is the safe one.
            _ => false,
        }
    }

    /// [`alike_labels`](Self::alike_labels) about a sum's cases: flattened
    /// first, so that a row spliced into a tail is recognized as the row written
    /// out flat, and then the tails as well.
    fn alike_row(&self, a: &Row, b: &Row) -> bool {
        let (a, b) = (self.canon(a), self.canon(b));
        self.alike_labels(&a.labels, &b.labels)
            && match (&a.rest, &b.rest) {
                (Rest::Closed, Rest::Closed) | (Rest::Undecided, Rest::Undecided) => true,
                (Rest::Var(x), Rest::Var(y)) => x == y,
                _ => false,
            }
    }

    /// [`alike`](Self::alike) about two presences.
    fn alike_presence(&self, a: &Presence, b: &Presence) -> bool {
        match (self.presence_of(a), self.presence_of(b)) {
            (Presence::Present, Presence::Present)
            | (Presence::Absent, Presence::Absent)
            | (Presence::Undecided, Presence::Undecided) => true,
            (Presence::Var(x), Presence::Var(y)) => x == y,
            _ => false,
        }
    }

    /// Whether `var` occurs in what it is about to be bound to, whichever sort
    /// that is. One variable space, so a core variable hiding inside a row is
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
    /// It is answered yes only about a core variable, and that is a fact about
    /// the solver rather than a hole in the walk. Two reasons, and between them
    /// they cover every route here:
    ///
    /// - A variable's sort is fixed where it was minted and never changes, so a
    ///   presence variable is never the same variable as a row or core one. Most
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

    /// Every variable `ty` mentions — in its core, and in the fields it carries.
    fn mentions_ty(&self, ty: &Rc<Ty>, found: &mut Vec<TyVar>) {
        let ty = self.resolve(ty);
        match &ty.core {
            Core::Var(var) => found.push(*var),
            Core::Arrow(from, to, effects) => {
                self.mentions_ty(from, found);
                self.mentions_ty(to, found);
                self.mentions_row(effects, found);
            }
            Core::Sum(cases) => self.mentions_row(cases, found),
            // A declared type is descended into as far as its arguments and no
            // further, here and in every walk below. What one stands for was
            // lowered from what the user wrote and mentions no variable at all
            // — lowering refuses a `..` or a `when` in a declaration for exactly
            // this reason — so there is nothing in the body to find and nothing
            // to rebuild; and stopping there is what keeps a walk over a type
            // that names itself finite. The arguments are the other half: they
            // were written at the use site and hold whatever it held, so
            // skipping them would miss a cycle.
            Core::Named { args, .. } => {
                for arg in args.iter() {
                    self.mentions_ty(arg, found);
                }
            }
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => {}
        }
        self.mentions_labels(&ty.fields, found);
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
            self.mentions_presence(&field.presence, found);
            self.mentions_ty(&field.ty, found);
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
        let ty = self.resolve(ty);
        // Every type carries fields, so every type says something here — and
        // the core beside them is the only place there is left to say it. The
        // labels land on the type's own core variable, which stands for a type
        // those labels are already on: `{ x: Nat, ..?3 }` reads "an `x`, and
        // whatever else `?3` is", so a `?3` with an `x` of its own would name
        // the field twice.
        let labels: IndexSet<String> = ty.fields.keys().cloned().collect();
        if let Core::Var(var) = ty.core {
            self.forbidden(var, Shape::Struct, labels);
        }
        for field in ty.fields.values() {
            let field = field.ty.clone();
            self.note_lacks(&field);
        }
        match &ty.core {
            Core::Arrow(from, to, effects) => {
                let (from, to, effects) = (from.clone(), to.clone(), effects.clone());
                self.note_lacks(&from);
                self.note_lacks(&to);
                self.note_lacks_row(&effects, Shape::Effect);
            }
            Core::Sum(cases) => {
                let cases = cases.clone();
                self.note_lacks_row(&cases, Shape::Sum);
            }
            // The body holds no row of its own to speak of, but an argument is
            // whatever the use site wrote — including an open row whose tail
            // must acquire the labels around it.
            //
            // And the declaration has something to say about the argument
            // itself, at every position it takes a row: `type WithX r = { x:
            // Nat, ..r }` says whatever is written for `r` has no `x`, and that
            // holds whether or not anything ever unfolds `WithX` to find out.
            // Recorded here, where the application is, rather than left to
            // [`Solve::unfold`] — a goal decided by [`Rule::Congruent`] never
            // unfolds either side, and the condition would simply be lost.
            Core::Named { symbol, args, .. } => {
                let (symbol, args) = (*symbol, args.clone());
                for (at, arg) in args.iter().enumerate() {
                    // Cloned out, since forbidding borrows the table; and only
                    // when there is something to forbid, which a parameter that
                    // tails nothing never has.
                    let demand = match self.params.get(&symbol).and_then(|kinds| kinds.get(at)) {
                        Some(kind) if !kind.lacks().is_empty() => {
                            Some((kind.cases().map(|(shape, _)| shape), kind.lacks().clone()))
                        }
                        _ => None,
                    };
                    match demand {
                        // A struct's `..` is the type's core, so the condition
                        // lands on the argument's own core variable — the one
                        // thing that would still be free to acquire the label.
                        Some((None, labels)) => {
                            if let Core::Var(var) = self.resolve(arg).core {
                                self.forbidden(var, Shape::Struct, labels);
                            }
                        }
                        // A sum's rest and an arrow's effects are both spliced
                        // into a row, so both read the argument for the labels
                        // it allows and put the condition on what is open past
                        // them.
                        Some((Some(shape), labels)) => {
                            let written = self.resolve(arg).cases();
                            self.forbid(&written, shape, &labels);
                        }
                        None => {}
                    }
                    self.note_lacks(arg);
                }
            }
            Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// [`note_lacks`](Self::note_lacks) about a sum's cases: every label it and
    /// its tail name is forbidden of whatever is still open past them, and each
    /// label's payload is walked in turn.
    fn note_lacks_row(&mut self, row: &Row, shape: Shape) {
        let flat = self.canon(row);
        let labels: IndexSet<String> = flat.labels.keys().cloned().collect();
        for field in flat.labels.values() {
            let ty = field.ty.clone();
            self.note_lacks(&ty);
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
    fn require(&mut self, span: Span, origin: Origin, formula: Formula) {
        self.store.batches.push(Batch {
            span,
            origin,
            formula,
            flipped: false,
        });
    }

    /// Point the use-site batches emitted since `from`, and still carrying
    /// `at`, at `span` instead. See [`Constrain::infer_term`]'s application
    /// arm, which is the one caller and the whole of the rule.
    fn aim(&mut self, from: usize, at: Span, span: Span) {
        for batch in &mut self.store.batches[from..] {
            if matches!(batch.origin, Origin::Instance(_)) && batch.span == at {
                batch.span = span;
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
            .map(|batch| Batch {
                span: batch.span,
                origin: match &batch.origin {
                    Origin::Coverage(coverage) => Origin::Coverage(Coverage {
                        arms: coverage.arms.iter().map(|arm| self.resolved(arm)).collect(),
                        fields: coverage
                            .fields
                            .iter()
                            .map(|(name, presence)| (name.clone(), self.presence_of(presence)))
                            .collect(),
                    }),
                    Origin::Instance(named) => Origin::Instance(self.settled_names(named)),
                    Origin::Annotation(named) => Origin::Annotation(self.settled_names(named)),
                },
                formula: self.resolved(&batch.formula),
                flipped: batch.flipped,
            })
            .collect();
        Store { batches }
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
    /// The body's side is projected onto the clause's own variables first:
    /// a match inside the definition may relate presences the annotation never
    /// mentions, and those are its own business.
    fn disagreement(
        &self,
        promised: &Formula,
        names: &[(String, Presence)],
        batches: Range<usize>,
    ) -> Option<(String, String)> {
        let allowed = self.resolved(promised);
        let needed = Formula::all(
            self.store.batches[batches]
                .iter()
                .map(|batch| self.resolved(&batch.formula)),
        );
        let mut atoms = Vec::new();
        allowed.atoms(&mut atoms);
        let needed = sat::project(&needed, &atoms);
        if sat::entails(&allowed, &needed) {
            return None;
        }
        // Both halves are quoted in the names the reader wrote, which means
        // looking each one up by the presence it decides — as the solve now has
        // it, not as the annotation minted it. A variable unified with another
        // is spelled by whichever of the two the store kept, and a lookup that
        // asked for the other would find nothing and print the number.
        let named = self.settled_names(&Named {
            labels: names.to_vec(),
        });
        Some((
            crate::ui::in_labels(&sat::project(&allowed, &atoms), &named.labels),
            crate::ui::in_labels(&needed, &named.labels),
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
    /// definition generalizes to `{x: 'a} -> {}` with no variables and no
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
            self.vars[var as usize] = Slot::Bound(Assigned::Presence(settled));
        }
    }

    /// Every presence variable a type still mentions, in the order it mentions
    /// them — which is the order the printed alphabet follows.
    ///
    /// A set, because a type may mention one variable twice — two labels
    /// sharing a presence is the whole of what a `when` name buys — and the
    /// order is the first mention's.
    fn presences_in(&self, ty: &Rc<Ty>, found: &mut IndexSet<TyVar>) {
        let ty = self.resolve(ty);
        for field in ty.fields.values() {
            if let Presence::Var(var) = self.presence_of(&field.presence) {
                found.insert(var);
            }
            let held = field.ty.clone();
            self.presences_in(&held, found);
        }
        match &ty.core {
            Core::Arrow(from, to, effects) => {
                let (from, to, effects) = (from.clone(), to.clone(), effects.clone());
                self.presences_in(&from, found);
                self.presences_in(&to, found);
                // An effect's presence is quantified like any other — a
                // `` `Log (when a) `` on an arrow is a presence the scheme
                // names — so one missed here would vanish from the scheme
                // instead of being reported.
                self.presences_in_row(&effects, found);
            }
            Core::Sum(cases) => {
                let cases = cases.clone();
                self.presences_in_row(&cases, found);
            }
            Core::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.presences_in(arg, found);
                }
            }
            Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// [`presences_in`](Self::presences_in) about a row: a sum's cases, or an
    /// arrow's effects. Flattened first, so a presence a tail already spliced
    /// in is found where it stands.
    fn presences_in_row(&self, row: &Row, found: &mut IndexSet<TyVar>) {
        for field in self.canon(row).labels.values() {
            if let Presence::Var(var) = self.presence_of(&field.presence) {
                found.insert(var);
            }
            let held = field.ty.clone();
            self.presences_in(&held, found);
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
    fn instantiate(&mut self, span: Span, scheme: &Scheme) -> Rc<Ty> {
        // One fresh variable per position the scheme bound, handed over as a
        // bare type: which sort each one is, is decided where it lands, since
        // that is what a scheme records. See [`Assigned::as_row`]. A presence
        // is minted in its own alphabet, for the reason [`Scheme`] gives.
        let fresh: Vec<Assigned> = (0..scheme.count())
            .map(|_| Assigned::Ty(self.fresh_type()))
            .collect();
        let presences: Vec<Presence> = (0..scheme.presences())
            .map(|_| self.fresh_presence())
            .collect();
        let ty = scheme.body().open(&fresh, &presences);
        // A scheme's body says which of its rows a quantified tail is the tail
        // of, but the condition that follows from that is not part of the
        // body: it lived beside the variables the scheme closed over, and this
        // copy's variables are new. Said again, of them.
        self.note_lacks(&ty);
        // And so is what it requires of its presences. Per instance, never per
        // scheme: two uses of one definition with different field sets are both
        // legal exactly when each instance's formula is separately satisfiable.
        let formula = scheme.formula().open(&presences);
        if !formula.is_true() {
            let mut labels = IndexMap::new();
            self.labels_in(&ty, &mut labels);
            let labels = labels.into_iter().collect();
            self.require(span, Origin::Instance(Named { labels }), formula);
        }
        ty
    }

    /// Every label a type names and what decides whether it is there, in the
    /// order the type names them — what a use-site complaint quotes its formula
    /// in. The first spelling of a label wins, which is the one a reader
    /// reading the type left to right meets.
    fn labels_in(&self, ty: &Rc<Ty>, found: &mut IndexMap<String, Presence>) {
        let ty = self.resolve(ty);
        for (name, field) in &ty.fields {
            found
                .entry(name.clone())
                .or_insert_with(|| self.presence_of(&field.presence));
            let held = field.ty.clone();
            self.labels_in(&held, found);
        }
        match &ty.core {
            Core::Arrow(from, to, effects) => {
                let (from, to, effects) = (from.clone(), to.clone(), effects.clone());
                self.labels_in(&from, found);
                self.labels_in(&to, found);
                for (name, field) in &self.canon(&effects).labels {
                    found
                        .entry(name.clone())
                        .or_insert_with(|| self.presence_of(&field.presence));
                }
            }
            Core::Sum(cases) => {
                for (name, field) in &self.canon(cases).labels {
                    found
                        .entry(name.clone())
                        .or_insert_with(|| self.presence_of(&field.presence));
                    let held = field.ty.clone();
                    self.labels_in(&held, found);
                }
            }
            Core::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.labels_in(arg, found);
                }
            }
            Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// [`unfold`] with the row conditions its result implies recorded against
    /// this table's variables.
    ///
    /// A declaration's body says which of its rows a `..` parameter is the tail
    /// of, but the condition that follows — `type WithX r = { x: Nat, ..r }`
    /// says `r` has no `x` — is not part of the body. It lived beside variables
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
        let unfolded = unfold(aliases, ty);
        if !Rc::ptr_eq(&unfolded, ty) {
            self.note_lacks(&unfolded);
        }
        unfolded
    }

    /// Whether the variables an annotation left open came back narrower than it
    /// promised — either because the definition decided one of them, or because
    /// it made two of them into one.
    ///
    /// Asked of the whole range at once rather than a variable at a time, and
    /// that is the point. Openness is not a property a variable has by itself:
    /// a `when` that came back as somebody else's still-open presence is a
    /// `when` the reader's type keeps, because the scheme quantifies whatever
    /// is left free and prints it under the name the annotation gave it. Two
    /// `when`s that came back as the *same* still-open presence are one `when`,
    /// however open it is, and the type the reader wrote is not the type the
    /// definition has. Only a range can tell those apart.
    ///
    /// Which is what a binding group turns on. Two definitions that name each
    /// other are solved together and monomorphically, so an annotated member's
    /// presences are unified with the variables standing in for its partner's
    /// almost immediately — and read one at a time that is indistinguishable
    /// from a body pinning them down, so every mutually recursive annotated
    /// definition was told to rewrite a type it had in fact kept.
    ///
    /// See [`Table::residue`] for what one variable coming back is.
    fn narrowed(&self, opened: Range<TyVar>) -> bool {
        let mut open: Vec<TyVar> = Vec::new();
        for var in opened {
            match self.residue(var) {
                Residue::Decided => return true,
                Residue::Nothing => {}
                Residue::Open(var) => match open.contains(&var) {
                    true => return true,
                    false => open.push(var),
                },
            }
        }
        false
    }

    /// What one variable an annotation left open came back as.
    ///
    /// A written type can leave open only `..`, `..r` and `when`, so a variable
    /// arriving here is one of four things: the core a struct's `..` lowered
    /// to, the tail a sum's did, the tail an *arrow's effect row* did, or a
    /// presence. Each sort is read in its own two spellings of "still nothing
    /// but a variable" and "nothing is known", which is the whole of the three
    /// arms — an effect tail needs no fourth, being a row like a sum's: one
    /// with labels is [`Residue::Decided`] and a bare [`Rest::Var`] is
    /// [`Residue::Open`], which is exactly what `let f : () -> Nat ! ..e = fn
    /// _ => greet ()` and `let f : Nat -> Nat ! ..e = fn x => x` want said of
    /// them.
    fn residue(&self, var: TyVar) -> Residue {
        let Slot::Bound(value) = &self.vars[var as usize] else {
            return Residue::Open(var);
        };
        match value {
            Assigned::Presence(presence) => match self.presence_of(presence) {
                Presence::Undecided => Residue::Nothing,
                Presence::Var(var) => Residue::Open(var),
                _ => Residue::Decided,
            },
            Assigned::Row(row) => match self.canon(row) {
                flat if !flat.labels.is_empty() => Residue::Decided,
                flat => match flat.rest {
                    Rest::Undecided => Residue::Nothing,
                    Rest::Var(var) => Residue::Open(var),
                    _ => Residue::Decided,
                },
            },
            Assigned::Ty(ty) => match self.resolve(ty) {
                ty if !ty.fields.is_empty() => Residue::Decided,
                ty => match ty.core {
                    Core::Undecided => Residue::Nothing,
                    Core::Var(var) => Residue::Open(var),
                    _ => Residue::Decided,
                },
            },
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
    /// condition, since a core variable's labels are the fields of whatever it
    /// is being bound to and there is no row here to read a shape from.
    fn lacked(&self, var: TyVar, value: &Assigned) -> Option<(Shape, Vec<(String, Presence)>)> {
        let (shape, lacks) = self.lacks.get(&var)?;
        // Whatever labels the value would bring with it, read at the shape the
        // condition was recorded in: the fields of a whole type, or the cases a
        // sum's rest stands for. A presence brings neither, which
        // [`Assigned::as_row`] says by answering with a row that names nothing
        // and [`Assigned::as_ty`] by answering with a type that carries none.
        let labels: IndexMap<String, RowField> = match shape {
            Shape::Struct => self.resolve(&value.as_ty()).fields.clone(),
            Shape::Sum | Shape::Effect => self.canon(&value.as_row()).labels,
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
    /// Without this a condition would survive exactly one binding: `..r`
    /// absorbing a `y` continues as a fresh tail, and that tail is where the
    /// next field to conflict would arrive.
    fn inherit_lacks(&mut self, var: TyVar, value: &Assigned) {
        self.note_lacks_value(value, self.lacks_shape(var));
        let Some((shape, labels)) = self.lacks.get(&var).cloned() else {
            return;
        };
        match shape {
            // A struct's condition travels through the core alone, there being
            // no tail left to put it on: the type a core variable stands for is
            // a type those labels are already on, so a copy of one arriving
            // there would name the field twice.
            Shape::Struct => {
                if let Core::Var(core) = self.resolve(&value.as_ty()).core {
                    self.forbidden(core, shape, labels);
                }
            }
            Shape::Sum | Shape::Effect => {
                let row = value.as_row();
                self.forbid(&row, shape, &labels);
            }
        }
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
    /// `fn x => x` read `'a -> 'a` rather than `'a -> 'a ! ..'b`, and one that
    /// genuinely does link two arrows occurs twice and is quantified.
    ///
    /// Only variables inference minted. `opened` is the range an annotation's
    /// lowering took, which is the range [`narrowed`](Self::narrowed) is asked
    /// about: a `..e` inside one is the reader's, and closing it would publish
    /// a scheme narrower than the annotation *and* slip past the too-open check
    /// — `residue` reports a still-open variable as [`Residue::Open`], so
    /// nothing would complain. An annotation is the contract.
    ///
    /// And only what this scheme would quantify: a variable below `level`
    /// belongs to a binder further out, which may still put an effect in it.
    fn close_effects(&mut self, ty: &Rc<Ty>, level: u32, opened: &Range<TyVar>) {
        // What the annotation's variables have *come to*, not what it minted:
        // two definitions that name each other are solved monomorphically, so
        // one member's tail is unified with the other's almost immediately, and
        // the variable left standing may be either. Followed through
        // [`residue`](Self::residue), which is the reading
        // [`narrowed`](Self::narrowed) already takes of the same range.
        let promised: Vec<TyVar> = opened
            .clone()
            .filter_map(|var| match self.residue(var) {
                Residue::Open(var) => Some(var),
                Residue::Decided | Residue::Nothing => None,
            })
            .collect();
        let mut counted: IndexMap<TyVar, usize> = IndexMap::new();
        self.count_effects(ty, &mut counted);
        for (var, count) in counted {
            if count != 1 || promised.contains(&var) || self.levels[var as usize] < level {
                continue;
            }
            self.vars[var as usize] = Slot::Bound(Assigned::Row(Rc::new(Row::closed())));
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
        let ty = self.resolve(ty);
        for field in ty.fields.values() {
            let held = field.ty.clone();
            self.count_effects(&held, found);
        }
        match &ty.core {
            Core::Arrow(from, to, effects) => {
                let (from, to) = (from.clone(), to.clone());
                self.count_effects(&from, found);
                self.count_effects(&to, found);
                let row = self.canon(effects);
                if let Rest::Var(var) = row.rest {
                    *found.entry(var).or_default() += 1;
                }
                for field in row.labels.values() {
                    let held = field.ty.clone();
                    self.count_effects(&held, found);
                }
            }
            Core::Sum(cases) => {
                for field in self.canon(cases).labels.values() {
                    let held = field.ty.clone();
                    self.count_effects(&held, found);
                }
            }
            Core::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.count_effects(arg, found);
                }
            }
            Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// Quantify everything in `ty` still unsolved at or above `level`.
    /// Returns the scheme and the substitution that built it, so the caller can
    /// spell the same variables the same way elsewhere.
    ///
    /// A variable below the level is left where it stands, as a [`Core::Var`]
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
        (
            Scheme::constrained(
                subst.types.len() as u32,
                subst.presences.len() as u32,
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
        if self.unsat {
            return Formula::True;
        }
        let mut presences = IndexSet::new();
        self.presences_in(ty, &mut presences);
        let atoms: Vec<Atom> = presences.into_iter().map(Atom::Var).collect();
        sat::project(&self.component(&atoms), &atoms)
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
    /// numbered on past its own quantifiers, so that a reader is shown letters
    /// rather than the solver's `?3`.
    ///
    /// Only for [`Output::locals`], and only once the solve that could still
    /// bind those variables is over. The scheme the solver instantiates is the
    /// one [`generalize`](Self::generalize) produced, free variables and all;
    /// numbering them would hand each use a copy of a variable it is meant to
    /// share.
    fn published(&self, scheme: &Scheme) -> Scheme {
        let mut subst = Subst::default();
        self.quantify(scheme.body(), &mut subst, 0);
        // Its own quantifiers already occupy the first positions of each
        // alphabet, so what was free numbers on from there — in its own
        // alphabet, since the two do not share a numbering.
        let shifted = Subst {
            types: subst
                .types
                .into_iter()
                .map(|(var, at)| (var, scheme.count() + at))
                .collect(),
            presences: subst
                .presences
                .into_iter()
                .map(|(var, at)| (var, scheme.presences() + at))
                .collect(),
        };
        let count = scheme.count() + shifted.types.len() as u32;
        let presences = scheme.presences() + shifted.presences.len() as u32;
        Scheme::constrained(
            count,
            presences,
            self.zonk(scheme.body(), &shifted),
            quantify_formula(scheme.formula(), &shifted),
        )
    }

    /// Number the generalizable variables of `ty` in first-occurrence order —
    /// which is what makes the leftmost variable print as `'a`.
    ///
    /// Presence variables are numbered after everything else, in a second
    /// walk, because they have an alphabet of their own: a presence prints as
    /// the bare `a` its `when` clause names it, a type as `'a`, and the two
    /// pools are independent. Numbering them in line would leave a visible gap
    /// in each — the scheme of `{ x when a: Nat } -> 'a` should call neither
    /// of its letters `b`.
    fn quantify(&self, ty: &Rc<Ty>, subst: &mut Subst, level: u32) {
        self.quantify_walk(ty, subst, level, false);
        self.quantify_walk(ty, subst, level, true);
    }

    /// One numbering pass over one type: over everything but the presence
    /// slots, or — when `presences` — over only them. A variable can only be one
    /// or the other, so the two passes cannot number one twice. Either way the
    /// descent is the same: the fields first and then the core they sit on,
    /// which is what makes `{ x: 'a, ..'b }` number left to right.
    ///
    /// Fields first because the core is what the `..` prints, and a tail is read
    /// last. The rule used to be the other way round, when a core was one thing
    /// a type had and its tail was another; now the core *is* the tail, so
    /// numbering it first would call the rightmost thing on the line `'a`. No
    /// special case for it either way — it is descended into exactly where it
    /// sits.
    fn quantify_walk(&self, ty: &Rc<Ty>, subst: &mut Subst, level: u32, presences: bool) {
        let ty = self.resolve(ty);
        self.quantify_labels(&ty.fields, subst, level, presences);
        match &ty.core {
            Core::Var(var) => {
                if !presences {
                    self.quantify_var(*var, subst, level);
                }
            }
            Core::Arrow(from, to, effects) => {
                self.quantify_walk(from, subst, level, presences);
                self.quantify_walk(to, subst, level, presences);
                // The effect row is quantified with everything else, and an
                // effect variable missed here would vanish from the scheme
                // rather than be reported.
                self.quantify_row(effects, subst, level, presences);
            }
            Core::Sum(cases) => self.quantify_row(cases, subst, level, presences),
            // An argument left open is the definition's to quantify, the same
            // as one written anywhere else: `WithX ..'a -> Nat` names its tail
            // because this descends.
            Core::Named { args, .. } => {
                for arg in args.iter() {
                    self.quantify_walk(arg, subst, level, presences);
                }
            }
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// [`quantify_walk`](Self::quantify_walk) over a sum's cases: the labels,
    /// and then the tail, which is read last for the reason a struct's core is.
    fn quantify_row(&self, row: &Row, subst: &mut Subst, level: u32, presences: bool) {
        let row = self.canon(row);
        self.quantify_labels(&row.labels, subst, level, presences);
        if !presences && let Rest::Var(var) = row.rest {
            self.quantify_var(var, subst, level);
        }
    }

    /// [`quantify_walk`](Self::quantify_walk) over a label map: each label's
    /// presence, on the pass that numbers those, and what it holds.
    fn quantify_labels(
        &self,
        labels: &IndexMap<String, RowField>,
        subst: &mut Subst,
        level: u32,
        presences: bool,
    ) {
        for field in labels.values() {
            if presences && let Presence::Var(var) = self.presence_of(&field.presence) {
                self.quantify_presence(var, subst, level);
            }
            self.quantify_walk(&field.ty, subst, level, presences);
        }
    }

    /// Number one variable, unless it already has a number or belongs to a
    /// binder further out. See [`Table::levels`].
    fn quantify_var(&self, var: TyVar, subst: &mut Subst, level: u32) {
        if self.levels[var as usize] < level {
            return;
        }
        let next = subst.types.len() as u32;
        subst.types.entry(var).or_insert(next);
    }

    /// [`quantify_var`](Self::quantify_var) in the other alphabet: a presence
    /// is numbered `a`, `b`, … among presences alone.
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
        let ty = self.resolve(ty);
        let fields = self.zonk_labels(&ty.fields, subst);
        let core = match &ty.core {
            // A variable with no number is one quantifying was asked to leave
            // alone: it belongs to a binder further out, and standing for
            // itself is exactly what a scheme of a nested binding has to say
            // about it.
            Core::Var(var) => match subst.types.get(var) {
                Some(at) => Core::Bound(*at),
                None => ty.core.clone(),
            },
            Core::Arrow(from, to, effects) => Core::Arrow(
                self.zonk(from, subst),
                self.zonk(to, subst),
                self.zonk_row(effects, subst),
            ),
            Core::Sum(cases) => Core::Sum(self.zonk_row(cases, subst)),
            // Rebuilt rather than handed back, because an argument may hold a
            // variable and what leaves here may not. Nothing downstream
            // resolves one, so a `Core::Var` that survived this would reach a
            // reader as an unanswerable `?3`.
            Core::Named { symbol, name, args } => Core::Named {
                symbol: *symbol,
                name: name.clone(),
                args: args.iter().map(|arg| self.zonk(arg, subst)).collect(),
            },
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => ty.core.clone(),
        };
        Rc::new(Ty { core, fields })
    }

    /// [`zonk`](Self::zonk) over a sum's cases.
    ///
    /// A tail the solve bound to a row is spliced in, so that what outlives the
    /// solver is one flat row rather than a chain of them:
    /// `` `A | ..(`B) `` is not a type anyone wrote. [`canon`](Self::canon) is what does
    /// that, and it is lossless: a tail stands for the labels its row does not
    /// write out, and [`Solve::assign`] refuses to bind one to a row that
    /// certainly has one of them, so the one copy a chain can repeat by the
    /// time a solve is over is a label settled absent — not part of what the
    /// row says, so dropping it for the outer label loses nothing. Before the
    /// lacks check existed the splice was where a repeated *present* field
    /// quietly lost a copy, and a definition came out with a type it had never
    /// been shown to have.
    fn zonk_row(&self, row: &Row, subst: &Subst) -> Row {
        let row = self.canon(row);
        let labels = self.zonk_labels(&row.labels, subst);
        let rest = match row.rest {
            // Left standing where it has no number, for the reason a core
            // variable is; see [`Table::zonk`].
            Rest::Var(var) => match subst.types.get(&var) {
                Some(at) => Rest::Bound(*at),
                None => Rest::Var(var),
            },
            decided => decided,
        };
        Row { labels, rest }
    }

    /// [`zonk`](Self::zonk) over a label map: each presence resolved to what it
    /// came to, and each label's type zonked in turn.
    fn zonk_labels(
        &self,
        labels: &IndexMap<String, RowField>,
        subst: &Subst,
    ) -> IndexMap<String, RowField> {
        labels
            .iter()
            .map(|(name, field)| {
                let field = RowField {
                    presence: match self.presence_of(&field.presence) {
                        Presence::Var(var) => match subst.presences.get(&var) {
                            Some(at) => Presence::Bound(*at),
                            None => Presence::Var(var),
                        },
                        decided => decided,
                    },
                    ty: self.zonk(&field.ty, subst),
                };
                (name.clone(), field)
            })
            .collect()
    }

    /// [`zonk`](Self::zonk) applied to every type the walk wrote into a
    /// definition's body, once the definition is solved.
    ///
    /// The substitution grows as the walk goes, because the body is a larger
    /// domain than the definition's own type: in `let a = k 1 (fn z => z)` the
    /// argument is typed `?5 -> ?5`, which `a : Nat` never mentions and
    /// generalization therefore never numbered. A variable like that is
    /// unconstrained rather than unknown, so it is quantified here and
    /// numbered on from the scheme's — which leaves no [`Core::Var`] anywhere in
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
    /// it spells `'a`.
    fn zonk_error(&self, kind: &ErrorKind, subst: &mut Subst) -> ErrorKind {
        match kind {
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
            ErrorKind::AnnotationTooOpen => ErrorKind::AnnotationTooOpen,
            // The presence complaints carry prose rather than types:
            // their formulas were already worded, at the moment the variables
            // in them still had labels to be named by. There is nothing here
            // for a later substitution to improve.
            ErrorKind::PresenceRequired { formula } => ErrorKind::PresenceRequired {
                formula: formula.clone(),
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
        }
    }
}

/// One formula with each variable a scheme quantified replaced by the position
/// it was given — [`Table::zonk`] about a formula rather than about a type.
///
/// A variable with no number is left standing, for the same reason a
/// [`Core::Var`] is: it belongs to a binder further out, and a conjunct linking
/// a quantified presence to an outer one moves into the scheme with the outer
/// one kept free. That is standard HM(X), and the level machinery is what
/// decided the entitlement.
fn quantify_formula(formula: &Formula, subst: &Subst) -> Formula {
    formula.rename(&|var| match subst.presences.get(&var) {
        Some(at) => Formula::bound(*at),
        None => Formula::var(var),
    })
}

/// The semantic type a written type denotes. A declared type stays the name it
/// was written as — `Endo` stays `Endo`, and what it stands for is looked up
/// where a shape is actually needed — and a type that failed to lower becomes
/// [`Core::Undecided`], which absorbs rather than cascades.
///
/// Keeping the name is what makes a recursive declaration lowerable at all: a
/// body that named its own meaning would have to contain it, and nothing
/// finite does. It is also what a reader gets told, since a type prints as
/// itself; the mint is here only to spell it, never to decide anything.
///
/// The table is here for the parts of an annotation that stand for something
/// the definition gets to decide: a `..` tail, a `..r` tail, and a `when` field
/// each lower to a fresh variable, minted at the level the annotation is
/// lowered at so that what stays unconstrained is quantified with the
/// definition. One call is one annotation, which is the whole scope of a
/// tail's name: every `..r` in it shares one variable, and no other
/// annotation's `r` can reach it.
///
/// A `type` declaration's body reaches none of that. A `when` and a bare `..` are
/// refused there outright, and the one tail it may have names a row parameter,
/// which lowers to a [`Core::Bound`] — a leaf whose value comes from the use
/// site, not a variable this table has to solve. So a declaration still lowers
/// to something with no [`Core::Var`] anywhere in it, which several walks here
/// rely on: it is why they may stop at a name rather than descend into what it
/// stands for.
fn lower_type(mint: &Mint, table: &mut Table, ty: &Type) -> Rc<Ty> {
    let mut tails = Tails::default();
    let lowered = lower(mint, table, &mut tails, ty);
    // A tail stands for the fields its row did not write out, and this is
    // where that is first true of a written one: `{ x: Nat, ..r }` says `r`
    // has no `x`. See [`Table::lacks`].
    table.note_lacks(&lowered);
    lowered
}

/// Every `..r` an annotation has written so far, by name, in the two sorts a
/// named tail can be: the core a struct's `..` stands for, and the rest a sum's
/// does. Two maps rather than one because the two are two sorts of value —
/// lowering has already refused a name used at both, so no name is ever in both.
/// See [`ir::ErrorKind::MixedTail`](crate::ir::ErrorKind::MixedTail).
#[derive(Default)]
struct Tails {
    cores: HashMap<String, Core>,
    rows: HashMap<String, Rest>,
    /// Every `when a` the annotation has written so far, by name. The same
    /// scope and the same rule: two labels wearing one name share one presence
    /// variable, which is how a type says two fields are there together, and
    /// what the `where` clause beside them resolves against.
    presences: HashMap<String, Presence>,
}

/// The semantic type and formula a written annotation denotes: [`lower_type`]
/// with the `where` clause the type's `when`s bind the names of.
///
/// The clause is read after the type, whatever order a reader's eye takes them
/// in, because there is nothing to resolve a name against until the labels that
/// bind it are lowered.
fn lower_annotation(
    mint: &Mint,
    table: &mut Table,
    annotation: &Annotation,
) -> (Rc<Ty>, Formula, Vec<(String, Presence)>) {
    let mut tails = Tails::default();
    let lowered = lower(mint, table, &mut tails, &annotation.ty);
    table.note_lacks(&lowered);
    let formula = match &annotation.clause {
        Some(clause) => clause_formula(&tails, clause),
        None => Formula::True,
    };
    // In one fixed order, because they come out of a hash map and the order it
    // hands them over is nobody's: a complaint about the clause names its
    // presences the same way on every run, which alphabetical is enough for.
    let mut names: Vec<(String, Presence)> = tails.presences.into_iter().collect();
    names.sort_by(|(one, _), (other, _)| one.cmp(other));
    (lowered, formula, names)
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
    let core = match &ty.tracked {
        TypeKind::Prim(prim) => (*prim).into(),
        TypeKind::Ident(symbol) => Core::Named {
            symbol: *symbol,
            name: mint.name(*symbol).into(),
            // Lowering counted the arguments, so a name that reaches here bare
            // is one that takes none.
            args: Rc::from([]),
        },
        TypeKind::Apply { head, args, .. } => Core::Named {
            symbol: *head,
            name: mint.name(*head).into(),
            args: args
                .iter()
                .map(|arg| lower(mint, table, tails, arg))
                .collect(),
        },
        // A parameter is the position it was declared at, which is what
        // unfolding hands an argument to. See [`Core::Bound`].
        TypeKind::Param { index, .. } => Core::Bound(*index),
        TypeKind::Arrow { from, to, effects } => Core::Arrow(
            lower(mint, table, tails, from),
            lower(mint, table, tails, to),
            effect_row(table, tails, effects),
        ),
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
                        ty: lower(mint, table, tails, value),
                    },
                    ir::TypeField::Absent { .. } => RowField {
                        presence: Presence::Absent,
                        ty: Rc::new(Ty::default()),
                    },
                };
                labels.insert(name.clone(), lowered);
            }
            return Rc::new(Ty {
                core: core_tail(table, tails, tail),
                fields: labels,
            });
        }
        // The struct arm again, about cases — except that a sum's cases are its
        // core and it carries no fields of its own. The one other difference is
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
                            Some(payload) => lower(mint, table, tails, payload),
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
            Core::Sum(row(table, tails, labels, tail))
        }
        // A hole the solver fills: a fresh variable, so whatever the position
        // meets decides it. The pattern desugar's field types are these — the
        // pattern says which fields are there, not what they hold — and the
        // variable is what lets a projection of the field and the demand share
        // one answer. See [`ir::TypeKind::Any`](crate::ir::TypeKind).
        TypeKind::Any => Core::Var(table.fresh_core()),
        TypeKind::Error => Core::Undecided,
    };
    Rc::new(Ty::plain(core))
}

/// Whether an annotation is the compiler's own demand rather than the reader's
/// promise: whether it holds a hole, which nothing a reader writes can.
///
/// What it exempts is the too-open check. A written `..` or `when` is a promise —
/// the definition works whatever the open part turns out to be — and a
/// definition that decides it has broken the promise. A hole is the opposite:
/// it is *there to be decided*, so holding the annotation to the promise would
/// report every exact pattern binding as a broken contract nobody wrote.
pub(crate) fn synthetic(ty: &Type) -> bool {
    match &ty.tracked {
        TypeKind::Any => true,
        // The desugar writes a hole only as a field's type, directly under
        // the struct that is the demand — see [`ir::TypeKind::Any`] — so the
        // walk needs no further reach: everything else a written type can be
        // is a written type.
        TypeKind::Struct { fields, .. } => fields
            .values()
            .any(|field| field.value().is_some_and(synthetic)),
        _ => false,
    }
}

/// Whether one label is there, as its `when` clause says it: no clause means it
/// is simply there, a named one is the variable everything of that name in this
/// annotation shares, and `when _` is a variable of its own that nothing can
/// name.
fn presence(table: &mut Table, tails: &mut Tails, when: &Option<Box<ir::When>>) -> Presence {
    let Some(when) = when else {
        return Presence::Present;
    };
    let Some(name) = &when.name else {
        return table.fresh_presence();
    };
    tails
        .presences
        .entry(name.clone())
        .or_insert_with(|| table.fresh_presence())
        .clone()
}

/// The cases of a written sum and what its `..` stands for, assembled into the
/// row. `..` is a variable this definition may decide, `..r` is that variable
/// shared across one annotation, and `..r` naming a parameter is a position an
/// argument is handed to.
fn row(
    table: &mut Table,
    tails: &mut Tails,
    labels: IndexMap<String, RowField>,
    tail: &Option<Tail>,
) -> Row {
    let rest = match tail.as_ref().map(|tail| &tail.of) {
        None => Rest::Closed,
        Some(ir::Row::Anything) => table.fresh_row(),
        Some(ir::Row::Named(name)) => tails
            .rows
            .entry(name.clone())
            .or_insert_with(|| table.fresh_row())
            .clone(),
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
/// may decide; `..e` is that variable shared across one annotation; and `..e`
/// naming a parameter is the position an argument is handed to.
///
/// The variables it mints land in the same range the annotation records, which
/// is what [`Table::narrowed`] is asked about: an effect tail a reader left open
/// is the reader's, and R23's closing rule may not touch one.
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
        labels.insert(name.clone(), lowered);
    }
    row(table, tails, labels, &effects.tail)
}

/// What a written struct's `..` stands for, which is the type its fields sit on.
///
/// [`row`] about the other shape, and shorter for it: a struct's rest is a whole
/// type, so each of the four tails lowers to the core that already means it. No
/// tail at all is [`Core::Unit`] — the type with nothing of its own, which
/// allows no field it does not name. A bare `..` is a variable this definition
/// may decide, `..r` is that variable shared across one annotation, and `..r`
/// naming a parameter is a [`Core::Bound`], the position an argument is handed
/// to — which is what keeps a declaration's body free of solver variables.
fn core_tail(table: &mut Table, tails: &mut Tails, tail: &Option<Tail>) -> Core {
    match tail.as_ref().map(|tail| &tail.of) {
        None => Core::Unit,
        Some(ir::Row::Anything) => Core::Var(table.fresh_core()),
        Some(ir::Row::Named(name)) => tails
            .cores
            .entry(name.clone())
            .or_insert_with(|| Core::Var(table.fresh_core()))
            .clone(),
        Some(ir::Row::Param { index, .. }) => Core::Bound(*index),
    }
}

/// What a declared type stands for: [`Core::Named`] replaced by the body it was
/// declared with, holding the arguments it was applied to, and again for as
/// long as that is another name.
///
/// Substituting the arguments is opening the declaration's [`Scheme`], which is
/// the same `open` that instantiates a definition's — a declaration's
/// parameters and a scheme's quantified variables are both [`Core::Bound`], and
/// both are handed their values from outside. One taking no arguments opens to
/// its body unchanged, which is what this did before there were any.
///
/// The one place a name is looked through, and always by one caller that needs
/// a shape rather than a name — never as a normalization pass. A type that
/// names itself unfolds forever if asked to, so nothing here asks: what comes
/// back is one shape deep, and the names inside it are still names.
///
/// What guarantees this terminates is the check in
/// [`ir::build`](crate::ir::build), not anything here. Each round is one of
/// two things and both run out: following a name to the name at the head of
/// its body walks a chain of declarations that lowering refuses to let close a
/// loop, and following one that stands for its own argument hands back a
/// strictly smaller piece of the type that came in. The same bargain
/// [`Table::resolve`] has with the occurs check.
///
/// A name with no declaration behind it is [`Core::Undecided`]: the only way to
/// write one is to repeat a type's name, which was already reported.
pub fn unfold(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
    let mut ty = ty.clone();
    // The budget is the one [`Table::resolve`] keeps, for the reason it keeps
    // it: it bounds what a bug in that check would cost rather than restating
    // the guarantee.
    //
    // What it bounds is a chain of *name bodies*. A step whose body is another
    // name applied to something follows one declaration to the one declaration
    // written at the head of its body, and a body has one head that `open`
    // never renames — so the successor is a function of the declaration alone,
    // and more than one such step per declaration means a declaration was
    // visited twice, which is the loop lowering refuses.
    //
    // A step whose body is a parameter hands back an argument instead. What is
    // left to unfold is then a piece of the type that came in rather than
    // anything this chain reached, so it starts a new chain and the budget
    // resets. Counting those against one bound would panic on
    // `type Id a = a` written `Id (Id Nat)`, which is a correct program.
    let mut budget = aliases.len();
    while let Core::Named { symbol, args, .. } = &ty.clone().core {
        // Indexed rather than looked up: a name that repeats a declaration
        // binds nothing, so a name lowering wrote into a type is one this table
        // has.
        let scheme = &aliases[symbol];
        budget = match &scheme.body().core {
            Core::Bound(_) => aliases.len(),
            _ => budget
                .checked_sub(1)
                .expect("a chain of declarations closed a loop that lowering should refuse"),
        };
        let fresh: Vec<Assigned> = args.iter().map(|arg| Assigned::Ty(arg.clone())).collect();
        // The name's own fields ride along: unfolding decides what the core
        // stands for and says nothing about the fields written outside it.
        // A declaration binds no presence — lowering refuses a `when` in one —
        // so there is no presence list for unfolding to supply.
        ty = splice(&ty.fields, &scheme.body().open(&fresh, &[]));
    }
    ty
}

/// A type whose core has been replaced by what it stands for: the inner type's
/// core, carrying both sets of labels, the outer ones winning.
///
/// The one splice a struct has, and the whole of what makes a core stand for a
/// type with fields of its own. `{ x: Nat, ..?3 }` with `?3` known to be
/// `{ y: Nat }` is `{ x: Nat, y: Nat }`; `WithX Nat` unfolded is a `Nat`
/// carrying an `x`. Both are this.
///
/// The outer labels win because they are the ones written where the whole type
/// is read. A label *certainly there* on both sides would be a type naming a
/// field twice, and there is no way left to reach one: lowering refuses an
/// argument that repeats a label the declaration it goes to already names, and
/// [`Solve::assign`]'s lacks check refuses every route through a variable that
/// would bring the label in present. What that check lets through is a copy
/// settled absent — not part of what the inner type says — so the one thing
/// the rule decides in practice is to drop it for the outer label, which is
/// the only reading, and it stays definite about what happens if either
/// refusal is ever weakened.
fn splice(outer: &IndexMap<String, RowField>, inner: &Rc<Ty>) -> Rc<Ty> {
    if outer.is_empty() {
        return inner.clone();
    }
    if inner.fields.is_empty() {
        return Rc::new(Ty {
            core: inner.core.clone(),
            fields: outer.clone(),
        });
    }
    let mut fields = outer.clone();
    for (name, field) in &inner.fields {
        fields.entry(name.clone()).or_insert_with(|| field.clone());
    }
    Rc::new(Ty {
        core: inner.core.clone(),
        fields,
    })
}

impl Ty {
    /// Replace each bound variable with what it was opened to.
    ///
    /// Two callers and one rule. Instantiating a definition's scheme hands each
    /// position a fresh variable; unfolding a declaration hands each position
    /// the argument written at the use site. Which sort a position needs is
    /// decided here rather than by the caller, because the position is what
    /// knows: see [`Assigned::as_row`].
    pub fn open(&self, fresh: &[Assigned], presences: &[Presence]) -> Rc<Ty> {
        let fields = open_labels(&self.fields, fresh, presences);
        let core = match &self.core {
            // Not a leaf: what the variable stands for may carry fields, and
            // the fields written outside it are kept over them. This is what a
            // struct's row parameter is — `type WithX r = { x: Nat, ..r }` puts
            // its parameter here — so opening one is the substitution every
            // other position already had.
            Core::Bound(index) => {
                return splice(&fields, &fresh[*index as usize].as_ty());
            }
            Core::Arrow(from, to, effects) => Core::Arrow(
                from.open(fresh, presences),
                to.open(fresh, presences),
                effects.open(fresh, presences),
            ),
            Core::Sum(cases) => Core::Sum(cases.open(fresh, presences)),
            // A declaration's own body reaches here holding its parameters, so
            // `type Wrap a = { inner: Pair a a }` depends on this arm entirely.
            Core::Named { symbol, name, args } => Core::Named {
                symbol: *symbol,
                name: name.clone(),
                args: args.iter().map(|arg| arg.open(fresh, presences)).collect(),
            },
            Core::Unit | Core::Nat | Core::Var(_) | Core::Undecided => self.core.clone(),
        };
        Rc::new(Ty { core, fields })
    }
}

impl Row {
    /// [`Ty::open`] over a sum's cases. A row parameter written at this tail
    /// stands for the cases whatever is handed to it allows, which is how an
    /// argument written as a whole type is read for the row it carries. See
    /// [`Assigned::as_row`].
    fn open(&self, fresh: &[Assigned], presences: &[Presence]) -> Row {
        let labels = open_labels(&self.labels, fresh, presences);
        let rest = match &self.rest {
            Rest::Bound(index) => Rest::More(Rc::new(fresh[*index as usize].as_row())),
            rest => rest.clone(),
        };
        Row { labels, rest }
    }
}

/// [`Ty::open`] over a label map: each label's presence and what it holds. The
/// map itself has no tail to open — a struct's is the core beside it, which
/// [`Ty::open`] handles where it sits.
fn open_labels(
    labels: &IndexMap<String, RowField>,
    fresh: &[Assigned],
    presences: &[Presence],
) -> IndexMap<String, RowField> {
    labels
        .iter()
        .map(|(name, field)| {
            let field = RowField {
                presence: field.presence.open(presences),
                ty: field.ty.open(fresh, presences),
            };
            (name.clone(), field)
        })
        .collect()
}

impl Presence {
    /// [`Ty::open`] over one presence, in the presence alphabet: a scheme
    /// numbers its `when` names apart from its `'a`s, so a bound presence
    /// indexes its own list.
    fn open(&self, presences: &[Presence]) -> Presence {
        match self {
            Presence::Bound(index) => presences[*index as usize].clone(),
            decided => decided.clone(),
        }
    }
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
/// still be one type — that is what the core beside them decides, in
/// [`Solve::labels`] — so failing here only means checking cannot push the
/// expected fields in
/// one by one and the literal is inferred and equated instead.
fn same_field_set<A, B>(want: &IndexMap<String, A>, have: &IndexMap<String, B>) -> bool {
    want.len() == have.len() && want.keys().all(|name| have.contains_key(name))
}
