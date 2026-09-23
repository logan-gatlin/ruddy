//! Replay finite structural contracts over callable conventions. Ordinary
//! inference has already established the value types; this pass preserves the
//! exact hidden arguments of callbacks forwarded through records and matches.

use super::{
    conventions::{Graph, Shape, ShapeId},
    *,
};
use crate::contracts::{Contract, Expr, Pattern};

#[derive(Clone)]
struct Value {
    ty: Arc<Ty>,
    shape: ShapeId,
    supplied: Vec<(ShapeId, bool)>,
    /// An ordinary captured function's type does not determine which hidden
    /// descriptors its implementation requires. Only argument conventions or
    /// monomorphic captures can authorize replacing the planner's result.
    known: bool,
}

/// A generic callback cannot be represented by an erased value convention:
/// its producer may require hidden descriptors that a later call must pass.
pub(super) fn missing_profile(
    graph: &Graph,
    aliases: &IndexMap<Symbol, Scheme>,
    ty: &Arc<Ty>,
    shape: ShapeId,
) -> bool {
    let mut pending = vec![(ty.clone(), shape)];
    let mut seen = HashMap::new();
    let mut named = HashSet::new();
    while let Some((ty, shape)) = pending.pop() {
        let shape = graph.exposed_id(shape);
        if seen
            .insert((Arc::as_ptr(&ty) as usize, shape), ty.clone())
            .is_some()
        {
            continue;
        }
        if matches!(
            graph.exposed(shape),
            Shape::Parameter(_) | Shape::Lazy | Shape::Sealed
        ) {
            continue;
        }
        match &*ty {
            Ty::Contract { fallback, .. }
            | Ty::Package(fallback)
            | Ty::Hidden { body: fallback, .. } => pending.push((fallback.clone(), shape)),
            Ty::Arrow(from, to, _) => {
                if let Shape::Arrow {
                    argument, result, ..
                } = graph.exposed(shape)
                {
                    pending.extend([(from.clone(), *argument), (to.clone(), *result)]);
                } else if !super::parameters(from).is_empty() || !super::parameters(to).is_empty() {
                    return true;
                }
            }
            Ty::Struct(row) | Ty::Sum(row) => {
                for (name, field) in &super::flattened(row).labels {
                    if matches!(field.presence, Presence::Absent) {
                        continue;
                    }
                    let member = match graph.exposed(shape) {
                        Shape::Record(fields) | Shape::Sum(fields) => {
                            fields.get(name).copied().unwrap_or(shape)
                        }
                        _ => shape,
                    };
                    pending.push((field.ty.clone(), member));
                }
            }
            Ty::Array(element) | Ty::Mut(_, element) => {
                let member = match graph.exposed(shape) {
                    Shape::Array(element) | Shape::Cell(element) => *element,
                    _ => shape,
                };
                pending.push((element.clone(), member));
            }
            Ty::Named { symbol, .. } if named.insert((*symbol, shape)) => {
                pending.push((inference::unfold(aliases, &ty), shape))
            }
            _ => {}
        }
    }
    false
}

pub(super) fn apply(
    graph: &mut Graph,
    aliases: &IndexMap<Symbol, Scheme>,
    function: &Arc<Ty>,
    function_shape: ShapeId,
    argument: &Arc<Ty>,
    argument_shape: ShapeId,
    fallback: ShapeId,
) -> Option<ShapeId> {
    let function = inference::unfold(aliases, function);
    let Ty::Contract { contract, .. } = &*function else {
        return None;
    };
    let supplied = graph
        .structural_arguments
        .get(&graph.exposed_id(function_shape))
        .map(|arguments| arguments.iter().map(|shape| (*shape, true)).collect())
        .unwrap_or_default();
    let mut evaluator = Evaluator {
        graph,
        aliases,
        remaining: 32768,
        active: HashSet::new(),
    };
    let function = Value {
        ty: function.clone(),
        shape: function_shape,
        supplied,
        known: true,
    };
    let argument = Value {
        ty: argument.clone(),
        shape: argument_shape,
        supplied: Vec::new(),
        known: true,
    };
    if contract.arguments.len() + 1 < contract.parameters {
        let mut supplied = function.supplied;
        if supplied.len() != contract.arguments.len() {
            supplied = contract
                .arguments
                .iter()
                .map(|ty| {
                    (
                        evaluator.graph.skeleton(ty, aliases),
                        super::parameters(ty).is_empty(),
                    )
                })
                .collect();
        }
        supplied.push((argument.shape, argument.known));
        if supplied.iter().any(|(_, known)| !known) {
            return None;
        }
        let key = evaluator.graph.exposed_id(fallback);
        evaluator
            .graph
            .structural_arguments
            .insert(key, supplied.into_iter().map(|(shape, _)| shape).collect());
        return Some(fallback);
    }
    evaluator
        .invoke(function, argument, 0)
        .filter(|value| value.known)
        .map(|value| value.shape)
}

