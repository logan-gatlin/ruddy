//! Filesystem-facing Ruddy compiler entry point used by the command-line tool.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fmt, fs,
    fs::OpenOptions,
    io::{self, IsTerminal as _, Read as _, Write as _},
    ops::Range,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};
use clap::{Parser, Subcommand};
use indexmap::IndexMap;
use ruddy::{
    artifact::{Artifact, Dependency, Kind},
    bundle::{self, Disk, Files},
    inference,
    symbol::{Bundle, Mint, Version},
    tracking::{FileManager, Span},
    ui,
};
use serde::{Deserialize, Serialize};

mod cache;
mod documentation;
pub mod lsp;
pub mod workspace;

pub use cache::fingerprint;

mod git;
pub use git::{LOCKFILE, LockedGit, LockedSelector, Lockfile, ruddy_home};

const MANIFEST: &str = "Ruddy.toml";
const ROOT: &str = "src/main.rud";
pub const DEFAULT_STD_GIT: &str = "https://github.com/logan-gatlin/ruddy.git";

const GITIGNORE: &str = ".gitignore";
const BUILD_DIRECTORY: &str = "build";
const NODE_PACKAGE: &[u8] = b"{\"type\":\"module\"}\n";
const INITIAL_VERSION: &str = "0.1.0";

#[derive(Debug, Parser)]
#[command(
    name = "ruddy",
    version,
    about = "The compiler and project manager for Ruddy",
    subcommand_required = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new Ruddy project.
    #[command(alias = "n")]
    New {
        /// Directory to create.
        path: PathBuf,
    },
    /// Compile a project and write its artifacts.
    #[command(alias = "b")]
    Build,
    /// Remove a project's build output.
    Clean,
    /// Type-check a project without writing build artifacts.
    Check,
    /// Generate Markdown documentation for the public API.
    Doc,
    /// Build and execute a JavaScript-targeted project.
    Run,
    /// Serve the language server over standard input and output.
    Lsp,
    /// Format Ruddy source files in place.
    #[command(alias = "f")]
    Fmt {
        /// Files or folders to format. With none, every source of the
        /// bundle the current folder belongs to.
        paths: Vec<PathBuf>,
        /// Write nothing; list the files that would change and fail if any.
        #[arg(long)]
        check: bool,
        /// Format standard input to standard output.
        #[arg(long, conflicts_with_all = ["paths", "check"])]
        stdin: bool,
    },
}

/// The successful filesystem action performed by [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A project was scaffolded at this path.
    Created(PathBuf),
    /// An artifact was written to this path.
    Built(PathBuf),
    /// Build output was removed from this path.
    Cleaned(PathBuf),
    /// This project was checked successfully.
    Checked(PathBuf),
    /// Public API documentation was written to this directory.
    Documented(PathBuf),
    /// This JavaScript module was built and executed successfully.
    Ran(PathBuf),
    /// The language server ran until its client asked it to exit.
    Served,
    /// Source files were formatted, or checked.
    Formatted(FormatReport),
    /// Standard input was formatted: the text to write to standard output,
    /// and the syntax errors it was formatted around.
    FormattedStdin {
        text: String,
        diagnostics: Vec<CompileDiagnostic>,
    },
}

/// What `ruddy fmt` did to a set of files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatReport {
    /// The files rewritten — or, under `--check`, the files that would be.
    pub changed: Vec<PathBuf>,
    /// The files that were already formatted.
    pub unchanged: Vec<PathBuf>,
    /// The files with lexical or syntactic errors, formatted around them.
    pub errors: Vec<PathBuf>,
    /// The diagnostics of those files, rendered for the terminal.
    pub diagnostics: Vec<CompileDiagnostic>,
    /// Whether this was a `--check` run.
    pub check: bool,
}

impl FormatReport {
    /// Whether the run should fail: a check that found unformatted files,
    /// or any file with syntax errors.
    pub fn failed(&self) -> bool {
        (self.check && !self.changed.is_empty()) || !self.errors.is_empty()
    }
}

/// A user-facing command-line or filesystem failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    rendered: String,
    usage: bool,
    exit_code: u8,
}

impl CliError {
    fn subprocess(message: impl Into<String>, status: std::process::ExitStatus) -> Self {
        let mut error = Self::one(message);
        error.exit_code = status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1);
        error
    }

    fn one(message: impl Into<String>) -> Self {
        Self {
            rendered: format!("error: {}", message.into()),
            usage: false,
            exit_code: 1,
        }
    }

    fn with_help_text(mut self, help: &str) -> Self {
        self.rendered.push_str(&format!("\nhelp: {help}"));
        self
    }

    fn clap(error: clap::Error) -> Self {
        Self {
            rendered: error.to_string().trim_end().to_owned(),
            usage: error.use_stderr(),
            exit_code: u8::try_from(error.exit_code()).unwrap_or(1),
        }
    }

    /// Whether this is an argument error accompanied by command usage.
    pub const fn is_usage(&self) -> bool {
        self.usage
    }

    /// Whether this value represents informational help or version output.
    pub const fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    /// The process exit code recommended for this failure or information.
    pub const fn exit_code(&self) -> u8 {
        self.exit_code
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
/// The iterator contains arguments after the executable name. Argument syntax,
/// aliases, help, and version output are provided by `clap`.
pub fn run<I, S>(arguments: I, current_directory: impl AsRef<Path>) -> Result<Outcome, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let arguments =
        std::iter::once(OsString::from("ruddy")).chain(arguments.into_iter().map(Into::into));
    let command = Cli::try_parse_from(arguments)
        .map_err(CliError::clap)?
        .command;
    let current_directory = current_directory.as_ref();

    match command {
        Command::New { path } => {
            let path = current_directory.join(path);
            new_project(&path)?;
            Ok(Outcome::Created(path))
        }
        Command::Build => build_project(current_directory).map(Outcome::Built),
        Command::Clean => clean_project(current_directory).map(Outcome::Cleaned),
        Command::Check => {
            check_project(current_directory)?;
            Ok(Outcome::Checked(current_directory.to_path_buf()))
        }
        Command::Doc => document_project(current_directory).map(Outcome::Documented),
        Command::Run => run_project(current_directory).map(Outcome::Ran),
        Command::Lsp => {
            let (connection, threads) = lsp_server::Connection::stdio();
            lsp::serve(connection)
                .map_err(|error| CliError::one(format!("language server: {error}")))?;
            threads
                .join()
                .map_err(|error| CliError::one(format!("language server: {error}")))?;
            Ok(Outcome::Served)
        }
        Command::Fmt {
            paths,
            check,
            stdin: true,
        } => {
            debug_assert!(paths.is_empty() && !check);
            let mut source = String::new();
            io::stdin().read_to_string(&mut source).map_err(|error| {
                CliError::one(format!("could not read standard input: {error}"))
            })?;
            let (text, diagnostics) = format_source(&source, Path::new("<stdin>"));
            Ok(Outcome::FormattedStdin { text, diagnostics })
        }
        Command::Fmt { paths, check, .. } => {
            format_paths(&paths, current_directory, check).map(Outcome::Formatted)
        }
    }
}

/// Format `paths` in place, or check them. With no paths, the sources of
/// the bundle `current_directory` belongs to: every `.rud` under the folder
/// its manifest's `root` is in, minus any folder with a manifest of its own,
/// which is another bundle's. Dependencies are never followed.
pub fn format_paths(
    paths: &[PathBuf],
    current_directory: impl AsRef<Path>,
    check: bool,
) -> Result<FormatReport, CliError> {
    let current_directory = current_directory.as_ref();
    let files = if paths.is_empty() {
        let bundle = enclosing_bundle(current_directory)?;
        let manifest =
            load_manifest(&bundle, None).map_err(|error| CliError::one(error.to_string()))?;
        let root = bundle.join(&manifest.root);
        let source_directory = root.parent().unwrap_or(&bundle).to_path_buf();
        let mut files = Vec::new();
        collect_sources(&source_directory, false, &mut files)?;
        files
    } else {
        let mut files = Vec::new();
        for path in paths {
            let path = current_directory.join(path);
            if path.is_dir() {
                collect_sources(&path, false, &mut files)?;
            } else if path.is_file() {
                files.push(path);
            } else {
                return Err(CliError::one(format!(
                    "could not find `{}`",
                    path.display()
                )));
            }
        }
        files
    };
    let mut report = FormatReport {
        check,
        ..FormatReport::default()
    };
    for path in files {
        let source = fs::read_to_string(&path).map_err(|error| {
            CliError::one(format!("could not read `{}`: {error}", path.display()))
        })?;
        let (text, diagnostics) = format_source(&source, &path);
        if !diagnostics.is_empty() {
            report.errors.push(path.clone());
            report.diagnostics.extend(diagnostics);
        }
        if text == source {
            report.unchanged.push(path);
            continue;
        }
        if !check {
            replace_file(&path, text.as_bytes())?;
        }
        report.changed.push(path);
    }
    Ok(report)
}

/// Format one source, with its syntax errors as diagnostics naming `path`.
fn format_source(source: &str, path: &Path) -> (String, Vec<CompileDiagnostic>) {
    let mut files = FileManager::new();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = files.register_new_file(name, source.to_string());
    let formatted = ruddy::format::format(source, id);
    let directory = path.parent().unwrap_or(Path::new(""));
    let diagnostics = source_diagnostics(
        &mut files,
        syntax_diagnostics(&formatted.lex_errors, &formatted.parse_errors),
        directory,
    );
    (formatted.text, diagnostics)
}

/// The folder of the bundle `directory` is in: the nearest folder at or
/// above it with a manifest.
fn enclosing_bundle(directory: &Path) -> Result<PathBuf, CliError> {
    let directory = fs::canonicalize(directory).map_err(|error| {
        CliError::one(format!(
            "could not resolve folder `{}`: {error}",
            directory.display()
        ))
    })?;
    let mut at = Some(directory.as_path());
    while let Some(folder) = at {
        if folder.join(MANIFEST).is_file() {
            return Ok(folder.to_path_buf());
        }
        at = folder.parent();
    }
    Err(CliError::one(format!(
        "no `{MANIFEST}` found in `{}` or any folder above it",
        directory.display()
    ))
    .with_help_text("run inside a bundle, or name the files to format"))
}

