//! Pass two: solving. See [`Solve`].

use std::{collections::HashSet, rc::Rc};

use indexmap::IndexMap;

use crate::{
    symbol::Symbol,
    tracking::Span,
    types::{Assigned, Core, Presence, Rest, Row, RowField, Scheme, Shape, Ty, TyVar},
};

use super::{
    Constraint, ConstraintKind, Effect, Error, ErrorKind, Goal, Rule, Side, Slot, Step, Table,
};

/// A row a goal is about, the type it belongs to, and the shape it was matched
/// on.
///
/// The three travel together everywhere below [`Solve::rows`]: a complaint
/// names the whole type, and is worded in the nouns of the shape. Paired rather
/// than read back where they are needed, so there is one reading of the shape —
/// the goal's — and nothing further in that could come to disagree with it.
///
/// The type is carried as well as the row because a complaint is about the
/// type. `` (`A 1).x `` is a missing *field* on a sum-cored type: the row that
/// went wrong is that type's field row, and the type beside the word is the
/// whole of what the reader wrote.
#[derive(Clone, Copy)]
struct Rowed<'a> {
    shape: Shape,
    ty: &'a Rc<Ty>,
    row: &'a Row,
}

impl<'a> Rowed<'a> {
    /// The fields a type carries, which every type has.
    fn fields(ty: &'a Rc<Ty>) -> Self {
        Self {
            shape: Shape::Struct,
            ty,
            row: &ty.fields,
        }
    }

    /// The cases a sum-cored type allows. The row is handed in rather than
    /// matched out, so that the one caller that knows the core is a sum is the
    /// one that says so.
    fn cases(ty: &'a Rc<Ty>, cases: &'a Row) -> Self {
        Self {
            shape: Shape::Sum,
            ty,
            row: cases,
        }
    }

    /// The type this row belongs to, with the row replaced. What a complaint
    /// names: the whole type, so that a base printed beside a missing field
    /// reads as the type the reader wrote rather than as the row the solver was
    /// looking at.
    fn rebuild(&self, row: Row) -> Rc<Ty> {
        match self.shape {
            Shape::Struct => Rc::new(Ty {
                core: self.ty.core.clone(),
                fields: row,
            }),
            Shape::Sum => Rc::new(Ty {
                core: Core::Sum(row),
                fields: self.ty.fields.clone(),
            }),
        }
    }
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
    /// `Ptr`'s in `type Ptr a = Nat` does not.
    ///
    /// The one thing [`Rule::Congruent`] is allowed to decide by. Where every
    /// parameter reaches a position of the body, each argument sits in a
    /// structural position of its own, so unifying two bodies decomposes into
    /// unifying the arguments and the two rules cannot disagree — congruence is
    /// then a shortcut with better spans and better wording, and never a second
    /// answer. Where a parameter is discarded they *do* disagree, and unfolding
    /// is the one that is right, so those fall through to it. See
    /// [`Core::Named`] and [`ir::relevance`](crate::ir).
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
}

