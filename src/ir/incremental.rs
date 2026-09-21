//! Incremental body lowering over an unchanged declaration environment.
//!
//! Declaration changes take the complete builder path. Ordinary body edits
//! retain its resolved declarations and only lower changed definitions. Query
//! consumers still decide whether the resulting semantic interfaces changed.
use super::*;
use crate::tracking::FileID;

#[derive(Clone)]
pub(super) struct Resolver {
    builtin_mut: Option<(Symbol, Symbol)>,
    mutation_uses: usize,
    parameter_kinds: HashMap<Symbol, Vec<ParamKind>>,
    module: Option<Module>,
    terms: Names,
    using: using::Imports,
    globals: HashMap<(Option<Module>, Namespace, String), (Symbol, Span)>,
    current: Symbol,
    current_base: usize,
    bases: HashMap<Symbol, usize>,
    modules: HashMap<(Option<Module>, String), (Module, Span)>,
    dependencies: HashMap<String, Module>,
    module_scopes: Vec<(Span, Module)>,
    std_prelude: Option<Module>,
    expanded: HashMap<Symbol, IndexMap<String, Symbol>>,
    operations: HashMap<Symbol, IndexSet<OperationSelector>>,
    answering: Answering,
    arities: HashMap<Symbol, usize>,
    pending_aliases: HashMap<Symbol, PendingAlias>,
    alias_bodies: HashMap<Symbol, AliasBody>,
    lifted_aliases: HashSet<Symbol>,
    row_templates: IndexMap<Symbol, Decl<Type>>,
    imported_row_sources: HashMap<Symbol, (Arc<artifact::Type>, usize)>,
    imported_symbols: HashMap<(Namespace, String), Symbol>,
    imported_effect_rows: ImportedEffectRows,
    fixed_spreads: Option<fixed_spreads::Expander>,
    expanding: Vec<Symbol>,
    cyclic: HashSet<Symbol>,
    imported_aliases: HashSet<Symbol>,
    imported_effect_params: HashMap<Symbol, Vec<ParamKind>>,
    params: HashMap<String, (Symbol, u32)>,
    vars: IndexMap<String, Declared>,
    rigids: u32,
    scoped: Vec<(String, u32)>,
    variances: HashMap<Slot, u8>,
}

impl Resolver {
    pub(super) fn capture(builder: &Builder<'_>) -> Self {
        Self {
            builtin_mut: builder.builtin_mut,
            mutation_uses: builder.mutation_uses,
            parameter_kinds: builder.parameter_kinds.clone(),
            module: builder.module,
            terms: builder.terms.clone(),
            using: builder.using.clone(),
            globals: builder.globals.clone(),
            current: builder.current,
            current_base: builder.current_base,
            bases: builder.bases.clone(),
            modules: builder.modules.clone(),
            dependencies: builder.dependencies.clone(),
            module_scopes: builder.module_scopes.clone(),
            std_prelude: builder.std_prelude,
            expanded: builder.expanded.clone(),
            operations: builder.operations.clone(),
            answering: builder.answering,
            arities: builder.arities.clone(),
            pending_aliases: builder.pending_aliases.clone(),
            alias_bodies: builder.alias_bodies.clone(),
            lifted_aliases: builder.lifted_aliases.clone(),
            row_templates: builder.row_templates.clone(),
            imported_row_sources: builder.imported_row_sources.clone(),
            imported_symbols: builder.imported_symbols.clone(),
            imported_effect_rows: builder.imported_effect_rows.clone(),
            fixed_spreads: builder.fixed_spreads.clone(),
            expanding: builder.expanding.clone(),
            cyclic: builder.cyclic.clone(),
            imported_aliases: builder.imported_aliases.clone(),
            imported_effect_params: builder.imported_effect_params.clone(),
            params: builder.params.clone(),
            vars: builder.vars.clone(),
            rigids: builder.rigids,
            scoped: builder.scoped.clone(),
            variances: builder.variances.clone(),
        }
    }

