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
    collections::{HashMap, HashSet},
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
    tracking::{FileID, FileManager, Span},
    ui,
};

use crate::{
    stage::{self, Build, Cx, Phases, Trace},
    wire::{
        CompileRequest, Diagnostic, FileInfo, FileSpec, Loc, Panic, Related, Severity, Snapshot,
        line_starts, loc, locate,
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
        diagnostics.push(raw(
            "bundle",
            "missing-root-file",
            if req.root == ROOT {
                format!("a document needs a `{ROOT}`; it is the bundle's root file")
            } else {
                format!("a document needs its configured root `{}`", req.root)
            },
            None,
        ));
    }

    // Identity is project configuration, not source syntax. Keep malformed
    // wire input recoverable like every compiler error: report it, mint under
    // a stable fallback, and still show all later phases that can run.
    let configured = Version::parse(&req.version)
        .ok()
        .filter(|version| version.build.is_empty())
        .and_then(|version| Bundle::new(&req.name, version));
    if configured.is_none() {
        diagnostics.push(raw(
            "bundle",
            "bad-bundle-identity",
            format!(
                "`{}@{}` is not a valid Ruddy bundle identity",
                req.name, req.version
            ),
            None,
        ));
    }
    let identity = configured.as_ref().map(ToString::to_string);

    // Resolve and compile saved dependency projects. Configuration failures are
    // recoverable so the active project's source phases remain inspectable.
    let dependency_started = Instant::now();
    let mut dependency_artifacts = Vec::new();
    let mut dependency_aliases = Vec::new();
    let mut dependency_interfaces = Vec::new();
    let mut linked_interfaces = Vec::new();
    if !req.dependencies.is_empty() {
        match scratch {
            None => diagnostics.push(raw(
                "dependencies",
                "missing-scratch-root",
                "dependency projects require a debugger scratch root".to_string(),
                None,
            )),
            Some(scratch) => {
                let project = match crate::docs::path(scratch, &req.document) {
                    Some(project) => project,
                    None => {
                        diagnostics.push(raw(
                            "dependencies",
                            "bad-document",
                            "the active document name is invalid".to_string(),
                            None,
                        ));
                        PathBuf::from("invalid-document")
                    }
                };
                let mut active = HashSet::new();
                let mut completed = HashSet::new();
                let mut resolved = Vec::new();
                for (alias, specification) in &req.dependencies {
                    let package = specification.package(alias);
                    let declared = specification.path();
                    let declared_path = Path::new(declared);
                    match crate::docs::dependency_path(scratch, &project, declared_path).and_then(
                        |path| {
                            validate_sandbox_graph(scratch, &path, &mut active, &mut completed)
                                .map(|()| path)
                        },
                    ) {
                        Ok(path) => resolved.push((alias.clone(), package.to_string(), path)),
                        Err(error) => diagnostics.push(raw(
                            "dependencies",
                            "dependency-path",
                            format!("dependency `{alias}` at `{declared}`: {error}"),
                            None,
                        )),
                    }
                }
                let direct_paths: Vec<_> = resolved
                    .iter()
                    .map(|(_, _, path)| {
                        std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())
                    })
                    .collect();
                dependency_aliases = resolved.iter().map(|(alias, _, _)| alias.clone()).collect();
                match ruddy_cli::compile_sandboxed_aliased_dependency_graph(resolved, scratch) {
                    Ok((graph, direct)) => {
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
                    Err(error) => diagnostics.push(raw(
                        "dependencies",
                        "dependency-build",
                        error.to_string(),
                        None,
                    )),
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

    let mut built = loaded.as_ref().and_then(|loaded| {
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
    // new error kind reaches both the moment it exists. What is added here is
    // what only the strip has: the quoted snippet, and the second span a
    // duplicate points back at.
    //
    // The source a span is quoted out of is the file it was written in, which
    // the loader registered and the file manager still holds — so a complaint
    // about a module file quotes that file rather than whatever the editor has
    // in front of it.
    let source = |span: Span| sources.get(&span.file_id).cloned().unwrap_or_default();
    if let Some(loaded) = &loaded {
        for file in &loaded.loaded {
            diagnostics.extend(file.lex_errors.iter().map(|error| {
                raw(
                    "lex",
                    error.kind.code(),
                    format!("{} {}", error.kind, quote(&source(error.span), error.span)),
                    loc(error.span, &index),
                )
            }));
            diagnostics.extend(file.parse_errors.iter().map(|error| {
                raw(
                    "parse",
                    error.code(),
                    format!("{error} {}", quote(&source(error.span), error.span)),
                    loc(error.span, &index),
                )
            }));
        }
        diagnostics.extend(loaded.errors.iter().map(|error| {
            raw(
                "bundle",
                error.kind.code(),
                error.kind.to_string(),
                loc(error.span, &index),
            )
        }));
    }
    if let Some(built) = &built {
        diagnostics.extend(
            built
                .errors
                .iter()
                .map(|error| ir_diagnostic(&source(error.span), error, &index)),
        );
    }
    if let Some(inferred) = &inferred {
        diagnostics.extend(
            inferred
                .errors
                .iter()
                .map(|error| inference_diagnostic(error, &index)),
        );
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

    // Every file the loader read, in load order, with what the page needs to
    // turn any `Loc` into a line and a column. Built from the loader's own list
    // rather than from the request, so a file no module declares is not in it
    // and a file that could not be read still is.
    let infos: Vec<FileInfo> = loaded
        .iter()
        .flat_map(|loaded| loaded.loaded.iter())
        .map(|file| {
            let content = source(file.id.span(0, 0));
            FileInfo {
                path: file.path.clone(),
                len: content.len(),
                line_starts: line_starts(&content),
            }
        })
        .collect();

    let symbols = stage::symbols::index(&mint);
    let cx = Cx {
        files: &infos,
        bundle: loaded.as_ref(),
        program: built.as_ref().map(|built| &built.program),
        inference: inferred.as_ref(),
        patterns: checked.as_ref(),
        lir: lowered.as_ref(),
        artifact: artifact.as_ref(),
        dependency_declarations: &req.dependencies,
        dependencies: &dependency_artifacts,
        dependency_interfaces: &dependency_interfaces,
        dependencies_valid: !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.stage == "dependencies"),
        artifact_panicked,
        mint: built.as_ref().map(|_| &mint),
        symbols: &symbols,
        micros,
        errored: !diagnostics.is_empty(),
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

/// Lowering's errors, which are the only ones with a second place to point at.
fn ir_diagnostic(source: &str, error: &ir::Error, files: &HashMap<FileID, u32>) -> Diagnostic {
    let mut diagnostic = raw(
        "ir",
        error.kind.code(),
        format!("{} {}", error.kind, quote(source, error.span)),
        loc(error.span, files),
    );
    // One diagnostic with two highlights, rather than the two loose lines the
    // CLI prints: the repeat is only legible next to what it repeats. Both of
    // lowering's repeats carry the span, a name given two rests carries where
    // the first one was written, and a panel that cross-highlighted one pair
    // and not the others would be showing the reader less than the compiler had
    // already worked out.
    let elsewhere = match &error.kind {
        ir::ErrorKind::Duplicate { previous, .. }
        | ir::ErrorKind::DuplicateParameter { previous } => Some((*previous, ui::FIRST_DEFINITION)),
        ir::ErrorKind::MixedTail { previous, .. } => Some((*previous, ui::FIRST_USE)),
        _ => None,
    };
    if let Some((previous, note)) = elsewhere {
        diagnostic.related.push(Related {
            span: loc(previous, files),
            message: note.to_string(),
        });
    }
    diagnostic
}

/// Inference's errors, two of which have a second place to point at: the
/// a variable that declared the variable a body broke its promise about. The
/// same pairing [`ir_diagnostic`] makes one phase earlier, and shown the same
/// way — one diagnostic with two highlights, because a broken promise is only
/// legible next to the promise.
fn inference_diagnostic(error: &inference::Error, files: &HashMap<FileID, u32>) -> Diagnostic {
    let mut diagnostic = raw(
        "types",
        error.kind.code(),
        error.kind.to_string(),
        loc(error.span, files),
    );
    let declared = match &error.kind {
        inference::ErrorKind::RigidBroken { declared, .. }
        | inference::ErrorKind::RigidField { declared, .. } => Some(*declared),
        _ => None,
    };
    if let Some(declared) = declared {
        diagnostic.related.push(Related {
            span: loc(declared, files),
            message: ui::DECLARED_HERE.to_string(),
        });
    }
    diagnostic
}

#[derive(serde::Deserialize)]
struct DependencyManifest {
    #[serde(default)]
    dependencies: indexmap::IndexMap<String, String>,
}

fn validate_sandbox_graph(
    scratch: &Path,
    project: &Path,
    active: &mut HashSet<PathBuf>,
    completed: &mut HashSet<PathBuf>,
) -> std::io::Result<()> {
    let project = std::fs::canonicalize(project)?;
    if completed.contains(&project) || !active.insert(project.clone()) {
        return Ok(());
    }
    let result = (|| {
        let configured_manifest = project.join(crate::docs::MANIFEST);
        let manifest_path = std::fs::canonicalize(&configured_manifest)?;
        let canonical_scratch = std::fs::canonicalize(scratch)?;
        if !manifest_path.starts_with(&canonical_scratch) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "manifest {} escapes debug/scratch",
                    configured_manifest.display()
                ),
            ));
        }
        let source = std::fs::read_to_string(&manifest_path)?;
        let manifest: DependencyManifest = toml::from_str(&source).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "could not parse manifest {}: {error}",
                    manifest_path.display()
                ),
            )
        })?;
        for declared in manifest.dependencies.values() {
            let child = crate::docs::dependency_path(scratch, &project, Path::new(declared))?;
            validate_sandbox_graph(scratch, &child, active, completed)?;
        }
        Ok(())
    })();
    active.remove(&project);
    if result.is_ok() {
        completed.insert(project);
    }
    result
}

fn raw(stage: &'static str, code: &'static str, message: String, span: Option<Loc>) -> Diagnostic {
    Diagnostic {
        id: 0,
        stage,
        severity: Severity::Error,
        code,
        message,
        span,
        related: Vec::new(),
    }
}

/// The source text a span covers, quoted for a message.
fn quote(source: &str, span: Span) -> String {
    match source.get(span.start..span.end()) {
        Some("") | None => "at end of input".to_string(),
        Some(text) => format!("`{text}`"),
    }
}
