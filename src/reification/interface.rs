//! Portable callable interfaces. Solver equations never cross a bundle
//! boundary: the interface contains only value-shape sharing and solved,
//! quantified demand ports.

use super::conventions::{Edge, Graph, Needs, Shape, ShapeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interface {
    pub root: u32,
    pub nodes: Vec<Node>,
    pub ports: Vec<BTreeSet<u32>>,
    pub construction: BTreeMap<u32, super::construction::Demand>,
    /// Callable conventions captured by a partially applied structural
    /// function, in the same order as its semantic contract arguments.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub structural_arguments: BTreeMap<u32, Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Value,
    Lazy,
    Sealed,
    Parameter(u32),
    Array(u32),
    Cell(u32),
    Record(BTreeMap<String, u32>),
    Sum(BTreeMap<String, u32>),
    Arrow {
        argument: u32,
        result: u32,
        /// An incoming callable's own quantified requirements, if any.
        port: Option<u32>,
        requirements: BTreeSet<Requirement>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Requirement {
    pub port: Option<u32>,
    pub parameter: u32,
}

impl Interface {
    pub fn export(
        graph: &Graph,
        root: ShapeId,
        parameters: &HashMap<u32, u32>,
        solved: &[BTreeSet<super::conventions::Requirement>],
        construction: &[super::construction::Demand],
    ) -> Self {
        let mut output = Self {
            root: 0,
            nodes: Vec::new(),
            ports: Vec::new(),
            construction: BTreeMap::new(),
            structural_arguments: BTreeMap::new(),
        };
        let mut nodes = HashMap::new();
        let mut ports = HashMap::new();
        let mut pending = Vec::new();
        fn node(
            graph: &Graph,
            output: &mut Interface,
            nodes: &mut HashMap<ShapeId, u32>,
            pending: &mut Vec<(ShapeId, u32)>,
            id: ShapeId,
        ) -> u32 {
            let id = graph.exposed_id(id);
            *nodes.entry(id).or_insert_with(|| {
                let at = output.nodes.len() as u32;
                output.nodes.push(Node::Value);
                pending.push((id, at));
                at
            })
        }
        fn port(
            graph: &Graph,
            output: &mut Interface,
            ports: &mut HashMap<u32, u32>,
            parameters: &HashMap<u32, u32>,
            id: u32,
        ) -> u32 {
            let id = graph.need_root(id);
            *ports.entry(id).or_insert_with(|| {
                let at = output.ports.len() as u32;
                output.ports.push(
                    graph
                        .free_parameters(id)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|p| parameters.get(p).copied())
                        .collect(),
                );
                at
            })
        }
        output.root = node(graph, &mut output, &mut nodes, &mut pending, root);
        while let Some((source, target)) = pending.pop() {
            let copied = match graph.exposed(source) {
                Shape::Value | Shape::Alias(_) => Node::Value,
                Shape::Lazy => Node::Lazy,
                Shape::Sealed => Node::Sealed,
                Shape::Parameter(index) => parameters
                    .get(index)
                    .map_or(Node::Value, |p| Node::Parameter(*p)),
                Shape::Cell(element) => {
                    Node::Cell(node(graph, &mut output, &mut nodes, &mut pending, *element))
                }
                Shape::Array(element) => {
                    Node::Array(node(graph, &mut output, &mut nodes, &mut pending, *element))
                }
                Shape::Record(fields) | Shape::Sum(fields) => {
                    let fields = fields
                        .iter()
                        .map(|(name, value)| {
                            (
                                name.clone(),
                                node(graph, &mut output, &mut nodes, &mut pending, *value),
                            )
                        })
                        .collect();
                    if matches!(graph.exposed(source), Shape::Record(_)) {
                        Node::Record(fields)
                    } else {
                        Node::Sum(fields)
                    }
                }
                Shape::Arrow {
                    argument,
                    result,
                    needs,
                } => {
                    let argument = node(graph, &mut output, &mut nodes, &mut pending, *argument);
                    let result = node(graph, &mut output, &mut nodes, &mut pending, *result);
                    // Supplying an instantiated incoming callable can leave
                    // an identity equation instead of a redirect. Publish the
                    // normalized port it denotes, preserving its binder.
                    let own_need = graph.free_parameters(*needs).map(|_| *needs).or_else(|| {
                        if !graph.needs[graph.need_root(*needs) as usize].variable {
                            return None;
                        }
                        let row = &solved[*needs as usize];
                        let source = row.first()?.port?;
                        let expected: BTreeSet<_> = graph
                            .free_parameters(source)?
                            .into_iter()
                            .map(|parameter| super::conventions::Requirement {
                                port: Some(source),
                                parameter,
                            })
                            .collect();
                        (*row == expected).then_some(source)
                    });
                    let own =
                        own_need.map(|id| port(graph, &mut output, &mut ports, parameters, id));
                    let requirements = solved[*needs as usize]
                        .iter()
                        .filter_map(|need| {
                            parameters
                                .get(&need.parameter)
                                .map(|parameter| Requirement {
                                    parameter: *parameter,
                                    port: need.port.map(|id| {
                                        port(graph, &mut output, &mut ports, parameters, id)
                                    }),
                                })
                        })
                        .collect();
                    let demand = &construction[*needs as usize];
                    let mapped = super::construction::Demand {
                        condition: demand.condition.rename(parameters),
                        ports: demand
                            .ports
                            .iter()
                            .map(|id| port(graph, &mut output, &mut ports, parameters, *id))
                            .collect(),
                    };
                    if !mapped.condition.is_true() || !mapped.ports.is_empty() {
                        output.construction.insert(target, mapped);
                    }
                    Node::Arrow {
                        argument,
                        result,
                        port: own,
                        requirements,
                    }
                }
            };
            output.nodes[target as usize] = copied;
            if let Some(arguments) = graph.structural_arguments.get(&source) {
                let arguments = arguments
                    .iter()
                    .map(|argument| node(graph, &mut output, &mut nodes, &mut pending, *argument))
                    .collect();
                output.structural_arguments.insert(target, arguments);
            }
        }
        output.normalize();
        output
    }

    /// Only an incoming callable's own port can be communicated across the
    /// bundle boundary. A requirement qualified by any other port, one an
    /// instantiation left unresolved inside the exporting bundle, still
    /// reserves an evidence slot in the exporter's lowering, so publish it
    /// unconditionally and keep only the ports an arrow binds.
    fn normalize(&mut self) {
        let bound: BTreeSet<u32> = self
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Arrow {
                    port: Some(port), ..
                } => Some(*port),
                _ => None,
            })
            .collect();
        let mut renumbered = vec![None; self.ports.len()];
        let mut ports = Vec::with_capacity(bound.len());
        for port in bound {
            renumbered[port as usize] = Some(ports.len() as u32);
            ports.push(std::mem::take(&mut self.ports[port as usize]));
        }
        // Constructor predicates already contain actual local requests and
        // supplied callees' requirements. An unbound symbolic port describes
        // possible descriptor slots, not a demand to construct those types.
        for demand in self.construction.values_mut() {
            let mut kept = BTreeSet::new();
            for p in &demand.ports {
                if let Some(p) = renumbered[*p as usize] {
                    kept.insert(p);
                }
            }
            demand.ports = kept;
        }
        self.ports = ports;
        for node in &mut self.nodes {
            if let Node::Arrow {
                port, requirements, ..
            } = node
            {
                *port = port.and_then(|port| renumbered[port as usize]);
                *requirements = std::mem::take(requirements)
                    .into_iter()
                    .map(|need| Requirement {
                        port: need.port.and_then(|port| renumbered[port as usize]),
                        parameter: need.parameter,
                    })
                    .collect();
            }
        }
    }

    /// Validate references and quantified positions before an imported graph
    /// can reach evidence inference. Cycles are permitted through constructors.
    pub fn validate(&self, count: u32, presences: u32) -> Result<(), String> {
        let valid_parameter = |p: u32| p >= presences && p < count;
        if self.root as usize >= self.nodes.len() {
            return Err("invalid callable interface root".into());
        }
        for parameters in &self.ports {
            if parameters.iter().any(|p| !valid_parameter(*p)) {
                return Err("invalid callable demand parameter".into());
            }
        }
        for (node, demand) in &self.construction {
            if !matches!(self.nodes.get(*node as usize), Some(Node::Arrow { .. })) {
                return Err("invalid construction demand node".into());
            }
            if demand
                .condition
                .parameters()
                .iter()
                .any(|p| !valid_parameter(*p))
            {
                return Err("invalid construction demand parameter".into());
            }
            if demand.ports.iter().any(|p| *p as usize >= self.ports.len()) {
                return Err("invalid construction demand port".into());
            }
        }
        let mut visited = HashSet::new();
        let mut pending = vec![self.root];
        for (function, arguments) in &self.structural_arguments {
            if !matches!(self.nodes.get(*function as usize), Some(Node::Arrow { .. })) {
                return Err("invalid structural callable argument owner".into());
            }
            if arguments.len() > crate::contracts::MAX_NODES {
                return Err("structural callable argument count exceeds its bound".into());
            }
        }
        let mut bound_ports = HashSet::new();
        while let Some(id) = pending.pop() {
            if id as usize >= self.nodes.len() {
                return Err("invalid callable shape reference".into());
            }
            if !visited.insert(id) {
                continue;
            }
            if let Some(arguments) = self.structural_arguments.get(&id) {
                pending.extend(arguments.iter().copied());
            }
            match &self.nodes[id as usize] {
                Node::Value | Node::Lazy | Node::Sealed => {}
                Node::Parameter(p) => {
                    if !valid_parameter(*p) {
                        return Err("invalid callable shape parameter".into());
                    }
                }
                Node::Array(element) | Node::Cell(element) => pending.push(*element),
                Node::Record(fields) | Node::Sum(fields) => pending.extend(fields.values()),
                Node::Arrow {
                    argument,
                    result,
                    port,
                    requirements,
                } => {
                    pending.extend([*argument, *result]);
                    if let Some(port) = port {
                        let Some(parameters) = self.ports.get(*port as usize) else {
                            return Err("invalid callable demand port".into());
                        };
                        bound_ports.insert(*port);
                        let expected = parameters
                            .iter()
                            .map(|parameter| Requirement {
                                port: Some(*port),
                                parameter: *parameter,
                            })
                            .collect();
                        if *requirements != expected {
                            return Err("inconsistent callable demand port".into());
                        }
                    }
                    for need in requirements {
                        if !valid_parameter(need.parameter) {
                            return Err("invalid callable requirement parameter".into());
                        }
                        if let Some(port) = need.port
                            && !self
                                .ports
                                .get(port as usize)
                                .is_some_and(|parameters| parameters.contains(&need.parameter))
                        {
                            return Err("invalid callable requirement port".into());
                        }
                    }
                }
            }
        }
        if visited.len() != self.nodes.len() {
            return Err("unreachable callable shape".into());
        }
        if bound_ports.len() != self.ports.len() {
            return Err("unbound callable demand port".into());
        }
        Ok(())
    }

    pub fn import(&self, graph: &mut Graph) -> ShapeId {
        let ports: Vec<_> = self
            .ports
            .iter()
            .map(|parameters| {
                graph.need(Needs {
                    variable: true,
                    port: Some(parameters.clone()),
                    ..Needs::default()
                })
            })
            .collect();
        let nodes: Vec<_> = self
            .nodes
            .iter()
            .map(|_| graph.shape(Shape::Value))
            .collect();
        for (id, source) in self.nodes.iter().enumerate() {
            let shape = match source {
                Node::Value => Shape::Value,
                Node::Lazy => Shape::Lazy,
                Node::Sealed => Shape::Sealed,
                Node::Parameter(p) => Shape::Parameter(*p),
                Node::Array(element) => Shape::Array(nodes[*element as usize]),
                Node::Cell(element) => Shape::Cell(nodes[*element as usize]),
                Node::Record(fields) => Shape::Record(
                    fields
                        .iter()
                        .map(|(name, node)| (name.clone(), nodes[*node as usize]))
                        .collect(),
                ),
                Node::Sum(fields) => Shape::Sum(
                    fields
                        .iter()
                        .map(|(name, node)| (name.clone(), nodes[*node as usize]))
                        .collect(),
                ),
                Node::Arrow {
                    argument,
                    result,
                    port,
                    requirements,
                } => {
                    let needs = port.map(|port| ports[port as usize]).unwrap_or_else(|| {
                        let mut needs = Needs::default();
                        let mut edges = BTreeMap::<u32, BTreeSet<u32>>::new();
                        for need in requirements {
                            if let Some(port) = need.port {
                                edges.entry(port).or_default().insert(need.parameter);
                            } else {
                                needs.direct.insert(need.parameter);
                            }
                        }
                        needs.edges = edges
                            .into_iter()
                            .map(|(port, parameters)| Edge {
                                source: ports[port as usize],
                                retain: Some(parameters),
                                ..Edge::default()
                            })
                            .collect();
                        graph.need(needs)
                    });
                    if let Some(demand) = self.construction.get(&(id as u32)) {
                        graph.needs[needs as usize].construction.condition =
                            demand.condition.clone();
                        graph.needs[needs as usize].construction.ports =
                            demand.ports.iter().map(|p| ports[*p as usize]).collect();
                    }
                    Shape::Arrow {
                        argument: nodes[*argument as usize],
                        result: nodes[*result as usize],
                        needs,
                    }
                }
            };
            graph.shapes[nodes[id] as usize] = shape;
        }
        for (function, arguments) in &self.structural_arguments {
            graph.structural_arguments.insert(
                nodes[*function as usize],
                arguments
                    .iter()
                    .map(|argument| nodes[*argument as usize])
                    .collect(),
            );
        }
        nodes[self.root as usize]
    }
}
