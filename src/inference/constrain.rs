//! Pass one: generation. See [`Constrain`].

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    ir::{self, Term, TermKind},
    symbol::{Mint, Symbol},
    tracking::{Span, Tracked},
    types::{Core, Formula, Presence, Rest, Row, RowField, Scheme, Shape, Ty},
};

use super::{
    Annotated, Binding, Constraint, ConstraintKind, Coverage, Named, Origin, Table,
    lower_annotation, same_field_set,
};

/// Pass one: the walk that says what has to hold, and solves nothing.
pub struct Constrain<'a> {
    pub table: &'a mut Table,
    /// How to spell a declared type's name, for the one arm that lowers a
    /// written type: a nested `let`'s annotation. Nothing is decided by it —
    /// see [`lower_type`], where the mint is only ever a speller.
    pub mint: &'a Mint,
    /// What each symbol in scope means. Symbols are globally unique, so one
    /// flat map serves every scope at once and nothing is ever popped: a
    /// lambda argument can never collide with a top-level definition.
    pub env: &'a mut HashMap<Symbol, Binding>,
    /// What the declared types stand for, for the two arms that have to see a
    /// shape rather than a name: applying something annotated `Endo`, and
    /// checking a term against an annotation of `list`.
    ///
    /// Reading this is not reading the variable table. It is fixed before the
    /// first definition is walked and mentions no variable, so an arm that
    /// consults it still cannot depend on how much an earlier arm had solved.
    pub aliases: &'a IndexMap<Symbol, Scheme>,
    pub out: Vec<Constraint>,
    /// Every annotation this walk lowered for a nested `let`, and the variables
    /// each left open. See [`Annotated`].
    pub annotated: Vec<Annotated>,
    /// What each effect's operations were declared to be: the argument and the
    /// result of the plain closed arrow performing one has.
    ///
    /// Read by the two arms that need a signature — an operation reference, and
    /// a handler arm, whose binder and body type come straight off it. Fixed
    /// before the first definition is walked and mentioning no variable, so
    /// reading it is not reading the table.
    pub operations: &'a Operations,
    /// Structural row identity for each source-resolved effect declaration.
    /// Symbols remain the operation-table keys; rows use this map instead.
    pub effect_ids: &'a IndexMap<Symbol, crate::types::EffectId>,
    /// The effects the place being walked allows, and whether a `fn` encloses
    /// it.
    ///
    /// R11's ambient. A definition's value is walked at the empty closed row —
    /// a value is computed where no handler can reach it — a `fn` mints a fresh
    /// row variable and walks its body at that, a handler hands its body a
    /// larger one, and every other form passes what it was given down
    /// unchanged.
    pub ambient: Ambient,
    /// The answer type of the innermost handler arm enclosing the walk, or
    /// `None` where no arm does.
    ///
    /// What a `raise` is checked against. Lowering has already refused a
    /// `raise` with no arm around it, and one under a `fn` inside an arm, so a
    /// `None` here is a term that already drew a complaint — and the arm rule
    /// mirrors the placement rule exactly: an arm's body sets it, a `fn` body
    /// clears it, and a nested `handle`'s *body* keeps whatever it was handed,
    /// because the outer handler is still on the stack while the inner one
    /// runs.
    pub answer: Option<Rc<Ty>>,
}

/// What every effect's operations were declared to be, by the effect and the
/// operation's own name: the argument and the result of the plain closed arrow
/// performing one has.
///
/// Named because two readers want it — generation, for an operation reference
/// and a handler arm — and because the pair of pairs reads as nothing at all
/// written out at each of them.
///
/// Insertion-ordered, because a third reader publishes it: [`Output::operations`](crate::inference::Output::operations)
/// hands the same table on to [`lir`](crate::lir), and a map whose order came
/// out of a hash would print differently from one run to the next.
pub type Operations = IndexMap<(Symbol, String), (Rc<Ty>, Rc<Ty>)>;

/// Where a term sits, as far as effects are concerned: what may be performed
/// there, and whether a `fn` encloses it.
///
/// The flag is not a second reading of the row. Both a top-level definition and
/// a function annotated `() -> Nat` are walked at the empty closed row, and the
/// two failures are not the same failure — one has no function to widen — so
/// which of them it is has to travel with the row rather than be read off it.
#[derive(Debug, Clone)]
pub struct Ambient {
    pub row: Row,
    pub inside: bool,
}

/// One entry of a match's column at one position: the sub-pattern an arm wrote
/// there, or the unit a bare tag's payload demands without anything having
/// been written — `#None` is `#None ()` to the types, and only to the
/// types. Each entry remembers which arm it came from, because a binder's view
/// is refined against the arms *above its own*; see [`Constrain::position`].
enum Col<'a> {
    Pattern(&'a ir::Pattern),
    Unit,
}

