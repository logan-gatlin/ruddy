//! Routing, static files, and the event stream.
//!
//! Blocking and boringly synchronous: a compile of a realistic snippet is well
//! under a millisecond, so the only reason there is more than one worker is so
//! that a held-open event stream cannot wedge the next `/compile`.

use std::{
    collections::VecDeque,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{
    docs, snapshot,
    wire::{CompileRequest, DocBody, ServerStatus},
};

/// Enough to keep a held-open poll and a compile from queueing behind each
/// other, with room for a second tab.
const WORKERS: usize = 4;

/// How long a poll waits before returning empty-handed. Long enough that idle
/// pages are quiet, short enough that a dead connection is noticed.
const POLL: Duration = Duration::from_secs(20);

/// Events kept for pages that were between polls when something happened.
const KEEP: usize = 64;

/// Requests larger than this are refused rather than buffered.
const MAX_BODY: u64 = 1024 * 1024;

pub struct Config {
    pub port: u16,
    pub web: PathBuf,
    pub scratch: PathBuf,
    pub doc: Option<String>,
    pub build: u64,
    pub build_error: Option<String>,
    pub watch: bool,
    pub watch_roots: Vec<PathBuf>,
}

pub struct State {
    pub cfg: Config,
    pub events: Events,
}

/// Events the page waits on, delivered by long polling.
///
/// Not server-sent events: `tiny_http` buffers a chunked body until it has
/// 8 KiB of it, so a stream of short frames would sit in the encoder and never
/// reach the browser. A held-open request that returns the moment something
/// happens gives the same latency through the ordinary response path.
#[derive(Default)]
pub struct Events {
    log: Mutex<Log>,
    arrived: Condvar,
}

#[derive(Default)]
struct Log {
    /// Number of events ever sent. A page polls with the sequence it last saw.
    seq: u64,
    recent: VecDeque<(u64, String)>,
}

impl Events {
    /// Record an event and wake every waiting page.
    pub fn send(&self, event: &str, data: &str) {
        let mut log = self.log.lock().unwrap();
        log.seq += 1;
        let seq = log.seq;
        log.recent
            .push_back((seq, format!(r#"{{"event":"{event}","data":{data}}}"#)));
        while log.recent.len() > KEEP {
            log.recent.pop_front();
        }
        drop(log);
        self.arrived.notify_all();
    }

    /// Everything after `since`, waiting up to [`POLL`] for something to
    /// happen. `None` means the page has never polled before, which returns
    /// immediately so it can learn the current sequence.
    pub fn poll(&self, since: Option<u64>) -> (u64, Vec<String>) {
        let mut log = self.log.lock().unwrap();
        let Some(since) = since else {
            return (log.seq, Vec::new());
        };
        if log.seq <= since {
            let (guard, _) = self.arrived.wait_timeout(log, POLL).unwrap();
            log = guard;
        }
        let events = log
            .recent
            .iter()
            .filter(|(seq, _)| *seq > since)
            .map(|(_, json)| json.clone())
            .collect();
        (log.seq, events)
    }
}

pub fn serve(cfg: Config) -> io::Result<()> {
    let address = format!("127.0.0.1:{}", cfg.port);
    let server = Server::http(&address).map_err(|err| io::Error::other(err.to_string()))?;
    let server = Arc::new(server);
    let state = Arc::new(State {
        cfg,
        events: Events::default(),
    });

    if state.cfg.watch {
        watch_sources(&state);
        watch_web(&state);
    }

    println!("ruddy-debug: http://{address}  (build {})", state.cfg.build);
    if let Some(error) = &state.cfg.build_error {
        println!("  serving the last good binary; rebuild failed:");
        println!("{}", indent(error));
    }

    let mut workers = Vec::new();
    for _ in 1..WORKERS {
        let server = Arc::clone(&server);
        let state = Arc::clone(&state);
        workers.push(thread::spawn(move || accept(&server, &state)));
    }
    accept(&server, &state);
    Ok(())
}

fn accept(server: &Server, state: &Arc<State>) {
    while let Ok(request) = server.recv() {
        let state = Arc::clone(state);
        // A poll holds its connection until something happens, so it gets a
        // thread of its own rather than one of the four workers.
        match is_poll(&request) {
            true => {
                thread::spawn(move || respond_poll(request, &state));
            }
            false => handle(request, &state),
        }
    }
}

fn is_poll(request: &Request) -> bool {
    *request.method() == Method::Get && path_of(request.url()) == "/events"
}

/// `GET /events?since=N`. Without `since`, the page is new and is told the
/// current sequence and build immediately; with it, the request waits.
fn respond_poll(request: Request, state: &State) {
    let since = request
        .url()
        .split_once("since=")
        .and_then(|(_, rest)| rest.split('&').next())
        .and_then(|value| value.parse::<u64>().ok());

    let (seq, events) = state.events.poll(since);
    let body = format!(
        r#"{{"seq":{seq},"build":{},"events":[{}]}}"#,
        state.cfg.build,
        events.join(",")
    );
    let _ = request.respond(
        Response::from_string(body)
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cache-Control", "no-store")),
    );
}

fn handle(mut request: Request, state: &State) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let path = path_of(&url).to_string();

    let result = match (&method, path.as_str()) {
        (Method::Get, "/") => file(&state.cfg.web.join("index.html")),
        (Method::Get, "/status") => json(&ServerStatus {
            build: state.cfg.build,
            watching: state.cfg.watch,
            doc: state.cfg.doc.clone(),
            build_error: state.cfg.build_error.clone(),
        }),
        (Method::Post, "/compile") => match body(&mut request) {
            Ok(body) => match serde_json::from_str::<CompileRequest>(&body) {
                Ok(req) => json(&snapshot::compile(&req, state.cfg.build)),
                Err(err) => fail(400, &format!("bad request: {err}")),
            },
            Err(err) => fail(413, &err),
        },
        (Method::Get, "/docs") => match docs::list(&state.cfg.scratch) {
            Ok(list) => json(&list),
            Err(err) => fail(500, &err.to_string()),
        },
        (Method::Get, path) if path.starts_with("/static/") => {
            static_file(&state.cfg.web, &path["/static/".len()..])
        }
        (_, path) if path.starts_with("/docs/") => {
            document(&method, &path["/docs/".len()..], &mut request, state)
        }
        _ => fail(404, "not found"),
    };

    let _ = request.respond(result);
}

fn document(
    method: &Method,
    name: &str,
    request: &mut Request,
    state: &State,
) -> Response<io::Cursor<Vec<u8>>> {
    if !docs::valid_name(name) {
        return fail(400, "invalid document name");
    }
    match method {
        Method::Get => match docs::read(&state.cfg.scratch, name) {
            Ok(doc) => json(&doc),
            Err(err) => fail(404, &err.to_string()),
        },
        Method::Put => {
            let body = match body(request) {
                Ok(body) => body,
                Err(err) => return fail(413, &err),
            };
            let files = match serde_json::from_str::<DocBody>(&body) {
                Ok(body) => body.files,
                Err(err) => return fail(400, &format!("bad request: {err}")),
            };
            // Every path is checked before anything is written, so a request
            // with one bad path leaves the document exactly as it was.
            if !files.iter().all(|file| docs::valid_file_path(&file.path)) {
                return fail(400, "invalid file path");
            }
            match docs::write(&state.cfg.scratch, name, &files) {
                Ok(modified_ms) => json(&serde_json::json!({ "modified_ms": modified_ms })),
                Err(err) => fail(500, &err.to_string()),
            }
        }
        Method::Delete => match docs::delete(&state.cfg.scratch, name) {
            Ok(()) => text(204, ""),
            Err(err) => fail(500, &err.to_string()),
        },
        _ => fail(405, "method not allowed"),
    }
}

fn body(request: &mut Request) -> Result<String, String> {
    if request
        .body_length()
        .is_some_and(|len| len as u64 > MAX_BODY)
    {
        return Err("request too large".to_string());
    }
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .map_err(|err| err.to_string())?;
    Ok(body)
}

/// Serve a file from the web directory. The path is rejected outright if it
/// contains a parent component, so nothing outside that directory is reachable.
fn static_file(web: &Path, rest: &str) -> Response<io::Cursor<Vec<u8>>> {
    let rest = rest.split('?').next().unwrap_or(rest);
    if rest.split('/').any(|part| part == ".." || part.is_empty()) {
        return fail(400, "bad path");
    }
    file(&web.join(rest))
}

fn file(path: &Path) -> Response<io::Cursor<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Response::from_data(bytes)
            .with_header(header("Content-Type", mime(path)))
            // The page is served straight from disk so that editing it and
            // reloading is the whole frontend loop; caching would undo that.
            .with_header(header("Cache-Control", "no-store")),
        Err(err) => fail(404, &format!("{}: {err}", path.display())),
    }
}

fn json<T: serde::Serialize>(value: &T) -> Response<io::Cursor<Vec<u8>>> {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            Response::from_data(bytes).with_header(header("Content-Type", "application/json"))
        }
        Err(err) => fail(500, &err.to_string()),
    }
}

fn text(status: u16, body: &str) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
}

fn fail(status: u16, message: &str) -> Response<io::Cursor<Vec<u8>>> {
    text(status, message)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is well formed")
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// A compiler source change ends the process with 75, which the `just dev`
/// supervisor reads as "rebuild and start me again".
fn watch_sources(state: &Arc<State>) {
    let roots = state.cfg.watch_roots.clone();
    let state = Arc::clone(state);
    crate::watch::spawn(roots, &["rs", "toml"], move || {
        state.events.send("rebuilding", "{}");
        // Give the frame a moment to reach the page before the socket dies.
        thread::sleep(Duration::from_millis(120));
        std::process::exit(75);
    });
}

/// Web assets are read from disk per request, so a change needs no restart —
/// only a reload, which the page does for itself.
fn watch_web(state: &Arc<State>) {
    let roots = vec![state.cfg.web.clone()];
    let state = Arc::clone(state);
    crate::watch::spawn(roots, &["html", "js", "css"], move || {
        state.events.send("reload-web", "{}");
    });
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
