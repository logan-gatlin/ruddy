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
    inference::{Origin, sat},
    types::Formula,
};

use crate::{
    stage::{Cx, Ids, Spec, plural, with_symbol},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let mut ids = Ids::default();
    let mut nodes: Vec<Node> = Vec::new();

    // The verdict is the running one, so a reader can watch the store narrow
    // batch by batch rather than being shown only where it ended up. Rebuilt as
    // the walk goes for the same reason inference rebuilds it: what a batch
    // says is only interesting beside everything said before it.
    let mut accumulated = Formula::True;
    // Which batch flipped the store, once one has: the verdict stays
    // unsatisfiable for every batch after it, and a row that named the batch it
    // is printed beside would blame each of them in turn for the one thing only
    // the first of them did.
    let mut flipped: Option<usize> = None;
    for (at, batch) in output.store.batches.iter().enumerate() {
        accumulated = accumulated.and(batch.formula.clone());
        if batch.flipped {
            flipped = Some(at);
        }
        let mut node =
            Node::new(ids.next(), batch.origin.code(), batch.formula.to_string()).at(batch.span);
        if batch.flipped {
            node = node.error();
        }
        node = node.child(Node::new(ids.next(), "origin", batch.origin.to_string()).at(batch.span));
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
        node = node.child(verdict.at(batch.span));
        // A match's coverage carries one disjunct per arm and the labels its
        // presences decide; both are what the patterns phase asks its questions
        // of, so both are shown rather than only their disjunction.
        if let Origin::Coverage(coverage) = &batch.origin {
            for (arm, formula) in coverage.arms.iter().enumerate() {
                node = node.child(
                    Node::new(ids.next(), format!("arm {arm}"), formula.to_string()).at(batch.span),
                );
            }
            for (name, presence) in &coverage.fields {
                node = node.child(
                    Node::new(ids.next(), format!("field {name}"), presence.to_string())
                        .at(batch.span),
                );
            }
        }
        // A use site carries which label each of its presences decides, and an
        // annotation the names its own `when`s bound — which is how each one's
        // complaint is worded, and so what a reader needs to read the formula
        // above.
        if let Origin::Instance(named) | Origin::Annotation(named) = &batch.origin {
            for (name, presence) in &named.labels {
                node = node.child(
                    Node::new(ids.next(), format!("label {name}"), presence.to_string())
                        .at(batch.span),
                );
            }
        }
        nodes.push(node);
    }

    // And what each definition ended up promising: the `where` clause on its
    // scheme, which is the whole of what the store bought.
    //
    // Every definition, including the ones that promise nothing. A tab about
    // presence is the place to be able to see that a definition's presences are
    // independent, and a row saying so is a shorter answer than a reader
    // scanning the list for a name that is not there.
    for (symbol, scheme) in &output.schemes {
        let clause = match scheme.formula().is_true() {
            true => "unconstrained".to_string(),
            false => format!("where {}", scheme.formula()),
        };
        let row = Node::new(ids.next(), format!("let {}", mint.name(*symbol)), clause);
        let row = match cx.program.and_then(|program| program.terms.get(symbol)) {
            Some(decl) => row.at(decl.name_span),
            None => row,
        };
        // Beside the clause, what the patterns phase walks the definition
        // under. The two differ whenever a nested binding has presences of its
        // own: the scheme quantifies only what its own type mentions, and a
        // reader who saw only that could not explain a reachability verdict
        // that turned on a nested `let`'s constraint.
        let promise = match output.promises.get(symbol) {
            Some(promise) if !promise.is_true() => promise.to_string(),
            _ => "unconstrained".to_string(),
        };
        let row = row.child(Node::new(ids.next(), "patterns assume", promise));
        nodes.push(with_symbol(row, cx, mint, *symbol));
    }

    Stage {
        micros: Some(cx.micros.infer),
        nodes,
        debug: format!("{:#?}", output.store),
        ..spec.stage(
            cx.status(),
            plural(output.store.batches.len(), "constraint"),
        )
    }
}
