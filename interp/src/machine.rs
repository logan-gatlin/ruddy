//! The driver: one transfer at a time, exactly as the artifact describes it.
//!
//! A Ruddy program in continuation-passing form never returns; it hands a
//! value to a continuation. This loop is the only thing that performs a
//! transfer, so a call, a handler entry, and a handler exit are data here
//! rather than Rust stack frames, and resuming a continuation costs no stack.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use ruddy::{
    artifact::{Artifact, Block, Callee, End, FieldKey, Literal, Op, Rep, Test},
    externs::Completion,
    types::Domains,
};

use crate::{
    Error,
    value::{Callback, Closure, Cont, Handler, Key, Native, Primitive, Record, Sum, Tag, Value},
};

/// What the driver does next.
enum Step {
    /// Run one block of one function with these arguments.
    Block {
        function: usize,
        block: usize,
        args: Vec<Value>,
        handler: Handler,
    },
    /// Call a Ruddy function.
    Call {
        callee: Value,
        args: Vec<Value>,
        continuation: Value,
        handler: Handler,
    },
    /// Call a value the host provides.
    Raw {
        callee: Value,
        args: Vec<Value>,
        continuation: Value,
        handler: Handler,
        completion: Completion,
    },
    /// Hand a value to a continuation.
    Resume { continuation: Value, value: Value },
    /// Install a handler identity and run the body under it.
    Enter {
        tag: Rc<Tag>,
        continuation: Value,
        body: Box<Step>,
    },
    /// Leave a handler, resuming where it was installed.
    Leave {
        tag: Rc<Tag>,
        value: Value,
        handler: Handler,
    },
}

/// Why a run stopped short of a value.
pub enum Fault {
    Error(Error),
    /// A handler belonging to an enclosing run was left from inside a nested
    /// one. It travels out to the run that owns the handler.
    Abort {
        tag: Rc<Tag>,
        value: Value,
    },
}

impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self::Error(error)
    }
}

/// A loaded program and the state of the run in progress.
pub struct Machine {
    pub domains: Domains,
    pub globals: HashMap<String, Value>,
    functions: Vec<ruddy::artifact::Function>,
    /// Every function's temporaries, so a block activation is one vector.
    widths: Vec<usize>,
    initializers: Vec<(String, usize)>,
    /// The externs by name, as the primitive each one stands for.
    externs: HashMap<String, Value>,
    /// The next run identity. A run owns the handlers it installs.
    runs: u64,
    /// Closures the host supplied as arguments. Such a closure may implement
    /// a narrower evidence convention than the call site passes, so its
    /// visible argument is taken from the end of the list.
    supplied: HashSet<usize>,
    /// How deep nested runs are, so an interpreter bug cannot exhaust the
    /// Rust stack silently.
    depth: usize,
}

/// A conversion that did not hold, as a fault.
fn failed(failure: crate::convert::Failure) -> Fault {
    Fault::Error(Error::Runtime(failure.message))
}
fn runtime(message: impl Into<String>) -> Fault {
    Fault::Error(Error::Runtime(message.into()))
}
fn unsupported(message: impl Into<String>) -> Fault {
    Fault::Error(Error::Unsupported(message.into()))
}
fn load(message: impl Into<String>) -> Error {
    Error::Load(message.into())
}

