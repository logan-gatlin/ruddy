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
    stage::{Cx, Ids, Spec, plural, with_symbol},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference.map(|output| output.diagnostics()) else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let mut ids = Ids::default();
    let mut total = 0;
    let mut nodes = Vec::new();

    // One row per definition, in the order inference solved them, so a
    // definition that constrains nothing is still visible as the empty group
    // it is rather than missing from the tab.
    for (symbol, constraints) in output.constraints() {
        let asked = counted(constraints);
        total += asked;
        let mut node = Node::new(
            ids.next(),
            format!("let {}", mint.name(*symbol)),
            plural(asked, "constraint"),
        );
        if let Some(decl) = program.terms.get(symbol) {
            node = node.at(cx.source.span(decl.name_at));
        }
        node = with_symbol(node, cx, mint, *symbol);
        node = node.children(rows(&mut ids, cx, constraints));
        nodes.push(node);
    }

    // No time of its own: generation and solving are one `infer` call, timed
    // once, and the `Types` tab is where that figure is reported. A second
    // chip would be the same microseconds counted twice.
    Stage {
        nodes,
        debug: format!("{:#?}", output.constraints()),
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
fn rows(ids: &mut Ids, cx: &Cx, constraints: &[Constraint]) -> Vec<Node> {
    constraints
        .iter()
        .map(|constraint| {
            // Each constraint wears the span the program said it at, which is
            // what lets clicking one highlight the term that demanded it.
            let mut node = Node::new(ids.next(), constraint.kind.code(), constraint.to_string())
                .at(cx.source.span(constraint.at))
                .field("_constraint_id", constraint.id.to_string())
                .field("_reason_id", constraint.reason.to_string())
                .field("_origin", constraint.origin.code())
                .field("_primary_subject", constraint.subjects.primary.code());
            if let Some(subject) = constraint.subjects.secondary {
                node = node.field("_secondary_subject", subject.code());
            }
            match &constraint.kind {
                ConstraintKind::Let { value, body, .. } => node
                    .children(rows(ids, cx, value))
                    .children(rows(ids, cx, body)),
                ConstraintKind::Match { arms, .. } => node.children(
                    arms.iter()
                        .enumerate()
                        .map(|(at, arm)| {
                            let arm_node = Node::new(
                                ids.next(),
                                format!("arm {at}"),
                                format!("{} -> {}", arm.raw, arm.effective),
                            )
                            .at(cx.source.span(arm.at));
                            let arm_node =
                                arm.requirements.iter().fold(arm_node, |node, requirement| {
                                    node.child(
                                        Node::new(
                                            ids.next(),
                                            requirement.batch.origin.code(),
                                            requirement.batch.formula.to_string(),
                                        )
                                        .at(cx.source.span(requirement.batch.at))
                                        .field("_batch_id", requirement.batch.id.to_string()),
                                    )
                                });
                            arm_node
                                .children(rows(ids, cx, &arm.constraints))
                                .children(rows(ids, cx, std::slice::from_ref(&arm.result)))
                        })
                        .collect::<Vec<_>>(),
                ),
                ConstraintKind::Project { .. }
                | ConstraintKind::Spread { .. }
                | ConstraintKind::Equal { .. }
                | ConstraintKind::Instance { .. }
                | ConstraintKind::Performs { .. }
                | ConstraintKind::CallbackCoverage { .. } => node,
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
            ConstraintKind::Match { arms, .. } => {
                1 + arms
                    .iter()
                    .map(|arm| arm.requirements.len() + counted(&arm.constraints) + 1)
                    .sum::<usize>()
            }
            ConstraintKind::Project { .. }
            | ConstraintKind::Spread { .. }
            | ConstraintKind::Equal { .. }
            | ConstraintKind::Instance { .. }
            | ConstraintKind::Performs { .. }
            | ConstraintKind::CallbackCoverage { .. } => 1,
        })
        .sum()
}
