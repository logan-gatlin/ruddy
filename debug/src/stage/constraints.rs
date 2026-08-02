//! What inference asked for, before it worked out the answer.
//!
//! Inference types each definition in two passes, and this tab is the first
//! one's whole output: the constraints generation emitted, in the order it
//! emitted them, grouped under the definition they came from. The `Types` tab
//! is the second pass's answer to them, so reading the two side by side is
//! reading the solver's input and its output.
//!
//! The types here are deliberately *unsolved* — `?3 ~ Nat`, not `Nat ~ Nat`.
//! A constraint that already showed its answer would say nothing about the
//! pass that produced it, and the answer is one tab away.

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let mut ids = Ids::default();
    let mut total = 0;
    let mut nodes = Vec::new();

    // One row per definition, in the order inference solved them, so a
    // definition that constrains nothing is still visible as the empty group
    // it is rather than missing from the tab.
    for (symbol, constraints) in &output.constraints {
        total += constraints.len();
        let mut node = Node::new(
            ids.next(),
            format!("let {}", mint.name(*symbol)),
            match constraints.len() {
                1 => "1 constraint".to_string(),
                count => format!("{count} constraints"),
            },
        );
        if let Some(decl) = program.terms.get(symbol) {
            node = node.at(decl.name_span);
        }
        if let Some(index) = cx.symbols.get(symbol) {
            node = node.symbol(*index);
        }
        // Each constraint wears the span the program said it at, which is what
        // lets clicking one highlight the term that demanded it.
        node = node.children(constraints.iter().map(|constraint| {
            Node::new(ids.next(), constraint.kind.code(), constraint.to_string())
                .at(constraint.span)
        }));
        nodes.push(node);
    }

    // No time of its own: generation and solving are one `infer` call, timed
    // once, and the `Types` tab is where that figure is reported. A second
    // chip would be the same microseconds counted twice.
    Stage {
        nodes,
        debug: format!("{:#?}", output.constraints),
        ..spec.stage(cx.status(), format!("{total} constraints"))
    }
}
