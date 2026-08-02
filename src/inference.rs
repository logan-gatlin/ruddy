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

use std::{
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
};

use indexmap::IndexMap;

use crate::{
    ir::{Program, Term, TermKind, Type, TypeKind},
    symbol::Symbol,
    tracking::Span,
    types::{Binding, Scheme, Slot, Ty, TyVar},
};

#[derive(Debug, Clone)]
pub struct Output {
    /// What each `type` declaration means, with every alias unfolded — the
    /// semantic type behind the written one.
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

/// The case of the solver that fired. One per arm of [`Solve::unify`], plus the
/// three a deferred projection can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// One side is [`Ty::Undecided`], which unifies with anything.
    Absorb,
    /// Both sides are already the same variable.
    Same,
    /// A variable against a type: the only rule that grows the solution.
    Bind,
    /// Two identical primitives.
    Prim,
    /// Two arrows, taken apart into argument and result.
    Arrow,
    /// Two structs with the same field names, taken apart field by field.
    Struct,
    /// Nothing above applied, and the two types cannot be made equal.
    Mismatch,
    /// A projection whose base turned out to be a struct.
    Project,
    /// A projection whose base is still unknown, put back for another round.
    Defer,
    /// A projection put back one round too many: nothing left will explain it.
    Stuck,
    /// Pointing an abandoned goal's result at [`Ty::Undecided`] so that one
    /// failure is not echoed by everything downstream of it.
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
    /// The goal became smaller goals, which follow it one level deeper.
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
    /// the context demanded — an annotation, or the shape an application needs
    /// — and `actual` is what the term turned out to be, which is the order a
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
    /// context demanded — an annotation, or the shape an application needs —
    /// and `actual` is what the term turned out to be.
    Mismatch { expected: Rc<Ty>, actual: Rc<Ty> },
    /// The occurs check fired: a variable would have to contain itself, as in
    /// `fn x => x x`. The cycle is reported rather than constructed, so the
    /// type language stays finite trees.
    Recursive,
    /// Projection out of a type that is not — or never became — a struct. A
    /// base that is still a variable when the solver runs out of constraints
    /// lands here too: nothing in the definition ever said what it was.
    NotAStruct { base: Rc<Ty> },
    /// Projection of a field the struct does not have. The name is carried
    /// rather than left to be read back out of the source, so the message can
    /// be written once here instead of once per reporter.
    MissingField { base: Rc<Ty>, field: String },
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
///
/// It holds no aliases: every written type reaches it already lowered, so
/// there is nothing left for it to unfold.
struct Constrain<'a> {
    table: &'a mut Table,
    /// What each symbol in scope means. Symbols are globally unique, so one
    /// flat map serves every scope at once and nothing is ever popped: a
    /// lambda argument can never collide with a top-level definition.
    env: &'a mut HashMap<Symbol, Binding>,
    out: Vec<Constraint>,
}

/// Pass two: the solver, which sees constraints and never terms.
struct Solve<'a> {
    table: &'a mut Table,
    errors: &'a mut Vec<Error>,
    steps: &'a mut Vec<Step>,
    /// Stamped onto every step this solve records.
    definition: Symbol,
    /// How deep inside a decomposition the solver currently is.
    depth: u32,
}

/// Assign a type to every term in the program, in place, and return the
/// schemes of its top-level definitions.
pub fn infer(program: &mut Program) -> Output {
    let mut table = Table::default();
    let mut env = HashMap::new();
    let mut aliases = IndexMap::new();
    let mut errors = Vec::new();

    // Aliases first: annotations refer to them. Lowering only lets a type name
    // a type declared above it, so one in-order pass unfolds every alias.
    for (symbol, decl) in &program.types {
        let ty = lower_type(&aliases, &decl.value);
        aliases.insert(*symbol, ty);
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
            .map(|annotation| lower_type(&aliases, annotation));

        let mut constrain = Constrain {
            table: &mut table,
            env: &mut env,
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

        Solve {
            table: &mut table,
            errors: &mut errors,
            steps: &mut steps,
            definition: *symbol,
            depth: 0,
        }
        .run(&generated);

        table.level = 0;
        let (scheme, subst) = table.generalize(&ty);
        // With the substitution in hand, resolve every type the walk wrote
        // into the body, so a term's type and its definition's scheme spell
        // the same variable the same way.
        table.zonk_term(&mut decl.value, &subst);
        env.insert(*symbol, Binding::Poly(scheme.clone()));
        schemes.insert(*symbol, scheme);
        constraints.insert(*symbol, generated);
    }

    // Deferred constraints are reported after the equalities that outran them,
    // which is not the order anyone reads a file in. Sorting by position puts
    // that back; the sort is stable, so two complaints about one span keep the
    // order the solver found them in.
    errors.sort_by_key(|error| error.span.start);

    // Error payloads resolve last: a variable in one may have been solved
    // after the error was recorded, and the later knowledge reads better.
    let none = HashMap::new();
    let errors = errors
        .iter()
        .map(|error| Error {
            span: error.span,
            kind: match &error.kind {
                ErrorKind::Mismatch { expected, actual } => ErrorKind::Mismatch {
                    expected: table.zonk(expected, &none),
                    actual: table.zonk(actual, &none),
                },
                ErrorKind::Recursive => ErrorKind::Recursive,
                ErrorKind::NotAStruct { base } => ErrorKind::NotAStruct {
                    base: table.zonk(base, &none),
                },
                ErrorKind::MissingField { base, field } => ErrorKind::MissingField {
                    base: table.zonk(base, &none),
                    field: field.clone(),
                },
            },
        })
        .collect();

    Output {
        aliases,
        schemes,
        constraints,
        steps,
        errors,
    }
}

impl ConstraintKind {
    /// A stable, greppable name for this kind of constraint, the way
    /// [`ErrorKind::code`] names a kind of error. The debugger labels its rows
    /// with it rather than with prose that may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            ConstraintKind::Equal { .. } => "equal",
            ConstraintKind::Field { .. } => "field",
        }
    }
}

