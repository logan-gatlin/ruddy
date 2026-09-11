//! Runtime type descriptors: instantiating a template with arguments,
//! projecting one position of a graph, building a conversion plan, and
//! deciding whether two descriptors are one type.
//!
//! A descriptor is a finite graph of nodes; index 0 is its root and an
//! `Alias` node stands for another index. These operations mirror the ones
//! the JavaScript runtime performs on the same descriptors, over the
//! interpreter's own values.

use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    rc::Rc,
};

use ruddy::{
    backend::host::NativeTemplate,
    reification::{Descriptor, Effect, Node, Projection},
};

use crate::value::{Plan, Type, Value};

impl Type {
    /// A descriptor with no plan, not yet a mirror.
    pub fn plain(nodes: Vec<Node>) -> Rc<Self> {
        Rc::new(Self {
            nodes,
            plan: None,
            mirror: Cell::new(false),
        })
    }

    /// The index of the node an index stands for, through any aliases.
    pub fn index(&self, mut at: u32) -> u32 {
        while let Some(Node::Alias(next)) = self.nodes.get(at as usize) {
            at = *next;
        }
        at
    }

    /// The node an index stands for.
    pub fn node(&self, at: u32) -> &Node {
        &self.nodes[self.index(at) as usize]
    }

    /// Whether the host may leave a field of a record node out.
    pub fn optional(&self, shape: u32, name: &str) -> bool {
        self.plan
            .as_ref()
            .and_then(|plan| plan.optional.get(&shape))
            .is_some_and(|fields| fields.contains(name))
    }
}

/// A node with every child reference moved by an offset.
fn shifted(node: &Node, offset: u32) -> Node {
    let shift = |index: &u32| index + offset;
    match node {
        Node::Nat
        | Node::Int
        | Node::Fixed(_)
        | Node::Real
        | Node::String
        | Node::Bool
        | Node::ForeignValue
        | Node::Parameter(_)
        | Node::HiddenBound(_) => node.clone(),
        Node::Array(child) => Node::Array(shift(child)),
        Node::Mirror(child) => Node::Mirror(shift(child)),
        Node::Hidden(child) => Node::Hidden(shift(child)),
        Node::Alias(child) => Node::Alias(shift(child)),
        Node::Arrow(children) => Node::Arrow(children.map(|child| child + offset)),
        Node::Effects(effects) => Node::Effects(
            effects
                .iter()
                .map(|effect| Effect {
                    identity: effect.identity.clone(),
                    payload: shift(&effect.payload),
                    args: effect.args.iter().map(shift).collect(),
                })
                .collect(),
        ),
        Node::Extend(children) => Node::Extend(children.map(|child| child + offset)),
        Node::Struct(fields) => Node::Struct(
            fields
                .iter()
                .map(|(name, child)| (name.clone(), shift(child)))
                .collect(),
        ),
        Node::Sum(fields) => Node::Sum(
            fields
                .iter()
                .map(|(name, child)| (name.clone(), shift(child)))
                .collect(),
        ),
    }
}

/// The children of a node, in the order the JavaScript runtime visits them.
fn children(node: &Node) -> Vec<u32> {
    match node {
        Node::Nat
        | Node::Int
        | Node::Fixed(_)
        | Node::Real
        | Node::String
        | Node::Bool
        | Node::ForeignValue
        | Node::Parameter(_)
        | Node::HiddenBound(_) => Vec::new(),
        Node::Array(child) | Node::Mirror(child) | Node::Hidden(child) | Node::Alias(child) => {
            vec![*child]
        }
        Node::Arrow(children) => children.to_vec(),
        Node::Extend(children) => children.to_vec(),
        Node::Effects(effects) => effects
            .iter()
            .flat_map(|effect| std::iter::once(effect.payload).chain(effect.args.iter().copied()))
            .collect(),
        Node::Struct(fields) | Node::Sum(fields) => {
            fields.iter().map(|(_, child)| *child).collect()
        }
    }
}