struct Evaluator<'a> {
    graph: &'a mut Graph,
    aliases: &'a IndexMap<Symbol, Scheme>,
    remaining: usize,
    active: HashSet<usize>,
}

impl Evaluator<'_> {
    fn step(&mut self, _depth: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(1)?;
        Some(())
    }

    fn invoke(&mut self, function: Value, argument: Value, depth: usize) -> Option<Value> {
        struct Environment {
            contract: Arc<Contract>,
            inputs: Vec<Value>,
            memo: HashMap<usize, Value>,
        }
        enum Work {
            Invoke(Value, Value, usize),
            Expression(Arc<Expr>, usize, usize),
            Remember(usize, usize),
            Leave(usize),
            Field(String),
            Record(Vec<String>, bool),
            Tag(String),
            Apply(usize),
            Then,
            Match(Arc<[crate::contracts::Arm]>, usize, usize),
        }
        let mut environments: Vec<Environment> = Vec::new();
        let mut work = vec![Work::Invoke(function, argument, depth)];
        let mut values: Vec<Value> = Vec::new();
        while let Some(next) = work.pop() {
            match next {
                Work::Invoke(function, argument, depth) => {
                    self.step(depth)?;
                    let ty = inference::unfold(self.aliases, &function.ty);
                    let Ty::Contract { fallback, contract } = &*ty else {
                        return None;
                    };
                    let mut supplied = function.supplied;
                    if supplied.len() != contract.arguments.len()
                        && let Some(arguments) = self
                            .graph
                            .structural_arguments
                            .get(&self.graph.exposed_id(function.shape))
                    {
                        supplied = arguments.iter().map(|shape| (*shape, true)).collect();
                    }
                    if supplied.len() != contract.arguments.len() {
                        supplied = contract
                            .arguments
                            .iter()
                            .map(|ty| {
                                (
                                    self.graph.skeleton(ty, self.aliases),
                                    super::parameters(ty).is_empty(),
                                )
                            })
                            .collect();
                    }
                    supplied.push((argument.shape, argument.known));
                    let mut arguments = contract.arguments.to_vec();
                    arguments.push(argument.ty);
                    if arguments.len() < contract.parameters {
                        let next = crate::contracts::representation::applied(
                            fallback,
                            arguments.last()?,
                            |ty| inference::unfold(self.aliases, ty),
                            super::flattened,
                            Clone::clone,
                        );
                        let ty = Arc::new(Ty::Contract {
                            fallback: next,
                            contract: Arc::new(Contract {
                                arguments: arguments.into(),
                                ..(**contract).clone()
                            }),
                        });
                        let shape = self.graph.skeleton(&ty, self.aliases);
                        values.push(Value {
                            ty,
                            shape,
                            supplied,
                            known: function.known && argument.known,
                        });
                        continue;
                    }
                    if arguments.len() != contract.parameters {
                        return None;
                    }
                    let key = Arc::as_ptr(&contract.body) as usize;
                    if !self.active.insert(key) {
                        return None;
                    }
                    let inputs = arguments
                        .into_iter()
                        .zip(supplied)
                        .map(|(ty, (shape, known))| Value {
                            ty,
                            shape,
                            supplied: Vec::new(),
                            known,
                        })
                        .collect();
                    let environment = environments.len();
                    environments.push(Environment {
                        contract: contract.clone(),
                        inputs,
                        memo: HashMap::new(),
                    });
                    work.push(Work::Leave(key));
                    work.push(Work::Expression(
                        contract.body.clone(),
                        environment,
                        depth + 1,
                    ));
                }
                Work::Leave(key) => {
                    self.active.remove(&key);
                }
                Work::Remember(environment, key) => {
                    environments[environment]
                        .memo
                        .insert(key, values.last()?.clone());
                }
                Work::Expression(expression, environment, depth) => {
                    self.step(depth)?;
                    let key = Arc::as_ptr(&expression) as usize;
                    if let Some(value) = environments[environment].memo.get(&key) {
                        values.push(value.clone());
                        continue;
                    }
                    work.push(Work::Remember(environment, key));
                    match &*expression {
                        Expr::Input(index) => {
                            values.push(environments[environment].inputs.get(*index)?.clone())
                        }
                        Expr::Capture(index) => {
                            let ty = environments[environment]
                                .contract
                                .captures
                                .get(*index)?
                                .clone();
                            let shape = self.graph.skeleton(&ty, self.aliases);
                            values.push(Value {
                                known: super::parameters(&ty).is_empty(),
                                ty,
                                shape,
                                supplied: Vec::new(),
                            });
                        }
                        Expr::Field { base, label } | Expr::Payload { base, label } => {
                            work.push(Work::Field(label.clone()));
                            work.push(Work::Expression(base.clone(), environment, depth + 1));
                        }
                        Expr::Record { fields, spread } => {
                            work.push(Work::Record(
                                fields.iter().map(|(name, _)| name.clone()).collect(),
                                spread.is_some(),
                            ));
                            for (_, expression) in fields.iter().rev() {
                                work.push(Work::Expression(
                                    expression.clone(),
                                    environment,
                                    depth + 1,
                                ));
                            }
                            if let Some(spread) = spread {
                                work.push(Work::Expression(spread.clone(), environment, depth + 1));
                            }
                        }
                        Expr::Tag { label, payload } => {
                            work.push(Work::Tag(label.clone()));
                            work.push(Work::Expression(payload.clone(), environment, depth + 1));
                        }
                        Expr::Apply { function, argument } => {
                            work.push(Work::Apply(depth + 1));
                            work.push(Work::Expression(argument.clone(), environment, depth + 1));
                            work.push(Work::Expression(function.clone(), environment, depth + 1));
                        }
                        Expr::Then { value, body } => {
                            work.push(Work::Then);
                            work.push(Work::Expression(body.clone(), environment, depth + 1));
                            work.push(Work::Expression(value.clone(), environment, depth + 1));
                        }
                        Expr::Match { scrutinee, arms } => {
                            work.push(Work::Match(arms.clone(), environment, depth));
                            work.push(Work::Expression(scrutinee.clone(), environment, depth + 1));
                        }
                    }
                }
                Work::Field(label) => {
                    let base = values.pop()?;
                    let ty = inference::unfold(self.aliases, &base.ty);
                    let row = match &*ty {
                        Ty::Struct(row) | Ty::Sum(row) => super::flattened(row),
                        _ => return None,
                    };
                    let ty = row.labels.get(&label)?.ty.clone();
                    let shape = match self.graph.exposed(base.shape) {
                        Shape::Record(fields) | Shape::Sum(fields) => *fields.get(&label)?,
                        Shape::Sealed => base.shape,
                        _ => return None,
                    };
                    values.push(Value {
                        ty,
                        shape,
                        supplied: Vec::new(),
                        known: base.known,
                    });
                }
                Work::Record(names, spread) => {
                    let count = names.len() + usize::from(spread);
                    let mut members = values
                        .split_off(values.len().checked_sub(count)?)
                        .into_iter();
                    let mut types = IndexMap::new();
                    let mut shapes = std::collections::BTreeMap::new();
                    let mut known = true;
                    if spread {
                        let spread = members.next()?;
                        known &= spread.known;
                        let ty = inference::unfold(self.aliases, &spread.ty);
                        let Ty::Struct(row) = &*ty else { return None };
                        let (labels, rest) = super::flattened(row).into_parts();
                        if !matches!(rest, Rest::Closed) {
                            return None;
                        }
                        types.extend(labels);
                        match self.graph.exposed(spread.shape) {
                            Shape::Record(fields) => shapes.extend(fields.clone()),
                            Shape::Value => {}
                            _ => return None,
                        }
                    }
                    for (name, value) in names.into_iter().zip(members) {
                        known &= value.known;
                        types.insert(
                            name.clone(),
                            crate::types::RowField {
                                presence: Presence::Present,
                                ty: value.ty,
                            },
                        );
                        shapes.insert(name, value.shape);
                    }
                    values.push(Value {
                        ty: Arc::new(Ty::Struct(Row {
                            labels: types,
                            rest: Rest::Closed,
                        })),
                        shape: self.graph.shape(Shape::Record(shapes)),
                        supplied: Vec::new(),
                        known,
                    });
                }
                Work::Tag(label) => {
                    let payload = values.pop()?;
                    let row = Row {
                        labels: IndexMap::from([(
                            label.clone(),
                            crate::types::RowField {
                                presence: Presence::Present,
                                ty: payload.ty,
                            },
                        )]),
                        rest: Rest::Closed,
                    };
                    values.push(Value {
                        known: payload.known,
                        ty: Arc::new(Ty::Sum(row)),
                        shape: self
                            .graph
                            .shape(Shape::Sum(std::collections::BTreeMap::from([(
                                label,
                                payload.shape,
                            )]))),
                        supplied: Vec::new(),
                    });
                }
                Work::Apply(depth) => {
                    let argument = values.pop()?;
                    let function = values.pop()?;
                    work.push(Work::Invoke(function, argument, depth));
                }
                Work::Then => {
                    let body = values.pop()?;
                    values.pop()?;
                    values.push(body);
                }
                Work::Match(arms, environment, depth) => {
                    let input = values.pop()?;
                    let mut selected = None;
                    for arm in arms.iter() {
                        if self.matches(&arm.pattern, &input.ty, depth + 1)? {
                            selected = Some(arm.body.clone());
                            break;
                        }
                    }
                    work.push(Work::Expression(selected?, environment, depth + 1));
                }
            }
        }
        (values.len() == 1).then(|| values.pop().unwrap())
    }

    fn matches(&mut self, pattern: &Pattern, ty: &Arc<Ty>, depth: usize) -> Option<bool> {
        let mut pending = vec![(pattern, ty.clone(), depth)];
        while let Some((pattern, ty, depth)) = pending.pop() {
            self.step(depth)?;
            if matches!(pattern, Pattern::Any) {
                continue;
            }
            let ty = inference::unfold(self.aliases, &ty);
            match (pattern, &*ty) {
                (Pattern::Record { fields, open }, Ty::Struct(row)) => {
                    let row = super::flattened(row);
                    for (name, pattern) in fields.iter().rev() {
                        let Some(field) = row.labels.get(name) else {
                            return matches!(row.rest, Rest::Closed).then_some(false);
                        };
                        match field.presence {
                            Presence::Absent => return Some(false),
                            Presence::Present => {}
                            _ => return None,
                        }
                        pending.push((pattern, field.ty.clone(), depth + 1));
                    }
                    if !open {
                        for (name, field) in &row.labels {
                            if fields.iter().any(|(field, _)| field == name) {
                                continue;
                            }
                            match field.presence {
                                Presence::Present => return Some(false),
                                Presence::Absent => {}
                                _ => return None,
                            }
                        }
                        if !matches!(row.rest, Rest::Closed) {
                            return None;
                        }
                    }
                }
                (Pattern::Tag { label, payload }, Ty::Sum(row)) => {
                    let row = super::flattened(row);
                    if !matches!(row.rest, Rest::Closed) {
                        return None;
                    }
                    let possible: Vec<_> = row
                        .labels
                        .iter()
                        .filter(|(_, field)| !matches!(field.presence, Presence::Absent))
                        .collect();
                    if possible.len() != 1 {
                        return None;
                    }
                    if possible[0].0 != label {
                        return Some(false);
                    }
                    pending.push((payload, possible[0].1.ty.clone(), depth + 1));
                }
                (_, Ty::Bound(_) | Ty::Var(_) | Ty::Undecided) => return None,
                _ => return Some(false),
            }
        }
        Some(true)
    }
}
