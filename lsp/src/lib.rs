//! LSP transport and newest-revision scheduling. Input handling runs separately
//! from analysis; only a current revision may publish diagnostics.
use crossbeam_channel::Sender;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use ruddy::{analysis::CompletionKind, cancellation::Cancellation, tracking::Span};
use ruddy_cli::workspace::{Workspace, file_identity};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};
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
        "hoverProvider":true,"definitionProvider":true,
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
    let mut worker = Worker {
        workspace: Workspace::new(root),
        documents: HashMap::new(),
        dirty: true,
        revision: 0,
        shutdown: false,
        shared,
        sender: connection.sender,
        project_error: None,
        background: false,
        background_errors: Vec::new(),
        published: None,
        published_closed: HashSet::new(),
        watched: watcher.inputs.clone(),
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
            if token.run(|| worker.refresh()).is_ok() {
                worker.publish()?;
            }
            worker.end();
        }
        if !worker.shutdown
            && !worker.dirty
            && !worker.background
            && !worker.documents.is_empty()
            && receive.is_empty()
        {
            let token = worker.begin(None);
            if let Ok(errors) = token.run(|| worker.workspace.check_background()) {
                worker.background_errors = errors;
                worker.background = true;
                worker.publish()?;
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
    documents: HashMap<PathBuf, Document>,
    dirty: bool,
    revision: u64,
    shutdown: bool,
    shared: Arc<Mutex<Schedule>>,
    sender: Sender<Message>,
    project_error: Option<String>,
    background: bool,
    background_errors: Vec<String>,
    published: Option<(u64, bool)>,
    published_closed: HashSet<String>,
    watched: Arc<Mutex<HashMap<PathBuf, Option<String>>>>,
}

impl Worker {
    fn begin(&self, request: Option<RequestId>) -> Cancellation {
        let token = Cancellation::default();
        let mut schedule = self.shared.lock().unwrap();
        if schedule.revision != self.revision
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
                            let start = range
                                .get("start")
                                .and_then(|position| offset(&text, position));
                            let end = range
                                .get("end")
                                .and_then(|position| offset(&text, position));
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
                        self.workspace
                            .set_overlay(&path, Some(document.text.clone()));
                        self.dirty = true;
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
                        self.sender.send(Message::Notification(Notification::new(
                            "textDocument/publishDiagnostics".into(),
                            json!({"uri":document.uri,"diagnostics":[]}),
                        )))?;
                    }
                    self.workspace.set_overlay(&path, None);
                    self.dirty = true;
                }
            }
            "workspace/didChangeWatchedFiles"
            | "workspace/didChangeConfiguration"
            | "textDocument/didSave" => self.dirty = true,
            _ => {}
        }
        Ok(())
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.project_error = self
            .workspace
            .refresh()
            .err()
            .map(|error| error.messages().join("\n"));
        *self.watched.lock().unwrap() = self.workspace.observed_files();
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
            "textDocument/hover" | "textDocument/completion" | "textDocument/definition"
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
            self.refresh();
            if let Some(path) = request
                .params
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .and_then(file_path)
                && self.workspace.request_file(&path)
            {
                self.published = None;
            }
            self.answer(&request)
        });
        self.end();
        self.shared.lock().unwrap().cancelled.remove(&request.id);
        let response = match result {
            Ok(Ok(value)) => Response::new_ok(request.id, value),
            Ok(Err(message)) => Response::new_err(request.id, -32602, message),
            Err(_) => Response::new_err(request.id, -32800, "analysis was cancelled".into()),
        };
        self.sender.send(Message::Response(response))?;
        // A request may have finished the revision's analysis before the idle
        // diagnostics task ran. Publish through the same revision barrier.
        self.publish()?;
        Ok(())
    }

    fn answer(&self, request: &Request) -> std::result::Result<Value, String> {
        let path = request
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(file_path)
            .ok_or("expected a file URI")?;
        let Some((project, logical)) = self.workspace.file(&path) else {
            return Ok(Value::Null);
        };
        let source = &project.analysis.sources[logical];
        let at = request
            .params
            .get("position")
            .and_then(|position| offset(source, position))
            .ok_or("invalid document position")?;
        Ok(match request.method.as_str() {
            "textDocument/hover" => project.analysis.hover(logical, at).map(|hover| json!({"contents":{"kind":"markdown","value":format!("```ruddy\n{}\n```",hover.ty)},"range":range(source, hover.span)})).unwrap_or(Value::Null),
            "textDocument/completion" => Value::Array(project.analysis.completions(logical, at).into_iter().map(|item| {
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
            "textDocument/definition" => self.workspace.definition(&path, at).and_then(|(path, span)| {
                let (target, logical) = self.workspace.file(&path)?;
                let uri = self.documents.get(&file_identity(&path)).map(|document| document.uri.clone())
                    .or_else(|| Url::from_file_path(path).ok().map(|uri| uri.to_string()))?;
                Some(json!({"uri":uri,"range":range(&target.analysis.sources[logical],span)}))
            }).unwrap_or(Value::Null),
            _ => Value::Null,
        })
    }

    fn publish(&mut self) -> Result<()> {
        if self.published == Some((self.revision, self.background)) || self.dirty {
            return Ok(());
        }
        let schedule = self.shared.lock().unwrap();
        if schedule.revision != self.revision {
            return Ok(());
        }
        for (path, document) in &self.documents {
            let mut diagnostics: Vec<_> = if let Some(message) = &self.project_error {
                vec![
                    json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":1,"source":"ruddy","code":"project","message":message}),
                ]
            } else if let Some((project, logical)) = self.workspace.file(path) {
                diagnostics_for(project, logical, &document.text)
            } else {
                Vec::new()
            };
            diagnostics.extend(self.background_errors.iter().map(|message| json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"severity":1,"source":"ruddy","code":"project","message":message})));
            self.sender.send(Message::Notification(Notification::new(
                "textDocument/publishDiagnostics".into(),
                json!({"uri":document.uri,"version":document.version,"diagnostics":diagnostics}),
            )))?;
        }
        if self.background {
            let mut current = HashSet::new();
            for project in &self.workspace.projects {
                for (logical, text) in &project.analysis.sources {
                    let path = file_identity(&project.source_directory.join(logical));
                    if self.documents.contains_key(&path) {
                        continue;
                    }
                    let Ok(uri) = Url::from_file_path(path) else {
                        continue;
                    };
                    let diagnostics = diagnostics_for(project, logical, text);
                    if !diagnostics.is_empty() || self.published_closed.contains(uri.as_str()) {
                        self.sender.send(Message::Notification(Notification::new(
                            "textDocument/publishDiagnostics".into(),
                            json!({"uri":uri.as_str(),"diagnostics":diagnostics}),
                        )))?;
                        current.insert(uri.to_string());
                    }
                }
            }
            for uri in self.published_closed.difference(&current) {
                if self.documents.values().any(|document| &document.uri == uri) {
                    continue;
                }
                self.sender.send(Message::Notification(Notification::new(
                    "textDocument/publishDiagnostics".into(),
                    json!({"uri":uri,"diagnostics":[]}),
                )))?;
            }
            self.published_closed = current;
        }
        self.published = Some((self.revision, self.background));
        Ok(())
    }
}

