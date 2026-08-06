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
//! Inference runs after lowering and mutates the [`Program`] it is handed:
//! every [`Term`]'s `ty` goes from [`Core::Undecided`] to what was inferred for
//! it, fully resolved, so nothing downstream ever needs the solver's variable
//! table to read a type. Errors do not stop either pass — a term that failed to
//! type still has a type, [`Core::Undecided`], which unifies with everything so
//! that one mistake is reported once rather than echoed by every consumer.

mod constrain;
mod solve;

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    ir::{self, Program, Tail, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{Assigned, Core, ParamKind, Presence, Rest, Row, RowField, Scheme, Shape, Ty, TyVar},
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
    pub errors: Vec<Error>,
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
    /// A variable standing for the rest of a row, against a row naming a field
    /// the first row already names. The lacks check fired, so — for the same
    /// reason [`Rule::Occurs`] is not a [`Rule::Bind`] — the refusal is a rule
    /// of its own rather than a binding that did not happen.
    ///
    /// The shape rides along for the wording, as it does on [`Rule::Presence`]:
    /// the rule is one rule, and a reader watching two sums be decided should
    /// still be read to in cases.
    Overlap { shape: Shape },
    /// Two identical primitives.
    Prim,
    /// Two arrows, taken apart into argument and result.
    Arrow,
    /// Two structs, taken apart: the fields both name against each other, and
    /// the fields only one names into what the other's tail allows.
    Struct,
    /// [`Rule::Struct`] about the other shape: two sums, the cases both name
    /// against each other and the cases only one names into what the other's
    /// tail allows.
    ///
    /// One rule in the code and two here, because a reader stepping through a
    /// solve is reading about their program: a line saying "two structs" over
    /// a goal about `` `Some `` and `` `None `` describes something they never
    /// wrote. See [`Solve::rows`], which is both.
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
    ExtraField {
        shape: Shape,
        base: Rc<Ty>,
        field: String,
    },
    /// An annotation left something open with `..` or `?` that the definition
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
    /// standing for anything with an `x` of its own would name the field
    /// twice — and the two copies could disagree. Only the label and what kind
    /// of row it was found in are carried: those are the two things both
    /// halves of the contradiction have in common, and the rows themselves are
    /// each half a type the reader never wrote down.
    RepeatedField { shape: Shape, field: String },
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
/// known to be, and what they may not stand for. Taken and put back by
/// [`Rule::Congruent`], which is the one rule that asks a question before it is
/// sure the question is the right one to have asked.
type Known = (Vec<Slot>, HashMap<TyVar, Lacks>);

/// What one name in scope means. Private for the same reason as [`Slot`]: a
/// binding exists only while a definition is being walked, and what survives
/// the walk is the [`Scheme`] in [`Output::schemes`].
#[derive(Debug, Clone)]
enum Binding {
    Mono(Rc<Ty>),
    Poly(Scheme),
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
    /// The variables the annotation minted, which are the ones it left for the
    /// definition to decide. Empty for a definition with no annotation. See
    /// [`Table::narrowed`].
    opened: Range<TyVar>,
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
    /// There is no generalization level beside it. A binding group is the only
    /// thing this language generalizes at and there is no `let` inside one, so
    /// every variable still unsolved when a group ends was minted by that group
    /// and is its members' to quantify. Rémy-style levels are what decides this
    /// where a definition can nest, and are what a nested `let` would have to
    /// bring back with it.
    ///
    /// A group rather than a definition, and the difference is only where the
    /// line falls: two definitions that name each other are solved together, so
    /// a variable one of them minted may still be open while the other is being
    /// walked. It is still nobody's but the group's, because the group is
    /// finished before anything outside it is looked at.
    vars: Vec<Slot>,
    /// What each variable may not stand for: the label names the rows it sits
    /// at the open end of already write out.
    ///
    /// `{ x: Nat, ..r }` reads "an `x`, and whatever else `r` is", so `r`
    /// standing for a row with an `x` of its own would give the type two
    /// fields of one name. Nothing in [`Row`] can express that side condition —
    /// a tail is an ordinary [`Rest::Var`] — so it is held here, beside the
    /// slots, and enforced at the one place a variable acquires a value.
    ///
    /// Core variables are under the same condition, for the same reason and
    /// with the same labels: `'c with { x: Nat }` reads "some type, which also
    /// has an `x`", so a `'c` with an `x` of its own would name the field
    /// twice. Their shape is always [`Shape::Struct`], since a core variable
    /// stands for a whole type and the labels forbidden of it are that type's
    /// fields.
    ///
    /// Insertion-ordered, so that a row breaking the rule twice always names
    /// the same field first and the complaint does not depend on a hash.
    lacks: HashMap<TyVar, Lacks>,
    /// What each declaration's parameters stand for, in the declaration's own
    /// order. Read only by [`Table::note_lacks`], and only for the one thing a
    /// [`ParamKind::Row`] carries that a type cannot: the labels the
    /// declaration already names at that tail, which whatever is written there
    /// may not name either.
    ///
    /// Held beside the variables rather than looked up through
    /// [`Solve::aliases`] because it is a fact about the *written* declaration
    /// and survives no unfolding: by the time a body has been opened, the tail
    /// its parameter sat in is an ordinary row and the condition has to have
    /// been said already.
    params: HashMap<Symbol, Vec<ParamKind>>,
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

    let mut schemes = IndexMap::new();
    let mut constraints = IndexMap::new();
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
                // `..r` and a `?` each lower to an ordinary variable, and a
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
                let annotated = program.terms[symbol]
                    .annotation
                    .as_ref()
                    .map(|annotation| lower_type(mint, &mut table, annotation));
                let opened = from..table.vars.len() as TyVar;
                let bound = annotated.unwrap_or_else(|| table.fresh_type());
                env.insert(*symbol, Binding::Mono(bound.clone()));
                Scoped {
                    symbol: *symbol,
                    bound,
                    opened,
                }
            })
            .collect();

        // One walk and one solve per member, in source order, over the shared
        // table — which decides the same things solving the union would, since
        // unification does not care what order it is asked in, and keeps a
        // [`Step`] able to name the definition it came from.
        let mut solved: Vec<Solved> = Vec::with_capacity(scoped.len());
        for scoped in scoped {
            let decl = &mut program.terms[&scoped.symbol];
            let mut constrain = Constrain {
                table: &mut table,
                env: &mut env,
                aliases: &aliases,
                out: Vec::new(),
            };
            // Checked against exactly what the rest of the group sees this
            // definition as. For an annotated one that is the annotation, as it
            // has always been; for the rest it is the variable standing in for
            // the definition, and checking against a bare variable is inferring
            // and equating — see [`Constrain::check_term`]. The equation is what
            // ties the name the body used to the type the body has.
            constrain.check_term(&mut decl.value, &scoped.bound);
            let generated = constrain.out;
            let ty = match &decl.annotation {
                Some(_) => scoped.bound.clone(),
                None => decl.value.ty.clone(),
            };
            let reported = errors.len();

            Solve {
                table: &mut table,
                errors: &mut errors,
                steps: &mut steps,
                aliases: &aliases,
                nominal: &nominal,
                definition: scoped.symbol,
                depth: 0,
                assumed: Vec::new(),
            }
            .run(&generated);

            solved.push(Solved {
                scoped,
                ty,
                generated,
                reported,
            });
        }

        // Where each member's complaints end, which is where the next one's
        // begin — and, for the last, where the group left the list.
        let mut bounds: Vec<usize> = solved.iter().map(|member| member.reported).collect();
        bounds.push(errors.len());

        // Generalization, once the whole group is solved and not before: a
        // member's type is not settled while another member of its own group
        // can still constrain it. Each is then quantified into a scheme of its
        // own. Two members can share a variable and each quantify it
        // separately, which loses the sharing — and there is no scope outside
        // for that to matter to, because this language has no `let` inside a
        // definition.
        for (at, member) in solved.into_iter().enumerate() {
            let symbol = member.scoped.symbol;
            let (from, to) = (bounds[at], bounds[at + 1]);
            let decl = &mut program.terms[&symbol];

            // One complaint per definition, not per variable: an annotation
            // that is too open is one thing to rewrite however many of its
            // `..`s and `?`s the body pinned down, and the reader is being sent
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
                && from == to
                && member.scoped.opened.clone().any(|var| table.narrowed(var))
            {
                errors.push(Error {
                    span: annotation.span,
                    kind: ErrorKind::AnnotationTooOpen,
                });
            }

            let (scheme, mut subst) = table.generalize(&member.ty);
            // With the substitution in hand, resolve every type the walk wrote
            // into the body, so a term's type and its definition's scheme spell
            // the same variable the same way.
            table.zonk_term(&mut decl.value, &mut subst);
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

    // Constraints are solved in the order the walk emitted them, which is not
    // quite the order anyone reads a file in — a body's demands come before
    // the annotation's on its result. Sorting by position puts that back; the
    // sort is stable, so two complaints about one span keep the order the
    // solver found them in.
    errors.sort_by_key(|error| error.span.start);

    Output {
        aliases,
        schemes,
        constraints,
        steps,
        errors,
    }
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
        (self.vars.clone(), self.lacks.clone())
    }

    /// Put back what [`snapshot`](Self::snapshot) took.
    ///
    /// The variables minted since go with it. Nothing can still be pointing at
    /// one: a fresh variable reaches the rest of the solve only by being bound
    /// into something, and every binding made since is being undone here too.
    fn restore(&mut self, (vars, lacks): Known) {
        self.vars = vars;
        self.lacks = lacks;
    }

    /// One more variable, of no sort yet. A variable's sort is fixed by the
    /// position it was minted for, and the three functions below are those
    /// positions; nothing else may call this.
    fn mint(&mut self) -> TyVar {
        let var = self.vars.len() as TyVar;
        self.vars.push(Slot::Unbound);
        var
    }

    /// A variable standing for a whole type: a bare core, carrying no fields
    /// of its own, so that binding it takes whatever it is against entire.
    fn fresh_type(&mut self) -> Rc<Ty> {
        let var = self.mint();
        Rc::new(Ty::plain(Core::Var(var)))
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
    /// The splice is what makes this more than a lookup. A variable stands for
    /// a whole type, so a type written `'c with { x: Nat }` becomes, once `'c`
    /// is known, that type's core carrying both its own labels and the `x` — the
    /// outer labels winning, the way [`Table::canon`] settles a tail's. Every
    /// reader of a type goes through here, so no reader has to know that a core
    /// can stand for something with fields of its own.
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

    /// A row flattened: its own labels joined with every label its tail has
    /// already accumulated, and what remains of the tail — an unbound variable,
    /// [`Rest::Closed`] or [`Rest::Undecided`] — as far as the solver has got.
    ///
    /// A read and nothing else, and one an [`IndexMap`] settles: the outer
    /// row's own labels are inserted first, so a label its tail also names is
    /// the label the row wrote out. A row that names one of its tail's labels
    /// is not a type, and there is no way left to write one — lowering refuses
    /// a declaration handed a row that repeats a label it already names, and
    /// [`Solve::assign`]'s lacks check refuses every route that goes through a
    /// variable — so what arrives here is a row and its tail agreeing.
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
        self.alike_row(&a.fields, &b.fields) && self.alike_core(&a.core, &b.core)
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
            (Core::Arrow(from, to), Core::Arrow(other_from, other_to)) => {
                self.alike(from, other_from) && self.alike(to, other_to)
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

    /// [`alike`](Self::alike) about two rows: flattened first, so that a row
    /// spliced into a tail is recognized as the row written out flat, and then
    /// name against name. The order the labels were written in decides nothing.
    fn alike_row(&self, a: &Row, b: &Row) -> bool {
        let (a, b) = (self.canon(a), self.canon(b));
        a.labels.len() == b.labels.len()
            && a.labels.iter().all(|(name, field)| {
                b.labels.get(name).is_some_and(|other| {
                    self.alike_presence(&field.presence, &other.presence)
                        && self.alike(&field.ty, &other.ty)
                })
            })
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
    fn occurs(&self, var: TyVar, value: &Assigned) -> bool {
        match value {
            Assigned::Ty(ty) => self.occurs_ty(var, ty),
            Assigned::Row(row) => self.occurs_row(var, row),
            Assigned::Presence(presence) => {
                matches!(self.presence_of(presence), Presence::Var(other) if other == var)
            }
        }
    }

    /// Whether `var` occurs in `ty` — in its core, or in the fields it carries.
    fn occurs_ty(&self, var: TyVar, ty: &Rc<Ty>) -> bool {
        let ty = self.resolve(ty);
        let core = match &ty.core {
            Core::Var(other) => *other == var,
            Core::Arrow(from, to) => self.occurs_ty(var, from) || self.occurs_ty(var, to),
            Core::Sum(cases) => self.occurs_row(var, cases),
            // A declared type is descended into as far as its arguments and no
            // further, here and in every walk below. What one stands for was
            // lowered from what the user wrote and mentions no variable at all
            // — lowering refuses a `..` or a `?` in a declaration for exactly
            // this reason — so there is nothing in the body to find and nothing
            // to rebuild; and stopping there is what keeps a walk over a type
            // that names itself finite. The arguments are the other half: they
            // were written at the use site and hold whatever it held, so
            // skipping them would miss a cycle.
            Core::Named { args, .. } => args.iter().any(|arg| self.occurs_ty(var, arg)),
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => false,
        };
        core || self.occurs_row(var, &ty.fields)
    }

    /// Whether `var` occurs in `row`. Every slot a row has, in one sequence: a
    /// label's presence is as much a place a variable can hide as its type is,
    /// and the tail is another, and none of the three is a different question.
    fn occurs_row(&self, var: TyVar, row: &Row) -> bool {
        let row = self.canon(row);
        matches!(row.rest, Rest::Var(other) if other == var)
            || row.labels.values().any(|field| {
                matches!(self.presence_of(&field.presence), Presence::Var(other) if other == var)
                    || self.occurs_ty(var, &field.ty)
            })
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
        // Every type carries fields, so every type says something here. The
        // labels land on whatever is still open past them *and* on the type's
        // own core variable, which stands for a type those labels are already
        // on: `'c with { x: Nat }` reads "some type, which also has an `x`", so
        // a `'c` with an `x` of its own would name the field twice.
        let labels = self.note_lacks_row(&ty.fields, Shape::Struct);
        if let Core::Var(var) = ty.core {
            self.forbidden(var, Shape::Struct, labels);
        }
        match &ty.core {
            Core::Arrow(from, to) => {
                let (from, to) = (from.clone(), to.clone());
                self.note_lacks(&from);
                self.note_lacks(&to);
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
                    // when there is something to forbid, which a type parameter
                    // never has.
                    let row = match self.params.get(&symbol).and_then(|kinds| kinds.get(at)) {
                        Some(ParamKind::Row { shape, lacks }) if !lacks.is_empty() => {
                            Some((*shape, lacks.clone()))
                        }
                        _ => None,
                    };
                    if let Some((shape, labels)) = row {
                        let written = self.resolve(arg).row(shape);
                        self.forbid(&written, shape, &labels);
                    }
                    self.note_lacks(arg);
                }
            }
            Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
        }
    }

    /// [`note_lacks`](Self::note_lacks) about one row: every label it and its
    /// tail name is forbidden of whatever is still open past them, and each
    /// label's type is walked in turn. What comes back is the labels, so that a
    /// type's core variable can be put under the same condition.
    fn note_lacks_row(&mut self, row: &Row, shape: Shape) -> IndexSet<String> {
        let flat = self.canon(row);
        let labels: IndexSet<String> = flat.labels.keys().cloned().collect();
        for field in flat.labels.values() {
            let ty = field.ty.clone();
            self.note_lacks(&ty);
        }
        if let Rest::Var(var) = flat.rest {
            self.forbidden(var, shape, labels.clone());
        }
        labels
    }

    /// Record that whatever is still open past `row` may not stand for any of
    /// `labels`: the tail chain followed to its end, and the condition put on
    /// the variable it lands at.
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

    /// Whether a variable an annotation left open came back decided.
    ///
    /// Bound is decided, with one exception: a variable that resolves to the
    /// undecided value of its own sort was abandoned rather than solved —
    /// which is the same question asked three ways, since the sorts have three
    /// spellings for "nothing is known". Something already
    /// failed and pointed it there, and a second complaint saying the
    /// annotation promised too much would be that failure said again in words
    /// about the wrong line. Bound to another variable still counts, unbound
    /// or not: the annotation said this part of the type was its own, and a
    /// definition that tied it to anything else has not kept that.
    fn narrowed(&self, var: TyVar) -> bool {
        let Slot::Bound(value) = &self.vars[var as usize] else {
            return false;
        };
        match value {
            Assigned::Ty(ty) => !matches!(self.resolve(ty).core, Core::Undecided),
            Assigned::Row(row) => {
                let flat = self.canon(row);
                !(flat.labels.is_empty() && matches!(flat.rest, Rest::Undecided))
            }
            Assigned::Presence(presence) => {
                !matches!(self.presence_of(presence), Presence::Undecided)
            }
        }
    }

    /// The first label `var` may not stand for that `value` names, if any, and
    /// the kind of row the condition came from: the lacks check, which
    /// [`Solve::assign`] runs before every binding the way it runs the occurs
    /// check.
    ///
    /// First in the row's own order rather than in the order the condition was
    /// recorded, so that the complaint names the label a reader would reach
    /// first reading the type left to right. The shape comes off the recorded
    /// condition, since a core variable's labels are the fields of whatever it
    /// is being bound to and there is no row here to read a shape from.
    fn repeated(&self, var: TyVar, value: &Assigned) -> Option<(Shape, String)> {
        let (shape, lacks) = self.lacks.get(&var)?;
        // Whatever labels the value would bring with it, read at the shape the
        // condition was recorded in: a row outright, or the row a whole type
        // carries. A presence brings none, which [`Assigned::as_row`] says by
        // answering with a row that names nothing.
        let named = self.canon(&value.as_row(*shape));
        let name = named.labels.keys().find(|name| lacks.contains(*name))?;
        Some((*shape, name.clone()))
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
        let row = value.as_row(shape);
        self.forbid(&row, shape, &labels);
        // A value that is a whole type puts the condition on its core variable
        // as well, which stands for a type those labels are already on.
        if let Assigned::Ty(ty) = value
            && let Core::Var(core) = self.resolve(ty).core
        {
            self.forbidden(core, shape, labels);
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

    /// Quantify everything in `ty` still unsolved deeper than the current
    /// level. Returns the scheme and the substitution that built it, so the
    /// caller can spell the same variables the same way elsewhere.
    fn generalize(&self, ty: &Rc<Ty>) -> (Scheme, HashMap<TyVar, u32>) {
        let mut subst = HashMap::new();
        self.quantify(ty, &mut subst);
        let body = self.zonk(ty, &subst);
        (Scheme::new(subst.len() as u32, body), subst)
    }

    /// Number the generalizable variables of `ty` in first-occurrence order —
    /// which is what makes the leftmost variable print as `'a`.
    ///
    /// Presence variables are numbered after everything else, in a second
    /// walk. A presence prints as the `?` on its field rather than as a
    /// letter, so numbering one in line would leave a visible gap: the scheme
    /// of `{ x?: Nat } -> 'a` should not call its one letter `'b`.
    fn quantify(&self, ty: &Rc<Ty>, subst: &mut HashMap<TyVar, u32>) {
        self.quantify_walk(ty, subst, false);
        self.quantify_walk(ty, subst, true);
    }

    /// One numbering pass: over everything but the presence slots, or — when
    /// `presences` — over only them. A variable can only be one or the other,
    /// so the two passes cannot number one twice.
    /// One numbering pass over one type: the core first and then the fields it
    /// carries, which is what makes `'c with { x: 'r }` number its core `'a`.
    /// No special case for a core — it is descended into exactly where it sits.
    fn quantify_walk(&self, ty: &Rc<Ty>, subst: &mut HashMap<TyVar, u32>, presences: bool) {
        let ty = self.resolve(ty);
        match &ty.core {
            Core::Var(var) => {
                if !presences {
                    Self::quantify_var(*var, subst);
                }
            }
            Core::Arrow(from, to) => {
                self.quantify_walk(from, subst, presences);
                self.quantify_walk(to, subst, presences);
            }
            Core::Sum(cases) => self.quantify_row(cases, subst, presences),
            // An argument left open is the definition's to quantify, the same
            // as one written anywhere else: `WithX ..'a -> Nat` names its tail
            // because this descends.
            Core::Named { args, .. } => {
                for arg in args.iter() {
                    self.quantify_walk(arg, subst, presences);
                }
            }
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => {}
        }
        self.quantify_row(&ty.fields, subst, presences);
    }

    /// [`quantify_walk`](Self::quantify_walk) over one row.
    fn quantify_row(&self, row: &Row, subst: &mut HashMap<TyVar, u32>, presences: bool) {
        let row = self.canon(row);
        for field in row.labels.values() {
            if presences && let Presence::Var(var) = self.presence_of(&field.presence) {
                Self::quantify_var(var, subst);
            }
            self.quantify_walk(&field.ty, subst, presences);
        }
        if !presences && let Rest::Var(var) = row.rest {
            Self::quantify_var(var, subst);
        }
    }

    /// Number one variable, unless it already has a number. Every variable
    /// this reaches is one the definition may quantify; see [`Table::vars`].
    fn quantify_var(var: TyVar, subst: &mut HashMap<TyVar, u32>) {
        let next = subst.len() as u32;
        subst.entry(var).or_insert(next);
    }

    /// Resolve a type all the way down, replacing each variable in `subst`
    /// with its quantified stand-in. What comes back never mentions the
    /// variable table, so it outlives the solver.
    fn zonk(&self, ty: &Rc<Ty>, subst: &HashMap<TyVar, u32>) -> Rc<Ty> {
        let ty = self.resolve(ty);
        let fields = self.zonk_row(&ty.fields, subst);
        let core = match &ty.core {
            // Indexed rather than looked up: everything that zonks quantifies
            // first — see [`Table::close`] and [`Table::generalize`] — so a
            // variable this walk can reach has a number by the time it does.
            Core::Var(var) => Core::Bound(subst[var]),
            Core::Arrow(from, to) => Core::Arrow(self.zonk(from, subst), self.zonk(to, subst)),
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

    /// [`zonk`](Self::zonk) over one row.
    ///
    /// A tail the solve bound to a row is spliced in, so that what outlives the
    /// solver is one flat row rather than a chain of them: `{ x: Nat, ..{ y:
    /// Nat } }` is not a type anyone wrote. [`canon`](Self::canon) is what does
    /// that, and it is lossless: a tail stands for the labels its row does not
    /// write out, and [`Solve::assign`] refuses to bind one to a row that
    /// writes out one of them, so by the time a solve is over no chain of rows
    /// repeats a name. Before the lacks check existed the splice was where a
    /// repeated field quietly lost a copy, and a definition came out with a
    /// type it had never been shown to have.
    fn zonk_row(&self, row: &Row, subst: &HashMap<TyVar, u32>) -> Row {
        let row = self.canon(row);
        let labels = row
            .labels
            .iter()
            .map(|(name, field)| {
                let field = RowField {
                    presence: match self.presence_of(&field.presence) {
                        Presence::Var(var) => Presence::Bound(subst[&var]),
                        decided => decided,
                    },
                    ty: self.zonk(&field.ty, subst),
                };
                (name.clone(), field)
            })
            .collect();
        let rest = match row.rest {
            Rest::Var(var) => Rest::Bound(subst[&var]),
            decided => decided,
        };
        Row { labels, rest }
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
    fn close(&self, ty: &Rc<Ty>, subst: &mut HashMap<TyVar, u32>) -> Rc<Ty> {
        self.quantify(ty, subst);
        self.zonk(ty, subst)
    }

    fn zonk_term(&self, term: &mut Term, subst: &mut HashMap<TyVar, u32>) {
        term.ty = self.close(&term.ty, subst);
        match &mut term.kind {
            TermKind::Apply { func, arg } => {
                self.zonk_term(func, subst);
                self.zonk_term(arg, subst);
            }
            TermKind::Fn { body, .. } => self.zonk_term(body, subst),
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
            TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
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
    fn zonk_error(&self, kind: &ErrorKind, subst: &mut HashMap<TyVar, u32>) -> ErrorKind {
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
            ErrorKind::RepeatedField { shape, field } => ErrorKind::RepeatedField {
                shape: *shape,
                field: field.clone(),
            },
        }
    }
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
/// the definition gets to decide: a `..` tail, a `..r` tail, and a `?` field
/// each lower to a fresh variable, minted at the level the annotation is
/// lowered at so that what stays unconstrained is quantified with the
/// definition. One call is one annotation, which is the whole scope of a
/// tail's name: every `..r` in it shares one variable, and no other
/// annotation's `r` can reach it.
///
/// A `type` declaration's body reaches none of that. A `?` and a bare `..` are
/// refused there outright, and the one tail it may have names a row parameter,
/// which lowers to a [`Core::Bound`] — a leaf whose value comes from the use
/// site, not a variable this table has to solve. So a declaration still lowers
/// to something with no [`Core::Var`] anywhere in it, which several walks here
/// rely on: it is why they may stop at a name rather than descend into what it
/// stands for.
fn lower_type(mint: &Mint, table: &mut Table, ty: &Type) -> Rc<Ty> {
    let mut rows: HashMap<String, Rest> = HashMap::new();
    let lowered = lower(mint, table, &mut rows, ty);
    // A tail stands for the fields its row did not write out, and this is
    // where that is first true of a written one: `{ x: Nat, ..r }` says `r`
    // has no `x`. See [`Table::lacks`].
    table.note_lacks(&lowered);
    lowered
}

/// The recursion inside [`lower_type`], carrying the annotation's named-tail
/// scope.
fn lower(mint: &Mint, table: &mut Table, rows: &mut HashMap<String, Rest>, ty: &Type) -> Rc<Ty> {
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
                .map(|arg| lower(mint, table, rows, arg))
                .collect(),
        },
        // A parameter is the position it was declared at, which is what
        // unfolding hands an argument to. See [`Core::Bound`].
        TypeKind::Param { index, .. } => Core::Bound(*index),
        TypeKind::Arrow { from, to } => {
            Core::Arrow(lower(mint, table, rows, from), lower(mint, table, rows, to))
        }
        // A written struct is unit carrying the fields that were written: the
        // fields are what the type says, and there is nothing else to it.
        TypeKind::Struct { fields, tail } => {
            let mut labels = IndexMap::new();
            for (name, field) in fields {
                let lowered = RowField {
                    presence: presence(table, field.optional),
                    ty: lower(mint, table, rows, &field.value),
                };
                labels.insert(name.clone(), lowered);
            }
            return Rc::new(Ty {
                core: Core::Unit,
                fields: row(table, rows, labels, tail),
            });
        }
        // The struct arm again, about cases — except that a sum's cases are its
        // core and its own field row is empty and closed. The one other
        // difference is the payload a case may not have written, which is unit
        // — the same type `()` is, built here rather than in the tree so that
        // what the reader wrote and what the compiler means stay two separate
        // things. See [`ir::TermKind::Tag`](crate::ir::TermKind::Tag).
        TypeKind::Sum { cases, tail } => {
            let mut labels = IndexMap::new();
            for (name, case) in cases {
                let carried = match &case.payload {
                    Some(payload) => lower(mint, table, rows, payload),
                    None => Rc::new(Ty::unit()),
                };
                let lowered = RowField {
                    presence: presence(table, case.optional),
                    ty: carried,
                };
                labels.insert(name.clone(), lowered);
            }
            Core::Sum(row(table, rows, labels, tail))
        }
        TypeKind::Error => Core::Undecided,
    };
    Rc::new(Ty::plain(core))
}

/// Whether one label is there, as the written `?` says it: a mark means the
/// definition decides, so the presence is a fresh variable, and no mark means
/// it is simply there.
fn presence(table: &mut Table, optional: bool) -> Presence {
    match optional {
        true => table.fresh_presence(),
        false => Presence::Present,
    }
}

/// The labels of a written row and what its `..` stands for, assembled into the
/// row. Shared by the two shapes, because the tail is the one part of a row
/// that reads the same either way: `..` is a variable this definition may
/// decide, `..r` is that variable shared across one annotation, and `..r`
/// naming a parameter is a position an argument is handed to.
fn row(
    table: &mut Table,
    rows: &mut HashMap<String, Rest>,
    labels: IndexMap<String, RowField>,
    tail: &Option<Tail>,
) -> Row {
    let rest = match tail.as_ref().map(|tail| &tail.of) {
        None => Rest::Closed,
        Some(ir::Row::Anything) => table.fresh_row(),
        Some(ir::Row::Named(name)) => rows
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
        ty = splice(&ty.fields, &scheme.body().open(&fresh));
    }
    ty
}

/// A type whose head has been replaced by what it stands for: the inner type's
/// core, carrying the labels written outside it with the inner type's own
/// joined underneath.
///
/// The outer labels win, the way [`Table::canon`] settles a tail's. What the
/// outer row leaves open is what the whole leaves open, and nothing is lost by
/// keeping it: a core only ever stands for a whole type where the type
/// mentioning it names no fields of its own — which is the line above — and for
/// a bare core otherwise.
fn splice(outer: &Row, inner: &Rc<Ty>) -> Rc<Ty> {
    if outer.is_trivial() {
        return inner.clone();
    }
    let mut labels = outer.labels.clone();
    for (name, field) in &inner.fields.labels {
        labels.entry(name.clone()).or_insert_with(|| field.clone());
    }
    Rc::new(Ty {
        core: inner.core.clone(),
        fields: Row {
            labels,
            rest: outer.rest.clone(),
        },
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
    pub fn open(&self, fresh: &[Assigned]) -> Rc<Ty> {
        let fields = self.fields.open(fresh, Shape::Struct);
        let core = match &self.core {
            // Not a leaf: what the variable stands for may carry fields, and
            // the fields written outside it are kept over them.
            Core::Bound(index) => {
                return splice(&fields, &fresh[*index as usize].as_ty());
            }
            Core::Arrow(from, to) => Core::Arrow(from.open(fresh), to.open(fresh)),
            Core::Sum(cases) => Core::Sum(cases.open(fresh, Shape::Sum)),
            // A declaration's own body reaches here holding its parameters, so
            // `type Wrap a = { inner: Pair a a }` depends on this arm entirely.
            Core::Named { symbol, name, args } => Core::Named {
                symbol: *symbol,
                name: name.clone(),
                args: args.iter().map(|arg| arg.open(fresh)).collect(),
            },
            Core::Unit | Core::Nat | Core::Var(_) | Core::Undecided => self.core.clone(),
        };
        Rc::new(Ty { core, fields })
    }
}

impl Row {
    /// [`Ty::open`] over one row. `shape` is what a row parameter written at
    /// this tail stands for, which is how an argument written as a whole type
    /// is read for the row it carries. See [`Assigned::as_row`].
    fn open(&self, fresh: &[Assigned], shape: Shape) -> Row {
        let labels = self
            .labels
            .iter()
            .map(|(name, field)| {
                let field = RowField {
                    presence: field.presence.open(fresh),
                    ty: field.ty.open(fresh),
                };
                (name.clone(), field)
            })
            .collect();
        let rest = match &self.rest {
            Rest::Bound(index) => Rest::More(Rc::new(fresh[*index as usize].as_row(shape))),
            rest => rest.clone(),
        };
        Row { labels, rest }
    }
}

impl Presence {
    /// [`Ty::open`] over one presence.
    fn open(&self, fresh: &[Assigned]) -> Presence {
        match self {
            Presence::Bound(index) => fresh[*index as usize].as_presence(),
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
/// still be one type — that is what a row's tail decides, in [`Solve::rows`]
/// — so failing here only means checking cannot push the expected fields in
/// one by one and the literal is inferred and equated instead.
fn same_field_set<A, B>(want: &IndexMap<String, A>, have: &IndexMap<String, B>) -> bool {
    want.len() == have.len() && want.keys().all(|name| have.contains_key(name))
}
