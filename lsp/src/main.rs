use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruddy::{
    Diagnostic as RuddyDiagnostic, DiagnosticSeverity as RuddyDiagnosticSeverity, Eng, Source,
    TextRange, check_text_fs, lower_diagnostics_fs, parse_text,
    ty::store::TypeStore,
    ty::{Kind, KindId, MetaTypeVariableId, TypeBinderId, TypeConstructor, TypeId, TypeKind},
    typed_ir as tir,
};
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity as LspDiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, MarkupContent, MarkupKind, MessageType,
    Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    documents: Mutex<HashMap<Url, String>>,
    bundle_diagnostics: Mutex<HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>>>,
    workspace_roots: Mutex<Vec<PathBuf>>,
    document_roots: Mutex<HashMap<Url, HashSet<Url>>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
            bundle_diagnostics: Mutex::new(HashMap::new()),
            workspace_roots: Mutex::new(Vec::new()),
            document_roots: Mutex::new(HashMap::new()),
        }
    }

    async fn analyze_open_document(&self, document_uri: Url, discovery_mode: RootDiscoveryMode) {
        let documents = {
            let documents = self.documents.lock().await;
            documents.clone()
        };

        if !documents.contains_key(&document_uri) {
            return;
        }

        let workspace_roots = {
            let workspace_roots = self.workspace_roots.lock().await;
            workspace_roots.clone()
        };

        let known_roots = {
            let document_roots = self.document_roots.lock().await;
            document_roots
                .get(&document_uri)
                .cloned()
                .unwrap_or_default()
        };

        let analysis = collect_bundle_snapshots_for_document(
            &document_uri,
            &documents,
            &workspace_roots,
            &known_roots,
            discovery_mode,
        );

        let mut roots_to_reconcile = known_roots;
        roots_to_reconcile.extend(analysis.root_snapshots.keys().cloned());

        let open_documents = documents.keys().cloned().collect();
        self.apply_analysis_update(document_uri, analysis, roots_to_reconcile, open_documents)
            .await;
    }

    async fn apply_analysis_update(
        &self,
        trigger_uri: Url,
        mut analysis: DocumentAnalysis,
        roots_to_reconcile: HashSet<Url>,
        open_documents: HashSet<Url>,
    ) {
        let updates = {
            let mut snapshots = self.bundle_diagnostics.lock().await;
            let mut document_roots = self.document_roots.lock().await;
            document_roots.retain(|document_uri, _| open_documents.contains(document_uri));

            let mut affected = HashSet::new();

            for root_uri in &roots_to_reconcile {
                if let Some(snapshot) = analysis.root_snapshots.remove(root_uri) {
                    let previous = snapshots
                        .insert(root_uri.clone(), snapshot)
                        .unwrap_or_default();
                    affected.extend(previous.keys().cloned());
                    if let Some(current) = snapshots.get(root_uri) {
                        affected.extend(current.keys().cloned());
                    }
                } else if let Some(removed) = snapshots.remove(root_uri) {
                    affected.extend(removed.keys().cloned());
                }
            }

            for open_document_uri in &open_documents {
                let roots_for_document =
                    document_roots.entry(open_document_uri.clone()).or_default();
                for root_uri in &roots_to_reconcile {
                    let includes_document = snapshots
                        .get(root_uri)
                        .is_some_and(|snapshot| snapshot_contains_uri(snapshot, open_document_uri));
                    if includes_document {
                        roots_for_document.insert(root_uri.clone());
                    } else {
                        roots_for_document.remove(root_uri);
                    }
                }
            }

            if analysis.linked_roots.is_empty() {
                document_roots.remove(&trigger_uri);
            } else {
                document_roots.insert(trigger_uri.clone(), analysis.linked_roots);
            }

            document_roots.retain(|document_uri, roots| {
                open_documents.contains(document_uri) && !roots.is_empty()
            });

            let active_roots = document_roots
                .values()
                .flat_map(|roots| roots.iter().cloned())
                .collect::<HashSet<_>>();

            let stale_roots = snapshots
                .keys()
                .filter(|root_uri| !active_roots.contains(*root_uri))
                .cloned()
                .collect::<Vec<_>>();

            for root_uri in stale_roots {
                if let Some(removed) = snapshots.remove(&root_uri) {
                    affected.extend(removed.keys().cloned());
                }
            }

            affected.extend(open_documents.iter().cloned());
            affected.insert(trigger_uri.clone());

            affected
                .into_iter()
                .map(|uri| {
                    let linked = document_roots
                        .get(&uri)
                        .is_some_and(|roots| !roots.is_empty());
                    let diagnostics = if open_documents.contains(&uri) && !linked {
                        vec![unlinked_file_diagnostic()]
                    } else {
                        aggregate_uri_diagnostics(&snapshots, &uri)
                    };
                    (uri, diagnostics)
                })
                .collect::<Vec<_>>()
        };

        for (uri, diagnostics) in updates {
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }

    async fn clear_closed_document(&self, document_uri: &Url) {
        let open_documents = {
            let documents = self.documents.lock().await;
            documents.keys().cloned().collect::<HashSet<_>>()
        };

        let updates = {
            let mut snapshots = self.bundle_diagnostics.lock().await;
            let mut document_roots = self.document_roots.lock().await;
            document_roots.remove(document_uri);
            document_roots.retain(|uri, roots| open_documents.contains(uri) && !roots.is_empty());

            let mut affected = HashSet::from([document_uri.clone()]);

            let active_roots = document_roots
                .values()
                .flat_map(|roots| roots.iter().cloned())
                .collect::<HashSet<_>>();

            let stale_roots = snapshots
                .keys()
                .filter(|root_uri| !active_roots.contains(*root_uri))
                .cloned()
                .collect::<Vec<_>>();

            for root_uri in stale_roots {
                if let Some(removed) = snapshots.remove(&root_uri) {
                    affected.extend(removed.keys().cloned());
                }
            }

            affected.extend(open_documents.iter().cloned());

            affected
                .into_iter()
                .map(|uri| {
                    let linked = document_roots
                        .get(&uri)
                        .is_some_and(|roots| !roots.is_empty());
                    let diagnostics = if open_documents.contains(&uri) && !linked {
                        vec![unlinked_file_diagnostic()]
                    } else {
                        aggregate_uri_diagnostics(&snapshots, &uri)
                    };
                    (uri, diagnostics)
                })
                .collect::<Vec<_>>()
        };

        for (uri, diagnostics) in updates {
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }
}

#[derive(Clone, Copy)]
enum RootDiscoveryMode {
    Full,
    LinkedOnly,
}

struct DocumentAnalysis {
    root_snapshots: HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>>,
    linked_roots: HashSet<Url>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let roots = workspace_roots_from_initialize(&params);
        {
            let mut workspace_roots = self.workspace_roots.lock().await;
            *workspace_roots = roots;
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..TextDocumentSyncOptions::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "ruddy language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        {
            let mut documents = self.documents.lock().await;
            documents.insert(uri.clone(), text.clone());
        }
        self.analyze_open_document(uri, RootDiscoveryMode::Full)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let text = change.text;
        {
            let mut documents = self.documents.lock().await;
            documents.insert(uri.clone(), text.clone());
        }
        self.analyze_open_document(uri, RootDiscoveryMode::LinkedOnly)
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(text) = params.text {
            {
                let mut documents = self.documents.lock().await;
                documents.insert(uri.clone(), text.clone());
            }
            self.analyze_open_document(uri, RootDiscoveryMode::Full)
                .await;
            return;
        }

        self.analyze_open_document(uri, RootDiscoveryMode::Full)
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut documents = self.documents.lock().await;
            documents.remove(&uri);
        }
        self.clear_closed_document(&uri).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let linked = {
            let document_roots = self.document_roots.lock().await;
            document_roots
                .get(&uri)
                .is_some_and(|roots| !roots.is_empty())
        };

        if !linked {
            return Ok(None);
        }

        let text = {
            let documents = self.documents.lock().await;
            documents.get(&uri).cloned()
        }
        .or_else(|| {
            uri.to_file_path()
                .ok()
                .and_then(|path| std::fs::read_to_string(path).ok())
        });

        let Some(text) = text else {
            return Ok(None);
        };

        Ok(collect_type_hover(&uri, &text, position))
    }
}

