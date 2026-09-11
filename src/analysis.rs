//! Current-buffer analysis shared by editor drivers. Filesystem acquisition is
//! the driver's responsibility; every source used here is an explicit input.
use crate::{
    artifact,
    bundle::{self, Files},
    inference, ir, parse, patterns,
    symbol::{Bundle, Mint, Symbol},
    token,
    tracking::{FileID, FileManager, Span},
    ui,
};
use salsa::Setter;
use std::{collections::HashMap, sync::Arc};

#[salsa::input]
struct File {
    text: Option<String>,
}

#[salsa::tracked(no_eq)]
fn syntax(db: &dyn salsa::Database, file: File, id: FileID) -> bundle::Syntax {
    let lexed = token::lex(file.text(db).as_deref().unwrap_or_default(), id);
    let parsed = parse::parse(lexed.tokens.clone());
    bundle::Syntax {
        tokens: lexed.tokens,
        lex_errors: lexed.errors,
        stmts: parsed.stmts,
        parse_errors: parsed.errors,
    }
}

#[derive(Default)]
pub struct Host {
    db: salsa::DatabaseImpl,
    files: HashMap<String, File>,
    inference: inference::Session,
    focus: Option<String>,
    generation: u64,
    owner: std::sync::Arc<()>,
}

impl Files for Host {
    fn read(&self, path: &str) -> Option<String> {
        self.files
            .get(path)
            .and_then(|file| file.text(&self.db).clone())
    }
    fn cached_syntax(&self, path: &str, id: FileID) -> Option<bundle::Syntax> {
        self.files
            .get(path)
            .map(|file| syntax(&self.db, *file, id).clone())
    }
}

impl Host {
    /// `None` records deletion. Setting unchanged text does not create a revision.
    pub fn set_file(&mut self, path: &str, text: Option<String>) {
        if let Some(file) = self.files.get(path) {
            if file.text(&self.db) != &text {
                file.set_text(&mut self.db).to(text);
            }
        } else {
            self.files
                .insert(path.to_owned(), File::new(&self.db, text));
        }
    }

    pub fn focus(&mut self, path: Option<&str>) {
        self.focus = path.map(str::to_owned);
    }

    pub fn complete(&mut self, analysis: &mut Analysis) {
        self.assert_current(analysis);
        if analysis.complete {
            return;
        }
        analysis.inferred = self.inference.complete(inference::Trace::Off);
        analysis.finish_checks();
        analysis.complete = true;
    }

    fn assert_current(&self, analysis: &Analysis) {
        assert!(
            std::sync::Arc::ptr_eq(&self.owner, &analysis.owner)
                && analysis.generation == self.generation,
            "request the current host revision"
        );
    }

    pub fn request_file(&mut self, analysis: &mut Analysis, path: &str) -> bool {
        self.assert_current(analysis);
        if analysis.complete {
            return false;
        }
        let mut selected: std::collections::HashSet<_> = analysis
            .inferred
            .semantics()
            .schemes()
            .keys()
            .copied()
            .collect();
        let before = selected.len();
        selected.extend(
            analysis
                .built
                .program
                .terms
                .iter()
                .filter(|(_, decl)| {
                    analysis
                        .paths
                        .get(&analysis.built.source.span(decl.name_at).file_id)
                        .is_some_and(|known| known == path)
                })
                .map(|(symbol, _)| *symbol),
        );
        if selected.len() == before {
            return false;
        }
        analysis.inferred = self.inference.request(&selected);
        analysis.finish_checks();
        true
    }

    pub fn solved_groups(&self) -> usize {
        self.inference.solved_groups()
    }

    pub fn analyze(
        &mut self,
        bundle: Bundle,
        root: &str,
        environment: &bundle::Environment,
    ) -> Analysis {
        self.analyze_with_dependencies(bundle, root, environment, &[], &[])
    }

