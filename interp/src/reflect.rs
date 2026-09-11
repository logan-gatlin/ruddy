//! The reflection intrinsics and the typed views they hand back.
//!
//! A mirror is the evidence the compiler already passes, made authentic: only
//! a descriptor that came through one of these operations is one, so no value
//! read from outside can be made to stand for a type. What a mirror yields is
//! either ordinary data, which grants nothing, or operations that take values
//! of the mirrored type apart and put them together, converting nothing.

use std::rc::Rc;

use ruddy::{
    reification::{Intrinsic, Node},
    types::{Bounds, Domains, FixedInt},
};

use crate::{
    machine::Machine,
    value::{Builtin, Key, Native, Record, Type, Value},
};

/// One reflection intrinsic on its evidence and its argument.
pub fn intrinsic(
    machine: &Machine,
    kind: Intrinsic,
    descriptor: &Value,
    value: &Value,
) -> Result<Value, String> {
    match kind {
        Intrinsic::Decode => Ok(cross(machine, descriptor, value, false)),
        Intrinsic::Encode => Ok(cross(machine, descriptor, value, true)),
        Intrinsic::Mirror | Intrinsic::TypeOf => {
            let mirror = descriptor
                .as_type()
                .map_err(|_| "missing runtime type information for a mirror".to_string())?;
            mirror.mirror.set(true);
            Ok(descriptor.clone())
        }
        Intrinsic::Describe => {
            let of = value.as_type()?;
            Ok(describe(machine.domains, of))
        }
        Intrinsic::Same => {
            let pair = value.as_record()?;
            let left = pair
                .get(&Key::named("0"))
                .ok_or("comparing mirrors takes a pair")?
                .as_type()?;
            let right = pair
                .get(&Key::named("1"))
                .ok_or("comparing mirrors takes a pair")?
                .as_type()?;
            Ok(if crate::types::same(left, right) {
                Value::some(Value::record(vec![
                    ("forward", pass_through()),
                    ("backward", pass_through()),
                ]))
            } else {
                Value::none()
            })
        }
        Intrinsic::Shape => shape(machine.domains, value.as_type()?),
    }
}

/// Carry a value across the boundary as data alone: no function contract is
/// written or reconstructed, and a failure says where the value stopped
/// being what it had to be.
fn cross(machine: &Machine, descriptor: &Value, value: &Value, outgoing: bool) -> Value {
    let Ok(plan) = descriptor.as_type() else {
        return failure(&crate::convert::Failure {
            path: "$".into(),
            expected: if outgoing {
                "writable data".into()
            } else {
                "readable data".into()
            },
            message: "host observation failed".into(),
        });
    };
    match crate::convert::convert(machine, plan, value, outgoing, 0, false, false) {
        Ok(value) => Value::some(value),
        Err(fault) => failure(&fault),
    }
}

fn failure(failure: &crate::convert::Failure) -> Value {
    Value::sum(
        "Error",
        Some(Value::record(vec![
            ("path", Value::string(&failure.path)),
            ("expected", Value::string(&failure.expected)),
            ("message", Value::string(&failure.message)),
        ])),
    )
}

/// A function of one value that hands it back unchanged: what an established
/// equality of two mirrors supplies in each direction, and what every typed
/// view's operations are where the mirror already proved the type.
fn pass_through() -> Value {
    Value::Native(Rc::new(Native::Builtin(Builtin::PassThrough)))
}

fn builtin_value(builtin: Builtin) -> Value {
    Value::Native(Rc::new(Native::Builtin(builtin)))
}

/// A mirror of one position of a mirror's graph, authentic because the whole
/// was.
fn mirror_at(of: &Rc<Type>, index: u32) -> Value {
    let part = crate::types::at(of, index);
    part.mirror.set(true);
    Value::Type(part)
}

/// An integer kind's exact domain, as `std::reflect::Domain`.
fn domain(bits: u32, signed: bool, min: &str, max: &str) -> Value {
    Value::record(vec![
        ("bits", Value::Nat(u64::from(bits))),
        ("signed", Value::Bool(signed)),
        ("min", Value::string(min)),
        ("max", Value::string(max)),
    ])
}

fn bounds(value: Bounds) -> Value {
    domain(value.bits, value.signed, value.min, value.max)
}

fn fixed_domain(kind: FixedInt) -> Value {
    domain(
        kind.bits(),
        kind.signed(),
        &kind.min().to_string(),
        &kind.max().to_string(),
    )
}

