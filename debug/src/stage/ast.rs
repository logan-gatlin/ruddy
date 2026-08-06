//! The parse tree.
//!
//! Each node's `text` is that subtree's own `Display` output, so the tree and
//! the rendered source can never drift apart: they are the same printer.

use ruddy::parse::{Expr, ExprKind, Stmt, StmtKind, Type, TypeKind};

use crate::{
    print,
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(stmts) = cx.stmts else {
        return crate::stage::skipped(spec, "parsing did not run");
    };

    let mut ids = Ids::default();
    let nodes: Vec<Node> = stmts.iter().map(|stmt| stmt_node(&mut ids, stmt)).collect();

    let (lets, types) = stmts
        .iter()
        .fold((0, 0), |(l, t), stmt| match stmt.tracked {
            StmtKind::Let { .. } => (l + 1, t),
            StmtKind::Type { .. } => (l, t + 1),
        });

    Stage {
        micros: Some(cx.micros.parse),
        nodes,
        debug: format!("{stmts:#?}"),
        ..spec.stage(cx.status(), format!("{types} type · {lets} let"))
    }
}

fn stmt_node(ids: &mut Ids, stmt: &Stmt) -> Node {
    let node = Node::new(ids.next(), "", print::ast::stmt(&stmt.tracked).to_string()).at(stmt.span);
    match &stmt.tracked {
        StmtKind::Let { name, ty, body } => {
            let mut let_node = Node {
                label: "Let".into(),
                ..node
            }
            .child(Node::new(ids.next(), "Name", name.tracked.clone()).at(name.span));
            // The ascription sits between the name and the body, where it was
            // written; a `let` without one simply has one child fewer. Its role
            // goes on the type's own node rather than on a wrapper around it: a
            // wrapper would carry the same text and the same span one row up.
            if let Some(ty) = ty {
                let mut ascribed = type_node(ids, ty);
                ascribed.label = format!("Ascribed {}", ascribed.label);
                let_node = let_node.child(ascribed);
            }
            // `body` is a `Tracked<Expr>` wrapped around an `Expr` that is
            // itself `Tracked`, hence the doubled hop to reach the node.
            let_node.child(expr_node(ids, &body.tracked))
        }
        StmtKind::Type { name, params, body } => {
            let mut type_node_ = Node {
                label: "Type".into(),
                ..node
            }
            .child(Node::new(ids.next(), "Name", name.tracked.clone()).at(name.span));
            for param in params {
                type_node_ = type_node_
                    .child(Node::new(ids.next(), "Param", param.tracked.clone()).at(param.span));
            }
            type_node_.child(type_node(ids, body))
        }
    }
}

fn expr_node(ids: &mut Ids, expr: &Expr) -> Node {
    let node = Node::new(ids.next(), "", print::ast::expr(&expr.tracked).to_string()).at(expr.span);
    match &expr.tracked {
        ExprKind::Apply { func, arg } => Node {
            label: "Apply".into(),
            ..node
        }
        .child(expr_node(ids, func))
        .child(expr_node(ids, arg)),
        ExprKind::Function { args, body } => Node {
            label: "Function".into(),
            ..node
        }
        .children(
            args.iter()
                .map(|arg| Node::new(ids.next(), "Arg", arg.tracked.clone()).at(arg.span)),
        )
        .child(expr_node(ids, body)),
        ExprKind::Struct(fields) => Node {
            label: "Struct".into(),
            ..node
        }
        .children(fields.iter().map(|(name, value)| {
            Node::new(
                ids.next(),
                format!("{}:", name.tracked),
                print::ast::expr(&value.tracked).to_string(),
            )
            .at(name.span)
            .child(expr_node(ids, value))
        })),
        // The case is a label rather than a name, so the node carries the span
        // it was written at and no symbol — the same as a field, and for the
        // same reason. A tag carrying nothing is a leaf.
        ExprKind::Tag { name, payload } => {
            let node = Node {
                label: "Tag".into(),
                ..node
            }
            .at(name.span);
            match payload {
                Some(payload) => node.child(expr_node(ids, payload)),
                None => node,
            }
        }
        // The field is a label rather than a name, so its node carries the span
        // it was written at and no symbol.
        ExprKind::Project { base, field } => Node {
            label: "Project".into(),
            ..node
        }
        .child(expr_node(ids, base))
        .child(Node::new(ids.next(), "Field", field.tracked.clone()).at(field.span)),
        ExprKind::Ident { name } => Node {
            label: "Ident".into(),
            ..node
        }
        .at(name.span),
        ExprKind::Natural(_) => Node {
            label: "Natural".into(),
            ..node
        },
        ExprKind::Unit => Node {
            label: "Unit".into(),
            ..node
        },
    }
}

fn type_node(ids: &mut Ids, ty: &Type) -> Node {
    let node = Node::new(ids.next(), "", print::ast::ty(&ty.tracked).to_string()).at(ty.span);
    match &ty.tracked {
        TypeKind::Struct { fields, tail } => {
            let mut kids: Vec<Node> = fields
                .iter()
                .map(|(name, field)| {
                    let mark = if field.optional { "?" } else { "" };
                    Node::new(
                        ids.next(),
                        format!("{}{mark}:", name.tracked),
                        print::ast::ty(&field.value.tracked).to_string(),
                    )
                    .at(name.span)
                    .child(type_node(ids, &field.value))
                })
                .collect();
            // The tail is a row of its own: it stands for the fields not
            // named, so it is shown beside them rather than folded into one.
            if let Some(tail) = tail {
                let name = tail.name.as_ref().map_or("", |name| name.tracked.as_str());
                kids.push(Node::new(ids.next(), "Rest", format!("..{name}")).at(tail.span));
            }
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(kids)
        }
        // The struct's row shown again about cases: one child per case, the
        // payload under it, and the tail beside them.
        TypeKind::Sum { cases, tail } => {
            let mut kids: Vec<Node> = cases
                .iter()
                .map(|(name, case)| {
                    let mark = if case.optional { "?" } else { "" };
                    let text = case
                        .payload
                        .as_ref()
                        .map_or(String::new(), |ty| print::ast::ty(&ty.tracked).to_string());
                    let node = Node::new(ids.next(), format!("`{}{mark}", name.tracked), text)
                        .at(name.span);
                    match &case.payload {
                        Some(payload) => node.child(type_node(ids, payload)),
                        None => node,
                    }
                })
                .collect();
            if let Some(tail) = tail {
                let name = tail.name.as_ref().map_or("", |name| name.tracked.as_str());
                kids.push(Node::new(ids.next(), "Rest", format!("..{name}")).at(tail.span));
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
        .child(type_node(ids, from))
        .child(type_node(ids, to)),
        TypeKind::Apply { head, args } => Node {
            label: "Apply".into(),
            ..node
        }
        .child(type_node(ids, head))
        .children(
            args.iter()
                .map(|arg| type_node(ids, arg))
                .collect::<Vec<_>>(),
        ),
        TypeKind::Ident { name } => Node {
            label: "Ident".into(),
            ..node
        }
        .at(name.span),
        TypeKind::Unit => Node {
            label: "Unit".into(),
            ..node
        },
    }
}
