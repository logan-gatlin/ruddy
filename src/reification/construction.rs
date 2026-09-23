//! Finite construction of immutable structural data. Possibility and authority
//! are separate: an opaque type may have values without exposing constructors.

use super::{Descriptor, Node};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

/// A row extension takes its constructor from its leftmost fragment. Every
/// other fragment contributes fields, even if it has a different constructor.
/// Unresolved tails retain their evidence predicate until instantiation.
fn row(descriptor: &Descriptor, at: u32) -> (bool, Vec<(String, u32)>, Vec<u32>) {
    let mut record = None;
    let mut fields = Vec::new();
    let mut tails = Vec::new();
    let mut pending = vec![at];
    let mut seen = HashSet::new();
    while let Some(at) = pending.pop() {
        if !seen.insert(at) {
            continue;
        }
        match &descriptor.nodes[at as usize] {
            Node::Alias(child) => pending.push(*child),
            Node::Extend([left, right]) => pending.extend([*right, *left]),
            Node::Struct(part) | Node::Sum(part) => {
                record.get_or_insert(matches!(descriptor.nodes[at as usize], Node::Struct(_)));
                fields.extend(part.iter().cloned());
            }
            _ => tails.push(at),
        }
    }
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    (record.unwrap_or(false), fields, tails)
}

/// Positive facts about one quantified type. Empty and Constructible are
/// mutually exclusive; an opaque type satisfies neither.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Property {
    Constructible,
    Empty,
    ConstructibleRecordRow,
    EmptyRecordRow,
    ConstructibleSumRow,
    EmptySumRow,
}

/// A canonical disjunction of conjunctions. Absorption keeps recursive
/// substitutions on a finite carrier rather than growing type expressions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Condition(pub BTreeSet<BTreeSet<(u32, Property)>>);

impl Default for Condition {
    fn default() -> Self {
        Self::yes()
    }
}