fn diagnostics_for(
    project: &ruddy_cli::workspace::ProjectAnalysis,
    logical: &str,
    text: &str,
) -> Vec<Value> {
    project.analysis.diagnostics.iter().filter(|diagnostic| project.analysis.paths.get(&diagnostic.primary.span.file_id).is_some_and(|path| path == logical)).map(|diagnostic| {
                    let mut message = diagnostic.title.clone();
                    for detail in std::iter::once(&diagnostic.primary.message).chain(&diagnostic.notes).chain(&diagnostic.help) {
                        if !detail.is_empty() && detail != &diagnostic.title { message.push('\n'); message.push_str(detail); }
                    }
                    let related: Vec<_> = diagnostic.related.iter().filter_map(|annotation| {
                        let logical = project.analysis.paths.get(&annotation.span.file_id)?;
                        let uri = Url::from_file_path(project.source_directory.join(logical)).ok()?;
                        let source = project.analysis.sources.get(logical)?;
                        Some(json!({"location":{"uri":uri.as_str(),"range":range(source,annotation.span)},"message":annotation.message}))
                    }).collect();
                    json!({"range":range(text,diagnostic.primary.span),"severity":1,"source":"ruddy","code":diagnostic.code,"message":message,"relatedInformation":related})
                }).collect()
}

fn file_path(uri: &str) -> Option<PathBuf> {
    Url::parse(uri)
        .ok()?
        .to_file_path()
        .ok()
        .map(|path| file_identity(&path))
}

/// Convert negotiated UTF-16 positions to compiler byte offsets.
pub fn offset(text: &str, position: &Value) -> Option<usize> {
    let line = usize::try_from(position.get("line")?.as_u64()?).ok()?;
    let character = usize::try_from(position.get("character")?.as_u64()?).ok()?;
    let start = if line == 0 {
        0
    } else {
        text.match_indices('\n').nth(line - 1)?.0 + 1
    };
    let line_text = text[start..].split('\n').next()?.trim_end_matches('\r');
    let mut units = 0;
    for (at, ch) in line_text.char_indices() {
        if units == character {
            return Some(start + at);
        }
        units += ch.len_utf16();
        if units > character {
            return None;
        }
    }
    Some(start + line_text.len())
}

pub fn position(text: &str, at: usize) -> Value {
    let at = at.min(text.len());
    let prefix = &text[..text.floor_char_boundary(at)];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let start = prefix.rfind('\n').map_or(0, |at| at + 1);
    json!({"line":line,"character":prefix[start..].encode_utf16().count()})
}
fn range(text: &str, span: Span) -> Value {
    json!({"start":position(text,span.start),"end":position(text,span.end())})
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

// Poll precisely the acquired input set, including absent candidates and files
// outside the root workspace. This also serves clients without dynamic watched-
// file registration. Open-buffer changes still arrive immediately via LSP.
struct Watcher {
    inputs: Arc<Mutex<HashMap<PathBuf, Option<String>>>>,
    stop: Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watcher {
    fn new(sender: Sender<Envelope>, schedule: Arc<Mutex<Schedule>>) -> Self {
        let inputs = Arc::new(Mutex::new(HashMap::<PathBuf, Option<String>>::new()));
        let watched = inputs.clone();
        let (stop, receive) = crossbeam_channel::bounded(1);
        let thread = std::thread::spawn(move || {
            while receive
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_err()
            {
                let mut inputs = watched.lock().unwrap();
                let mut changed = false;
                for (path, before) in inputs.iter_mut() {
                    let current = std::fs::read_to_string(path).ok();
                    if *before != current {
                        *before = current;
                        changed = true;
                    }
                }
                if changed {
                    let mut schedule = schedule.lock().unwrap();
                    schedule.revision += 1;
                    if let Some((token, _)) = &schedule.active {
                        token.cancel();
                    }
                    if sender
                        .send(Envelope {
                            revision: schedule.revision,
                            message: Message::Notification(Notification::new(
                                "workspace/didChangeWatchedFiles".into(),
                                json!({"changes":[]}),
                            )),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            inputs,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
