//! The lowered program.
//!
//! Every node that names a symbol carries its index, which is what lets a click
//! on one occurrence highlight the binder and every other use of the same
//! symbol across all panels.

use ruddy::{
    ir::{Decl, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
};

use crate::{
    print,
    stage::{Cx, Ids},
    wire::{Node, Stage, View},
};

pub fn build(cx: &Cx) -> Stage {
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return crate::stage::skipped("ir", "IR", View::Tree, "lowering did not run");
    };

    let mut ids = Ids::default();
    let mut nodes = Vec::new();

    // Types first, then terms — the order `Program`'s own printer uses.
    for (symbol, decl) in &program.types {
        nodes.push(decl_node(
            &mut ids, cx, mint, "type", *symbol, decl, type_node,
        ));
    }
    for (symbol, decl) in &program.terms {
        nodes.push(decl_node(
            &mut ids, cx, mint, "let", *symbol, decl, term_node,
        ));
    }

    Stage {
        id: "ir",
        title: "IR",
        view: View::Tree,
        status: cx.status(),
        summary: format!(
            "{} types · {} terms",
            program.types.len(),
            program.terms.len()
        ),
        micros: cx.micros.build,
        nodes,
        text: None,
        display: print::ir::program(program, mint).to_string(),
        debug: format!("{program:#?}"),
        annotates: None,
    }
}

fn decl_node<T>(
    ids: &mut Ids,
    cx: &Cx,
    mint: &Mint,
    keyword: &str,
    symbol: Symbol,
    decl: &Decl<T>,
    value: impl Fn(&mut Ids, &Cx, &Mint, &T) -> Node,
) -> Node {
    let mut node = Node::new(
        ids.next(),
        format!("{keyword} {}", mint.name(symbol)),
        mint.path(symbol).to_string(),
    )
    .at(decl.name_span);
    if let Some(index) = cx.symbols.get(&symbol) {
        node = node.symbol(*index);
    }
    // Only a term is ever ascribed one, so this child is simply absent on a
    // `type` declaration rather than empty. Labelled the way the AST panel
    // labels the same thing: the role on the type's own node, not a wrapper
    // repeating its text and span.
    if let Some(annotation) = &decl.annotation {
        let mut ascribed = type_node(ids, cx, mint, annotation);
        ascribed.label = format!("Ascribed {}", ascribed.label);
        node = node.child(ascribed);
    }
    node.child(value(ids, cx, mint, &decl.value))
}

fn term_node(ids: &mut Ids, cx: &Cx, mint: &Mint, term: &Term) -> Node {
    let node = Node::new(
        ids.next(),
        "",
        print::ir::term(&term.kind, mint).to_string(),
    )
    .at(term.span);
    match &term.kind {
        TermKind::Error => Node {
            label: "Error".into(),
            ..node
        }
        .error(),
        TermKind::Ident(symbol) => with_symbol(
            Node {
                label: "Ident".into(),
                ..node
            },
            cx,
            *symbol,
        ),
        // A literal names nothing, so this is the one leaf of the term tree
        // that carries neither a symbol nor a child.
        TermKind::Natural(_) => Node {
            label: "Natural".into(),
            ..node
        },
        TermKind::Apply { func, arg } => Node {
            label: "Apply".into(),
            ..node
        }
        .child(term_node(ids, cx, mint, func))
        .child(term_node(ids, cx, mint, arg)),
        TermKind::Fn { arg, body } => {
            let bound = with_symbol(
                Node::new(ids.next(), "Arg", mint.name(arg.tracked)).at(arg.span),
                cx,
                arg.tracked,
            );
            Node {
                label: "Fn".into(),
                ..node
            }
            .child(bound)
            .child(term_node(ids, cx, mint, body))
        }
        // The field names no symbol, so its node is a plain leaf — the one
        // place in the term tree where an identifier-looking thing is not one.
        TermKind::Project { base, field } => Node {
            label: "Project".into(),
            ..node
        }
        .child(term_node(ids, cx, mint, base))
        .child(Node::new(ids.next(), "Field", field.tracked.clone()).at(field.span)),
        TermKind::Struct(fields) => Node {
            label: "Struct".into(),
            ..node
        }
        .children(fields.iter().map(|(name, field)| {
            Node::new(
                ids.next(),
                format!("{name}:"),
                print::ir::term(&field.value.kind, mint).to_string(),
            )
            .at(field.name_span)
            .child(term_node(ids, cx, mint, &field.value))
        })),
    }
}

fn type_node(ids: &mut Ids, cx: &Cx, mint: &Mint, ty: &Type) -> Node {
    let node = Node::new(ids.next(), "", print::ir::ty(&ty.tracked, mint).to_string()).at(ty.span);
    match &ty.tracked {
        TypeKind::Error => Node {
            label: "Error".into(),
            ..node
        }
        .error(),
        TypeKind::Ident(symbol) => with_symbol(
            Node {
                label: "Ident".into(),
                ..node
            },
            cx,
            *symbol,
        ),
        // A primitive is resolved from its spelling rather than from the name
        // table, so there is no symbol to cross-highlight it by.
        TypeKind::Prim(_) => Node {
            label: "Prim".into(),
            ..node
        },
        TypeKind::Arrow { from, to } => Node {
            label: "Arrow".into(),
            ..node
        }
        .child(type_node(ids, cx, mint, from))
        .child(type_node(ids, cx, mint, to)),
        TypeKind::Struct(fields) => Node {
            label: "Struct".into(),
            ..node
        }
        .children(fields.iter().map(|(name, field)| {
            Node::new(
                ids.next(),
                format!("{name}:"),
                print::ir::ty(&field.value.tracked, mint).to_string(),
            )
            .at(field.name_span)
            .child(type_node(ids, cx, mint, &field.value))
        })),
    }
}

fn with_symbol(node: Node, cx: &Cx, symbol: Symbol) -> Node {
    match cx.symbols.get(&symbol) {
        Some(index) => node.symbol(*index),
        None => node,
    }
}
