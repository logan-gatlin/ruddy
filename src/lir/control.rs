//! Bundle-local construction of closed CPS blocks. The structured construction
//! form is private to lowering and never crosses the LIR or artifact interface.
use super::*;
use std::collections::{BTreeSet, HashMap};

pub(super) fn lower(source: lower::Output) -> Output {
    let mut functions = Vec::new();
    let mut next = 0;
    let mut reps = HashMap::new();
    for function in &source.functions {
        for param in &function.params {
            reps.insert(param.temp, rep(param.rep));
            next = next.max(param.temp + 1);
        }
        collect(&function.body, &mut reps, &mut next);
    }
    for global in &source.globals {
        collect(&global.body, &mut reps, &mut next);
    }
    for function in source.functions {
        let id = functions.len();
        functions.push(build(
            id,
            function.name,
            function.params,
            function.body,
            function.span,
            &mut reps,
            &mut next,
        ));
    }
    let mut globals = Vec::new();
    for global in source.globals {
        let initializer = functions.len();
        functions.push(build(
            initializer,
            format!("{}#init", global.name),
            vec![],
            global.body,
            global.span,
            &mut reps,
            &mut next,
        ));
        globals.push(Global {
            type_interface: None,
            adapter: None,
            callable: None,
            symbol: global.symbol,
            name: global.name,
            initializer,
            span: global.span,
        });
    }
    Output {
        functions,
        globals,
        externs: source
            .externs
            .into_iter()
            .map(|e| Extern {
                symbol: e.symbol,
                name: e.name,
                target: e.target,
                span: e.span,
                rep: rep(e.rep),
            })
            .collect(),
    }
}

fn collect(block: &lower::Block, reps: &mut HashMap<Temp, Rep>, next: &mut Temp) {
    let mut pending = vec![block];
    while let Some(block) = pending.pop() {
        for i in &block.instrs {
            reps.insert(
                i.temp,
                if matches!(i.op, lower::Op::NewTag) {
                    Rep::Handler
                } else {
                    rep(i.rep)
                },
            );
            *next = (*next).max(i.temp + 1);
            match &i.op {
                lower::Op::Catch { body, .. } => pending.push(body),
                lower::Op::SwitchTag {
                    cases, fallback, ..
                } => {
                    pending.extend(cases.iter().map(|c| &c.block));
                    pending.extend(fallback.as_deref());
                }
                lower::Op::SwitchPrim {
                    cases, fallback, ..
                } => {
                    pending.extend(cases.iter().map(|c| &c.block));
                    pending.extend(fallback.as_deref());
                }
                lower::Op::SwitchPresence {
                    present, absent, ..
                } => {
                    pending.push(present);
                    pending.push(absent);
                }
                lower::Op::SwitchRest { none, some, .. } => {
                    pending.push(none);
                    pending.push(some);
                }
                lower::Op::SwitchLen { cases, beyond, .. } => {
                    pending.extend(cases.iter().map(|c| &c.block));
                    pending.push(beyond);
                }
                _ => {}
            }
        }
    }
}

