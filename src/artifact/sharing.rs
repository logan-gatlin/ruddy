//! Preserve semantic type DAGs without changing occurrence-based package owners.

use std::collections::{HashMap, HashSet};

use crate::types::{Presence, Rest, Row, Ty};

/// Upper bound on the portable constructors emitted for a single type root.
/// Shared package-free definitions contribute once; package-containing paths
/// retain their occurrence identity and therefore contribute for each use.
pub(super) const LIMIT: usize = 1_000_000;

pub(super) struct Plan {
    pub shared: HashSet<usize>,
    pub cost: usize,
}

fn children(value: &Ty) -> Vec<&Ty> {
    fn row<'a>(mut row: &'a Row, children: &mut Vec<&'a Ty>) {
        loop {
            children.extend(row.labels.values().filter_map(|field| {
                (!matches!(field.presence, Presence::Absent)).then_some(field.ty.as_ref())
            }));
            match &row.rest {
                Rest::More(more) => row = more,
                _ => break,
            }
        }
    }
    let mut out = Vec::new();
    match value {
        Ty::Contract { fallback, contract } => {
            out.push(fallback.as_ref());
            out.extend(contract.type_operands().map(|ty| ty.as_ref()));
        }
        Ty::Named { args: types, .. } => {
            out.extend(types.iter().map(|ty| ty.as_ref()));
        }
        Ty::Arrow(from, to, effects) => {
            out.extend([from.as_ref(), to.as_ref()]);
            row(effects, &mut out);
        }
        Ty::Struct(fields) | Ty::Sum(fields) => row(fields, &mut out),
        Ty::Mut(region, element) => out.extend([region.as_ref(), element.as_ref()]),
        Ty::Package(inner)
        | Ty::Hidden { body: inner, .. }
        | Ty::Array(inner)
        | Ty::Mirror(inner)
        | Ty::TypeInfo(inner) => out.push(inner),
        _ => {}
    }
    out
}

impl Plan {
    pub fn new(root: &Ty) -> Self {
        let key = |ty: &Ty| ty as *const Ty as usize;
        let mut counts = HashMap::<usize, usize>::new();
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        let mut work = vec![(root, false)];
        while let Some((ty, finish)) = work.pop() {
            if finish {
                order.push(ty);
            } else {
                *counts.entry(key(ty)).or_default() += 1;
                if seen.insert(key(ty)) {
                    work.push((ty, true));
                    work.extend(children(ty).into_iter().rev().map(|ty| (ty, false)));
                }
            }
        }
        let mut packages = HashMap::new();
        let mut shared = HashSet::new();
        let mut costs = HashMap::new();
        let mut definitions = 0usize;
        for ty in order {
            let children = children(ty);
            let packaged =
                matches!(ty, Ty::Package(_)) || children.iter().any(|ty| packages[&key(ty)]);
            packages.insert(key(ty), packaged);
            let cost = children.iter().fold(1usize, |cost, ty| {
                cost.saturating_add(costs[&key(ty)]).min(LIMIT + 1)
            });
            // Leaf sharing adds wire metadata without saving any traversal.
            if !packaged && !children.is_empty() && counts[&key(ty)] > 1 {
                shared.insert(key(ty));
                definitions = definitions.saturating_add(cost).min(LIMIT + 1);
                costs.insert(key(ty), 1);
            } else {
                costs.insert(key(ty), cost);
            }
        }
        Self {
            shared,
            cost: costs[&key(root)].saturating_add(definitions).min(LIMIT + 1),
        }
    }
}

/// Previously decoded shared nodes were checked at their own definition.
/// Stopping at them keeps validation linear across nested shared definitions.
pub(super) fn contains_unshared_package(root: &super::Type) -> bool {
    use super::{Rest, Type};
    let mut pending = vec![root];
    let mut rows = Vec::new();
    while !pending.is_empty() || !rows.is_empty() {
        if let Some(row) = rows.pop() {
            let row: &super::Row = row;
            pending.extend(row.labels.iter().map(|(_, field)| &field.ty));
            if let Rest::More(more) = &row.rest {
                rows.push(more);
            }
            continue;
        }
        match pending.pop().expect("pending type") {
            Type::Package(_) => return true,
            Type::Shared(_) => {}
            Type::Contract { fallback, contract } => {
                pending.push(fallback);
                pending.extend(contract.type_operands());
            }
            Type::Named { args: types, .. } => pending.extend(types),
            Type::Arrow(from, to, row) => {
                pending.extend([from.as_ref(), to.as_ref()]);
                rows.push(row);
            }
            Type::Struct(row) | Type::Sum(row) => rows.push(row),
            Type::Mut(region, inner) => pending.extend([region.as_ref(), inner.as_ref()]),
            Type::Array(inner)
            | Type::Mirror(inner)
            | Type::TypeInfo(inner)
            | Type::Hidden { body: inner, .. } => pending.push(inner),
            _ => {}
        }
    }
    false
}

/// Effect aliases still have an owned written argument tree. Refuse a compact
/// interface whose expansion would exceed that tree's budget before allocating
/// it, rather than erasing a contract or losing its argument obligations.
pub(super) fn syntax_cost(root: &super::Type) -> usize {
    use super::{Rest, Type};
    fn children(value: &Type) -> Vec<&Type> {
        fn row<'a>(mut row: &'a super::Row, out: &mut Vec<&'a Type>) {
            loop {
                out.extend(row.labels.iter().filter_map(|(_, field)| {
                    (!matches!(field.presence, super::Presence::Absent)).then_some(&field.ty)
                }));
                match &row.rest {
                    Rest::More(more) => row = more,
                    _ => break,
                }
            }
        }
        let mut out = Vec::new();
        match value {
            Type::Shared(inner) => out.push(inner.as_ref()),
            Type::Contract { contract, .. } => out.extend(contract.type_operands()),
            Type::Named { args: types, .. } => out.extend(types),
            Type::Arrow(from, to, effects) => {
                out.extend([from.as_ref(), to.as_ref()]);
                row(effects, &mut out);
            }
            Type::Struct(fields) | Type::Sum(fields) => row(fields, &mut out),
            Type::Mut(region, inner) => out.extend([region.as_ref(), inner.as_ref()]),
            Type::Package(inner)
            | Type::Array(inner)
            | Type::Mirror(inner)
            | Type::TypeInfo(inner)
            | Type::Hidden { body: inner, .. } => out.push(inner),
            _ => {}
        }
        out
    }
    let mut costs = HashMap::<*const Type, usize>::new();
    let mut work = vec![(root, false)];
    while let Some((ty, finish)) = work.pop() {
        if costs.contains_key(&(ty as *const Type)) {
            continue;
        }
        if finish {
            let cost = children(ty).into_iter().fold(1usize, |cost, child| {
                cost.saturating_add(costs[&(child as *const Type)])
                    .min(LIMIT + 1)
            });
            costs.insert(ty, cost);
        } else {
            work.push((ty, true));
            work.extend(children(ty).into_iter().rev().map(|child| (child, false)));
        }
    }
    costs[&(root as *const Type)]
}
