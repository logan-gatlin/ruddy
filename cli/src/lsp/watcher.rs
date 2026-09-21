//! Watch acquired files and absent module candidates. Native events select the
//! paths to read; infrequent reconciliation recovers from dropped events and
//! filesystems without reliable native notifications.
use super::{Envelope, Schedule};
use crate::workspace::{file_identity, normalize};
use crossbeam_channel::Sender;
use lsp_server::{Message, Notification};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use url::Url;

type Inputs = HashMap<PathBuf, Option<String>>;
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

pub(super) enum Command {
    Inputs(Inputs),
    Event(notify::Result<Event>),
    Stop,
}

pub(super) struct Watcher {
    pub inputs: Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn new(sender: Sender<Envelope>, schedule: Arc<Mutex<Schedule>>) -> Self {
        let (inputs, receive) = crossbeam_channel::unbounded();
        let events = inputs.clone();
        let thread = std::thread::spawn(move || {
            let mut watcher = notify::recommended_watcher(move |event| {
                let _ = events.send(Command::Event(event));
            })
            .ok();
            let mut state = State::default();
            let mut reconcile_at = Instant::now() + RECONCILE_INTERVAL;
            loop {
                let mut paths = HashSet::new();
                let mut reconcile = false;
                let command =
                    receive.recv_timeout(reconcile_at.saturating_duration_since(Instant::now()));
                match command {
                    Ok(Command::Stop) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => reconcile = true,
                    Ok(command) => state.command(command, &mut paths, &mut reconcile),
                }
                // A save can produce create, rename and write events; observe its
                // final contents once, rather than cancelling on every event.
                for command in receive.try_iter() {
                    if matches!(command, Command::Stop) {
                        return;
                    }
                    state.command(command, &mut paths, &mut reconcile);
                }
                // Watch before reading newly subscribed files, closing the gap
                // between the compiler's acquisition and watch installation.
                state.watches_dirty |= reconcile;
                if reconcile {
                    // A replaced directory can leave a watch attached to the
                    // old inode even though the same pathname exists again.
                    state.invalidated.extend(state.directories.iter().cloned());
                }
                let identities_changed = state.update_watches(watcher.as_mut());
                paths.extend(identities_changed.iter().cloned());
                if reconcile {
                    paths.extend(state.inputs.keys().cloned());
                    reconcile_at = Instant::now() + RECONCILE_INTERVAL;
                }
                let mut changes = Vec::new();
                for path in paths {
                    let Some(before) = state.inputs.get_mut(&path) else {
                        continue;
                    };
                    let current = std::fs::read_to_string(&path).ok();
                    if *before != current || identities_changed.contains(&path) {
                        let kind = match (&*before, &current) {
                            (None, Some(_)) => 1,
                            (Some(_), None) => 3,
                            _ => 2,
                        };
                        *before = current;
                        if let Ok(uri) = Url::from_file_path(&path) {
                            changes.push(json!({"uri":uri.as_str(),"type":kind}));
                        }
                    }
                }
                if changes.is_empty() {
                    continue;
                }
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
                            json!({"changes":changes}),
                        )),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            inputs,
            thread: Some(thread),
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.inputs.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct State {
    inputs: Inputs,
    directories: HashSet<PathBuf>,
    identities: HashMap<PathBuf, PathBuf>,
    watches_dirty: bool,
    invalidated: HashSet<PathBuf>,
}

impl State {
    fn command(&mut self, command: Command, paths: &mut HashSet<PathBuf>, reconcile: &mut bool) {
        match command {
            Command::Inputs(inputs) => {
                self.watches_dirty |= inputs.len() != self.inputs.len()
                    || inputs.keys().any(|path| !self.inputs.contains_key(path));
                paths.extend(
                    inputs
                        .iter()
                        .filter(|(path, text)| self.inputs.get(*path) != Some(*text))
                        .map(|(path, _)| path.clone()),
                );
                self.inputs = inputs;
            }
            Command::Event(Ok(event)) if event.need_rescan() => *reconcile = true,
            Command::Event(Ok(event)) if !matches!(event.kind, EventKind::Access(_)) => {
                if event.paths.is_empty() {
                    *reconcile = true;
                }
                self.watches_dirty |= !matches!(
                    event.kind,
                    EventKind::Modify(notify::event::ModifyKind::Data(_))
                );
                for changed in event.paths {
                    let changed = normalize(&changed);
                    self.invalidated.extend(
                        self.directories
                            .iter()
                            .filter(|directory| directory.starts_with(&changed))
                            .cloned(),
                    );
                    paths.extend(
                        self.inputs
                            .keys()
                            .filter(|candidate| {
                                candidate.starts_with(&changed)
                                    || self
                                        .identities
                                        .get(*candidate)
                                        .is_some_and(|identity| identity.starts_with(&changed))
                            })
                            .cloned(),
                    );
                }
            }
            Command::Event(Err(_)) => *reconcile = true,
            _ => {}
        }
    }

    fn update_watches(&mut self, watcher: Option<&mut RecommendedWatcher>) -> HashSet<PathBuf> {
        if !std::mem::take(&mut self.watches_dirty) {
            return HashSet::new();
        }
        let Some(watcher) = watcher else {
            return HashSet::new();
        };
        for path in self.invalidated.drain() {
            let _ = watcher.unwatch(&path);
            self.directories.remove(&path);
        }
        let mut desired = HashSet::new();
        let previous = std::mem::take(&mut self.identities);
        let mut identities_changed = HashSet::new();
        for path in self.inputs.keys() {
            add_parent(path, &mut desired);
            let identity = watch_identity(path, &mut desired);
            add_parent(&identity, &mut desired);
            if previous.get(path).is_some_and(|before| *before != identity) {
                identities_changed.insert(path.clone());
            }
            self.identities.insert(path.clone(), identity);
        }
        for path in self.directories.difference(&desired) {
            let _ = watcher.unwatch(path);
        }
        self.directories
            .retain(|path| desired.contains(path) && path.is_dir());
        for path in desired {
            if !self.directories.contains(&path)
                && watcher.watch(&path, RecursiveMode::NonRecursive).is_ok()
            {
                self.directories.insert(path);
            }
        }
        identities_changed
    }
}

fn add_parent(path: &Path, directories: &mut HashSet<PathBuf>) {
    if let Some(parent) = path.ancestors().skip(1).find(|ancestor| ancestor.is_dir()) {
        // Aliases of one directory must share a native watch; otherwise
        // unwatching one spelling can remove the other spelling's descriptor.
        directories.insert(std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_owned()));
    }
}

// Canonicalization alone forgets the target of a broken symlink. Resolve link
// text as well so recreating a deleted target is still observed immediately.
fn watch_identity(path: &Path, directories: &mut HashSet<PathBuf>) -> PathBuf {
    let mut pending: VecDeque<OsString> = path
        .components()
        .map(|component| component.as_os_str().to_owned())
        .collect();
    let mut resolved = PathBuf::new();
    let mut links = 0;
    while let Some(component) = pending.pop_front() {
        match Path::new(&component).components().next() {
            Some(Component::ParentDir) => {
                resolved.pop();
                continue;
            }
            Some(Component::CurDir) | None => continue,
            _ => resolved.push(component),
        }
        if let Ok(target) = std::fs::read_link(&resolved) {
            // Every link in a chain can be retargeted independently. Subscribe
            // to its parent even when it is outside the original source tree.
            add_parent(&resolved, directories);
            links += 1;
            if links > 32 {
                return file_identity(path);
            }
            resolved.pop();
            if target.is_absolute() {
                resolved.clear();
            }
            // Resolve parents before interpreting '..' in a relative target:
            // a lexical parent can itself be a symlink to another directory.
            for component in target.components().rev() {
                pending.push_front(component.as_os_str().to_owned());
            }
        }
    }
    file_identity(&resolved)
}
