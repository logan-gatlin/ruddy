//! Checked conversion at the foreign boundary.
//!
//! A conversion plan says what a value must be for it to cross. Going out,
//! a Ruddy value becomes what a host caller reads: a sum becomes a record of
//! its tag and payload, the unit value becomes nothing at all. Coming in, a
//! host value is checked against the same plan before any of it is trusted,
//! and the first thing that does not hold names its own position.

use std::{collections::HashSet, rc::Rc};

use ruddy::{reification::Node, types::Bounds};

use crate::{
    machine::Machine,
    value::{Key, Native, Record, Type, Value},
};

/// What a value had to be, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: String,
    pub expected: String,
    pub message: String,
}

impl Failure {
    fn at(path: &str, expected: impl Into<String>) -> Self {
        let expected = expected.into();
        Self {
            message: format!("foreign value at {path} must be {expected}"),
            path: path.to_owned(),
            expected,
        }
    }
}

/// Convert one value across the boundary, in the direction `outgoing` names,
/// starting at the plan's node `root`. `callable` is false where a function
/// contract may not be reconstructed, and `host_export` marks the outermost
/// conversion of a value this program publishes.
pub fn convert(
    machine: &Machine,
    plan: &Rc<Type>,
    value: &Value,
    outgoing: bool,
    root: u32,
    callable: bool,
    host_export: bool,
) -> Result<Value, Failure> {
    let mut active = HashSet::new();
    Conversion {
        machine,
        plan,
        outgoing,
        callable,
        host_export,
        active: &mut active,
    }
    .walk(root, value, "$")
}

struct Conversion<'a> {
    machine: &'a Machine,
    plan: &'a Rc<Type>,
    outgoing: bool,
    callable: bool,
    host_export: bool,
    /// The values on the path from the root, so a cycle is refused rather
    /// than followed forever.
    active: &'a mut HashSet<usize>,
}

/// A value's identity, where it has one a cycle could be built from.
fn identity(value: &Value) -> Option<usize> {
    match value {
        Value::Record(record) => Some(Rc::as_ptr(record) as usize),
        Value::Array(items) => Some(Rc::as_ptr(items) as usize),
        Value::Sum(sum) => Some(Rc::as_ptr(sum) as usize),
        _ => None,
    }
}

