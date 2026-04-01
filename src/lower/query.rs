use std::cell::RefCell;
use std::collections::HashSet;

use crate::engine::Source;
use crate::parser::{ast, parse_source};
use crate::reporting::{Diag, Diagnostic, TextRange};
use crate::resolver::{FilesystemResolver, Resolver};
use salsa::Accumulator;

use super::resolver_registry::{resolver_dispatch, resolver_token};
use super::*;

#[salsa::tracked]
fn load_canonical_source<'db>(
    db: &'db dyn salsa::Database,
    resolver: ResolverToken<'db>,
    source_canon: InternedString<'db>,
) -> Option<Source> {
    let dispatch = resolver_dispatch(resolver.key(db))?;
    let source_name = source_canon.text(db).clone();
    let contents = (dispatch.resolve)(&source_name)?;
    Some(Source::new(db, source_name, contents))
}

#[salsa::tracked]
pub(super) fn lower_module_query<'db>(
    db: &'db dyn salsa::Database,
    request: ModuleRequest<'db>,
) -> ModuleLoweringResult<'db> {
    let module_path_text = request.module_path(db).text(db).clone();
    if module_stack_contains(&module_path_text) {
        Diag(Diagnostic::error(
            TextRange::generated(),
            format!("cyclic module reference detected while lowering `{module_path_text}`"),
        ))
        .accumulate(db);

        return ModuleLoweringResult {
            module: ir::LoweredModule {
                path: ir::QualifiedName::from_text(&module_path_text, TextRange::generated()),
                source_name: request.source_canon(db).text(db).clone(),
                range: TextRange::generated(),
                statements: vec![ir::Statement::Error(ir::ErrorNode {
                    range: TextRange::generated(),
                })],
                exports: Default::default(),
            },
            children: Vec::new(),
        };
    }
    let _guard = push_module_stack(module_path_text.clone());

    let Some(dispatch) = resolver_dispatch(request.resolver(db).key(db)) else {
        Diag(Diagnostic::error(
            TextRange::generated(),
            "lowering resolver is not registered",
        ))
        .accumulate(db);
        return ModuleLoweringResult {
            module: ir::LoweredModule {
                path: ir::QualifiedName::from_text(&module_path_text, TextRange::generated()),
                source_name: request.source_canon(db).text(db).clone(),
                range: TextRange::generated(),
                statements: Vec::new(),
                exports: Default::default(),
            },
            children: Vec::new(),
        };
    };

    let source = if request.source_canon(db) == request.root_source_canon(db) {
        request
            .root_source(db)
            .or_else(|| load_canonical_source(db, request.resolver(db), request.source_canon(db)))
    } else {
        load_canonical_source(db, request.resolver(db), request.source_canon(db))
    };

    let Some(source) = source else {
        Diag(Diagnostic::error(
            TextRange::generated(),
            format!(
                "failed to load source `{}` while lowering `{module_path_text}`",
                request.source_canon(db).text(db)
            ),
        ))
        .accumulate(db);
        return ModuleLoweringResult {
            module: ir::LoweredModule {
                path: ir::QualifiedName::from_text(&module_path_text, TextRange::generated()),
                source_name: request.source_canon(db).text(db).clone(),
                range: TextRange::generated(),
                statements: vec![ir::Statement::Error(ir::ErrorNode {
                    range: TextRange::generated(),
                })],
                exports: Default::default(),
            },
            children: Vec::new(),
        };
    };

    let parsed = parse_source(db, source);
    if request.source_canon(db) != request.root_source_canon(db) && parsed.ast.bundle_name.is_some()
    {
        Diag(Diagnostic::error(
            parsed.ast.range,
            "imported module file must not declare `bundle`",
        ))
        .accumulate(db);
    }

    let module_path =
        ir::QualifiedName::from_text(request.module_path(db).text(db), TextRange::generated());
    let file_root =
        ir::QualifiedName::from_text(request.file_root_path(db).text(db), TextRange::generated());
    let source_contents = source.contents(db);
    let Some((module_body, module_range)) = locate_module_body(
        db,
        source,
        source_contents,
        &parsed.ast,
        &module_path.segments,
        &file_root.segments,
    ) else {
        return ModuleLoweringResult {
            module: ir::LoweredModule {
                path: module_path.clone().range(TextRange::generated()),
                source_name: request.source_canon(db).text(db).clone(),
                range: TextRange::generated(),
                statements: vec![ir::Statement::Error(ir::ErrorNode {
                    range: TextRange::generated(),
                })],
                exports: Default::default(),
            },
            children: Vec::new(),
        };
    };

    let mut lowerer = ModuleLowerer::new(
        db,
        request,
        source,
        request.source_canon(db).text(db).clone(),
        module_path.segments.clone(),
        dispatch,
    );
    let statements = lowerer.lower_statements(&module_body);
    let exports = lowerer.exports();

    ModuleLoweringResult {
        module: ir::LoweredModule {
            path: module_path.clone().range(module_range),
            source_name: request.source_canon(db).text(db).clone(),
            range: module_range,
            statements,
            exports,
        },
        children: lowerer.children,
    }
}

