//! Tests for [`ruddy_debug::snapshot`].

use std::collections::HashMap;

use ruddy_debug::{
    snapshot::{compile, guard, install_hook},
    stage::REGISTRY,
    wire::{BundleSpec, CompileRequest, Node, Snapshot, Stage, View},
};

const DEMO: &str = include_str!("../../demo.hc");

fn snapshot(source: &str) -> Snapshot {
    compile(
        &CompileRequest {
            source: source.to_string(),
            revision: 3,
            bundle: BundleSpec::default(),
        },
        1,
    )
}

fn nodes(stage: &Stage) -> Vec<&Node> {
    fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
        for node in nodes {
            out.push(node);
            walk(&node.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&stage.nodes, &mut out);
    out
}

#[test]
fn every_stage_reports_on_the_demo() {
    let snapshot = snapshot(DEMO);
    let ids: Vec<_> = snapshot.stages.iter().map(|stage| stage.id).collect();
    assert_eq!(
        ids,
        [
            "tokens",
            "ast",
            "ir",
            "constraints",
            "solve",
            "types",
            "symbols",
            "types-ir"
        ]
    );
    assert_eq!(snapshot.revision, 3);
    assert!(snapshot.panic.is_none());
    for stage in &snapshot.stages {
        assert!(!stage.nodes.is_empty(), "{} produced nothing", stage.id);
        // The pane bar prints this beside the tab strip, so a stage that
        // counted nothing leaves a blank there rather than a count.
        assert!(!stage.summary.is_empty(), "{} counted nothing", stage.id);
    }

    // The tab strip is the stages that annotate nothing, so the annotator at
    // the end of the registry adds no second "Types" tab.
    let titles: Vec<_> = snapshot
        .stages
        .iter()
        .filter(|stage| stage.annotates.is_none())
        .map(|stage| stage.title)
        .collect();
    assert_eq!(
        titles,
        [
            "Tokens",
            "AST",
            "IR",
            "Constraints",
            "Solve",
            "Types",
            "Symbols"
        ]
    );
}

/// A span hygiene check on the compiler, not on the debugger: every offset
/// a stage hands out has to be a real position in the source it came from.
/// Bad `merge` arithmetic shows up here rather than as a mangled highlight.
#[test]
fn every_span_lies_inside_the_source() {
    let snapshot = snapshot(DEMO);
    for stage in &snapshot.stages {
        for node in nodes(stage) {
            let Some([start, end]) = node.span else {
                continue;
            };
            assert!(
                start <= end && end <= DEMO.len(),
                "{}: {} {:?} has span {start}..{end} in {} bytes",
                stage.id,
                node.label,
                node.text,
                DEMO.len()
            );
            assert!(
                DEMO.get(start..end).is_some(),
                "{}: {} has a span that splits a character",
                stage.id,
                node.label
            );
        }
    }
    for diagnostic in &snapshot.diagnostics {
        if let Some([start, end]) = diagnostic.span {
            assert!(DEMO.get(start..end).is_some(), "{}", diagnostic.code);
        }
    }
}

/// `Node::symbol` is what the page paints occurrences from: it walks every
/// stage, collects the span of every node carrying the symbol, and highlights
/// all of them in the editor as uses of that name. So a node may only claim it
/// when its span really is somewhere the name was written — which is a
/// property of the whole snapshot, checkable here, and not one any single
/// stage can be trusted to have remembered. A stage that wants the association
/// without the span has `Node::owner` for it.
#[test]
fn a_node_naming_a_symbol_is_spanned_at_the_name() {
    let snapshot = snapshot(DEMO);
    let symbols = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage is registered");
    // The symbols stage is the index every `symbol` points into, and its own
    // rows are labelled with the names.
    let names: HashMap<u32, &str> = symbols
        .nodes
        .iter()
        .map(|node| (node.id, node.label.as_str()))
        .collect();

    let mut checked = 0;
    for stage in &snapshot.stages {
        for node in nodes(stage) {
            let (Some(index), Some([start, end])) = (node.symbol, node.span) else {
                continue;
            };
            let name = names
                .get(&index)
                .unwrap_or_else(|| panic!("{}: {} points at no symbol row", stage.id, node.label));
            assert_eq!(
                &DEMO[start..end],
                *name,
                "{}: {} claims to name {name} at {start}..{end}",
                stage.id,
                node.label
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no stage claimed a symbol at all");

    // And the association a solve step does want is the one that costs it no
    // occurrence: its span is whatever sub-expression the constraint came
    // from, which is nowhere the definition's name appears.
    let solve = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    assert!(!solve.nodes.is_empty());
    assert!(
        solve.nodes.iter().all(|node| node.symbol.is_none()),
        "a solve step claimed its span as an occurrence"
    );
    assert!(
        solve.nodes.iter().any(|node| node.owner.is_some()),
        "no solve step says which definition it belongs to"
    );
}

#[test]
fn diagnostics_are_reported_in_source_order() {
    let snapshot = snapshot(DEMO);
    let codes: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(codes.contains(&"undefined-term"), "{codes:?}");
    assert!(codes.contains(&"unrecognized-character"), "{codes:?}");

    let offsets: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.span.map(|span| span[0]).unwrap_or(0))
        .collect();
    assert!(
        offsets.windows(2).all(|pair| pair[0] <= pair[1]),
        "{offsets:?}"
    );
}

/// A literal has to show up in every panel that renders a term, and to be
/// coloured as a literal in the editor — which the tokens stage is what
/// drives.
#[test]
fn a_natural_reaches_every_stage() {
    let snapshot = snapshot("let n = 42\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    for id in ["tokens", "ast", "ir"] {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        let node = nodes(stage)
            .into_iter()
            .find(|node| node.label == "Natural")
            .unwrap_or_else(|| panic!("{id} rendered no natural"));

        assert_eq!(node.text, "42", "{id}");
        assert_eq!(node.span, Some([8, 10]), "{id}");
        // A literal names nothing, so no panel may point it at a symbol.
        assert_eq!(node.symbol, None, "{id}");
    }

    let tokens = nodes(&snapshot.stages[0]);
    let literal = tokens.iter().find(|node| node.label == "Natural").unwrap();
    let class = literal
        .fields
        .iter()
        .find(|field| field.name == "_class")
        .expect("the editor is told what to paint it");
    assert_eq!(class.value, "number");
}

/// The three forms phase 0 adds, in one line, checked through every panel
/// that renders them. A stage that stopped matching on one of them would
/// fail to compile; this is the check that it renders it too.
#[test]
fn the_surface_prerequisites_reach_every_stage() {
    let source = "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let labelled = |id: &str, label: &str| -> Vec<String> {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        nodes(stage)
            .into_iter()
            .filter(|node| node.label == label)
            .map(|node| node.text.clone())
            .collect()
    };

    for id in ["ast", "ir"] {
        // The ascription is a child of the declaration in both trees, and the
        // node carrying it is the arrow's own: one node, labelled with its role
        // and its kind, rather than a wrapper repeating the type below it.
        assert_eq!(
            labelled(id, "Ascribed Arrow"),
            ["{ x: Nat, y: Nat } -> Nat"],
            "{id}"
        );
        assert_eq!(labelled(id, "Arrow"), [] as [&str; 0], "{id}");
        assert_eq!(labelled(id, "Project"), ["p.x"], "{id}");
        assert_eq!(labelled(id, "Field"), ["x"], "{id}");
    }

    // `Nat` is an ordinary identifier until lowering, which is the one
    // place the two trees are meant to differ.
    assert_eq!(labelled("ir", "Prim"), ["Nat", "Nat", "Nat"]);
    assert_eq!(labelled("tokens", "Arrow"), ["->"]);
    assert_eq!(labelled("tokens", "Dot"), ["."]);
}

/// The row forms — an optional field, and a named tail — checked through
/// every panel that renders them: the tokens they lex to, the field and tail
/// rows of both trees, the constraint the projection becomes, and the scheme
/// the definition ends with.
#[test]
fn rows_reach_every_stage() {
    let source = "let f : { x?: Nat, y: Nat, ..r } -> Nat = fn p => p.y\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered")
    };
    let labelled = |id: &str, label: &str| -> Vec<String> {
        nodes(stage(id))
            .into_iter()
            .filter(|node| node.label == label)
            .map(|node| node.text.clone())
            .collect()
    };

    assert_eq!(labelled("tokens", "DotDot"), [".."]);
    assert_eq!(labelled("tokens", "Question"), ["?"]);

    for id in ["ast", "ir"] {
        // The optional field is a row of the struct's like any other, wearing
        // its `?` in the label; the tail is a row of its own.
        assert_eq!(labelled(id, "x?:"), ["Nat"], "{id}");
        assert_eq!(labelled(id, "Rest"), ["..r"], "{id}");
    }

    // The projection's demand is an ordinary equality against an open row,
    // and the Constraints tab shows it unsolved: the tail is still the
    // variable the annotation lowered to.
    let constraints: Vec<&str> = stage("constraints")
        .nodes
        .iter()
        .flat_map(|group| &group.children)
        .map(|node| node.text.as_str())
        .collect();
    assert!(
        constraints.iter().any(|text| text.contains("..?")),
        "{constraints:?}"
    );

    // The scheme spells the quantified tail with its letter and keeps the
    // field optional — nothing in the body decided `x` either way.
    let types: Vec<&str> = stage("types")
        .nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(types, ["{ x?: Nat, y: Nat, ..'a } -> Nat"]);
}

