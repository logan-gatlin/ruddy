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

use ruddy::inference::{Constraint, ConstraintKind};

use crate::{
    stage::{Cx, Ids, Spec, plural},
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
        let asked = counted(constraints);
        total += asked;
        let mut node = Node::new(
            ids.next(),
            format!("let {}", mint.name(*symbol)),
            plural(asked, "constraint"),
        );
        if let Some(decl) = program.terms.get(symbol) {
            node = node.at(decl.name_span);
        }
        if let Some(index) = cx.symbols.get(symbol) {
            node = node.symbol(*index);
        }
        node = node.children(rows(&mut ids, constraints));
        nodes.push(node);
    }

    // No time of its own: generation and solving are one `infer` call, timed
    // once, and the `Types` tab is where that figure is reported. A second
    // chip would be the same microseconds counted twice.
    Stage {
        nodes,
        debug: format!("{:#?}", output.constraints),
        ..spec.stage(cx.status(), plural(total, "constraint"))
    }
}

/// One row per constraint, in the order generation emitted them.
///
/// A list, until a nested `let` made it a tree: a `let` holds what its value
/// requires and what its body requires, solved in that order and at two
/// different levels, so its two lists sit beneath it rather than in line with
/// it. That nesting *is* the solve order, which is the whole reason the tab
/// shows it.
fn rows(ids: &mut Ids, constraints: &[Constraint]) -> Vec<Node> {
    constraints
        .iter()
        .map(|constraint| {
            // Each constraint wears the span the program said it at, which is
            // what lets clicking one highlight the term that demanded it.
            let node = Node::new(ids.next(), constraint.kind.code(), constraint.to_string())
                .at(constraint.span);
            match &constraint.kind {
                ConstraintKind::Let { value, body, .. } => {
                    node.children(rows(ids, value)).children(rows(ids, body))
                }
                ConstraintKind::Equal { .. } | ConstraintKind::Instance { .. } => node,
            }
        })
        .collect()
}

/// How many constraints a list holds, counting the ones a nested `let` carries
/// under it. The summary is a count of what the tab shows, and a `let` shows
/// everything inside it.
fn counted(constraints: &[Constraint]) -> usize {
    constraints
        .iter()
        .map(|constraint| match &constraint.kind {
            ConstraintKind::Let { value, body, .. } => 1 + counted(value) + counted(body),
            ConstraintKind::Equal { .. } | ConstraintKind::Instance { .. } => 1,
        })
        .sum()
}
