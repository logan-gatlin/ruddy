//! Ordinary inference for the program written in a structural annotation.
//!
//! Pattern names describe projections, not independent polymorphic types.
//! Reusing the value inference rules makes all arms share ordinary payloads,
//! while the retained contract still describes conditional fields and cases.

use std::{collections::HashMap, collections::HashSet, sync::Arc};

use indexmap::IndexMap;

use crate::{
    contracts::{Contract, Expr, Pattern},
    ir::{self, Term, TermKind},
    symbol::{Bundle, Mint, Namespace, Symbol, Version},
    tracking::Anchor,
    types::{Formula, Presence, Row, RowField, Scheme, Ty, TyVar},
};

use super::{
    Binding, Constrain, Env, Error, ErrorKind, ExplainedScheme, Solve, Subject, Subst, Table,
    Tails, VarSort, constrain::Ambient,
};

#[derive(Hash, PartialEq, Eq)]
pub(super) struct Key {
    body: usize,
    operands: Vec<usize>,
}

pub(super) struct Template {
    // Retain the allocations whose identities form the cache key.
    _contract: Contract,
    scheme: Scheme,
}

pub(super) struct Checked {
    pub fallback: Arc<Ty>,
    /// Implicit variables introduced by the contract's own inference, as
    /// opposed to holes explicitly written in its captured ordinary types.
    pub variables: Vec<TyVar>,
    pub errors: Vec<Error>,
}

fn skeleton(parameters: usize) -> Arc<Ty> {
    let mut fallback = Arc::new(Ty::Undecided);
    for _ in 0..parameters {
        fallback = Arc::new(Ty::Arrow(Arc::new(Ty::Undecided), fallback, Row::closed()));
    }
    fallback
}

/// Alias templates cannot contain inference variables from the declaration's
/// table. Their ordinary part is inferred afresh when the alias is opened.
pub(super) fn lower(
    mint: &Mint,
    table: &mut Table,
    tails: &mut Tails,
    at: Anchor,
    contract: Arc<Contract>,
    annotation: bool,
) -> Ty {
    let fallback = if annotation {
        let checked = check(mint, table, at, &contract);
        tails.representation_variables.extend(checked.variables);
        table.contract_annotation_errors.extend(checked.errors);
        checked.fallback
    } else {
        skeleton(contract.remaining_parameters())
    };
    Ty::Contract { fallback, contract }
}

fn is_template(mut fallback: &Ty) -> bool {
    loop {
        match fallback {
            Ty::Arrow(parameter, result, _) if matches!(&**parameter, Ty::Undecided) => {
                fallback = result;
            }
            Ty::Undecided => return true,
            _ => return false,
        }
    }
}

fn private_mint() -> Mint {
    Mint::new(
        Bundle::new("contract-annotations", Version::new(0, 0, 0))
            .expect("the internal bundle name is valid"),
    )
}

/// Opening a closed alias template produces fresh shared payload variables.
/// Its partial applications subsequently retain this inferred fallback.
pub(super) fn reconstruct(table: &mut Table, at: Anchor, ty: Arc<Ty>) -> Checked {
    let Ty::Contract { fallback, contract } = &*ty else {
        return Checked {
            fallback: ty,
            variables: Vec::new(),
            errors: Vec::new(),
        };
    };
    if !is_template(fallback)
        || table
            .active_contract_annotations
            .contains(&(Arc::as_ptr(&contract.body) as usize))
    {
        return Checked {
            fallback: ty,
            variables: Vec::new(),
            errors: Vec::new(),
        };
    }
    let mut checked = check(&private_mint(), table, at, contract);
    checked.fallback = Arc::new(Ty::Contract {
        fallback: checked.fallback,
        contract: contract.clone(),
    });
    checked
}

