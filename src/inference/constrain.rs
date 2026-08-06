//! Pass one: generation. See [`Constrain`].

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    ir::{Term, TermKind},
    symbol::Symbol,
    tracking::Span,
    types::{Assigned, Core, Presence, Rest, Row, RowField, Scheme, Ty},
};

use super::{Binding, Constraint, ConstraintKind, Table, same_field_set};

/// Pass one: the walk that says what has to hold, and solves nothing.
pub struct Constrain<'a> {
    pub table: &'a mut Table,
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
                    fields: Row {
                        labels: tys,
                        rest: Rest::Closed,
                    },
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
            // that has the field, whatever else it may also have. The core,
            // the field's type and the tail are all variables, so `fn p => p.x`
            // is not a base waiting to be explained; it is a definition
            // polymorphic in everything but the field it reads.
            TermKind::Project { base, field } => {
                self.infer_term(base);
                let result = self.table.fresh_type();
                let core = Core::Var(self.table.fresh_core());
                let rest = self.table.fresh_row();
                let want = Rc::new(Ty {
                    core,
                    fields: Row {
                        labels: [(field.tracked.clone(), RowField::present(result.clone()))]
                            .into_iter()
                            .collect(),
                        rest,
                    },
                });
                // "Whatever else it may also have" is everything but the field
                // just read, so both the tail and the core minted for it lack
                // that name — the core because it stands for a type the field
                // is already on, and a second copy of it could disagree.
                self.table.note_lacks(&want);
                let actual = base.ty.clone();
                // The field name is the only thing the user can fix about a
                // type that does not have it, whatever kind of type that is.
                self.checks(field.span, &actual, &want);
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
                if matches!(shape.fields.rest, Rest::Closed)
                    && shape
                        .fields
                        .labels
                        .values()
                        .all(|field| matches!(field.presence, Presence::Present))
                    && same_field_set(fields, &shape.fields.labels) =>
            {
                for (name, field) in fields.iter_mut() {
                    let want = shape.fields.labels[name].ty.clone();
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
    /// other, which is exactly the let/lambda distinction.
    fn lookup(&mut self, symbol: Symbol) -> Rc<Ty> {
        // Indexed rather than looked up. A lambda's argument is bound where the
        // walk enters its body; a top-level definition is bound before any body
        // that could name it is walked — its own group's, monomorphically, and
        // every earlier group's as a scheme — and a name that resolved to
        // nothing already became `TermKind::Error`. A lookup falling back to a
        // fresh variable would hide the day one of those stops holding.
        match self.env[&symbol].clone() {
            Binding::Mono(ty) => ty,
            Binding::Poly(scheme) => self.instantiate(&scheme),
        }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Rc<Ty> {
        // One fresh variable per position the scheme bound, handed over as a
        // bare type: which sort each one is, is decided where it lands, since
        // that is what a scheme records. See [`Assigned::as_row`].
        let fresh: Vec<Assigned> = (0..scheme.count())
            .map(|_| Assigned::Ty(self.table.fresh_type()))
            .collect();
        let ty = scheme.body().open(&fresh);
        // A scheme's body says which of its rows a quantified tail is the tail
        // of, but the condition that follows from that is not part of the
        // body: it lived beside the variables the scheme closed over, and this
        // copy's variables are new. Said again, of them.
        self.table.note_lacks(&ty);
        ty
    }
}