impl Machine {
    pub fn load(artifact: &Artifact) -> Result<Self, Error> {
        let lir = artifact.lir();
        let functions = lir.functions.clone();
        let widths = functions
            .iter()
            .map(|function| {
                let mut width = function.continuation;
                for param in &function.params {
                    width = width.max(param.temp);
                }
                for block in &function.blocks {
                    for param in &block.params {
                        width = width.max(param.temp);
                    }
                    for instr in &block.instrs {
                        width = width.max(instr.temp);
                    }
                }
                width as usize + 1
            })
            .collect();
        let mut externs = HashMap::new();
        for external in &lir.externs {
            externs.insert(
                external.name.clone(),
                Value::Primitive(Rc::new(Primitive {
                    target: Rc::from(external.target.as_str()),
                    applied: Vec::new(),
                })),
            );
        }
        let mut globals = externs.clone();
        let mut initializers = Vec::new();
        for global in &lir.globals {
            // A name is known from the moment the artifact declares it. A
            // definition read before its own initializer has run carries
            // nothing, exactly as it does on the other backend.
            globals.entry(global.name.clone()).or_insert(Value::Absent);
            let initializer = usize::try_from(global.initializer)
                .ok()
                .filter(|id| *id < functions.len())
                .ok_or_else(|| load(format!("global `{}` has no initializer", global.name)))?;
            initializers.push((global.name.clone(), initializer));
        }
        // Every name an instruction reads must be one this artifact defines.
        // The artifact's own invariant covers its function and block tables;
        // its extern and global tables are the executing target's to check.
        for function in &functions {
            for block in &function.blocks {
                for instr in &block.instrs {
                    match &instr.op {
                        ruddy::artifact::Op::Extern { target } if !externs.contains_key(target) => {
                            return Err(load(format!("no host value named `{target}`")));
                        }
                        ruddy::artifact::Op::Global { target, .. }
                            if !globals.contains_key(target) =>
                        {
                            return Err(load(format!("no definition named `{target}`")));
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(Self {
            domains: artifact.header().domains,
            globals,
            functions,
            widths,
            initializers,
            externs,
            runs: 0,
            supplied: HashSet::new(),
            depth: 0,
        })
    }

    /// Run every global's initializer, in the order the artifact lists them.
    pub fn initialize(&mut self) -> Result<(), Error> {
        for (name, function) in std::mem::take(&mut self.initializers) {
            let closure = Value::Closure(Rc::new(Closure {
                function,
                captures: Vec::new(),
            }));
            let value = self
                .start(closure, Vec::new(), None)
                .map_err(|fault| self.report(fault))
                .map_err(|error| match error {
                    Error::Runtime(message) => {
                        Error::Runtime(format!("initializing `{name}`: {message}"))
                    }
                    other => other,
                })?;
            self.globals.insert(name, value);
        }
        Ok(())
    }

    /// Call a function value with one visible argument, from outside any run.
    pub fn call(&mut self, function: Value, argument: Value) -> Result<Value, Error> {
        self.apply(function, argument, None)
            .map_err(|fault| self.report(fault))
    }

    /// A fault as an error. An abort that reached the outermost run means a
    /// handler was left after the run that installed it had finished.
    fn report(&self, fault: Fault) -> Error {
        match fault {
            Fault::Error(error) => error,
            Fault::Abort { .. } => Error::Runtime("invalid handler exit".into()),
        }
    }

    /// Call a value with one argument and run it to completion, the way a
    /// host caller does: evidence records precede the visible argument.
    fn apply(
        &mut self,
        function: Value,
        argument: Value,
        handler: Handler,
    ) -> Result<Value, Fault> {
        match &function {
            Value::Callback(callback) => {
                let inner = callback.inner.clone();
                self.apply(inner, argument, handler)
            }
            Value::Native(native) => match &**native {
                Native::Builtin(_) => {
                    let result = self.builtin(native.clone(), argument)?;
                    Ok(result)
                }
                Native::Outgoing { .. } | Native::Incoming { .. } => {
                    self.start(function.clone(), vec![argument], handler)
                }
            },
            Value::Primitive(_) => self.start(function.clone(), vec![argument], handler),
            Value::Closure(closure) => {
                let arity = self
                    .functions
                    .get(closure.function)
                    .ok_or_else(|| runtime("closure outside the function table"))?
                    .params
                    .len();
                let count = arity.saturating_sub(closure.captures.len());
                let mut args = Vec::with_capacity(count.max(1));
                for _ in 1..count {
                    args.push(Value::unit());
                }
                if count > 0 {
                    args.push(argument);
                }
                self.start(function.clone(), args, handler)
            }
            other => Err(runtime(format!("cannot call {}", other.kind()))),
        }
    }

    /// Start a run: a call with a root continuation, driven to its value.
    fn start(&mut self, callee: Value, args: Vec<Value>, handler: Handler) -> Result<Value, Fault> {
        self.depth += 1;
        if self.depth > 512 {
            self.depth -= 1;
            return Err(runtime("too many nested foreign calls"));
        }
        for argument in &args {
            if let Value::Closure(closure) = argument {
                self.supplied.insert(Rc::as_ptr(closure) as usize);
            }
        }
        self.runs += 1;
        let run = self.runs;
        let result = self.drive(
            Step::Call {
                callee,
                args,
                continuation: Value::Cont(Rc::new(Cont::Root)),
                handler: handler.clone(),
            },
            handler,
            run,
        );
        self.depth -= 1;
        result
    }

    /// The transfer loop. It ends when the root continuation takes a value.
    fn drive(&mut self, start: Step, base: Handler, run: u64) -> Result<Value, Fault> {
        let mut next = start;
        let mut current = base.clone();
        loop {
            match next {
                Step::Block {
                    function,
                    block,
                    args,
                    handler,
                } => {
                    current = handler;
                    next = self.block(function, block, args, &current)?;
                }
                Step::Resume {
                    continuation,
                    value,
                } => {
                    let Value::Cont(cont) = &continuation else {
                        return Err(runtime(format!(
                            "expected a continuation, found {}",
                            continuation.kind()
                        )));
                    };
                    match &**cont {
                        Cont::Root => {
                            expire(&current, &base);
                            return Ok(value);
                        }
                        Cont::Code {
                            function,
                            block,
                            captures,
                            handler,
                        } => {
                            let mut args = captures.clone();
                            args.push(value);
                            next = Step::Block {
                                function: *function,
                                block: *block,
                                args,
                                handler: handler.clone(),
                            };
                        }
                        Cont::Converting {
                            plan,
                            to,
                            callable,
                            parent,
                        } => {
                            let converted = crate::convert::convert(
                                self, plan, &value, false, *to, *callable, false,
                            )
                            .map_err(failed)?;
                            next = Step::Resume {
                                continuation: parent.clone(),
                                value: converted,
                            };
                        }
                    }
                }
                Step::Call {
                    callee,
                    args,
                    continuation,
                    handler,
                } => {
                    current = handler.clone();
                    next = self.enter_call(callee, args, continuation, handler)?;
                }
                Step::Raw {
                    callee,
                    args,
                    continuation,
                    handler,
                    completion,
                } => {
                    current = handler.clone();
                    if completion != Completion::Immediate {
                        return Err(unsupported(
                            "a foreign call that completes later than immediately",
                        ));
                    }
                    match self.foreign(callee, args) {
                        Ok(value) => {
                            next = Step::Resume {
                                continuation,
                                value,
                            };
                        }
                        // A nested run may leave a handler this run installed.
                        Err(Fault::Abort { tag, value }) => {
                            next = Step::Leave {
                                tag,
                                value,
                                handler: current.clone(),
                            };
                        }
                        Err(fault) => return Err(fault),
                    }
                }
                Step::Enter {
                    tag,
                    continuation,
                    body,
                } => {
                    let Step::Block {
                        function,
                        block,
                        args,
                        handler,
                    } = *body
                    else {
                        return Err(runtime("a handler body must be a block"));
                    };
                    tag.active.set(true);
                    *tag.parent.borrow_mut() = handler;
                    *tag.continuation.borrow_mut() = Some(continuation);
                    tag.owner.set(run);
                    next = Step::Block {
                        function,
                        block,
                        args,
                        handler: Some(tag),
                    };
                }
                Step::Leave {
                    tag,
                    value,
                    handler,
                } => {
                    current = handler.clone();
                    if !tag.active.get() || !inside(&current, &tag) {
                        return Err(runtime("invalid handler exit"));
                    }
                    if tag.owner.get() != run {
                        return Err(Fault::Abort { tag, value });
                    }
                    let continuation = tag
                        .continuation
                        .borrow()
                        .clone()
                        .ok_or_else(|| runtime("invalid handler exit"))?;
                    let parent = tag.parent.borrow().clone();
                    expire(&current, &parent);
                    current = parent;
                    next = Step::Resume {
                        continuation,
                        value,
                    };
                }
            }
        }
    }

    /// A call to a Ruddy function, or to one converted at a boundary.
    fn enter_call(
        &mut self,
        callee: Value,
        args: Vec<Value>,
        continuation: Value,
        handler: Handler,
    ) -> Result<Step, Fault> {
        let callee = match &callee {
            Value::Callback(callback) => callback.inner.clone(),
            _ => callee,
        };
        match &callee {
            Value::Closure(closure) => {
                let function = self
                    .functions
                    .get(closure.function)
                    .ok_or_else(|| runtime("closure outside the function table"))?;
                let wanted =
                    function.params.len() - closure.captures.len().min(function.params.len());
                let mut args = args;
                // A closure the host supplied can implement a narrower
                // evidence convention. Its visible argument remains last.
                if args.len() != wanted && self.supplied.contains(&(Rc::as_ptr(closure) as usize)) {
                    while args.len() > wanted {
                        args.remove(0);
                    }
                    while args.len() < wanted {
                        args.insert(0, Value::unit());
                    }
                }
                let mut supplied = closure.captures.clone();
                supplied.extend(args);
                if supplied.len() != function.params.len() {
                    return Err(runtime(format!(
                        "`{}` takes {} arguments and was given {}",
                        function.name,
                        function.params.len(),
                        supplied.len()
                    )));
                }
                // A function's parameters are its whole frame; its entry
                // block declares only the ones it reads.
                let entry = function.entry as usize;
                let mut frame = vec![Value::Absent; self.widths[closure.function]];
                for (param, value) in function.params.iter().zip(supplied) {
                    frame[param.temp as usize] = value;
                }
                frame[function.continuation as usize] = continuation;
                let args = function.blocks[entry]
                    .params
                    .iter()
                    .map(|param| frame[param.temp as usize].clone())
                    .collect();
                Ok(Step::Block {
                    function: closure.function,
                    block: entry,
                    args,
                    handler,
                })
            }
            Value::Native(native) => match &**native {
                // A host function reached through a conversion plan: its
                // argument crosses out, and its result crosses back in.
                Native::Incoming {
                    plan,
                    from,
                    to,
                    function,
                    slots,
                } => {
                    let argument = args
                        .last()
                        .cloned()
                        .ok_or_else(|| runtime("a native call takes one argument"))?;
                    let plan = if slots.is_empty() {
                        plan.clone()
                    } else {
                        crate::convert::supply_slots(plan, slots, &args).map_err(failed)?
                    };
                    let converted =
                        crate::convert::convert(self, &plan, &argument, true, *from, true, false)
                            .map_err(failed)?;
                    Ok(Step::Raw {
                        callee: function.clone(),
                        args: vec![converted],
                        continuation: Value::Cont(Rc::new(Cont::Converting {
                            plan,
                            to: *to,
                            callable: true,
                            parent: continuation,
                        })),
                        handler,
                        completion: Completion::Immediate,
                    })
                }
                Native::Outgoing { .. } | Native::Builtin(_) => {
                    let argument = args
                        .last()
                        .cloned()
                        .ok_or_else(|| runtime("a native call takes one argument"))?;
                    let value = self.native(native.clone(), argument, handler.clone())?;
                    Ok(Step::Resume {
                        continuation,
                        value,
                    })
                }
            },
            Value::Primitive(_) => {
                let value = self.foreign(callee.clone(), args)?;
                Ok(Step::Resume {
                    continuation,
                    value,
                })
            }
            other => Err(runtime(format!("cannot call {}", other.kind()))),
        }
    }

    /// Call a value the host provides: a primitive of the portable table, or
    /// a function the interpreter itself implements.
    fn foreign(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, Fault> {
        match &callee {
            Value::Primitive(primitive) => {
                let target = primitive.target.clone();
                let mut applied = primitive.applied.clone();
                applied.extend(args);
                let Some(arity) = crate::prim::arity(&target) else {
                    return Err(unsupported(format!(
                        "the host value `{target}`, which this interpreter does not provide"
                    )));
                };
                if applied.len() < arity {
                    return Ok(Value::Primitive(Rc::new(Primitive { target, applied })));
                }
                if applied.len() > arity {
                    return Err(runtime(format!(
                        "`{target}` takes {arity} arguments and was given {}",
                        applied.len()
                    )));
                }
                crate::prim::call(&target, &applied, self.domains)
                    .map_err(|message| runtime(format!("{target}: {message}")))
            }
            Value::Native(native) => {
                let argument = args
                    .into_iter()
                    .next_back()
                    .ok_or_else(|| runtime("a native call takes one argument"))?;
                self.native(native.clone(), argument, None)
            }
            Value::Closure(_) | Value::Callback(_) => {
                let argument = args
                    .into_iter()
                    .next_back()
                    .ok_or_else(|| runtime("a call takes one argument"))?;
                self.apply(callee.clone(), argument, None)
            }
            other => Err(runtime(format!(
                "cannot call {} as a host value",
                other.kind()
            ))),
        }
    }

    /// One call of a function the interpreter implements.
    fn native(
        &mut self,
        native: Rc<Native>,
        argument: Value,
        handler: Handler,
    ) -> Result<Value, Fault> {
        match &*native {
            Native::Builtin(_) => self.builtin(native.clone(), argument),
            Native::Outgoing {
                plan,
                from,
                to,
                inner,
                callable,
                host_export,
            } => {
                let converted = crate::convert::convert(
                    self,
                    plan,
                    &argument,
                    false,
                    *from,
                    *callable,
                    *host_export,
                )
                .map_err(failed)?;
                let result = self.apply(inner.clone(), converted, handler)?;
                crate::convert::convert(self, plan, &result, true, *to, *callable, *host_export)
                    .map_err(failed)
            }
            Native::Incoming {
                plan,
                from,
                to,
                function,
                ..
            } => {
                let converted =
                    crate::convert::convert(self, plan, &argument, true, *from, true, false)
                        .map_err(failed)?;
                let result = self.foreign(function.clone(), vec![converted])?;
                crate::convert::convert(self, plan, &result, false, *to, true, false)
                    .map_err(failed)
            }
        }
    }

    /// One operation of a typed view.
    fn builtin(&mut self, native: Rc<Native>, argument: Value) -> Result<Value, Fault> {
        let Native::Builtin(builtin) = &*native else {
            return Err(runtime("expected a view operation"));
        };
        crate::reflect::builtin(builtin, argument).map_err(runtime)
    }

    /// Run one block: its instructions, then its terminator.
    fn block(
        &mut self,
        function: usize,
        block: usize,
        args: Vec<Value>,
        handler: &Handler,
    ) -> Result<Step, Fault> {
        let code = self
            .functions
            .get(function)
            .ok_or_else(|| runtime("call outside the function table"))?;
        let body: &Block = code
            .blocks
            .get(block)
            .ok_or_else(|| runtime("jump outside the block table"))?;
        let mut temps: Vec<Value> = vec![Value::Absent; self.widths[function]];
        if args.len() != body.params.len() {
            return Err(runtime(format!(
                "block {block} of `{}` takes {} arguments and was given {}",
                code.name,
                body.params.len(),
                args.len()
            )));
        }
        for (param, value) in body.params.iter().zip(args) {
            temps[param.temp as usize] = value;
        }
        // The block's own code is cloned out of the table so that an
        // instruction may call back into the machine while it runs.
        let body = body.clone();
        for instr in &body.instrs {
            let value = self.operation(&instr.op, instr.rep, &temps, handler)?;
            temps[instr.temp as usize] = value;
        }
        let at = |temp: &u32| -> Value { temps[*temp as usize].clone() };
        let edge = |edge: &ruddy::artifact::Edge| Step::Block {
            function,
            block: edge.block as usize,
            args: edge.args.iter().map(at).collect(),
            handler: handler.clone(),
        };
        Ok(match &body.end {
            End::Continue {
                continuation,
                value,
            } => Step::Resume {
                continuation: at(continuation),
                value: at(value),
            },
            End::Jump(target) => edge(target),
            End::Branch { test, yes, no } => {
                if self.test(test, &temps)? {
                    edge(yes)
                } else {
                    edge(no)
                }
            }
            End::Call {
                callee,
                args,
                continuation,
            } => Step::Call {
                callee: match callee {
                    Callee::Direct(id) => Value::Closure(Rc::new(Closure {
                        function: *id as usize,
                        captures: Vec::new(),
                    })),
                    Callee::Indirect(temp) => at(temp),
                },
                args: args.iter().map(at).collect(),
                continuation: at(continuation),
                handler: handler.clone(),
            },
            End::RawCall {
                callee,
                args,
                continuation,
                completion,
            } => Step::Raw {
                callee: at(callee),
                args: args.iter().map(at).collect(),
                continuation: at(continuation),
                handler: handler.clone(),
                completion: *completion,
            },
            End::Enter {
                tag,
                body: target,
                continuation,
            } => {
                let Value::Tag(tag) = at(tag) else {
                    return Err(runtime("entering a handler requires an identity"));
                };
                Step::Enter {
                    tag,
                    continuation: at(continuation),
                    body: Box::new(edge(target)),
                }
            }
            End::Leave { tag, value } | End::Abort { tag, value } => {
                let Value::Tag(tag) = at(tag) else {
                    return Err(runtime("leaving a handler requires an identity"));
                };
                Step::Leave {
                    tag,
                    value: at(value),
                    handler: handler.clone(),
                }
            }
            End::Unreachable => return Err(runtime("unreachable Ruddy branch")),
        })
    }

    fn test(&self, test: &Test, temps: &[Value]) -> Result<bool, Fault> {
        let at = |temp: &u32| &temps[*temp as usize];
        Ok(match test {
            Test::Tag { on, name } => match at(on) {
                Value::Sum(sum) => &*sum.tag == name.as_str(),
                other => {
                    return Err(runtime(format!(
                        "expected a sum to test, found {}",
                        other.kind()
                    )));
                }
            },
            Test::Literal { on, value } => same(at(on), &literal(value)),
            Test::Presence { on, field } => match at(on) {
                Value::Record(record) => record.contains_key(&Key::named(field)),
                _ => false,
            },
            Test::Rest { on, fields } => match at(on) {
                Value::Record(record) => record.keys().all(|key| match key {
                    Key::Named(name) => fields.iter().any(|known| known == &**name),
                    Key::Unnamed => false,
                }),
                _ => true,
            },
            Test::Length { on, length } => match at(on) {
                Value::Array(items) => items.len() as u64 == *length,
                other => {
                    return Err(runtime(format!(
                        "expected an array to measure, found {}",
                        other.kind()
                    )));
                }
            },
        })
    }

    /// One instruction's value.
    fn operation(
        &mut self,
        op: &Op,
        rep: Rep,
        temps: &[Value],
        handler: &Handler,
    ) -> Result<Value, Fault> {
        let at = |temp: &u32| temps[*temp as usize].clone();
        Ok(match op {
            Op::Const(value) => literal(value),
            Op::Neg(value) => match (rep, at(value)) {
                (Rep::Int, Value::Int(value)) => Value::Int(value.wrapping_neg()),
                (Rep::Nat, Value::Nat(value)) => Value::Nat(0u64.wrapping_sub(value)),
                (_, Value::Real(value)) => Value::Real(-value),
                (_, other) => return Err(runtime(format!("cannot negate {}", other.kind()))),
            },
            Op::Not(value) => Value::Bool(!at(value).as_bool().map_err(runtime)?),
            Op::Allocate(value) => Value::Cell(Rc::new(RefCell::new(at(value)))),
            Op::Read(value) => match at(value) {
                Value::Cell(cell) => cell.borrow().clone(),
                other => return Err(runtime(format!("cannot read {}", other.kind()))),
            },
            Op::Write { left, right } => match at(left) {
                Value::Cell(cell) => {
                    let value = at(right);
                    *cell.borrow_mut() = value.clone();
                    value
                }
                other => return Err(runtime(format!("cannot write to {}", other.kind()))),
            },
            Op::And { left, right } => Value::Bool(
                at(left).as_bool().map_err(runtime)? && at(right).as_bool().map_err(runtime)?,
            ),
            Op::Or { left, right } => Value::Bool(
                at(left).as_bool().map_err(runtime)? || at(right).as_bool().map_err(runtime)?,
            ),
            Op::Xor { left, right } => Value::Bool(
                at(left).as_bool().map_err(runtime)? != at(right).as_bool().map_err(runtime)?,
            ),
            Op::Add { left, right } => arithmetic(rep, "add", at(left), at(right))?,
            Op::Sub { left, right } => arithmetic(rep, "subtract", at(left), at(right))?,
            Op::Mul { left, right } => arithmetic(rep, "multiply", at(left), at(right))?,
            Op::Div { left, right } => arithmetic(rep, "divide", at(left), at(right))?,
            Op::Struct(fields) => Value::Record(Rc::new(
                fields
                    .iter()
                    .map(|(key, temp)| (key_of(key), at(temp)))
                    .collect(),
            )),
            Op::Array(values) => Value::array(values.iter().map(at).collect()),
            Op::Concat(values) => {
                let mut items = Vec::new();
                for value in values {
                    items.extend(at(value).as_array().map_err(runtime)?.iter().cloned());
                }
                Value::array(items)
            }
            Op::Merge(values) => {
                let mut fields = Record::new();
                for value in values {
                    for (key, field) in at(value).as_record().map_err(runtime)?.iter() {
                        fields.insert(key.clone(), field.clone());
                    }
                }
                Value::Record(Rc::new(fields))
            }
            Op::Project { base, field } => match at(base) {
                Value::Record(record) => {
                    record.get(&key_of(field)).cloned().unwrap_or(Value::Absent)
                }
                Value::Absent => Value::Absent,
                other => {
                    return Err(runtime(format!("cannot read a field of {}", other.kind())));
                }
            },
            Op::Tag { name, payload } => Value::Sum(Rc::new(Sum {
                tag: Rc::from(name.as_str()),
                payload: payload.as_ref().map(at),
            })),
            Op::Payload(value) => match at(value) {
                Value::Sum(sum) => sum.payload.clone().unwrap_or(Value::Absent),
                other => {
                    return Err(runtime(format!(
                        "expected a sum to take apart, found {}",
                        other.kind()
                    )));
                }
            },
            Op::Nth { base, index } => element(&at(base), *index as usize)?,
            Op::NthBack { base, index } => {
                let base = at(base);
                let length = base.as_array().map_err(runtime)?.len();
                let index = length
                    .checked_sub(*index as usize + 1)
                    .ok_or_else(|| runtime("array index out of range"))?;
                element(&base, index)?
            }
            Op::Slice { base, start, drop } => {
                let base = at(base);
                let items = base.as_array().map_err(runtime)?;
                let start = (*start as usize).min(items.len());
                let end = items.len().saturating_sub(*drop as usize).max(start);
                Value::array(items[start..end].to_vec())
            }
            Op::Closure { func, captures } => Value::Closure(Rc::new(Closure {
                function: *func as usize,
                captures: captures.iter().map(at).collect(),
            })),
            Op::Continuation { code, captures } => Value::Cont(Rc::new(Cont::Code {
                function: code.function as usize,
                block: code.block as usize,
                captures: captures.iter().map(at).collect(),
                handler: handler.clone(),
            })),
            Op::Extern { target } => self
                .externs
                .get(target)
                .cloned()
                .ok_or_else(|| runtime(format!("no host value named `{target}`")))?,
            Op::Global { target, .. } => self
                .globals
                .get(target)
                .cloned()
                .ok_or_else(|| runtime(format!("no definition named `{target}`")))?,
            Op::NewTag => Value::Tag(Rc::new(Tag::default())),
            Op::Callback { value, mode } => Value::Callback(Rc::new(Callback {
                inner: at(value),
                mode: *mode,
            })),
            Op::TypeDescriptor {
                template,
                arguments,
            } => {
                let arguments: Vec<Value> = arguments.iter().map(at).collect();
                crate::types::instantiate(template, &arguments, false).map_err(runtime)?
            }
            Op::NativePlan {
                template,
                arguments,
            } => {
                let arguments: Vec<Value> = arguments.iter().map(at).collect();
                crate::types::native_plan(template, &arguments).map_err(runtime)?
            }
            Op::TypeProjection { descriptor, path } => {
                crate::types::project(&at(descriptor), path).map_err(runtime)?
            }
            Op::Convert {
                descriptor,
                value,
                direction,
            } => {
                let descriptor = at(descriptor);
                let plan = descriptor.as_type().map_err(runtime)?.clone();
                let outgoing = *direction == ruddy::backend::host::Direction::ToJs;
                crate::convert::convert(self, &plan, &at(value), outgoing, 0, true, false)
                    .map_err(failed)?
            }
            Op::Reflect {
                kind,
                descriptor,
                value,
            } => crate::reflect::intrinsic(self, *kind, &at(descriptor), &at(value))
                .map_err(runtime)?,
        })
    }
}

fn key_of(key: &FieldKey) -> Key {
    match key {
        FieldKey::Named(name) => Key::Named(Rc::from(name.as_str())),
        FieldKey::UnnamedOperation => Key::Unnamed,
    }
}

fn element(base: &Value, index: usize) -> Result<Value, Fault> {
    base.as_array()
        .map_err(runtime)?
        .get(index)
        .cloned()
        .ok_or_else(|| runtime("array index out of range"))
}

fn literal(value: &Literal) -> Value {
    match value {
        Literal::Natural(value) => Value::Nat(*value),
        Literal::Integer(value) => Value::Int(*value),
        Literal::Fixed(value) => Value::Fixed(value.kind(), value.value()),
        Literal::Real(bits) => Value::Real(f64::from_bits(*bits)),
        Literal::String(value) => Value::string(value),
        Literal::Bool(value) => Value::Bool(*value),
    }
}

/// Whether a value equals a literal: exact equality, and for real numbers
/// the same equality a match arm uses, under which a NaN matches a NaN.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Nat(left), Value::Nat(right)) => left == right,
        (Value::Int(left), Value::Int(right)) => left == right,
        (Value::Fixed(left, value), Value::Fixed(right, other)) => left == right && value == other,
        (Value::Real(left), Value::Real(right)) => {
            left.to_bits() == right.to_bits() || left == right
        }
        (Value::Str(left), Value::Str(right)) => left == right,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        // A retagged artifact can compare an integer against a real literal.
        (Value::Nat(left), Value::Real(right)) => *left as f64 == *right,
        (Value::Int(left), Value::Real(right)) => *left as f64 == *right,
        _ => false,
    }
}

/// Arithmetic under the representation the artifact assigned the result.
fn arithmetic(rep: Rep, name: &str, left: Value, right: Value) -> Result<Value, Fault> {
    let divisor = |value: i128| {
        (value != 0)
            .then_some(value)
            .ok_or_else(|| runtime("division by zero"))
    };
    Ok(match rep {
        Rep::Nat => {
            let (left, right) = (
                left.as_nat().map_err(runtime)?,
                right.as_nat().map_err(runtime)?,
            );
            Value::Nat(match name {
                "add" => left.wrapping_add(right),
                // A natural number never goes below zero.
                "subtract" => left.saturating_sub(right),
                "multiply" => left.wrapping_mul(right),
                _ => left
                    .checked_div(right)
                    .ok_or_else(|| runtime("division by zero"))?,
            })
        }
        Rep::Int => {
            let (left, right) = (
                left.as_int().map_err(runtime)?,
                right.as_int().map_err(runtime)?,
            );
            Value::Int(match name {
                "add" => left.wrapping_add(right),
                "subtract" => left.wrapping_sub(right),
                "multiply" => left.wrapping_mul(right),
                _ => left
                    .checked_div(right)
                    .ok_or_else(|| runtime("division by zero"))?,
            })
        }
        Rep::Fixed(kind) => {
            let (a, b) = (
                left.as_integer().map_err(runtime)?,
                right.as_integer().map_err(runtime)?,
            );
            let value = match name {
                "add" => a + b,
                "subtract" => a - b,
                "multiply" => a * b,
                _ => a / divisor(b)?,
            };
            Value::Fixed(kind, crate::prim::wrap(kind, value))
        }
        _ => {
            let (left, right) = (
                left.as_real().map_err(runtime)?,
                right.as_real().map_err(runtime)?,
            );
            Value::Real(match name {
                "add" => left + right,
                "subtract" => left - right,
                "multiply" => left * right,
                _ => left / right,
            })
        }
    })
}

/// Whether a handler is this one or is nested inside it.
fn inside(handler: &Handler, tag: &Rc<Tag>) -> bool {
    let mut at = handler.clone();
    while let Some(current) = at {
        if Rc::ptr_eq(&current, tag) {
            return true;
        }
        at = current.parent.borrow().clone();
    }
    false
}

/// Deactivate every handler from one up to, but not including, another.
fn expire(from: &Handler, stop: &Handler) {
    let mut at = from.clone();
    while let Some(current) = at {
        if stop.as_ref().is_some_and(|stop| Rc::ptr_eq(stop, &current)) {
            break;
        }
        current.active.set(false);
        *current.continuation.borrow_mut() = None;
        current.owner.set(0);
        at = current.parent.borrow().clone();
    }
}
