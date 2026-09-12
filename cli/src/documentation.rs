//! Markdown pages for the checked, source-visible API of one bundle.
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use super::{CliError, replace_file};

use indexmap::IndexMap;
use ruddy::{
    artifact::{self, Data, Metadata},
    compile::AcceptedProgram,
    ir,
    parse::{Annotation, PatternKind, Stmt, StmtKind},
    symbol::Symbol,
    tracking::{FileManager, Span},
    types::{Row, Ty},
};

#[derive(Debug)]
pub(crate) struct Page {
    pub filename: String,
    pub markdown: String,
}

struct Item {
    name: String,
    signature: String,
    documentation: String,
}

#[derive(Default)]
struct Module {
    documentation: String,
    types: Vec<Item>,
    effects: Vec<Item>,
    values: Vec<Item>,
}

fn documentation(metadata: &Metadata) -> String {
    match metadata.get("doc") {
        Some(Data::String(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

fn filename(module: &str) -> String {
    if module.is_empty() {
        "bundle.md".into()
    } else {
        format!("{}.md", module.replace("::", "."))
    }
}

fn heading(bundle: &str, module: &str) -> String {
    if module.is_empty() {
        return bundle.to_owned();
    }
    let segments: Vec<_> = module.split("::").collect();
    let mut path = String::new();
    let mut parts = vec![format!("[{bundle}](bundle.md)")];
    for (index, segment) in segments.iter().enumerate() {
        if !path.is_empty() {
            path.push_str("::");
        }
        path.push_str(segment);
        if index + 1 == segments.len() {
            parts.push((*segment).to_owned());
        } else {
            parts.push(format!("[{segment}]({})", filename(&path)));
        }
    }
    parts.join("::")
}

/// Source declarations supply readable spellings; accepted semantics supply
/// inferred signatures and the authoritative public export boundary.
pub(crate) fn render(
    accepted: &AcceptedProgram,
    statements: &[Stmt],
    files: &mut FileManager,
) -> Result<Vec<Page>, String> {
    let header = accepted.artifact().header();
    let prefix = format!("{}@{}::", header.identity.name, header.identity.version);
    let relative = |name: &str| name.strip_prefix(&prefix).unwrap().to_owned();
    let mut modules = IndexMap::from([(String::new(), Module::default())]);
    let mut filenames = HashSet::from(["bundle.md".to_owned()]);
    for module in &header.modules {
        let name = relative(&module.name);
        let file = filename(&name);
        if !filenames.insert(file.to_lowercase()) {
            return Err(format!(
                "module `{name}` collides with another documentation page `{file}`"
            ));
        }
        modules.insert(
            name,
            Module {
                documentation: documentation(&module.metadata),
                ..Module::default()
            },
        );
    }

    let mut written = HashMap::new();
    collect_source(statements, files, &mut written);
    let item = |name: &str, metadata: &Metadata, span: Span, inferred: String| {
        let relative = relative(name);
        let (module, name) = relative.rsplit_once("::").unwrap_or(("", &relative));
        (
            module.to_owned(),
            Item {
                name: name.to_owned(),
                signature: written.get(&span).cloned().unwrap_or(inferred),
                documentation: documentation(metadata),
            },
        )
    };
    let mint = accepted.mint();
    let program = accepted.ir();
    let semantics = accepted.semantics();
    for (symbol, declaration) in &program.types {
        let name = artifact::qualified(mint, *symbol);
        if let Some(export) = header
            .types
            .iter()
            .find(|entry| entry.name == name && entry.exported)
        {
            let (module, item) = item(
                &name,
                &export.metadata,
                accepted.source().span(declaration.name_at),
                inferred_type(
                    mint.name(*symbol),
                    declaration.params.len(),
                    &semantics.aliases()[symbol],
                ),
            );
            modules.get_mut(&module).unwrap().types.push(item);
        }
    }
    for (symbol, declaration) in &program.effects {
        let name = artifact::qualified(mint, *symbol);
        if let Some(export) = header
            .effects
            .iter()
            .find(|entry| entry.name == name && entry.exported)
        {
            let (module, item) = item(
                &name,
                &export.metadata,
                accepted.source().span(declaration.name_at),
                inferred_effect(mint.name(*symbol), *symbol, declaration, semantics),
            );
            modules.get_mut(&module).unwrap().effects.push(item);
        }
    }
    let values = program
        .externs
        .iter()
        .map(|(symbol, decl)| (symbol, decl.name_at, "extern", &semantics.externs()[symbol]))
        .chain(program.terms.iter().filter_map(|(symbol, decl)| {
            semantics
                .schemes()
                .get(symbol)
                .map(|scheme| (symbol, decl.name_at, "let", scheme))
        }));
    for (symbol, at, keyword, scheme) in values {
        let name = artifact::qualified(mint, *symbol);
        if let Some(export) = header.values.iter().find(|entry| entry.name == name) {
            let signature = format!("{keyword} {}: {scheme}", mint.name(*symbol));
            let (module, item) = item(
                &name,
                &export.metadata,
                accepted.source().span(at),
                signature,
            );
            modules.get_mut(&module).unwrap().values.push(item);
        }
    }

    let names: Vec<_> = modules.keys().cloned().collect();
    Ok(modules
        .into_iter()
        .map(|(name, mut module)| {
            let mut markdown = format!("# {}\n\n", heading(&header.identity.name, &name));
            prose(&mut markdown, &module.documentation);
            let mut children: Vec<_> = names
                .iter()
                .filter(|child| {
                    !child.is_empty()
                        && child.rsplit_once("::").map_or("", |(parent, _)| parent) == name
                })
                .collect();
            children.sort_by_key(|child| child.to_lowercase());
            if !children.is_empty() {
                markdown.push_str("## Modules\n\n");
                let links = children
                    .into_iter()
                    .map(|child| {
                        format!(
                            "[{}]({})",
                            child.rsplit("::").next().unwrap(),
                            filename(child)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\\\n");
                markdown.push_str(&links);
                markdown.push_str("\n\n");
            }
            for (heading, items) in [
                ("Types", &mut module.types),
                ("Effects", &mut module.effects),
                ("Values", &mut module.values),
            ] {
                if items.is_empty() {
                    continue;
                }
                markdown.push_str(&format!("## {heading}\n\n"));
                items.sort_by_key(|item| item.name.to_lowercase());
                for item in items {
                    // Literal types may contain backticks, so size the fence to
                    // keep every valid signature inside its code block.
                    let fence = "`".repeat(
                        3.max(
                            item.signature
                                .split(|c| c != '`')
                                .map(str::len)
                                .max()
                                .unwrap_or(0)
                                + 1,
                        ),
                    );
                    markdown.push_str(&format!(
                        "### {}\n\n{fence}ruddy\n{}\n{fence}\n\n",
                        item.name,
                        item.signature.trim()
                    ));
                    prose(&mut markdown, &item.documentation);
                }
            }
            Page {
                filename: filename(&name),
                markdown: format!("{}\n", markdown.trim_end()),
            }
        })
        .collect())
}

fn parameters(count: usize) -> String {
    (0..count)
        .map(|index| format!(" {}", Ty::Bound(index as u32)))
        .collect()
}

fn inferred_type(name: &str, parameter_count: usize, scheme: &ruddy::types::Scheme) -> String {
    format!("type {name}{} = {scheme}", parameters(parameter_count))
}

fn inferred_effect(
    name: &str,
    symbol: Symbol,
    declaration: &ir::Decl<ir::Effect>,
    semantics: &ruddy::inference::Semantics,
) -> String {
    let parameters = parameters(declaration.params.len());
    match &declaration.value {
        ir::Effect::Operations(operations) if operations.is_empty() => {
            format!("effect {name}{parameters}")
        }
        ir::Effect::Operations(operations) => {
            let operations = operations
                .keys()
                .map(|selector| {
                    let (from, to) = &semantics.operations()[&(symbol, selector.clone())];
                    let signature = Ty::Arrow(from.clone(), to.clone(), Row::closed());
                    match selector {
                        ir::OperationSelector::Unnamed => signature.to_string(),
                        ir::OperationSelector::Named(name) => format!("{name}: {signature}"),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("effect {name}{parameters} = {{ {operations} }}")
        }
        // Source-declared aliases use their written signature. This remains a
        // valid declaration for a future compiler-provided alias without one.
        ir::Effect::Alias(_) => format!("effect {name}{parameters}"),
    }
}

fn prose(output: &mut String, text: &str) {
    if !text.is_empty() {
        output.push_str(text);
        output.push_str("\n\n");
    }
}

fn annotation_end(annotation: &Annotation) -> usize {
    annotation
        .clause
        .as_ref()
        .map_or(annotation.ty.span.end(), |clause| clause.span.end())
}

fn collect_source(
    statements: &[Stmt],
    files: &mut FileManager,
    written: &mut HashMap<Span, String>,
) {
    for statement in statements {
        if let StmtKind::Module {
            body: Some(body), ..
        } = &statement.kind
        {
            collect_source(body, files, written);
            continue;
        }
        let (name, end) = match &statement.kind {
            StmtKind::Type { name, .. } | StmtKind::Effect { name, .. } => {
                (name, statement.span.end())
            }
            StmtKind::Extern { name, ty, abi, .. } => {
                (name, annotation_end(ty).max(abi.span.end()))
            }
            StmtKind::Let {
                pattern,
                ty: Some(ty),
                ..
            } => {
                let PatternKind::Ident { name } = &pattern.tracked else {
                    continue;
                };
                (name, annotation_end(ty))
            }
            _ => continue,
        };
        let source = &files.get_file(statement.span.file_id).content;
        let signature = source[statement.span.start..end].trim().to_owned();
        written.insert(name.span, signature);
    }
}

/// Replace generated pages and retire pages for removed or newly private
/// modules. The ownership marker keeps hand-written Markdown untouched.
pub(crate) fn write(
    target: &Path,
    bundle: &str,
    frontmatter: Option<&str>,
    pages: &[Page],
) -> Result<(), CliError> {
    let marker = format!("<!-- Generated by ruddy doc for {bundle}. -->\n");
    let io_error = |error| {
        CliError::one(format!(
            "could not update documentation in `{}`: {error}",
            target.display()
        ))
    };
    fs::create_dir_all(target).map_err(io_error)?;
    let filenames: HashSet<_> = pages.iter().map(|page| page.filename.as_str()).collect();
    let mut stale = Vec::new();
    for entry in fs::read_dir(target).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if entry.file_type().map_err(io_error)?.is_file()
            && path.extension().is_some_and(|extension| extension == "md")
            && !filenames.contains(entry.file_name().to_string_lossy().as_ref())
            && fs::read(&path)
                .map_err(io_error)?
                .ends_with(marker.as_bytes())
        {
            stale.push(path);
        }
    }
    for page in pages {
        let markdown = format!(
            "{}{}\n{marker}",
            frontmatter.unwrap_or_default(),
            page.markdown
        );
        replace_file(&target.join(&page.filename), markdown.as_bytes())?;
    }
    for path in stale {
        fs::remove_file(path).map_err(io_error)?;
    }
    Ok(())
}
