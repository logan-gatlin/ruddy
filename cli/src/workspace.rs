//! Filesystem acquisition for one editor build context. The compiler sees only
//! explicit inputs; overlays apply throughout reachable local dependencies.
use super::*;
use ruddy::{
    analysis::{Analysis, Host},
    artifact::{Artifact, Header},
    ir,
};

pub struct ProjectAnalysis {
    pub directory: PathBuf,
    pub source_directory: PathBuf,
    pub analysis: Analysis,
    pub interface: Header,
    build: Build,
    manifest_source: String,
    focus: Option<String>,
    dependencies: Vec<(String, PathBuf, u64)>,
    inputs: HashMap<PathBuf, Option<String>>,
    revision: u64,
    artifact: Option<Artifact>,
}

/// Work performed by a workspace refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub rebuilt: usize,
    pub reused: usize,
    /// Projects whose current analysis has not yet been lowered.
    pub background: usize,
}

struct GitSelection {
    specification: DependencySpec,
    checkout: PathBuf,
    used: bool,
}

pub struct Workspace {
    root: PathBuf,
    git_selections: Vec<GitSelection>,
    lock_source: Option<String>,
    focus: Option<PathBuf>,
    overlays: HashMap<PathBuf, String>,
    hosts: HashMap<PathBuf, Host>,
    pub projects: Vec<ProjectAnalysis>,
    next_revision: u64,
    observed: std::cell::RefCell<HashMap<PathBuf, Option<String>>>,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: fs::canonicalize(&root).unwrap_or_else(|_| normalize(&root)),
            git_selections: Vec::new(),
            lock_source: None,
            focus: None,
            overlays: HashMap::new(),
            hosts: HashMap::new(),
            projects: Vec::new(),
            next_revision: 0,
            observed: Default::default(),
        }
    }

    pub fn focus(&mut self, path: &Path) {
        self.focus = Some(file_identity(path));
    }

    /// Closing a buffer removes the overlay; the next refresh reads disk.
    pub fn set_overlay(&mut self, path: &Path, text: Option<String>) {
        let path = file_identity(path);
        match text {
            Some(text) => {
                self.overlays.insert(path, text);
            }
            None => {
                self.overlays.remove(&path);
            }
        }
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
        self.visit(
            self.root.clone(),
            build,
            &mut resolver,
            &mut active,
            &mut projects,
            &mut previous,
            &mut report,
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
        directory: PathBuf,
        build: Build,
        resolver: &mut Option<git::Resolver>,
        active: &mut Vec<PathBuf>,
        projects: &mut Vec<ProjectAnalysis>,
        previous: &mut HashMap<PathBuf, ProjectAnalysis>,
        report: &mut RefreshReport,
    ) -> Result<usize, CompileError> {
        ruddy::cancellation::checkpoint();
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
        for (alias, spec, _default_std) in specifications {
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
            let at = self.visit(child, build, resolver, active, projects, previous, report)?;
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
        let focus = (directory == self.root)
            .then(|| {
                self.focus.as_ref().map(|path| {
                    path.strip_prefix(&source_directory)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .flatten();
        let dependency_revisions: Vec<_> = dependencies
            .iter()
            .map(|(alias, at)| {
                (
                    alias.clone(),
                    projects[*at].directory.clone(),
                    projects[*at].revision,
                )
            })
            .collect();
        if let Some(project) = previous.remove(&directory)
            && project.build == build
            && project.manifest_source == manifest_source
            && project.source_directory == source_directory
            && project.focus == focus
            && project.dependencies == dependency_revisions
            && project
                .inputs
                .iter()
                .all(|(path, source)| self.read(path) == *source)
        {
            active.pop();
            let at = projects.len();
            projects.push(project);
            report.reused += 1;
            return Ok(at);
        }
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
        active.pop();
        self.next_revision += 1;
        let at = projects.len();
        projects.push(ProjectAnalysis {
            directory,
            source_directory,
            analysis,
            interface,
            build,
            manifest_source,
            focus,
            dependencies: dependency_revisions,
            inputs: inputs.into_inner(),
            revision: self.next_revision,
            artifact: None,
        });
        report.rebuilt += 1;
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
            self.hosts
                .get_mut(&project.directory)
                .expect("a current project host")
                .complete(&mut project.analysis);
            let mut interface = project.analysis.interface();
            interface.kind = project.interface.kind;
            interface.dependencies = project.interface.dependencies.clone();
            project.interface = interface;
        }
        let mut errors = Vec::new();
        let mut extra = vec![Vec::new(); self.projects.len()];
        for at in 0..self.projects.len() {
            ruddy::cancellation::checkpoint();
            let (dependencies, current) = self.projects.split_at_mut(at);
            let project = &mut current[0];
            let linked: Vec<_> = dependencies
                .iter()
                .filter_map(|project| project.artifact.as_ref())
                .collect();
            if project.artifact.is_none() {
                let (artifact, diagnostics) = project.analysis.lower(&linked);
                extra[at] = diagnostics;
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
        {
            if let Err(error) = ruddy::backend::js::check_exports(
                root,
                dependencies,
                project.build.platform.backend(),
            ) {
                errors.push(error.to_string());
            }
        }
        for (project, diagnostics) in self.projects.iter_mut().zip(extra) {
            for diagnostic in diagnostics {
                if !project.analysis.diagnostics.contains(&diagnostic) {
                    project.analysis.diagnostics.push(diagnostic);
                }
            }
        }
        errors
    }

    pub fn request_file(&mut self, path: &Path) -> bool {
        let path = file_identity(path);
        for project in &mut self.projects {
            let logical = project
                .analysis
                .sources
                .keys()
                .find(|logical| file_identity(&project.source_directory.join(logical)) == path)
                .cloned();
            if let Some(logical) = logical {
                return self
                    .hosts
                    .get_mut(&project.directory)
                    .expect("current host")
                    .request_file(&mut project.analysis, &logical);
            }
        }
        false
    }

    pub fn file(&self, path: &Path) -> Option<(&ProjectAnalysis, &str)> {
        let path = file_identity(path);
        self.projects.iter().find_map(|project| {
            project
                .analysis
                .sources
                .keys()
                .find(|logical| file_identity(&project.source_directory.join(logical)) == path)
                .map(|logical| (project, logical.as_str()))
        })
    }

    pub fn definition(&self, path: &Path, offset: usize) -> Option<(PathBuf, Span)> {
        let (project, logical) = self.file(path)?;
        if let Some(span) = project.analysis.definition(logical, offset) {
            let path = project.analysis.paths.get(&span.file_id)?;
            return Some((normalize(&project.source_directory.join(path)), span));
        }
        let symbol = project.analysis.referenced_symbol(logical, offset)?;
        let qualified = project.analysis.mint().external(symbol)?;
        self.projects.iter().find_map(|project| {
            let symbol = project.analysis.symbol_named(qualified)?;
            let span = project.analysis.binding_span(symbol)?;
            let path = project.analysis.paths.get(&span.file_id)?;
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
