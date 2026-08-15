//! The lowered instruction stream, as a tree.
//!
//! One root per global and then per function, wearing its header line; under it one
//! row per instruction, labelled with its opcode and showing the line the
//! listing would print — representation and all — with the child blocks of a
//! `catch` or a `switch` nested underneath, each behind the answer that selects
//! it. The rows come from [`print::lir`], which is also what the tests read, so
//! the tab and the canonical listing cannot drift apart.
//!
//! Temps are numbered program-wide, so the tab lights `%17` wherever it appears
//! — its one definition and every use, in whatever function each of them is in.
//! That is why the highlight is unscoped where the type tabs' is not.
//!
//! The globals lead, which is the other way round from the listing: a reader
//! opening the tab is looking for the definition they wrote, while a reader of
//! the listing wants the functions a global's one line refers to already above
//! it.
//!
//! LIR runs only on a program every earlier phase accepted, so this tab reports
//! `Skipped` for anything with an error in it — including one the reader has
//! only just typed.

use ruddy::lir::{Block, Instr, Output, Terminator};

use crate::{
    print,
    stage::{Cx, Ids, Spec, plural},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(output) = cx.lir else {
        return crate::stage::skipped(spec, "lowering to LIR did not run");
    };

    let mut ids = Ids::default();
    let mut nodes = Vec::new();
    for global in &output.globals {
        let node = Node::new(ids.next(), "global", print::lir::header(global)).at(global.span);
        nodes.push(node.children(rows(output, &global.body, &mut ids)));
    }
    for function in &output.functions {
        let node = Node::new(ids.next(), "fn", print::lir::signature(function)).at(function.span);
        nodes.push(node.children(rows(output, &function.body, &mut ids)));
    }

    Stage {
        micros: Some(cx.micros.lir),
        nodes,
        debug: format!("{output:#?}"),
        ..spec.stage(
            cx.status(),
            format!(
                "{} · {}",
                plural(output.globals.len(), "global"),
                plural(output.functions.len(), "function")
            ),
        )
    }
}

/// One block's rows: its instructions, then the terminator that ends it.
fn rows(output: &Output, block: &Block, ids: &mut Ids) -> Vec<Node> {
    let mut nodes: Vec<Node> = block
        .instrs
        .iter()
        .map(|instr| row(output, instr, ids))
        .collect();
    nodes.push(end(&block.end, ids));
    nodes
}

/// One instruction, with whatever blocks it owns nested under it. A `catch`'s
/// single body hangs directly off the instruction; a dispatch's blocks each hang
/// off the answer that selects them.
fn row(output: &Output, instr: &Instr, ids: &mut Ids) -> Node {
    let mut node = Node::new(
        ids.next(),
        print::lir::opcode(&instr.op),
        print::lir::instruction(output, instr),
    )
    .at(instr.span);
    for (label, block) in print::lir::arms(&instr.op) {
        node = match label {
            Some(label) => {
                let case = Node::new(ids.next(), "case", format!("{label} =>")).at(instr.span);
                node.child(case.children(rows(output, block, ids)))
            }
            None => node.children(rows(output, block, ids)),
        };
    }
    node
}

fn end(terminator: &Terminator, ids: &mut Ids) -> Node {
    Node::new(
        ids.next(),
        print::lir::end_label(terminator),
        print::lir::terminator(terminator),
    )
    .at(terminator.span)
}