/// `()` is one piece of punctuation in the surface syntax, and the AST keeps
/// it that way — a term position and a type position both read back as `()`.
/// Lowering folds both into the struct with no fields, so the IR reads every
/// occurrence back as `{}` instead, regardless of which namespace it came
/// from. The two trees therefore say different things about the same three
/// characters — which is the sort of difference the panels exist to show.
#[test]
fn unit_is_the_empty_struct_in_the_ir() {
    let snapshot = snapshot("type T = ()\nlet u : () = ()\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    // Every node the source spelled `()`, by label and by how the stage renders
    // it back — in the IR the two are no longer the same string.
    let unit_nodes = |id: &str| -> Vec<(String, String)> {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        nodes(stage)
            .into_iter()
            .filter(|node| node.text == "()" || node.text == "{}")
            .map(|node| (node.label.clone(), node.text.clone()))
            .collect()
    };

    // Two type positions and one term; the parse tree calls all three Unit and
    // renders all three as the punctuation they were written as.
    assert_eq!(
        unit_nodes("ast"),
        [
            ("Unit".into(), "()".into()),
            ("Ascribed Unit".into(), "()".into()),
            ("Unit".into(), "()".into()),
        ] as [(String, String); 3]
    );
    // Lowering folds the unit value and the unit type alike into a fieldless
    // `Struct`, which reads back as `{}` rather than as what was written.
    assert_eq!(
        unit_nodes("ir"),
        [
            ("Struct".into(), "{}".into()),
            ("Ascribed Struct".into(), "{}".into()),
            ("Struct".into(), "{}".into()),
        ] as [(String, String); 3]
    );
}

#[test]
fn a_bad_literal_is_a_diagnostic_of_its_own() {
    let codes = |source: &str| {
        snapshot(source)
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>()
    };
    // The lexer emits no token for a literal it rejected, so the `let` is left
    // with no body — which the parser reports where the body would have gone,
    // just before the characters the lexer is complaining about, rather than
    // standing a `()` in for it.
    assert_eq!(
        codes("let n = 1x\n"),
        ["unexpected-token", "malformed-natural"]
    );
    assert_eq!(
        codes(&format!("let n = {}0\n", u128::MAX)),
        ["unexpected-token", "natural-too-large"]
    );
}

/// A repeat is only legible next to what it repeats, so it has to arrive as
/// one diagnostic with a related span rather than as two loose lines.
#[test]
fn a_duplicate_carries_the_definition_it_repeats() {
    let snapshot = snapshot("let x = ()\nlet x = ()\n");
    let duplicate = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate-term")
        .expect("the repeat is reported");
    assert_eq!(duplicate.related.len(), 1);
    assert_eq!(duplicate.related[0].span, Some([4, 5]));
}

/// The types stage reports what inference concluded, and the annotating
/// stage carries the same conclusions keyed by the IR stage's own node ids.
/// That keying is the whole contract behind `annotates`, and the page's badge
/// painting is silently wrong rather than broken if it ever slips — which is
/// why it is asserted here rather than left to the eye.
#[test]
fn inferred_types_reach_the_panels() {
    let snapshot = snapshot("let id = fn x => x\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    let types = stage("types");
    assert_eq!(types.annotates, None);
    let scheme = nodes(types)
        .into_iter()
        .find(|node| node.label == "let id")
        .expect("the definition has a row");
    assert_eq!(scheme.text, "'a -> 'a");

    let badges = stage("types-ir");
    assert_eq!(badges.annotates, Some("ir"));
    assert!(!badges.nodes.is_empty());
    // Every badge decorates a row the IR stage actually renders.
    let ir_ids: std::collections::HashSet<_> =
        nodes(stage("ir")).into_iter().map(|node| node.id).collect();
    for badge in &badges.nodes {
        assert!(ir_ids.contains(&badge.id), "badge {} has no row", badge.id);
    }
    // The declaration row wears the scheme.
    let texts: Vec<_> = badges.nodes.iter().map(|node| node.text.as_str()).collect();
    assert!(texts.contains(&"'a -> 'a"), "{texts:?}");
}

/// Every row the IR stage renders a term on wears the type inference gave it.
/// The badges used to come from a second walk that mirrored the IR's child
/// layout by hand, so a change to one could silently stop the other; both now
/// fall out of the same walk, and this is the check that they still line up
/// across every shape a term can take.
#[test]
fn every_term_row_in_the_ir_wears_its_type() {
    let snapshot = snapshot("let f : { x: Nat } -> Nat = fn p => p.x\nlet n = f { x: 1 }\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    let badges: HashMap<u32, &str> = stage("types-ir")
        .nodes
        .iter()
        .map(|node| (node.id, node.text.as_str()))
        .collect();

    // Labels no type node ever carries, so every row reached here is a term.
    const TERMS: [&str; 5] = ["Apply", "Fn", "Project", "Natural", "Arg"];
    let rows = nodes(stage("ir"));
    let terms: Vec<&&Node> = rows
        .iter()
        .filter(|node| TERMS.contains(&node.label.as_str()))
        .collect();
    // `fn p => p.x`, its `p`, `p.x`, `f { x: 1 }` and the `1` inside it.
    assert_eq!(terms.len(), 5, "{terms:#?}");
    for node in &terms {
        assert!(
            badges.contains_key(&node.id),
            "{} {:?} wears no type",
            node.label,
            node.text
        );
    }

    let badge = |label: &str, text: &str| -> &str {
        let node = rows
            .iter()
            .find(|node| node.label == label && node.text == text)
            .unwrap_or_else(|| panic!("the IR renders no {label} {text:?}"));
        badges[&node.id]
    };
    // The bound name has no term of its own to carry a type; the lambda's
    // arrow is where it comes from.
    assert_eq!(badge("Arg", "p"), "{ x: Nat }");
    assert_eq!(badge("Project", "p.x"), "Nat");
    assert_eq!(badge("Apply", "f { x: 1 }"), "Nat");
    assert_eq!(badge("Natural", "1"), "Nat");
}

/// A declared type is shown as what it stands for, one step deep, and says
/// under itself which declarations it leads back through. The row above can
/// only show the name coming back; whether that name leads anywhere is what
/// two types declared in terms of each other make impossible to read off.
#[test]
fn the_types_tab_says_which_declarations_are_recursive() {
    let snapshot = snapshot(
        "type list = { val: Nat, next: list }\n\
         type forest = { head: tree }\n\
         type tree = { val: Nat, kids: forest }\n\
         type Endo = Nat -> Nat\n",
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let types = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage is registered");
    let row = |name: &str| {
        types
            .nodes
            .iter()
            .find(|node| node.label == format!("type {name}"))
            .unwrap_or_else(|| panic!("no row for {name}"))
    };

    // One step deep: the name inside is still a name, which is the only way a
    // type that names itself can be rendered at all.
    assert_eq!(row("list").text, "{ val: Nat, next: list }");
    assert_eq!(row("Endo").text, "Nat -> Nat");

    let recursion = |name: &str| -> Option<&str> {
        row(name)
            .children
            .iter()
            .find(|child| child.label == "recursive")
            .map(|child| child.text.as_str())
    };
    // A declaration that names itself is a loop of one.
    assert_eq!(recursion("list"), Some("list"));
    // And one that does not is the case the row cannot show on its own:
    // neither of these mentions itself, and the loop runs through a
    // declaration written below the first of them.
    assert_eq!(recursion("forest"), Some("forest, tree"));
    assert_eq!(recursion("tree"), Some("tree, forest"));
    // And an alias that leads nowhere says nothing.
    assert_eq!(recursion("Endo"), None);
}

/// A lambda's argument badge comes from the arrow the lambda has, which is an
/// arrow just as much when the annotation was a name for one.
#[test]
fn an_argument_wears_its_type_through_a_declared_type() {
    let snapshot = snapshot("type Endo = Nat -> Nat\nlet id : Endo = fn x => x\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    let badges: HashMap<u32, &str> = stage("types-ir")
        .nodes
        .iter()
        .map(|node| (node.id, node.text.as_str()))
        .collect();
    let arg = nodes(stage("ir"))
        .into_iter()
        .find(|node| node.label == "Arg")
        .expect("the IR renders the bound name");
    assert_eq!(badges[&arg.id], "Nat");
}

/// The strip's messages are inference's own words. They were a copy of the
/// CLI driver's, and the two had already drifted apart on this very sentence —
/// `tests/src/inference.rs` pins the other end of it.
#[test]
fn a_type_error_is_a_diagnostic() {
    let mismatch = snapshot("let n : Nat = fn x => x\n");
    let codes: Vec<_> = mismatch
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(codes, ["type-mismatch"]);

    let missing = snapshot("let f : { x: Nat } -> Nat = fn p => p.y\n");
    let [diagnostic] = missing.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", missing.diagnostics);
    };
    assert_eq!(diagnostic.code, "missing-field");
    assert_eq!(diagnostic.message, "no field `y` on `{ x: Nat }`");
}

/// The two complaints rows added reach the strip like every other, and the
/// rule behind one of them reaches the Solve tab. A diagnostic the compiler
/// can raise and the debugger cannot show is one nobody working on the
/// compiler ever sees.
#[test]
fn a_row_error_reaches_the_strip_and_the_solve_tab() {
    let repeated = snapshot("let h : { x: { y: Nat, ..r } } -> { ..r } = fn p => { y: {} }\n");
    let [diagnostic] = repeated.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", repeated.diagnostics);
    };
    assert_eq!(diagnostic.code, "repeated-field");
    assert_eq!(
        diagnostic.message,
        "`..` covers only the fields a type does not already name, \
         and here it would have to cover `y`"
    );

    // The step that refused the binding is red, labelled with its rule, and
    // carries the same words the strip does.
    let solve = repeated
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    let overlap = solve
        .nodes
        .iter()
        .find(|node| node.label == "overlap")
        .expect("the refusal is a step");
    assert!(overlap.error);
    let field = |node: &Node, name: &str| {
        node.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };
    assert_eq!(field(overlap, "_error"), Some(diagnostic.message.clone()));
    assert_eq!(
        field(overlap, "_rule"),
        Some("the rest of a row cannot be a row naming a field the row already names".to_string())
    );

    let narrowed = snapshot("let f : { x?: Nat, .. } -> Nat = fn p => p.x\n");
    let [diagnostic] = narrowed.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", narrowed.diagnostics);
    };
    assert_eq!(diagnostic.code, "annotation-too-open");
    // Spanned at the annotation, which is the text the reader has to change.
    assert_eq!(diagnostic.span, Some([8, 30]));

    let flat = snapshot("let n = 1\nlet bad = n.x\n");
    let [diagnostic] = flat.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", flat.diagnostics);
    };
    assert_eq!(diagnostic.code, "not-a-struct");
    assert_eq!(
        diagnostic.message,
        "`Nat` is not a struct, so it has no fields to read"
    );
}

#[test]
fn symbols_round_trip_through_the_mangler() {
    let snapshot = snapshot("let f = fn x => x\ntype T = ()\n");
    let symbols = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage is registered");
    assert_eq!(symbols.summary, "3 symbols");
    for node in nodes(symbols) {
        let demangle = node
            .fields
            .iter()
            .find(|field| field.name == "demangle")
            .expect("every row checks itself");
        assert_eq!(demangle.value, "ok", "{} did not round-trip", node.label);
        assert!(!node.error);
    }

    // Each row's text is the path, and the lambda's `x` is a local: it belongs
    // to no module, so the panel shows it one segment in rather than beside
    // the top-level `f` it is not addressable alongside.
    let paths: Vec<(&str, &str)> = nodes(symbols)
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    // In mint order, which is the row order: types are lowered first, and a
    // lambda's binder is minted before the definition it is bound inside.
    assert_eq!(
        paths,
        [("T", "demo::T"), ("x", "demo::_::x"), ("f", "demo::f")]
    );
}

/// The compiler will panic while it is being worked on. When it does, the
/// panic has to become a result rather than take the server with it.
#[test]
fn a_panic_becomes_a_result() {
    install_hook();
    let mut slot = None;
    let value = guard("ir", &mut slot, || panic!("the name table lied"));

    assert!(value.is_none());
    let panicked = slot.expect("the panic was captured");
    assert_eq!(panicked.stage, "ir");
    assert_eq!(panicked.message, "the name table lied");
    assert!(panicked.location.contains("snapshot.rs"));

    // Only the first panic is kept: the ones after it are the same bug seen
    // from a stage that was handed nothing.
    let mut slot = Some(panicked);
    guard("ast", &mut slot, || panic!("second"));
    assert_eq!(slot.expect("still the first").stage, "ir");
}

#[test]
fn a_snapshot_survives_the_wire() {
    let snapshot = snapshot(DEMO);
    let json = serde_json::to_string(&snapshot).expect("serializes");
    let back: serde_json::Value = serde_json::from_str(&json).expect("parses");

    // Every registered stage reaches the page, annotators included: the count
    // comes from the registry so that adding a stage cannot quietly leave one
    // off the wire without also being noticed here.
    assert_eq!(
        back["stages"].as_array().expect("stages").len(),
        REGISTRY.len()
    );
    assert_eq!(back["stages"][0]["view"], "list");
    assert_eq!(back["stages"][1]["view"], "tree");
    assert!(back["line_starts"].as_array().expect("line starts").len() > 1);
    assert_eq!(back["source_len"], DEMO.len());
    // Underscored fields are the page's, and have to survive too: the
    // editor's colouring is built from them.
    assert!(json.contains("_class"));

    // Everything the pane bar and the highlighter read off a stage. A field
    // the page branches on is no use to it left behind on this side.
    let types = back["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|stage| stage["id"] == "types")
        .expect("the types stage is registered");
    assert_eq!(types["scoped"], true);
    // A stage owning no phase says so on the wire rather than reporting a zero
    // the page has to guess the meaning of.
    let solve = back["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|stage| stage["id"] == "solve")
        .expect("the solve stage is registered");
    assert!(solve["micros"].is_null());
    assert!(types["micros"].as_u64().is_some());
    assert!(types["summary"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(json.contains("\"owner\""));
}

#[test]
fn an_empty_buffer_is_not_an_error() {
    let snapshot = snapshot("");
    assert!(snapshot.diagnostics.is_empty());
    assert!(snapshot.panic.is_none());
    assert_eq!(snapshot.line_starts, vec![0]);
}

#[test]
fn a_bad_bundle_is_reported_rather_than_fatal() {
    let snapshot = compile(
        &CompileRequest {
            source: "let x = ()".to_string(),
            revision: 0,
            bundle: BundleSpec {
                name: "not a name".to_string(),
                version: "1.0.0".to_string(),
            },
        },
        1,
    );
    assert_eq!(snapshot.diagnostics[0].code, "bad-bundle");
    // The fallback bundle still lowers the program.
    let ir = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "ir")
        .expect("ir stage");
    assert_eq!(ir.nodes.len(), 1);
}

/// The `Solve` tab is a timeline the page walks with a cursor, building its two
/// state panels by appending each step's `_bind` and `_error` as it passes
/// them. That only works if those fields appear exactly on the steps that
/// changed something, and if a binding is only ever added — never rewritten,
/// because stepping backwards is dropping the tail of the list.
#[test]
fn a_solver_step_declares_what_it_added_to_the_state() {
    let snapshot = snapshot(
        "let fst : { x: Nat } -> Nat = fn p => p.x\nlet miss : { x: Nat } -> Nat = fn p => p.y\n",
    );
    let stage = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    assert!(matches!(stage.view, View::Steps), "{:?}", stage.view);
    assert!(!stage.nodes.is_empty());

    let field = |node: &Node, name: &str| {
        node.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };

    let mut bound = Vec::new();
    let mut failed = Vec::new();
    for node in &stage.nodes {
        // Everything the page reads off every step, on every step.
        for name in ["_rule", "_effect", "_depth", "_def"] {
            assert!(field(node, name).is_some(), "{} has no {name}", node.label);
        }
        let depth = field(node, "_depth").expect("a depth");
        assert!(depth.parse::<u32>().is_ok(), "depth {depth:?}");

        // A step that failed is the red one, and the only one carrying an
        // error: the page paints from `error` and accumulates from `_error`,
        // so the two cannot be allowed to disagree.
        assert_eq!(
            node.error,
            field(node, "_error").is_some(),
            "{} is {} but {} an error",
            node.label,
            if node.error { "red" } else { "not red" },
            if node.error { "carries no" } else { "carries" },
        );
        if let Some(bind) = field(node, "_bind") {
            // The solution panel is the `_effect` column's bindings collected,
            // so a step's `_bind` is its `_effect` and not a second wording of
            // it. The two had been written out separately, identically, which
            // is two places for one notation to drift from.
            assert_eq!(field(node, "_effect"), Some(bind.clone()), "{}", node.label);
            bound.push(bind);
        }
        if let Some(error) = field(node, "_error") {
            failed.push(error);
        }
    }

    // Appended, never rewritten.
    let mut once = bound.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), bound.len(), "{bound:?}");

    // What the reader would have collected by the end is what inference
    // reported: one error, said the same way in both places.
    let reported: Vec<&str> = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.stage == "types")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(failed, reported);
}

/// The pane bar prints one timing chip per compiler phase, and which stages own
/// a phase is a fact about the registry rather than something to be read off a
/// number. `Constraints` and `Solve` are two views of the one `infer` call and
/// the annotator does no phase at all, so all three report nothing; everyone
/// else reports what their phase took. The page filtered on `micros > 0`
/// instead, which is a measurement standing in for that fact — and the
/// measurement truncates to whole microseconds, so a phase quick enough to
/// round to zero silently lost its chip.
#[test]
fn only_the_stages_that_own_a_phase_report_a_time() {
    let snapshot = snapshot(DEMO);
    let ids = |timed: bool| -> Vec<&str> {
        snapshot
            .stages
            .iter()
            .filter(|stage| stage.micros.is_some() == timed)
            .map(|stage| stage.id)
            .collect()
    };
    assert_eq!(ids(true), ["tokens", "ast", "ir", "types", "symbols"]);
    assert_eq!(ids(false), ["constraints", "solve", "types-ir"]);
}

/// A tab's raw view dumps what that tab owns, and no more.
/// `inference::Output` is one struct carrying three tabs' worth of material,
/// and `Types` had been dumping the whole of it — so the constraint list and
/// every solver step crossed the wire twice, on every keystroke, since each one
/// posts a fresh snapshot.
#[test]
fn a_raw_dump_carries_only_its_own_tab() {
    let snapshot = snapshot("let fst : { x: Nat } -> Nat = fn p => p.x\n");
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    // Each of the three payloads, dumped by the one tab that shows it.
    let types = stage("types");
    assert!(types.debug.contains("schemes"), "{}", types.debug);
    assert!(stage("constraints").debug.contains("Constraint"));
    assert!(stage("solve").debug.contains("Step"));

    // And not by the other two.
    assert!(
        !types.debug.contains("Step"),
        "the Types dump repeats the solve"
    );
    assert!(
        !types.debug.contains("Constraint"),
        "the Types dump repeats the constraints"
    );
}

/// Summaries sit in the pane bar as a phrase somebody reads, so the count and
/// its noun agree. Three stages had spelled that out for themselves and the
/// rest had not, which is how `1 schemes` reached the bar of a tool whose whole
/// subject is getting the details right.
#[test]
fn a_count_of_one_is_said_in_the_singular() {
    let snapshot = snapshot("let a : Nat = 1\n");
    let summary = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
            .summary
            .as_str()
    };
    assert_eq!(summary("constraints"), "1 constraint");
    assert_eq!(summary("solve"), "1 step");
    assert_eq!(summary("types"), "1 scheme");
    assert_eq!(summary("symbols"), "1 symbol");
    assert_eq!(summary("ir"), "0 types · 1 term");
}

