//! The contract expression graph has no type operands or compiler identities.
//! Encode references in a flat, topologically ordered table so sharing survives
//! disk round trips and malformed cycles cannot enter the evaluator.

use super::Contract;
use crate::contracts::{Arm, Expr, Pattern};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Graph {
    root: usize,
    nodes: Vec<Node>,
    patterns: Vec<PatternNode>,
}

#[derive(Serialize, Deserialize)]
enum Node {
    Input(usize),
    Capture(usize),
    Field(usize, String),
    Payload(usize, String),
    Record(Vec<(String, usize)>, Option<usize>),
    Tag(String, usize),
    Apply(usize, usize),
    Then(usize, usize),
    Match(usize, Vec<(usize, usize)>),
}

#[derive(Serialize, Deserialize)]
enum PatternNode {
    Any,
    Record(Vec<(String, usize)>, bool),
    Tag(String, usize),
}

fn children(expr: &Expr) -> Vec<&Arc<Expr>> {
    match expr {
        Expr::Input(_) | Expr::Capture(_) => Vec::new(),
        Expr::Field { base, .. } | Expr::Payload { base, .. } => vec![base],
        Expr::Record { fields, spread } => fields
            .iter()
            .map(|(_, value)| value)
            .chain(spread.iter())
            .collect(),
        Expr::Tag { payload, .. } => vec![payload],
        Expr::Apply { function, argument } => vec![function, argument],
        Expr::Then { value, body } => vec![value, body],
        Expr::Match { scrutinee, arms } => std::iter::once(scrutinee)
            .chain(arms.iter().map(|arm| &arm.body))
            .collect(),
    }
}

fn pattern(root: &Pattern, nodes: &mut Vec<PatternNode>) -> usize {
    let mut pending = vec![(root, false)];
    let mut ids = HashMap::new();
    while let Some((current, visited)) = pending.pop() {
        if !visited {
            pending.push((current, true));
            match current {
                Pattern::Any => {}
                Pattern::Record { fields, .. } => {
                    pending.extend(fields.iter().rev().map(|(_, child)| (child, false)))
                }
                Pattern::Tag { payload, .. } => pending.push((payload, false)),
            }
            continue;
        }
        let at = |child: &Pattern| ids[&(child as *const Pattern)];
        let node = match current {
            Pattern::Any => PatternNode::Any,
            Pattern::Record { fields, open } => PatternNode::Record(
                fields
                    .iter()
                    .map(|(name, child)| (name.clone(), at(child)))
                    .collect(),
                *open,
            ),
            Pattern::Tag { label, payload } => PatternNode::Tag(label.clone(), at(payload)),
        };
        ids.insert(current as *const Pattern, nodes.len());
        nodes.push(node);
    }
    ids[&(root as *const Pattern)]
}

pub(super) fn encode(root: &Arc<Expr>) -> String {
    let mut pending = vec![(root, false)];
    let mut ids = HashMap::new();
    let mut nodes = Vec::new();
    let mut patterns = Vec::new();
    while let Some((current, visited)) = pending.pop() {
        let pointer = Arc::as_ptr(current);
        if ids.contains_key(&pointer) {
            continue;
        }
        if !visited {
            pending.push((current, true));
            pending.extend(
                children(current)
                    .into_iter()
                    .rev()
                    .map(|child| (child, false)),
            );
            continue;
        }
        let at = |child: &Arc<Expr>| ids[&Arc::as_ptr(child)];
        let node = match current.as_ref() {
            Expr::Input(index) => Node::Input(*index),
            Expr::Capture(index) => Node::Capture(*index),
            Expr::Field { base, label } => Node::Field(at(base), label.clone()),
            Expr::Payload { base, label } => Node::Payload(at(base), label.clone()),
            Expr::Record { fields, spread } => Node::Record(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), at(value)))
                    .collect(),
                spread.as_ref().map(at),
            ),
            Expr::Tag { label, payload } => Node::Tag(label.clone(), at(payload)),
            Expr::Apply { function, argument } => Node::Apply(at(function), at(argument)),
            Expr::Then { value, body } => Node::Then(at(value), at(body)),
            Expr::Match { scrutinee, arms } => Node::Match(
                at(scrutinee),
                arms.iter()
                    .map(|arm| (pattern(&arm.pattern, &mut patterns), at(&arm.body)))
                    .collect(),
            ),
        };
        ids.insert(pointer, nodes.len());
        nodes.push(node);
    }
    serde_json::to_string(&Graph {
        root: ids[&Arc::as_ptr(root)],
        nodes,
        patterns,
    })
    .expect("serializable contract graph")
}

fn take_pattern(patterns: &mut [Option<Pattern>], index: usize) -> Result<Pattern, &'static str> {
    patterns
        .get_mut(index)
        .and_then(Option::take)
        .ok_or("contract pattern reference must name an unused preceding node")
}

