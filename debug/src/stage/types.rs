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
    stage::{Cx, Ids, Spec, Trace, plural},
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

    // A row is a keyword, the name it declares, and what that declaration
    // turned out to mean. Aliases and schemes differ in the keyword, in which
    // map the meaning comes out of, and in which map the name's span comes out
    // of — and in nothing else, so they are one row-builder over two sources
    // rather than the same eighteen lines twice.
    //
    // Aliases first, then schemes: the order the program prints in, and the
    // order inference solved them in.
    let aliases = output.aliases.iter().map(|(symbol, ty)| {
        (
            "type",
            *symbol,
            ty.to_string(),
            program.types.get(symbol).map(|decl| decl.name_span),
        )
    });
    let schemes = output.schemes.iter().map(|(symbol, scheme)| {
        (
            "let",
            *symbol,
            scheme.to_string(),
            program.terms.get(symbol).map(|decl| decl.name_span),
        )
    });

    let nodes: Vec<Node> = aliases
        .chain(schemes)
        .map(|(keyword, symbol, meaning, name_span)| {
            let mut node = Node::new(
                ids.next(),
                format!("{keyword} {}", mint.name(symbol)),
                meaning,
            );
            if let Some(span) = name_span {
                node = node.at(span);
            }
            if let Some(index) = cx.symbols.get(&symbol) {
                node = node.symbol(*index);
            }
            node
        })
        .collect();

    Stage {
        micros: Some(cx.micros.infer),
        nodes,
        // Only what this tab owns. `Output` also carries the constraint list
        // and every solver step, which the `Constraints` and `Solve` tabs dump
        // in full already — dumping the whole of it here sent both across the
        // wire a second time on every keystroke, for a raw view nobody would
        // read them in. What is left out and not shown anywhere raw is
        // `errors`, which reaches the page as diagnostics instead.
        debug: format!(
            "aliases: {:#?}\n\nschemes: {:#?}",
            output.aliases, output.schemes
        ),
        ..spec.stage(cx.status(), plural(output.schemes.len(), "scheme"))
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
