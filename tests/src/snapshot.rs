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