fn build(
    id: FuncId,
    name: String,
    params: Vec<lower::Param>,
    body: lower::Block,
    span: Span,
    reps: &mut HashMap<Temp, Rep>,
    next: &mut Temp,
) -> Function {
    let continuation = *next;
    *next += 1;
    reps.insert(continuation, Rep::Cont);
    let mut builder = Builder {
        id,
        blocks: vec![],
        reps,
        next,
    };
    let entry = builder.sequence(&body.instrs, &body.end, Finish::Return(continuation));
    Function {
        suspension: Suspension::MaySuspend,
        name,
        params: params
            .into_iter()
            .map(|p| Param {
                temp: p.temp,
                rep: rep(p.rep),
            })
            .collect(),
        continuation,
        entry,
        blocks: builder.blocks,
        span,
    }
}
#[derive(Clone, Copy)]
enum Finish {
    Return(Temp),
    Join { block: BlockId, result: Temp },
    Leave(Temp),
}
struct Builder<'a> {
    id: FuncId,
    blocks: Vec<Block>,
    reps: &'a mut HashMap<Temp, Rep>,
    next: &'a mut Temp,
}
impl Builder<'_> {
    fn edge(&self, block: BlockId) -> Edge {
        Edge {
            block,
            args: self.blocks[block].params.iter().map(|p| p.temp).collect(),
        }
    }
    fn finish(&self, finish: Finish, value: Temp) -> End {
        match finish {
            Finish::Return(continuation) => End::Continue {
                continuation,
                value,
            },
            Finish::Leave(tag) => End::Leave { tag, value },
            Finish::Join { block, result } => {
                let mut edge = self.edge(block);
                for v in &mut edge.args {
                    if *v == result {
                        *v = value;
                    }
                }
                End::Jump(edge)
            }
        }
    }
    fn block(&mut self, instrs: Vec<Instr>, kind: End, span: Span) -> BlockId {
        let mut free = BTreeSet::new();
        let mut defined = BTreeSet::new();
        for i in &instrs {
            for t in i.op.uses() {
                if !defined.contains(&t) {
                    free.insert(t);
                }
            }
            defined.insert(i.temp);
        }
        for t in kind.uses() {
            if !defined.contains(&t) {
                free.insert(t);
            }
        }
        let params = free
            .into_iter()
            .map(|temp| Param {
                temp,
                rep: *self.reps.get(&temp).unwrap_or(&Rep::Any),
            })
            .collect();
        let id = self.blocks.len();
        self.blocks.push(Block {
            params,
            result: None,
            instrs,
            end: Terminator { span, kind },
        });
        id
    }
    fn result(&mut self, block: BlockId, result: Temp) {
        let b = &mut self.blocks[block];
        b.params.retain(|p| p.temp != result);
        b.params.push(Param {
            temp: result,
            rep: *self.reps.get(&result).unwrap_or(&Rep::Any),
        });
        b.result = Some(result);
    }
    fn continuation(&mut self, finish: Finish, instrs: &mut Vec<Instr>) -> Temp {
        if let Finish::Return(k) = finish {
            return k;
        }
        let (block, result) = match finish {
            Finish::Join { block, result } => (block, result),
            Finish::Leave(tag) => {
                let result = *self.next;
                *self.next += 1;
                self.reps.insert(result, Rep::Any);
                (
                    self.block(vec![], End::Leave { tag, value: result }, Span::default()),
                    result,
                )
            }
            Finish::Return(_) => unreachable!(),
        };
        self.result(block, result);
        let params = &self.blocks[block].params;
        let captures = params[..params.len() - 1].iter().map(|p| p.temp).collect();
        let temp = *self.next;
        *self.next += 1;
        self.reps.insert(temp, Rep::Cont);
        instrs.push(Instr {
            temp,
            rep: Rep::Cont,
            span: Span::default(),
            op: Op::Continuation {
                code: CodeRef {
                    function: self.id,
                    block,
                },
                captures,
            },
        });
        temp
    }
    fn branch(&mut self, test: Test, yes: BlockId, no: BlockId, span: Span) -> BlockId {
        self.block(
            vec![],
            End::Branch {
                test,
                yes: self.edge(yes),
                no: self.edge(no),
            },
            span,
        )
    }
    fn sequence(
        &mut self,
        source: &[lower::Instr],
        end: &lower::Terminator,
        finish: Finish,
    ) -> BlockId {
        let mut instrs = Vec::new();
        for (offset, i) in source.iter().enumerate() {
            if matches!(
                i.op,
                lower::Op::Call { .. }
                    | lower::Op::RawCall { .. }
                    | lower::Op::Catch { .. }
                    | lower::Op::SwitchTag { .. }
                    | lower::Op::SwitchPrim { .. }
                    | lower::Op::SwitchPresence { .. }
                    | lower::Op::SwitchRest { .. }
                    | lower::Op::SwitchLen { .. }
            ) {
                let tail = offset + 1 == source.len()
                    && matches!(end.kind,lower::End::Ret(v)|lower::End::Yield(v) if v==i.temp);
                let destination = if tail {
                    finish
                } else {
                    let block = self.sequence(&source[offset + 1..], end, finish);
                    self.result(block, i.temp);
                    Finish::Join {
                        block,
                        result: i.temp,
                    }
                };
                let kind = match &i.op {
                    lower::Op::Call { callee, args } => End::Call {
                        callee: match callee {
                            lower::Callee::Direct(f) => Callee::Direct(*f),
                            lower::Callee::Indirect(t) => Callee::Indirect(*t),
                        },
                        args: args.clone(),
                        continuation: self.continuation(destination, &mut instrs),
                    },
                    lower::Op::RawCall {
                        callee,
                        args,
                        completion,
                    } => End::RawCall {
                        callee: *callee,
                        args: args.clone(),
                        completion: *completion,
                        continuation: self.continuation(destination, &mut instrs),
                    },
                    lower::Op::Catch { tag, body } => {
                        let continuation = self.continuation(destination, &mut instrs);
                        let target = self.sequence(&body.instrs, &body.end, Finish::Leave(*tag));
                        End::Enter {
                            tag: *tag,
                            body: self.edge(target),
                            continuation,
                        }
                    }
                    lower::Op::SwitchTag {
                        on,
                        cases,
                        fallback,
                    } => {
                        let mut no = if let Some(b) = fallback {
                            self.sequence(&b.instrs, &b.end, destination)
                        } else {
                            self.block(vec![], End::Unreachable, i.span)
                        };
                        for case in cases.iter().rev() {
                            let yes =
                                self.sequence(&case.block.instrs, &case.block.end, destination);
                            no = self.branch(
                                Test::Tag {
                                    on: *on,
                                    name: case.name.clone(),
                                },
                                yes,
                                no,
                                i.span,
                            );
                        }
                        End::Jump(self.edge(no))
                    }
                    lower::Op::SwitchPrim {
                        on,
                        cases,
                        fallback,
                    } => {
                        let mut no = if let Some(b) = fallback {
                            self.sequence(&b.instrs, &b.end, destination)
                        } else {
                            self.block(vec![], End::Unreachable, i.span)
                        };
                        for case in cases.iter().rev() {
                            let yes =
                                self.sequence(&case.block.instrs, &case.block.end, destination);
                            no = self.branch(
                                Test::Literal {
                                    on: *on,
                                    value: case.value.clone(),
                                },
                                yes,
                                no,
                                i.span,
                            );
                        }
                        End::Jump(self.edge(no))
                    }
                    lower::Op::SwitchPresence {
                        on,
                        field,
                        present,
                        absent,
                    } => {
                        let yes = self.sequence(&present.instrs, &present.end, destination);
                        let no = self.sequence(&absent.instrs, &absent.end, destination);
                        End::Branch {
                            test: Test::Presence {
                                on: *on,
                                field: field.clone(),
                            },
                            yes: self.edge(yes),
                            no: self.edge(no),
                        }
                    }
                    lower::Op::SwitchRest {
                        on,
                        fields,
                        none,
                        some,
                    } => {
                        let yes = self.sequence(&none.instrs, &none.end, destination);
                        let no = self.sequence(&some.instrs, &some.end, destination);
                        End::Branch {
                            test: Test::Rest {
                                on: *on,
                                fields: fields.clone(),
                            },
                            yes: self.edge(yes),
                            no: self.edge(no),
                        }
                    }
                    lower::Op::SwitchLen { on, cases, beyond } => {
                        let mut no = self.sequence(&beyond.instrs, &beyond.end, destination);
                        for case in cases.iter().rev() {
                            let yes =
                                self.sequence(&case.block.instrs, &case.block.end, destination);
                            no = self.branch(
                                Test::Length {
                                    on: *on,
                                    length: case.len,
                                },
                                yes,
                                no,
                                i.span,
                            );
                        }
                        End::Jump(self.edge(no))
                    }
                    _ => unreachable!(),
                };
                return self.block(instrs, kind, i.span);
            }
            instrs.push(Instr {
                temp: i.temp,
                rep: *self.reps.get(&i.temp).unwrap_or(&Rep::Any),
                span: i.span,
                op: ordinary(&i.op),
            });
        }
        let kind = match end.kind {
            lower::End::Ret(v) | lower::End::Yield(v) => self.finish(finish, v),
            lower::End::Throw { tag, value } => End::Abort { tag, value },
        };
        self.block(instrs, kind, end.span)
    }
}
fn key(k: &lower::FieldKey) -> FieldKey {
    match k {
        lower::FieldKey::Named(s) => FieldKey::Named(s.clone()),
        lower::FieldKey::UnnamedOperation => FieldKey::UnnamedOperation,
    }
}
fn rep(r: lower::Rep) -> Rep {
    match r {
        lower::Rep::Nat => Rep::Nat,
        lower::Rep::Int => Rep::Int,
        lower::Rep::Fixed(kind) => Rep::Fixed(kind),
        lower::Rep::Real => Rep::Real,
        lower::Rep::String => Rep::String,
        lower::Rep::Boolean => Rep::Boolean,
        lower::Rep::TypeDescriptor => Rep::TypeDescriptor,
        lower::Rep::NativePlan => Rep::NativePlan,
        lower::Rep::BoxedAny => Rep::BoxedAny,
        lower::Rep::HostValue => Rep::HostValue,
        lower::Rep::Unit => Rep::Unit,
        lower::Rep::Struct => Rep::Struct,
        lower::Rep::Array => Rep::Array,
        lower::Rep::Sum => Rep::Sum,
        lower::Rep::Fn => Rep::Fn,
        lower::Rep::Any => Rep::Any,
    }
}
fn ordinary(value: &lower::Op) -> Op {
    use lower::Op as Source;
    match value {
        Source::Callback { value, mode } => Op::Callback {
            value: *value,
            mode: *mode,
        },
        Source::Const(value) => Op::Const(value.clone()),
        Source::Neg(value) => Op::Neg(*value),
        Source::Not(value) => Op::Not(*value),
        Source::Allocate(value) => Op::Allocate(*value),
        Source::Read(value) => Op::Read(*value),
        Source::Write { left, right } => Op::Write {
            left: *left,
            right: *right,
        },
        Source::And { left, right } => Op::And {
            left: *left,
            right: *right,
        },
        Source::Or { left, right } => Op::Or {
            left: *left,
            right: *right,
        },
        Source::Xor { left, right } => Op::Xor {
            left: *left,
            right: *right,
        },
        Source::Add { left, right } => Op::Add {
            left: *left,
            right: *right,
        },
        Source::Sub { left, right } => Op::Sub {
            left: *left,
            right: *right,
        },
        Source::Mul { left, right } => Op::Mul {
            left: *left,
            right: *right,
        },
        Source::Div { left, right } => Op::Div {
            left: *left,
            right: *right,
        },
        Source::Struct(fields) => Op::Struct(
            fields
                .iter()
                .map(|(field, temp)| (key(field), *temp))
                .collect(),
        ),
        Source::Array(values) => Op::Array(values.clone()),
        Source::Merge(values) => Op::Merge(values.clone()),
        Source::Concat(values) => Op::Concat(values.clone()),
        Source::Project { base, field } => Op::Project {
            base: *base,
            field: key(field),
        },
        Source::Tag { name, payload } => Op::Tag {
            name: name.clone(),
            payload: *payload,
        },
        Source::Payload(value) => Op::Payload(*value),
        Source::Nth { base, index } => Op::Nth {
            base: *base,
            index: *index,
        },
        Source::NthBack { base, index } => Op::NthBack {
            base: *base,
            index: *index,
        },
        Source::Slice { base, start, drop } => Op::Slice {
            base: *base,
            start: *start,
            drop: *drop,
        },
        Source::Closure { func, captures } => Op::Closure {
            func: *func,
            captures: captures.clone(),
        },
        Source::Extern { symbol, name } => Op::Extern {
            symbol: *symbol,
            name: name.clone(),
        },
        Source::Global { symbol, name } => Op::Global {
            callable: None,
            symbol: *symbol,
            name: name.clone(),
        },
        Source::TypeProjection { descriptor, path } => Op::TypeProjection {
            descriptor: *descriptor,
            path: path.clone(),
        },
        Source::TypeDescriptor {
            template,
            arguments,
        } => Op::TypeDescriptor {
            template: template.clone(),
            arguments: arguments.clone(),
        },
        Source::NativePlan {
            template,
            arguments,
        } => Op::NativePlan {
            template: template.clone(),
            arguments: arguments.clone(),
        },
        Source::Reflect {
            kind,
            descriptor,
            value,
        } => Op::Reflect {
            kind: *kind,
            descriptor: *descriptor,
            value: *value,
        },
        Source::Convert {
            descriptor,
            value,
            direction,
        } => Op::Convert {
            descriptor: *descriptor,
            value: *value,
            direction: *direction,
        },
        Source::NewTag => Op::NewTag,
        _ => unreachable!("control instructions are split before ordinary lowering"),
    }
}