/// Check even unused alias contracts after all mutually recursive names are
/// available. The temporary table cannot leak implicit inference slots into
/// a closed declaration or constrain another declaration's captures.
pub(super) fn validate_declaration(table: &mut Table, at: Anchor, root: &Arc<Ty>) {
    enum Part<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
    }
    let mut pending = vec![Part::Ty(root)];
    let mut seen = HashSet::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Ty(ty) => {
                if !seen.insert(ty as *const Ty) {
                    continue;
                }
                match ty {
                    Ty::Contract { contract, .. } => {
                        let mut local = Table::new(at.definition, table.signatures.clone());
                        let checked = check(&private_mint(), &mut local, at, contract);
                        table.contract_annotation_errors.extend(checked.errors);
                        pending.extend(contract.type_operands().map(|ty| Part::Ty(ty)));
                    }
                    Ty::Arrow(input, result, effects) => {
                        pending.extend([Part::Ty(input), Part::Ty(result), Part::Row(effects)]);
                    }
                    Ty::Struct(row) | Ty::Sum(row) => pending.push(Part::Row(row)),
                    Ty::Mut(region, inner) => pending.extend([Part::Ty(region), Part::Ty(inner)]),
                    Ty::Array(inner)
                    | Ty::Mirror(inner)
                    | Ty::TypeInfo(inner)
                    | Ty::Package(inner)
                    | Ty::Hidden { body: inner, .. } => pending.push(Part::Ty(inner)),
                    Ty::Named { args, .. } => pending.extend(args.iter().map(|ty| Part::Ty(ty))),
                    _ => {}
                }
            }
            Part::Row(row) => {
                pending.extend(row.labels.values().map(|field| Part::Ty(&field.ty)));
                if let crate::types::Rest::More(row) = &row.rest {
                    pending.push(Part::Row(row));
                }
            }
        }
    }
}

/// Infer the ordinary signature of a finite contract with the same rules as
/// its corresponding value program. Captures are monomorphic operands here:
/// their declared polymorphism was already instantiated by annotation lowering.
pub(super) fn check(mint: &Mint, table: &mut Table, at: Anchor, contract: &Contract) -> Checked {
    let identity = Key {
        body: Arc::as_ptr(&contract.body) as usize,
        operands: contract
            .type_operands()
            .map(|ty| Arc::as_ptr(ty) as usize)
            .collect(),
    };
    if let Some(template) = table.contract_annotation_templates.get(&identity) {
        let scheme = template.scheme.clone();
        let first = table.vars.len();
        let fallback = table.instantiate_scoped(at, &ExplainedScheme::imported(scheme), None);
        return Checked {
            fallback,
            variables: (first as TyVar..table.vars.len() as TyVar).collect(),
            errors: Vec::new(),
        };
    }
    if !table.active_contract_annotations.insert(identity.body) {
        return Checked {
            fallback: skeleton(contract.remaining_parameters()),
            variables: Vec::new(),
            errors: Vec::new(),
        };
    }
    let checked = check_inner(mint, table, at, contract);
    table.active_contract_annotations.remove(&identity.body);
    if checked.errors.is_empty() && !contains_bound(contract) {
        let mut subst = Subst::default();
        for variable in &checked.variables {
            if table.var_meta[*variable as usize].sort == VarSort::Presence {
                let index = subst.presences.len() as u32;
                subst.presences.insert(*variable, index);
            }
        }
        let presences = subst.presences.len() as u32;
        for variable in &checked.variables {
            if table.var_meta[*variable as usize].sort != VarSort::Presence {
                let index = presences + subst.types.len() as u32;
                subst.types.insert(*variable, index);
            }
        }
        let scheme = Scheme::constrained(
            subst.next(),
            presences,
            table.zonk(&checked.fallback, &subst),
            Formula::True,
        );
        table.contract_annotation_templates.insert(
            identity,
            Template {
                _contract: contract.clone(),
                scheme,
            },
        );
    }
    checked
}

