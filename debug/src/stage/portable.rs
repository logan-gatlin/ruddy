//! Which of a program's host values a second target could provide.
//!
//! The JavaScript backend reads an extern's target as JavaScript source, so
//! every extern runs there by construction. Another target implements the
//! portable primitive table instead, and nothing else. This panel holds the
//! linked program's host values against that table, so what keeps a program
//! on one target is visible before another one is written. It runs nothing.

use crate::{
    stage::{Cx, Ids, Spec, plural},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(linked) = cx.linked else {
        return crate::stage::skipped(spec, "static linking did not run");
    };
    let mut ids = Ids::default();
    let externs = &linked.lir().externs;
    let mut portable = 0;
    let mut nodes: Vec<Node> = externs
        .iter()
        .map(|external| {
            let arity = ruddy_interp::prim::arity(&external.target);
            if arity.is_some() {
                portable += 1;
            }
            let node = Node::new(
                ids.next(),
                external.name.clone(),
                match arity {
                    Some(_) => "portable".to_owned(),
                    None => "JavaScript only".to_owned(),
                },
            );
            let node = node.child(Node::new(
                ids.next(),
                "Target",
                external.target.trim().to_owned(),
            ));
            match arity {
                Some(arity) => {
                    node.child(Node::new(ids.next(), "Contract", plural(arity, "argument")))
                }
                None => node.child(Node::new(
                    ids.next(),
                    "Contract",
                    "none: this target text is JavaScript a second target cannot read",
                )),
            }
        })
        .collect();
    if nodes.is_empty() {
        nodes.push(Node::new(
            ids.next(),
            "No host values",
            "this program runs on every target",
        ));
    }
    let summary = if externs.is_empty() {
        "no host values".to_owned()
    } else {
        format!("{portable} of {} portable", externs.len())
    };
    Stage {
        micros: None,
        nodes,
        debug: format!("{:#?}", externs),
        ..spec.stage(cx.status(), summary)
    }
}
