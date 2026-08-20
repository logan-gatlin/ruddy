//! Pass two: solving. See [`Solve`].

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use indexmap::IndexMap;

use crate::{
    symbol::Symbol,
    tracking::Span,
    types::{
        Assigned, Core, Formula, Presence, Rest, Row, RowField, Scheme, Sense, Shape, Ty, TyVar,
    },
};

use super::{
    Constraint, ConstraintKind, Effect, Error, ErrorKind, Goal, Rule, Side, Slot, Step, Table,
};

/// What a set of labels says about the ones it does not name.
///
/// The one thing the two shapes still differ in, and so the one thing the shared
/// label rule has to be told. A struct's fields run out where the core beside
/// them says they do — `{ x: Nat, ..'r }` is the type `'r` carrying an `x` —
/// and a
/// sum's cases run out where its row's [`Rest`] says they do, because
/// [`Core::Sum`] is the only core with a case row and there is nowhere else for
/// a sum's tail to live.
///
/// Everything else about matching two sets of labels — the extras each way, the
/// shared ones, the presence clashes, the noun a complaint is worded in — is the
/// same question twice, so it is written once and reaches both through this. Two
/// copies of it that have to agree is a defect, not an implementation choice.
#[derive(Debug, Clone)]
enum Tail {
    Core(Core),
    Rest(Rest),
    /// An arrow's effects: what is past them, and the two sides they sit
    /// beside.
    ///
    /// A variant of its own rather than a shape carried beside a [`Rest`],
    /// because what a complaint about it names is different: a sum's labels are
    /// the whole of its core, and an arrow's effects are one of three things
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
/// type. `(#A 1).x` is a missing *field* on a sum-cored type: the labels that
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
            Tail::Core(Core::Var(var))
            | Tail::Rest(Rest::Var(var))
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
            Tail::Core(Core::Rigid { id, name })
            | Tail::Rest(Rest::Rigid { id, name })
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
            Tail::Core(Core::Undecided)
                | Tail::Rest(Rest::Undecided)
                | Tail::Effects {
                    rest: Rest::Undecided,
                    ..
                }
        )
    }

    /// Whether this tail allows nothing more at all — every label it does not
    /// name is absent. Neither open nor abandoned is the whole of it: `Nat`
    /// carries the fields written beside it and no others, exactly as a closed
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

    /// Which of the two sets of labels this is a tail of, which is the reading
    /// every complaint under it is worded in.
    fn shape(&self) -> Shape {
        match self {
            Tail::Core(_) => Shape::Struct,
            Tail::Rest(_) => Shape::Sum,
            Tail::Effects { .. } => Shape::Effect,
        }
    }

    /// This tail carrying `labels`, as the value a variable takes: a whole type
    /// for a struct's fields, a row for a sum's cases or an arrow's effects.
    fn value(&self, labels: IndexMap<String, RowField>) -> Assigned {
        match self {
            Tail::Core(core) => Assigned::Ty(Rc::new(Ty {
                core: core.clone(),
                fields: labels,
            })),
            Tail::Rest(rest) | Tail::Effects { rest, .. } => Assigned::Row(Rc::new(Row {
                labels,
                rest: rest.clone(),
            })),
        }
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
    /// The fields a type carries, which every type has.
    fn fields(ty: &'a Rc<Ty>, tail: &'a Tail) -> Self {
        Self {
            ty,
            labels: &ty.fields,
            tail,
        }
    }

    /// The cases a sum-cored type allows. The row is handed in rather than
    /// matched out, so that the one caller that knows the core is a sum is the
    /// one that says so.
    fn cases(ty: &'a Rc<Ty>, cases: &'a IndexMap<String, RowField>, tail: &'a Tail) -> Self {
        Self {
            ty,
            labels: cases,
            tail,
        }
    }

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
            Tail::Core(core) => Rc::new(Ty {
                core: core.clone(),
                fields: labels,
            }),
            Tail::Rest(rest) => Rc::new(Ty {
                core: Core::Sum(Row {
                    labels,
                    rest: rest.clone(),
                }),
                fields: self.ty.fields.clone(),
            }),
            // The arrow with its effect row replaced: what the reader wrote,
            // said as far as the solve has got. The two sides stay as they are,
            // since the goal that failed was about the effects alone.
            Tail::Effects { from, to, rest } => Rc::new(Ty {
                core: Core::Arrow(
                    from.clone(),
                    to.clone(),
                    Row {
                        labels,
                        rest: rest.clone(),
                    },
                ),
                fields: self.ty.fields.clone(),
            }),
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
    /// The scheme each nested `let` currently in scope published, for as long
    /// as its body is being solved. [`Constrain`](super::Constrain)'s
    /// environment, about the one kind of name generation could not decide.
    pub schemes: HashMap<Symbol, Scheme>,
    /// Every scheme published, kept. [`Output::locals`](super::Output::locals).
    pub locals: &'a mut IndexMap<Symbol, Scheme>,
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
                ConstraintKind::Instance { symbol, ty } => self.instance(span, *symbol, ty),
                ConstraintKind::Performs {
                    performed,
                    ambient,
                    inside,
                } => self.performs(span, performed, ambient, *inside),
            }
        }
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
        let expects = Rowed::cases(&want_ty, &opened.labels, &left);
        let actuals = Rowed::cases(&have_ty, &have.labels, &right);
        self.labels(span, expects, actuals);
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
    fn instance(&mut self, span: Span, symbol: Symbol, ty: &Rc<Ty>) {
        let scheme = self.schemes[&symbol].clone();
        // At the level of the use site, which is where the table is: a copy is
        // as new as the place it was made, whatever the scheme was generalized
        // at.
        let copy = self.table.instantiate(span, &scheme);
        self.unify(span, &copy, ty);
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the solve continues.
    ///
    /// Two types are equal when they name the same labels with the same
    /// presences and types, and their cores agree about everything else. One
    /// function decides a type; there is no separate rule for a struct, because
    /// a struct is a [`Core::Unit`] carrying labels and every other type carries
    /// labels too.
    ///
    /// Four steps, in this order:
    ///
    /// 1. [`Core::Undecided`] on either side absorbs, as it always has.
    /// 2. A *bare* variable — a [`Core::Var`] carrying no labels — takes the
    ///    whole type it is against, fields and all, which is what keeps
    ///    `fn x => x` inferring `a -> a`. Before unfolding, so that one
    ///    against a declared type takes the type by the name it was written as:
    ///    a definition then reads as its annotation said, and the solver has one
    ///    less thing to unfold later.
    /// 3. A name beside labels is unfolded and the goal asked again. See
    ///    [`Solve::unwrapped`].
    /// 4. Everything else is [`Solve::fielded`]: the labels and the core, in the
    ///    order it gives.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
        let lhs = self.table.resolve(expected);
        let rhs = self.table.resolve(actual);
        let goal = Goal::Type {
            expected: lhs.clone(),
            actual: rhs.clone(),
        };
        // One variable against itself is already the same thing, so the arms
        // below must not read it as a variable against a type and try to bind
        // it to itself. Its labels may still differ, which is what the
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
            (Core::Var(var), _) if !itself && lhs.fields.is_empty() => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(rhs.clone()));
            }
            (_, Core::Var(var)) if !itself && rhs.fields.is_empty() => {
                let var = *var;
                self.assign(span, goal, var, Assigned::Ty(lhs.clone()));
            }
            _ => self.fielded(span, goal, &lhs, &rhs),
        }
    }

    /// Decide two types by their labels and their cores.
    ///
    /// A type carrying no labels on either side is its core and nothing else, so
    /// the core's own rule is the whole step and the trace reads exactly as it
    /// did before fields were a property of every type: `Nat` against `Nat` is
    /// one [`Rule::Prim`] and nothing more.
    ///
    /// Where either side carries a label there is a [`Rule::Struct`] over the
    /// whole of it, with the label steps and the core step one level under. Two
    /// [`Core::Unit`] cores record no step of their own — they are already the
    /// same thing — which is what keeps a struct against a struct reading byte
    /// for byte as it always has.
    ///
    /// Which of the two halves goes first is decided by whether a core variable
    /// is involved, and only there does the order differ from what it was. A
    /// core variable is what the extras have to be absorbed into, so the labels
    /// have to be settled before it can be told what it stands for: `1.x`
    /// records the fields and their failure, and no longer binds the base's core
    /// first. Where neither core is a variable there is nothing to absorb into
    /// and the cores are the larger question — two types that cannot be equal at
    /// all are a mismatch of what the reader wrote, not a field missing from a
    /// type that was never the right one — so those are decided first and the
    /// labels only if they agreed.
    ///
    /// Either way the two halves are one goal, so a failure in one abandons the
    /// other: a core bound to a fieldless type with labels it could not take
    /// beside it is a type nothing can be. See [`Solve::abandon`].
    fn fielded(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let carried = !(lhs.fields.is_empty() && rhs.fields.is_empty());
        let named =
            matches!(lhs.core, Core::Named { .. }) || matches!(rhs.core, Core::Named { .. });
        if carried && named {
            return self.unwrapped(span, goal, lhs, rhs);
        }
        // The core step's goal is the two cores, each as a type carrying no
        // labels, so a binding reads `?1 ~ {}` rather than repeating the whole
        // types the step above already showed. With no labels on either side the
        // two are the same thing said twice, and this is that one thing.
        let cores = Goal::Type {
            expected: Rc::new(Ty::plain(lhs.core.clone())),
            actual: Rc::new(Ty::plain(rhs.core.clone())),
        };
        if !carried {
            self.cores(span, cores, lhs, rhs);
            return;
        }

        self.step(span, Rule::Struct, goal, Effect::Decomposed);
        self.depth += 1;
        let absorbing = matches!(lhs.core, Core::Var(_)) || matches!(rhs.core, Core::Var(_));
        let reported = self.errors.len();
        if absorbing || self.cores(span, cores, lhs, rhs) {
            let (left, right) = (Tail::Core(lhs.core.clone()), Tail::Core(rhs.core.clone()));
            self.labels(span, Rowed::fields(lhs, &left), Rowed::fields(rhs, &right));
            if self.errors.len() > reported {
                self.abandon(span, lhs, rhs);
            }
        }
        self.depth -= 1;
    }

    /// Replace a declared type beside labels with what it stands for, and ask
    /// the goal again.
    ///
    /// What a declaration stands for keeps its fields behind the name, so
    /// binding a core variable to the name would leave the labels beside it with
    /// nothing to be decided against — which is why this comes before the labels
    /// rather than after. It is the rule `cores` has always applied to a name
    /// against a shape, restated for the order the labels now go in, and it is
    /// what keeps `a.x` working where `a : WithX { y: Nat }`.
    ///
    /// Recorded as [`Rule::Unfold`] with [`Effect::Decomposed`], the same rule
    /// as [`Solve::unfold`]'s two-sided unfolding, and *not* pushed onto
    /// [`Solve::assumed`]: that stack is keyed on a goal about two names, and
    /// this is one side of one type.
    ///
    /// Termination is [`ir::build`](crate::ir)'s check and not the assumption
    /// stack. Each unfold replaces a name with its body under the arguments
    /// written at that mention, so the chain descends either along the
    /// declarations' core graph, which
    /// [`ir::ErrorKind::EndlessFields`](crate::ir::ErrorKind) makes acyclic, or
    /// into a strictly smaller written argument. Both are finite. That is the
    /// whole of why this rule is safe, and none of it may be relaxed without
    /// coming back here first.
    fn unwrapped(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        let lhs = self.table.unfolded(self.aliases, lhs);
        let rhs = self.table.unfolded(self.aliases, rhs);
        self.step(span, Rule::Unfold, goal, Effect::Decomposed);
        self.depth += 1;
        self.unify(span, &lhs, &rhs);
        self.depth -= 1;
    }

    /// Decide two cores, and say whether the labels beside them are still worth
    /// deciding. A goal that failed decides nothing, and one that was replaced —
    /// an unfolding — carries its own fields along with it.
    ///
    /// A core variable bound here takes the *core* it is against and no labels,
    /// not the whole type: the labels are the caller's to decide, and binding
    /// them in as well would decide them twice.
    fn cores(&mut self, span: Span, goal: Goal, lhs: &Rc<Ty>, rhs: &Rc<Ty>) -> bool {
        match (&lhs.core, &rhs.core) {
            // Two types with nothing of their own are already the same thing,
            // and saying so would be a step about nothing — which is what keeps
            // a struct against a struct recording exactly what it always did.
            (Core::Unit, Core::Unit) => true,
            (Core::Var(a), Core::Var(b)) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None);
                true
            }
            // A variable is equal to itself and to nothing else.
            // Two rigids with one id are the same variable however differently
            // the two sides were reached; two with different ids are two
            // promises about two independent choices the caller makes, and one
            // is no more the other than `Nat` is.
            (Core::Rigid { id: a, .. }, Core::Rigid { id: b, .. }) if a == b => {
                self.step(span, Rule::Same, goal, Effect::None);
                true
            }
            // Meeting anything else is the promise broken — except an unbound
            // variable, which takes the rigid as it would take any other type
            // and is left to the binding rules below. A declared name is not
            // looked through first: what the reader wrote is the name, and
            // unfolding it would quote them a shape they never put on the page.
            (Core::Rigid { name, id }, other) if !matches!(other, Core::Var(_)) => {
                let (name, id) = (name.clone(), *id);
                self.rigid_broken(span, goal, name, id, Sense::Type, rhs)
            }
            (other, Core::Rigid { name, id }) if !matches!(other, Core::Var(_)) => {
                let (name, id) = (name.clone(), *id);
                self.rigid_broken(span, goal, name, id, Sense::Type, lhs)
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
                self.mismatch(span, goal, lhs, rhs)
            }
            (Core::Named { .. }, _) | (_, Core::Named { .. }) => {
                self.unfold(span, goal, lhs, rhs);
                false
            }
            // A core variable takes the core it is against. A name is looked
            // through before this, in [`Solve::unwrapped`], for the reason that
            // rule gives; a *bare* variable is taken before either, in
            // [`Solve::unify`], because there is nothing beside it to decide.
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
            (Core::Nat, Core::Nat)
            | (Core::Int, Core::Int)
            | (Core::Real, Core::Real)
            | (Core::String, Core::String)
            | (Core::Boolean, Core::Boolean) => {
                self.step(span, Rule::Prim, goal, Effect::None);
                true
            }
            // Three goals rather than two, and the third is not opened: an
            // annotation says what it says, so `let h : Nat -> Nat = f` with
            // `f : Nat -> Nat + !Log` is refused. Opening happens where a
            // function is *applied* and nowhere else. See R13.
            (Core::Arrow(from1, to1, does1), Core::Arrow(from2, to2, does2)) => {
                let (from1, to1) = (from1.clone(), to1.clone());
                let (from2, to2) = (from2.clone(), to2.clone());
                let (want, have) = (self.table.canon(does1), self.table.canon(does2));
                self.step(span, Rule::Arrow, goal, Effect::Decomposed);
                self.depth += 1;
                self.unify(span, &from1, &from2);
                self.unify(span, &to1, &to2);
                let (left, right) = (
                    Tail::Effects {
                        from: from1.clone(),
                        to: to1.clone(),
                        rest: want.rest.clone(),
                    },
                    Tail::Effects {
                        from: from2.clone(),
                        to: to2.clone(),
                        rest: have.rest,
                    },
                );
                let expects = Rowed::cases(lhs, &want.labels, &left);
                let actuals = Rowed::cases(rhs, &have.labels, &right);
                self.labels(span, expects, actuals);
                self.depth -= 1;
                true
            }
            // Two sums: the same row code, read in cases. A sum against
            // anything else falls through to the arm below and is an ordinary
            // mismatch — a struct and a sum are two types however much their
            // insides look alike, and lining their labels up would be answering
            // a question nobody asked.
            (Core::Sum(cases), Core::Sum(others)) => {
                let (want, have) = (self.table.canon(cases), self.table.canon(others));
                let (left, right) = (Tail::Rest(want.rest.clone()), Tail::Rest(have.rest.clone()));
                let expects = Rowed::cases(lhs, &want.labels, &left);
                let actuals = Rowed::cases(rhs, &have.labels, &right);
                // Flattened first and each put back in the type it came out of,
                // which is what the goal is recorded as: a tail already bound to
                // a row is where that differs from the rows as they arrived, and
                // it is exactly the case where the unflattened goal cannot
                // account for its own children — the labels under it come from a
                // row the line above does not show.
                let expected = expects.rebuild(want.labels.clone());
                let actual = actuals.rebuild(have.labels.clone());
                let goal = Goal::Type { expected, actual };
                self.step(span, Rule::Sum, goal, Effect::Decomposed);
                self.depth += 1;
                self.labels(span, expects, actuals);
                self.depth -= 1;
                true
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
    /// Named as the two whole types rather than as their cores: a `Nat` against
    /// `{ x: Nat }` is a mismatch of what the reader wrote, and the cores alone
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
    fn labels(&mut self, span: Span, lhs: Rowed<'_>, rhs: Rowed<'_>) {
        let goal = Goal::Type {
            expected: lhs.ty.clone(),
            actual: rhs.ty.clone(),
        };

        let mut only_want: IndexMap<String, RowField> = lhs
            .labels
            .iter()
            .filter(|(name, _)| !rhs.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();
        let mut only_have: IndexMap<String, RowField> = rhs
            .labels
            .iter()
            .filter(|(name, _)| !lhs.labels.contains_key(*name))
            .map(|(name, field)| (name.clone(), field.clone()))
            .collect();

        // Two sides sharing one tail cannot differ in labels: whatever the
        // tail absorbed from one side it would grow on the other, and the
        // two would chase each other forever. The occurs check inside
        // `assign` refuses the binding when only one side has extras, but the
        // both-sided case binds cleanly every round and never converges, so
        // the pair is refused here — the same cycle, caught one level up.
        //
        // A label certainly absent is the exception. `{ \y, ..'r }` against
        // `{ ..'r }` differs in nothing a value could have: the absence says
        // `'r` may not stand for a `y`, which is the condition the row already
        // put on `'r` when it was written — so the label grows nothing and the
        // two sides are one row. The condition is said of the tail once more
        // here, since this path skips the absorption that would otherwise
        // carry it, and the labels ask nothing further.
        if let (Some(a), Some(b)) = (lhs.tail.var(), rhs.tail.var())
            && a == b
            && !(only_want.is_empty() && only_have.is_empty())
        {
            let absent = |field: &RowField| {
                matches!(self.table.presence_of(&field.presence), Presence::Absent)
            };
            if only_want.values().all(&absent) && only_have.values().all(&absent) {
                let labels: Vec<String> =
                    only_want.keys().chain(only_have.keys()).cloned().collect();
                self.table.forbidden(a, lhs.shape(), labels);
                only_want.clear();
                only_have.clear();
            } else {
                let error = Error {
                    span,
                    kind: ErrorKind::Recursive,
                };
                let abandoned = [
                    lhs.tail.value(lhs.labels.clone()),
                    rhs.tail.value(rhs.labels.clone()),
                ];
                self.fail(span, Rule::Occurs, goal, error, &abandoned);
                return;
            }
        }

        match (only_want.is_empty(), only_have.is_empty()) {
            // The two name the same labels, so what each allows beyond them is
            // simply the other. Two closed tails already agree, and saying so
            // would be a step about nothing.
            (true, true) => {
                if !(lhs.tail.closed() && rhs.tail.closed()) {
                    self.tails(span, lhs.tail, rhs.tail);
                }
            }
            (true, false) => self.absorb(span, Side::Expected, only_have, rhs.tail, lhs),
            (false, true) => self.absorb(span, Side::Actual, only_want, lhs.tail, rhs),
            // Extras both ways continue as one fresh tail, which is what makes
            // the two sides end as one rather than merely overlapping — and
            // what closes both sides against each other where neither has room
            // for them, since the tail one closes then closes the other.
            (false, false) => {
                let rest = self.fresh_tail(lhs.shape());
                self.absorb(span, Side::Expected, only_have, &rest, lhs);
                self.absorb(span, Side::Actual, only_want, &rest, rhs);
            }
        }

        let shared: Vec<_> = lhs
            .labels
            .iter()
            .filter_map(|(name, field)| {
                rhs.labels
                    .get(name)
                    .map(|other| (name.clone(), field.clone(), other.clone()))
            })
            .collect();
        for (name, want, have) in shared {
            self.field(span, &name, lhs, rhs, &want, &have);
        }
    }

    /// A variable standing for whatever a set of labels of this shape allows
    /// beyond the ones it names: a fresh core for a struct's fields, a fresh
    /// tail for a sum's cases. One variable table and one sort per position, as
    /// everywhere else.
    fn fresh_tail(&mut self, shape: Shape) -> Tail {
        match shape {
            Shape::Struct => Tail::Core(Core::Var(self.table.fresh_core())),
            Shape::Sum => Tail::Rest(self.table.fresh_row()),
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
    /// whichever sort they are: the core rule for a struct's fields, and
    /// [`Solve::rests`] for a sum's cases.
    ///
    /// The one place the shared rule has to know which shape it is on, and the
    /// reason it is one line: everything above this is the same question about
    /// two sets of labels, and everything below it is a question the two shapes
    /// answer with different machinery.
    fn tails(&mut self, span: Span, lhs: &Tail, rhs: &Tail) {
        match (lhs, rhs) {
            // Both rows, whichever of the two readings: what is past a set of
            // labels is a row either way, and which shape it was is the
            // wording's business rather than the rule's.
            (
                Tail::Rest(left) | Tail::Effects { rest: left, .. },
                Tail::Rest(right) | Tail::Effects { rest: right, .. },
            ) => {
                // Which reading the pair is, for the one complaint down there
                // that quotes it: a rigid rest broken in a sum is quoted as the
                // sum it refused, and one broken in an effect row as the arrow
                // that would carry it.
                let shape = match (lhs, rhs) {
                    (Tail::Effects { .. }, _) | (_, Tail::Effects { .. }) => Shape::Effect,
                    _ => Shape::Sum,
                };
                self.rests(span, left, right, shape);
            }
            // Two cores, and nothing else can reach here: which shape a set of
            // labels is, is decided by the syntax that wrote it, and the two
            // sides of a goal were matched on it before either arrived. So the
            // pair is read as the two types the cores are, which is what a
            // struct's tail *is*.
            //
            // Resolved, because a core reaching here may be a variable this very
            // rule closed a moment ago — [`Solve::absorb`] settles both sides of
            // a pair one after the other, and the second must see what the first
            // decided rather than bind the variable twice.
            (left, right) => {
                let left = self.table.resolve(&left.bare().as_ty());
                let right = self.table.resolve(&right.bare().as_ty());
                let goal = Goal::Type {
                    expected: left.clone(),
                    actual: right.clone(),
                };
                self.cores(span, goal, &left, &right);
            }
        }
    }

    /// Make two of a sum's tails the same tail: a variable takes the other, and
    /// an undecided one absorbs it. The struct's half of [`Solve::tails`] is the
    /// core rule; this is the other.
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
            // The core rule's arms about a sum's rest, and the same rule: a
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
    ) {
        let shape = expected.shape();
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
            let goal = goal_of(expected, actual);
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
    /// `type T 'a = { next: T { x: 'a } }` grows its argument every round, so
    /// no goal about it is ever asked twice, and the stack only gets longer.
    /// Without (3) the key would be unsound rather than merely coarse —
    /// keyed on the two names alone, a declaration reached inside its own group
    /// at two different arguments has its second question answered by the first
    /// question's assumption, and two types that differ are accepted as equal.
    /// That is the whole reason the arguments are here.
    ///
    /// And (4) is trusted bare: no fuel backs it the way [`Table::resolve`]'s
    /// budget backs the occurs check, so a declaration
    /// [`ir::build`](crate::ir::build) wrongly let through is a hang here, not
    /// a panic. Deliberate: the bound (4) actually promises — every list drawn
    /// from a finite set of arguments — is combinatorial in the program, so
    /// any counter tight enough to fire on a bug is tight enough to fire on a
    /// correct program first. The refusals in `ir::build` are the whole
    /// guarantee, and a change to them must be measured against this map.
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
    /// stands for the labels its row does not write out, so a row that
    /// certainly has one of them is not a value that tail can take — and a
    /// core variable is under the same condition, since it stands for a type
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

    /// Abandon the cores of two types whose labels could not be made to agree.
    ///
    /// [`fail`](Self::fail) abandons what a failing rule was about, and a core
    /// is the one thing it cannot reach. A type is its core and its fields, both
    /// decided by one goal, so a failure over the fields is a failure of the
    /// whole of it — and the core was bound in between, by the step before. Left
    /// standing, a core variable bound to a fieldless type while labels sit
    /// beside it is a type nothing can be: the definition publishes it, and
    /// every use of the definition is refused again for the one mistake already
    /// reported. `let f = fn p => { a: p.x, b: p 1 }` is the shape of it — `p`
    /// would have to be a function carrying an `x` — and that is `f`'s mistake,
    /// said once, wherever `f` is then used.
    ///
    /// Settled rather than recovered because there is no longer a variable to
    /// recover: this goal bound it, so [`recover_ty`](Self::recover_ty) would
    /// resolve straight through the binding and find nothing to abandon.
    ///
    /// The cores and nothing else. What the two types hold besides them — a
    /// label's type, an arrow's halves — was not decided by the goal that
    /// failed, and a complaint already made names one of these two types: a
    /// reader is better served by ``no field `y` on `a -> a` `` than by the
    /// same sentence about `? -> ?`.
    fn abandon(&mut self, span: Span, lhs: &Rc<Ty>, rhs: &Rc<Ty>) {
        for core in [&lhs.core, &rhs.core] {
            if let Core::Var(var) = core {
                let var = *var;
                // Unless the failure already reached it. [`fail`](Self::fail)
                // abandons the values the rule was about, and where the labels
                // go first those values are whole types with these very cores
                // in them — so a core may already be pointed at the undecided
                // type, and settling it again would show a reader one act as
                // two. Which also covers the two sides being one variable.
                let settled = self.table.resolve(&Rc::new(Ty::plain(Core::Var(var))));
                if matches!(settled.core, Core::Undecided) {
                    continue;
                }
                self.settle(span, var, Assigned::Ty(Rc::new(Ty::default())));
            }
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
            Core::Arrow(from, to, effects) => {
                let (from, to, effects) = (from.clone(), to.clone(), effects.clone());
                self.recover_ty(span, &from);
                self.recover_ty(span, &to);
                self.recover_row(span, &effects);
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
            Core::Unit
            | Core::Nat
            | Core::Int
            | Core::Real
            | Core::String
            | Core::Boolean
            | Core::Bound(_)
            | Core::Rigid { .. }
            | Core::Undecided => {}
        }
        let fields = ty.fields.clone();
        self.recover_labels(span, &fields);
    }

    /// [`recover`](Self::recover) over a sum's cases: every label, and then the
    /// tail saying what else the row might have had.
    fn recover_row(&mut self, span: Span, row: &Row) {
        let row = self.table.canon(row);
        self.recover_labels(span, &row.labels);
        self.recover_rest(span, &row.rest);
    }

    /// [`recover`](Self::recover) over a label map: each label whole, its
    /// presence and its type. Missing one would leave a variable unbound for
    /// generalization to quantify.
    fn recover_labels(&mut self, span: Span, labels: &IndexMap<String, RowField>) {
        for field in labels.values() {
            let (presence, ty) = (field.presence.clone(), field.ty.clone());
            self.recover_presence(span, &presence);
            self.recover_ty(span, &ty);
        }
    }

    /// [`recover`](Self::recover) over a tail.
    fn recover_rest(&mut self, span: Span, rest: &Rest) {
        if let Rest::Var(var) = self.table.canon(&Row::of(rest.clone())).rest {
            self.settle(span, var, Assigned::Row(Rc::new(Row::of(Rest::Undecided))));
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
        Shape::Effect => (Sense::Effects, row_ty(row)),
        _ => (Sense::Cases, Rc::new(Ty::plain(Core::Sum(row.clone())))),
    }
}

fn row_ty(row: &Row) -> Rc<Ty> {
    Rc::new(Ty::plain(Core::Arrow(
        Rc::new(Ty::unit()),
        Rc::new(Ty::unit()),
        row.clone(),
    )))
}

fn goal_of(expected: Assigned, actual: Assigned) -> Goal {
    match (expected, actual) {
        (Assigned::Row(expected), Assigned::Row(actual)) => Goal::Row { expected, actual },
        // Anything else is two types. A presence never reaches here — nothing
        // builds one out of a tail — and two values of different sorts never
        // meet, for the reason [`Solve::tails`] gives.
        (expected, actual) => Goal::Type {
            expected: expected.as_ty(),
            actual: actual.as_ty(),
        },
    }
}
