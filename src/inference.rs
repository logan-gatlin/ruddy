//! Assigning a type to every term.
//!
//! Hindley–Milner: unification, `let`-generalization, and Rémy-style levels
//! to decide what a definition may quantify over. Types are equated, never
//! ordered — two types either unify or they are an error — so a term's type
//! is the one type it has rather than a bound on it.
//!
//! Each definition is typed in two passes, and the [`Constraint`] list is all
//! they share.
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
//! Generalization is why the two passes alternate per definition rather than
//! running over the whole program: `let id = fn x => x` has to become a scheme
//! before the next definition's `id 1` can instantiate it.
//!
//! Inference runs after lowering and mutates the [`Program`] it is handed:
//! every [`Term`]'s `ty` goes from [`Ty::Undecided`] to what was inferred for
//! it, fully resolved, so nothing downstream ever needs the solver's variable
//! table to read a type. Errors do not stop either pass — a term that failed to
//! type still has a type, [`Ty::Undecided`], which unifies with everything so
//! that one mistake is reported once rather than echoed by every consumer.

use std::{collections::HashMap, rc::Rc, slice};

use indexmap::{IndexMap, IndexSet};

use crate::{
    ir::{Program, Tail, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{RowField, Scheme, Ty, TyVar},
};

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
    /// What the rule was applied to, in the shape a constraint has, since that
    /// is what it is — either the original one or a part of it.
    pub goal: ConstraintKind,
    pub effect: Effect,
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
    /// declared type. Either way there is nothing to take apart.
    Same,
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
    Overlap,
    /// Two identical primitives.
    Prim,
    /// Two arrows, taken apart into argument and result.
    Arrow,
    /// Two structs, taken apart: the fields both name against each other, and
    /// the fields only one names into what the other's tail allows.
    Struct,
    /// Whether one field is there, decided: present agrees with present and
    /// absent with absent, and a field one side must have while the other
    /// side cannot is where a missing or extra field is discovered.
    Presence,
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
    /// Pointing what an abandoned goal would have decided at [`Ty::Undecided`]
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
    /// A variable now points at a type.
    Bound { var: TyVar, ty: Rc<Ty> },
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
    /// Where the base of a projection was written, when this is the demand a
    /// projection makes of its base. A base that is not a struct at all is a
    /// complaint about the base, not about the field name — which is what
    /// `span` points at, being the only part of a projection the reader can
    /// change when the struct simply lacks the field.
    ///
    /// Only that one demand can be wrong in two places, so only it carries a
    /// second span; every other constraint has the one span its complaint
    /// belongs at.
    pub base_span: Option<Span>,
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
    /// A field demanded — by a projection, or by a struct type that requires
    /// it — of a struct that does not have it. The name is carried rather
    /// than left to be read back out of the source, so the message can be
    /// written once here instead of once per reporter.
    MissingField { base: Rc<Ty>, field: String },
    /// A field the struct has but the type it is against does not allow: a
    /// closed struct type lists every field there is, so one more is as wrong
    /// as one missing. Worded from the type rather than the field's own span
    /// because the type is the side that says what is allowed.
    ExtraField { base: Rc<Ty>, field: String },
    /// A struct shape was demanded of something that is not a struct at all.
    ///
    /// Held apart from [`ErrorKind::Mismatch`] by what the two sides are worth
    /// saying. A demand for `{ x: ?, .. }` is not a type anybody wrote — it is
    /// the solver's way of asking "does this have an `x`" — so quoting it back
    /// as an expectation shows the reader the bookkeeping rather than the
    /// mistake. Only the type that is not a struct is named, because it is the
    /// only one of the two on the page.
    ///
    /// Two written types that happen to be a closed struct and a `Nat` stay an
    /// ordinary mismatch: both of those the reader wrote, and naming both is
    /// the better message.
    NotAStruct { base: Rc<Ty> },
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
    /// A `..` was decided to stand for a field the row it tails already names.
    /// `{ x: Nat, ..r }` says "an `x`, plus whatever else `r` is", so `r`
    /// standing for anything with an `x` of its own would name the field
    /// twice — and the two copies could disagree. Only the label is carried:
    /// it is the one thing both halves of the contradiction have in common,
    /// and the rows it was found between are each half a type the reader never
    /// wrote down.
    RepeatedField { field: String },
}

/// What one type variable is known to be. Private to inference, and rightly so:
/// it is the solver's working state rather than part of the type language, and
/// nothing downstream ever sees a [`Ty::Var`] to want a slot for — generalizing
/// and zonking are what make sure of that.
#[derive(Debug, Clone)]
enum Slot {
    Unbound { level: u32 },
    Bound(Rc<Ty>),
}

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

/// Every type variable ever minted, and the level fresh ones are born at.
///
/// Both passes hold this: generation mints into it, solving binds in it, and
/// generalization reads it. It is the only state that outlives a pass.
#[derive(Default)]
struct Table {
    /// One slot per variable; [`Ty::Var`] indexes into it.
    vars: Vec<Slot>,
    /// The current generalization level. Fresh variables are born at it, and
    /// generalization quantifies exactly the variables born deeper than it.
    level: u32,
    /// What each row variable may not stand for: the field names the rows it
    /// is the tail of already write out.
    ///
    /// `{ x: Nat, ..r }` reads "an `x`, and whatever else `r` is", so `r`
    /// standing for a row with an `x` of its own would give the type two
    /// fields of one name. Nothing in [`Ty`] can express that side condition —
    /// a tail is an ordinary [`Ty::Var`] — so it is held here, beside the
    /// slots, and enforced at the one place a variable acquires a value.
    ///
    /// Insertion-ordered, so that a row breaking the rule twice always names
    /// the same field first and the complaint does not depend on a hash.
    lacks: HashMap<TyVar, IndexSet<String>>,
}

/// Pass one: the walk that says what has to hold, and solves nothing.
struct Constrain<'a> {
    table: &'a mut Table,
    /// What each symbol in scope means. Symbols are globally unique, so one
    /// flat map serves every scope at once and nothing is ever popped: a
    /// lambda argument can never collide with a top-level definition.
    env: &'a mut HashMap<Symbol, Binding>,
    /// What the declared types stand for, for the two arms that have to see a
    /// shape rather than a name: applying something annotated `Endo`, and
    /// checking a term against an annotation of `list`.
    ///
    /// Reading this is not reading the variable table. It is fixed before the
    /// first definition is walked and mentions no variable, so an arm that
    /// consults it still cannot depend on how much an earlier arm had solved.
    aliases: &'a IndexMap<Symbol, Scheme>,
    out: Vec<Constraint>,
}

/// Pass two: the solver, which sees constraints and never terms.
struct Solve<'a> {
    table: &'a mut Table,
    errors: &'a mut Vec<Error>,
    steps: &'a mut Vec<Step>,
    /// What the declared types stand for, so a goal about a name can become a
    /// goal about a shape. See [`unfold`].
    aliases: &'a IndexMap<Symbol, Scheme>,
    /// Stamped onto every step this solve records.
    definition: Symbol,
    /// How deep inside a decomposition the solver currently is.
    depth: u32,
    /// The pairs of declarations the goals currently open were reached by
    /// unfolding, innermost last.
    ///
    /// Two recursive types are equal when assuming they are equal never leads
    /// to a contradiction, so meeting a pair already on this stack ends the
    /// goal rather than starting it again. Kept as a stack rather than a set
    /// because the assumption holds for the goals the unfolding broke into and
    /// no further, which is the same scope [`Solve::depth`] tracks.
    ///
    /// Two declarations, rather than two types, because a pair of declarations
    /// is the only pair that can come back round — see [`Solve::unfold`].
    assumed: Vec<(Symbol, Symbol)>,
    /// [`Constraint::base_span`] of the goal as it arrived, and only of that
    /// goal: [`Solve::unify`] takes it on the way in, so what a goal decomposes
    /// into never sees it. Nothing under a projection's demand is about the
    /// base — it is about a type nested inside one — so the base's span would
    /// be pointing somewhere the reader cannot act on.
    ///
    /// Held beside the solve rather than passed down for the same reason: a
    /// parameter every recursive call had to pass `None` to would read as
    /// though the span were something the decomposition could use.
    base_span: Option<Span>,
}

