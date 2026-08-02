//! The solve, one rule at a time.
//!
//! Where the `Constraints` tab is what generation asked and the `Types` tab is
//! what came back, this is the middle: every act the solver performed, in
//! order, so it can be stepped through rather than only read as its result.
//!
//! The page keeps a cursor into this list and shows three things from it — the
//! rule about to fire, the solution so far, and the errors so far. Only the
//! first of those is a row; the other two the page accumulates itself, from the
//! `_bind` and `_error` fields each step carries. That is the whole reason
//! those are separate from the rendered effect: appending strings to a list is
//! something the page can do without knowing what a binding *is*.

use ruddy::inference::Effect;

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    // A step names its definition by symbol and carries its own types, so the
    // mint is the only thing outside the solve this needs.
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let mut ids = Ids::default();
    let nodes: Vec<Node> = output
        .steps
        .iter()
        .map(|step| {
            let mut node = Node::new(ids.next(), step.rule.code(), step.goal.to_string())
                .at(step.span)
                // What the rule does, in the compiler's own words rather than
                // the page's: one wording, wherever it is read.
                .field("_rule", step.rule.to_string())
                .field("_effect", step.effect.to_string())
                .field("_depth", step.depth.to_string())
                .field("_def", mint.name(step.definition).to_string());

            // The two fields the page accumulates, present only on the steps
            // that changed something, so the page appends without deciding.
            match &step.effect {
                Effect::Bound { var, ty } => node = node.field("_bind", format!("?{var} := {ty}")),
                Effect::Failed(kind) => node = node.field("_error", kind.to_string()).error(),
                Effect::None | Effect::Decomposed => {}
            }

            if let Some(index) = cx.symbols.get(&step.definition) {
                node = node.symbol(*index);
            }
            node
        })
        .collect();

    // No time of its own: the inference phase is timed once, on `Types`.
    let summary = match nodes.len() {
        1 => "1 step".to_string(),
        count => format!("{count} steps"),
    };
    Stage {
        nodes,
        debug: format!("{:#?}", output.steps),
        ..spec.stage(cx.status(), summary)
    }
}