/// Every `.rud` file under `directory`, in path order. A folder holding a
/// manifest of its own is another bundle's and is skipped — unless it is
/// the one asked for, which `nested` says.
fn collect_sources(
    directory: &Path,
    nested: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), CliError> {
    if nested && directory.join(MANIFEST).is_file() {
        return Ok(());
    }
    let unreadable = |error: io::Error| {
        CliError::one(format!(
            "could not read folder `{}`: {error}",
            directory.display()
        ))
    };
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .map_err(unreadable)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(unreadable)?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_sources(&path, true, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rud") {
            files.push(path);
        }
    }
    Ok(())
}

/// Create a new Ruddy project without replacing an existing path.
///
/// A scaffolding or Git initialization failure can leave a partial project in
/// place; it is not removed by pathname because another process may have
/// replaced or populated its entries.
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
    let source_directory = root.parent().expect("the root has a parent");
    fs::create_dir(source_directory).map_err(|error| {
        CliError::one(format!(
            "could not create source directory {}: {error}",
            source_directory.display()
        ))
    })?;
    write_new_file(
        &manifest,
        &format!(
            "name = {name:?}\nversion = {INITIAL_VERSION:?}\nkind = \"executable\"\nroot = {ROOT:?}\ntarget = \"js\"\n\n[dependencies]\n"
        ),
    )?;
    write_new_file(&root, "let main = fn _ => ()\n")?;
    write_new_file(&path.join(GITIGNORE), "/build/\n")?;
    initialize_git(path)
}

fn initialize_git(path: &Path) -> Result<(), CliError> {
    gix::init(path).map(|_| ()).map_err(|error| {
        CliError::one(format!(
            "could not initialize Git repository {}: {error}",
            path.display()
        ))
    })
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

/// Type-check and link a project without writing build artifacts.
pub fn check_project(directory: impl AsRef<Path>) -> Result<(), CliError> {
    compile(directory).map(|_| ()).map_err(|error| CliError {
        rendered: error.to_string(),
        usage: false,
        exit_code: 1,
    })
}

/// Generate flat Markdown pages for the enclosing bundle's public API.
pub fn document_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
    let directory = enclosing_bundle(directory.as_ref())?;
    let manifest =
        load_manifest(&directory, None).map_err(|error| CliError::one(error.to_string()))?;
    let target = directory.join(
        manifest
            .documentation
            .target
            .as_deref()
            .unwrap_or(Path::new("docs")),
    );
    let (_, pages) =
        compile_graph_output(&directory, true).map_err(|error| CliError::one(error.to_string()))?;
    documentation::write(
        &target,
        &manifest.name,
        manifest.documentation.frontmatter.as_deref(),
        &pages,
    )?;
    Ok(target)
}

/// Remove the current project's `build/` entry, if one exists.
///
/// A malformed source file or manifest does not prevent stale output from being
/// cleaned. The project directory itself must still exist and contain a regular
/// `Ruddy.toml` file; the marker is deliberately not parsed.
pub fn clean_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
    let directory = fs::canonicalize(directory.as_ref()).map_err(|error| {
        CliError::one(format!(
            "could not resolve project folder {}: {error}",
            directory.as_ref().display()
        ))
    })?;
    if !directory.is_dir() {
        return Err(CliError::one(format!(
            "project path {} is not a folder",
            directory.display()
        )));
    }
    let manifest = directory.join(MANIFEST);
    let manifest_metadata = fs::symlink_metadata(&manifest).map_err(|error| {
        CliError::one(format!(
            "project marker {} is not a regular file: {error}",
            manifest.display()
        ))
    })?;
    if !manifest_metadata.file_type().is_file() {
        return Err(CliError::one(format!(
            "project marker {} is not a regular file",
            manifest.display()
        )));
    }
    let build = directory.join(BUILD_DIRECTORY);
    let metadata = match fs::symlink_metadata(&build) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(build),
        Err(error) => {
            return Err(CliError::one(format!(
                "could not inspect build output {}: {error}",
                build.display()
            )));
        }
    };
    let removed = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(&build)
    } else if metadata.file_type().is_symlink() {
        // Unix removes either kind of symlink as a file. Windows requires
        // directory symlinks to be removed with `remove_dir`; the failed first
        // attempt does not traverse or alter the target.
        fs::remove_file(&build).or_else(|_| fs::remove_dir(&build))
    } else {
        fs::remove_file(&build)
    };
    removed.map_err(|error| {
        CliError::one(format!(
            "could not remove build output {}: {error}",
            build.display()
        ))
    })?;
    Ok(build)
}

struct InstalledBuild {
    artifact: PathBuf,
    javascript: Option<PathBuf>,
    target: Target,
    run: RunConfig,
    directory: PathBuf,
}

/// Compile the complete project graph, then write each local project's canonical
/// artifact to its own `build/` directory, dependencies first. Immutable Git
/// cache checkouts are never modified. No artifact is touched unless the entire
/// graph compiles and backend generation succeeds.
pub fn build_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
    install_project(directory.as_ref()).map(|installed| installed.artifact)
}

fn install_project(directory: &Path) -> Result<InstalledBuild, CliError> {
    let graph = compile_graph(directory).map_err(|error| CliError {
        rendered: error.to_string(),
        usage: false,
        exit_code: 1,
    })?;
    let linked = link_rooted(&graph).map_err(|error| CliError {
        rendered: error.to_string(),
        usage: false,
        exit_code: 1,
    })?;
    let last = graph.projects.len().checked_sub(1);
    let root_project = last
        .and_then(|index| graph.projects.get(index))
        .ok_or_else(|| CliError::one("the project graph was empty"))?;
    let target = root_project.target;
    let run = root_project.run.clone();
    let directory = root_project.directory.clone();
    // Generation is deliberately completed before any build output is touched.
    // The backend consumes the already linked root rather than relinking or
    // reading an artifact back from disk.
    let javascript = (target == Target::Js)
        .then(|| {
            ruddy::backend::js::generate_for_platform(&linked, root_project.platform.backend())
                .map_err(|error| CliError::one(format!("could not generate JavaScript: {error}")))
        })
        .transpose()?;
    let mut root_artifact = None;
    let mut root_javascript = None;
    for (index, project) in graph.projects.into_iter().enumerate() {
        if project.source != ProjectSource::Local {
            continue;
        }
        let build = project.directory.join(BUILD_DIRECTORY);
        fs::create_dir_all(&build).map_err(|error| {
            CliError::one(format!(
                "could not create build directory {}: {error}",
                build.display()
            ))
        })?;
        let artifact_path = build.join(format!(
            "{}.artifact",
            project.artifact.header().identity.name
        ));
        let artifact = if Some(index) == last {
            &linked
        } else {
            &project.artifact
        };
        replace_file(&artifact_path, artifact.print().as_bytes())?;
        if Some(index) == last {
            if let Some(javascript) = &javascript {
                // The generated `.js` is always ESM, independent of any
                // ancestor package scope in which the project happens to live.
                replace_file(&build.join("package.json"), NODE_PACKAGE)?;
                let path = build.join(format!("{}.js", project.artifact.header().identity.name));
                replace_file(&path, javascript.as_bytes())?;
                root_javascript = Some(path);
            }
            root_artifact = Some(artifact_path);
        }
    }
    Ok(InstalledBuild {
        artifact: root_artifact.ok_or_else(|| CliError::one("the project graph was empty"))?,
        javascript: root_javascript,
        target,
        run,
        directory,
    })
}

/// Build and execute the installed root JavaScript module. By default this uses
/// Node.js; `[run].js` may select a different shell runner.
pub fn run_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
    let manifest = load_manifest(directory.as_ref(), None)
        .map_err(|error| CliError::one(error.to_string()))?;
    if manifest.kind != Kind::Executable || manifest.target() != Target::Js {
        return Err(CliError::one(
            "`ruddy run` requires `kind = \"executable\"` and `target = \"js\"` (the executable default)",
        ));
    }
    let installed = install_project(directory.as_ref())?;
    if installed.target != Target::Js {
        return Err(CliError::one(
            "`ruddy run` requires the root manifest to set `target = \"js\"`",
        ));
    }
    let javascript = installed
        .javascript
        .ok_or_else(|| CliError::one("the JavaScript build output was not installed"))?;
    match installed.run.js.as_deref() {
        Some(runner) => execute_javascript_runner(runner, &javascript, &installed.directory)?,
        None => execute_node_module(OsStr::new("node"), &javascript, &installed.directory)?,
    }
    Ok(javascript)
}

fn execute_javascript_runner(runner: &str, path: &Path, directory: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    let mut command = {
        let mut command = ProcessCommand::new("sh");
        // Pass the path positionally rather than interpolating it into shell
        // source, while retaining the configured command's shell syntax.
        command
            .arg("-c")
            .arg(format!("{runner} \"$1\""))
            .arg("ruddy run")
            .arg(path);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command
            .arg("/S")
            .arg("/C")
            .arg(format!(r#"{runner} "%RUDDY_RUN_JAVASCRIPT%""#))
            .env("RUDDY_RUN_JAVASCRIPT", path);
        command
    };
    command.current_dir(directory);
    let status = command.status().map_err(|error| {
        CliError::one(format!(
            "could not start JavaScript runner `{runner}`: {error}"
        ))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::subprocess(
            format!("JavaScript runner `{runner}` exited with {status}"),
            status,
        ))
    }
}

/// Execute one JavaScript module with Node.js, as [`run_project`] does by
/// default. Node must be available as `node` on `PATH`.
pub fn execute_javascript_module(path: impl AsRef<Path>) -> Result<(), CliError> {
    let path = path.as_ref();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliError::one(format!("could not determine current directory: {error}"))
            })?
            .join(path)
    };
    let directory = absolute.parent().ok_or_else(|| {
        CliError::one(format!("JavaScript path {} has no parent", path.display()))
    })?;
    execute_node_module(OsStr::new("node"), &absolute, directory)
}

