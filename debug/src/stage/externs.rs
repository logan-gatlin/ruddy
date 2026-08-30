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
            let target = format!("{:?}", decl.value.target.tracked);
            let mut node = with_symbol(
                Node::new(
                    ids.next(),
                    format!("extern {}", mint.name(*symbol)),
                    target.clone(),
                )
                .at(decl.name_span),
                cx,
                mint,
                *symbol,
            );
            if let Some(inferred) = cx.inference.and_then(|output| output.externs.get(symbol)) {
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
            node.child(Node::new(ids.next(), "Target", target).at(decl.value.target.span))
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
