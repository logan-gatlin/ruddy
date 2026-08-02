//! Assigning a type to every term.
//!
//! Hindley–Milner: unification, `let`-generalization, and Rémy-style levels
//! to decide what a definition may quantify over. Types are equated, never
//! ordered — two types either unify or they are an error — so a term's type
//! is the one type it has rather than a bound on it.
//!
//! Inference runs after lowering and mutates the [`Program`] it is handed:
//! every [`Term`]'s `ty` goes from [`Ty::Undecided`] to what was inferred for
//! it, fully resolved, so nothing downstream ever needs the solver's variable
//! table to read a type. Errors do not stop the walk — a term that failed to
//! type still has a type, [`Ty::Undecided`], which unifies with everything so
//! that one mistake is reported once rather than echoed by every consumer.

use std::{
    collections::{HashMap, HashSet},
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
    pub errors: Vec<Error>,
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
    /// Projection out of a type that is not — or is not yet known to be — a
    /// struct. A base that is still a variable lands here too: without an
    /// annotation, plain unification cannot conjure the struct to project from.
    NotAStruct { base: Rc<Ty> },
    /// Projection of a field the struct does not have.
    MissingField { base: Rc<Ty> },
}

struct Infer {
    /// One slot per type variable ever minted; [`Ty::Var`] indexes into it.
    vars: Vec<Slot>,
    /// What each symbol in scope means. Symbols are globally unique, so one
    /// flat map serves every scope at once and nothing is ever popped: a
    /// lambda argument can never collide with a top-level definition.
    env: HashMap<Symbol, Binding>,
    aliases: IndexMap<Symbol, Rc<Ty>>,
    errors: Vec<Error>,
    /// The current generalization level. Fresh variables are born at it, and
    /// generalization quantifies exactly the variables born deeper than it.
    level: u32,
}

/// Assign a type to every term in the program, in place, and return the
/// schemes of its top-level definitions.
pub fn infer(program: &mut Program) -> Output {
    let mut cx = Infer {
        vars: Vec::new(),
        env: HashMap::new(),
        aliases: IndexMap::new(),
        errors: Vec::new(),
        level: 0,
    };

    // Aliases first: annotations refer to them. Lowering only lets a type name
    // a type declared above it, so one in-order pass unfolds every alias.
    for (symbol, decl) in &program.types {
        let ty = cx.lower_type(&decl.value);
        cx.aliases.insert(*symbol, ty);
    }

    let mut schemes = IndexMap::new();
    for (symbol, decl) in program.terms.iter_mut() {
        // Each definition is solved one level in, so that everything still
        // unsolved when it ends is provably local to it and can be quantified.
        cx.level = 1;
        let ty = match &decl.annotation {
            // The annotation is the contract: the body is checked against it,
            // and it — not whatever the body inferred along the way — is what
            // the definition means to everyone downstream.
            Some(annotation) => {
                let expected = cx.lower_type(annotation);
                cx.check_term(&mut decl.value, &expected);
                expected
            }
            None => {
                cx.infer_term(&mut decl.value);
                decl.value.ty.clone()
            }
        };
        cx.level = 0;
        let (scheme, subst) = cx.generalize(&ty);
        // With the substitution in hand, resolve every type the walk wrote
        // into the body, so a term's type and its definition's scheme spell
        // the same variable the same way.
        cx.zonk_term(&mut decl.value, &subst);
        cx.env.insert(*symbol, Binding::Poly(scheme.clone()));
        schemes.insert(*symbol, scheme);
    }

    // Error payloads resolve last: a variable in one may have been solved
    // after the error was recorded, and the later knowledge reads better.
    let none = HashMap::new();
    let errors = cx
        .errors
        .iter()
        .map(|error| Error {
            span: error.span,
            kind: match &error.kind {
                ErrorKind::Mismatch { expected, actual } => ErrorKind::Mismatch {
                    expected: cx.zonk(expected, &none),
                    actual: cx.zonk(actual, &none),
                },
                ErrorKind::Recursive => ErrorKind::Recursive,
                ErrorKind::NotAStruct { base } => ErrorKind::NotAStruct {
                    base: cx.zonk(base, &none),
                },
                ErrorKind::MissingField { base } => ErrorKind::MissingField {
                    base: cx.zonk(base, &none),
                },
            },
        })
        .collect();

    Output {
        aliases: cx.aliases,
        schemes,
        errors,
    }
}

