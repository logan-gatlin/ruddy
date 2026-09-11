//! Callable evidence interfaces. Shapes follow values; requirements describe
//! invocation, independently of the computation that constructs that value.
//!
//! Requirement equations contain only finite sets of solved type parameters
//! and edges between a finite collection of demand ports. Substitution maps a
//! parameter to the free parameters of its instantiated type, never to an
//! expanding type expression. Consequently recursive equations are monotone
//! over a finite carrier and reach a fixed point.

use crate::{
    symbol::Symbol,
    types::{Scheme, Ty},
};
use indexmap::IndexMap;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
};

pub type ShapeId = u32;
pub type NeedId = u32;

#[derive(Debug, Clone)]
pub enum Shape {
    Value,
    /// An unobserved component forwards its caller's callable convention.
    /// Inspecting the value reveals one structural layer; distinct positions
    /// retain independent variables without expanding a shared type DAG.
    Lazy,
    /// A value crossing reflection has a canonical, descriptor-free callable
    /// convention, including any callable carried by an abstract parameter.
    Sealed,
    Parameter(u32),
    Array(ShapeId),
    Cell(ShapeId),
    Record(BTreeMap<String, ShapeId>),
    Sum(BTreeMap<String, ShapeId>),
    Arrow {
        argument: ShapeId,
        result: ShapeId,
        needs: NeedId,
    },
    Alias(ShapeId),
}

/// A quantified demand port represents the requirements of an incoming
/// callable. Its possible slots are restricted to parameters of that callable.
/// A supplied port instead follows the offered callable's requirements.
#[derive(Debug, Clone, Default)]
pub struct Needs {
    pub variable: bool,
    pub redirect: Option<NeedId>,
    pub direct: BTreeSet<u32>,
    pub port: Option<BTreeSet<u32>>,
    pub edges: Vec<Edge>,
    pub instance: Option<Instance>,
}

#[derive(Debug, Clone, Default)]
pub struct Edge {
    pub source: NeedId,
    pub substitute: BTreeMap<u32, BTreeSet<u32>>,
    pub retain: Option<BTreeSet<u32>>,
}

/// A use retains a live reference to its scheme's finite equations. The type
/// substitution maps only free parameters; the port substitution specializes
/// higher-order inputs without copying an unfinished recursive definition.
#[derive(Debug, Clone)]
pub struct Instance {
    pub source: NeedId,
    pub substitute: BTreeMap<u32, BTreeSet<u32>>,
    pub ports: BTreeMap<NeedId, NeedId>,
}

/// A known requirement or a slot whose presence depends on a quantified port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Requirement {
    pub port: Option<NeedId>,
    pub parameter: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub shapes: Vec<Shape>,
    pub needs: Vec<Needs>,
    /// Recursive definitions allocate their ports before any forward use can
    /// instantiate them. Ordinary definitions reveal components on demand.
    eager: bool,
}

impl Graph {
    pub fn shape(&mut self, shape: Shape) -> ShapeId {
        let id = self.shapes.len() as ShapeId;
        self.shapes.push(shape);
        id
    }

    pub fn need(&mut self, needs: Needs) -> NeedId {
        let id = self.needs.len() as NeedId;
        self.needs.push(needs);
        id
    }

    /// Build a finite callable-shape skeleton from the compiler's regular type
    /// graph. Scalar identity and effects remain in the ordinary semantic type.
    pub fn skeleton(&mut self, ty: &Arc<Ty>, aliases: &IndexMap<Symbol, Scheme>) -> ShapeId {
        self.skeleton_with(ty, aliases, false, !self.eager)
    }

    fn reveal(&mut self, id: ShapeId, ty: &Arc<Ty>, aliases: &IndexMap<Symbol, Scheme>) {
        let id = self.exposed_id(id);
        if matches!(self.shapes[id as usize], Shape::Lazy) {
            let revealed = self.skeleton(ty, aliases);
            self.shapes[id as usize] = Shape::Alias(revealed);
        }
    }

