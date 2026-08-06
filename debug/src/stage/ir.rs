//! The lowered program.
//!
//! Every node that names a symbol carries its index, which is what lets a click
//! on one occurrence highlight the binder and every other use of the same
//! symbol across all panels.

use ruddy::{
    inference,
    ir::{Decl, Row, Tail, Term, TermKind, Type, TypeKind},
    symbol::{Mint, Symbol},
    types::Ty,
};

use crate::{
    print,
    stage::{Cx, Ids, Spec, Trace, plural, stands_for},
    wire::{Node, Stage},
};

/// The rows, and the [`Trace`] the stage annotating them reads. Both fall out
/// of one walk: nothing else knows what shape this tree has, so nothing else
/// can be wrong about it.
pub fn build(spec: &Spec, cx: &Cx) -> (Stage, Trace) {
    let mut trace = Trace::default();
    let (Some(program), Some(mint)) = (cx.program, cx.mint) else {
        return (crate::stage::skipped(spec, "lowering did not run"), trace);
    };

    let mut ids = Ids::default();
    let mut nodes = Vec::new();

    // Types first, then terms — the order `Program`'s own printer uses.
    for (symbol, decl) in &program.types {
        let node = decl_node(&mut ids, cx, mint, "type", *symbol, decl, type_node);
        trace.decls.push(node.id);
        nodes.push(node);
    }
    for (symbol, decl) in &program.terms {
        let node = decl_node(
            &mut ids,
            cx,
            mint,
            "let",
            *symbol,
            decl,
            |ids, cx, mint, term| term_node(ids, cx, mint, term, &mut trace),
        );
        trace.decls.push(node.id);
        nodes.push(node);
    }

    let summary = format!(
        "{} · {}",
        plural(program.types.len(), "type"),
        plural(program.terms.len(), "term")
    );
    let stage = Stage {
        micros: Some(cx.micros.build),
        nodes,
        debug: format!("{program:#?}"),
        ..spec.stage(cx.status(), summary)
    };
    (stage, trace)
}

fn decl_node<T>(
    ids: &mut Ids,
    cx: &Cx,
    mint: &Mint,
    keyword: &str,
    symbol: Symbol,
    decl: &Decl<T>,
    value: impl FnOnce(&mut Ids, &Cx, &Mint, &T) -> Node,
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
    // The binders, before the body that uses them, which is where they are
    // written and how the AST panel already shows them. A parameter is a local
    // symbol like a lambda's argument, so it cross-highlights with every
    // `Param` row in the body and with the AST's own — and without a row here
    // the IR tab was the one panel where clicking one lit nothing up. The text
    // says what it stands for, since a `type` declaration's binder is the only
    // binder in the language that stands for anything but a type.
    for param in &decl.params {
        node = node.child(with_symbol(
            Node::new(ids.next(), "Param", stands_for(mint, param)).at(param.span),
            cx,
            param.symbol,
        ));
    }
    node.child(value(ids, cx, mint, &decl.value))
}

