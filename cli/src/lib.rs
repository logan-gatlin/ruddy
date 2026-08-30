//! Filesystem-facing Ruddy compiler entry point used by the command-line tool.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::{OsStr, OsString},
    fmt, fs,
    fs::OpenOptions,
    io::{IsTerminal as _, Write as _},
    ops::Range,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    rc::Rc,
};

use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};
use boa_engine::{
    Context, JsError, JsValue, Module, Source,
    builtins::promise::{OperationType, Promise, PromiseState},
    context::{HostHooks, time::JsInstant},
    job::{GenericJob, IntervalJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob},
    module::SimpleModuleLoader,
    object::{JsObject, builtins::JsPromise},
};
use boa_runtime::{
    extensions::{ConsoleExtension, FetchExtension},
    fetch::BlockingReqwestFetcher,
};
use clap::{Parser, Subcommand};
use futures_concurrency::future::FutureGroup;
use futures_lite::{StreamExt, future};
use indexmap::IndexMap;
use ruddy::{
    artifact::{Artifact, Dependency},
    bundle::{self, Disk, Files},
    inference, ir, lir, patterns,
    symbol::{Bundle, Mint, Version},
    tracking::{FileManager, Span},
    ui,
};
use serde::{Deserialize, Serialize};

mod git;
pub use git::{LOCKFILE, LockedGit, LockedSelector, Lockfile, ruddy_home};

const MANIFEST: &str = "Ruddy.toml";
const ROOT: &str = "main.hc";
const GITIGNORE: &str = ".gitignore";
const BUILD_DIRECTORY: &str = "build";
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
    /// Build and execute a JavaScript-targeted project.
    Run,
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
    /// This JavaScript module was built and executed successfully.
    Ran(PathBuf),
}

/// A user-facing command-line or filesystem failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    rendered: String,
    usage: bool,
    exit_code: u8,
}

impl CliError {
    fn one(message: impl Into<String>) -> Self {
        Self {
            rendered: format!("error: {}", message.into()),
            usage: false,
            exit_code: 1,
        }
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
        Command::Run => run_project(current_directory).map(Outcome::Ran),
    }
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
    write_new_file(
        &manifest,
        &format!(
            "name = {name:?}\nversion = {INITIAL_VERSION:?}\nroot = \"main.hc\"\n\n[dependencies]\n"
        ),
    )?;
    write_new_file(&root, "let main = 0n\n")?;
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
            ruddy::backend::js::generate(&linked)
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
            project.artifact.header.identity.name
        ));
        let artifact = if Some(index) == last {
            &linked
        } else {
            &project.artifact
        };
        replace_file(&artifact_path, artifact.print().as_bytes())?;
        if Some(index) == last {
            if let Some(javascript) = &javascript {
                let path = build.join(format!("{}.js", project.artifact.header.identity.name));
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
/// Boa's standard runtime extensions; `[run].js` may select a shell runner.
pub fn run_project(directory: impl AsRef<Path>) -> Result<PathBuf, CliError> {
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
        None => execute_javascript_module(&javascript)?,
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
        Err(CliError::one(format!(
            "JavaScript runner `{runner}` exited with {status}"
        )))
    }
}

type RejectedPromises = Rc<RefCell<Vec<(JsObject<Promise>, JsValue)>>>;

#[derive(Default)]
struct RuntimeHooks {
    unhandled: RejectedPromises,
}

impl RuntimeHooks {
    fn first_unhandled(&self) -> Option<JsValue> {
        self.unhandled
            .borrow()
            .first()
            .map(|(_, reason)| reason.clone())
    }
}

impl HostHooks for RuntimeHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject<Promise>,
        operation: OperationType,
        _context: &mut Context,
    ) {
        match operation {
            OperationType::Reject => {
                let PromiseState::Rejected(reason) = JsPromise::from(promise.clone()).state()
                else {
                    return;
                };
                self.unhandled.borrow_mut().push((promise.clone(), reason));
            }
            OperationType::Handle => self
                .unhandled
                .borrow_mut()
                .retain(|(unhandled, _)| unhandled != promise),
        }
    }
}

