//! The lowered instruction stream, as a tree.
//!
//! Functions own flat parameterized blocks. Each block lists value instructions
//! and one terminal transfer, including the destination and capture environment
//! of saved continuations. Rendering is shared with the canonical LIR listing.
//!
//! Temp identities are preserved when forwarded into another block, so the
//! debugger highlights each occurrence across the function's control flow.
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

    let labels = cx
        .program
        .map_or_else(Default::default, print::lir::Labels::new);
    let mut ids = Ids::default();
    let mut nodes = Vec::new();
    for external in &output.externs {
        nodes.push(
            Node::new(ids.next(), "extern", print::lir::extern_header(external)).at(external.span),
        );
    }
    for global in &output.globals {
        let node = Node::new(ids.next(), "global", print::lir::header(global)).at(global.span);
        nodes.push(node.child(Node::new(
            ids.next(),
            "initializer",
            format!("f{}", global.initializer),
        )));
    }
    for (id, function) in output.functions.iter().enumerate() {
        let node = Node::new(ids.next(), "fn", print::lir::function_header(id, function))
            .at(function.span);
        let blocks: Vec<_> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(id, b)| {
                let node = Node::new(ids.next(), "block", print::lir::block_header(id, b));
                node.children(rows(output, &labels, b, &mut ids))
            })
            .collect();
        nodes.push(node.children(blocks));
    }

    Stage {
        micros: Some(cx.micros.lir),
        nodes,
        debug: format!("{output:#?}"),
        ..spec.stage(
            cx.status(),
            match output.externs.is_empty() {
                true => format!(
                    "{} · {}",
                    plural(output.globals.len(), "global"),
                    plural(output.functions.len(), "function")
                ),
                false => format!(
                    "{} · {} · {}",
                    plural(output.externs.len(), "extern"),
                    plural(output.globals.len(), "global"),
                    plural(output.functions.len(), "function")
                ),
            },
        )
    }
}

/// One block's rows: its instructions, then the terminator that ends it.
fn rows(output: &Output, labels: &print::lir::Labels, block: &Block, ids: &mut Ids) -> Vec<Node> {
    let mut nodes: Vec<Node> = block
        .instrs
        .iter()
        .map(|instr| row(output, labels, instr, ids))
        .collect();
    nodes.push(end(output, &block.end, ids));
    nodes
}

/// One value instruction, with its source navigation anchor.
fn row(output: &Output, labels: &print::lir::Labels, instr: &Instr, ids: &mut Ids) -> Node {
    Node::new(
        ids.next(),
        print::lir::opcode(&instr.op),
        print::lir::instruction(output, labels, instr),
    )
    .at(instr.span)
}

fn end(output: &Output, terminator: &Terminator, ids: &mut Ids) -> Node {
    Node::new(
        ids.next(),
        print::lir::end_label(terminator),
        print::lir::terminator(output, terminator),
    )
    .at(terminator.span)
}