pub(super) fn decode(text: &str) -> Result<Arc<Expr>, &'static str> {
    let graph: Graph =
        serde_json::from_str(text).map_err(|_| "invalid contract expression graph")?;
    if graph.nodes.len() > crate::contracts::MAX_NODES {
        return Err("contract expression graph exceeds its node bound");
    }
    if graph.patterns.len() > crate::contracts::MAX_NODES {
        return Err("contract pattern graph exceeds its node bound");
    }
    let mut patterns = Vec::with_capacity(graph.patterns.len());
    let mut pattern_depths = Vec::with_capacity(graph.patterns.len());
    for node in graph.patterns {
        let references: Vec<_> = match &node {
            PatternNode::Any => Vec::new(),
            PatternNode::Record(fields, _) => fields.iter().map(|(_, index)| *index).collect(),
            PatternNode::Tag(_, index) => vec![*index],
        };
        let depth = depth(&references, &pattern_depths)?;
        let pattern = match node {
            PatternNode::Any => Pattern::Any,
            PatternNode::Record(fields, open) => Pattern::Record {
                fields: fields
                    .into_iter()
                    .map(|(name, index)| Ok((name, take_pattern(&mut patterns, index)?)))
                    .collect::<Result<Vec<_>, &'static str>>()?
                    .into(),
                open,
            },
            PatternNode::Tag(label, index) => Pattern::Tag {
                label,
                payload: Box::new(take_pattern(&mut patterns, index)?),
            },
        };
        patterns.push(Some(pattern));
        pattern_depths.push(depth);
    }
    let mut nodes: Vec<Arc<Expr>> = Vec::with_capacity(graph.nodes.len());
    let mut depths = Vec::with_capacity(graph.nodes.len());
    for node in graph.nodes {
        let references: Vec<_> = match &node {
            Node::Input(_) | Node::Capture(_) => Vec::new(),
            Node::Field(index, _) | Node::Payload(index, _) | Node::Tag(_, index) => vec![*index],
            Node::Record(fields, spread) => fields
                .iter()
                .map(|(_, index)| *index)
                .chain(spread.iter().copied())
                .collect(),
            Node::Apply(left, right) | Node::Then(left, right) => vec![*left, *right],
            Node::Match(scrutinee, arms) => std::iter::once(*scrutinee)
                .chain(arms.iter().map(|(_, body)| *body))
                .collect(),
        };
        let depth = depth(&references, &depths)?;
        let at = |index: usize| {
            nodes
                .get(index)
                .cloned()
                .ok_or("contract expression reference must name a preceding node")
        };
        let expr = match node {
            Node::Input(index) => Expr::Input(index),
            Node::Capture(index) => Expr::Capture(index),
            Node::Field(index, label) => Expr::Field {
                base: at(index)?,
                label,
            },
            Node::Payload(index, label) => Expr::Payload {
                base: at(index)?,
                label,
            },
            Node::Record(fields, spread) => Expr::Record {
                fields: fields
                    .into_iter()
                    .map(|(name, index)| Ok((name, at(index)?)))
                    .collect::<Result<Vec<_>, &'static str>>()?
                    .into(),
                spread: spread.map(at).transpose()?,
            },
            Node::Tag(label, index) => Expr::Tag {
                label,
                payload: at(index)?,
            },
            Node::Apply(function, argument) => Expr::Apply {
                function: at(function)?,
                argument: at(argument)?,
            },
            Node::Then(value, body) => Expr::Then {
                value: at(value)?,
                body: at(body)?,
            },
            Node::Match(scrutinee, arms) => Expr::Match {
                scrutinee: at(scrutinee)?,
                arms: arms
                    .into_iter()
                    .map(|(pattern, body)| {
                        Ok(Arm {
                            pattern: take_pattern(&mut patterns, pattern)?,
                            body: at(body)?,
                        })
                    })
                    .collect::<Result<Vec<_>, &'static str>>()?
                    .into(),
            },
        };
        nodes.push(Arc::new(expr));
        depths.push(depth);
    }
    if graph.root.checked_add(1) != Some(nodes.len()) || patterns.iter().any(Option::is_some) {
        return Err("contract graph has an invalid root or unused pattern");
    }
    nodes.pop().ok_or("contract graph is empty")
}

fn depth(references: &[usize], prior: &[usize]) -> Result<usize, &'static str> {
    let mut depth = 0;
    for index in references {
        let child = prior
            .get(*index)
            .ok_or("contract graph reference must name a preceding node")?;
        depth = depth.max(child + 1);
    }
    if depth > crate::contracts::MAX_DEPTH {
        return Err("contract expression graph exceeds its depth bound");
    }
    Ok(depth)
}