    pub fn analyze_with_dependencies(
        &mut self,
        identity: Bundle,
        root: &str,
        environment: &bundle::Environment,
        imports: &[ir::DependencyImport<'_>],
        linked: &[&artifact::Artifact],
    ) -> Analysis {
        let imports: Vec<_> = imports
            .iter()
            .map(|import| ir::InterfaceImport {
                alias: import.alias,
                header: import.artifact.header(),
            })
            .collect();
        let linked: Vec<_> = linked.iter().map(|artifact| artifact.header()).collect();
        self.analyze_with_interfaces(identity, root, environment, &imports, &linked)
    }

    pub fn analyze_with_interfaces(
        &mut self,
        identity: Bundle,
        root: &str,
        environment: &bundle::Environment,
        imports: &[ir::InterfaceImport<'_>],
        linked: &[&artifact::Header],
    ) -> Analysis {
        let mut files = FileManager::new();
        let loaded = bundle::load(&mut files, self, root, environment);
        self.finish_analysis(identity, loaded, imports, linked)
    }

    /// Acquire inputs through a driver before invoking any syntax query. Module
    /// discovery records missing and competing candidates as well as source text.
    pub fn analyze_from_files(
        &mut self,
        identity: Bundle,
        root: &str,
        environment: &bundle::Environment,
        provider: &dyn Files,
        imports: &[ir::InterfaceImport<'_>],
        linked: &[&artifact::Header],
    ) -> Analysis {
        struct Inputs<'a> {
            host: std::cell::RefCell<&'a mut Host>,
            provider: &'a dyn Files,
        }
        impl Files for Inputs<'_> {
            fn read(&self, path: &str) -> Option<String> {
                let text = self.provider.read(path);
                self.host.borrow_mut().set_file(path, text.clone());
                text
            }
            fn cached_syntax(&self, path: &str, id: FileID) -> Option<bundle::Syntax> {
                self.host.borrow().cached_syntax(path, id)
            }
        }
        let mut files = FileManager::new();
        let inputs = Inputs {
            host: std::cell::RefCell::new(self),
            provider,
        };
        let loaded = bundle::load(&mut files, &inputs, root, environment);
        inputs
            .host
            .into_inner()
            .finish_analysis(identity, loaded, imports, linked)
    }

    fn finish_analysis(
        &mut self,
        identity: Bundle,
        loaded: bundle::Output,
        imports: &[ir::InterfaceImport<'_>],
        linked: &[&artifact::Header],
    ) -> Analysis {
        self.generation += 1;
        let paths: HashMap<_, _> = loaded
            .loaded
            .iter()
            .map(|file| (file.id, file.path.clone()))
            .collect();
        let sources = loaded
            .loaded
            .iter()
            .map(|file| (file.path.clone(), self.read(&file.path).unwrap_or_default()))
            .collect();
        let mut diagnostics: Vec<_> = loaded
            .errors
            .iter()
            .map(|error| error.diagnostic())
            .collect();
        for file in &loaded.loaded {
            diagnostics.extend(file.lex_errors.iter().map(|error| error.diagnostic()));
            diagnostics.extend(file.parse_errors.iter().map(|error| error.diagnostic()));
        }
        let syntax_clean = diagnostics.is_empty();
        let mut mint = Mint::new(identity);
        let mut built = ir::build_with_interfaces(&mut mint, loaded.stmts, imports, linked);
        built.errors.extend(
            built.program.terms.values().filter_map(|declaration| {
                crate::externs::export_request(&declaration.metadata).err()
            }),
        );
        diagnostics.extend(
            built
                .errors
                .iter()
                .map(|error| error.diagnostic(&built.source)),
        );
        let mint = Arc::new(mint);
        let built = Prepared {
            program: Arc::new(built.program),
            source: built.source,
            names: built.names,
            errors: built.errors,
        };
        let selected = self.focus.as_ref().map(|path| {
            built
                .program
                .terms
                .iter()
                .filter(|(_, decl)| {
                    paths.get(&built.source.span(decl.name_at).file_id) == Some(path)
                })
                .map(|(symbol, _)| *symbol)
                .collect()
        });
        let inferred = self.inference.infer_shared(
            mint.clone(),
            built.program.clone(),
            inference::Trace::Off,
            selected.as_ref(),
        );
        let mut file_terms: HashMap<String, Vec<Symbol>> = HashMap::new();
        for (symbol, decl) in &built.program.terms {
            if let Some(path) = paths.get(&built.source.span(decl.name_at).file_id) {
                file_terms.entry(path.clone()).or_default().push(*symbol);
            }
        }
        let mut analysis = Analysis {
            file_terms,
            module_files: loaded.module_files,
            mint,
            built,
            inferred,
            checks: patterns::Output {
                errors: Vec::new(),
                reports: Vec::new(),
            },
            syntax_clean,
            linked: linked.iter().map(|header| (*header).clone()).collect(),
            dependencies: imports
                .iter()
                .map(|import| artifact::Dependency {
                    name: import.header.identity.name.clone(),
                    version: import.header.identity.version.clone(),
                })
                .collect(),
            frontend: diagnostics.clone(),
            diagnostics,
            paths,
            sources,
            complete: selected.is_none(),
            generation: self.generation,
            owner: self.owner.clone(),
        };
        analysis.finish_checks();
        analysis
    }
}

struct Prepared {
    program: Arc<ir::Program>,
    source: crate::tracking::SourceMap,
    names: ir::ScopeNames,
    errors: Vec<ir::Error>,
}

pub struct Analysis {
    file_terms: HashMap<String, Vec<Symbol>>,
    module_files: HashMap<Span, FileID>,
    mint: Arc<Mint>,
    built: Prepared,
    inferred: inference::Output,
    checks: patterns::Output,
    syntax_clean: bool,
    frontend: Vec<ui::Diagnostic>,
    complete: bool,
    generation: u64,
    owner: std::sync::Arc<()>,
    linked: Vec<artifact::Header>,
    dependencies: Vec<artifact::Dependency>,
    pub diagnostics: Vec<ui::Diagnostic>,
    pub paths: HashMap<FileID, String>,
    pub sources: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub span: Span,
    pub ty: String,
    pub runtime_information: Option<String>,
}

impl Analysis {
    fn finish_checks(&mut self) {
        self.diagnostics = self.frontend.clone();
        self.diagnostics.extend(
            self.inferred
                .errors()
                .iter()
                .map(|error| error.diagnostic(&self.built.source)),
        );
        if self.inferred.errors().is_empty() {
            self.diagnostics.extend(
                crate::externs::review(self.inferred.semantics())
                    .iter()
                    .map(|error| error.diagnostic(&self.built.source)),
            );
        }
        // Unrequested bodies explicitly retain undecided types and do not have
        // the semantic promises pattern checking requires.
        self.checks = patterns::check_inferred(&self.built.program, &self.inferred);
        self.diagnostics.extend(
            self.checks
                .errors
                .iter()
                .map(|error| error.diagnostic(&self.built.source)),
        );
    }

    pub fn mint(&self) -> &Mint {
        &self.mint
    }

    /// Background-only lowering. Refuse recovered syntax or dependency bodies
    /// whose interfaces differ from the exact frontend inputs. The coherent
    /// compiler fields remain private so accepted state cannot mix revisions.
    pub fn lower(
        &self,
        linked: &[&artifact::Artifact],
    ) -> (Option<artifact::Artifact>, Vec<ui::Diagnostic>) {
        if !self.complete
            || !self.syntax_clean
            || self
                .linked
                .iter()
                .any(|header| !linked.iter().any(|artifact| artifact.header() == header))
        {
            return (None, Vec::new());
        }
        let summaries = linked
            .iter()
            .flat_map(|artifact| &artifact.lir().globals)
            .filter_map(|global| {
                global
                    .callable
                    .map(|summary| (global.name.clone(), summary))
            })
            .collect();
        let mut program = (*self.built.program).clone();
        self.inferred.apply_types(&mut program);
        let built = ir::Output {
            program,
            source: self.built.source.clone(),
            names: self.built.names.clone(),
            errors: self.built.errors.clone(),
        };
        match crate::compile::accept(
            (*self.mint).clone(),
            built,
            self.inferred.clone(),
            self.checks.clone(),
            self.dependencies.clone(),
            summaries,
        ) {
            Ok(accepted) => (Some(accepted.artifact().clone()), Vec::new()),
            Err(partial) => {
                let diagnostics =
                    partial
                        .ir
                        .errors
                        .iter()
                        .filter(|error| {
                            !self.built.errors.iter().any(|old| {
                                old.at == error.at && old.kind.code() == error.kind.code()
                            })
                        })
                        .map(|error| error.diagnostic(&self.built.source))
                        .collect();
                (None, diagnostics)
            }
        }
    }

    pub fn interface(&self) -> artifact::Header {
        artifact::interface(
            &self.mint,
            &self.built.program,
            self.inferred.semantics(),
            Vec::new(),
        )
    }

    fn terms_in<'a>(
        &'a self,
        path: &str,
    ) -> impl Iterator<Item = (Symbol, &'a ir::Decl<ir::Term>)> {
        self.file_terms
            .get(path)
            .into_iter()
            .flatten()
            .map(|symbol| {
                (
                    *symbol,
                    self.inferred
                        .semantics()
                        .typed()
                        .get(symbol)
                        .unwrap_or(&self.built.program.terms[symbol]),
                )
            })
    }

    fn contains(&self, span: Span, path: &str, offset: usize) -> bool {
        self.paths
            .get(&span.file_id)
            .is_some_and(|known| known == path)
            && span.start <= offset
            && offset < span.end()
    }

    pub fn hover(&self, path: &str, offset: usize) -> Option<Hover> {
        let program = &self.built.program;
        let declarations = self
            .terms_in(path)
            .map(|(symbol, decl)| (symbol, decl.name_at))
            .chain(
                program
                    .externs
                    .iter()
                    .map(|(symbol, decl)| (*symbol, decl.name_at)),
            )
            .chain(
                program
                    .types
                    .iter()
                    .map(|(symbol, decl)| (*symbol, decl.name_at)),
            )
            .chain(
                program
                    .effects
                    .iter()
                    .map(|(symbol, decl)| (*symbol, decl.name_at)),
            )
            .chain(
                program
                    .modules
                    .iter()
                    .map(|(symbol, decl)| (*symbol, decl.name_at)),
            );
        for (symbol, name_at) in declarations {
            let span = self.built.source.span(name_at);
            if self.contains(span, path, offset) {
                return self.declaration_type(symbol).map(|ty| Hover {
                    span,
                    ty,
                    runtime_information: self
                        .inferred
                        .semantics()
                        .schemes()
                        .get(&symbol)
                        .or_else(|| self.inferred.semantics().externs().get(&symbol))
                        .and_then(crate::reification::explain),
                });
            }
        }
        self.term_at(path, offset).map(|term| Hover {
            span: self.built.source.span(term.at),
            ty: term.ty.to_string(),
            runtime_information: None,
        })
    }

    fn declaration_type(&self, symbol: Symbol) -> Option<String> {
        use crate::types::{Row, Ty};
        let semantics = self.inferred.semantics();
        if let Some(scheme) = semantics
            .schemes()
            .get(&symbol)
            .or_else(|| semantics.externs().get(&symbol))
            .or_else(|| semantics.aliases().get(&symbol))
        {
            return Some(scheme.to_string());
        }
        let name = self.mint.name(symbol);
        if self.built.program.modules.contains_key(&symbol) {
            return Some(format!("module {name}"));
        }
        let declaration = self.built.program.effects.get(&symbol)?;
        // Semantic signatures use positional bound variables. Match their
        // spelling in the header, regardless of the source parameter names.
        let parameters = (0..declaration.params.len())
            .map(|index| format!(" {}", Ty::Bound(index as u32)))
            .collect::<String>();
        let body = match &declaration.value {
            ir::Effect::Operations(operations) => {
                let signatures = operations
                    .keys()
                    .map(|selector| {
                        let (from, to) = semantics.operations().get(&(symbol, selector.clone()))?;
                        let signature = Ty::Arrow(from.clone(), to.clone(), Row::closed());
                        Some(match selector {
                            ir::OperationSelector::Named(name) => format!("{name}: {signature}"),
                            ir::OperationSelector::Unnamed => signature.to_string(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                format!("{{ {} }}", signatures.join(", "))
            }
            ir::Effect::Alias(_) => {
                let alias = semantics.effect_aliases().get(&symbol)?;
                let mut cases = alias
                    .cases
                    .iter()
                    .map(|(effect, args)| {
                        let mut case = format!("!{}", self.mint.name(*effect));
                        for arg in args {
                            case.push_str(&format!(" ({arg})"));
                        }
                        case
                    })
                    .collect::<Vec<_>>();
                if let Some(tail) = alias.tail {
                    cases.push(format!("..{}", Ty::Bound(tail)));
                }
                if cases.is_empty() {
                    "()".into()
                } else {
                    cases.join(" + ")
                }
            }
        };
        Some(format!("effect {name}{parameters} = {body}"))
    }

    fn written_types(&self, path: &str) -> Vec<&ir::Type> {
        let mut roots = Vec::new();
        for (_, decl) in self.terms_in(path) {
            if let Some(annotation) = &decl.annotation {
                roots.push(&annotation.ty);
            }
            for term in decl.value.walk() {
                if let ir::TermKind::Let {
                    annotation: Some(annotation),
                    ..
                } = &term.kind
                {
                    roots.push(&annotation.ty);
                }
            }
        }
        for decl in self.built.program.externs.values() {
            if let Some(annotation) = &decl.annotation {
                roots.push(&annotation.ty);
            }
        }
        roots.extend(self.built.program.types.values().map(|decl| &decl.value));
        for decl in self.built.program.effects.values() {
            match &decl.value {
                ir::Effect::Operations(operations) => {
                    for operation in operations.values() {
                        roots.push(&operation.from);
                        roots.push(&operation.to);
                    }
                }
                ir::Effect::Alias(alias) => roots.push(&alias.expanded),
            }
        }
        roots.retain(|ty| {
            self.paths
                .get(&self.built.source.span(ty.at).file_id)
                .is_some_and(|known| known == path)
        });
        roots
    }

    fn term_at(&self, path: &str, offset: usize) -> Option<&ir::Term> {
        self.terms_in(path)
            .flat_map(|(_, decl)| decl.value.walk())
            .filter(|term| self.contains(self.built.source.span(term.at), path, offset))
            .min_by_key(|term| self.built.source.span(term.at).width)
    }

    pub fn definition(&self, path: &str, offset: usize) -> Option<Span> {
        if let Some((_, file)) = self
            .module_files
            .iter()
            .find(|(name, _)| self.contains(**name, path, offset))
        {
            return Some(file.span(0, 0));
        }
        self.binding_span(self.referenced_symbol(path, offset)?)
    }

    pub fn referenced_symbol(&self, path: &str, offset: usize) -> Option<Symbol> {
        if let Some(term) = self.term_at(path, offset) {
            match &term.kind {
                ir::TermKind::Ident(symbol) => return Some(*symbol),
                ir::TermKind::Operation { effect, .. }
                    if self.contains(self.built.source.span(effect.at), path, offset) =>
                {
                    return Some(effect.anchored);
                }
                _ => {}
            }
        }
        use ir::TypeKind as T;
        let mut work = self.written_types(path);
        while let Some(ty) = work.pop() {
            if !self.contains(self.built.source.span(ty.at), path, offset) {
                continue;
            }
            let effects = match &ty.anchored {
                T::Ident(symbol) => return Some(*symbol),
                T::Apply {
                    head,
                    head_at,
                    args,
                } => {
                    if self.contains(self.built.source.span(*head_at), path, offset) {
                        return Some(*head);
                    }
                    work.extend(args);
                    None
                }
                T::Array(element) => {
                    work.push(element);
                    None
                }
                T::Mut(region, element) => {
                    work.push(region);
                    work.push(element);
                    None
                }
                T::Struct { fields, .. } => {
                    work.extend(fields.values().filter_map(|field| field.value()));
                    None
                }
                T::Sum { cases, .. } => {
                    work.extend(cases.values().filter_map(|case| case.payload()));
                    None
                }
                T::Arrow { from, to, effects } => {
                    work.push(from);
                    work.push(to);
                    Some(&**effects)
                }
                T::Effects(effects) => Some(&**effects),
                _ => None,
            };
            if let Some(effects) = effects {
                for effect in effects.effects.values() {
                    let (symbol, at, args, expanded) = match effect {
                        ir::EffectLabel::Written {
                            symbol,
                            name_at,
                            args,
                            expanded,
                            ..
                        }
                        | ir::EffectLabel::Absent {
                            symbol,
                            name_at,
                            args,
                            expanded,
                        } => (symbol, name_at, args, expanded),
                    };
                    if !expanded && self.contains(self.built.source.span(*at), path, offset) {
                        return Some(*symbol);
                    }
                    work.extend(args);
                }
            }
        }
        None
    }

    pub fn symbol_named(&self, qualified: &str) -> Option<Symbol> {
        self.mint.symbols().find(|symbol| {
            !self.mint.is_local(*symbol) && artifact::qualified(&self.mint, *symbol) == qualified
        })
    }

    pub fn binding_span(&self, symbol: Symbol) -> Option<Span> {
        let program = &self.built.program;
        let global = program
            .terms
            .get(&symbol)
            .map(|decl| decl.name_at)
            .or_else(|| program.externs.get(&symbol).map(|decl| decl.name_at))
            .or_else(|| program.types.get(&symbol).map(|decl| decl.name_at))
            .or_else(|| program.effects.get(&symbol).map(|decl| decl.name_at));
        if let Some(at) = global {
            return self.built.source.located(at);
        }
        for term in program.terms.values().flat_map(|decl| decl.value.walk()) {
            let at = match &term.kind {
                ir::TermKind::Fn { arg, .. } if arg.anchored == symbol => Some(arg.at),
                ir::TermKind::Let { name, .. } if name.anchored == symbol => Some(name.at),
                ir::TermKind::Handle { handler, .. } => handler
                    .arms
                    .iter()
                    .find(|arm| arm.binder.anchored == symbol)
                    .map(|arm| arm.binder.at)
                    .or_else(|| {
                        handler
                            .ret
                            .as_ref()
                            .filter(|ret| ret.binder.anchored == symbol)
                            .map(|ret| ret.binder.at)
                    }),
                ir::TermKind::Match { arms, .. } => arms
                    .iter()
                    .find_map(|(pattern, _)| pattern_binding(pattern, symbol)),
                _ => None,
            };
            if let Some(at) = at {
                return self.built.source.located(at);
            }
        }
        None
    }
}

fn pattern_binding(pattern: &ir::Pattern, symbol: Symbol) -> Option<crate::tracking::Anchor> {
    pattern_bindings(pattern)
        .into_iter()
        .find(|name| name.anchored == symbol)
        .map(|name| name.at)
}

fn pattern_bindings(pattern: &ir::Pattern) -> Vec<crate::tracking::Anchored<Symbol>> {
    use ir::PatternKind as P;
    let mut out = Vec::new();
    let mut work = vec![pattern];
    while let Some(pattern) = work.pop() {
        match &pattern.anchored {
            P::Bind(name) => out.push(*name),
            P::Struct { fields, .. } => work.extend(fields.values().map(|field| &field.value)),
            P::Tag {
                payload: Some(payload),
                ..
            } => work.push(payload),
            P::Array {
                before,
                rest,
                after,
            } => {
                work.extend(before);
                work.extend(after);
                if let Some(name) = rest.as_ref().and_then(|rest| rest.name) {
                    out.push(name);
                }
            }
            _ => {}
        }
    }
    out
}

fn record_fields(
    inferred: &inference::Output,
    ty: &Arc<crate::types::Ty>,
) -> Vec<(String, Arc<crate::types::Ty>)> {
    use crate::types::{Presence, Rest, Ty};
    let ty = inference::unfold(inferred.semantics().aliases(), ty);
    let mut ty = &*ty;
    while let Ty::Package(inner) | Ty::Hidden { body: inner, .. } = ty {
        ty = inner;
    }
    let Ty::Struct(row) = ty else {
        return Vec::new();
    };
    let mut row = row;
    let mut fields = Vec::new();
    loop {
        fields.extend(
            row.labels
                .iter()
                .filter(|(_, field)| !matches!(field.presence, Presence::Absent))
                .map(|(label, field)| (label.clone(), field.ty.clone())),
        );
        match &row.rest {
            Rest::More(next) => row = next,
            _ => break,
        }
    }
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Value,
    Type,
    Effect,
    Module,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
}

impl Analysis {
    pub fn completions(&self, path: &str, offset: usize) -> Vec<Completion> {
        use crate::symbol::Namespace;
        use crate::types::Ty;
        let Some(text) = self
            .sources
            .get(path)
            .and_then(|source| source.get(..offset))
        else {
            return Vec::new();
        };
        let start = text
            .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
            .map_or(0, |at| at + text[at..].chars().next().unwrap().len_utf8());
        let prefix = &text[start..];
        let before = &text[..start];
        let mut candidates: Vec<(String, Option<std::sync::Arc<Ty>>, CompletionKind)> = Vec::new();
        let owner = self
            .terms_in(path)
            .filter(|(_, decl)| {
                let span = self.built.source.span(decl.value.at);
                self.paths
                    .get(&span.file_id)
                    .is_some_and(|known| known == path)
                    && span.start <= offset
                    && offset <= span.end()
            })
            .min_by_key(|(_, decl)| self.built.source.span(decl.value.at).width)
            .map(|(symbol, _)| symbol);
        let module = owner
            .and_then(|symbol| self.mint.parent(symbol))
            .or_else(|| {
                self.paths
                    .iter()
                    .find(|(_, known)| known.as_str() == path)
                    .and_then(|(file, _)| self.built.names.scope_at(*file, offset))
            });
        let mut modules = vec![module];
        let mut current = module;
        while let Some(at) = current {
            current = self.mint.parent(at.symbol());
            modules.push(current);
        }
        if let Some(prelude) = self.built.names.prelude {
            modules.push(Some(prelude));
        }
        let effect_context = before.ends_with('!');
        let qualifier = before.strip_suffix('!').unwrap_or(before);
        let qualified = qualifier.strip_suffix("::").map(|before| {
            let start = before
                .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == ':'))
                .map_or(0, |at| at + before[at..].chars().next().unwrap().len_utf8());
            let mut components = before[start..].split("::");
            let first = components.next()?;
            let mut parent = modules
                .iter()
                .find_map(|module| self.built.names.module(*module, first))?;
            for component in components {
                parent = self.built.names.module(Some(parent), component)?;
            }
            Some(parent)
        });
        let type_context = !effect_context
            && (self.written_types(path).iter().any(|ty| {
                let span = self.built.source.span(ty.at);
                span.start <= start && offset <= span.end()
            }) || self
                .paths
                .iter()
                .find(|(_, known)| known.as_str() == path)
                .is_some_and(|(file, _)| incomplete_type_context(text, *file)));
        if type_context && qualified.is_none() {
            candidates.extend(
                [
                    "Nat", "Int", "Real", "String", "Bool", "Nat8", "Nat16", "Nat32", "Nat64",
                    "Int8", "Int16", "Int32", "Int64",
                ]
                .into_iter()
                .map(|name| (name.to_owned(), None, CompletionKind::Type)),
            );
        }
        let visible: Vec<_> = match qualified {
            Some(Some(module)) => vec![Some(module)],
            Some(None) => Vec::new(),
            None => modules,
        };
        // Iterate outer scopes first: inner declarations shadow them when the
        // final candidate list is deduplicated.
        for module in visible.into_iter().rev() {
            for (namespace, name, symbol) in self.built.names.globals(module) {
                let kind = match namespace {
                    Namespace::Terms if !type_context && !effect_context => CompletionKind::Value,
                    Namespace::Types if type_context => CompletionKind::Type,
                    Namespace::Effects if effect_context => CompletionKind::Effect,
                    _ => continue,
                };
                let ty = self
                    .inferred
                    .semantics()
                    .schemes()
                    .get(&symbol)
                    .or_else(|| self.inferred.semantics().externs().get(&symbol))
                    .or_else(|| self.built.program.external_schemes.get(&symbol))
                    .map(|scheme| scheme.body().clone());
                candidates.push((name.to_owned(), ty, kind));
            }
            candidates.extend(
                self.built
                    .names
                    .modules(module)
                    .map(|(name, _)| (name.to_owned(), None, CompletionKind::Module)),
            );
        }
        if qualified.is_none() && !type_context && !effect_context {
            for term in self.terms_in(path).flat_map(|(_, decl)| decl.value.walk()) {
                let scope = self.built.source.span(term.at);
                if self
                    .paths
                    .get(&scope.file_id)
                    .is_none_or(|known| known != path)
                    || offset < scope.start
                    || offset > scope.end()
                {
                    continue;
                }
                match &term.kind {
                    ir::TermKind::Fn { arg, .. } => {
                        let ty = match &*term.ty {
                            Ty::Arrow(from, ..) => Some(from.clone()),
                            _ => None,
                        };
                        candidates.push((
                            self.mint.name(arg.anchored).to_owned(),
                            ty,
                            CompletionKind::Value,
                        ));
                    }
                    ir::TermKind::Let { name, .. } => {
                        let ty = self
                            .inferred
                            .semantics()
                            .locals()
                            .get(&name.anchored)
                            .map(|scheme| scheme.body().clone());
                        candidates.push((
                            self.mint.name(name.anchored).to_owned(),
                            ty,
                            CompletionKind::Value,
                        ));
                    }
                    ir::TermKind::Match { arms, .. } => {
                        for (pattern, body) in arms {
                            let span = self.built.source.span(body.at);
                            if span.start <= offset && offset <= span.end() {
                                candidates.extend(pattern_bindings(pattern).into_iter().map(
                                    |name| {
                                        (
                                            self.mint.name(name.anchored).to_owned(),
                                            None,
                                            CompletionKind::Value,
                                        )
                                    },
                                ));
                            }
                        }
                    }
                    ir::TermKind::Handle { handler, .. } => {
                        for (binder, body) in handler
                            .arms
                            .iter()
                            .map(|arm| (&arm.binder, &arm.body))
                            .chain(handler.ret.iter().map(|ret| (&ret.binder, &*ret.body)))
                        {
                            let span = self.built.source.span(body.at);
                            if span.start <= offset && offset <= span.end() {
                                candidates.push((
                                    self.mint.name(binder.anchored).to_owned(),
                                    None,
                                    CompletionKind::Value,
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(receiver) = before.strip_suffix('.') {
            let start = receiver
                .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.'))
                .map_or(0, |at| {
                    at + receiver[at..].chars().next().unwrap().len_utf8()
                });
            let mut components = receiver[start..].split('.');
            let name = components.next().unwrap_or_default();
            let mut ty = candidates
                .iter()
                .rev()
                .find(|(label, _, _)| label == name)
                .and_then(|(_, ty, _)| ty.clone());
            for field in components {
                ty = ty.and_then(|ty| {
                    record_fields(&self.inferred, &ty)
                        .into_iter()
                        .find(|(label, _)| label == field)
                        .map(|(_, ty)| ty)
                });
            }
            if ty.is_none() {
                ty = receiver
                    .len()
                    .checked_sub(1)
                    .and_then(|at| self.term_at(path, at))
                    .map(|term| term.ty.clone());
            }
            let mut fields: Vec<_> = ty
                .into_iter()
                .flat_map(|ty| record_fields(&self.inferred, &ty))
                .filter(|(label, _)| label.starts_with(prefix))
                .map(|(label, ty)| Completion {
                    label,
                    detail: Some(ty.to_string()),
                    kind: CompletionKind::Field,
                })
                .collect();
            fields.sort_by(|a, b| a.label.cmp(&b.label));
            return fields;
        }
        let mut out: Vec<_> = candidates
            .into_iter()
            .rev()
            .filter(|(label, _, _)| label.starts_with(prefix))
            .map(|(label, ty, kind)| Completion {
                label,
                detail: ty.map(|ty| ty.to_string()),
                kind,
            })
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out.dedup_by(|a, b| a.label == b.label);
        out
    }
}

// A missing `=` can prevent the parser from retaining a declaration. Token
// context still recognizes its annotation without confusing a record value's
// colon with a type annotation, or treating comment/string contents as syntax.
fn incomplete_type_context(text: &str, file: FileID) -> bool {
    use token::Kind as T;
    let tokens = token::lex(text, file).tokens;
    let Some(at) = tokens.iter().rposition(|token| {
        matches!(
            token.tracked,
            T::Let | T::Extern | T::Type | T::Effect | T::Fn | T::Return | T::End
        )
    }) else {
        return false;
    };
    let rest = &tokens[at + 1..];
    match tokens[at].tracked {
        T::Let | T::Extern => {
            rest.iter().any(|token| matches!(token.tracked, T::Colon))
                && !rest.iter().any(|token| matches!(token.tracked, T::Equal))
        }
        T::Type => rest.iter().any(|token| matches!(token.tracked, T::Equal)),
        T::Effect => rest.iter().any(|token| matches!(token.tracked, T::Colon)),
        _ => false,
    }
}
