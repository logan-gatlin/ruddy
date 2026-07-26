//! Polls a set of directories for changes.
//!
//! Polling rather than an inotify dependency: 250 ms is imperceptible next to
//! the `cargo build` that follows a source change, and the dependency list
//! stays at four crates.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

const INTERVAL: Duration = Duration::from_millis(250);

/// Watch `roots` for changes to files with one of `extensions`, calling
/// `on_change` once per detected change. Runs until the process exits.
pub fn spawn(
    roots: Vec<PathBuf>,
    extensions: &'static [&'static str],
    on_change: impl Fn() + Send + 'static,
) {
    thread::spawn(move || {
        let mut previous = fingerprint(&roots, extensions);
        loop {
            thread::sleep(INTERVAL);
            let current = fingerprint(&roots, extensions);
            if current != previous {
                previous = current;
                on_change();
            }
        }
    });
}

/// Modification times keyed by path. A `BTreeMap` so the comparison does not
/// depend on directory iteration order, and so a file appearing or vanishing
/// counts as a change just as an edit does.
fn fingerprint(roots: &[PathBuf], extensions: &[&str]) -> BTreeMap<PathBuf, SystemTime> {
    let mut seen = BTreeMap::new();
    for root in roots {
        walk(root, extensions, &mut seen);
    }
    seen
}

fn walk(path: &Path, extensions: &[&str], seen: &mut BTreeMap<PathBuf, SystemTime>) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.is_file() {
        let matches = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e));
        if matches && let Ok(modified) = meta.modified() {
            seen.insert(path.to_path_buf(), modified);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        walk(&entry.path(), extensions, seen);
    }
}
