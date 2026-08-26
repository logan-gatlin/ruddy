//! Filesystem-facing Ruddy compiler entry point used by the command-line tool.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fmt, fs,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use ruddy::{
    artifact::{Artifact, Dependency},
    bundle::{self, Disk, Files},
    inference, ir, lir, patterns,
    symbol::{Bundle, Mint, Version},
    tracking::{FileManager, Span},
};
use serde::Deserialize;

const MANIFEST: &str = "Ruddy.toml";
const ROOT: &str = "main.hc";
const BUILD_DIRECTORY: &str = "build";
const INITIAL_VERSION: &str = "0.1.0";

/// The command-line syntax accepted by [`run`].
pub const USAGE: &str = "ruddy new <path> | ruddy build";

/// The successful filesystem action performed by [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A project was scaffolded at this path.
    Created(PathBuf),
    /// An artifact was written to this path.
    Built(PathBuf),
}

/// A user-facing command-line or filesystem failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    rendered: String,
    usage: bool,
}

impl CliError {
    fn one(message: impl Into<String>) -> Self {
        Self {
            rendered: format!("error: {}", message.into()),
            usage: false,
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self {
            rendered: format!("error: {}", message.into()),
            usage: true,
        }
    }

    /// Whether the command's usage should be printed after this error.
    pub const fn is_usage(&self) -> bool {
        self.usage
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.rendered)
    }
}

impl std::error::Error for CliError {}

/// Parse and execute a Ruddy command relative to `current_directory`.
///
/// The iterator contains arguments after the executable name. Only `new
/// <path>` and `build` are accepted.
pub fn run<I, S>(arguments: I, current_directory: impl AsRef<Path>) -> Result<Outcome, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut arguments = arguments.into_iter().map(Into::into);
    let Some(command) = arguments.next() else {
        return Err(CliError::usage("expected a subcommand (`new` or `build`)"));
    };

    if command == OsStr::new("new") {
        let Some(path) = arguments.next() else {
            return Err(CliError::usage("`new` requires a project path"));
        };
        if arguments.next().is_some() {
            return Err(CliError::usage("`new` accepts exactly one project path"));
        }
        let path = current_directory.as_ref().join(path);
        new_project(&path)?;
        Ok(Outcome::Created(path))
    } else if command == OsStr::new("build") {
        if arguments.next().is_some() {
            return Err(CliError::usage("`build` does not accept arguments"));
        }
        build_project(current_directory).map(Outcome::Built)
    } else {
        Err(CliError::usage(format!(
            "unknown subcommand `{}`; expected `new` or `build`",
            command.to_string_lossy()
        )))
    }
}

/// Create a new Ruddy project without replacing an existing path.
///
/// A write failure can leave a partial project in place; it is not removed by
/// pathname because another process may have replaced or populated its entries.
pub fn new_project(path: impl AsRef<Path>) -> Result<(), CliError> {
    let path = path.as_ref();
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CliError::one(format!(
                "project path `{}` has no valid bundle name",
                path.display()
            ))
        })?;
    let version = Version::new(0, 1, 0);
    if Bundle::new(name, version).is_none() {
        return Err(CliError::one(format!(
            "project directory name `{name}` is not a valid Ruddy bundle name; names must start with an ASCII letter and contain only ASCII letters, digits, `-`, or `_`"
        )));
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::one(format!(
                "could not create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::create_dir(path).map_err(|error| {
        CliError::one(format!(
            "could not create project directory {}: {error}",
            path.display()
        ))
    })?;

    let manifest = path.join(MANIFEST);
    let root = path.join(ROOT);
    write_new_file(
        &manifest,
        &format!(
            "name = {name:?}\nversion = {INITIAL_VERSION:?}\nroot = \"main.hc\"\n\n[dependencies]\n"
        ),
    )?;
    write_new_file(&root, "let main = 0n\n")
}

