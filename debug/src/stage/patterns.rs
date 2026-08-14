//! What the pattern checks found, per match.
//!
//! One section per match: the solved scrutinee type on the match's own row,
//! the scrutinee's span on a row of its own for cross-highlighting, one row
//! per arm wearing its verdict — reachable, unreachable, starved, or skipped
//! when the typing failed and the checks stood aside — and the coverage line,
//! exhaustive or the witness the phase found. The prose comes from the
//! compiler's own wordings in `ruddy::ui`, so the tab and the diagnostics
//! strip cannot describe one match differently.

use ruddy::patterns::{Coverage, Verdict};

use crate::{
    print,
    stage::{Cx, Ids, Spec, plural},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.patterns else {
        return crate::stage::skipped(spec, "pattern checking did not run");
    };

    let mut ids = Ids::default();
    let nodes: Vec<Node> = output
        .reports
        .iter()
        .map(|report| {
            let mut node = Node::new(ids.next(), "match", report.scrutinee.to_string())
                .at(report.span)
                .child(
                    Node::new(ids.next(), "scrutinee", report.scrutinee.to_string())
                        .at(report.scrutinee_span),
                );
            for arm in &report.arms {
                let row = Node::new(
                    ids.next(),
                    arm.verdict.to_string(),
                    print::ir::pattern(&arm.pattern.tracked, mint).to_string(),
                )
                .at(arm.span);
                let row = match arm.verdict {
                    Verdict::Unreachable => row.error(),
                    _ => row,
                };
                node = node.child(row);
            }
            let coverage = match &report.coverage {
                Coverage::Exhaustive => Node::new(ids.next(), "coverage", "exhaustive"),
                Coverage::Unhandled(witness) => {
                    Node::new(ids.next(), "coverage", format!("unhandled: {witness}")).error()
                }
                Coverage::Skipped => Node::new(ids.next(), "coverage", "skipped"),
            };
            node.child(coverage.at(report.span))
        })
        .collect();

    Stage {
        micros: Some(cx.micros.patterns),
        nodes,
        debug: format!("{output:#?}"),
        ..spec.stage(cx.status(), plural(output.reports.len(), "match"))
    }
}
