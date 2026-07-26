//! Runs the compiler over one snippet and turns the result into a [`Snapshot`].
//!
//! The compiler is under active development, so it will panic — on a bad
//! `expect`, an `unreachable!`, or an out-of-range span. Every phase and every
//! stage therefore runs inside `catch_unwind`: a panic becomes a rendered
//! result with a message, a location and a backtrace, the stages that already
//! succeeded are still shown, and neither the server nor the editor's contents
//! are lost.

use std::{
    cell::RefCell,
    panic::{self, AssertUnwindSafe},
    sync::Once,
    time::Instant,
};

use ruddy::{
    ir, parse,
    symbol::{Bundle, Mint, Namespace, Version},
    token,
    tracking::{FileManager, Span},
};

use crate::{
    stage::{self, Cx, Phases},
    wire::{CompileRequest, Diagnostic, Panic, Range, Related, Severity, Snapshot, line_starts},
};

thread_local! {
    /// Filled by the panic hook on the thread that panicked, drained by
    /// [`guard`] immediately after `catch_unwind` returns.
    static LAST_PANIC: RefCell<Option<Panic>> = const { RefCell::new(None) };
}

/// The name the editor's buffer is registered under. It is never read from
/// disk; only spans care that the file exists.
const SCRATCH: &str = "<editor>";

pub fn compile(req: &CompileRequest, build: u64) -> Snapshot {
    install_hook();

    let source = &req.source;
    let mut files = FileManager::new();
    let file_id = files.register_new_file(SCRATCH.to_string(), source.clone());

    let mut panicked: Option<Panic> = None;
    let mut diagnostics = Vec::new();
    let mut micros = Phases::default();

    let started = Instant::now();
    let lexed = guard("lex", &mut panicked, || token::lex(source, file_id));
    micros.lex = started.elapsed().as_micros() as u64;

    let parsed = lexed.as_ref().and_then(|lexed| {
        let started = Instant::now();
        let out = guard("parse", &mut panicked, || {
            parse::parse(lexed.tokens.clone())
        });
        micros.parse = started.elapsed().as_micros() as u64;
        out
    });

    let bundle = match Version::parse(&req.bundle.version)
        .ok()
        .and_then(|version| Bundle::new(&req.bundle.name, version))
    {
        Some(bundle) => bundle,
        None => {
            diagnostics.push(raw(
                "bundle",
                "bad-bundle",
                format!(
                    "`{}@{}` is not a valid bundle identity; falling back to demo@0.1.0",
                    req.bundle.name, req.bundle.version
                ),
                None,
            ));
            Bundle::new("demo", Version::new(0, 1, 0)).expect("the fallback bundle is valid")
        }
    };
    let mut mint = Mint::new(bundle);

    let built = parsed.as_ref().and_then(|parsed| {
        let started = Instant::now();
        let out = guard("ir", &mut panicked, || {
            ir::build(&mut mint, parsed.stmts.clone())
        });
        micros.build = started.elapsed().as_micros() as u64;
        out
    });

    if let Some(lexed) = &lexed {
        diagnostics.extend(
            lexed
                .errors
                .iter()
                .map(|error| lex_diagnostic(source, error)),
        );
    }
    if let Some(parsed) = &parsed {
        diagnostics.extend(parsed.errors.iter().map(|error| {
            raw(
                "parse",
                "unexpected-token",
                format!("unexpected token {}", quote(source, error.span)),
                Some(error.span),
            )
        }));
    }
    if let Some(built) = &built {
        diagnostics.extend(
            built
                .errors
                .iter()
                .map(|error| ir_diagnostic(source, error)),
        );
    }

    // Sorted by where the reader would look for them, then numbered, so a
    // diagnostic's id matches its position in the strip.
    diagnostics.sort_by_key(|d| d.span.map(|s| s[0]).unwrap_or(0));
    for (i, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic.id = i as u32;
    }

    let symbols = stage::symbols::index(&mint);
    let cx = Cx {
        source,
        tokens: lexed.as_ref().map(|lexed| lexed.tokens.as_slice()),
        stmts: parsed.as_ref().map(|parsed| parsed.stmts.as_slice()),
        program: built.as_ref().map(|built| &built.program),
        mint: built.as_ref().map(|_| &mint),
        symbols: &symbols,
        micros,
        errored: !diagnostics.is_empty(),
    };

    let stages = stage::REGISTRY
        .iter()
        .map(|(id, build)| {
            guard(id, &mut panicked, || build(&cx)).unwrap_or_else(|| {
                stage::panicked(id, stage::title_of(id), crate::wire::View::Tree)
            })
        })
        .collect();

    Snapshot {
        revision: req.revision,
        build,
        source_len: source.len(),
        line_starts: line_starts(source),
        stages,
        diagnostics,
        panic: panicked,
    }
}

