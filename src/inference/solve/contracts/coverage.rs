//! Presence obligations exposed by finite structural case selection.
//!
//! A record's optional field describes alternative runtime shapes. A sum's
//! optional label instead admits that runtime tag; choosing it never excludes
//! the other admitted tags. Preserve that distinction when relating a refined
//! case back to the caller's symbolic input.

use super::*;

pub(super) struct CaseGuard {
    // Retain the allocation whose address keys the map until normalization
    // finishes, so a later temporary type cannot reuse its identity.
    _input: Arc<Ty>,
    pub(super) condition: Formula,
    /// Sum presences admit tags; they do not identify the one runtime tag.
    /// Results selected by a sum match therefore need a producer-owned choice.
    pub(super) producer_owned: bool,
    reason: Option<ReasonId>,
}

pub(super) struct CaseScope {
    guard: Option<Formula>,
    refinement: Option<usize>,
    reasons: Vec<ReasonId>,
}

impl Structural<'_, '_> {
    pub(super) fn guard_cases(
        &mut self,
        original: &Arc<Ty>,
        cases: &[Case],
        uncovered: &[Arc<Ty>],
    ) -> Result<(), Error> {
        let producer_owned = matches!(&*self.resolve_type(original), Ty::Sum(_));
        let mut remaining = 32_768usize;
        let mut coverage = Formula::True;
        for input in uncovered {
            let condition = self.case_condition(original, input, &mut remaining, 0)?;
            coverage = coverage.and(condition.not());
        }
        let coverage = self.solve.table.resolved(&coverage);
        // Concrete invalid inputs still fail immediately. Symbolic uncovered
        // shapes become requirements on the input presences, so annotation
        // checking can compare the complete promise with the derived domain.
        if matches!(coverage, Formula::False) {
            return Err(self.failure(Failure::UncoveredMatch));
        }
        let reason = if coverage.is_true() {
            self.solve.constraint_reason
        } else {
            let slot = self.solve.table.store.batches.len();
            let reason = self.solve.table.require_because(
                self.at,
                Origin::Instance(Named {
                    labels: Vec::new(),
                    shape: None,
                }),
                coverage,
                self.solve.constraint_reason,
            );
            let batch = self.solve.table.store.batches[slot].clone();
            let guarded = self.solve.guarded_batch(batch);
            self.solve.table.store.batches[slot] = guarded;
            Some(reason)
        };
        for case in cases {
            let condition = self.case_condition(original, &case.input, &mut remaining, 0)?;
            self.case_guards.insert(
                Arc::as_ptr(&case.input) as usize,
                CaseGuard {
                    _input: case.input.clone(),
                    condition,
                    producer_owned,
                    reason,
                },
            );
        }
        Ok(())
    }