    fn skeleton_with(
        &mut self,
        ty: &Arc<Ty>,
        aliases: &IndexMap<Symbol, Scheme>,
        native: bool,
        shallow: bool,
    ) -> ShapeId {
        let (mut root, graph) = crate::ir::representation_graph(ty, aliases);
        while graph[root].0 == "package" {
            root = graph[root].1[0].1;
        }
        // Closed value types have no hidden descriptor slots, even when their
        // expanded callable tree is exponentially larger than its alias DAG.
        // Keep that entire region erased instead of expanding redundant ports.
        let mut relevant = vec![false; graph.len()];
        let mut parents = vec![Vec::new(); graph.len()];
        let mut pending = Vec::new();
        for (at, (label, edges)) in graph.iter().enumerate() {
            if label.starts_with("param:") {
                relevant[at] = true;
                pending.push(at);
            }
            for (label, child) in edges {
                if label != "effects" && label != "region" {
                    parents[*child].push(at);
                }
            }
        }
        while let Some(at) = pending.pop() {
            for parent in &parents[at] {
                if !relevant[*parent] {
                    relevant[*parent] = true;
                    pending.push(*parent);
                }
            }
        }
        enum Work {
            Enter(usize, ShapeId),
            Leave(usize),
        }
        let mut active = HashMap::new();
        let root_shape = self.shape(Shape::Value);
        let mut work = vec![Work::Enter(root, root_shape)];
        while let Some(next) = work.pop() {
            let (at, target) = match next {
                Work::Enter(at, target) => (at, target),
                Work::Leave(at) => {
                    if !native {
                        active.remove(&at);
                    }
                    continue;
                }
            };
            if !relevant[at] {
                continue;
            }
            active.insert(at, target);
            work.push(Work::Leave(at));
            let (label, edges) = &graph[at];
            let mut child = |output: &mut Self, source: usize| {
                if let Some(existing) = active.get(&source) {
                    return *existing;
                }
                if let Some(index) = graph[source].0.strip_prefix("param:") {
                    return output.shape(if native {
                        Shape::Sealed
                    } else {
                        Shape::Parameter(index.parse().expect("bound graph parameter"))
                    });
                }
                if shallow {
                    return output.shape(if relevant[source] {
                        Shape::Lazy
                    } else {
                        Shape::Value
                    });
                }
                let id = output.shape(Shape::Value);
                if native {
                    active.insert(source, id);
                }
                work.push(Work::Enter(source, id));
                id
            };
            let edge = |name: &str| {
                edges
                    .iter()
                    .find_map(|(key, value)| (key == name).then_some(*value))
            };
            let shape = match label.as_str() {
                "arrow" => {
                    let argument = if native {
                        self.shape(Shape::Sealed)
                    } else {
                        child(self, edge("from").expect("arrow argument"))
                    };
                    let result = child(self, edge("to").expect("arrow result"));
                    let mut parameters = BTreeSet::new();
                    let mut seen = HashSet::new();
                    // A native input must be converted now, including captured
                    // callback evidence. Returned functions instead receive
                    // their descriptors when invoked, even inside aggregates.
                    let mut pending = if native {
                        vec![(edge("from").unwrap(), false), (edge("to").unwrap(), true)]
                    } else {
                        vec![(at, false)]
                    };
                    while let Some((node, defer_functions)) = pending.pop() {
                        if !seen.insert((node, defer_functions)) {
                            continue;
                        }
                        if (defer_functions && graph[node].0 == "arrow")
                            || (native && graph[node].0 == "mut")
                        {
                            continue;
                        }
                        if let Some(index) = graph[node].0.strip_prefix("param:") {
                            parameters.insert(index.parse().expect("bound graph parameter"));
                        }
                        pending.extend(graph[node].1.iter().filter_map(|(key, node)| {
                            (key != "effects" && key != "region")
                                .then_some((*node, defer_functions))
                        }));
                    }
                    let needs = self.need(if native {
                        Needs {
                            direct: parameters,
                            ..Needs::default()
                        }
                    } else {
                        Needs {
                            variable: true,
                            port: Some(parameters),
                            ..Needs::default()
                        }
                    });
                    Shape::Arrow {
                        argument,
                        result,
                        needs,
                    }
                }
                "array" => Shape::Array(child(self, edge("element").expect("array element"))),
                "mut" if !native => Shape::Cell(self.shape(Shape::Sealed)),
                "package" => Shape::Alias(child(self, edge("body").expect("package body"))),
                "fields" | "sum" => {
                    let mut fields = BTreeMap::new();
                    for (label, node) in edges {
                        if let Some((name, presence)) = label.rsplit_once(':')
                            && presence != "\\"
                            && let Some((_, name)) = name.split_once(':')
                        {
                            fields.insert(name.to_owned(), child(self, *node));
                        }
                    }
                    if label == "fields" {
                        Shape::Record(fields)
                    } else {
                        Shape::Sum(fields)
                    }
                }
                name if name.starts_with("param:") => {
                    if native {
                        Shape::Sealed
                    } else {
                        Shape::Parameter(name[6..].parse().expect("bound graph parameter"))
                    }
                }
                _ => Shape::Value,
            };
            self.shapes[target as usize] = shape;
        }
        root_shape
    }

    pub fn union(&mut self, sources: impl IntoIterator<Item = NeedId>) -> NeedId {
        self.need(Needs {
            edges: sources
                .into_iter()
                .map(|source| Edge {
                    source,
                    ..Edge::default()
                })
                .collect(),
            ..Needs::default()
        })
    }

    /// A cell fixes its stored callable convention for all aliases. Evidence
    /// captured by the sealing adapter is needed now; no user body runs here.
    fn sealing(&mut self, value: ShapeId) -> NeedId {
        let mut pending = vec![value];
        let mut seen = HashSet::new();
        let mut needs = Vec::new();
        while let Some(value) = pending.pop() {
            if !seen.insert(value) {
                continue;
            }
            match &self.shapes[value as usize] {
                Shape::Arrow {
                    argument,
                    result,
                    needs: row,
                } => {
                    needs.push(*row);
                    pending.extend([*argument, *result]);
                }
                Shape::Alias(child) | Shape::Array(child) => pending.push(*child),
                Shape::Record(fields) | Shape::Sum(fields) => pending.extend(fields.values()),
                _ => {}
            }
        }
        self.union(needs)
    }

    pub fn select(&mut self, source: NeedId, parameters: BTreeSet<u32>) -> NeedId {
        self.need(Needs {
            edges: vec![Edge {
                source,
                retain: Some(parameters),
                ..Edge::default()
            }],
            ..Needs::default()
        })
    }

    /// Fit an offered shape to fresh expected ports. Preserve the offered
    /// interface; lowering can then adapt it to the expected convention.
    pub fn supply(&mut self, expected: ShapeId, offered: ShapeId) {
        self.fit(expected, offered, false);
    }

    /// A control-flow join combines invocation demands without constraining
    /// either offered callable to acquire the other branch's requirements.
    pub fn join(&mut self, expected: ShapeId, offered: ShapeId) {
        self.fit(expected, offered, true);
    }