fn write_new_file(path: &Path, contents: &str) -> Result<(), CliError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CliError::one(format!("could not create {}: {error}", path.display())))?;
    // Leave a partial scaffold behind on failure. Once this handle is dropped,
    // pathname cleanup could delete an entry another process installed in its
    // place; `create_new` still guarantees that this invocation overwrites none.
    file.write_all(contents.as_bytes())
        .map_err(|error| CliError::one(format!("could not write {}: {error}", path.display())))
}

/// Compile the complete project graph, then write each project's canonical
/// artifact to that project's own `build/` directory, dependencies first.
/// No artifact is touched unless the entire graph compiles successfully.
pub fn build_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
    let graph = compile_graph(directory).map_err(|error| CliError {
        rendered: error.to_string(),
        usage: false,
    })?;
    let mut root = None;
    for project in graph.projects {
        let build = project.directory.join(BUILD_DIRECTORY);
        fs::create_dir_all(&build).map_err(|error| {
            CliError::one(format!(
                "could not create build directory {}: {error}",
                build.display()
            ))
        })?;
        let path = build.join(format!(
            "{}.artifact",
            project.artifact.header.identity.name
        ));
        replace_file(&path, project.artifact.print().as_bytes())?;
        root = Some(path);
    }
    root.ok_or_else(|| CliError::one("the project graph was empty"))
}

/// Write beside the old artifact first, so a failed write cannot truncate the
/// last successful build. Replacing the destination is atomic where the host's
/// rename operation supports replacement; Windows uses a recoverable backup.
fn replace_file(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("artifact");
    let mut temporary = None;
    for attempt in 0..100 {
        let candidate =
            path.with_file_name(format!(".{name}.tmp-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(CliError::one(format!(
                    "could not write artifact {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let Some((temporary_path, mut file)) = temporary else {
        return Err(CliError::one(format!(
            "could not write artifact {}: no temporary filename was available",
            path.display()
        )));
    };

    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary_path);
        return Err(CliError::one(format!(
            "could not write artifact {}: {error}",
            path.display()
        )));
    }
    match fs::rename(&temporary_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            #[cfg(windows)]
            {
                replace_existing_windows(path, &temporary_path, error)
            }
            #[cfg(not(windows))]
            {
                let _ = fs::remove_file(&temporary_path);
                Err(CliError::one(format!(
                    "could not write artifact {}: {error}",
                    path.display()
                )))
            }
        }
    }
}

/// Windows does not replace an existing file with `rename`. Move the old
/// regular file aside first, restore it if installation fails, and never move
/// a blocking directory out of the way.
#[cfg(windows)]
fn replace_existing_windows(
    path: &Path,
    temporary_path: &Path,
    initial_error: std::io::Error,
) -> Result<(), CliError> {
    let is_file = fs::metadata(path).is_ok_and(|metadata| metadata.is_file());
    if !is_file {
        let _ = fs::remove_file(temporary_path);
        return Err(CliError::one(format!(
            "could not write artifact {}: {initial_error}",
            path.display()
        )));
    }

    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("artifact");
    let mut backup = None;
    for attempt in 0..100 {
        let candidate =
            path.with_file_name(format!(".{name}.old-{}-{attempt}", std::process::id()));
        match fs::rename(path, &candidate) {
            Ok(()) => {
                backup = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                let _ = fs::remove_file(temporary_path);
                return Err(CliError::one(format!(
                    "could not replace artifact {}: {error}",
                    path.display()
                )));
            }
        }
    }
    let Some(backup) = backup else {
        let _ = fs::remove_file(temporary_path);
        return Err(CliError::one(format!(
            "could not replace artifact {}: no backup filename was available",
            path.display()
        )));
    };

    if let Err(error) = fs::rename(temporary_path, path) {
        let _ = fs::remove_file(temporary_path);
        if let Err(restore_error) = fs::rename(&backup, path) {
            return Err(CliError::one(format!(
                "could not replace artifact {}: {error}; the previous artifact remains at {} because restoring it failed: {restore_error}",
                path.display(),
                backup.display()
            )));
        }
        return Err(CliError::one(format!(
            "could not replace artifact {}: {error}",
            path.display()
        )));
    }

    fs::remove_file(&backup).map_err(|error| {
        CliError::one(format!(
            "replaced artifact {}, but could not remove backup {}: {error}",
            path.display(),
            backup.display()
        ))
    })
}

