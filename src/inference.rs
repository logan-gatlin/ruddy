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

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    ir::{Program, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{Scheme, Ty, TyVar},
};

#[derive(Debug, Clone)]
pub struct Output {
    /// What each `type` declaration stands for: the semantic type its body
    /// denotes, one step deep. A name inside a body stays a [`Ty::Named`] and
    /// is looked up here again, which is how a declaration that names itself
    /// stays a finite value — and why this map, not the type, is what a
    /// recursive type is made of. See [`unfold`].
    pub aliases: IndexMap<Symbol, Rc<Ty>>,
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
/// and the assumption that ends an unfolding likewise — plus the three a
/// deferred projection can take and the recovery that follows every failure.
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
    /// Two identical primitives.
    Prim,
    /// Two arrows, taken apart into argument and result.
    Arrow,
    /// Two structs with the same field names, taken apart field by field.
    Struct,
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
    /// A projection whose base turned out to be a struct.
    Project,
    /// A projection whose base is still unknown, put back for another round.
    Defer,
    /// A projection put back one round too many: nothing left will explain it.
    Stuck,
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
    /// `base` must be a struct with a `name` field, whose type is `result`.
    ///
    /// Plain equality cannot say this: it can equate `base` with a struct, but
    /// only if it already knows which one, and a projection names one field
    /// rather than the whole type. So the constraint is kept whole and solved
    /// late — after every equality has had its say, and retried until no more
    /// bases become known — which is what lets `p.x` be written before the
    /// constraint that explains what `p` is.
    Field {
        base: Rc<Ty>,
        /// Where the base was written. A base that is not a struct is a
        /// complaint about the base, not about the field name.
        base_span: Span,
        name: String,
        result: Rc<Ty>,
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
    /// type language stays finite trees.
    Recursive,
    /// Projection out of a type that is not a struct: `1.x`, or a field read
    /// off a function.
    NotAStruct { base: Rc<Ty> },
    /// Projection out of a base nothing in the definition ever said the type
    /// of, as in an unannotated `fn p => p.x`. The fix is an annotation rather
    /// than a different base, so it is a complaint of its own rather than a
    /// [`ErrorKind::NotAStruct`] whose payload happens to be a variable: which
    /// of the two it is has to be decided where the solver gives up, because
    /// giving up is itself what points that variable at [`Ty::Undecided`].
    UnknownBase,
    /// Projection of a field the struct does not have. The name is carried
    /// rather than left to be read back out of the source, so the message can
    /// be written once here instead of once per reporter.
    MissingField { base: Rc<Ty>, field: String },
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
    aliases: &'a IndexMap<Symbol, Rc<Ty>>,
    out: Vec<Constraint>,
}

/// A [`ConstraintKind::Field`] taken apart once, at the moment the solver sets
/// it aside.
///
/// Projections are the constraints that cannot be answered in the order they
/// arrived, so they are collected and retried until a round learns nothing.
/// Collecting them as whole [`Constraint`]s meant every use re-destructured a
/// kind whose shape had already been established, behind an `unreachable!` that
/// could only ever be read as noise. Taking them apart where it is *known* they
/// are field constraints leaves the retry loop with the four things it uses and
/// no arm to explain.
struct Projection {
    /// Where the field name was written — the only part of a projection the
    /// user can change when the struct does not have it.
    span: Span,
    base: Rc<Ty>,
    /// Where the base was written. A base that is not a struct is a complaint
    /// about the base, not about the field name.
    base_span: Span,
    name: String,
    result: Rc<Ty>,
}

/// Pass two: the solver, which sees constraints and never terms.
struct Solve<'a> {
    table: &'a mut Table,
    errors: &'a mut Vec<Error>,
    steps: &'a mut Vec<Step>,
    /// What the declared types stand for, so a goal about a name can become a
    /// goal about a shape. See [`unfold`].
    aliases: &'a IndexMap<Symbol, Rc<Ty>>,
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
        aliases.insert(*symbol, lower_type(mint, &decl.value));
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
        let annotated = decl
            .annotation
            .as_ref()
            .map(|annotation| lower_type(mint, annotation));

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
        }
        .run(&generated);

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

    // Deferred constraints are reported after the equalities that outran them,
    // which is not the order anyone reads a file in. Sorting by position puts
    // that back; the sort is stable, so two complaints about one span keep the
    // order the solver found them in.
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
            Ty::Struct(fields) => {
                for ty in fields.values() {
                    if self.occurs(var, level, ty) {
                        return true;
                    }
                }
                false
            }
            // A declared type is not descended into, here or in any of the
            // walks below. What one stands for was lowered from what the user
            // wrote and mentions no variable at all, so there is nothing
            // inside it to find, no level to pull up, and nothing to rebuild —
            // and stopping is what keeps a walk over a type that names itself
            // finite.
            Ty::Nat | Ty::Named { .. } | Ty::Bound(_) | Ty::Undecided => false,
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
    fn quantify(&self, ty: &Rc<Ty>, subst: &mut HashMap<TyVar, u32>) {
        let ty = self.resolve(ty);
        match &*ty {
            Ty::Var(var) => {
                let Slot::Unbound { level } = self.vars[*var as usize] else {
                    unreachable!("resolve only stops at unbound variables");
                };
                if level > self.level && !subst.contains_key(var) {
                    subst.insert(*var, subst.len() as u32);
                }
            }
            Ty::Arrow(from, to) => {
                self.quantify(from, subst);
                self.quantify(to, subst);
            }
            Ty::Struct(fields) => {
                for ty in fields.values() {
                    self.quantify(ty, subst);
                }
            }
            Ty::Nat | Ty::Named { .. } | Ty::Bound(_) | Ty::Undecided => {}
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
            Ty::Struct(fields) => Rc::new(Ty::Struct(
                fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.zonk(ty, subst)))
                    .collect(),
            )),
            Ty::Nat | Ty::Named { .. } | Ty::Bound(_) | Ty::Undecided => ty,
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
            ErrorKind::NotAStruct { base } => ErrorKind::NotAStruct {
                base: self.close(base, subst),
            },
            ErrorKind::UnknownBase => ErrorKind::UnknownBase,
            ErrorKind::MissingField { base, field } => ErrorKind::MissingField {
                base: self.close(base, subst),
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
        self.out.push(Constraint {
            span,
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
                    tys.insert(name.clone(), field.value.ty.clone());
                }
                Rc::new(Ty::Struct(tys))
            }
            // The one place the walk cannot name the type it produced: which
            // type `.field` has depends on a base the walk is in no position
            // to know. So it mints a variable, says what relates the two, and
            // leaves the reading of the struct to the solver.
            TermKind::Project { base, field } => {
                self.infer_term(base);
                let result = self.table.fresh();
                self.out.push(Constraint {
                    // The field name is the only thing the user can fix about
                    // a struct that does not have it.
                    span: field.span,
                    kind: ConstraintKind::Field {
                        base: base.ty.clone(),
                        base_span: base.span,
                        name: field.tracked.clone(),
                        result: result.clone(),
                    },
                });
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
    /// only ever loses an arrow or a field on the way down. Written types
    /// mention no solver variables, so checking, like the rest of generation,
    /// never has to ask the table anything.
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
            (TermKind::Struct(fields), Ty::Struct(tys)) if same_field_set(fields, tys) => {
                for (name, field) in fields.iter_mut() {
                    let want = tys[name].clone();
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
        open(scheme.body(), &fresh)
    }
}

impl Solve<'_> {
    /// Solve everything generation asked for, in the order that can answer it.
    ///
    /// Equalities first, as they were emitted: each stands on its own, and
    /// solving them is what gives the projections a base to look at. Then the
    /// projections, over and over, because solving one is itself an equality
    /// and may be what makes the next one's base known. When a round learns
    /// nothing, no later round can either.
    fn run(&mut self, constraints: &[Constraint]) {
        let mut projections = Vec::new();
        for constraint in constraints {
            match &constraint.kind {
                ConstraintKind::Equal { expected, actual } => {
                    self.unify(constraint.span, expected, actual)
                }
                ConstraintKind::Field {
                    base,
                    base_span,
                    name,
                    result,
                } => projections.push(Projection {
                    span: constraint.span,
                    base: base.clone(),
                    base_span: *base_span,
                    name: name.clone(),
                    result: result.clone(),
                }),
            }
        }

        while !projections.is_empty() {
            let waiting = projections.len();
            let mut deferred = Vec::new();
            for projection in projections {
                if !self.project(&projection) {
                    deferred.push(projection);
                }
            }
            projections = deferred;
            if projections.len() == waiting {
                break;
            }
        }

        // What is still waiting has nothing left to wait for: the definition
        // never said what the base was, which asks for an annotation.
        for projection in &projections {
            let (base, result, goal) = self.resolved(projection);
            let span = projection.span;
            // The same arm [`Solve::project`] has, and for the same reason:
            // giving up on one link of `x.a.b.c` points the next link's base
            // at `Undecided`, and complaining about that again would name a
            // type `?` the user never wrote, once per link of the chain.
            if matches!(*base, Ty::Undecided) {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &result);
                continue;
            }
            self.fail(
                span,
                Rule::Stuck,
                goal,
                Error {
                    span: projection.base_span,
                    kind: ErrorKind::UnknownBase,
                },
                &[base, result],
            );
        }
    }

    /// Try to read one field out of one base. Returns whether it got anywhere:
    /// a base that is still an unbound variable might yet be learned from
    /// another projection, so it is left for the next round rather than failed
    /// here.
    fn project(&mut self, projection: &Projection) -> bool {
        let (base, result, goal) = self.resolved(projection);
        let span = projection.span;

        match &*base {
            Ty::Struct(fields) => {
                match fields.get(&projection.name).cloned() {
                    Some(ty) => {
                        self.step(span, Rule::Project, goal, Effect::Decomposed);
                        // `result` is the side the context demanded — whatever
                        // equality already put there — so it is the `expected`
                        // one, and a mismatch reads as the context wanting one
                        // type and the field being another.
                        self.depth += 1;
                        self.unify(span, &result, &ty);
                        self.depth -= 1;
                    }
                    None => {
                        let kind = ErrorKind::MissingField {
                            base: base.clone(),
                            field: projection.name.clone(),
                        };
                        self.fail(span, Rule::Project, goal, Error { span, kind }, &[result]);
                    }
                }
                true
            }
            // Whatever made the base undecided was reported where it failed.
            Ty::Undecided => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &result);
                true
            }
            Ty::Var(_) => {
                self.step(span, Rule::Defer, goal, Effect::None);
                false
            }
            // A field is read off a shape, and a name is not one yet. No
            // assumption is needed the way [`Solve::unfold`] needs one: a
            // projection asks about one type rather than a pair, and unfolding
            // one type reaches a shape or runs out of names.
            Ty::Named { .. } => {
                self.step(span, Rule::Unfold, goal, Effect::Decomposed);
                self.depth += 1;
                let unfolded = self.project(&Projection {
                    base: unfold(self.aliases, &base),
                    span: projection.span,
                    base_span: projection.base_span,
                    name: projection.name.clone(),
                    result: projection.result.clone(),
                });
                self.depth -= 1;
                unfolded
            }
            _ => {
                let kind = ErrorKind::NotAStruct { base: base.clone() };
                let error = Error {
                    span: projection.base_span,
                    kind,
                };
                self.fail(span, Rule::Project, goal, error, &[result]);
                true
            }
        }
    }

    /// A projection with its two types resolved as far as the solver has got:
    /// the base and the result the rule about to fire is actually looking at,
    /// rather than what generation wrote down.
    ///
    /// The third value is the same pair again in the shape a [`Step`] records,
    /// since a step's goal is a constraint. Built here rather than by each arm
    /// so that what the reader is shown and what the rule decided on cannot be
    /// resolved to two different points in the solve.
    fn resolved(&self, projection: &Projection) -> (Rc<Ty>, Rc<Ty>, ConstraintKind) {
        let base = self.table.resolve(&projection.base);
        let result = self.table.resolve(&projection.result);
        let goal = ConstraintKind::Field {
            base: base.clone(),
            base_span: projection.base_span,
            name: projection.name.clone(),
            result: result.clone(),
        };
        (base, result, goal)
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the solve continues.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
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
            // One declaration is only ever equal to itself, so this saves an
            // unfolding rather than deciding anything. Two *different*
            // declarations fall through to unfolding and are equal whenever
            // what they stand for is: names are a barrier to unfolding, never
            // to equality.
            (Ty::Named { symbol: a, .. }, Ty::Named { symbol: b, .. }) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            (Ty::Named { .. }, _) | (_, Ty::Named { .. }) => self.unfold(span, goal, &lhs, &rhs),
            (Ty::Nat, Ty::Nat) => self.step(span, Rule::Prim, goal, Effect::None),
            (Ty::Arrow(from1, to1), Ty::Arrow(from2, to2)) => {
                let (from1, to1) = (from1.clone(), to1.clone());
                let (from2, to2) = (from2.clone(), to2.clone());
                self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                self.depth += 1;
                self.unify(span, &from1, &from2);
                self.unify(span, &to1, &to2);
                self.depth -= 1;
            }
            (Ty::Struct(want), Ty::Struct(have)) if same_field_set(want, have) => {
                let pairs: Vec<_> = want
                    .iter()
                    .map(|(name, ty)| (ty.clone(), have[name].clone()))
                    .collect();
                self.step(span, Rule::Struct, goal, Effect::Decomposed);
                self.depth += 1;
                for (want, have) in pairs {
                    self.unify(span, &want, &have);
                }
                self.depth -= 1;
            }
            _ => {
                let kind = ErrorKind::Mismatch {
                    expected: lhs.clone(),
                    actual: rhs.clone(),
                };
                self.fail(
                    span,
                    Rule::Mismatch,
                    goal,
                    Error { span, kind },
                    &[lhs.clone(), rhs.clone()],
                );
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
            Ty::Struct(fields) => {
                for ty in fields.values().cloned().collect::<Vec<_>>() {
                    self.recover(span, &ty);
                }
            }
            Ty::Nat | Ty::Named { .. } | Ty::Bound(_) | Ty::Undecided => {}
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
/// A free function, and the reason generation never needs the variable table:
/// what comes back is built out of primitives, arrows, structs and names, and
/// mentions no [`Ty::Var`] at all.
fn lower_type(mint: &Mint, ty: &Type) -> Rc<Ty> {
    match &ty.tracked {
        TypeKind::Prim(prim) => Rc::new((*prim).into()),
        TypeKind::Ident(symbol) => Rc::new(Ty::Named {
            symbol: *symbol,
            name: mint.name(*symbol).into(),
        }),
        TypeKind::Arrow { from, to } => {
            Rc::new(Ty::Arrow(lower_type(mint, from), lower_type(mint, to)))
        }
        TypeKind::Struct(fields) => Rc::new(Ty::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), lower_type(mint, &field.value)))
                .collect(),
        )),
        TypeKind::Error => Rc::new(Ty::Undecided),
    }
}