/// Instantiate a template with descriptor arguments in parameter order. A
/// missing argument makes the whole descriptor missing. With `partial`, a
/// row extension over a parameter is left for a later instantiation.
pub fn instantiate(
    template: &Descriptor,
    arguments: &[Value],
    partial: bool,
) -> Result<Value, String> {
    if arguments
        .iter()
        .any(|argument| matches!(argument, Value::Absent))
    {
        return Ok(Value::Absent);
    }
    let arguments = arguments
        .iter()
        .map(|argument| argument.as_type().cloned())
        .collect::<Result<Vec<_>, _>>()?;
    let extends = template
        .nodes
        .iter()
        .any(|node| matches!(node, Node::Extend(_)));
    if arguments.is_empty() && !extends {
        return Ok(Value::Type(Type::plain(template.nodes.clone())));
    }
    let mut root = 0usize;
    while let Some(Node::Alias(next)) = template.nodes.get(root) {
        root = *next as usize;
    }
    if let Some(Node::Parameter(parameter)) = template.nodes.get(root) {
        return Ok(Value::Type(
            arguments
                .get(*parameter as usize)
                .ok_or("runtime type argument out of range")?
                .clone(),
        ));
    }
    let mut nodes = template.nodes.clone();
    let offsets: Vec<u32> = arguments
        .iter()
        .map(|argument| {
            let offset = nodes.len() as u32;
            nodes.extend(argument.nodes.iter().map(|node| shifted(node, offset)));
            offset
        })
        .collect();
    for node in nodes.iter_mut().take(template.nodes.len()) {
        if let Node::Parameter(parameter) = node {
            *node = Node::Alias(
                *offsets
                    .get(*parameter as usize)
                    .ok_or("runtime type argument out of range")?,
            );
        }
    }
    let lookup = Type::plain(nodes);
    let mut nodes = lookup.nodes.clone();
    for at in 0..template.nodes.len() {
        if !matches!(nodes[at], Node::Extend(_)) {
            continue;
        }
        let mut fields: Vec<(String, u32)> = Vec::new();
        let mut work = vec![at as u32];
        let mut seen = HashSet::new();
        let mut kind: Option<bool> = None;
        let mut deferred = false;
        while let Some(index) = work.pop() {
            let index = lookup.index(index);
            if !seen.insert(index) {
                return Err("cyclic runtime row extension".into());
            }
            match &lookup.nodes[index as usize] {
                Node::Parameter(_) if partial => {
                    deferred = true;
                    break;
                }
                Node::Extend(children) => work.extend(children.iter().copied()),
                Node::Struct(part) if kind != Some(false) => {
                    kind = Some(true);
                    fields.extend(part.iter().cloned());
                }
                Node::Sum(part) if kind != Some(true) => {
                    kind = Some(false);
                    fields.extend(part.iter().cloned());
                }
                _ => return Err("invalid runtime row extension".into()),
            }
        }
        if deferred {
            continue;
        }
        fields.sort_by(|left, right| left.0.cmp(&right.0));
        if fields.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("duplicate runtime row field".into());
        }
        nodes[at] = if kind == Some(false) {
            Node::Sum(fields)
        } else {
            Node::Struct(fields)
        };
    }
    Ok(Value::Type(Type::plain(nodes)))
}

/// One position of a descriptor as a descriptor of its own: the graph with
/// a new root aliasing that position.
pub fn at(of: &Type, index: u32) -> Rc<Type> {
    let mut nodes = Vec::with_capacity(of.nodes.len() + 1);
    nodes.push(Node::Alias(index + 1));
    nodes.extend(of.nodes.iter().map(|node| shifted(node, 1)));
    Type::plain(nodes)
}

/// Follow a projection path from the root of a descriptor.
pub fn project(descriptor: &Value, path: &[Projection]) -> Result<Value, String> {
    if matches!(descriptor, Value::Absent) {
        return Ok(Value::Absent);
    }
    let mut current = descriptor.as_type()?.clone();
    let mut index = 0u32;
    for step in path {
        index = current.index(index);
        let node = current.nodes[index as usize].clone();
        let next = match (step, &node) {
            (Projection::Element, Node::Array(child)) => Some(*child),
            (Projection::Argument, Node::Arrow(children)) => Some(children[0]),
            (Projection::Result, Node::Arrow(children)) => Some(children[1]),
            (Projection::Field(name), Node::Struct(fields) | Node::Sum(fields)) => fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, child)| *child),
            (Projection::Remainder(removed), Node::Struct(fields) | Node::Sum(fields)) => {
                let kept: Vec<(String, u32)> = fields
                    .iter()
                    .filter(|(name, _)| !removed.contains(name))
                    .map(|(name, child)| (name.clone(), child + 1))
                    .collect();
                let mut nodes = at(&current, index).nodes.clone();
                nodes[0] = if matches!(node, Node::Struct(_)) {
                    Node::Struct(kept)
                } else {
                    Node::Sum(kept)
                };
                current = Type::plain(nodes);
                Some(0)
            }
            (Projection::Remainder(_), _) => return Err("invalid runtime row projection".into()),
            _ => None,
        };
        index = next.ok_or("invalid runtime type projection")?;
    }
    Ok(Value::Type(at(&current, index)))
}