impl Rule {
    /// A stable, greppable name for this rule, the way [`ErrorKind::code`]
    /// names a kind of error.
    pub fn code(&self) -> &'static str {
        match self {
            Rule::Absorb => "absorb",
            Rule::Same => "same",
            Rule::Bind => "bind",
            Rule::Prim => "prim",
            Rule::Arrow => "arrow",
            Rule::Struct => "struct",
            Rule::Mismatch => "mismatch",
            Rule::Project => "project",
            Rule::Defer => "defer",
            Rule::Stuck => "stuck",
            Rule::Recover => "recover",
        }
    }
}

/// What a rule does, in a phrase. Said here rather than by whoever is showing
/// the solve, so a reader stepping through it and a reader reading the code are
/// told the same thing.
impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Rule::Absorb => "one side is undecided, which unifies with anything",
            Rule::Same => "already the same variable",
            Rule::Bind => "a variable takes the type it is against",
            Rule::Prim => "the same primitive on both sides",
            Rule::Arrow => "two arrows: argument against argument, result against result",
            Rule::Struct => "two structs: field against field, matched by name",
            Rule::Mismatch => "no rule applies, so the two types cannot be made equal",
            Rule::Project => "the base is a struct, so the field can be read off it",
            Rule::Defer => "the base is still unknown; wait for another round",
            Rule::Stuck => "no round will explain the base now",
            Rule::Recover => "the abandoned result becomes undecided, so nothing echoes it",
        })
    }
}

/// What a step changed, as one line: nothing, a new binding, the goals it broke
/// into, or the complaint it ended in.
impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::None => f.write_str("no change"),
            Effect::Bound { var, ty } => write!(f, "?{var} := {ty}"),
            Effect::Decomposed => f.write_str("broken into smaller goals"),
            Effect::Failed(kind) => kind.fmt(f),
        }
    }
}

impl ErrorKind {
    /// A stable, greppable name for this kind of error. Reporters key on it
    /// rather than on the message, which is prose and may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            ErrorKind::Mismatch { .. } => "type-mismatch",
            ErrorKind::Recursive => "recursive-type",
            ErrorKind::NotAStruct { .. } => "not-a-struct",
            ErrorKind::MissingField { .. } => "missing-field",
        }
    }
}

/// A constraint prints as what it demands, with `~` for "must unify with" —
/// the notation the literature uses, and short enough to sit in a debugger row.
impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Display for ConstraintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintKind::Equal { expected, actual } => write!(f, "{expected} ~ {actual}"),
            ConstraintKind::Field {
                base, name, result, ..
            } => write!(f, "{base}.{name} ~ {result}"),
        }
    }
}