/// One row per term, and one trace entry alongside it: the id this row was
/// given, and the type of the term it stands for. The stage that badges the IR
/// reads those pairs rather than walking the tree a second time.
fn term_node(ids: &mut Ids, cx: &Cx, mint: &Mint, term: &Term, trace: &mut Trace) -> Node {
    let node = Node::new(
        ids.next(),
        "",
        print::ir::term(&term.kind, mint).to_string(),
    )
    .at(term.span);
    trace.terms.push((node.id, term.ty.clone()));
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
        .child(term_node(ids, cx, mint, func, trace))
        .child(term_node(ids, cx, mint, arg, trace)),
        TermKind::Fn { arg, body } => {
            let bound = with_symbol(
                Node::new(ids.next(), "Arg", mint.name(arg.tracked)).at(arg.span),
                cx,
                arg.tracked,
            );
            // The bound name has no term of its own to carry a type, but the
            // lambda's arrow knows it — through a declared type if that is
            // what the annotation was, since a lambda checked against `Endo`
            // has an argument as surely as one checked against `Nat -> Nat`.
            let shape = cx
                .inference
                .map(|inferred| inference::unfold(&inferred.aliases, &term.ty));
            if let Some(Ty::Arrow(from, _)) = shape.as_deref() {
                trace.terms.push((bound.id, from.clone()));
            }
            Node {
                label: "Fn".into(),
                ..node
            }
            .child(bound)
            .child(term_node(ids, cx, mint, body, trace))
        }
        // The case names no symbol either, for the reason the field below
        // does not: it is a label scoped to whichever sum it turns out to be a
        // case of. A tag carrying nothing is a leaf.
        TermKind::Tag { name, payload } => {
            let node = Node {
                label: "Tag".into(),
                ..node
            }
            .at(name.span);
            match payload {
                Some(payload) => node.child(term_node(ids, cx, mint, payload, trace)),
                None => node,
            }
        }
        // The field names no symbol, so its node is a plain leaf — the one
        // place in the term tree where an identifier-looking thing is not one.
        TermKind::Project { base, field } => Node {
            label: "Project".into(),
            ..node
        }
        .child(term_node(ids, cx, mint, base, trace))
        .child(Node::new(ids.next(), "Field", field.tracked.clone()).at(field.span)),
        TermKind::Struct(fields) => {
            // Built eagerly rather than through `children`: the closure a lazy
            // iterator would need borrows the trace for as long as it lives.
            let wrappers: Vec<Node> = fields
                .iter()
                .map(|(name, field)| {
                    Node::new(
                        ids.next(),
                        format!("{name}:"),
                        print::ir::term(&field.value.kind, mint).to_string(),
                    )
                    .at(field.name_span)
                    .child(term_node(ids, cx, mint, &field.value, trace))
                })
                .collect();
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(wrappers)
        }
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
        // A parameter is a local symbol like a lambda's argument, so it
        // cross-highlights against the head of its own declaration.
        TypeKind::Param { symbol, .. } => with_symbol(
            Node {
                label: "Param".into(),
                ..node
            },
            cx,
            *symbol,
        ),
        TypeKind::Apply {
            head,
            head_span,
            args,
        } => {
            let head = with_symbol(
                Node::new(ids.next(), "Head", mint.name(*head).to_string()).at(*head_span),
                cx,
                *head,
            );
            Node {
                label: "Apply".into(),
                ..node
            }
            .child(head)
            .children(
                args.iter()
                    .map(|arg| type_node(ids, cx, mint, arg))
                    .collect::<Vec<_>>(),
            )
        }
        // A primitive is resolved from its spelling rather than from the name
        // table, so there is no symbol to cross-highlight it by.
        TypeKind::Prim(_) => Node {
            label: "Prim".into(),
            ..node
        },
        // The struct's row again about cases; see the arm below for what the
        // `Rest` child is.
        TypeKind::Sum { cases, tail } => {
            let mut kids: Vec<Node> = cases
                .iter()
                .map(|(name, case)| {
                    let mark = if case.optional { "?" } else { "" };
                    let text = case.payload.as_ref().map_or(String::new(), |ty| {
                        print::ir::ty(&ty.tracked, mint).to_string()
                    });
                    let node =
                        Node::new(ids.next(), format!("`{name}{mark}"), text).at(case.name_span);
                    match &case.payload {
                        Some(payload) => node.child(type_node(ids, cx, mint, payload)),
                        None => node,
                    }
                })
                .collect();
            if let Some(tail) = tail {
                kids.push(rest_node(ids, mint, tail));
            }
            Node {
                label: "Sum".into(),
                ..node
            }
            .children(kids)
        }
        TypeKind::Arrow { from, to } => Node {
            label: "Arrow".into(),
            ..node
        }
        .child(type_node(ids, cx, mint, from))
        .child(type_node(ids, cx, mint, to)),
        TypeKind::Struct { fields, tail } => {
            let mut kids: Vec<Node> = fields
                .iter()
                .map(|(name, field)| {
                    let mark = if field.optional { "?" } else { "" };
                    Node::new(
                        ids.next(),
                        format!("{name}{mark}:"),
                        print::ir::ty(&field.value.tracked, mint).to_string(),
                    )
                    .at(field.name_span)
                    .child(type_node(ids, cx, mint, &field.value))
                })
                .collect();
            // The tail is a row of its own: it stands for the labels not
            // named, so it is shown beside them rather than folded into one.
            if let Some(tail) = tail {
                kids.push(rest_node(ids, mint, tail));
            }
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(kids)
        }
    }
}

/// The `..` tail of either shape of row, as the row of its own it is shown as.
/// A row parameter prints as the name it was declared with, for the reason a
/// type parameter does.
fn rest_node(ids: &mut Ids, mint: &Mint, tail: &Tail) -> Node {
    let name = match &tail.of {
        Row::Anything => String::new(),
        Row::Named(name) => name.clone(),
        Row::Param { symbol, .. } => mint.name(*symbol).to_string(),
    };
    Node::new(ids.next(), "Rest", format!("..{name}")).at(tail.span)
}

fn with_symbol(node: Node, cx: &Cx, symbol: Symbol) -> Node {
    match cx.symbols.get(&symbol) {
        Some(index) => node.symbol(*index),
        None => node,
    }
}