/// A mirror's graph as ordinary data: one case per node, indices kept, so
/// recursion stays finite and nothing here can be turned back into a mirror.
fn describe(domains: Domains, of: &Rc<Type>) -> Value {
    let field = |name: &str, node: u32| {
        Value::record(vec![
            ("name", Value::string(name)),
            ("node", Value::Nat(u64::from(node))),
        ])
    };
    let fields = |entries: &[(String, u32)]| {
        Value::array(
            entries
                .iter()
                .map(|(name, node)| field(name, *node))
                .collect(),
        )
    };
    let effects = |of: &Rc<Type>, at: u32| match of.node(at) {
        Node::Effects(effects) => Value::array(
            effects
                .iter()
                .map(|effect| field(&effect.identity, effect.payload))
                .collect(),
        ),
        _ => Value::array(Vec::new()),
    };
    let nodes = of
        .nodes
        .iter()
        .map(|node| match node {
            Node::Nat => Value::sum("Nat", Some(bounds(domains.nat()))),
            Node::Int => Value::sum("Int", Some(bounds(domains.int()))),
            Node::Fixed(kind) => Value::sum("Fixed", Some(fixed_domain(*kind))),
            Node::Real => Value::sum("Real", None),
            Node::String => Value::sum("String", None),
            Node::Bool => Value::sum("Bool", None),
            Node::ForeignValue => Value::sum("Foreign", None),
            Node::Array(child) => Value::sum("Array", Some(Value::Nat(u64::from(*child)))),
            Node::Arrow(arrow) => Value::sum(
                "Function",
                Some(Value::record(vec![
                    ("argument", Value::Nat(u64::from(arrow[0]))),
                    ("result", Value::Nat(u64::from(arrow[1]))),
                    ("effects", effects(of, arrow[2])),
                ])),
            ),
            Node::Effects(row) => Value::sum(
                "Effects",
                Some(Value::array(
                    row.iter()
                        .map(|effect| field(&effect.identity, effect.payload))
                        .collect(),
                )),
            ),
            Node::Struct(entries) => Value::sum("Record", Some(fields(entries))),
            Node::Sum(entries) => Value::sum("Sum", Some(fields(entries))),
            Node::Alias(child) => Value::sum("Alias", Some(Value::Nat(u64::from(*child)))),
            Node::Extend(parts) => Value::sum(
                "Extend",
                Some(Value::record(vec![
                    ("base", Value::Nat(u64::from(parts[0]))),
                    ("rest", Value::Nat(u64::from(parts[1]))),
                ])),
            ),
            Node::Parameter(index) => Value::sum("Parameter", Some(Value::Nat(u64::from(*index)))),
            Node::Mirror(child) => Value::sum("Mirror", Some(Value::Nat(u64::from(*child)))),
            Node::Hidden(child) => Value::sum("Hidden", Some(Value::Nat(u64::from(*child)))),
            Node::HiddenBound(index) => Value::sum("Variable", Some(Value::Nat(u64::from(*index)))),
        })
        .collect();
    Value::record(vec![
        ("root", Value::Nat(0)),
        ("nodes", Value::array(nodes)),
    ])
}