/// What went wrong, in one sentence. Every reporter — the CLI driver, the
/// debugger's diagnostic strip, whatever comes next — prints this rather than
/// matching on the variants itself, so a new one cannot reach a reader phrased
/// two different ways or not at all.
impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Mismatch { expected, actual } => {
                write!(f, "type mismatch: expected `{expected}`, found `{actual}`")
            }
            ErrorKind::Recursive => f.write_str("recursive type"),
            // A base that is still a variable means inference never learned
            // enough, which asks for an annotation rather than a different
            // base — the message has to say which problem it is.
            ErrorKind::NotAStruct { base } => match **base {
                Ty::Var(_) => {
                    f.write_str("cannot infer the type being projected from; annotate it")
                }
                _ => write!(f, "cannot project a field out of `{base}`"),
            },
            ErrorKind::MissingField { base, field } => {
                write!(f, "no field `{field}` on `{base}`")
            }
        }
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
    fn resolve(&self, ty: &Rc<Ty>) -> Rc<Ty> {
        let mut ty = ty.clone();
        let mut visited = HashSet::new();
        while let Ty::Var(v) = &*ty {
            match &self.vars[*v as usize] {
                Slot::Bound(inner) => {
                    if !visited.insert(inner.as_ref() as *const Ty) {
                        panic!("Circular reference in a type");
                    };
                    ty = inner.clone()
                }
                Slot::Unbound { .. } => break,
            }
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
            Ty::Nat | Ty::Bound(_) | Ty::Undecided => false,
        }
    }

    /// Quantify everything in `ty` still unsolved deeper than the current
    /// level. Returns the scheme and the substitution that built it, so the
    /// caller can spell the same variables the same way elsewhere.
    fn generalize(&mut self, ty: &Rc<Ty>) -> (Scheme, HashMap<TyVar, u32>) {
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
            Ty::Nat | Ty::Bound(_) | Ty::Undecided => {}
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
            Ty::Nat | Ty::Bound(_) | Ty::Undecided => ty,
        }
    }

    /// [`zonk`](Self::zonk) applied to every type the walk wrote into a
    /// definition's body, once the definition is solved.
    fn zonk_term(&self, term: &mut Term, subst: &HashMap<TyVar, u32>) {
        term.ty = self.zonk(&term.ty, subst);
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
}

