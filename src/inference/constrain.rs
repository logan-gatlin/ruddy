//! Pass one: generation. See [`Constrain`].

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    ir::{self, Term, TermKind},
    symbol::{Mint, Symbol},
    tracking::{Span, Tracked},
    types::{Core, Presence, Rest, Row, RowField, Scheme, Ty, TyVar},
};

use super::{Annotated, Binding, Constraint, ConstraintKind, Table, lower_type, same_field_set};

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
}

/// One entry of a match's column at one position: the sub-pattern an arm wrote
/// there, or the unit a bare tag's payload demands without anything having
/// been written — `` `None `` is `` `None () `` to the types, and only to the
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
                let bound = match annotation {
                    Some(annotation) => {
                        // The variables it minted are counted off, so that an
                        // annotation the value went on to decide can be
                        // refused. See [`Annotated`]. Counting is not
                        // inspecting: how many variables exist says nothing
                        // about what any of them has been solved to, which is
                        // the invariant this pass keeps.
                        let from = self.table.vars.len() as TyVar;
                        let bound = lower_type(self.mint, self.table, annotation);
                        self.annotated.push(Annotated {
                            span: annotation.span,
                            opened: from..self.table.vars.len() as TyVar,
                        });
                        bound
                    }
                    None => self.table.fresh_type(),
                };
                // Monomorphically, the same rule a binding group follows: a use
                // of the name inside its own value is the one type being
                // decided rather than a copy of a scheme that does not exist
                // yet.
                self.env.insert(name.tracked, Binding::Mono(bound.clone()));
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
                        value: required,
                        body: rest,
                    },
                });
                // What the expression evaluates to is what its body evaluates
                // to; the value is what the name is, not what the `let` is.
                body.ty.clone()
            }
            TermKind::Apply { func, arg } => {
                self.infer_term(func);
                self.infer_term(arg);
                let applied = func.ty.clone();
                // Through a name, so that something annotated `Endo` is
                // applied as the arrow it stands for. The arrow the arm then
                // works with is the unfolded one, which is the only shape a
                // call site can take apart.
                match &self.table.unfolded(self.aliases, &applied).core {
                    // The function already knows what it takes, so the demand
                    // on the argument is the parameter type and the result is
                    // the arrow's own. Written this way round, a mismatch
                    // reads "expected <parameter>, found <argument>": the
                    // parameter is what the context asked for, and the
                    // argument is the term the reader can change.
                    Core::Arrow(from, to) => {
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
                        let param = self.table.fresh_type();
                        let result = self.table.fresh_type();
                        let wanted = Rc::new(Ty::plain(Core::Arrow(param.clone(), result.clone())));
                        self.checks(span, &applied, &wanted);
                        let actual = arg.ty.clone();
                        self.checks(arg.span, &actual, &param);
                        result
                    }
                }
            }
            TermKind::Fn { arg, body } => {
                let param = self.table.fresh_type();
                self.env.insert(arg.tracked, Binding::Mono(param.clone()));
                self.infer_term(body);
                Rc::new(Ty::plain(Core::Arrow(param, body.ty.clone())))
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
                        self.position(&columns, &mut Vec::new(), &root)
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

    /// The type of one position of a match, from the column of sub-patterns
    /// the arms wrote there — R7's rule, one recursive step per position.
    ///
    /// The demand is the union of what the whole column tests, never any one
    /// arm's view: the tags tested here are the listed cases of one sum row,
    /// each payload typed by the same rule one level down across every arm
    /// that tests it; a natural test demands `Nat`; a struct pattern demands
    /// its named fields of the position — the same demand a projection makes,
    /// with the same lacks note — each field's type from the sub-position; and
    /// `()` or a bare tag's payload demands unit. The row is closed over its
    /// listed cases iff no arm is irrefutable at the position — a binder at or
    /// above it — and otherwise its rest is a fresh row variable that lacks
    /// the listed names, as a tag literal's tail does.
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
    /// after `` `Some x `` its sum-without-`Some`, and it degrades correctly:
    /// after `` `A `X ``, a later catch-all still sees `` `A `` present,
    /// because `` `A `` values with other payloads reach it.
    fn position(
        &mut self,
        columns: &Columns,
        path: &mut Vec<ir::Step>,
        entries: &[(usize, Col)],
    ) -> Rc<Ty> {
        let mut binds: Vec<(usize, Tracked<Symbol>)> = Vec::new();
        let mut unit = false;
        let mut naturals = false;
        let mut tags: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        let mut fields: IndexMap<&str, Vec<(usize, Col)>> = IndexMap::new();
        for (arm, entry) in entries {
            match entry {
                Col::Unit => unit = true,
                Col::Pattern(pattern) => match &pattern.tracked {
                    ir::PatternKind::Bind(name) => binds.push((*arm, *name)),
                    // A binder minus the binding: it demands nothing of the
                    // position and leaves its row open — the matrix already
                    // counts it among the binds — and there is no name here
                    // for any environment to learn.
                    ir::PatternKind::Wildcard => {}
                    ir::PatternKind::Unit => unit = true,
                    ir::PatternKind::Natural(_) => naturals = true,
                    ir::PatternKind::Tag { name, payload } => {
                        let payload = payload.as_deref().map(Col::Pattern).unwrap_or(Col::Unit);
                        tags.entry(name.tracked.as_str())
                            .or_default()
                            .push((*arm, payload));
                    }
                    ir::PatternKind::Struct(named) => {
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
        // Whether an arm is irrefutable here — a binder at or above, or a
        // catch-all arm — which is what decides whether the row closes. Read
        // off the matrix rather than tracked down the recursion, so this and
        // the lowering checks answer from one place.
        let open = columns.matrix.open(path);
        let mut demands: Vec<Rc<Ty>> = Vec::new();
        // The listed cases, kept beside their row's rest for the binder views
        // below: a view is the same labels re-read, not a second demand.
        let mut listed: Option<(IndexMap<String, RowField>, Rest)> = None;
        if !tags.is_empty() {
            let mut labels = IndexMap::new();
            for (name, payloads) in &tags {
                path.push(ir::Step::Payload(name.to_string()));
                let payload = self.position(columns, path, payloads);
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
        if !fields.is_empty() {
            let mut named = IndexMap::new();
            for (name, subs) in &fields {
                path.push(ir::Step::Field(name.to_string()));
                let field = self.position(columns, path, subs);
                path.pop();
                named.insert(name.to_string(), RowField::present(field));
            }
            let ty = Rc::new(Ty {
                core: Core::Var(self.table.fresh_core()),
                fields: named,
            });
            self.table.note_lacks(&ty);
            demands.push(ty);
        }
        if unit {
            demands.push(Rc::new(Ty::unit()));
        }
        let mut demands = demands.into_iter();
        // A column that only binds demands nothing: the position is a fresh
        // type the scrutinee decides — the `c` of the column-union example.
        let ty = demands.next().unwrap_or_else(|| self.table.fresh_type());
        for also in demands {
            self.checks(columns.at, &also, &ty);
        }
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
        ty
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
    pub(super) fn check_term(&mut self, term: &mut Term, expected: &Rc<Ty>) {
        // Checking looks through a name — an annotation of `list` still pushes
        // into a struct literal — but `term.ty` is set from `expected` rather
        // than from this, so the term keeps the name the user wrote and prints
        // as it.
        let shape = self.table.unfolded(self.aliases, expected);
        match (&mut term.kind, &shape.core) {
            (TermKind::Fn { arg, body }, Core::Arrow(from, to)) => {
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
            Binding::Poly(scheme) => self.table.instantiate(&scheme),
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