fn execute_node_module(program: &OsStr, path: &Path, directory: &Path) -> Result<(), CliError> {
    let available = ProcessCommand::new(program)
        .arg("--version")
        .current_dir(directory)
        .output()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CliError::one(
                    "Node.js is required to run JavaScript, but `node` was not found on PATH",
                )
            } else {
                CliError::one(format!(
                    "could not check whether Node.js is available: {error}"
                ))
            }
        })?;
    if !available.status.success() {
        return Err(CliError::one(format!(
            "Node.js is required to run JavaScript, but `node --version` exited with {}",
            available.status
        )));
    }

    let output = ProcessCommand::new(program)
        .arg(path)
        .current_dir(directory)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .output()
        .map_err(|error| CliError::one(format!("could not start Node.js: {error}")))?;
    if output.status.success() {
        io::stderr().write_all(&output.stderr).map_err(|error| {
            CliError::one(format!("could not write Node.js diagnostics: {error}"))
        })?;
        return Ok(());
    }

    let diagnostics = String::from_utf8_lossy(&output.stderr);
    let diagnostics = diagnostics.trim_end();
    let detail = if diagnostics.is_empty() {
        String::new()
    } else {
        format!("\n{diagnostics}")
    };
    Err(CliError::subprocess(
        format!("Node.js exited with {}{detail}", output.status),
        output.status,
    ))
}

#[cfg(test)]
mod node_tests {
    use super::*;

    #[test]
    fn missing_node_has_a_focused_error() {
        let directory = std::env::current_dir().unwrap();
        let missing = directory.join("ruddy-test-node-executable-that-does-not-exist");
        let error = execute_node_module(missing.as_os_str(), Path::new("module.js"), &directory)
            .unwrap_err();
        assert!(error.to_string().contains("Node.js is required"), "{error}");
        assert!(error.to_string().contains("not found on PATH"), "{error}");
    }
}

/// Write beside the destination first, so a failed write cannot truncate the
/// previous contents. Installation atomically replaces an existing file on
/// Unix and Windows.
pub(crate) fn replace_file(path: &Path, contents: &[u8]) -> Result<(), CliError> {
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

/// Windows' standard `rename` doesn't replace an existing file. `MoveFileExW`
/// supplies the same-volume atomic replacement needed by artifacts and locks.
#[cfg(windows)]
fn replace_existing_windows(
    path: &Path,
    temporary_path: &Path,
    _initial_error: std::io::Error,
) -> Result<(), CliError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both pointers address NUL-terminated UTF-16 buffers for the
    // duration of the call, and the flags require no additional structures.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced != 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        let _ = fs::remove_file(temporary_path);
        Err(CliError::one(format!(
            "could not replace artifact {}: {error}",
            path.display()
        )))
    }
}

/// One compiler or project diagnostic, retained until its reporter chooses a
/// terminal, browser, or editor presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileDiagnostic {
    stage: &'static str,
    code: &'static str,
    message: String,
    sources: Vec<OwnedDiagnosticSource>,
    primary: Option<OwnedDiagnosticLabel>,
    related: Vec<OwnedDiagnosticLabel>,
    help: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedDiagnosticSource {
    path: String,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedDiagnosticLabel {
    source: usize,
    range: Range<usize>,
    message: String,
}

impl CompileDiagnostic {
    fn plain(stage: &'static str, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            code,
            message: message.into(),
            sources: Vec::new(),
            primary: None,
            related: Vec::new(),
            help: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The phase which produced this diagnostic.
    pub const fn stage(&self) -> &'static str {
        self.stage
    }

    /// Its stable, greppable identifier.
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// The reader-facing headline, without severity or layout.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn help(&self) -> &[String] {
        &self.help
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Render this diagnostic through the same Ariadne boundary as source
    /// diagnostics from the active bundle.
    pub fn render(&self, color: bool) -> String {
        let available: Vec<_> = self
            .sources
            .iter()
            .map(|source| DiagnosticSource {
                path: &source.path,
                source: &source.source,
            })
            .collect();
        let primary = self.primary.as_ref().map(|label| DiagnosticLabel {
            source: label.source,
            range: label.range.clone(),
            message: &label.message,
        });
        let related: Vec<_> = self
            .related
            .iter()
            .map(|label| DiagnosticLabel {
                source: label.source,
                range: label.range.clone(),
                message: &label.message,
            })
            .collect();
        let help = (!self.help.is_empty()).then(|| self.help.join("; "));
        let notes: Vec<_> = self.notes.iter().map(String::as_str).collect();
        render_diagnostic_with_advice(
            self.stage,
            self.code,
            &self.message,
            &available,
            primary.as_ref(),
            &related,
            help.as_deref(),
            &notes,
            color,
        )
    }

    fn with_help(mut self, message: impl Into<String>) -> Self {
        let message = message.into();
        if !self.help.contains(&message) {
            self.help.push(message);
        }
        self
    }

    fn with_note(mut self, message: impl Into<String>) -> Self {
        let message = message.into();
        if !self.notes.contains(&message) {
            self.notes.push(message);
        }
        self
    }
}

/// User-facing failures while loading projects and compiling their bundles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    diagnostics: Vec<CompileDiagnostic>,
    // A plain rendered cache preserves the existing `messages()` API. Display
    // renders afresh so terminal colour is chosen only at the final boundary.
    messages: Vec<String>,
}

impl CompileError {
    fn report(code: &'static str, message: impl Into<String>) -> Self {
        Self::from_diagnostics(vec![CompileDiagnostic::plain("project", code, message)])
    }

    fn from_diagnostics(diagnostics: Vec<CompileDiagnostic>) -> Self {
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(false))
            .collect();
        Self {
            diagnostics,
            messages,
        }
    }

    fn map_diagnostics(self, mut map: impl FnMut(CompileDiagnostic) -> CompileDiagnostic) -> Self {
        Self::from_diagnostics(self.diagnostics.into_iter().map(&mut map).collect())
    }

    fn with_help(self, message: impl Into<String>) -> Self {
        let message = message.into();
        self.map_diagnostics(|diagnostic| diagnostic.with_help(message.clone()))
    }

    fn with_note(self, message: impl Into<String>) -> Self {
        let message = message.into();
        self.map_diagnostics(|diagnostic| diagnostic.with_note(message.clone()))
    }

    /// The structured diagnostics in display order.
    pub fn diagnostics(&self) -> &[CompileDiagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<CompileDiagnostic> {
        self.diagnostics
    }

    /// The individual plain-text diagnostics in display order.
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            formatter.write_str(&diagnostic.render(stderr_color()))?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Write only the canonical linked artifact.
    #[default]
    Artifact,
    /// Write the canonical linked artifact and a JavaScript ESM module.
    Js,
}

impl Target {
    /// The backend policy when a manifest omits its target.
    pub fn default_for(kind: Kind) -> Self {
        match kind {
            Kind::Library => Self::Artifact,
            Kind::Executable => Self::Js,
        }
    }

    /// The target as `Ruddy.toml` spells it, which is also what an `@if`
    /// guard in source names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Artifact => "artifact",
            Self::Js => "js",
        }
    }
}

/// Where a build's output is loaded and run. Distinct from [`Target`], which
/// is what the compiler emits: the same JavaScript runs under Node or in a
/// browser, and what differs is the host — which externs exist, which effects
/// the runtime handles. The platform decides what `@if` guards see and which
/// cache entry a dependency gets. A library builds for either; an executable
/// is refused for the web until the backend has a web entry adapter, since
/// today's adapter and epilogue are Node's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Node.js, the platform every build has had until now.
    #[default]
    Node,
    /// A browser.
    Web,
}

impl Platform {
    pub fn backend(self) -> ruddy::backend::js::Platform {
        match self {
            Self::Node => ruddy::backend::js::Platform::Node,
            Self::Web => ruddy::backend::js::Platform::Web,
        }
    }
    /// The platform as `Ruddy.toml` spells it, which is also what an `@if`
    /// guard in source names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Web => "web",
        }
    }
}

/// What a build is: the target it emits and the platform it runs on. Every
/// project in a graph is compiled for the root's build, so a dependency's
/// guards see the build its consumer is making.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Build {
    pub target: Target,
    pub platform: Platform,
    /// The domains `Nat` and `Int` are bound to, from the root's `integers`.
    pub domains: ruddy::types::Domains,
}

impl Build {
    /// The build a target, a platform, and a requested integer precision
    /// describe, or why they describe none. Every way into compilation comes
    /// through here, so a combination one entry point refuses cannot reach a
    /// backend through another. An omitted precision takes the default; one
    /// that is written and unsupported is an error rather than the default.
    pub fn resolve(
        target: Target,
        platform: Platform,
        integers: Option<u32>,
    ) -> Result<Self, CompileError> {
        let Some(bits) = integers else {
            return Ok(Self {
                target,
                platform,
                domains: ruddy::types::Domains::default(),
            });
        };
        let domains = ruddy::types::Domains::from_name(&bits.to_string()).ok_or_else(|| {
            CompileError::report(
                "manifest-invalid",
                format!("`integers = {bits}` is not a precision the compiler binds"),
            )
            .with_help("set `integers` to 53, 32, or 64")
        })?;
        if target == Target::Js && domains == ruddy::types::Domains::Bits64 {
            return Err(CompileError::report(
                "manifest-invalid",
                "the JavaScript target cannot hold 64-bit `Nat` and `Int`",
            )
            .with_help(
                "set `integers` to 53 or 32, or use Nat64 and Int64 where 64 bits are needed",
            ));
        }
        Ok(Self {
            target,
            platform,
            domains,
        })
    }

