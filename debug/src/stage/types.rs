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

use indexmap::IndexMap;
use ruddy::{
    ir::Program,
    symbol::{Mint, Symbol},
    types::{Core, Rest, Row, RowField, Scheme, Ty},
};

use crate::{
    stage::{Cx, Ids, Spec, Trace, plural, stands_for},
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
            // A declaration's parameters print as `'a`, `'b` in the meaning
            // above, because that is what they are once the body is lowered.
            // Which is unreadable on its own: these rows are what map each
            // letter back to the name it was written as, and to what it stands
            // for — a row, and what the row may not name, being the two things
            // the meaning column cannot say.
            if let Some(decl) = program.types.get(&symbol) {
                for (index, param) in decl.params.iter().enumerate() {
                    let letter = Core::Bound(index as u32).to_string();
                    let mut row =
                        Node::new(ids.next(), letter, stands_for(mint, param)).at(param.span);
                    if let Some(index) = cx.symbols.get(&param.symbol) {
                        row = row.symbol(*index);
                    }
                    node = node.child(row);
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
    let mut path = Vec::new();
    walk(aliases, start, start, &mut path).then_some(path)
}

/// Whether `at` leads back to `start`, pushing each declaration onto `path` as
/// it is entered and popping it again when it leads nowhere. `path` doubles as
/// the visited set, which is what keeps this finite: a declaration already on
/// it is a loop that does not pass through `start` and is somebody else's row.
fn walk(
    aliases: &IndexMap<Symbol, Scheme>,
    at: Symbol,
    start: Symbol,
    path: &mut Vec<Symbol>,
) -> bool {
    if path.contains(&at) {
        return false;
    }
    path.push(at);
    let mut named = Vec::new();
    if let Some(scheme) = aliases.get(&at) {
        names_in(scheme.body(), &mut named);
    }
    if named.contains(&start) || named.iter().any(|next| walk(aliases, *next, start, path)) {
        return true;
    }
    path.pop();
    false
}

/// Every declaration a type mentions, in the order it mentions them. Stops at
/// a name rather than looking up what it stands for — following one is the
/// caller's business, and the reason this cannot run away.
///
/// Both halves of a type, since both can name one: the core it is, and the
/// fields it carries. A type carrying fields is one inference built rather than
/// one anybody wrote, but it prints in this tab like any other and a declared
/// name inside its fields is mentioned just as much.
fn names_in(ty: &Ty, out: &mut Vec<Symbol>) {
    match &ty.core {
        // The arguments are walked though the body is not: a declaration
        // reached only through one — `type Rose a = { kids: List (Rose a) }` —
        // is mentioned just as much as one written bare, and a row that missed
        // it would not show as recursive.
        Core::Named { symbol, args, .. } => {
            out.push(*symbol);
            for arg in args.iter() {
                names_in(arg, out);
            }
        }
        Core::Arrow(from, to) => {
            names_in(from, out);
            names_in(to, out);
        }
        Core::Sum(cases) => names_in_row(cases, out),
        Core::Unit | Core::Nat | Core::Var(_) | Core::Bound(_) | Core::Undecided => {}
    }
    names_in_labels(&ty.fields, out);
}

/// [`names_in`] over a sum's cases: what each case carries, and whatever a tail
/// already spliced in carries. A presence never holds a name, but a tail bound
/// to more cases does, and one missed there would not show as recursive.
fn names_in_row(row: &Row, out: &mut Vec<Symbol>) {
    names_in_labels(&row.labels, out);
    if let Rest::More(more) = &row.rest {
        names_in_row(more, out);
    }
}

/// [`names_in`] over a label map: what each label holds. A type's fields are one
/// — they have no tail of their own, their tail being the core beside them,
/// which [`names_in`] descends where it sits. So a name sitting in a core under
/// fields, as `WithX Nat with { y: Nat }` has, is still found.
fn names_in_labels(labels: &IndexMap<String, RowField>, out: &mut Vec<Symbol>) {
    for field in labels.values() {
        names_in(&field.ty, out);
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
