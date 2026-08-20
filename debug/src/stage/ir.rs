//! The lowered program.
//!
//! Every node that names a symbol carries its index, which is what lets a click
//! on one occurrence highlight the binder and every other use of the same
//! symbol across all panels.

use ruddy::{
    inference,
    ir::{
        Annotation, Clause, ClauseKind, Decl, Effect, EffectLabel, EffectRow, Handler, Pattern,
        PatternKind, Row, SumCase, Tail, Term, TermKind, Type, TypeField, TypeKind, Variable, When,
    },
    symbol::{Mint, Symbol},
    tracking::Span,
    types::{Core, Sense, Shape},
};

use crate::{
    print,
    stage::{Cx, Ids, Spec, Trace, plural, stands_for, with_symbol},
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

    // Effects first, then types, then terms — the order `Program`'s own
    // printer uses, which is the order lowering read them in.
    for (symbol, decl) in &program.effects {
        let node = decl_node(&mut ids, cx, mint, "effect", *symbol, decl, effect_node);
        trace.decls.push(node.id);
        nodes.push(node);
    }
    for (symbol, decl) in &program.types {
        // A declaration body declares no variables, so nothing in it links to
        // one: its own parameters are symbols, and cross-highlight as those.
        let node = decl_node(
            &mut ids,
            cx,
            mint,
            "type",
            *symbol,
            decl,
            |ids, cx, mint, ty| type_node(ids, cx, mint, ty, &[]),
        );
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
        "{} · {} · {} · {}",
        plural(program.effects.len(), "effect"),
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
    let mut node = with_symbol(
        Node::new(
            ids.next(),
            format!("{keyword} {}", mint.name(symbol)),
            mint.path(symbol).to_string(),
        )
        .at(decl.name_span),
        cx,
        mint,
        symbol,
    );
    // Only a term is ever ascribed one, so this child is simply absent on a
    // `type` declaration rather than empty. Labelled the way the AST panel
    // labels the same thing: the role on the type's own node, not a wrapper
    // repeating its text and span.
    if let Some(annotation) = &decl.annotation {
        let mut ascribed = annotation_node(ids, cx, mint, annotation);
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
            mint,
            param.symbol,
        ));
    }
    node.child(value(ids, cx, mint, &decl.value))
}

