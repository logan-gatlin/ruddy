//! Choose concrete instantiations for reflection's otherwise unconstrained
//! body-only variables. These variables are not caller-owned parameters: no
//! public or locally generalized contract mentions them. Empty rows and the
//! empty sum give them a consistent, uninhabited instantiation without changing
//! unification or the contracts callers see.

use std::{collections::BTreeSet, sync::Arc};

use crate::{
    ir::TermKind,
    reification,
    types::{Presence, Rest, Row, Ty},
};

pub(super) fn instantiate(
    semantics: &mut super::Semantics,
    requirements: &reification::Analysis,
) -> bool {
    let solved = requirements.callables.graph.solve();
    let mut changed = false;
    for (symbol, declaration) in &mut semantics.typed {
        let mut retained = reification::parameters(&declaration.value.ty);
        let mut local_regions = BTreeSet::new();
        let mut work = vec![(&declaration.value, BTreeSet::new())];
        while let Some((term, mut enclosing)) = work.pop() {
            if matches!(term.kind, TermKind::Fn { .. }) {
                enclosing.extend(reification::parameters(&term.ty));
            }
            // An allocation's region that cannot leave its enclosing function
            // is local, even when a later local closure captures that region.
            if matches!(
                term.kind,
                TermKind::Unary {
                    op: crate::ir::UnaryOp::Allocate,
                    ..
                }
            ) && let Ty::Mut(region, _) = &*term.ty
                && let Ty::Bound(index) = &**region
                && !enclosing.contains(index)
            {
                local_regions.insert(*index);
            }
            if let TermKind::Let { value, .. } = &term.kind {
                retained.extend(reification::parameters(&value.ty));
            }
            let mut nested = Vec::new();
            reification::children(term, &mut nested);
            work.extend(nested.into_iter().map(|term| (term, enclosing.clone())));
        }
        retained = retained.difference(&local_regions).copied().collect();
        let flow = requirements.callables.occurrences[&declaration.value.at];
        let needed: BTreeSet<_> = solved[flow.evaluation as usize]
            .iter()
            .map(|need| need.parameter)
            .chain(requirements.bindings[symbol].parameters.iter().copied())
            .filter(|parameter| *parameter < reification::SCOPED && !retained.contains(parameter))
            .collect();
        if needed.is_empty() {
            continue;
        }
        changed = true;
        let never = Arc::new(Ty::Sum(Row::closed()));
        let mut work = vec![&mut declaration.value];
        while let Some(term) = work.pop() {
            if !reification::parameters(&term.ty).is_disjoint(&needed) {
                term.ty = super::substitute_type_with(
                    &term.ty,
                    Presence::Bound,
                    |index| {
                        if needed.contains(&index) {
                            never.clone()
                        } else {
                            Arc::new(Ty::Bound(index))
                        }
                    },
                    |index| {
                        Row::of(if needed.contains(&index) {
                            Rest::Closed
                        } else {
                            Rest::Bound(index)
                        })
                    },
                );
            }
            match &mut term.kind {
                TermKind::Unary { value, .. } | TermKind::Raise(value) => work.push(value),
                TermKind::Binary { left, right, .. } => work.extend([&mut **left, &mut **right]),
                TermKind::Apply { func, arg } => work.extend([&mut **func, &mut **arg]),
                TermKind::Fn { body, .. } => work.push(body),
                TermKind::Let { value, body, .. } => work.extend([&mut **value, &mut **body]),
                TermKind::Struct { fields, spread } => {
                    work.extend(fields.values_mut().map(|field| &mut field.value));
                    if let Some(spread) = spread {
                        work.push(&mut spread.value);
                    }
                }
                TermKind::Array(items) => work.extend(items.iter_mut().map(|item| &mut item.value)),
                TermKind::Project { base, .. } => work.push(base),
                TermKind::Tag {
                    payload: Some(payload),
                    ..
                } => work.push(payload),
                TermKind::Match { scrutinee, arms } => {
                    work.push(scrutinee);
                    work.extend(arms.iter_mut().map(|(_, body)| body));
                }
                TermKind::Handle { body, handler } => {
                    work.push(body);
                    work.extend(handler.arms.iter_mut().map(|arm| &mut arm.body));
                    if let Some(ret) = &mut handler.ret {
                        work.push(&mut ret.body);
                    }
                }
                _ => {}
            }
        }
    }
    changed
}
