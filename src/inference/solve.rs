//! Pass two: solving. See [`Solve`].

use std::{collections::HashSet, rc::Rc, slice};

use indexmap::IndexMap;

use crate::{
    symbol::Symbol,
    tracking::Span,
    types::{RowField, Scheme, Shape, Ty, TyVar},
};

use super::{Constraint, ConstraintKind, Effect, Error, ErrorKind, Rule, Side, Slot, Step, Table};

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
    /// Each entry is the two [`Ty::Named`]s themselves — names *and* arguments
    /// — because that is the goal, and an assumption keyed on less than the goal
    /// answers questions it was never asked. Compared with [`Table::alike`]
    /// rather than by identity, since unfolding rebuilds an argument each round
    /// rather than passing the same allocation on. See [`Solve::unfold`].
    pub assumed: Vec<(Rc<Ty>, Rc<Ty>)>,
    /// [`Constraint::base_span`] of the goal as it arrived, and only of that
    /// goal: [`Solve::unify`] takes it on the way in, so what a goal decomposes
    /// into never sees it. Nothing under a projection's demand is about the
    /// base — it is about a type nested inside one — so the base's span would
    /// be pointing somewhere the reader cannot act on.
    ///
    /// Held beside the solve rather than passed down for the same reason: a
    /// parameter every recursive call had to pass `None` to would read as
    /// though the span were something the decomposition could use.
    pub base_span: Option<Span>,
}