    /// The facts source may ask about this build, in the order a complaint
    /// lists them.
    pub fn environment(self) -> bundle::Environment {
        bundle::Environment::new([
            ("target", self.target.name()),
            ("platform", self.platform.name()),
            ("integers", self.domains.name()),
        ])
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    /// Shell command used by `ruddy run` for a JavaScript target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,
}

impl RunConfig {
    pub fn is_default(&self) -> bool {
        self.js.is_none()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    version: String,
    root: PathBuf,
    kind: Kind,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    platform: Option<Platform>,
    /// The precision of `Nat` and `Int` in bits: `53`, JavaScript's safe
    /// integers and the default, `32`, or `64`.
    #[serde(default)]
    integers: Option<u32>,
    #[serde(default)]
    run: RunConfig,
    #[serde(default)]
    documentation: DocumentationConfig,
    dependencies: ManifestDependencies,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationConfig {
    target: Option<PathBuf>,
    frontmatter: Option<String>,
}

impl Manifest {
    fn target(&self) -> Target {
        self.target
            .unwrap_or_else(|| Target::default_for(self.kind))
    }

    fn platform(&self) -> Platform {
        self.platform.unwrap_or_default()
    }

    /// The build this manifest asks for, when it is the root.
    fn build(&self) -> Result<Build, CompileError> {
        Build::resolve(self.target(), self.platform(), self.integers)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ManifestDependencies {
    /// The implicit dependency available under the reserved source alias `std`.
    #[serde(default)]
    std: StdConfig,
    #[serde(flatten)]
    declared: IndexMap<String, ManifestDependency>,
}

pub type ManifestDependency = DependencySpec;

/// Configuration for the implicit `std` dependency in a Ruddy manifest.
///
/// An omitted `[dependencies].std` entry uses [`StdConfig::Default`], `false`
/// disables standard library injection, and dependency syntax selects an override.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StdConfig {
    /// Resolve `std` from the default Git repository through the shared cache.
    #[default]
    Default,
    /// Do not inject a standard-library dependency for this project.
    Disabled,
    /// Resolve `std` using the normal path or Git dependency machinery.
    Dependency(DependencySpec),
}

impl StdConfig {
    /// Whether this setting is the omitted, default Git standard library.
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    /// Whether standard-library injection is disabled.
    pub const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    /// The configured override, if any.
    pub const fn dependency(&self) -> Option<&DependencySpec> {
        match self {
            Self::Dependency(specification) => Some(specification),
            Self::Default | Self::Disabled => None,
        }
    }
}

impl<'de> Deserialize<'de> for StdConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StdConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("false, a standard-library path string, or a dependency table")
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                if value {
                    Err(E::custom(
                        "`std = true` is invalid; omit `std` from `[dependencies]` to use the default standard library",
                    ))
                } else {
                    Ok(StdConfig::Disabled)
                }
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StdConfig::Dependency(DependencySpec::from(value)))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StdConfig::Dependency(DependencySpec::from(value)))
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                DependencySpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(StdConfig::Dependency)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for StdConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Dependency(specification) => specification.serialize(serializer),
            Self::Default => Err(serde::ser::Error::custom(
                "the default std setting must be represented by omitting the field",
            )),
        }
    }
}

/// A path or HTTPS Git dependency as accepted in `Ruddy.toml` and debugger requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum DependencySpec {
    Path(PathBuf),
    Detailed(DependencyDetail),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum GitSelector<'a> {
    Default,
    Branch(&'a str),
    Tag(&'a str),
    Rev(&'a str),
}

impl DependencySpec {
    pub fn bundle<'a>(&'a self, alias: &'a str) -> &'a str {
        match self {
            Self::Path(_) => alias,
            Self::Detailed(detail) => detail.bundle.as_deref().unwrap_or(alias),
        }
    }
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Detailed(detail) => detail.path.as_deref(),
        }
    }
    pub fn git(&self) -> Option<&str> {
        match self {
            Self::Path(_) => None,
            Self::Detailed(detail) => detail.git.as_deref(),
        }
    }
    pub fn selector(&self) -> Result<GitSelector<'_>, CompileError> {
        let Self::Detailed(detail) = self else {
            return Ok(GitSelector::Default);
        };
        let selectors = [
            detail.branch.as_deref(),
            detail.tag.as_deref(),
            detail.rev.as_deref(),
        ];
        if selectors.iter().flatten().count() > 1 {
            return Err(CompileError::report(
                "dependency-selectors-conflict",
                "a Git dependency can select only one branch, tag, or revision",
            )
            .with_help("remove all but one of `branch`, `tag`, and `rev`"));
        }
        for value in selectors.iter().flatten() {
            if value.is_empty() {
                return Err(CompileError::report(
                    "dependency-selector-empty",
                    "a Git branch, tag, or revision cannot be empty",
                )
                .with_help("provide a value or remove the empty setting"));
            }
        }
        Ok(if let Some(v) = selectors[0] {
            GitSelector::Branch(v)
        } else if let Some(v) = selectors[1] {
            GitSelector::Tag(v)
        } else if let Some(v) = selectors[2] {
            GitSelector::Rev(v)
        } else {
            GitSelector::Default
        })
    }
    /// Validate source exclusivity, HTTPS Git URLs, and selectors.
    pub fn validate(&self) -> Result<(), CompileError> {
        let Self::Detailed(detail) = self else {
            return Ok(());
        };
        match (detail.path.is_some(), detail.git.as_deref()) {
            (true, Some(_)) => {
                return Err(CompileError::report(
                    "dependency-source-conflict",
                    "a dependency cannot use both `path` and `git`",
                )
                .with_help("remove either `path` or `git`"));
            }
            (false, None) => {
                return Err(CompileError::report(
                    "dependency-source-missing",
                    "a dependency needs either a local `path` or a `git` URL",
                )
                .with_help("add `path = \"...\"` or `git = \"https://...\"`"));
            }
            (_, Some(url)) if !url.starts_with("https://") => {
                return Err(CompileError::report(
                    "dependency-git-not-https",
                    "this Git dependency URL must use HTTPS",
                )
                .with_help("use a URL beginning with `https://`"));
            }
            _ => {}
        }
        if detail.git.is_none()
            && (detail.branch.is_some() || detail.tag.is_some() || detail.rev.is_some())
        {
            return Err(CompileError::report(
                "dependency-selector-without-git",
                "`branch`, `tag`, and `rev` can be used only with a Git dependency",
            )
            .with_help("add a `git` URL or remove the selector"));
        }
        self.selector()?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for DependencySpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = DependencySpec;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a dependency path string or dependency table")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(DependencySpec::Path(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(DependencySpec::Path(value.into()))
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut detail = DependencyDetail {
                    bundle: None,
                    path: None,
                    git: None,
                    branch: None,
                    tag: None,
                    rev: None,
                };
                while let Some(key) = map.next_key::<String>()? {
                    let slot = match key.as_str() {
                        "bundle" => &mut detail.bundle,
                        "git" => &mut detail.git,
                        "branch" => &mut detail.branch,
                        "tag" => &mut detail.tag,
                        "rev" => &mut detail.rev,
                        "path" => {
                            if detail.path.is_some() {
                                return Err(serde::de::Error::duplicate_field("path"));
                            }
                            detail.path = Some(PathBuf::from(map.next_value::<String>()?));
                            continue;
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                &key,
                                &["bundle", "path", "git", "branch", "tag", "rev"],
                            ));
                        }
                    };
                    if slot.is_some() {
                        return Err(serde::de::Error::duplicate_field(match key.as_str() {
                            "bundle" => "bundle",
                            "git" => "git",
                            "branch" => "branch",
                            "tag" => "tag",
                            _ => "rev",
                        }));
                    }
                    *slot = Some(map.next_value()?);
                }
                Ok(DependencySpec::Detailed(detail))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl From<String> for DependencySpec {
    fn from(path: String) -> Self {
        Self::Path(path.into())
    }
}
impl From<&str> for DependencySpec {
    fn from(path: &str) -> Self {
        Self::Path(path.into())
    }
}

/// The storage provenance of a compiled project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSource {
    /// An explicitly compiled root or a configured local path dependency.
    Local,
    /// A non-root project inside Ruddy's immutable global Git cache.
    GitCache,
}

/// One successfully compiled project in a dependency graph.
#[derive(Debug, Clone)]
pub struct CompiledProject {
    /// Canonical project directory containing `Ruddy.toml`.
    pub directory: PathBuf,
    /// Whether this project is local or from the immutable Git cache.
    pub source: ProjectSource,
    /// The output target configured by this project's manifest.
    ///
    /// Only the requested root's target affects [`build_project`].
    pub target: Target,
    pub platform: Platform,
    /// Runtime configuration from this project's manifest.
    ///
    /// Only the requested root's configuration affects [`run_project`].
    pub run: RunConfig,
    /// The project's canonical in-memory artifact.
    pub artifact: Artifact,
}

/// Successfully compiled projects in unique dependency-first order.
///
/// APIs that accept several roots return a forest, so the final project is not
/// necessarily a root whose closure contains every preceding project.
#[derive(Debug, Clone)]
pub struct CompiledGraph {
    pub projects: Vec<CompiledProject>,
}

fn link_rooted(graph: &CompiledGraph) -> Result<Artifact, CompileError> {
    let artifacts: Vec<_> = graph
        .projects
        .iter()
        .map(|project| project.artifact.clone())
        .collect();
    ruddy::link::link(&artifacts)
        .map_err(|error| CompileError::report("link-failed", error.to_string()))
}

/// Compile and statically link the project in `directory`.
/// Git dependencies may populate the Ruddy cache and a successful resolution may
/// atomically update the root `Ruddy.lock`; no build artifacts are written.
pub fn compile(directory: impl AsRef<Path>) -> Result<Artifact, CompileError> {
    link_rooted(&compile_graph(directory)?)
}

/// Compile every unique project reachable from `directory`, dependencies first.
/// This single-root graph ends with the requested project.
pub fn compile_graph(directory: impl AsRef<Path>) -> Result<CompiledGraph, CompileError> {
    compile_graph_output(directory.as_ref(), false).map(|(graph, _)| graph)
}