    fn fit(&mut self, expected: ShapeId, offered: ShapeId, joining: bool) {
        let mut work = vec![(expected, offered, joining)];
        let mut seen = HashSet::new();
        while let Some((expected, offered, joining)) = work.pop() {
            if expected == offered || !seen.insert((expected, offered, joining)) {
                continue;
            }
            match (
                self.shapes[expected as usize].clone(),
                self.shapes[offered as usize].clone(),
            ) {
                (Shape::Alias(next), _) => work.push((next, offered, joining)),
                (_, Shape::Alias(next)) => work.push((expected, next, joining)),
                (Shape::Lazy, _) => self.shapes[expected as usize] = Shape::Alias(offered),
                (_, Shape::Lazy) => self.shapes[offered as usize] = Shape::Alias(expected),
                (Shape::Sealed, Shape::Parameter(_)) => {
                    self.shapes[offered as usize] = Shape::Sealed
                }
                (Shape::Sealed, Shape::Array(element)) => work.push((expected, element, false)),
                (Shape::Sealed, Shape::Record(fields)) | (Shape::Sealed, Shape::Sum(fields)) => {
                    work.extend(fields.values().map(|member| (expected, *member, false)));
                }
                (Shape::Parameter(_), _) => self.shapes[expected as usize] = Shape::Alias(offered),
                (
                    Shape::Arrow {
                        argument: a,
                        result: b,
                        needs,
                    },
                    Shape::Arrow {
                        argument: x,
                        result: y,
                        needs: from,
                    },
                ) => {
                    if joining {
                        let row = &mut self.needs[needs as usize];
                        row.variable = false;
                        row.port = None;
                        row.edges.push(Edge {
                            source: from,
                            ..Edge::default()
                        });
                    } else {
                        self.unify_needs(needs, from);
                    }
                    work.extend([(x, a, false), (b, y, joining)]);
                }
                (Shape::Array(a), Shape::Array(b)) => work.push((a, b, joining)),
                (Shape::Cell(a), Shape::Cell(b)) => work.push((a, b, true)),
                (Shape::Record(a), Shape::Record(b)) | (Shape::Sum(a), Shape::Sum(b)) => {
                    work.extend(a.into_iter().filter_map(|(name, target)| {
                        b.get(&name).map(|source| (target, *source, joining))
                    }));
                }
                _ => {}
            }
        }
    }

    pub(super) fn need_root(&self, mut id: NeedId) -> NeedId {
        while let Some(next) = self.needs[id as usize].redirect {
            id = next;
        }
        id
    }

    fn unify_needs(&mut self, expected: NeedId, offered: NeedId) {
        let expected = self.need_root(expected);
        let offered = self.need_root(offered);
        if expected == offered {
            return;
        }
        let a = self.needs[expected as usize].clone();
        let b = self.needs[offered as usize].clone();
        match (a.variable, b.variable) {
            (true, true) if a.instance.is_some() || b.instance.is_some() => {
                self.needs[expected as usize].port = None;
                self.needs[expected as usize].edges.push(Edge {
                    source: offered,
                    ..Edge::default()
                });
            }
            (true, true) => {
                let row = &mut self.needs[offered as usize];
                row.direct.extend(a.direct);
                row.edges.extend(a.edges);
                row.port = match (a.port, b.port) {
                    (Some(a), Some(b)) => Some(a.union(&b).copied().collect()),
                    _ => None,
                };
                self.needs[expected as usize] = Needs {
                    variable: true,
                    redirect: Some(offered),
                    ..Needs::default()
                };
            }
            (true, false) => {
                let row = &mut self.needs[expected as usize];
                row.port = None;
                row.edges.push(Edge {
                    source: offered,
                    ..Edge::default()
                });
            }
            (false, true) => {
                let row = &mut self.needs[offered as usize];
                row.port = None;
                row.edges.push(Edge {
                    source: expected,
                    ..Edge::default()
                });
            }
            (false, false) => {
                self.needs[expected as usize].edges.push(Edge {
                    source: offered,
                    ..Edge::default()
                });
            }
        }
    }

