//! Filesystem acquisition for one editor build context. The compiler sees only
//! explicit inputs; overlays apply throughout reachable local dependencies.
use super::*;
use ruddy::{
    analysis::{Analysis, Host},
    artifact::{Artifact, Header},
    ir,
};
use std::collections::HashSet;

pub struct ProjectAnalysis {
    pub directory: PathBuf,
    pub source_directory: PathBuf,
    analysis: Option<Analysis>,
    pub interface: Header,
    build: Build,
    manifest_source: String,
    focus: Option<String>,
    dependencies: Vec<(String, PathBuf, u64)>,
    inputs: HashMap<PathBuf, Option<String>>,
    interface_revision: u64,
    cache_key: u64,
    cacheable: bool,
    artifact: Option<Artifact>,
}

/// Work performed by a workspace refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub rebuilt: usize,
    pub reused: usize,
    /// Dependencies admitted from the persistent artifact cache.
    pub cached: usize,
    /// Projects whose current analysis has not yet been lowered.
    pub background: usize,
}

struct GitSelection {
    specification: DependencySpec,
    checkout: PathBuf,
    used: bool,
}

struct Visit {
    directory: PathBuf,
    cacheable: bool,
}

struct RefreshHistory<'a> {
    previous: &'a mut HashMap<PathBuf, ProjectAnalysis>,
    report: &'a mut RefreshReport,
}

pub struct Workspace {
    root: PathBuf,
    git_selections: Vec<GitSelection>,
    lock_source: Option<String>,
    focus: Option<PathBuf>,
    overlays: HashMap<PathBuf, String>,
    hosts: HashMap<PathBuf, Host>,
    pub projects: Vec<ProjectAnalysis>,
    next_interface_revision: u64,
    cache: Option<crate::cache::ArtifactCache>,
    cache_all_dependencies: bool,
    source_required: HashSet<PathBuf>,
    observed: std::cell::RefCell<HashMap<PathBuf, Option<String>>>,
}

impl ProjectAnalysis {
    pub fn analysis(&self) -> &Analysis {
        self.analysis
            .as_ref()
            .expect("source analysis was requested before it was read")
    }

