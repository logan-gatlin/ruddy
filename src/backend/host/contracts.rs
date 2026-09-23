//! Ordinary native layouts become permissions only after universal annotation
//! checking against the exported value's exact structural contract.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::artifact::{DeclaredType, Formula, Parameter, Presence, Rest, Row, Scheme, Sense, Type};

pub(super) struct Witnesses {
    names: HashMap<String, String>,
    declarations: Vec<DeclaredType>,
}

impl Witnesses {
    pub(super) fn new(
        prefix: &str,
        namespace: &str,
        declarations: &HashMap<&str, &DeclaredType>,
    ) -> Self {
        // Ordinary aliases already name exactly the intended type. Reusing
        // them also keeps their payload identity aligned with captured calls.
        let mut sorted: Vec<_> = declarations
            .iter()
            .filter(|(_, declaration)| needed(&declaration.scheme.body, declarations))
            .map(|(name, _)| *name)
            .collect();
        sorted.sort_unstable();
        let names = sorted
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    (*name).to_owned(),
                    format!("{prefix}{namespace}WitnessType_{index}"),
                )
            })
            .collect::<HashMap<_, _>>();
        let declarations = sorted
            .into_iter()
            .map(|name| {
                let original = declarations[name];
                let mut declaration = original.clone();
                declaration.name = names[name].clone();
                declaration.exported = true;
                declaration.scheme = ordinary(&original.scheme, &names);
                declaration
            })
            .collect();
        Self {
            names,
            declarations,
        }
    }

    pub(super) fn declarations(&self) -> &[DeclaredType] {
        &self.declarations
    }

    pub(super) fn value(
        &self,
        name: String,
        scheme: &Scheme,
        declarations: &HashMap<&str, &DeclaredType>,
    ) -> DeclaredType {
        let mut params = parameters(scheme, declarations);
        let mut witness = ordinary(scheme, &self.names);
        let arity = arity(&scheme.body, declarations);
        let result = witness.count;
        if inferred_result(&mut witness.body, arity, result) {
            witness.count += 1;
            params.push(Parameter {
                sense: Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            });
        }
        DeclaredType {
            name,
            exported: true,
            params,
            scheme: witness,
            metadata: Default::default(),
        }
    }
}

/// Eta inference may solve ordinary type holes, but it must not narrow a native
/// optional field into a required one. Restore those universally offered
/// choices in a separate declaration, then check the original value directly.
pub(super) fn candidate(
    name: String,
    scheme: &Scheme,
    declarations: &HashMap<&str, &DeclaredType>,
) -> DeclaredType {
    let mut params = parameters(scheme, declarations);
    let mut scheme = scheme.clone();
    scheme.formula = Formula::True;
    scheme.callable = None;
    scheme.representations.clear();
    scheme.existentials.clear();
    let mut body = &mut scheme.body;
    while let Type::Shared(inner) = body {
        body = Arc::make_mut(inner);
    }
    if let Type::Named { name, args } = body
        && let Some(declaration) = declarations.get(name.as_str())
    {
        for (argument, parameter) in args.iter_mut().zip(&declaration.params) {
            if parameter.sense == Sense::Presence {
                *argument = Type::Presence(Presence::Bound(scheme.count));
                scheme.count += 1;
                params.push(parameter.clone());
            }
        }
    }
    scheme.body = erase(&scheme.body, &HashMap::new(), true);
    scheme.presences = 0;
    DeclaredType {
        name,
        exported: true,
        params,
        scheme,
        metadata: Default::default(),
    }
}

