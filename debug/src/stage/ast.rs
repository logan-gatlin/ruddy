//! The parse tree.
//!
//! Each node's `text` is that subtree's own `Display` output, so the tree and
//! the rendered source can never drift apart: they are the same printer.

use ruddy::parse::{
    Annotation, ArgKind, ArmHead, Clause, ClauseKind, EffectCase, EffectLabel, EffectRow, Expr,
    ExprKind, Pattern, PatternKind, Stmt, StmtKind, SumCase, Type, TypeField, TypeKind, When,
};

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

    let (lets, types, effects) =
        stmts
            .iter()
            .fold((0, 0, 0), |(l, t, e), stmt| match stmt.tracked {
                StmtKind::Let { .. } => (l + 1, t, e),
                StmtKind::Type { .. } => (l, t + 1, e),
                StmtKind::Effect { .. } => (l, t, e + 1),
            });

    Stage {
        micros: Some(cx.micros.parse),
        nodes,
        debug: format!("{stmts:#?}"),
        ..spec.stage(
            cx.status(),
            format!("{effects} effect · {types} type · {lets} let"),
        )
    }
}

fn stmt_node(ids: &mut Ids, stmt: &Stmt) -> Node {
    let node = Node::new(ids.next(), "", print::ast::stmt(&stmt.tracked).to_string()).at(stmt.span);
    match &stmt.tracked {
        StmtKind::Let { pattern, ty, body } => {
            let mut let_node = Node {
                label: "Let".into(),
                ..node
            }
            .child(pattern_node(ids, pattern));
            // The ascription sits between the name and the body, where it was
            // written; a `let` without one simply has one child fewer. Its role
            // goes on the type's own node rather than on a wrapper around it: a
            // wrapper would carry the same text and the same span one row up.
            if let Some(ty) = ty {
                let mut ascribed = annotation_node(ids, ty);
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
            type_node_.child(annotation_node(ids, body))
        }
        // The name, then one row per case: an operation with its signature
        // under it, and an alias's case as the leaf it is. The empty effect
        // has the name and nothing else.
        StmtKind::Effect { name, cases } => {
            let mut effect_node = Node {
                label: "Effect".into(),
                ..node
            }
            .child(Node::new(ids.next(), "Name", name.tracked.clone()).at(name.span));
            for (op, case) in cases {
                let row = match case {
                    EffectCase::Operation { signature } => Node::new(
                        ids.next(),
                        format!("`{}:", op.tracked),
                        print::ast::ty(&signature.tracked).to_string(),
                    )
                    .at(op.span)
                    .child(type_node(ids, signature)),
                    EffectCase::Alias => {
                        Node::new(ids.next(), "Names", format!("`{}", op.tracked)).at(op.span)
                    }
                };
                effect_node = effect_node.child(row);
            }
            effect_node
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
        // A `_` argument's row reads as the `_` it was written as; it binds
        // nothing, so there is no name for the row to carry instead.
        ExprKind::Function { args, body } => Node {
            label: "Function".into(),
            ..node
        }
        .children(args.iter().map(|arg| {
            let text = match &arg.tracked {
                ArgKind::Name(name) => name.clone(),
                ArgKind::Wildcard => "_".to_string(),
            };
            Node::new(ids.next(), "Arg", text).at(arg.span)
        }))
        .child(expr_node(ids, body)),
        // The statement's row again, about the expression: the name it binds,
        // the type it was ascribed, the value and the body. Labelled with the
        // name, since that is what a reader scanning the tree is looking for.
        ExprKind::Let {
            pattern,
            ty,
            value,
            body,
        } => {
            let mut let_node = Node {
                label: format!("Let {}", pattern.tracked),
                ..node
            }
            .child(pattern_node(ids, pattern));
            if let Some(ty) = ty {
                let mut ascribed = annotation_node(ids, ty);
                ascribed.label = format!("Ascribed {}", ascribed.label);
                let_node = let_node.child(ascribed);
            }
            let_node
                .child(expr_node(ids, value))
                .child(expr_node(ids, body))
        }
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
        // The scrutinee first, then one wrapper per arm holding the arm's
        // pattern and body — the shape the reader wrote, before flattening.
        ExprKind::Match { scrutinee, arms } => {
            let mut match_node = Node {
                label: "Match".into(),
                ..node
            }
            .child(expr_node(ids, scrutinee));
            for arm in &arms[..] {
                let span = arm.pattern.span.merge(arm.body.span);
                let arm_node = Node::new(ids.next(), "Arm", format!("{}", arm.pattern.tracked))
                    .at(span)
                    .child(pattern_node(ids, &arm.pattern))
                    .child(expr_node(ids, &arm.body));
                match_node = match_node.child(arm_node);
            }
            match_node
        }
        // The handled expression first, then one wrapper per arm holding what
        // it answers, the name it binds and its body — the match's shape,
        // about operations instead of patterns.
        ExprKind::Handle { body, arms } => {
            let mut handle_node = Node {
                label: "Handle".into(),
                ..node
            }
            .child(expr_node(ids, body));
            for arm in arms {
                let (label, at) = match &arm.head {
                    ArmHead::Operation { effect, op } => (
                        format!("{}.`{}", effect.tracked, op.tracked),
                        effect.span.merge(op.span),
                    ),
                    ArmHead::Return { span } => ("return".to_string(), *span),
                };
                let binder = match &arm.binder.tracked {
                    ArgKind::Name(name) => name.clone(),
                    ArgKind::Wildcard => "_".to_string(),
                };
                let arm_node = Node::new(ids.next(), "Arm", label)
                    .at(at.merge(arm.body.span))
                    .child(Node::new(ids.next(), "Binder", binder).at(arm.binder.span))
                    .child(expr_node(ids, &arm.body));
                handle_node = handle_node.child(arm_node);
            }
            handle_node
        }
        ExprKind::Raise(value) => Node {
            label: "Raise".into(),
            ..node
        }
        .child(expr_node(ids, value)),
        // The effect is a name and the operation a label scoped to it, so the
        // row carries the span the two were written at and no symbol — the
        // parse tree resolves nothing.
        ExprKind::Operation { effect, op } => Node {
            label: "Operation".into(),
            ..node
        }
        .at(effect.span.merge(op.span)),
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

/// One written pattern, as a tree: each node's text is the pattern's own
/// `Display`, the same rule the expression nodes keep. A pun is a leaf wearing
/// the field's name; a renamed field holds its sub-pattern.
fn pattern_node(ids: &mut Ids, pattern: &Pattern) -> Node {
    let node = Node::new(ids.next(), "", pattern.tracked.to_string()).at(pattern.span);
    match &pattern.tracked {
        PatternKind::Ident { name } => Node {
            label: "Bind".into(),
            ..node
        }
        .at(name.span),
        // A leaf like a binder, minus the symbol a binder would carry: there
        // is nothing to cross-highlight, because nothing can use it.
        PatternKind::Wildcard => Node {
            label: "Wildcard".into(),
            ..node
        },
        PatternKind::Natural(_) => Node {
            label: "Natural".into(),
            ..node
        },
        PatternKind::Unit => Node {
            label: "Unit".into(),
            ..node
        },
        PatternKind::Tag { name, payload } => {
            let tag_node = Node {
                label: "Tag".into(),
                ..node
            }
            .at(name.span);
            match payload {
                Some(payload) => tag_node.child(pattern_node(ids, payload)),
                None => tag_node,
            }
        }
        PatternKind::Struct { fields, rest } => {
            let mut kids: Vec<Node> = fields
                .iter()
                .map(|(name, sub)| match sub {
                    Some(sub) => Node::new(
                        ids.next(),
                        format!("{}:", name.tracked),
                        sub.tracked.to_string(),
                    )
                    .at(name.span)
                    .child(pattern_node(ids, sub)),
                    None => Node::new(ids.next(), "Bind", name.tracked.clone()).at(name.span),
                })
                .collect();
            // The `..` that makes the pattern open is a row of its own, the
            // way a struct type's tail is shown: it stands for the fields not
            // named, so it sits beside them rather than folding into one.
            if let Some(rest) = rest {
                kids.push(Node::new(ids.next(), "Rest", "..").at(*rest));
            }
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(kids)
        }
    }
}

/// The `when` clause a label wears, as the row's label shows it: nothing when
/// the label is simply there, and the clause as written otherwise.
fn when_text(when: &Option<Box<When>>) -> String {
    match when {
        None => String::new(),
        Some(when) => match &when.name {
            Some(name) => format!(" when {}", name.tracked),
            None => " when _".to_string(),
        },
    }
}

/// One written ascription: the type, and — when one was written — the `where`
/// clause beside it, as a row of its own.
///
/// The clause is a sibling of the type rather than a child, because that is
/// what it is: it constrains the presences the type's `when`s bound, and
/// burying it under the type would say it was part of one.
fn annotation_node(ids: &mut Ids, annotation: &Annotation) -> Node {
    let node = type_node(ids, &annotation.ty);
    match &annotation.clause {
        Some(clause) => {
            let row = Node::new(ids.next(), "Where", clause_text(clause)).at(clause.span);
            node.child(row.children(clause_kids(ids, clause)))
        }
        None => node,
    }
}

/// One clause as the source it was written from — through the shared printer,
/// so the row and the type's own text cannot spell one clause two ways.
fn clause_text(clause: &Clause) -> String {
    print::ast::clause(&clause.tracked).to_string()
}

/// The parts of a clause, each a row of its own, so a reader can cross-
/// highlight the name a formula uses against the `when` that bound it.
fn clause_kids(ids: &mut Ids, clause: &Clause) -> Vec<Node> {
    let parts: Vec<&Clause> = match &clause.tracked {
        ClauseKind::Name(_) => Vec::new(),
        ClauseKind::Not(inner) => vec![inner],
        ClauseKind::And(left, right)
        | ClauseKind::Or(left, right)
        | ClauseKind::Equal(left, right)
        | ClauseKind::NotEqual(left, right) => vec![left, right],
    };
    parts
        .into_iter()
        .map(|part| {
            let row = Node::new(ids.next(), "", clause_text(part)).at(part.span);
            row.children(clause_kids(ids, part))
        })
        .collect()
}

/// The `! <effects>` clause an arrow carries, as a row of its own: one child
/// per effect it names, and the `..` tail beside them.
fn effects_node(ids: &mut Ids, effects: &EffectRow) -> Node {
    let mut kids: Vec<Node> = effects
        .effects
        .iter()
        .map(|(name, label)| match label {
            EffectLabel::Written { when } => {
                let mark = when_text(when);
                Node::new(
                    ids.next(),
                    format!("`{}{mark}", name.tracked),
                    String::new(),
                )
                .at(name.span)
            }
            // The absent effect is a leaf wearing the `\`, spanning the whole
            // `` \`Name `` — the struct's absent field again.
            EffectLabel::Absent => {
                Node::new(ids.next(), format!("\\`{}", name.tracked), String::new()).at(name.span)
            }
        })
        .collect();
    if let Some(tail) = &effects.tail {
        let name = tail.name.as_ref().map_or("", |name| name.tracked.as_str());
        kids.push(Node::new(ids.next(), "Rest", format!("..{name}")).at(tail.span));
    }
    Node::new(ids.next(), "Effects", String::new())
        .at(effects.span)
        .children(kids)
}

fn type_node(ids: &mut Ids, ty: &Type) -> Node {
    let node = Node::new(ids.next(), "", print::ast::ty(&ty.tracked).to_string()).at(ty.span);
    match &ty.tracked {
        TypeKind::Struct { fields, tail } => {
            let mut kids: Vec<Node> = fields
                .iter()
                .map(|(name, field)| match field {
                    TypeField::Written { when, value } => {
                        let mark = when_text(when);
                        Node::new(
                            ids.next(),
                            format!("{}{mark}:", name.tracked),
                            print::ast::ty(&value.tracked).to_string(),
                        )
                        .at(name.span)
                        .child(type_node(ids, value))
                    }
                    // An absent field is a leaf: there is no type under it,
                    // and the key's span covers the whole `\name`.
                    TypeField::Absent => {
                        Node::new(ids.next(), format!("\\{}", name.tracked), String::new())
                            .at(name.span)
                    }
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
                .map(|(name, case)| match case {
                    SumCase::Written { when, payload } => {
                        let mark = when_text(when);
                        let text = payload
                            .as_ref()
                            .map_or(String::new(), |ty| print::ast::ty(&ty.tracked).to_string());
                        let node = Node::new(ids.next(), format!("`{}{mark}", name.tracked), text)
                            .at(name.span);
                        match payload {
                            Some(payload) => node.child(type_node(ids, payload)),
                            None => node,
                        }
                    }
                    // The struct's absent field again: a leaf wearing the `\`,
                    // spanning the whole `` \`Name ``.
                    SumCase::Absent => {
                        Node::new(ids.next(), format!("\\`{}", name.tracked), String::new())
                            .at(name.span)
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
        TypeKind::Arrow { from, to, effects } => {
            let arrow = Node {
                label: "Arrow".into(),
                ..node
            }
            .child(type_node(ids, from))
            .child(type_node(ids, to));
            // The row is a child of its own, beside the two sides: it is the
            // third thing the arrow says, and folding it into the result would
            // put it on the wrong arrow.
            match effects {
                Some(effects) => arrow.child(effects_node(ids, effects)),
                None => arrow,
            }
        }
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
