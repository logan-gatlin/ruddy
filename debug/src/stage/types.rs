//! What inference concluded, twice over.
//!
//! [`build`] is the `Types` tab: one row per declaration with its scheme (or,
//! for a `type` declaration, what its name stands for — one step deep, since a
//! name inside it is still a name and a recursive one has no other depth to be
//! shown at). And
//! [`annotate`] is the same information where it is most useful — painted onto
//! the IR tab's own rows as inline badges, so every subterm shows its inferred
//! type in place. That is what [`Stage::annotates`](crate::wire::Stage) is
//! for: a stage naming another stage's id owns no tab, and its nodes carry
//! *that* stage's node ids rather than ids of their own.

use std::collections::HashSet;

use indexmap::IndexMap;
use ruddy::{
    ir::{Program, Term, TermKind},
    symbol::{Mint, Symbol},
    tracking::Tracked,
    types::{Rest, Row, Scheme, Ty},
};

use crate::{
    stage::{Cx, Ids, Spec, Trace, plural, stands_for, with_symbol},
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
    let externs = output.externs.iter().map(|(symbol, scheme)| {
        (
            "extern",
            *symbol,
            scheme.to_string(),
            program.externs.get(symbol).map(|decl| decl.name_span),
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
        .chain(externs)
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
            node = with_symbol(node, cx, mint, symbol);
            // A declaration's parameters print as `a`, `b` in the meaning
            // above, because that is what they are once the body is lowered.
            // Which is unreadable on its own: these rows are what map each
            // letter back to the name it was written as, and to what it stands
            // for — a row, and what the row may not name, being the two things
            // the meaning column cannot say.
            if let Some(decl) = program.types.get(&symbol) {
                for (index, param) in decl.params.iter().enumerate() {
                    let letter = Ty::Bound(index as u32).to_string();
                    let row = Node::new(ids.next(), letter, stands_for(mint, param)).at(param.span);
                    node = node.child(with_symbol(row, cx, mint, param.symbol));
                }
            }
            // A recursive declaration says so under itself, naming the loop it
            // closes. The row above already shows the name coming back — what
            // it cannot show is whether the name it came back through leads
            // anywhere, which for a pair declared in terms of each other is
            // the whole question.
            if let Some(loop_names) = loop_through(&output.aliases, symbol) {
                node = node.child(named_row(&mut ids, mint, loop_names));
            }
            // And a recursive definition says the same thing, off the groups
            // lowering published rather than off a walk of its own: a scheme
            // shows none of this, since a definition solved with its group
            // prints exactly like one solved alone.
            if let Some(members) = grouped_with(program, symbol) {
                node = node.child(named_row(&mut ids, mint, members));
            }
            // And every name bound inside it, with the scheme it was given. A
            // local binding is no definition, so it has no row of its own up
            // here — but it is generalized exactly as a definition is, and the
            // whole point of a nested `let` is that it can be used at two
            // types, which is a thing only its scheme says.
            //
            // Found by walking the definition's value rather than by reading
            // the symbol: a local is minted beside its module, not under the
            // definition it was written in, so the tree is the only thing that
            // knows whose it is.
            if let Some(decl) = program.terms.get(&symbol) {
                for local in locals(&decl.value) {
                    let scheme = match output.locals.get(&local.tracked) {
                        Some(scheme) => scheme.to_string(),
                        // Nothing was published for it, which means inference
                        // never reached it: the definition failed to lower, and
                        // its value was erased.
                        None => String::new(),
                    };
                    let row = Node::new(
                        ids.next(),
                        format!("local {}", mint.name(local.tracked)),
                        scheme,
                    )
                    .at(local.span);
                    node = node.child(with_symbol(row, cx, mint, local.tracked));
                }
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
            "aliases: {:#?}\n\nexterns: {:#?}\n\nschemes: {:#?}",
            output.aliases, output.externs, output.schemes
        ),
        ..spec.stage(
            cx.status(),
            match output.externs.is_empty() {
                true => plural(output.schemes.len(), "scheme"),
                false => format!(
                    "{} · {}",
                    plural(output.externs.len(), "extern"),
                    plural(output.schemes.len(), "scheme")
                ),
            },
        )
    }
}

/// Every name a nested `let` binds inside one definition's value, in the order
/// they were written — which is the order they were walked, and so the order
/// their schemes were published in.
fn locals(term: &Term) -> Vec<Tracked<Symbol>> {
    let mut out = Vec::new();
    walk_locals(term, &mut out);
    out
}

fn walk_locals(term: &Term, out: &mut Vec<Tracked<Symbol>>) {
    match &term.kind {
        TermKind::Let {
            name, value, body, ..
        } => {
            out.push(*name);
            walk_locals(value, out);
            walk_locals(body, out);
        }
        TermKind::Unary { value, .. } => walk_locals(value, out),
        TermKind::Binary { left, right, .. } => {
            walk_locals(left, out);
            walk_locals(right, out);
        }
        TermKind::Apply { func, arg } => {
            walk_locals(func, out);
            walk_locals(arg, out);
        }
        TermKind::Fn { body, .. } => walk_locals(body, out),
        TermKind::Struct(fields) => {
            for field in fields.values() {
                walk_locals(&field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                walk_locals(payload, out);
            }
        }
        TermKind::Project { base, .. } => walk_locals(base, out),
        // A match's binders are lambda-argument-like — no scheme is published
        // for one — so only what sits inside is walked.
        TermKind::Match { scrutinee, arms } => {
            walk_locals(scrutinee, out);
            for (_, body) in arms {
                walk_locals(body, out);
            }
        }
        // A handler arm's binder is a lambda argument's twin, so no scheme is
        // published for one either; only what sits inside is walked.
        TermKind::Handle { body, handler } => {
            walk_locals(body, out);
            for arm in &handler.arms {
                walk_locals(&arm.body, out);
            }
            if let Some(ret) = &handler.ret {
                walk_locals(&ret.body, out);
            }
        }
        TermKind::Raise(value) => walk_locals(value, out),
        TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// The `recursive` child: the names something loops through, or is grouped
/// with, itself first and the rest in the order they were found. One row
/// builder for the two, because they are one row — a reader looking at it wants
/// the same thing of a `type` and a `let`.
fn named_row(ids: &mut Ids, mint: &Mint, names: Vec<Symbol>) -> Node {
    Node::new(
        ids.next(),
        "recursive",
        names
            .iter()
            .map(|symbol| mint.name(*symbol))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The definitions `symbol` is typed together with, itself first and the rest
/// in group order, or `None` when its group refers back into itself nowhere.
///
/// `None` for a `type` declaration too: groups are a term's business, and a
/// declaration's own recursion is [`loop_through`]'s to find.
fn grouped_with(program: &Program, symbol: Symbol) -> Option<Vec<Symbol>> {
    let group = program
        .groups
        .iter()
        .find(|group| group.members.contains(&symbol))?;
    group.recursive.then(|| {
        std::iter::once(symbol)
            .chain(
                group
                    .members
                    .iter()
                    .copied()
                    .filter(|member| *member != symbol),
            )
            .collect()
    })
}

/// The declarations one has to pass through to get from `start` back to
/// `start`, itself first, or `None` when it never does.
///
/// A declaration mentioning itself is `[start]`; two declared in terms of each
/// other are `[start, other]`. Depth-first, so what comes back is the first
/// loop found rather than the shortest — either answers the question the row
/// asks, and showing every loop a declaration is part of is not what the row is
/// for.
fn loop_through(aliases: &IndexMap<Symbol, Scheme>, start: Symbol) -> Option<Vec<Symbol>> {
    struct Frame {
        at: Symbol,
        named: Vec<Symbol>,
        next: usize,
    }

    let named = aliases.get(&start).map_or_else(Vec::new, |scheme| {
        let mut named = Vec::new();
        names_in(scheme.body(), &mut named);
        named
    });
    let mut path = vec![start];
    if named.contains(&start) {
        return Some(path);
    }
    let mut active = HashSet::from([start]);
    let mut dead = HashSet::new();
    let mut work = vec![Frame {
        at: start,
        named,
        next: 0,
    }];

    while let Some(frame) = work.last_mut() {
        let Some(next) = frame.named.get(frame.next).copied() else {
            let frame = work.pop().expect("the exhausted frame is present");
            active.remove(&frame.at);
            dead.insert(frame.at);
            path.pop();
            continue;
        };
        frame.next += 1;
        // An active name closes a different loop; a dead one has already been
        // proved unable to reach this fixed start.
        if active.contains(&next) || dead.contains(&next) {
            continue;
        }
        let named = aliases.get(&next).map_or_else(Vec::new, |scheme| {
            let mut named = Vec::new();
            names_in(scheme.body(), &mut named);
            named
        });
        active.insert(next);
        path.push(next);
        if named.contains(&start) {
            return Some(path);
        }
        work.push(Frame {
            at: next,
            named,
            next: 0,
        });
    }
    None
}

/// Every declaration a type mentions, in the order it mentions them. Stops at
/// a name rather than looking up what it stands for — following one is the
/// caller's business, and the reason this cannot run away.
///
/// Both halves of a type, since both can name one: the constructor it is, and the
/// fields it carries. A struct type is one inference built rather than
/// one anybody wrote, but it prints in this tab like any other and a declared
/// name inside its fields is mentioned just as much.
fn names_in(ty: &Ty, out: &mut Vec<Symbol>) {
    enum Work<'a> {
        Ty(&'a Ty),
        Row(&'a Row),
    }

    let mut work = vec![Work::Ty(ty)];
    while let Some(next) = work.pop() {
        match next {
            Work::Ty(ty) => match ty {
                // The arguments are walked though the body is not: a
                // declaration reached only through one is mentioned just as
                // much as one written bare.
                Ty::Named { symbol, args, .. } => {
                    out.push(*symbol);
                    work.extend(args.iter().rev().map(|arg| Work::Ty(arg)));
                }
                // An effect row names effects and no declared types.
                Ty::Arrow(from, to, _) => {
                    work.push(Work::Ty(to));
                    work.push(Work::Ty(from));
                }
                Ty::Struct(row) | Ty::Sum(row) => work.push(Work::Row(row)),
                Ty::Nat
                | Ty::Int
                | Ty::Real
                | Ty::String
                | Ty::Boolean
                | Ty::Var(_)
                | Ty::Bound(_)
                | Ty::Rigid { .. }
                | Ty::Undecided => {}
            },
            Work::Row(row) => {
                // Labels precede the spliced tail, and each label keeps source
                // order despite the LIFO work stack.
                if let Rest::More(more) = &row.rest {
                    work.push(Work::Row(more));
                }
                work.extend(row.labels.values().rev().map(|field| Work::Ty(&field.ty)));
            }
        }
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
    let externs = program
        .externs
        .keys()
        .map(|symbol| output.externs.get(symbol).map(|scheme| scheme.to_string()));
    let schemes = program
        .terms
        .keys()
        .map(|symbol| output.schemes.get(symbol).map(|scheme| scheme.to_string()));
    let mut nodes: Vec<Node> = trace
        .decls
        .iter()
        .zip(aliases.chain(externs).chain(schemes))
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
