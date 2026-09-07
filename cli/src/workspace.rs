//! Filesystem acquisition for one editor build context. The compiler sees only
//! explicit inputs; overlays apply throughout reachable local dependencies.
use super::*;
use ruddy::{
    analysis::{Analysis, Host},
    artifact::Header,
    ir,
};

pub struct ProjectAnalysis {
    pub directory: PathBuf,
    pub source_directory: PathBuf,
    pub analysis: Analysis,
    pub interface: Header,
    build: Build,
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
    pub fn refresh(&mut self) -> Result<(), CompileError> {
        self.projects.clear();
        self.observed.borrow_mut().clear();
        let lock_source = self.read(&self.root.join("Ruddy.lock"));
        if self.lock_source != lock_source {
            self.git_selections.clear();
            self.lock_source = lock_source;
        }
        for selection in &mut self.git_selections {
            selection.used = false;
        }
        let build = self.manifest(&self.root)?.build();
        let mut resolver = None;
        let mut active = Vec::new();
        let mut projects = Vec::new();
        self.visit(
            self.root.clone(),
            build,
            &mut resolver,
            &mut active,
            &mut projects,
        )?;
        let reachable: std::collections::HashSet<_> = projects
            .iter()
            .map(|project: &ProjectAnalysis| project.directory.clone())
            .collect();
        self.hosts
            .retain(|directory, _| reachable.contains(directory));
        self.git_selections.retain(|selection| selection.used);
        self.projects = projects;
        Ok(())
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
        let manifest = self.manifest(&directory)?;
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
        for (alias, spec, _installed_default) in specifications {
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
            let at = self.visit(child, build, resolver, active, projects)?;
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
        }
        impl Files for Sources<'_> {
            fn read(&self, path: &str) -> Option<String> {
                let path = normalize(&self.directory.join(path));
                let disk = fs::read_to_string(&path).ok();
                self.observed
                    .borrow_mut()
                    .insert(path.clone(), disk.clone());
                self.overlays.get(&file_identity(&path)).cloned().or(disk)
            }
        }
        let sources = Sources {
            directory: &source_directory,
            overlays: &self.overlays,
            observed: &self.observed,
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
        let at = projects.len();
        projects.push(ProjectAnalysis {
            directory,
            source_directory,
            analysis,
            interface,
            build,
        });
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
        let mut artifacts = Vec::new();
        let mut errors = Vec::new();
        let mut extra = Vec::new();
        for project in &self.projects {
            ruddy::cancellation::checkpoint();
            let linked: Vec<_> = artifacts.iter().collect();
            let (artifact, diagnostics) = project.analysis.lower(&linked);
            extra.push(diagnostics);
            let Some(artifact) = artifact else {
                continue;
            };
            let artifact = match project.interface.kind {
                Kind::Library => artifact,
                Kind::Executable => match ruddy::entry::executable(&artifact, &linked) {
                    Ok(artifact) => artifact,
                    Err(error) => {
                        errors.push(error.to_string());
                        continue;
                    }
                },
            };
            if project.build.target == Target::Js
                && let Err(error) = ruddy::backend::js::check_entry(&artifact, &linked)
            {
                errors.push(error.to_string());
            }
            artifacts.push(artifact);
        }
        if let (Some(project), Some((root, dependencies))) =
            (self.projects.last(), artifacts.split_last())
            && project.build.target == Target::Js
            && artifacts.len() == self.projects.len()
        {
            let dependencies: Vec<_> = dependencies.iter().collect();
            if let Err(error) = ruddy::backend::js::check_exports(
                root,
                &dependencies,
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
        let symbol = project.analysis.referenced_symbol(logical, offset)?;
        if let Some(span) = project.analysis.binding_span(symbol) {
            let path = project.analysis.paths.get(&span.file_id)?;
            return Some((normalize(&project.source_directory.join(path)), span));
        }
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