fn contains_bound(contract: &Contract) -> bool {
    enum Part<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
    }
    let mut pending: Vec<_> = contract.type_operands().map(|ty| Part::Ty(ty)).collect();
    let mut seen = HashSet::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Ty(ty) => {
                if !seen.insert(ty as *const Ty) {
                    continue;
                }
                match ty {
                    Ty::Bound(_)
                    | Ty::Rigid { .. }
                    | Ty::Presence(crate::types::Presence::Bound(_)) => return true,
                    Ty::Contract { fallback, contract } => {
                        pending.push(Part::Ty(fallback));
                        pending.extend(contract.type_operands().map(|ty| Part::Ty(ty)));
                    }
                    Ty::Arrow(input, output, row) => {
                        pending.extend([Part::Ty(input), Part::Ty(output), Part::Row(row)])
                    }
                    Ty::Struct(row) | Ty::Sum(row) => pending.push(Part::Row(row)),
                    Ty::Array(inner)
                    | Ty::Mirror(inner)
                    | Ty::TypeInfo(inner)
                    | Ty::Package(inner)
                    | Ty::Hidden { body: inner, .. } => pending.push(Part::Ty(inner)),
                    Ty::Mut(region, inner) => pending.extend([Part::Ty(region), Part::Ty(inner)]),
                    Ty::Named { args, .. } => pending.extend(args.iter().map(|ty| Part::Ty(ty))),
                    _ => {}
                }
            }
            Part::Row(row) => {
                if matches!(row.rest, crate::types::Rest::Bound(_))
                    || row
                        .labels
                        .values()
                        .any(|field| matches!(field.presence, crate::types::Presence::Bound(_)))
                {
                    return true;
                }
                pending.extend(row.labels.values().map(|field| Part::Ty(&field.ty)));
                if let crate::types::Rest::More(row) = &row.rest {
                    pending.push(Part::Row(row));
                }
            }
        }
    }
    false
}

fn ordinary_capture(table: &mut Table, capture: &Arc<Ty>) -> Arc<Ty> {
    // Expose the captured declaration's checked ordinary witness without
    // evaluating its structural graph. `unfold` stops at that contract node,
    // so a shared alias chain remains finite here.
    let shape = super::unfold(&table.signatures.aliases, &table.resolve(capture));
    let mut ordinary = match &*shape {
        // Nested contracts are lowered and validated independently. Their
        // ordinary witness is enough while checking the enclosing graph and
        // avoids recursively rechecking an entire captured contract DAG.
        Ty::Contract { fallback, .. } => fallback.clone(),
        _ => shape,
    };
    while let Ty::Contract { fallback, .. } = &*ordinary {
        ordinary = table.resolve(fallback);
    }
    erase_shapes(table, &ordinary, 0, &mut HashMap::new())
}

/// A contract graph decides which fields and tags are present. Its ordinary
/// witness checks the payloads at those positions, but must not make the row
/// chosen in one arm equal to the row chosen in another arm.
fn erase_shapes(
    table: &mut Table,
    ty: &Arc<Ty>,
    depth: usize,
    cache: &mut HashMap<*const Ty, (Arc<Ty>, Arc<Ty>)>,
) -> Arc<Ty> {
    if depth > crate::contracts::MAX_DEPTH {
        return Arc::new(Ty::Undecided);
    }
    let ty = table.resolve(ty);
    if let Some((_, erased)) = cache.get(&Arc::as_ptr(&ty)) {
        return erased.clone();
    }
    let erased = match &*ty {
        Ty::Struct(row) | Ty::Sum(row) => {
            let row = table.canon(row);
            let mut labels = IndexMap::new();
            for (label, field) in &row.labels {
                if matches!(table.presence_of(&field.presence), Presence::Absent) {
                    continue;
                }
                let payload = erase_shapes(table, &field.ty, depth + 1, cache);
                labels.insert(
                    label.clone(),
                    RowField {
                        presence: table.fresh_presence_for(Subject::Annotation),
                        ty: payload,
                    },
                );
            }
            let row = Row {
                labels,
                rest: table.fresh_row_for(Subject::Annotation),
            };
            Arc::new(if matches!(&*ty, Ty::Struct(_)) {
                Ty::Struct(row)
            } else {
                Ty::Sum(row)
            })
        }
        Ty::Arrow(input, output, effects) => Arc::new(Ty::Arrow(
            erase_shapes(table, input, depth + 1, cache),
            erase_shapes(table, output, depth + 1, cache),
            effects.clone(),
        )),
        Ty::Array(inner) => Arc::new(Ty::Array(erase_shapes(table, inner, depth + 1, cache))),
        Ty::Mirror(inner) => Arc::new(Ty::Mirror(erase_shapes(table, inner, depth + 1, cache))),
        Ty::TypeInfo(inner) => Arc::new(Ty::TypeInfo(erase_shapes(table, inner, depth + 1, cache))),
        Ty::Package(inner) => erase_shapes(table, inner, depth + 1, cache),
        Ty::Contract { fallback, .. } => erase_shapes(table, fallback, depth + 1, cache),
        _ => ty.clone(),
    };
    cache.insert(Arc::as_ptr(&ty), (ty, erased.clone()));
    erased
}