/// Run one phase, converting a panic into a recorded [`Panic`] and a missing
/// result. Only the first panic of a run is kept: the ones after it are usually
/// the same bug seen from a stage that was handed nothing.
fn guard<T>(stage: &str, slot: &mut Option<Panic>, f: impl FnOnce() -> T) -> Option<T> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            let captured = LAST_PANIC.with(|last| last.borrow_mut().take());
            if slot.is_none() {
                let mut panicked = captured.unwrap_or_else(|| Panic {
                    stage: String::new(),
                    message: message_of(&payload),
                    location: "unknown".to_string(),
                    backtrace: String::new(),
                });
                panicked.stage = stage.to_string();
                eprintln!(
                    "  panic in {stage}: {} ({})",
                    panicked.message, panicked.location
                );
                *slot = Some(panicked);
            }
            None
        }
    }
}

fn message_of(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked".to_string())
}

/// Capture panics instead of letting them reach the console with a full
/// backtrace nobody asked for. The default hook is replaced rather than
/// chained: the same information reaches the page, which is where it is useful.
fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let panicked = Panic {
                stage: String::new(),
                message: info.payload_as_str().unwrap_or("panicked").to_string(),
                location: info
                    .location()
                    .map(|location| location.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                backtrace: std::backtrace::Backtrace::force_capture().to_string(),
            };
            LAST_PANIC.with(|last| *last.borrow_mut() = Some(panicked));
        }));
    });
}

fn lex_diagnostic(source: &str, error: &token::Error) -> Diagnostic {
    let (code, what) = match error.kind {
        token::ErrorKind::Unrecognized => ("unrecognized-character", "unrecognized character"),
        token::ErrorKind::MalformedNatural => ("malformed-natural", "malformed natural number"),
        token::ErrorKind::NaturalTooLarge => (
            "natural-too-large",
            "natural number too large to fit in 128 bits",
        ),
    };
    raw(
        "lex",
        code,
        format!("{what} {}", quote(source, error.span)),
        Some(error.span),
    )
}

fn ir_diagnostic(source: &str, error: &ir::Error) -> Diagnostic {
    match &error.kind {
        ir::ErrorKind::Undefined { namespace } => raw(
            "ir",
            match namespace {
                Namespace::Types => "undefined-type",
                _ => "undefined-term",
            },
            format!("undefined {namespace} {}", quote(source, error.span)),
            Some(error.span),
        ),
        ir::ErrorKind::Duplicate {
            namespace,
            previous,
        } => {
            let mut diagnostic = raw(
                "ir",
                match namespace {
                    Namespace::Types => "duplicate-type",
                    _ => "duplicate-term",
                },
                format!("duplicate {namespace} {}", quote(source, error.span)),
                Some(error.span),
            );
            // One diagnostic with two highlights, rather than the two loose
            // lines the CLI prints: the repeat is only legible next to what it
            // repeats.
            diagnostic.related.push(Related {
                span: range(*previous),
                message: "first defined here".to_string(),
            });
            diagnostic
        }
        ir::ErrorKind::DuplicateField => raw(
            "ir",
            "duplicate-field",
            format!("duplicate field {}", quote(source, error.span)),
            Some(error.span),
        ),
    }
}

fn raw(stage: &'static str, code: &'static str, message: String, span: Option<Span>) -> Diagnostic {
    Diagnostic {
        id: 0,
        stage,
        severity: Severity::Error,
        code,
        message,
        span: span.and_then(range),
        related: Vec::new(),
    }
}

fn range(span: Span) -> Option<Range> {
    match span.is_generated() {
        true => None,
        false => Some([span.start, span.end()]),
    }
}

/// The source text a span covers, quoted for a message.
fn quote(source: &str, span: Span) -> String {
    match source.get(span.start..span.end()) {
        Some("") | None => "at end of input".to_string(),
        Some(text) => format!("`{text}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{BundleSpec, Node, Stage};

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
}