impl Solve<'_> {
    /// Solve everything generation asked for, in the order it was asked.
    /// Every constraint is an equality, and rows are why that is enough: a
    /// projection's demand is an ordinary struct type with an open tail, so
    /// nothing has to wait for a later round to know what its base is.
    pub fn run(&mut self, constraints: &[Constraint]) {
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
            // and a recursion may not grow its arguments. See [`Ty::Named`].
            //
            // Arity belongs to the declaration, so one symbol means one count
            // and the zip drops nothing.
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
                // said once, with everything it touched pointed at
                // [`Ty::Undecided`].
                let reported = self.errors.len();
                let stepped = self.steps.len();
                let known = self.table.snapshot();
                self.step(span, Rule::Congruent, goal.clone(), Effect::Decomposed);
                self.depth += 1;
                for (x, y) in pairs {
                    self.unify(span, &x, &y);
                }
                self.depth -= 1;
                if self.errors.len() > reported {
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
                        &[lhs.clone(), rhs.clone()],
                    );
                }
            }
            (Ty::Named { .. }, _) | (_, Ty::Named { .. }) => self.unfold(span, goal, &lhs, &rhs),
            (Ty::Nat, Ty::Nat) => self.step(span, Rule::Prim, goal, Effect::None),
            // A constant meeting itself, which is [`Rule::Same`] like any other
            // pair that is already the same thing. The presences reach it from
            // no path: every presence pair goes through [`Solve::field`], which
            // decides all four combinations of the constants itself and only
            // calls back here when at least one side is still a variable. Kept
            // because being unreachable is a property of the callers rather
            // than of the type language — two presences *are* equal when they
            // are the same constant — and a fall-through to the mismatch arm
            // below would report that as a contradiction.
            //
            // Said as `Same` rather than as [`Rule::Presence`] because that
            // rule is worded about the field or case whose presence it is
            // deciding, and here there is no row in sight to have one.
            //
            // Their mismatches never reach the arm below either: a presence
            // clash is intercepted where the field's name is known, so the
            // complaint can name the field instead of saying `present` against
            // `absent`.
            (Ty::Present, Ty::Present) | (Ty::Absent, Ty::Absent) | (Ty::Empty, Ty::Empty) => {
                self.step(span, Rule::Same, goal, Effect::None)
            }
            (Ty::Arrow(from1, to1), Ty::Arrow(from2, to2)) => {
                let (from1, to1) = (from1.clone(), to1.clone());
                let (from2, to2) = (from2.clone(), to2.clone());
                self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                self.depth += 1;
                self.unify(span, &from1, &from2);
                self.unify(span, &to1, &to2);
                self.depth -= 1;
            }
            // Two rows of one shape. Two of different shapes fall through to
            // the arm below and are an ordinary mismatch: a struct and a sum
            // are two types however much their insides look alike, and lining
            // their labels up would be answering a question nobody asked.
            (Ty::Row { shape: a, .. }, Ty::Row { shape: b, .. }) if a == b => {
                self.rows(span, *a, &lhs, &rhs)
            }
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
                    _ if self.demanded(&lhs) && rhs.is_fieldless() => Error {
                        span: base_span.unwrap_or(span),
                        kind: ErrorKind::NotAStruct { base: rhs.clone() },
                    },
                    _ if self.demanded(&rhs) && lhs.is_fieldless() => Error {
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

    /// Make two rows of one shape the same row. [`Rule::Struct`] — or
    /// [`Rule::Sum`], which is the same rule about the other shape — replaces
    /// the pair with everything that has to hold of it: labels only one side
    /// names flow into the other side's tail, and the labels both name are
    /// decided one by one.
    ///
    /// The tails go first, the shared labels after. The other order would let
    /// a shared label's own unification bind one of the tails behind the
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
    fn rows(&mut self, span: Span, shape: Shape, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let mut want_repeats = Vec::new();
        let (want, want_rest) = self.canon(lhs, &mut want_repeats);
        let mut have_repeats = Vec::new();
        let (have, have_rest) = self.canon(rhs, &mut have_repeats);
        let expected = Rc::new(Ty::Row {
            shape,
            fields: want.clone(),
            rest: want_rest.clone(),
        });
        let actual = Rc::new(Ty::Row {
            shape,
            fields: have.clone(),
            rest: have_rest.clone(),
        });
        let goal = ConstraintKind::Equal {
            expected: expected.clone(),
            actual: actual.clone(),
        };

        let rule = match shape {
            Shape::Struct => Rule::Struct,
            Shape::Sum => Rule::Sum,
        };
        self.step(span, rule, goal.clone(), Effect::Decomposed);
        self.depth += 1;

        // What flattening found twice, refused now that there is a rule to
        // refuse it under. Both sides together, because it is one row that went
        // wrong however many labels it went wrong at; see [`Solve::repeat`].
        //
        // And then this goal is over. Refusing is a failure like any other, so
        // it has already abandoned every copy of every repeated field —
        // [`Solve::fail`]'s rule that reporting and recovering are one act —
        // and going on to line the two rows up would compare whichever copy
        // flattening happened to keep against a field the reader can no longer
        // see, and report a second time about a type nobody wrote.
        let repeats: Vec<_> = want_repeats.into_iter().chain(have_repeats).collect();
        if !repeats.is_empty() {
            self.repeat(span, shape, goal, &repeats);
            self.depth -= 1;
            return;
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

    /// Every field a row and its own tail both name: refused, and every copy
    /// abandoned so the contradiction is reported once rather than echoed by
    /// whatever was waiting on any of them.
    ///
    /// A row naming a field its own tail also names is not a type, and this is
    /// one of the two ways to write one: handing a declaration's `..` parameter
    /// a row that repeats a field the declaration already names, as
    /// `WithX { x: Nat }` does against `type WithX r = { x: Nat, ..r }`. The
    /// other way goes through a variable, and [`Solve::assign`]'s lacks check
    /// catches it before it is ever bound; this one has no variable to catch,
    /// because substitution put a written row straight into the tail.
    ///
    /// Refused rather than unified. Deciding the two copies against each other
    /// would accept `WithX { x: Nat }` — both copies are `Nat` — and then
    /// [`Table::zonk`] would keep one of them silently, which is exactly the
    /// failure the lacks check was added to end.
    ///
    /// Worded as [`ErrorKind::RepeatedField`], the same complaint
    /// [`Solve::assign`] makes when a tail would be *bound* to a row that
    /// repeats a field. One contradiction, one sentence, whichever way it was
    /// reached — so the whole list arrives at once, the first label is the one
    /// named, and the rest are abandoned rather than reported. Naming the first
    /// is the rule [`Table::repeated`] states: the field a reader would reach
    /// first reading the type left to right, expected side before actual.
    /// Abandoning all of them is [`Solve::fail`]'s rule: reporting and
    /// recovering are one act, and a copy left unrecovered is a variable
    /// generalization would go on to quantify.
    fn repeat(
        &mut self,
        span: Span,
        shape: Shape,
        goal: ConstraintKind,
        repeats: &[(String, RowField, RowField)],
    ) {
        let Some((name, ..)) = repeats.first() else {
            return;
        };
        let error = Error {
            span,
            kind: ErrorKind::RepeatedField {
                shape,
                field: name.clone(),
            },
        };
        let abandoned: Vec<Rc<Ty>> = repeats
            .iter()
            .flat_map(|(_, first, second)| {
                [
                    first.ty.clone(),
                    second.ty.clone(),
                    first.presence.clone(),
                    second.presence.clone(),
                ]
            })
            .collect();
        self.fail(span, Rule::Overlap { shape }, goal, error, &abandoned);
    }

    /// A row flattened: its own labels joined with every label its tail
    /// has already accumulated, and what remains of the tail — an unbound
    /// variable, [`Ty::Empty`], or [`Ty::Undecided`] — resolved as far as the
    /// solver has got.
    ///
    /// The shape is not returned because it never changes on the way down: a
    /// tail holds a row of the shape it is the tail of, which lowering enforces
    /// where an argument is written and unification enforces everywhere else.
    /// The caller knew it before this was called and still knows it after.
    ///
    /// A read and nothing else. It used to unify the labels it met twice on
    /// the way down, which made the shape of a goal something the solver
    /// settled before it had recorded the rule that was settling it; the pairs
    /// go into `repeats` instead, for [`Solve::rows`] to decide under its own
    /// step.
    ///
    /// What goes into `repeats` is a row naming a field its own tail also
    /// names, which is not a type. There is one way to write one: handing a
    /// declaration's `..` parameter a row that repeats a field the declaration
    /// already names. Every other route runs through a variable, and the lacks
    /// check in [`Solve::assign`] refuses those before anything is bound —
    /// substitution is the one that has no variable to refuse, because it puts
    /// a written row straight into the tail.
    ///
    /// The pair is collected rather than dropped because an [`IndexMap`] holds
    /// one entry per key: without somewhere for a second copy to go, flattening
    /// would silently keep one of two field types, which is the failure the
    /// lacks check exists to end. [`Solve::rows`] refuses what lands here.
    fn canon(
        &self,
        ty: &Rc<Ty>,
        repeats: &mut Vec<(String, RowField, RowField)>,
    ) -> (IndexMap<String, RowField>, Rc<Ty>) {
        let Ty::Row { fields, rest, .. } = &**ty else {
            unreachable!("canon is only called on rows");
        };
        let mut fields = fields.clone();
        let mut rest = self.table.resolve(rest);
        while let Ty::Row {
            fields: inner,
            rest: deeper,
            ..
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
    ///
    /// Structs only, though an open sum is far commoner: a tag literal is open
    /// by its nature — one case of however many — and is a type the reader
    /// wrote rather than a question the solver made up, so quoting it back at
    /// them is exactly right and this must not call it a demand.
    fn demanded(&self, ty: &Rc<Ty>) -> bool {
        let Ty::Row {
            shape: Shape::Struct,
            rest,
            ..
        } = &**ty
        else {
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
        let Ty::Row { shape, .. } = &*resolved else {
            return resolved;
        };
        let shape = *shape;
        let mut fields: IndexMap<String, RowField> = IndexMap::new();
        let mut row = resolved;
        while let Ty::Row {
            fields: named,
            rest,
            ..
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
        Rc::new(Ty::Row {
            shape,
            fields,
            rest: row,
        })
    }

    /// Decide one field both rows name: whether it is there must agree, and
    /// while it may be, the types must too. A clash of the constants is
    /// worded as the field the actual side is missing or the extra one it
    /// has, never as `present` against `absent` — the field's name is known
    /// here, and the complaint should name it.
    ///
    /// Which noun the rule is read in comes off the rows themselves, the way
    /// [`Solve::absorb`] reads the shape it continues extras as: the two are
    /// the rows this is deciding a label of, so nothing can arrive disagreeing
    /// with them.
    fn field(
        &mut self,
        span: Span,
        name: &str,
        expected_base: &Rc<Ty>,
        actual_base: &Rc<Ty>,
        want: &RowField,
        have: &RowField,
    ) {
        let Ty::Row { shape, .. } = &**expected_base else {
            unreachable!("field is only called with the rows the label belongs to");
        };
        let shape = *shape;
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
                self.fail(
                    span,
                    Rule::Presence { shape },
                    goal,
                    Error { span, kind },
                    &abandoned,
                );
            }
            (Ty::Absent, Ty::Present) => {
                let kind = ErrorKind::ExtraField {
                    base: self.frozen(expected_base),
                    field: name.to_string(),
                };
                let abandoned = [want.ty.clone(), have.ty.clone()];
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

    /// Push the labels only one row names into the other row's tail, to
    /// continue as `rest`. `side` says which side of the goal the tail sits
    /// on, so that every act performed on its behalf keeps the direction the
    /// constraint was worded in; `base` is the row the tail belongs to,
    /// which is the type a complaint names, and `shape` is what the row the
    /// extras continue as has to be.
    ///
    /// An open tail takes the row whole — one binding, occurs-checked like
    /// any other. A closed tail takes nothing: each label must turn out
    /// absent, one that certainly is not is a missing or extra label by
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
        // What the extras continue as is a row of the shape they came out of,
        // and `base` is that row — so it is read off rather than passed
        // alongside, which is one less thing that could arrive disagreeing
        // with it.
        let Ty::Row { shape, .. } = &**base else {
            unreachable!("absorb is only called with the row the tail belongs to");
        };
        let shape = *shape;
        if !matches!(&**tail, Ty::Empty) {
            let row = Rc::new(Ty::Row {
                shape,
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
                        Rule::Presence { shape },
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
                        Rule::Presence { shape },
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
    /// [`Ty::Named`], which states the rule, and [`Solve::nominal`], which is
    /// where the "bound to be" is checked.
    fn unfold(&mut self, span: Span, goal: ConstraintKind, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let pair = match (&**lhs, &**rhs) {
            (Ty::Named { .. }, Ty::Named { .. }) => Some((lhs.clone(), rhs.clone())),
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
        if let Some((shape, field)) = self.table.repeated(var, ty) {
            let error = Error {
                span,
                kind: ErrorKind::RepeatedField { shape, field },
            };
            self.fail(
                span,
                Rule::Overlap { shape },
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
            Ty::Row { fields, rest, .. } => {
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