fn check_inner(mint: &Mint, table: &mut Table, at: Anchor, contract: &Contract) -> Checked {
    let first_variable = table.vars.len();
    let mut builder = Builder {
        mint: mint.clone(),
        at,
        remaining: 32_768,
        inputs: Vec::new(),
        captures: Vec::new(),
        env: Env {
            outer: Arc::new(HashMap::new()),
            own: HashMap::new(),
        },
    };
    for index in 0..contract.parameters {
        let symbol = builder.symbol(&format!("contract_input_{index}"));
        builder.inputs.push(symbol);
    }
    for (index, capture) in contract.captures.iter().enumerate() {
        let symbol = builder.symbol(&format!("contract_capture_{index}"));
        builder
            .env
            .insert(symbol, Binding::Mono(ordinary_capture(table, capture)));
        builder.captures.push(symbol);
    }
    let Some(mut term) = builder.expression(&contract.body, 0) else {
        return Checked {
            fallback: Arc::new(Ty::Undecided),
            variables: Vec::new(),
            errors: vec![Error::new(
                at,
                ErrorKind::StructuralContract {
                    message: "structural annotation exceeds its finite inference budget".into(),
                },
            )],
        };
    };
    for input in builder.inputs.iter().rev() {
        term = builder.term(TermKind::Fn {
            arg: at.anchor(*input),
            body: Box::new(term),
        });
    }
    for (index, argument) in contract.arguments.iter().enumerate() {
        let symbol = builder.symbol(&format!("contract_argument_{index}"));
        builder.env.insert(symbol, Binding::Mono(argument.clone()));
        term = builder.term(TermKind::Apply {
            func: Box::new(term),
            arg: Box::new(builder.term(TermKind::Ident(symbol))),
        });
    }

    let level = table.level;
    let signatures = table.signatures.clone();
    let effect_ids = IndexMap::new();
    let effect_params = IndexMap::new();
    // A saturated residual describes a computation which will run at its use
    // site. Its effects must not be mistaken for top-level execution here.
    let ambient = Row::of(table.fresh_row_for(Subject::AmbientEffects));
    let group_members: Vec<_> = builder
        .inputs
        .iter()
        .chain(&builder.captures)
        .copied()
        .collect();
    let mut constrain = Constrain {
        table,
        mint: &builder.mint,
        env: &mut builder.env,
        group_members: &group_members,
        aliases: &signatures.aliases,
        out: Vec::new(),
        annotated: Vec::new(),
        opens: Vec::new(),
        opening: 0,
        operations: &signatures.operations,
        effect_ids: &effect_ids,
        effect_params: &effect_params,
        ambient: Ambient {
            row: ambient,
            inside: true,
            boundary_at: at,
            label_spans: IndexMap::new(),
        },
        mutations: Vec::new(),
        answer: None,
        presence_guard: Formula::True,
    };
    constrain.infer_term(&mut term);
    let constraints = constrain.out;
    let generated_end = table.store.batches.len();
    let mut errors = Vec::new();
    let mut steps = Vec::new();
    let mut locals = IndexMap::new();
    let mut refinements = Vec::new();
    Solve {
        table,
        errors: &mut errors,
        steps: &mut steps,
        aliases: &signatures.aliases,
        nominal: &signatures.nominal,
        definition: at.definition,
        depth: 0,
        constraint: None,
        constraint_reason: None,
        family_origins: HashMap::new(),
        assumed: Vec::new(),
        schemes: HashMap::new(),
        locals: &mut locals,
        guard: None,
        active_refinement: None,
        guard_reasons: Vec::new(),
        refinements: &mut refinements,
        generated_end,
    }
    .run(&constraints);
    table.level = level;
    let mut fallback = table.resolve(&term.ty);
    while let Ty::Contract { fallback: next, .. } = &*fallback {
        fallback = table.resolve(next);
    }
    table.close_effects(&fallback, level);
    // Synthetic constraints are implementation details, not source nodes.
    // Publish direct annotation errors instead of dangling debugger links to
    // a constraint arena which the value program never contained.
    let errors = errors
        .into_iter()
        .map(|error| {
            Error::new(
                at,
                ErrorKind::StructuralContract {
                    message: format!(
                        "incompatible ordinary types in structural annotation: {}",
                        error.kind
                    ),
                },
            )
        })
        .collect();
    Checked {
        fallback,
        variables: (first_variable as TyVar..table.vars.len() as TyVar).collect(),
        errors,
    }
}