    fn builder<'a>(&self, mint: &'a mut Mint) -> Builder<'a> {
        Builder {
            mint,
            source: SourceMap::default(),
            errors: Vec::new(),
            builtin_mut: self.builtin_mut,
            mutation_uses: self.mutation_uses,
            parameter_kinds: self.parameter_kinds.clone(),
            module: self.module,
            terms: self.terms.clone(),
            using: self.using.clone(),
            globals: self.globals.clone(),
            current: self.current,
            current_base: self.current_base,
            bases: self.bases.clone(),
            modules: self.modules.clone(),
            dependencies: self.dependencies.clone(),
            module_scopes: self.module_scopes.clone(),
            std_prelude: self.std_prelude,
            expanded: self.expanded.clone(),
            operations: self.operations.clone(),
            answering: self.answering,
            arities: self.arities.clone(),
            pending_aliases: self.pending_aliases.clone(),
            alias_bodies: self.alias_bodies.clone(),
            lifted_aliases: self.lifted_aliases.clone(),
            row_templates: self.row_templates.clone(),
            imported_row_sources: self.imported_row_sources.clone(),
            imported_symbols: self.imported_symbols.clone(),
            imported_effect_rows: self.imported_effect_rows.clone(),
            fixed_spreads: self.fixed_spreads.clone(),
            expanding: self.expanding.clone(),
            cyclic: self.cyclic.clone(),
            imported_aliases: self.imported_aliases.clone(),
            imported_effect_params: self.imported_effect_params.clone(),
            params: self.params.clone(),
            vars: self.vars.clone(),
            rigids: self.rigids,
            scoped: self.scoped.clone(),
            variances: self.variances.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct Capture {
    pub resolver: Option<Resolver>,
    pub rigids: HashMap<Symbol, (u32, u32)>,
    pub mutation_uses: HashMap<Symbol, usize>,
    pub rows: Option<RowChecks>,
}

/// Executed lowering work, independent of later inference-cache hits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoweringStats {
    pub declaration_builds: usize,
    pub lowered_bodies: usize,
    pub reused_bodies: usize,
    pub group_builds: usize,
}

/// An editor publication shares the immutable program with its lowering cache.
/// Source locations and visible names belong to this particular text revision.
#[derive(Clone, Debug)]
pub struct SharedOutput {
    pub program: Arc<Program>,
    pub source: SourceMap,
    pub names: ScopeNames,
    pub errors: Vec<Error>,
}

impl From<Output> for SharedOutput {
    fn from(output: Output) -> Self {
        Self {
            program: Arc::new(output.program),
            source: output.source,
            names: output.names,
            errors: output.errors,
        }
    }
}

/// Retains declarations and resolved bodies across editor revisions. A changed
/// declaration environment always uses the ordinary complete builder.
#[derive(Clone, Default)]
pub struct Session {
    cached: Option<Arc<Cached>>,
    stats: LoweringStats,
    changed: Option<HashSet<Symbol>>,
}

struct Cached {
    mint: Mint,
    output: SharedOutput,
    resolver: Resolver,
    rigids: HashMap<Symbol, (u32, u32)>,
    mutation_uses: HashMap<Symbol, usize>,
    rows: RowChecks,
    shape: ShapeInput,
    dependencies: Vec<(String, artifact::Header)>,
    linked: Vec<artifact::Header>,
    references: HashMap<Symbol, Vec<Symbol>>,
    aliases: HashMap<Symbol, Vec<(Symbol, Option<Symbol>)>>,
}

#[derive(Clone, PartialEq, Eq)]
struct Header {
    file: FileID,
    text: Arc<str>,
}

#[derive(Clone)]
struct BodyInput {
    module: Vec<String>,
    name: TrackedString,
    body: Span,
    text: Arc<str>,
}

#[derive(Clone, Default)]
struct ShapeInput {
    headers: Vec<Header>,
    bodies: Vec<BodyInput>,
    points: Vec<(FileID, usize)>,
}

impl Session {
    pub fn stats(&self) -> LoweringStats {
        self.stats
    }

