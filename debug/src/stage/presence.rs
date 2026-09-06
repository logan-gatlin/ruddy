//! The propositional constraint store, as inference built it.
//!
//! One section per batch, in program order: what the batch came from — a
//! match's coverage, a use of a constrained name, an annotation's own clause —
//! the formula it contributed, and the running verdict once it is conjoined
//! with everything before it. The batch that turned that verdict from
//! satisfiable to unsatisfiable is marked, and it is the only one that ever is:
//! the cascade rule gives it the single resulting error and suppresses what
//! follows.
//!
//! Under the batches, what each definition's scheme came to require of the
//! presences it quantifies — the `where` clause a reader sees on the type —
//! and beside it what the patterns phase walks that definition under, which is
//! the same store read over every presence the body can name rather than only
//! the ones the type does. The ordinary program is the one with no section at
//! all, which is what R13 asks the tab to look like.
//!
//! Every row carries its origin's span, so clicking one lights the match, the
//! use site or the annotation it came from, exactly as the other stages do.

use ruddy::{
    inference::{Origin, display_presence_path, effective_conditions, sat},
    tracking::Span,
    types::Formula,
};

use crate::{
    print,
    stage::{Cx, Ids, Spec, plural, with_symbol},
    wire::{Node, Stage},
};

fn origin_details(ids: &mut Ids, origin: &Origin, span: Span) -> Vec<Node> {
    match origin {
        Origin::Coverage(coverage) => {
            let effective = effective_conditions(&coverage.arms);
            let mut rows = Vec::new();
            for (arm, (raw, ordered)) in coverage.arms.iter().zip(effective).enumerate() {
                rows.push(
                    Node::new(ids.next(), format!("arm {arm} raw"), raw.to_string()).at(span),
                );
                rows.push(
                    Node::new(
                        ids.next(),
                        format!("arm {arm} effective"),
                        ordered.to_string(),
                    )
                    .at(span),
                );
            }
            for (name, presence) in &coverage.fields {
                rows.push(
                    Node::new(
                        ids.next(),
                        format!("field {}", print::label(print::Shape::Struct, name)),
                        presence.to_string(),
                    )
                    .at(span),
                );
            }
            rows
        }
        Origin::Instance(named) | Origin::Annotation(named) | Origin::Refinement(named) => named
            .labels
            .iter()
            .map(|(name, presence)| {
                Node::new(ids.next(), format!("label {name}"), presence.to_string()).at(span)
            })
            .collect(),
        Origin::Guarded(guarded) => {
            let mut rows = vec![
                Node::new(ids.next(), "premise", guarded.premise.to_string()).at(span),
                Node::new(ids.next(), "obligation", guarded.obligation.to_string()).at(span),
            ];
            rows.extend(origin_details(ids, &guarded.origin, span));
            rows
        }
    }
}

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };
    let semantics = output.semantics();
    let diagnostics = output.diagnostics();

    let mut ids = Ids::default();
    let mut nodes: Vec<Node> = Vec::new();

    // The verdict is the running one, so a reader can watch the store narrow
    // batch by batch rather than being shown only where it ended up. Rebuilt as
    // the walk goes for the same reason inference rebuilds it: what a batch
    // says is only interesting beside everything said before it.
    //
    // One scope at a time: a batch's variables are numbered by the group that
    // solved it, so a running conjunction across groups would be reading two
    // groups' `?0` as one. The verdict starts over where the scope changes.
    let mut accumulated = Formula::True;
    // Which batch flipped the store, once one has: the verdict stays
    // unsatisfiable for every batch after it, and a row that named the batch it
    // is printed beside would blame each of them in turn for the one thing only
    // the first of them did.
    let mut flipped: Option<usize> = None;
    let mut scope = None;
    for (at, batch) in semantics.store().batches.iter().enumerate() {
        if scope != Some(batch.id.scope()) {
            scope = Some(batch.id.scope());
            accumulated = Formula::True;
            flipped = None;
        }
        accumulated = accumulated.and(batch.formula.clone());
        if batch.flipped {
            flipped = Some(at);
        }
        let mut node = Node::new(ids.next(), batch.origin.code(), batch.formula.to_string())
            .at(cx.source.span(batch.at))
            .field("_batch_id", batch.id.to_string())
            .field("_reason_id", batch.reason.to_string());
        if batch.flipped {
            node = node.error();
        }
        node = node.child(
            Node::new(ids.next(), "origin", batch.origin.to_string()).at(cx.source.span(batch.at)),
        );
        // The verdict, with the evidence beside it: a model where there is one,
        // and the mark on the batch itself where there is not.
        let verdict = match sat::model(&accumulated) {
            Some(model) => {
                let mut named: Vec<String> = model
                    .into_iter()
                    .map(|(atom, there)| match there {
                        true => format!("{atom}"),
                        false => format!("not {atom}"),
                    })
                    .collect();
                // Sorted, because a solver's model has no order of its own and
                // a row that reshuffled between two identical runs would be a
                // row nobody could read.
                named.sort();
                let row = match named.is_empty() {
                    true => "satisfiable".to_string(),
                    false => format!("satisfiable: {}", named.join(", ")),
                };
                Node::new(ids.next(), "verdict", row)
            }
            None => Node::new(
                ids.next(),
                "verdict",
                match flipped {
                    Some(flipped) => format!("unsatisfiable — flipped by batch {flipped}"),
                    // Only reachable from a store that never marked a flip,
                    // which inference does not build; a row saying what it can
                    // see beats one asserting a batch number it cannot.
                    None => "unsatisfiable".to_string(),
                },
            )
            .error(),
        };
        node = node.child(verdict.at(cx.source.span(batch.at)));
        node = node.children(origin_details(
            &mut ids,
            &batch.origin,
            cx.source.span(batch.at),
        ));
        nodes.push(node);
    }

    // The readable refinement trace: one match group and one row per written
    // arm, with the raw coverage beside its ordered (and, when nested,
    // conjoined) assumption, the boundary SAT verdict, entailed named facts,
    // and every implication structural solving emitted.
    let mut groups: Vec<(Span, Vec<&ruddy::inference::Refinement>)> = Vec::new();
    for refinement in diagnostics.refinements() {
        match groups
            .iter_mut()
            .find(|(span, _)| *span == cx.source.span(refinement.match_at))
        {
            Some((_, arms)) => arms.push(refinement),
            None => groups.push((cx.source.span(refinement.match_at), vec![refinement])),
        }
    }
    for (span, arms) in groups {
        let mut matched =
            Node::new(ids.next(), "match refinement", plural(arms.len(), "arm")).at(span);
        for (arm_at, refinement) in arms.into_iter().enumerate() {
            let mut arm = Node::new(
                ids.next(),
                format!("arm {arm_at}"),
                match refinement.reachable {
                    true => "assumption satisfiable",
                    false => "assumption unreachable — solved without refinement",
                },
            )
            .at(cx.source.span(refinement.arm_at))
            .child(
                Node::new(ids.next(), "raw coverage", refinement.raw.to_string())
                    .at(cx.source.span(refinement.arm_at)),
            )
            .child(
                Node::new(
                    ids.next(),
                    "effective assumption",
                    refinement.effective.to_string(),
                )
                .at(cx.source.span(refinement.arm_at)),
            );
            let path = |segments: &[String]| display_presence_path(segments);
            for (name, presence) in &refinement.fields {
                arm = arm.child(
                    Node::new(
                        ids.next(),
                        format!("presence {}", path(name)),
                        presence.to_string(),
                    )
                    .at(cx.source.span(refinement.arm_at)),
                );
            }
            for fact in &refinement.facts {
                arm = arm.child(
                    Node::new(
                        ids.next(),
                        "entailed",
                        format!(
                            "{}{}",
                            if fact.present { "" } else { "not " },
                            path(&fact.field)
                        ),
                    )
                    .at(cx.source.span(refinement.arm_at)),
                );
            }
            for obligation in &refinement.obligations {
                arm = arm.child(
                    Node::new(
                        ids.next(),
                        "guarded obligation",
                        format!(
                            "{} -> {} ({})",
                            obligation.premise, obligation.obligation, obligation.formula
                        ),
                    )
                    .at(cx.source.span(obligation.at)),
                );
            }
            matched = matched.child(arm);
        }
        nodes.push(matched);
    }

    // And what each definition ended up promising: the `where` clause on its
    // scheme, which is the whole of what the store bought.
    //
    // Every definition, including the ones that promise nothing. A tab about
    // presence is the place to be able to see that a definition's presences are
    // independent, and a row saying so is a shorter answer than a reader
    // scanning the list for a name that is not there.
    for (symbol, scheme) in semantics.schemes() {
        let clause = match scheme.formula().is_true() {
            true => "unconstrained".to_string(),
            false => format!("where {}", scheme.formula()),
        };
        let row = Node::new(ids.next(), format!("let {}", mint.name(*symbol)), clause);
        let row = match cx.program.and_then(|program| program.terms.get(symbol)) {
            Some(decl) => row.at(cx.source.span(decl.name_at)),
            None => row,
        };
        // Beside the clause, what the patterns phase walks the definition
        // under. The two differ whenever a nested binding has presences of its
        // own: the scheme quantifies only what its own type mentions, and a
        // reader who saw only that could not explain a reachability verdict
        // that turned on a nested `let`'s constraint.
        let promise = match semantics.promises().get(symbol) {
            Some(promise) if !promise.is_true() => promise.to_string(),
            _ => "unconstrained".to_string(),
        };
        let row = row.child(Node::new(ids.next(), "patterns assume", promise));
        nodes.push(with_symbol(row, cx, mint, *symbol));
    }

    Stage {
        micros: Some(cx.micros.infer),
        nodes,
        debug: format!(
            "store: {:#?}\nrefinements: {:#?}",
            semantics.store(),
            diagnostics.refinements()
        ),
        ..spec.stage(
            cx.status(),
            plural(semantics.store().batches.len(), "constraint"),
        )
    }
}