fn compile_graph_output(
    directory: &Path,
    document: bool,
) -> Result<(CompiledGraph, Vec<documentation::Page>), CompileError> {
    let root = canonical_project(directory)?;
    let resolver = git::Resolver::new(&root)?;
    let build = load_manifest(&root, None)?.build()?;
    let mut compiler = GraphCompiler {
        document,
        root: Some(root.clone()),
        resolver: Some(resolver),
        build: Some(build),
        // Beside the Git checkouts, under Ruddy home: one standard library
        // compiled once serves every project.
        cache: ruddy_home()
            .ok()
            .map(|home| cache::ArtifactCache::at(home.join("cache").join("artifacts"))),
        ..GraphCompiler::default()
    };
    compiler.visit(root, None)?;
    if build.target == Target::Js {
        let (root, dependencies) = compiler.projects.split_last().expect("root was compiled");
        let dependencies: Vec<_> = dependencies
            .iter()
            .map(|project| &project.artifact)
            .collect();
        ruddy::backend::js::check_exports(&root.artifact, &dependencies, build.platform.backend())
            .map_err(|error| {
                CompileError::report(
                    if matches!(error, ruddy::backend::js::Error::ExportType { .. }) {
                        "unsupported-export-type"
                    } else {
                        "unsupported-export-effects"
                    },
                    error.to_string(),
                )
            })?;
    }
    if let Some(resolver) = &compiler.resolver {
        resolver.write_if_changed()?;
    }
    Ok((
        CompiledGraph {
            projects: compiler.projects,
        },
        compiler.documentation,
    ))
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
    compile_aliased_dependency_graph_inner(
        dependencies.into_iter().map(|(name, path)| {
            let name = name.into();
            (name.clone(), name, path.as_ref().to_path_buf())
        }),
        None,
    )
}

/// Compile dependency roots while confining every configured root and module
/// source read to `sandbox`. The boundary is canonicalized once and each file
/// is canonicalized immediately before it is read, so symlinked module
/// candidates cannot escape it.
pub fn compile_sandboxed_dependency_graph<I, N, P>(
    dependencies: I,
    sandbox: impl AsRef<Path>,
) -> Result<(CompiledGraph, Vec<Dependency>), CompileError>
where
    I: IntoIterator<Item = (N, P)>,
    N: Into<String>,
    P: AsRef<Path>,
{
    let roots = dependencies.into_iter().map(|(name, path)| {
        let name = name.into();
        (name.clone(), name, path.as_ref().to_path_buf())
    });
    compile_sandboxed_aliased_dependency_graph(roots, sandbox)
}