/// A native caller supplies every optional-field combination. A successful
/// source annotation may still infer a narrower presence scheme; that is a
/// useful language behavior, but cannot prove this boundary permission.
pub(super) fn universal(scheme: &Scheme, candidate: &DeclaredType) -> bool {
    let Type::Named { name, args } = scheme.body.unshared() else {
        return false;
    };
    if name != &candidate.name || args.len() != candidate.params.len() {
        return false;
    }
    let mut independent = HashSet::new();
    for (argument, parameter) in args.iter().zip(&candidate.params) {
        if parameter.sense != Sense::Presence {
            continue;
        }
        let Type::Presence(Presence::Bound(index)) = argument.unshared() else {
            return false;
        };
        if *index >= scheme.presences
            || scheme.existentials.contains(index)
            || !independent.insert(*index)
        {
            return false;
        }
    }
    enum Work<'a> {
        Visit(&'a Formula),
        Finish(&'a Formula),
    }
    let mut work = vec![Work::Visit(&scheme.formula)];
    let mut values = Vec::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Visit(formula) => match formula {
                Formula::True => values.push(Some(true)),
                Formula::False => values.push(Some(false)),
                Formula::Var(_) | Formula::Bound(_) => values.push(None),
                Formula::Not(inner) | Formula::Owned(_, inner) => {
                    work.push(Work::Finish(formula));
                    work.push(Work::Visit(inner));
                }
                Formula::And(a, b)
                | Formula::Or(a, b)
                | Formula::Iff(a, b)
                | Formula::Xor(a, b) => {
                    work.push(Work::Finish(formula));
                    work.push(Work::Visit(b));
                    work.push(Work::Visit(a));
                }
            },
            Work::Finish(formula) => {
                let b = values.pop().unwrap();
                let value = match formula {
                    Formula::Not(_) => b.map(|b| !b),
                    Formula::Owned(_, _) => b,
                    _ => {
                        let a = values.pop().unwrap();
                        match formula {
                            Formula::And(_, _) if a == Some(false) || b == Some(false) => {
                                Some(false)
                            }
                            Formula::Or(_, _) if a == Some(true) || b == Some(true) => Some(true),
                            Formula::Iff(left, right) if left == right => Some(true),
                            Formula::Xor(left, right) if left == right => Some(false),
                            Formula::And(_, _) => a.zip(b).map(|(a, b)| a && b),
                            Formula::Or(_, _) => a.zip(b).map(|(a, b)| a || b),
                            Formula::Iff(_, _) => a.zip(b).map(|(a, b)| a == b),
                            Formula::Xor(_, _) => a.zip(b).map(|(a, b)| a != b),
                            _ => unreachable!("formula constructor"),
                        }
                    }
                };
                values.push(value);
            }
        }
    }
    values.pop() == Some(Some(true))
}

/// Ask ordinary body inference for the fully applied result, rather than
/// promising every widening variable retained in the representation witness.
fn inferred_result(mut ty: &mut Type, mut arity: usize, result: u32) -> bool {
    while arity > 0 {
        match ty {
            Type::Shared(inner) => ty = Arc::make_mut(inner),
            Type::Arrow(_, to, _) if arity == 1 => {
                **to = Type::Bound(result);
                return true;
            }
            Type::Arrow(_, to, _) => {
                ty = to;
                arity -= 1;
            }
            _ => return false,
        }
    }
    false
}

pub(super) fn arity<'a>(mut ty: &'a Type, declarations: &HashMap<&str, &'a DeclaredType>) -> usize {
    let mut named = HashSet::new();
    loop {
        match ty.unshared() {
            Type::Contract { contract, .. } => {
                return contract.parameters.saturating_sub(contract.arguments.len());
            }
            Type::Named { name, .. } if named.insert(name) => {
                let Some(declaration) = declarations.get(name.as_str()) else {
                    return 0;
                };
                ty = &declaration.scheme.body;
            }
            _ => return 0,
        }
    }
}

pub(super) fn needed(root: &Type, declarations: &HashMap<&str, &DeclaredType>) -> bool {
    let mut pending = vec![root];
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    while let Some(ty) = pending.pop() {
        let ty = ty.unshared();
        if !seen.insert(ty as *const Type) {
            continue;
        }
        match ty {
            Type::Contract { .. } => return true,
            Type::Mirror(_) | Type::TypeInfo(_) => {}
            Type::Named { name, args } => {
                pending.extend(args);
                if names.insert(name)
                    && let Some(declaration) = declarations.get(name.as_str())
                {
                    pending.push(&declaration.scheme.body);
                }
            }
            _ => children(ty, &mut pending),
        }
    }
    false
}