#[derive(Debug)]
enum ClockJob {
    Timeout(TimeoutJob),
    Interval(IntervalJob),
}

impl ClockJob {
    fn cancelled(&self) -> bool {
        match self {
            Self::Timeout(job) => job.cancelled(),
            Self::Interval(job) => job.cancelled(),
        }
    }
}

/// Boa's simple executor with an observable ECMAScript microtask checkpoint.
///
/// Promise jobs always reach quiescence before a timer is dispatched. This is
/// the boundary at which the host reports rejected promises; a timer therefore
/// cannot retroactively handle a rejection from the preceding task. The other
/// queues deliberately mirror `SimpleJobExecutor`.
#[derive(Default)]
struct RuntimeJobExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    finalization_registry_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    clock_jobs: RefCell<BTreeMap<JsInstant, VecDeque<ClockJob>>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    unhandled: RejectedPromises,
}

impl RuntimeJobExecutor {
    fn new(unhandled: RejectedPromises) -> Self {
        Self {
            unhandled,
            ..Self::default()
        }
    }

    fn clear(&self) {
        self.promise_jobs.borrow_mut().clear();
        self.async_jobs.borrow_mut().clear();
        self.clock_jobs.borrow_mut().clear();
        self.generic_jobs.borrow_mut().clear();
        self.finalization_registry_jobs.borrow_mut().clear();
    }

    fn has_immediate_jobs(&self) -> bool {
        !self.promise_jobs.borrow().is_empty()
            || !self.async_jobs.borrow().is_empty()
            || !self.generic_jobs.borrow().is_empty()
    }

    fn run_microtask_checkpoint(
        &self,
        context: &RefCell<&mut Context>,
    ) -> boa_engine::JsResult<()> {
        loop {
            let jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
            if jobs.is_empty() {
                return Ok(());
            }
            for job in jobs {
                job.call(&mut context.borrow_mut())?;
            }
        }
    }

    fn prune_cancelled_clock_jobs(&self) {
        self.clock_jobs.borrow_mut().retain(|_, jobs| {
            jobs.retain(|job| !job.cancelled());
            !jobs.is_empty()
        });
    }

    fn pop_due_clock_job(&self, now: JsInstant) -> Option<ClockJob> {
        let mut clock_jobs = self.clock_jobs.borrow_mut();
        loop {
            let at = *clock_jobs.first_key_value()?.0;
            if at > now {
                return None;
            }
            let (job, empty) = {
                let jobs = clock_jobs.get_mut(&at).expect("deadline came from map");
                let job = jobs.pop_front();
                (job, jobs.is_empty())
            };
            if empty {
                clock_jobs.remove(&at);
            }
            if job.as_ref().is_some_and(|job| !job.cancelled()) {
                return job;
            }
        }
    }
}

