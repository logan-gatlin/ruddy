//! Revision-scoped diagnostic rendering, separate from scheduler publication.
use super::{LineIndex, positions::LineIndexes};
use crate::workspace::ProjectAnalysis;
use ruddy::cancellation::checkpoint;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use url::Url;

struct Cached {
    owner: Arc<()>,
    revision: (u64, u64),
    diagnostic_count: usize,
    index: Arc<LineIndex>,
    values: Vec<Value>,
}

struct DiagnosticIndex {
    owner: Arc<()>,
    revision: (u64, u64),
    count: usize,
    files: HashMap<String, Arc<[usize]>>,
}

#[derive(Default)]
pub(super) struct Diagnostics {
    indexes: LineIndexes,
    rendered: RefCell<HashMap<PathBuf, Cached>>,
    projects: RefCell<HashMap<PathBuf, DiagnosticIndex>>,
}

impl Diagnostics {
    pub fn index(&self, path: &Path, text: &str) -> Arc<LineIndex> {
        self.indexes.get(path, text)
    }

    pub fn retain(&self, paths: &HashSet<PathBuf>) {
        self.indexes.retain(paths);
        self.rendered
            .borrow_mut()
            .retain(|path, _| paths.contains(path));
        self.projects
            .borrow_mut()
            .retain(|directory, _| paths.iter().any(|path| path.starts_with(directory)));
    }

    pub fn for_file(&self, project: &ProjectAnalysis, logical: &str, text: &str) -> Vec<Value> {
        checkpoint();
        let analysis = project.analysis();
        let path = project.source_directory.join(logical);
        let index = self.index(&path, text);
        let revision = analysis.diagnostic_revision();
        // Retain the identity as well as its local revision counters: a new
        // host can start at the same counters, and addresses must not be reused.
        let owner = analysis.cache_identity();
        if let Some(cached) = self.rendered.borrow().get(&path)
            && Arc::ptr_eq(&cached.owner, &owner)
            && cached.revision == revision
            && cached.diagnostic_count == analysis.diagnostics.len()
            && Arc::ptr_eq(&index, &cached.index)
        {
            return cached.values.clone();
        }
        let selected = {
            let mut projects = self.projects.borrow_mut();
            let needs_index = projects.get(&project.source_directory).is_none_or(|index| {
                !Arc::ptr_eq(&index.owner, &owner)
                    || index.revision != revision
                    || index.count != analysis.diagnostics.len()
            });
            if needs_index {
                let mut files: HashMap<String, Vec<usize>> = HashMap::new();
                for (at, diagnostic) in analysis.diagnostics.iter().enumerate() {
                    checkpoint();
                    if let Some(logical) = analysis.paths.get(&diagnostic.primary.span.file_id) {
                        files.entry(logical.clone()).or_default().push(at);
                    }
                }
                projects.insert(
                    project.source_directory.clone(),
                    DiagnosticIndex {
                        owner: owner.clone(),
                        revision,
                        count: analysis.diagnostics.len(),
                        files: files
                            .into_iter()
                            .map(|(logical, diagnostics)| (logical, diagnostics.into()))
                            .collect(),
                    },
                );
            }
            projects[&project.source_directory]
                .files
                .get(logical)
                .cloned()
                .unwrap_or_default()
        };
        let mut related_indexes = HashMap::new();
        let mut diagnostics = Vec::new();
        for &at in &*selected {
            checkpoint();
            let diagnostic = &analysis.diagnostics[at];
            let mut message = diagnostic.title.clone();
            for detail in std::iter::once(&diagnostic.primary.message)
                .chain(&diagnostic.notes)
                .chain(&diagnostic.help)
            {
                if !detail.is_empty() && detail != &diagnostic.title {
                    message.push('\n');
                    message.push_str(detail);
                }
            }
            let related: Vec<_> = diagnostic.related.iter().filter_map(|annotation| {
                checkpoint();
                let logical = analysis.paths.get(&annotation.span.file_id)?;
                let path = project.source_directory.join(logical);
                let uri = Url::from_file_path(&path).ok()?;
                let source = analysis.sources.get(logical)?;
                let index = related_indexes.entry(logical).or_insert_with(|| self.index(&path, source));
                Some(json!({"location":{"uri":uri.as_str(),"range":index.range(annotation.span)},"message":annotation.message}))
            }).collect();
            diagnostics.push(json!({"range":index.range(diagnostic.primary.span),"severity":1,"source":"ruddy","code":diagnostic.code,"message":message,"relatedInformation":related}));
        }
        diagnostics.extend(analysis.tail_recursion(logical).iter().map(|fix| {
            checkpoint();
            tail_diagnostic(&index, fix)
        }));
        checkpoint();
        self.rendered.borrow_mut().insert(
            path,
            Cached {
                owner,
                revision,
                diagnostic_count: analysis.diagnostics.len(),
                index,
                values: diagnostics.clone(),
            },
        );
        diagnostics
    }
}

pub(super) fn tail_diagnostic(index: &LineIndex, fix: &ruddy::analysis::TailRecursion) -> Value {
    json!({"range":index.range(fix.binding),"severity":3,"source":"ruddy",
        "code":ruddy::ui::TAIL_RECURSION_CODE,"message":ruddy::ui::TAIL_RECURSION_MESSAGE})
}