impl Conversion<'_> {
    fn walk(&mut self, index: u32, value: &Value, path: &str) -> Result<Value, Failure> {
        let shape = self.plan.index(index);
        let node = self.plan.nodes[shape as usize].clone();
        match &node {
            Node::Nat | Node::Int => self.integer(&node, value, path),
            Node::Real => match value {
                Value::Real(value) => Ok(Value::Real(*value)),
                Value::Nat(value) => Ok(Value::Real(*value as f64)),
                Value::Int(value) => Ok(Value::Real(*value as f64)),
                _ => Err(Failure::at(path, "Real")),
            },
            Node::String => match value {
                Value::Str(text) => Ok(Value::Str(text.clone())),
                _ => Err(Failure::at(path, "String")),
            },
            Node::Bool => match value {
                Value::Bool(value) => Ok(Value::Bool(*value)),
                _ => Err(Failure::at(path, "Bool")),
            },
            // A foreign value crosses as it is, in either direction.
            Node::ForeignValue => Ok(value.clone()),
            Node::Fixed(kind) => {
                let found = value
                    .as_integer()
                    .map_err(|_| Failure::at(path, kind.name()))?;
                if found < kind.min() || found > kind.max() {
                    return Err(Failure::at(path, kind.name()));
                }
                Ok(Value::Fixed(*kind, found))
            }
            Node::Mirror(_) => match value {
                Value::Type(descriptor) if descriptor.mirror.get() => Ok(value.clone()),
                _ => Err(Failure::at(path, "an authentic mirror")),
            },
            Node::Hidden(_) => {
                if self.outgoing {
                    Ok(Value::Package(Rc::new(value.clone())))
                } else {
                    match value {
                        Value::Package(inner) => Ok((**inner).clone()),
                        _ => Err(Failure::at(path, "a package this program made")),
                    }
                }
            }
            Node::Arrow(arrow) => {
                if !self.callable {
                    return Err(Failure::at(
                        path,
                        "a verifiable data type; a function contract needs an adapter",
                    ));
                }
                if !value.is_callable() {
                    return Err(Failure::at(path, "Function"));
                }
                let (from, to) = (arrow[0], arrow[1]);
                Ok(Value::Native(Rc::new(if self.outgoing {
                    Native::Outgoing {
                        plan: self.plan.clone(),
                        from,
                        to,
                        inner: value.clone(),
                        callable: self.callable,
                        host_export: self.host_export,
                    }
                } else {
                    Native::Incoming {
                        plan: self.plan.clone(),
                        from,
                        to,
                        function: value.clone(),
                        slots: crate::types::native_slots(self.plan, shape),
                    }
                })))
            }
            Node::Struct(fields) if fields.is_empty() => {
                // The unit value carries nothing, so nothing is what a host
                // may supply for it, and a host handed nothing takes nothing
                // back. A value that is there still has to be a record.
                if matches!(value, Value::Absent) {
                    Ok(if self.outgoing {
                        Value::Absent
                    } else {
                        Value::unit()
                    })
                } else if !self.outgoing && self.callable {
                    Ok(Value::unit())
                } else if value.as_record().is_ok() {
                    Ok(Value::unit())
                } else {
                    Err(Failure::at(path, "Struct"))
                }
            }
            Node::Struct(fields) => {
                let record = value.as_record().map_err(|_| Failure::at(path, "Struct"))?;
                self.enter(value, path)?;
                let mut out = Record::new();
                for (name, child) in fields {
                    let key = Key::named(name);
                    let inner = format!("{path}.{name}");
                    match record.get(&key) {
                        Some(field) => {
                            out.insert(key, self.walk(*child, field, &inner)?);
                        }
                        None if self.plan.optional(shape, name) => {}
                        None => {
                            self.leave(value);
                            return Err(Failure::at(&inner, "a present field"));
                        }
                    }
                }
                self.leave(value);
                Ok(Value::Record(Rc::new(out)))
            }
            Node::Sum(cases) => {
                let (tag, payload) = self.case(value).ok_or_else(|| Failure::at(path, "Sum"))?;
                let Some((name, child)) = cases.iter().find(|(name, _)| *name == tag) else {
                    let names: Vec<&str> = cases.iter().map(|(name, _)| name.as_str()).collect();
                    return Err(Failure::at(path, format!("one of {}", names.join(", "))));
                };
                let inner = format!("{path}.{name}");
                self.enter(value, path)?;
                let converted = self.walk(*child, &payload, &inner);
                self.leave(value);
                let converted = converted?;
                Ok(if self.outgoing {
                    Value::record(vec![("tag", Value::string(name)), ("value", converted)])
                } else {
                    Value::Sum(Rc::new(crate::value::Sum {
                        tag: Rc::from(name.as_str()),
                        payload: Some(converted),
                    }))
                })
            }
            Node::Array(child) => {
                let items = value.as_array().map_err(|_| Failure::at(path, "Array"))?;
                self.enter(value, path)?;
                let mut out = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    match self.walk(*child, item, &format!("{path}[{index}]")) {
                        Ok(value) => out.push(value),
                        Err(failure) => {
                            self.leave(value);
                            return Err(failure);
                        }
                    }
                }
                self.leave(value);
                Ok(Value::array(out))
            }
            _ => Err(Failure::at(path, "a supported native type")),
        }
    }

    /// A target integer, checked against the domain this program bound.
    fn integer(&self, node: &Node, value: &Value, path: &str) -> Result<Value, Failure> {
        let natural = matches!(node, Node::Nat);
        let bounds: Bounds = if natural {
            self.machine.domains.nat()
        } else {
            self.machine.domains.int()
        };
        let name = if natural { "Nat" } else { "Int" };
        let found = match value {
            Value::Nat(value) => i128::from(*value),
            Value::Int(value) => i128::from(*value),
            Value::Fixed(_, value) => *value,
            // An integral real number is the same number.
            Value::Real(value) if value.fract() == 0.0 && value.is_finite() => *value as i128,
            _ => return Err(Failure::at(path, name)),
        };
        let low: i128 = bounds.min.parse().unwrap_or(i128::MIN);
        let high: i128 = bounds.max.parse().unwrap_or(i128::MAX);
        if found < low || found > high {
            return Err(Failure::at(path, name));
        }
        // Integers have one zero, so a negative zero from outside is zero.
        Ok(if natural {
            Value::Nat(found as u64)
        } else {
            Value::Int(found as i64)
        })
    }

    /// The tag and payload of a value crossing at a sum position, or nothing
    /// where the value is not a tagged one at all. A host may supply either a
    /// record carrying a tag and a value or a value this program made.
    fn case(&self, value: &Value) -> Option<(String, Value)> {
        match value {
            Value::Sum(sum) => Some((
                sum.tag.to_string(),
                sum.payload.clone().unwrap_or(Value::Absent),
            )),
            Value::Record(record) if !self.outgoing => {
                let tag = record
                    .get(&Key::named("tag"))
                    .and_then(|tag| tag.as_str().ok().map(str::to_owned))?;
                let payload = record
                    .get(&Key::named("value"))
                    .cloned()
                    .unwrap_or(Value::Absent);
                Some((tag, payload))
            }
            _ => None,
        }
    }

    fn enter(&mut self, value: &Value, path: &str) -> Result<(), Failure> {
        if let Some(identity) = identity(value)
            && !self.active.insert(identity)
        {
            return Err(Failure::at(path, "an acyclic value"));
        }
        Ok(())
    }
    fn leave(&mut self, value: &Value) {
        if let Some(identity) = identity(value) {
            self.active.remove(&identity);
        }
    }
}

/// Fill a plan's parameters from the descriptors a call supplies. Each entry
/// of `slots` names the plan parameter the call argument at that position
/// stands for.
pub fn supply_slots(plan: &Rc<Type>, slots: &[u32], args: &[Value]) -> Result<Rc<Type>, Failure> {
    let Some(existing) = plan.plan.as_ref() else {
        return Ok(plan.clone());
    };
    let mut supplied = existing.arguments.clone();
    for (index, slot) in slots.iter().enumerate() {
        let argument = args.get(index).cloned().unwrap_or(Value::Absent);
        let known = supplied.get(*slot as usize);
        if matches!(argument, Value::Absent) && matches!(known, None | Some(Value::Absent)) {
            return Err(Failure {
                path: "$".into(),
                expected: "runtime type information".into(),
                message: "missing runtime type information for a native call".into(),
            });
        }
        if !matches!(argument, Value::Absent)
            && let Some(slot) = supplied.get_mut(*slot as usize)
        {
            *slot = argument;
        }
    }
    let plan =
        crate::types::native_plan(&existing.template, &supplied).map_err(|message| Failure {
            path: "$".into(),
            expected: "a conversion plan".into(),
            message,
        })?;
    plan.as_type().cloned().map_err(|message| Failure {
        path: "$".into(),
        expected: "a conversion plan".into(),
        message,
    })
}