impl JobExecutor for RuntimeJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(job) => {
                self.clock_jobs
                    .borrow_mut()
                    .entry(context.clock().now() + job.timeout())
                    .or_default()
                    .push_back(ClockJob::Timeout(job));
            }
            Job::IntervalJob(job) => {
                self.clock_jobs
                    .borrow_mut()
                    .entry(context.clock().now() + job.interval())
                    .or_default()
                    .push_back(ClockJob::Interval(job));
            }
            Job::GenericJob(job) => self.generic_jobs.borrow_mut().push_back(job),
            Job::FinalizationRegistryCleanupJob(job) => {
                self.finalization_registry_jobs.borrow_mut().push_back(job)
            }
            _ => unreachable!("Boa 0.22 job category"),
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> boa_engine::JsResult<()> {
        future::block_on(self.run_jobs_async(&RefCell::new(context)))
    }

    async fn run_jobs_async(
        self: Rc<Self>,
        context: &RefCell<&mut Context>,
    ) -> boa_engine::JsResult<()> {
        let mut async_jobs = FutureGroup::new();
        let mut finalization_jobs = FutureGroup::new();
        loop {
            for job in std::mem::take(&mut *self.async_jobs.borrow_mut()) {
                async_jobs.insert(job.call(context));
            }
            for job in std::mem::take(&mut *self.finalization_registry_jobs.borrow_mut()) {
                finalization_jobs.insert(job.call(context));
            }

            if let Err(error) = self.run_microtask_checkpoint(context) {
                self.clear();
                return Err(error);
            }
            if !self.unhandled.borrow().is_empty() {
                self.clear();
                return Ok(());
            }

            let jobs = std::mem::take(&mut *self.generic_jobs.borrow_mut());
            for job in jobs {
                if let Err(error) = job.call(&mut context.borrow_mut()) {
                    self.clear();
                    return Err(error);
                }
            }
            if !self.promise_jobs.borrow().is_empty() {
                context.borrow_mut().clear_kept_objects();
                continue;
            }

            let now = context.borrow().clock().now();
            if let Some(job) = self.pop_due_clock_job(now) {
                let result = match job {
                    ClockJob::Timeout(job) => job.call(&mut context.borrow_mut()),
                    ClockJob::Interval(job) => {
                        let interval = job.interval();
                        let result = job.call(&mut context.borrow_mut());
                        if result.is_ok() && !job.cancelled() {
                            self.clock_jobs
                                .borrow_mut()
                                .entry(now + interval)
                                .or_default()
                                .push_back(ClockJob::Interval(job));
                        }
                        result
                    }
                };
                if let Err(error) = result {
                    self.clear();
                    return Err(error);
                }
                // A timer callback is one host task. Its microtasks and promise
                // rejections become observable before any other work proceeds.
                if let Err(error) = self.run_microtask_checkpoint(context) {
                    self.clear();
                    return Err(error);
                }
                if !self.unhandled.borrow().is_empty() {
                    self.clear();
                    return Ok(());
                }
            }

            if let Some(Err(error)) = future::poll_once(async_jobs.next()).await.flatten() {
                self.clear();
                return Err(error);
            }
            context.borrow_mut().clear_kept_objects();

            if self.has_immediate_jobs() {
                continue;
            }
            if async_jobs.is_empty() {
                match future::poll_once(finalization_jobs.next()).await.flatten() {
                    Some(Err(error)) => {
                        self.clear();
                        return Err(error);
                    }
                    _ if self.has_immediate_jobs() => continue,
                    _ => {}
                }
                // A task can cancel a later timer after the due job was selected.
                // Remove it before choosing a deadline so cancellation never sleeps.
                self.prune_cancelled_clock_jobs();
                let deadline = self
                    .clock_jobs
                    .borrow()
                    .first_key_value()
                    .map(|(at, _)| *at);
                let Some(deadline) = deadline else {
                    break;
                };
                let now = context.borrow().clock().now();
                if deadline > now {
                    std::thread::sleep((deadline - now).into());
                }
            } else {
                future::yield_now().await;
            }
        }
        Ok(())
    }
}

/// Execute one JavaScript module with the same Boa runtime used by [`run_project`].
///
/// Module initialization and queued jobs run to completion. A rejected module
/// or a promise that remains unhandled after its microtask checkpoint is
/// returned as a runtime error.
pub fn execute_javascript_module(path: impl AsRef<Path>) -> Result<(), CliError> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| {
        CliError::one(format!("JavaScript path {} has no parent", path.display()))
    })?;
    let loader = Rc::new(SimpleModuleLoader::new(parent).map_err(boa_error)?);
    let hooks = Rc::new(RuntimeHooks::default());
    let executor = Rc::new(RuntimeJobExecutor::new(hooks.unhandled.clone()));
    let mut context = Context::builder()
        .module_loader(loader)
        .job_executor(executor.clone())
        .host_hooks(hooks.clone())
        .build()
        .map_err(boa_error)?;
    boa_runtime::register(
        (
            ConsoleExtension::default(),
            FetchExtension(BlockingReqwestFetcher::default()),
        ),
        None,
        &mut context,
    )
    .map_err(boa_error)?;
    let source = Source::from_filepath(path).map_err(|error| {
        CliError::one(format!(
            "could not read JavaScript module {}: {error}",
            path.display()
        ))
    })?;
    let module = Module::parse(source, None, &mut context).map_err(boa_error)?;
    let evaluation = module.load_link_evaluate(&mut context);
    if matches!(evaluation.state(), PromiseState::Pending) {
        context.run_jobs().map_err(boa_error)?;
    }
    let state = evaluation.state();
    if let PromiseState::Rejected(value) = state {
        return Err(boa_error(JsError::from_opaque(value)));
    }
    if let Some(reason) = hooks.first_unhandled() {
        return Err(CliError::one(format!(
            "JavaScript runtime: unhandled promise rejection: {}",
            JsError::from_opaque(reason)
        )));
    }
    match state {
        PromiseState::Fulfilled(_) => Ok(()),
        PromiseState::Pending => Err(CliError::one(
            "JavaScript module evaluation remained pending after the job queue completed",
        )),
        PromiseState::Rejected(_) => unreachable!("handled above"),
    }
}