impl Constrain<'_> {
    /// Record that two types have to be the same. The walk's only verb: it
    /// says so and moves on, which is the whole of what generation does.
    fn equal(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
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
                let result = self.table.fresh();
                let wanted = Rc::new(Ty::Arrow(arg.ty.clone(), result.clone()));
                // The function side is the `actual`: applying a non-function
                // should read as "expected an arrow, found what you applied".
                self.equal(span, &wanted, &func.ty.clone());
                result
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
        match (&mut term.kind, &**expected) {
            (TermKind::Fn { arg, body }, Ty::Arrow(from, to)) => {
                let (from, to) = (from.clone(), to.clone());
                self.env.insert(arg.tracked, Binding::Mono(from));
                self.check_term(body, &to);
                term.ty = expected.clone();
            }
            (TermKind::Struct(fields), Ty::Struct(tys))
                if fields.len() == tys.len() && fields.keys().all(|k| tys.contains_key(k)) =>
            {
                for (name, field) in fields.iter_mut() {
                    let want = tys[name].clone();
                    self.check_term(&mut field.value, &want);
                }
                term.ty = expected.clone();
            }
            _ => {
                self.infer_term(term);
                let actual = term.ty.clone();
                self.equal(term.span, expected, &actual);
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
                ConstraintKind::Field { .. } => projections.push(constraint),
            }
        }

        while !projections.is_empty() {
            let waiting = projections.len();
            let mut deferred = Vec::new();
            for constraint in projections {
                if !self.project(constraint) {
                    deferred.push(constraint);
                }
            }
            projections = deferred;
            if projections.len() == waiting {
                break;
            }
        }

        // What is still waiting has nothing left to wait for: the definition
        // never said what the base was, which asks for an annotation.
        for constraint in projections {
            let ConstraintKind::Field {
                base_span, result, ..
            } = &constraint.kind
            else {
                unreachable!("only field constraints are deferred");
            };
            let (base_span, result) = (*base_span, result.clone());
            let goal = self.goal_of(constraint);
            let ConstraintKind::Field { base, .. } = &goal else {
                unreachable!("a field constraint's goal is a field goal");
            };
            let kind = ErrorKind::NotAStruct { base: base.clone() };
            self.error(base_span, kind.clone());
            self.step(constraint.span, Rule::Stuck, goal, Effect::Failed(kind));
            self.recover(constraint.span, &result);
        }
    }

    /// Try to read one field out of one base. Returns whether it got anywhere:
    /// a base that is still an unbound variable might yet be learned from
    /// another projection, so it is left for the next round rather than failed
    /// here.
    fn project(&mut self, constraint: &Constraint) -> bool {
        let ConstraintKind::Field {
            base_span, name, ..
        } = &constraint.kind
        else {
            unreachable!("only field constraints are deferred");
        };
        let (span, base_span, name) = (constraint.span, *base_span, name.clone());
        let goal = self.goal_of(constraint);
        let ConstraintKind::Field { base, result, .. } = &goal else {
            unreachable!("a field constraint's goal is a field goal");
        };
        let (base, result) = (base.clone(), result.clone());

        match &*base {
            Ty::Struct(fields) => {
                match fields.get(&name).cloned() {
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
                            field: name,
                        };
                        self.error(span, kind.clone());
                        self.step(span, Rule::Project, goal, Effect::Failed(kind));
                        self.recover(span, &result);
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
            _ => {
                let kind = ErrorKind::NotAStruct { base: base.clone() };
                self.error(base_span, kind.clone());
                self.step(span, Rule::Project, goal, Effect::Failed(kind));
                self.recover(span, &result);
                true
            }
        }
    }

    /// A constraint with both its types resolved as far as the solver has got:
    /// what the rule about to fire is actually looking at, rather than what
    /// generation wrote down.
    fn goal_of(&self, constraint: &Constraint) -> ConstraintKind {
        match &constraint.kind {
            ConstraintKind::Equal { expected, actual } => ConstraintKind::Equal {
                expected: self.table.resolve(expected),
                actual: self.table.resolve(actual),
            },
            ConstraintKind::Field {
                base,
                base_span,
                name,
                result,
            } => ConstraintKind::Field {
                base: self.table.resolve(base),
                base_span: *base_span,
                name: name.clone(),
                result: self.table.resolve(result),
            },
        }
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
            // was reported where it failed.
            (Ty::Undecided, _) | (_, Ty::Undecided) => {
                self.step(span, Rule::Absorb, goal, Effect::None)
            }
            (Ty::Var(a), Ty::Var(b)) if a == b => self.step(span, Rule::Same, goal, Effect::None),
            (Ty::Var(var), _) => {
                let effect = self.bind(span, *var, &rhs);
                self.step(span, Rule::Bind, goal, effect);
            }
            (_, Ty::Var(var)) => {
                let effect = self.bind(span, *var, &lhs);
                self.step(span, Rule::Bind, goal, effect);
            }
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
            // Fields match by name, not position: structs are records, and
            // `{ x: Nat, y: Nat }` written in either order is the same type.
            (Ty::Struct(want), Ty::Struct(have))
                if want.len() == have.len() && want.keys().all(|k| have.contains_key(k)) =>
            {
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
                self.error(span, kind.clone());
                self.step(span, Rule::Mismatch, goal, Effect::Failed(kind));
            }
        }
    }

    /// Point an unbound variable at a type, unless the type contains the
    /// variable itself — the occurs check that keeps every type a finite
    /// tree. On failure the variable stays unbound; the cycle is reported at
    /// the constraint that would have closed it.
    ///
    /// Returns what it did, for the step that is about to record it. The error
    /// is pushed here rather than by the caller, so that a reporter reading
    /// [`Output::errors`] and a reader stepping through the solve are looking
    /// at the same failure.
    fn bind(&mut self, span: Span, var: TyVar, ty: &Rc<Ty>) -> Effect {
        let Slot::Unbound { level } = self.table.vars[var as usize] else {
            unreachable!("resolve only stops at unbound variables");
        };
        if self.table.occurs(var, level, ty) {
            self.error(span, ErrorKind::Recursive);
            return Effect::Failed(ErrorKind::Recursive);
        }
        self.table.vars[var as usize] = Slot::Bound(ty.clone());
        Effect::Bound {
            var,
            ty: ty.clone(),
        }
    }

    /// Abandon a goal that was just reported: its result becomes
    /// [`Ty::Undecided`], which unifies with everything, so the one complaint
    /// is not echoed by every term downstream of it. No occurs check —
    /// `Undecided` mentions no variables to close a cycle with.
    ///
    /// A step of its own, because it changes the solution: a reader following
    /// the state would otherwise see a variable acquire a value that no rule
    /// they were shown gave it.
    fn recover(&mut self, span: Span, result: &Rc<Ty>) {
        let Ty::Var(var) = &*self.table.resolve(result) else {
            return;
        };
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

    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.errors.push(Error { span, kind });
    }
}

/// The semantic type a written type denotes. Aliases unfold — this is where
/// `Endo` becomes `Nat -> Nat` — and a type that failed to lower becomes
/// [`Ty::Undecided`], which absorbs rather than cascades.
///
/// A free function, and the reason generation never needs the variable table:
/// what comes back is built out of primitives, arrows, structs and other
/// aliases, and mentions no [`Ty::Var`] at all.
fn lower_type(aliases: &IndexMap<Symbol, Rc<Ty>>, ty: &Type) -> Rc<Ty> {
    match &ty.tracked {
        TypeKind::Prim(prim) => Rc::new((*prim).into()),
        TypeKind::Ident(symbol) => aliases
            .get(symbol)
            .cloned()
            .unwrap_or_else(|| Rc::new(Ty::Undecided)),
        TypeKind::Arrow { from, to } => Rc::new(Ty::Arrow(
            lower_type(aliases, from),
            lower_type(aliases, to),
        )),
        TypeKind::Struct(fields) => Rc::new(Ty::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), lower_type(aliases, &field.value)))
                .collect(),
        )),
        TypeKind::Error => Rc::new(Ty::Undecided),
    }
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
        Ty::Nat | Ty::Var(_) | Ty::Undecided => ty.clone(),
    }
}
