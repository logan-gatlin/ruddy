//! The dependency graphs this debugger process has already compiled.
//!
//! Every compile request rebuilds the standard library and every declared
//! dependency from source, and for a one-line program that was most of the
//! wall time. The graph a request needs depends only on what the request asks
//! for and on what is in the project directories the answer was read from, so
//! the last answer is kept and handed back while both are unchanged.
//!
//! Nothing here touches the disk except to re-read the inputs. The memo lives
//! in this process and dies with it, and `just dev` restarts the server on
//! every compiler change, so an artifact built by one compiler can never be
//! read by another — which is the failure a persisted cache invited before.

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use indexmap::IndexMap;
use ruddy::artifact::Dependency;
use ruddy_cli::{CompileError, CompiledGraph, DependencySpec, StdConfig, Target, fingerprint};

/// The most requests remembered before the memo is emptied. A browser debugs
/// one document at a time, so this is far more than one ever needs; the
/// bound only keeps a long test run from accumulating graphs.
const CAPACITY: usize = 8;

/// One remembered answer and what it was read from.
struct Entry {
    /// Every project directory the graph compiled, plus the project's own
    /// lockfile: the files under these are everything the compile read.
    inputs: Vec<PathBuf>,
    fingerprint: u64,
    graph: CompiledGraph,
    direct: Vec<Dependency>,
    paths: Vec<PathBuf>,
}

static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What [`ruddy_cli::compile_sandboxed_project_dependencies`] would answer,
/// compiled only when the request or the sources it reads have changed since
/// this process last answered it.
///
/// Only successes are remembered: an error is cheap to reproduce and its
/// wording may depend on state the fingerprint does not see.
pub fn compile(
    std: &StdConfig,
    dependencies: &IndexMap<String, DependencySpec>,
    project: &Path,
    scratch: &Path,
    target: Target,
) -> Result<(CompiledGraph, Vec<Dependency>, Vec<PathBuf>), CompileError> {
    let key = key(std, dependencies, project, scratch, target);
    if let Some(entry) = CACHE.lock().unwrap().get(&key)
        && fingerprint(&entry.inputs) == entry.fingerprint
    {
        return Ok((
            entry.graph.clone(),
            entry.direct.clone(),
            entry.paths.clone(),
        ));
    }

    let specifications = dependencies
        .iter()
        .map(|(alias, specification)| (alias.clone(), specification.clone()));
    let (graph, direct, paths) = ruddy_cli::compile_sandboxed_project_dependencies(
        std,
        specifications,
        project,
        scratch,
        target,
    )?;

    let mut inputs: Vec<PathBuf> = graph
        .projects
        .iter()
        .map(|project| project.directory.clone())
        .collect();
    inputs.push(project.join("Ruddy.lock"));
    let entry = Entry {
        fingerprint: fingerprint(&inputs),
        inputs,
        graph: graph.clone(),
        direct: direct.clone(),
        paths: paths.clone(),
    };
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CAPACITY {
        cache.clear();
    }
    cache.insert(key, entry);
    Ok((graph, direct, paths))
}

/// Everything about a request that decides which graph it gets, before any
/// file is read: the configuration, the target its dependencies are compiled
/// for, where it is resolved from, and the environment that locates the
/// installed standard library.
fn key(
    std: &StdConfig,
    dependencies: &IndexMap<String, DependencySpec>,
    project: &Path,
    scratch: &Path,
    target: Target,
) -> String {
    // `Debug` rather than the wire form: the default standard library has no
    // wire form of its own, being the field's absence.
    format!(
        "{std:?}\n{dependencies:?}\n{}\n{}\n{}\n{:?}\n{:?}",
        target.name(),
        project.display(),
        scratch.display(),
        env::var_os("RUDDY_HOME"),
        env::var_os("HOME"),
    )
}