impl Infer {
    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.errors.push(Error { span, kind });
    }

    fn fresh(&mut self) -> Rc<Ty> {
        let var = self.vars.len() as TyVar;
        self.vars.push(Slot::Unbound { level: self.level });
        Rc::new(Ty::Var(var))
    }

    /// Follow bound variables until reaching something that is not one. Only
    /// the head is resolved; a composite's children still need their own
    /// resolution, which is what [`zonk`](Self::zonk) does exhaustively.
    fn resolve_ty(&self, ty: &Rc<Ty>) -> Rc<Ty> {
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

    /// The semantic type a written type denotes. Aliases unfold — this is
    /// where `Endo` becomes `Nat -> Nat` — and a type that failed to lower
    /// becomes [`Ty::Undecided`], which absorbs rather than cascades.
    fn lower_type(&self, ty: &Type) -> Rc<Ty> {
        match &ty.tracked {
            TypeKind::Prim(prim) => Rc::new((*prim).into()),
            TypeKind::Ident(symbol) => self
                .aliases
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| Rc::new(Ty::Undecided)),
            TypeKind::Arrow { from, to } => {
                Rc::new(Ty::Arrow(self.lower_type(from), self.lower_type(to)))
            }
            TypeKind::Struct(fields) => Rc::new(Ty::Struct(
                fields
                    .iter()
                    .map(|(name, field)| (name.clone(), self.lower_type(&field.value)))
                    .collect(),
            )),
            TypeKind::Error => Rc::new(Ty::Undecided),
        }
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
                let result = self.fresh();
                let wanted = Rc::new(Ty::Arrow(arg.ty.clone(), result.clone()));
                // The function side is the `actual`: applying a non-function
                // should read as "expected an arrow, found what you applied".
                self.unify(span, &wanted, &func.ty.clone());
                result
            }
            TermKind::Fn { arg, body } => {
                let param = self.fresh();
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
            TermKind::Project { base, field } => {
                self.infer_term(base);
                let resolved = self.resolve_ty(&base.ty);
                match &*resolved {
                    Ty::Struct(fields) => match fields.get(&field.tracked) {
                        Some(ty) => ty.clone(),
                        None => {
                            self.error(
                                field.span,
                                ErrorKind::MissingField {
                                    base: resolved.clone(),
                                },
                            );
                            Rc::new(Ty::Undecided)
                        }
                    },
                    Ty::Undecided => Rc::new(Ty::Undecided),
                    _ => {
                        self.error(
                            base.span,
                            ErrorKind::NotAStruct {
                                base: resolved.clone(),
                            },
                        );
                        Rc::new(Ty::Undecided)
                    }
                }
            }
        };
    }

    /// Check `term` against a type the context already knows. Checking pushes
    /// expected types *into* binders — an annotated `fn p => p.x` learns `p`'s
    /// type from the annotation before the body needs it, which inference
    /// alone could not order. Everywhere the shapes do not line up, checking
    /// falls back to inferring and unifying.
    fn check_term(&mut self, term: &mut Term, expected: &Rc<Ty>) {
        let resolved = self.resolve_ty(expected);
        match (&mut term.kind, &*resolved) {
            (TermKind::Fn { arg, body }, Ty::Arrow(from, to)) => {
                let (from, to) = (from.clone(), to.clone());
                self.env.insert(arg.tracked, Binding::Mono(from));
                self.check_term(body, &to);
                term.ty = resolved.clone();
            }
            (TermKind::Struct(fields), Ty::Struct(tys))
                if fields.len() == tys.len() && fields.keys().all(|k| tys.contains_key(k)) =>
            {
                for (name, field) in fields.iter_mut() {
                    let want = tys[name].clone();
                    self.check_term(&mut field.value, &want);
                }
                term.ty = resolved.clone();
            }
            _ => {
                self.infer_term(term);
                let actual = term.ty.clone();
                self.unify(term.span, &resolved, &actual);
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
        let fresh: Vec<_> = (0..scheme.count()).map(|_| self.fresh()).collect();
        open(scheme.body(), &fresh)
    }

    /// Make `expected` and `actual` the same type, or report where they
    /// cannot be. Failure leaves both sides as they were: the error is
    /// recorded once and the walk continues.
    fn unify(&mut self, span: Span, expected: &Rc<Ty>, actual: &Rc<Ty>) {
        let lhs = self.resolve_ty(expected);
        let rhs = self.resolve_ty(actual);
        match (&*lhs, &*rhs) {
            // Undecided is the absorbing error type: whatever failed under it
            // was reported where it failed.
            (Ty::Undecided, _) | (_, Ty::Undecided) => {}
            (Ty::Var(a), Ty::Var(b)) if a == b => {}
            (Ty::Var(var), _) => self.bind(span, *var, &rhs),
            (_, Ty::Var(var)) => self.bind(span, *var, &lhs),
            (Ty::Nat, Ty::Nat) => {}
            (Ty::Arrow(from1, to1), Ty::Arrow(from2, to2)) => {
                self.unify(span, from1, from2);
                self.unify(span, to1, to2);
            }
            // Fields match by name, not position: structs are records, and
            // `{ x: Nat, y: Nat }` written in either order is the same type.
            (Ty::Struct(want), Ty::Struct(have))
                if want.len() == have.len() && want.keys().all(|k| have.contains_key(k)) =>
            {
                for (name, ty) in want {
                    self.unify(span, ty, &have[name]);
                }
            }
            _ => self.error(
                span,
                ErrorKind::Mismatch {
                    expected: lhs.clone(),
                    actual: rhs.clone(),
                },
            ),
        }
    }

    /// Point an unbound variable at a type, unless the type contains the
    /// variable itself — the occurs check that keeps every type a finite
    /// tree. On failure the variable stays unbound; the cycle is reported at
    /// the constraint that would have closed it.
    fn bind(&mut self, span: Span, var: TyVar, ty: &Rc<Ty>) {
        let Slot::Unbound { level } = self.vars[var as usize] else {
            unreachable!("resolve_ty only stops at unbound variables");
        };
        if self.occurs(var, level, ty) {
            self.error(span, ErrorKind::Recursive);
            return;
        }
        self.vars[var as usize] = Slot::Bound(ty.clone());
    }

    /// Whether `var` occurs in `ty` — and, on the same walk, the level
    /// adjustment: every unbound variable in `ty` is pulled up to `level` if
    /// it was deeper, because it is about to be reachable from something at
    /// `level` and must not be generalized past it.
    fn occurs(&mut self, var: TyVar, level: u32, ty: &Rc<Ty>) -> bool {
        let ty = self.resolve_ty(ty);
        match &*ty {
            Ty::Var(other) => {
                if *other == var {
                    return true;
                }
                let Slot::Unbound { level: at } = &mut self.vars[*other as usize] else {
                    unreachable!("resolve_ty only stops at unbound variables");
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
        let ty = self.resolve_ty(ty);
        match &*ty {
            Ty::Var(var) => {
                let Slot::Unbound { level } = self.vars[*var as usize] else {
                    unreachable!("resolve_ty only stops at unbound variables");
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
        let ty = self.resolve_ty(ty);
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