fn workspace_roots_from_initialize(params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(workspace_folders) = params.workspace_folders.as_ref() {
        for folder in workspace_folders {
            if let Ok(path) = folder.uri.to_file_path() {
                roots.push(path);
            }
        }
    }

    if roots.is_empty()
        && let Some(root_uri) = params.root_uri.as_ref()
        && let Ok(path) = root_uri.to_file_path()
    {
        roots.push(path);
    }

    roots
}

fn collect_bundle_snapshots_for_document(
    target_uri: &Url,
    documents: &HashMap<Url, String>,
    workspace_roots: &[PathBuf],
    known_roots: &HashSet<Url>,
    discovery_mode: RootDiscoveryMode,
) -> DocumentAnalysis {
    let root_candidates = match discovery_mode {
        RootDiscoveryMode::Full => {
            collect_full_root_candidates(target_uri, documents, workspace_roots)
        }
        RootDiscoveryMode::LinkedOnly => {
            collect_linked_root_candidates(target_uri, documents, known_roots)
        }
    };

    let mut root_snapshots = HashMap::new();
    let mut linked_roots = HashSet::new();
    for (root_uri, root_text) in root_candidates {
        let snapshot = collect_bundle_diagnostics(&root_uri, &root_text);
        if snapshot_contains_uri(&snapshot, target_uri) {
            linked_roots.insert(root_uri.clone());
        }
        root_snapshots.insert(root_uri, snapshot);
    }

    DocumentAnalysis {
        root_snapshots,
        linked_roots,
    }
}

fn collect_full_root_candidates(
    target_uri: &Url,
    documents: &HashMap<Url, String>,
    workspace_roots: &[PathBuf],
) -> Vec<(Url, String)> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let open_documents_by_key = documents
        .iter()
        .map(|(uri, text)| (normalized_uri_key(uri), (uri.clone(), text.clone())))
        .collect::<HashMap<_, _>>();

    for (uri, text) in documents {
        if !source_declares_bundle(uri, text) {
            continue;
        }

        let key = normalized_uri_key(uri);
        if seen.insert(key) {
            candidates.push((uri.clone(), text.clone()));
        }
    }

    for root_uri in discover_workspace_hc_uris(workspace_roots, target_uri) {
        let key = normalized_uri_key(&root_uri);
        if !seen.insert(key.clone()) {
            continue;
        }

        if let Some((open_uri, open_text)) = open_documents_by_key.get(&key) {
            if source_declares_bundle(open_uri, open_text) {
                candidates.push((open_uri.clone(), open_text.clone()));
            }
            continue;
        }

        let Some(root_text) = read_uri_text(&root_uri) else {
            continue;
        };

        if source_declares_bundle(&root_uri, &root_text) {
            candidates.push((root_uri, root_text));
        }
    }

    candidates
}

fn collect_linked_root_candidates(
    target_uri: &Url,
    documents: &HashMap<Url, String>,
    known_roots: &HashSet<Url>,
) -> Vec<(Url, String)> {
    let mut candidate_uris = known_roots.clone();
    if let Some(target_text) = documents.get(target_uri)
        && source_declares_bundle(target_uri, target_text)
    {
        candidate_uris.insert(target_uri.clone());
    }

    candidate_uris
        .into_iter()
        .filter_map(|root_uri| {
            let root_text = documents
                .get(&root_uri)
                .cloned()
                .or_else(|| read_uri_text(&root_uri))?;

            if source_declares_bundle(&root_uri, &root_text) {
                Some((root_uri, root_text))
            } else {
                None
            }
        })
        .collect()
}

fn discover_workspace_hc_uris(workspace_roots: &[PathBuf], target_uri: &Url) -> Vec<Url> {
    let mut scan_roots = workspace_roots.to_vec();
    if scan_roots.is_empty()
        && let Ok(target_path) = target_uri.to_file_path()
        && let Some(parent) = target_path.parent()
    {
        scan_roots.push(parent.to_path_buf());
    }

    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for root in scan_roots {
        collect_hc_files(&root, &mut files, &mut seen);
    }

    files
        .into_iter()
        .filter_map(|path| Url::from_file_path(path).ok())
        .collect()
}