/// A user-facing failure while loading a manifest or compiling its bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    messages: Vec<String>,
}

impl CompileError {
    fn one(message: impl Into<String>) -> Self {
        Self {
            messages: vec![format!("error: {}", message.into())],
        }
    }

    fn diagnostics(messages: Vec<String>) -> Self {
        Self { messages }
    }

    /// The individual diagnostics in display order.
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, message) in self.messages.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            formatter.write_str(message)?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    version: String,
    root: PathBuf,
    dependencies: IndexMap<String, PathBuf>,
}

/// One successfully compiled project in a dependency graph.
#[derive(Debug, Clone)]
pub struct CompiledProject {
    /// Canonical project directory containing `Ruddy.toml`.
    pub directory: PathBuf,
    /// The project's canonical in-memory artifact.
    pub artifact: Artifact,
}

/// A successfully compiled graph, in unique dependency-first order. The root
/// project is always last.
#[derive(Debug, Clone)]
pub struct CompiledGraph {
    pub projects: Vec<CompiledProject>,
}

/// Compile the project in `directory` recursively and return only its artifact.
/// This operation is side-effect free; use [`build_project`] to write artifacts.
pub fn compile(directory: impl AsRef<Path>) -> Result<Artifact, CompileError> {
    compile_graph(directory)?
        .projects
        .pop()
        .map(|project| project.artifact)
        .ok_or_else(|| CompileError::one("the project graph was empty"))
}

/// Compile every unique project reachable from `directory`, dependencies first.
pub fn compile_graph(directory: impl AsRef<Path>) -> Result<CompiledGraph, CompileError> {
    let root = canonical_project(directory.as_ref())?;
    let mut compiler = GraphCompiler::default();
    compiler.visit(root, None)?;
    Ok(CompiledGraph {
        projects: compiler.projects,
    })
}

/// Compile several dependency roots with one shared graph cache. The returned
/// identities correspond to the input roots in order; graph projects remain
/// unique and dependency-first across all roots.
pub fn compile_dependency_graph<I, N, P>(
    dependencies: I,
) -> Result<(CompiledGraph, Vec<Dependency>), CompileError>
where
    I: IntoIterator<Item = (N, P)>,
    N: Into<String>,
    P: AsRef<Path>,
{
    let mut compiler = GraphCompiler::default();
    let mut direct = Vec::new();
    for (expected, directory) in dependencies {
        let expected = expected.into();
        let directory = canonical_project(directory.as_ref())?;
        let manifest = load_manifest(&directory)?;
        if manifest.name != expected {
            return Err(CompileError::one(format!(
                "dependency key `{expected}` resolves to project `{}` instead",
                manifest.name
            )));
        }
        let index = compiler.visit(directory, Some((expected, PathBuf::new())))?;
        let artifact = &compiler.projects[index].artifact;
        direct.push(Dependency {
            name: artifact.header.identity.name.clone(),
            version: artifact.header.identity.version.clone(),
        });
    }
    Ok((
        CompiledGraph {
            projects: compiler.projects,
        },
        direct,
    ))
}

#[derive(Default)]
struct GraphCompiler {
    completed: HashMap<PathBuf, usize>,
    active: Vec<(PathBuf, String)>,
    projects: Vec<CompiledProject>,
}

impl GraphCompiler {
    fn visit(
        &mut self,
        directory: PathBuf,
        edge: Option<(String, PathBuf)>,
    ) -> Result<usize, CompileError> {
        if let Some(&index) = self.completed.get(&directory) {
            return Ok(index);
        }
        if let Some(at) = self.active.iter().position(|(path, _)| path == &directory) {
            let mut chain: Vec<String> = self.active[at..]
                .iter()
                .map(|(path, name)| format!("{name} ({})", path.display()))
                .collect();
            if let Some((name, declared)) = edge {
                chain.push(format!("{name} ({})", declared.display()));
            }
            return Err(CompileError::one(format!(
                "dependency cycle: {}",
                chain.join(" -> ")
            )));
        }

        let manifest = load_manifest(&directory)?;
        let identity = configured_identity(&manifest.name, &manifest.version)?;
        let active_name = edge
            .as_ref()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| manifest.name.clone());
        self.active.push((directory.clone(), active_name));

