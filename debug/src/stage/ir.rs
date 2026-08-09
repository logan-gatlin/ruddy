//! The lowered program.
//!
//! Every node that names a symbol carries its index, which is what lets a click
//! on one occurrence highlight the binder and every other use of the same
//! symbol across all panels.

use ruddy::{
    inference,
    ir::{
        Decl, Pattern, PatternKind, Row, SumCase, Tail, Term, TermKind, Type, TypeField, TypeKind,
    },
    symbol::{Mint, Symbol},
    types::Core,
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

    // The groups are counted beside the two tables because they are the third
    // thing lowering decided: how many separate schemes the terms will turn
    // into, which is the whole difference between a file of definitions and one
    // definition written many times.
    let summary = format!(
        "{} · {} · {}",
        plural(program.types.len(), "type"),
        plural(program.terms.len(), "term"),
        plural(program.groups.len(), "group")
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
            if let Some(Core::Arrow(from, _)) = shape.as_deref().map(|ty| &ty.core) {
                trace.terms.push((bound.id, from.clone()));
            }
            Node {
                label: "Fn".into(),
                ..node
            }
            .child(bound)
            .child(term_node(ids, cx, mint, body, trace))
        }
        // The declaration's row again, about the expression. The name is a
        // local symbol like a lambda's argument, so it cross-highlights with
        // every use of it in the value and the body — and with the AST tab's
        // own row for the same binding.
        TermKind::Let {
            name,
            annotation,
            value,
            body,
        } => {
            let mut node = Node {
                label: format!("Let {}", mint.name(name.tracked)),
                ..node
            }
            .child(with_symbol(
                Node::new(ids.next(), "Name", mint.name(name.tracked)).at(name.span),
                cx,
                name.tracked,
            ));
            if let Some(annotation) = annotation {
                let mut ascribed = type_node(ids, cx, mint, annotation);
                ascribed.label = format!("Ascribed {}", ascribed.label);
                node = node.child(ascribed);
            }
            node.child(term_node(ids, cx, mint, value, trace))
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
        // The scrutinee, then each written arm: its normalized pattern — the
        // binders symbols like a lambda's argument, so they cross-highlight
        // with their uses — and its body. Reading this beside the AST tab's
        // match shows the normalization: puns expanded, symbols minted, and
        // nothing else changed, because nothing else is.
        TermKind::Match { scrutinee, arms } => {
            let mut match_node = Node {
                label: "Match".into(),
                ..node
            }
            .child(term_node(ids, cx, mint, scrutinee, trace));
            for (pattern, body) in arms {
                match_node = match_node
                    .child(pattern_node(ids, cx, mint, pattern))
                    .child(term_node(ids, cx, mint, body, trace));
            }
            match_node
        }
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

/// One normalized pattern as a row per node, mirroring the AST tab's pattern
/// wrappers: the text is the pattern printed as source, and a binder carries
/// its symbol so it cross-highlights with every use in the arm's body.
fn pattern_node(ids: &mut Ids, cx: &Cx, mint: &Mint, pattern: &Pattern) -> Node {
    let node = Node::new(
        ids.next(),
        "",
        print::ir::pattern(&pattern.tracked, mint).to_string(),
    )
    .at(pattern.span);
    match &pattern.tracked {
        PatternKind::Bind(name) => with_symbol(
            Node {
                label: "Bind".into(),
                ..node
            },
            cx,
            name.tracked,
        ),
        PatternKind::Natural(_) => Node {
            label: "Natural".into(),
            ..node
        },
        PatternKind::Unit => Node {
            label: "Unit".into(),
            ..node
        },
        // A bare tag is a leaf: the unit it constrains its payload to is the
        // type's business, and nothing here was written to show.
        PatternKind::Tag { payload, .. } => {
            let node = Node {
                label: "Tag".into(),
                ..node
            };
            match payload {
                Some(payload) => node.child(pattern_node(ids, cx, mint, payload)),
                None => node,
            }
        }
        PatternKind::Struct(fields) => {
            let kids: Vec<Node> = fields
                .iter()
                .map(|(name, field)| {
                    Node::new(
                        ids.next(),
                        format!("{name}:"),
                        print::ir::pattern(&field.value.tracked, mint).to_string(),
                    )
                    .at(field.name_span)
                    .child(pattern_node(ids, cx, mint, &field.value))
                })
                .collect();
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(kids)
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
                .map(|(name, case)| match case {
                    SumCase::Written {
                        name_span,
                        optional,
                        payload,
                    } => {
                        let mark = if *optional { "?" } else { "" };
                        let text = payload.as_ref().map_or(String::new(), |ty| {
                            print::ir::ty(&ty.tracked, mint).to_string()
                        });
                        let node =
                            Node::new(ids.next(), format!("`{name}{mark}"), text).at(*name_span);
                        match payload {
                            Some(payload) => node.child(type_node(ids, cx, mint, payload)),
                            None => node,
                        }
                    }
                    // An absent case is a leaf wearing the `\`, spanning the
                    // whole `` \`Name `` for cross-highlighting.
                    SumCase::Absent { name_span } => {
                        Node::new(ids.next(), format!("\\`{name}"), String::new()).at(*name_span)
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
                .map(|(name, field)| match field {
                    TypeField::Written {
                        name_span,
                        optional,
                        value,
                    } => {
                        let mark = if *optional { "?" } else { "" };
                        Node::new(
                            ids.next(),
                            format!("{name}{mark}:"),
                            print::ir::ty(&value.tracked, mint).to_string(),
                        )
                        .at(*name_span)
                        .child(type_node(ids, cx, mint, value))
                    }
                    // An absent field is a leaf: there is no type under it,
                    // and the span covers the whole `\name`.
                    TypeField::Absent { name_span } => {
                        Node::new(ids.next(), format!("\\{name}"), String::new()).at(*name_span)
                    }
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