/// Assign a type to every term in the program, in place, and return the
/// schemes of its top-level definitions.
pub fn infer(mint: &Mint, program: &mut Program) -> Output {
    let mut table = Table::default();
    let mut env = HashMap::new();
    let mut aliases = IndexMap::new();
    let mut errors = Vec::new();

    // Aliases first: annotations refer to them. A name inside a body stays a
    // name, so this pass reads no alias it is still building and the order it
    // runs in decides nothing — which is what lets two declarations refer to
    // each other.
    for (symbol, decl) in &program.types {
        // Binding nothing, until declarations have parameters to bind.
        let body = lower_type(mint, &mut table, &decl.value);
        aliases.insert(*symbol, Scheme::new(0, body));
    }

    let mut schemes = IndexMap::new();
    let mut constraints = IndexMap::new();
    let mut steps = Vec::new();
    for (symbol, decl) in program.terms.iter_mut() {
        // Each definition is solved one level in, so that everything still
        // unsolved when it ends is provably local to it and can be quantified.
        table.level = 1;

        // The annotation is the contract: the body is checked against it, and
        // it — not whatever the body's constraints worked out along the way —
        // is what the definition means to everyone downstream.
        //
        // Which is only honest while the body cannot decide the parts of it
        // the annotation left open. Nothing stops it: a `..`, a `..r` and a
        // `?` each lower to an ordinary variable, and a variable is something
        // the solve is free to bind. So the variables this lowering mints are
        // counted off here and checked once the definition is solved — see
        // [`Table::narrowed`]. The alternative is to make them unbindable, and
        // that is a larger language than this one has: a variable that refuses
        // to be bound needs a rule saying what happens when something tries,
        // and the answer has to travel all the way back to the annotation
        // anyway. Checking says the same thing, at the one place that knows
        // which variables came from where.
        let opened = table.vars.len() as TyVar;
        let annotated = decl
            .annotation
            .as_ref()
            .map(|annotation| lower_type(mint, &mut table, annotation));
        let closed = table.vars.len() as TyVar;

        let mut constrain = Constrain {
            table: &mut table,
            env: &mut env,
            aliases: &aliases,
            out: Vec::new(),
        };
        let ty = match &annotated {
            Some(expected) => {
                constrain.check_term(&mut decl.value, expected);
                expected.clone()
            }
            None => {
                constrain.infer_term(&mut decl.value);
                decl.value.ty.clone()
            }
        };
        let generated = constrain.out;

        // Where this definition's complaints begin. Each is resolved against
        // the substitution its own definition ends with, so which ones are its
        // own has to be marked before the next solve appends to the list.
        let reported = errors.len();

        Solve {
            table: &mut table,
            errors: &mut errors,
            steps: &mut steps,
            aliases: &aliases,
            definition: *symbol,
            depth: 0,
            assumed: Vec::new(),
            base_span: None,
        }
        .run(&generated);

        // One complaint per definition, not per variable: an annotation that
        // is too open is one thing to rewrite however many of its `..`s and
        // `?`s the body pinned down, and the reader is being sent to the same
        // line either way. Held back when the definition already said
        // something, because everything a failure abandons it also decides —
        // pointing it at `Ty::Undecided` is a binding like any other — and a
        // second complaint about the fallout of the first is one mistake said
        // twice.
        if let Some(annotation) = &decl.annotation
            && errors.len() == reported
            && (opened..closed).any(|var| table.narrowed(var))
        {
            errors.push(Error {
                span: annotation.span,
                kind: ErrorKind::AnnotationTooOpen,
            });
        }

        table.level = 0;
        let (scheme, mut subst) = table.generalize(&ty);
        // With the substitution in hand, resolve every type the walk wrote
        // into the body, so a term's type and its definition's scheme spell
        // the same variable the same way.
        table.zonk_term(&mut decl.value, &mut subst);
        // And the same for what it complained about, which is why this waits
        // until the definition is solved rather than running where the error
        // was reported: a variable in a payload may have been solved after the
        // fact, and the later knowledge reads better. Nothing past here can
        // touch this definition's variables, so this is the last word on them.
        for error in &mut errors[reported..] {
            error.kind = table.zonk_error(&error.kind, &mut subst);
        }
        env.insert(*symbol, Binding::Poly(scheme.clone()));
        schemes.insert(*symbol, scheme);
        constraints.insert(*symbol, generated);
    }

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
    fn fresh(&mut self) -> Rc<Ty> {
        let var = self.vars.len() as TyVar;
        self.vars.push(Slot::Unbound { level: self.level });
        Rc::new(Ty::Var(var))
    }

    /// Follow bound variables until reaching something that is not one. Only
    /// the head is resolved; a composite's children still need their own
    /// resolution, which is what [`zonk`](Self::zonk) does exhaustively.
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
            let Slot::Bound(inner) = &self.vars[*v as usize] else {
                break;
            };
            budget = budget
                .checked_sub(1)
                .expect("a chain of bound variables closed a cycle the occurs check should refuse");
            ty = inner.clone();
        }
        ty
    }

    /// Whether `var` occurs in `ty` — and, on the same walk, the level
    /// adjustment: every unbound variable in `ty` is pulled up to `level` if
    /// it was deeper, because it is about to be reachable from something at
    /// `level` and must not be generalized past it.
    fn occurs(&mut self, var: TyVar, level: u32, ty: &Rc<Ty>) -> bool {
        let ty = self.resolve(ty);
        match &*ty {
            Ty::Var(other) => {
                if *other == var {
                    return true;
                }
                let Slot::Unbound { level: at } = &mut self.vars[*other as usize] else {
                    unreachable!("resolve only stops at unbound variables");
                };
                *at = (*at).min(level);
                false
            }
            Ty::Arrow(from, to) => self.occurs(var, level, from) || self.occurs(var, level, to),
            Ty::Struct { fields, rest } => {
                for field in fields.values() {
                    if self.occurs(var, level, &field.presence)
                        || self.occurs(var, level, &field.ty)
                    {
                        return true;
                    }
                }
                self.occurs(var, level, rest)
            }
            // A declared type is descended into as far as its arguments and no
            // further, here and in every walk below. What one stands for was
            // lowered from what the user wrote and mentions no variable at all
            // — lowering refuses a `..` or a `?` in a declaration for exactly
            // this reason — so there is nothing in the body to find, no level
            // to pull up, and nothing to rebuild; and stopping there is what
            // keeps a walk over a type that names itself finite. The arguments
            // are the other half: they were written at the use site and hold
            // whatever it held, so skipping them would miss a cycle and leave a
            // level unraised, and generalization would quantify a variable that
            // escapes.
            Ty::Named { args, .. } => args.iter().any(|arg| self.occurs(var, level, arg)),
            Ty::Nat | Ty::Bound(_) | Ty::Undecided | Ty::Present | Ty::Absent | Ty::Empty => false,
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
    /// past them — the flattening [`Solve::canon`] does, for the same reason.
    fn note_lacks(&mut self, ty: &Rc<Ty>) {
        let ty = self.resolve(ty);
        match &*ty {
            Ty::Arrow(from, to) => {
                let (from, to) = (from.clone(), to.clone());
                self.note_lacks(&from);
                self.note_lacks(&to);
            }
            Ty::Struct { .. } => {
                let mut labels = IndexSet::new();
                let mut row = ty.clone();
                while let Ty::Struct { fields, rest } = &*row.clone() {
                    for (name, field) in fields {
                        labels.insert(name.clone());
                        self.note_lacks(&field.ty);
                    }
                    row = self.resolve(rest);
                }
                if let Ty::Var(var) = &*row {
                    self.lacks.entry(*var).or_default().extend(labels);
                }
            }
            // The body holds no row of its own to speak of, but an argument is
            // whatever the use site wrote — including an open row whose tail
            // must acquire the labels around it.
            Ty::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.note_lacks(arg);
                }
            }
            Ty::Nat
            | Ty::Var(_)
            | Ty::Bound(_)
            | Ty::Undecided
            | Ty::Present
            | Ty::Absent
            | Ty::Empty => {}
        }
    }

    /// Whether a variable an annotation left open came back decided.
    ///
    /// Bound is decided, with one exception: a variable that resolves to
    /// [`Ty::Undecided`] was abandoned rather than solved. Something already
    /// failed and pointed it there, and a second complaint saying the
    /// annotation promised too much would be that failure said again in words
    /// about the wrong line. Bound to another variable still counts, unbound
    /// or not: the annotation said this part of the type was its own, and a
    /// definition that tied it to anything else has not kept that.
    fn narrowed(&self, var: TyVar) -> bool {
        let Slot::Bound(ty) = &self.vars[var as usize] else {
            return false;
        };
        !matches!(&*self.resolve(ty), Ty::Undecided)
    }

    /// The first field `var` may not stand for that `ty` names, if any: the
    /// lacks check, which [`Solve::assign`] runs before every binding the way
    /// it runs the occurs check.
    ///
    /// First in the row's own order rather than in the order the condition was
    /// recorded, so that the complaint names the field a reader would reach
    /// first reading the type left to right.
    fn repeated(&self, var: TyVar, ty: &Rc<Ty>) -> Option<String> {
        let lacks = self.lacks.get(&var)?;
        let mut row = self.resolve(ty);
        loop {
            let Ty::Struct { fields, rest } = &*row.clone() else {
                return None;
            };
            if let Some(name) = fields.keys().find(|name| lacks.contains(*name)) {
                return Some(name.clone());
            }
            row = self.resolve(rest);
        }
    }

    /// Carry the lacks condition across a binding. What `var` may not stand
    /// for, whatever is still open past `ty` may not stand for either — and
    /// `ty`'s own rows impose their names on their own tails, which is
    /// [`note_lacks`](Self::note_lacks).
    ///
    /// Without this a condition would survive exactly one binding: `..r`
    /// absorbing a `y` continues as a fresh tail, and that tail is where the
    /// next field to conflict would arrive.
    fn inherit_lacks(&mut self, var: TyVar, ty: &Rc<Ty>) {
        self.note_lacks(ty);
        let Some(labels) = self.lacks.get(&var).cloned() else {
            return;
        };
        let mut row = self.resolve(ty);
        while let Ty::Struct { rest, .. } = &*row.clone() {
            row = self.resolve(rest);
        }
        if let Ty::Var(other) = &*row {
            self.lacks.entry(*other).or_default().extend(labels);
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
    fn quantify_walk(&self, ty: &Rc<Ty>, subst: &mut HashMap<TyVar, u32>, presences: bool) {
        let ty = self.resolve(ty);
        match &*ty {
            Ty::Var(var) => {
                if !presences {
                    self.quantify_var(*var, subst);
                }
            }
            Ty::Arrow(from, to) => {
                self.quantify_walk(from, subst, presences);
                self.quantify_walk(to, subst, presences);
            }
            Ty::Struct { fields, rest } => {
                for field in fields.values() {
                    if presences && let Ty::Var(var) = &*self.resolve(&field.presence) {
                        self.quantify_var(*var, subst);
                    }
                    self.quantify_walk(&field.ty, subst, presences);
                }
                self.quantify_walk(rest, subst, presences);
            }
            // An argument left open is the definition's to quantify, the same
            // as one written anywhere else: `WithX ..'a -> Nat` names its tail
            // because this descends.
            Ty::Named { args, .. } => {
                for arg in args.iter() {
                    self.quantify_walk(arg, subst, presences);
                }
            }
            Ty::Nat | Ty::Bound(_) | Ty::Undecided | Ty::Present | Ty::Absent | Ty::Empty => {}
        }
    }

    fn quantify_var(&self, var: TyVar, subst: &mut HashMap<TyVar, u32>) {
        let Slot::Unbound { level } = self.vars[var as usize] else {
            unreachable!("resolve only stops at unbound variables");
        };
        if level > self.level && !subst.contains_key(&var) {
            subst.insert(var, subst.len() as u32);
        }
    }

    /// Resolve a type all the way down, replacing each variable in `subst`
    /// with its quantified stand-in. What comes back never mentions the
    /// variable table, so it outlives the solver.
    fn zonk(&self, ty: &Rc<Ty>, subst: &HashMap<TyVar, u32>) -> Rc<Ty> {
        let ty = self.resolve(ty);
        match &*ty {
            Ty::Var(var) => match subst.get(var) {
                Some(&index) => Rc::new(Ty::Bound(index)),
                None => ty,
            },
            Ty::Arrow(from, to) => Rc::new(Ty::Arrow(self.zonk(from, subst), self.zonk(to, subst))),
            Ty::Struct { fields, rest } => {
                let mut fields: IndexMap<String, RowField> = fields
                    .iter()
                    .map(|(name, field)| {
                        let field = RowField {
                            presence: self.zonk(&field.presence, subst),
                            ty: self.zonk(&field.ty, subst),
                        };
                        (name.clone(), field)
                    })
                    .collect();
                // A tail the solve bound to a row is spliced in, so that what
                // outlives the solver is one flat struct rather than a chain
                // of them: `{ x: Nat, ..{ y: Nat } }` is not a type anyone
                // wrote. The tail was zonked first, so it is flat already and
                // one splice reaches its end.
                //
                // The two sides cannot share a label. A tail stands for the
                // fields its row does not write out, and [`Solve::assign`]
                // refuses to bind one to a row that writes out one of them, so
                // by the time a solve is over no chain of rows repeats a name.
                // `or_insert` is what says that here: it is a no-op, and the
                // splice being lossless is the invariant it stands on. Before
                // the lacks check existed this line was where a repeated field
                // quietly lost a copy, and a definition came out with a type
                // it had never been shown to have.
                let mut rest = self.zonk(rest, subst);
                while let Ty::Struct {
                    fields: inner,
                    rest: deeper,
                } = &*rest.clone()
                {
                    for (name, field) in inner {
                        fields.entry(name.clone()).or_insert_with(|| field.clone());
                    }
                    rest = deeper.clone();
                }
                Rc::new(Ty::Struct { fields, rest })
            }
            // Rebuilt rather than handed back, because an argument may hold a
            // variable and what leaves here may not. Nothing downstream
            // resolves one, so a `Ty::Var` that survived this would reach a
            // reader as an unanswerable `?3`.
            Ty::Named { symbol, name, args } if !args.is_empty() => Rc::new(Ty::Named {
                symbol: *symbol,
                name: name.clone(),
                args: args.iter().map(|arg| self.zonk(arg, subst)).collect(),
            }),
            Ty::Nat
            | Ty::Named { .. }
            | Ty::Bound(_)
            | Ty::Undecided
            | Ty::Present
            | Ty::Absent
            | Ty::Empty => ty,
        }
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
            ErrorKind::MissingField { base, field } => ErrorKind::MissingField {
                base: self.close(base, subst),
                field: field.clone(),
            },
            ErrorKind::ExtraField { base, field } => ErrorKind::ExtraField {
                base: self.close(base, subst),
                field: field.clone(),
            },
            ErrorKind::NotAStruct { base } => ErrorKind::NotAStruct {
                base: self.close(base, subst),
            },
            ErrorKind::AnnotationTooOpen => ErrorKind::AnnotationTooOpen,
            ErrorKind::RepeatedField { field } => ErrorKind::RepeatedField {
                field: field.clone(),
            },
        }
    }
}

