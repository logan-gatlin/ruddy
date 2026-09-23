//! Join structural branches with ordinary payloads and correlated row presence.

use super::*;
use crate::types::Atom;

#[derive(Clone)]
struct Contributor {
    branch: usize,
    ty: Arc<Ty>,
}

struct Results {
    admitted: Vec<Formula>,
    selected: Option<Vec<Formula>>,
    producer_owned: bool,
    owned: bool,
    remaining: usize,
}

enum Work {
    Type(Vec<Contributor>, usize),
    Row {
        sum: bool,
        fields: Vec<(String, Presence)>,
        rest: Rest,
    },
}

impl Structural<'_, '_> {
    pub(super) fn join_results(
        &mut self,
        alternatives: Vec<(Arc<Ty>, Arc<Ty>)>,
    ) -> Result<Arc<Ty>, Error> {
        if alternatives.is_empty() {
            // An enclosing premise can make every finite case unreachable.
            // Its result contributes no ordinary constraint to a live branch.
            return Ok(Arc::new(Ty::Undecided));
        }
        if alternatives.len() == 1 {
            let (input, result) = alternatives.into_iter().next().unwrap();
            // A concrete structural case can settle presences already carried
            // by an ordinary captured result, including a returned function's
            // conditional effect. Specialize a copy under the selected case;
            // the captured result itself can also serve another case and must
            // remain generic there.
            let condition = self
                .case_guards
                .get(&(Arc::as_ptr(&input) as usize))
                .map(|case| case.condition.clone())
                .unwrap_or(Formula::True);
            return Ok(self.solve.table.specialize_under(&result, &condition));
        }
        let mut results = Results {
            admitted: alternatives
                .iter()
                .map(|(input, _)| {
                    self.case_guards
                        .get(&(Arc::as_ptr(input) as usize))
                        .map(|case| case.condition.clone())
                        .unwrap_or(Formula::True)
                })
                .collect(),
            selected: None,
            producer_owned: alternatives.iter().any(|(input, _)| {
                self.case_guards
                    .get(&(Arc::as_ptr(input) as usize))
                    .is_some_and(|case| case.producer_owned)
            }),
            owned: false,
            remaining: 32_768,
        };
        let mut work = vec![Work::Type(
            alternatives
                .into_iter()
                .enumerate()
                .map(|(branch, (_, ty))| Contributor { branch, ty })
                .collect(),
            0,
        )];
        let mut values = Vec::new();
        while let Some(next) = work.pop() {
            results.remaining = results
                .remaining
                .checked_sub(1)
                .ok_or_else(|| self.failure(Failure::WorkLimit))?;
            match next {
                Work::Type(contributors, depth) => {
                    let first = &contributors[0].ty;
                    if contributors
                        .iter()
                        .all(|part| self.solve.table.alike(first, &part.ty))
                    {
                        values.push(first.clone());
                        continue;
                    }
                    if depth > crate::contracts::MAX_DEPTH {
                        return Err(self.failure(Failure::DepthLimit));
                    }
                    let contributors: Vec<_> = contributors
                        .into_iter()
                        .map(|part| Contributor {
                            branch: part.branch,
                            ty: self.resolve_type(&part.ty),
                        })
                        .collect();
                    let sum = match &*contributors[0].ty {
                        Ty::Struct(_) => Some(false),
                        Ty::Sum(_) => Some(true),
                        _ => None,
                    };
                    let rows: Option<Vec<_>> = sum.and_then(|sum| {
                        contributors
                            .iter()
                            .map(|part| match (&*part.ty, sum) {
                                (Ty::Struct(row), false) | (Ty::Sum(row), true) => {
                                    Some((part.branch, self.solve.table.canon(row)))
                                }
                                _ => None,
                            })
                            .collect()
                    });
                    let Some(rows) = rows else {
                        // Scalar, callable, mutable and nominal payloads use
                        // the ordinary match-family rule. This still rejects
                        // incompatible concrete types, while conditional
                        // effects on otherwise compatible arrows retain the
                        // input case which enables them.
                        let types: Vec<_> =
                            contributors.iter().map(|part| part.ty.clone()).collect();
                        let family = self.solve.family_type(&types);
                        for part in &contributors {
                            let guard = results.admitted[part.branch].clone();
                            if self.push_condition(guard, self.solve.constraint_reason)? {
                                self.solve.unify(self.at, &family, &part.ty);
                                self.pop_case();
                            }
                        }
                        values.push(self.solve.table.resolve(&family));
                        continue;
                    };
                    let sum = sum.unwrap();
                    let mut names = IndexSet::new();
                    for (_, row) in &rows {
                        names.extend(row.labels.keys().cloned());
                    }
                    let rest = rows[0].1.rest.clone();
                    for (_, row) in rows.iter().skip(1) {
                        self.solve.unify(
                            self.at,
                            &Arc::new(Ty::Struct(Row::of(rest.clone()))),
                            &Arc::new(Ty::Struct(Row::of(row.rest.clone()))),
                        );
                    }
                    let mut fields = Vec::new();
                    let mut children = Vec::new();
                    for name in names {
                        let presences: Vec<_> = rows
                            .iter()
                            .map(|(branch, row)| {
                                (
                                    *branch,
                                    row.labels.get(&name).map_or(Presence::Absent, |field| {
                                        self.solve.table.presence_of(&field.presence)
                                    }),
                                )
                            })
                            .collect();
                        let presence = self.result_presence(&presences, sum, &mut results)?;
                        let payloads: Vec<_> = rows
                            .iter()
                            .filter_map(|(branch, row)| {
                                row.labels.get(&name).and_then(|field| {
                                    (!matches!(
                                        self.solve.table.presence_of(&field.presence),
                                        Presence::Absent
                                    ))
                                    .then(|| Contributor {
                                        branch: *branch,
                                        ty: field.ty.clone(),
                                    })
                                })
                            })
                            .collect();
                        fields.push((name, presence));
                        children.push(if payloads.is_empty() {
                            vec![Contributor {
                                branch: rows[0].0,
                                ty: Arc::new(Ty::Undecided),
                            }]
                        } else {
                            payloads
                        });
                    }
                    work.push(Work::Row { sum, fields, rest });
                    work.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|children| Work::Type(children, depth + 1)),
                    );
                }
                Work::Row { sum, fields, rest } => {
                    let payloads = values.split_off(values.len() - fields.len());
                    let row = Row {
                        labels: fields
                            .into_iter()
                            .zip(payloads)
                            .map(|((name, presence), ty)| (name, RowField { presence, ty }))
                            .collect(),
                        rest,
                    };
                    values.push(Arc::new(if sum { Ty::Sum(row) } else { Ty::Struct(row) }));
                }
            }
        }
        let result = values.pop().expect("one merged structural result");
        self.solve.table.note_lacks(&result);
        if results.owned
            || !crate::inference::collect_owned_existentials(
                &result,
                &self.solve.table.abstract_existentials,
            )
            .is_empty()
        {
            let package = Arc::new(Ty::Package(result));
            // The package is produced here, once. Future scheme instantiation
            // owns the alpha-renaming of identities and their guarantees.
            self.solve
                .table
                .register_package_guarantees(&package, Formula::True);
            Ok(package)
        } else {
            Ok(result)
        }
    }

    fn result_presence(
        &mut self,
        presences: &[(usize, Presence)],
        sum: bool,
        results: &mut Results,
    ) -> Result<Presence, Error> {
        let first = &presences[0].1;
        if presences.iter().all(|(_, presence)| presence == first) {
            return Ok(first.clone());
        }
        if !sum && results.selected.is_none() {
            let mut disjoint = true;
            'pairs: for (index, left) in results.admitted.iter().enumerate() {
                for right in &results.admitted[index + 1..] {
                    if crate::inference::sat::satisfiable(&left.clone().and(right.clone())) {
                        disjoint = false;
                        break 'pairs;
                    }
                }
            }
            results.selected = Some(if disjoint && !results.producer_owned {
                // A record's input presence already selects its runtime shape.
                // Keep that relationship caller-owned rather than inventing a
                // producer choice that would prevent inferring input demands.
                results.admitted.clone()
            } else {
                let mut remaining = Formula::True;
                let mut selected = Vec::new();
                for (index, admitted) in results.admitted.iter().enumerate() {
                    let branch = if index + 1 == results.admitted.len() {
                        remaining.clone()
                    } else {
                        let choice = self
                            .solve
                            .table
                            .fresh_presence_for(super::super::super::Subject::MatchResult);
                        let Presence::Var(var) = choice else {
                            unreachable!()
                        };
                        self.solve.table.existential_witnesses.insert(var);
                        self.solve.table.abstract_existentials.insert(var);
                        results.owned = true;
                        let branch = remaining.clone().and(choice.formula());
                        remaining = remaining.and(choice.formula().not());
                        branch
                    };
                    self.result_requirement(branch.clone().not().or(admitted.clone()));
                    selected.push(branch);
                }
                selected
            });
        }
        let guards = if sum {
            &results.admitted
        } else {
            results.selected.as_ref().unwrap()
        };
        let formula = Formula::any(
            presences
                .iter()
                .map(|(branch, presence)| guards[*branch].clone().and(presence.formula())),
        );
        let formula = self.solve.table.resolved(&formula);
        match &formula {
            Formula::True => return Ok(Presence::Present),
            Formula::False => return Ok(Presence::Absent),
            Formula::Atom(Atom::Var(var)) => return Ok(Presence::Var(*var)),
            Formula::Atom(Atom::Bound(index)) => return Ok(Presence::Bound(*index)),
            _ => {}
        }
        let presence = self
            .solve
            .table
            .fresh_presence_for(super::super::super::Subject::MatchResult);
        let mut atoms = Vec::new();
        formula.atoms(&mut atoms);
        if atoms.iter().any(|atom| {
            matches!(atom, Atom::Var(var) if self.solve.table.abstract_existentials.contains(var))
        }) && let Presence::Var(var) = presence
        {
            self.solve.table.existential_witnesses.insert(var);
            self.solve.table.abstract_existentials.insert(var);
            results.owned = true;
        }
        self.result_requirement(presence.formula().iff(formula));
        Ok(presence)
    }

    fn result_requirement(&mut self, requirement: Formula) {
        if requirement.is_true() {
            return;
        }
        let slot = self.solve.table.store.batches.len();
        self.solve.table.require_because(
            self.at,
            Origin::Instance(Named {
                labels: Vec::new(),
                shape: None,
            }),
            requirement,
            self.solve.constraint_reason,
        );
        let batch = self.solve.table.store.batches[slot].clone();
        self.solve.table.store.batches[slot] = self.solve.guarded_batch(batch);
    }
}