#[salsa::tracked]
pub fn lower_source<'db>(
    db: &'db dyn salsa::Database,
    source: Source,
    resolver: ResolverToken<'db>,
) -> ir::LoweredSource {
    let parsed = parse_source(db, source);
    let source_name = source.name(db).clone();
    let source_contents = source.contents(db).clone();

    let bundle_name = parsed
        .ast
        .bundle_name
        .as_ref()
        .and_then(|name| name.range.text(&source_contents))
        .unwrap_or_else(|| "_".to_owned());

    let root_module_path_text = bundle_name;

    let source_canon = resolver_dispatch(resolver.key(db))
        .and_then(|dispatch| (dispatch.canonize)(&source_name, &source_name))
        .unwrap_or(source_name);

    let root_request = ModuleRequest::new(
        db,
        InternedString::new(db, root_module_path_text.clone()),
        InternedString::new(db, source_canon.clone()),
        InternedString::new(db, root_module_path_text.clone()),
        InternedString::new(db, root_module_path_text),
        InternedString::new(db, source_canon),
        resolver,
        Some(source),
    );

    let root_lowered = lower_module_query(db, root_request);
    let mut modules = Vec::new();
    let mut queue = vec![root_request];
    let mut visited = HashSet::new();

    while let Some(request) = queue.pop() {
        let path_text = request.module_path(db).text(db).clone();
        if !visited.insert(path_text) {
            continue;
        }
        let lowered = lower_module_query(db, request);
        queue.extend(lowered.children.iter().copied());
        modules.push(lowered.module);
    }

    modules.sort_by_key(|module| module.path.text());

    ir::LoweredSource {
        root_module: root_lowered.module.path,
        modules,
    }
}

pub fn lower_text<R: Resolver>(db: &dyn salsa::Database, source: Source) -> ir::LoweredSource {
    let resolver = resolver_token::<R>(db);
    lower_source(db, source, resolver)
}

pub fn lower_text_fs(db: &dyn salsa::Database, source: Source) -> ir::LoweredSource {
    lower_text::<FilesystemResolver>(db, source)
}

pub fn lower_diagnostics<R: Resolver>(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    let resolver = resolver_token::<R>(db);
    let mut seen = HashSet::new();
    lower_source::accumulated::<Diag>(db, source, resolver)
        .into_iter()
        .map(|diag| diag.0.clone())
        .filter(|diag| seen.insert((diag.severity, diag.range, diag.message.clone())))
        .collect()
}

pub fn lower_diagnostics_fs(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    lower_diagnostics::<FilesystemResolver>(db, source)
}

fn identifier_eq(
    source_contents: &str,
    identifier: &Option<ast::Identifier>,
    expected: &str,
) -> bool {
    identifier
        .as_ref()
        .and_then(|ident| ident.range.text(source_contents))
        .is_some_and(|text| text == expected)
}

fn locate_module_body(
    db: &dyn salsa::Database,
    source: Source,
    source_contents: &str,
    file: &ast::AstFile,
    module_path: &[String],
    file_root: &[String],
) -> Option<(Vec<ast::Statement>, TextRange)> {
    if !module_path.starts_with(file_root) {
        Diag(Diagnostic::error(
            file.range,
            format!(
                "invalid lowering request: module `{}` is not rooted at `{}`",
                module_path.join(PATH_SEP),
                file_root.join(PATH_SEP)
            ),
        ))
        .accumulate(db);
        return None;
    }

    let mut current_body = file.statements.clone();
    let mut current_range = file.range;

    for segment in &module_path[file_root.len()..] {
        let mut next_body = None;
        let mut external_ref_range = None;

        for statement in &current_body {
            match statement {
                ast::Statement::Module { name, body, range } => {
                    if identifier_eq(source_contents, name, segment) {
                        next_body = Some((body.clone(), *range));
                        break;
                    }
                }
                ast::Statement::ModuleRef { name, range, .. } => {
                    if identifier_eq(source_contents, name, segment) {
                        external_ref_range = Some(*range);
                    }
                }
                _ => {}
            }
        }

        if let Some((body, range)) = next_body {
            current_body = body;
            current_range = range;
            continue;
        }

        if let Some(range) = external_ref_range {
            Diag(Diagnostic::error(
                range,
                format!(
                    "module `{segment}` is an external reference and has no inline body in `{}`",
                    source.name(db)
                ),
            ))
            .accumulate(db);
            return None;
        }

        Diag(Diagnostic::error(
            file.range,
            format!(
                "failed to locate inline module `{}` in `{}`",
                module_path.join(PATH_SEP),
                source.name(db)
            ),
        ))
        .accumulate(db);
        return None;
    }

    Some((current_body, current_range))
}

thread_local! {
    static MODULE_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn module_stack_contains(module_path: &str) -> bool {
    MODULE_STACK.with(|stack| stack.borrow().iter().any(|item| item == module_path))
}

struct ModuleStackGuard {
    module_path: String,
}

impl Drop for ModuleStackGuard {
    fn drop(&mut self) {
        MODULE_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(last) = stack.pop() {
                debug_assert_eq!(last, self.module_path);
            }
        });
    }
}

fn push_module_stack(module_path: String) -> ModuleStackGuard {
    MODULE_STACK.with(|stack| stack.borrow_mut().push(module_path.clone()));
    ModuleStackGuard { module_path }
}