impl Constrain<'_> {
    /// Record that `actual` — the type a term turned out to have — has to be
    /// the type the context demanded of it. The walk's only verb: it says so
    /// and moves on, which is the whole of what generation does.
    ///
    /// The demand goes last, and the name says which way round that is,
    /// because nothing downstream can put it back: [`Solve::unify`]
    /// decomposes structurally and swaps nothing, so a mismatch is worded in
    /// whatever order this was called in. An arm that had to remember an
    /// `expected, actual` pair got applications backwards and told the reader
    /// their annotation was the mistake.
    fn checks(&mut self, span: Span, actual: &Rc<Ty>, expected: &Rc<Ty>) {
        self.checks_base(span, None, actual, expected);
    }

    /// [`checks`](Self::checks) for a demand two spans can be right for. See
    /// [`Constraint::base_span`]: a projection asks one question with two ways
    /// of failing, and which of them the solver arrives at decides which of the
    /// two spans the reader is shown.
    fn checks_base(
        &mut self,
        span: Span,
        base_span: Option<Span>,
        actual: &Rc<Ty>,
        expected: &Rc<Ty>,
    ) {
        self.out.push(Constraint {
            span,
            base_span,
            kind: ConstraintKind::Equal {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        });
    }

    /// Infer a type for `term` and write it into `term.ty`.
    fn infer_term(&mut self, term: &mut Term) {
        let span = term.span;
        term.ty = match &mut term.kind {
            // The error term absorbs: it unifies with anything, so the one
            // diagnostic lowering already reported stays the only one.
            TermKind::Error => Rc::new(Ty::Undecided),
            TermKind::Natural(_) => Rc::new(Ty::Nat),
            TermKind::Ident(symbol) => {
                let symbol = *symbol;
                self.lookup(symbol)
            }
            TermKind::Apply { func, arg } => {
                self.infer_term(func);
                self.infer_term(arg);
                let applied = func.ty.clone();
                // Through a name, so that something annotated `Endo` is
                // applied as the arrow it stands for. The arrow the arm then
                // works with is the unfolded one, which is the only shape a
                // call site can take apart.
                match &*unfold(self.aliases, &applied) {
                    // The function already knows what it takes, so the demand
                    // on the argument is the parameter type and the result is
                    // the arrow's own. Written this way round, a mismatch
                    // reads "expected <parameter>, found <argument>": the
                    // parameter is what the context asked for, and the
                    // argument is the term the reader can change.
                    Ty::Arrow(from, to) => {
                        let (from, to) = (from.clone(), to.clone());
                        let actual = arg.ty.clone();
                        self.checks(arg.span, &actual, &from);
                        to
                    }
                    // Nothing is known about the function yet, so what the
                    // call site demands is the arrow shape itself, and the
                    // function is the term being checked against it: applying
                    // a non-function reads as "expected an arrow, found what
                    // you applied".
                    //
                    // The parameter is a variable of its own, and the argument
                    // is checked against it in a second constraint, because
                    // writing the argument into the demanded arrow asks two
                    // questions at once and answers both wrong as soon as the
                    // function turns out to be an arrow after all.
                    // [`Solve::unify`] decomposes without swapping, so the
                    // argument would come back out on the `expected` side and
                    // a mismatch would name the parameter as what was found —
                    // the very inversion [`Constrain::checks`] is ordered to
                    // prevent — carrying the whole application's span instead
                    // of the argument's. And a function that is not one would
                    // abandon the argument's type along with the arrow it was
                    // written into, since [`Solve::fail`] cannot tell which
                    // half of a demand the failure was about.
                    _ => {
                        let param = self.table.fresh();
                        let result = self.table.fresh();
                        let wanted = Rc::new(Ty::Arrow(param.clone(), result.clone()));
                        self.checks(span, &applied, &wanted);
                        let actual = arg.ty.clone();
                        self.checks(arg.span, &actual, &param);
                        result
                    }
                }
            }
            TermKind::Fn { arg, body } => {
                let param = self.table.fresh();
                self.env.insert(arg.tracked, Binding::Mono(param.clone()));
                self.infer_term(body);
                Rc::new(Ty::Arrow(param, body.ty.clone()))
            }
            TermKind::Struct(fields) => {
                let mut tys = IndexMap::new();
                for (name, field) in fields.iter_mut() {
                    self.infer_term(&mut field.value);
                    tys.insert(name.clone(), RowField::present(field.value.ty.clone()));
                }
                // A literal's fields are all there, and are all it has: the
                // tail is closed. Openness belongs to demands, not to values.
                Rc::new(Ty::Struct {
                    fields: tys,
                    rest: Rc::new(Ty::Empty),
                })
            }
            // The walk cannot name the type it produced — which type `.field`
            // has depends on a base the walk is in no position to know — but
            // it can say everything a projection demands of the base: a
            // struct that has the field, whatever else it may also have. The
            // field's type and the tail are variables, so `fn p => p.x` is
            // not a base waiting to be explained; it is a definition
            // polymorphic in everything but the field it reads.
            TermKind::Project { base, field } => {
                self.infer_term(base);
                let result = self.table.fresh();
                let rest = self.table.fresh();
                let want = Rc::new(Ty::Struct {
                    fields: [(field.tracked.clone(), RowField::present(result.clone()))]
                        .into_iter()
                        .collect(),
                    rest,
                });
                // "Whatever else it may also have" is everything but the field
                // just read, so the tail minted for it lacks that name.
                self.table.note_lacks(&want);
                let actual = base.ty.clone();
                // The field name is the only thing the user can fix about a
                // struct that does not have it — and the base is the only
                // thing they can fix about something that is not a struct.
                self.checks_base(field.span, Some(base.span), &actual, &want);
                result
            }
        };
    }

    /// Check `term` against a type the context already knows. Checking pushes
    /// expected types *into* binders — an annotated `fn p => p.x` learns `p`'s
    /// type from the annotation before the body needs it, which inference
    /// alone could not order. Everywhere the shapes do not line up, checking
    /// falls back to inferring and equating.
    ///
    /// `expected` is always a written type: it starts at an annotation and
    /// only ever loses an arrow or a field on the way down. The variables a
    /// written type can mention — a tail, a `?` field — arrive fresh from
    /// [`lower_type`] and unbound, so checking matches on what was literally
    /// written and, like the rest of generation, never has to ask the table
    /// anything.
    fn check_term(&mut self, term: &mut Term, expected: &Rc<Ty>) {
        // Checking looks through a name — an annotation of `list` still pushes
        // into a struct literal — but `term.ty` is set from `expected` rather
        // than from this, so the term keeps the name the user wrote and prints
        // as it.
        let shape = unfold(self.aliases, expected);
        match (&mut term.kind, &*shape) {
            (TermKind::Fn { arg, body }, Ty::Arrow(from, to)) => {
                let (from, to) = (from.clone(), to.clone());
                self.env.insert(arg.tracked, Binding::Mono(from));
                self.check_term(body, &to);
                term.ty = expected.clone();
            }
            // Only the exact closed shape a literal already has is pushed
            // into one field by field: every expected field certainly there,
            // no room for more, and the same names on both sides. Anything
            // open or optional falls through to inferring the literal and
            // letting row unification line the two up — which decides the
            // same things, just without the better spans pushing gives. The
            // gate reads the written type's own syntax, never the table, so
            // generation stays a description of the term.
            (TermKind::Struct(fields), Ty::Struct { fields: tys, rest })
                if matches!(&**rest, Ty::Empty)
                    && tys
                        .values()
                        .all(|field| matches!(&*field.presence, Ty::Present))
                    && same_field_set(fields, tys) =>
            {
                for (name, field) in fields.iter_mut() {
                    let want = tys[name].ty.clone();
                    self.check_term(&mut field.value, &want);
                }
                term.ty = expected.clone();
            }
            _ => {
                self.infer_term(term);
                let actual = term.ty.clone();
                self.checks(term.span, &actual, expected);
            }
        }
    }

    /// The type of one name in scope. A polymorphic binding is instantiated —
    /// each use gets its own copy of the quantified variables — while a
    /// monomorphic one is shared, so uses of a lambda argument constrain each
    /// other, which is exactly the let/lambda distinction.
    fn lookup(&mut self, symbol: Symbol) -> Rc<Ty> {
        match self.env.get(&symbol).cloned() {
            Some(Binding::Mono(ty)) => ty,
            Some(Binding::Poly(scheme)) => self.instantiate(&scheme),
            // Every resolved name was bound before any use of it could be
            // lowered; anything else already became `TermKind::Error`.
            None => Rc::new(Ty::Undecided),
        }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Rc<Ty> {
        let fresh: Vec<_> = (0..scheme.count()).map(|_| self.table.fresh()).collect();
        let ty = open(scheme.body(), &fresh);
        // A scheme's body says which of its rows a quantified tail is the tail
        // of, but the condition that follows from that is not part of the
        // body: it lived beside the variables the scheme closed over, and this
        // copy's variables are new. Said again, of them.
        self.table.note_lacks(&ty);
        ty
    }
}

impl Solve<'_> {
    /// Solve everything generation asked for, in the order it was asked.
    /// Every constraint is an equality, and rows are why that is enough: a
    /// projection's demand is an ordinary struct type with an open tail, so
    /// nothing has to wait for a later round to know what its base is.
    fn run(&mut self, constraints: &[Constraint]) {
        for constraint in constraints {
            let ConstraintKind::Equal { expected, actual } = &constraint.kind;
            self.base_span = constraint.base_span;
            self.unify(constraint.span, expected, actual);
        }
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the solve continues.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
        // Taken rather than read, so that this call is the only one it can
        // reach: see [`Solve::base_span`].
        let base_span = self.base_span.take();
        let lhs = self.table.resolve(expected);
        let rhs = self.table.resolve(actual);
        let goal = ConstraintKind::Equal {
            expected: lhs.clone(),
            actual: rhs.clone(),
        };
        match (&*lhs, &*rhs) {
            // Undecided is the absorbing error type: whatever failed under it
            // was reported where it failed. Absorbing is not the same as
            // learning nothing, though — the other side is a type this goal
            // was going to decide, and leaving its variables unbound would let
            // generalization quantify a term that only reached the solver
            // through a failure.
            (Ty::Undecided, _) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &rhs);
            }
            (_, Ty::Undecided) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &lhs);
            }
            (Ty::Var(a), Ty::Var(b)) if a == b => self.step(span, Rule::Same, goal, Effect::None),
            // Before unfolding, so that a variable against a declared type
            // takes the type by the name it was written as. What a definition
            // is inferred to be then reads as what its annotations said, and
            // the solver has one less thing to unfold later.
            (Ty::Var(var), _) => self.assign(span, goal, *var, &rhs),
            (_, Ty::Var(var)) => self.assign(span, goal, *var, &lhs),
            // One declaration applied to nothing is only ever equal to itself,
            // so this saves an unfolding rather than deciding anything. Two
            // *different* declarations fall through to unfolding and are equal
            // whenever what they stand for is.
            //
            // Matched on the arguments rather than through a `..`, so that the
            // arm which will have to compare them cannot be reached before it
            // exists. Two applications of one declaration are equal when their
            // arguments are — decided without unfolding either, which is what
            // will keep a declaration that leads back to itself from unfolding
            // forever — and until declarations take arguments there is no such
            // pair to meet. See [`Ty::Named`].
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
            ) if a == b && xs.is_empty() && ys.is_empty() => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            (Ty::Named { .. }, _) | (_, Ty::Named { .. }) => self.unfold(span, goal, &lhs, &rhs),
            (Ty::Nat, Ty::Nat) => self.step(span, Rule::Prim, goal, Effect::None),
            // The presence constants meeting themselves. No path reaches this:
            // every presence pair goes through [`Solve::field`], which decides
            // all four combinations of the constants itself and only calls
            // back here when at least one side is still a variable. Kept
            // because being unreachable is a property of the callers rather
            // than of the type language — two presences *are* equal when they
            // are the same constant — and a fall-through to the mismatch arm
            // below would report that as a contradiction.
            //
            // Their mismatches never reach the arm below either: a presence
            // clash is intercepted where the field's name is known, so the
            // complaint can name the field instead of saying `present` against
            // `absent`.
            (Ty::Present, Ty::Present) | (Ty::Absent, Ty::Absent) => {
                self.step(span, Rule::Presence, goal, Effect::None)
            }
            (Ty::Empty, Ty::Empty) => self.step(span, Rule::Same, goal, Effect::None),
            (Ty::Arrow(from1, to1), Ty::Arrow(from2, to2)) => {
                let (from1, to1) = (from1.clone(), to1.clone());
                let (from2, to2) = (from2.clone(), to2.clone());
                self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                self.depth += 1;
                self.unify(span, &from1, &from2);
                self.unify(span, &to1, &to2);
                self.depth -= 1;
            }
            (Ty::Struct { .. }, Ty::Struct { .. }) => self.rows(span, &lhs, &rhs),
            // Nothing applies. Which complaint that is depends on whether
            // either side is a demand rather than a written type: an open row
            // is one — no closed written type has a tail — so a projection
            // meeting a `Nat` is worded as the `Nat` having no fields instead
            // of as two types that cannot be made equal. Both directions,
            // since which side the demand landed on is decided by whoever
            // emitted the constraint.
            //
            // A projection is the one goal that came with somewhere else to
            // say it: the base is what has no fields, so that is what the
            // complaint underlines, while a struct that merely lacks the field
            // is still about the name that was read.
            _ => {
                let error = match (&*lhs, &*rhs) {
                    _ if self.demanded(&lhs) && fieldless(&rhs) => Error {
                        span: base_span.unwrap_or(span),
                        kind: ErrorKind::NotAStruct { base: rhs.clone() },
                    },
                    _ if self.demanded(&rhs) && fieldless(&lhs) => Error {
                        span: base_span.unwrap_or(span),
                        kind: ErrorKind::NotAStruct { base: lhs.clone() },
                    },
                    _ => Error {
                        span,
                        kind: ErrorKind::Mismatch {
                            expected: lhs.clone(),
                            actual: rhs.clone(),
                        },
                    },
                };
                self.fail(
                    span,
                    Rule::Mismatch,
                    goal,
                    error,
                    &[lhs.clone(), rhs.clone()],
                );
            }
        }
    }

    /// Make two structs the same row. [`Rule::Struct`] replaces the pair with
    /// everything that has to hold of it: fields only one side names flow
    /// into the other side's tail, and the fields both name are decided one
    /// by one.
    ///
    /// The tails go first, the shared fields after. The other order would let
    /// a shared field's own unification bind one of the tails behind the
    /// resolved copy this function is holding — a field's type can mention
    /// its own row's tail — and an act performed on a stale tail is an act
    /// performed on the wrong type.
    ///
    /// The step comes before all of it, and everything the rule does is one
    /// level under it. Flattening is where that used to leak: [`Solve::canon`]
    /// decided things, and it ran before the step, so what it decided appeared
    /// in the trace above and beside the rule it belongs to rather than
    /// beneath it. Flattening is now a read, and what it finds to decide is
    /// decided here, in the rule's own scope.
    ///
    /// Which is also what the goal is recorded as: the two rows flattened, not
    /// the two as they arrived. A tail already bound to a row is where those
    /// differ, and it is exactly the case where the unflattened goal cannot
    /// account for its own children — the fields under it come from a row the
    /// line above does not show.
    fn rows(&mut self, span: Span, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let mut want_repeats = Vec::new();
        let (want, want_rest) = self.canon(lhs, &mut want_repeats);
        let mut have_repeats = Vec::new();
        let (have, have_rest) = self.canon(rhs, &mut have_repeats);
        let expected = Rc::new(Ty::Struct {
            fields: want.clone(),
            rest: want_rest.clone(),
        });
        let actual = Rc::new(Ty::Struct {
            fields: have.clone(),
            rest: have_rest.clone(),
        });
        let goal = ConstraintKind::Equal {
            expected: expected.clone(),
            actual: actual.clone(),
        };

        self.step(span, Rule::Struct, goal.clone(), Effect::Decomposed);
        self.depth += 1;

        // What flattening found twice, decided now that there is a rule to
        // decide it under. Unreachable — see [`Solve::canon`] — and kept for
        // the reason the arm there is kept.
        for (name, first, second) in want_repeats {
            self.field(span, &name, &expected, &expected, &first, &second);
        }
        for (name, first, second) in have_repeats {
            self.field(span, &name, &actual, &actual, &first, &second);
        }

        let only_want: IndexMap<String, RowField> = want
            .iter()
            .filter(|(name, _)| !have.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();
        let only_have: IndexMap<String, RowField> = have
            .iter()
            .filter(|(name, _)| !want.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();

        // Two rows sharing one tail cannot differ in fields: whatever the
        // tail absorbed from one side it would grow on the other, and the
        // rows would chase each other forever. The occurs check inside
        // `assign` refuses the binding when only one side has extras, but the
        // both-sided case binds cleanly every round and never converges, so
        // the pair is refused here — the same cycle, caught one level up.
        if let (Ty::Var(a), Ty::Var(b)) = (&*want_rest, &*have_rest)
            && a == b
            && !(only_want.is_empty() && only_have.is_empty())
        {
            let error = Error {
                span,
                kind: ErrorKind::Recursive,
            };
            self.fail(span, Rule::Occurs, goal, error, &[lhs.clone(), rhs.clone()]);
            self.depth -= 1;
            return;
        }

        match (only_want.is_empty(), only_have.is_empty()) {
            // The rows name the same fields, so the tails are simply each
            // other. Two closed tails already agree, and saying so would be
            // a step about nothing.
            (true, true) => {
                if !matches!((&*want_rest, &*have_rest), (Ty::Empty, Ty::Empty)) {
                    self.unify(span, &want_rest, &have_rest);
                }
            }
            (true, false) => self.absorb(
                span,
                Side::Expected,
                &want_rest,
                only_have,
                &have_rest,
                &expected,
            ),
            (false, true) => self.absorb(
                span,
                Side::Actual,
                &have_rest,
                only_want,
                &want_rest,
                &actual,
            ),
            // Extras both ways continue as one fresh tail, which is what
            // makes the two rows end as the same row rather than merely
            // overlapping ones.
            (false, false) => {
                let rest = self.table.fresh();
                self.absorb(
                    span,
                    Side::Expected,
                    &want_rest,
                    only_have,
                    &rest,
                    &expected,
                );
                self.absorb(span, Side::Actual, &have_rest, only_want, &rest, &actual);
            }
        }

        let shared: Vec<_> = want
            .iter()
            .filter_map(|(name, field)| {
                have.get(name)
                    .map(|other| (name.clone(), field.clone(), other.clone()))
            })
            .collect();
        for (name, want, have) in shared {
            self.field(span, &name, &expected, &actual, &want, &have);
        }
        self.depth -= 1;
    }

    /// A struct flattened: its own fields joined with every field its tail
    /// has already accumulated, and what remains of the tail — an unbound
    /// variable, [`Ty::Empty`], or [`Ty::Undecided`] — resolved as far as the
    /// solver has got.
    ///
    /// A read and nothing else. It used to unify the labels it met twice on
    /// the way down, which made the shape of a goal something the solver
    /// settled before it had recorded the rule that was settling it; the pairs
    /// go into `repeats` instead, for [`Solve::rows`] to decide under its own
    /// step.
    ///
    /// Nothing ever goes into `repeats`. A tail can only carry a field its own
    /// row already names if something bound it to a row that names one, and
    /// that is exactly what the lacks check in [`Solve::assign`] refuses — so
    /// no chain of rows repeats a label. The pair is collected rather than
    /// dropped because an [`IndexMap`] holds one entry per key: without
    /// somewhere for a second copy to go, flattening would silently keep one
    /// of two field types, which is the failure the lacks check exists to end.
    fn canon(
        &self,
        ty: &Rc<Ty>,
        repeats: &mut Vec<(String, RowField, RowField)>,
    ) -> (IndexMap<String, RowField>, Rc<Ty>) {
        let Ty::Struct { fields, rest } = &**ty else {
            unreachable!("canon is only called on structs");
        };
        let mut fields = fields.clone();
        let mut rest = self.table.resolve(rest);
        while let Ty::Struct {
            fields: inner,
            rest: deeper,
        } = &*rest.clone()
        {
            for (name, field) in inner {
                match fields.get(name) {
                    Some(existing) => repeats.push((name.clone(), existing.clone(), field.clone())),
                    None => {
                        fields.insert(name.clone(), field.clone());
                    }
                }
            }
            rest = self.table.resolve(deeper);
        }
        (fields, rest)
    }

    /// Whether this type is a struct with a tail still open — which, for a
    /// type that reached the solver, means it is a demand rather than
    /// something written down.
    ///
    /// A written struct type is closed unless the user put a `..` in it, and a
    /// struct literal is closed always; what has an open tail is the shape a
    /// projection asks of its base. Not a proof — an annotation with a `..` in
    /// it is open too — but the wording it picks is right for that case as
    /// well: whatever was asked of a `Nat`, the answer is that a `Nat` has no
    /// fields.
    fn demanded(&self, ty: &Rc<Ty>) -> bool {
        let Ty::Struct { rest, .. } = &**ty else {
            return false;
        };
        matches!(&*self.table.resolve(rest), Ty::Var(_) | Ty::Undecided)
    }

    /// A row as it reads at this moment: the tail spliced in as far as it has
    /// been decided, and every presence fixed at what it has been decided to
    /// be — still open becomes [`Ty::Undecided`], which prints as the `?` the
    /// user would have written and which nothing downstream will rewrite.
    ///
    /// For the payload of a complaint, and only for that. A complaint's types
    /// are otherwise resolved at the end of the definition, on purpose: a
    /// variable solved after the fact usually reads better for having been.
    /// Presences are the exception, because the goals that decide them are the
    /// siblings of the one that failed. `{ a?: Nat, b: Nat }` against
    /// `{ b: 1, c: 2 }` complains about `c` and, in the same decomposition,
    /// settles `a` absent — and an absent field is not part of what a type
    /// says, so by the end the complaint named a type reading `{ b: Nat }`,
    /// which nobody wrote and which does not explain why `c` is refused.
    ///
    /// Field types and whatever is left of the tail stay live: those are not
    /// decided by the failing goal's siblings, and later knowledge about them
    /// is knowledge the reader wants.
    fn frozen(&self, ty: &Rc<Ty>) -> Rc<Ty> {
        let resolved = self.table.resolve(ty);
        if !matches!(&*resolved, Ty::Struct { .. }) {
            return resolved;
        }
        let mut fields: IndexMap<String, RowField> = IndexMap::new();
        let mut row = resolved;
        while let Ty::Struct {
            fields: named,
            rest,
        } = &*row.clone()
        {
            for (name, field) in named {
                let presence = self.table.resolve(&field.presence);
                let field = RowField {
                    presence: match &*presence {
                        Ty::Present | Ty::Absent => presence,
                        _ => Rc::new(Ty::Undecided),
                    },
                    ty: field.ty.clone(),
                };
                fields.entry(name.clone()).or_insert(field);
            }
            row = self.table.resolve(rest);
        }
        Rc::new(Ty::Struct { fields, rest: row })
    }

    /// Decide one field both rows name: whether it is there must agree, and
    /// while it may be, the types must too. A clash of the constants is
    /// worded as the field the actual side is missing or the extra one it
    /// has, never as `present` against `absent` — the field's name is known
    /// here, and the complaint should name it.
    fn field(
        &mut self,
        span: Span,
        name: &str,
        expected_base: &Rc<Ty>,
        actual_base: &Rc<Ty>,
        want: &RowField,
        have: &RowField,
    ) {
        let p1 = self.table.resolve(&want.presence);
        let p2 = self.table.resolve(&have.presence);
        let goal = ConstraintKind::Equal {
            expected: p1.clone(),
            actual: p2.clone(),
        };
        match (&*p1, &*p2) {
            // The common case: certainly there on both sides, so presence
            // has nothing to say and the types carry the whole question.
            (Ty::Present, Ty::Present) => self.unify(span, &want.ty, &have.ty),
            (Ty::Present, Ty::Absent) => {
                let kind = ErrorKind::MissingField {
                    base: self.frozen(actual_base),
                    field: name.to_string(),
                };
                let abandoned = [want.ty.clone(), have.ty.clone()];
                self.fail(span, Rule::Presence, goal, Error { span, kind }, &abandoned);
            }
            (Ty::Absent, Ty::Present) => {
                let kind = ErrorKind::ExtraField {
                    base: self.frozen(expected_base),
                    field: name.to_string(),
                };
                let abandoned = [want.ty.clone(), have.ty.clone()];
                self.fail(span, Rule::Presence, goal, Error { span, kind }, &abandoned);
            }
            // At least one side is still a variable or undecided: the
            // presences unify as anything else does, and the types follow
            // unless the field just settled absent — an absent field's type
            // slot means nothing, and constraining it would reject rows that
            // agree.
            _ => {
                self.unify(span, &p1, &p2);
                if !matches!(&*self.table.resolve(&p1), Ty::Absent) {
                    self.unify(span, &want.ty, &have.ty);
                }
            }
        }
    }

    /// Push the fields only one row names into the other row's tail, to
    /// continue as `rest`. `side` says which side of the goal the tail sits
    /// on, so that every act performed on its behalf keeps the direction the
    /// constraint was worded in; `base` is the struct the tail belongs to,
    /// which is the type a complaint names.
    ///
    /// An open tail takes the row whole — one binding, occurs-checked like
    /// any other. A closed tail takes nothing: each field must turn out
    /// absent, one that certainly is not is a missing or extra field by
    /// `side`, and whatever would have continued as `rest` is closed too.
    fn absorb(
        &mut self,
        span: Span,
        side: Side,
        tail: &Rc<Ty>,
        extras: IndexMap<String, RowField>,
        rest: &Rc<Ty>,
        base: &Rc<Ty>,
    ) {
        if !matches!(&**tail, Ty::Empty) {
            let row = Rc::new(Ty::Struct {
                fields: extras,
                rest: rest.clone(),
            });
            match side {
                Side::Expected => self.unify(span, tail, &row),
                Side::Actual => self.unify(span, &row, tail),
            }
            return;
        }

        let absent = Rc::new(Ty::Absent);
        for (name, field) in &extras {
            let presence = self.table.resolve(&field.presence);
            match (&*presence, side) {
                (Ty::Absent, _) => {}
                // A field certainly there, against a closed tail on the
                // expected side: the term has a field the type does not
                // allow. On the actual side it is the other complaint: the
                // type demands a field the term does not have.
                (Ty::Present, Side::Expected) => {
                    let goal = ConstraintKind::Equal {
                        expected: absent.clone(),
                        actual: presence.clone(),
                    };
                    let kind = ErrorKind::ExtraField {
                        base: self.frozen(base),
                        field: name.clone(),
                    };
                    let error = Error { span, kind };
                    self.fail(
                        span,
                        Rule::Presence,
                        goal,
                        error,
                        slice::from_ref(&field.ty),
                    );
                }
                (Ty::Present, Side::Actual) => {
                    let goal = ConstraintKind::Equal {
                        expected: presence.clone(),
                        actual: absent.clone(),
                    };
                    let kind = ErrorKind::MissingField {
                        base: self.frozen(base),
                        field: name.clone(),
                    };
                    let error = Error { span, kind };
                    self.fail(
                        span,
                        Rule::Presence,
                        goal,
                        error,
                        slice::from_ref(&field.ty),
                    );
                }
                (_, Side::Expected) => self.unify(span, &absent, &field.presence),
                (_, Side::Actual) => self.unify(span, &field.presence, &absent),
            }
        }
        if !matches!(&**rest, Ty::Empty) {
            let empty = Rc::new(Ty::Empty);
            match side {
                Side::Expected => self.unify(span, &empty, rest),
                Side::Actual => self.unify(span, rest, &empty),
            }
        }
    }

    /// Replace whichever sides are declared types with what they stand for,
    /// and ask the goal again.
    ///
    /// This is where recursive types are decided, and it is the only rule that
    /// can be asked the same question twice: `list` against a second
    /// declaration of the same shape unfolds to two structs whose `next`
    /// fields are the two declarations again. So a pair of declarations is
    /// remembered while the goals it broke into are open, and meeting it again
    /// is [`Rule::Assume`] — the two are equal exactly when assuming so leads
    /// to no contradiction, and every contradiction there could be is one of
    /// the goals in between.
    ///
    /// A pair of *declarations* is the only pair worth remembering, because it
    /// is the only pair that can come back round. Every [`Ty`] is a finite
    /// tree — a name is a leaf, not an edge — so a name against a shape
    /// descends into a strictly smaller shape each time and runs out, and only
    /// a name reached from a name can be where it started. That also makes
    /// this terminate: there are finitely many pairs of declarations, and the
    /// stack refuses to take one twice.
    ///
    /// Equality stays structural throughout. A name is a barrier to unfolding
    /// and nothing else: two declarations that unfold the same way are one
    /// type however differently they were written, and the same declaration on
    /// both sides is decided by [`Rule::Same`] as a shortcut, never as a rule
    /// that names carry meaning.
    fn unfold(&mut self, span: Span, goal: ConstraintKind, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let pair = match (&**lhs, &**rhs) {
            (Ty::Named { symbol: a, .. }, Ty::Named { symbol: b, .. }) => Some((*a, *b)),
            _ => None,
        };
        if pair.is_some_and(|pair| self.assumed.contains(&pair)) {
            self.step(span, Rule::Assume, goal, Effect::None);
            return;
        }
        let lhs = unfold(self.aliases, lhs);
        let rhs = unfold(self.aliases, rhs);
        self.step(span, Rule::Unfold, goal, Effect::Decomposed);
        self.assumed.extend(pair);
        self.depth += 1;
        self.unify(span, &lhs, &rhs);
        self.depth -= 1;
        if pair.is_some() {
            self.assumed.pop();
        }
    }

    /// Point an unbound variable at a type, unless the type contains the
    /// variable itself — the occurs check that keeps every type a finite tree.
    /// Either way one step is recorded, and the rule it names is the one that
    /// actually applied: a cycle is [`Rule::Occurs`], not a [`Rule::Bind`] that
    /// happened to leave the variable where it was.
    ///
    /// Recursive types do not soften this, and this is the line that says so.
    /// A type may lead back to itself only through a declaration, where a
    /// person wrote down what it is; `fn x => x x` asks the solver to invent
    /// one, which is the difference between a type the language has and a type
    /// nothing could have written.
    ///
    /// The lacks check beside it is the row's version of the same idea: a tail
    /// stands for the fields its row does not write out, so a row that writes
    /// one of them out is not a value that tail can take. Refused here rather
    /// than noticed later for a reason the occurs check does not share —
    /// nothing later is guaranteed to notice. Two rows sharing a tail are only
    /// compared again if the program happens to bring them back together, and
    /// [`Table::zonk`] keeps one copy of a repeated field without a word, so
    /// the contradiction reached the reader as a silently narrowed type or as
    /// a mismatch somewhere else entirely.
    fn assign(&mut self, span: Span, goal: ConstraintKind, var: TyVar, ty: &Rc<Ty>) {
        let Slot::Unbound { level } = self.table.vars[var as usize] else {
            unreachable!("resolve only stops at unbound variables");
        };
        if self.table.occurs(var, level, ty) {
            let error = Error {
                span,
                kind: ErrorKind::Recursive,
            };
            self.fail(
                span,
                Rule::Occurs,
                goal,
                error,
                &[Rc::new(Ty::Var(var)), ty.clone()],
            );
            return;
        }
        // One complaint per binding, not per label: a tail that would have to
        // repeat two fields is one thing gone wrong with one row, and naming
        // the first of them is what the reader has to look at either way.
        if let Some(field) = self.table.repeated(var, ty) {
            let error = Error {
                span,
                kind: ErrorKind::RepeatedField { field },
            };
            self.fail(
                span,
                Rule::Overlap,
                goal,
                error,
                &[Rc::new(Ty::Var(var)), ty.clone()],
            );
            return;
        }
        self.table.inherit_lacks(var, ty);
        self.table.vars[var as usize] = Slot::Bound(ty.clone());
        let effect = Effect::Bound {
            var,
            ty: ty.clone(),
        };
        self.step(span, Rule::Bind, goal, effect);
    }

    /// Report a failure and abandon what it was about, in one act: the
    /// complaint, the step that ends the goal, and then every type in
    /// `abandoned` pointed at [`Ty::Undecided`].
    ///
    /// The one way the solver has of failing, and deliberately so. Reporting
    /// and recovering used to be two calls an arm had to remember to make in
    /// order, and the arms disagreed: a mismatch reported without recovering,
    /// so the variable it had abandoned stayed unbound, and generalization
    /// quantified it — which made a term that failed to type polymorphic, and
    /// therefore silently acceptable to every later use of it.
    ///
    /// The error carries its own span rather than taking `span`, because the
    /// two are not always the same: a projection is stepped where it was
    /// written and complained about at its base.
    fn fail(
        &mut self,
        span: Span,
        rule: Rule,
        goal: ConstraintKind,
        error: Error,
        abandoned: &[Rc<Ty>],
    ) {
        let kind = error.kind.clone();
        self.errors.push(error);
        self.step(span, rule, goal, Effect::Failed(kind));
        for ty in abandoned {
            self.recover(span, ty);
        }
    }

    /// Abandon a type a failed goal would have decided: every variable still
    /// unsolved in it becomes [`Ty::Undecided`], which unifies with
    /// everything, so the one complaint is not echoed by every term downstream
    /// of it. No occurs check — `Undecided` mentions no variables to close a
    /// cycle with.
    ///
    /// A step per variable, because each one changes the solution: a reader
    /// following the state would otherwise see a variable acquire a value that
    /// no rule they were shown gave it.
    fn recover(&mut self, span: Span, ty: &Rc<Ty>) {
        match &*self.table.resolve(ty) {
            Ty::Var(var) => {
                let (var, undecided) = (*var, Rc::new(Ty::Undecided));
                self.table.vars[var as usize] = Slot::Bound(undecided.clone());
                let goal = ConstraintKind::Equal {
                    expected: Rc::new(Ty::Var(var)),
                    actual: undecided.clone(),
                };
                self.step(
                    span,
                    Rule::Recover,
                    goal,
                    Effect::Bound { var, ty: undecided },
                );
            }
            // A composite is abandoned by abandoning what it is made of: the
            // goal that would have decided `?1 -> ?2` decided neither half.
            Ty::Arrow(from, to) => {
                let (from, to) = (from.clone(), to.clone());
                self.recover(span, &from);
                self.recover(span, &to);
            }
            // A field is abandoned whole: its presence, its type, and the tail
            // saying what else the struct might have had. Missing one would
            // leave a variable unbound for generalization to quantify.
            Ty::Struct { fields, rest } => {
                let parts: Vec<_> = fields
                    .values()
                    .flat_map(|field| [field.presence.clone(), field.ty.clone()])
                    .chain([rest.clone()])
                    .collect();
                for ty in parts {
                    self.recover(span, &ty);
                }
            }
            // For the reason a field is: an argument the abandoned goal would
            // have decided is left for generalization to quantify otherwise.
            Ty::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.recover(span, arg);
                }
            }
            Ty::Nat | Ty::Bound(_) | Ty::Undecided | Ty::Present | Ty::Absent | Ty::Empty => {}
        }
    }

    fn step(&mut self, span: Span, rule: Rule, goal: ConstraintKind, effect: Effect) {
        self.steps.push(Step {
            definition: self.definition,
            span,
            depth: self.depth,
            rule,
            goal,
            effect,
        });
    }
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
/// the definition gets to decide: a `..` tail, a `..r` tail, and a `?` field
/// each lower to a fresh variable, minted at the level the annotation is
/// lowered at so that what stays unconstrained is quantified with the
/// definition. One call is one annotation, which is the whole scope of a
/// tail's name: every `..r` in it shares one variable, and no other
/// annotation's `r` can reach it. A `type` declaration's body mentions none
/// of these — lowering refused them — so an alias never lowers to anything
/// with a variable in it, which several walks here rely on.
fn lower_type(mint: &Mint, table: &mut Table, ty: &Type) -> Rc<Ty> {
    let mut rows = HashMap::new();
    let lowered = lower(mint, table, &mut rows, ty);
    // A tail stands for the fields its row did not write out, and this is
    // where that is first true of a written one: `{ x: Nat, ..r }` says `r`
    // has no `x`. See [`Table::lacks`].
    table.note_lacks(&lowered);
    lowered
}

