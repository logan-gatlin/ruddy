//! Runs the compiler over one snippet and turns the result into a [`Snapshot`].
//!
//! The compiler is under active development, so it will panic — on a bad
//! `expect`, an `unreachable!`, or an out-of-range span. Every phase and every
//! stage therefore runs inside `catch_unwind`: a panic becomes a rendered
//! result with a message, a location and a backtrace, the stages that already
//! succeeded are still shown, and neither the server nor the editor's contents
//! are lost.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Once,
    time::Instant,
};

use ruddy::{
    artifact,
    bundle::{self, Files},
    inference, ir, lir, patterns,
    symbol::{Bundle, Mint, Version},
    tracking::{FileID, FileManager},
    ui,
};

use crate::{
    stage::{self, Build, Cx, Phases, Trace},
    wire::{
        CompileRequest, Diagnostic, FileInfo, FileSpec, InferenceCause, Loc, Panic, Related,
        Severity, Snapshot, line_starts, loc, locate,
    },
};

thread_local! {
    /// Filled by the panic hook on the thread that panicked, drained by
    /// [`guard`] immediately after `catch_unwind` returns.
    static LAST_PANIC: RefCell<Option<Panic>> = const { RefCell::new(None) };
    /// Set while a [`guard`] is running. A panic outside one is not the
    /// compiler's — it is a bug in the server, or a failed assertion in a test
    /// — and has to reach the console the way it normally would.
    static GUARDING: Cell<bool> = const { Cell::new(false) };
}

/// The root file of every document, by convention. Its path and the bundle
/// identity are supplied separately from source, just as a project manifest
/// supplies them to the command-line driver.
pub const ROOT: &str = "main.hc";

/// The [`Files`] a request is: whatever the page has in its editor, and nothing
/// off the disk.
///
/// A path the request does not carry simply is not there, which is what makes a
/// missing module file the same complaint in the debugger as at the command
/// line.
struct Requested<'a>(&'a [FileSpec]);

impl Files for Requested<'_> {
    fn read(&self, path: &str) -> Option<String> {
        self.0
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.source.clone())
    }
}

pub fn compile(req: &CompileRequest, build: u64) -> Snapshot {
    compile_inner(req, build, None)
}

/// Compile an active browser project while loading dependency projects from a
/// sandboxed debugger scratch directory.
pub fn compile_at(req: &CompileRequest, build: u64, scratch: &Path) -> Snapshot {
    compile_inner(req, build, Some(scratch))
}

