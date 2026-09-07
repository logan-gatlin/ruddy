//! Diagnostic value flow is independent of semantic inference. Replaying this
//! bounded DAG computation updates exact effect origins in cached constraints.
use super::*;

pub(super) fn refresh(
    program: &Program,
    signatures: &Signatures,
    constraints: &mut IndexMap<Symbol, Vec<Constraint>>,
) {
    let mut flow = Flow {
        program,
        signatures,
        env: HashMap::new(),
        values: HashMap::new(),
        scopes: Vec::new(),
        origins: HashMap::new(),
    };
    for group in &program.groups {
        if !group
            .members
            .iter()
            .any(|symbol| constraints.contains_key(symbol))
        {
            continue;
        }
        let mut published = Vec::new();
        for symbol in &group.members {
            let value = &program.terms[symbol].value;
            flow.visit(value);
            published.push((*symbol, flow.value(value)));
            flow.values.clear();
        }
        // Recursive uses have no published value-flow summary inside the SCC.
        flow.env.extend(published);
    }
    let mut work: Vec<_> = constraints.values_mut().flatten().collect();
    while let Some(constraint) = work.pop() {
        match &mut constraint.kind {
            ConstraintKind::Performs { effect_origins, .. }
            | ConstraintKind::Isolate { effect_origins, .. } => {
                *effect_origins = flow
                    .origins
                    .get(&constraint.at)
                    .cloned()
                    .unwrap_or_default();
            }
            ConstraintKind::Let { value, body, .. } => {
                work.extend(value);
                work.extend(body);
            }
            ConstraintKind::Match { arms, .. } => {
                for arm in arms {
                    work.extend(&mut arm.constraints);
                    work.push(&mut arm.result);
                }
            }
            _ => {}
        }
    }
}

struct Flow<'a> {
    program: &'a Program,
    signatures: &'a Signatures,
    env: HashMap<Symbol, EffectProvenance>,
    values: HashMap<Anchor, EffectProvenance>,
    scopes: Vec<Vec<EffectSource>>,
    origins: HashMap<Anchor, Vec<EffectSource>>,
}
impl Flow<'_> {
    fn value(&self, term: &ir::Term) -> EffectProvenance {
        self.values.get(&term.at).cloned().unwrap_or_default()
    }
    fn visit(&mut self, root: &ir::Term) {
        use ir::TermKind as T;
        enum Task<'a> {
            Enter(&'a ir::Term),
            Exit(&'a ir::Term),
            Bind(Symbol, &'a ir::Term),
        }
        let mut work = vec![Task::Enter(root)];
        while let Some(task) = work.pop() {
            crate::cancellation::checkpoint();
            let term = match task {
                Task::Bind(symbol, value) => {
                    self.env.insert(symbol, self.value(value));
                    continue;
                }
                Task::Enter(term) => {
                    work.push(Task::Exit(term));
                    match &term.kind {
                        T::Fn { arg, body } => {
                            self.env
                                .insert(arg.anchored, EffectProvenance::parameter(arg.anchored));
                            self.scopes.push(Vec::new());
                            work.push(Task::Enter(body));
                        }
                        T::Let {
                            name, value, body, ..
                        } => {
                            work.push(Task::Enter(body));
                            work.push(Task::Bind(name.anchored, value));
                            work.push(Task::Enter(value));
                        }
                        T::Unary { value, .. } | T::Raise(value) => work.push(Task::Enter(value)),
                        T::Binary { left, right, .. } => {
                            work.push(Task::Enter(right));
                            work.push(Task::Enter(left));
                        }
                        T::Apply { func, arg } => {
                            work.push(Task::Enter(arg));
                            work.push(Task::Enter(func));
                        }
                        T::Struct { fields, spread } => {
                            if let Some(spread) = spread {
                                work.push(Task::Enter(&spread.value));
                            }
                            work.extend(
                                fields.values().rev().map(|field| Task::Enter(&field.value)),
                            );
                        }
                        T::Array(items) => {
                            work.extend(items.iter().rev().map(|item| Task::Enter(&item.value)))
                        }
                        T::Tag {
                            payload: Some(payload),
                            ..
                        } => work.push(Task::Enter(payload)),
                        T::Project { base, .. } => work.push(Task::Enter(base)),
                        T::Match { scrutinee, arms } => {
                            work.extend(arms.iter().rev().map(|(_, body)| Task::Enter(body)));
                            work.push(Task::Enter(scrutinee));
                        }
                        T::Handle { body, handler } => {
                            if let Some(ret) = &handler.ret {
                                work.push(Task::Enter(&ret.body));
                            }
                            work.extend(
                                handler.arms.iter().rev().map(|arm| Task::Enter(&arm.body)),
                            );
                            work.push(Task::Enter(body));
                        }
                        _ => {}
                    }
                    continue;
                }
                Task::Exit(term) => term,
            };
            let value = match &term.kind {
                T::Ident(symbol) => self.env.get(symbol).cloned().unwrap_or_default(),
                T::Let { body, .. } => self.value(body),
                T::Fn { arg, body } => {
                    let callable = self.scopes.pop().expect("function value flow scope");
                    self.origins.insert(term.at, callable.clone());
                    EffectProvenance::function(callable, arg.anchored, self.value(body))
                }
                T::Apply { func, arg } => {
                    let func = self.value(func);
                    let resolved = match func.argument {
                        Some(symbol) => func.substitute(symbol, &self.value(arg)),
                        None => func,
                    };
                    let origins = match &resolved.value_parameter {
                        Some(parameter) => vec![EffectSource::Parameter {
                            symbol: parameter.symbol,
                            path: parameter.path.clone(),
                        }],
                        None => resolved.callable.clone(),
                    };
                    if let Some(scope) = self.scopes.last_mut() {
                        for origin in &origins {
                            if !scope.contains(origin) {
                                scope.push(origin.clone());
                            }
                        }
                    }
                    self.origins.insert(term.at, origins);
                    resolved.call_result().unwrap_or_default()
                }
                T::Struct { fields, .. } => EffectProvenance::from_fields(
                    fields
                        .iter()
                        .filter_map(|(name, field)| {
                            self.values
                                .get(&field.value.at)
                                .map(|value| (name.clone(), value.clone()))
                        })
                        .collect(),
                ),
                T::Project { base, field } => self
                    .value(base)
                    .projected(field.anchored.clone())
                    .unwrap_or_default(),
                T::Operation { effect, .. } => self
                    .program
                    .effect_ids
                    .get(&effect.anchored)
                    .zip(
                        self.signatures
                            .effect_declaration_spans
                            .get(&effect.anchored),
                    )
                    .map(|(interface, at)| {
                        EffectProvenance::origin(EffectOrigin {
                            symbol: effect.anchored,
                            interface: interface.clone(),
                            declaration_at: *at,
                        })
                    })
                    .unwrap_or_default(),
                _ => EffectProvenance::default(),
            };
            if value != EffectProvenance::default() {
                self.values.insert(term.at, value);
            }
        }
    }
}