/// Resolve path and Git dependency specifications for a debugger project.
/// Local paths remain confined to `sandbox`; fetched Git trees are confined to
/// their immutable cache checkout. The project's `Ruddy.lock` is updated only
/// after the complete dependency graph compiles.
pub fn compile_sandboxed_dependency_specs<I, A>(
    dependencies: I,
    project: impl AsRef<Path>,
    sandbox: impl AsRef<Path>,
    build: Build,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError>
where
    I: IntoIterator<Item = (A, DependencySpec)>,
    A: Into<String>,
{
    compile_sandboxed_dependency_specs_inner(
        dependencies
            .into_iter()
            .map(|(alias, specification)| (alias.into(), specification, false)),
        project.as_ref(),
        sandbox.as_ref(),
        build,
    )
}

/// Resolve a debugger project's implicit standard library and declarations,
/// compiling every one of them for `build`, the project's own.
///
/// Local roots remain inside `sandbox`; Git dependencies use the shared cache.
pub fn compile_sandboxed_project_dependencies<I, A>(
    std: &StdConfig,
    dependencies: I,
    project: impl AsRef<Path>,
    sandbox: impl AsRef<Path>,
    build: Build,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError>
where
    I: IntoIterator<Item = (A, DependencySpec)>,
    A: Into<String>,
{
    let specifications = dependency_specs(
        std,
        dependencies
            .into_iter()
            .map(|(alias, spec)| (alias.into(), spec)),
    )?;
    compile_sandboxed_dependency_specs_inner(
        specifications,
        project.as_ref(),
        sandbox.as_ref(),
        build,
    )
}

fn dependency_specs(
    std: &StdConfig,
    dependencies: impl IntoIterator<Item = (String, DependencySpec)>,
) -> Result<Vec<(String, DependencySpec, bool)>, CompileError> {
    let mut specifications = Vec::new();
    match std {
        StdConfig::Default => {
            let specification = DependencySpec::Detailed(DependencyDetail {
                bundle: None,
                path: None,
                git: Some(DEFAULT_STD_GIT.into()),
                branch: None,
                tag: None,
                rev: None,
            });
            specifications.push(("std".to_string(), specification, true));
        }
        StdConfig::Disabled => {}
        StdConfig::Dependency(specification) => {
            specifications.push(("std".to_string(), specification.clone(), false));
        }
    }
    for (alias, specification) in dependencies {
        if alias == "std" {
            return Err(CompileError::report(
                "dependency-alias-reserved",
                "`std` is reserved for the standard-library dependency",
            )
            .with_help("configure it as `[dependencies].std`, not as a separate dependency"));
        }
        specifications.push((alias, specification, false));
    }
    Ok(specifications)
}

fn compile_sandboxed_dependency_specs_inner<I>(
    dependencies: I,
    project: &Path,
    sandbox: &Path,
    build: Build,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError>
where
    I: IntoIterator<Item = (String, DependencySpec, bool)>,
{
    let sandbox = fs::canonicalize(sandbox).map_err(|error| {
        CompileError::report(
            "workspace-unavailable",
            format!("could not open debugger workspace `{}`", sandbox.display()),
        )
        .with_note(error.to_string())
    })?;
    let project = canonical_project_in(project, Some(&sandbox))?;
    let resolver = git::Resolver::new(&project)?;
    let mut compiler = GraphCompiler {
        // A sandboxed compile writes nowhere outside its sandbox, its cache
        // included.
        cache: Some(cache::ArtifactCache::at(
            sandbox.join(".cache").join("artifacts"),
        )),
        sandbox: Some(sandbox),
        resolver: Some(resolver),
        build: Some(build),
        ..GraphCompiler::default()
    };
    let mut direct = Vec::new();
    let mut paths = Vec::new();
    for (alias, specification, default_std) in dependencies {
        specification.validate().map_err(|error| {
            dependency_error(
                &alias,
                specification.bundle(&alias),
                Path::new("<source>"),
                &project,
                error,
            )
        })?;
        if !source_identifier(&alias) {
            return Err(invalid_dependency_alias(&alias));
        }
        let expected = specification.bundle(&alias).to_string();
        let declared = specification
            .path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(specification.git().unwrap_or("<source>")));
        let contextualize = |error| {
            let error = dependency_error(&alias, &expected, &declared, &project, error);
            if default_std {
                default_std_error(error)
            } else {
                error
            }
        };
        let directory = if let Some(path) = specification.path() {
            canonical_project_in(&project.join(path), compiler.sandbox.as_deref())
                .map_err(&contextualize)?
        } else {
            let path = compiler
                .resolver
                .as_mut()
                .expect("resolver")
                .resolve(&specification)
                .map_err(&contextualize)?;
            let path = canonical_project(&path).map_err(&contextualize)?;
            compiler.git_roots.push(path.clone());
            path
        };
        let manifest = load_manifest(
            &directory,
            compiler
                .git_roots
                .iter()
                .find(|root| directory.starts_with(root))
                .map(PathBuf::as_path)
                .or(compiler.sandbox.as_deref()),
        )
        .map_err(&contextualize)?;
        if manifest.name != expected {
            let error = dependency_name_mismatch(
                &alias,
                &expected,
                &manifest.name,
                &directory.join(MANIFEST),
            );
            return Err(if default_std {
                default_std_error(error)
            } else {
                error
            });
        }
        let index = compiler
            .visit(directory.clone(), Some((alias.clone(), declared.clone())))
            .map_err(&contextualize)?;
        let artifact = &compiler.projects[index].artifact;
        direct.push(Dependency {
            name: artifact.header().identity.name.clone(),
            version: artifact.header().identity.version.clone(),
        });
        paths.push(directory);
    }
    if let Some(resolver) = &compiler.resolver {
        resolver.write_if_changed()?;
    }
    Ok((
        CompiledGraph {
            projects: compiler.projects,
        },
        direct,
        paths,
    ))
}

/// Compile dependency roots whose source aliases differ from bundle names.
pub fn compile_sandboxed_aliased_dependency_graph<I, A, N, P>(
    dependencies: I,
    sandbox: impl AsRef<Path>,
) -> Result<(CompiledGraph, Vec<Dependency>), CompileError>
where
    I: IntoIterator<Item = (A, N, P)>,
    A: Into<String>,
    N: Into<String>,
    P: AsRef<Path>,
{
    let sandbox = fs::canonicalize(sandbox.as_ref()).map_err(|error| {
        CompileError::report(
            "workspace-unavailable",
            format!(
                "could not open debugger workspace `{}`",
                sandbox.as_ref().display()
            ),
        )
        .with_note(error.to_string())
    })?;
    compile_aliased_dependency_graph_inner(
        dependencies.into_iter().map(|(alias, bundle, path)| {
            (alias.into(), bundle.into(), path.as_ref().to_path_buf())
        }),
        Some(sandbox),
    )
}

fn compile_aliased_dependency_graph_inner<I>(
    dependencies: I,
    sandbox: Option<PathBuf>,
) -> Result<(CompiledGraph, Vec<Dependency>), CompileError>
where
    I: IntoIterator<Item = (String, String, PathBuf)>,
{
    let mut compiler = GraphCompiler {
        sandbox,
        ..GraphCompiler::default()
    };
    let mut direct = Vec::new();
    for (alias, expected, directory) in dependencies {
        if !source_identifier(&alias) {
            return Err(invalid_dependency_alias(&alias));
        }
        let context = format!(
            "while loading dependency `{alias}` as project `{expected}` from `{}`",
            directory.display()
        );
        let contextualize = |error: CompileError| error.with_note(context.clone());
        let directory = canonical_project_in(&directory, compiler.sandbox.as_deref())
            .map_err(&contextualize)?;
        let manifest =
            load_manifest(&directory, compiler.sandbox.as_deref()).map_err(&contextualize)?;
        if manifest.name != expected {
            return Err(dependency_name_mismatch(
                &alias,
                &expected,
                &manifest.name,
                &directory.join(MANIFEST),
            ));
        }
        let index = compiler
            .visit(directory, Some((alias, PathBuf::new())))
            .map_err(&contextualize)?;
        let artifact = &compiler.projects[index].artifact;
        direct.push(Dependency {
            name: artifact.header().identity.name.clone(),
            version: artifact.header().identity.version.clone(),
        });
    }
    Ok((
        CompiledGraph {
            projects: compiler.projects,
        },
        direct,
    ))
}

struct GraphCompiler {
    document: bool,
    documentation: Vec<documentation::Page>,
    sandbox: Option<PathBuf>,
    resolver: Option<git::Resolver>,
    root: Option<PathBuf>,
    git_cache_root: Option<PathBuf>,
    git_roots: Vec<PathBuf>,
    completed: HashMap<PathBuf, usize>,
    identities: HashMap<(String, String), PathBuf>,
    active: Vec<(PathBuf, String)>,
    projects: Vec<CompiledProject>,
    /// Where dependency artifacts are kept between runs, when they are.
    cache: Option<cache::ArtifactCache>,
    /// The cache key of each project in `projects` that has one: every
    /// dependency, since a root is what the run is for and never cached.
    keys: HashMap<usize, u64>,
    /// The root's build, which every project in the graph is compiled for: a
    /// dependency's `@if` guards are judged against it, not against the
    /// dependency's own manifest. `None` when the graph has no single root,
    /// in which case each root's own manifest decides.
    build: Option<Build>,
}

impl Default for GraphCompiler {
    fn default() -> Self {
        Self {
            document: false,
            documentation: Vec::new(),
            sandbox: None,
            resolver: None,
            root: None,
            git_cache_root: git::canonical_checkouts_root(),
            git_roots: Vec::new(),
            completed: HashMap::new(),
            identities: HashMap::new(),
            active: Vec::new(),
            projects: Vec::new(),
            cache: None,
            keys: HashMap::new(),
            build: None,
        }
    }
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
                .map(|(_, name)| format!("`{name}`"))
                .collect();
            if let Some((name, _)) = edge {
                chain.push(format!("`{name}`"));
            }
            return Err(CompileError::report(
                "dependency-cycle",
                format!("dependency cycle: {}", chain.join(" → ")),
            )
            .with_help("remove or replace one dependency in this cycle"));
        }

        let boundary = self
            .git_roots
            .iter()
            .find(|root| directory.starts_with(root))
            .cloned()
            .or_else(|| self.sandbox.clone());
        let manifest = load_manifest(&directory, boundary.as_deref())?;
        let identity = configured_identity(&manifest.name, &manifest.version)?;
        let identity_key = (manifest.name.clone(), manifest.version.clone());
        if let Some(previous) = self.identities.get(&identity_key)
            && previous != &directory
        {
            return Err(CompileError::report(
                "project-identity-conflict",
                format!(
                    "two projects declare the same name and version `{}@{}`",
                    manifest.name, manifest.version
                ),
            )
            .with_note(format!("first project: `{}`", previous.display()))
            .with_note(format!("second project: `{}`", directory.display()))
            .with_help("change one project's `name` or `version` in `Ruddy.toml`"));
        }
        self.identities.insert(identity_key, directory.clone());
        let active_name = edge
            .as_ref()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| manifest.name.clone());
        self.active.push((directory.clone(), active_name));

        // Synthesize std before declared dependencies for deterministic graph,
        // header, and source-import order. Each visited manifest makes this
        // decision independently, including path and Git dependencies.
        let dependencies = dependency_specs(
            &manifest.dependencies.std,
            manifest
                .dependencies
                .declared
                .iter()
                .map(|(alias, spec)| (alias.clone(), spec.clone())),
        )?;

        let mut dependency_artifacts = Vec::with_capacity(dependencies.len());
        let mut dependency_indices = Vec::with_capacity(dependencies.len());
        for (alias, specification, default_std) in &dependencies {
            let expected = specification.bundle(alias).to_string();
            if let Err(error) = specification.validate() {
                self.active.pop();
                let error =
                    dependency_error(alias, &expected, Path::new("<source>"), &directory, error);
                return Err(if *default_std {
                    default_std_error(error)
                } else {
                    error
                });
            }
            if !source_identifier(alias) {
                self.active.pop();
                return Err(invalid_dependency_alias(alias));
            }
            let declared = specification
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(specification.git().unwrap_or("<source>")));
            let child = if let Some(path) = specification.path() {
                canonical_project_in(&directory.join(path), boundary.as_deref())
            } else {
                self.resolver
                    .as_mut()
                    .ok_or_else(|| {
                        CompileError::report(
                            "git-dependency-unavailable",
                            "Git dependencies are not available in this compilation mode",
                        )
                    })
                    .and_then(|resolver| resolver.resolve(specification))
                    .and_then(|path| canonical_project(&path))
                    .inspect(|child| {
                        if !self.git_roots.contains(child) {
                            self.git_roots.push(child.clone());
                        }
                    })
            }
            .map_err(|error: CompileError| {
                let error = dependency_error(alias, &expected, &declared, &directory, error);
                if *default_std {
                    default_std_error(error)
                } else {
                    error
                }
            })?;
            let child_boundary = self
                .git_roots
                .iter()
                .find(|root| child.starts_with(root))
                .map(PathBuf::as_path)
                .or(self.sandbox.as_deref());
            let child_manifest = load_manifest(&child, child_boundary).map_err(|error| {
                let error = dependency_error(alias, &expected, &declared, &directory, error);
                if *default_std {
                    default_std_error(error)
                } else {
                    error
                }
            })?;
            if child_manifest.name != expected {
                self.active.pop();
                let error = dependency_name_mismatch(
                    alias,
                    &expected,
                    &child_manifest.name,
                    &child.join(MANIFEST),
                )
                .with_note(format!(
                    "declared at `{}` in `{}`",
                    declared.display(),
                    directory.join(MANIFEST).display()
                ));
                return Err(if *default_std {
                    default_std_error(error)
                } else {
                    error
                });
            }
            if child_manifest.kind == Kind::Executable {
                return Err(CompileError::report(
                    "executable-dependency",
                    format!("executable bundle `{expected}` cannot be a dependency"),
                ));
            }
            let index = self
                .visit(child, Some((alias.clone(), declared.clone())))
                .map_err(|error| {
                    let error = dependency_error(alias, &expected, &declared, &directory, error);
                    if *default_std {
                        default_std_error(error)
                    } else {
                        error
                    }
                })?;
            let child_artifact = &self.projects[index].artifact;
            if child_artifact.header().kind == Kind::Executable {
                return Err(CompileError::report(
                    "executable-dependency",
                    format!("executable bundle `{expected}` cannot be a dependency"),
                ));
            }
            dependency_artifacts.push((alias.clone(), child_artifact.clone()));
            dependency_indices.push(index);
        }

        let linked_artifacts = self
            .projects
            .iter()
            .map(|project| project.artifact.clone())
            .collect();
        let target = manifest.target();
        let run = manifest.run.clone();
        // What this project is compiled for: the root's build when there is
        // one root, and its own otherwise.
        let build = match self.build {
            Some(build) => build,
            None => manifest.build()?,
        };
        // A dependency is compiled only when the cache has no artifact for
        // this compiler, these sources, these dependencies and this build.
        // The root is what the run is for, and always compiled.
        let key = match &self.cache {
            Some(_) if edge.is_some() => dependency_indices
                .iter()
                .map(|index| self.keys.get(index).copied())
                .collect::<Option<Vec<u64>>>()
                .map(|children| cache::key(&directory, &children, build)),
            _ => None,
        };
        let cached = key.and_then(|key| self.cache.as_ref()?.load(key, &manifest.name));
        let artifact = match cached {
            Some(artifact) => artifact,
            None => {
                let artifact = compile_one(
                    &directory,
                    manifest,
                    identity,
                    dependency_artifacts,
                    linked_artifacts,
                    boundary.as_deref(),
                    build,
                    if self.document && self.root.as_ref() == Some(&directory) {
                        Some(&mut self.documentation)
                    } else {
                        None
                    },
                )?;
                if let (Some(key), Some(cache)) = (key, &mut self.cache) {
                    cache.store(key, &artifact);
                }
                artifact
            }
        };
        self.active.pop();
        let index = self.projects.len();
        if let Some(key) = key {
            self.keys.insert(index, key);
        }
        self.projects.push(CompiledProject {
            source: if self.root.as_ref() == Some(&directory) {
                ProjectSource::Local
            } else if self
                .git_roots
                .iter()
                .any(|root| directory.starts_with(root))
                || self
                    .git_cache_root
                    .as_ref()
                    .is_some_and(|root| directory.starts_with(root))
            {
                ProjectSource::GitCache
            } else {
                ProjectSource::Local
            },
            directory: directory.clone(),
            target,
            platform: build.platform,
            run,
            artifact,
        });
        self.completed.insert(directory, index);
        Ok(index)
    }
}

fn canonical_project(directory: &Path) -> Result<PathBuf, CompileError> {
    canonical_project_in(directory, None)
}

fn canonical_project_in(directory: &Path, sandbox: Option<&Path>) -> Result<PathBuf, CompileError> {
    let canonical = fs::canonicalize(directory).map_err(|error| {
        CompileError::report(
            "project-unavailable",
            format!("could not open project folder `{}`", directory.display()),
        )
        .with_note(error.to_string())
        .with_help("check that the dependency path exists and is readable")
    })?;
    if !canonical.is_dir() {
        return Err(CompileError::report(
            "project-not-a-folder",
            format!("project path `{}` is not a folder", directory.display()),
        )
        .with_help("point the dependency at the folder containing `Ruddy.toml`"));
    }
    if let Some(sandbox) = sandbox
        && !canonical.starts_with(sandbox)
    {
        return Err(CompileError::report(
            "project-outside-workspace",
            format!(
                "project `{}` is outside the debugger workspace `{}`",
                canonical.display(),
                sandbox.display()
            ),
        )
        .with_help("move the project under the workspace or use an HTTPS Git dependency"));
    }
    Ok(canonical)
}

