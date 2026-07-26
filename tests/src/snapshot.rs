//! Tests for [`ruddy_debug::snapshot`].

use ruddy_debug::{
    snapshot::{compile, guard, install_hook},
    wire::{BundleSpec, CompileRequest, Node, Snapshot, Stage},
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
    assert_eq!(ids, ["tokens", "ast", "ir", "symbols"]);
    assert_eq!(snapshot.revision, 3);
    assert!(snapshot.panic.is_none());
    for stage in &snapshot.stages {
        assert!(!stage.nodes.is_empty(), "{} produced nothing", stage.id);
    }
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

#[test]
fn a_bad_literal_is_a_diagnostic_of_its_own() {
    let codes = |source: &str| {
        snapshot(source)
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>()
    };
    assert_eq!(codes("let n = 1x\n"), ["malformed-natural"]);
    assert_eq!(
        codes(&format!("let n = {}0\n", u128::MAX)),
        ["natural-too-large"]
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

    assert_eq!(back["stages"].as_array().expect("stages").len(), 4);
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
