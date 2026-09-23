//! Resource accounting for semantic captures before publishing another scheme.

use super::*;
use std::collections::HashSet;

pub(crate) const MAX_TYPE_NODES: usize = 131_072;
pub(crate) const MAX_TYPE_EDGES: usize = 262_144;
const MAX_CONTRACT_TYPE_NODES: usize = 8_192;
const MAX_CONTRACT_TYPE_EDGES: usize = 32_768;

#[derive(Clone, Copy, Debug)]
pub(crate) struct TypeGraphLimit {
    nodes: usize,
    edges: usize,
}

impl fmt::Display for TypeGraphLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "structural contract captures exceed the semantic type graph budget ({} nodes or {} edges)",
            self.nodes, self.edges
        )
    }
}

/// Count actual shared nodes, including representation fallbacks and delayed
/// arguments. Independent polymorphic instantiations remain independent: this
/// bounds their growth without interning variables from different instances.
pub(crate) fn check_type_graph(
    root: &Arc<Ty>,
    resolve: impl Fn(&Arc<Ty>) -> Arc<Ty>,
    canonical_row: impl Fn(&Row) -> Row,
) -> Result<(), TypeGraphLimit> {
    enum Work {
        Type(Arc<Ty>),
        Row(Row),
    }
    let mut pending = vec![Work::Type(root.clone())];
    let mut seen = HashSet::new();
    let mut retained = Vec::new();
    let mut edges = 0usize;
    let mut limit = TypeGraphLimit {
        nodes: MAX_TYPE_NODES,
        edges: MAX_TYPE_EDGES,
    };
    while let Some(work) = pending.pop() {
        let before = pending.len();
        match work {
            Work::Type(ty) => {
                let ty = resolve(&ty);
                if !seen.insert(Arc::as_ptr(&ty)) {
                    continue;
                }
                if matches!(&*ty, Ty::Contract { .. }) {
                    // Captures also carry polymorphic instantiation and source
                    // evidence. Bound them before publication can duplicate
                    // that metadata again; ordinary type graphs keep their
                    // existing, larger structural allowance.
                    limit = TypeGraphLimit {
                        nodes: MAX_CONTRACT_TYPE_NODES,
                        edges: MAX_CONTRACT_TYPE_EDGES,
                    };
                }
                if seen.len() > limit.nodes || edges > limit.edges {
                    return Err(limit);
                }
                // Resolvers may create temporary wrappers. Retain their
                // allocations so address reuse cannot hide distinct nodes.
                retained.push(ty.clone());
                let count = match &*ty {
                    Ty::Contract { contract, .. } => 1 + contract.type_operands().count(),
                    Ty::Named { args: items, .. } => items.len(),
                    Ty::Arrow(..) => 3,
                    Ty::Mut(..) => 2,
                    Ty::Package(_)
                    | Ty::Hidden { .. }
                    | Ty::Array(_)
                    | Ty::Mirror(_)
                    | Ty::TypeInfo(_)
                    | Ty::Struct(_)
                    | Ty::Sum(_) => 1,
                    _ => 0,
                };
                edges = edges.checked_add(count).ok_or(limit)?;
                if edges > limit.edges {
                    return Err(limit);
                }
                match &*ty {
                    Ty::Contract { fallback, contract } => {
                        pending.push(Work::Type(fallback.clone()));
                        pending.extend(contract.type_operands().cloned().map(Work::Type));
                    }
                    Ty::Named { args: items, .. } => {
                        pending.extend(items.iter().cloned().map(Work::Type));
                    }
                    Ty::Arrow(from, to, effects) => {
                        pending.push(Work::Type(from.clone()));
                        pending.push(Work::Type(to.clone()));
                        pending.push(Work::Row(effects.clone()));
                    }
                    Ty::Mut(region, body) => {
                        pending.push(Work::Type(region.clone()));
                        pending.push(Work::Type(body.clone()));
                    }
                    Ty::Package(body)
                    | Ty::Hidden { body, .. }
                    | Ty::Array(body)
                    | Ty::Mirror(body)
                    | Ty::TypeInfo(body) => pending.push(Work::Type(body.clone())),
                    Ty::Struct(row) | Ty::Sum(row) => pending.push(Work::Row(row.clone())),
                    _ => {}
                }
            }
            Work::Row(row) => {
                let row = canonical_row(&row);
                let count = row.labels.len() + usize::from(matches!(row.rest, Rest::More(_)));
                edges = edges.checked_add(count).ok_or(limit)?;
                if edges > limit.edges {
                    return Err(limit);
                }
                pending.extend(
                    row.labels
                        .values()
                        .map(|field| Work::Type(field.ty.clone())),
                );
                if let Rest::More(rest) = &row.rest {
                    pending.push(Work::Row((**rest).clone()));
                }
            }
        }
        debug_assert!(pending.len().saturating_sub(before) <= MAX_TYPE_EDGES);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(args: Arc<[Arc<Ty>]>) -> Arc<Ty> {
        Arc::new(Ty::Named {
            symbol: Symbol::GENERATED,
            name: "Graph".into(),
            args,
        })
    }

    #[test]
    fn shared_type_graphs_count_allocations_without_expanding_paths() {
        let mut graph = Arc::new(Ty::Nat);
        for _ in 0..1024 {
            graph = named(Arc::from([graph.clone(), graph]));
        }
        assert!(check_type_graph(&graph, Arc::clone, Clone::clone).is_ok());
    }

    #[test]
    fn independently_allocated_type_nodes_have_a_publication_limit() {
        let graph = named((0..MAX_TYPE_NODES).map(|_| Arc::new(Ty::Nat)).collect());
        assert!(check_type_graph(&graph, Arc::clone, Clone::clone).is_err());
    }

    #[test]
    fn structural_captures_have_a_smaller_metadata_allowance() {
        let ordinary = named(
            (0..MAX_CONTRACT_TYPE_NODES)
                .map(|_| Arc::new(Ty::Nat))
                .collect(),
        );
        assert!(check_type_graph(&ordinary, Arc::clone, Clone::clone).is_ok());
        let contract = Arc::new(Ty::Contract {
            fallback: ordinary,
            contract: Arc::new(Contract {
                parameters: 1,
                body: Arc::new(Expr::Input(0)),
                captures: Arc::from([]),
                arguments: Arc::from([]),
                effect_sink: None,
            }),
        });
        assert!(check_type_graph(&contract, Arc::clone, Clone::clone).is_err());
    }

    #[test]
    fn excessive_edges_are_rejected_before_queuing_their_children() {
        let value = Arc::new(Ty::Nat);
        let graph = named(vec![value; MAX_TYPE_EDGES + 1].into());
        assert!(check_type_graph(&graph, Arc::clone, Clone::clone).is_err());
    }
}
