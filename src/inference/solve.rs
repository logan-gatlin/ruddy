//! Pass two: solving. See [`Solve`].

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};

use crate::{
    symbol::Symbol,
    tracking::Span,
    types::{Assigned, Formula, Presence, Rest, Row, RowField, Scheme, Sense, Shape, Ty, TyVar},
};

use super::{
    Batch, Constraint, ConstraintKind, DeferredRequirement, Effect, Error, ErrorKind, Goal,
    GuardedArm, GuardedObligation, GuardedOrigin, Known, Named, Origin, Refinement, RefinementFact,
    Rule, Side, Slot, Step, Table,
};

/// What a set of labels says about the ones it does not name.
///
/// The one thing the two shapes still differ in, and so the one thing the shared
/// label rule has to be told. A struct's fields run out where the constructor beside
/// them says they do — `{ x: Nat, ..'r }` is the type `'r` carrying an `x` —
/// and a
/// sum's cases run out where its row's [`Rest`] says they do, because
/// [`Ty::Sum`] is the only type with a case row and there is nowhere else for
/// a sum's tail to live.
///
/// Everything else about matching two sets of labels — the extras each way, the
/// shared ones, the presence clashes, the noun a complaint is worded in — is the
/// same question twice, so it is written once and reaches both through this. Two
/// copies of it that have to agree is a defect, not an implementation choice.
#[derive(Debug, Clone)]
enum Tail {
    Fields(Rest),
    Cases(Rest),
    /// An arrow's effects: what is past them, and the two sides they sit
    /// beside.
    ///
    /// A variant of its own rather than a shape carried beside a [`Rest`],
    /// because what a complaint about it names is different: a sum's labels are
    /// the whole of its constructor, and an arrow's effects are one of three things
    /// the arrow says. The two sides ride along so that
    /// [`rebuild`](Rowed::rebuild) can put the arrow back together — they are
    /// what the goal that failed did *not* decide, so they stand as they were.
    Effects {
        from: Rc<Ty>,
        to: Rc<Ty>,
        rest: Rest,
    },
}

/// A set of labels a goal is about, the type it belongs to, and the tail that
/// says what is not named.
///
/// The three travel together everywhere below [`Solve::labels`]: a complaint
/// names the whole type, and is worded in the nouns of the shape. Paired rather
/// than read back where they are needed, so there is one reading of the shape —
/// the tail's — and nothing further in that could come to disagree with it.
///
/// The type is carried as well as the labels because a complaint is about the
/// type. `(#A 1).x` is a missing *field* on a sum type: the labels that
/// went wrong are that type's fields, and the type beside the word is the whole
/// of what the reader wrote.
#[derive(Clone, Copy)]
struct Rowed<'a> {
    ty: &'a Rc<Ty>,
    labels: &'a IndexMap<String, RowField>,
    tail: &'a Tail,
}

impl Tail {
    /// The variable this tail still is, if it is one. What the extras beside it
    /// are absorbed into, and the whole of what makes a set of labels open.
    fn var(&self) -> Option<TyVar> {
        match self {
            Tail::Fields(Rest::Var(var))
            | Tail::Cases(Rest::Var(var))
            | Tail::Effects {
                rest: Rest::Var(var),
                ..
            } => Some(*var),
            _ => None,
        }
    }

    /// The a variable variable this tail is, if it is one: its spelling and
    /// its id. What a label demanded of it is refused by name.
    fn rigid(&self) -> Option<(Rc<str>, u32)> {
        match self {
            Tail::Fields(Rest::Rigid { id, name })
            | Tail::Cases(Rest::Rigid { id, name })
            | Tail::Effects {
                rest: Rest::Rigid { id, name },
                ..
            } => Some((name.clone(), *id)),
            _ => None,
        }
    }

    /// Whether this tail absorbs whatever it is put against: a failure abandoned
    /// the question, so nothing it decides is worth deciding again.
    fn absorbs(&self) -> bool {
        matches!(
            self,
            Tail::Fields(Rest::Undecided)
                | Tail::Cases(Rest::Undecided)
                | Tail::Effects {
                    rest: Rest::Undecided,
                    ..
                }
        )
    }

    /// Whether this tail allows nothing more at all — every label it does not
    /// name is absent. Neither open nor abandoned is the whole of it: `Nat`
    /// carries the fields written in it and no others, exactly as a closed
    /// row of cases allows the cases it names and no others.
    ///
    /// A rigid is not one, and that is the whole of what it costs the shared
    /// label rule. It allows whatever its annotation's caller picks, so two
    /// sides that name the same labels have not agreed merely by having no
    /// extras between them — the tails still have to meet, and two rigids that
    /// are not the same variable are two promises that cannot both be kept.
    /// Nothing can be *pushed into* one either, which is what
    /// [`Solve::absorb`] rules on separately.
    fn closed(&self) -> bool {
        self.var().is_none() && !self.absorbs() && self.rigid().is_none()
    }

    fn rest(&self) -> &Rest {
        match self {
            Tail::Fields(r) | Tail::Cases(r) | Tail::Effects { rest: r, .. } => r,
        }
    }

    /// Which of the two sets of labels this is a tail of, which is the reading
    /// every complaint under it is worded in.
    fn shape(&self) -> Shape {
        match self {
            Tail::Fields(_) => Shape::Struct,
            Tail::Cases(_) => Shape::Sum,
            Tail::Effects { .. } => Shape::Effect,
        }
    }

    /// This tail carrying `labels`, as the value a variable takes: a whole type
    /// for a struct's fields, a row for a sum's cases or an arrow's effects.
    fn value(&self, labels: IndexMap<String, RowField>) -> Assigned {
        let rest = match self {
            Tail::Fields(rest) | Tail::Cases(rest) | Tail::Effects { rest, .. } => rest,
        };
        Assigned::Row(Rc::new(Row {
            labels,
            rest: rest.clone(),
        }))
    }

    /// This tail as the value a variable standing for it would take: the tail
    /// alone, with no labels in front of it. What a goal about two tails is
    /// worded as, so a binding reads `?1 ~ {}` rather than repeating the whole
    /// types the step above already showed.
    fn bare(&self) -> Assigned {
        self.value(IndexMap::new())
    }
}

impl<'a> Rowed<'a> {
    /// Which of the two sets of labels this is, read off the tail — the one
    /// place the two differ, and so the one place worth reading it from.
    fn shape(&self) -> Shape {
        self.tail.shape()
    }

    /// The type these labels belong to, with the labels replaced. What a
    /// complaint names: the whole type, so that a base printed beside a missing
    /// field reads as the type the reader wrote rather than as the labels the
    /// solver was looking at.
    fn rebuild(&self, labels: IndexMap<String, RowField>) -> Rc<Ty> {
        match self.tail {
            Tail::Fields(rest) => Rc::new(Ty::Struct(Row {
                labels,
                rest: rest.clone(),
            })),
            Tail::Cases(rest) => Rc::new(Ty::Sum(Row {
                labels,
                rest: rest.clone(),
            })),
            // The arrow with its effect row replaced: what the reader wrote,
            // said as far as the solve has got. The two sides stay as they are,
            // since the goal that failed was about the effects alone.
            Tail::Effects { from, to, rest } => Rc::new(Ty::Arrow(
                from.clone(),
                to.clone(),
                Row {
                    labels,
                    rest: rest.clone(),
                },
            )),
        }
    }
}

/// Everything a [`ConstraintKind::Let`] says besides the name it binds: what
/// the name stands for while its value is walked, the level that value was
/// walked at, what its annotation promised, and the two constraint lists.
///
/// One value rather than seven arguments, because it is one thing: a nested
/// binding's whole scoping, read off the constraint that recorded it.
struct Scoping<'a> {
    bound: &'a Rc<Ty>,
    level: u32,
    promised: &'a Formula,
    rigids: &'a [u32],
    value: &'a [Constraint],
    body: &'a [Constraint],
}

struct Rollback {
    stepped: usize,
    stored: usize,
    traced: Vec<usize>,
    known: Known,
}

#[derive(Clone)]
struct SolveRow {
    ty: Rc<Ty>,
    labels: Rc<IndexMap<String, RowField>>,
    tail: Tail,
}

impl SolveRow {
    fn view(&self) -> Rowed<'_> {
        Rowed {
            ty: &self.ty,
            labels: &self.labels,
            tail: &self.tail,
        }
    }
}

struct SolveLabel {
    name: String,
    expected: SolveRow,
    actual: SolveRow,
    want: RowField,
    have: RowField,
}

enum SolveWork {
    Ty(Rc<Ty>, Rc<Ty>, u32),
    Labels(SolveRow, SolveRow, u32),
    Label {
        name: String,
        expected: SolveRow,
        actual: SolveRow,
        want: RowField,
        have: RowField,
        depth: u32,
    },
    FinishCongruent {
        lhs: Rc<Ty>,
        rhs: Rc<Ty>,
        depth: u32,
        reported: usize,
        rollback: Option<Rollback>,
    },
    FinishUnfold {
        assumption: Option<(Symbol, Symbol)>,
        depth: u32,
    },
}

/// Pass two: the solver, which sees constraints and never terms.
pub struct Solve<'a> {
    pub table: &'a mut Table,
    pub errors: &'a mut Vec<Error>,
    pub steps: &'a mut Vec<Step>,
    /// What the declared types stand for, so a goal about a name can become a
    /// goal about a shape. See [`unfold`].
    pub aliases: &'a IndexMap<Symbol, Scheme>,
    /// The declarations that are nominal within themselves: those every
    /// parameter of which survives unfolding, as `Pair`'s and `WithX`'s do and
    /// `Ptr`'s in `type Ptr 'a = Nat` does not.
    ///
    /// The one thing [`Rule::Congruent`] is allowed to decide by. Where every
    /// parameter reaches a position of the body, each argument sits in a
    /// structural position of its own, so unifying two bodies decomposes into
    /// unifying the arguments and the two rules cannot disagree — congruence is
    /// then a shortcut with better spans and better wording, and never a second
    /// answer. Where a parameter is discarded they *do* disagree, and unfolding
    /// is the one that is right, so those fall through to it. See
    /// [`Ty::Named`] and [`ir::relevance`](crate::ir).
    pub nominal: &'a HashSet<Symbol>,
    /// Stamped onto every step this solve records.
    pub definition: Symbol,
    /// How deep inside a decomposition the solver currently is.
    pub depth: u32,
    /// The goals about two declared types that the goals currently open were
    /// reached by unfolding, innermost last.
    ///
    /// Two recursive types are equal when assuming they are equal never leads
    /// to a contradiction, so meeting a goal already on this stack ends it
    /// rather than starting it again. Kept as a stack rather than a set because
    /// the assumption holds for the goals the unfolding broke into and no
    /// further, which is the same scope [`Solve::depth`] tracks.
    ///
    /// Each entry is the two named types themselves — names *and* arguments —
    /// because that is the goal, and an assumption keyed on less than the goal
    /// answers questions it was never asked. Compared with [`Table::alike`]
    /// rather than by identity, since unfolding rebuilds an argument each round
    /// rather than passing the same allocation on. See [`Solve::unfold`].
    pub assumed: Vec<(Rc<Ty>, Rc<Ty>)>,
    /// The scheme each nested `let` currently in scope published, for as long
    /// as its body is being solved. [`Constrain`](super::Constrain)'s
    /// environment, about the one kind of name generation could not decide.
    pub schemes: HashMap<Symbol, Scheme>,
    /// Every scheme published, kept. [`Output::locals`](super::Output::locals).
    pub locals: &'a mut IndexMap<Symbol, Scheme>,
    /// The reachable ordered arm premise currently in force. `None` is the
    /// ordinary solver, including an arm whose premise is contradictory.
    pub guard: Option<Formula>,
    /// The arm report guarded obligations are appended to.
    pub active_refinement: Option<usize>,
    pub refinements: &'a mut Vec<Refinement>,
    /// End of generation-time store slots for this definition. Guarded
    /// structural relations appended while solving participate in later arm
    /// boundaries separately.
    pub generated_end: usize,
}