fn boa_error(error: JsError) -> CliError {
    CliError::one(format!("JavaScript runtime: {error}"))
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Write only the canonical linked artifact.
    #[default]
    Lib,
    /// Write the canonical linked artifact and a JavaScript ESM module.
    Js,
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
    #[serde(default)]
    target: Target,
    #[serde(default)]
    run: RunConfig,
    dependencies: ManifestDependencies,
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
    /// Resolve `std` from `$RUDDY_HOME/std` (or `$HOME/.ruddy/std`).
    #[default]
    Default,
    /// Do not inject a standard-library dependency for this project.
    Disabled,
    /// Resolve `std` using the normal path or Git dependency machinery.
    Dependency(DependencySpec),
}

impl StdConfig {
    /// Whether this setting is the omitted, default-installed standard library.
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
            return Err(CompileError::one(
                "Git dependency has conflicting `branch`, `tag`, and `rev` selectors",
            ));
        }
        for value in selectors.iter().flatten() {
            if value.is_empty() {
                return Err(CompileError::one(
                    "Git dependency selectors must not be empty",
                ));
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
                return Err(CompileError::one(
                    "dependency cannot specify both `path` and `git`",
                ));
            }
            (false, None) => {
                return Err(CompileError::one(
                    "dependency must specify either `path` or `git`",
                ));
            }
            (_, Some(url)) if !url.starts_with("https://") => {
                return Err(CompileError::one(format!(
                    "Git dependency URL `{url}` must use HTTPS"
                )));
            }
            _ => {}
        }
        if detail.git.is_none()
            && (detail.branch.is_some() || detail.tag.is_some() || detail.rev.is_some())
        {
            return Err(CompileError::one(
                "dependency selectors require a `git` source",
            ));
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
    /// The implicitly selected standard library installed under Ruddy home.
    InstalledStd,
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
    ruddy::link::link(&artifacts).map_err(|error| CompileError::one(error.to_string()))
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
    let root = canonical_project(directory.as_ref())?;
    let resolver = git::Resolver::new(&root)?;
    let mut compiler = GraphCompiler {
        root: Some(root.clone()),
        resolver: Some(resolver),
        ..GraphCompiler::default()
    };
    compiler.visit(root, None)?;
    if let Some(resolver) = &compiler.resolver {
        resolver.write_if_changed()?;
    }
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
    )
}