fn default_std_error(error: CompileError) -> CompileError {
    error.map_diagnostics(|diagnostic| {
        diagnostic.with_help("check access to `https://github.com/logan-gatlin/ruddy.git`, configure `[dependencies].std` to use another project, or set it to `false`")
    })
}

fn dependency_error(
    alias: &str,
    expected: &str,
    declared: &Path,
    parent: &Path,
    error: CompileError,
) -> CompileError {
    let manifest = parent.join(MANIFEST);
    let context = if declared == Path::new("<source>") {
        format!(
            "dependency `{alias}` expects project `{expected}` in `{}`",
            manifest.display()
        )
    } else {
        format!(
            "dependency `{alias}` expects project `{expected}` at `{}`, declared in `{}`",
            redact_git_url(&declared.to_string_lossy()),
            manifest.display()
        )
    };
    error.map_diagnostics(|diagnostic| diagnostic.with_note(context.clone()))
}

/// Compile one project for `build`, the root's, against the artifacts of its
/// dependencies.
#[allow(clippy::too_many_arguments)]
fn compile_one(
    directory: &Path,
    manifest: Manifest,
    identity: Bundle,
    dependencies: Vec<(String, Artifact)>,
    linked: Vec<Artifact>,
    sandbox: Option<&Path>,
    build: Build,
    documentation: Option<&mut Vec<documentation::Page>>,
) -> Result<Artifact, CompileError> {
    if sandbox.is_some() && manifest.root.is_absolute() {
        return Err(CompileError::report(
            "project-root-outside-workspace",
            "a dependency project's `root` must be a relative path",
        )
        .with_help("write a path relative to the project's `Ruddy.toml`"));
    }
    let Some(name) = configured_file_name(&manifest.root) else {
        return Err(CompileError::report(
            "project-root-invalid",
            "`root` must name a Ruddy source file",
        )
        .with_help("set `root` to a file such as `main.rud`"));
    };
    let source_directory = manifest.root.parent().unwrap_or(Path::new(""));
    let root = directory.join(&manifest.root);
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));

    if let Some(workspace) = sandbox
        && let Ok(canonical_root) = fs::canonicalize(&root)
        && !canonical_root.starts_with(workspace)
    {
        return Err(CompileError::report(
            "project-root-outside-workspace",
            format!(
                "project root file `{}` is outside the debugger workspace `{}`",
                canonical_root.display(),
                workspace.display()
            ),
        )
        .with_help("move the source file under the project folder or choose another `root`"));
    }

    let disk = sandbox.map_or_else(
        || Disk::new(parent),
        |sandbox| Disk::sandboxed(parent, sandbox),
    );
    if disk.read(name).is_none() {
        return Err(CompileError::report(
            "project-root-missing",
            format!("project root file `{}` could not be read", root.display()),
        )
        .with_note(format!(
            "configured by `root` in `{}`",
            directory.join(MANIFEST).display()
        ))
        .with_help("create the file or update `root` in `Ruddy.toml`"));
    }

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &disk, name, &build.environment());

    // Parser recovery is useful for finding more syntax complaints, but its
    // placeholder statements are not an input to semantic phases. Apart from
    // avoiding cascades, this keeps IR, inference and pattern checking from
    // having to make promises about malformed syntax trees.
    let frontend_errors = loaded
        .loaded
        .iter()
        .map(|file| file.lex_errors.len() + file.parse_errors.len())
        .sum::<usize>()
        + loaded.errors.len();
    if frontend_errors != 0 {
        let mut diagnostics = Vec::with_capacity(frontend_errors);
        for file in &loaded.loaded {
            let mut source_errors = syntax_diagnostics(&file.lex_errors, &file.parse_errors);
            source_errors.extend(
                loaded
                    .errors
                    .iter()
                    .filter(|error| error.span.file_id == file.id)
                    .map(|error| ("bundle", error.diagnostic_in(source_directory))),
            );
            diagnostics.extend(source_diagnostics(
                &mut files,
                source_errors,
                source_directory,
            ));
        }
        return Err(CompileError::from_diagnostics(diagnostics));
    }

    let mint = Mint::new(identity);
    // The core seam receives one dependency graph. Direct interfaces select
    // their aliases from this linked collection rather than arriving as an
    // unrelated second slice. Every artifact here was validated when it was
    // compiled or read, so each crosses the seam as the proof it already is.
    let mut checked: Vec<&Artifact> = linked.iter().collect();
    for (_, dependency) in &dependencies {
        if !checked
            .iter()
            .any(|artifact| artifact.header().identity == dependency.header().identity)
        {
            checked.push(dependency);
        }
    }
    let dependencies: Vec<_> = checked
        .iter()
        .map(|artifact| ruddy::compile::Dependency {
            alias: dependencies
                .iter()
                .find(|(_, dependency)| artifact.header().identity == dependency.header().identity)
                .map(|(alias, _)| alias.as_str()),
            artifact: ruddy::compile::DependencyArtifact::Checked(artifact),
        })
        .collect();
    let doc_statements = documentation.as_ref().map(|_| loaded.stmts.clone());
    let accepted = ruddy::compile::compile_bound(
        mint,
        loaded.stmts,
        &dependencies,
        inference::Trace::Off,
        build.domains,
    );
    let accepted = match accepted {
        Ok(accepted) => accepted,
        Err(partial) => {
            let errors = partial.errors.len();
            if errors != 0 {
                let mut diagnostics = Vec::with_capacity(errors);
                let source = &partial.ir.source;
                for error in &partial.ir.errors {
                    diagnostics.push(source_diagnostic(
                        &mut files,
                        "ir",
                        &error.diagnostic(source),
                        source_directory,
                    ));
                }
                for error in partial.inference.errors() {
                    diagnostics.push(source_diagnostic(
                        &mut files,
                        "types",
                        &error.diagnostic(source),
                        source_directory,
                    ));
                }
                for error in &partial.patterns.errors {
                    diagnostics.push(diagnostic(
                        &mut files,
                        "patterns",
                        error.kind.code(),
                        source.span(error.at),
                        &error.kind,
                        source_directory,
                    ));
                }
                return Err(CompileError::from_diagnostics(diagnostics));
            }
            unreachable!("partial compilation always has an error")
        }
    };

    if let Some(pages) = documentation {
        *pages = documentation::render(&accepted, doc_statements.as_ref().unwrap(), &mut files)
            .map_err(|message| CompileError::report("documentation-invalid", message))?;
    }
    let artifact = match manifest.kind {
        Kind::Library => accepted.artifact().clone(),
        Kind::Executable => ruddy::entry::executable(accepted.artifact(), &checked)
            .map_err(|error| CompileError::report("invalid-entry-point", error.to_string()))?,
    };
    if manifest.target() == Target::Js {
        ruddy::backend::js::check_entry(&artifact, &checked).map_err(|error| {
            CompileError::report("unsupported-entry-effects", error.to_string())
        })?;
    }
    Ok(artifact)
}

fn toml_error_note(source: &str, error: &toml::de::Error) -> String {
    let Some(start) = error.span().map(|span| span.start) else {
        return error.message().to_string();
    };
    let before = &source[..start.min(source.len())];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit_once('\n')
        .map_or(before, |(_, line)| line)
        .chars()
        .count()
        + 1;
    format!("{} at line {line}, column {column}", error.message())
}

fn redact_git_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return if url.contains(['@', '?', '#']) {
            "<redacted dependency source>".to_string()
        } else {
            url.to_string()
        };
    };
    let without_fragment = rest.split_once('#').map_or(rest, |(before, _)| before);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(before, _)| before);
    let (authority, path) = without_query
        .split_once('/')
        .map_or((without_query, ""), |(authority, path)| (authority, path));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{path}")
    }
}

fn invalid_dependency_alias(alias: &str) -> CompileError {
    CompileError::report(
        "dependency-alias-invalid",
        format!("dependency alias `{alias}` cannot be used as a module name"),
    )
    .with_help(
        "use letters, digits, and `_`, beginning with a letter or `_`, and avoid Ruddy keywords",
    )
}

fn dependency_name_mismatch(
    alias: &str,
    expected: &str,
    found: &str,
    manifest: &Path,
) -> CompileError {
    CompileError::report(
        "dependency-name-mismatch",
        format!(
            "dependency `{alias}` expects project `{expected}`, but `{}` declares `{found}`",
            manifest.display()
        ),
    )
    .with_help(format!(
        "set this dependency's `bundle` to `{found}`, or change the dependency project's `name` to `{expected}`"
    ))
}

fn source_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !matches!(
            name,
            "_" | "let"
                | "in"
                | "type"
                | "end"
                | "with"
                | "match"
                | "fn"
                | "effect"
                | "handle"
                | "raise"
                | "and"
                | "or"
                | "xor"
                | "not"
                | "module"
                | "true"
                | "false"
        )
}