/// A parameterized declaration's meaning prints its parameters as `'a`, `'b` —
/// which says nothing on its own about which is which. The Types tab carries a
/// row per parameter mapping each letter back to the name it was written as,
/// and without them the tab is unreadable for exactly the declarations that
/// most need reading.
#[test]
fn the_types_tab_maps_each_letter_back_to_its_parameter() {
    let snap = snapshot("type Pair A B = { first: A, second: B }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage");

    let rows = nodes(stage);
    let pair = rows
        .iter()
        .find(|node| node.label == "type Pair")
        .expect("a row for the declaration");
    assert_eq!(pair.text, "{ first: 'a, second: 'b }");

    let letters: Vec<(&str, &str)> = pair
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(letters, vec![("'a", "A"), ("'b", "B")]);
}

/// The IR tab shows an application as its head and its arguments, and the head
/// carries the declaration's symbol so it cross-highlights with the row that
/// declares it.
#[test]
fn the_ir_tab_takes_an_application_apart() {
    let snap = snapshot("type Box A = { it: A }\ntype N = Box Nat");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "ir")
        .expect("the ir stage");

    let rows = nodes(stage);
    let apply = rows
        .iter()
        .find(|node| node.label == "Apply")
        .expect("a row for the application");
    assert_eq!(apply.text, "Box Nat");

    let kids: Vec<&str> = apply
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(kids, vec!["Head", "Prim"]);
    assert!(
        apply.children[0].symbol.is_some(),
        "the head should cross-highlight: {:#?}",
        apply.children[0]
    );
}

/// The Types tab says which parameters stand for a set of fields, because
/// nothing else on the row does: a row is the only reason a declared type can
/// be open, and the meaning column spells every parameter `'a` alike.
#[test]
fn the_types_tab_says_which_parameters_are_rows() {
    let snap = snapshot("type Both A r = { it: A, ..r }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage");

    let both = nodes(stage)
        .into_iter()
        .find(|node| node.label == "type Both")
        .expect("a row for the declaration");
    let letters: Vec<(&str, &str)> = both
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(letters, vec![("'a", "A"), ("'b", "..r")]);
}

/// A type parameter is a symbol like any other — minted as a local, the way a
/// lambda's argument is — so it reaches the Symbols tab with no special case,
/// and the path it is listed under is one `demangle` can read back.
#[test]
fn a_type_parameter_is_a_symbol_like_any_other() {
    let snap = snapshot("type Pair A B = { first: A, second: B }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage");

    let listed: Vec<&str> = nodes(stage)
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    for name in ["Pair", "A", "B"] {
        assert!(listed.contains(&name), "{name} is missing from {listed:?}");
    }
}
