//! Evidence belongs to invocation. Constructing an adapter captures values and
//! available evidence but never evaluates the adapted Ruddy computation.
use super::*;
use crate::reification::conventions::{Shape as CallableShape, ShapeId};
use std::collections::{BTreeMap, HashSet};

type AdapterKey = (String, ShapeId, String, ShapeId, Vec<u32>);

struct ReifiedAdapter {
    key: AdapterKey,
    function: FuncId,
}

impl Lower<'_> {
    fn adapter_key(
        &self,
        want: &Arc<Ty>,
        want_shape: ShapeId,
        have: &Arc<Ty>,
        have_shape: ShapeId,
        captures: &[u32],
    ) -> Option<AdapterKey> {
        let want = crate::ir::representation_key(want, self.inference.aliases())?;
        let have = crate::ir::representation_key(have, self.inference.aliases())?;
        Some((
            want,
            self.reification.callables.graph.exposed_id(want_shape),
            have,
            self.reification.callables.graph.exposed_id(have_shape),
            captures.to_vec(),
        ))
    }

    fn capture_descriptors(&mut self, owner: usize, parameters: &[u32]) {
        for parameter in parameters {
            let at = self.frames[..=owner]
                .iter()
                .rposition(|frame| frame.representations.contains_key(parameter))
                .expect("available captured descriptor");
            let value = self.thread(at, self.frames[at].representations[parameter]);
            self.top().representations.insert(*parameter, value);
        }
    }

    pub(super) fn callable_shape(&self, at: Anchor) -> ShapeId {
        self.reification.callables.occurrences[&at].value
    }

    pub(super) fn callable_member(&self, id: ShapeId, name: &str) -> ShapeId {
        match self.reification.callables.graph.exposed(id) {
            CallableShape::Record(fields) | CallableShape::Sum(fields) => {
                fields.get(name).copied().unwrap_or(self.erased_callable)
            }
            _ => self.erased_callable,
        }
    }

    pub(super) fn callable_element(&self, id: ShapeId) -> ShapeId {
        match self.reification.callables.graph.exposed(id) {
            CallableShape::Array(element) | CallableShape::Cell(element) => *element,
            _ => self.erased_callable,
        }
    }

    pub(super) fn callable_slots(&self, id: ShapeId) -> BTreeMap<u32, bool> {
        let mut slots = BTreeMap::new();
        if let CallableShape::Arrow { needs, .. } = self.reification.callables.graph.exposed(id) {
            for need in &self.requirements[*needs as usize] {
                let mandatory = slots.entry(need.parameter).or_insert(false);
                *mandatory |= need.port.is_none();
            }
        }
        slots
    }

    pub(super) fn callable_children(&self, id: ShapeId) -> (ShapeId, ShapeId) {
        match self.reification.callables.graph.exposed(id) {
            CallableShape::Arrow {
                argument, result, ..
            } => (*argument, *result),
            _ => (self.erased_callable, self.erased_callable),
        }
    }

    pub(super) fn callable_demands(&self, id: ShapeId) -> bool {
        let mut seen = HashSet::new();
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            match &self.reification.callables.graph.shapes[id as usize] {
                CallableShape::Arrow {
                    argument,
                    result,
                    needs,
                } => {
                    if !self.requirements[*needs as usize].is_empty() {
                        return true;
                    }
                    pending.extend([*argument, *result]);
                }
                CallableShape::Alias(inner)
                | CallableShape::Array(inner)
                | CallableShape::Cell(inner) => pending.push(*inner),
                CallableShape::Record(fields) | CallableShape::Sum(fields) => {
                    pending.extend(fields.values())
                }
                _ => {}
            }
        }
        false
    }

    pub(super) fn callable_params(&mut self, id: ShapeId, params: &mut Vec<Param>) {
        for parameter in self.callable_slots(id).keys() {
            let temp = self.fresh(Rep::TypeDescriptor);
            self.top().representations.insert(*parameter, temp);
            params.push(Param {
                temp,
                rep: Rep::TypeDescriptor,
            });
        }
    }

    pub(super) fn absent_representation(&mut self, body: &mut Body) -> Temp {
        // A missing conditional slot has the same absence representation as
        // a missing conditional effect record. Actual reflection still demands
        // an authentic descriptor before it can construct an Any package.
        let empty = self.emit(
            body,
            Span::default(),
            Rep::Struct,
            Op::Struct(IndexMap::new()),
        );
        self.emit(
            body,
            Span::default(),
            Rep::TypeDescriptor,
            Op::Project {
                base: empty,
                field: FieldKey::named("type".to_owned()),
            },
        )
    }

    pub(super) fn callable_args(&mut self, id: ShapeId, body: &mut Body) -> Vec<Temp> {
        self.callable_slots(id)
            .keys()
            .map(|parameter| self.representation(&Arc::new(Ty::Bound(*parameter)), body))
            .collect()
    }

    pub(super) fn reified_fitted(
        &mut self,
        want: &Arc<Ty>,
        want_shape: ShapeId,
        have: &Arc<Ty>,
        have_shape: ShapeId,
        temp: Temp,
        body: &mut Body,
    ) -> Temp {
        self.reified_with(
            want,
            want_shape,
            have,
            have_shape,
            temp,
            body,
            &mut Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reified_with(
        &mut self,
        want: &Arc<Ty>,
        want_shape: ShapeId,
        have: &Arc<Ty>,
        have_shape: ShapeId,
        temp: Temp,
        body: &mut Body,
        adapters: &mut Vec<ReifiedAdapter>,
    ) -> Temp {
        if matches!(
            self.reification.callables.graph.exposed(want_shape),
            CallableShape::Lazy
        ) || matches!(
            self.reification.callables.graph.exposed(have_shape),
            CallableShape::Lazy
        ) {
            // Unobserved components are forwarded with the convention supplied
            // by the caller. No code in this function inspects their layout.
            let value = self.fitted(want, have, temp, body);
            self.callable_held[value as usize] = Some(
                if matches!(
                    self.reification.callables.graph.exposed(want_shape),
                    CallableShape::Lazy
                ) {
                    have_shape
                } else {
                    want_shape
                },
            );
            return value;
        }
        if matches!(self.rep(want), Rep::Struct | Rep::Array | Rep::Sum)
            && self.rep(want) == self.rep(have)
            && (want_shape != have_shape || !same_finite_syntax(want, have))
            && (self.callable_demands(want_shape) || self.callable_demands(have_shape))
        {
            return self
                .reified_container(want, want_shape, have, have_shape, temp, body, adapters);
        }
        if (!self.callable_demands(want_shape) && !self.callable_demands(have_shape))
            || self.rep(want) != Rep::Fn
            || self.rep(have) != Rep::Fn
        {
            let value = self.fitted(want, have, temp, body);
            let profile = if matches!(self.rep(have), Rep::Struct | Rep::Array | Rep::Sum)
                && self.rep(have) == self.rep(want)
            {
                have_shape
            } else {
                want_shape
            };
            self.callable_held[value as usize] = Some(profile);
            return value;
        }
        if want_shape == have_shape && same_finite_syntax(want, have) {
            self.hold(temp, want);
            self.callable_held[temp as usize] = Some(want_shape);
            return temp;
        }
        let (want_from, want_to, want_row) = self.arrow(want);
        let (have_from, have_to, have_row) = self.arrow(have);
        let (want_arg_shape, want_result_shape) = self.callable_children(want_shape);
        let (have_arg_shape, have_result_shape) = self.callable_children(have_shape);
        let supplied = crate::reification::instantiate(have, want, self.inference.aliases());
        let reverse = crate::reification::instantiate(want, have, self.inference.aliases());
        let wanted_slots = self.callable_slots(want_shape);
        let offered_slots = self.callable_slots(have_shape);

        let captured_parameters: Vec<_> = crate::reification::parameters(want)
            .into_iter()
            .filter(|parameter| {
                !wanted_slots.contains_key(parameter)
                    && self
                        .frames
                        .iter()
                        .any(|frame| frame.representations.contains_key(parameter))
            })
            .collect();
        let key = self.adapter_key(want, want_shape, have, have_shape, &captured_parameters);
        if let Some(adapter) = key
            .as_ref()
            .and_then(|key| adapters.iter().find(|adapter| adapter.key == *key))
        {
            let function = adapter.function;
            let mut captures = vec![temp];
            captures.extend(
                captured_parameters
                    .iter()
                    .map(|parameter| self.representation(&Arc::new(Ty::Bound(*parameter)), body)),
            );
            let value = self.emit(
                body,
                Span::default(),
                Rep::Fn,
                Op::Closure {
                    func: function,
                    captures,
                },
            );
            self.hold(value, want);
            self.callable_held[value as usize] = Some(want_shape);
            return value;
        }
        let name = self.lifted_name();
        let id = self.slot(name.clone());
        if let Some(key) = key {
            adapters.push(ReifiedAdapter { key, function: id });
        }
        let owner = self.frames.len() - 1;
        self.frames.push(Frame::default());
        let wrapped = self.thread(owner, temp);
        self.capture_descriptors(owner, &captured_parameters);
        let mut params = Vec::new();
        self.callable_params(want_shape, &mut params);
        self.evidence_params(&want_row, &mut params);
        let arg = self.fresh(self.rep(&want_from));
        self.hold(arg, &want_from);
        self.callable_held[arg as usize] = Some(want_arg_shape);
        params.push(Param {
            temp: arg,
            rep: self.rep(&want_from),
        });
        let mut lifted = Body::default();
        let mut args = Vec::new();
        for (parameter, mandatory) in offered_slots {
            // The incoming convention may quantify the entire array/record,
            // while the offered callback quantifies one of its components.
            // Their bound indices belong to different schemes; coincident
            // numeric indices must never stand in for this decomposition.
            if !supplied.contains_key(&parameter) {
                let projection = wanted_slots.keys().find_map(|incoming| {
                    let ty = reverse.get(incoming)?;
                    crate::reification::projection(ty, parameter, self.inference.aliases())
                        .map(|path| (*incoming, path))
                });
                if let Some((incoming, path)) = projection {
                    let descriptor =
                        self.representation(&Arc::new(Ty::Bound(incoming)), &mut lifted);
                    args.push(self.emit(
                        &mut lifted,
                        Span::default(),
                        Rep::TypeDescriptor,
                        Op::TypeProjection { descriptor, path },
                    ));
                    continue;
                }
            }
            let ty = supplied
                .get(&parameter)
                .cloned()
                .unwrap_or_else(|| Arc::new(Ty::Bound(parameter)));
            let used = mandatory
                || crate::reification::parameters(&ty)
                    .iter()
                    .any(|parameter| wanted_slots.contains_key(parameter));
            args.push(if used {
                self.representation(&ty, &mut lifted)
            } else {
                self.absent_representation(&mut lifted)
            });
        }
        args.extend(self.adapter_effect_args(&have_row, &mut lifted));
        let arg = self.reified_with(
            &have_from,
            have_arg_shape,
            &want_from,
            want_arg_shape,
            arg,
            &mut lifted,
            adapters,
        );
        args.push(arg);
        let value = self.emit(
            &mut lifted,
            Span::default(),
            self.rep(&have_to),
            Op::Call {
                callee: Callee::Indirect(wrapped),
                args,
            },
        );
        self.contain(value, &have_to);
        self.hold(value, &have_to);
        self.callable_held[value as usize] = Some(have_result_shape);
        let value = self.reified_with(
            &want_to,
            want_result_shape,
            &have_to,
            have_result_shape,
            value,
            &mut lifted,
            adapters,
        );
        let frame = self.frames.pop().expect("callable adapter frame");
        let params = Self::with_captures(&frame, params);
        let captures = frame.captures.iter().map(|capture| capture.outer).collect();
        self.fill(
            id,
            Function {
                name,
                params,
                body: lifted.seal(Terminator {
                    span: Span::default(),
                    kind: End::Ret(value),
                }),
                span: Span::default(),
            },
        );
        let result = self.emit(
            body,
            Span::default(),
            Rep::Fn,
            Op::Closure { func: id, captures },
        );
        self.hold(result, want);
        self.callable_held[result as usize] = Some(want_shape);
        result
    }
    /// An adapter transports the evidence its caller supplies. Looking up
    /// ambient handlers here would capture the construction context instead
    /// and give recursive adapter instances different capture layouts.
    fn adapter_effect_args(&mut self, declared: &Row, body: &mut Body) -> Vec<Temp> {
        let records = self.top().evidence.clone();
        let tail = self.top().tails.last().map(|(_, temp)| *temp);
        let mut args = Vec::new();
        for name in shape(declared).names {
            args.push(if let Some(value) = records.get(&name) {
                *value
            } else {
                self.emit(
                    body,
                    Span::default(),
                    Rep::Struct,
                    Op::Project {
                        base: tail.expect("the incoming callable row covers every offered effect"),
                        field: FieldKey::named(name),
                    },
                )
            });
        }
        if tail_key(declared).is_some() {
            let bundle = if matches!(declared.rest, Rest::Closed) {
                let mut selected = IndexMap::new();
                for (name, field) in &declared.labels {
                    if matches!(field.presence, Presence::Present | Presence::Absent) {
                        continue;
                    }
                    let value = if let Some(value) = records.get(name) {
                        Some(*value)
                    } else {
                        tail.map(|base| {
                            self.emit(
                                body,
                                Span::default(),
                                Rep::Struct,
                                Op::Project {
                                    base,
                                    field: FieldKey::named(name.clone()),
                                },
                            )
                        })
                    };
                    if let Some(value) = value {
                        selected.insert(FieldKey::named(name.clone()), value);
                    }
                }
                self.emit(body, Span::default(), Rep::Struct, Op::Struct(selected))
            } else {
                match (tail, records.is_empty()) {
                    (Some(tail), true) => tail,
                    (tail, _) => {
                        let named = self.emit(
                            body,
                            Span::default(),
                            Rep::Struct,
                            Op::Struct(named_fields(records)),
                        );
                        if let Some(tail) = tail {
                            self.emit(
                                body,
                                Span::default(),
                                Rep::Struct,
                                Op::Merge(vec![tail, named]),
                            )
                        } else {
                            named
                        }
                    }
                }
            };
            args.push(bundle);
        }
        args
    }

    #[allow(clippy::too_many_arguments)]
    fn reified_container(
        &mut self,
        want: &Arc<Ty>,
        want_shape: ShapeId,
        have: &Arc<Ty>,
        have_shape: ShapeId,
        temp: Temp,
        body: &mut Body,
        adapters: &mut Vec<ReifiedAdapter>,
    ) -> Temp {
        let captured: Vec<_> = crate::reification::parameters(want)
            .union(&crate::reification::parameters(have))
            .copied()
            .filter(|p| {
                self.frames
                    .iter()
                    .any(|f| f.representations.contains_key(p))
            })
            .collect();
        let key = self.adapter_key(want, want_shape, have, have_shape, &captured);
        let existing = key
            .as_ref()
            .and_then(|key| adapters.iter().find(|a| a.key == *key))
            .map(|a| a.function);
        let function = if let Some(function) = existing {
            function
        } else {
            let name = self.lifted_name();
            let function = self.slot(name.clone());
            if let Some(key) = key {
                adapters.push(ReifiedAdapter { key, function });
            }
            let owner = self.frames.len() - 1;
            self.frames.push(Frame::default());
            let source = self.thread(owner, temp);
            self.capture_descriptors(owner, &captured);
            let mut lifted = Body::default();
            let result = match self.rep(have) {
                Rep::Array => {
                    let have_element = self.array_element(have).unwrap();
                    let want_element = self.array_element(want).unwrap();
                    let have_profile = self.callable_element(have_shape);
                    let want_profile = self.callable_element(want_shape);
                    let empty = self.child(Span::default(), |_, _| source);
                    let nonempty = self.child(Span::default(), |low, inner| {
                        let head = low.emit(
                            inner,
                            Span::default(),
                            low.rep(&have_element),
                            Op::Nth {
                                base: source,
                                index: 0,
                            },
                        );
                        let head = low.reified_with(
                            &want_element,
                            want_profile,
                            &have_element,
                            have_profile,
                            head,
                            inner,
                            adapters,
                        );
                        let tail = low.emit(
                            inner,
                            Span::default(),
                            Rep::Array,
                            Op::Slice {
                                base: source,
                                start: 1,
                                drop: 0,
                            },
                        );
                        let tail = low.reified_with(
                            want, want_shape, have, have_shape, tail, inner, adapters,
                        );
                        let head =
                            low.emit(inner, Span::default(), Rep::Array, Op::Array(vec![head]));
                        low.emit(
                            inner,
                            Span::default(),
                            Rep::Array,
                            Op::Concat(vec![head, tail]),
                        )
                    });
                    self.emit(
                        &mut lifted,
                        Span::default(),
                        Rep::Array,
                        Op::SwitchLen {
                            on: source,
                            cases: vec![LenCase {
                                len: 0,
                                block: empty,
                            }],
                            beyond: Box::new(nonempty),
                        },
                    )
                }
                Rep::Struct => {
                    let Ty::Struct(row) = &*self.erased(have) else {
                        unreachable!()
                    };
                    let mut result = source;
                    for (name, field) in std::mem::take(&mut flat(row).labels) {
                        if matches!(field.presence, Presence::Absent) {
                            continue;
                        }
                        let have_profile = self.callable_member(have_shape, &name);
                        let want_profile = self.callable_member(want_shape, &name);
                        if !self.callable_demands(have_profile)
                            && !self.callable_demands(want_profile)
                        {
                            continue;
                        }
                        let want_type = self.member_of(want, &name).unwrap_or(field.ty.clone());
                        let present = self.child(Span::default(), |low, inner| {
                            let member = low.emit(
                                inner,
                                Span::default(),
                                low.rep(&field.ty),
                                Op::Project {
                                    base: source,
                                    field: FieldKey::named(name.clone()),
                                },
                            );
                            let member = low.reified_with(
                                &want_type,
                                want_profile,
                                &field.ty,
                                have_profile,
                                member,
                                inner,
                                adapters,
                            );
                            let over = low.emit(
                                inner,
                                Span::default(),
                                Rep::Struct,
                                Op::Struct(IndexMap::from([(
                                    FieldKey::named(name.clone()),
                                    member,
                                )])),
                            );
                            low.emit(
                                inner,
                                Span::default(),
                                Rep::Struct,
                                Op::Merge(vec![result, over]),
                            )
                        });
                        let absent = self.child(Span::default(), |_, _| result);
                        result = self.emit(
                            &mut lifted,
                            Span::default(),
                            Rep::Struct,
                            Op::SwitchPresence {
                                on: source,
                                field: name,
                                present: Box::new(present),
                                absent: Box::new(absent),
                            },
                        );
                    }
                    result
                }
                Rep::Sum => {
                    let Ty::Sum(row) = &*self.erased(have) else {
                        unreachable!()
                    };
                    let mut cases = Vec::new();
                    for (name, field) in std::mem::take(&mut flat(row).labels) {
                        if matches!(field.presence, Presence::Absent) {
                            continue;
                        }
                        let have_profile = self.callable_member(have_shape, &name);
                        let want_profile = self.callable_member(want_shape, &name);
                        if !self.callable_demands(have_profile)
                            && !self.callable_demands(want_profile)
                        {
                            continue;
                        }
                        let want_type = self.member_of(want, &name).unwrap_or(field.ty.clone());
                        let block = self.child(Span::default(), |low, inner| {
                            let member = low.emit(
                                inner,
                                Span::default(),
                                low.rep(&field.ty),
                                Op::Payload(source),
                            );
                            let member = low.reified_with(
                                &want_type,
                                want_profile,
                                &field.ty,
                                have_profile,
                                member,
                                inner,
                                adapters,
                            );
                            low.emit(
                                inner,
                                Span::default(),
                                Rep::Sum,
                                Op::Tag {
                                    name: name.clone(),
                                    payload: Some(member),
                                },
                            )
                        });
                        cases.push(TagCase { name, block });
                    }
                    let fallback = self.child(Span::default(), |_, _| source);
                    self.emit(
                        &mut lifted,
                        Span::default(),
                        Rep::Sum,
                        Op::SwitchTag {
                            on: source,
                            cases,
                            fallback: Some(Box::new(fallback)),
                        },
                    )
                }
                _ => unreachable!(),
            };
            let frame = self.frames.pop().unwrap();
            let params = Self::with_captures(&frame, Vec::new());
            self.fill(
                function,
                Function {
                    name,
                    params,
                    body: lifted.seal(Terminator {
                        span: Span::default(),
                        kind: End::Ret(result),
                    }),
                    span: Span::default(),
                },
            );
            function
        };
        let mut captures = vec![temp];
        for parameter in captured {
            captures.push(self.representation(&Arc::new(Ty::Bound(parameter)), body));
        }
        let adapter = self.emit(
            body,
            Span::default(),
            Rep::Fn,
            Op::Closure {
                func: function,
                captures,
            },
        );
        let result = self.emit(
            body,
            Span::default(),
            self.rep(want),
            Op::Call {
                callee: Callee::Indirect(adapter),
                args: Vec::new(),
            },
        );
        self.contain(result, want);
        self.callable_held[result as usize] = Some(want_shape);
        result
    }
}