fn compile_inner(req: &CompileRequest, build: u64, scratch: Option<&Path>) -> Snapshot {
    install_hook();

    let mut files = FileManager::new();
    let mut panicked: Option<Panic> = None;
    let mut diagnostics = Vec::new();
    let mut micros = Phases::default();

    let fs = Requested(&req.files);
    let started = Instant::now();
    let loaded = guard("bundle", &mut panicked, || {
        bundle::load(&mut files, &fs, &req.root)
    });
    micros.load = started.elapsed().as_micros() as u64;
    // Lexing and parsing happen inside the load, once per file, so the two tabs
    // that render their output report the sum rather than a figure of their
    // own.
    if let Some(loaded) = &loaded {
        micros.lex = loaded.loaded.iter().map(|file| file.lex_micros).sum();
        micros.parse = loaded.loaded.iter().map(|file| file.parse_micros).sum();
    }

    // What every file the loader read holds, keyed by the id its spans name.
    // Taken once, here, so that a diagnostic can be quoted out of the file it
    // was written in without the file manager being borrowed for the rest of
    // the run.
    let sources: HashMap<FileID, String> = loaded
        .iter()
        .flat_map(|loaded| loaded.loaded.iter())
        .map(|file| (file.id, files.get_file(file.id).content.clone()))
        .collect();

    // Which position in `Snapshot::files` each file the loader read sits at.
    // This is the index every `Loc` on the wire points into, and the loader's
    // own order — root first, then depth-first — is the order the page shows
    // the file strip in.
    let index: HashMap<FileID, u32> = loaded
        .iter()
        .flat_map(|loaded| loaded.loaded.iter())
        .enumerate()
        .map(|(at, file)| (file.id, at as u32))
        .collect();

    // A request with no root has nothing to load. Said here rather than by the
    // loader, which reads an absent file as an empty one: only the debugger
    // knows that the page is meant to have put one there.
    if fs.read(&req.root).is_none() {
        let mut diagnostic = raw(
            "bundle",
            "project-root-missing",
            format!("this document needs its root file `{}`", req.root),
            None,
        );
        diagnostic
            .help
            .push(format!("create `{}` or choose another root file", req.root));
        diagnostics.push(diagnostic);
    }

    // Identity is project configuration, not source syntax. Keep malformed
    // wire input recoverable like every compiler error: report it, mint under
    // a stable fallback, and still show all later phases that can run.
    let parsed_version = Version::parse(&req.version).ok();
    let configured = parsed_version
        .clone()
        .filter(|version| version.build.is_empty())
        .and_then(|version| Bundle::new(&req.name, version));
    if configured.is_none() {
        let mut diagnostic = if parsed_version.is_none() {
            raw(
                "bundle",
                "project-version-invalid",
                format!("`{}` is not a valid project version", req.version),
                None,
            )
        } else if parsed_version
            .as_ref()
            .is_some_and(|version| !version.build.is_empty())
        {
            raw(
                "bundle",
                "project-version-build-suffix",
                format!(
                    "project version `{}` has an unsupported `+` suffix",
                    req.version
                ),
                None,
            )
        } else {
            raw(
                "bundle",
                "project-name-invalid",
                format!("`{}` is not a valid project name", req.name),
                None,
            )
        };
        diagnostic.help.push(if parsed_version.is_none() {
            "use three numbers such as `1.2.3`".to_string()
        } else if parsed_version
            .as_ref()
            .is_some_and(|version| !version.build.is_empty())
        {
            "remove the `+...` suffix from the version".to_string()
        } else {
            "start with an ASCII letter and use only ASCII letters, digits, `-`, or `_`".to_string()
        });
        diagnostics.push(diagnostic);
    }
    let identity = configured.as_ref().map(ToString::to_string);

    // Resolve and compile saved dependency projects, including the standard
    // library synthesized ahead of explicit declarations. Configuration
    // failures are recoverable so source, token, and AST views remain
    // inspectable, but semantic phases wait for the requested imports.
    let dependency_started = Instant::now();
    let mut dependency_artifacts = Vec::new();
    let mut dependency_aliases = Vec::new();
    let mut dependency_interfaces = Vec::new();
    let mut linked_interfaces = Vec::new();
    if !req.std.is_disabled() || !req.dependencies.is_empty() {
        match scratch {
            None => {
                let mut diagnostic = raw(
                    "dependencies",
                    "dependency-workspace-missing",
                    "saved dependencies need a document workspace".to_string(),
                    None,
                );
                diagnostic.help.push(
                    "open the debugger with a scratch directory before adding dependencies"
                        .to_string(),
                );
                diagnostics.push(diagnostic);
            }
            Some(scratch) => {
                let project = match crate::docs::path(scratch, &req.document) {
                    Some(project) => project,
                    None => {
                        let mut diagnostic = raw(
                            "dependencies",
                            "document-name-invalid",
                            "this document name cannot be used for saved dependencies".to_string(),
                            None,
                        );
                        diagnostic.help.push(
                            "rename the document using letters, digits, `-`, or `_`".to_string(),
                        );
                        diagnostics.push(diagnostic);
                        PathBuf::from("invalid-document")
                    }
                };
                let specifications = req
                    .dependencies
                    .iter()
                    .map(|(alias, specification)| (alias.clone(), specification.clone()));
                match ruddy_cli::compile_sandboxed_project_dependencies(
                    &req.std,
                    specifications,
                    &project,
                    scratch,
                ) {
                    Ok((graph, direct, direct_paths)) => {
                        dependency_aliases = if req.std.is_disabled() {
                            Vec::new()
                        } else {
                            vec!["std".to_string()]
                        };
                        dependency_aliases.extend(req.dependencies.keys().cloned());
                        linked_interfaces = graph
                            .projects
                            .iter()
                            .map(|project| project.artifact.clone())
                            .collect();
                        // Select source-visible roots by their canonical graph
                        // path, not by the first matching bundle identity.
                        dependency_interfaces = direct_paths
                            .iter()
                            .filter_map(|path| {
                                graph
                                    .projects
                                    .iter()
                                    .find(|project| &project.directory == path)
                            })
                            .map(|project| project.artifact.clone())
                            .collect();
                        dependency_artifacts = direct;
                    }
                    Err(error) => diagnostics.extend(
                        error
                            .into_diagnostics()
                            .into_iter()
                            .map(dependency_diagnostic),
                    ),
                }
            }
        }
    }
    micros.dependencies = dependency_started.elapsed().as_micros() as u64;

    let fallback = || {
        Bundle::new("fallback", Version::new(0, 0, 0))
            .expect("the debugger fallback identity is valid")
    };
    let mut mint = Mint::new(configured.unwrap_or_else(fallback));

    let dependencies_valid = !diagnostics
        .iter()
        .any(|diagnostic| diagnostic.stage == "dependencies");
    let frontend_clean = loaded.as_ref().is_some_and(|loaded| {
        loaded.errors.is_empty()
            && loaded
                .loaded
                .iter()
                .all(|file| file.lex_errors.is_empty() && file.parse_errors.is_empty())
    });

    let mut built = loaded
        .as_ref()
        .filter(|_| frontend_clean && dependencies_valid)
        .and_then(|loaded| {
            let started = Instant::now();
            let out = guard("ir", &mut panicked, || {
                let imports: Vec<_> = dependency_aliases
                    .iter()
                    .zip(&dependency_interfaces)
                    .map(|(alias, artifact)| ir::DependencyImport { alias, artifact })
                    .collect();
                ir::build_with_dependency_imports(
                    &mut mint,
                    loaded.stmts.clone(),
                    &imports,
                    &linked_interfaces,
                )
            });
            micros.build = started.elapsed().as_micros() as u64;
            out
        });

    // Inference mutates the program it types, so it borrows `built` mutably
    // and finishes before any stage looks at either.
    let inferred = built.as_mut().and_then(|built| {
        let started = Instant::now();
        let out = guard("types", &mut panicked, || {
            inference::infer(&mint, &mut built.program)
        });
        micros.infer = started.elapsed().as_micros() as u64;
        out
    });

    // The pattern checks read the program inference just finished writing
    // solved types into, and run only when both phases did.
    let checked = match (&built, &inferred) {
        (Some(built), Some(inferred)) => {
            let started = Instant::now();
            let out = guard("patterns", &mut panicked, || {
                patterns::check(&built.program, inferred)
            });
            micros.patterns = started.elapsed().as_micros() as u64;
            out
        }
        _ => None,
    };

    // Every phase words and codes its own errors in `ruddy::ui`, so the strip
    // and the CLI driver cannot describe the same program differently, and a
    // new error kind reaches both the moment it exists. This layer only maps
    // the shared diagnostic's spans into debugger file locations.
    if let Some(loaded) = &loaded {
        for file in &loaded.loaded {
            let mut source_errors: Vec<_> = file
                .lex_errors
                .iter()
                .map(|error| source_error("lex", error.diagnostic(), &index))
                .chain(
                    file.parse_errors
                        .iter()
                        .map(|error| source_error("parse", error.diagnostic(), &index)),
                )
                .collect();
            source_errors.sort_by_key(|error| error.span.map_or(usize::MAX, |span| span.range[0]));
            diagnostics.extend(source_errors);
        }
        diagnostics.extend(
            loaded
                .errors
                .iter()
                .map(|error| source_error("bundle", error.diagnostic(), &index)),
        );
    }
    if let Some(built) = &built {
        diagnostics.extend(
            built
                .errors
                .iter()
                .map(|error| source_error("ir", error.diagnostic(), &index)),
        );
    }
    if let Some(inferred) = &inferred {
        diagnostics.extend(inferred.errors.iter().map(|error| {
            let mut diagnostic = source_error("types", error.diagnostic(), &index);
            diagnostic.inference_error_id = Some(error.id.get());
            diagnostic.inference_cause = Some(match error.cause {
                inference::ErrorCause::Step(id) => InferenceCause::Step { step_id: id.get() },
                inference::ErrorCause::Batch(id) => InferenceCause::Batch { batch_id: id.get() },
                inference::ErrorCause::Direct => InferenceCause::Direct,
            });
            diagnostic.inference_explanation = error
                .explanation
                .as_ref()
                .map(|explanation| wire_explanation(explanation, &index));
            diagnostic
        }));
    }
    if let Some(checked) = &checked {
        diagnostics.extend(checked.errors.iter().map(|error| {
            raw(
                "patterns",
                error.kind.code(),
                error.kind.to_string(),
                loc(error.span, &index),
            )
        }));
    }

    // Sorted by where the reader would look for them — file first, then offset
    // — and numbered, so a diagnostic's id matches its position in the strip.
    diagnostics.sort_by_key(|d| d.span.map(|at| (at.file, at.range[0])).unwrap_or((0, 0)));
    for (i, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic.id = i as u32;
    }

    // Lowering to LIR runs on an accepted program and nothing else, which is
    // the rule `main.rs` follows too — so it is gated on the whole diagnostic
    // list rather than on any one phase having produced a value. Its tab
    // reports `Skipped` the moment the reader types something wrong.
    let lowered = match (&built, &inferred, &checked, diagnostics.is_empty()) {
        (Some(built), Some(inferred), Some(_), true) => {
            let started = Instant::now();
            let out = guard("lir", &mut panicked, || {
                lir::lower(&mint, &built.program, inferred)
            });
            micros.lir = started.elapsed().as_micros() as u64;
            out
        }
        _ => None,
    };

    // The artifact is the first disk-boundary representation. Like LIR, it
    // only exists for an accepted program, and retains no source spans.
    let mut artifact_panicked = false;
    let artifact = match (&built, &inferred, &lowered) {
        (Some(built), Some(inferred), Some(lowered)) => {
            let started = Instant::now();
            let dependencies = dependency_artifacts.clone();
            let out = guard("artifact", &mut panicked, || {
                artifact::build_with_dependencies(
                    &mint,
                    &built.program,
                    inferred,
                    lowered,
                    dependencies,
                )
            });
            artifact_panicked = out.is_none();
            micros.artifact = started.elapsed().as_micros() as u64;
            out
        }
        _ => None,
    };

    // Linking is a distinct final phase. Dependency artifacts remain separate
    // compilation boundaries above; this output copies their code into the
    // active root and therefore needs no external artifacts at execution time.
    let errored_before_link = !diagnostics.is_empty();
    let mut link_error = None;
    let mut link_panicked = false;
    let linked = artifact.as_ref().and_then(|artifact| {
        let started = Instant::now();
        let mut graph = linked_interfaces.clone();
        graph.push(artifact.clone());
        let out = guard("link", &mut panicked, || ruddy::link::link(&graph));
        link_panicked = out.is_none();
        micros.link = started.elapsed().as_micros() as u64;
        match out {
            Some(Ok(linked)) => Some(linked),
            Some(Err(error)) => {
                let message = error.to_string();
                diagnostics.push(raw("link", "invalid-artifact-graph", message.clone(), None));
                link_error = Some(message);
                None
            }
            None => None,
        }
    });
    diagnostics.sort_by_key(|d| d.span.map(|at| (at.file, at.range[0])).unwrap_or((0, 0)));
    for (i, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic.id = i as u32;
    }

    // Code generation is always exposed by the debugger, independently of a
    // project's manifest target. It consumes the same linked, in-memory root
    // artifact as the command-line backend and is guarded like every other
    // compiler phase.
    let mut js_error = None;
    let mut js_panicked = false;
    let js = linked.as_ref().and_then(|linked| {
        let started = Instant::now();
        let out = guard("js", &mut panicked, || ruddy::backend::js::generate(linked));
        js_panicked = out.is_none();
        micros.js = started.elapsed().as_micros() as u64;
        match out {
            Some(Ok(source)) => Some(source),
            Some(Err(error)) => {
                let message = error.to_string();
                diagnostics.push(raw("js", "javascript-generation", message.clone(), None));
                js_error = Some(message);
                None
            }
            None => None,
        }
    });
    diagnostics.sort_by_key(|d| d.span.map(|at| (at.file, at.range[0])).unwrap_or((0, 0)));
    for (i, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic.id = i as u32;
    }

    // Every file the loader read, in load order, with what the page needs to
    // turn any `Loc` into a line and a column. Built from the loader's own list
    // rather than from the request, so a file no module declares is not in it
    // and a file that could not be read still is.
    let stage_sources: Vec<String> = loaded
        .iter()
        .flat_map(|loaded| loaded.loaded.iter())
        .map(|file| sources.get(&file.id).cloned().unwrap_or_default())
        .collect();
    let infos: Vec<FileInfo> = loaded
        .iter()
        .flat_map(|loaded| loaded.loaded.iter())
        .zip(&stage_sources)
        .map(|(file, content)| FileInfo {
            path: file.path.clone(),
            len: content.len(),
            line_starts: line_starts(content),
        })
        .collect();

    let symbols = stage::symbols::index(&mint);
    let cx = Cx {
        files: &infos,
        sources: &stage_sources,
        diagnostics: &diagnostics,
        bundle: loaded.as_ref(),
        program: built.as_ref().map(|built| &built.program),
        inference: inferred.as_ref(),
        patterns: checked.as_ref(),
        lir: lowered.as_ref(),
        artifact: artifact.as_ref(),
        linked: linked.as_ref(),
        js: js.as_deref(),
        js_error: js_error.as_deref(),
        js_panicked,
        standard_library: &req.std,
        dependency_declarations: &req.dependencies,
        dependency_aliases: &dependency_aliases,
        dependencies: &dependency_artifacts,
        dependency_interfaces: &dependency_interfaces,
        dependencies_valid: !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.stage == "dependencies"),
        artifact_panicked,
        link_error: link_error.as_deref(),
        link_panicked,
        mint: built.as_ref().map(|_| &mint),
        symbols: &symbols,
        micros,
        // A link-only diagnostic belongs to the final tab and must not
        // retroactively downgrade compiler phases that already succeeded.
        errored: errored_before_link,
    };

    // In registry order, which is also dependency order: a stage that annotates
    // another is registered after it, so the trace it reads is already there.
    let mut traces: HashMap<&'static str, Trace> = HashMap::new();
    let mut stages = Vec::with_capacity(stage::REGISTRY.len());
    for spec in stage::REGISTRY {
        let built = match spec.build {
            Build::Panel(build) => guard(spec.id, &mut panicked, || build(spec, &cx)),
            Build::Traced(build) => {
                guard(spec.id, &mut panicked, || build(spec, &cx)).map(|(stage, trace)| {
                    traces.insert(spec.id, trace);
                    stage
                })
            }
            Build::Annotator(build) => {
                let empty = Trace::default();
                let trace = spec
                    .annotates
                    .and_then(|id| traces.get(id))
                    .unwrap_or(&empty);
                guard(spec.id, &mut panicked, || build(spec, &cx, trace))
            }
        };
        stages.push(built.unwrap_or_else(|| stage::panicked(spec)));
    }
    // Every node's span becomes a position on the wire in one pass, now that
    // the file index is settled. See [`Node::at`].
    locate(&mut stages, &index);

    Snapshot {
        revision: req.revision,
        build,
        files: infos,
        bundle: identity,
        stages,
        diagnostics,
        panic: panicked,
    }
}