    pub(super) fn free_parameters(&self, id: NeedId) -> Option<BTreeSet<u32>> {
        let mut id = self.need_root(id);
        let mut substitutions = Vec::new();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(id) {
                return None;
            }
            let needs = &self.needs[id as usize];
            let parameters = needs.port.as_ref()?;
            if let Some(instance) = &needs.instance {
                substitutions.push(&instance.substitute);
                id = self.need_root(instance.source);
            } else {
                let mut parameters = parameters.clone();
                for substitution in substitutions.into_iter().rev() {
                    parameters = parameters
                        .into_iter()
                        .flat_map(|p| {
                            substitution
                                .get(&p)
                                .cloned()
                                .unwrap_or_else(|| BTreeSet::from([p]))
                        })
                        .collect();
                }
                return Some(parameters);
            }
        }
    }

    /// Solve the finite monotone equations. Ports remain symbolic until a
    /// scheme instantiation supplies a callable at that position. Template
    /// edges remain live, including forward references in recursive groups.
    pub fn solve(&self) -> Vec<BTreeSet<Requirement>> {
        let mut values: Vec<BTreeSet<Requirement>> = self
            .needs
            .iter()
            .enumerate()
            .map(|(id, needs)| {
                needs
                    .direct
                    .iter()
                    .map(|parameter| Requirement {
                        port: None,
                        parameter: *parameter,
                    })
                    .chain(
                        self.free_parameters(id as NeedId)
                            .into_iter()
                            .flatten()
                            .map(|parameter| Requirement {
                                port: Some(self.need_root(id as NeedId)),
                                parameter,
                            }),
                    )
                    .collect()
            })
            .collect();
        let mut dependents = vec![Vec::new(); self.needs.len()];
        for (id, needs) in self.needs.iter().enumerate() {
            for edge in &needs.edges {
                dependents[edge.source as usize].push(id);
            }
            if let Some(source) = needs.redirect {
                dependents[source as usize].push(id);
            }
            if let Some(instance) = &needs.instance {
                dependents[instance.source as usize].push(id);
                for target in instance.ports.values() {
                    dependents[*target as usize].push(id);
                }
            }
        }
        let mut pending: std::collections::VecDeque<_> = (0..self.needs.len()).collect();
        let mut queued = vec![true; self.needs.len()];
        while let Some(id) = pending.pop_front() {
            queued[id] = false;
            let mut added = self.needs[id]
                .redirect
                .map(|source| values[source as usize].clone())
                .unwrap_or_default();
            for edge in &self.needs[id].edges {
                for need in &values[edge.source as usize] {
                    let parameters = edge
                        .substitute
                        .get(&need.parameter)
                        .cloned()
                        .unwrap_or_else(|| BTreeSet::from([need.parameter]));
                    added.extend(
                        parameters
                            .into_iter()
                            .filter(|parameter| {
                                edge.retain
                                    .as_ref()
                                    .is_none_or(|allowed| allowed.contains(parameter))
                            })
                            .map(|parameter| Requirement { parameter, ..*need }),
                    );
                }
            }
            if let Some(instance) = &self.needs[id].instance {
                for need in &values[instance.source as usize] {
                    let parameters = instance
                        .substitute
                        .get(&need.parameter)
                        .cloned()
                        .unwrap_or_else(|| BTreeSet::from([need.parameter]));
                    let targets: Vec<_> = need
                        .port
                        .into_iter()
                        .flat_map(|port| {
                            instance.ports.iter().filter_map(move |(source, target)| {
                                (self.need_root(*source) == self.need_root(port)).then_some(*target)
                            })
                        })
                        .collect();
                    if targets.is_empty() {
                        added.extend(
                            parameters
                                .into_iter()
                                .map(|parameter| Requirement { parameter, ..*need }),
                        );
                    } else {
                        for target in targets {
                            for supplied in values[target as usize]
                                .iter()
                                .filter(|need| parameters.contains(&need.parameter))
                            {
                                // One abstract descriptor slot can instantiate
                                // to a compound type. Once used, every component
                                // is needed to construct that slot, even if the
                                // supplied callback later projects only one.
                                if supplied.port.is_none() {
                                    added.extend(parameters.iter().map(|parameter| Requirement {
                                        parameter: *parameter,
                                        port: None,
                                    }));
                                } else {
                                    added.insert(*supplied);
                                }
                            }
                        }
                    }
                }
            }
            let old = values[id].len();
            values[id].extend(added);
            if values[id].len() != old {
                for target in &dependents[id] {
                    if !queued[*target] {
                        queued[*target] = true;
                        pending.push_back(*target);
                    }
                }
            }
        }
        values
    }
}

impl Graph {
    /// Instantiate a scheme's shape and demand graph together. Nodes belonging
    /// to the surrounding lexical environment retain their identity. A type
    /// shape variable shared by input and output is replaced once, so erased
    /// forwarding preserves callable and aggregate conventions. Distinct
    /// values of one ordinary type keep independent shape variables.
    pub fn instantiate(
        &mut self,
        root: ShapeId,
        since: (usize, usize),
        types: &HashMap<u32, Arc<Ty>>,
        aliases: &IndexMap<Symbol, Scheme>,
    ) -> ShapeId {
        enum Work {
            Shape(ShapeId, ShapeId),
            Need(NeedId, NeedId),
        }
        let substitute: BTreeMap<_, _> = types
            .iter()
            .map(|(index, ty)| (*index, super::parameters(ty)))
            .collect();
        let mut shapes = HashMap::new();
        let mut needs = HashMap::new();
        let mut work = Vec::new();
        fn shape(
            graph: &mut Graph,
            source: ShapeId,
            since: usize,
            map: &mut HashMap<ShapeId, ShapeId>,
            work: &mut Vec<Work>,
        ) -> ShapeId {
            if (source as usize) < since {
                return source;
            }
            *map.entry(source).or_insert_with(|| {
                let target = graph.shape(Shape::Value);
                work.push(Work::Shape(source, target));
                target
            })
        }
        fn need(
            graph: &mut Graph,
            source: NeedId,
            since: usize,
            map: &mut HashMap<NeedId, NeedId>,
            work: &mut Vec<Work>,
        ) -> NeedId {
            let source = graph.need_root(source);
            if (source as usize) < since {
                return source;
            }
            *map.entry(source).or_insert_with(|| {
                let target = graph.need(Needs::default());
                work.push(Work::Need(source, target));
                target
            })
        }
        let result = shape(self, root, since.0, &mut shapes, &mut work);
        while let Some(next) = work.pop() {
            match next {
                Work::Shape(source, target) => {
                    let copied = match self.shapes[source as usize].clone() {
                        Shape::Parameter(index) => {
                            if let Some(ty) = types.get(&index) {
                                Shape::Alias(self.skeleton(ty, aliases))
                            } else {
                                Shape::Parameter(index)
                            }
                        }
                        Shape::Value => Shape::Value,
                        Shape::Lazy => Shape::Lazy,
                        Shape::Sealed => Shape::Sealed,
                        Shape::Alias(inner) => {
                            Shape::Alias(shape(self, inner, since.0, &mut shapes, &mut work))
                        }
                        Shape::Cell(inner) => {
                            Shape::Cell(shape(self, inner, since.0, &mut shapes, &mut work))
                        }
                        Shape::Array(inner) => {
                            Shape::Array(shape(self, inner, since.0, &mut shapes, &mut work))
                        }
                        Shape::Record(fields) => Shape::Record(
                            fields
                                .into_iter()
                                .map(|(key, inner)| {
                                    (key, shape(self, inner, since.0, &mut shapes, &mut work))
                                })
                                .collect(),
                        ),
                        Shape::Sum(fields) => Shape::Sum(
                            fields
                                .into_iter()
                                .map(|(key, inner)| {
                                    (key, shape(self, inner, since.0, &mut shapes, &mut work))
                                })
                                .collect(),
                        ),
                        Shape::Arrow {
                            argument,
                            result,
                            needs: row,
                        } => Shape::Arrow {
                            argument: shape(self, argument, since.0, &mut shapes, &mut work),
                            result: shape(self, result, since.0, &mut shapes, &mut work),
                            needs: need(self, row, since.1, &mut needs, &mut work),
                        },
                    };
                    self.shapes[target as usize] = copied;
                }
                Work::Need(source, target) => {
                    let original = self.needs[source as usize].clone();
                    // Discover local demand ports, but keep the source
                    // equations live rather than copying their current state.
                    for edge in &original.edges {
                        need(self, edge.source, since.1, &mut needs, &mut work);
                    }
                    if let Some(source) = original.redirect {
                        need(self, source, since.1, &mut needs, &mut work);
                    }
                    if let Some(instance) = &original.instance {
                        for target in instance.ports.values() {
                            need(self, *target, since.1, &mut needs, &mut work);
                        }
                    }
                    self.needs[target as usize] = Needs {
                        variable: original.variable,
                        port: Some(BTreeSet::new()),
                        instance: Some(Instance {
                            source,
                            substitute: substitute.clone(),
                            ports: BTreeMap::new(),
                        }),
                        ..Needs::default()
                    };
                }
            }
        }
        let ports = needs
            .iter()
            .map(|(source, target)| (*source, *target))
            .collect::<BTreeMap<_, _>>();
        for target in needs.values() {
            self.needs[*target as usize]
                .instance
                .as_mut()
                .expect("instantiated demand")
                .ports = ports.clone();
        }
        result
    }