/// One `effect` declaration's cases: an operation with its two sides under it,
/// or the effect an alias names, as a row that cross-highlights to the
/// declaration it points at.
fn effect_node(ids: &mut Ids, cx: &Cx, mint: &Mint, effect: &Effect) -> Node {
    let node = Node::new(ids.next(), "Cases", String::new());
    match effect {
        Effect::Operations(operations) => node.children(
            operations
                .iter()
                .map(|(name, operation)| {
                    Node::new(
                        ids.next(),
                        format!("{name}:"),
                        format!(
                            "{} -> {}",
                            print::ir::ty(&operation.from.tracked, mint),
                            print::ir::ty(&operation.to.tracked, mint),
                        ),
                    )
                    .at(operation.name_span)
                    .child(type_node(ids, cx, mint, &operation.from, &[]))
                    .child(type_node(ids, cx, mint, &operation.to, &[]))
                })
                .collect::<Vec<_>>(),
        ),
        Effect::Alias(aliases) => node.children(
            aliases
                .iter()
                .map(|(name, aliased)| {
                    with_symbol(
                        Node::new(ids.next(), "Names", print::label(Shape::Effect, name))
                            .at(named(aliased.name_span, name)),
                        cx,
                        mint,
                        aliased.symbol,
                    )
                })
                .collect::<Vec<_>>(),
        ),
    }
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
            mint,
            *symbol,
        ),
        // A literal names nothing, so this is the one leaf of the term tree
        // that carries neither a symbol nor a child.
        TermKind::Natural(_) => Node {
            label: "Natural".into(),
            ..node
        },
        TermKind::Integer(_) => Node {
            label: "Integer".into(),
            ..node
        },
        TermKind::Real(_) => Node {
            label: "Real".into(),
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
                mint,
                arg.tracked,
            );
            // The bound name has no term of its own to carry a type, but the
            // lambda's arrow knows it — through a declared type if that is
            // what the annotation was, since a lambda checked against `Endo`
            // has an argument as surely as one checked against `Nat -> Nat`.
            let shape = cx
                .inference
                .map(|inferred| inference::unfold(&inferred.aliases, &term.ty));
            if let Some(Core::Arrow(from, _, _)) = shape.as_deref().map(|ty| &ty.core) {
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
                mint,
                name.tracked,
            ));
            if let Some(annotation) = annotation {
                let mut ascribed = annotation_node(ids, cx, mint, annotation);
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
        // The handled expression, then one wrapper per arm: what it answers,
        // the name it binds — a symbol, so it cross-highlights with its uses —
        // and its body. The effects the handler discharges get a row of their
        // own, since which they are is what lowering decided and nothing else
        // on the page says.
        TermKind::Handle { body, handler } => {
            let mut handle_node = Node {
                label: "Handle".into(),
                ..node
            }
            .child(term_node(ids, cx, mint, body, trace))
            .child(discharges_node(ids, cx, mint, handler));
            for arm in &handler.arms {
                let head = Node::new(
                    ids.next(),
                    "Arm",
                    format!("!{}.{}", mint.name(arm.effect.tracked), arm.op.tracked),
                )
                .at(named(arm.effect.span, mint.name(arm.effect.tracked)))
                .child(with_symbol(
                    Node::new(ids.next(), "Binder", mint.name(arm.binder.tracked))
                        .at(arm.binder.span),
                    cx,
                    mint,
                    arm.binder.tracked,
                ))
                .child(term_node(ids, cx, mint, &arm.body, trace));
                handle_node = handle_node.child(with_symbol(head, cx, mint, arm.effect.tracked));
            }
            if let Some(ret) = &handler.ret {
                let head = Node::new(ids.next(), "Return", String::new())
                    .at(ret.span)
                    .child(with_symbol(
                        Node::new(ids.next(), "Binder", mint.name(ret.binder.tracked))
                            .at(ret.binder.span),
                        cx,
                        mint,
                        ret.binder.tracked,
                    ))
                    .child(term_node(ids, cx, mint, &ret.body, trace));
                handle_node = handle_node.child(head);
            }
            handle_node
        }
        TermKind::Raise(value) => Node {
            label: "Raise".into(),
            ..node
        }
        .child(term_node(ids, cx, mint, value, trace)),
        // The effect is a symbol and cross-highlights to its declaration; the
        // operation is a label scoped to that declaration, so it names none.
        TermKind::Operation { effect, .. } => with_symbol(
            Node {
                label: "Operation".into(),
                ..node
            }
            // Spanned at the effect's own name rather than at the whole
            // `!Eff.op`, and inside the `!` at that: the row claims the effect,
            // and a claim has to be spanned where the name it claims was
            // written.
            .at(named(effect.span, mint.name(effect.tracked))),
            cx,
            mint,
            effect.tracked,
        ),
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

/// The effects a handler discharges, as a row of its own: which they are is
/// R15's coverage check answered, and nothing else on the page shows it.
fn discharges_node(ids: &mut Ids, cx: &Cx, mint: &Mint, handler: &Handler) -> Node {
    let labels: Vec<String> = handler
        .discharges
        .iter()
        .map(|effect| format!("!{}", mint.name(effect.tracked)))
        .collect();
    let node = Node::new(ids.next(), "Discharges", labels.join(" | "));
    node.children(
        handler
            .discharges
            .iter()
            .map(|effect| {
                with_symbol(
                    Node::new(ids.next(), "", format!("!{}", mint.name(effect.tracked)))
                        .at(named(effect.span, mint.name(effect.tracked))),
                    cx,
                    mint,
                    effect.tracked,
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// The `+ <effects>` clause an arrow carries, as a row of its own: one child
/// per effect it names — each cross-highlighting to the declaration it names —
/// and the `..` tail beside them.
fn effects_node(
    ids: &mut Ids,
    cx: &Cx,
    mint: &Mint,
    effects: &EffectRow,
    scope: &[Variable],
) -> Node {
    let mut kids: Vec<Node> = effects
        .effects
        .iter()
        .map(|(name, label)| {
            let text = match label {
                EffectLabel::Written { when, .. } => {
                    format!(
                        "{}{}",
                        print::label(Shape::Effect, name.name()),
                        when_text(when)
                    )
                }
                EffectLabel::Absent { .. } => {
                    format!("\\{}", print::label(Shape::Effect, name.name()))
                }
            };
            let row = Node::new(ids.next(), text, String::new())
                .at(named(label.name_span(), name.name()));
            // A label an alias put here is *about* the effect without being an
            // occurrence of its name — the name on the page is the alias's —
            // so it takes the association without the span claim.
            match label.expanded() {
                true => match cx.symbols.get(&label.symbol()) {
                    Some(index) => row.owner(*index),
                    None => row,
                },
                false => with_symbol(row, cx, mint, label.symbol()),
            }
        })
        .collect();
    if let Some(tail) = &effects.tail {
        kids.push(rest_node(ids, mint, tail, scope));
    }
    Node::new(ids.next(), "Effects", String::new())
        .at(effects.span)
        .children(kids)
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
            mint,
            name.tracked,
        ),
        // The AST tab's wildcard leaf again, surviving normalization as the
        // `_` it is: no symbol, since nothing can use it.
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
        PatternKind::Struct { fields, rest } => {
            let mut kids: Vec<Node> = fields
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
            // The `..` survives normalization as itself, and is shown the way
            // the AST tab shows it: a row of its own, beside the fields it
            // stands apart from.
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

/// One lowered ascription: the type, the variables its an annotation introduced,
/// and — when one was written — the formula beside them.
///
/// All three are siblings of the type rather than children, exactly as the AST
/// panel shows them: a clause declares what the type uses and constrains the
/// presences its `when`s wear, and burying it under the type would say it was
/// part of one.
///
/// A variable's row says the sort lowering read it as, which is the whole of
/// what the panel adds over the AST's: the name is written bare and what it
/// stands for follows from where the type uses it, exactly as a declaration
/// parameter's does. The rows carry the declaring name's span, and the
/// variable's own id as their [`Node::link`], which every use of that name in
/// the type carries too — so clicking a `..'r` in the type lights the `let r`
/// that declared it, the same cross-highlighting a declaration's header gets
/// from its symbols.
fn annotation_node(ids: &mut Ids, cx: &Cx, mint: &Mint, annotation: &Annotation) -> Node {
    let scope = annotation.variables.as_slice();
    let mut node = type_node(ids, cx, mint, &annotation.ty, scope);
    for variable in &annotation.variables {
        let row = Node::new(
            ids.next(),
            format!("Variable {}", variable.name),
            stands_for_variable(variable),
        )
        .at(variable.span)
        .link(variable.id);
        node = node.child(row);
    }
    match &annotation.clause {
        Some(clause) => {
            let row = Node::new(ids.next(), "Where", clause_text(mint, clause)).at(clause.span);
            node.child(row.children(clause_kids(ids, mint, clause)))
        }
        None => node,
    }
}

/// What one declared variable turned out to stand for: the way its uses spell
/// it, and the reading behind that spelling.
///
/// The [`stands_for`] of a variable, and the reading comes off the
/// compiler's own `Display for Sense` for the reason that one's labels come off
/// `ui::label`: a row here and a complaint about the same variable should not
/// be able to call it two different things.
fn stands_for_variable(variable: &Variable) -> String {
    let written = match variable.sense {
        Sense::Type => format!("'{}", variable.name),
        Sense::Cases | Sense::Effects => format!("..'{}", variable.name),
        Sense::Presence => format!("when '{}", variable.name),
    };
    format!("{written} ({})", variable.sense)
}

/// One clause as the source it was lowered from — through the shared printer,
/// so this row and the type's own text cannot spell one clause two ways.
fn clause_text(mint: &Mint, clause: &Clause) -> String {
    print::ir::clause(&clause.tracked, mint).to_string()
}

/// The parts of a clause, each a row of its own, so a reader can cross-
/// highlight the name a formula uses against the `when` that bound it.
fn clause_kids(ids: &mut Ids, mint: &Mint, clause: &Clause) -> Vec<Node> {
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
            let row = Node::new(ids.next(), "", clause_text(mint, part)).at(part.span);
            row.children(clause_kids(ids, mint, part))
        })
        .collect()
}

/// The `when` clause a lowered label wears, as the row's label shows it.
fn when_text(when: &Option<Box<When>>) -> String {
    match when {
        None => String::new(),
        Some(when) => match &when.name {
            Some(name) => format!(" when '{name}"),
            None => " when _".to_string(),
        },
    }
}

/// The declaration this name was written against, when the name is one of the
/// variables in scope. A use and its declaration share the variable's id, which
/// is unique across the program, so two annotations that each write `a` group
/// separately.
fn link_of(scope: &[Variable], name: &str) -> Option<u32> {
    scope
        .iter()
        .find(|variable| variable.name == name)
        .map(|variable| variable.id)
}

/// `node` grouped with the row that declared the name it uses, when there is
/// one — [`with_symbol`]'s counterpart for a name no symbol table holds.
fn linked(node: Node, link: Option<u32>) -> Node {
    match link {
        Some(id) => node.link(id),
        None => node,
    }
}

/// One written type. `scope` is the variables the annotation around it declared
/// — empty inside a declaration body, which declares none — and is what a use in
/// the type is grouped against.
fn type_node(ids: &mut Ids, cx: &Cx, mint: &Mint, ty: &Type, scope: &[Variable]) -> Node {
    let node = Node::new(ids.next(), "", print::ir::ty(&ty.tracked, mint).to_string()).at(ty.span);
    match &ty.tracked {
        TypeKind::Error => Node {
            label: "Error".into(),
            ..node
        }
        .error(),
        // A row handed to a declaration as its effects, shown as the row an
        // arrow carries is: the labels claim their symbols the same way.
        TypeKind::Effects(effects) => effects_node(ids, cx, mint, effects, scope),
        // A position left for inference to decide, written `_` by the reader
        // or by the pattern desugar's exact demand: it binds nothing and names
        // nothing, so there is nothing to cross-highlight.
        TypeKind::Hole => Node {
            label: "Hole".into(),
            ..node
        },
        // A use of a variable. The name is scoped to its own
        // annotation and resolves to no symbol, so instead of a symbol the row
        // carries the declared variable's id as its link, which is what groups
        // it with the `Variable` row that declared it.
        TypeKind::Var(name) => linked(
            Node {
                label: "Var".into(),
                ..node
            },
            link_of(scope, name),
        ),
        TypeKind::Ident(symbol) => with_symbol(
            Node {
                label: "Ident".into(),
                ..node
            },
            cx,
            mint,
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
            mint,
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
                mint,
                *head,
            );
            Node {
                label: "Apply".into(),
                ..node
            }
            .child(head)
            .children(
                args.iter()
                    .map(|arg| type_node(ids, cx, mint, arg, scope))
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
                        when,
                        payload,
                    } => {
                        let mark = when_text(when);
                        let text = payload.as_ref().map_or(String::new(), |ty| {
                            print::ir::ty(&ty.tracked, mint).to_string()
                        });
                        let node =
                            Node::new(ids.next(), format!("#{name}{mark}"), text).at(*name_span);
                        match payload {
                            Some(payload) => node.child(type_node(ids, cx, mint, payload, scope)),
                            None => node,
                        }
                    }
                    // An absent case is a leaf wearing the `\`, spanning the
                    // whole `\#Name` for cross-highlighting.
                    SumCase::Absent { name_span } => {
                        Node::new(ids.next(), format!("\\#{name}"), String::new()).at(*name_span)
                    }
                })
                .collect();
            if let Some(tail) = tail {
                kids.push(rest_node(ids, mint, tail, scope));
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
            .child(type_node(ids, cx, mint, from, scope))
            .child(type_node(ids, cx, mint, to, scope));
            // The row is a child beside the two sides, the way the AST tab
            // shows it — and a pure arrow, which wrote none, simply has one
            // child fewer.
            match effects.written {
                true => arrow.child(effects_node(ids, cx, mint, effects, scope)),
                false => arrow,
            }
        }
        TypeKind::Struct { fields, tail } => {
            let mut kids: Vec<Node> = fields
                .iter()
                .map(|(name, field)| match field {
                    TypeField::Written {
                        name_span,
                        when,
                        value,
                    } => {
                        let mark = when_text(when);
                        Node::new(
                            ids.next(),
                            format!("{name}{mark}:"),
                            print::ir::ty(&value.tracked, mint).to_string(),
                        )
                        .at(*name_span)
                        .child(type_node(ids, cx, mint, value, scope))
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
                kids.push(rest_node(ids, mint, tail, scope));
            }
            Node {
                label: "Struct".into(),
                ..node
            }
            .children(kids)
        }
    }
}

/// Where the *name* of a label was written, without the marks it wears: the
/// `#` that makes a tag one, the `!` that makes an effect one, and the `\`
/// before either written absent.
///
/// A node claiming a symbol has to be spanned where the name is — the page
/// paints those spans in the editor as uses of it — and a label's own span
/// covers its marks as well. The name is the tail of the span, since every mark
/// a label can wear comes before it.
fn named(span: Span, name: &str) -> Span {
    span.file_id.span(span.end() - name.len(), name.len())
}

/// The `..` tail of either shape of row, as the row of its own it is shown as.
/// A row parameter prints as the name it was declared with, for the reason a
/// type parameter does.
///
/// A named tail is a use of a declared variable, so it is grouped with the
/// declaration the same way a bare name in a type position is; a bare `..`
/// names nothing and a parameter is a symbol, and neither needs the group.
fn rest_node(ids: &mut Ids, mint: &Mint, tail: &Tail, scope: &[Variable]) -> Node {
    let (name, link) = match &tail.of {
        Row::Anything => (String::new(), None),
        Row::Named(name) => (format!("'{name}"), link_of(scope, name)),
        Row::Param { symbol, .. } => (format!("'{}", mint.name(*symbol)), None),
    };
    linked(
        Node::new(ids.next(), "Rest", format!("..{name}")).at(tail.span),
        link,
    )
}
