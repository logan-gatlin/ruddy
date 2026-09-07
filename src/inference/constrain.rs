//! Pass one: generation. See [`Constrain`].

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    ir::{self, Term, TermKind},
    symbol::{Mint, Symbol},
    tracking::{Anchor, Anchored},
    types::{Assigned, Formula, ParamKind, Presence, Rest, Row, RowField, Scheme, Shape, Ty},
};

use super::{
    Annotated, Binding, Constraint, ConstraintKind, ConstraintOrigin, ConstraintSubjects, Coverage,
    DeferredRequirement, Env, ExplainedScheme, GuardedArm, Named, Origin, SemanticPivot, Subject,
    Table, effective_conditions, lower_annotation, same_field_set,
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
    /// lambda argument can never collide with a top-level definition. What
    /// this walk binds goes in the group's own layer; what earlier groups
    /// published is read through the shared one. See [`Env`].
    pub env: &'a mut Env,
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
    /// What each effect's parameters stand for, in order: one fresh argument
    /// per entry is minted wherever an operation is referred to or an effect
    /// is handled, and the operation signatures are opened over them.
    pub effect_params: &'a IndexMap<Symbol, Vec<ParamKind>>,
    /// Declaration spans keyed by the source-resolved identity. Kept apart
    /// from structural row keys because equivalent same-named interfaces are
    /// allowed to coalesce semantically.
    pub effect_declaration_spans: &'a IndexMap<Symbol, Anchor>,
    /// Compilation-local operation-value provenance. It mirrors only value
    /// flow (call results and struct fields), never semantic type identity.
    pub term_effect_provenance: HashMap<Anchor, super::EffectProvenance>,
    /// Provenance published by nested values before solving can publish their
    /// semantic schemes.
    pub local_effect_provenance: HashMap<Symbol, super::EffectProvenance>,
    /// Parameter placeholders used to preserve higher-order value flow.
    pub binding_effect_provenance: HashMap<Symbol, super::EffectProvenance>,
    /// Exact sources performed by each callable body currently being walked.
    /// A nested closure gets its own entry, so merely constructing or returning
    /// that closure cannot make the enclosing callable appear to invoke it.
    pub callable_effect_scopes: Vec<Vec<super::EffectSource>>,
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
    /// Syntactic conjunction of qualifying arm conditions enclosing the walk.
    /// It is recorded on nested annotations for their post-solve contract
    /// check; generation never asks whether it is satisfiable.
    pub presence_guard: Formula,
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
pub type Operations = IndexMap<(Symbol, ir::OperationSelector), (Rc<Ty>, Rc<Ty>)>;

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
    /// The written function, or top-level name, whose effect boundary owns this
    /// ambient. Handlers extend the row but do not replace its owner.
    pub boundary_at: Anchor,
    /// Written handler labels currently extending this ambient row.
    pub label_spans: IndexMap<String, Anchor>,
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
    at: Anchor,
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
fn collect_presence_paths(
    ty: &Rc<Ty>,
    prefix: &mut super::PresencePath,
    found: &mut Vec<(super::PresencePath, Presence)>,
) {
    let Ty::Struct(row) = &**ty else { return };
    for (name, field) in &row.labels {
        prefix.push(name.clone());
        found.push((prefix.clone(), field.presence.clone()));
        if !matches!(field.presence, Presence::Absent) {
            collect_presence_paths(&field.ty, prefix, found);
        }
        prefix.pop();
    }
}

pub(super) fn structural_presence_paths(ty: &Rc<Ty>) -> Vec<(super::PresencePath, Presence)> {
    let mut found = Vec::new();
    collect_presence_paths(ty, &mut Vec::new(), &mut found);
    found
}

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
            Col::Pattern(pattern) => match &pattern.anchored {
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
    fn constraint(
        &mut self,
        span: Anchor,
        origin: ConstraintOrigin,
        subjects: ConstraintSubjects,
        kind: ConstraintKind,
    ) -> Constraint {
        let id = self.table.constraint_id();
        let reason = self.table.constraint_reason_for(id, &kind);
        Constraint {
            id,
            reason,
            at: span,
            origin,
            subjects,
            kind,
        }
    }

    fn mark_function_input(&mut self, func: &Term) {
        if let TermKind::Ident(symbol) = &func.kind {
            self.out
                .last_mut()
                .expect("just emitted argument check")
                .subjects
                .semantic_pivot = Some(SemanticPivot::FunctionInput(*symbol));
        }
    }

    fn mark_branch_result(constraint: &mut Constraint, family: Anchor) {
        constraint.subjects.semantic_pivot = Some(SemanticPivot::BranchResult(family));
    }

    fn emit(
        &mut self,
        span: Anchor,
        origin: ConstraintOrigin,
        subjects: ConstraintSubjects,
        kind: ConstraintKind,
    ) {
        let constraint = self.constraint(span, origin, subjects, kind);
        self.out.push(constraint);
    }

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
    #[allow(clippy::too_many_arguments)]
    fn checks(
        &mut self,
        span: Anchor,
        actual: &Rc<Ty>,
        expected: &Rc<Ty>,
        origin: ConstraintOrigin,
        expected_subject: Subject,
        expected_span: Option<Anchor>,
        actual_subject: Subject,
    ) {
        self.emit(
            span,
            origin,
            ConstraintSubjects::pair_at(
                expected_subject,
                expected_span,
                actual_subject,
                Some(span),
            ),
            ConstraintKind::Equal {
                expected: expected.clone(),
                actual: actual.clone(),
            },
        );
    }

    /// Hold every as-yet unowned batch generated since `from` inert in its
    /// source-order slot. A nested qualifying arm has already claimed its own
    /// slots, so an enclosing arm naturally takes only the nested coverage and
    /// direct requirements around them.
    fn defer_requirements(&mut self, from: usize) -> Vec<DeferredRequirement> {
        let mut requirements = Vec::new();
        for at in from..self.table.store.batches.len() {
            if !self.table.deferred.insert(at) {
                continue;
            }
            let batch = self.table.store.batches[at].clone();
            self.table.store.batches[at].formula = Formula::True;
            requirements.push(DeferredRequirement { at, batch });
        }
        requirements
    }

    /// Infer a type for `term` and write it into `term.ty`.
    pub(super) fn infer_term(&mut self, term: &mut Term) {
        let span = term.at;
        term.ty = match &mut term.kind {
            // The error term absorbs: it unifies with anything, so the one
            // diagnostic lowering already reported stays the only one.
            TermKind::Error => Rc::new(Ty::default()),
            TermKind::Natural(_) => Rc::new(Ty::plain(Ty::Nat)),
            TermKind::Fixed(value) => Rc::new(Ty::plain(Ty::Fixed(value.kind()))),
            TermKind::Integer(_) => Rc::new(Ty::plain(Ty::Int)),
            TermKind::Real(_) => Rc::new(Ty::plain(Ty::Real)),
            TermKind::String(_) => Rc::new(Ty::plain(Ty::String)),
            TermKind::Boolean(_) => Rc::new(Ty::plain(Ty::Boolean)),
            TermKind::Unary {
                op: crate::ir::UnaryOp::Allocate,
                value,
            } => {
                self.infer_term(value);
                let region = self.table.fresh_region();
                self.mutation(span, region.clone());
                Rc::new(Ty::Mut(region, value.ty.clone()))
            }
            TermKind::Unary {
                op: crate::ir::UnaryOp::Read,
                value,
            } => {
                let region = self.table.fresh_region();
                let element = self.table.fresh_type_for(Subject::Term);
                self.check_term(
                    value,
                    &Rc::new(Ty::Mut(region.clone(), element.clone())),
                    Subject::Context,
                    None,
                );
                self.mutation(span, region);
                element
            }
            TermKind::Binary {
                op: crate::ir::BinaryOp::Write,
                left,
                right,
            } => {
                let region = self.table.fresh_region();
                let element = self.table.fresh_type_for(Subject::Term);
                self.check_term(
                    left,
                    &Rc::new(Ty::Mut(region.clone(), element.clone())),
                    Subject::Context,
                    None,
                );
                self.check_term(right, &element, Subject::Context, None);
                self.mutation(span, region);
                element
            }
            TermKind::Unary { op, value } => match op {
                crate::ir::UnaryOp::Allocate | crate::ir::UnaryOp::Read => unreachable!(),
                crate::ir::UnaryOp::Neg => {
                    let real = Rc::new(Ty::plain(Ty::Real));
                    self.check_term(value, &real, Subject::Context, None);
                    real
                }
                crate::ir::UnaryOp::Not => {
                    let boolean = Rc::new(Ty::plain(Ty::Boolean));
                    self.check_term(value, &boolean, Subject::Context, None);
                    boolean
                }
            },
            TermKind::Binary { op, left, right } => {
                let core = match op {
                    crate::ir::BinaryOp::Write => unreachable!(),
                    crate::ir::BinaryOp::Add
                    | crate::ir::BinaryOp::Sub
                    | crate::ir::BinaryOp::Mul
                    | crate::ir::BinaryOp::Div => Ty::Real,
                    crate::ir::BinaryOp::And
                    | crate::ir::BinaryOp::Or
                    | crate::ir::BinaryOp::Xor => Ty::Boolean,
                };
                let ty = Rc::new(Ty::plain(core));
                self.check_term(left, &ty, Subject::Context, None);
                self.check_term(right, &ty, Subject::Context, None);
                ty
            }
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
                let expected_subject = if annotation.is_some() {
                    Subject::Annotation
                } else {
                    Subject::LocalBinding
                };
                let expected_span = annotation.as_ref().map(|annotation| annotation.ty.at);
                let (bound, promised, rigids) = match annotation {
                    Some(annotation) => {
                        let lowered = lower_annotation(self.mint, self.table, annotation);
                        // The clause is the contract, said in the store: what a
                        // use of this name sees, and what the value under it is
                        // held to.
                        if !lowered.formula.is_true() {
                            let origin = Origin::Annotation(Named {
                                labels: lowered.names.clone(),
                                shape: None,
                            });
                            self.table.require(
                                annotation.ty.at,
                                origin,
                                lowered.assumptions.clone(),
                            );
                        }
                        self.annotated.push(Annotated {
                            span: annotation.ty.at,
                            guard: self.presence_guard.clone(),
                            promised: lowered.formula.clone(),
                            names: lowered.names,
                        });
                        // A recursive use inside the value is a copy of what the
                        // annotation declared, sharing what it left to
                        // inference: the same rule a top-level annotated
                        // definition follows, about a smaller scope.
                        self.table.authoritative_bindings.insert(name.anchored);
                        self.table
                            .authoritative_spans
                            .insert(name.anchored, annotation.ty.at);
                        self.env.insert(
                            name.anchored,
                            Binding::Poly(ExplainedScheme::imported(lowered.scheme)),
                        );
                        (lowered.ty, lowered.formula, lowered.rigids)
                    }
                    None => {
                        // Monomorphically, the same rule a binding group
                        // follows: a use of the name inside its own value is
                        // the one type being decided rather than a copy of a
                        // scheme that does not exist yet.
                        let bound = self.table.fresh_type_for(Subject::LocalBinding);
                        self.env.insert(name.anchored, Binding::Mono(bound.clone()));
                        (bound, Formula::True, Vec::new())
                    }
                };
                let outer = std::mem::take(&mut self.out);
                let initializer_effects =
                    Row::of(self.table.fresh_row_for(Subject::AmbientEffects));
                let enclosing = self.enter(Ambient {
                    row: initializer_effects.clone(),
                    ..self.ambient.clone()
                });
                self.check_term(value, &bound, expected_subject, expected_span);
                self.leave(enclosing);
                let required = std::mem::replace(&mut self.out, outer);

                self.table.level -= 1;
                let value_effect_provenance = self
                    .term_effect_provenance
                    .get(&value.at)
                    .cloned()
                    .unwrap_or_default();
                self.local_effect_provenance
                    .insert(name.anchored, value_effect_provenance);
                // And polymorphically in the body, where the scheme exists.
                // Nothing is put back afterwards: a symbol is unique, so the
                // name a nested `let` binds can never be one anything outside
                // its body could have meant — the scope was decided by
                // lowering, and this map only says what each symbol is.
                self.env.insert(name.anchored, Binding::Local);
                let outer = std::mem::take(&mut self.out);
                self.infer_term(body);
                let rest = std::mem::replace(&mut self.out, outer);

                self.table
                    .local_names
                    .insert(name.anchored, Rc::from(self.mint.name(name.anchored)));
                self.emit(
                    name.at,
                    ConstraintOrigin::Binding,
                    ConstraintSubjects::one(Subject::Binding),
                    ConstraintKind::Let {
                        symbol: name.anchored,
                        bound,
                        level,
                        promised,
                        rigids,
                        effect_provenance: self
                            .local_effect_provenance
                            .get(&name.anchored)
                            .cloned()
                            .unwrap_or_default(),
                        initializer_effects,
                        ambient: self.ambient.row.clone(),
                        inside: self.ambient.inside,
                        value: required,
                        body: rest,
                    },
                );
                // What the expression evaluates to is what its body evaluates
                // to; the value is what the name is, not what the `let` is.
                if let Some(provenance) = self.term_effect_provenance.get(&body.at).cloned() {
                    self.term_effect_provenance.insert(span, provenance);
                }
                body.ty.clone()
            }
            TermKind::Apply { func, arg } => {
                let opened = self.table.store.batches.len();
                self.infer_term(func);
                let at = func.at;
                self.infer_term(arg);
                // A constrained scheme opened by the function is a demand on
                // the argument: what the function requires among its fields is
                // required of the value written here, so that is where a
                // violation belongs. Only the batches still carrying the
                // function's own span move — the ones an inner application
                // already aimed at its own argument are already where they
                // belong.
                self.table.aim(opened, at, arg.at);
                let applied = func.ty.clone();
                let func_provenance = self
                    .term_effect_provenance
                    .get(&func.at)
                    .cloned()
                    .unwrap_or_default();
                let arg_provenance = self
                    .term_effect_provenance
                    .get(&arg.at)
                    .cloned()
                    .unwrap_or_default();
                let resolved_func = match func_provenance.argument {
                    Some(symbol) => func_provenance.substitute(symbol, &arg_provenance),
                    None => func_provenance,
                };
                let effect_origins = match &resolved_func.value_parameter {
                    Some(parameter) => vec![super::EffectSource::Parameter {
                        symbol: parameter.symbol,
                        path: parameter.path.clone(),
                    }],
                    None => resolved_func.callable.clone(),
                };
                if let Some(scope) = self.callable_effect_scopes.last_mut() {
                    for source in &effect_origins {
                        if !scope.contains(source) {
                            scope.push(source.clone());
                        }
                    }
                }
                // A symbolic callable's returned value remains symbolic too:
                // subsequent projections and invocations resolve this path
                // against the exact argument. Merely obtaining the result does
                // not add that result's callable origin to `effect_origins`.
                if let Some(result) = resolved_func.call_result() {
                    self.term_effect_provenance.insert(span, result);
                }
                // Through a name, so that something annotated `Endo` is
                // applied as the arrow it stands for. The arrow the arm then
                // works with is the unfolded one, which is the only shape a
                // call site can take apart.
                // What calling it may perform: the arrow's own row where the
                // function already is one, and a fresh variable the equation
                // below ties to it where it is not. Either way the application
                // opens it into the ambient — R12 — which is what makes an
                // effect row an upper bound rather than a demand.
                let (result, performed) = match &*self.table.unfolded(self.aliases, &applied) {
                    // The function already knows what it takes, so the demand
                    // on the argument is the parameter type and the result is
                    // the arrow's own. Written this way round, a mismatch
                    // reads "expected <parameter>, found <argument>": the
                    // parameter is what the context asked for, and the
                    // argument is the term the reader can change.
                    Ty::Arrow(from, to, does) => {
                        let (from, mut to, does) = (from.clone(), to.clone(), does.clone());
                        let actual = arg.ty.clone();
                        self.checks(
                            arg.at,
                            &actual,
                            &from,
                            ConstraintOrigin::ApplicationArgument,
                            Subject::Parameter,
                            Some(func.at),
                            Subject::Argument,
                        );
                        self.mark_function_input(func);
                        // Application is the semantic destruction point of a
                        // packaged result. Open it during generation so its
                        // invocation-fresh guarantee occupies the call's
                        // source-order store position (rather than leaking in
                        // later when equality happens to solve).
                        if matches!(&*to, Ty::Package(_)) {
                            to = self.table.open_package(arg.at, &to);
                        }
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
                        let param = self.table.fresh_type_for(Subject::Parameter);
                        let result = self.table.fresh_type_for(Subject::Context);
                        let does = Row::of(self.table.fresh_row_for(Subject::PerformedEffects));
                        let wanted = Rc::new(Ty::plain(Ty::Arrow(
                            param.clone(),
                            result.clone(),
                            does.clone(),
                        )));
                        self.checks(
                            func.at,
                            &applied,
                            &wanted,
                            ConstraintOrigin::ApplicationCallee,
                            Subject::CallShape,
                            None,
                            Subject::Callee,
                        );
                        let actual = arg.ty.clone();
                        self.checks(
                            arg.at,
                            &actual,
                            &param,
                            ConstraintOrigin::ApplicationArgument,
                            Subject::Parameter,
                            Some(func.at),
                            Subject::Argument,
                        );
                        self.mark_function_input(func);
                        (result, does)
                    }
                };
                self.emit(
                    span,
                    ConstraintOrigin::ApplicationEffects,
                    ConstraintSubjects::pair_at(
                        Subject::PerformedEffects,
                        Some(span),
                        Subject::AmbientEffects,
                        Some(self.ambient.boundary_at),
                    ),
                    ConstraintKind::Performs {
                        performed,
                        ambient: self.ambient.row.clone(),
                        effect_origins,
                        ambient_label_spans: self.ambient.label_spans.clone(),
                        inside: self.ambient.inside,
                    },
                );
                result
            }
            // A `fn` mints the row its own arrow carries and walks its body at
            // it, which is what makes "what this may do" a property of the
            // function rather than of wherever it was written.
            TermKind::Fn { arg, body } => {
                self.table.level += 1;
                let level = self.table.level;
                let param = self.table.fresh_type_for(Subject::Parameter);
                let does = Row::of(self.table.fresh_row_for(Subject::AmbientEffects));
                self.env.insert(arg.anchored, Binding::Mono(param.clone()));
                self.binding_effect_provenance.insert(
                    arg.anchored,
                    super::EffectProvenance::parameter(arg.anchored),
                );
                self.callable_effect_scopes.push(Vec::new());
                let outer = self.enter(Ambient {
                    row: does.clone(),
                    inside: true,
                    boundary_at: span,
                    label_spans: IndexMap::new(),
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
                let callable = self
                    .callable_effect_scopes
                    .pop()
                    .expect("function effect scope");
                let result = self
                    .term_effect_provenance
                    .get(&body.at)
                    .cloned()
                    .unwrap_or_default();
                self.term_effect_provenance.insert(
                    span,
                    super::EffectProvenance::function(callable.clone(), arg.anchored, result),
                );
                let external = Row::of(self.table.fresh_row_for(Subject::AmbientEffects));
                self.emit(
                    span,
                    ConstraintOrigin::Binding,
                    ConstraintSubjects::one(Subject::AmbientEffects),
                    ConstraintKind::Isolate {
                        effect_origins: callable,
                        input: param.clone(),
                        output: body.ty.clone(),
                        internal: does,
                        external: external.clone(),
                        level,
                    },
                );
                self.table.level -= 1;
                Rc::new(Ty::Arrow(param, body.ty.clone(), external))
            }
            // An operation is an ordinary value of its declared signature, with
            // the effect's own label as the row of its outermost arrow, closed.
            // So performing it is applying it, and the application's opening
            // rule is what puts the label in the ambient.
            TermKind::Operation { effect, selector } => {
                // Indexed rather than looked up: lowering refuses every
                // operation reference it cannot resolve, and a signature that
                // failed to lower is still a signature — the two sides absorb
                // as the undecided type rather than going missing.
                //
                // The effect's parameters are instantiated afresh for this
                // reference: the signature is opened over new variables, and
                // the label the reference carries holds them, so what the
                // operation is applied to and what its result is used as
                // decide the application the surrounding row carries.
                let (from, to) =
                    self.operations[&(effect.anchored, selector.anchored.clone())].clone();
                let fresh = self.fresh_effect_arguments(effect.anchored);
                let (from, to) = (from.open(&fresh), to.open(&fresh));
                self.table.note_lacks(&from);
                self.table.note_lacks(&to);
                let does = Row {
                    labels: [(
                        self.effect_ids[&effect.anchored].row_key(),
                        RowField::present(argument_tuple(&fresh)),
                    )]
                    .into_iter()
                    .collect(),
                    rest: Rest::Closed,
                };
                if let Some((interface, declaration_span)) = self
                    .effect_ids
                    .get(&effect.anchored)
                    .zip(self.effect_declaration_spans.get(&effect.anchored))
                {
                    let origin = super::EffectOrigin {
                        symbol: effect.anchored,
                        interface: interface.clone(),
                        declaration_at: *declaration_span,
                    };
                    self.term_effect_provenance
                        .insert(span, super::EffectProvenance::origin(origin));
                }
                Rc::new(Ty::plain(Ty::Arrow(from, to, does)))
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
                    self.checks(
                        value.at,
                        &actual,
                        &answer,
                        ConstraintOrigin::Raise,
                        Subject::HandlerAnswer,
                        None,
                        Subject::RaisedValue,
                    );
                }
                self.table.fresh_type_for(Subject::RaiseResult)
            }
            TermKind::Struct { fields, spread } => {
                let mut tys = IndexMap::new();
                let mut provenance_fields = IndexMap::new();
                for (name, field) in fields.iter_mut() {
                    self.infer_term(&mut field.value);
                    if let Some(value) = self.term_effect_provenance.get(&field.value.at).cloned() {
                        provenance_fields.insert(name.clone(), value);
                    }
                    tys.insert(name.clone(), RowField::present(field.value.ty.clone()));
                }
                let provenance = super::EffectProvenance::from_fields(provenance_fields);
                if provenance != super::EffectProvenance::default() {
                    self.term_effect_provenance.insert(span, provenance);
                }
                match spread {
                    // A literal's fields are all there, and are all it has:
                    // the tail is closed. Openness belongs to demands, not to
                    // values. Nothing of its own beside them, which is what
                    // makes a struct a struct rather than a shape of its own.
                    None => Rc::new(Ty::Struct(Row {
                        labels: tys,
                        rest: Rest::Closed,
                    })),
                    // With a spread the literal has its own fields and then
                    // whatever the spread value has past them, and that is
                    // what the walk says: one rest, shared between what the
                    // value is asked to be and what the literal is. The
                    // named fields are the literal's own — certainly there,
                    // holding the written value — whether or not the value
                    // spread has them, and whatever it holds where it does;
                    // so the demand on it names them too, each with a
                    // presence and a type of its own that nothing else
                    // mentions. Which value is spread is not for the walk to
                    // know, so the demand is emitted as the constraint it is
                    // and left to the solver — see [`Solve::spread`]
                    // (super::solve::Solve::spread).
                    Some(spread) => {
                        self.infer_term(&mut spread.value);
                        let rest = self.table.fresh_row_for(Subject::StructSpread);
                        let shadowed = tys
                            .keys()
                            .map(|name| {
                                let presence = self.table.fresh_presence_for(Subject::StructSpread);
                                let ty = self.table.fresh_type_for(Subject::StructSpread);
                                (name.clone(), RowField { presence, ty })
                            })
                            .collect();
                        let demand = Rc::new(Ty::Struct(Row {
                            labels: shadowed,
                            rest: rest.clone(),
                        }));
                        let result = Rc::new(Ty::Struct(Row { labels: tys, rest }));
                        // One subject, because one side of this ever reaches
                        // a reader: the demand is fresh variables and cannot
                        // conflict with anything, so a complaint is always
                        // about what the value spread brought in.
                        self.emit(
                            spread.at,
                            ConstraintOrigin::StructSpread,
                            ConstraintSubjects::one(Subject::StructSpread),
                            ConstraintKind::Spread {
                                operand: spread.value.ty.clone(),
                                demand,
                                result: result.clone(),
                                operand_span: spread.value.at,
                            },
                        );
                        result
                    }
                }
            }
            // A spread item is an array of the literal's own type, checked as
            // such; a plain one is an element.
            TermKind::Array(items) => {
                let element = self.table.fresh_type_for(Subject::Term);
                let array = Rc::new(Ty::Array(element.clone()));
                for item in items {
                    match item.spread {
                        Some(dots) => {
                            self.check_term(&mut item.value, &array, Subject::Spread, Some(dots))
                        }
                        None => self.check_term(&mut item.value, &element, Subject::Context, None),
                    }
                }
                array
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
                let rest = self.table.fresh_row_for(Subject::Term);
                let ty = Rc::new(Ty::plain(Ty::Sum(Row {
                    labels: [(name.anchored.clone(), RowField::present(carried))]
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
            // say that — the constructor the field sits on, and the field's own type —
            // so `fn p => p.x` is not a base waiting to be explained; it is a
            // definition polymorphic in everything but the field it reads.
            TermKind::Project { base, field } => {
                self.infer_term(base);
                if let Some(provenance) = self
                    .term_effect_provenance
                    .get(&base.at)
                    .and_then(|provenance| provenance.projected(field.anchored.clone()))
                {
                    self.term_effect_provenance.insert(span, provenance);
                }
                let result = self.table.fresh_type_for(Subject::ProjectionResult);
                self.emit(
                    field.at,
                    ConstraintOrigin::Projection,
                    ConstraintSubjects::pair(Subject::ProjectionBase, Subject::ProjectionResult),
                    ConstraintKind::Project {
                        base: base.ty.clone(),
                        field: field.anchored.clone(),
                        result: result.clone(),
                        base_span: base.at,
                    },
                );
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
                let result = self.table.fresh_type_for(Subject::MatchResult);
                let mut qualifying = None;
                let expected = match arms.is_empty() {
                    true => Rc::new(Ty::plain(Ty::Sum(Row {
                        labels: IndexMap::new(),
                        rest: Rest::Closed,
                    }))),
                    false => {
                        let columns = Columns {
                            matrix: ir::Matrix::new(arms.iter().map(|(pattern, _)| pattern)),
                            patterns: arms.iter().map(|(pattern, _)| pattern).collect(),
                            at: scrutinee.at,
                        };
                        let root: Vec<(usize, Col)> = columns
                            .patterns
                            .iter()
                            .enumerate()
                            .map(|(arm, pattern)| (arm, Col::Pattern(pattern)))
                            .collect();
                        let (demand, cover) = self.position(&columns, &mut Vec::new(), &root);
                        // The column-to-constraint conversion: what the arms
                        // cover between them, over the finite presences the
                        // demand just minted. Its ordered per-arm forms also
                        // become the explicit guarded constraint below.
                        if let Some(cover) = cover {
                            let raw: Vec<Formula> = (0..arms.len())
                                .map(|arm| cover.get(&arm).cloned().unwrap_or(Formula::True))
                                .collect();
                            let fields = match &*demand {
                                Ty::Struct(row) => row
                                    .labels
                                    .iter()
                                    .map(|(name, field)| (name.clone(), field.presence.clone()))
                                    .collect(),
                                _ => Vec::new(),
                            };
                            let paths = structural_presence_paths(&demand);
                            let formula = Formula::any(raw.clone());
                            let premise_reason = self.table.require(
                                span,
                                Origin::Coverage(Coverage {
                                    arms: raw.clone(),
                                    fields,
                                    paths,
                                }),
                                formula,
                            );
                            qualifying = Some((raw, premise_reason));
                        }
                        demand
                    }
                };
                let actual = scrutinee.ty.clone();
                self.checks(
                    scrutinee.at,
                    &actual,
                    &expected,
                    ConstraintOrigin::MatchScrutinee,
                    Subject::PatternDemand,
                    None,
                    Subject::MatchScrutinee,
                );

                match qualifying {
                    Some((raw, premise_reason)) => {
                        let effective = effective_conditions(&raw);
                        let mut guarded = Vec::with_capacity(arms.len());
                        for (((pattern, body), raw), effective) in
                            arms.iter_mut().zip(raw).zip(effective)
                        {
                            // The arm is a scope in the generated constraint
                            // tree. Store batches emitted while walking it are
                            // held inert in their source-order slots and travel
                            // with it, so solving can put the premise around them.
                            let outer = std::mem::take(&mut self.out);
                            let required = self.table.store.batches.len();
                            let combined = self.presence_guard.clone().and(effective.clone());
                            let enclosing = std::mem::replace(&mut self.presence_guard, combined);
                            self.infer_term(body);
                            self.presence_guard = enclosing;
                            let constraints = std::mem::replace(&mut self.out, outer);
                            let requirements = self.defer_requirements(required);
                            let mut arm_result = self.constraint(
                                body.at,
                                ConstraintOrigin::MatchArm,
                                ConstraintSubjects::pair(Subject::MatchResult, Subject::MatchArm),
                                ConstraintKind::Equal {
                                    expected: result.clone(),
                                    actual: body.ty.clone(),
                                },
                            );
                            Self::mark_branch_result(&mut arm_result, span);
                            guarded.push(GuardedArm {
                                at: pattern.at,
                                raw,
                                effective,
                                premise_reason,
                                constraints,
                                requirements,
                                result: arm_result,
                            });
                        }
                        self.emit(
                            span,
                            ConstraintOrigin::Match,
                            ConstraintSubjects::pair(Subject::MatchScrutinee, Subject::MatchResult),
                            ConstraintKind::Match {
                                scrutinee: expected,
                                result: result.clone(),
                                arms: guarded,
                                store_end: self.table.store.batches.len(),
                            },
                        );
                    }
                    // A mixed tag/literal column keeps the old flat equality
                    // constraints exactly.
                    None => {
                        for (_, body) in arms.iter_mut() {
                            self.infer_term(body);
                            let actual = body.ty.clone();
                            self.checks(
                                body.at,
                                &actual,
                                &result,
                                ConstraintOrigin::MatchArm,
                                Subject::MatchResult,
                                None,
                                Subject::MatchArm,
                            );
                            Self::mark_branch_result(
                                self.out.last_mut().expect("just emitted branch result"),
                                span,
                            );
                        }
                    }
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
        let answer = self.table.fresh_type_for(Subject::HandlerAnswer);
        // One instantiation of each handled effect's parameters, shared by
        // the label the body is handled under and every arm of that effect:
        // the computation and the arms together decide one application.
        let instances: IndexMap<String, Vec<Assigned>> = handler
            .discharges
            .iter()
            .map(|effect| {
                (
                    self.effect_ids[&effect.anchored].row_key(),
                    self.fresh_effect_arguments(effect.anchored),
                )
            })
            .collect();
        let discharged: IndexMap<String, RowField> = instances
            .iter()
            .map(|(key, fresh)| (key.clone(), RowField::present(argument_tuple(fresh))))
            .collect();
        let extended = Row {
            labels: discharged,
            rest: Rest::More(Rc::new(self.ambient.row.clone())),
        };
        // What the ambient may not stand for: an effect this handler already
        // discharges. A row that acquired one would name it twice, which is
        // exactly the masking R16 leaves refused.
        self.table.note_lacks_row(&extended, Shape::Effect);
        let mut label_spans = self.ambient.label_spans.clone();
        label_spans.extend(
            handler
                .discharges
                .iter()
                .map(|effect| (self.effect_ids[&effect.anchored].row_key(), effect.at)),
        );
        let outer = self.enter(Ambient {
            row: extended,
            inside: self.ambient.inside,
            boundary_at: self.ambient.boundary_at,
            label_spans,
        });
        self.infer_term(body);
        self.leave(outer);

        // The arms run in the outer row, but that row now has a lacks fact for
        // every discharged effect. Retain the written labels as provenance
        // while walking those arms so an overlap can point to both operations.
        let previous_label_spans = self.ambient.label_spans.clone();
        self.ambient.label_spans.extend(
            handler
                .discharges
                .iter()
                .map(|effect| (self.effect_ids[&effect.anchored].row_key(), effect.at)),
        );

        // An arm answers the handler around it, whichever arm a `raise` inside
        // it is written in.
        let held = self.answer.replace(answer.clone());
        for arm in &mut handler.arms {
            // Indexed rather than looked up, for the reason an operation
            // reference is: lowering keeps no arm whose operation it could not
            // resolve.
            let (from, to) =
                self.operations[&(arm.effect.anchored, arm.selector.anchored.clone())].clone();
            let key = self.effect_ids[&arm.effect.anchored].row_key();
            let fresh = instances.get(&key).cloned().unwrap_or_default();
            let (from, to) = (from.open(&fresh), to.open(&fresh));
            self.table.note_lacks(&from);
            self.table.note_lacks(&to);
            self.env.insert(arm.binder.anchored, Binding::Mono(from));
            self.infer_term(&mut arm.body);
            let actual = arm.body.ty.clone();
            self.checks(
                arm.body.at,
                &actual,
                &to,
                ConstraintOrigin::HandlerArm,
                Subject::Context,
                None,
                Subject::HandlerArm,
            );
        }
        match &mut handler.ret {
            // `| return p => e` binds `p` at the type of the handled
            // expression and gives the answer, which is the one thing that can
            // make the whole expression a different type from its body.
            Some(ret) => {
                self.env
                    .insert(ret.binder.anchored, Binding::Mono(body.ty.clone()));
                self.infer_term(&mut ret.body);
                let actual = ret.body.ty.clone();
                self.checks(
                    ret.body.at,
                    &actual,
                    &answer,
                    ConstraintOrigin::HandlerReturn,
                    Subject::HandlerAnswer,
                    None,
                    Subject::HandlerReturn,
                );
            }
            // With none, the answer is what the body came to — said as one
            // equation rather than as a rule of its own.
            None => {
                let actual = body.ty.clone();
                self.checks(
                    body.at,
                    &actual,
                    &answer,
                    ConstraintOrigin::HandlerFallback,
                    Subject::HandlerAnswer,
                    None,
                    Subject::HandlerBody,
                );
            }
        }
        self.answer = held;
        self.ambient.label_spans = previous_label_spans;
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
    /// across the arms that mention it. The demand is closed — its constructor the
    /// closed empty struct, so no further fields can attach — iff every
    /// entry is an exact struct or unit pattern; any `..`, binder or wildcard
    /// entry leaves it open, a fresh struct-row tail with the projection's lacks note.
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
        let mut binds: Vec<(usize, Anchored<Symbol>)> = Vec::new();
        let mut primitives: Vec<Ty> = Vec::new();
        let mut tags: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        let mut fields: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        // Whether any array pattern tests the position, the element patterns
        // of every one — from either end — and the names their rests bind.
        let mut arrays = false;
        let mut elements: Vec<(usize, Col)> = Vec::new();
        let mut rests: Vec<Anchored<Symbol>> = Vec::new();
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
                Col::Pattern(pattern) => match &pattern.anchored {
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
                    ir::PatternKind::Natural(_) => primitives.push(Ty::Nat),
                    ir::PatternKind::Fixed(value) => primitives.push(Ty::Fixed(value.kind())),
                    ir::PatternKind::Integer(_) => primitives.push(Ty::Int),
                    ir::PatternKind::Real(_) => primitives.push(Ty::Real),
                    ir::PatternKind::String(_) => primitives.push(Ty::String),
                    ir::PatternKind::Boolean(_) => primitives.push(Ty::Boolean),
                    ir::PatternKind::Tag { name, payload } => {
                        let payload = payload.as_deref().map(Col::Pattern).unwrap_or(Col::Unit);
                        tags.entry(name.anchored.as_str())
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
                    // A length test is a gap no presence formula speaks of,
                    // so the column keeps the matrix walk, as a tag's does.
                    ir::PatternKind::Array {
                        before,
                        rest,
                        after,
                    } => {
                        arrays = true;
                        exact = false;
                        qualifies = false;
                        for element in before.iter().chain(after) {
                            elements.push((*arm, Col::Pattern(element)));
                        }
                        if let Some(name) = rest.as_ref().and_then(|rest| rest.name) {
                            rests.push(name);
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
                true => self.table.fresh_row_for(Subject::PatternDemand),
                false => Rest::Closed,
            };
            let ty = Rc::new(Ty::plain(Ty::Sum(Row {
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
        if !primitives.is_empty() {
            exact = false;
            qualifies = false;
            demands.extend(primitives.into_iter().map(|core| Rc::new(Ty::plain(core))));
        }
        // An array's elements are of one type, so every element pattern of
        // every arm — wherever it sits among them — constrains the one element
        // position, and a rest binds the array of them.
        if arrays {
            path.push(ir::Step::Element);
            let (element, _) = self.position(columns, path, &elements);
            path.pop();
            let ty = Rc::new(Ty::Array(element));
            for binder in rests {
                self.env.insert(binder.anchored, Binding::Mono(ty.clone()));
            }
            demands.push(ty);
        }
        if structs {
            // The column-union rule for fields. A field is certainly there
            // only when every entry of the column asks for it; a field some
            // entries do without gets a fresh presence variable, so whether it
            // is there is the scrutinee's to decide — the inference behind an
            // optional field. The demand closes over the named fields exactly
            // when every entry is exact: its constructor is then the fieldless unit,
            // which no further field can attach to. An open demand keeps a
            // fresh struct-row tail with the projection's lacks note, asking only for
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
                    false => self.table.fresh_presence_for(Subject::PatternDemand),
                };
                named.insert(
                    name.to_string(),
                    RowField {
                        presence,
                        ty: field,
                    },
                );
            }
            let rest = match exact {
                true => Rest::Closed,
                false => self.table.fresh_row_for(Subject::PatternDemand),
            };
            let ty = Rc::new(Ty::Struct(Row {
                labels: named.clone(),
                rest,
            }));
            self.table.note_lacks(&ty);
            demands.push(ty);
        }
        let mut demands = demands.into_iter();
        // A column that only binds demands nothing: the position is a fresh
        // type the scrutinee decides — the `c` of the column-union example.
        let ty = demands
            .next()
            .unwrap_or_else(|| self.table.fresh_type_for(Subject::PatternDemand));
        for also in demands {
            self.checks(
                columns.at,
                &also,
                &ty,
                ConstraintOrigin::Pattern,
                Subject::PatternDemand,
                None,
                Subject::PatternDemand,
            );
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
                    Rc::new(Ty::plain(Ty::Sum(Row {
                        labels: refined,
                        rest: rest.clone(),
                    })))
                }
                None => ty.clone(),
            };
            self.env.insert(binder.anchored, Binding::Mono(view));
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
    pub(super) fn check_term(
        &mut self,
        term: &mut Term,
        expected: &Rc<Ty>,
        expected_subject: Subject,
        expected_span: Option<Anchor>,
    ) {
        // Checking looks through a name — an annotation of `list` still pushes
        // into a struct literal — but `term.ty` is set from `expected` rather
        // than from this, so the term keeps the name the user wrote and prints
        // as it.
        let shape = self.table.unfolded(self.aliases, expected);
        match (&mut term.kind, &*shape) {
            // Inputs and results use the annotation immediately. Effects meet
            // that same promise after fresh local state has been isolated;
            // the final relation preserves all remaining row sharing.
            (TermKind::Fn { arg, body }, Ty::Arrow(from, to, does)) => {
                let (from, to, does) = (from.clone(), to.clone(), does.clone());
                self.table.level += 1;
                let level = self.table.level;
                let internal = Row::of(self.table.fresh_row_for(Subject::AmbientEffects));
                self.env.insert(arg.anchored, Binding::Mono(from.clone()));
                self.binding_effect_provenance.insert(
                    arg.anchored,
                    super::EffectProvenance::parameter(arg.anchored),
                );
                self.callable_effect_scopes.push(Vec::new());
                let outer = self.enter(Ambient {
                    row: internal.clone(),
                    inside: true,
                    boundary_at: term.at,
                    label_spans: IndexMap::new(),
                });
                let held = self.answer.take();
                self.check_term(body, &to, expected_subject, expected_span);
                self.answer = held;
                self.leave(outer);
                let callable = self
                    .callable_effect_scopes
                    .pop()
                    .expect("checked function effect scope");
                let result = self
                    .term_effect_provenance
                    .get(&body.at)
                    .cloned()
                    .unwrap_or_default();
                self.term_effect_provenance.insert(
                    term.at,
                    super::EffectProvenance::function(callable.clone(), arg.anchored, result),
                );
                self.emit(
                    term.at,
                    ConstraintOrigin::ApplicationEffects,
                    ConstraintSubjects::one(Subject::AmbientEffects),
                    ConstraintKind::Isolate {
                        effect_origins: callable,
                        input: from,
                        output: to,
                        internal,
                        external: does,
                        level,
                    },
                );
                self.table.level -= 1;
                term.ty = expected.clone();
            }
            // Only the exact closed shape a literal already has is pushed
            // into one field by field: every expected field certainly there,
            // no room for more, and the same names on both sides. Anything
            // open or optional falls through to inferring the literal and
            // letting row unification line the two up — which decides the
            // same things, just without the better spans pushing gives. The
            // gate reads the written type's own syntax, never the table, so
            // generation stays a description of the term. A literal with a
            // spread falls through too: what fields it has is the spread
            // value's to say, and only the solver finds out.
            (
                TermKind::Struct {
                    fields,
                    spread: None,
                },
                Ty::Struct(row),
            ) if matches!(row.rest, Rest::Closed)
                && row
                    .labels
                    .values()
                    .all(|field| matches!(field.presence, Presence::Present))
                && same_field_set(fields, &row.labels) =>
            {
                let mut provenance_fields = IndexMap::new();
                for (name, field) in fields.iter_mut() {
                    let want = row.labels[name].ty.clone();
                    self.check_term(&mut field.value, &want, expected_subject, expected_span);
                    if let Some(value) = self.term_effect_provenance.get(&field.value.at).cloned() {
                        provenance_fields.insert(name.clone(), value);
                    }
                }
                let provenance = super::EffectProvenance::from_fields(provenance_fields);
                if provenance != super::EffectProvenance::default() {
                    self.term_effect_provenance.insert(term.at, provenance);
                }
                term.ty = expected.clone();
            }
            (TermKind::Array(items), Ty::Array(element)) => {
                for item in items {
                    match item.spread {
                        Some(dots) => {
                            self.check_term(&mut item.value, expected, Subject::Spread, Some(dots))
                        }
                        None => self.check_term(
                            &mut item.value,
                            element,
                            expected_subject,
                            expected_span,
                        ),
                    }
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
            (TermKind::Tag { name, payload }, Ty::Sum(cases))
                if cases
                    .labels
                    .get(&name.anchored)
                    .is_some_and(|case| matches!(case.presence, Presence::Present)) =>
            {
                let want = cases.labels[&name.anchored].ty.clone();
                match payload {
                    Some(payload) => {
                        self.check_term(payload, &want, expected_subject, expected_span)
                    }
                    // Nothing written is unit, and the case has to carry one.
                    // Said as a constraint rather than pushed, since there is
                    // no term here to push into — and worded with the tag's own
                    // span, which is the whole of what the reader wrote.
                    None => {
                        let carried = Rc::new(Ty::unit());
                        self.checks(
                            name.at,
                            &carried,
                            &want,
                            ConstraintOrigin::ContextualCheck,
                            expected_subject,
                            expected_span,
                            Subject::Term,
                        );
                    }
                }
                term.ty = expected.clone();
            }
            _ => {
                self.infer_term(term);
                let actual = term.ty.clone();
                self.checks(
                    term.at,
                    &actual,
                    expected,
                    ConstraintOrigin::ContextualCheck,
                    expected_subject,
                    expected_span,
                    Subject::Term,
                );
            }
        }
    }

    /// The type of one name in scope. A polymorphic binding is instantiated —
    /// each use gets its own copy of the quantified variables — while a
    /// monomorphic one is shared, so uses of a lambda argument constrain each
    /// other, which is exactly the let/lambda distinction. A name a nested
    /// `let` bound is the polymorphic case with the scheme still to come, so
    /// the copy is asked for rather than made.
    fn lookup(&mut self, span: Anchor, symbol: Symbol) -> Rc<Ty> {
        // Indexed rather than looked up. A lambda's argument is bound where the
        // walk enters its body; a nested `let`'s name is bound before its own
        // value is walked; a top-level definition is bound before any body
        // that could name it is walked — its own group's, monomorphically, and
        // every earlier group's as a scheme — and a name that resolved to
        // nothing already became `TermKind::Error`. A lookup falling back to a
        // fresh variable would hide the day one of those stops holding.
        match self
            .env
            .get(symbol)
            .cloned()
            .expect("every name the lowering kept is bound before it is read")
        {
            Binding::Mono(ty) => {
                if let Some(provenance) = self.binding_effect_provenance.get(&symbol).cloned() {
                    self.term_effect_provenance.insert(span, provenance);
                }
                ty
            }
            Binding::Poly(scheme) => {
                if *scheme.effect_provenance != super::EffectProvenance::default() {
                    self.term_effect_provenance
                        .insert(span, (*scheme.effect_provenance).clone());
                }
                self.table.instantiate_local(span, symbol, &scheme)
            }
            // The scheme is not written yet, so the walk says what this use is
            // rather than what it has: a fresh copy of whatever the enclosing
            // [`ConstraintKind::Let`] publishes. Which keeps the invariant this
            // pass is built on — nothing here reads the table — over the one
            // construct that would otherwise have to wait for a solve.
            Binding::Local => {
                if let Some(provenance) = self.local_effect_provenance.get(&symbol).cloned() {
                    self.term_effect_provenance.insert(span, provenance);
                }
                let ty = self.table.fresh_type_for(Subject::Instance);
                // The scheme does not exist yet, but its possible store batch
                // still has a source position. Reserve an inert slot now; the
                // solver fills it (under any active arm premise) once the local
                // has been generalized, or omits it at publication if the
                // scheme requires nothing.
                let requirement = self.table.store.batches.len();
                self.table.require(
                    span,
                    Origin::Instance(Named {
                        labels: Vec::new(),
                        shape: None,
                    }),
                    Formula::True,
                );
                self.table.deferred.insert(requirement);
                self.emit(
                    span,
                    ConstraintOrigin::Instance,
                    ConstraintSubjects::pair(Subject::Scheme, Subject::Instance),
                    ConstraintKind::Instance {
                        symbol,
                        ty: ty.clone(),
                        requirement,
                    },
                );
                ty
            }
        }
    }
}

impl Constrain<'_> {
    /// Require access to the cell region at this expression.
    fn mutation(&mut self, span: Anchor, region: Rc<Ty>) {
        let performed = Row {
            labels: [(
                crate::types::mutation_effect().row_key(),
                RowField::present(argument_tuple(&[Assigned::Ty(region)])),
            )]
            .into(),
            rest: Rest::Closed,
        };
        self.emit(
            span,
            ConstraintOrigin::ApplicationEffects,
            ConstraintSubjects::one(Subject::PerformedEffects),
            ConstraintKind::Performs {
                performed,
                ambient: self.ambient.row.clone(),
                effect_origins: Vec::new(),
                ambient_label_spans: self.ambient.label_spans.clone(),
                inside: self.ambient.inside,
            },
        );
    }

    /// One fresh argument per parameter the effect declares, of the sort the
    /// parameter stands for: a type variable for a type, and for a row a fresh
    /// row wrapped as the type a row argument is written as — a struct for
    /// fields, a sum for cases and effects — forbidden the labels the
    /// declaration names beside it.
    fn fresh_effect_arguments(&mut self, effect: Symbol) -> Vec<Assigned> {
        let kinds = self.effect_params.get(&effect).cloned().unwrap_or_default();
        kinds
            .iter()
            .map(|kind| match kind.row() {
                None if kind.sense() == crate::types::Sense::Region => {
                    Assigned::Ty(self.table.fresh_region())
                }
                None => Assigned::Ty(self.table.fresh_type_for(Subject::Instance)),
                Some((shape, lacks)) => {
                    let rest = self.table.fresh_row_for(Subject::Instance);
                    let row = Row::of(rest);
                    self.table.forbid(&row, shape, lacks);
                    Assigned::Ty(Rc::new(match shape {
                        Shape::Struct => Ty::Struct(row),
                        Shape::Sum => Ty::Sum(row),
                        Shape::Effect => Ty::effects_argument(row),
                    }))
                }
            })
            .collect()
    }
}

/// The payload an effect label carries: its arguments, in order, as the
/// positional struct a tuple is. An effect without parameters carries the
/// unit tuple, as every label once did.
pub(crate) fn argument_tuple(args: &[Assigned]) -> Rc<Ty> {
    Rc::new(Ty::Struct(Row {
        labels: args
            .iter()
            .enumerate()
            .map(|(at, arg)| (at.to_string(), RowField::present(arg.as_ty())))
            .collect(),
        rest: Rest::Closed,
    }))
}
