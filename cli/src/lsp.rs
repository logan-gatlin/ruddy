//! The language server: LSP transport and newest-revision scheduling. Input handling runs separately
//! from analysis; only a current revision may publish diagnostics.
mod diagnostics;
mod positions;
mod watcher;
use crate::workspace::{Workspace, file_identity};
use crossbeam_channel::Sender;
use diagnostics::{Diagnostics, tail_diagnostic};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
pub use positions::LineIndex;
use ruddy::{
    analysis::CompletionKind,
    cancellation::{Cancellation, checkpoint},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use watcher::Watcher;

// Full-project inference/lowering should wait through ordinary typing pauses.
// Foreground requests and active-file diagnostics do not wait for this timer.
const BACKGROUND_QUIET_INTERVAL: Duration = Duration::from_millis(300);
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct Schedule {
    revision: u64,
    active: Option<(Cancellation, Option<RequestId>)>,
    cancelled: HashSet<RequestId>,
    pending: HashSet<RequestId>,
}

struct Envelope {
    revision: u64,
    message: Message,
}
struct Document {
    uri: String,
    version: i64,
    text: String,
}

/// Serve an LSP connection (stdio in production, channel pairs in integration tests).
pub fn serve(connection: Connection) -> Result<()> {
    let params = connection.initialize(json!({
        "positionEncoding": "utf-16",
        "textDocumentSync": {"openClose":true,"change":2,"save":{"includeText":false}},
        "codeActionProvider":{"codeActionKinds":["quickfix"]},
        "hoverProvider":true,"definitionProvider":true,"documentFormattingProvider":true,
        "completionProvider":{"triggerCharacters":[".",":","!"]}
    }))?;
    let root = params
        .get("rootUri")
        .and_then(Value::as_str)
        .or_else(|| {
            params
                .pointer("/workspaceFolders/0/uri")
                .and_then(Value::as_str)
        })
        .and_then(file_path)
        .unwrap_or(std::env::current_dir()?);
    let shared = Arc::new(Mutex::new(Schedule::default()));
    let reader_schedule = shared.clone();
    let (send, receive) = crossbeam_channel::unbounded();
    let watcher = Watcher::new(send.clone(), shared.clone());
    let router = std::thread::spawn(move || {
        for message in connection.receiver {
            let mut schedule = reader_schedule.lock().unwrap();
            match &message {
                Message::Notification(notification) if changes_revision(&notification.method) => {
                    schedule.revision += 1;
                    if let Some((token, _)) = &schedule.active {
                        token.cancel();
                    }
                }
                Message::Notification(notification) if notification.method == "$/cancelRequest" => {
                    if let Some(id) = notification
                        .params
                        .get("id")
                        .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok())
                    {
                        if let Some((token, Some(active))) = &schedule.active
                            && active == &id
                        {
                            token.cancel();
                        }
                        if schedule.pending.contains(&id) {
                            schedule.cancelled.insert(id);
                        }
                    }
                }
                Message::Request(request) => {
                    schedule.pending.insert(request.id.clone());
                    if let Some((token, active)) = &schedule.active
                        && (active.is_none() || request.method == "shutdown")
                    {
                        token.cancel();
                    }
                }
                _ => {}
            }
            let exit = matches!(&message, Message::Notification(notification) if notification.method == "exit");
            if send
                .send(Envelope {
                    revision: schedule.revision,
                    message,
                })
                .is_err()
                || exit
            {
                break;
            }
        }
    });
    let mut workspace = Workspace::new(root);
    workspace.set_notification_driven(true);
    let mut worker = Worker {
        workspace,
        versioned_edits: params
            .pointer("/capabilities/workspace/workspaceEdit/documentChanges")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        documents: HashMap::new(),
        dirty: true,
        revision: 0,
        shutdown: false,
        shared,
        sender: connection.sender,
        project_error: None,
        background: false,
        background_due: None,
        background_errors: Vec::new(),
        published: None,
        published_closed: HashSet::new(),
        watched: watcher.inputs.clone(),
        diagnostics: Diagnostics::default(),
        published_payloads: HashMap::new(),
    };
    let mut pending = VecDeque::new();
    loop {
        if pending.is_empty() {
            let Ok(envelope) = receive.recv() else {
                break;
            };
            pending.push_back(envelope);
        }
        pending.extend(receive.try_iter());
        while let Some(envelope) = pending.pop_front() {
            worker.revision = envelope.revision;
            match envelope.message {
                Message::Notification(notification) => {
                    if notification.method == "exit" {
                        router.join().map_err(|_| "LSP input thread panicked")?;
                        return if worker.shutdown {
                            Ok(())
                        } else {
                            Err("exit received before shutdown".into())
                        };
                    }
                    worker.notification(notification)?;
                }
                Message::Request(request) => worker.request(request)?,
                Message::Response(_) => {}
            }
            pending.extend(receive.try_iter());
        }
        if !worker.shutdown && !worker.documents.is_empty() && worker.dirty {
            let token = worker.begin(None);
            if let Ok(result) = token.run(|| {
                worker.refresh();
                worker.publish()
            }) {
                result?;
            }
            worker.end();
        }
        if !worker.shutdown
            && !worker.dirty
            && !worker.background
            && !worker.documents.is_empty()
            && receive.is_empty()
        {
            if let Some(remaining) = worker
                .background_due
                .and_then(|due| due.checked_duration_since(Instant::now()))
            {
                match receive.recv_timeout(remaining) {
                    Ok(envelope) => {
                        pending.push_back(envelope);
                        continue;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
            let token = worker.begin(None);
            if let Ok(result) = token.run(|| {
                worker.background_errors = worker.workspace.check_background();
                worker.background = true;
                worker.background_due = None;
                worker.publish()
            }) {
                result?;
            }
            worker.end();
        }
        // Analysis can finish before a new request cancels diagnostic rendering.
        // Retry that publication independently of whether background checks have
        // completed, including after requests handled without semantic analysis.
        if !worker.shutdown
            && !worker.dirty
            && !worker.documents.is_empty()
            && worker.published != Some((worker.revision, worker.background))
        {
            let token = worker.begin(None);
            if let Ok(result) = token.run(|| worker.publish()) {
                result?;
            }
            worker.end();
        }
    }
    router.join().map_err(|_| "LSP input thread panicked")?;
    Ok(())
}

fn changes_revision(method: &str) -> bool {
    matches!(
        method,
        "textDocument/didOpen"
            | "textDocument/didChange"
            | "textDocument/didClose"
            | "textDocument/didSave"
            | "workspace/didChangeWatchedFiles"
            | "workspace/didChangeConfiguration"
    )
}

struct Worker {
    workspace: Workspace,
    versioned_edits: bool,
    documents: HashMap<PathBuf, Document>,
    dirty: bool,
    revision: u64,
    shutdown: bool,
    shared: Arc<Mutex<Schedule>>,
    sender: Sender<Message>,
    project_error: Option<String>,
    background: bool,
    background_due: Option<Instant>,
    background_errors: Vec<String>,
    published: Option<(u64, bool)>,
    published_closed: HashSet<String>,
    watched: Sender<watcher::Command>,
    diagnostics: Diagnostics,
    published_payloads: HashMap<String, Value>,
}

impl Worker {
    fn begin(&self, request: Option<RequestId>) -> Cancellation {
        let token = Cancellation::default();
        let mut schedule = self.shared.lock().unwrap();
        if schedule.revision != self.revision
            || (request.is_none() && !schedule.pending.is_empty())
            || request
                .as_ref()
                .is_some_and(|id| schedule.cancelled.contains(id))
        {
            token.cancel();
        }
        schedule.active = Some((token.clone(), request));
        token
    }
    fn end(&self) {
        self.shared.lock().unwrap().active = None;
    }

    fn notification(&mut self, notification: Notification) -> Result<()> {
        if self.shutdown {
            return Ok(());
        }
        let params = &notification.params;
        let path = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(file_path);
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                if let (Some(path), Some(uri), Some(text), Some(version)) = (
                    path,
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params.pointer("/textDocument/text").and_then(Value::as_str),
                    params
                        .pointer("/textDocument/version")
                        .and_then(Value::as_i64),
                ) {
                    self.workspace.focus(&path);
                    self.workspace.set_overlay(&path, Some(text.to_owned()));
                    self.documents.insert(
                        path,
                        Document {
                            uri: uri.to_owned(),
                            version,
                            text: text.to_owned(),
                        },
                    );
                    self.dirty = true;
                }
            }
            "textDocument/didChange" => {
                if let (Some(path), Some(version), Some(changes)) = (
                    path,
                    params
                        .pointer("/textDocument/version")
                        .and_then(Value::as_i64),
                    params.get("contentChanges").and_then(Value::as_array),
                ) && let Some(document) = self
                    .documents
                    .get_mut(&path)
                    .filter(|document| version > document.version)
                {
                    let mut text = document.text.clone();
                    let mut valid = true;
                    for change in changes {
                        let Some(replacement) = change.get("text").and_then(Value::as_str) else {
                            valid = false;
                            break;
                        };
                        if let Some(range) = change.get("range") {
                            let index = self.diagnostics.index(&path, &text);
                            let start = range
                                .get("start")
                                .and_then(|position| index.offset(position));
                            let end = range.get("end").and_then(|position| index.offset(position));
                            if let (Some(start), Some(end)) = (start, end) {
                                if start <= end {
                                    text.replace_range(start..end, replacement);
                                } else {
                                    valid = false;
                                    break;
                                }
                            } else {
                                valid = false;
                                break;
                            }
                        } else {
                            text = replacement.to_owned();
                        }
                    }
                    if valid {
                        document.text = text;
                        document.version = version;
                        self.workspace.focus(&path);
                        self.dirty |= self
                            .workspace
                            .set_overlay(&path, Some(document.text.clone()));
                    }
                }
            }
            "textDocument/didClose" => {
                if let Some(path) = path
                    && self.documents.get(&path).is_some_and(|document| {
                        params.pointer("/textDocument/uri").and_then(Value::as_str)
                            == Some(document.uri.as_str())
                    })
                {
                    if let Some(document) = self.documents.remove(&path) {
                        self.published_payloads.remove(&document.uri);
                        self.sender.send(Message::Notification(Notification::new(
                            "textDocument/publishDiagnostics".into(),
                            json!({"uri":document.uri,"diagnostics":[]}),
                        )))?;
                    }
                    self.dirty |= self.workspace.set_overlay(&path, None);
                }
            }
            "workspace/didChangeWatchedFiles" => {
                let paths: Vec<_> = params
                    .get("changes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|change| {
                        // Keep lexical loader paths for symlink deletion/retargeting;
                        // canonicalizing an event after a deletion loses its identity.
                        Url::parse(change.get("uri")?.as_str()?)
                            .ok()?
                            .to_file_path()
                            .ok()
                    })
                    .collect();
                if paths.is_empty() {
                    self.workspace.invalidate_all();
                } else {
                    self.workspace.invalidate_paths(paths);
                }
                self.dirty = true;
            }
            "workspace/didChangeConfiguration" => {
                self.workspace.invalidate_all();
                self.dirty = true;
            }
            "textDocument/didSave" => {
                if let Some(path) = path {
                    self.workspace.invalidate_paths([path]);
                } else {
                    self.workspace.invalidate_all();
                }
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        match self.workspace.refresh() {
            Ok(report) => {
                self.project_error = None;
                self.background_due =
                    (report.background > 0).then(|| Instant::now() + BACKGROUND_QUIET_INTERVAL);
            }
            Err(error) => {
                self.project_error = Some(error.messages().join("\n"));
                self.background_due = None;
            }
        }
        let _ = self
            .watched
            .send(watcher::Command::Inputs(self.workspace.observed_files()));
        let paths = self
            .workspace
            .projects
            .iter()
            .filter_map(|project| {
                project
                    .source_analysis()
                    .map(|analysis| (project, analysis))
            })
            .flat_map(|(project, analysis)| {
                analysis
                    .sources
                    .keys()
                    .map(|logical| project.source_directory.join(logical))
            })
            .chain(self.documents.keys().cloned())
            .collect();
        self.diagnostics.retain(&paths);
        self.background = false;
        self.background_errors.clear();
        self.dirty = false;
    }

    fn request(&mut self, request: Request) -> Result<()> {
        let _registration = RequestRegistration {
            schedule: self.shared.clone(),
            id: request.id.clone(),
        };
        if request.method == "shutdown" {
            self.shutdown = true;
            self.sender
                .send(Message::Response(Response::new_ok(request.id, Value::Null)))?;
            return Ok(());
        }
        if self.shutdown {
            self.sender.send(Message::Response(Response::new_err(
                request.id,
                -32600,
                "server has shut down".into(),
            )))?;
            return Ok(());
        }
        let supported = matches!(
            request.method.as_str(),
            "textDocument/hover"
                | "textDocument/completion"
                | "textDocument/definition"
                | "textDocument/formatting"
                | "textDocument/codeAction"
        );
        if !supported {
            self.sender.send(Message::Response(Response::new_err(
                request.id,
                -32601,
                "method not supported".into(),
            )))?;
            return Ok(());
        }
        let token = self.begin(Some(request.id.clone()));
        let result = token.run(|| {
            let path = request
                .params
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .and_then(file_path);
            if request.method == "textDocument/formatting"
                && let Some(path) = &path
                && let Some(document) = self.documents.get(path)
            {
                let index = self.diagnostics.index(path, &document.text);
                return Ok(format_edits(&index, &document.text));
            }
            let positional = matches!(
                request.method.as_str(),
                "textDocument/hover" | "textDocument/completion"
            );
            let at = path.as_ref().and_then(|path| {
                let text = self
                    .documents
                    .get(path)
                    .map(|document| document.text.as_str())
                    .or_else(|| {
                        self.workspace
                            .file(path)
                            .map(|(project, logical)| project.analysis().sources[logical].as_str())
                    })?;
                self.diagnostics
                    .index(path, text)
                    .offset(request.params.get("position")?)
            });
            if positional && let Some(path) = &path {
                self.workspace.focus_at(path, at);
            }
            self.refresh();
            if let Some(path) = &path {
                // Closed documents can have changed on disk since the last
                // publication. Resolve their position against the refreshed
                // snapshot before choosing which definition to infer.
                let at = if self.documents.contains_key(path) {
                    at
                } else {
                    self.workspace.file(path).and_then(|(project, logical)| {
                        self.diagnostics
                            .index(path, &project.analysis().sources[logical])
                            .offset(request.params.get("position")?)
                    })
                };
                let expanded = if positional {
                    match at {
                        Some(at) if request.method == "textDocument/completion" => {
                            self.workspace.request_completion(path, at)
                        }
                        Some(at) => self.workspace.request_at(path, at),
                        None => self.workspace.request_file(path),
                    }
                } else {
                    request.method == "textDocument/codeAction" && self.workspace.request_file(path)
                };
                if expanded {
                    self.published = None;
                }
            }
            self.answer(&request)
        });
        self.end();
        let response = match result {
            Ok(Ok(value)) => Response::new_ok(request.id, value),
            Ok(Err(message)) => Response::new_err(request.id, -32602, message),
            Err(_) => Response::new_err(request.id, -32800, "analysis was cancelled".into()),
        };
        {
            let schedule = self.shared.lock().unwrap();
            let response = if schedule.cancelled.contains(&response.id) {
                Response::new_err(response.id, -32800, "request was cancelled".into())
            } else if schedule.revision != self.revision {
                Response::new_err(
                    response.id,
                    -32801,
                    "document changed during analysis".into(),
                )
            } else {
                response
            };
            self.sender.send(Message::Response(response))?;
        }
        drop(_registration);
        // A request may have finished the revision's analysis before the idle
        // diagnostics task ran. Publish through the same revision barrier.
        let token = self.begin(None);
        let published = token.run(|| self.publish());
        self.end();
        if let Ok(result) = published {
            result?;
        }
        Ok(())
    }

    fn answer(&mut self, request: &Request) -> std::result::Result<Value, String> {
        let path = request
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(file_path)
            .ok_or("expected a file URI")?;
        if request.method == "textDocument/definition" {
            let Some(index) = self.workspace.file(&path).map(|(project, logical)| {
                self.diagnostics
                    .index(&path, &project.analysis().sources[logical])
            }) else {
                return Ok(Value::Null);
            };
            let at = request
                .params
                .get("position")
                .and_then(|position| index.offset(position))
                .ok_or("invalid document position")?;
            return Ok(self
                .workspace
                .definition(&path, at)
                .and_then(|(path, span)| {
                    let (target, logical) = self.workspace.file(&path)?;
                    let uri = self
                        .documents
                        .get(&file_identity(&path))
                        .map(|document| document.uri.clone())
                        .or_else(|| Url::from_file_path(&path).ok().map(|uri| uri.to_string()))?;
                    let index = self
                        .diagnostics
                        .index(&path, &target.analysis().sources[logical]);
                    Some(json!({"uri":uri,"range":index.range(span)}))
                })
                .unwrap_or(Value::Null));
        }
        let Some((project, logical)) = self.workspace.file(&path) else {
            return Ok(Value::Null);
        };
        let analysis = project.analysis();
        let source = &analysis.sources[logical];
        let index = self.diagnostics.index(&path, source);
        // Formatting is whole-document and positionless: one edit replacing
        // everything, or none when the buffer is already formatted. Syntax
        // errors are formatted around, as the command line does.
        if request.method == "textDocument/formatting" {
            return Ok(format_edits(&index, source));
        }
        if request.method == "textDocument/codeAction" {
            let Some(document) = self.documents.get(&path) else {
                return Ok(json!([]));
            };
            if request
                .params
                .pointer("/context/only")
                .and_then(Value::as_array)
                .is_some_and(|kinds| {
                    !kinds
                        .iter()
                        .any(|kind| matches!(kind.as_str(), Some("" | "quickfix")))
                })
            {
                return Ok(json!([]));
            }
            let start = request
                .params
                .pointer("/range/start")
                .and_then(|p| index.offset(p))
                .ok_or("invalid action range")?;
            let end = request
                .params
                .pointer("/range/end")
                .and_then(|p| index.offset(p))
                .ok_or("invalid action range")?;
            if start > end {
                return Err("invalid action range".into());
            }
            let actions: Vec<_> = project
                .analysis()
                .tail_recursion(logical)
                .into_iter()
                .filter(|fix| start <= fix.binding.end() && end >= fix.binding.start)
                .map(|fix| {
                    let diagnostic = tail_diagnostic(&index, &fix);
                    let edits = json!([{"range":index.range(fix.span),"newText":fix.replacement}]);
                    let edit = if self.versioned_edits {
                        json!({"documentChanges":[{
                            "textDocument":{"uri":document.uri,"version":document.version},"edits":edits
                        }]})
                    } else {
                        json!({"changes":{(document.uri.clone()):edits}})
                    };
                    json!({
                        "title":ruddy::ui::TAIL_RECURSION_ACTION,"kind":"quickfix",
                        "diagnostics":[diagnostic],"edit":edit
                    })
                })
                .collect();
            return Ok(json!(actions));
        }
        let at = request
            .params
            .get("position")
            .and_then(|position| index.offset(position))
            .ok_or("invalid document position")?;
        Ok(match request.method.as_str() {
            "textDocument/hover" => analysis.hover(logical, at).map(|hover| json!({"contents":{"kind":"markdown","value":format!("```ruddy\n{}\n```{}",hover.ty,hover.runtime_information.map(|note| format!("\n\n{note}")).unwrap_or_default())},"range":index.range(hover.span)})).unwrap_or(Value::Null),
            "textDocument/completion" => Value::Array(analysis.completions(logical, at).into_iter().map(|item| {
                // LSP has no general type kind. Class and TypeParameter both
                // misdescribe aliases and primitive types; name the category
                // in the detail instead of making clients display either one.
                let kind = match item.kind {
                    CompletionKind::Type => {
                        let detail = item.detail.map_or_else(|| format!("type {}", item.label), |ty| format!("type {} = {ty}", item.label));
                        return json!({"label":item.label,"detail":detail});
                    }
                    CompletionKind::Value => 6,
                    CompletionKind::Effect => 8,
                    CompletionKind::Module => 9,
                    CompletionKind::Field => 5,
                };
                json!({"label":item.label,"detail":item.detail,"kind":kind})
            }).collect()),
            _ => Value::Null,
        })
    }

    fn publish(&mut self) -> Result<()> {
        if self.published == Some((self.revision, self.background)) || self.dirty {
            return Ok(());
        }
        checkpoint();
        // Rendering, linting and JSON construction are cancellable work and
        // must never prevent the router from accepting the next edit.
        let mut payloads = Vec::new();
        for (path, document) in &self.documents {
            checkpoint();
            let mut diagnostics = if let Some(message) = &self.project_error {
                vec![project_diagnostic(message)]
            } else if let Some((project, logical)) = self.workspace.file(path) {
                self.diagnostics.for_file(project, logical, &document.text)
            } else {
                Vec::new()
            };
            diagnostics.extend(
                self.background_errors
                    .iter()
                    .map(|message| project_diagnostic(message)),
            );
            payloads.push((
                document.uri.clone(),
                json!({"uri":document.uri,"version":document.version,"diagnostics":diagnostics}),
            ));
        }
        let mut current = self.published_closed.clone();
        if self.background {
            current.clear();
            for project in &self.workspace.projects {
                let Some(analysis) = project.source_analysis() else {
                    continue;
                };
                for (logical, text) in &analysis.sources {
                    checkpoint();
                    let path = file_identity(&project.source_directory.join(logical));
                    if self.documents.contains_key(&path) {
                        continue;
                    }
                    let Ok(uri) = Url::from_file_path(path) else {
                        continue;
                    };
                    let diagnostics = self.diagnostics.for_file(project, logical, text);
                    if !diagnostics.is_empty() || self.published_closed.contains(uri.as_str()) {
                        payloads.push((
                            uri.to_string(),
                            json!({"uri":uri.as_str(),"diagnostics":diagnostics}),
                        ));
                        current.insert(uri.to_string());
                    }
                }
            }
            for uri in self.published_closed.difference(&current) {
                checkpoint();
                if self.documents.values().any(|document| &document.uri == uri) {
                    continue;
                }
                payloads.push((uri.clone(), json!({"uri":uri,"diagnostics":[]})));
            }
        }
        // Open documents publish on each analyzed revision and again when full
        // background checks finish, even when their text/version did not change
        // (for example, a dependency edit). The early guard deduplicates repeated
        // publication of one phase; closed files also deduplicate equal payloads.
        let notifications: Vec<_> = payloads
            .iter()
            .filter(|(uri, params)| {
                params.get("version").is_some() || self.published_payloads.get(uri) != Some(params)
            })
            .map(|(_, params)| {
                checkpoint();
                Message::Notification(Notification::new(
                    "textDocument/publishDiagnostics".into(),
                    params.clone(),
                ))
            })
            .collect();
        checkpoint();
        {
            // The only scheduling critical section is the final revision check
            // and enqueue of already-rendered notifications.
            let schedule = self.shared.lock().unwrap();
            if schedule.revision != self.revision {
                return Ok(());
            }
            for notification in notifications {
                self.sender.send(notification)?;
            }
        }
        self.published_payloads.extend(payloads);
        self.published_payloads.retain(|uri, _| {
            current.contains(uri) || self.documents.values().any(|document| &document.uri == uri)
        });
        self.published_closed = current;
        self.published = Some((self.revision, self.background));
        Ok(())
    }
}

fn project_diagnostic(message: &str) -> Value {
    json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":1,"source":"ruddy","code":"project","message":message})
}

fn format_edits(index: &LineIndex, source: &str) -> Value {
    let formatted = ruddy::format::format(source, ruddy::tracking::FileID::GENERATED);
    if formatted.text == source {
        return Value::Array(Vec::new());
    }
    json!([{
        "range": {"start": index.position(0), "end": index.position(source.len())},
        "newText": formatted.text,
    }])
}

fn file_path(uri: &str) -> Option<PathBuf> {
    Url::parse(uri)
        .ok()?
        .to_file_path()
        .ok()
        .map(|path| file_identity(&path))
}

/// Convert negotiated UTF-16 positions to compiler byte offsets.
/// Repeated conversions should use a shared `LineIndex`.
pub fn offset(text: &str, position: &Value) -> Option<usize> {
    LineIndex::new(text).offset(position)
}

pub fn position(text: &str, at: usize) -> Value {
    LineIndex::new(text).position(at)
}

// Every response path releases queued cancellation state, including unknown
// requests and shutdown. Registration remains live until the response is sent.
struct RequestRegistration {
    schedule: Arc<Mutex<Schedule>>,
    id: RequestId,
}
impl Drop for RequestRegistration {
    fn drop(&mut self) {
        let mut schedule = self.schedule.lock().unwrap();
        schedule.pending.remove(&self.id);
        schedule.cancelled.remove(&self.id);
    }
}