        let mut dependencies = Vec::with_capacity(manifest.dependencies.len());
        for (expected, declared) in &manifest.dependencies {
            let joined = directory.join(declared);
            let child = canonical_project(&joined)
                .map_err(|error| dependency_error(expected, declared, &directory, error))?;
            let child_manifest = load_manifest(&child)
                .map_err(|error| dependency_error(expected, declared, &directory, error))?;
            if child_manifest.name != *expected {
                self.active.pop();
                return Err(CompileError::one(format!(
                    "dependency `{expected}` declared as `{}` by {} contains project `{}` instead",
                    declared.display(),
                    directory.join(MANIFEST).display(),
                    child_manifest.name
                )));
            }
            let index = self
                .visit(child, Some((expected.clone(), declared.clone())))
                .map_err(|error| dependency_error(expected, declared, &directory, error))?;
            let child_artifact = &self.projects[index].artifact;
            dependencies.push(Dependency {
                name: child_artifact.header.identity.name.clone(),
                version: child_artifact.header.identity.version.clone(),
            });
        }

        let artifact = compile_one(&directory, manifest, identity, dependencies)?;
        self.active.pop();
        let index = self.projects.len();
        self.projects.push(CompiledProject {
            directory: directory.clone(),
            artifact,
        });
        self.completed.insert(directory, index);
        Ok(index)
    }
}

fn canonical_project(directory: &Path) -> Result<PathBuf, CompileError> {
    let canonical = fs::canonicalize(directory).map_err(|error| {
        CompileError::one(format!(
            "could not resolve project folder {}: {error}",
            directory.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(CompileError::one(format!(
            "project path {} is not a folder",
            directory.display()
        )));
    }
    Ok(canonical)
}

fn dependency_error(
    name: &str,
    declared: &Path,
    parent: &Path,
    error: CompileError,
) -> CompileError {
    CompileError::diagnostics(
        error
            .messages
            .into_iter()
            .map(|message| {
                format!(
                    "error: dependency `{name}` at `{}` from {}:\n{}",
                    declared.display(),
                    parent.join(MANIFEST).display(),
                    message
                )
            })
            .collect(),
    )
}

fn compile_one(
    directory: &Path,
    manifest: Manifest,
    identity: Bundle,
    dependencies: Vec<Dependency>,
) -> Result<Artifact, CompileError> {
    let Some(name) = configured_file_name(&manifest.root) else {
        return Err(CompileError::one("manifest field `root` must name a file"));
    };
    let source_directory = manifest.root.parent().unwrap_or(Path::new(""));
    let root = directory.join(&manifest.root);
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));

    let disk = Disk::new(parent);
    if disk.read(name).is_none() {
        return Err(CompileError::one(format!(
            "could not read bundle root {} configured by {}",
            root.display(),
            directory.join(MANIFEST).display()
        )));
    }

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &disk, name);
    let mut mint = Mint::new(identity);
    let mut built = ir::build(&mut mint, loaded.stmts);
    let inferred = inference::infer(&mint, &mut built.program);
    let checked = patterns::check(&built.program, &inferred);

    let errors = loaded
        .loaded
        .iter()
        .map(|file| file.lex_errors.len() + file.parse_errors.len())
        .sum::<usize>()
        + loaded.errors.len()
        + built.errors.len()
        + inferred.errors.len()
        + checked.errors.len();

    if errors != 0 {
        let mut diagnostics = Vec::with_capacity(errors);
        for file in &loaded.loaded {
            for error in &file.lex_errors {
                diagnostics.push(diagnostic(
                    &mut files,
                    "lex",
                    error.kind.code(),
                    error.span,
                    &error.kind,
                    source_directory,
                ));
            }
            for error in &file.parse_errors {
                diagnostics.push(diagnostic(
                    &mut files,
                    "parse",
                    error.code(),
                    error.span,
                    error,
                    source_directory,
                ));
            }
        }
        for error in &loaded.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "bundle",
                error.kind.code(),
                error.span,
                &BundleMessage {
                    kind: &error.kind,
                    source_directory,
                },
                source_directory,
            ));
        }
        for error in &built.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "ir",
                error.kind.code(),
                error.span,
                &error.kind,
                source_directory,
            ));
        }
        for error in &inferred.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "types",
                error.kind.code(),
                error.span,
                &error.kind,
                source_directory,
            ));
        }
        for error in &checked.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "patterns",
                error.kind.code(),
                error.span,
                &error.kind,
                source_directory,
            ));
        }
        return Err(CompileError::diagnostics(diagnostics));
    }

    let lowered = lir::lower(&mint, &built.program, &inferred);
    let mut artifact = Artifact::build(&mint, &built.program, &inferred, &lowered);
    artifact.header.dependencies = dependencies;
    Ok(artifact)
}