fn collect_hc_files(root: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();

        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "target" {
                continue;
            }
            collect_hc_files(&path, out, seen);
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let is_hc = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("hc"));

        if !is_hc {
            continue;
        }

        let key = normalized_path_key(&path);
        if seen.insert(key) {
            out.push(path);
        }
    }
}

fn read_uri_text(uri: &Url) -> Option<String> {
    let path = uri.to_file_path().ok()?;
    std::fs::read_to_string(path).ok()
}

fn source_declares_bundle(uri: &Url, text: &str) -> bool {
    let db = Eng::default();
    let source = Source::new(&db, uri_to_source_name(uri), text.to_owned());
    parse_text(&db, source).ast.bundle_name.is_some()
}

fn snapshot_contains_uri(snapshot: &HashMap<Url, Vec<LspDiagnostic>>, target_uri: &Url) -> bool {
    let target_key = normalized_uri_key(target_uri);
    snapshot
        .keys()
        .any(|uri| normalized_uri_key(uri) == target_key)
}

fn normalized_uri_key(uri: &Url) -> String {
    uri.to_file_path()
        .ok()
        .map(|path| normalized_path_key(&path))
        .unwrap_or_else(|| uri.to_string())
}

fn normalized_path_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn unlinked_file_diagnostic() -> LspDiagnostic {
    LspDiagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        severity: Some(LspDiagnosticSeverity::WARNING),
        source: Some("ruddy".to_owned()),
        message: "this file is not part of any bundle; language features are disabled".to_owned(),
        ..LspDiagnostic::default()
    }
}

fn collect_bundle_diagnostics(root_uri: &Url, root_text: &str) -> HashMap<Url, Vec<LspDiagnostic>> {
    let source_name = uri_to_source_name(root_uri);
    let db = Eng::default();
    let mut diagnostics_by_uri: HashMap<Url, Vec<LspDiagnostic>> = HashMap::new();
    diagnostics_by_uri.entry(root_uri.clone()).or_default();

    let check_source = Source::new(&db, source_name.clone(), root_text.to_owned());
    let checked = check_text_fs(&db, check_source);
    for module in &checked.source.modules {
        if let Some(module_uri) = source_name_to_uri(&module.source_name) {
            diagnostics_by_uri.entry(module_uri).or_default();
        }
    }

    let mut seen = HashSet::new();

    let mut record_diagnostic = |diagnostic: RuddyDiagnostic| {
        let (target_uri, source_text) = diagnostic_target(root_uri, root_text, &db, &diagnostic);
        let lsp_diagnostic = to_lsp_diagnostic(&source_text, diagnostic);
        let signature = lsp_diagnostic_signature(&target_uri, &lsp_diagnostic);
        if seen.insert(signature) {
            diagnostics_by_uri
                .entry(target_uri)
                .or_default()
                .push(lsp_diagnostic);
        }
    };

    let lower_source = Source::new(&db, source_name.clone(), root_text.to_owned());
    for diagnostic in lower_diagnostics_fs(&db, lower_source) {
        record_diagnostic(diagnostic);
    }

    for diagnostic in checked.diagnostics {
        record_diagnostic(diagnostic);
    }

    diagnostics_by_uri
}

fn aggregate_uri_diagnostics(
    snapshots: &HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>>,
    target_uri: &Url,
) -> Vec<LspDiagnostic> {
    let target_key = normalized_uri_key(target_uri);
    let mut aggregated = Vec::new();
    let mut seen = HashSet::new();
    for snapshot in snapshots.values() {
        for (uri, diagnostics) in snapshot {
            if normalized_uri_key(uri) != target_key {
                continue;
            }

            for diagnostic in diagnostics {
                let key = (
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                    diagnostic.range.end.line,
                    diagnostic.range.end.character,
                    diagnostic.severity.map(diagnostic_severity_key),
                    diagnostic.source.clone().unwrap_or_default(),
                    diagnostic.message.clone(),
                );

                if seen.insert(key) {
                    aggregated.push(diagnostic.clone());
                }
            }
        }
    }
    aggregated
}

fn diagnostic_severity_key(severity: LspDiagnosticSeverity) -> u8 {
    match severity {
        LspDiagnosticSeverity::ERROR => 1,
        LspDiagnosticSeverity::WARNING => 2,
        LspDiagnosticSeverity::INFORMATION => 3,
        LspDiagnosticSeverity::HINT => 4,
        _ => 0,
    }
}

fn lsp_diagnostic_signature(
    uri: &Url,
    diagnostic: &LspDiagnostic,
) -> (String, u32, u32, u32, u32, u8, String, String) {
    (
        uri.to_string(),
        diagnostic.range.start.line,
        diagnostic.range.start.character,
        diagnostic.range.end.line,
        diagnostic.range.end.character,
        diagnostic
            .severity
            .map(diagnostic_severity_key)
            .unwrap_or_default(),
        diagnostic.source.clone().unwrap_or_default(),
        diagnostic.message.clone(),
    )
}

fn diagnostic_target(
    root_uri: &Url,
    root_text: &str,
    db: &Eng,
    diagnostic: &RuddyDiagnostic,
) -> (Url, String) {
    match diagnostic.range.source() {
        Some(source) => {
            let source_name = source.name(db);
            let uri = source_name_to_uri(source_name).unwrap_or_else(|| root_uri.clone());
            let text = source.contents(db).clone();
            (uri, text)
        }
        None => (root_uri.clone(), root_text.to_owned()),
    }
}

fn to_lsp_diagnostic(text: &str, diagnostic: RuddyDiagnostic) -> LspDiagnostic {
    LspDiagnostic {
        range: to_lsp_range(text, diagnostic.range),
        severity: Some(to_lsp_severity(diagnostic.severity)),
        source: Some("ruddy".to_owned()),
        message: diagnostic.message,
        ..LspDiagnostic::default()
    }
}

fn to_lsp_range(text: &str, range: TextRange) -> Range {
    match range {
        TextRange::Located { start, length, .. } => {
            let start_offset = start.as_usize();
            let end_offset = start_offset.saturating_add(length.as_usize());
            Range::new(
                byte_offset_to_position(text, start_offset),
                byte_offset_to_position(text, end_offset),
            )
        }
        TextRange::Generated => Range::new(Position::new(0, 0), Position::new(0, 0)),
    }
}

