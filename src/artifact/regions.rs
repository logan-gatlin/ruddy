//! Validate region positions using the same declaration headers as an importer.

use std::collections::HashMap;

use super::{EffectKind, Header, Parameter, Presence, Rest, Row, Sense, Type};

pub(super) fn validate(header: &Header) -> Result<(), &'static str> {
    let types: HashMap<_, _> = header
        .types
        .iter()
        .map(|decl| (decl.name.as_str(), decl.params.as_slice()))
        .collect();
    let effects: HashMap<_, _> = header
        .effects
        .iter()
        .filter_map(|decl| {
            decl.identity.as_ref().map(|id| {
                (
                    crate::types::EffectId::structural(id.name.clone(), id.interface.clone())
                        .row_key(),
                    decl.params.as_slice(),
                )
            })
        })
        .collect();
    let mutation = crate::types::mutation_effect();
    for effect in &header.effects {
        let named_mutation = effect.name.rsplit("::").next() == Some("mut");
        let claims_mutation = effect.identity.as_ref().is_some_and(|id| {
            crate::types::EffectId::structural(id.name.clone(), id.interface.clone()) == mutation
        });
        if named_mutation || claims_mutation {
            let valid = named_mutation
                && claims_mutation
                && effect.params.len() == 1
                && effect.params[0].sense == Sense::Region
                && matches!(&effect.kind, EffectKind::Operations(operations) if operations.is_empty());
            if !valid {
                return Err("invalid builtin mutation declaration");
            }
        }
    }
    let mut validator = Validator {
        types,
        effects,
        slots: HashMap::new(),
        count: 0,
        presences: 0,
    };
    for declaration in &header.types {
        validator.scope(
            declaration.scheme.count,
            declaration.scheme.presences,
            &declaration.params,
        )?;
        validator.ty(&declaration.scheme.body, Some(Sense::Type))?;
    }
    for value in &header.values {
        validator.scope(value.scheme.count, value.scheme.presences, &[])?;
        validator.ty(&value.scheme.body, Some(Sense::Type))?;
    }
    for effect in &header.effects {
        validator.scope(effect.params.len() as u32, 0, &effect.params)?;
        match &effect.kind {
            EffectKind::Operations(operations) => {
                for operation in operations {
                    validator.ty(&operation.from, Some(Sense::Type))?;
                    validator.ty(&operation.to, Some(Sense::Type))?;
                }
            }
            EffectKind::Alias(row) => {
                for case in &row.cases {
                    let kinds = header
                        .effects
                        .iter()
                        .find(|effect| effect.name == case.name)
                        .map(|effect| &effect.params);
                    for (index, arg) in case.args.iter().enumerate() {
                        validator.ty(
                            arg,
                            kinds
                                .and_then(|params| params.get(index))
                                .map(|param| param.sense),
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

struct Validator<'a> {
    types: HashMap<&'a str, &'a [Parameter]>,
    effects: HashMap<String, &'a [Parameter]>,
    slots: HashMap<u32, Sense>,
    count: u32,
    presences: u32,
}

impl Validator<'_> {
    fn scope(
        &mut self,
        count: u32,
        presences: u32,
        params: &[Parameter],
    ) -> Result<(), &'static str> {
        self.count = count;
        self.presences = presences;
        self.slots.clear();
        for (index, param) in params.iter().enumerate() {
            if param.sense == Sense::Region && !param.lacks.is_empty() {
                return Err("region parameters cannot carry row lacks");
            }
            self.slots.insert(index as u32, param.sense);
        }
        Ok(())
    }

    fn bound(&mut self, index: u32, sense: Option<Sense>) -> Result<(), &'static str> {
        if index < self.presences || index >= self.count {
            return Err("region or type bound is outside the quantifier space");
        }
        if let Some(sense) = sense {
            if let Some(prior) = self.slots.get(&index) {
                if (*prior == Sense::Region) != (sense == Sense::Region) {
                    return Err("region variable is also used at another kind");
                }
            } else {
                self.slots.insert(index, sense);
            }
        }
        Ok(())
    }

    fn ty(&mut self, root: &Type, sense: Option<Sense>) -> Result<(), &'static str> {
        enum Work<'a> {
            Ty(&'a Type, Option<Sense>),
            Row(&'a Row, Sense),
        }
        let mut work = vec![Work::Ty(root, sense)];
        let mutation = crate::types::mutation_effect().row_key();
        while let Some(part) = work.pop() {
            match part {
                Work::Ty(ty, sense) => {
                    if let Type::Bound(index) = ty {
                        self.bound(*index, sense)?;
                        continue;
                    }
                    if sense == Some(Sense::Region) {
                        return Err("a region position requires a bound region variable");
                    }
                    match ty {
                        Type::Mut(region, element) => {
                            work.push(Work::Ty(region, Some(Sense::Region)));
                            work.push(Work::Ty(element, Some(Sense::Type)));
                        }
                        Type::Package(inner) | Type::Array(inner) => {
                            work.push(Work::Ty(inner, Some(Sense::Type)))
                        }
                        Type::Arrow(from, to, row) => {
                            work.push(Work::Ty(from, Some(Sense::Type)));
                            work.push(Work::Ty(to, Some(Sense::Type)));
                            work.push(Work::Row(row, Sense::Effects));
                        }
                        Type::Struct(row) => work.push(Work::Row(row, Sense::Fields)),
                        Type::Sum(row) => work.push(Work::Row(
                            row,
                            if sense == Some(Sense::Effects) {
                                Sense::Effects
                            } else {
                                Sense::Cases
                            },
                        )),
                        Type::Named { name, args } => {
                            for (index, arg) in args.iter().enumerate() {
                                let expected = self
                                    .types
                                    .get(name.as_str())
                                    .and_then(|params| params.get(index))
                                    .map(|param| param.sense);
                                work.push(Work::Ty(arg, expected));
                            }
                        }
                        _ => {}
                    }
                }
                Work::Row(row, sense) => {
                    match &row.rest {
                        Rest::Bound(index) => self.bound(*index, Some(sense))?,
                        Rest::More(more) => work.push(Work::Row(more, sense)),
                        _ => {}
                    }
                    for (name, field) in &row.labels {
                        if matches!(field.presence, Presence::Absent) {
                            continue;
                        }
                        if sense == Sense::Effects {
                            let builtin = name == &mutation;
                            let kinds = self.effects.get(name);
                            // Imported effect headers may live only in a dependency.
                            // Their tuple is still an argument list: an unknown slot
                            // kind must not turn a region argument into a value type.
                            if builtin || kinds.is_some() || matches!(&field.ty, Type::Struct(_)) {
                                let Type::Struct(args) = &field.ty else {
                                    return Err("invalid effect argument tuple");
                                };
                                if builtin
                                    && (args.labels.len() != 1
                                        || args.labels[0].0 != "0"
                                        || !matches!(args.rest, Rest::Closed))
                                {
                                    return Err("mutation requires exactly one region argument");
                                }
                                for (index, (_, arg)) in args.labels.iter().enumerate() {
                                    let expected = if builtin {
                                        Some(Sense::Region)
                                    } else {
                                        kinds
                                            .and_then(|params| params.get(index))
                                            .map(|param| param.sense)
                                    };
                                    work.push(Work::Ty(&arg.ty, expected));
                                }
                                continue;
                            }
                        }
                        work.push(Work::Ty(
                            &field.ty,
                            if sense == Sense::Effects {
                                None
                            } else {
                                Some(Sense::Type)
                            },
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