/// The recursion inside [`lower_type`], carrying the annotation's named-tail
/// scope.
fn lower(mint: &Mint, table: &mut Table, rows: &mut HashMap<String, Rc<Ty>>, ty: &Type) -> Rc<Ty> {
    match &ty.tracked {
        TypeKind::Prim(prim) => Rc::new((*prim).into()),
        TypeKind::Ident(symbol) => Rc::new(Ty::Named {
            symbol: *symbol,
            name: mint.name(*symbol).into(),
            // A name written bare is applied to nothing, which is every
            // declaration until the type language grows a binder.
            args: Rc::from([]),
        }),
        TypeKind::Arrow { from, to } => Rc::new(Ty::Arrow(
            lower(mint, table, rows, from),
            lower(mint, table, rows, to),
        )),
        TypeKind::Struct { fields, tail } => Rc::new(Ty::Struct {
            fields: fields
                .iter()
                .map(|(name, field)| {
                    let presence = match field.optional {
                        true => table.fresh(),
                        false => Rc::new(Ty::Present),
                    };
                    let lowered = RowField {
                        presence,
                        ty: lower(mint, table, rows, &field.value),
                    };
                    (name.clone(), lowered)
                })
                .collect(),
            rest: match tail {
                None => Rc::new(Ty::Empty),
                Some(Tail { name: None, .. }) => table.fresh(),
                Some(Tail {
                    name: Some(name), ..
                }) => rows
                    .entry(name.clone())
                    .or_insert_with(|| table.fresh())
                    .clone(),
            },
        }),
        TypeKind::Error => Rc::new(Ty::Undecided),
    }
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
/// What guarantees this terminates is the check in
/// [`ir::build`](crate::ir::build), not anything here: a chain of names that
/// closed a loop would be a type declared as itself, which lowering refuses,
/// so following one strictly shrinks what is left to follow. The same bargain
/// [`Table::resolve`] has with the occurs check.
///
/// A name with no declaration behind it is [`Ty::Undecided`]: the only way to
/// write one is to repeat a type's name, which was already reported.
pub fn unfold(aliases: &IndexMap<Symbol, Scheme>, ty: &Rc<Ty>) -> Rc<Ty> {
    let mut ty = ty.clone();
    // The budget is the one [`Table::resolve`] keeps, for the reason it keeps
    // it: a chain of declarations cannot close a loop, because
    // [`ir::build`](crate::ir::build) refuses one. This bounds what a bug in
    // that check would cost rather than restating the guarantee.
    let mut budget = aliases.len();
    while let Ty::Named { symbol, args, .. } = &*ty.clone() {
        let Some(scheme) = aliases.get(symbol) else {
            return Rc::new(Ty::Undecided);
        };
        budget = budget
            .checked_sub(1)
            .expect("a chain of declarations closed a loop that lowering should refuse");
        ty = open(scheme.body(), args);
    }
    ty
}

/// Whether this type is one a field could not be read off whatever else is
/// true of it. The two the language has: a number and a function.
///
/// Listed rather than written as "not a struct", so that a type added later
/// has to be considered instead of quietly inheriting a message about fields.
/// The presence and row constants are excluded on the same principle — one of
/// those meeting a struct is a bug in the solver, and "`absent` is not a
/// struct" is not the diagnostic that should carry it to anyone.
fn fieldless(ty: &Rc<Ty>) -> bool {
    matches!(&**ty, Ty::Nat | Ty::Arrow(..))
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

/// Replace each [`Ty::Bound`] with its instantiation. A free function rather
/// than a method: a scheme's body mentions no solver variables, so opening it
/// needs nothing from the solver's state.
fn open(ty: &Rc<Ty>, fresh: &[Rc<Ty>]) -> Rc<Ty> {
    match &**ty {
        Ty::Bound(index) => fresh[*index as usize].clone(),
        Ty::Arrow(from, to) => Rc::new(Ty::Arrow(open(from, fresh), open(to, fresh))),
        Ty::Struct { fields, rest } => Rc::new(Ty::Struct {
            fields: fields
                .iter()
                .map(|(name, field)| {
                    let field = RowField {
                        presence: open(&field.presence, fresh),
                        ty: open(&field.ty, fresh),
                    };
                    (name.clone(), field)
                })
                .collect(),
            rest: open(rest, fresh),
        }),
        // A declaration's own body reaches here holding its parameters, so
        // `type Wrap a = { inner: Pair a a }` depends on this arm entirely.
        Ty::Named { symbol, name, args } if !args.is_empty() => Rc::new(Ty::Named {
            symbol: *symbol,
            name: name.clone(),
            args: args.iter().map(|arg| open(arg, fresh)).collect(),
        }),
        Ty::Nat
        | Ty::Named { .. }
        | Ty::Var(_)
        | Ty::Undecided
        | Ty::Present
        | Ty::Absent
        | Ty::Empty => ty.clone(),
    }
}