fn to_lsp_severity(severity: RuddyDiagnosticSeverity) -> LspDiagnosticSeverity {
    match severity {
        RuddyDiagnosticSeverity::Error => LspDiagnosticSeverity::ERROR,
        RuddyDiagnosticSeverity::Warning => LspDiagnosticSeverity::WARNING,
        RuddyDiagnosticSeverity::Note => LspDiagnosticSeverity::INFORMATION,
    }
}

fn collect_type_hover(uri: &Url, text: &str, position: Position) -> Option<Hover> {
    let source_name = uri_to_source_name(uri);
    let db = Eng::default();
    let source = Source::new(&db, source_name.clone(), text.to_owned());
    let checked = check_text_fs(&db, source);
    let byte_offset = position_to_byte_offset(text, position);
    let candidate = find_type_candidate(&db, &checked.source, &source_name, byte_offset)?;
    let rendered_type = format_type_for_hover(&checked.type_store, candidate.ty);

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```text\n{rendered_type}\n```"),
        }),
        range: Some(to_lsp_range(text, candidate.range)),
    })
}

#[derive(Clone, Copy)]
struct TypeInfoCandidate {
    range: TextRange,
    ty: TypeId,
}

fn find_type_candidate(
    db: &Eng,
    source: &tir::Source,
    target_source_name: &str,
    byte_offset: usize,
) -> Option<TypeInfoCandidate> {
    let mut best = None;

    for module in &source.modules {
        for statement in &module.statements {
            visit_statement_for_type(db, statement, target_source_name, byte_offset, &mut best);
        }
    }

    best
}

fn visit_statement_for_type(
    db: &Eng,
    statement: &tir::Statement,
    target_source_name: &str,
    byte_offset: usize,
    best: &mut Option<TypeInfoCandidate>,
) {
    if let tir::Statement::Let { kind, .. } = statement
        && let tir::LetStatementKind::PatternBinding { pattern, value } = kind
    {
        visit_pattern_for_type(db, pattern, target_source_name, byte_offset, best);
        visit_expr_for_type(db, value, target_source_name, byte_offset, best);
    }
}

fn visit_pattern_for_type(
    db: &Eng,
    pattern: &tir::Pattern,
    target_source_name: &str,
    byte_offset: usize,
    best: &mut Option<TypeInfoCandidate>,
) {
    consider_type_candidate(
        db,
        pattern.range,
        pattern.ty,
        target_source_name,
        byte_offset,
        best,
    );

    match &pattern.kind {
        tir::PatternKind::Constructor { argument, .. } => {
            visit_pattern_for_type(db, argument, target_source_name, byte_offset, best)
        }
        tir::PatternKind::Annotated { pattern, .. } => {
            visit_pattern_for_type(db, pattern, target_source_name, byte_offset, best)
        }
        tir::PatternKind::Tuple { elements } => {
            for element in elements {
                visit_pattern_for_type(db, element, target_source_name, byte_offset, best);
            }
        }
        tir::PatternKind::Array { elements } => {
            for element in elements {
                if let tir::ArrayPatternElement::Item(item) = element {
                    visit_pattern_for_type(db, item, target_source_name, byte_offset, best);
                }
            }
        }
        tir::PatternKind::Record { fields, .. } => {
            for field in fields {
                if let Some(value) = &field.value {
                    visit_pattern_for_type(db, value, target_source_name, byte_offset, best);
                }
            }
        }
        tir::PatternKind::ConstructorName { .. }
        | tir::PatternKind::Binding { .. }
        | tir::PatternKind::Hole
        | tir::PatternKind::Literal(_)
        | tir::PatternKind::Error(_) => {}
    }
}

fn visit_expr_for_type(
    db: &Eng,
    expr: &tir::Expr,
    target_source_name: &str,
    byte_offset: usize,
    best: &mut Option<TypeInfoCandidate>,
) {
    consider_type_candidate(
        db,
        expr.range,
        expr.ty,
        target_source_name,
        byte_offset,
        best,
    );

    match &expr.kind {
        tir::ExprKind::Let {
            pattern,
            value,
            body,
        } => {
            visit_pattern_for_type(db, pattern, target_source_name, byte_offset, best);
            visit_expr_for_type(db, value, target_source_name, byte_offset, best);
            visit_expr_for_type(db, body, target_source_name, byte_offset, best);
        }
        tir::ExprKind::Function { params, body } => {
            for param in params {
                visit_pattern_for_type(db, param, target_source_name, byte_offset, best);
            }
            visit_expr_for_type(db, body, target_source_name, byte_offset, best);
        }
        tir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            visit_expr_for_type(db, condition, target_source_name, byte_offset, best);
            visit_expr_for_type(db, then_branch, target_source_name, byte_offset, best);
            visit_expr_for_type(db, else_branch, target_source_name, byte_offset, best);
        }
        tir::ExprKind::Match { scrutinee, arms } => {
            visit_expr_for_type(db, scrutinee, target_source_name, byte_offset, best);
            for arm in arms {
                visit_pattern_for_type(db, &arm.pattern, target_source_name, byte_offset, best);
                visit_expr_for_type(db, &arm.body, target_source_name, byte_offset, best);
            }
        }
        tir::ExprKind::Apply { callee, argument } => {
            visit_expr_for_type(db, callee, target_source_name, byte_offset, best);
            visit_expr_for_type(db, argument, target_source_name, byte_offset, best);
        }
        tir::ExprKind::FieldAccess { expr, .. } => {
            visit_expr_for_type(db, expr, target_source_name, byte_offset, best)
        }
        tir::ExprKind::Tuple { elements } => {
            for element in elements {
                visit_expr_for_type(db, element, target_source_name, byte_offset, best);
            }
        }
        tir::ExprKind::Array { elements } => {
            for element in elements {
                match element {
                    tir::ArrayElement::Item(item) => {
                        visit_expr_for_type(db, item, target_source_name, byte_offset, best)
                    }
                    tir::ArrayElement::Spread { expr, .. } => {
                        visit_expr_for_type(db, expr, target_source_name, byte_offset, best)
                    }
                }
            }
        }
        tir::ExprKind::Record { fields } => {
            for field in fields {
                visit_expr_for_type(db, &field.value, target_source_name, byte_offset, best);
            }
        }
        tir::ExprKind::Name(_)
        | tir::ExprKind::Literal(_)
        | tir::ExprKind::Unit
        | tir::ExprKind::InlineWasm { .. }
        | tir::ExprKind::Error(_) => {}
    }
}

