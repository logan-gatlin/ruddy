use std::collections::{HashMap, HashSet};

use ruddy::{
    Diagnostic as RuddyDiagnostic, DiagnosticSeverity as RuddyDiagnosticSeverity, Eng, Source,
    TextRange, lower_diagnostics_fs,
};
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity as LspDiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
    Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    documents: Mutex<HashMap<Url, String>>,
    bundle_diagnostics: Mutex<HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
            bundle_diagnostics: Mutex::new(HashMap::new()),
        }
    }

    async fn publish_bundle_diagnostics(&self, root_uri: Url, root_text: String) {
        let diagnostics = collect_bundle_diagnostics(&root_uri, &root_text);

        let updates = {
            let mut snapshots = self.bundle_diagnostics.lock().await;
            let previous = snapshots
                .insert(root_uri.clone(), diagnostics)
                .unwrap_or_default();
            let mut affected: HashSet<Url> = previous.keys().cloned().collect();
            if let Some(current) = snapshots.get(&root_uri) {
                affected.extend(current.keys().cloned());
            }

            affected
                .into_iter()
                .map(|uri| {
                    let diagnostics = aggregate_uri_diagnostics(&snapshots, &uri);
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

    async fn clear_bundle_diagnostics(&self, root_uri: &Url) {
        let updates = {
            let mut snapshots = self.bundle_diagnostics.lock().await;
            let removed = snapshots.remove(root_uri).unwrap_or_default();
            let affected: HashSet<Url> = removed.keys().cloned().collect();

            affected
                .into_iter()
                .map(|uri| {
                    let diagnostics = aggregate_uri_diagnostics(&snapshots, &uri);
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

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
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
        self.publish_bundle_diagnostics(uri, text).await;
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
        self.publish_bundle_diagnostics(uri, text).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(text) = params.text {
            {
                let mut documents = self.documents.lock().await;
                documents.insert(uri.clone(), text.clone());
            }
            self.publish_bundle_diagnostics(uri, text).await;
            return;
        }

        let text = {
            let documents = self.documents.lock().await;
            documents.get(&uri).cloned()
        };

        if let Some(text) = text {
            self.publish_bundle_diagnostics(uri, text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut documents = self.documents.lock().await;
            documents.remove(&uri);
        }
        self.clear_bundle_diagnostics(&uri).await;
    }
}

fn collect_bundle_diagnostics(root_uri: &Url, root_text: &str) -> HashMap<Url, Vec<LspDiagnostic>> {
    let source_name = uri_to_source_name(root_uri);
    let db = Eng::default();
    let source = Source::new(&db, source_name, root_text.to_owned());
    let mut diagnostics_by_uri: HashMap<Url, Vec<LspDiagnostic>> = HashMap::new();

    for diagnostic in lower_diagnostics_fs(&db, source) {
        let (target_uri, source_text) = diagnostic_target(root_uri, root_text, &db, &diagnostic);
        diagnostics_by_uri
            .entry(target_uri)
            .or_default()
            .push(to_lsp_diagnostic(&source_text, diagnostic));
    }

    diagnostics_by_uri
}

fn aggregate_uri_diagnostics(
    snapshots: &HashMap<Url, HashMap<Url, Vec<LspDiagnostic>>>,
    target_uri: &Url,
) -> Vec<LspDiagnostic> {
    let mut aggregated = Vec::new();
    let mut seen = HashSet::new();
    for snapshot in snapshots.values() {
        if let Some(diagnostics) = snapshot.get(target_uri) {
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
}