struct Builder {
    mint: Mint,
    at: Anchor,
    remaining: usize,
    inputs: Vec<Symbol>,
    captures: Vec<Symbol>,
    env: Env,
}

impl Builder {
    fn symbol(&mut self, name: &str) -> Symbol {
        self.mint.local(None, Namespace::Terms, name)
    }

    fn term(&self, kind: TermKind) -> Term {
        Term {
            ty: Arc::new(Ty::Undecided),
            at: self.at,
            kind,
        }
    }

    fn spend(&mut self, depth: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(1)?;
        (depth <= 256).then_some(())
    }

    fn pattern(&mut self, pattern: &Pattern, depth: usize) -> Option<ir::Pattern> {
        self.spend(depth)?;
        let kind = match pattern {
            Pattern::Any => ir::PatternKind::Wildcard,
            Pattern::Record { fields, open } => ir::PatternKind::Struct {
                fields: fields
                    .iter()
                    .map(|(label, pattern)| {
                        Some((
                            label.clone(),
                            ir::Field {
                                name_at: self.at,
                                value: self.pattern(pattern, depth + 1)?,
                            },
                        ))
                    })
                    .collect::<Option<_>>()?,
                rest: open.then_some(self.at),
            },
            Pattern::Tag { label, payload } => ir::PatternKind::Tag {
                name: self.at.anchor(label.clone()),
                payload: Some(Box::new(self.pattern(payload, depth + 1)?)),
            },
        };
        Some(self.at.anchor(kind))
    }

    fn expression(&mut self, expr: &Expr, depth: usize) -> Option<Term> {
        self.spend(depth)?;
        let kind = match expr {
            Expr::Input(index) => TermKind::Ident(*self.inputs.get(*index)?),
            Expr::Capture(index) => TermKind::Ident(*self.captures.get(*index)?),
            Expr::Field { base, label } => TermKind::Project {
                base: Box::new(self.expression(base, depth + 1)?),
                field: self.at.anchor(label.clone()),
            },
            Expr::Payload { base, label } => {
                let payload = self.symbol("contract_payload");
                TermKind::Match {
                    scrutinee: Box::new(self.expression(base, depth + 1)?),
                    arms: vec![(
                        self.at.anchor(ir::PatternKind::Tag {
                            name: self.at.anchor(label.clone()),
                            payload: Some(Box::new(
                                self.at
                                    .anchor(ir::PatternKind::Bind(self.at.anchor(payload))),
                            )),
                        }),
                        self.term(TermKind::Ident(payload)),
                    )],
                }
            }
            Expr::Record { fields, spread } => TermKind::Struct {
                fields: fields
                    .iter()
                    .map(|(label, value)| {
                        Some((
                            label.clone(),
                            ir::Field {
                                name_at: self.at,
                                value: self.expression(value, depth + 1)?,
                            },
                        ))
                    })
                    .collect::<Option<_>>()?,
                spread: match spread {
                    Some(value) => Some(ir::Spread {
                        at: self.at,
                        value: Box::new(self.expression(value, depth + 1)?),
                    }),
                    None => None,
                },
            },
            Expr::Tag { label, payload } => TermKind::Tag {
                name: self.at.anchor(label.clone()),
                payload: Some(Box::new(self.expression(payload, depth + 1)?)),
            },
            Expr::Apply { function, argument } => TermKind::Apply {
                func: Box::new(self.expression(function, depth + 1)?),
                arg: Box::new(self.expression(argument, depth + 1)?),
            },
            Expr::Match { scrutinee, arms } => TermKind::Match {
                scrutinee: Box::new(self.expression(scrutinee, depth + 1)?),
                arms: arms
                    .iter()
                    .map(|arm| {
                        Some((
                            self.pattern(&arm.pattern, depth + 1)?,
                            self.expression(&arm.body, depth + 1)?,
                        ))
                    })
                    .collect::<Option<_>>()?,
            },
            Expr::Then { value, body } => TermKind::Let {
                name: self.at.anchor(self.symbol("contract_then")),
                annotation: None,
                value: Box::new(self.expression(value, depth + 1)?),
                body: Box::new(self.expression(body, depth + 1)?),
            },
        };
        Some(self.term(kind))
    }
}