/// Everything one match's columns are read against: the tests-by-position the
/// written matrix makes — the universes behind the handled-case refinement —
/// and the arms' patterns in order, so a binder in arm `i` can ask what the
/// arms before `i` fully handle. The span is the scrutinee's, where every
/// demand the columns build is aimed.
struct Columns<'a> {
    matrix: ir::Matrix,
    patterns: Vec<&'a ir::Pattern>,
    at: Span,
}

/// What one column contributes to the covered set: the conjunction of presence
/// literals each arm demands at this position and everywhere below it, by arm.
///
/// `None` when the column does not qualify — see R6. Which is not the same as
/// an empty map: a column of nothing but binders qualifies and contributes
/// nothing, which is the formula that is always true and the reason such a
/// column leaves the match exhaustive.
///
/// Only the arms that reach the position are keyed. An arm that never gets here
/// — a field it did not mention — says nothing about it, and the enclosing
/// disjunct is where its own literals already are.
type Cover = Option<IndexMap<usize, Formula>>;

/// The covered set of one qualifying column: one disjunct per arm that reaches
/// it, as R6 defines them.
///
/// An exact column's universe is the closed field set — every label anything in
/// the column mentions — so an arm that does *not* mention one of them is
/// saying it is not there, and the literal is negated. An open column claims
/// nothing about what it did not mention, so only the positives go in. Either
/// way a nested qualifying sub-pattern's literals join the same conjunction:
/// `{x: {a}}` covers "an `x`, no `y`, and an `a` inside the `x`", which is one
/// disjunct rather than two constraints.
///
/// A binder or a wildcard covers the position outright, which is the empty
/// conjunction — and a disjunction with that in it is [`Formula::True`], so a
/// column with one emits nothing, exactly as R6 says.
fn covered(
    entries: &[(usize, Col)],
    named: &IndexMap<String, RowField>,
    nested: &IndexMap<String, IndexMap<usize, Formula>>,
    exact: bool,
) -> IndexMap<usize, Formula> {
    let mut out = IndexMap::new();
    for (arm, entry) in entries {
        let mentioned: Vec<&str> = match entry {
            // A bare tag's payload demands unit, which is the exact struct
            // naming no fields — the same reading `()` gets.
            Col::Unit => Vec::new(),
            Col::Pattern(pattern) => match &pattern.tracked {
                ir::PatternKind::Unit => Vec::new(),
                ir::PatternKind::Struct { fields, .. } => {
                    fields.keys().map(String::as_str).collect()
                }
                // A binder or a wildcard: nothing is demanded here, so the
                // conjunction is empty and the arm covers whatever reaches it.
                _ => {
                    out.insert(*arm, Formula::True);
                    continue;
                }
            },
        };
        let here = named.iter().filter_map(|(name, field)| {
            let literal = field.presence.formula();
            match (mentioned.contains(&name.as_str()), exact) {
                (true, _) => Some(literal),
                (false, true) => Some(literal.not()),
                (false, false) => None,
            }
        });
        // Indexed rather than looked up: a column only reaches here having
        // qualified, and qualifying is every sub-position answering, so every
        // field mentioned anywhere in it has an entry.
        let below = mentioned
            .iter()
            .filter_map(|name| nested[*name].get(arm).cloned());
        out.insert(*arm, Formula::all(here.chain(below)));
    }
    out
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
    pub(super) fn infer_term(&mut self, term: &mut Term) {
        let span = term.span;
        term.ty = match &mut term.kind {
            // The error term absorbs: it unifies with anything, so the one
            // diagnostic lowering already reported stays the only one.
            TermKind::Error => Rc::new(Ty::default()),
            TermKind::Natural(_) => Rc::new(Ty::plain(Core::Nat)),
            TermKind::Ident(symbol) => {
                let symbol = *symbol;
                self.lookup(span, symbol)
            }
            // A name bound for the length of a body, and everything about that
            // said in the constraint language rather than done here: the walk
            // still reads nothing out of the table, and what has to happen in
            // what order is [`ConstraintKind::Let`]'s to say.
            //
            // The level is raised over the value and dropped again for the
            // body, so a variable is minted at the level it was written at —
            // which is the whole of what decides, later, whether the value is
            // entitled to quantify it.
            TermKind::Let {
                name,
                annotation,
                value,
                body,
            } => {
                self.table.level += 1;
                let level = self.table.level;
                // What the name is bound to inside its own value. An annotation
                // is the contract, so the value is checked against it and the
                // recursive uses the annotation exists for are checked against
                // it too; without one it is a variable the value decides.
                let (bound, promised, rigids) = match annotation {
                    Some(annotation) => {
                        let lowered = lower_annotation(self.mint, self.table, annotation);
                        // The clause is the contract, said in the store: what a
                        // use of this name sees, and what the value under it is
                        // held to.
                        if !lowered.formula.is_true() {
                            let origin = Origin::Annotation(Named {
                                labels: lowered.names.clone(),
                            });
                            self.table
                                .require(annotation.ty.span, origin, lowered.formula.clone());
                        }
                        self.annotated.push(Annotated {
                            span: annotation.ty.span,
                            promised: lowered.formula.clone(),
                            names: lowered.names,
                        });
                        // A recursive use inside the value is a copy of what the
                        // annotation declared, sharing what it left to
                        // inference: the same rule a top-level annotated
                        // definition follows, about a smaller scope.
                        self.env.insert(name.tracked, Binding::Poly(lowered.scheme));
                        (lowered.ty, lowered.formula, lowered.rigids)
                    }
                    None => {
                        // Monomorphically, the same rule a binding group
                        // follows: a use of the name inside its own value is
                        // the one type being decided rather than a copy of a
                        // scheme that does not exist yet.
                        let bound = self.table.fresh_type();
                        self.env.insert(name.tracked, Binding::Mono(bound.clone()));
                        (bound, Formula::True, Vec::new())
                    }
                };
                let outer = std::mem::take(&mut self.out);
                self.check_term(value, &bound);
                let required = std::mem::replace(&mut self.out, outer);

                self.table.level -= 1;
                // And polymorphically in the body, where the scheme exists.
                // Nothing is put back afterwards: a symbol is unique, so the
                // name a nested `let` binds can never be one anything outside
                // its body could have meant — the scope was decided by
                // lowering, and this map only says what each symbol is.
                self.env.insert(name.tracked, Binding::Local);
                let outer = std::mem::take(&mut self.out);
                self.infer_term(body);
                let rest = std::mem::replace(&mut self.out, outer);

                self.out.push(Constraint {
                    span,
                    kind: ConstraintKind::Let {
                        symbol: name.tracked,
                        bound,
                        level,
                        promised,
                        rigids,
                        value: required,
                        body: rest,
                    },
                });
                // What the expression evaluates to is what its body evaluates
                // to; the value is what the name is, not what the `let` is.
                body.ty.clone()
            }
            TermKind::Apply { func, arg } => {
                let opened = self.table.store.batches.len();
                self.infer_term(func);
                let at = func.span;
                self.infer_term(arg);
                // A constrained scheme opened by the function is a demand on
                // the argument: what the function requires among its fields is
                // required of the value written here, so that is where a
                // violation belongs. Only the batches still carrying the
                // function's own span move — the ones an inner application
                // already aimed at its own argument are already where they
                // belong.
                self.table.aim(opened, at, arg.span);
                let applied = func.ty.clone();
                // Through a name, so that something annotated `Endo` is
                // applied as the arrow it stands for. The arrow the arm then
                // works with is the unfolded one, which is the only shape a
                // call site can take apart.
                // What calling it may perform: the arrow's own row where the
                // function already is one, and a fresh variable the equation
                // below ties to it where it is not. Either way the application
                // opens it into the ambient — R12 — which is what makes an
                // effect row an upper bound rather than a demand.
                let (result, performed) = match &self.table.unfolded(self.aliases, &applied).core {
                    // The function already knows what it takes, so the demand
                    // on the argument is the parameter type and the result is
                    // the arrow's own. Written this way round, a mismatch
                    // reads "expected <parameter>, found <argument>": the
                    // parameter is what the context asked for, and the
                    // argument is the term the reader can change.
                    Core::Arrow(from, to, does) => {
                        let (from, to, does) = (from.clone(), to.clone(), does.clone());
                        let actual = arg.ty.clone();
                        self.checks(arg.span, &actual, &from);
                        (to, does)
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
                        let param = self.table.fresh_type();
                        let result = self.table.fresh_type();
                        let does = Row::of(self.table.fresh_row());
                        let wanted = Rc::new(Ty::plain(Core::Arrow(
                            param.clone(),
                            result.clone(),
                            does.clone(),
                        )));
                        self.checks(span, &applied, &wanted);
                        let actual = arg.ty.clone();
                        self.checks(arg.span, &actual, &param);
                        (result, does)
                    }
                };
                self.out.push(Constraint {
                    span,
                    kind: ConstraintKind::Performs {
                        performed,
                        ambient: self.ambient.row.clone(),
                        inside: self.ambient.inside,
                    },
                });
                result
            }
            // A `fn` mints the row its own arrow carries and walks its body at
            // it, which is what makes "what this may do" a property of the
            // function rather than of wherever it was written.
            TermKind::Fn { arg, body } => {
                let param = self.table.fresh_type();
                let does = Row::of(self.table.fresh_row());
                self.env.insert(arg.tracked, Binding::Mono(param.clone()));
                let outer = self.enter(Ambient {
                    row: does.clone(),
                    inside: true,
                });
                // And a closure answers no arm, whichever one it was written
                // in: it can outlive the `handle` and be called with nothing on
                // the stack, which is why lowering refuses a `raise` here at
                // all. The two rules mirror each other rather than one leaning
                // on the other having run.
                let held = self.answer.take();
                self.infer_term(body);
                self.answer = held;
                self.leave(outer);
                Rc::new(Ty::plain(Core::Arrow(param, body.ty.clone(), does)))
            }
            // An operation is an ordinary value of its declared signature, with
            // the effect's own label as the row of its outermost arrow, closed.
            // So performing it is applying it, and the application's opening
            // rule is what puts the label in the ambient.
            TermKind::Operation { effect, op } => {
                // Indexed rather than looked up: lowering refuses every
                // operation reference it cannot resolve, and a signature that
                // failed to lower is still a signature — the two sides absorb
                // as the undecided type rather than going missing.
                let (from, to) = &self.operations[&(effect.tracked, op.tracked.clone())];
                let does = Row {
                    labels: [(
                        self.effect_ids[&effect.tracked].row_key(),
                        RowField::present(Rc::new(Ty::unit())),
                    )]
                    .into_iter()
                    .collect(),
                    rest: Rest::Closed,
                };
                Rc::new(Ty::plain(Core::Arrow(from.clone(), to.clone(), does)))
            }
            TermKind::Handle { body, handler } => self.handle(body, handler),
            // `raise` does not return, so its own type is a fresh variable
            // nothing constrains — the typing `raise` has in ML — and what it
            // carries is the handler's answer. Which handler is lowering's
            // answer, and it has already refused every `raise` that has none.
            TermKind::Raise(value) => {
                self.infer_term(value);
                if let Some(answer) = self.answer.clone() {
                    let actual = value.ty.clone();
                    self.checks(value.span, &actual, &answer);
                }
                self.table.fresh_type()
            }
            TermKind::Struct(fields) => {
                let mut tys = IndexMap::new();
                for (name, field) in fields.iter_mut() {
                    self.infer_term(&mut field.value);
                    tys.insert(name.clone(), RowField::present(field.value.ty.clone()));
                }
                // A literal's fields are all there, and are all it has: the
                // tail is closed. Openness belongs to demands, not to values.
                // Nothing of its own beside them, which is what makes a struct
                // unit carrying fields rather than a shape of its own.
                Rc::new(Ty {
                    core: Core::Unit,
                    fields: tys,
                })
            }
            // A tag is one case of a sum, and which sum is not for the literal
            // to say — so the type it gets names that case and leaves the tail
            // open. That is the whole of what makes a sum row-polymorphic, and
            // it is the exact opposite of the struct above: a literal record
            // has every field it will ever have, and a literal tag is one case
            // of however many the context turns out to allow.
            //
            // A case written with no payload carries unit, which is the same
            // type `()` is. Said here rather than in the tree; see
            // [`ir::TermKind::Tag`](crate::ir::TermKind::Tag).
            TermKind::Tag { name, payload } => {
                let carried = match payload {
                    Some(payload) => {
                        self.infer_term(payload);
                        payload.ty.clone()
                    }
                    None => Rc::new(Ty::unit()),
                };
                let rest = self.table.fresh_row();
                let ty = Rc::new(Ty::plain(Core::Sum(Row {
                    labels: [(name.tracked.clone(), RowField::present(carried))]
                        .into_iter()
                        .collect(),
                    rest,
                })));
                // "However many the context allows" is every case but this
                // one, so the tail minted for it lacks this name.
                self.table.note_lacks(&ty);
                ty
            }
            // The walk cannot name the type it produced — which type `.field`
            // has depends on a base the walk is in no position to know — but
            // it can say everything a projection demands of the base: a type
            // that has the field, whatever else it may also have. Two variables
            // say that — the core the field sits on, and the field's own type —
            // so `fn p => p.x` is not a base waiting to be explained; it is a
            // definition polymorphic in everything but the field it reads.
            TermKind::Project { base, field } => {
                self.infer_term(base);
                let result = self.table.fresh_type();
                let core = Core::Var(self.table.fresh_core());
                let want = Rc::new(Ty {
                    core,
                    fields: [(field.tracked.clone(), RowField::present(result.clone()))]
                        .into_iter()
                        .collect(),
                });
                // "Whatever else it may also have" is the core beside the
                // field, and it lacks that name: it stands for a type the field
                // is already on, and a second copy of it could disagree. One
                // variable says this where two used to.
                self.table.note_lacks(&want);
                let actual = base.ty.clone();
                // The field name is the only thing the user can fix about a
                // type that does not have it, whatever kind of type that is.
                self.checks(field.span, &actual, &want);
                result
            }
            // The scrutinee is what the written matrix, read column-wise,
            // says it is: at every position, the union over all arms of what
            // is tested there — never any one arm's view. The demand that
            // builds is checked against the scrutinee, each binder is bound
            // monomorphically to its position's type — refined so a case the
            // earlier arms fully handle is absent in its view — and every
            // arm's body unifies with the match's own type. Zero arms close
            // the row over nothing: the scrutinee is the empty sum, and the
            // match's own type stays the fresh variable minted below — the
            // empty sum's eliminator. See [`Constrain::position`] for the
            // column rule.
            TermKind::Match { scrutinee, arms } => {
                self.infer_term(scrutinee);
                let result = self.table.fresh_type();
                let expected = match arms.is_empty() {
                    true => Rc::new(Ty::plain(Core::Sum(Row {
                        labels: IndexMap::new(),
                        rest: Rest::Closed,
                    }))),
                    false => {
                        let columns = Columns {
                            matrix: ir::Matrix::new(arms.iter().map(|(pattern, _)| pattern)),
                            patterns: arms.iter().map(|(pattern, _)| pattern).collect(),
                            at: scrutinee.span,
                        };
                        let root: Vec<(usize, Col)> = columns
                            .patterns
                            .iter()
                            .enumerate()
                            .map(|(arm, pattern)| (arm, Col::Pattern(pattern)))
                            .collect();
                        let (demand, cover) = self.position(&columns, &mut Vec::new(), &root);
                        // The column-to-constraint conversion, R6: what the
                        // arms between them cover, as a formula over the
                        // presences the demand just minted. Emitted for the
                        // whole match rather than per position — a nested
                        // column's literals are already inside the enclosing
                        // arm's disjunct.
                        if let Some(cover) = cover {
                            let arms: Vec<Formula> = (0..arms.len())
                                .map(|arm| cover.get(&arm).cloned().unwrap_or(Formula::True))
                                .collect();
                            let fields = demand
                                .fields
                                .iter()
                                .map(|(name, field)| (name.clone(), field.presence.clone()))
                                .collect();
                            let formula = Formula::any(arms.clone());
                            self.table.require(
                                span,
                                Origin::Coverage(Coverage { arms, fields }),
                                formula,
                            );
                        }
                        demand
                    }
                };
                let actual = scrutinee.ty.clone();
                self.checks(scrutinee.span, &actual, &expected);
                // Whichever arm a value picks is what the match comes to, so
                // every body is the one type the match has.
                for (_, body) in arms.iter_mut() {
                    self.infer_term(body);
                    let actual = body.ty.clone();
                    self.checks(body.span, &actual, &result);
                }
                result
            }
        };
    }

    /// Walk `body` at a larger ambient, and every arm at the one the handler
    /// itself sits at.
    ///
    /// R16, and the whole of it. A handler introduces no row of its own: like
    /// every other term form it is checked *at* an ambient, and what it does is
    /// hand its body `D | ..A` — the ambient extended with the effects it
    /// discharges, each present. That the ambient must *lack* those is the
    /// existing lacks condition, said here, and it is what refuses an arm
    /// re-performing the effect its own handler discharges.
    ///
    /// The arms run where the `handle` was written rather than inside the
    /// computation, so they are walked at `A` itself — which is how a handler's
    /// own effects reach the enclosing function's row.
    ///
    /// An arm's value *is* the operation's result, so both halves of its type
    /// come straight off the declaration and `Ans` appears in neither. `Ans` is
    /// what the `return` arm gives, or the handled expression's own type where
    /// none was written, and it is what the whole expression comes to.
    fn handle(&mut self, body: &mut Term, handler: &mut ir::Handler) -> Rc<Ty> {
        let answer = self.table.fresh_type();
        let discharged: IndexMap<String, RowField> = handler
            .discharges
            .iter()
            .map(|effect| {
                (
                    self.effect_ids[&effect.tracked].row_key(),
                    RowField::present(Rc::new(Ty::unit())),
                )
            })
            .collect();
        let extended = Row {
            labels: discharged,
            rest: Rest::More(Rc::new(self.ambient.row.clone())),
        };
        // What the ambient may not stand for: an effect this handler already
        // discharges. A row that acquired one would name it twice, which is
        // exactly the masking R16 leaves refused.
        self.table.note_lacks_row(&extended, Shape::Effect);
        let outer = self.enter(Ambient {
            row: extended,
            inside: self.ambient.inside,
        });
        self.infer_term(body);
        self.leave(outer);

        // An arm answers the handler around it, whichever arm a `raise` inside
        // it is written in.
        let held = self.answer.replace(answer.clone());
        for arm in &mut handler.arms {
            // Indexed rather than looked up, for the reason an operation
            // reference is: lowering keeps no arm whose operation it could not
            // resolve.
            let (from, to) = self.operations[&(arm.effect.tracked, arm.op.tracked.clone())].clone();
            self.env.insert(arm.binder.tracked, Binding::Mono(from));
            self.infer_term(&mut arm.body);
            let actual = arm.body.ty.clone();
            self.checks(arm.body.span, &actual, &to);
        }
        match &mut handler.ret {
            // `| return p => e` binds `p` at the type of the handled
            // expression and gives the answer, which is the one thing that can
            // make the whole expression a different type from its body.
            Some(ret) => {
                self.env
                    .insert(ret.binder.tracked, Binding::Mono(body.ty.clone()));
                self.infer_term(&mut ret.body);
                let actual = ret.body.ty.clone();
                self.checks(ret.body.span, &actual, &answer);
            }
            // With none, the answer is what the body came to — said as one
            // equation rather than as a rule of its own.
            None => {
                let actual = body.ty.clone();
                self.checks(body.span, &actual, &answer);
            }
        }
        self.answer = held;
        answer
    }

    /// Walk what follows at `ambient`, handing back what was in force so the
    /// caller can put it back. See [`Constrain::leave`].
    fn enter(&mut self, ambient: Ambient) -> Ambient {
        std::mem::replace(&mut self.ambient, ambient)
    }

    /// Put back the ambient [`enter`](Self::enter) took.
    fn leave(&mut self, ambient: Ambient) {
        self.ambient = ambient;
    }

    /// The type of one position of a match, from the column of sub-patterns
    /// the arms wrote there — one recursive step per position.
    ///
    /// The demand is the union of what the whole column tests, never any one
    /// arm's view: the tags tested here are the listed cases of one sum row,
    /// each payload typed by the same rule one level down across every arm
    /// that tests it; a natural test demands `Nat`; and the struct and unit
    /// patterns together make one struct demand, by the column-union rule.
    /// Each field mentioned anywhere in the column is present iff *every*
    /// entry of the column is a struct pattern mentioning it — otherwise its
    /// presence is a fresh variable, which is what lets unification infer an
    /// optional field — and each field's type comes from its sub-position
    /// across the arms that mention it. The demand is closed — its core the
    /// fieldless [`Core::Unit`], so no further fields can attach — iff every
    /// entry is an exact struct or unit pattern; any `..`, binder or wildcard
    /// entry leaves it open, a fresh core with the projection's lacks note.
    /// `()` and `{}` are one pattern — an exact struct naming no fields — so a
    /// column of them alone demands unit, exactly as it always has.
    ///
    /// A sum row is closed over its listed cases iff no arm is irrefutable at
    /// the position — a binder at or above it — and otherwise its rest is a
    /// fresh row variable that lacks the listed names, as a tag literal's
    /// tail does.
    ///
    /// A position tested two ways at once — cases and fields, say — is one
    /// value asked to be two things; the demands are equated against each
    /// other at the scrutinee, and the mismatch falls out of the solve. The
    /// mixes worth their own words were already refused at lowering.
    ///
    /// Binders are bound here, monomorphically, to the position's type — for a
    /// position with cases, refined so that every listed case *fully handled*
    /// by the arms above the binder's own is absent in the binder's view. A
    /// case is fully handled iff those arms alone leave it no unhandled
    /// values, which is the same analysis the lowering checks run
    /// ([`ir::Matrix::handled`]). That is what gives the classic catch-all
    /// after `#Some x` its sum-without-`Some`, and it degrades correctly:
    /// after `#A #X`, a later catch-all still sees `#A` present,
    /// because `#A` values with other payloads reach it.
    fn position(
        &mut self,
        columns: &Columns,
        path: &mut Vec<ir::Step>,
        entries: &[(usize, Col)],
    ) -> (Rc<Ty>, Cover) {
        let mut binds: Vec<(usize, Tracked<Symbol>)> = Vec::new();
        let mut naturals = false;
        let mut tags: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        let mut fields: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        // Whether the column qualifies for coverage-to-constraint conversion:
        // every entry a struct, unit, binder or wildcard, and the struct
        // entries not a mix of exact and `..`-open. A tag or a natural test is
        // a gap that is not propositional over finitely many presences, so a
        // column with one keeps today's behaviour end to end.
        let mut qualifies = true;
        let mut exacts = false;
        // Whether any struct or unit pattern tests the position at all, and —
        // for the closure rule — whether every entry is an exact one. `()`
        // and `{}` are one pattern, the exact struct naming no fields, so a
        // unit entry counts as exact; anything that is not a struct or unit
        // pattern leaves the demand open.
        let mut structs = false;
        let mut exact = true;
        for (arm, entry) in entries {
            match entry {
                Col::Unit => {
                    structs = true;
                    exacts = true;
                }
                Col::Pattern(pattern) => match &pattern.tracked {
                    ir::PatternKind::Bind(name) => {
                        binds.push((*arm, *name));
                        exact = false;
                    }
                    // A binder minus the binding: it demands nothing of the
                    // position and leaves its row open — the matrix already
                    // counts it among the binds — and there is no name here
                    // for any environment to learn.
                    ir::PatternKind::Wildcard => exact = false,
                    ir::PatternKind::Unit => {
                        structs = true;
                        exacts = true;
                    }
                    ir::PatternKind::Natural(_) => {
                        naturals = true;
                        exact = false;
                        qualifies = false;
                    }
                    ir::PatternKind::Tag { name, payload } => {
                        let payload = payload.as_deref().map(Col::Pattern).unwrap_or(Col::Unit);
                        tags.entry(name.tracked.as_str())
                            .or_default()
                            .push((*arm, payload));
                        exact = false;
                        qualifies = false;
                    }
                    ir::PatternKind::Struct {
                        fields: named,
                        rest,
                    } => {
                        structs = true;
                        match rest.is_some() {
                            true => exact = false,
                            false => exacts = true,
                        }
                        for (name, field) in named {
                            fields
                                .entry(name.as_str())
                                .or_default()
                                .push((*arm, Col::Pattern(&field.value)));
                        }
                    }
                },
            }
        }
        // An exact entry says the value has exactly these fields, which is a
        // claim about the *rest* — the fields nobody named — as much as about
        // the ones it names. That is only propositional over finitely many
        // presences where the whole column is exact and the demand closes over
        // its own field set; beside anything that leaves the row open — another
        // arm's `..`, a binder, a wildcard — the claim reaches fields no
        // formula has a variable for. So a column with an exact entry qualifies
        // only when every entry is one, and a column with none qualifies
        // whatever else is in it.
        if exacts && !exact {
            qualifies = false;
        }
        // Whether an arm is irrefutable here — a binder at or above, or a
        // catch-all arm — which is what decides whether the row closes. Read
        // off the matrix rather than tracked down the recursion, so this and
        // the lowering checks answer from one place.
        let open = columns.matrix.open(path);
        let mut demands: Vec<Rc<Ty>> = Vec::new();
        // The demand's own fields, and what each sub-column covers per arm:
        // the two halves the covered set is built from, kept out here because
        // a column with no struct entry has neither and still has to answer.
        let mut named: IndexMap<String, RowField> = IndexMap::new();
        let mut nested: IndexMap<String, IndexMap<usize, Formula>> = IndexMap::new();
        // The listed cases, kept beside their row's rest for the binder views
        // below: a view is the same labels re-read, not a second demand.
        let mut listed: Option<(IndexMap<String, RowField>, Rest)> = None;
        if !tags.is_empty() {
            let mut labels = IndexMap::new();
            for (name, payloads) in &tags {
                path.push(ir::Step::Payload(name.to_string()));
                // A tag already disqualified this column, so the sub-column's
                // own coverage has nobody to contribute to.
                let (payload, _) = self.position(columns, path, payloads);
                path.pop();
                labels.insert(name.to_string(), RowField::present(payload));
            }
            let rest = match open {
                true => self.table.fresh_row(),
                false => Rest::Closed,
            };
            let ty = Rc::new(Ty::plain(Core::Sum(Row {
                labels: labels.clone(),
                rest: rest.clone(),
            })));
            // The rest stands for the cases not listed, so it lacks the
            // listed names — what a tag literal's tail says, said of a
            // column's.
            self.table.note_lacks(&ty);
            listed = Some((labels, rest));
            demands.push(ty);
        }
        if naturals {
            demands.push(Rc::new(Ty::plain(Core::Nat)));
        }
        if structs {
            // The column-union rule for fields. A field is certainly there
            // only when every entry of the column asks for it; a field some
            // entries do without gets a fresh presence variable, so whether it
            // is there is the scrutinee's to decide — the inference behind an
            // optional field. The demand closes over the named fields exactly
            // when every entry is exact: its core is then the fieldless unit,
            // which no further field can attach to. An open demand keeps a
            // fresh core with the projection's lacks note, asking only for
            // the named fields' presences.
            let total = entries.len();
            for (name, subs) in &fields {
                path.push(ir::Step::Field(name.to_string()));
                let (field, sub) = self.position(columns, path, subs);
                path.pop();
                // Every tested sub-position has to qualify too: a column whose
                // fields hide a natural test hides a gap that is not
                // propositional, however plain the fields above it look.
                match sub {
                    Some(sub) => {
                        nested.insert(name.to_string(), sub);
                    }
                    None => qualifies = false,
                }
                let presence = match subs.len() == total {
                    true => Presence::Present,
                    false => self.table.fresh_presence(),
                };
                named.insert(
                    name.to_string(),
                    RowField {
                        presence,
                        ty: field,
                    },
                );
            }
            let core = match exact {
                true => Core::Unit,
                false => Core::Var(self.table.fresh_core()),
            };
            let ty = Rc::new(Ty {
                core,
                fields: named.clone(),
            });
            self.table.note_lacks(&ty);
            demands.push(ty);
        }
        let mut demands = demands.into_iter();
        // A column that only binds demands nothing: the position is a fresh
        // type the scrutinee decides — the `c` of the column-union example.
        let ty = demands.next().unwrap_or_else(|| self.table.fresh_type());
        for also in demands {
            self.checks(columns.at, &also, &ty);
        }
        let cover = match qualifies {
            false => None,
            true => Some(covered(entries, &named, &nested, exact)),
        };
        for (arm, binder) in binds {
            let view = match &listed {
                // The refinement: every case the arms above this one fully
                // handle is absent in the binder's view — the value reaching
                // it cannot be one — over the same payloads and the same
                // rest. An absent case's type is deliberately unconstrained;
                // a case the value cannot be carries nothing.
                Some((labels, rest)) => {
                    let earlier = &columns.patterns[..arm];
                    let refined = labels
                        .iter()
                        .map(|(name, field)| {
                            let field = match columns.matrix.handled(earlier, path, name) {
                                true => RowField {
                                    presence: Presence::Absent,
                                    ty: Rc::new(Ty::default()),
                                },
                                false => field.clone(),
                            };
                            (name.clone(), field)
                        })
                        .collect();
                    Rc::new(Ty::plain(Core::Sum(Row {
                        labels: refined,
                        rest: rest.clone(),
                    })))
                }
                None => ty.clone(),
            };
            self.env.insert(binder.tracked, Binding::Mono(view));
        }
        (ty, cover)
    }

    /// Check `term` against a type the context already knows. Checking pushes
    /// expected types *into* binders — an annotated `fn p => p.x` learns `p`'s
    /// type from the annotation before the body needs it, which inference
    /// alone could not order. Everywhere the shapes do not line up, checking
    /// falls back to inferring and equating.
    ///
    /// `expected` is always a written type: it starts at an annotation and
    /// only ever loses an arrow or a field on the way down. The variables a
    /// written type can mention — a tail, a `when` field — arrive fresh from
    /// [`lower_type`] and unbound, so checking matches on what was literally
    /// written and, like the rest of generation, never has to ask the table
    /// anything.
    pub(super) fn check_term(&mut self, term: &mut Term, expected: &Rc<Ty>) {
        // Checking looks through a name — an annotation of `list` still pushes
        // into a struct literal — but `term.ty` is set from `expected` rather
        // than from this, so the term keeps the name the user wrote and prints
        // as it.
        let shape = self.table.unfolded(self.aliases, expected);
        match (&mut term.kind, &shape.core) {
            // The lambda's arrow *is* the annotation, so its effect row is the
            // annotation's: the body is walked at what the reader wrote it may
            // do, and a `fn` that mints its own row here would be checking
            // against a promise nobody made.
            (TermKind::Fn { arg, body }, Core::Arrow(from, to, does)) => {
                let (from, to, does) = (from.clone(), to.clone(), does.clone());
                self.env.insert(arg.tracked, Binding::Mono(from));
                let outer = self.enter(Ambient {
                    row: does,
                    inside: true,
                });
                let held = self.answer.take();
                self.check_term(body, &to);
                self.answer = held;
                self.leave(outer);
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
            (TermKind::Struct(fields), Core::Unit)
                if shape
                    .fields
                    .values()
                    .all(|field| matches!(field.presence, Presence::Present))
                    && same_field_set(fields, &shape.fields) =>
            {
                for (name, field) in fields.iter_mut() {
                    let want = shape.fields[name].ty.clone();
                    self.check_term(&mut field.value, &want);
                }
                term.ty = expected.clone();
            }
            // A tag is pushed into whenever the expected sum names its case as
            // one it certainly allows: what the case carries is what the
            // payload is checked against, and the cases the literal does not
            // name are exactly what its own open tail would have absorbed.
            //
            // Which is a weaker gate than the struct's above, and rightly so.
            // Pushing a closed struct into a literal is only safe when the two
            // name the same fields, because a literal has every field it will
            // ever have; a tag has one case out of however many, so a sum with
            // more cases than the literal names is the ordinary case rather
            // than the one to fall back on.
            (TermKind::Tag { name, payload }, Core::Sum(cases))
                if cases
                    .labels
                    .get(&name.tracked)
                    .is_some_and(|case| matches!(case.presence, Presence::Present)) =>
            {
                let want = cases.labels[&name.tracked].ty.clone();
                match payload {
                    Some(payload) => self.check_term(payload, &want),
                    // Nothing written is unit, and the case has to carry one.
                    // Said as a constraint rather than pushed, since there is
                    // no term here to push into — and worded with the tag's own
                    // span, which is the whole of what the reader wrote.
                    None => {
                        let carried = Rc::new(Ty::unit());
                        self.checks(name.span, &carried, &want);
                    }
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
    /// other, which is exactly the let/lambda distinction. A name a nested
    /// `let` bound is the polymorphic case with the scheme still to come, so
    /// the copy is asked for rather than made.
    fn lookup(&mut self, span: Span, symbol: Symbol) -> Rc<Ty> {
        // Indexed rather than looked up. A lambda's argument is bound where the
        // walk enters its body; a nested `let`'s name is bound before its own
        // value is walked; a top-level definition is bound before any body
        // that could name it is walked — its own group's, monomorphically, and
        // every earlier group's as a scheme — and a name that resolved to
        // nothing already became `TermKind::Error`. A lookup falling back to a
        // fresh variable would hide the day one of those stops holding.
        match self.env[&symbol].clone() {
            Binding::Mono(ty) => ty,
            Binding::Poly(scheme) => self.table.instantiate(span, &scheme),
            // The scheme is not written yet, so the walk says what this use is
            // rather than what it has: a fresh copy of whatever the enclosing
            // [`ConstraintKind::Let`] publishes. Which keeps the invariant this
            // pass is built on — nothing here reads the table — over the one
            // construct that would otherwise have to wait for a solve.
            Binding::Local => {
                let ty = self.table.fresh_type();
                self.out.push(Constraint {
                    span,
                    kind: ConstraintKind::Instance {
                        symbol,
                        ty: ty.clone(),
                    },
                });
                ty
            }
        }
    }
}