fn configured_identity(name: &str, configured_version: &str) -> Result<Bundle, CompileError> {
    let version = Version::parse(configured_version).map_err(|error| {
        CompileError::report(
            "project-version-invalid",
            format!("`{configured_version}` is not a valid project version"),
        )
        .with_help("use three numbers such as `1.2.3`")
        .with_note(error.to_string())
    })?;
    if !version.build.is_empty() {
        return Err(CompileError::report(
            "project-version-build-suffix",
            format!("project version `{version}` has an unsupported `+` suffix"),
        )
        .with_help("remove the `+...` suffix from `version`"));
    }
    Bundle::new(name, version).ok_or_else(|| {
        CompileError::report(
            "project-name-invalid",
            format!("`{name}` is not a valid project name"),
        )
        .with_help("start with an ASCII letter and use only ASCII letters, digits, `-`, or `_`")
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

fn load_manifest(directory: &Path, sandbox: Option<&Path>) -> Result<Manifest, CompileError> {
    let configured = directory.join(MANIFEST);
    let path = match sandbox {
        Some(sandbox) => {
            let canonical = fs::canonicalize(&configured).map_err(|error| {
                CompileError::report(
                    "manifest-unavailable",
                    format!("could not open `{}`", configured.display()),
                )
                .with_note(error.to_string())
                .with_help("check that the project contains a readable `Ruddy.toml`")
            })?;
            if !canonical.starts_with(sandbox) {
                return Err(CompileError::report(
                    "manifest-outside-workspace",
                    format!(
                        "project settings `{}` are outside the debugger workspace `{}`",
                        canonical.display(),
                        sandbox.display()
                    ),
                )
                .with_help("keep `Ruddy.toml` inside the dependency project folder"));
            }
            canonical
        }
        // Command-line builds intentionally retain their unrestricted root
        // semantics; only debugger callers opt into a filesystem boundary.
        None => configured,
    };
    let source = fs::read_to_string(&path).map_err(|error| {
        CompileError::report(
            "manifest-unreadable",
            format!("could not read `{}`", path.display()),
        )
        .with_note(error.to_string())
    })?;
    parse_manifest(directory, &source)
}

fn parse_manifest(directory: &Path, source: &str) -> Result<Manifest, CompileError> {
    let path = directory.join(MANIFEST);
    let manifest: Manifest = toml::from_str(source).map_err(|error| {
        CompileError::report(
            "manifest-invalid",
            format!("`{}` contains invalid project settings", path.display()),
        )
        .with_note(toml_error_note(source, &error))
        .with_help("fix the named field in `Ruddy.toml` and try again")
    })?;
    // The JavaScript backend's entry adapter and epilogue are Node's: they
    // write to `process.stdout` and call `process.exit`. Until there is a web
    // adapter, an executable for the web would be a Node program with the
    // wrong label, so it is refused here, where every build begins.
    if manifest.kind == Kind::Executable && manifest.platform() == Platform::Web {
        return Err(CompileError::report(
            "platform-unsupported",
            format!(
                "`{}` asks for an executable on the `web` platform, which is not supported yet",
                path.display()
            ),
        )
        .with_note("executables run under Node's entry adapter; a web library builds as any other")
        .with_help("set `platform = \"node\"`, or make the project a library"));
    }
    Ok(manifest)
}

/// One source file available while rendering a compiler diagnostic.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticSource<'a> {
    pub path: &'a str,
    pub source: &'a str,
}

/// One highlighted source range in a rendered compiler diagnostic.
#[derive(Debug, Clone)]
pub struct DiagnosticLabel<'a> {
    pub source: usize,
    pub range: Range<usize>,
    pub message: &'a str,
}

/// Render the source diagnostic shared by the CLI and debugger.
///
/// `primary` is absent for failures that do not belong to source text, such as
/// project configuration and linking errors. Related labels use a distinct
/// colour so the place that explains an error remains visually separate from
/// the place that raised it.
pub fn render_diagnostic(
    phase: &str,
    code: &str,
    message: &str,
    available: &[DiagnosticSource<'_>],
    primary: Option<&DiagnosticLabel<'_>>,
    related: &[DiagnosticLabel<'_>],
    color: bool,
) -> String {
    render_diagnostic_with_advice(
        phase,
        code,
        message,
        available,
        primary,
        related,
        None,
        &[],
        color,
    )
}

/// Render a diagnostic with optional actionable help and explanatory notes.
///
/// The shorter [`render_diagnostic`] entry point remains for callers whose
/// diagnostic model does not yet carry advice.
#[allow(clippy::too_many_arguments)] // Public rendering boundary keeps each presentation channel explicit.
pub fn render_diagnostic_with_advice(
    phase: &str,
    code: &str,
    message: &str,
    available: &[DiagnosticSource<'_>],
    primary: Option<&DiagnosticLabel<'_>>,
    related: &[DiagnosticLabel<'_>],
    help: Option<&str>,
    notes: &[&str],
    color: bool,
) -> String {
    let _ = phase;
    let Some(primary) = primary else {
        let heading = format!("[{code}] Error");
        let mut rendered = if color {
            format!("\x1b[31m{heading}:\x1b[0m {message}")
        } else {
            format!("{heading}: {message}")
        };
        if let Some(help) = help {
            rendered.push_str(&format!("\nhelp: {help}"));
        }
        for note in notes {
            rendered.push_str(&format!("\nnote: {note}"));
        }
        return rendered;
    };
    let source = available
        .get(primary.source)
        .expect("a diagnostic's primary source exists");
    let mut report = Report::build(
        ReportKind::Error,
        (source.path.to_string(), primary.range.clone()),
    )
    .with_code(code)
    .with_config(
        Config::default()
            .with_index_type(IndexType::Byte)
            .with_compact(true)
            .with_color(color),
    )
    .with_message(message);

    let mut primary_label =
        Label::new((source.path.to_string(), primary.range.clone())).with_color(Color::Red);
    // The report title already states the error. A label repeats text only
    // when it contributes distinct, location-specific information.
    if !primary.message.is_empty() && primary.message != message {
        primary_label = primary_label.with_message(primary.message);
    }
    report = report.with_label(primary_label);
    for label in related {
        let source = available
            .get(label.source)
            .expect("a diagnostic's related source exists");
        let mut related_label =
            Label::new((source.path.to_string(), label.range.clone())).with_color(Color::Blue);
        if !label.message.is_empty() {
            related_label = related_label.with_message(label.message);
        }
        report = report.with_label(related_label);
    }
    if let Some(help) = help {
        report = report.with_help(help);
    }
    for note in notes {
        report = report.with_note(note);
    }

    let mut rendered = Vec::new();
    report
        .finish()
        .write(
            sources(
                available
                    .iter()
                    .map(|source| (source.path.to_string(), source.source)),
            ),
            &mut rendered,
        )
        .expect("writing a diagnostic to memory cannot fail");
    String::from_utf8(rendered)
        .expect("Ariadne diagnostics are UTF-8")
        .trim_end()
        .to_owned()
}

/// The diagnostics of one file's lexical and syntactic errors, each with the
/// phase that found it.
fn syntax_diagnostics(
    lex_errors: &[ruddy::token::Error],
    parse_errors: &[ruddy::parse::Error],
) -> Vec<(&'static str, ui::Diagnostic)> {
    lex_errors
        .iter()
        .map(|error| ("lex", error.diagnostic()))
        .chain(
            parse_errors
                .iter()
                .map(|error| ("parse", error.diagnostic())),
        )
        .collect()
}

/// One file's diagnostics, rendered in source order.
fn source_diagnostics(
    files: &mut FileManager,
    mut errors: Vec<(&'static str, ui::Diagnostic)>,
    source_directory: &Path,
) -> Vec<CompileDiagnostic> {
    errors.sort_by_key(|(_, error)| error.primary.span.start);
    errors
        .iter()
        .map(|(stage, error)| source_diagnostic(files, stage, error, source_directory))
        .collect()
}

fn source_diagnostic(
    files: &mut FileManager,
    stage: &'static str,
    diagnostic: &ui::Diagnostic,
    source_directory: &Path,
) -> CompileDiagnostic {
    let primary_file_id = diagnostic.primary.span.file_id;
    let file = files.get_file(primary_file_id);
    let mut sources = vec![OwnedDiagnosticSource {
        path: source_directory
            .join(&file.path)
            .to_string_lossy()
            .into_owned(),
        source: file.content.clone(),
    }];
    let mut source_indexes = HashMap::from([(primary_file_id, 0)]);
    let primary = OwnedDiagnosticLabel {
        source: 0,
        range: diagnostic.primary.span.start..diagnostic.primary.span.end(),
        message: diagnostic.primary.message.clone(),
    };
    let mut related = Vec::with_capacity(diagnostic.related.len());
    for label in &diagnostic.related {
        // A generated recovery declaration has no source location to show.
        if label.span.is_generated() {
            continue;
        }
        let source = match source_indexes.get(&label.span.file_id) {
            Some(source) => *source,
            None => {
                let file = files.get_file(label.span.file_id);
                let source = sources.len();
                sources.push(OwnedDiagnosticSource {
                    path: source_directory
                        .join(&file.path)
                        .to_string_lossy()
                        .into_owned(),
                    source: file.content.clone(),
                });
                source_indexes.insert(label.span.file_id, source);
                source
            }
        };
        related.push(OwnedDiagnosticLabel {
            source,
            range: label.span.start..label.span.end(),
            message: label.message.clone(),
        });
    }
    CompileDiagnostic {
        stage,
        code: diagnostic.code,
        message: diagnostic.title.clone(),
        sources,
        primary: Some(primary),
        related,
        help: diagnostic.help.clone(),
        notes: diagnostic.notes.clone(),
    }
}

fn diagnostic(
    files: &mut FileManager,
    stage: &'static str,
    code: &'static str,
    span: Span,
    message: &impl fmt::Display,
    source_directory: &Path,
) -> CompileDiagnostic {
    let file = files.get_file(span.file_id);
    let message = message.to_string();
    CompileDiagnostic {
        stage,
        code,
        message: message.clone(),
        sources: vec![OwnedDiagnosticSource {
            path: source_directory
                .join(&file.path)
                .to_string_lossy()
                .into_owned(),
            source: file.content.clone(),
        }],
        primary: Some(OwnedDiagnosticLabel {
            source: 0,
            range: span.start..span.end(),
            message,
        }),
        related: Vec::new(),
        help: Vec::new(),
        notes: Vec::new(),
    }
}

/// Keep redirected output plain while respecting the conventional overrides.
/// The debugger asks [`render_diagnostic`] for colour explicitly because its
/// destination is a browser rather than this process's standard error stream.
pub fn stderr_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some()
        || std::env::var_os("CLICOLOR").is_some_and(|value| value == "0")
    {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
        return true;
    }
    std::io::stderr().is_terminal() && std::env::var_os("TERM").is_none_or(|value| value != "dumb")
}