/// What a declared type stands for: [`Ty::Named`] replaced by the body it was
/// declared with, and again for as long as that is another name.
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
pub fn unfold(aliases: &IndexMap<Symbol, Rc<Ty>>, ty: &Rc<Ty>) -> Rc<Ty> {
    let mut ty = ty.clone();
    while let Ty::Named { symbol, .. } = &*ty {
        let Some(body) = aliases.get(symbol) else {
            return Rc::new(Ty::Undecided);
        };
        ty = body.clone();
    }
    ty
}

/// Whether two field maps carry exactly the same names, in whatever order.
///
/// Structs are records: `{ x: Nat, y: Nat }` written either way round is one
/// type, so what decides whether two of them line up is the set of names and
/// nothing else. Generic over what the names map to because the two callers
/// have different things there — a struct literal's fields against a written
/// type's, and one semantic type against another — while the rule is the same
/// rule, and was worth writing once.
///
/// Exact equality is deliberate, and this is the one place to change it. A
/// wider struct does not satisfy a narrower demand: `{ x: Nat, y: Nat }` will
/// not pass where `{ x: Nat }` is expected, and a struct literal missing a
/// field is not checked against the annotation field by field. That follows
/// from types here being equated and never ordered — see this module's own
/// header — so admitting width subtyping would be a change to the language
/// rather than to a predicate. If the spec ever adopts it, it is this function
/// that grows a direction, and both call sites inherit it.
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
        Ty::Struct(fields) => Rc::new(Ty::Struct(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), open(ty, fresh)))
                .collect(),
        )),
        Ty::Nat | Ty::Named { .. } | Ty::Var(_) | Ty::Undecided => ty.clone(),
    }
}