    pub fn exposed_id(&self, mut id: ShapeId) -> ShapeId {
        let mut seen = HashSet::new();
        while let Shape::Alias(next) = &self.shapes[id as usize] {
            if !seen.insert(id) {
                break;
            }
            id = *next;
        }
        id
    }

    pub fn exposed(&self, id: ShapeId) -> &Shape {
        &self.shapes[self.exposed_id(id) as usize]
    }
}

use crate::{
    inference,
    ir::{self, Program, Term, TermKind},
    tracking::Anchor,
};

#[derive(Debug, Clone, Copy)]
pub struct Flow {
    pub value: ShapeId,
    pub evaluation: NeedId,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub ty: Arc<Ty>,
    pub value: ShapeId,
    /// Nodes allocated before this binding belong to the lexical environment.
    pub since: (usize, usize),
    pub generalized: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub graph: Graph,
    pub bindings: IndexMap<Symbol, Binding>,
    pub occurrences: IndexMap<Anchor, Flow>,
}

impl Plan {
    pub fn infer(program: &Program, semantics: &inference::Semantics) -> Self {
        let mut planner = Planner {
            plan: Self::default(),
            aliases: semantics.aliases(),
            universe: BTreeSet::new(),
            raised: None,
        };
        for declaration in semantics.typed().values() {
            let mut work = vec![&declaration.value];
            while let Some(term) = work.pop() {
                planner.universe.extend(super::parameters(&term.ty));
                super::children(term, &mut work);
            }
        }
        for (symbol, scheme) in &program.external_schemes {
            if let Some(interface) = scheme.callable() {
                let since = (
                    planner.plan.graph.shapes.len(),
                    planner.plan.graph.needs.len(),
                );
                let value = interface.import(&mut planner.plan.graph);
                planner.plan.bindings.insert(
                    *symbol,
                    Binding {
                        ty: scheme.body().clone(),
                        value,
                        since,
                        generalized: true,
                    },
                );
            } else {
                planner.seed(
                    *symbol,
                    scheme.body(),
                    scheme.representations().iter().copied().collect(),
                );
            }
        }
        for (symbol, declaration) in &program.externs {
            let ty = semantics.externs()[symbol].body();
            if let Some(intrinsic) =
                super::Intrinsic::recognize(&declaration.value.target.anchored, ty, planner.aliases)
            {
                planner.seed(
                    *symbol,
                    ty,
                    super::parameters(&intrinsic.represented(ty, planner.aliases)),
                );
                let root = planner.plan.bindings[symbol].value;
                if let Shape::Arrow {
                    argument, result, ..
                } = planner.plan.graph.exposed(root).clone()
                {
                    match intrinsic {
                        super::Intrinsic::Upcast => {
                            planner.plan.graph.shapes[argument as usize] = Shape::Sealed
                        }
                        super::Intrinsic::Downcast => {
                            if let Shape::Sum(fields) = planner.plan.graph.exposed(result).clone()
                                && let Some(payload) = fields.get("Some")
                            {
                                planner.plan.graph.shapes[*payload as usize] = Shape::Sealed;
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                if declaration.value.array_intrinsic {
                    planner.seed(*symbol, ty, BTreeSet::new());
                } else {
                    let since = (
                        planner.plan.graph.shapes.len(),
                        planner.plan.graph.needs.len(),
                    );
                    let value = planner
                        .plan
                        .graph
                        .skeleton_with(ty, planner.aliases, true, false);
                    planner.plan.bindings.insert(
                        *symbol,
                        Binding {
                            ty: ty.clone(),
                            value,
                            since,
                            generalized: true,
                        },
                    );
                }
            }
        }
        for group in &program.groups {
            planner.plan.graph.eager = group.recursive;
            let since = (
                planner.plan.graph.shapes.len(),
                planner.plan.graph.needs.len(),
            );
            for symbol in &group.members {
                if let Some(declaration) = semantics.typed().get(symbol) {
                    let value = planner
                        .plan
                        .graph
                        .skeleton(&declaration.value.ty, planner.aliases);
                    planner.plan.bindings.insert(
                        *symbol,
                        Binding {
                            ty: declaration.value.ty.clone(),
                            value,
                            since,
                            generalized: false,
                        },
                    );
                }
            }
            for symbol in &group.members {
                if let Some(declaration) = semantics.typed().get(symbol) {
                    let flow = planner.term(&declaration.value);
                    let binding = planner.plan.bindings[symbol].value;
                    planner.plan.graph.supply(binding, flow.value);
                    planner.plan.graph.shapes[binding as usize] = Shape::Alias(flow.value);
                }
            }
            for symbol in &group.members {
                if let Some(binding) = planner.plan.bindings.get_mut(symbol) {
                    binding.generalized = true;
                }
            }
        }
        planner.plan.graph.eager = false;
        planner.plan
    }
}

struct Planner<'a> {
    plan: Plan,
    aliases: &'a IndexMap<Symbol, Scheme>,
    universe: BTreeSet<u32>,
    raised: Option<ShapeId>,
}

impl Planner<'_> {
    fn empty(&mut self) -> NeedId {
        self.plan.graph.need(Needs::default())
    }

    fn seed(&mut self, symbol: Symbol, ty: &Arc<Ty>, demands: BTreeSet<u32>) {
        let since = (self.plan.graph.shapes.len(), self.plan.graph.needs.len());
        let value = self
            .plan
            .graph
            .skeleton_with(ty, self.aliases, false, false);
        for needs in &mut self.plan.graph.needs[since.1..] {
            needs.port = None;
            needs.variable = false;
        }
        if let Shape::Arrow { needs, .. } = self.plan.graph.exposed(value).clone() {
            self.plan.graph.needs[needs as usize].direct = demands;
        }
        self.plan.bindings.insert(
            symbol,
            Binding {
                ty: ty.clone(),
                value,
                since,
                generalized: true,
            },
        );
    }

    fn term(&mut self, term: &Term) -> Flow {
        let evaluation = self.empty();
        let flow = match &term.kind {
            TermKind::Operation { .. } => {
                // Effect records have one stable convention across handler
                // installations and separately compiled perform sites. Values
                // crossing them seal their callable evidence into captures.
                let argument = self.plan.graph.shape(Shape::Sealed);
                let result = self.plan.graph.shape(Shape::Sealed);
                let needs = self.empty();
                let value = self.plan.graph.shape(Shape::Arrow {
                    argument,
                    result,
                    needs,
                });
                Flow { value, evaluation }
            }
            TermKind::Ident(symbol) => {
                let value = if let Some(binding) = self.plan.bindings.get(symbol).cloned() {
                    if binding.generalized || binding.since.0 != usize::MAX {
                        let types = super::instantiate(&binding.ty, &term.ty, self.aliases);
                        self.plan.graph.instantiate(
                            binding.value,
                            binding.since,
                            &types,
                            self.aliases,
                        )
                    } else {
                        binding.value
                    }
                } else {
                    self.plan.graph.skeleton(&term.ty, self.aliases)
                };
                Flow { value, evaluation }
            }
            TermKind::Unary {
                op: ir::UnaryOp::Allocate,
                value: inner,
            } => {
                let initial = self.term(inner);
                let value = self.plan.graph.skeleton(&term.ty, self.aliases);
                if let Shape::Cell(element) = self.plan.graph.exposed(value).clone() {
                    self.plan.graph.supply(element, initial.value);
                }
                let sealing = self.plan.graph.sealing(initial.value);
                Flow {
                    value,
                    evaluation: self.plan.graph.union([initial.evaluation, sealing]),
                }
            }
            TermKind::Unary {
                op: ir::UnaryOp::Read,
                value: cell,
            } => {
                let cell = self.term(cell);
                let value = match self.plan.graph.exposed(cell.value) {
                    Shape::Cell(element) => *element,
                    _ => self.plan.graph.skeleton(&term.ty, self.aliases),
                };
                Flow {
                    value,
                    evaluation: cell.evaluation,
                }
            }
            TermKind::Binary {
                op: ir::BinaryOp::Write,
                left,
                right,
            } => {
                let left = self.term(left);
                let right = self.term(right);
                let value = match self.plan.graph.exposed(left.value).clone() {
                    Shape::Cell(element) => {
                        self.plan.graph.supply(element, right.value);
                        element
                    }
                    _ => right.value,
                };
                let sealing = self.plan.graph.sealing(right.value);
                Flow {
                    value,
                    evaluation: self
                        .plan
                        .graph
                        .union([left.evaluation, right.evaluation, sealing]),
                }
            }
            TermKind::Fn { arg, body } => {
                let arrow = inference::unfold(self.aliases, &term.ty);
                let from = match &*arrow {
                    Ty::Arrow(from, _, _) => from.clone(),
                    _ => Arc::new(Ty::Undecided),
                };
                let argument = self.plan.graph.skeleton(&from, self.aliases);
                self.plan.bindings.insert(
                    arg.anchored,
                    Binding {
                        ty: from,
                        value: argument,
                        since: (usize::MAX, usize::MAX),
                        generalized: false,
                    },
                );
                let body = self.term(body);
                let allowed = super::parameters(&term.ty);
                let captured = self.universe.difference(&allowed).copied().collect();
                let needs = self.plan.graph.select(body.evaluation, allowed);
                let evaluation = self.plan.graph.select(body.evaluation, captured);
                let value = self.plan.graph.shape(Shape::Arrow {
                    argument,
                    result: body.value,
                    needs,
                });
                Flow { value, evaluation }
            }
            TermKind::Apply { func, arg } => {
                let func = self.term(func);
                let arg = self.term(arg);
                match self.plan.graph.exposed(func.value).clone() {
                    Shape::Arrow {
                        argument,
                        result,
                        needs,
                    } => {
                        let sealing = if matches!(self.plan.graph.exposed(argument), Shape::Sealed)
                        {
                            self.plan.graph.sealing(arg.value)
                        } else {
                            self.empty()
                        };
                        self.plan.graph.supply(argument, arg.value);
                        Flow {
                            value: result,
                            evaluation: self.plan.graph.union([
                                func.evaluation,
                                arg.evaluation,
                                sealing,
                                needs,
                            ]),
                        }
                    }
                    _ => Flow {
                        value: self.plan.graph.skeleton(&term.ty, self.aliases),
                        evaluation: self.plan.graph.union([func.evaluation, arg.evaluation]),
                    },
                }
            }
            TermKind::Let {
                name, value, body, ..
            } => {
                let outer = self.plan.graph.eager;
                let mut pending = vec![value.as_ref()];
                while let Some(term) = pending.pop() {
                    if matches!(term.kind, TermKind::Ident(symbol) if symbol == name.anchored) {
                        self.plan.graph.eager = true;
                        break;
                    }
                    super::children(term, &mut pending);
                }
                let since = (self.plan.graph.shapes.len(), self.plan.graph.needs.len());
                let placeholder = self.plan.graph.skeleton(&value.ty, self.aliases);
                self.plan.bindings.insert(
                    name.anchored,
                    Binding {
                        ty: value.ty.clone(),
                        value: placeholder,
                        since,
                        generalized: false,
                    },
                );
                let value = self.term(value);
                self.plan.graph.supply(placeholder, value.value);
                self.plan.graph.shapes[placeholder as usize] = Shape::Alias(value.value);
                self.plan
                    .bindings
                    .get_mut(&name.anchored)
                    .unwrap()
                    .generalized = true;
                self.plan.graph.eager = outer;
                let body = self.term(body);
                Flow {
                    value: body.value,
                    evaluation: self.plan.graph.union([value.evaluation, body.evaluation]),
                }
            }
            TermKind::Struct { fields, spread } => {
                let mut members = BTreeMap::new();
                let mut evaluations = Vec::new();
                if let Some(spread) = spread {
                    let spread = self.term(&spread.value);
                    evaluations.push(spread.evaluation);
                    if let Shape::Record(fields) = self.plan.graph.exposed(spread.value) {
                        members.extend(fields.clone());
                    }
                }
                for (name, field) in fields {
                    let flow = self.term(&field.value);
                    members.insert(name.clone(), flow.value);
                    evaluations.push(flow.evaluation);
                }
                Flow {
                    value: self.plan.graph.shape(Shape::Record(members)),
                    evaluation: self.plan.graph.union(evaluations),
                }
            }
            TermKind::Project { base, field } => {
                let base = self.term(base);
                let value = match self.plan.graph.exposed(base.value) {
                    Shape::Record(fields) => fields.get(&field.anchored).copied(),
                    _ => None,
                }
                .unwrap_or_else(|| self.plan.graph.skeleton(&term.ty, self.aliases));
                Flow {
                    value,
                    evaluation: base.evaluation,
                }
            }
            TermKind::Array(items) => {
                let value = self.plan.graph.skeleton(&term.ty, self.aliases);
                let mut evaluations = Vec::new();
                let element = match self.plan.graph.exposed(value) {
                    Shape::Array(element) => Some(*element),
                    _ => None,
                };
                for item in items {
                    let flow = self.term(&item.value);
                    evaluations.push(flow.evaluation);
                    let offered = if item.spread.is_some() {
                        match self.plan.graph.exposed(flow.value) {
                            Shape::Array(element) => Some(*element),
                            _ => None,
                        }
                    } else {
                        Some(flow.value)
                    };
                    if let (Some(element), Some(offered)) = (element, offered) {
                        self.plan.graph.join(element, offered);
                    }
                }
                Flow {
                    value,
                    evaluation: self.plan.graph.union(evaluations),
                }
            }
            TermKind::Tag { name, payload } => {
                let payload = payload.as_ref().map(|payload| self.term(payload));
                let member = payload.map_or_else(
                    || self.plan.graph.shape(Shape::Value),
                    |payload| payload.value,
                );
                Flow {
                    value: self.plan.graph.shape(Shape::Sum(BTreeMap::from([(
                        name.anchored.clone(),
                        member,
                    )]))),
                    evaluation: payload.map_or(evaluation, |payload| payload.evaluation),
                }
            }
            TermKind::Match { scrutinee, arms } => {
                let scrutinee_type = scrutinee.ty.clone();
                let scrutinee = self.term(scrutinee);
                let value = self.plan.graph.skeleton(&term.ty, self.aliases);
                let mut evaluations = vec![scrutinee.evaluation];
                for (pattern, body) in arms {
                    self.pattern(pattern, scrutinee.value, &scrutinee_type);
                    let body = self.term(body);
                    self.plan.graph.join(value, body.value);
                    evaluations.push(body.evaluation);
                }
                Flow {
                    value,
                    evaluation: self.plan.graph.union(evaluations),
                }
            }
            TermKind::Raise(value) => {
                let raised = self.term(value);
                if let Some(target) = self.raised {
                    self.plan.graph.join(target, raised.value);
                }
                Flow {
                    value: self.plan.graph.skeleton(&term.ty, self.aliases),
                    evaluation: raised.evaluation,
                }
            }
            TermKind::Handle { body, handler } => {
                let body = self.term(body);
                let value = self.plan.graph.skeleton(&term.ty, self.aliases);
                let mut evaluations = vec![body.evaluation];
                if let Some(ret) = &handler.ret {
                    self.plan.bindings.insert(
                        ret.binder.anchored,
                        Binding {
                            ty: Arc::new(Ty::Undecided),
                            value: body.value,
                            since: (usize::MAX, usize::MAX),
                            generalized: false,
                        },
                    );
                    let result = self.term(&ret.body);
                    evaluations.push(result.evaluation);
                    self.plan.graph.join(value, result.value);
                } else {
                    self.plan.graph.join(value, body.value);
                }
                let outer = self.raised.replace(value);
                for arm in &handler.arms {
                    let sealed = self.plan.graph.shape(Shape::Sealed);
                    self.plan.bindings.insert(
                        arm.binder.anchored,
                        Binding {
                            ty: Arc::new(Ty::Undecided),
                            value: sealed,
                            since: (usize::MAX, usize::MAX),
                            generalized: false,
                        },
                    );
                    let flow = self.term(&arm.body);
                    evaluations.push(flow.evaluation);
                    evaluations.push(self.plan.graph.sealing(flow.value));
                    self.plan.graph.supply(sealed, flow.value);
                }
                self.raised = outer;
                Flow {
                    value,
                    evaluation: self.plan.graph.union(evaluations),
                }
            }
            _ => {
                let mut children = Vec::new();
                super::children(term, &mut children);
                let evaluations = children
                    .into_iter()
                    .map(|child| self.term(child).evaluation)
                    .collect::<Vec<_>>();
                Flow {
                    value: self.plan.graph.skeleton(&term.ty, self.aliases),
                    evaluation: self.plan.graph.union(evaluations),
                }
            }
        };
        self.plan.graph.reveal(flow.value, &term.ty, self.aliases);
        self.plan.occurrences.insert(term.at, flow);
        flow
    }

    fn pattern(&mut self, pattern: &ir::Pattern, value: ShapeId, ty: &Arc<Ty>) {
        self.plan.graph.reveal(value, ty, self.aliases);
        let mut ty = inference::unfold(self.aliases, ty);
        while let Ty::Package(inner) = &*ty {
            ty = inference::unfold(self.aliases, inner);
        }
        match &pattern.anchored {
            // The payload reads the value as the hidden type's body, at the
            // type the arm opened it to.
            ir::PatternKind::Hidden {
                id,
                name,
                pattern: payload,
            } => {
                let opened = match &*ty {
                    Ty::Hidden { binder, body, .. } => crate::types::open_hidden(
                        body,
                        *binder,
                        &Arc::new(Ty::Rigid {
                            id: *id,
                            name: name.anchored.as_str().into(),
                        }),
                    ),
                    _ => Arc::new(Ty::Undecided),
                };
                self.pattern(payload, value, &opened);
            }
            ir::PatternKind::Bind(name) => {
                self.plan.bindings.insert(
                    name.anchored,
                    Binding {
                        ty: Arc::new(Ty::Undecided),
                        value,
                        since: (usize::MAX, usize::MAX),
                        generalized: false,
                    },
                );
            }
            ir::PatternKind::Struct { fields, .. } => {
                if let Shape::Record(members) = self.plan.graph.exposed(value).clone() {
                    for (name, field) in fields {
                        if let Some(member) = members.get(name)
                            && let Ty::Struct(row) = &*ty
                            && let Some(field_ty) = super::flattened(row).labels.get(name)
                        {
                            self.pattern(&field.value, *member, &field_ty.ty);
                        }
                    }
                }
            }
            ir::PatternKind::Tag {
                name,
                payload: Some(payload),
            } => {
                if let Shape::Sum(members) = self.plan.graph.exposed(value)
                    && let Some(member) = members.get(&name.anchored).copied()
                    && let Ty::Sum(row) = &*ty
                    && let Some(field) = super::flattened(row).labels.get(&name.anchored)
                {
                    self.pattern(payload, member, &field.ty);
                }
            }
            ir::PatternKind::Array {
                before,
                rest,
                after,
            } => {
                if let Some(name) = rest.as_ref().and_then(|rest| rest.name.as_ref()) {
                    self.plan.bindings.insert(
                        name.anchored,
                        Binding {
                            ty: Arc::new(Ty::Undecided),
                            value,
                            since: (usize::MAX, usize::MAX),
                            generalized: false,
                        },
                    );
                }
                if let Shape::Array(element) = self.plan.graph.exposed(value).clone() {
                    for pattern in before.iter().chain(after) {
                        if let Ty::Array(element_ty) = &*ty {
                            self.pattern(pattern, element, element_ty);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Descriptors consumed by one native arrow; returned callables remain latent.
pub fn native_invocation_parameters(
    ty: &Arc<Ty>,
    aliases: &IndexMap<Symbol, Scheme>,
) -> BTreeSet<u32> {
    let mut graph = Graph::default();
    let root = graph.skeleton_with(ty, aliases, true, false);
    match graph.exposed(root) {
        Shape::Arrow { needs, .. } => graph.needs[*needs as usize].direct.clone(),
        _ => BTreeSet::new(),
    }
}