impl Condition {
    pub fn yes() -> Self {
        Self(BTreeSet::from([BTreeSet::new()]))
    }
    pub fn no() -> Self {
        Self(BTreeSet::new())
    }
    pub fn atom(parameter: u32, property: Property) -> Self {
        Self(BTreeSet::from([BTreeSet::from([(parameter, property)])]))
    }
    pub fn is_true(&self) -> bool {
        self.0.contains(&BTreeSet::new())
    }
    pub fn is_false(&self) -> bool {
        self.0.is_empty()
    }
    pub fn parameters(&self) -> BTreeSet<u32> {
        self.0
            .iter()
            .flat_map(|term| term.iter().map(|(p, _)| *p))
            .collect()
    }
    fn insert(&mut self, term: BTreeSet<(u32, Property)>) {
        if term.iter().any(|(p, kind)| {
            term.contains(&(
                *p,
                match kind {
                    Property::Empty => Property::Constructible,
                    Property::Constructible => Property::Empty,
                    Property::ConstructibleRecordRow => Property::EmptyRecordRow,
                    Property::EmptyRecordRow => Property::ConstructibleRecordRow,
                    Property::ConstructibleSumRow => Property::EmptySumRow,
                    Property::EmptySumRow => Property::ConstructibleSumRow,
                },
            ))
        }) || self.0.iter().any(|known| known.is_subset(&term))
        {
            return;
        }
        self.0.retain(|known| !term.is_subset(known));
        self.0.insert(term);
    }
    pub fn and(&self, other: &Self) -> Self {
        if self.is_false() || other.is_false() {
            return Self::no();
        }
        if self.is_true() {
            return other.clone();
        }
        if other.is_true() || self == other {
            return self.clone();
        }
        let mut result = Self::no();
        for left in &self.0 {
            for right in &other.0 {
                result.insert(left.union(right).copied().collect());
            }
        }
        result
    }
    pub fn or(&self, other: &Self) -> Self {
        if self.is_true() || other.is_true() {
            return Self::yes();
        }
        if self.is_false() {
            return other.clone();
        }
        if other.is_false() || self == other {
            return self.clone();
        }
        let mut result = self.clone();
        for term in &other.0 {
            result.insert(term.clone());
        }
        result
    }
    pub fn substitute(&self, facts: &BTreeMap<u32, Facts>) -> Self {
        if self.is_true() || self.is_false() {
            return self.clone();
        }
        let mut result = Self::no();
        for term in &self.0 {
            let mut substituted = Self::yes();
            for (p, property) in term {
                let fact = facts
                    .get(p)
                    .map(|f| match property {
                        Property::Constructible => &f.constructible,
                        Property::Empty => &f.empty,
                        Property::ConstructibleRecordRow => &f.record_constructible,
                        Property::EmptyRecordRow => &f.record_empty,
                        Property::ConstructibleSumRow => &f.sum_constructible,
                        Property::EmptySumRow => &f.sum_empty,
                    })
                    .cloned()
                    .unwrap_or_else(|| Self::atom(*p, *property));
                substituted = substituted.and(&fact);
            }
            result = result.or(&substituted);
        }
        result
    }
    pub fn retain(&self, parameters: &BTreeSet<u32>) -> Self {
        if self.is_true() || self.is_false() {
            return self.clone();
        }
        let mut result = Self::no();
        for term in &self.0 {
            result.insert(
                term.iter()
                    .filter(|(p, _)| parameters.contains(p))
                    .copied()
                    .collect(),
            );
        }
        result
    }
    pub fn rename(&self, parameters: &std::collections::HashMap<u32, u32>) -> Self {
        let mut result = Self::no();
        for term in &self.0 {
            result.insert(
                term.iter()
                    .filter_map(|(p, property)| parameters.get(p).map(|p| (*p, *property)))
                    .collect(),
            );
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub constructible: Condition,
    pub empty: Condition,
    pub record_constructible: Condition,
    pub record_empty: Condition,
    pub sum_constructible: Condition,
    pub sum_empty: Condition,
}

impl Facts {
    pub fn parameter(p: u32) -> Self {
        Self {
            constructible: Condition::atom(p, Property::Constructible),
            empty: Condition::atom(p, Property::Empty),
            record_constructible: Condition::atom(p, Property::ConstructibleRecordRow),
            record_empty: Condition::atom(p, Property::EmptyRecordRow),
            sum_constructible: Condition::atom(p, Property::ConstructibleSumRow),
            sum_empty: Condition::atom(p, Property::EmptySumRow),
        }
    }
    pub fn unavailable() -> Self {
        Self {
            constructible: Condition::no(),
            empty: Condition::no(),
            record_constructible: Condition::no(),
            record_empty: Condition::no(),
            sum_constructible: Condition::no(),
            sum_empty: Condition::no(),
        }
    }
    pub fn substitute(&self, arguments: &BTreeMap<u32, Facts>) -> Self {
        Self {
            constructible: self.constructible.substitute(arguments),
            empty: self.empty.substitute(arguments),
            record_constructible: self.record_constructible.substitute(arguments),
            record_empty: self.record_empty.substitute(arguments),
            sum_constructible: self.sum_constructible.substitute(arguments),
            sum_empty: self.sum_empty.substitute(arguments),
        }
    }
    pub fn of(
        ty: &std::sync::Arc<crate::types::Ty>,
        aliases: &indexmap::IndexMap<crate::symbol::Symbol, crate::types::Scheme>,
    ) -> Self {
        if let Some(p) = super::evidence_slot(ty) {
            return Self::parameter(p);
        }
        use crate::types::Ty;
        match ty.as_ref() {
            Ty::Nat | Ty::Int | Ty::Fixed(_) | Ty::Real | Ty::String | Ty::Bool => {
                return Self {
                    constructible: Condition::yes(),
                    ..Self::unavailable()
                };
            }
            Ty::Arrow(..)
            | Ty::ForeignValue
            | Ty::Mut(..)
            | Ty::Hidden { .. }
            | Ty::HiddenVar { .. }
            | Ty::Mirror(_)
            | Ty::TypeInfo(_) => return Self::unavailable(),
            _ => {}
        }
        let Ok((descriptor, parameters)) = Descriptor::template(ty, aliases) else {
            return Self::unavailable();
        };
        Self::template(&descriptor, &parameters)
    }
    fn row_facts(
        descriptor: &Descriptor,
        at: u32,
        parameters: &[u32],
        record: bool,
        empty: &[Condition],
        finite: &[Condition],
        supported: &[Condition],
    ) -> (Condition, Condition, Condition) {
        let (_, fields, tails) = row(descriptor, at);
        let mut emptiness = if record {
            Condition::no()
        } else {
            Condition::yes()
        };
        let mut construction = if record {
            Condition::yes()
        } else {
            Condition::no()
        };
        let mut support = Condition::yes();
        let mut include = |e: &Condition, f: &Condition, s: &Condition| {
            emptiness = if record {
                emptiness.or(e)
            } else {
                emptiness.and(e)
            };
            construction = if record {
                construction.and(f)
            } else {
                construction.or(f)
            };
            support = support.and(&e.or(s));
        };
        for (_, child) in fields {
            let child = child as usize;
            include(&empty[child], &finite[child], &supported[child]);
        }
        for tail in tails {
            let Node::Parameter(index) = descriptor.nodes[tail as usize] else {
                return (Condition::no(), Condition::no(), Condition::no());
            };
            let parameter = parameters[index as usize];
            let e = Condition::atom(
                parameter,
                if record {
                    Property::EmptyRecordRow
                } else {
                    Property::EmptySumRow
                },
            );
            let c = Condition::atom(
                parameter,
                if record {
                    Property::ConstructibleRecordRow
                } else {
                    Property::ConstructibleSumRow
                },
            );
            include(&e, &c, &c);
        }
        (emptiness, construction, support)
    }
    pub fn template(descriptor: &Descriptor, parameters: &[u32]) -> Self {
        let count = descriptor.nodes.len();
        let mut reachable = BTreeSet::new();
        let mut pending = vec![0];
        while let Some(at) = pending.pop() {
            if !reachable.insert(at) {
                continue;
            }
            match &descriptor.nodes[at] {
                Node::Alias(child) | Node::Array(child) => pending.push(*child as usize),
                Node::Struct(fields) | Node::Sum(fields) => {
                    pending.extend(fields.iter().map(|(_, child)| *child as usize))
                }
                Node::Extend(children) => {
                    pending.extend(children.iter().map(|child| *child as usize))
                }
                _ => {}
            }
        }
        let mut empty = vec![Condition::yes(); count];
        let mut finite = vec![Condition::no(); count];
        let mut supported = vec![Condition::yes(); count];
        // Empty is a greatest fixed point, finite construction a least fixed
        // point, and complete constructor support a greatest fixed point.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Phase {
            Empty,
            Finite,
            Supported,
        }
        for phase in [Phase::Empty, Phase::Finite, Phase::Supported] {
            loop {
                let mut changed = false;
                for &at in &reachable {
                    let node = &descriptor.nodes[at];
                    let values = match phase {
                        Phase::Empty => &empty,
                        Phase::Finite => &finite,
                        Phase::Supported => &supported,
                    };
                    let all = |children: Vec<u32>| {
                        children
                            .iter()
                            .fold(Condition::yes(), |a, p| a.and(&values[*p as usize]))
                    };
                    let any = |children: Vec<u32>| {
                        children
                            .iter()
                            .fold(Condition::no(), |a, p| a.or(&values[*p as usize]))
                    };
                    let next = match node {
                        Node::Parameter(index) => {
                            let facts = Self::parameter(parameters[*index as usize]);
                            match phase {
                                Phase::Empty => facts.empty,
                                Phase::Finite => facts.constructible,
                                _ => facts.empty.or(&facts.constructible),
                            }
                        }
                        Node::Nat
                        | Node::Int
                        | Node::Fixed(_)
                        | Node::Real
                        | Node::String
                        | Node::Bool => {
                            if phase == Phase::Empty {
                                Condition::no()
                            } else {
                                Condition::yes()
                            }
                        }
                        Node::Alias(child) => values[*child as usize].clone(),
                        Node::Array(child) => match phase {
                            Phase::Empty => Condition::no(),
                            Phase::Finite => Condition::yes(),
                            _ => empty[*child as usize].or(&values[*child as usize]),
                        },
                        Node::Struct(fields) => {
                            let children = fields.iter().map(|(_, p)| *p).collect();
                            match phase {
                                Phase::Empty => any(children),
                                Phase::Finite => all(children),
                                _ => empty[at].or(&all(children)),
                            }
                        }
                        Node::Sum(fields) => {
                            let children = fields.iter().map(|(_, p)| *p).collect();
                            match phase {
                                Phase::Empty => all(children),
                                Phase::Finite => any(children),
                                _ => fields.iter().fold(Condition::yes(), |a, (_, p)| {
                                    a.and(&empty[*p as usize].or(&values[*p as usize]))
                                }),
                            }
                        }
                        Node::Extend(_) => {
                            let (record, _, _) = row(descriptor, at as u32);
                            let (e, f, s) = Self::row_facts(
                                descriptor, at as u32, parameters, record, &empty, &finite,
                                &supported,
                            );
                            match phase {
                                Phase::Empty => e,
                                Phase::Finite => f,
                                Phase::Supported => {
                                    if record {
                                        empty[at].or(&s)
                                    } else {
                                        s
                                    }
                                }
                            }
                        }
                        _ => Condition::no(),
                    };
                    let values = match phase {
                        Phase::Empty => &mut empty,
                        Phase::Finite => &mut finite,
                        Phase::Supported => &mut supported,
                    };
                    if values[at] != next {
                        values[at] = next;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        }
        let (record_empty, record_finite, record_supported) =
            Self::row_facts(descriptor, 0, parameters, true, &empty, &finite, &supported);
        let (sum_empty, sum_finite, sum_supported) = Self::row_facts(
            descriptor, 0, parameters, false, &empty, &finite, &supported,
        );
        Self {
            constructible: finite[0].and(&supported[0]),
            empty: empty[0].clone(),
            record_constructible: record_finite.and(&record_supported),
            record_empty,
            sum_constructible: sum_finite.and(&sum_supported),
            sum_empty,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Demand {
    pub condition: Condition,
    pub ports: BTreeSet<u32>,
}

impl Demand {
    pub fn include(&mut self, other: &Self) {
        self.condition = self.condition.and(&other.condition);
        self.ports.extend(&other.ports);
    }
}

impl super::conventions::Graph {
    /// A concrete callable cannot carry an impossible invocation requirement,
    /// even when this expression only stores or returns it. Follow actual
    /// planned values, never fabricate demands from an opaque value's type.
    pub(super) fn unavailable_construction_values(&self, demands: &[Demand]) -> Vec<bool> {
        use super::conventions::Shape;
        let mut unavailable = vec![false; self.shapes.len()];
        let mut parents = vec![Vec::new(); self.shapes.len()];
        let mut pending = VecDeque::new();
        for (id, shape) in self.shapes.iter().enumerate() {
            let mut child = |value: u32| parents[value as usize].push(id);
            match shape {
                Shape::Alias(value) | Shape::Array(value) | Shape::Cell(value) => child(*value),
                Shape::Record(fields) | Shape::Sum(fields) => {
                    for value in fields.values() {
                        child(*value);
                    }
                }
                Shape::Arrow { result, needs, .. } => {
                    // Arguments describe accepted input, not a possessed value.
                    child(*result);
                    if demands[*needs as usize].condition.is_false() {
                        unavailable[id] = true;
                        pending.push_back(id);
                    }
                }
                _ => {}
            }
        }
        while let Some(id) = pending.pop_front() {
            for parent in &parents[id] {
                if !unavailable[*parent] {
                    unavailable[*parent] = true;
                    pending.push_back(*parent);
                }
            }
        }
        unavailable
    }

    /// Construction requirements use the same invocation and captured-value
    /// ports as descriptive evidence, but retain predicates on whole types.
    pub fn solve_construction(&self) -> Vec<Demand> {
        let mut values: Vec<_> = self
            .needs
            .iter()
            .enumerate()
            .map(|(id, need)| {
                let mut demand = need.construction.clone();
                if self.free_parameters(id as u32).is_some() {
                    demand.ports.insert(self.need_root(id as u32));
                }
                demand
            })
            .collect();
        let super::conventions::DependencyIndex {
            mut dependents,
            group_targets,
            group_of,
            ..
        } = self.dependency_index();
        // Port targets become dependencies only when this instance actually
        // reads that port. Its source is already a permanent dependency, so a
        // newly discovered source port installs its target edges before use.
        // Waking every member of a shared instantiation for every target
        // change makes long forwarding chains needlessly quadratic per round.
        let mut observed = vec![HashSet::new(); self.needs.len()];
        let mut queue: VecDeque<_> = (0..self.needs.len()).collect();
        let mut queued = vec![true; self.needs.len()];
        while let Some(id) = queue.pop_front() {
            queued[id] = false;
            let need = &self.needs[id];
            let mut added = need
                .redirect
                .map(|source| values[source as usize].clone())
                .unwrap_or_default();
            for edge in &need.edges {
                let mut demand = values[edge.source as usize].clone();
                if let Some(retain) = edge.construction_retain.as_ref().or(edge.retain.as_ref()) {
                    demand.condition = demand.condition.retain(retain);
                }
                added.include(&demand);
            }
            if let Some(instance) = &need.instance {
                let source = &values[instance.source as usize];
                added.condition = added.condition.and(
                    &source
                        .condition
                        .substitute(&instance.construction_substitute),
                );
                for port in &source.ports {
                    let targets: Vec<_> = group_targets
                        [group_of[id].expect("an instance is in its group")]
                    .get(&self.need_root(*port))
                    .into_iter()
                    .flatten()
                    .copied()
                    .collect();
                    if targets.is_empty() {
                        added.ports.insert(*port);
                    } else {
                        for target in targets {
                            if target != id as u32 {
                                if observed[id].insert(target) {
                                    dependents[target as usize].push(id);
                                }
                                added.include(&values[target as usize]);
                            }
                        }
                    }
                }
            }
            let mut combined = values[id].clone();
            combined.include(&added);
            if combined != values[id] {
                values[id] = combined;
                for target in &dependents[id] {
                    if !queued[*target] {
                        queued[*target] = true;
                        queue.push_back(*target);
                    }
                }
            }
        }
        values
    }
}

#[derive(Debug, Clone)]
pub struct Construction {
    /// A finite value may exist. Unknown parameters and opaque types count as
    /// possible; only a proof of emptiness permits pruning a nested position.
    pub possible: Vec<bool>,
    /// A decreasing rank for a finite construction. Variants choose the first
    /// case of minimum rank, in the descriptor's canonical field order.
    pub rank: Vec<Option<usize>>,
}

impl Construction {
    pub fn analyze(descriptor: &Descriptor) -> Self {
        let mut possible = vec![false; descriptor.nodes.len()];
        let mut rank = vec![None; descriptor.nodes.len()];
        let mut parents = vec![Vec::new(); descriptor.nodes.len()];
        for (at, node) in descriptor.nodes.iter().enumerate() {
            let children: Vec<_> = match node {
                Node::Alias(child) | Node::Array(child) | Node::Mirror(child) => vec![*child],
                Node::Struct(fields) | Node::Sum(fields) => {
                    fields.iter().map(|(_, child)| *child).collect()
                }
                Node::Extend(_) => {
                    let (_, fields, tails) = row(descriptor, at as u32);
                    fields
                        .into_iter()
                        .map(|(_, child)| child)
                        .chain(tails)
                        .collect()
                }
                _ => Vec::new(),
            };
            for child in children {
                parents[child as usize].push(at);
            }
        }
        let mut pending: VecDeque<_> = (0..descriptor.nodes.len()).collect();
        let mut queued = vec![true; descriptor.nodes.len()];
        while let Some(at) = pending.pop_front() {
            queued[at] = false;
            let (inhabited, height) = match &descriptor.nodes[at] {
                Node::Nat | Node::Int | Node::Fixed(_) | Node::Real | Node::String | Node::Bool => {
                    (true, Some(0))
                }
                Node::Array(_) => (true, Some(0)),
                Node::Alias(child) => (possible[*child as usize], rank[*child as usize]),
                Node::Struct(fields) => (
                    fields.iter().all(|(_, child)| possible[*child as usize]),
                    fields
                        .iter()
                        .map(|(_, child)| rank[*child as usize])
                        .collect::<Option<Vec<_>>>()
                        .map(|ranks| ranks.into_iter().max().map_or(0, |rank| rank + 1)),
                ),
                Node::Sum(fields) => (
                    fields.iter().any(|(_, child)| possible[*child as usize]),
                    fields
                        .iter()
                        .filter_map(|(_, child)| rank[*child as usize])
                        .min()
                        .map(|rank| rank + 1),
                ),
                Node::Extend(_) => {
                    let (record, fields, tails) = row(descriptor, at as u32);
                    let children: Vec<_> = fields
                        .iter()
                        .map(|(_, child)| *child)
                        .chain(tails)
                        .collect();
                    if record {
                        (
                            children.iter().all(|child| possible[*child as usize]),
                            children
                                .iter()
                                .map(|child| rank[*child as usize])
                                .collect::<Option<Vec<_>>>()
                                .map(|ranks| ranks.into_iter().max().map_or(0, |rank| rank + 1)),
                        )
                    } else {
                        (
                            children.iter().any(|child| possible[*child as usize]),
                            children
                                .iter()
                                .filter_map(|child| rank[*child as usize])
                                .min()
                                .map(|rank| rank + 1),
                        )
                    }
                }
                // Parameters are checked when their caller supplies a concrete
                // type. Opaque types may be inhabited but have no recipe here.
                Node::Parameter(_) => (true, Some(0)),
                _ => (true, None),
            };
            if possible[at] != inhabited || rank[at] != height {
                possible[at] = inhabited;
                rank[at] = height;
                for parent in &parents[at] {
                    if !queued[*parent] {
                        queued[*parent] = true;
                        pending.push_back(*parent);
                    }
                }
            }
        }
        Self { possible, rank }
    }

    /// Check every potentially inhabited part, not just a selected default.
    /// Otherwise unavailable constructors would silently remove valid cases.
    pub fn review(&self, descriptor: &Descriptor) -> Result<(), String> {
        if !self.possible[0] {
            return Err(
                "construction evidence is unavailable: this type has no finite value".into(),
            );
        }
        let mut seen = HashSet::new();
        let mut pending = VecDeque::from([(0, "$".to_owned())]);
        while let Some((at, path)) = pending.pop_front() {
            if !seen.insert(at) || !self.possible[at as usize] {
                continue;
            }
            match &descriptor.nodes[at as usize] {
                Node::Nat
                | Node::Int
                | Node::Fixed(_)
                | Node::Real
                | Node::String
                | Node::Bool
                | Node::Parameter(_) => {}
                Node::Alias(child) => pending.push_back((*child, path)),
                Node::Array(child) => pending.push_back((*child, format!("{path}[]"))),
                Node::Extend(_) => {
                    let (_, fields, tails) = row(descriptor, at);
                    pending.extend(
                        fields
                            .into_iter()
                            .map(|(name, child)| (child, format!("{path}.{name}"))),
                    );
                    pending.extend(tails.into_iter().map(|child| (child, path.clone())));
                }
                Node::Struct(fields) | Node::Sum(fields) => {
                    for (name, child) in fields {
                        pending.push_back((*child, format!("{path}.{name}")));
                    }
                }
                node => {
                    let reason = match node {
                        Node::Arrow(_) => "function values have no automatic constructor",
                        Node::Cell(_) => "cells require scoped allocation",
                        Node::ForeignValue => {
                            "foreign values require explicit construction authority"
                        }
                        Node::Hidden(_) | Node::HiddenBound(_) => {
                            "hidden types require scoped construction evidence"
                        }
                        Node::Mirror(_) => {
                            "mirror construction requires evidence for its indexed type"
                        }
                        _ => "the type's complete construction evidence is unavailable",
                    };
                    return Err(format!(
                        "construction evidence is unavailable at {path}: {reason}"
                    ));
                }
            }
        }
        if self.rank[0].is_none() {
            return Err(
                "construction evidence is unavailable: no finite construction recipe exists".into(),
            );
        }
        Ok(())
    }
}