fn consider_type_candidate(
    db: &Eng,
    range: TextRange,
    ty: TypeId,
    target_source_name: &str,
    byte_offset: usize,
    best: &mut Option<TypeInfoCandidate>,
) {
    let Some((start, end)) = range_bounds_for_source(db, range, target_source_name) else {
        return;
    };

    if byte_offset < start || byte_offset > end {
        return;
    }

    let span = end.saturating_sub(start);
    let is_better = match best {
        Some(current) => {
            let current_span = range_bounds_for_source(db, current.range, target_source_name)
                .map(|(current_start, current_end)| current_end.saturating_sub(current_start))
                .unwrap_or(usize::MAX);
            span <= current_span
        }
        None => true,
    };

    if is_better {
        *best = Some(TypeInfoCandidate { range, ty });
    }
}

fn range_bounds_for_source(
    db: &Eng,
    range: TextRange,
    target_source_name: &str,
) -> Option<(usize, usize)> {
    let TextRange::Located {
        source,
        start,
        length,
    } = range
    else {
        return None;
    };

    if source.name(db) != target_source_name {
        return None;
    }

    let start = start.as_usize();
    let end = start.saturating_add(length.as_usize());
    Some((start, end))
}

fn position_to_byte_offset(text: &str, position: Position) -> usize {
    let mut line: u32 = 0;
    let mut character: u32 = 0;

    for (byte_offset, ch) in text.char_indices() {
        if line == position.line {
            if ch == '\n' {
                return byte_offset;
            }

            if character >= position.character {
                return byte_offset;
            }

            let width = ch.len_utf16() as u32;
            if character.saturating_add(width) > position.character {
                return byte_offset;
            }

            character = character.saturating_add(width);
            continue;
        }

        if ch == '\n' {
            line = line.saturating_add(1);
            character = 0;
            if line > position.line {
                return byte_offset;
            }
        }
    }

    text.len()
}

fn format_type_for_hover(store: &TypeStore, ty: TypeId) -> String {
    let mut formatter = TypeFormatter::new(store);
    formatter.format_type(ty)
}

struct TypeFormatter<'store> {
    store: &'store TypeStore,
    binder_names: HashMap<TypeBinderId, String>,
    meta_names: HashMap<MetaTypeVariableId, String>,
}

impl<'store> TypeFormatter<'store> {
    fn new(store: &'store TypeStore) -> Self {
        Self {
            store,
            binder_names: HashMap::new(),
            meta_names: HashMap::new(),
        }
    }

    fn format_type(&mut self, ty: TypeId) -> String {
        self.meta_names.clear();

        let implicit_meta_binders = self.collect_implicit_meta_binders(ty);
        if implicit_meta_binders.is_empty() {
            return self.format_type_prec(ty, 0);
        }

        let mut used_names = self.collect_bound_names(ty);
        let mut next_name_index = 0usize;
        let mut binder_texts = Vec::with_capacity(implicit_meta_binders.len());

        for (meta_var, kind) in &implicit_meta_binders {
            let name = self.next_fresh_meta_name(&mut used_names, &mut next_name_index);
            binder_texts.push(self.format_binder_text(&name, *kind));
            self.meta_names.insert(*meta_var, name);
        }

        let body_text = self.format_type_prec(ty, 0);

        for (meta_var, _) in implicit_meta_binders {
            self.meta_names.remove(&meta_var);
        }

        format!("for {} in {body_text}", binder_texts.join(" "))
    }

    fn collect_implicit_meta_binders(&self, ty: TypeId) -> Vec<(MetaTypeVariableId, KindId)> {
        let mut binders = Vec::new();
        let mut seen_meta_vars = HashSet::new();
        let mut visited_types = HashSet::new();
        self.collect_implicit_meta_binders_inner(
            ty,
            &mut visited_types,
            &mut seen_meta_vars,
            &mut binders,
        );
        binders
    }

