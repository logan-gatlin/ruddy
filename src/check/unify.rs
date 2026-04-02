use crate::ty::store::TypeStore;
use crate::ty::{
    Kind, KindId, KindVariableId, MetaTypeVariableId, TraitPredicate, TypeBinder, TypeId, TypeKind,
};

/// Describes why two types or kinds could not be unified.
#[derive(Debug, Clone, PartialEq)]
pub enum UnificationError {
    /// Two types have incompatible structure.
    TypeMismatch { expected: TypeId, found: TypeId },
    /// Two kinds have incompatible structure.
    KindMismatch { expected: KindId, found: KindId },
    /// Solving a meta-variable would create an infinite type.
    OccursCheck { var: MetaTypeVariableId, ty: TypeId },
    /// Solving a kind variable would create an infinite kind.
    KindOccursCheck { var: KindVariableId, kind: KindId },
}

/// Tracks solutions for meta type variables and kind variables.
///
/// Operates on types stored in a [`TypeStore`]. The table records which
/// meta-variables have been solved and to what — it does not own the types
/// themselves.
#[derive(Debug)]
pub struct UnificationTable {
    /// Solution for each meta type variable. `None` means unsolved.
    type_solutions: Vec<Option<TypeId>>,
    /// The kind of each meta type variable.
    type_var_kinds: Vec<KindId>,
    /// Solution for each kind variable. `None` means unsolved.
    kind_solutions: Vec<Option<KindId>>,
}

impl UnificationTable {
    pub fn new() -> Self {
        Self {
            type_solutions: Vec::new(),
            type_var_kinds: Vec::new(),
            kind_solutions: Vec::new(),
        }
    }

    // --- Variable allocation ---

    /// Allocate a fresh meta type variable with the given kind.
    ///
    /// Returns both the raw variable id and a [`TypeId`] referencing it in the
    /// store.
    pub fn fresh_type_var(
        &mut self,
        store: &mut TypeStore,
        kind: KindId,
    ) -> (MetaTypeVariableId, TypeId) {
        let var = MetaTypeVariableId(self.type_solutions.len() as u32);
        self.type_solutions.push(None);
        self.type_var_kinds.push(kind);
        let ty = store.mk_meta(var, kind);
        (var, ty)
    }

    /// Allocate a fresh kind variable.
    ///
    /// Returns both the raw variable id and a [`KindId`] referencing it in the
    /// store.
    pub fn fresh_kind_var(&mut self, store: &mut TypeStore) -> (KindVariableId, KindId) {
        let var = KindVariableId(self.kind_solutions.len() as u32);
        self.kind_solutions.push(None);
        let kind = store.alloc_kind(Kind::Variable(var));
        (var, kind)
    }

    /// Look up the current solution for a meta type variable.
    pub fn probe_type_var(&self, var: MetaTypeVariableId) -> Option<TypeId> {
        self.type_solutions[var.0 as usize]
    }

    /// Look up the current solution for a kind variable.
    pub fn probe_kind_var(&self, var: KindVariableId) -> Option<KindId> {
        self.kind_solutions[var.0 as usize]
    }

    /// Number of allocated meta type variables.
    pub fn type_var_count(&self) -> usize {
        self.type_solutions.len()
    }

    /// Get the kind of a meta type variable.
    pub fn type_var_kind(&self, var: MetaTypeVariableId) -> KindId {
        self.type_var_kinds[var.0 as usize]
    }

    /// Whether a meta type variable has been solved.
    pub fn is_type_var_solved(&self, var: MetaTypeVariableId) -> bool {
        self.type_solutions[var.0 as usize].is_some()
    }

    // --- Shallow resolution ---

    /// Follow meta-variable solution chains to find the current representative
    /// type. Returns the first non-meta type or the first unsolved meta
    /// variable encountered.
    pub fn shallow_resolve_type(&self, store: &TypeStore, mut ty: TypeId) -> TypeId {
        loop {
            match &store.get_type(ty).kind {
                TypeKind::MetaTypeVariable(var) => match self.type_solutions[var.0 as usize] {
                    Some(solution) => ty = solution,
                    None => return ty,
                },
                _ => return ty,
            }
        }
    }

    /// Follow kind-variable solution chains.
    pub fn shallow_resolve_kind(&self, store: &TypeStore, mut kind: KindId) -> KindId {
        loop {
            match store.get_kind(kind) {
                Kind::Variable(var) => match self.kind_solutions[var.0 as usize] {
                    Some(solution) => kind = solution,
                    None => return kind,
                },
                _ => return kind,
            }
        }
    }