/// A mirror's outermost structure as `std::reflect::Shape`.
fn shape(domains: Domains, of: &Rc<Type>) -> Result<Value, String> {
    let typed = |name: &str| {
        Value::sum(
            name,
            Some(Value::record(vec![
                ("read", pass_through()),
                ("make", pass_through()),
            ])),
        )
    };
    Ok(match of.node(0).clone() {
        Node::Nat => typed("Nat"),
        Node::Int => typed("Int"),
        Node::Real => typed("Real"),
        Node::String => typed("String"),
        Node::Bool => typed("Bool"),
        Node::Fixed(kind) => typed(kind.name()),
        Node::ForeignValue => Value::sum("Foreign", None),
        Node::Array(element) => Value::sum(
            "Array",
            Some(Value::record(vec![
                ("element", mirror_at(of, element)),
                ("read", pass_through()),
                ("make", pass_through()),
            ])),
        ),
        Node::Struct(fields) => {
            let views = fields
                .iter()
                .map(|(name, index)| {
                    Value::record(vec![
                        ("name", Value::string(name)),
                        ("mirror", mirror_at(of, *index)),
                        ("presence", Value::sum("Required", None)),
                        (
                            "read",
                            builtin_value(Builtin::ReadField(Rc::from(name.as_str()))),
                        ),
                        (
                            "bind",
                            builtin_value(Builtin::Bind {
                                record: of.clone(),
                                name: Rc::from(name.as_str()),
                                field: crate::types::at(of, *index),
                            }),
                        ),
                    ])
                })
                .collect();
            Value::sum(
                "Record",
                Some(Value::record(vec![
                    ("mirror", Value::Type(of.clone())),
                    ("fields", Value::array(views)),
                    ("build", builtin_value(Builtin::Build(of.clone()))),
                ])),
            )
        }
        Node::Sum(cases) => {
            let views = cases
                .iter()
                .map(|(name, index)| {
                    Value::record(vec![
                        ("name", Value::string(name)),
                        ("mirror", mirror_at(of, *index)),
                        (
                            "project",
                            builtin_value(Builtin::ProjectCase(Rc::from(name.as_str()))),
                        ),
                        (
                            "inject",
                            Value::some(builtin_value(Builtin::InjectCase(Rc::from(
                                name.as_str(),
                            )))),
                        ),
                    ])
                })
                .collect();
            Value::sum(
                "Sum",
                Some(Value::record(vec![
                    ("mirror", Value::Type(of.clone())),
                    ("cases", Value::array(views)),
                ])),
            )
        }
        Node::Arrow(_) => Value::sum("Function", Some(describe(domains, of))),
        Node::Hidden(_) => Value::sum("Hidden", Some(describe(domains, of))),
        Node::Mirror(_) => Value::sum("Mirror", Some(describe(domains, of))),
        _ => return Err("unknown runtime type node".into()),
    })
}

/// One operation of a typed view.
pub fn builtin(builtin: &Builtin, argument: Value) -> Result<Value, String> {
    Ok(match builtin {
        Builtin::PassThrough => argument,
        Builtin::ReadField(name) => Value::some(argument.field(name)),
        Builtin::Bind {
            record,
            name,
            field,
        } => Value::record(vec![
            ("record", Value::Type(record.clone())),
            ("name", Value::string(name)),
            ("mirror", Value::Type(field.clone())),
            ("value", argument),
        ]),
        Builtin::Build(of) => build(of, &argument)?,
        Builtin::ProjectCase(name) => {
            let sum = argument.as_sum()?;
            if *sum.tag == **name {
                Value::some(sum.payload.clone().unwrap_or_else(Value::unit))
            } else {
                Value::none()
            }
        }
        Builtin::InjectCase(name) => Value::sum(name, Some(argument)),
    })
}

/// Build a record from bindings. A binding is accepted on what it proves —
/// an equivalent mirror of this record and of the field it names — and never
/// on where it was made.
fn build(of: &Rc<Type>, bindings: &Value) -> Result<Value, String> {
    let Node::Struct(fields) = of.node(0).clone() else {
        return Err("building a record requires a record type".into());
    };
    let reject = |reason: &str, name: &str| {
        Ok(Value::sum(
            "Error",
            Some(Value::sum(reason, Some(Value::string(name)))),
        ))
    };
    let mut bound: Record = Record::new();
    for binding in bindings.as_array()?.iter() {
        let name = binding.field("name");
        let name = name.as_str()?;
        let Ok(record) = binding.field("record").as_type().cloned() else {
            return reject("Foreign", name);
        };
        if !crate::types::same(&record, of) {
            return reject("Foreign", name);
        }
        let Some((_, index)) = fields.iter().find(|(known, _)| known == name) else {
            return reject("Unknown", name);
        };
        let Ok(mirror) = binding.field("mirror").as_type().cloned() else {
            return reject("Mismatched", name);
        };
        if !crate::types::same(&mirror, &crate::types::at(of, *index)) {
            return reject("Mismatched", name);
        }
        if bound.contains_key(&Key::named(name)) {
            return reject("Duplicate", name);
        }
        bound.insert(Key::named(name), binding.field("value"));
    }
    if let Some((missing, _)) = fields
        .iter()
        .find(|(name, _)| !bound.contains_key(&Key::named(name)))
    {
        return reject("Missing", missing);
    }
    // The record is built in the type's field order, not the bindings'.
    let mut out = Record::new();
    for (name, _) in &fields {
        let key = Key::named(name);
        let value = bound.get(&key).cloned().unwrap_or(Value::Absent);
        out.insert(key, value);
    }
    Ok(Value::some(Value::Record(Rc::new(out))))
}