    pub(crate) fn source_analysis(&self) -> Option<&Analysis> {
        self.analysis.as_ref()
    }
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        let cache = ruddy_home()
            .ok()
            .map(|home| crate::cache::ArtifactCache::at(home.join("cache").join("artifacts")));
        Self::with_cache(root, cache, false)
    }

    /// Construct an editor workspace with an isolated persistent artifact cache.
    pub fn with_artifact_cache(root: PathBuf, cache: PathBuf) -> Self {
        Self::with_cache(root, Some(crate::cache::ArtifactCache::at(cache)), true)
    }

    fn with_cache(
        root: PathBuf,
        cache: Option<crate::cache::ArtifactCache>,
        cache_all_dependencies: bool,
    ) -> Self {
        Self {
            root: fs::canonicalize(&root).unwrap_or_else(|_| normalize(&root)),
            git_selections: Vec::new(),
            lock_source: None,
            focus: None,
            overlays: HashMap::new(),
            hosts: HashMap::new(),
            projects: Vec::new(),
            next_interface_revision: 0,
            cache,
            cache_all_dependencies,
            source_required: HashSet::new(),
            observed: Default::default(),
        }
    }

    /// Select a file for foreground analysis. Expanding an existing partial
    /// analysis does not rebuild the project merely because the editor moved.
    pub fn focus(&mut self, path: &Path) {
        let path = file_identity(path);
        self.focus = Some(path.clone());
        for project in &mut self.projects {
            let Some(analysis) = project.analysis.as_mut() else {
                if path.starts_with(&project.source_directory) {
                    self.source_required.insert(project.directory.clone());
                }
                continue;
            };
            let Some(logical) = analysis
                .sources
                .keys()
                .find(|logical| file_identity(&project.source_directory.join(logical)) == path)
                .cloned()
            else {
                continue;
            };
            project.focus = Some(logical.clone());
            let host = self
                .hosts
                .get_mut(&project.directory)
                .expect("a current project host");
            host.focus(Some(&logical));
            if host.request_file(analysis, &logical) {
                let kind = project.interface.kind;
                let dependencies = project.interface.dependencies.clone();
                let mut interface = analysis.interface();
                interface.kind = kind;
                interface.dependencies = dependencies;
                if project.interface != interface {
                    self.next_interface_revision += 1;
                    project.interface_revision = self.next_interface_revision;
                }
                project.interface = interface;
                project.artifact = None;
            }
            break;
        }
    }

    /// Set or remove an open-buffer overlay, returning whether the effective
    /// source changed. Transport revisions whose text is identical need not
    /// invalidate semantic work.
    pub fn set_overlay(&mut self, path: &Path, text: Option<String>) -> bool {
        let path = file_identity(path);
        let disk = fs::read_to_string(&path).ok();
        let before = self.overlays.get(&path).cloned().or_else(|| disk.clone());
        match text {
            Some(text) => {
                self.overlays.insert(path.clone(), text);
            }
            None => {
                self.overlays.remove(&path);
            }
        }
        let after = self.overlays.get(&path).cloned().or(disk);
        before != after
    }

    fn read(&self, path: &Path) -> Option<String> {
        let disk = fs::read_to_string(path).ok();
        self.observed
            .borrow_mut()
            .insert(normalize(path), disk.clone());
        self.overlays.get(&file_identity(path)).cloned().or(disk)
    }

    fn manifest(&self, directory: &Path) -> Result<Manifest, CompileError> {
        let source = self.read(&directory.join(MANIFEST)).ok_or_else(|| {
            CompileError::report(
                "manifest-unreadable",
                format!("could not read `{}`", directory.join(MANIFEST).display()),
            )
        })?;
        parse_manifest(directory, &source)
    }

    /// Refresh the whole dependency graph against the root's build settings.
    /// Failed source definitions still publish explicit recovery interfaces.
    pub fn refresh(&mut self) -> Result<RefreshReport, CompileError> {
        let mut previous: HashMap<_, _> = std::mem::take(&mut self.projects)
            .into_iter()
            .map(|project| (project.directory.clone(), project))
            .collect();
        self.observed.borrow_mut().clear();
        let lock_source = self.read(&self.root.join("Ruddy.lock"));
        if self.lock_source != lock_source {
            self.git_selections.clear();
            self.lock_source = lock_source;
        }
        for selection in &mut self.git_selections {
            selection.used = false;
        }
        let build = self.manifest(&self.root)?.build()?;
        let mut resolver = None;
        let mut active = Vec::new();
        let mut projects = Vec::new();
        let mut report = RefreshReport::default();
        let mut history = RefreshHistory {
            previous: &mut previous,
            report: &mut report,
        };
        self.visit(
            Visit {
                directory: self.root.clone(),
                cacheable: false,
            },
            build,
            &mut resolver,
            &mut active,
            &mut projects,
            &mut history,
        )?;
        let reachable: std::collections::HashSet<_> = projects
            .iter()
            .map(|project: &ProjectAnalysis| project.directory.clone())
            .collect();
        self.hosts
            .retain(|directory, _| reachable.contains(directory));
        self.git_selections.retain(|selection| selection.used);
        report.background = projects
            .iter()
            .filter(|project| project.artifact.is_none())
            .count();
        self.projects = projects;
        Ok(report)
    }

    fn resolve_git(
        &mut self,
        specification: &DependencySpec,
        resolver: &mut Option<git::Resolver>,
    ) -> Result<PathBuf, CompileError> {
        if let Some(selection) = self.git_selections.iter_mut().find(|selection| {
            &selection.specification == specification && selection.checkout.is_dir()
        }) {
            selection.used = true;
            return Ok(selection.checkout.clone());
        }
        // Acquisition owns its resolver and advisory lock. A blocked network
        // read cannot hold up the editor: cancellation drops this receiver while
        // the acquisition thread observes the same signal and releases its lock.
        let mut acquisition = match resolver.take() {
            Some(resolver) => resolver,
            None => git::Resolver::new(&self.root)?,
        };
        let specification_owned = specification.clone();
        let cancellation = ruddy::cancellation::Cancellation::current();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = cancellation.run(|| {
                let result = acquisition.resolve(&specification_owned);
                (acquisition, result)
            });
            let _ = send.send(result);
        });
        let checkout = loop {
            ruddy::cancellation::checkpoint();
            match receive.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(Ok((returned, result))) => {
                    *resolver = Some(returned);
                    break result?;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                _ => {
                    ruddy::cancellation::checkpoint();
                    return Err(CompileError::report(
                        "git-acquisition-failed",
                        "Git dependency acquisition stopped",
                    ));
                }
            }
        };
        self.git_selections.push(GitSelection {
            specification: specification.clone(),
            checkout: checkout.clone(),
            used: true,
        });
        Ok(checkout)
    }

    fn visit(
        &mut self,
        visit: Visit,
        build: Build,
        resolver: &mut Option<git::Resolver>,
        active: &mut Vec<PathBuf>,
        projects: &mut Vec<ProjectAnalysis>,
        history: &mut RefreshHistory<'_>,
    ) -> Result<usize, CompileError> {
        ruddy::cancellation::checkpoint();
        let Visit {
            directory,
            cacheable,
        } = visit;
        let directory = fs::canonicalize(&directory).map_err(|error| {
            CompileError::report(
                "project-unavailable",
                format!("{}: {error}", directory.display()),
            )
        })?;
        if let Some(at) = projects
            .iter()
            .position(|project| project.directory == directory)
        {
            return Ok(at);
        }
        if active.contains(&directory) {
            return Err(CompileError::report(
                "dependency-cycle",
                format!("dependency cycle through `{}`", directory.display()),
            ));
        }
        active.push(directory.clone());
        let manifest_source = self.read(&directory.join(MANIFEST)).ok_or_else(|| {
            CompileError::report(
                "manifest-unreadable",
                format!("could not read `{}`", directory.join(MANIFEST).display()),
            )
        })?;
        let manifest = parse_manifest(&directory, &manifest_source)?;
        let identity = configured_identity(&manifest.name, &manifest.version)?;
        let specifications = dependency_specs(
            &manifest.dependencies.std,
            manifest
                .dependencies
                .declared
                .iter()
                .map(|(alias, spec)| (alias.clone(), spec.clone())),
        )?;
        let mut dependencies = Vec::new();
        for (alias, spec, default_std) in specifications {
            if !source_identifier(&alias) {
                return Err(invalid_dependency_alias(&alias));
            }
            spec.validate()?;
            let child = if let Some(path) = spec.path() {
                normalize(&directory.join(path))
            } else {
                self.resolve_git(&spec, resolver)?
            };
            let expected = spec.bundle(&alias);
            let child_manifest = self.manifest(&child)?;
            if child_manifest.name != expected {
                return Err(dependency_name_mismatch(
                    &alias,
                    expected,
                    &child_manifest.name,
                    &child.join(MANIFEST),
                ));
            }
            if child_manifest.kind == Kind::Executable {
                return Err(CompileError::report(
                    "executable-dependency",
                    format!("executable bundle `{expected}` cannot be a dependency"),
                ));
            }
            let at = self.visit(
                Visit {
                    directory: child,
                    cacheable: default_std || self.cache_all_dependencies,
                },
                build,
                resolver,
                active,
                projects,
                history,
            )?;
            dependencies.push((alias, at));
        }
        if projects.iter().any(|project| {
            project.interface.identity.name == manifest.name
                && project.interface.identity.version == manifest.version
        }) {
            return Err(CompileError::report(
                "project-identity-conflict",
                format!(
                    "two projects declare `{}@{}`",
                    manifest.name, manifest.version
                ),
            ));
        }
        let root = normalize(&directory.join(&manifest.root));
        let source_directory = root
            .parent()
            .ok_or_else(|| {
                CompileError::report(
                    "project-root-invalid",
                    "project root has no parent directory",
                )
            })?
            .to_path_buf();
        let name = configured_file_name(&root).ok_or_else(|| {
            CompileError::report(
                "project-root-invalid",
                "project root is not a source filename",
            )
        })?;
        let focus = self
            .focus
            .as_ref()
            .and_then(|path| path.strip_prefix(&source_directory).ok())
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| {
                history
                    .previous
                    .get(&directory)
                    .and_then(|project| project.focus.clone())
            });
        let dependency_interfaces: Vec<_> = dependencies
            .iter()
            .map(|(alias, at)| {
                (
                    alias.clone(),
                    projects[*at].directory.clone(),
                    projects[*at].interface_revision,
                )
            })
            .collect();
        if history.previous.get(&directory).is_some_and(|project| {
            project.build == build
                && project.manifest_source == manifest_source
                && project.source_directory == source_directory
                && (project.analysis.is_some() || !self.source_required.contains(&directory))
                && project.dependencies == dependency_interfaces
                && project
                    .inputs
                    .iter()
                    .all(|(path, source)| self.read(path) == *source)
        }) {
            active.pop();
            let at = projects.len();
            projects.push(
                history
                    .previous
                    .remove(&directory)
                    .expect("the reusable project is present"),
            );
            history.report.reused += 1;
            return Ok(at);
        }
        let dependency_keys: Vec<_> = dependencies
            .iter()
            .map(|(_, at)| projects[*at].cache_key)
            .collect();
        let cache_key = crate::cache::key(&directory, &dependency_keys, build);
        let may_use_cache = cacheable
            && directory != self.root
            && focus.is_none()
            && !self.source_required.contains(&directory)
            && !self
                .overlays
                .keys()
                .any(|path| path.starts_with(&source_directory));
        if may_use_cache
            && let Some(artifact) = self
                .cache
                .as_ref()
                .and_then(|cache| cache.load(cache_key, &manifest.name))
        {
            let mut inputs = HashMap::new();
            for path in crate::cache::input_paths(std::slice::from_ref(&directory)) {
                let source = fs::read_to_string(&path).ok();
                self.observed
                    .borrow_mut()
                    .insert(path.clone(), source.clone());
                inputs.insert(path, source);
            }
            let interface = artifact.header().clone();
            let interface_revision = history
                .previous
                .get(&directory)
                .filter(|project| project.interface == interface)
                .map(|project| project.interface_revision)
                .unwrap_or_else(|| {
                    self.next_interface_revision += 1;
                    self.next_interface_revision
                });
            active.pop();
            let at = projects.len();
            projects.push(ProjectAnalysis {
                directory,
                source_directory,
                analysis: None,
                interface,
                build,
                manifest_source,
                focus,
                dependencies: dependency_interfaces,
                inputs,
                interface_revision,
                cache_key,
                cacheable,
                artifact: Some(artifact),
            });
            history.report.cached += 1;
            return Ok(at);
        }
        struct Sources<'a> {
            directory: &'a Path,
            overlays: &'a HashMap<PathBuf, String>,
            observed: &'a std::cell::RefCell<HashMap<PathBuf, Option<String>>>,
            inputs: &'a std::cell::RefCell<HashMap<PathBuf, Option<String>>>,
        }
        impl Files for Sources<'_> {
            fn read(&self, path: &str) -> Option<String> {
                let path = normalize(&self.directory.join(path));
                let disk = fs::read_to_string(&path).ok();
                self.observed
                    .borrow_mut()
                    .insert(path.clone(), disk.clone());
                let source = self.overlays.get(&file_identity(&path)).cloned().or(disk);
                self.inputs.borrow_mut().insert(path, source.clone());
                source
            }
        }
        let inputs = std::cell::RefCell::new(HashMap::new());
        let sources = Sources {
            directory: &source_directory,
            overlays: &self.overlays,
            observed: &self.observed,
            inputs: &inputs,
        };
        if sources.read(name).is_none() {
            return Err(CompileError::report(
                "project-root-missing",
                format!("project root `{}` could not be read", root.display()),
            ));
        }
        let imports: Vec<_> = dependencies
            .iter()
            .map(|(alias, at)| ir::InterfaceImport {
                alias,
                header: &projects[*at].interface,
            })
            .collect();
        let linked: Vec<_> = projects.iter().map(|project| &project.interface).collect();
        let host = self.hosts.entry(directory.clone()).or_default();
        host.focus(focus.as_deref());
        let analysis = host.analyze_from_files(
            identity,
            name,
            &build.environment(),
            &sources,
            &imports,
            &linked,
        );
        let mut interface = analysis.interface();
        interface.kind = manifest.kind;
        interface.dependencies = dependencies
            .iter()
            .map(|(_, at)| {
                let identity = &projects[*at].interface.identity;
                Dependency {
                    name: identity.name.clone(),
                    version: identity.version.clone(),
                }
            })
            .collect();
        let interface_revision = history
            .previous
            .get(&directory)
            .filter(|project| project.interface == interface)
            .map(|project| project.interface_revision)
            .unwrap_or_else(|| {
                self.next_interface_revision += 1;
                self.next_interface_revision
            });
        active.pop();
        let at = projects.len();
        projects.push(ProjectAnalysis {
            directory,
            source_directory,
            analysis: Some(analysis),
            interface,
            build,
            manifest_source,
            focus,
            dependencies: dependency_interfaces,
            inputs: inputs.into_inner(),
            interface_revision,
            cache_key,
            cacheable,
            artifact: None,
        });
        history.report.rebuilt += 1;
        Ok(at)
    }

    /// Exact disk inputs acquired by the last refresh, including missing module
    /// candidates. The transport can watch these without scanning directories.
    pub fn observed_files(&self) -> HashMap<PathBuf, Option<String>> {
        self.observed.borrow().clone()
    }

    /// Lower and validate the current graph only when foreground requests are
    /// idle. Cancellation abandons these temporary artifacts, never the frontend.
    pub fn check_background(&mut self) -> Vec<String> {
        for project in &mut self.projects {
            if project.artifact.is_some() {
                continue;
            }
            ruddy::cancellation::checkpoint();
            let analysis = project
                .analysis
                .as_mut()
                .expect("an uncached project has source analysis");
            self.hosts
                .get_mut(&project.directory)
                .expect("a current project host")
                .complete(analysis);
            let mut interface = analysis.interface();
            interface.kind = project.interface.kind;
            interface.dependencies = project.interface.dependencies.clone();
            if project.interface != interface {
                self.next_interface_revision += 1;
                project.interface_revision = self.next_interface_revision;
            }
            project.interface = interface;
        }
        let mut errors = Vec::new();
        let mut extra = vec![Vec::new(); self.projects.len()];
        for (at, new_diagnostics) in extra.iter_mut().enumerate() {
            ruddy::cancellation::checkpoint();
            let (dependencies, current) = self.projects.split_at_mut(at);
            let project = &mut current[0];
            let linked: Vec<_> = dependencies
                .iter()
                .filter_map(|project| project.artifact.as_ref())
                .collect();
            if project.artifact.is_none() {
                let (artifact, diagnostics) = project
                    .analysis
                    .as_ref()
                    .expect("an uncached project has source analysis")
                    .lower(&linked);
                *new_diagnostics = diagnostics;
                let Some(artifact) = artifact else {
                    continue;
                };
                project.artifact = match project.interface.kind {
                    Kind::Library => Some(artifact),
                    Kind::Executable => match ruddy::entry::executable(&artifact, &linked) {
                        Ok(artifact) => Some(artifact),
                        Err(error) => {
                            errors.push(error.to_string());
                            None
                        }
                    },
                };
                if project.cacheable
                    && let (Some(cache), Some(artifact)) = (&mut self.cache, &project.artifact)
                {
                    cache.store(project.cache_key, artifact);
                }
            }
            if project.build.target == Target::Js
                && let Some(artifact) = &project.artifact
                && let Err(error) = ruddy::backend::js::check_entry(artifact, &linked)
            {
                errors.push(error.to_string());
            }
        }
        let artifacts: Vec<_> = self
            .projects
            .iter()
            .filter_map(|project| project.artifact.as_ref())
            .collect();
        if let (Some(project), Some((root, dependencies))) =
            (self.projects.last(), artifacts.split_last())
            && project.build.target == Target::Js
            && artifacts.len() == self.projects.len()
            && let Err(error) = ruddy::backend::js::check_exports(
                root,
                dependencies,
                project.build.platform.backend(),
            )
        {
            errors.push(error.to_string());
        }
        for (project, diagnostics) in self.projects.iter_mut().zip(extra) {
            for diagnostic in diagnostics {
                let analysis = project
                    .analysis
                    .as_mut()
                    .expect("background diagnostics belong to analyzed projects");
                if !analysis.diagnostics.contains(&diagnostic) {
                    analysis.diagnostics.push(diagnostic);
                }
            }
        }
        errors
    }

    pub fn request_file(&mut self, path: &Path) -> bool {
        let path = file_identity(path);
        for project in &mut self.projects {
            let Some(analysis) = project.analysis.as_mut() else {
                continue;
            };
            let logical = analysis
                .sources
                .keys()
                .find(|logical| file_identity(&project.source_directory.join(logical)) == path)
                .cloned();
            if let Some(logical) = logical {
                return self
                    .hosts
                    .get_mut(&project.directory)
                    .expect("current host")
                    .request_file(analysis, &logical);
            }
        }
        false
    }

    pub fn file(&self, path: &Path) -> Option<(&ProjectAnalysis, &str)> {
        let path = file_identity(path);
        self.projects.iter().find_map(|project| {
            project
                .analysis
                .as_ref()?
                .sources
                .keys()
                .find(|logical| file_identity(&project.source_directory.join(logical)) == path)
                .map(|logical| (project, logical.as_str()))
        })
    }

    pub fn definition(&mut self, path: &Path, offset: usize) -> Option<(PathBuf, Span)> {
        let target = {
            let (project, logical) = self.file(path)?;
            let analysis = project.analysis();
            if let Some(span) = analysis.definition(logical, offset) {
                let path = analysis.paths.get(&span.file_id)?;
                return Some((normalize(&project.source_directory.join(path)), span));
            }
            let symbol = analysis.referenced_symbol(logical, offset)?;
            analysis.mint().external(symbol)?.to_owned()
        };
        let cached: Vec<_> = self
            .projects
            .iter()
            .filter(|project| project.analysis.is_none())
            .map(|project| project.directory.clone())
            .collect();
        if !cached.is_empty() {
            self.source_required.extend(cached);
            self.refresh().ok()?;
        }
        self.projects.iter().find_map(|project| {
            let analysis = project.analysis.as_ref()?;
            let symbol = analysis.symbol_named(&target)?;
            let span = analysis.binding_span(symbol)?;
            let path = analysis.paths.get(&span.file_id)?;
            Some((normalize(&project.source_directory.join(path)), span))
        })
    }
}

/// Normalize paths even for buffers and module candidates not yet on disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
                if result.file_name().is_some_and(|name| name != "..") =>
            {
                result.pop();
            }
            std::path::Component::ParentDir if result.has_root() => {}
            component => result.push(component.as_os_str()),
        }
    }
    result
}

/// Canonical identity for documents, including an unsaved suffix whose nearest
/// existing ancestor may be a symlink. Watchers keep the loader's original paths.
pub fn file_identity(path: &Path) -> PathBuf {
    let path = normalize(path);
    let mut existing = path.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(mut canonical) = fs::canonicalize(existing) {
            canonical.extend(missing.into_iter().rev());
            return canonical;
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return path;
        };
        missing.push(name);
        existing = parent;
    }
}
