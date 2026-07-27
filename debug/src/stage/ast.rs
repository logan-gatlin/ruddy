//! The parse tree.
//!
//! Each node's `text` is that subtree's own `Display` output, so the tree and
//! the rendered source can never drift apart: they are the same printer.

use ruddy::parse::{Expr, ExprKind, Stmt, StmtKind, Type, TypeKind};

use crate::{
    stage::{Cx, Ids},
    wire::{Node, Stage, View},
};

pub fn build(cx: &Cx) -> Stage {
    let Some(stmts) = cx.stmts else {
        return crate::stage::skipped("ast", "AST", View::Tree, "parsing did not run");
    };

    let mut ids = Ids::default();
    let nodes: Vec<Node> = stmts.iter().map(|stmt| stmt_node(&mut ids, stmt)).collect();

    let display = stmts
        .iter()
        .map(|stmt| stmt.tracked.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let (lets, types) = stmts
        .iter()
        .fold((0, 0), |(l, t), stmt| match stmt.tracked {
            StmtKind::Let { .. } => (l + 1, t),
            StmtKind::Type { .. } => (l, t + 1),
        });

    Stage {
        id: "ast",
        title: "AST",
        view: View::Tree,
        status: cx.status(),
        summary: format!("{types} type · {lets} let"),
        micros: cx.micros.parse,
        nodes,
        text: None,
        display,
        debug: format!("{stmts:#?}"),
        annotates: None,
    }
}

fn stmt_node(ids: &mut Ids, stmt: &Stmt) -> Node {
    let node = Node::new(ids.next(), "", stmt.tracked.to_string()).at(stmt.span);
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
        StmtKind::Type { name, body } => Node {
            label: "Type".into(),
            ..node
        }
        .child(Node::new(ids.next(), "Name", name.tracked.clone()).at(name.span))
        .child(type_node(ids, body)),
    }
}

fn expr_node(ids: &mut Ids, expr: &Expr) -> Node {
    let node = Node::new(ids.next(), "", expr.tracked.to_string()).at(expr.span);
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
                value.tracked.to_string(),
            )
            .at(name.span)
            .child(expr_node(ids, value))
        })),
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
    let node = Node::new(ids.next(), "", ty.tracked.to_string()).at(ty.span);
    match &ty.tracked {
        TypeKind::Apply { func, arg } => Node {
            label: "Apply".into(),
            ..node
        }
        .child(type_node(ids, func))
        .child(type_node(ids, arg)),
        TypeKind::Lambda { args, body } => Node {
            label: "Function".into(),
            ..node
        }
        .children(
            args.iter()
                .map(|arg| Node::new(ids.next(), "Arg", arg.tracked.clone()).at(arg.span)),
        )
        .child(type_node(ids, body)),
        TypeKind::Struct(fields) => Node {
            label: "Struct".into(),
            ..node
        }
        .children(fields.iter().map(|(name, value)| {
            Node::new(
                ids.next(),
                format!("{}:", name.tracked),
                value.tracked.to_string(),
            )
            .at(name.span)
            .child(type_node(ids, value))
        })),
        TypeKind::Arrow { from, to } => Node {
            label: "Arrow".into(),
            ..node
        }
        .child(type_node(ids, from))
        .child(type_node(ids, to)),
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