    // --- Type unification ---

    /// Unify two types, recording solutions for any meta-variables encountered.
    ///
    /// `Forall` types are never unified directly — the bidirectional checker
    /// handles them via subsumption (skolemization + instantiation). Attempting
    /// to unify two `Forall` types returns [`UnificationError::TypeMismatch`].
    /// Expects `a` and `b` to have already successfully unified kinds.
    pub fn unify_types(
        &mut self,
        store: &TypeStore,
        a: TypeId,
        b: TypeId,
    ) -> Result<(), UnificationError> {
        let a = self.shallow_resolve_type(store, a);
        let b = self.shallow_resolve_type(store, b);

        if a == b {
            return Ok(());
        }

        let a_ty_kind = store.get_type(a).kind_id;
        let b_ty_kind = store.get_type(b).kind_id;
        self.unify_kinds(store, a_ty_kind, b_ty_kind)?;

        let a_kind = store.get_type(a).kind.clone();
        let b_kind = store.get_type(b).kind.clone();

        match (&a_kind, &b_kind) {
            // Solve meta-variables.
            (TypeKind::MetaTypeVariable(var), _) => self.solve_type_var(store, *var, b),
            (_, TypeKind::MetaTypeVariable(var)) => self.solve_type_var(store, *var, a),

            // Constructors must match exactly.
            (TypeKind::Constructor(c1), TypeKind::Constructor(c2)) => {
                if c1 == c2 {
                    Ok(())
                } else {
                    Err(UnificationError::TypeMismatch {
                        expected: a,
                        found: b,
                    })
                }
            }

            // Applications unify pairwise.
            (TypeKind::Application(f1, a1), TypeKind::Application(f2, a2)) => {
                let (f1, a1, f2, a2) = (*f1, *a1, *f2, *a2);
                self.unify_types(store, f1, f2)?;
                self.unify_types(store, a1, a2)
            }

            // Rigid variables must be identical (already handled by a == b above,
            // but included for clarity).
            (TypeKind::RigidTypeVariable(v1), TypeKind::RigidTypeVariable(v2)) if v1 == v2 => {
                Ok(())
            }

            // Error types absorb mismatches to prevent cascading diagnostics.
            (TypeKind::Error, _) | (_, TypeKind::Error) => Ok(()),

            // Everything else is a mismatch — including Forall types, which the
            // bidirectional checker handles via subsumption rather than raw
            // unification.
            _ => Err(UnificationError::TypeMismatch {
                expected: a,
                found: b,
            }),
        }
    }

    /// Solve a meta type variable to the given type, after occurs check.
    fn solve_type_var(
        &mut self,
        store: &TypeStore,
        var: MetaTypeVariableId,
        ty: TypeId,
    ) -> Result<(), UnificationError> {
        if self.occurs_in_type(store, var, ty) {
            return Err(UnificationError::OccursCheck { var, ty });
        }
        self.type_solutions[var.0 as usize] = Some(ty);
        Ok(())
    }

    /// Check whether `var` occurs anywhere inside `ty` (would create infinite
    /// type).
    fn occurs_in_type(&self, store: &TypeStore, var: MetaTypeVariableId, ty: TypeId) -> bool {
        let ty = self.shallow_resolve_type(store, ty);
        match &store.get_type(ty).kind {
            TypeKind::MetaTypeVariable(v) => *v == var,
            TypeKind::Application(f, a) => {
                self.occurs_in_type(store, var, *f) || self.occurs_in_type(store, var, *a)
            }
            TypeKind::Forall(_, preds, body) => {
                self.occurs_in_type(store, var, *body)
                    || preds.iter().any(|p| {
                        p.arguments
                            .iter()
                            .any(|&a| self.occurs_in_type(store, var, a))
                    })
            }
            TypeKind::Constructor(_) | TypeKind::RigidTypeVariable(_) | TypeKind::Error => false,
        }
    }

    // --- Kind unification ---