    fn collect_implicit_meta_binders_inner(
        &self,
        ty: TypeId,
        visited_types: &mut HashSet<TypeId>,
        seen_meta_vars: &mut HashSet<MetaTypeVariableId>,
        binders: &mut Vec<(MetaTypeVariableId, KindId)>,
    ) {
        if !visited_types.insert(ty) {
            return;
        }

        let node = self.store.get_type(ty);
        match &node.kind {
            TypeKind::MetaTypeVariable(var) => {
                if seen_meta_vars.insert(*var) {
                    binders.push((*var, node.kind_id));
                }
            }
            TypeKind::Application(func, argument) => {
                self.collect_implicit_meta_binders_inner(
                    *func,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
                self.collect_implicit_meta_binders_inner(
                    *argument,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
            }
            TypeKind::Lambda(_, body) => {
                self.collect_implicit_meta_binders_inner(
                    *body,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
            }
            TypeKind::Record(row) => {
                self.collect_implicit_meta_binders_inner(
                    *row,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
            }
            TypeKind::RowExtend { field, tail, .. } => {
                self.collect_implicit_meta_binders_inner(
                    *field,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
                self.collect_implicit_meta_binders_inner(
                    *tail,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
            }
            TypeKind::Forall(_, predicates, body) => {
                for predicate in predicates {
                    for argument in &predicate.arguments {
                        self.collect_implicit_meta_binders_inner(
                            *argument,
                            visited_types,
                            seen_meta_vars,
                            binders,
                        );
                    }
                }
                self.collect_implicit_meta_binders_inner(
                    *body,
                    visited_types,
                    seen_meta_vars,
                    binders,
                );
            }
            TypeKind::Constructor(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => {}
        }
    }

    fn collect_bound_names(&self, ty: TypeId) -> HashSet<String> {
        let mut names = HashSet::new();
        let mut visited_types = HashSet::new();
        self.collect_bound_names_inner(ty, &mut visited_types, &mut names);
        names
    }

    fn collect_bound_names_inner(
        &self,
        ty: TypeId,
        visited_types: &mut HashSet<TypeId>,
        names: &mut HashSet<String>,
    ) {
        if !visited_types.insert(ty) {
            return;
        }

        match &self.store.get_type(ty).kind {
            TypeKind::Forall(binders, predicates, body) => {
                for binder in binders {
                    if !binder.name.is_empty() {
                        names.insert(binder.name.clone());
                    }
                }
                for predicate in predicates {
                    for argument in &predicate.arguments {
                        self.collect_bound_names_inner(*argument, visited_types, names);
                    }
                }
                self.collect_bound_names_inner(*body, visited_types, names);
            }
            TypeKind::Lambda(binder, body) => {
                if !binder.name.is_empty() {
                    names.insert(binder.name.clone());
                }
                self.collect_bound_names_inner(*body, visited_types, names);
            }
            TypeKind::Application(func, argument) => {
                self.collect_bound_names_inner(*func, visited_types, names);
                self.collect_bound_names_inner(*argument, visited_types, names);
            }
            TypeKind::Record(row) => {
                self.collect_bound_names_inner(*row, visited_types, names);
            }
            TypeKind::RowExtend { field, tail, .. } => {
                self.collect_bound_names_inner(*field, visited_types, names);
                self.collect_bound_names_inner(*tail, visited_types, names);
            }
            TypeKind::Constructor(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::MetaTypeVariable(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => {}
        }
    }

    fn next_fresh_meta_name(
        &self,
        used_names: &mut HashSet<String>,
        next_name_index: &mut usize,
    ) -> String {
        loop {
            let candidate = self.meta_name_from_index(*next_name_index);
            *next_name_index += 1;
            if used_names.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    fn meta_name_from_index(&self, mut index: usize) -> String {
        let mut chars = Vec::new();
        loop {
            let remainder = index % 26;
            chars.push((b'a' + remainder as u8) as char);
            if index < 26 {
                break;
            }
            index = (index / 26) - 1;
        }

        chars.iter().rev().collect()
    }

    fn format_type_prec(&mut self, ty: TypeId, min_precedence: u8) -> String {
        if let TypeKind::Forall(binders, predicates, body) = &self.store.get_type(ty).kind {
            let text = self.format_forall_type(binders, predicates, *body);
            return if min_precedence > 0 {
                format!("({text})")
            } else {
                text
            };
        }

        if let TypeKind::Lambda(_, _) = &self.store.get_type(ty).kind {
            let text = self.format_lambda_type(ty);
            return if min_precedence > 0 {
                format!("({text})")
            } else {
                text
            };
        }

        let (head, args) = self.decompose_application(ty);
        if let TypeKind::Constructor(TypeConstructor::Arrow) = &self.store.get_type(head).kind
            && args.len() == 2
        {
            let from = self.format_type_prec(args[0], 1);
            let to = self.format_type_prec(args[1], 0);
            let text = format!("{from} -> {to}");
            return if min_precedence > 0 {
                format!("({text})")
            } else {
                text
            };
        }

        if let TypeKind::Constructor(TypeConstructor::Tuple(arity)) =
            &self.store.get_type(head).kind
        {
            if *arity == 0 && args.is_empty() {
                return "()".to_owned();
            }

            if args.len() == *arity {
                let elements = args
                    .iter()
                    .map(|argument| self.format_type_prec(*argument, 0))
                    .collect::<Vec<_>>();
                return format!("({})", elements.join(", "));
            }
        }

        if let TypeKind::Constructor(TypeConstructor::Array) = &self.store.get_type(head).kind
            && args.len() == 1
        {
            let element = self.format_type_prec(args[0], 0);
            return format!("[{element}]");
        }

        if !args.is_empty() {
            let mut text = self.format_type_prec(head, 2);
            for argument in args {
                text.push(' ');
                text.push_str(&self.format_type_prec(argument, 2));
            }

            return if min_precedence > 1 {
                format!("({text})")
            } else {
                text
            };
        }

        self.format_atomic_type(ty)
    }

    fn format_forall_type(
        &mut self,
        binders: &[ruddy::ty::TypeBinder],
        predicates: &[ruddy::ty::TraitPredicate],
        body: TypeId,
    ) -> String {
        let (binder_names, previous_bindings) = self.enter_binders(binders);

        let body_text = self.format_type_prec(body, 0);
        let constraint_suffix = if predicates.is_empty() {
            String::new()
        } else {
            let constraints = predicates
                .iter()
                .map(|predicate| self.format_predicate(predicate))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" where {constraints}")
        };

        self.leave_binders(previous_bindings);

        let quantifier_prefix = if binder_names.is_empty() {
            String::new()
        } else {
            format!("for {} in ", binder_names.join(" "))
        };

        format!("{quantifier_prefix}{body_text}{constraint_suffix}")
    }

    fn format_lambda_type(&mut self, ty: TypeId) -> String {
        let (binders, body) = self.collect_lambda_chain(ty);
        let (binder_names, previous_bindings) = self.enter_binders(&binders);
        let body_text = self.format_type_prec(body, 0);
        self.leave_binders(previous_bindings);
        format!("fn {} => {body_text}", binder_names.join(" "))
    }

    fn format_predicate(&mut self, predicate: &ruddy::ty::TraitPredicate) -> String {
        let trait_name = predicate
            .trait_ref
            .as_ref()
            .map(|path| path.text())
            .unwrap_or_else(|| "<trait>".to_owned());

        if predicate.arguments.is_empty() {
            return trait_name;
        }

        let arguments = predicate
            .arguments
            .iter()
            .map(|argument| self.format_type_prec(*argument, 2))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{trait_name} {arguments}")
    }

    fn format_atomic_type(&mut self, ty: TypeId) -> String {
        match &self.store.get_type(ty).kind {
            TypeKind::Constructor(constructor) => self.format_constructor_name(constructor),
            TypeKind::RigidTypeVariable(binder_id) => self
                .binder_names
                .get(binder_id)
                .cloned()
                .unwrap_or_else(|| format!("t{}", binder_id.0)),
            TypeKind::MetaTypeVariable(var) => self
                .meta_names
                .get(var)
                .cloned()
                .unwrap_or_else(|| format!("?{}", var.0)),
            TypeKind::Record(row) => self.format_record_type(*row),
            TypeKind::RowEmpty | TypeKind::RowExtend { .. } => {
                format!("row {{{}}}", self.format_row_contents(ty))
            }
            TypeKind::Forall(binders, predicates, body) => {
                self.format_forall_type(binders, predicates, *body)
            }
            TypeKind::Lambda(_, _) => self.format_lambda_type(ty),
            TypeKind::Error => "<error>".to_owned(),
            TypeKind::Application(_, _) => unreachable!("application should be decomposed first"),
        }
    }

    fn format_record_type(&mut self, row: TypeId) -> String {
        format!("{{{}}}", self.format_row_contents(row))
    }

    fn format_row_contents(&mut self, mut row: TypeId) -> String {
        let mut fields = Vec::new();

        loop {
            let kind = self.store.get_type(row).kind.clone();
            match kind {
                TypeKind::RowEmpty => break,
                TypeKind::RowExtend { label, field, tail } => {
                    fields.push(format!("{label}: {}", self.format_type_prec(field, 0)));
                    row = tail;
                }
                TypeKind::Error => {
                    if fields.is_empty() {
                        return "<error>".to_owned();
                    }
                    return format!("{} | <error>", fields.join(", "));
                }
                _ => {
                    let tail = self.format_type_prec(row, 0);
                    if fields.is_empty() {
                        return format!("| {tail}");
                    }
                    return format!("{} | {tail}", fields.join(", "));
                }
            }
        }

        fields.join(", ")
    }

    fn format_constructor_name(&self, constructor: &TypeConstructor) -> String {
        match constructor {
            TypeConstructor::Named(path) => path.text(),
            TypeConstructor::Arrow => "->".to_owned(),
            TypeConstructor::Tuple(0) => "()".to_owned(),
            TypeConstructor::Tuple(arity) => format!("Tuple{arity}"),
            TypeConstructor::Array => "Array".to_owned(),
            TypeConstructor::Bool => "Bool".to_owned(),
            TypeConstructor::Integer => "Integer".to_owned(),
            TypeConstructor::Natural => "Natural".to_owned(),
            TypeConstructor::Real => "Real".to_owned(),
            TypeConstructor::String => "String".to_owned(),
            TypeConstructor::Glyph => "Glyph".to_owned(),
        }
    }

    fn decompose_application(&self, ty: TypeId) -> (TypeId, Vec<TypeId>) {
        let mut head = ty;
        let mut arguments = Vec::new();

        while let TypeKind::Application(callee, argument) = &self.store.get_type(head).kind {
            arguments.push(*argument);
            head = *callee;
        }

        arguments.reverse();
        (head, arguments)
    }

    fn collect_lambda_chain(&self, ty: TypeId) -> (Vec<ruddy::ty::TypeBinder>, TypeId) {
        let mut binders = Vec::new();
        let mut body = ty;

        while let TypeKind::Lambda(binder, next_body) = &self.store.get_type(body).kind {
            binders.push(binder.clone());
            body = *next_body;
        }

        (binders, body)
    }

    fn enter_binders(
        &mut self,
        binders: &[ruddy::ty::TypeBinder],
    ) -> (Vec<String>, Vec<(TypeBinderId, Option<String>)>) {
        let mut previous_bindings = Vec::with_capacity(binders.len());
        let mut binder_names = Vec::with_capacity(binders.len());

        for (index, binder) in binders.iter().enumerate() {
            let name = if binder.name.is_empty() {
                format!("t{index}")
            } else {
                binder.name.clone()
            };

            previous_bindings.push((binder.id, self.binder_names.insert(binder.id, name.clone())));
            binder_names.push(self.format_binder_text(&name, binder.kind));
        }

        (binder_names, previous_bindings)
    }

    fn leave_binders(&mut self, previous_bindings: Vec<(TypeBinderId, Option<String>)>) {
        for (binder_id, previous) in previous_bindings {
            if let Some(previous_name) = previous {
                self.binder_names.insert(binder_id, previous_name);
            } else {
                self.binder_names.remove(&binder_id);
            }
        }
    }

    fn format_binder_text(&self, name: &str, kind: KindId) -> String {
        if matches!(self.store.get_kind(kind), Kind::Type) {
            name.to_owned()
        } else {
            format!("({name} :: {})", self.format_kind(kind))
        }
    }

    fn format_kind(&self, kind: KindId) -> String {
        self.format_kind_prec(kind, 0)
    }

    fn format_kind_prec(&self, kind: KindId, min_precedence: u8) -> String {
        match self.store.get_kind(kind) {
            Kind::Type => "Type".to_owned(),
            Kind::Row => "Row".to_owned(),
            Kind::Variable(var) => format!("?k{}", var.0),
            Kind::Error => "<kind error>".to_owned(),
            Kind::Arrow(from, to) => {
                let from = self.format_kind_prec(*from, 1);
                let to = self.format_kind_prec(*to, 0);
                let text = format!("{from} -> {to}");
                if min_precedence > 0 {
                    format!("({text})")
                } else {
                    text
                }
            }
        }
    }
}

fn byte_offset_to_position(text: &str, byte_offset: usize) -> Position {
    let mut clamped = byte_offset.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }

    let mut line: u32 = 0;
    let mut character: u32 = 0;
    for ch in text[..clamped].chars() {
        if ch == '\n' {
            line = line.saturating_add(1);
            character = 0;
            continue;
        }

        character = character.saturating_add(ch.len_utf16() as u32);
    }

    Position::new(line, character)
}

fn uri_to_source_name(uri: &Url) -> String {
    uri.to_file_path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| uri.to_string())
}

fn source_name_to_uri(source_name: &str) -> Option<Url> {
    Url::from_file_path(source_name)
        .ok()
        .or_else(|| Url::parse(source_name).ok())
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        path.push(format!(
            "ruddy_lsp_{label}_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&path).expect("failed to create test temp dir");
        path
    }

    fn file_uri(path: &Path) -> Url {
        Url::from_file_path(path).expect("valid file uri")
    }

    #[test]
    fn aggregate_uri_diagnostics_deduplicates_identical_entries() {
        let target_uri = Url::parse("file:///tmp/core/ops.hc").expect("valid target uri");
        let root_a = Url::parse("file:///tmp/core/bundle.hc").expect("valid root a uri");
        let root_b = Url::parse("file:///tmp/core/ops.hc").expect("valid root b uri");

        let diagnostic = LspDiagnostic {
            range: Range::new(Position::new(37, 25), Position::new(37, 32)),
            severity: Some(LspDiagnosticSeverity::ERROR),
            source: Some("ruddy".to_owned()),
            message: "failed to resolve type name".to_owned(),
            ..LspDiagnostic::default()
        };

        let mut snapshots: HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>> = HashMap::new();
        snapshots.insert(
            root_a,
            HashMap::from([(target_uri.clone(), vec![diagnostic.clone()])]),
        );
        snapshots.insert(
            root_b,
            HashMap::from([(target_uri.clone(), vec![diagnostic.clone()])]),
        );

        let aggregated = aggregate_uri_diagnostics(&snapshots, &target_uri);
        assert_eq!(aggregated.len(), 1);
    }

    #[test]
    fn collect_bundle_diagnostics_includes_type_checker_diagnostics() {
        let uri = Url::parse("file:///tmp/type_diag.hc").expect("valid uri");
        let text = ["bundle demo", "let value = if 1 then 2 else 3"].join("\n");

        let diagnostics = collect_bundle_diagnostics(&uri, &text);
        let root_diagnostics = diagnostics.get(&uri).cloned().unwrap_or_default();

        assert!(
            root_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("type mismatch")),
            "expected checker type mismatch diagnostic, got: {root_diagnostics:?}"
        );
    }

    #[test]
    fn collect_type_hover_reports_inferred_type() {
        let uri = Url::parse("file:///tmp/type_hover.hc").expect("valid uri");
        let text = ["bundle demo", "let value = true"].join("\n");

        let hover = collect_type_hover(&uri, &text, Position::new(1, 12))
            .expect("expected hover information at literal");

        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markdown hover content");
        };

        assert!(
            content.value.contains("Bool"),
            "expected Bool in hover content, got: {}",
            content.value
        );
    }

    #[test]
    fn collect_type_hover_formats_record_types() {
        let uri = Url::parse("file:///tmp/type_hover_record.hc").expect("valid uri");
        let text = ["bundle demo", "let get = fn r => r.x"].join("\n");

        let hover = collect_type_hover(&uri, &text, Position::new(1, 5))
            .expect("expected hover information");

        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markdown hover content");
        };

        assert!(
            content.value.contains("{x:"),
            "expected record type in hover content, got: {}",
            content.value
        );
    }

    #[test]
    fn format_type_for_hover_uses_type_lambda_surface_syntax() {
        let mut store = TypeStore::new();
        let kind = store.kind_type();
        let binder = ruddy::ty::TypeBinder {
            id: TypeBinderId(0),
            name: "a".to_owned(),
            kind,
            range: TextRange::Generated,
        };
        let rigid = store.mk_rigid(binder.id, kind);
        let lambda = store.mk_lambda(binder, rigid);

        assert_eq!(format_type_for_hover(&store, lambda), "fn a => a");
    }

    #[test]
    fn format_type_for_hover_quantifies_unresolved_meta_variables() {
        let mut store = TypeStore::new();
        let kind = store.kind_type();
        let meta = store.mk_meta(MetaTypeVariableId(1), kind);
        let arrow = store.mk_arrow(meta, meta);

        assert_eq!(format_type_for_hover(&store, arrow), "for a in a -> a");
    }

    #[test]
    fn format_type_for_hover_uses_kind_annotations_in_forall_binders() {
        let mut store = TypeStore::new();
        let type_kind = store.kind_type();
        let higher_kind = store.kind_arrow(type_kind, type_kind);

        let f_binder = ruddy::ty::TypeBinder {
            id: TypeBinderId(0),
            name: "f".to_owned(),
            kind: higher_kind,
            range: TextRange::Generated,
        };
        let a_binder = ruddy::ty::TypeBinder {
            id: TypeBinderId(1),
            name: "a".to_owned(),
            kind: type_kind,
            range: TextRange::Generated,
        };

        let f = store.mk_rigid(f_binder.id, higher_kind);
        let a = store.mk_rigid(a_binder.id, type_kind);
        let fa = store.mk_application(f, a, type_kind);
        let arrow = store.mk_arrow(fa, fa);
        let ty = store.mk_forall(vec![f_binder, a_binder], Vec::new(), arrow);

        assert_eq!(
            format_type_for_hover(&store, ty),
            "for (f :: Type -> Type) a in f a -> f a"
        );
    }

    #[test]
    fn full_bundle_discovery_links_imported_files() {
        let temp_dir = unique_temp_dir("linked_import");
        let root_path = temp_dir.join("root.hc");
        let imported_path = temp_dir.join("imported.hc");

        std::fs::write(
            &root_path,
            ["bundle demo", "module Imported in \"imported.hc\""].join("\n"),
        )
        .expect("failed to write bundle root");
        std::fs::write(&imported_path, "let value = 1").expect("failed to write imported file");

        let imported_uri = file_uri(&imported_path);
        let imported_text = std::fs::read_to_string(&imported_path).expect("read imported file");
        let documents = HashMap::from([(imported_uri.clone(), imported_text)]);
        let analysis = collect_bundle_snapshots_for_document(
            &imported_uri,
            &documents,
            std::slice::from_ref(&temp_dir),
            &HashSet::new(),
            RootDiscoveryMode::Full,
        );

        let root_uri = file_uri(&root_path);
        assert!(
            analysis
                .linked_roots
                .iter()
                .any(|uri| normalized_uri_key(uri) == normalized_uri_key(&root_uri)),
            "expected root.hc bundle snapshot to include imported file"
        );

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn unlinked_file_diagnostic_is_at_top_of_file() {
        let diagnostic = unlinked_file_diagnostic();
        assert_eq!(diagnostic.range.start, Position::new(0, 0));
        assert_eq!(diagnostic.range.end, Position::new(0, 0));
        assert_eq!(diagnostic.severity, Some(LspDiagnosticSeverity::WARNING));
        assert!(diagnostic.message.contains("not part of any bundle"));
    }
}