fn configured_identity(name: &str, configured_version: &str) -> Result<Bundle, CompileError> {
    let version = Version::parse(configured_version).map_err(|error| {
        CompileError::one(format!(
            "manifest field `version` has invalid semantic version `{configured_version}`: {error}"
        ))
    })?;
    if !version.build.is_empty() {
        return Err(CompileError::one(format!(
            "manifest field `version` value `{version}` uses unsupported build metadata"
        )));
    }
    Bundle::new(name, version).ok_or_else(|| {
        CompileError::one(format!(
            "manifest field `name` value `{name}` is not a valid Ruddy bundle name"
        ))
    })
}

fn configured_file_name(root: &Path) -> Option<&str> {
    let configured = root.to_str()?;
    let final_component = configured.rsplit(std::path::is_separator).next()?;
    if final_component.is_empty() || matches!(final_component, "." | "..") {
        return None;
    }
    root.file_name()?.to_str()
}

fn load_manifest(directory: &Path) -> Result<Manifest, CompileError> {
    let path = directory.join(MANIFEST);
    let source = fs::read_to_string(&path).map_err(|error| {
        CompileError::one(format!(
            "could not read manifest {}: {error}",
            path.display()
        ))
    })?;
    toml::from_str(&source).map_err(|error| {
        CompileError::one(format!(
            "could not parse manifest {}: {error}",
            path.display()
        ))
    })
}

/// A bundle complaint rendered from the project boundary. The loader keeps
/// candidate paths relative to the bundle root for debugger and in-memory
/// consumers; at the CLI those instructions need the configured source
/// directory prefix to name files the user can actually create or delete.
struct BundleMessage<'a> {
    kind: &'a bundle::ErrorKind,
    source_directory: &'a Path,
}

impl fmt::Display for BundleMessage<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            bundle::ErrorKind::ModuleFileMissing { beside, inside } => write!(
                formatter,
                "this module has no file; create `{}` or `{}`",
                self.source_directory.join(beside).display(),
                self.source_directory.join(inside).display(),
            ),
            bundle::ErrorKind::ModuleFileAmbiguous { beside, inside } => write!(
                formatter,
                "this module has two files; delete one of `{}` or `{}`",
                self.source_directory.join(beside).display(),
                self.source_directory.join(inside).display(),
            ),
        }
    }
}

fn diagnostic(
    files: &mut FileManager,
    phase: &str,
    code: &str,
    span: Span,
    message: &impl fmt::Display,
    source_directory: &Path,
) -> String {
    let file = files.get_file(span.file_id);
    let before = file.content.get(..span.start).unwrap_or(&file.content);
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    let path = source_directory.join(&file.path);
    format!(
        "{}:{line}:{column}: error[{phase}/{code}]: {message}",
        path.display()
    )
}
