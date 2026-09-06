//! Target-provided declarations, as a backend import table.

use crate::{
    stage::{Cx, Ids, Spec, plural, with_symbol},
    wire::{Node, Stage},
};

/// Render externs separately from IR because their whole point is that they do
/// not have the initializer tree an ordinary global has. A backend can read
/// this table directly: Ruddy symbol, source-spanned foreign target and
/// declared scheme sit together, with the LIR representation when it ran.
pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let mut ids = Ids::default();
    let mut nodes: Vec<Node> = program
        .externs
        .iter()
        .map(|(symbol, decl)| {
            let target = decl.value.target.to_string();
            let mut node = with_symbol(
                Node::new(
                    ids.next(),
                    format!("extern {}", mint.name(*symbol)),
                    target.clone(),
                )
                .at(cx.source.span(decl.name_at)),
                cx,
                mint,
                *symbol,
            );
            if let Some(inferred) = cx
                .inference
                .and_then(|output| output.semantics().externs().get(symbol))
            {
                node = node.child(Node::new(ids.next(), "Declared", inferred.to_string()));
            }
            if let Some(external) = cx.lir.and_then(|output| {
                output
                    .externs
                    .iter()
                    .find(|external| external.symbol == *symbol)
            }) {
                node = node.child(Node::new(
                    ids.next(),
                    "Representation",
                    crate::print::lir::rep(external.rep),
                ));
            }
            node = node.child(boundary(&mut ids, cx, &decl.value.abi));
            node.child(
                Node::new(ids.next(), "Target", target).at(cx.source.span(decl.value.target.at)),
            )
        })
        .collect();
    if nodes.is_empty() {
        nodes.push(Node::new(
            ids.next(),
            "No externs",
            "this program imports no target values",
        ));
    }
    Stage {
        micros: None,
        nodes,
        debug: format!("{:#?}", program.externs),
        ..spec.stage(cx.status(), plural(program.externs.len(), "extern"))
    }
}

fn boundary(ids: &mut Ids, cx: &Cx, abi: &ruddy::ir::ExternType) -> Node {
    use ruddy::ir::ExternTypeKind;
    let node = Node::new(ids.next(), "Boundary", "").at(cx.source.span(abi.at));
    match &abi.anchored {
        ExternTypeKind::Annotated { metadata, inner } => {
            let mut node = node;
            for (key, attribute) in metadata {
                let (label, text) = if key == "async" {
                    ("Completion protocol", "promise (@async)".to_owned())
                } else {
                    (
                        "Boundary attribute",
                        format!("@{key} {:?}", attribute.value.anchored),
                    )
                };
                node = node
                    .child(Node::new(ids.next(), label, text).at(cx.source.span(attribute.key_at)));
            }
            node.child(boundary(ids, cx, inner))
        }
        ExternTypeKind::Group(inner) => node.child(boundary(ids, cx, inner)),
        ExternTypeKind::Function {
            parameters, result, ..
        } => {
            let mut node = node;
            for (i, parameter) in parameters.iter().enumerate() {
                node = node.child(
                    Node::new(ids.next(), format!("Parameter {}", i + 1), "")
                        .child(boundary(ids, cx, parameter)),
                );
            }
            node.child(Node::new(ids.next(), "Result", "").child(boundary(ids, cx, result)))
        }
        ExternTypeKind::Ordinary(_) => node,
    }
}