/// Resolve a debugger project's implicit standard library and declarations.
///
/// Custom local roots remain inside `sandbox`. The only extra trusted tree is
/// the exact canonical `$RUDDY_HOME/std` root when `std` is defaulted; a custom
/// specification that happens to use the same alias receives no exception.
pub fn compile_sandboxed_project_dependencies<I, A>(
    std: &StdConfig,
    dependencies: I,
    project: impl AsRef<Path>,
    sandbox: impl AsRef<Path>,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError>
where
    I: IntoIterator<Item = (A, DependencySpec)>,
    A: Into<String>,
{
    let mut specifications = Vec::new();
    match std {
        StdConfig::Default => {
            let specification = ruddy_home()
                .map(|home| DependencySpec::Path(home.join("std")))
                .map_err(default_std_error)?;
            specifications.push(("std".to_string(), specification, true));
        }
        StdConfig::Disabled => {}
        StdConfig::Dependency(specification) => {
            specifications.push(("std".to_string(), specification.clone(), false));
        }
    }
    for (alias, specification) in dependencies {
        let alias = alias.into();
        if alias == "std" {
            return Err(CompileError::one(
                "dependency alias `std` is reserved for the standard-library setting under `[dependencies]`",
            ));
        }
        specifications.push((alias, specification, false));
    }
    compile_sandboxed_dependency_specs_inner(specifications, project.as_ref(), sandbox.as_ref())
}

fn compile_sandboxed_dependency_specs_inner<I>(
    dependencies: I,
    project: &Path,
    sandbox: &Path,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError>
where
    I: IntoIterator<Item = (String, DependencySpec, bool)>,
{
    let sandbox = fs::canonicalize(sandbox).map_err(|error| {
        CompileError::one(format!(
            "could not resolve sandbox folder {}: {error}",
            sandbox.display()
        ))
    })?;
    let project = canonical_project_in(project, Some(&sandbox))?;
    let resolver = git::Resolver::new(&project)?;
    let mut compiler = GraphCompiler {
        sandbox: Some(sandbox),
        resolver: Some(resolver),
        ..GraphCompiler::default()
    };
    let mut direct = Vec::new();
    let mut paths = Vec::new();
    for (alias, specification, installed_default) in dependencies {
        specification.validate()?;
        if !source_identifier(&alias) {
            return Err(CompileError::one(format!(
                "dependency alias `{alias}` is not a valid Ruddy source identifier"
            )));
        }
        let expected = specification.bundle(&alias).to_string();
        let directory = if let Some(path) = specification.path() {
            if installed_default {
                let directory =
                    canonical_project(&project.join(path)).map_err(default_std_error)?;
                compiler.installed_std_roots.push(directory.clone());
                directory
            } else {
                canonical_project_in(&project.join(path), compiler.sandbox.as_deref())?
            }
        } else {
            let path = compiler
                .resolver
                .as_mut()
                .expect("resolver")
                .resolve(&specification)?;
            let path = canonical_project(&path)?;
            compiler.git_roots.push(path.clone());
            path
        };
        let manifest = load_manifest(
            &directory,
            compiler
                .git_roots
                .iter()
                .chain(compiler.installed_std_roots.iter())
                .find(|root| directory.starts_with(root))
                .map(PathBuf::as_path)
                .or(compiler.sandbox.as_deref()),
        )
        .map_err(|error| {
            if installed_default {
                default_std_error(error)
            } else {
                error
            }
        })?;
        if manifest.name != expected {
            let error = CompileError::one(format!(
                "dependency key `{expected}` resolves to project `{}` instead",
                manifest.name
            ));
            return Err(if installed_default {
                default_std_error(error)
            } else {
                error
            });
        }
        let index = compiler
            .visit(directory.clone(), Some((expected, PathBuf::new())))
            .map_err(|error| {
                if installed_default {
                    default_std_error(error)
                } else {
                    error
                }
            })?;
        let artifact = &compiler.projects[index].artifact;
        direct.push(Dependency {
            name: artifact.header.identity.name.clone(),
            version: artifact.header.identity.version.clone(),
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
        CompileError::one(format!(
            "could not resolve sandbox folder {}: {error}",
            sandbox.as_ref().display()
        ))
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
            return Err(CompileError::one(format!(
                "dependency alias `{alias}` is not a valid Ruddy source identifier"
            )));
        }
        let directory = canonical_project_in(&directory, compiler.sandbox.as_deref())?;
        let manifest = load_manifest(&directory, compiler.sandbox.as_deref())?;
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

struct GraphCompiler {
    sandbox: Option<PathBuf>,
    resolver: Option<git::Resolver>,
    root: Option<PathBuf>,
    git_cache_root: Option<PathBuf>,
    git_roots: Vec<PathBuf>,
    installed_std_roots: Vec<PathBuf>,
    completed: HashMap<PathBuf, usize>,
    identities: HashMap<(String, String), PathBuf>,
    active: Vec<(PathBuf, String)>,
    projects: Vec<CompiledProject>,
}

impl Default for GraphCompiler {
    fn default() -> Self {
        Self {
            sandbox: None,
            resolver: None,
            root: None,
            git_cache_root: git::canonical_checkouts_root(),
            git_roots: Vec::new(),
            installed_std_roots: Vec::new(),
            completed: HashMap::new(),
            identities: HashMap::new(),
            active: Vec::new(),
            projects: Vec::new(),
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

        let boundary = self
            .git_roots
            .iter()
            .chain(
                self.installed_std_roots
                    .iter()
                    .filter(|_| self.sandbox.is_some()),
            )
            .find(|root| directory.starts_with(root))
            .cloned()
            .or_else(|| self.sandbox.clone());
        let manifest = load_manifest(&directory, boundary.as_deref())?;
        let identity = configured_identity(&manifest.name, &manifest.version)?;
        let identity_key = (manifest.name.clone(), manifest.version.clone());
        if let Some(previous) = self.identities.get(&identity_key)
            && previous != &directory
        {
            return Err(CompileError::one(format!(
                "projects {} and {} both declare bundle {}@{}",
                previous.display(),
                directory.display(),
                manifest.name,
                manifest.version
            )));
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
        let mut dependencies = Vec::with_capacity(manifest.dependencies.declared.len() + 1);
        match &manifest.dependencies.std {
            StdConfig::Default => {
                let specification = ruddy_home()
                    .map(|home| DependencySpec::Path(home.join("std")))
                    .map_err(default_std_error)?;
                dependencies.push(("std".to_string(), specification, true));
            }
            StdConfig::Disabled => {}
            StdConfig::Dependency(specification) => {
                dependencies.push(("std".to_string(), specification.clone(), false));
            }
        }
        dependencies.extend(
            manifest
                .dependencies
                .declared
                .iter()
                .map(|(alias, specification)| (alias.clone(), specification.clone(), false)),
        );

        let mut dependency_artifacts = Vec::with_capacity(dependencies.len());
        for (alias, specification, installed_default) in &dependencies {
            let expected = specification.bundle(alias).to_string();
            if let Err(error) = specification.validate() {
                self.active.pop();
                let error = dependency_error(&expected, Path::new("<source>"), &directory, error);
                return Err(if *installed_default {
                    default_std_error(error)
                } else {
                    error
                });
            }
            if !source_identifier(alias) {
                self.active.pop();
                return Err(CompileError::one(format!(
                    "dependency alias `{alias}` is not a valid Ruddy source identifier"
                )));
            }
            let declared = specification
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(specification.git().unwrap_or("<source>")));
            let child = if let Some(path) = specification.path() {
                // The installed default is the one narrowly trusted tree in a
                // sandboxed build. Configured path overrides remain confined to
                // the declaring project's existing boundary.
                let candidate = directory.join(path);
                let child = canonical_project_in(
                    &candidate,
                    if *installed_default {
                        None
                    } else {
                        boundary.as_deref()
                    },
                );
                if *installed_default {
                    child.inspect(|child| {
                        if !self.installed_std_roots.contains(child) {
                            self.installed_std_roots.push(child.clone());
                        }
                    })
                } else {
                    child
                }
            } else {
                self.resolver
                    .as_mut()
                    .ok_or_else(|| {
                        CompileError::one(
                            "Git dependencies are unavailable in this compilation mode",
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
                let error = dependency_error(&expected, &declared, &directory, error);
                if *installed_default {
                    default_std_error(error)
                } else {
                    error
                }
            })?;
            let child_boundary = self
                .git_roots
                .iter()
                .chain(
                    self.installed_std_roots
                        .iter()
                        .filter(|_| self.sandbox.is_some()),
                )
                .find(|root| child.starts_with(root))
                .map(PathBuf::as_path)
                .or(self.sandbox.as_deref());
            let child_manifest = load_manifest(&child, child_boundary).map_err(|error| {
                let error = dependency_error(&expected, &declared, &directory, error);
                if *installed_default {
                    default_std_error(error)
                } else {
                    error
                }
            })?;
            if child_manifest.name != expected {
                self.active.pop();
                let error = CompileError::one(format!(
                    "dependency `{expected}` declared as `{}` by {} contains project `{}` instead",
                    declared.display(),
                    directory.join(MANIFEST).display(),
                    child_manifest.name
                ));
                return Err(if *installed_default {
                    default_std_error(error)
                } else {
                    error
                });
            }
            let index = self
                .visit(child, Some((expected.clone(), declared.clone())))
                .map_err(|error| {
                    let error = dependency_error(&expected, &declared, &directory, error);
                    if *installed_default {
                        default_std_error(error)
                    } else {
                        error
                    }
                })?;
            let child_artifact = &self.projects[index].artifact;
            dependency_artifacts.push((alias.clone(), child_artifact.clone()));
        }

        let linked_artifacts = self
            .projects
            .iter()
            .map(|project| project.artifact.clone())
            .collect();
        let target = manifest.target;
        let run = manifest.run.clone();
        let artifact = compile_one(
            &directory,
            manifest,
            identity,
            dependency_artifacts,
            linked_artifacts,
            boundary.as_deref(),
        )?;
        self.active.pop();
        let index = self.projects.len();
        self.projects.push(CompiledProject {
            source: if self.root.as_ref() == Some(&directory) {
                ProjectSource::Local
            } else if self
                .installed_std_roots
                .iter()
                .any(|root| directory.starts_with(root))
            {
                ProjectSource::InstalledStd
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
    if let Some(sandbox) = sandbox
        && !canonical.starts_with(sandbox)
    {
        return Err(CompileError::one(format!(
            "project folder {} escapes sandbox {}",
            canonical.display(),
            sandbox.display()
        )));
    }
    Ok(canonical)
}

fn default_std_error(error: CompileError) -> CompileError {
    CompileError::diagnostics(
        error
            .messages
            .into_iter()
            .map(|message| {
                format!(
                    "{message}\nhelp: install the Ruddy standard library in $RUDDY_HOME/std, configure `[dependencies].std` to another dependency, or set it to `false`"
                )
            })
            .collect(),
    )
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
    dependencies: Vec<(String, Artifact)>,
    linked: Vec<Artifact>,
    sandbox: Option<&Path>,
) -> Result<Artifact, CompileError> {
    if sandbox.is_some() && manifest.root.is_absolute() {
        return Err(CompileError::one(
            "manifest field `root` must be relative in a sandboxed build",
        ));
    }
    let Some(name) = configured_file_name(&manifest.root) else {
        return Err(CompileError::one("manifest field `root` must name a file"));
    };
    let source_directory = manifest.root.parent().unwrap_or(Path::new(""));
    let root = directory.join(&manifest.root);
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));

    let disk = sandbox.map_or_else(
        || Disk::new(parent),
        |sandbox| Disk::sandboxed(parent, sandbox),
    );
    if disk.read(name).is_none() {
        return Err(CompileError::one(format!(
            "could not read bundle root {} configured by {}",
            root.display(),
            directory.join(MANIFEST).display()
        )));
    }

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &disk, name);

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
            let mut source_errors: Vec<_> = file
                .lex_errors
                .iter()
                .map(|error| error.diagnostic())
                .chain(file.parse_errors.iter().map(|error| error.diagnostic()))
                .collect();
            source_errors.sort_by_key(|error| error.primary.span.start);
            diagnostics.extend(
                source_errors
                    .iter()
                    .map(|error| source_diagnostic(&mut files, error, source_directory)),
            );
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
        return Err(CompileError::diagnostics(diagnostics));
    }

    let mut mint = Mint::new(identity);
    let imports: Vec<_> = dependencies
        .iter()
        .map(|(alias, artifact)| ir::DependencyImport { alias, artifact })
        .collect();
    let mut built = ir::build_with_dependency_imports(&mut mint, loaded.stmts, &imports, &linked);
    let inferred = inference::infer(&mint, &mut built.program);
    let checked = patterns::check(&built.program, &inferred);

    let errors = built.errors.len() + inferred.errors.len() + checked.errors.len();
    if errors != 0 {
        let mut diagnostics = Vec::with_capacity(errors);
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
    let identities = dependencies
        .iter()
        .map(|(_, artifact)| Dependency {
            name: artifact.header.identity.name.clone(),
            version: artifact.header.identity.version.clone(),
        })
        .collect();
    Ok(ruddy::artifact::build_with_dependencies(
        &mint,
        &built.program,
        &inferred,
        &lowered,
        identities,
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

fn load_manifest(directory: &Path, sandbox: Option<&Path>) -> Result<Manifest, CompileError> {
    let configured = directory.join(MANIFEST);
    let path = match sandbox {
        Some(sandbox) => {
            let canonical = fs::canonicalize(&configured).map_err(|error| {
                CompileError::one(format!(
                    "could not resolve manifest {}: {error}",
                    configured.display()
                ))
            })?;
            if !canonical.starts_with(sandbox) {
                return Err(CompileError::one(format!(
                    "manifest {} escapes sandbox {}",
                    canonical.display(),
                    sandbox.display()
                )));
            }
            canonical
        }
        // Command-line builds intentionally retain their unrestricted root
        // semantics; only debugger callers opt into a filesystem boundary.
        None => configured,
    };
    let source = fs::read_to_string(&path).map_err(|error| {
        CompileError::one(format!(
            "could not read manifest {}: {error}",
            path.display()
        ))
    })?;
    let manifest: Manifest = toml::from_str(&source).map_err(|error| {
        CompileError::one(format!(
            "could not parse manifest {}: {error}",
            path.display()
        ))
    })?;
    Ok(manifest)
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
    let kind = format!("error[{code}]");
    let Some(primary) = primary else {
        let mut rendered = if color {
            format!("\x1b[31m{kind}:\x1b[0m {message}")
        } else {
            format!("{kind}: {message}")
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

fn source_diagnostic(
    files: &mut FileManager,
    diagnostic: &ui::Diagnostic,
    source_directory: &Path,
) -> String {
    let file = files.get_file(diagnostic.primary.span.file_id);
    let path = source_directory.join(&file.path);
    let path = path.to_string_lossy();
    let available = [DiagnosticSource {
        path: path.as_ref(),
        source: &file.content,
    }];
    let primary = DiagnosticLabel {
        source: 0,
        range: diagnostic.primary.span.start..diagnostic.primary.span.end(),
        message: &diagnostic.primary.message,
    };
    let related: Vec<_> = diagnostic
        .related
        .iter()
        .filter(|label| label.span.file_id == diagnostic.primary.span.file_id)
        .map(|label| DiagnosticLabel {
            source: 0,
            range: label.span.start..label.span.end(),
            message: &label.message,
        })
        .collect();
    let help = diagnostic.help.first().map(String::as_str);
    let notes: Vec<_> = diagnostic.notes.iter().map(String::as_str).collect();
    render_diagnostic_with_advice(
        "",
        diagnostic.code,
        &diagnostic.title,
        &available,
        Some(&primary),
        &related,
        help,
        &notes,
        stderr_color(),
    )
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
    let path = source_directory.join(&file.path);
    let path = path.to_string_lossy();
    let message = message.to_string();
    let available = [DiagnosticSource {
        path: path.as_ref(),
        source: &file.content,
    }];
    let primary = DiagnosticLabel {
        source: 0,
        range: span.start..span.end(),
        message: &message,
    };
    render_diagnostic(
        phase,
        code,
        &message,
        &available,
        Some(&primary),
        &[],
        stderr_color(),
    )
}

/// Keep redirected output plain while respecting the conventional overrides.
/// The debugger asks [`render_diagnostic`] for colour explicitly because its
/// destination is a browser rather than this process's standard error stream.
fn stderr_color() -> bool {
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