/// Run one phase, converting a panic into a recorded [`Panic`] and a missing
/// result. Only the first panic of a run is kept: the ones after it are usually
/// the same bug seen from a stage that was handed nothing.
pub fn guard<T>(stage: &str, slot: &mut Option<Panic>, f: impl FnOnce() -> T) -> Option<T> {
    let outer = GUARDING.replace(true);
    let caught = panic::catch_unwind(AssertUnwindSafe(f));
    GUARDING.set(outer);
    match caught {
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
/// backtrace nobody asked for. Only the ones a [`guard`] is around: the same
/// information reaches the page, which is where it is useful. Anything else is
/// nobody's compiler bug and falls through to the hook that was already there.
pub fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let outer = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !GUARDING.with(Cell::get) {
                outer(info);
                return;
            }
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

fn wire_explanation(
    explanation: &inference::InferenceExplanation,
    files: &HashMap<FileID, u32>,
) -> crate::wire::InferenceExplanation {
    fn description(value: inference::TypeDescription) -> &'static str {
        use inference::TypeDescription as T;
        match value {
            T::NaturalNumber => "natural-number",
            T::Integer => "integer",
            T::RealNumber => "real-number",
            T::Text => "text",
            T::Boolean => "boolean",
            T::Function => "function",
            T::Struct => "struct",
            T::TaggedValue => "tagged-value",
            T::DeclaredType => "declared-type",
            T::Undecided => "undecided",
        }
    }
    fn fact(
        fact: &inference::ExplanationFact,
        files: &HashMap<FileID, u32>,
    ) -> crate::wire::ExplanationFact {
        let payload = match fact.payload {
            inference::ExplanationFactPayload::RequiresType => "requires-type",
            inference::ExplanationFactPayload::UsedAsFunction => "used-as-function",
            inference::ExplanationFactPayload::SuppliesArgument => "supplies-argument",
            inference::ExplanationFactPayload::BranchResult => "branch-result",
            inference::ExplanationFactPayload::LabelDemand => "label-demand",
            inference::ExplanationFactPayload::ClosedRow => "closed-row",
            inference::ExplanationFactPayload::LabelIntroduction => "label-introduction",
            inference::ExplanationFactPayload::LabelForbidden => "label-forbidden",
            inference::ExplanationFactPayload::CallerChoiceDeclaration => {
                "caller-choice-declaration"
            }
            inference::ExplanationFactPayload::CallerChoiceUse => "caller-choice-use",
            inference::ExplanationFactPayload::CallerChoiceDestination => {
                "caller-choice-destination"
            }
            inference::ExplanationFactPayload::EffectUse => "effect-use",
            inference::ExplanationFactPayload::EffectBoundary => "effect-boundary",
            inference::ExplanationFactPayload::EffectDeclaration => "effect-declaration",
        };
        crate::wire::ExplanationFact {
            span: loc(fact.span, files),
            constraint_id: fact.constraint.get(),
            origin: fact.origin.code(),
            subject: fact.subject.code(),
            payload,
        }
    }
    let full: Vec<_> = explanation
        .full_facts
        .iter()
        .map(|item| fact(item, files))
        .collect();
    let abridged = explanation
        .abridged
        .iter()
        .filter_map(|at| full.get(*at).cloned())
        .collect();
    let contradiction = &explanation.contradiction;
    crate::wire::InferenceExplanation {
        abridged,
        full,
        pivot: explanation
            .pivot
            .as_ref()
            .map(|pivot| crate::wire::ExplanationPivot {
                name: pivot.name.clone(),
                kind: match pivot.kind {
                    inference::ExplanationPivotKind::WrittenValue => "written-value",
                    inference::ExplanationPivotKind::FunctionInput => "function-input",
                    inference::ExplanationPivotKind::BranchResult => "branch-result",
                    inference::ExplanationPivotKind::ProjectedField => "projected-field",
                    inference::ExplanationPivotKind::FunctionEffects => "function-effects",
                    inference::ExplanationPivotKind::Value => "value",
                },
                references: pivot.references.clone(),
                introduced_at: pivot.introduced_at,
            }),
        omitted_facts: explanation.omitted_facts,
        contradiction: crate::wire::ExplanationContradiction {
            kind: match contradiction.kind {
                inference::ContradictionKind::IncompatibleTypes => "incompatible-types",
                inference::ContradictionKind::ValueUsedAsFunction => "value-used-as-function",
                inference::ContradictionKind::RecursiveValue => "recursive-value",
                inference::ContradictionKind::ProjectionOnNonStruct => "projection-on-non-struct",
                inference::ContradictionKind::LabelUnavailable => "label-unavailable",
                inference::ContradictionKind::RepeatedLabel => "repeated-label",
                inference::ContradictionKind::CallerChoice => "caller-choice",
                inference::ContradictionKind::CallerChoiceEscape => "caller-choice-escape",
                inference::ContradictionKind::UnhandledEffect => "unhandled-effect",
                inference::ContradictionKind::EffectNotAllowed => "effect-not-allowed",
            },
            left: description(contradiction.left),
            right: description(contradiction.right),
            row: contradiction
                .row
                .as_ref()
                .map(|row| crate::wire::ExplanationRow {
                    shape: match row.shape {
                        ruddy::types::Shape::Struct => "struct",
                        ruddy::types::Shape::Sum => "sum",
                        ruddy::types::Shape::Effect => "effect",
                    },
                    label: row.label.clone(),
                }),
            repairs: contradiction.repairs.map(|repair| match repair {
                inference::RepairDirection::ChangeFirstUse => "change-first-use",
                inference::RepairDirection::ChangeSecondUse => "change-second-use",
            }),
        },
        cause: crate::wire::ExplanationCause {
            error_id: explanation.cause.error.get(),
            seed_reason_id: explanation.cause.seed.map(|id| id.get()),
            constraint_ids: explanation
                .cause
                .constraints
                .iter()
                .map(|id| id.get())
                .collect(),
            reason_ids: explanation
                .cause
                .reasons
                .iter()
                .map(|id| id.get())
                .collect(),
            omitted_reasons: explanation.cause.omitted_reasons,
        },
    }
}

fn source_error(
    stage: &'static str,
    source_diagnostic: ui::Diagnostic,
    files: &HashMap<FileID, u32>,
) -> Diagnostic {
    let mut diagnostic = raw(
        stage,
        source_diagnostic.code,
        source_diagnostic.title,
        loc(source_diagnostic.primary.span, files),
    );
    diagnostic.label = source_diagnostic.primary.message;
    diagnostic.help = source_diagnostic.help;
    diagnostic.notes = source_diagnostic.notes;
    diagnostic.related = source_diagnostic
        .related
        .into_iter()
        .filter(|annotation| !annotation.span.is_generated())
        .map(|annotation| Related {
            span: loc(annotation.span, files),
            message: annotation.message,
        })
        .collect();
    diagnostic
}

fn dependency_diagnostic(report: ruddy_cli::CompileDiagnostic) -> Diagnostic {
    let code = if report.code() == "project-error" {
        "dependency-build"
    } else {
        report.code()
    };
    Diagnostic {
        id: 0,
        inference_error_id: None,
        inference_cause: None,
        inference_explanation: None,
        stage: "dependencies",
        severity: Severity::Error,
        code,
        message: report.message().to_string(),
        label: String::new(),
        help: report.help().to_vec(),
        notes: report.notes().to_vec(),
        report: Some(report),
        span: None,
        related: Vec::new(),
    }
}

fn raw(stage: &'static str, code: &'static str, message: String, span: Option<Loc>) -> Diagnostic {
    Diagnostic {
        id: 0,
        inference_error_id: None,
        inference_cause: None,
        inference_explanation: None,
        stage,
        severity: Severity::Error,
        code,
        message,
        label: String::new(),
        help: Vec::new(),
        notes: Vec::new(),
        report: None,
        span,
        related: Vec::new(),
    }
}