fn children<'a>(ty: &'a Type, pending: &mut Vec<&'a Type>) {
    let mut row = None;
    match ty {
        Type::Arrow(from, to, effects) => {
            pending.extend([&**from, &**to]);
            row = Some(effects);
        }
        Type::Struct(fields) | Type::Sum(fields) => row = Some(fields),
        Type::Package(inner)
        | Type::Hidden { body: inner, .. }
        | Type::Array(inner)
        | Type::Mirror(inner)
        | Type::TypeInfo(inner) => pending.push(inner),
        Type::Mut(region, inner) => pending.extend([&**region, &**inner]),
        Type::Named { args, .. } => pending.extend(args),
        Type::Contract { fallback, .. } => pending.push(fallback),
        Type::Shared(inner) => pending.push(inner),
        _ => {}
    }
    while let Some(here) = row {
        pending.extend(here.labels.iter().map(|(_, field)| &field.ty));
        row = match &here.rest {
            Rest::More(more) => Some(more),
            _ => None,
        };
    }
}

fn ordinary(scheme: &Scheme, names: &HashMap<String, String>) -> Scheme {
    let mut ordinary = scheme.clone();
    ordinary.body = erase(&scheme.body, names, false);
    // The native shape accepts every optional-field combination. Assuming the
    // original relationship formula would hide precisely the caller obligations
    // this annotation must prove. Producer-owned package scopes are retained.
    ordinary.formula = Formula::True;
    ordinary.callable = None;
    ordinary.representations.clear();
    ordinary
}

/// Keep portable sharing and declaration back edges, and never recursively
/// expand an alias. Reflection positions retain their exact semantic identity.
fn erase(root: &Type, names: &HashMap<String, String>, close_rows: bool) -> Type {
    enum Work<'a> {
        Ty(&'a Type),
        Build(&'a Type),
        Row(&'a Row, bool),
        BuildRow(&'a Row, bool),
        Shared(*const Type),
    }
    let mut work = vec![Work::Ty(root)];
    let mut types = Vec::new();
    let mut rows = Vec::<Row>::new();
    let mut shared = HashMap::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Ty(ty) => match ty {
                Type::Contract { fallback, .. } => work.push(Work::Ty(fallback)),
                Type::Shared(inner) => {
                    let key = Arc::as_ptr(inner);
                    if let Some(value) = shared.get(&key) {
                        types.push(Type::Shared(Arc::clone(value)));
                    } else {
                        work.push(Work::Shared(key));
                        work.push(Work::Ty(inner));
                    }
                }
                Type::Mirror(_) | Type::TypeInfo(_) => types.push(ty.clone()),
                Type::Arrow(from, to, row) => {
                    work.push(Work::Build(ty));
                    work.push(Work::Row(row, true));
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                Type::Struct(row) | Type::Sum(row) => {
                    work.push(Work::Build(ty));
                    work.push(Work::Row(row, false));
                }
                Type::Package(inner) | Type::Array(inner) | Type::Hidden { body: inner, .. } => {
                    work.push(Work::Build(ty));
                    work.push(Work::Ty(inner));
                }
                Type::Mut(region, inner) => {
                    work.push(Work::Build(ty));
                    work.push(Work::Ty(inner));
                    work.push(Work::Ty(region));
                }
                Type::Named { args, .. } => {
                    work.push(Work::Build(ty));
                    work.extend(args.iter().rev().map(Work::Ty));
                }
                _ => types.push(ty.clone()),
            },
            Work::Shared(key) => {
                let value = Arc::new(types.pop().expect("shared witness node"));
                shared.insert(key, value.clone());
                types.push(Type::Shared(value));
            }
            Work::Row(row, effects) => {
                work.push(Work::BuildRow(row, effects));
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row(more, effects));
                }
                work.extend(
                    row.labels
                        .iter()
                        .rev()
                        .map(|(_, field)| Work::Ty(&field.ty)),
                );
            }
            Work::BuildRow(row, effects) => {
                let rest = match &row.rest {
                    Rest::More(_) => Rest::More(Box::new(rows.pop().expect("witness row tail"))),
                    Rest::Bound(_) | Rest::Var(_) if close_rows && !effects => Rest::Closed,
                    rest => rest.clone(),
                };
                let start = types.len() - row.labels.len();
                let fields = types.drain(start..);
                rows.push(Row {
                    labels: row
                        .labels
                        .iter()
                        .zip(fields)
                        .map(|((name, field), ty)| {
                            (
                                name.clone(),
                                crate::artifact::RowField {
                                    presence: field.presence.clone(),
                                    ty,
                                },
                            )
                        })
                        .collect(),
                    rest,
                });
            }
            Work::Build(original) => {
                let rebuilt = match original {
                    Type::Arrow(..) => {
                        let row = rows.pop().unwrap();
                        let to = types.pop().unwrap();
                        let from = types.pop().unwrap();
                        Type::Arrow(Box::new(from), Box::new(to), row)
                    }
                    Type::Struct(_) => Type::Struct(rows.pop().unwrap()),
                    Type::Sum(_) => Type::Sum(rows.pop().unwrap()),
                    Type::Package(_) => Type::Package(Box::new(types.pop().unwrap())),
                    Type::Array(_) => Type::Array(Box::new(types.pop().unwrap())),
                    Type::Hidden { binder, name, .. } => Type::Hidden {
                        binder: *binder,
                        name: name.clone(),
                        body: Box::new(types.pop().unwrap()),
                    },
                    Type::Mut(..) => {
                        let inner = types.pop().unwrap();
                        let region = types.pop().unwrap();
                        Type::Mut(Box::new(region), Box::new(inner))
                    }
                    Type::Named { name, args } => {
                        let start = types.len() - args.len();
                        Type::Named {
                            name: names.get(name).unwrap_or(name).clone(),
                            args: types.drain(start..).collect(),
                        }
                    }
                    _ => unreachable!("witness constructor"),
                };
                types.push(rebuilt);
            }
        }
    }
    types.pop().expect("one ordinary native witness")
}