impl Solve<'_> {
    /// Solve everything generation asked for, in the order it was asked.
    /// Every constraint is an equality, and rows are why that is enough: a
    /// projection's demand is an ordinary type with an open field row, so
    /// nothing has to wait for a later round to know what its base is.
    pub fn run(&mut self, constraints: &[Constraint]) {
        for constraint in constraints {
            let ConstraintKind::Equal { expected, actual } = &constraint.kind;
            self.unify(constraint.span, expected, actual);
        }
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the solve continues.
    ///
    /// Two types are equal when their cores are equal and their field rows are
    /// equal, and that is the whole rule. Which leaves three shapes of trace.
    /// Undecided absorbs, as it always has. A *bare* variable — a core with no
    /// fields of its own — takes the whole type it is against, fields and all,
    /// which is what keeps `fn x => x` inferring `'a -> 'a`. And everything
    /// else is the core's own rule followed by the field rows, except that two
    /// unit cores are nothing but their field rows and so record only the
    /// struct step, byte for byte what a struct against a struct has always
    /// recorded.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
        let lhs = self.table.resolve(expected);
        let rhs = self.table.resolve(actual);
        let goal = Goal::Type {
            expected: lhs.clone(),
            actual: rhs.clone(),
        };
        // One variable against itself is already the same thing, so the arms
        // below must not read it as a variable against a type and try to bind
        // it to itself. Its field rows may still differ, which is what the
        // fall-through decides.
        let itself = matches!((&lhs.core, &rhs.core), (Core::Var(a), Core::Var(b)) if a == b);
        match (&lhs.core, &rhs.core) {
            // Undecided is the absorbing error type: whatever failed under it
            // was reported where it failed. Absorbing is not the same as
            // learning nothing, though — the other side is a type this goal
            // was going to decide, and leaving its variables unbound would let
            // generalization quantify a term that only reached the solver
            // through a failure.
            (Core::Undecided, _) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover_ty(span, &rhs);
            }
            (_, Core::Undecided) => {
                self.step(span, Rule::Absorb, goal, Effect::None);
                self.recover_ty(span, &lhs);
            }
            // A bare variable takes the whole type. Before unfolding, so that
            // one against a declared type takes the type by the name it was
            // written as: what a definition is inferred to be then reads as
            // what its annotations said, and the solver has one less thing to
            // unfold later.
            (Core::Var(var), _) if !itself && lhs.fields.is_trivial() => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(rhs.clone()));
            }
            (_, Core::Var(var)) if !itself && rhs.fields.is_trivial() => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(lhs.clone()));
            }
            // Two types with nothing of their own are their fields and nothing
            // else, so the field rows are the whole question and the struct
            // rule is the whole step.
            (Core::Unit, Core::Unit) => self.rows(span, Rowed::fields(&lhs), Rowed::fields(&rhs)),
            // The core's own rule, and then — one level under it — the fields
            // both sides carry, when either side has any to speak of. A type
            // with no fields is every type the language can write, so this
            // second step is invisible to every program that could be written
            // before fields were a property of all of them.
            _ => {
                let decided = self.cores(span, goal, &lhs, &rhs);
                let carried = !(lhs.fields.is_trivial() && rhs.fields.is_trivial());
                if decided && carried {
                    self.depth += 1;
                    self.rows(span, Rowed::fields(&lhs), Rowed::fields(&rhs));
                    self.depth -= 1;
                }
            }
        }
    }

    /// Decide two cores, and say whether the field rows beside them are still
    /// worth deciding. A goal that failed decides nothing, and one that was
    /// replaced — an unfolding — carries its own fields along with it.
    ///
    /// A core variable bound here takes the *core* it is against and an empty
    /// closed row, not the whole type: the fields are what the caller decides
    /// next, and binding them in as well would decide them twice.
    fn cores(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) -> bool {
        match (&lhs.core, &rhs.core) {
            (Core::Var(a), Core::Var(b)) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None);
                true
            }
            // One declaration applied to nothing is only ever equal to itself,
            // so this saves an unfolding rather than deciding anything. Two
            // *different* declarations fall through to unfolding and are equal
            // whenever what they stand for is.
            //
            // Arity belongs to the declaration, so one empty argument list
            // means the other is empty too.
            (
                Core::Named {
                    symbol: a,
                    args: xs,
                    ..
                },
                Core::Named { symbol: b, .. },
            ) if a == b && xs.is_empty() => {
                self.step(span, Rule::Same, goal, Effect::None);
                true
            }
            // Two applications of one declaration are equal when their
            // arguments are — and this is a shortcut to unfolding rather than a
            // rule that could contradict it, which is the whole of what
            // [`Solve::nominal`] is checked for. Where every parameter reaches a
            // position of the body, the two answers are the same answer, and
            // this one is reached without building either body and complains in
            // words about the types the reader wrote. Where a parameter is
            // discarded they differ — `Ptr A` and `Ptr B` both stand for `Nat`,
            // so they are one type — and the arm below decides it by unfolding.
            //
            // Not termination either: the assumption stack catches a
            // declaration leading back to itself, since it is keyed on the goal
            // and a recursion may not grow its arguments. See [`Core::Named`].
            //
            // Arity belongs to the declaration, so one symbol means one count
            // and the zip drops nothing.
            (
                Core::Named {
                    symbol: a,
                    args: xs,
                    ..
                },
                Core::Named {
                    symbol: b,
                    args: ys,
                    ..
                },
            ) if a == b && self.nominal.contains(a) => {
                let pairs: Vec<_> = xs.iter().cloned().zip(ys.iter().cloned()).collect();
                // Where to put everything back if the arguments turn out not to
                // agree. A congruence that fails is a failure of the two
                // applications, so what the reader is shown is the
                // applications, not a complaint about the third field of a
                // struct they wrote the name of.
                //
                // Which means the attempt leaves nothing behind — the
                // complaints it made, the steps it recorded and the bindings it
                // took. The bindings because a goal that failed decides nothing
                // and [`Solve::fail`] below is what decides what it abandons;
                // the steps because a reader replaying them builds the
                // solution out of them, and a step whose binding is no longer
                // in the table would put them one type ahead of the solver. So
                // the trace shows the answer rather than the working, which is
                // the same bargain [`Solve::fail`] already makes: one failure,
                // said once, with everything it touched pointed at the
                // undecided type.
                let reported = self.errors.len();
                let stepped = self.steps.len();
                let known = self.table.snapshot();
                self.step(span, Rule::Congruent, goal.clone(), Effect::Decomposed);
                self.depth += 1;
                for (x, y) in pairs {
                    self.unify(span, &x, &y);
                }
                self.depth -= 1;
                if self.errors.len() <= reported {
                    return true;
                }
                self.errors.truncate(reported);
                self.steps.truncate(stepped);
                self.table.restore(known);
                let error = Error {
                    span,
                    kind: ErrorKind::Mismatch {
                        expected: lhs.clone(),
                        actual: rhs.clone(),
                    },
                };
                self.fail(
                    span,
                    Rule::Mismatch,
                    goal,
                    error,
                    &[Assigned::Ty(lhs.clone()), Assigned::Ty(rhs.clone())],
                );
                false
            }
            (Core::Named { .. }, _) | (_, Core::Named { .. }) => {
                self.unfold(span, goal, lhs, rhs);
                false
            }
            // A core variable takes the core it is against — and the type it
            // ends up standing for is decided by that core alone, so a name
            // must be looked through first. What a declaration stands for keeps
            // its fields behind the name, and binding the variable to the name
            // would leave the fields beside it with nothing to be decided
            // against. A *bare* variable is the exception and is taken before
            // this, in [`Solve::unify`], because there is nothing beside it.
            (Core::Var(var), core) => {
                let (var, core) = (*var, core.clone());
                self.assign(span, goal, var, Assigned::Ty(Rc::new(Ty::plain(core))));
                true
            }
            (core, Core::Var(var)) => {
                let (var, core) = (*var, core.clone());
                self.assign(span, goal, var, Assigned::Ty(Rc::new(Ty::plain(core))));
                true
            }
            (Core::Nat, Core::Nat) => {
                self.step(span, Rule::Prim, goal, Effect::None);
                true
            }
            (Core::Arrow(from1, to1), Core::Arrow(from2, to2)) => {
                let (from1, to1) = (from1.clone(), to1.clone());
                let (from2, to2) = (from2.clone(), to2.clone());
                self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                self.depth += 1;
                self.unify(span, &from1, &from2);
                self.unify(span, &to1, &to2);
                self.depth -= 1;
                true
            }
            // Two sums: the same row code, read in cases. A sum against
            // anything else falls through to the arm below and is an ordinary
            // mismatch — a struct and a sum are two types however much their
            // insides look alike, and lining their labels up would be answering
            // a question nobody asked.
            (Core::Sum(cases), Core::Sum(others)) => {
                self.rows(span, Rowed::cases(lhs, cases), Rowed::cases(rhs, others));
                true
            }
            // Nothing applies, and the two types cannot be made equal. Named as
            // the two whole types rather than as their cores: a `Nat` against
            // `{ x: Nat }` is a mismatch of what the reader wrote, and the
            // cores alone would quote them a unit they never mentioned.
            _ => {
                let error = Error {
                    span,
                    kind: ErrorKind::Mismatch {
                        expected: lhs.clone(),
                        actual: rhs.clone(),
                    },
                };
                self.fail(
                    span,
                    Rule::Mismatch,
                    goal,
                    error,
                    &[Assigned::Ty(lhs.clone()), Assigned::Ty(rhs.clone())],
                );
                false
            }
        }
    }

    /// Make two rows of one shape the same row. [`Rule::Struct`] — or
    /// [`Rule::Sum`], which is the same rule about the other shape — replaces
    /// the pair with everything that has to hold of it: labels only one side
    /// names flow into the other side's tail, and the labels both name are
    /// decided one by one.
    ///
    /// The tails go first, the shared labels after. The other order would let
    /// a shared label's own unification bind one of the tails behind the
    /// flattened copy this function is holding — a field's type can mention
    /// its own row's tail — and an act performed on a stale tail is an act
    /// performed on the wrong type.
    ///
    /// The step comes before all of it, and everything the rule does is one
    /// level under it. Flattening is where that used to leak: [`Table::canon`]
    /// decided things, and it ran before the step, so what it decided appeared
    /// in the trace above and beside the rule it belongs to rather than
    /// beneath it. Flattening is now a read, and what it finds to decide is
    /// decided here, in the rule's own scope.
    ///
    /// Which is also what the goal is recorded as: the two rows flattened, and
    /// each put back in the type it came out of. A tail already bound to a row
    /// is where that differs from the rows as they arrived, and it is exactly
    /// the case where the unflattened goal cannot account for its own children
    /// — the labels under it come from a row the line above does not show.
    fn rows(&mut self, span: Span, lhs: Rowed<'_>, rhs: Rowed<'_>) {
        let shape = lhs.shape;
        let want = self.table.canon(lhs.row);
        let have = self.table.canon(rhs.row);
        let expected = lhs.rebuild(want.clone());
        let actual = rhs.rebuild(have.clone());
        let goal = Goal::Type {
            expected: expected.clone(),
            actual: actual.clone(),
        };

        let rule = match shape {
            Shape::Struct => Rule::Struct,
            Shape::Sum => Rule::Sum,
        };
        self.step(span, rule, goal.clone(), Effect::Decomposed);
        self.depth += 1;

        let only_want: IndexMap<String, RowField> = want
            .labels
            .iter()
            .filter(|(name, _)| !have.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();
        let only_have: IndexMap<String, RowField> = have
            .labels
            .iter()
            .filter(|(name, _)| !want.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();

        // Two rows sharing one tail cannot differ in labels: whatever the
        // tail absorbed from one side it would grow on the other, and the
        // rows would chase each other forever. The occurs check inside
        // `assign` refuses the binding when only one side has extras, but the
        // both-sided case binds cleanly every round and never converges, so
        // the pair is refused here — the same cycle, caught one level up.
        if let (Rest::Var(a), Rest::Var(b)) = (&want.rest, &have.rest)
            && a == b
            && !(only_want.is_empty() && only_have.is_empty())
        {
            let error = Error {
                span,
                kind: ErrorKind::Recursive,
            };
            let abandoned = [
                Assigned::Row(Rc::new(want.clone())),
                Assigned::Row(Rc::new(have.clone())),
            ];
            self.fail(span, Rule::Occurs, goal, error, &abandoned);
            self.depth -= 1;
            return;
        }

        let expects = Rowed {
            shape,
            ty: &expected,
            row: &want,
        };
        let actuals = Rowed {
            shape,
            ty: &actual,
            row: &have,
        };

        match (only_want.is_empty(), only_have.is_empty()) {
            // The rows name the same labels, so the tails are simply each
            // other. Two closed tails already agree, and saying so would be
            // a step about nothing.
            (true, true) => {
                if !matches!((&want.rest, &have.rest), (Rest::Closed, Rest::Closed)) {
                    self.rests(span, &want.rest, &have.rest);
                }
            }
            (true, false) => self.absorb(
                span,
                Side::Expected,
                &want.rest,
                only_have,
                &have.rest,
                expects,
            ),
            (false, true) => self.absorb(
                span,
                Side::Actual,
                &have.rest,
                only_want,
                &want.rest,
                actuals,
            ),
            // Extras both ways continue as one fresh tail, which is what
            // makes the two rows end as the same row rather than merely
            // overlapping ones.
            (false, false) => {
                let rest = self.table.fresh_row();
                self.absorb(span, Side::Expected, &want.rest, only_have, &rest, expects);
                self.absorb(span, Side::Actual, &have.rest, only_want, &rest, actuals);
            }
        }

        let shared: Vec<_> = want
            .labels
            .iter()
            .filter_map(|(name, field)| {
                have.labels
                    .get(name)
                    .map(|other| (name.clone(), field.clone(), other.clone()))
            })
            .collect();
        for (name, want, have) in shared {
            self.field(span, &name, expects, actuals, &want, &have);
        }
        self.depth -= 1;
    }

    /// Make two tails the same tail: a variable takes the other, and an
    /// undecided one absorbs it.
    ///
    /// Only reached where the two rows name the same labels, so there is
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
    fn rests(&mut self, span: Span, lhs: &Rest, rhs: &Rest) {
        let want = Rc::new(self.table.canon(&bare(lhs)));
        let have = Rc::new(self.table.canon(&bare(rhs)));
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

    /// A row as it reads at this moment: the tail spliced in as far as it has
    /// been decided, every presence fixed at what it has been decided to
    /// be — still open becomes undecided, which prints as the `?` the user
    /// would have written and which nothing downstream will rewrite — and the
    /// whole put back into the type the row belongs to.
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
        let flat = self.table.canon(row.row);
        let labels = flat
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
        row.rebuild(Row {
            labels,
            rest: flat.rest,
        })
    }

    /// Decide one label both rows name: whether it is there must agree, and
    /// while it may be, the types must too. A clash of the constants is
    /// worded as the label the actual side is missing or the extra one it
    /// has, never as `present` against `absent` — the label's name is known
    /// here, and the complaint should name it.
    ///
    /// Which noun the rule is read in is handed down from [`Solve::rows`], the
    /// way [`Solve::absorb`]'s is: the shape is what the two rows were matched
    /// on, so passing it costs nothing and leaves no second reading of it here
    /// to drift.
    fn field(
        &mut self,
        span: Span,
        name: &str,
        expected: Rowed<'_>,
        actual: Rowed<'_>,
        want: &RowField,
        have: &RowField,
    ) {
        let shape = expected.shape;
        let p1 = self.table.presence_of(&want.presence);
        let p2 = self.table.presence_of(&have.presence);
        let goal = Goal::Presence {
            expected: p1.clone(),
            actual: p2.clone(),
        };
        match (&p1, &p2) {
            // The common case: certainly there on both sides, so presence
            // has nothing to say and the types carry the whole question.
            (Presence::Present, Presence::Present) => self.unify(span, &want.ty, &have.ty),
            (Presence::Present, Presence::Absent) => {
                let kind = ErrorKind::MissingField {
                    shape,
                    base: self.frozen(actual),
                    field: name.to_string(),
                };
                let abandoned = [Assigned::Ty(want.ty.clone()), Assigned::Ty(have.ty.clone())];
                self.fail(
                    span,
                    Rule::Presence { shape },
                    goal,
                    Error { span, kind },
                    &abandoned,
                );
            }
            (Presence::Absent, Presence::Present) => {
                let kind = ErrorKind::ExtraField {
                    shape,
                    base: self.frozen(expected),
                    field: name.to_string(),
                };
                let abandoned = [Assigned::Ty(want.ty.clone()), Assigned::Ty(have.ty.clone())];
                self.fail(
                    span,
                    Rule::Presence { shape },
                    goal,
                    Error { span, kind },
                    &abandoned,
                );
            }
            // At least one side is still a variable or undecided: the
            // presences unify as anything else does, and the types follow
            // unless the label just settled absent — an absent label's type
            // slot means nothing, and constraining it would reject rows that
            // agree.
            _ => {
                self.presences(span, &p1, &p2);
                if !matches!(self.table.presence_of(&p1), Presence::Absent) {
                    self.unify(span, &want.ty, &have.ty);
                }
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

    /// Push the labels only one row names into the other row's tail, to
    /// continue as `rest`. `side` says which side of the goal the tail sits
    /// on, so that every act performed on its behalf keeps the direction the
    /// constraint was worded in, and `base` is the type the tail's row belongs
    /// to, which is what a complaint names and the shape it is worded in.
    ///
    /// An open tail takes the row whole — one binding, occurs-checked like
    /// any other. A closed tail takes nothing: each label must turn out
    /// absent, one that certainly is not is a missing or extra label by
    /// `side`, and whatever would have continued as `rest` is closed too.
    fn absorb(
        &mut self,
        span: Span,
        side: Side,
        tail: &Rest,
        extras: IndexMap<String, RowField>,
        rest: &Rest,
        base: Rowed<'_>,
    ) {
        let shape = base.shape;
        if !matches!(tail, Rest::Closed) {
            let row = Rc::new(Row {
                labels: extras,
                rest: rest.clone(),
            });
            let (expected, actual) = match side {
                Side::Expected => (Rc::new(bare(tail)), row.clone()),
                Side::Actual => (row.clone(), Rc::new(bare(tail))),
            };
            let goal = Goal::Row { expected, actual };
            // Not a second row rule: what the extras are was decided above, and
            // this is the act of putting them somewhere. A variable takes them;
            // an undecided tail absorbs them, and everything they would have
            // decided is abandoned with them.
            match tail {
                Rest::Var(var) => self.assign(span, goal, *var, Assigned::Row(row)),
                _ => {
                    self.step(span, Rule::Absorb, goal, Effect::None);
                    self.recover(span, &Assigned::Row(row));
                }
            }
            return;
        }

        for (name, field) in &extras {
            let presence = self.table.presence_of(&field.presence);
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
        if !matches!(rest, Rest::Closed) {
            match side {
                Side::Expected => self.rests(span, &Rest::Closed, rest),
                Side::Actual => self.rests(span, rest, &Rest::Closed),
            }
        }
    }

    /// Replace whichever sides are declared types with what they stand for,
    /// and ask the goal again.
    ///
    /// This is where recursive types are decided, and it is the only rule that
    /// can be asked the same question twice: `list` against a second
    /// declaration of the same shape unfolds to two structs whose `next`
    /// fields are the two declarations again. So the goal is remembered while
    /// the goals it broke into are open, and meeting it again is
    /// [`Rule::Assume`] — the two are equal exactly when assuming so leads to
    /// no contradiction, and every contradiction there could be is one of the
    /// goals in between.
    ///
    /// What is remembered is the goal itself: both names *and* both argument
    /// lists. Why that is sound, and why it still repeats often enough to be
    /// worth having. Four things, two of them rules enforced somewhere else — so
    /// this comment is the map of what this rule rests on, and none of it may be
    /// relaxed without coming back here first.
    ///
    /// 1. A name against a shape descends into a strictly smaller shape each
    ///    time, so it runs out. Unchanged: a name is still the only way a type
    ///    leads back to itself.
    /// 2. Two applications of the *same* declaration usually never reach here:
    ///    [`Rule::Congruent`] took them, comparing arguments instead of
    ///    unfolding. Usually and not always — a declaration that discards a
    ///    parameter gets no congruence, because there the two rules would
    ///    disagree and unfolding is the one that is right — so an entry may name
    ///    one declaration twice. Nothing here needs it not to: what an entry
    ///    stands for is the goal, and the goal is asked again only if it comes
    ///    back at the same arguments, which is (3) and (4) below.
    /// 3. An entry stands for the question it was pushed for and for no other,
    ///    because it *is* that question. `Tree {}` against `Tree2 Nat` is not
    ///    `Tree Nat` against `Tree2 Nat`, so it is asked rather than assumed —
    ///    and it is a different question, since one unfolds to `{ v: {}, .. }`
    ///    and the other to `{ v: Nat, .. }`.
    /// 4. The key still repeats, which is what makes assuming ever fire. Inside
    ///    one group of mutually recursive declarations every argument handed on
    ///    is either one that came in or a type written out in the program with
    ///    no parameter in it — [`ir::build`](crate::ir::build) refuses anything
    ///    that would grow. So the arguments reachable within a group are drawn
    ///    from a finite set, the argument lists built from them are finite too,
    ///    and a goal that keeps coming back must come back at a list already
    ///    seen. A name from a *different* group belongs to a strictly lower
    ///    group in the dependency order, from which nothing in the enclosing
    ///    group is reachable, so descending through groups is descent through a
    ///    finite acyclic order.
    ///
    /// Without (4) nothing finite would come back. A declaration written
    /// `type T a = { next: T { x: a } }` grows its argument every round, so no
    /// goal about it is ever asked twice, and the stack only gets longer.
    /// Without (3) the key would be unsound rather than merely coarse —
    /// keyed on the two names alone, a declaration reached inside its own group
    /// at two different arguments has its second question answered by the first
    /// question's assumption, and two types that differ are accepted as equal.
    /// That is the whole reason the arguments are here.
    ///
    /// The arguments are compared as they stand *now*, which is why
    /// [`Table::alike`] resolves instead of this stack storing them resolved on
    /// the way in. A variable inside an argument may be bound while the goal is
    /// open, and when it is, the goal on the stack is the goal it has become —
    /// the same two types under a substitution that has since decided more of
    /// them, which is what the solver is in the middle of proving. Freezing the
    /// key when it was pushed would compare against something no longer
    /// believed, and would miss the repeat.
    ///
    /// Equality is structural throughout: two types are one type when what they
    /// stand for is, however differently they were written and whatever they
    /// were called. Comparing two applications of one declaration argument by
    /// argument is a way of reaching that answer sooner where it is bound to be
    /// the same answer, not a second notion of equality beside it. See
    /// [`Core::Named`], which states the rule, and [`Solve::nominal`], which is
    /// where the "bound to be" is checked.
    fn unfold(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let pair = match (&lhs.core, &rhs.core) {
            (Core::Named { .. }, Core::Named { .. }) => Some((lhs.clone(), rhs.clone())),
            _ => None,
        };
        let already = pair.is_some()
            && self
                .assumed
                .iter()
                .any(|(a, b)| self.table.alike(a, lhs) && self.table.alike(b, rhs));
        if already {
            self.step(span, Rule::Assume, goal, Effect::None);
            return;
        }
        let remembered = pair.is_some();
        let lhs = self.table.unfolded(self.aliases, lhs);
        let rhs = self.table.unfolded(self.aliases, rhs);
        self.step(span, Rule::Unfold, goal, Effect::Decomposed);
        self.assumed.extend(pair);
        self.depth += 1;
        self.unify(span, &lhs, &rhs);
        self.depth -= 1;
        if remembered {
            self.assumed.pop();
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
    /// stands for the labels its row does not write out, so a row that writes
    /// one of them out is not a value that tail can take — and a core variable
    /// is under the same condition, since it stands for a type the labels
    /// beside it are already on. Refused here rather than noticed later for a
    /// reason the occurs check does not share — nothing later is guaranteed to
    /// notice. Two rows sharing a tail are only compared again if the program
    /// happens to bring them back together, and [`Table::zonk`] keeps one copy
    /// of a repeated label without a word, so the contradiction reached the
    /// reader as a silently narrowed type or as a mismatch somewhere else
    /// entirely.
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
        // One complaint per binding, not per label: a tail that would have to
        // repeat two fields is one thing gone wrong with one row, and naming
        // the first of them is what the reader has to look at either way.
        if let Some((shape, field)) = self.table.repeated(var, &value) {
            let error = Error {
                span,
                kind: ErrorKind::RepeatedField { shape, field },
            };
            let abandoned = [value.variable(var), value];
            self.fail(span, Rule::Overlap { shape }, goal, error, &abandoned);
            return;
        }
        self.table.inherit_lacks(var, &value);
        self.table.vars[var as usize] = Slot::Bound(value.clone());
        self.step(span, Rule::Bind, goal, Effect::Bound { var, value });
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

    /// [`recover`](Self::recover) over a type: its core, and then the fields it
    /// carries. A composite is abandoned by abandoning what it is made of —
    /// the goal that would have decided `?1 -> ?2` decided neither half.
    fn recover_ty(&mut self, span: Span, ty: &Rc<Ty>) {
        let ty = self.table.resolve(ty);
        match &ty.core {
            Core::Var(var) => {
                let var = *var;
                self.settle(span, var, Assigned::Ty(Rc::new(Ty::default())));
            }
            Core::Arrow(from, to) => {
                let (from, to) = (from.clone(), to.clone());
                self.recover_ty(span, &from);
                self.recover_ty(span, &to);
            }
            Core::Sum(cases) => {
                let cases = cases.clone();
                self.recover_row(span, &cases);
            }
            // For the reason an arrow's halves are: an argument the abandoned
            // goal would have decided is left for generalization to quantify
            // otherwise.
            Core::Named { args, .. } => {
                for arg in args.clone().iter() {
                    self.recover_ty(span, arg);
                }
            }
            Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => {}
        }
        let fields = ty.fields.clone();
        self.recover_row(span, &fields);
    }

    /// [`recover`](Self::recover) over a row. A label is abandoned whole: its
    /// presence and its type, and then the tail saying what else the row might
    /// have had. Missing one would leave a variable unbound for generalization
    /// to quantify.
    fn recover_row(&mut self, span: Span, row: &Row) {
        let row = self.table.canon(row);
        for field in row.labels.values() {
            let (presence, ty) = (field.presence.clone(), field.ty.clone());
            self.recover_presence(span, &presence);
            self.recover_ty(span, &ty);
        }
        self.recover_rest(span, &row.rest);
    }

    /// [`recover`](Self::recover) over a tail.
    fn recover_rest(&mut self, span: Span, rest: &Rest) {
        if let Rest::Var(var) = self.table.canon(&bare(rest)).rest {
            self.settle(
                span,
                var,
                Assigned::Row(Rc::new(Row {
                    labels: IndexMap::new(),
                    rest: Rest::Undecided,
                })),
            );
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
                expected: Rc::new(Ty::plain(Core::Var(var))),
                actual: ty.clone(),
            },
            Assigned::Row(row) => Goal::Row {
                expected: Rc::new(bare(&Rest::Var(var))),
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

/// A tail as the row it stands for: no labels of its own, and everything past
/// them. What a tail is compared and bound as, since the sort a row variable
/// has is the row sort.
fn bare(rest: &Rest) -> Row {
    Row {
        labels: IndexMap::new(),
        rest: rest.clone(),
    }
}
