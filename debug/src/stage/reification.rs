//! Runtime evidence inference, displayed as finite value and demand graphs.
use crate::{
    stage::{Cx, Ids, Spec, with_symbol},
    wire::{Node, Stage},
};
use ruddy::reification::conventions::Shape;

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let (Some(inference), Some(mint)) = (cx.inference, cx.mint) else {
        return crate::stage::skipped(spec, "inference did not run");
    };
    let plan = &inference.semantics().reification().callables;
    let solved = plan.graph.solve();
    let mut ids = Ids::default();
    let mut nodes = Vec::new();
    for (symbol, binding) in &plan.bindings {
        nodes.push(with_symbol(
            Node::new(
                ids.next(),
                mint.name(*symbol),
                format!("value {}", binding.value),
            ),
            cx,
            mint,
            *symbol,
        ));
    }
    for (symbol, slot) in &plan.evidence {
        nodes.push(with_symbol(
            Node::new(
                ids.next(),
                mint.name(*symbol),
                format!("mirror bound by a pattern; evidence for type {slot}"),
            ),
            cx,
            mint,
            *symbol,
        ));
    }
    for (index, shape) in plan.graph.shapes.iter().enumerate() {
        let text = match shape {
            Shape::Lazy => "unobserved component; forwards the supplied callable convention".into(),
            Shape::Arrow {
                argument,
                result,
                needs,
            } => {
                let demands = solved[*needs as usize]
                    .iter()
                    .map(|need| match need.port {
                        Some(port) => format!(
                            "type {} when demand port {port} is supplied",
                            need.parameter
                        ),
                        None => format!("type {}", need.parameter),
                    })
                    .collect::<Vec<_>>();
                format!(
                    "argument {argument}, result {result}; invocation needs {}",
                    if demands.is_empty() {
                        "no descriptors".into()
                    } else {
                        demands.join(", ")
                    }
                )
            }
            _ => format!("{shape:?}"),
        };
        nodes.push(Node::new(ids.next(), format!("value {index}"), text));
    }
    for (at, flow) in &plan.occurrences {
        if !solved[flow.evaluation as usize].is_empty() {
            nodes.push(
                Node::new(
                    ids.next(),
                    "Evaluation",
                    format!(
                        "constructs value {}; needs {:?}",
                        flow.value, solved[flow.evaluation as usize]
                    ),
                )
                .at(cx.source.span(*at)),
            );
        }
    }
    Stage {
        nodes,
        debug: format!("{plan:#?}"),
        ..spec.stage(
            cx.status(),
            format!(
                "{} value shapes, {} demand equations",
                plan.graph.shapes.len(),
                plan.graph.needs.len()
            ),
        )
    }
}