    /// Unify two kinds.
    pub fn unify_kinds(
        &mut self,
        store: &TypeStore,
        a: KindId,
        b: KindId,
    ) -> Result<(), UnificationError> {
        let a = self.shallow_resolve_kind(store, a);
        let b = self.shallow_resolve_kind(store, b);

        if a == b {
            return Ok(());
        }

        let a_data = store.get_kind(a).clone();
        let b_data = store.get_kind(b).clone();

        match (&a_data, &b_data) {
            (Kind::Variable(var), _) => self.solve_kind_var(store, *var, b),
            (_, Kind::Variable(var)) => self.solve_kind_var(store, *var, a),

            (Kind::Type, Kind::Type) => Ok(()),

            (Kind::Arrow(from1, to1), Kind::Arrow(from2, to2)) => {
                let (from1, to1, from2, to2) = (*from1, *to1, *from2, *to2);
                self.unify_kinds(store, from1, from2)?;
                self.unify_kinds(store, to1, to2)
            }

            (Kind::Error, _) | (_, Kind::Error) => Ok(()),

            _ => Err(UnificationError::KindMismatch {
                expected: a,
                found: b,
            }),
        }
    }

    /// Solve a kind variable, after occurs check.
    fn solve_kind_var(
        &mut self,
        store: &TypeStore,
        var: KindVariableId,
        kind: KindId,
    ) -> Result<(), UnificationError> {
        if self.occurs_in_kind(store, var, kind) {
            return Err(UnificationError::KindOccursCheck { var, kind });
        }
        self.kind_solutions[var.0 as usize] = Some(kind);
        Ok(())
    }

    /// Check whether `var` occurs anywhere inside `kind`.
    fn occurs_in_kind(&self, store: &TypeStore, var: KindVariableId, kind: KindId) -> bool {
        let kind = self.shallow_resolve_kind(store, kind);
        match store.get_kind(kind) {
            Kind::Variable(v) => *v == var,
            Kind::Arrow(from, to) => {
                self.occurs_in_kind(store, var, *from) || self.occurs_in_kind(store, var, *to)
            }
            Kind::Type | Kind::Error => false,
        }
    }

    // --- Zonking ---

    /// Substitute all solved meta-variables in `ty`, producing a type free of
    /// solved indirections. Unsolved meta-variables remain as-is. May allocate
    /// new types in the store when rebuilding structure.
    pub fn zonk_type(&mut self, store: &mut TypeStore, ty: TypeId) -> TypeId {
        let resolved = self.shallow_resolve_type(store, ty);
        let type_data = store.get_type(resolved).clone();

        match type_data.kind {
            TypeKind::MetaTypeVariable(_)
            | TypeKind::Constructor(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::Error => resolved,

            TypeKind::Application(f, a) => {
                let zf = self.zonk_type(store, f);
                let za = self.zonk_type(store, a);
                if zf == f && za == a {
                    resolved
                } else {
                    let zk = self.zonk_kind(store, type_data.kind_id);
                    store.mk_application(zf, za, zk)
                }
            }

            TypeKind::Forall(binders, predicates, body) => {
                let zbody = self.zonk_type(store, body);
                let zbinders: Vec<TypeBinder> = binders
                    .iter()
                    .map(|b| TypeBinder {
                        id: b.id,
                        name: b.name.clone(),
                        kind: self.zonk_kind(store, b.kind),
                        range: b.range,
                    })
                    .collect();
                let zpreds: Vec<TraitPredicate> = predicates
                    .iter()
                    .map(|p| TraitPredicate {
                        trait_ref: p.trait_ref.clone(),
                        arguments: p
                            .arguments
                            .iter()
                            .map(|&a| self.zonk_type(store, a))
                            .collect(),
                        range: p.range,
                    })
                    .collect();
                if zbody == body && zbinders == binders && zpreds == predicates {
                    resolved
                } else {
                    store.mk_forall(zbinders, zpreds, zbody)
                }
            }
        }
    }

    /// Substitute all solved kind-variables in `kind`. May allocate new kinds
    /// in the store when rebuilding structure.
    pub fn zonk_kind(&mut self, store: &mut TypeStore, kind: KindId) -> KindId {
        let resolved = self.shallow_resolve_kind(store, kind);
        let kind_data = store.get_kind(resolved).clone();

        match kind_data {
            Kind::Variable(_) | Kind::Type | Kind::Error => resolved,
            Kind::Arrow(from, to) => {
                let zfrom = self.zonk_kind(store, from);
                let zto = self.zonk_kind(store, to);
                if zfrom == from && zto == to {
                    resolved
                } else {
                    store.kind_arrow(zfrom, zto)
                }
            }
        }
    }
}

impl Default for UnificationTable {
    fn default() -> Self {
        Self::new()
    }
}