    /// `None` means the declaration environment was rebuilt. Otherwise only
    /// these definitions have new lowered bodies in the current output.
    pub fn changed_symbols(&self) -> Option<&HashSet<Symbol>> {
        self.changed.as_ref()
    }

    pub fn build_with_interfaces(
        &mut self,
        mint: &mut Mint,
        stmts: Vec<Stmt>,
        dependencies: &[InterfaceImport<'_>],
        linked: &[&artifact::Header],
        sources: &HashMap<FileID, String>,
    ) -> SharedOutput {
        crate::cancellation::checkpoint();
        let shape = ShapeInput::read(
            &stmts,
            sources,
            self.cached.as_ref().map(|cached| &cached.shape),
        );
        if let (Some(previous), Some(shape)) = (&self.cached, &shape)
            && previous.mint.bundle() == mint.bundle()
            && previous.shape.headers == shape.headers
            && previous.dependencies.len() == dependencies.len()
            && previous
                .dependencies
                .iter()
                .zip(dependencies)
                .all(|((alias, header), current)| {
                    alias == current.alias && header == current.header
                })
            && previous.linked.len() == linked.len()
            && previous.linked.iter().zip(linked).all(|(a, b)| a == *b)
            && let Some((next, changed, grouped)) = previous.update(shape, &stmts)
        {
            self.stats.lowered_bodies += changed.len();
            self.stats.reused_bodies += next.output.program.terms.len() - changed.len();
            self.stats.group_builds += usize::from(grouped);
            *mint = next.mint.clone();
            let output = next.output.clone();
            self.cached = Some(Arc::new(next));
            self.changed = Some(changed);
            return output;
        }

        // Publish the cache only after all stages complete. Cancellation leaves
        // the previous successfully built revision available to the next edit.
        let mut capture = Capture::default();
        let output = SharedOutput::from(build_captured(
            mint,
            stmts,
            dependencies,
            linked,
            Some(&mut capture),
        ));
        self.stats.declaration_builds += 1;
        self.stats.lowered_bodies += output.program.terms.len();
        self.stats.group_builds += 1;
        self.changed = None;
        self.cached = shape.filter(|_| output.errors.is_empty()).map(|shape| {
            Arc::new(Cached {
                mint: mint.clone(),
                resolver: capture
                    .resolver
                    .expect("the completed builder has a resolver"),
                rigids: capture.rigids,
                mutation_uses: capture.mutation_uses,
                rows: capture
                    .rows
                    .expect("the completed builder checked row arguments"),
                shape,
                dependencies: dependencies
                    .iter()
                    .map(|import| (import.alias.to_owned(), import.header.clone()))
                    .collect(),
                linked: linked.iter().map(|header| (*header).clone()).collect(),
                references: output
                    .program
                    .terms
                    .iter()
                    .map(|(symbol, decl)| {
                        (*symbol, reference_set(&decl.value, &output.program.terms))
                    })
                    .collect(),
                aliases: output
                    .program
                    .terms
                    .iter()
                    .map(|(symbol, decl)| (*symbol, alias_projection(*symbol, &decl.value)))
                    .collect(),
                output: output.clone(),
            })
        });
        output
    }
}

impl ShapeInput {
    fn read(
        stmts: &[Stmt],
        sources: &HashMap<FileID, String>,
        previous: Option<&Self>,
    ) -> Option<Self> {
        fn text(sources: &HashMap<FileID, String>, span: Span) -> Option<&str> {
            sources.get(&span.file_id)?.get(span.start..span.end())
        }
        fn walk(
            stmts: &[Stmt],
            sources: &HashMap<FileID, String>,
            previous: Option<&ShapeInput>,
            module: &mut Vec<String>,
            out: &mut ShapeInput,
        ) -> Option<()> {
            for stmt in stmts {
                crate::cancellation::checkpoint();
                out.points.push((stmt.span.file_id, stmt.span.start));
                out.points.push((stmt.span.file_id, stmt.span.end()));
                let mut header = String::new();
                for attribute in &stmt.attributes {
                    let value = text(sources, attribute.span)?;
                    header.push_str(&format!("{}:{value}", value.len()));
                    out.points
                        .push((attribute.span.file_id, attribute.span.start));
                    out.points
                        .push((attribute.span.file_id, attribute.span.end()));
                }
                match &stmt.kind {
                    StmtKind::Let { pattern, body, .. }
                        if matches!(pattern.tracked, parse::PatternKind::Ident { .. }) =>
                    {
                        let parse::PatternKind::Ident { name } = &pattern.tracked else {
                            unreachable!("guard selects a named definition")
                        };
                        let source = sources.get(&stmt.span.file_id)?;
                        header.push_str(source.get(stmt.span.start..body.span.start)?);
                        header.push_str(source.get(body.span.end()..stmt.span.end())?);
                        let body_text = text(sources, body.span)?;
                        let body_text = previous
                            .and_then(|previous| previous.bodies.get(out.bodies.len()))
                            .filter(|previous| previous.text.as_ref() == body_text)
                            .map_or_else(|| Arc::from(body_text), |previous| previous.text.clone());
                        out.bodies.push(BodyInput {
                            module: module.clone(),
                            name: name.clone(),
                            body: body.span,
                            text: body_text,
                        });
                        out.points.push((body.span.file_id, body.span.start));
                        out.points.push((body.span.file_id, body.span.end()));
                    }
                    StmtKind::Module { name, body } => {
                        out.points.push((name.span.file_id, name.span.start));
                        out.points.push((name.span.file_id, name.span.end()));
                        header.push_str("module ");
                        header.push_str(&name.tracked);
                        out.headers.push(Header {
                            file: stmt.span.file_id,
                            text: header.into(),
                        });
                        module.push(name.tracked.clone());
                        if let Some(body) = body {
                            walk(body, sources, previous, module, out)?;
                        }
                        module.pop();
                        header = "end module".to_owned();
                    }
                    _ => header.push_str(text(sources, stmt.span)?),
                }
                out.headers.push(Header {
                    file: stmt.span.file_id,
                    text: header.into(),
                });
            }
            Some(())
        }
        let mut shape = Self::default();
        walk(stmts, sources, previous, &mut Vec::new(), &mut shape)?;
        Some(shape)
    }
}

fn reference_set(term: &Term, declarations: &IndexMap<Symbol, Decl<Term>>) -> Vec<Symbol> {
    let mut names = Vec::new();
    references(term, &mut names);
    names.retain(|symbol| declarations.contains_key(symbol));
    names.sort_unstable();
    names.dedup();
    names
}

// Circular initializer rejection depends on direct forwarding, not all
// references: `fn x => b x` and `b` refer to the same definition but only the
// latter forwards its value. Retain that separate projection for each body.
fn alias_projection(symbol: Symbol, term: &Term) -> Vec<(Symbol, Option<Symbol>)> {
    fn target(mut term: &Term) -> Option<Symbol> {
        while let TermKind::Let { body, .. } = &term.kind {
            term = body;
        }
        match term.kind {
            TermKind::Ident(symbol) => Some(symbol),
            _ => None,
        }
    }
    let mut aliases = vec![(symbol, target(term))];
    for term in term.walk() {
        if let TermKind::Let { name, value, .. } = &term.kind {
            aliases.push((name.anchored, target(value)));
        }
    }
    aliases
}

fn has_alias_cycle(
    program: &Program,
    aliases: &HashMap<Symbol, Vec<(Symbol, Option<Symbol>)>>,
) -> bool {
    let edges: HashMap<_, _> = program
        .terms
        .keys()
        .flat_map(|symbol| aliases[symbol].iter().copied())
        .collect();
    let mut done = HashSet::new();
    for root in edges.keys() {
        let mut current = *root;
        let mut open = HashSet::new();
        while !done.contains(&current) {
            crate::cancellation::checkpoint();
            if !open.insert(current) {
                return true;
            }
            let Some(Some(next)) = edges.get(&current) else {
                break;
            };
            current = *next;
        }
        done.extend(open);
    }
    false
}

fn named_body<'a>(
    stmts: &'a [Stmt],
    at: &mut usize,
    wanted: usize,
) -> Option<(&'a Option<parse::Annotation>, &'a Body)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { pattern, ty, body }
                if matches!(pattern.tracked, parse::PatternKind::Ident { .. }) =>
            {
                if *at == wanted {
                    return Some((ty, body));
                }
                *at += 1;
            }
            StmtKind::Module {
                body: Some(body), ..
            } => {
                if let Some(found) = named_body(body, at, wanted) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Positional information does not participate in declaration identity. The
/// corresponding unchanged syntax boundaries relocate existing source anchors.
struct Relocations {
    points: HashMap<FileID, Vec<(usize, usize)>>,
}

impl Relocations {
    fn new(before: &ShapeInput, after: &ShapeInput) -> Option<Self> {
        if before.points.len() != after.points.len() {
            return None;
        }
        let mut points: HashMap<FileID, Vec<(usize, usize)>> = HashMap::new();
        for ((file, old), (new_file, new)) in before.points.iter().zip(&after.points) {
            if file != new_file {
                return None;
            }
            points.entry(*file).or_default().push((*old, *new));
        }
        for file in points.values_mut() {
            file.sort_unstable();
            file.dedup();
            if file
                .windows(2)
                .any(|pair| pair[0].0 == pair[1].0 || pair[0].1 > pair[1].1)
            {
                return None;
            }
        }
        Some(Self { points })
    }

    fn position(&self, file: FileID, old: usize) -> usize {
        let Some(points) = self.points.get(&file) else {
            return old;
        };
        let index = points
            .partition_point(|(at, _)| *at <= old)
            .saturating_sub(1);
        let (before, after) = points[index];
        old.saturating_add(after).saturating_sub(before)
    }

    fn span(&self, span: Span) -> Span {
        Span {
            start: self.position(span.file_id, span.start),
            ..span
        }
    }

    fn scope(&self, span: Span) -> Span {
        if span.width == usize::MAX {
            return span;
        }
        let start = self.position(span.file_id, span.start);
        let end = self.position(span.file_id, span.end());
        Span {
            start,
            width: end.saturating_sub(start),
            ..span
        }
    }
}

impl Cached {
    fn update(&self, shape: &ShapeInput, stmts: &[Stmt]) -> Option<(Self, HashSet<Symbol>, bool)> {
        if self.shape.bodies.len() != shape.bodies.len()
            || !self.resolver.pending_aliases.is_empty()
        {
            return None;
        }
        let relocation = Relocations::new(&self.shape, shape)?;
        let mut changed = HashSet::new();
        let mut replacements = Vec::new();
        let mut replaced_spans = Vec::new();
        for (index, (previous, current)) in self.shape.bodies.iter().zip(&shape.bodies).enumerate()
        {
            if previous.text == current.text {
                continue;
            }
            let mut module = None;
            for name in &current.module {
                module = Some(self.output.names.module(module, name)?);
            }
            let symbol = *self.output.names.globals.get(&(
                module,
                Namespace::Terms,
                current.name.tracked.clone(),
            ))?;
            if !self.rigids.contains_key(&symbol) || !changed.insert(symbol) {
                return None;
            }
            replacements.push((module, symbol, current, index));
            replaced_spans.push(previous.body);
        }
        let mut mint = self.mint.clone();
        let mut output = self.output.clone();
        output
            .source
            .remap(|at, span| (!changed.contains(&at.definition)).then(|| relocation.span(span)));
        let mut builder = self.resolver.builder(&mut mint);
        for (_, span) in builder.globals.values_mut() {
            *span = relocation.span(*span);
        }
        for (_, span) in builder.modules.values_mut() {
            *span = relocation.span(*span);
        }
        for (span, _) in &mut builder.module_scopes {
            *span = relocation.scope(*span);
        }
        builder.using.remap(|span| {
            if replaced_spans.iter().any(|body| {
                body.file_id == span.file_id && body.start <= span.start && span.end() <= body.end()
            }) {
                None
            } else {
                Some(relocation.scope(span))
            }
        });
        // Metadata and declarations are unchanged. Their anchors stay valid;
        // only their source-map positions move with the text around them.
        let mut references = self.references.clone();
        let mut mutation_uses = self.mutation_uses.clone();
        let mut total_mutation_uses = self.resolver.mutation_uses;
        let mut aliases = self.aliases.clone();
        let mut check_aliases = false;
        let mut regroup = false;
        let mut operation_sets: HashMap<EffectId, IndexSet<OperationSelector>> = HashMap::new();
        for (symbol, identity) in &output.program.effect_ids {
            let selectors = operation_sets.entry(identity.clone()).or_default();
            if let Some(known) = builder.operations.get(symbol) {
                selectors.extend(known.iter().cloned());
            }
        }
        for (module, symbol, current, index) in replacements {
            let (annotation, body) = named_body(stmts, &mut 0, index)?;
            crate::cancellation::checkpoint();
            builder.mint.reset_definition(symbol);
            builder.module = module;
            builder.define(Some(symbol), current.name.span.start);
            builder.rigids = self.rigids[&symbol].0;
            builder.terms.bindings.clear();
            builder.params.clear();
            builder.vars.clear();
            builder.scoped.clear();
            builder.answering = Answering::Nowhere;
            let mutation_before = builder.mutation_uses;
            let annotation = annotation
                .clone()
                .map(|ty| builder.written(ty, Place::Annotation));
            let mut value = builder.term(body.tracked.clone());
            let mutations = builder.mutation_uses - mutation_before;
            total_mutation_uses = total_mutation_uses - mutation_uses[&symbol] + mutations;
            mutation_uses.insert(symbol, mutations);
            if builder.rigids != self.rigids[&symbol].1
                || builder.builtin_mut != self.resolver.builtin_mut
            {
                // Global rigid ranges and first-use builtins are part of the
                // declaration snapshot; rebuild if this edit changes either.
                return None;
            }
            let mut annotation = annotation;
            if let Some(annotation) = &mut annotation {
                rekey_type(
                    &mut annotation.ty,
                    &output.program.effect_ids,
                    &mut builder.errors,
                );
            }
            rekey_term(
                &mut value,
                &output.program.effect_ids,
                &operation_sets,
                &mut builder.errors,
            );
            let names = reference_set(&value, &output.program.terms);
            regroup |= references.get(&symbol) != Some(&names);
            references.insert(symbol, names);
            let forwarding = alias_projection(symbol, &value);
            check_aliases |= aliases.get(&symbol) != Some(&forwarding);
            aliases.insert(symbol, forwarding);
            let declaration = Arc::make_mut(&mut output.program).terms.get_mut(&symbol)?;
            declaration.annotation = annotation;
            declaration.name_at = builder.anchor(current.name.span);
            declaration.value = value;
        }
        if !builder.errors.is_empty()
            || (check_aliases && has_alias_cycle(&output.program, &aliases))
            || (self.resolver.mutation_uses != 0 && total_mutation_uses == 0)
        {
            return None;
        }
        builder.mutation_uses = total_mutation_uses;
        if !changed.is_empty() {
            builder.errors.extend(row_arguments_selected(
                Arc::make_mut(&mut output.program),
                &builder.parameter_kinds,
                &self.rows,
                Some(&changed),
            ));
        }
        if !builder.errors.is_empty() {
            return None;
        }
        if regroup {
            let program = Arc::make_mut(&mut output.program);
            program.groups = grouping(&program.terms);
        }
        output.source.extend(std::mem::take(&mut builder.source));
        output.names.imports = builder.using.clone();
        output.names.scopes = builder.module_scopes.clone();
        let resolver = Resolver::capture(&builder);
        drop(builder);
        Some((
            Self {
                mint,
                output,
                resolver,
                rigids: self.rigids.clone(),
                mutation_uses,
                rows: self.rows.clone(),
                shape: shape.clone(),
                dependencies: self.dependencies.clone(),
                linked: self.linked.clone(),
                references,
                aliases,
            },
            changed,
            regroup,
        ))
    }
}
