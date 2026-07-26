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
pub fn guard<T>(stage: &str, slot: &mut Option<Panic>, f: impl FnOnce() -> T) -> Option<T> {
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
pub fn install_hook() {
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