/// A conversion plan: the host's template instantiated with the arguments
/// known so far, remembering which fields the host may omit. Row extensions
/// inherit the optionality of their known constituent fields.
pub fn native_plan(template: &NativeTemplate, arguments: &[Value]) -> Result<Value, String> {
    let schema = &template.descriptor;
    let partial: Vec<Value> = arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| match argument {
            Value::Absent => Value::Type(Type::plain(vec![Node::Parameter(index as u32)])),
            other => other.clone(),
        })
        .collect();
    let instantiated = instantiate(schema, &partial, true)?;
    let nodes = instantiated.as_type()?.nodes.clone();
    let mut optional: BTreeMap<u32, BTreeSet<String>> = template.optional_fields.clone();
    for (index, node) in schema.nodes.iter().enumerate() {
        if !matches!(node, Node::Extend(_)) {
            continue;
        }
        let mut fields = BTreeSet::new();
        let mut pending = vec![index as u32];
        let mut seen = HashSet::new();
        while let Some(at) = pending.pop() {
            if !seen.insert(at) {
                continue;
            }
            if let Some(known) = template.optional_fields.get(&at) {
                fields.extend(known.iter().cloned());
            }
            match schema.nodes.get(at as usize) {
                Some(Node::Alias(child)) => pending.push(*child),
                Some(Node::Extend(children)) => pending.extend(children.iter().copied()),
                _ => {}
            }
        }
        if !fields.is_empty() {
            optional.insert(index as u32, fields);
        }
    }
    Ok(Value::Type(Rc::new(Type {
        nodes,
        plan: Some(Plan {
            template: template.clone(),
            arguments: arguments.to_vec(),
            optional,
        }),
        mirror: Cell::new(false),
    })))
}

/// The parameters an arrow of a plan's template still needs before a call
/// through it can convert: every parameter reachable from its argument, and
/// from its result short of a further arrow.
pub fn native_slots(plan: &Type, index: u32) -> Vec<u32> {
    let Some(schema) = plan.plan.as_ref().map(|plan| &plan.template.descriptor) else {
        return Vec::new();
    };
    let Some(Node::Arrow(arrow)) = schema.nodes.get(index as usize) else {
        return Vec::new();
    };
    let mut work = vec![(arrow[0], false), (arrow[1], true)];
    let mut seen = HashSet::new();
    let mut slots = BTreeSet::new();
    while let Some((index, defer)) = work.pop() {
        if !seen.insert((index, defer)) {
            continue;
        }
        let node = &schema.nodes[index as usize];
        match node {
            Node::Parameter(parameter) => {
                slots.insert(*parameter);
                continue;
            }
            Node::Arrow(_) if defer => continue,
            _ => {}
        }
        work.extend(children(node).into_iter().map(|child| (child, defer)));
    }
    slots.into_iter().collect()
}

/// Whether two descriptors describe one type: the same graph, primitives,
/// and effect contracts, compared up to aliasing and shared references.
pub fn same(left: &Type, right: &Type) -> bool {
    let mut work = vec![(0u32, 0u32)];
    let mut seen = HashSet::new();
    while let Some((a, b)) = work.pop() {
        let a = left.index(a);
        let b = right.index(b);
        if !seen.insert((a, b)) {
            continue;
        }
        let (Some(x), Some(y)) = (left.nodes.get(a as usize), right.nodes.get(b as usize)) else {
            return false;
        };
        match (x, y) {
            (Node::Nat, Node::Nat)
            | (Node::Int, Node::Int)
            | (Node::Real, Node::Real)
            | (Node::String, Node::String)
            | (Node::Bool, Node::Bool)
            | (Node::ForeignValue, Node::ForeignValue) => {}
            (Node::Fixed(p), Node::Fixed(q)) if p == q => {}
            (Node::Parameter(p), Node::Parameter(q))
            | (Node::HiddenBound(p), Node::HiddenBound(q))
                if p == q => {}
            (Node::Array(p), Node::Array(q))
            | (Node::Mirror(p), Node::Mirror(q))
            | (Node::Hidden(p), Node::Hidden(q)) => work.push((*p, *q)),
            (Node::Arrow(p), Node::Arrow(q)) => {
                work.extend(p.iter().copied().zip(q.iter().copied()));
            }
            (Node::Effects(p), Node::Effects(q)) => {
                if p.len() != q.len() {
                    return false;
                }
                for (e, f) in p.iter().zip(q) {
                    if e.identity != f.identity || e.args.len() != f.args.len() {
                        return false;
                    }
                    work.push((e.payload, f.payload));
                    work.extend(e.args.iter().copied().zip(f.args.iter().copied()));
                }
            }
            (Node::Extend(p), Node::Extend(q)) => {
                work.extend(p.iter().copied().zip(q.iter().copied()));
            }
            (Node::Struct(p), Node::Struct(q)) | (Node::Sum(p), Node::Sum(q)) => {
                if p.len() != q.len() {
                    return false;
                }
                for ((n, c), (m, d)) in p.iter().zip(q) {
                    if n != m {
                        return false;
                    }
                    work.push((*c, *d));
                }
            }
            _ => return false,
        }
    }
    true
}
