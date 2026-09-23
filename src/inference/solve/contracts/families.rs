//! Structural patterns share one constructor family at each payload position.
//!
//! A catchall covers other shapes in that family; it does not turn a record
//! matcher into a runtime test for arbitrary scalar or function values.

use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
enum Family {
    #[default]
    Any,
    Record(BTreeMap<String, Family>),
    Sum(BTreeMap<String, Family>),
}

impl Family {
    fn merge(&mut self, pattern: &Pattern, budget: &mut usize) -> Result<(), Failure> {
        *budget = budget.checked_sub(1).ok_or(Failure::WorkLimit)?;
        if matches!(pattern, Pattern::Any) {
            return Ok(());
        }
        match pattern {
            Pattern::Any => unreachable!(),
            Pattern::Record { fields, .. } => {
                if matches!(self, Self::Any) {
                    *self = Self::Record(BTreeMap::new());
                }
                let Self::Record(children) = self else {
                    return Err(Failure::UncoveredMatch);
                };
                for (label, child) in fields.iter() {
                    children
                        .entry(label.clone())
                        .or_default()
                        .merge(child, budget)?;
                }
            }
            Pattern::Tag { label, payload } => {
                if matches!(self, Self::Any) {
                    *self = Self::Sum(BTreeMap::new());
                }
                let Self::Sum(children) = self else {
                    return Err(Failure::UncoveredMatch);
                };
                children
                    .entry(label.clone())
                    .or_default()
                    .merge(payload, budget)?;
            }
        }
        Ok(())
    }
}

impl Structural<'_, '_> {
    pub(super) fn match_family(
        &mut self,
        input: &Arc<Ty>,
        patterns: &[Pattern],
    ) -> Result<bool, Error> {
        let mut family = Family::Any;
        let mut budget = 32768usize;
        for pattern in patterns {
            family
                .merge(pattern, &mut budget)
                .map_err(|error| self.failure(error))?;
        }
        self.check_family(input, &family, &mut budget)
    }

    fn check_family(
        &mut self,
        input: &Arc<Ty>,
        family: &Family,
        budget: &mut usize,
    ) -> Result<bool, Error> {
        *budget = budget
            .checked_sub(1)
            .ok_or_else(|| self.failure(Failure::WorkLimit))?;
        if matches!(family, Family::Any) {
            return Ok(true);
        }
        let input = self.resolve_type(input);
        match (&*input, family) {
            (Ty::Struct(row), Family::Record(children)) | (Ty::Sum(row), Family::Sum(children)) => {
                let row = self.solve.table.canon(row);
                for (label, child) in children {
                    if matches!(child, Family::Any) {
                        continue;
                    }
                    let Some(field) = row.labels.get(label) else {
                        // A flexible sum tail permits later widening; it
                        // contributes no runtime tag of this value. The
                        // partitioner closes that tail before selecting arms.
                        if matches!((&*input, &row.rest), (Ty::Sum(_), Rest::Var(_))) {
                            continue;
                        }
                        // An absent field has no payload to check. A rigid or
                        // flexible unknown tail may contain that field later.
                        if !matches!(row.rest, Rest::Closed | Rest::Undecided) {
                            return Ok(false);
                        }
                        continue;
                    };
                    if matches!(
                        self.solve.table.presence_of(&field.presence),
                        Presence::Absent
                    ) {
                        continue;
                    }
                    if !self.check_family(&field.ty, child, budget)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Ty::Var(_) | Ty::Rigid { .. } | Ty::Bound(_) | Ty::Contract { .. }, _) => Ok(false),
            (Ty::Undecided, _) => Ok(true),
            _ => Err(self.failure(Failure::UncoveredMatch)),
        }
    }
}