impl Solve<'_> {
    /// Solve everything generation asked for, in the order it was asked.
    /// Every constraint is an equality, and rows are why that is enough: a
    /// projection's demand is an ordinary type with an open field row, so
    /// nothing has to wait for a later round to know what its base is.
    pub fn run(&mut self, constraints: &[Constraint]) {
        for constraint in constraints {
            let span = constraint.span;
            match &constraint.kind {
                ConstraintKind::Project {
                    base,
                    field,
                    result,
                    base_span,
                } => self.project(span, *base_span, base, field, result),
                ConstraintKind::Equal { expected, actual } => self.unify(span, expected, actual),
                ConstraintKind::Let {
                    symbol,
                    bound,
                    level,
                    promised,
                    rigids,
                    value,
                    body,
                } => self.bind_local(
                    *symbol,
                    &Scoping {
                        bound,
                        level: *level,
                        promised,
                        rigids,
                        value,
                        body,
                    },
                ),
                ConstraintKind::Instance {
                    symbol,
                    ty,
                    requirement,
                } => self.instance(span, *symbol, ty, *requirement),
                ConstraintKind::Match {
                    scrutinee,
                    result,
                    arms,
                    store_end,
                } => self.matched(span, scrutinee, result, arms, *store_end),
                ConstraintKind::Performs {
                    performed,
                    ambient,
                    inside,
                } => self.performs(span, performed, ambient, *inside),
            }
        }
    }

    /// Solve a field projection after exposing only the base's outer constructor.
    fn project(
        &mut self,
        field_span: Span,
        base_span: Span,
        base: &Rc<Ty>,
        field: &str,
        result: &Rc<Ty>,
    ) {
        let base = self.table.resolve(base);
        let exposed = super::unfold(self.aliases, &base);
        match &*exposed {
            Ty::Nat
            | Ty::Int
            | Ty::Real
            | Ty::String
            | Ty::Boolean
            | Ty::Arrow(..)
            | Ty::Sum(_) => {
                let goal = Goal::Type {
                    expected: Rc::new(Ty::unit()),
                    actual: exposed.clone(),
                };
                let error = Error {
                    span: base_span,
                    kind: ErrorKind::NotAStruct { base: exposed },
                };
                self.fail(
                    base_span,
                    Rule::Mismatch,
                    goal,
                    error,
                    &[Assigned::Ty(result.clone())],
                );
            }
            Ty::Rigid { id, name } => {
                let error = Error {
                    span: field_span,
                    kind: ErrorKind::RigidField {
                        shape: Shape::Struct,
                        field: field.to_string(),
                        name: name.clone(),
                        declared: self.table.declared(*id),
                    },
                };
                let goal = Goal::Type {
                    expected: Rc::new(Ty::unit()),
                    actual: exposed.clone(),
                };
                self.fail(
                    field_span,
                    Rule::Mismatch,
                    goal,
                    error,
                    &[Assigned::Ty(result.clone())],
                );
            }
            _ => {
                let want = Rc::new(Ty::Struct(Row {
                    labels: [(field.to_string(), RowField::present(result.clone()))]
                        .into_iter()
                        .collect(),
                    rest: self.table.fresh_row(),
                }));
                self.table.note_lacks(&want);
                self.unify(field_span, &want, &base);
            }
        }
    }

    /// Solve one qualifying match as a finite tree of arm-local constraints.
    /// SAT is read at the fixed arm boundary and only decides whether the
    /// already built effective condition is used as a premise; it never
    /// regenerates a constraint or asks unification to run again.
    fn matched(
        &mut self,
        span: Span,
        scrutinee: &Rc<Ty>,
        result: &Rc<Ty>,
        arms: &[GuardedArm],
        store_end: usize,
    ) {
        let enclosing_guard = self.guard.clone();
        let enclosing_refinement = self.active_refinement;
        let mut guards = Vec::with_capacity(arms.len());
        let mut reports = Vec::with_capacity(arms.len());

        for arm in arms {
            let combined = match &enclosing_guard {
                Some(outer) => outer.clone().and(arm.effective.clone()),
                None => arm.effective.clone(),
            };
            let effective = self.table.resolved(&combined);
            let allowed = self
                .table
                .consistent_known(store_end, self.generated_end)
                .and(effective.clone());
            let reachable = crate::inference::sat::satisfiable(&allowed);

            let mut named = IndexMap::new();
            self.table
                .presence_paths(scrutinee, &mut Vec::new(), &mut named);
            let fields: Vec<(super::PresencePath, Presence)> = named.into_iter().collect();
            let facts = if reachable {
                fields
                    .iter()
                    .filter_map(|(field, presence)| {
                        let literal = self.table.presence_of(presence).formula();
                        if crate::inference::sat::entails(&allowed, &literal) {
                            Some(RefinementFact {
                                field: field.clone(),
                                present: true,
                            })
                        } else if crate::inference::sat::entails(&allowed, &literal.not()) {
                            Some(RefinementFact {
                                field: field.clone(),
                                present: false,
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let report = self.refinements.len();
            self.refinements.push(Refinement {
                definition: self.definition,
                match_span: span,
                arm_span: arm.span,
                raw: self.table.resolved(&arm.raw),
                effective: effective.clone(),
                reachable,
                fields,
                facts,
                obligations: Vec::new(),
            });
            reports.push(report);

            // A contradictory assumption is deliberately not installed. The
            // arm keeps today's ordinary typing and still contributes to the
            // result family.
            // `true -> Q` is just `Q`. Keeping that arm on the ordinary path
            // preserves direct aliases and diagnostics for a sole catch-all
            // while changing no logical requirement.
            let guard = (reachable && !effective.is_true()).then_some(effective);
            guards.push(guard.clone());
            self.guard = guard;
            self.active_refinement = Some(report);
            for requirement in &arm.requirements {
                self.commit_deferred(requirement.clone());
            }
            self.run(&arm.constraints);
        }

        // Build the structural family after every body-local constraint has
        // had its ordinary say. Its finite label unions carry fresh presences;
        // relating those to each arm under that arm's premise is where the
        // input/output relationship is published.
        self.guard = None;
        self.active_refinement = enclosing_refinement;
        let body_types: Vec<Rc<Ty>> = arms.iter().map(|arm| arm.ty.clone()).collect();
        let family = self.family_type(&body_types);
        self.unify(span, result, &family);
        for ((arm, guard), report) in arms.iter().zip(guards).zip(reports) {
            self.guard = guard;
            self.active_refinement = Some(report);
            self.unify(arm.span, &family, &arm.ty);
        }
        self.guard = None;
        self.active_refinement = enclosing_refinement;
        self.alias_result_presences(span, &family, scrutinee);

        self.guard = enclosing_guard;
        self.active_refinement = enclosing_refinement;
    }

    /// A finite structural family for the arm body types. Every label in the
    /// finite union gets one fresh result presence; constructors, tails and payloads
    /// remain ordinary structural types and are checked by the unifications
    /// that follow.
    fn family_type(&mut self, types: &[Rc<Ty>]) -> Rc<Ty> {
        self.family_type_with(types, &mut Vec::new())
    }

    fn family_type_with(&mut self, types: &[Rc<Ty>], unfolding: &mut Vec<Vec<Rc<Ty>>>) -> Rc<Ty> {
        let _ = types
            .first()
            .expect("a structural family has a contributor");
        let resolved: Vec<Rc<Ty>> = types.iter().map(|ty| self.table.resolve(ty)).collect();
        let mut symbols = Vec::new();
        let mut other_concrete = false;
        for ty in &resolved {
            match &**ty {
                Ty::Named { symbol, .. } => {
                    if !symbols.contains(symbol) {
                        symbols.push(*symbol);
                    }
                }
                Ty::Var(_) | Ty::Undecided => {}
                _ => other_concrete = true,
            }
        }
        let repeated = unfolding.iter().any(|earlier| {
            earlier.len() == resolved.len()
                && earlier
                    .iter()
                    .zip(&resolved)
                    .all(|(a, b)| self.table.alike(a, b))
        });
        if !symbols.is_empty() && (symbols.len() > 1 || other_concrete) && !repeated {
            unfolding.push(resolved.clone());
            let expanded: Vec<_> = resolved
                .iter()
                .map(|ty| self.table.unfolded(self.aliases, ty))
                .collect();
            let family = self.family_type_with(&expanded, unfolding);
            unfolding.pop();
            return family;
        }
        let Some(chosen) = resolved
            .iter()
            .find(|ty| !matches!(&***ty, Ty::Var(_) | Ty::Undecided))
        else {
            return self.table.fresh_type();
        };
        Rc::new(match &**chosen {
            Ty::Arrow(..) => {
                let arrows: Vec<_> = resolved
                    .iter()
                    .filter_map(|ty| match &**ty {
                        Ty::Arrow(a, b, e) => Some((a.clone(), b.clone(), e.clone())),
                        _ => None,
                    })
                    .collect();
                let from: Vec<_> = arrows.iter().map(|x| x.0.clone()).collect();
                let to: Vec<_> = arrows.iter().map(|x| x.1.clone()).collect();
                let effects: Vec<_> = arrows.iter().map(|x| x.2.clone()).collect();
                Ty::Arrow(
                    self.family_type_with(&from, unfolding),
                    self.family_type_with(&to, unfolding),
                    self.family_row(&effects, unfolding),
                )
            }
            Ty::Struct(..) => {
                let rows: Vec<_> = resolved
                    .iter()
                    .filter_map(|ty| ty.fields().cloned())
                    .collect();
                Ty::Struct(self.family_row(&rows, unfolding))
            }
            Ty::Sum(..) => {
                let rows: Vec<_> = resolved
                    .iter()
                    .filter_map(|ty| match &**ty {
                        Ty::Sum(r) => Some(r.clone()),
                        _ => None,
                    })
                    .collect();
                Ty::Sum(self.family_row(&rows, unfolding))
            }
            Ty::Named { symbol, name, args } => {
                let merged = (0..args.len())
                    .map(|at| {
                        let xs: Vec<_> = resolved
                            .iter()
                            .filter_map(|ty| match &**ty {
                                Ty::Named { args, .. } => args.get(at).cloned(),
                                _ => None,
                            })
                            .collect();
                        self.family_type_with(&xs, unfolding)
                    })
                    .collect();
                Ty::Named {
                    symbol: *symbol,
                    name: name.clone(),
                    args: merged,
                }
            }
            other => other.clone(),
        })
    }

    fn family_labels(
        &mut self,
        maps: &[IndexMap<String, RowField>],
        unfolding: &mut Vec<Vec<Rc<Ty>>>,
    ) -> IndexMap<String, RowField> {
        let mut names = Vec::new();
        for labels in maps {
            for name in labels.keys() {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
            .into_iter()
            .map(|name| {
                let fields: Vec<RowField> = maps
                    .iter()
                    .filter_map(|labels| labels.get(&name).cloned())
                    .collect();
                let payloads: Vec<Rc<Ty>> = fields
                    .iter()
                    .filter(|field| {
                        !matches!(self.table.presence_of(&field.presence), Presence::Absent)
                    })
                    .map(|field| field.ty.clone())
                    .collect();
                let ty = match payloads.is_empty() {
                    true => Rc::new(Ty::default()),
                    false => self.family_type_with(&payloads, unfolding),
                };
                (
                    name,
                    RowField {
                        presence: self.table.fresh_presence(),
                        ty,
                    },
                )
            })
            .collect()
    }

    fn family_row(&mut self, rows: &[Row], unfolding: &mut Vec<Vec<Rc<Ty>>>) -> Row {
        let _ = rows.first().expect("a row family has a contributor");
        let flat: Vec<Row> = rows.iter().map(|row| self.table.canon(row)).collect();
        let maps: Vec<IndexMap<String, RowField>> =
            flat.iter().map(|row| row.labels.clone()).collect();
        let labels = self.family_labels(&maps, unfolding);
        let rest = if flat.iter().any(|row| matches!(row.rest, Rest::Var(_))) {
            self.table.fresh_row()
        } else {
            flat[0].rest.clone()
        };
        Row { labels, rest }
    }

    /// Where the completed store proves a synthesized result presence is the
    /// same variable as one on the scrutinee, publish that sharing directly in
    /// the type. This is a fixed match-end boundary, not feedback: it only
    /// folds an already entailed alias and never creates another constraint.
    fn alias_result_presences(&mut self, span: Span, result: &Rc<Ty>, scrutinee: &Rc<Ty>) {
        let known = self.table.known();
        if !crate::inference::sat::satisfiable(&known) {
            return;
        }
        let mut outputs = IndexSet::new();
        let mut inputs = IndexSet::new();
        self.table.presences_in(result, &mut outputs);
        self.table.presences_in(scrutinee, &mut inputs);
        // `presences_in` canonicalizes each presence before collecting it, so
        // both sets contain only still-unbound variables. Relating an output to
        // an input binds the output to the input, never the other way round, so
        // the input set remains canonical throughout this loop.
        for output in outputs {
            for input in &inputs {
                let equal = Formula::var(output).iff(Formula::var(*input));
                if crate::inference::sat::entails(&known, &equal) {
                    self.presences(span, &Presence::Var(output), &Presence::Var(*input));
                    break;
                }
            }
        }
    }

    /// Activate one generation-time batch in the exact store slot source order
    /// gave it, wrapping it in the arm implication where the assumption is in
    /// force.
    fn commit_deferred(&mut self, requirement: DeferredRequirement) {
        let DeferredRequirement { at, batch } = requirement;
        let replacement = self.guarded_batch(batch);
        self.table.store.batches[at] = replacement;
        self.table.deferred.remove(&at);
    }

    fn guarded_batch(&mut self, batch: Batch) -> Batch {
        let Some(premise) = self.guard.clone() else {
            return batch;
        };
        let obligation = batch.formula.clone();
        let formula = premise.clone().not().or(obligation.clone());
        let origin = Origin::Guarded(GuardedOrigin {
            premise: premise.clone(),
            obligation: obligation.clone(),
            origin: Box::new(batch.origin),
        });
        self.record_obligation(
            batch.span,
            premise.clone(),
            obligation.clone(),
            formula.clone(),
        );
        Batch {
            definition: batch.definition,
            span: batch.span,
            origin,
            formula,
            flipped: false,
        }
    }

    fn record_obligation(
        &mut self,
        span: Span,
        premise: Formula,
        obligation: Formula,
        formula: Formula,
    ) {
        let at = self
            .active_refinement
            .expect("a guarded obligation belongs to the active arm");
        self.refinements[at].obligations.push(GuardedObligation {
            span,
            premise,
            obligation,
            formula,
        });
    }

    /// An application: make the place it was written in allow what calling the
    /// function may perform.
    ///
    /// R12's opening rule, and the one rule in the solver that widens rather
    /// than equates. A callee whose row still ends in a variable takes the
    /// ambient outright — its tail absorbs the difference — and a closed one is
    /// opened first, so `!Log` performed where `!Log | !IO` is allowed
    /// goes through and the `!IO` stays the ambient's own.
    ///
    /// Which effects the ambient cannot possibly take is decided here rather
    /// than left to the row rule below, because the complaint is a different
    /// complaint: a row that cannot take a label is an extra field, and an
    /// effect nothing will handle is something the reader fixes by widening a
    /// signature or writing a handler. Both readings are R11's.
    fn performs(&mut self, span: Span, performed: &Row, ambient: &Row, inside: bool) {
        let want = self.table.canon(performed);
        let have = self.table.canon(ambient);
        let goal = Goal::Row {
            expected: Rc::new(want.clone()),
            actual: Rc::new(have.clone()),
        };
        let refused = want.labels.iter().find(|(name, field)| {
            if !matches!(self.table.presence_of(&field.presence), Presence::Present) {
                return false;
            }
            match have.labels.get(*name) {
                // Named and settled absent: the ambient says outright that
                // this effect is not performed here.
                Some(there) => matches!(self.table.presence_of(&there.presence), Presence::Absent),
                // Not named at all, and no room past the ones that are.
                None => matches!(have.rest, Rest::Closed | Rest::Bound(_)),
            }
        });
        if let Some((effect, _)) = refused {
            let effect = effect.clone();
            let kind = match inside {
                true => ErrorKind::NotAllowed { effect },
                false => ErrorKind::Unhandled { effect },
            };
            let abandoned = [Assigned::Row(Rc::new(want.clone()))];
            self.fail(span, Rule::Performs, goal, Error { span, kind }, &abandoned);
            return;
        }
        // A closed row is opened with a fresh tail, which is what makes the row
        // an upper bound: the ambient may allow more, and whatever it allows
        // past the callee's own labels lands there. A row still ending in a
        // variable is already open and unifies as it stands.
        let opened = match want.rest {
            Rest::Closed | Rest::Bound(_) => Row {
                labels: want.labels.clone(),
                rest: self.table.fresh_row(),
            },
            _ => want.clone(),
        };
        self.step(span, Rule::Performs, goal, Effect::Decomposed);
        self.depth += 1;
        // The two rows belong to no type the reader wrote — an ambient is the
        // place a term sits in rather than part of anything — so each is named
        // as the arrow it would be the effects of, which is what a complaint
        // about a shared label would quote. Nothing above lets one through:
        // the labels that could clash were ruled on already.
        let unit = || Rc::new(Ty::unit());
        let (left, right) = (
            Tail::Effects {
                from: unit(),
                to: unit(),
                rest: opened.rest.clone(),
            },
            Tail::Effects {
                from: unit(),
                to: unit(),
                rest: have.rest.clone(),
            },
        );
        let (want_ty, have_ty) = (row_ty(&opened), row_ty(&have));
        let expected = SolveRow {
            ty: want_ty,
            labels: Rc::new(opened.labels),
            tail: left,
        };
        let actual = SolveRow {
            ty: have_ty,
            labels: Rc::new(have.labels),
            tail: right,
        };
        for field in self.labels(span, expected, actual) {
            self.field(
                span,
                &field.name,
                field.expected.view(),
                field.actual.view(),
                &field.want,
                &field.have,
            )
            .into_iter()
            .for_each(|(left, right)| self.unify(span, &left, &right));
        }
        self.depth -= 1;
    }

    /// A name bound for the length of a body: solve what its value requires,
    /// generalize, and solve the body with the scheme in scope.
    ///
    /// The order is the whole of it, and it is the order the constraint records
    /// rather than one the walk could have kept: generation had solved nothing
    /// when it met the `let`, so it could no more generalize the value than
    /// name the scheme a use of the name is a copy of.
    ///
    /// The level is set from the constraint rather than counted here, so that a
    /// variable minted while the value is solved is minted where the value was
    /// written — and dropped again for the body, which is one binder further
    /// out. Everything still unbound at or above it is the value's to quantify;
    /// what an enclosing binder owns was pushed below it by
    /// [`Table::demote`](super::Table) as the value was solved.
    ///
    /// The scheme is released when the body ends, the way lowering released the
    /// name. Nothing could reach it afterwards — a symbol is unique — but a
    /// scope that is not closed is a scope that is not a scope.
    fn bind_local(&mut self, symbol: Symbol, scoping: &Scoping<'_>) {
        let &Scoping {
            bound,
            level,
            promised,
            rigids,
            value,
            body,
        } = scoping;
        self.table.level = level;
        self.run(value);
        // R23's closing rule, said about a nested binding on the same terms: a
        // `let` in the middle of a body is generalized exactly as one at the
        // top of a file is.
        self.table.close_effects(bound, level);
        // A nested binding's scheme carries what the store requires of the
        // presences it quantifies, exactly as a definition's does — a `let` in
        // the middle of a body is generalized on the same terms as one at the
        // top of a file. Which includes R10: an annotated binding publishes its
        // annotation's clause and not what its value worked out, since the
        // clause is the contract and the value has been held to it. Silent once
        // something has flipped the store — the cascade rule, and what
        // [`Table::required`](super::Table) does for the unannotated case.
        let required = if !promised.is_true() && !self.table.unsat {
            self.table.resolved(promised)
        } else if let Some(guard) = &self.guard {
            self.table.required_given(bound, guard)
        } else {
            self.table.required(bound)
        };
        // A variable this binding did not declare means nothing in
        // the scheme it is about to publish, exactly as it means nothing in a
        // definition's. See [`Table::escapes`](super::Table).
        self.table.escapes(bound, rigids, self.errors);
        let (scheme, _) = self.table.generalize(bound, level, required);
        self.table.level = level - 1;
        self.locals.insert(symbol, scheme.clone());
        self.schemes.insert(symbol, scheme);
        self.run(body);
        self.schemes.remove(&symbol);
    }

    /// A use of a let-bound name: a fresh copy of the scheme it was published
    /// with, equated with the variable generation minted for the term.
    ///
    /// Indexed rather than looked up, and that is an invariant rather than a
    /// diagnostic: generation emits this only for a name it bound itself, and
    /// the constraint sits inside the body of the `let` that bound it, so a
    /// symbol with no scheme in scope cannot arise.
    fn instance(&mut self, span: Span, symbol: Symbol, ty: &Rc<Ty>, requirement: usize) {
        let scheme = self.schemes[&symbol].clone();
        // At the level of the use site, which is where the table is: a copy is
        // as new as the place it was made, whatever the scheme was generalized
        // at.
        let required = self.table.store.batches.len();
        let copy = self.table.instantiate(span, &scheme);
        let mut batches: Vec<Batch> = self.table.store.batches.drain(required..).collect();
        match batches.pop() {
            Some(mut batch) => {
                // Application generation may have aimed the reserved slot at
                // the argument; retain that source attribution.
                batch.span = self.table.store.batches[requirement].span;
                let replacement = self.guarded_batch(batch);
                self.table.store.batches[requirement] = replacement;
            }
            None => {
                self.table.empty_batches.insert(requirement);
            }
        }
        self.table.deferred.remove(&requirement);
        self.unify(span, &copy, ty);
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the solve continues.
    ///
    /// Two types are equal when they name the same labels with the same
    /// presences and types, and their constructors and row tails agree about everything else. One
    /// function decides a type; there is no separate rule for a struct, because
    /// a struct has an explicit row and non-struct types do not have fields.
    ///
    /// Four steps, in this order:
    ///
    /// 1. [`Ty::Undecided`] on either side absorbs, as it always has.
    /// 2. A *bare* variable — a [`Ty::Var`] carrying no labels — takes the
    ///    whole type it is against, fields and all, which is what keeps
    ///    `fn x => x` inferring `a -> a`. Before unfolding, so that one
    ///    against a declared type takes the type by the name it was written as:
    ///    a definition then reads as its annotation said, and the solver has one
    ///    less thing to unfold later.
    /// 3. A name beside labels is unfolded and the goal asked again. See
    ///    [`Solve::unwrapped`].
    /// 4. Everything else is [`Solve::fielded`]: the labels and the constructor, in the
    ///    order it gives.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
        let lhs = self.table.resolve(expected);
        let rhs = self.table.resolve(actual);
        self.unify_direct(span, lhs, rhs);
    }

    /// Decide a type goal with an explicit continuation stack.
    ///
    /// The stack is not only an equality fast path. An unequal leaf may sit at
    /// the bottom of an arbitrarily deep pair of arrows, named arguments, or
    /// equal-label rows, and reaching that leaf must not borrow one native
    /// frame per enclosing constructor. A row that needs the full label rule,
    /// or a name that needs unfolding, falls back only at that node; recursive
    /// payload goals immediately enter this trampoline again.
    fn unify_direct(&mut self, span: Span, lhs: Rc<Ty>, rhs: Rc<Ty>) {
        let original_depth = self.depth;
        let mut congruences = 0usize;
        // Every recursive type goal stays in this invocation's work list, so
        // assumptions can be indexed as entries are opened below.
        let mut assumption_index: HashMap<(Symbol, Symbol), Vec<usize>> = HashMap::new();
        let mut work = vec![SolveWork::Ty(lhs, rhs, original_depth)];
        while let Some(part) = work.pop() {
            match part {
                SolveWork::Ty(lhs, rhs, depth) => {
                    self.depth = depth;
                    let (lhs, rhs) = (self.table.resolve(&lhs), self.table.resolve(&rhs));
                    let goal = Goal::Type {
                        expected: lhs.clone(),
                        actual: rhs.clone(),
                    };
                    match (&*lhs, &*rhs) {
                        (Ty::Undecided, _) => {
                            self.step(span, Rule::Absorb, goal, Effect::None);
                            self.recover_ty(span, &rhs);
                        }
                        (_, Ty::Undecided) => {
                            self.step(span, Rule::Absorb, goal, Effect::None);
                            self.recover_ty(span, &lhs);
                        }
                        (Ty::Var(a), Ty::Var(b)) if a == b => {
                            self.step(span, Rule::Same, goal, Effect::None)
                        }
                        (Ty::Rigid { id: a, .. }, Ty::Rigid { id: b, .. }) if a == b => {
                            self.step(span, Rule::Same, goal, Effect::None)
                        }
                        (Ty::Var(_), _)
                        | (_, Ty::Var(_))
                        | (Ty::Rigid { .. }, _)
                        | (_, Ty::Rigid { .. }) => {
                            self.types(span, goal, &lhs, &rhs);
                        }
                        (
                            Ty::Named { symbol, args, .. },
                            Ty::Named {
                                symbol: other,
                                args: others,
                                ..
                            },
                        ) if symbol == other
                            && args.len() == others.len()
                            && (args.is_empty() || self.nominal.contains(symbol)) =>
                        {
                            if args.is_empty() {
                                self.step(span, Rule::Same, goal, Effect::None);
                            } else {
                                let reported = self.errors.len();
                                let rollback = (congruences == 0).then(|| Rollback {
                                    stepped: self.steps.len(),
                                    stored: self.table.store.batches.len(),
                                    traced: self
                                        .refinements
                                        .iter()
                                        .map(|refinement| refinement.obligations.len())
                                        .collect(),
                                    known: self.table.snapshot(),
                                });
                                congruences += 1;
                                self.step(span, Rule::Congruent, goal, Effect::Decomposed);
                                work.push(SolveWork::FinishCongruent {
                                    lhs: lhs.clone(),
                                    rhs: rhs.clone(),
                                    depth,
                                    reported,
                                    rollback,
                                });
                                work.extend(args.iter().zip(others.iter()).rev().map(
                                    |(left, right)| {
                                        SolveWork::Ty(left.clone(), right.clone(), depth + 1)
                                    },
                                ));
                            }
                        }
                        (Ty::Nat, Ty::Nat)
                        | (Ty::Int, Ty::Int)
                        | (Ty::Real, Ty::Real)
                        | (Ty::String, Ty::String)
                        | (Ty::Boolean, Ty::Boolean) => {
                            self.step(span, Rule::Prim, goal, Effect::None);
                        }
                        (Ty::Arrow(from, to, effects), Ty::Arrow(other, result, performs)) => {
                            let (want, have) =
                                (self.table.canon(effects), self.table.canon(performs));
                            self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                            let left = SolveRow {
                                ty: lhs.clone(),
                                labels: Rc::new(want.labels),
                                tail: Tail::Effects {
                                    from: from.clone(),
                                    to: to.clone(),
                                    rest: want.rest,
                                },
                            };
                            let right = SolveRow {
                                ty: rhs.clone(),
                                labels: Rc::new(have.labels),
                                tail: Tail::Effects {
                                    from: other.clone(),
                                    to: result.clone(),
                                    rest: have.rest,
                                },
                            };
                            work.push(SolveWork::Labels(left, right, depth + 1));
                            work.push(SolveWork::Ty(to.clone(), result.clone(), depth + 1));
                            work.push(SolveWork::Ty(from.clone(), other.clone(), depth + 1));
                        }
                        (Ty::Struct(fields), Ty::Struct(others))
                            if fields.labels.is_empty()
                                && others.labels.is_empty()
                                && matches!(fields.rest, Rest::Closed)
                                && matches!(others.rest, Rest::Closed) => {}
                        (Ty::Struct(fields), Ty::Struct(others)) => {
                            let (want, have) = (self.table.canon(fields), self.table.canon(others));
                            let expected = Rc::new(Ty::Struct(want.clone()));
                            let actual = Rc::new(Ty::Struct(have.clone()));
                            self.step(
                                span,
                                Rule::Struct,
                                Goal::Type { expected, actual },
                                Effect::Decomposed,
                            );
                            work.push(SolveWork::Labels(
                                SolveRow {
                                    ty: lhs,
                                    labels: Rc::new(want.labels),
                                    tail: Tail::Fields(want.rest),
                                },
                                SolveRow {
                                    ty: rhs,
                                    labels: Rc::new(have.labels),
                                    tail: Tail::Fields(have.rest),
                                },
                                depth + 1,
                            ));
                        }
                        (Ty::Sum(cases), Ty::Sum(others)) => {
                            let (want, have) = (self.table.canon(cases), self.table.canon(others));
                            let expected = Rc::new(Ty::Sum(want.clone()));
                            let actual = Rc::new(Ty::Sum(have.clone()));
                            self.step(
                                span,
                                Rule::Sum,
                                Goal::Type { expected, actual },
                                Effect::Decomposed,
                            );
                            work.push(SolveWork::Labels(
                                SolveRow {
                                    ty: lhs,
                                    labels: Rc::new(want.labels),
                                    tail: Tail::Cases(want.rest),
                                },
                                SolveRow {
                                    ty: rhs,
                                    labels: Rc::new(have.labels),
                                    tail: Tail::Cases(have.rest),
                                },
                                depth + 1,
                            ));
                        }
                        (Ty::Named { .. }, _) | (_, Ty::Named { .. }) => {
                            let pair = match (&*lhs, &*rhs) {
                                (
                                    Ty::Named { symbol: left, .. },
                                    Ty::Named { symbol: right, .. },
                                ) => Some(((*left, *right), (lhs.clone(), rhs.clone()))),
                                _ => None,
                            };
                            let already = pair.as_ref().is_some_and(|(key, _)| {
                                assumption_index.get(key).is_some_and(|entries| {
                                    entries.iter().fold(false, |found, at| {
                                        let (left, right) = &self.assumed[*at];
                                        found
                                            | (self.table.alike(left, &lhs)
                                                & self.table.alike(right, &rhs))
                                    })
                                })
                            });
                            match already {
                                true => self.step(span, Rule::Assume, goal, Effect::None),
                                false => {
                                    let exposed_left = self.table.unfolded(self.aliases, &lhs);
                                    let exposed_right = self.table.unfolded(self.aliases, &rhs);
                                    self.step(span, Rule::Unfold, goal, Effect::Decomposed);
                                    let assumption = pair.map(|(key, pair)| {
                                        let at = self.assumed.len();
                                        self.assumed.push(pair);
                                        assumption_index.entry(key).or_default().push(at);
                                        key
                                    });
                                    work.push(SolveWork::FinishUnfold { assumption, depth });
                                    work.push(SolveWork::Ty(
                                        exposed_left,
                                        exposed_right,
                                        depth + 1,
                                    ));
                                }
                            }
                        }
                        _ => {
                            self.types(span, goal, &lhs, &rhs);
                        }
                    }
                }
                SolveWork::Labels(lhs, rhs, depth) => {
                    self.depth = depth;
                    work.extend(self.labels(span, lhs, rhs).into_iter().rev().map(|field| {
                        SolveWork::Label {
                            name: field.name,
                            expected: field.expected,
                            actual: field.actual,
                            want: field.want,
                            have: field.have,
                            depth,
                        }
                    }));
                }
                SolveWork::Label {
                    name,
                    expected,
                    actual,
                    want,
                    have,
                    depth,
                } => {
                    self.depth = depth;
                    if let Some((left, right)) =
                        self.field(span, &name, expected.view(), actual.view(), &want, &have)
                    {
                        work.push(SolveWork::Ty(left, right, depth));
                    }
                }
                SolveWork::FinishCongruent {
                    lhs,
                    rhs,
                    depth,
                    reported,
                    rollback,
                } => {
                    self.depth = depth;
                    congruences -= 1;
                    if self.errors.len() > reported
                        && let Some(rollback) = rollback
                    {
                        self.errors.truncate(reported);
                        self.steps.truncate(rollback.stepped);
                        self.table.store.batches.truncate(rollback.stored);
                        for (refinement, obligations) in
                            self.refinements.iter_mut().zip(rollback.traced)
                        {
                            refinement.obligations.truncate(obligations);
                        }
                        self.table.restore(rollback.known);
                        let goal = Goal::Type {
                            expected: lhs.clone(),
                            actual: rhs.clone(),
                        };
                        self.mismatch(span, goal, &lhs, &rhs);
                    }
                }
                SolveWork::FinishUnfold { assumption, depth } => {
                    self.depth = depth;
                    if let Some(key) = assumption {
                        self.assumed.pop();
                        let entries = assumption_index
                            .get_mut(&key)
                            .expect("an open assumption is indexed");
                        entries.pop();
                    }
                }
            }
        }
        self.depth = original_depth;
    }

    /// Decide two types by their labels and their constructors and row tails.
    ///
    /// A type with no struct labels on either side is its constructor and nothing else, so
    /// the constructor's own rule is the whole step and the trace reads exactly as it
    /// did before fields were a property of every type: `Nat` against `Nat` is
    /// one [`Rule::Prim`] and nothing more.
    ///
    /// Where either side carries a label there is a [`Rule::Struct`] over the
    /// whole of it, with the label steps and the row-tail step one level under. Two
    /// identical closed row tails record no step of their own — they are already the
    /// same thing — which is what keeps a struct against a struct reading byte
    /// for byte as it always has.
    ///
    /// Which of the two halves goes first is decided by whether a row-tail variable
    /// is involved, and only there does the order differ from what it was. A
    /// row-tail variable is what the extras have to be absorbed into, so the labels
    /// have to be settled before it can be told what it stands for: `1.x`
    /// records the fields and their failure, and no longer binds the base's row tail
    /// first. Where neither row tail is a variable there is nothing to absorb into
    /// and the constructors and row tails are the larger question — two types that cannot be equal at
    /// all are a mismatch of what the reader wrote, not a field missing from a
    /// type that was never the right one — so those are decided first and the
    /// labels only if they agreed.
    ///
    /// Either way the two halves are one goal, so a failure in one abandons the
    /// other: a row tail bound inconsistently with its labels
    /// beside it is a type nothing can be. See [`Solve::abandon`].
    fn types(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) -> bool {
        match (&**lhs, &**rhs) {
            (Ty::Var(var), _) => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(rhs.clone()));
                true
            }
            (_, Ty::Var(var)) => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(lhs.clone()));
                true
            }
            // A variable is equal to itself and to nothing else. Equal rigids
            // and equal unbound variables were discharged by the iterative
            // direct path before reaching this matcher.
            // Meeting anything else is the promise broken — except an unbound
            // variable, which takes the rigid as it would take any other type
            // and is left to the binding rules below. A declared name is not
            // looked through first: what the reader wrote is the name, and
            // unfolding it would quote them a shape they never put on the page.
            (Ty::Rigid { name, id }, _) => {
                let (name, id) = (name.clone(), *id);
                self.rigid_broken(span, goal, name, id, Sense::Type, rhs)
            }
            (_, Ty::Rigid { name, id }) => {
                let (name, id) = (name.clone(), *id);
                self.rigid_broken(span, goal, name, id, Sense::Type, lhs)
            }
            // Nothing applies, and the two types cannot be made equal.
            _ => self.mismatch(span, goal, lhs, rhs),
        }
    }

    /// The two types cannot be made equal: report it, abandon both of them, and
    /// say the field rows are not worth deciding.
    ///
    /// Two arms above end here — no rule applying, and a congruence whose
    /// arguments disagreed, which is the same answer reached after putting back
    /// everything the attempt did. One function because it is one act, said the
    /// same way both times: a reader who meets the two complaints should not have
    /// to check whether they abandon the same things.
    ///
    /// Named as the two whole types rather than as their constructors and row tails: a `Nat` against
    /// `{ x: Nat }` is a mismatch of what the reader wrote, and the constructors and row tails alone
    /// would quote them a unit they never mentioned.
    /// A variable met something it cannot be: report it, abandon
    /// what the goal was about, and say the labels beside them are not worth
    /// deciding.
    ///
    /// [`mismatch`](Self::mismatch) about the one thing that is not a mismatch
    /// of two written types. The reader is not shown two types that failed to
    /// agree — one of them is a name standing in for a choice they handed to
    /// their caller — so the complaint names what the expression turned out to
    /// be and what it had promised to be, and points back at the declaration
    /// that promised it.
    ///
    /// Recorded as [`Rule::Mismatch`], which is what it is: no rule applied.
    #[allow(clippy::too_many_arguments)]
    fn rigid_broken(
        &mut self,
        span: Span,
        goal: Goal,
        name: Rc<str>,
        id: u32,
        sense: Sense,
        found: &Rc<Ty>,
    ) -> bool {
        let error = Error {
            span,
            kind: ErrorKind::RigidBroken {
                found: found.clone(),
                name,
                sense,
                declared: self.table.declared(id),
            },
        };
        let abandoned = [Assigned::Ty(found.clone())];
        self.fail(span, Rule::Mismatch, goal, error, &abandoned);
        false
    }

    fn mismatch(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) -> bool {
        let error = Error {
            span,
            kind: ErrorKind::Mismatch {
                expected: lhs.clone(),
                actual: rhs.clone(),
            },
        };
        let abandoned = [Assigned::Ty(lhs.clone()), Assigned::Ty(rhs.clone())];
        self.fail(span, Rule::Mismatch, goal, error, &abandoned);
        false
    }

    /// Make two sets of labels the same, whichever of the two shapes they are.
    ///
    /// The rule written once and reaching both, with the one thing that differs
    /// abstracted into [`Tail`]: labels only one side names flow into what the
    /// other side allows beyond its own, and the labels both name are decided
    /// one by one. `shape` decides the nouns a complaint is worded in and
    /// nothing else.
    ///
    /// The tails go first, the shared labels after. The other order would let
    /// a shared label's own unification bind one of the tails behind the
    /// flattened copy this function is holding — a field's type can mention
    /// its own tail — and an act performed on a stale tail is an act performed
    /// on the wrong type.
    fn labels(&mut self, span: Span, lhs: SolveRow, rhs: SolveRow) -> Vec<SolveLabel> {
        let expected = lhs.view();
        let actual = rhs.view();
        let goal = Goal::Type {
            expected: expected.ty.clone(),
            actual: actual.ty.clone(),
        };

        let mut only_want: IndexMap<String, RowField> = expected
            .labels
            .iter()
            .filter(|(name, _)| !actual.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();
        let mut only_have: IndexMap<String, RowField> = actual
            .labels
            .iter()
            .filter(|(name, _)| !expected.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();

        if let (Some(a), Some(b)) = (expected.tail.var(), actual.tail.var())
            && a == b
            && !(only_want.is_empty() && only_have.is_empty())
        {
            let guarded = self.guard.is_some();
            let absent = |field: &RowField| {
                matches!(self.table.presence_of(&field.presence), Presence::Absent)
            };
            if guarded || only_want.values().all(&absent) && only_have.values().all(&absent) {
                let labels: Vec<String> =
                    only_want.keys().chain(only_have.keys()).cloned().collect();
                self.table.forbidden(a, expected.shape(), labels);
                if guarded {
                    for field in only_want.values().chain(only_have.values()) {
                        let presence = self.table.presence_of(&field.presence);
                        self.presences(span, &Presence::Absent, &presence);
                    }
                }
                only_want.clear();
                only_have.clear();
            } else {
                let error = Error {
                    span,
                    kind: ErrorKind::Recursive,
                };
                let abandoned = [
                    expected.tail.value(expected.labels.clone()),
                    actual.tail.value(actual.labels.clone()),
                ];
                self.fail(span, Rule::Occurs, goal, error, &abandoned);
                return Vec::new();
            }
        }

        match (only_want.is_empty(), only_have.is_empty()) {
            (true, true) => {
                if !(expected.tail.closed() && actual.tail.closed()) {
                    self.tails(span, expected.tail, actual.tail);
                }
            }
            (true, false) => self.absorb(span, Side::Expected, only_have, actual.tail, expected),
            (false, true) => self.absorb(span, Side::Actual, only_want, expected.tail, actual),
            (false, false) => {
                let rest = self.fresh_tail(expected.shape());
                self.absorb(span, Side::Expected, only_have, &rest, expected);
                self.absorb(span, Side::Actual, only_want, &rest, actual);
            }
        }

        let shared: Vec<_> = expected
            .labels
            .iter()
            .filter_map(|(name, field)| {
                actual
                    .labels
                    .get(name)
                    .map(|other| (name.clone(), field.clone(), other.clone()))
            })
            .collect();
        shared
            .into_iter()
            .map(|(name, want, have)| SolveLabel {
                name,
                expected: lhs.clone(),
                actual: rhs.clone(),
                want,
                have,
            })
            .collect()
    }

    /// A variable standing for whatever a set of labels of this shape allows
    /// beyond the ones it names: a fresh tail for a struct's fields, a fresh
    /// tail for a sum's cases. One variable table and one sort per position, as
    /// everywhere else.
    fn fresh_tail(&mut self, shape: Shape) -> Tail {
        match shape {
            Shape::Struct => Tail::Fields(self.table.fresh_row()),
            Shape::Sum => Tail::Cases(self.table.fresh_row()),
            // The two sides only matter to a complaint that names the whole
            // arrow, and a *fresh* tail is one nothing has yet gone wrong
            // about: it stands for what the two sides allow beyond the labels
            // named, which is a row and nothing else.
            Shape::Effect => Tail::Effects {
                from: Rc::new(Ty::unit()),
                to: Rc::new(Ty::unit()),
                rest: self.table.fresh_row(),
            },
        }
    }

    /// Decide what two tails allow beyond the labels their sides name, in
    /// whichever sort they are: the row-tail rule for a struct's fields, and
    /// [`Solve::rests`] for a sum's cases.
    ///
    /// The one place the shared rule has to know which shape it is on, and the
    /// reason it is one line: everything above this is the same question about
    /// two sets of labels, and everything below it is a question the two shapes
    /// answer with different machinery.
    fn tails(&mut self, span: Span, lhs: &Tail, rhs: &Tail) {
        self.rests(span, lhs.rest(), rhs.rest(), lhs.shape());
    }

    /// Make two of a sum's tails the same tail: a variable takes the other, and
    /// an undecided one absorbs it. The struct's half of [`Solve::tails`] is the
    /// row-tail rule; this is the other.
    ///
    /// Only reached where the two rows name the same cases, so there is
    /// nothing to push into either side and the question is what the two allow
    /// beyond them. Each tail is flattened first, because one of them may be a
    /// tail this very rule has just closed — [`Solve::absorb`] shuts both sides
    /// of a pair of closed rows, one after the other, and the second call must
    /// see what the first decided rather than bind the variable twice.
    ///
    /// A variable takes the whole of what is past the other tail, labels
    /// included. There are none to take where the callers reach this, but
    /// binding the flattened row rather than its bare end is what makes that a
    /// fact about the callers instead of something this has to be told.
    fn rests(&mut self, span: Span, lhs: &Rest, rhs: &Rest, shape: Shape) {
        let want = Rc::new(self.table.canon(&Row::of(lhs.clone())));
        let have = Rc::new(self.table.canon(&Row::of(rhs.clone())));
        let goal = Goal::Row {
            expected: want.clone(),
            actual: have.clone(),
        };
        match (&want.rest, &have.rest) {
            (Rest::Undecided, _) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover_row(span, &have);
            }
            (_, Rest::Undecided) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover_row(span, &want);
            }
            (Rest::Var(a), Rest::Var(b)) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            // The row-tail rule's arms about a sum's rest, and the same rule: a
            // declared rest is equal to itself, takes an unbound variable as
            // any row would, and breaks its promise against anything else.
            (Rest::Rigid { id: a, .. }, Rest::Rigid { id: b, .. }) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            (Rest::Rigid { name, id }, other) if !matches!(other, Rest::Var(_)) => {
                let (name, id) = (name.clone(), *id);
                let (sense, found) = rest_found(&have, shape);
                self.rigid_broken(span, goal, name, id, sense, &found);
            }
            (other, Rest::Rigid { name, id }) if !matches!(other, Rest::Var(_)) => {
                let (name, id) = (name.clone(), *id);
                let (sense, found) = rest_found(&want, shape);
                self.rigid_broken(span, goal, name, id, sense, &found);
            }
            (Rest::Var(var), _) => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Row(have));
            }
            (_, Rest::Var(var)) => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Row(want));
            }
            // Two tails that are already the same thing: two closed ones, which
            // is what a row closed from both sides comes to.
            _ => self.step(span, Rule::Same, goal, Effect::None),
        }
    }

    /// A set of labels as it reads at this moment: every presence fixed at what
    /// it has been decided to be — still open becomes undecided, which prints as
    /// the mark the user would have written and which nothing downstream will
    /// rewrite — and the whole put back into the type the labels belong to.
    ///
    /// Already flattened by the time it arrives: [`Solve::labels`] is handed a
    /// sum's cases canonical and a type's fields have no chain to follow.
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
    /// Label types and whatever is left of the tail stay live: those are not
    /// decided by the failing goal's siblings, and later knowledge about them
    /// is knowledge the reader wants.
    fn frozen(&self, row: Rowed<'_>) -> Rc<Ty> {
        let labels = row
            .labels
            .iter()
            .map(|(name, field)| {
                let presence = match self.table.presence_of(&field.presence) {
                    decided @ (Presence::Present | Presence::Absent) => decided,
                    _ => Presence::Undecided,
                };
                let field = RowField {
                    presence,
                    ty: field.ty.clone(),
                };
                (name.clone(), field)
            })
            .collect();
        row.rebuild(labels)
    }

    /// Decide one label both rows name: whether it is there must agree, and
    /// while it may be, the types must too. A clash of the constants is
    /// worded as the label the actual side is missing or the extra one it
    /// has, never as `present` against `absent` — the label's name is known
    /// here, and the complaint should name it.
    ///
    /// Which noun the rule is read in comes off the tail the side carries, the
    /// way [`Solve::absorb`]'s does: the shape is what the two sides were
    /// matched on, so there is one reading of it and nothing here to drift.
    fn field(
        &mut self,
        span: Span,
        name: &str,
        expected: Rowed<'_>,
        actual: Rowed<'_>,
        want: &RowField,
        have: &RowField,
    ) -> Option<(Rc<Ty>, Rc<Ty>)> {
        let shape = expected.shape();
        let p1 = self.table.presence_of(&want.presence);
        let p2 = self.table.presence_of(&have.presence);
        let goal = Goal::Presence {
            expected: p1.clone(),
            actual: p2.clone(),
        };
        if self.guard.is_some() {
            self.guarded_presence(span, &p1, &p2);
            // Payload typing is not dependent on the branch premise. An
            // explicitly absent slot carries no payload to compare; everything
            // else keeps ordinary structural compatibility.
            return match (&p1, &p2) {
                (Presence::Absent, _) | (_, Presence::Absent) => None,
                _ => Some((want.ty.clone(), have.ty.clone())),
            };
        }
        match (&p1, &p2) {
            // The common case: certainly there on both sides, so presence
            // has nothing to say and the types carry the whole question.
            (Presence::Present, Presence::Present) => Some((want.ty.clone(), have.ty.clone())),
            // One side must have the label and the other cannot. Which of the two
            // complaints that is, is which way round they are, and nothing else
            // about the failure differs: the same rule, the same goal, and both
            // label types abandoned either way. So it is one arm — the label is
            // missing from the side that cannot have it, and the type named is
            // the side that says what is allowed.
            (Presence::Present, Presence::Absent) | (Presence::Absent, Presence::Present) => {
                let field = name.to_string();
                let kind = match p1 {
                    Presence::Present => ErrorKind::MissingField {
                        shape,
                        base: self.frozen(actual),
                        field,
                    },
                    _ => ErrorKind::ExtraField {
                        shape,
                        base: self.frozen(expected),
                        field,
                    },
                };
                let abandoned = [Assigned::Ty(want.ty.clone()), Assigned::Ty(have.ty.clone())];
                self.fail(
                    span,
                    Rule::Presence { shape },
                    goal,
                    Error { span, kind },
                    &abandoned,
                );
                None
            }
            // At least one side is still a variable or undecided: the
            // presences unify as anything else does, and the types follow
            // unless the label just settled absent — an absent label's type
            // slot means nothing, and constraining it would reject rows that
            // agree.
            _ => {
                self.presences(span, &p1, &p2);
                (!matches!(self.table.presence_of(&p1), Presence::Absent))
                    .then(|| (want.ty.clone(), have.ty.clone()))
            }
        }
    }

    /// Make two presences agree.
    ///
    /// Only ever reached with at least one side still a variable or undecided:
    /// [`Solve::field`] decides every clash of the constants itself, where the
    /// label's name is known and the complaint can name it. So the last arm is
    /// two constants that already agree, which is [`Rule::Same`] like any other
    /// pair that is already the same thing — said as `Same` rather than as
    /// [`Rule::Presence`] because that rule is worded about the label whose
    /// presence it is deciding, and here there is no row in sight to have one.
    fn presences(&mut self, span: Span, lhs: &Presence, rhs: &Presence) {
        if self.guard.is_some() {
            self.guarded_presence(span, lhs, rhs);
            return;
        }
        let goal = Goal::Presence {
            expected: lhs.clone(),
            actual: rhs.clone(),
        };
        match (lhs, rhs) {
            (Presence::Undecided, _) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &Assigned::Presence(rhs.clone()));
            }
            (_, Presence::Undecided) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover(span, &Assigned::Presence(lhs.clone()));
            }
            (Presence::Var(a), Presence::Var(b)) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            (Presence::Var(var), other) => {
                let (var, other) = (*var, other.clone());
                self.assign(span, goal, var, Assigned::Presence(other));
            }
            (other, Presence::Var(var)) => {
                let (var, other) = (*var, other.clone());
                self.assign(span, goal, var, Assigned::Presence(other));
            }
            _ => self.step(span, Rule::Same, goal, Effect::None),
        }
    }

    /// Record presence equality under the active arm premise without binding
    /// either presence globally.
    fn guarded_presence(&mut self, span: Span, lhs: &Presence, rhs: &Presence) {
        let premise = self
            .guard
            .clone()
            .expect("guarded presence equality has an active premise");
        let lhs = self.table.presence_of(lhs);
        let rhs = self.table.presence_of(rhs);
        let obligation = match (&lhs, &rhs) {
            (Presence::Undecided, _) | (_, Presence::Undecided) => Formula::True,
            (Presence::Present, Presence::Present) | (Presence::Absent, Presence::Absent) => {
                Formula::True
            }
            (Presence::Present, Presence::Absent) | (Presence::Absent, Presence::Present) => {
                Formula::False
            }
            (Presence::Present, other) | (other, Presence::Present) => other.formula(),
            (Presence::Absent, other) | (other, Presence::Absent) => other.formula().not(),
            _ => lhs.formula().iff(rhs.formula()),
        };
        let formula = premise.clone().not().or(obligation.clone());
        let labels = self
            .active_refinement
            .map(|at| {
                self.refinements[at]
                    .fields
                    .iter()
                    .map(|(path, presence)| (super::display_presence_path(path), presence.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let origin = Origin::Guarded(GuardedOrigin {
            premise: premise.clone(),
            obligation: obligation.clone(),
            origin: Box::new(Origin::Refinement(Named { labels })),
        });
        if !obligation.is_true() {
            self.table.require(span, origin, formula.clone());
        }
        let goal = Goal::Presence {
            expected: lhs,
            actual: rhs,
        };
        self.step(
            span,
            Rule::Refine,
            goal,
            Effect::Guarded {
                premise: premise.clone(),
                obligation: obligation.clone(),
            },
        );
        self.record_obligation(span, premise, obligation, formula);
    }

    /// Push the labels only one side names into what the other side allows
    /// beyond its own, to continue as `rest`. `side` says which side of the goal
    /// the absorbing tail sits on, so that every act performed on its behalf
    /// keeps the direction the constraint was worded in, and `base` is the side
    /// itself — the type a complaint names, and the tail doing the absorbing.
    ///
    /// An open tail takes the labels whole — one binding, occurs-checked like
    /// any other. A closed tail takes nothing: each label must turn out
    /// absent, one that certainly is not is a missing or extra label by
    /// `side`, and whatever would have continued as `rest` is decided against
    /// this side's own tail, which allows nothing more either.
    ///
    /// Both complaints are reachable from one projection, and `side` is the
    /// whole of the difference. `1.x` and `let b : Nat -> Nat = fn p => p.x`
    /// put the projection's demand on the expected side and read ``no field `x`
    /// on `Nat` `` — [`ErrorKind::MissingField`], and the common case, because a
    /// demand is usually what a type is being checked against. Naming the
    /// definition instead, as `let g = fn p => p.x` then `let b : Nat -> Nat =
    /// g` does, puts the demand on the actual side and the annotation on the
    /// expected one, and the same refusal comes out as
    /// [`ErrorKind::ExtraField`]. Nothing about the projection decides that;
    /// where the annotation sits does.
    fn absorb(
        &mut self,
        span: Span,
        side: Side,
        extras: IndexMap<String, RowField>,
        rest: &Tail,
        base: Rowed<'_>,
    ) {
        let shape = base.shape();
        let tail = base.tail;
        let rigid = tail.rigid();
        // A rigid takes nothing: it is not closed — it allows whatever its
        // caller picks — but there is no variable to put the extras in either,
        // so the labels are ruled on one at a time below, exactly as they are
        // against a row that allows nothing more.
        if rigid.is_none() && !tail.closed() {
            let value = rest.value(extras);
            let (expected, actual) = match side {
                Side::Expected => (tail.bare(), value.clone()),
                Side::Actual => (value.clone(), tail.bare()),
            };
            let goal = Goal::Row {
                expected: Rc::new(expected.as_row()),
                actual: Rc::new(actual.as_row()),
            };
            // Not a second label rule: what the extras are was decided above,
            // and this is the act of putting them somewhere. A variable takes
            // them; an undecided tail absorbs them, and everything they would
            // have decided is abandoned with them.
            match tail.var() {
                Some(var) => self.assign(span, goal, var, value),
                None => {
                    self.step(span, Rule::Absorb, goal, Effect::None);
                    self.recover(span, &value);
                }
            }
            return;
        }

        for (name, field) in &extras {
            let presence = self.table.presence_of(&field.presence);
            // A label certainly there, demanded of a variable. The
            // variable stands for whatever the caller picks, so it may not have
            // one — and unlike a closed row, which at least lists what it does
            // allow, there is nothing here to show the reader but the promise
            // they made. This is what replaces the lacks bookkeeping for a
            // rigid: nothing can ever be bound to one, so a demand on one is
            // refused outright rather than recorded for later.
            //
            // A label still being decided is not a demand and is not refused:
            // it settles absent below, the one answer the promise leaves open.
            if let (Presence::Present, Some((rigid, id))) = (&presence, &rigid) {
                let error = Error {
                    span,
                    kind: ErrorKind::RigidField {
                        shape,
                        field: name.clone(),
                        name: rigid.clone(),
                        declared: self.table.declared(*id),
                    },
                };
                let goal = Goal::Presence {
                    expected: Presence::Absent,
                    actual: presence.clone(),
                };
                let abandoned = [Assigned::Ty(field.ty.clone())];
                self.fail(span, Rule::Mismatch, goal, error, &abandoned);
                continue;
            }
            match (&presence, side) {
                (Presence::Absent, _) => {}
                // A label certainly there, against a closed tail on the
                // expected side: the term has a label the type does not
                // allow. On the actual side it is the other complaint: the
                // type demands a label the term does not have.
                (Presence::Present, Side::Expected) => {
                    let goal = Goal::Presence {
                        expected: Presence::Absent,
                        actual: presence.clone(),
                    };
                    let kind = ErrorKind::ExtraField {
                        shape,
                        base: self.frozen(base),
                        field: name.clone(),
                    };
                    let error = Error { span, kind };
                    let abandoned = [Assigned::Ty(field.ty.clone())];
                    self.fail(span, Rule::Presence { shape }, goal, error, &abandoned);
                }
                (Presence::Present, Side::Actual) => {
                    let goal = Goal::Presence {
                        expected: presence.clone(),
                        actual: Presence::Absent,
                    };
                    let kind = ErrorKind::MissingField {
                        shape,
                        base: self.frozen(base),
                        field: name.clone(),
                    };
                    let error = Error { span, kind };
                    let abandoned = [Assigned::Ty(field.ty.clone())];
                    self.fail(span, Rule::Presence { shape }, goal, error, &abandoned);
                }
                (_, Side::Expected) => self.presences(span, &Presence::Absent, &presence),
                (_, Side::Actual) => self.presences(span, &presence, &Presence::Absent),
            }
        }
        // And whatever would have continued past them allows nothing more
        // either, which is this side's own tail said of it. Skipped where
        // `rest` already allows nothing more, which would be a step saying what
        // both sides had already said.
        if !rest.closed() {
            match side {
                Side::Expected => self.tails(span, tail, rest),
                Side::Actual => self.tails(span, rest, tail),
            }
        }
    }

    /// Point an unbound variable at a value of its own sort, unless the value
    /// contains the variable itself — the occurs check that keeps every type a
    /// finite tree. Either way one step is recorded, and the rule it names is
    /// the one that actually applied: a cycle is [`Rule::Occurs`], not a
    /// [`Rule::Bind`] that happened to leave the variable where it was.
    ///
    /// Recursive types do not soften this, and this is the line that says so.
    /// A type may lead back to itself only through a declaration, where a
    /// person wrote down what it is; `fn x => x x` asks the solver to invent
    /// one, which is the difference between a type the language has and a type
    /// nothing could have written.
    ///
    /// The lacks check beside it is the row's version of the same idea: a tail
    /// stands for the labels its row does not write out, so a row that
    /// certainly has one of them is not a value that tail can take — and a
    /// row-tail variable is under the same condition, since it stands for a type
    /// the labels beside it are already on. Refused here rather than noticed
    /// later for a reason the occurs check does not share — nothing later is
    /// guaranteed to notice. Two rows sharing a tail are only compared again
    /// if the program happens to bring them back together, and [`Table::zonk`]
    /// keeps one copy of a repeated label without a word, so the contradiction
    /// reached the reader as a silently narrowed type or as a mismatch
    /// somewhere else entirely.
    ///
    /// Certainly has, not names: presence is what the condition is really
    /// about, because only a label that is there is a copy at all. A label the
    /// row names settled absent is not part of what the type says — the label
    /// rule already passes one over a closed row without a word, and a
    /// complaint here would refuse a row the solver itself holds equal to one
    /// it accepts. And a label still being decided has just met the thing that
    /// decides it: absent is the one answer both sides allow, and it is the
    /// answer [`Solve::absorb`] gives the same label against a closed row.
    fn assign(&mut self, span: Span, goal: Goal, var: TyVar, value: Assigned) {
        if self.table.occurs(var, &value) {
            let error = Error {
                span,
                kind: ErrorKind::Recursive,
            };
            let abandoned = [value.variable(var), value];
            self.fail(span, Rule::Occurs, goal, error, &abandoned);
            return;
        }
        // Binding a shared structural variable still installs the ordinary
        // row-tail shape, but no presence constant or alias from this arm may
        // hitch a ride and become global. Give every such slot a shared fresh
        // presence and relate it to the arm's view under the premise.
        let value = match (self.guard.is_some(), value) {
            (true, Assigned::Ty(ty)) => Assigned::Ty(self.guarded_type(span, &ty)),
            (true, Assigned::Row(row)) => Assigned::Row(Rc::new(self.guarded_row(span, &row))),
            // Direct presence equality is intercepted by `presences`, so a
            // guarded assignment never has a presence value to abstract. The
            // fallback is also the ordinary, unguarded assignment path.
            (_, value) => value,
        };
        if let Some((shape, named)) = self.table.lacked(var, &value) {
            // One complaint per binding, not per label: a tail that would have
            // to repeat two fields is one thing gone wrong with one row, and
            // naming the first of them is what the reader has to look at
            // either way. Ruled on before anything is settled, so a refusal
            // leaves no binding behind that only this goal wanted.
            if let Some((field, _)) = named
                .iter()
                .find(|(_, presence)| matches!(presence, Presence::Present))
            {
                let error = Error {
                    span,
                    kind: ErrorKind::RepeatedField {
                        shape,
                        field: field.clone(),
                    },
                };
                let abandoned = [value.variable(var), value];
                self.fail(span, Rule::Overlap { shape }, goal, error, &abandoned);
                return;
            }
            // No label is certainly there, so the ones still being decided are
            // decided: absent, the one answer the condition leaves open. Said
            // through the presence rule so the act is a step like any other.
            // The rest already answer the condition — settled absent, or
            // abandoned by a failure that was reported where it happened —
            // and two labels never share a presence variable, so no entry
            // here is a stale reading of another.
            for (_, presence) in &named {
                if matches!(presence, Presence::Var(_)) {
                    self.presences(span, &Presence::Absent, presence);
                }
            }
        }
        self.table.inherit_lacks(var, &value);
        // And the levels travel the same way the conditions do: what this
        // variable stands for is as old as this variable is, so everything
        // inside it belongs no deeper. See [`Table::demote`](super::Table).
        self.table.demote(var, &value);
        self.table.vars[var as usize] = Slot::Bound(value.clone());
        self.step(span, Rule::Bind, goal, Effect::Bound { var, value });
    }

    fn guarded_type(&mut self, span: Span, ty: &Rc<Ty>) -> Rc<Ty> {
        enum Work {
            Type(Rc<Ty>),
            Row(Row),
            Fields {
                pending: std::vec::IntoIter<(String, RowField)>,
                labels: IndexMap<String, RowField>,
                rest: Rest,
            },
            Field {
                name: String,
                presence: Presence,
                pending: std::vec::IntoIter<(String, RowField)>,
                labels: IndexMap<String, RowField>,
                rest: Rest,
            },
            Arrow,
            Struct,
            Sum,
            Named {
                symbol: Symbol,
                name: Rc<str>,
                count: usize,
            },
        }

        let mut work = vec![Work::Type(ty.clone())];
        let mut types = Vec::new();
        let mut rows = Vec::new();
        while let Some(part) = work.pop() {
            match part {
                Work::Type(ty) => {
                    let ty = self.table.resolve(&ty);
                    match &*ty {
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Arrow);
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Type(to.clone()));
                            work.push(Work::Type(from.clone()));
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
                                count: args.len(),
                            });
                            work.extend(args.iter().rev().cloned().map(Work::Type));
                        }
                        other => types.push(Rc::new(other.clone())),
                    }
                }
                Work::Row(row) => {
                    let row = self.table.canon(&row);
                    work.push(Work::Fields {
                        pending: row.labels.into_iter().collect::<Vec<_>>().into_iter(),
                        labels: IndexMap::new(),
                        rest: row.rest,
                    });
                }
                Work::Fields {
                    mut pending,
                    labels,
                    rest,
                } => match pending.next() {
                    Some((name, field)) => {
                        let resolved = self.table.presence_of(&field.presence);
                        let presence = match &resolved {
                            Presence::Undecided => Presence::Undecided,
                            resolved => {
                                let shared = self.table.fresh_presence();
                                self.guarded_presence(span, &shared, resolved);
                                shared
                            }
                        };
                        if matches!(resolved, Presence::Absent) {
                            // An absent slot has no semantic payload. Do not
                            // copy or abstract variables hidden in malformed
                            // imported recovery data; keep the slot explicitly
                            // irrelevant instead.
                            let mut labels = labels;
                            labels.insert(
                                name,
                                RowField {
                                    presence,
                                    ty: Rc::new(Ty::Undecided),
                                },
                            );
                            work.push(Work::Fields {
                                pending,
                                labels,
                                rest,
                            });
                        } else {
                            work.push(Work::Field {
                                name,
                                presence,
                                pending,
                                labels,
                                rest,
                            });
                            work.push(Work::Type(field.ty));
                        }
                    }
                    None => rows.push(Row { labels, rest }),
                },
                Work::Field {
                    name,
                    presence,
                    pending,
                    mut labels,
                    rest,
                } => {
                    let ty = types.pop().expect("guarded field payload");
                    labels.insert(name, RowField { presence, ty });
                    work.push(Work::Fields {
                        pending,
                        labels,
                        rest,
                    });
                }
                Work::Arrow => {
                    let effects = rows.pop().expect("guarded arrow effects");
                    let to = types.pop().expect("guarded arrow result");
                    let from = types.pop().expect("guarded arrow parameter");
                    types.push(Rc::new(Ty::Arrow(from, to, effects)));
                }
                Work::Struct => {
                    let row = rows.pop().expect("guarded struct row");
                    types.push(Rc::new(Ty::Struct(row)));
                }
                Work::Sum => {
                    let row = rows.pop().expect("guarded sum row");
                    types.push(Rc::new(Ty::Sum(row)));
                }
                Work::Named {
                    symbol,
                    name,
                    count,
                } => {
                    let split = types.len() - count;
                    let args: Vec<_> = types.drain(split..).collect();
                    types.push(Rc::new(Ty::Named {
                        symbol,
                        name,
                        args: args.into(),
                    }));
                }
            }
        }
        types.pop().expect("guarded type result")
    }

    fn guarded_row(&mut self, span: Span, row: &Row) -> Row {
        let guarded = self.guarded_type(span, &Rc::new(Ty::Struct(row.clone())));
        guarded.fields().cloned().unwrap_or_default()
    }

    /// Report a failure and abandon what it was about, in one act: the
    /// complaint, the step that ends the goal, and then every value in
    /// `abandoned` pointed at the undecided value of its own sort.
    ///
    /// The one way the solver has of failing, and deliberately so. Reporting
    /// and recovering used to be two calls an arm had to remember to make in
    /// order, and the arms disagreed: a mismatch reported without recovering,
    /// so the variable it had abandoned stayed unbound, and generalization
    /// quantified it — which made a term that failed to type polymorphic, and
    /// therefore silently acceptable to every later use of it.
    ///
    /// The error carries its own span rather than taking `span`, because the
    /// two need not be the same.
    fn fail(&mut self, span: Span, rule: Rule, goal: Goal, error: Error, abandoned: &[Assigned]) {
        let kind = error.kind.clone();
        self.errors.push(error);
        self.step(span, rule, goal, Effect::Failed(kind));
        for value in abandoned {
            self.recover(span, value);
        }
    }

    /// Abandon a value nothing will decide: every variable still unsolved in it
    /// becomes undecided, which unifies with everything, so the one complaint is
    /// not echoed by every term downstream of it. No occurs check — an undecided
    /// value mentions no variables to close a cycle with.
    ///
    /// The one way in, whichever sort the value is. Two kinds of caller reach
    /// it: [`fail`](Self::fail), abandoning what a complaint was about, and the
    /// arms that meet something already undecided — an undecided presence or an
    /// undecided tail absorbs whatever it was put against, and everything that
    /// would have decided it is abandoned with it.
    fn recover(&mut self, span: Span, value: &Assigned) {
        match value {
            Assigned::Ty(ty) => self.recover_ty(span, ty),
            Assigned::Row(row) => self.recover_row(span, row),
            Assigned::Presence(presence) => self.recover_presence(span, presence),
        }
    }

    /// [`recover`](Self::recover) over a type: its constructor, and then the fields it
    /// carries. A composite is abandoned by abandoning what it is made of —
    /// the goal that would have decided `?1 -> ?2` decided neither half.
    fn recover_ty(&mut self, span: Span, ty: &Rc<Ty>) {
        self.recover_parts(span, Some(ty.clone()), None);
    }

    /// [`recover`](Self::recover) over a sum's cases: every label, and then the
    /// tail saying what else the row might have had.
    fn recover_row(&mut self, span: Span, row: &Row) {
        self.recover_parts(span, None, Some(row.clone()));
    }

    /// Iterative recovery for imported semantic trees. A failure can abandon a
    /// value whose artifact contains 30,000 arrows, names, rows, or field
    /// payloads; recovery still has to settle every variable and presence in
    /// it without borrowing the native call stack from the malformed input.
    fn recover_parts(&mut self, span: Span, ty: Option<Rc<Ty>>, row: Option<Row>) {
        enum Work {
            Type(Rc<Ty>),
            Row(Row),
            Presence(Presence),
            Rest(Rest),
        }

        let mut work = Vec::new();
        if let Some(row) = row {
            work.push(Work::Row(row));
        }
        if let Some(ty) = ty {
            work.push(Work::Type(ty));
        }
        while let Some(part) = work.pop() {
            match part {
                Work::Type(ty) => {
                    let ty = self.table.resolve(&ty);
                    match &*ty {
                        Ty::Var(var) => {
                            self.settle(span, *var, Assigned::Ty(Rc::new(Ty::Undecided)))
                        }
                        Ty::Arrow(from, to, effects) => {
                            work.push(Work::Row(effects.clone()));
                            work.push(Work::Type(to.clone()));
                            work.push(Work::Type(from.clone()));
                        }
                        Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row.clone())),
                        Ty::Named { args, .. } => {
                            work.extend(args.iter().rev().cloned().map(Work::Type));
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
                    let row = self.table.canon(&row);
                    work.push(Work::Rest(row.rest));
                    for field in row.labels.into_values().rev() {
                        let presence = self.table.presence_of(&field.presence);
                        if !matches!(presence, Presence::Absent) {
                            work.push(Work::Type(field.ty));
                        }
                        work.push(Work::Presence(presence));
                    }
                }
                Work::Presence(presence) => self.recover_presence(span, &presence),
                Work::Rest(rest) => {
                    if let Rest::Var(var) = self.table.canon(&Row::of(rest)).rest {
                        self.settle(span, var, Assigned::Row(Rc::new(Row::of(Rest::Undecided))));
                    }
                }
            }
        }
    }

    /// [`recover`](Self::recover) over a presence.
    fn recover_presence(&mut self, span: Span, presence: &Presence) {
        if let Presence::Var(var) = self.table.presence_of(presence) {
            self.settle(span, var, Assigned::Presence(Presence::Undecided));
        }
    }

    /// Bind one abandoned variable, and say so: a reader following the state
    /// would otherwise see a variable acquire a value that no rule they were
    /// shown gave it.
    fn settle(&mut self, span: Span, var: TyVar, value: Assigned) {
        self.table.vars[var as usize] = Slot::Bound(value.clone());
        let goal = match &value {
            Assigned::Ty(ty) => Goal::Type {
                expected: Rc::new(Ty::plain(Ty::Var(var))),
                actual: ty.clone(),
            },
            Assigned::Row(row) => Goal::Row {
                expected: Rc::new(Row::of(Rest::Var(var))),
                actual: row.clone(),
            },
            Assigned::Presence(presence) => Goal::Presence {
                expected: Presence::Var(var),
                actual: presence.clone(),
            },
        };
        self.step(span, Rule::Recover, goal, Effect::Bound { var, value });
    }

    fn step(&mut self, span: Span, rule: Rule, goal: Goal, effect: Effect) {
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

/// A goal about two values of one sort, worded in that sort.
///
/// What [`Solve::absorb`] records, and the one place the shared label rule has
/// to say which sort it is holding: a struct's tail is a whole type and a sum's
/// is a row, so the same act is a [`Goal::Type`] on one shape and a [`Goal::Row`]
/// on the other.
/// An effect row as the arrow it would be the effects of: `{} -> {} ! <row>`.
///
/// What [`Rowed`] wants a type for, in the one place there is none to be had. An
/// ambient is where a term sits rather than part of any type, so a complaint
/// about one has no written type to name — and naming the row alone would print
/// it in the struct's braces, which is what [`Display for Row`](Row) falls back
/// to when nothing hands it a shape.
/// What a broken rigid rest turned out to be, quoted in its own reading: the
/// sum that refused it, or the arrow that would have carried the effects. The
/// sense rides along so the wording can follow it.
fn rest_found(row: &Row, shape: Shape) -> (Sense, Rc<Ty>) {
    match shape {
        Shape::Struct => (Sense::Fields, Rc::new(Ty::plain(Ty::Struct(row.clone())))),
        Shape::Sum => (Sense::Cases, Rc::new(Ty::plain(Ty::Sum(row.clone())))),
        Shape::Effect => (Sense::Effects, row_ty(row)),
    }
}

fn row_ty(row: &Row) -> Rc<Ty> {
    Rc::new(Ty::plain(Ty::Arrow(
        Rc::new(Ty::unit()),
        Rc::new(Ty::unit()),
        row.clone(),
    )))
}
