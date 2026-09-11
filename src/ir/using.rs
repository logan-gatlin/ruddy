//! Lexical imports share declaration identities, never declaration membership.
use super::*;

type Bindings = HashMap<(Namespace, String), Imported>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Symbol(Symbol),
    Module(Option<Module>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Imported {
    targets: Vec<Target>,
    explicit: bool,
    span: Span,
    fallback: Option<Box<Imported>>,
}

pub(super) enum PathError {
    Missing(TrackedString),
    Invalid(String),
}

impl From<PathError> for String {
    fn from(error: PathError) -> Self {
        match error {
            PathError::Missing(name) => format!("undefined module `{}`", name.tracked),
            PathError::Invalid(message) => message,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Imports {
    modules: HashMap<Option<Module>, Bindings>,
    pub locals: Vec<LocalScope>,
    completed: Vec<(Span, Bindings)>,
}

#[derive(Debug, Clone)]
pub(super) struct LocalScope {
    bindings: Bindings,
    term_mark: usize,
}

impl LocalScope {
    pub fn new(term_mark: usize) -> Self {
        Self {
            bindings: Bindings::new(),
            term_mark,
        }
    }
}

#[derive(Clone)]
struct Import {
    absolute: bool,
    path: Vec<TrackedString>,
    alias: Option<TrackedString>,
    glob: bool,
    empty: bool,
    span: Span,
}

fn flatten(tree: parse::UseTree, parent: &[TrackedString], absolute: bool, out: &mut Vec<Import>) {
    let absolute = absolute || tree.absolute;
    let mut path = parent.to_vec();
    path.extend(tree.prefix);
    match tree.kind {
        parse::UseKind::Name { alias } => out.push(Import {
            absolute,
            path,
            alias,
            glob: false,
            empty: false,
            span: tree.span,
        }),
        parse::UseKind::Glob => out.push(Import {
            absolute,
            path,
            alias: None,
            glob: true,
            empty: false,
            span: tree.span,
        }),
        parse::UseKind::Group(children) if children.is_empty() => out.push(Import {
            absolute,
            path,
            alias: None,
            glob: false,
            empty: true,
            span: tree.span,
        }),
        parse::UseKind::Group(children) => {
            for child in children {
                flatten(child, &path, absolute, out);
            }
        }
    }
}

impl Import {
    fn binding_name(&self) -> Option<String> {
        if self.glob || self.empty {
            return None;
        }
        if let Some(alias) = &self.alias {
            return Some(alias.tracked.clone());
        }
        let mut path = self.path.as_slice();
        if path.len() > 1 && path.last()?.tracked == "self" {
            path = &path[..path.len() - 1];
        }
        Some(path.last()?.tracked.clone())
    }
}

fn lookup(bindings: &Bindings, namespace: Namespace, name: &str) -> Result<Option<Target>, String> {
    imported_target(bindings.get(&(namespace, name.to_owned())), name)
}

fn imported_target(binding: Option<&Imported>, name: &str) -> Result<Option<Target>, String> {
    match binding {
        None => Ok(None),
        Some(binding) if binding.targets.len() == 1 => Ok(Some(binding.targets[0])),
        Some(_) => Err(format!("`{name}` is ambiguous between glob imports")),
    }
}

fn excluding(binding: Option<&Imported>, excluded: Option<Span>) -> Option<&Imported> {
    binding.and_then(|binding| {
        if Some(binding.span) == excluded {
            binding.fallback.as_deref()
        } else {
            Some(binding)
        }
    })
}

fn insert(
    bindings: &mut Bindings,
    key: (Namespace, String),
    target: Target,
    explicit: bool,
    span: Span,
) -> Option<Span> {
    match bindings.get_mut(&key) {
        None => {
            bindings.insert(
                key,
                Imported {
                    targets: vec![target],
                    explicit,
                    span,
                    fallback: None,
                },
            );
        }
        Some(previous) if explicit && previous.explicit => return Some(previous.span),
        Some(previous) if explicit => {
            *previous = Imported {
                targets: vec![target],
                explicit,
                span,
                fallback: Some(Box::new(previous.clone())),
            }
        }
        Some(previous) if previous.explicit => match &mut previous.fallback {
            Some(fallback) if !fallback.targets.contains(&target) => fallback.targets.push(target),
            None => {
                previous.fallback = Some(Box::new(Imported {
                    targets: vec![target],
                    explicit: false,
                    span,
                    fallback: None,
                }))
            }
            _ => {}
        },
        Some(previous) if !previous.explicit && !previous.targets.contains(&target) => {
            previous.targets.push(target)
        }
        _ => {}
    }
    None
}

impl Builder<'_> {
    fn declared_target(
        &self,
        module: Option<Module>,
        namespace: Namespace,
        name: &str,
    ) -> Option<Target> {
        if namespace == Namespace::Modules {
            self.modules
                .get(&(module, name.to_owned()))
                .map(|(module, _)| Target::Module(Some(*module)))
        } else {
            self.global_in(module, namespace, name).map(Target::Symbol)
        }
    }

    pub(super) fn lookup_import_name(
        &self,
        namespace: Namespace,
        name: &str,
        locals: bool,
    ) -> Result<Option<Target>, String> {
        self.lookup_import_name_except(namespace, name, locals, None)
    }

    fn lookup_import_name_except(
        &self,
        namespace: Namespace,
        name: &str,
        locals: bool,
        excluded: Option<Span>,
    ) -> Result<Option<Target>, String> {
        {
            let mut end = self.terms.bindings.len();
            for scope in self.using.locals.iter().rev() {
                if locals && namespace == Namespace::Terms {
                    if let Some(binding) = self.terms.bindings[scope.term_mark..end]
                        .iter()
                        .rev()
                        .find(|binding| binding.name == name)
                    {
                        return Ok(Some(Target::Symbol(binding.symbol)));
                    }
                    end = scope.term_mark;
                }
                if let Some(target) = lookup(&scope.bindings, namespace, name)? {
                    return Ok(Some(target));
                }
            }
            if locals
                && namespace == Namespace::Terms
                && let Some(binding) = self.terms.bindings[..end]
                    .iter()
                    .rev()
                    .find(|binding| binding.name == name)
            {
                return Ok(Some(Target::Symbol(binding.symbol)));
            }
        }
        let mut module = self.module;
        loop {
            if let Some(target) = self.declared_target(module, namespace, name) {
                return Ok(Some(target));
            }
            let binding = excluding(
                self.using
                    .modules
                    .get(&module)
                    .and_then(|bindings| bindings.get(&(namespace, name.to_owned()))),
                excluded,
            );
            if let Some(target) = imported_target(binding, name)? {
                return Ok(Some(target));
            }
            let Some(current) = module else {
                break;
            };
            module = self.mint.parent(current.symbol());
        }
        if namespace == Namespace::Modules
            && let Some(module) = self.dependencies.get(name)
        {
            return Ok(Some(Target::Module(Some(*module))));
        }
        Ok(self
            .std_prelude
            .and_then(|module| self.declared_target(Some(module), namespace, name)))
    }

    pub(super) fn import_modules_from(
        &self,
        path: &[TrackedString],
        absolute: bool,
    ) -> Result<Option<Module>, PathError> {
        self.import_modules_except(path, None, absolute)
    }

    fn import_modules_except(
        &self,
        path: &[TrackedString],
        excluded: Option<Span>,
        absolute: bool,
    ) -> Result<Option<Module>, PathError> {
        let mut module = None;
        let mut anchors = true;
        for (index, segment) in path.iter().enumerate() {
            module = match segment.tracked.as_str() {
                "bundle" if index == 0 && !absolute => None,
                "self" if index == 0 && !absolute => self.module,
                "super" if anchors && !absolute => {
                    let current = if index == 0 { self.module } else { module };
                    let Some(current) = current else {
                        return Err(PathError::Invalid(
                            "`super` climbs above the bundle root".into(),
                        ));
                    };
                    self.mint.parent(current.symbol())
                }
                name => {
                    anchors = false;
                    let found = if index == 0 && absolute {
                        self.dependencies
                            .get(name)
                            .copied()
                            .map(|module| Target::Module(Some(module)))
                    } else if index == 0 {
                        self.lookup_import_name_except(Namespace::Modules, name, true, excluded)
                            .map_err(PathError::Invalid)?
                    } else {
                        self.declared_target(module, Namespace::Modules, name)
                    };
                    match found {
                        Some(Target::Module(module)) => module,
                        _ => return Err(PathError::Missing(segment.clone())),
                    }
                }
            };
        }
        Ok(module)
    }

    fn import_targets(&self, import: &Import) -> Result<Vec<(Namespace, String, Target)>, String> {
        let namespaces = [
            Namespace::Terms,
            Namespace::Types,
            Namespace::Effects,
            Namespace::Modules,
        ];
        if import.glob || import.empty {
            if import.path.is_empty() {
                return if import.empty {
                    Ok(Vec::new())
                } else {
                    Err("a glob requires a module path".into())
                };
            }
            let module = self.import_modules_from(&import.path, import.absolute)?;
            if import.empty {
                return Ok(Vec::new());
            }
            if module == self.module && self.using.locals.is_empty() {
                return Err("a module cannot glob-import itself".into());
            }
            let mut targets: Vec<_> = self
                .globals
                .iter()
                .filter(|((parent, _, _), _)| *parent == module)
                .map(|((_, namespace, name), (symbol, _))| {
                    (*namespace, name.clone(), Target::Symbol(*symbol))
                })
                .collect();
            targets.extend(
                self.modules
                    .iter()
                    .filter(|((parent, _), _)| *parent == module)
                    .map(|((_, name), (module, _))| {
                        (
                            Namespace::Modules,
                            name.clone(),
                            Target::Module(Some(*module)),
                        )
                    }),
            );
            targets.sort_by(|a, b| a.1.cmp(&b.1).then((a.0 as u8).cmp(&(b.0 as u8))));
            return Ok(targets);
        }
        let Some(last) = import.path.last() else {
            return Err("an import requires a path".into());
        };
        let mut path = import.path.clone();
        let grouped_self = last.tracked == "self" && path.len() > 1;
        if grouped_self {
            path.pop();
        }
        let last = path.last().unwrap();
        let anchor =
            !import.absolute && matches!(last.tracked.as_str(), "bundle" | "self" | "super");
        let name = import
            .alias
            .as_ref()
            .map(|a| a.tracked.clone())
            .unwrap_or_else(|| last.tracked.clone());
        if anchor && import.alias.is_none() {
            return Err("importing `bundle`, `self`, or `super` requires `as name`".into());
        }
        if grouped_self || anchor {
            let excluded =
                (self.using.locals.is_empty() && path.len() == 1 && name == last.tracked)
                    .then_some(import.span);
            return Ok(vec![(
                Namespace::Modules,
                name,
                Target::Module(self.import_modules_except(&path, excluded, import.absolute)?),
            )]);
        }
        let mut targets = Vec::new();
        let parent = if path.len() > 1 {
            Some(self.import_modules_from(&path[..path.len() - 1], import.absolute)?)
        } else {
            None
        };
        for namespace in namespaces {
            let target = match parent {
                Some(module) => self.declared_target(module, namespace, &last.tracked),
                None if import.absolute => {
                    if namespace == Namespace::Modules {
                        self.dependencies
                            .get(&last.tracked)
                            .copied()
                            .map(|module| Target::Module(Some(module)))
                    } else {
                        None
                    }
                }
                None => {
                    // A same-name import copies an existing lexical name; it
                    // cannot use its own output as evidence for that name.
                    let excluded = (self.using.locals.is_empty() && name == last.tracked)
                        .then_some(import.span);
                    self.lookup_import_name_except(namespace, &last.tracked, false, excluded)?
                }
            };
            if let Some(target) = target {
                targets.push((namespace, name.clone(), target));
            }
        }
        if targets.is_empty() {
            return Err(format!("unresolved import `{}`", last.tracked));
        }
        Ok(targets)
    }

    pub(super) fn resolve_imports(&mut self, trees: Vec<(Option<Module>, parse::UseTree)>) {
        self.define(None, 0);
        let mut imports = Vec::new();
        for (module, tree) in trees {
            let mut flat = Vec::new();
            flatten(tree, &[], false, &mut flat);
            imports.extend(flat.into_iter().map(|import| (module, import)));
        }
        // Rebuild from the preceding round: aliases may refer forward, and a
        // glob discovered later must still make earlier uses ambiguous.
        let mut converged = false;
        for _ in 0..=imports.len().saturating_mul(2) {
            crate::cancellation::checkpoint();
            let mut next = HashMap::new();
            for (module, import) in &imports {
                self.module = *module;
                if let Ok(targets) = self.import_targets(import) {
                    for (namespace, name, target) in targets {
                        insert(
                            next.entry(*module).or_insert_with(Bindings::new),
                            (namespace, name),
                            target,
                            !import.glob,
                            import.span,
                        );
                    }
                }
            }
            if next == self.using.modules {
                converged = true;
                break;
            }
            self.using.modules = next;
        }
        if !converged {
            for (_, import) in &imports {
                self.error(
                    import.span,
                    ErrorKind::Using {
                        message: "import resolution did not converge; check for an import cycle"
                            .into(),
                    },
                );
            }
        }
        self.check_import_cycles(&imports);
        let mut checked = HashMap::new();
        for (module, import) in imports {
            self.module = module;
            match self.import_targets(&import) {
                Err(message) => self.error(import.span, ErrorKind::Using { message }),
                Ok(targets) => {
                    for (namespace, name, target) in targets {
                        let declared = if namespace == Namespace::Modules {
                            self.modules
                                .get(&(module, name.clone()))
                                .map(|(_, span)| *span)
                        } else {
                            self.globals
                                .get(&(module, namespace, name.clone()))
                                .map(|(_, span)| *span)
                        };
                        let duplicate = insert(
                            checked.entry(module).or_insert_with(Bindings::new),
                            (namespace, name.clone()),
                            target,
                            !import.glob,
                            import.span,
                        );
                        if !import.glob
                            && let Some(previous) = declared.or(duplicate)
                        {
                            let previous = self.anchor(previous);
                            self.error(
                                import.span,
                                ErrorKind::Duplicate {
                                    name,
                                    namespace,
                                    previous,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    /// Validate dependencies in their actual namespaces after provisional
    /// lookup has settled. A value import may depend on a module with the same
    /// spelling; only cycles of the selected bindings are invalid.
    fn check_import_cycles(&mut self, imports: &[(Option<Module>, Import)]) {
        let mut dependencies = HashMap::new();
        for (module, import) in imports {
            self.module = *module;
            let Ok(targets) = self.import_targets(import) else {
                continue;
            };
            let Some(first) = import.path.first() else {
                continue;
            };
            if import.absolute || matches!(first.tracked.as_str(), "bundle" | "self" | "super") {
                continue;
            }
            let bare = import.path.len() == 1 && !import.glob;
            let identity = !import.glob
                && import.binding_name().as_deref() == Some(first.tracked.as_str())
                && (bare || (import.path.len() == 2 && import.path[1].tracked == "self"));
            for (namespace, _, _) in targets {
                let source_namespace = if bare { namespace } else { Namespace::Modules };
                let mut at = *module;
                loop {
                    if self
                        .declared_target(at, source_namespace, &first.tracked)
                        .is_some()
                    {
                        break;
                    }
                    let binding = excluding(
                        self.using.modules.get(&at).and_then(|bindings| {
                            bindings.get(&(source_namespace, first.tracked.clone()))
                        }),
                        identity.then_some(import.span),
                    );
                    if let Some(binding) = binding {
                        dependencies
                            .insert((import.span, namespace), (binding.span, source_namespace));
                        break;
                    }
                    let Some(current) = at else {
                        break;
                    };
                    at = self.mint.parent(current.symbol());
                }
            }
        }
        let mut reported = HashSet::new();
        for start in dependencies.keys() {
            let mut seen = HashSet::new();
            let mut node = *start;
            while let Some(next) = dependencies.get(&node) {
                if !seen.insert(node) {
                    if reported.insert(node.0) {
                        self.error(
                            node.0,
                            ErrorKind::Using {
                                message: "cyclic import: no declaration grounds this alias chain"
                                    .into(),
                            },
                        );
                    }
                    break;
                }
                node = *next;
            }
        }
    }

    pub(super) fn check_local_import_conflicts(&mut self, pattern: &parse::Pattern) {
        let mut names = Vec::new();
        pattern_names(pattern, &mut names);
        for name in names {
            let previous = self
                .using
                .locals
                .last()
                .and_then(|scope| {
                    scope
                        .bindings
                        .get(&(Namespace::Terms, name.tracked.clone()))
                })
                .filter(|binding| binding.explicit)
                .map(|binding| binding.span);
            if let Some(previous) = previous {
                let previous = self.anchor(previous);
                self.error(
                    name.span,
                    ErrorKind::Duplicate {
                        name: name.tracked.clone(),
                        namespace: Namespace::Terms,
                        previous,
                    },
                );
            }
        }
    }

    pub(super) fn resolve_local_import(&mut self, tree: parse::UseTree, block: Span) {
        let start = tree.span.end();
        let mut imports = Vec::new();
        flatten(tree, &[], false, &mut imports);
        for import in imports {
            match self.import_targets(&import) {
                Err(message) => self.error(import.span, ErrorKind::Using { message }),
                Ok(targets) => {
                    for (namespace, name, target) in targets {
                        let scope = self
                            .using
                            .locals
                            .last_mut()
                            .expect("a local import belongs to a block");
                        let local_conflict = namespace == Namespace::Terms
                            && !import.glob
                            && self.terms.bindings[scope.term_mark..]
                                .iter()
                                .any(|binding| binding.name == name);
                        let previous = insert(
                            &mut scope.bindings,
                            (namespace, name.clone()),
                            target,
                            !import.glob,
                            import.span,
                        );
                        if let Some(previous) = previous {
                            let previous = self.anchor(previous);
                            self.error(
                                import.span,
                                ErrorKind::Duplicate {
                                    name,
                                    namespace,
                                    previous,
                                },
                            );
                        } else if local_conflict {
                            self.error(
                                import.span,
                                ErrorKind::Using {
                                    message: format!(
                                        "import `{name}` conflicts with a local binding"
                                    ),
                                },
                            );
                        }
                    }
                }
            }
        }
        let span = block.file_id.span(start, block.end().saturating_sub(start));
        self.using
            .completed
            .push((span, self.using.locals.last().unwrap().bindings.clone()));
    }
}

fn named_targets(bindings: &Bindings) -> impl Iterator<Item = (Namespace, &str, Option<Symbol>)> {
    bindings.iter().filter_map(|((namespace, name), binding)| {
        if binding.targets.len() != 1 {
            return None;
        }
        let symbol = match binding.targets[0] {
            Target::Symbol(symbol) => Some(symbol),
            Target::Module(module) => module.map(Module::symbol),
        };
        Some((*namespace, name.as_str(), symbol))
    })
}

impl ScopeNames {
    pub fn dependency_names(&self) -> impl Iterator<Item = (Namespace, &str, Option<Symbol>)> {
        self.dependencies
            .iter()
            .map(|(name, module)| (Namespace::Modules, name.as_str(), Some(module.symbol())))
    }

    /// Lexical candidates include imports; qualified member lookup does not.
    pub fn lexical_names(
        &self,
        module: Option<Module>,
    ) -> impl Iterator<Item = (Namespace, &str, Option<Symbol>)> {
        self.dependency_names()
            .filter(move |_| module.is_none())
            .chain(
                self.imports
                    .modules
                    .get(&module)
                    .into_iter()
                    .flat_map(named_targets),
            )
            .chain(
                self.globals(module)
                    .map(|(namespace, name, symbol)| (namespace, name, Some(symbol))),
            )
            .chain(
                self.modules(module)
                    .map(|(name, module)| (Namespace::Modules, name, Some(module.symbol()))),
            )
    }

    fn local_import_scopes(&self, file: crate::tracking::FileID, offset: usize) -> Vec<&Bindings> {
        let mut scopes: Vec<_> = self
            .imports
            .completed
            .iter()
            .filter(|(span, _)| {
                span.file_id == file && span.start <= offset && offset <= span.end()
            })
            .collect();
        scopes.sort_by_key(|(span, _)| (std::cmp::Reverse(span.width), span.start));
        scopes.into_iter().map(|(_, bindings)| bindings).collect()
    }

    pub fn local_import_names(
        &self,
        file: crate::tracking::FileID,
        offset: usize,
    ) -> Vec<(Namespace, &str, Option<Symbol>)> {
        self.local_import_scopes(file, offset)
            .into_iter()
            .flat_map(named_targets)
            .collect()
    }

    /// Resolve a completion qualifier with the same lexical/strict distinction
    /// as source paths. `Some(None)` is the bundle root; `None` is unresolved.
    pub fn resolve_module_path(
        &self,
        mint: &Mint,
        module: Option<Module>,
        file: crate::tracking::FileID,
        offset: usize,
        path: &[&str],
    ) -> Option<Option<Module>> {
        if path.first() == Some(&"") {
            let mut at = *self.dependencies.get(*path.get(1)?)?;
            for name in &path[2..] {
                at = self.module(Some(at), name)?;
            }
            return Some(Some(at));
        }
        let mut at = None;
        let mut anchors = true;
        for (index, name) in path.iter().copied().enumerate() {
            at = match name {
                "bundle" if index == 0 => None,
                "self" if index == 0 => module,
                "super" if anchors => mint.parent(if index == 0 { module? } else { at? }.symbol()),
                _ => {
                    anchors = false;
                    if index != 0 {
                        Some(self.module(at, name)?)
                    } else {
                        let mut found = None;
                        for bindings in self.local_import_scopes(file, offset).into_iter().rev() {
                            if let Some(Target::Module(target)) =
                                lookup(bindings, Namespace::Modules, name).ok()?
                            {
                                found = Some(target);
                                break;
                            }
                        }
                        let mut scope = module;
                        while found.is_none() {
                            if let Some(target) = self.module(scope, name) {
                                found = Some(Some(target));
                                break;
                            }
                            if let Some(bindings) = self.imports.modules.get(&scope)
                                && let Some(Target::Module(target)) =
                                    lookup(bindings, Namespace::Modules, name).ok()?
                            {
                                found = Some(target);
                                break;
                            }
                            let Some(current) = scope else {
                                break;
                            };
                            scope = mint.parent(current.symbol());
                        }
                        found
                            .or_else(|| self.dependencies.get(name).copied().map(Some))
                            .or_else(|| {
                                self.prelude
                                    .and_then(|prelude| self.module(Some(prelude), name))
                                    .map(Some)
                            })?
                    }
                }
            };
        }
        Some(at)
    }
}
