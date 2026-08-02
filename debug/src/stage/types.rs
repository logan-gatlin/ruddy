//! What inference concluded, twice over.
//!
//! [`build`] is the `Types` tab: one row per declaration with its scheme (or,
//! for a `type` declaration, the semantic type its alias unfolds to). And
//! [`annotate`] is the same information where it is most useful — painted onto
//! the IR tab's own rows as inline badges, so every subterm shows its inferred
//! type in place. That is what [`Stage::annotates`](crate::wire::Stage) is
//! for: a stage naming another stage's id owns no tab, and its nodes carry
//! *that* stage's node ids rather than ids of their own.

use ruddy::{ir::Term, types::Ty};

use crate::{
    stage::{Cx, Ids},
    wire::{Node, Stage, View},
};

pub fn build(cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped("types", "Types", View::Tree, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped("types", "Types", View::Tree, "inference did not run");
    };

    let mut ids = Ids::default();
    let mut nodes = Vec::new();
    let mut display = String::new();

    // Aliases first, then schemes — the order the program prints in, and the
    // order inference solved them in.
    for (symbol, ty) in &output.aliases {
        let mut node = Node::new(
            ids.next(),
            format!("type {}", mint.name(*symbol)),
            ty.to_string(),
        );
        if let Some(decl) = program.types.get(symbol) {
            node = node.at(decl.name_span);
        }
        if let Some(index) = cx.symbols.get(symbol) {
            node = node.symbol(*index);
        }
        display.push_str(&format!("type {} = {ty}\n", mint.name(*symbol)));
        nodes.push(node);
    }
    for (symbol, scheme) in &output.schemes {
        let mut node = Node::new(
            ids.next(),
            format!("let {}", mint.name(*symbol)),
            scheme.to_string(),
        );
        if let Some(decl) = program.terms.get(symbol) {
            node = node.at(decl.name_span);
        }
        if let Some(index) = cx.symbols.get(symbol) {
            node = node.symbol(*index);
        }
        display.push_str(&format!("let {} : {scheme}\n", mint.name(*symbol)));
        nodes.push(node);
    }

    Stage {
        id: "types",
        title: "Types",
        view: View::Tree,
        status: cx.status(),
        summary: format!("{} schemes", output.schemes.len()),
        micros: cx.micros.infer,
        nodes,
        text: None,
        display,
        debug: format!("{output:#?}"),
        annotates: None,
    }
}

/// The inferred types as badges on the IR tab. Nodes here carry the *IR
/// stage's* ids — that is the whole contract: the page looks each id up among
/// the rows it already renders and paints this node's text beside them.
pub fn annotate(cx: &Cx) -> Stage {
    let stage = |nodes: Vec<Node>, summary: String, status| Stage {
        id: "types-ir",
        title: "Types",
        view: View::Tree,
        status,
        summary,
        micros: 0,
        nodes,
        text: None,
        display: String::new(),
        debug: String::new(),
        annotates: Some("ir"),
    };

    let Some(program) = cx.program else {
        return crate::stage::skipped("types-ir", "Types", View::Tree, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped("types-ir", "Types", View::Tree, "inference did not run");
    };

    // Rebuild the IR stage to learn its node ids. Same input, same builder,
    // same counter — so the ids match the tab the page renders, without the
    // two stages sharing any state.
    let ir = super::ir::build(cx);
    let mut nodes = Vec::new();

    for (node, (symbol, _)) in ir.nodes.iter().zip(&program.types) {
        if let Some(ty) = output.aliases.get(symbol) {
            nodes.push(Node::new(node.id, "", ty.to_string()));
        }
    }
    for (node, (symbol, decl)) in ir
        .nodes
        .iter()
        .skip(program.types.len())
        .zip(&program.terms)
    {
        // The declaration row wears the scheme; the subterms their own types.
        if let Some(scheme) = output.schemes.get(symbol) {
            nodes.push(Node::new(node.id, "", scheme.to_string()));
        }
        if let Some(value) = node.children.last() {
            badge_term(&mut nodes, value, &decl.value);
        }
    }

    let summary = format!("{} annotations", nodes.len());
    stage(nodes, summary, cx.status())
}

/// Walk an IR stage node and the term it was built from in lockstep, emitting
/// one badge per term row. The child layout mirrored here is the one
/// [`ir::term_node`](super::ir) lays down; the `if let`s make a drift between
/// the two a missing badge rather than a panic.
fn badge_term(out: &mut Vec<Node>, node: &Node, term: &Term) {
    out.push(Node::new(node.id, "", term.ty.to_string()));
    match &term.kind {
        ruddy::ir::TermKind::Apply { func, arg } => {
            if let [func_node, arg_node] = node.children.as_slice() {
                badge_term(out, func_node, func);
                badge_term(out, arg_node, arg);
            }
        }
        ruddy::ir::TermKind::Fn { body, .. } => {
            if let [arg_node, body_node] = node.children.as_slice() {
                // The argument has no term of its own to carry a type, but the
                // lambda's arrow knows it.
                if let Ty::Arrow(from, _) = &*term.ty {
                    out.push(Node::new(arg_node.id, "", from.to_string()));
                }
                badge_term(out, body_node, body);
            }
        }
        ruddy::ir::TermKind::Project { base, .. } => {
            if let Some(base_node) = node.children.first() {
                badge_term(out, base_node, base);
            }
        }
        ruddy::ir::TermKind::Struct(fields) => {
            for (wrapper, field) in node.children.iter().zip(fields.values()) {
                if let Some(value_node) = wrapper.children.first() {
                    badge_term(out, value_node, &field.value);
                }
            }
        }
        ruddy::ir::TermKind::Ident(_)
        | ruddy::ir::TermKind::Natural(_)
        | ruddy::ir::TermKind::Error => {}
    }
}