    fn case_condition(
        &mut self,
        original: &Arc<Ty>,
        refined: &Arc<Ty>,
        remaining: &mut usize,
        depth: usize,
    ) -> Result<Formula, Error> {
        *remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| self.failure(Failure::WorkLimit))?;
        // Partitioning copies only the selected structural paths. Unchanged
        // payloads can contain recursive aliases; unfolding both sides here
        // would follow those aliases forever despite adding no condition.
        if Arc::ptr_eq(original, refined) || self.solve.table.alike(original, refined) {
            return Ok(Formula::True);
        }
        if depth > crate::contracts::MAX_DEPTH {
            return Err(self.failure(Failure::DepthLimit));
        }
        let original = self.resolve_type(original);
        let refined = self.resolve_type(refined);
        match (&*original, &*refined) {
            (Ty::Struct(before), Ty::Struct(after)) => {
                let before = self.solve.table.canon(before);
                let after = self.solve.table.canon(after);
                let mut condition = Formula::True;
                if matches!(before.rest, Rest::Closed) {
                    for (label, field) in after.labels.iter() {
                        if !before.labels.contains_key(label) {
                            condition = condition.and(
                                self.solve
                                    .table
                                    .presence_of(&field.presence)
                                    .formula()
                                    .not(),
                            );
                        }
                    }
                }
                for (label, field) in before.labels.iter() {
                    let present = self.solve.table.presence_of(&field.presence);
                    let Some(selected) = after.labels.get(label) else {
                        condition = condition.and(present.formula().not());
                        continue;
                    };
                    let selected_presence = self.solve.table.presence_of(&selected.presence);
                    if present != selected_presence {
                        condition =
                            condition.and(present.formula().iff(selected_presence.formula()));
                    }
                    if !matches!(selected_presence, Presence::Absent) {
                        let payload =
                            self.case_condition(&field.ty, &selected.ty, remaining, depth + 1)?;
                        condition = condition.and(selected_presence.formula().not().or(payload));
                    }
                }
                Ok(condition)
            }
            (Ty::Sum(before), Ty::Sum(after)) => {
                let before = self.solve.table.canon(before);
                let after = self.solve.table.canon(after);
                let mut condition = Formula::False;
                for (label, selected) in after.labels.iter() {
                    if matches!(
                        self.solve.table.presence_of(&selected.presence),
                        Presence::Absent
                    ) {
                        continue;
                    }
                    let Some(field) = before.labels.get(label) else {
                        // A symbolic open tail cannot establish a finite tag
                        // partition; split_match already leaves it deferred.
                        if matches!(before.rest, Rest::Closed) {
                            continue;
                        }
                        return Ok(Formula::True);
                    };
                    let allowed = self.solve.table.presence_of(&field.presence).formula();
                    let payload =
                        self.case_condition(&field.ty, &selected.ty, remaining, depth + 1)?;
                    condition = condition.or(allowed.and(payload));
                }
                Ok(condition)
            }
            (Ty::Struct(_), _) | (Ty::Sum(_), _) | (_, Ty::Struct(_)) | (_, Ty::Sum(_)) => {
                Ok(Formula::False)
            }
            // The partitioner only refines structural constructors. Any other
            // unchanged payload contributes no additional presence condition.
            _ => Ok(Formula::True),
        }
    }

    pub(super) fn push_case(&mut self, input: &Arc<Ty>) -> Result<bool, Error> {
        let (condition, reason) = self
            .case_guards
            .get(&(Arc::as_ptr(input) as usize))
            .map(|guard| (guard.condition.clone(), guard.reason))
            .unwrap_or((Formula::True, None));
        self.push_condition(condition, reason)
    }

    pub(super) fn push_condition(
        &mut self,
        condition: Formula,
        reason: Option<ReasonId>,
    ) -> Result<bool, Error> {
        let combined = match &self.solve.guard {
            Some(outer) => outer.clone().and(condition.clone()),
            None => condition.clone(),
        };
        let effective = self.solve.table.resolved(&combined);
        // Normalization can add many small row obligations. Replaying a new
        // SAT solver for every prefix on every branch makes composition
        // quadratic in the growing store. Its incremental solver handles the
        // ordinary consistent case; prefix recovery is needed only after a
        // contradiction has already been reported.
        let known = if self.solve.table.store_satisfiable() {
            self.solve.table.known()
        } else {
            self.solve
                .table
                .consistent_known(self.solve.generated_end, self.solve.generated_end)
        };
        if !crate::inference::sat::satisfiable(&known.and(effective.clone())) {
            return Ok(false);
        }
        self.case_scopes.push(CaseScope {
            guard: self.solve.guard.clone(),
            refinement: self.solve.active_refinement,
            reasons: self.solve.guard_reasons.clone(),
        });
        if condition.is_true() {
            return Ok(true);
        }
        let report = self.solve.refinements.len();
        self.solve.refinements.push(Refinement {
            definition: self.solve.definition,
            match_at: self.at,
            arm_at: self.at,
            raw: condition,
            effective: effective.clone(),
            reachable: true,
            fields: Vec::new(),
            facts: Vec::new(),
            obligations: Vec::new(),
        });
        self.solve.guard = (!effective.is_true()).then_some(effective);
        self.solve.active_refinement = Some(report);
        if let Some(reason) = reason {
            self.solve.guard_reasons.push(reason);
        }
        Ok(true)
    }

    pub(super) fn pop_case(&mut self) {
        let previous = self
            .case_scopes
            .pop()
            .expect("every entered structural case is restored");
        self.solve.guard = previous.guard;
        self.solve.active_refinement = previous.refinement;
        self.solve.guard_reasons = previous.reasons;
    }
}
