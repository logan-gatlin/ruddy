//! What inference concluded, twice over.
//!
//! [`build`] is the `Types` tab: one row per declaration with its scheme (or,
//! for a `type` declaration, the semantic type its alias unfolds to). And
//! [`annotate`] is the same information where it is most useful — painted onto
//! the IR tab's own rows as inline badges, so every subterm shows its inferred
//! type in place. That is what [`Stage::annotates`](crate::wire::Stage) is
//! for: a stage naming another stage's id owns no tab, and its nodes carry
//! *that* stage's node ids rather than ids of their own.

use crate::{
    stage::{Cx, Ids, Spec, Trace},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    let mut ids = Ids::default();
    let mut nodes = Vec::new();

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
        nodes.push(node);
    }

    Stage {
        micros: cx.micros.infer,
        nodes,
        debug: format!("{output:#?}"),
        ..spec.stage(cx.status(), format!("{} schemes", output.schemes.len()))
    }
}

/// The inferred types as badges on the IR tab. Nodes here carry the *IR
/// stage's* ids — that is the whole contract: the page looks each id up among
/// the rows it already renders and paints this node's text beside them.
///
/// The ids come from the [`Trace`] the IR stage published as it built those
/// rows, so this stage neither rebuilds them nor mirrors their shape: it has
/// nothing to be wrong about.
pub fn annotate(spec: &Spec, cx: &Cx, trace: &Trace) -> Stage {
    let Some(program) = cx.program else {
        return crate::stage::skipped(spec, "lowering did not run");
    };
    let Some(output) = cx.inference else {
        return crate::stage::skipped(spec, "inference did not run");
    };

    // A declaration row wears what its definition means as a whole — an
    // unfolded alias, or a scheme — in the order the IR stage renders them.
    let aliases = program
        .types
        .keys()
        .map(|symbol| output.aliases.get(symbol).map(|ty| ty.to_string()));
    let schemes = program
        .terms
        .keys()
        .map(|symbol| output.schemes.get(symbol).map(|scheme| scheme.to_string()));
    let mut nodes: Vec<Node> = trace
        .decls
        .iter()
        .zip(aliases.chain(schemes))
        .filter_map(|(id, text)| Some(Node::new(*id, "", text?)))
        .collect();

    // Every other row is a term, wearing the type inference gave it.
    nodes.extend(
        trace
            .terms
            .iter()
            .map(|(id, ty)| Node::new(*id, "", ty.to_string())),
    );

    let summary = format!("{} annotations", nodes.len());
    Stage {
        nodes,
        ..spec.stage(cx.status(), summary)
    }
}
