//! Specialize checked runtime layouts without adding semantic type equations.
//!
//! Structural application checks the contract body. Its fallback only records
//! how arguments and results are passed; matching its variables here must never
//! bind a solver variable or establish an input/output promise.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::types::{Presence, Rest, Row, Ty};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Variable(u32),
    Bound(u32),
}

fn type_key(ty: &Ty) -> Option<Key> {
    match ty {
        Ty::Var(index) => Some(Key::Variable(*index)),
        Ty::Bound(index) => Some(Key::Bound(*index)),
        _ => None,
    }
}
fn row_key(rest: &Rest) -> Option<Key> {
    match rest {
        Rest::Var(index) => Some(Key::Variable(*index)),
        Rest::Bound(index) => Some(Key::Bound(*index)),
        _ => None,
    }
}
fn presence_key(presence: &Presence) -> Option<Key> {
    match presence {
        Presence::Var(index) => Some(Key::Variable(*index)),
        Presence::Bound(index) => Some(Key::Bound(*index)),
        _ => None,
    }
}

/// The result layout after one argument, with actual callback/row layouts
/// substituted into the checked arrow's result. All resolver callbacks are
/// read-only views of the solver; the returned graph owns the substitutions.
pub(crate) fn applied(
    function: &Arc<Ty>,
    argument: &Arc<Ty>,
    resolve: impl Fn(&Arc<Ty>) -> Arc<Ty>,
    row: impl Fn(&Row) -> Row,
    presence: impl Fn(&Presence) -> Presence,
) -> Arc<Ty> {
    let mut function = resolve(function);
    while let Ty::Contract { fallback, .. } = &*function {
        function = resolve(fallback);
    }
    let Ty::Arrow(parameter, result, _) = &*function else {
        return Arc::new(Ty::Undecided);
    };
    enum Match {
        Type(Arc<Ty>, Arc<Ty>),
        Row(Row, Row),
        LeaveAliases(Option<crate::symbol::Symbol>, Option<crate::symbol::Symbol>),
    }
    let mut types = HashMap::<Key, Arc<Ty>>::new();
    let mut rows = HashMap::<Key, Row>::new();
    let mut presences = HashMap::<Key, Presence>::new();
    let mut pending = vec![Match::Type(parameter.clone(), argument.clone())];
    let mut seen = HashMap::new();
    let mut active_aliases = HashSet::new();
    while let Some(next) = pending.pop() {
        match next {
            Match::Type(from, to) => {
                let key = (Arc::as_ptr(&from) as usize, Arc::as_ptr(&to) as usize);
                if seen.insert(key, (from.clone(), to.clone())).is_some() {
                    continue;
                }
                // Named heads are left finite: matching their explicit type
                // arguments is sufficient when both use the same declaration.
                if let (
                    Ty::Named {
                        symbol: a,
                        args: xs,
                        ..
                    },
                    Ty::Named {
                        symbol: b,
                        args: ys,
                        ..
                    },
                ) = (&*from, &*to)
                    && a == b
                {
                    pending.extend(
                        xs.iter()
                            .cloned()
                            .zip(ys.iter().cloned())
                            .map(|(a, b)| Match::Type(a, b)),
                    );
                    continue;
                }
                let name = |ty: &Ty| match ty {
                    Ty::Named { symbol, .. } => Some(*symbol),
                    _ => None,
                };
                let aliases = (name(&from), name(&to));
                if aliases != (None, None) {
                    if !active_aliases.insert(aliases) {
                        continue;
                    }
                    pending.push(Match::LeaveAliases(aliases.0, aliases.1));
                }
                let from = resolve(&from);
                let to = resolve(&to);
                if let Some(key) = type_key(&from) {
                    types.entry(key).or_insert(to);
                    continue;
                }
                match (&*from, &*to) {
                    (Ty::Contract { fallback, .. }, _) => {
                        pending.push(Match::Type(fallback.clone(), to))
                    }
                    (_, Ty::Contract { fallback, .. }) => {
                        pending.push(Match::Type(from, fallback.clone()))
                    }
                    (Ty::Arrow(a, b, e), Ty::Arrow(x, y, f)) => {
                        pending.extend([
                            Match::Row(e.clone(), f.clone()),
                            Match::Type(b.clone(), y.clone()),
                            Match::Type(a.clone(), x.clone()),
                        ]);
                    }
                    (Ty::Struct(a), Ty::Struct(b)) | (Ty::Sum(a), Ty::Sum(b)) => {
                        pending.push(Match::Row(a.clone(), b.clone()))
                    }
                    (Ty::Array(a), Ty::Array(b))
                    | (Ty::Mirror(a), Ty::Mirror(b))
                    | (Ty::TypeInfo(a), Ty::TypeInfo(b))
                    | (Ty::Package(a), Ty::Package(b)) => {
                        pending.push(Match::Type(a.clone(), b.clone()))
                    }
                    (Ty::Mut(a, b), Ty::Mut(x, y)) => pending.extend([
                        Match::Type(a.clone(), x.clone()),
                        Match::Type(b.clone(), y.clone()),
                    ]),
                    _ => {}
                }
            }
            Match::LeaveAliases(a, b) => {
                active_aliases.remove(&(a, b));
            }
            Match::Row(from, to) => {
                let from = row(&from);
                let mut to = row(&to);
                for (name, field) in &from.labels {
                    if let Some(actual) = to.labels.shift_remove(name) {
                        let formal_presence = presence(&field.presence);
                        let actual_presence = presence(&actual.presence);
                        if let Some(key) = presence_key(&formal_presence) {
                            presences.entry(key).or_insert(actual_presence.clone());
                        }
                        if !matches!(formal_presence, Presence::Absent)
                            && !matches!(actual_presence, Presence::Absent)
                        {
                            pending.push(Match::Type(field.ty.clone(), actual.ty));
                        }
                    }
                }
                if let Some(key) = row_key(&from.rest) {
                    rows.entry(key).or_insert(to);
                }
            }
        }
    }

    enum Build {
        Type(Arc<Ty>),
        Remember(Arc<Ty>),
        Row(Row),
        FinishRow(Row),
        Arrow,
        Contract(Arc<crate::contracts::Contract>),
        Wrapper(Arc<Ty>),
        Struct(bool),
        Mut,
        Named(Arc<Ty>, usize),
    }
    let mut pending = vec![Build::Type(result.clone())];
    let mut built = Vec::<Arc<Ty>>::new();
    let mut built_rows = Vec::<Row>::new();
    let mut memo = HashMap::<usize, (Arc<Ty>, Arc<Ty>)>::new();
    while let Some(next) = pending.pop() {
        match next {
            Build::Type(original) => {
                let key = Arc::as_ptr(&original) as usize;
                if let Some((_, value)) = memo.get(&key) {
                    built.push(value.clone());
                    continue;
                }
                pending.push(Build::Remember(original.clone()));
                // Preserve alias heads while rebuilding explicit arguments.
                // Unfolding recursive result layouts is neither needed nor finite.
                let ty = if matches!(&*original, Ty::Named { .. }) {
                    original
                } else {
                    resolve(&original)
                };
                if let Some(value) = type_key(&ty).and_then(|key| types.get(&key)) {
                    built.push(value.clone());
                    continue;
                }
                match &*ty {
                    Ty::Arrow(from, to, effects) => {
                        pending.extend([
                            Build::Arrow,
                            Build::Row(effects.clone()),
                            Build::Type(to.clone()),
                            Build::Type(from.clone()),
                        ]);
                    }
                    Ty::Contract { fallback, contract } => {
                        pending.extend([
                            Build::Contract(contract.clone()),
                            Build::Type(fallback.clone()),
                        ]);
                    }
                    Ty::Struct(row) | Ty::Sum(row) => pending.extend([
                        Build::Struct(matches!(&*ty, Ty::Sum(_))),
                        Build::Row(row.clone()),
                    ]),
                    Ty::Array(inner)
                    | Ty::Mirror(inner)
                    | Ty::TypeInfo(inner)
                    | Ty::Package(inner)
                    | Ty::Hidden { body: inner, .. } => {
                        pending.extend([Build::Wrapper(ty.clone()), Build::Type(inner.clone())])
                    }
                    Ty::Mut(region, inner) => pending.extend([
                        Build::Mut,
                        Build::Type(inner.clone()),
                        Build::Type(region.clone()),
                    ]),
                    Ty::Named { args, .. } => {
                        pending.push(Build::Named(ty.clone(), args.len()));
                        pending.extend(args.iter().rev().cloned().map(Build::Type));
                    }
                    Ty::Presence(value) => {
                        let value = presence(value);
                        let value = presence_key(&value)
                            .and_then(|key| presences.get(&key))
                            .cloned()
                            .unwrap_or(value);
                        built.push(Arc::new(Ty::Presence(value)));
                    }
                    _ => built.push(ty),
                }
            }
            Build::Remember(original) => {
                memo.insert(
                    Arc::as_ptr(&original) as usize,
                    (original, built.last().expect("rebuilt layout").clone()),
                );
            }
            Build::Row(original) => {
                let mut resolved = row(&original);
                for field in resolved.labels.values_mut() {
                    let value = presence(&field.presence);
                    field.presence = presence_key(&value)
                        .and_then(|key| presences.get(&key))
                        .cloned()
                        .unwrap_or(value);
                }
                let children: Vec<_> = resolved
                    .labels
                    .values()
                    .map(|field| field.ty.clone())
                    .collect();
                pending.push(Build::FinishRow(resolved));
                pending.extend(children.into_iter().rev().map(Build::Type));
            }
            Build::FinishRow(mut row) => {
                let children = built.split_off(built.len() - row.labels.len());
                for (field, ty) in row.labels.values_mut().zip(children) {
                    field.ty = ty;
                }
                if let Some(more) = row_key(&row.rest).and_then(|key| rows.get(&key)) {
                    row.rest = more.rest.clone();
                    for (name, field) in &more.labels {
                        row.labels
                            .entry(name.clone())
                            .or_insert_with(|| field.clone());
                    }
                }
                built_rows.push(row);
            }
            Build::Arrow => {
                let effects = built_rows.pop().expect("arrow row");
                let to = built.pop().expect("arrow result");
                let from = built.pop().expect("arrow argument");
                built.push(Arc::new(Ty::Arrow(from, to, effects)));
            }
            Build::Contract(contract) => {
                let fallback = built.pop().expect("nested layout");
                built.push(Arc::new(Ty::Contract { fallback, contract }));
            }
            Build::Struct(sum) => {
                let row = built_rows.pop().expect("value row");
                built.push(Arc::new(if sum { Ty::Sum(row) } else { Ty::Struct(row) }));
            }
            Build::Wrapper(original) => {
                let body = built.pop().expect("wrapped layout");
                built.push(Arc::new(match &*original {
                    Ty::Array(_) => Ty::Array(body),
                    Ty::Mirror(_) => Ty::Mirror(body),
                    Ty::TypeInfo(_) => Ty::TypeInfo(body),
                    Ty::Package(_) => Ty::Package(body),
                    Ty::Hidden { binder, name, .. } => Ty::Hidden {
                        binder: *binder,
                        name: name.clone(),
                        body,
                    },
                    _ => unreachable!(),
                }));
            }
            Build::Mut => {
                let inner = built.pop().expect("cell layout");
                let region = built.pop().expect("cell region");
                built.push(Arc::new(Ty::Mut(region, inner)));
            }
            Build::Named(original, count) => {
                let args = built.split_off(built.len() - count);
                let Ty::Named { symbol, name, .. } = &*original else {
                    unreachable!()
                };
                built.push(Arc::new(Ty::Named {
                    symbol: *symbol,
                    name: name.clone(),
                    args: args.into(),
                }));
            }
        }
    }
    built.pop().expect("applied result layout")
}
