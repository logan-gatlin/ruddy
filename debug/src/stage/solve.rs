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
//!
//! The one piece of solver state with no field of its own is the stack of goals
//! held open across an unfolding, and it needs none: an `assume` row's goal is
//! that stack's key exactly — two declared types with their arguments — so the
//! row already says which question was taken as answered, and a field for it
//! would print the goal column twice.

use std::collections::HashMap;

use ruddy::inference::{DefaultAssignment, DefaultBinding, Effect, ReasonOrigin, VarSort};

use crate::{
    stage::{Cx, Ids, Spec, plural},
    wire::{Node, Stage},
};

fn sort_code(sort: VarSort) -> &'static str {
    match sort {
        VarSort::Region => "region",
        VarSort::Type => "type",
        VarSort::Row => "row",
        VarSort::Presence => "presence",
    }
}

fn reason_fields(
    origin: ReasonOrigin,
) -> (
    &'static str,
    Option<String>,
    Option<&'static str>,
    Option<&'static str>,
) {
    match origin {
        ReasonOrigin::Variable { sort, subject } => (
            "variable",
            None,
            Some(subject.code()),
            Some(sort_code(sort)),
        ),
        ReasonOrigin::Constraint(id) => ("constraint", Some(id.to_string()), None, None),
        ReasonOrigin::Contract(id) => ("contract", Some(id.to_string()), None, None),
        ReasonOrigin::Batch(id) => ("batch", Some(id.to_string()), None, None),
        ReasonOrigin::Step(id) => ("step", Some(id.to_string()), None, None),
        ReasonOrigin::Recovery => ("recovery", None, None, None),
        ReasonOrigin::DefaultBinding { var, kind, .. } => (
            match kind {
                DefaultBinding::Sat => "sat-binding",
                DefaultBinding::CloseEffects => "close-effects-binding",
            },
            Some(var.to_string()),
            None,
            None,
        ),
    }
}

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    // A step names its definition by symbol and carries its own types, so the
    // mint is the only thing outside the solve this needs.
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference.map(|output| output.diagnostics()) else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let error_messages: HashMap<_, _> = output
        .errors()
        .iter()
        .map(|error| (error.id, error.diagnostic(cx.source).title))
        .collect();
    let mut ids = Ids::default();
    let mut nodes: Vec<Node> = output
        .steps()
        .iter()
        .map(|step| {
            let effect = step.effect.to_string();
            let mut node = Node::new(ids.next(), step.rule.code(), step.goal.to_string())
                .at(cx.source.span(step.at))
                // What the rule does, in the compiler's own words rather than
                // the page's: one wording, wherever it is read.
                .field("_rule", step.rule.to_string())
                .field("_effect", effect.clone())
                .field("_depth", step.depth.to_string())
                .field("_step_id", step.id.to_string())
                .field("_reason_id", step.reason.to_string())
                .field(
                    "_constraint_id",
                    step.constraint.map(|id| id.to_string()).unwrap_or_default(),
                )
                .field(
                    "_error_id",
                    step.error.map(|id| id.to_string()).unwrap_or_default(),
                )
                .field("_def", mint.name(step.definition).to_string());

            // The two fields the page accumulates, present only on the steps
            // that changed something, so the page appends without deciding.
            match &step.effect {
                // The solution panel is the `_effect` column's bindings
                // collected, so it says what that column says — the compiler's
                // own `Display for Effect` — rather than spelling `?4 := Nat`
                // a second time here. It had been spelled twice, identically,
                // which is two places for one notation to change in.
                Effect::Bound { by, because, .. } => {
                    node = node
                        .field("_bind", effect)
                        .field("_bind_by", by.to_string());
                    if let Some(because) = because {
                        node = node.field("_recovery_because", because.to_string());
                    }
                }
                Effect::Failed(kind) => {
                    let message = step
                        .error
                        .and_then(|id| error_messages.get(&id).cloned())
                        .unwrap_or_else(|| kind.to_string());
                    node = node.field("_error", message).error();
                }
                Effect::None | Effect::Decomposed | Effect::Guarded { .. } => {}
            }

            // `owner`, not `symbol`: a step is spanned by the sub-expression
            // its constraint came from, which is nowhere the definition's name
            // was written. Claiming it as an occurrence made clicking `fst`
            // paint `p.x` in the editor as a use of `fst`.
            if let Some(index) = cx.symbols.get(&step.definition) {
                node = node.owner(*index);
            }
            node
        })
        .collect();

    // Provenance is debugger wire data, not an opaque Rust Debug appendix.
    // Per-step scalar fields make the common query cheap; complete JSON arenas
    // ride on one explicitly tagged metadata record, present even when there
    // are no steps and excluded from solve replay by wire consumers.
    let reason_by_id: HashMap<_, _> = output
        .reasons()
        .iter()
        .map(|reason| (reason.id, reason))
        .collect();
    for (step, node) in output.steps().iter().zip(&mut nodes) {
        let reason = reason_by_id
            .get(&step.reason)
            .expect("every published step has a reason");
        let mut decorated = std::mem::take(node)
            .field(
                "_reason_parents",
                reason
                    .parents
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            )
            .field("_reason_origin", "step");
        if let Effect::Bound { var, .. } = step.effect {
            let meta = output
                .variable(step.id.scope(), var)
                .expect("every bound variable was minted in the step's scope");
            decorated = decorated
                .field("_var_id", var.to_string())
                .field("_var_sort", sort_code(meta.sort))
                .field("_var_subject", meta.subject.code())
                .field("_var_minted_by", meta.minted_by.to_string());
        }
        *node = decorated;
    }
    {
        // A variable's number is its table's; the scope says which table.
        let variables: Vec<_> = output
            .variables()
            .iter()
            .flat_map(|(scope, variables)| {
                variables.iter().enumerate().map(|(var, meta)| {
                    serde_json::json!({
                        "id": var,
                        "scope": format!("{:x}", scope.bits()),
                        "sort": sort_code(meta.sort),
                        "subject": meta.subject.code(),
                        "minted_by": meta.minted_by.to_string(),
                    })
                })
            })
            .collect();
        let reasons: Vec<_> = output
            .reasons()
            .iter()
            .map(|reason| {
                let (origin, origin_id, subject, sort) = reason_fields(reason.origin);
                let assigned = match reason.origin {
                    ReasonOrigin::DefaultBinding { assigned, .. } => Some(match assigned {
                        DefaultAssignment::Present => "present",
                        DefaultAssignment::Absent => "absent",
                        DefaultAssignment::EmptyRow => "empty-row",
                    }),
                    _ => None,
                };
                serde_json::json!({
                    "id": reason.id.to_string(),
                    "parents": reason.parents.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                    "origin": origin,
                    "origin_id": origin_id,
                    "sort": sort,
                    "subject": subject,
                    "assigned": assigned,
                    "reachable": reason.reachable,
                })
            })
            .collect();
        let metadata = Node::new(ids.next(), "metadata", "")
            .field("_record", "metadata")
            .field("_variables", serde_json::to_string(&variables).unwrap())
            .field("_reasons", serde_json::to_string(&reasons).unwrap());
        nodes.insert(0, metadata);
    }

    // No time of its own: the inference phase is timed once, on `Types`, which
    // is what leaving `micros` unset says.
    let summary = plural(output.steps().len(), "step");
    Stage {
        nodes,
        debug: format!(
            "steps = {:#?}\nvariables = {:#?}\nreasons = {:#?}",
            output.steps(),
            output.variables(),
            output.reasons()
        ),
        ..spec.stage(cx.status(), summary)
    }
}