fn parameters(scheme: &Scheme, declarations: &HashMap<&str, &DeclaredType>) -> Vec<Parameter> {
    let mut params: Vec<_> = (0..scheme.count)
        .map(|index| Parameter {
            sense: if index < scheme.presences {
                Sense::Presence
            } else {
                Sense::Type
            },
            lacks: Vec::new(),
            relevant: false,
        })
        .collect();
    let mut work = vec![(&scheme.body, Sense::Type)];
    let mut seen = HashSet::new();
    while let Some((ty, sense)) = work.pop() {
        let ty = ty.unshared();
        if !seen.insert((ty as *const Type, sense as u8)) {
            continue;
        }
        let mut row = None;
        let mut effect_row = false;
        match ty {
            Type::Bound(index) => {
                if let Some(param) = params.get_mut(*index as usize) {
                    param.sense = sense;
                    param.relevant = true;
                }
            }
            Type::Presence(Presence::Bound(index)) => {
                if let Some(param) = params.get_mut(*index as usize) {
                    param.sense = Sense::Presence;
                    param.relevant = true;
                }
            }
            Type::Arrow(from, to, effects) => {
                work.extend([(&**from, Sense::Type), (&**to, Sense::Type)]);
                row = Some(effects);
                effect_row = true;
            }
            Type::Struct(fields) | Type::Sum(fields) => row = Some(fields),
            Type::Named { name, args } => {
                let declaration = declarations.get(name.as_str());
                work.extend(args.iter().enumerate().map(|(index, ty)| {
                    (
                        ty,
                        declaration
                            .and_then(|decl| decl.params.get(index))
                            .map_or(Sense::Type, |param| param.sense),
                    )
                }));
            }
            Type::Contract { fallback, .. } => work.push((fallback, sense)),
            Type::Mut(region, inner) => {
                work.extend([(&**region, Sense::Region), (&**inner, Sense::Type)])
            }
            _ => {
                let mut children_here = Vec::new();
                children(ty, &mut children_here);
                work.extend(children_here.into_iter().map(|ty| (ty, Sense::Type)));
            }
        }
        while let Some(here) = row {
            for (_, field) in &here.labels {
                if let Presence::Bound(index) = field.presence
                    && let Some(param) = params.get_mut(index as usize)
                {
                    param.sense = Sense::Presence;
                    param.relevant = true;
                }
                work.push((&field.ty, Sense::Type));
            }
            row = match &here.rest {
                Rest::More(more) => Some(more),
                Rest::Bound(index) => {
                    if let Some(param) = params.get_mut(*index as usize) {
                        param.sense = if effect_row {
                            Sense::Effects
                        } else {
                            Sense::Row
                        };
                        param.relevant = true;
                        for (name, _) in &here.labels {
                            if !param.lacks.contains(name) {
                                param.lacks.push(name.clone());
                            }
                        }
                    }
                    None
                }
                _ => None,
            };
        }
    }
    params
}