pub(super) fn validate(contract: &Contract) -> Result<(), &'static str> {
    if contract.parameters > crate::contracts::MAX_NODES
        || contract.captures.len() > crate::contracts::MAX_NODES
    {
        return Err("contract parameter or capture count exceeds its structural bound");
    }
    if contract.arguments.len() > contract.parameters {
        return Err("contract arguments exceed its function parameters");
    }
    if contract
        .effect_sink
        .as_ref()
        .is_some_and(|sink| !matches!(sink.unshared(), super::Type::Arrow(..)))
    {
        return Err("contract effect sink is not an arrow");
    }
    let placeholder = Arc::new(crate::types::Ty::Undecided);
    let semantic = crate::contracts::Contract {
        parameters: contract.parameters,
        body: contract.body.clone(),
        captures: vec![placeholder.clone(); contract.captures.len()].into(),
        arguments: vec![placeholder.clone(); contract.arguments.len()].into(),
        effect_sink: contract.effect_sink.as_ref().map(|_| {
            Arc::new(crate::types::Ty::Arrow(
                placeholder.clone(),
                placeholder.clone(),
                crate::types::Row::of(crate::types::Rest::Closed),
            ))
        }),
    };
    if !semantic.validate() {
        return Err("contract graph has invalid references or exceeds its structural bounds");
    }
    let mut pending = vec![&contract.body];
    let mut visited = HashSet::new();
    while let Some(expr) = pending.pop() {
        if !visited.insert(Arc::as_ptr(expr)) {
            continue;
        }
        match expr.as_ref() {
            Expr::Input(index) if *index >= contract.parameters => {
                return Err("contract input is outside its parameter space");
            }
            Expr::Capture(index) if *index >= contract.captures.len() => {
                return Err("contract capture is outside its capture space");
            }
            Expr::Record { fields, .. } => {
                let mut names = HashSet::new();
                if fields.iter().any(|(name, _)| !names.insert(name)) {
                    return Err("contract record repeats a field");
                }
            }
            Expr::Match { arms, .. } => {
                let mut patterns: Vec<_> = arms.iter().map(|arm| &arm.pattern).collect();
                while let Some(pattern) = patterns.pop() {
                    match pattern {
                        Pattern::Any => {}
                        Pattern::Record { fields, .. } => {
                            let mut names = HashSet::new();
                            if fields.iter().any(|(name, _)| !names.insert(name)) {
                                return Err("contract pattern repeats a field");
                            }
                            patterns.extend(fields.iter().map(|(_, child)| child));
                        }
                        Pattern::Tag { payload, .. } => patterns.push(payload),
                    }
                }
            }
            _ => {}
        }
        pending.extend(children(expr));
    }
    Ok(())
}

pub(super) fn annotation_cases(contract: &Contract) -> Option<Vec<(&super::Type, &super::Type)>> {
    use super::{Presence, Rest, Type};
    fn pattern(ty: &Type, expected: &Pattern) -> bool {
        let mut pending = vec![(ty, expected)];
        while let Some((ty, expected)) = pending.pop() {
            match ty.unshared() {
                Type::Struct(row) => {
                    let Pattern::Record { fields, open } = expected else {
                        return false;
                    };
                    let present: Vec<_> = row
                        .labels
                        .iter()
                        .filter(|(_, field)| matches!(field.presence, Presence::Present))
                        .collect();
                    if fields.len() != present.len()
                        || *open
                            != (!matches!(row.rest, Rest::Closed)
                                || row.labels.iter().any(|(_, field)| {
                                    !matches!(field.presence, Presence::Present | Presence::Absent)
                                }))
                    {
                        return false;
                    }
                    for ((label, inner), (name, field)) in fields.iter().zip(present) {
                        if label != name {
                            return false;
                        }
                        pending.push((&field.ty, inner));
                    }
                }
                Type::Sum(row)
                    if matches!(row.rest, Rest::Closed)
                        && row
                            .labels
                            .iter()
                            .filter(|(_, field)| !matches!(field.presence, Presence::Absent))
                            .count()
                            == 1 =>
                {
                    let Pattern::Tag { label, payload } = expected else {
                        return false;
                    };
                    let (name, field) = row
                        .labels
                        .iter()
                        .find(|(_, field)| !matches!(field.presence, Presence::Absent))
                        .unwrap();
                    if label != name {
                        return false;
                    }
                    pending.push((&field.ty, payload));
                }
                _ if matches!(expected, Pattern::Any) => {}
                _ => return false,
            }
        }
        true
    }
    if contract.parameters != 1 || !contract.arguments.is_empty() || contract.effect_sink.is_some()
    {
        return None;
    }
    let Expr::Match { scrutinee, arms } = contract.body.as_ref() else {
        return None;
    };
    if !matches!(scrutinee.as_ref(), Expr::Input(0)) {
        return None;
    }
    arms.iter()
        .map(|arm| {
            let Expr::Apply { function, argument } = arm.body.as_ref() else {
                return None;
            };
            if !matches!(argument.as_ref(), Expr::Input(0)) {
                return None;
            }
            let Expr::Capture(index) = function.as_ref() else {
                return None;
            };
            let Type::Arrow(from, to, effects) = contract.captures.get(*index)?.unshared() else {
                return None;
            };
            (effects.labels.is_empty()
                && matches!(effects.rest, Rest::Closed)
                && pattern(from, &arm.pattern))
            .then_some((from.as_ref(), to.as_ref()))
        })
        .collect()
}
