//! Editor inputs are acquired once and invalidated by filesystem notifications.
//! Missing files are cached too: creating a previously missing module invalidates
//! precisely the same entry as changing or deleting an existing module.
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use super::{file_identity, normalize};

#[derive(Default)]
pub(super) struct Files {
    disk: RefCell<HashMap<PathBuf, Option<String>>>,
    identities: RefCell<HashMap<PathBuf, PathBuf>>,
    listings: RefCell<HashMap<PathBuf, Vec<PathBuf>>>,
    fingerprints: RefCell<HashMap<PathBuf, u64>>,
    acquired: RefCell<HashSet<PathBuf>>,
    pending_acquired: RefCell<HashSet<PathBuf>>,
    pub(super) reads: Cell<usize>,
    pub(super) scans: Cell<usize>,
}

impl Files {
    pub(super) fn identity(&self, path: &Path) -> PathBuf {
        let path = normalize(path);
        if let Some(identity) = self.identities.borrow().get(&path) {
            return identity.clone();
        }
        let identity = file_identity(&path);
        self.identities.borrow_mut().insert(path, identity.clone());
        identity
    }

    pub(super) fn read_disk(&self, path: &Path) -> Option<String> {
        let path = normalize(path);
        if let Some(source) = self.disk.borrow().get(&path) {
            return source.clone();
        }
        self.reads.set(self.reads.get() + 1);
        let source = fs::read_to_string(&path).ok();
        self.disk.borrow_mut().insert(path, source.clone());
        source
    }

    pub(super) fn read(
        &self,
        path: &Path,
        overlays: &HashMap<PathBuf, String>,
        observed: &RefCell<HashMap<PathBuf, Option<String>>>,
    ) -> Option<String> {
        let path = normalize(path);
        let first = self.acquired.borrow_mut().insert(path.clone());
        if first {
            // Fingerprinting also reads files which no module has acquired.
            // They are not watched yet, so their cached text may be stale.
            self.disk.borrow_mut().remove(&path);
            self.identities.borrow_mut().remove(&path);
            self.pending_acquired.borrow_mut().insert(path.clone());
        }
        let disk = self.read_disk(&path);
        observed.borrow_mut().insert(path.clone(), disk.clone());
        let source = overlays.get(&self.identity(&path)).cloned().or(disk);
        if first {
            self.changed_overlay(&path);
            for (directory, paths) in self.listings.borrow_mut().iter_mut() {
                if path.starts_with(directory) {
                    // A newly referenced module can have been created while
                    // it was unwatched, after this directory was enumerated.
                    if source.is_some() && !paths.contains(&path) {
                        paths.push(path.clone());
                        paths.sort();
                    } else if source.is_none() {
                        paths.retain(|known| known != &path);
                    }
                }
            }
        }
        source
    }

    pub(super) fn retain_acquired(&self, observed: &HashMap<PathBuf, Option<String>>) {
        // Once a module leaves the graph its watcher subscription disappears.
        // Reintroducing it must acquire fresh text just like a new module.
        self.acquired
            .borrow_mut()
            .retain(|path| observed.contains_key(path));
        self.pending_acquired.borrow_mut().clear();
    }

    pub(super) fn abandon_acquired(&self) {
        // Cancelled refreshes never publish their newly discovered watcher
        // inputs. Retry those acquisitions without discarding existing watches.
        for path in self.pending_acquired.borrow_mut().drain() {
            self.acquired.borrow_mut().remove(&path);
        }
    }

    pub(super) fn changed_overlay(&self, path: &Path) {
        let identities = self.identities.borrow();
        self.fingerprints.borrow_mut().retain(|directory, _| {
            !path.starts_with(directory)
                && !identities
                    .iter()
                    .any(|(lexical, identity)| identity == path && lexical.starts_with(directory))
        });
    }

    pub(super) fn invalidate(&self, path: &Path) {
        let path = normalize(path);
        let identity = self.identity(&path);
        let aliases: Vec<_> = self
            .identities
            .borrow()
            .iter()
            .filter(|(known, target)| known.starts_with(&path) || target.starts_with(&identity))
            .map(|(known, _)| known.clone())
            .collect();
        for alias in aliases.into_iter().chain(std::iter::once(path)) {
            self.disk.borrow_mut().remove(&alias);
            self.acquired.borrow_mut().remove(&alias);
            self.identities.borrow_mut().remove(&alias);
            self.listings
                .borrow_mut()
                .retain(|directory, _| !alias.starts_with(directory));
            self.changed_overlay(&alias);
        }
    }

    pub(super) fn clear(&self) {
        self.disk.borrow_mut().clear();
        self.identities.borrow_mut().clear();
        self.listings.borrow_mut().clear();
        self.fingerprints.borrow_mut().clear();
        self.acquired.borrow_mut().clear();
        self.pending_acquired.borrow_mut().clear();
    }

    pub(super) fn paths(&self, directory: &Path) -> Vec<PathBuf> {
        if let Some(paths) = self.listings.borrow().get(directory) {
            return paths.clone();
        }
        self.scans.set(self.scans.get() + 1);
        let paths = crate::cache::input_paths(&[directory.to_path_buf()]);
        self.listings
            .borrow_mut()
            .insert(directory.to_path_buf(), paths.clone());
        paths
    }

    pub(super) fn fingerprint(&self, directory: &Path, overlays: &HashMap<PathBuf, String>) -> u64 {
        if let Some(fingerprint) = self.fingerprints.borrow().get(directory) {
            return *fingerprint;
        }
        let mut paths = self.paths(directory);
        paths.extend(
            overlays
                .keys()
                .filter(|path| path.starts_with(directory))
                .cloned(),
        );
        paths.sort();
        paths.dedup();
        let mut bytes = Vec::new();
        for path in paths {
            bytes.extend_from_slice(path.to_string_lossy().as_bytes());
            bytes.push(0);
            let source = overlays
                .get(&self.identity(&path))
                .cloned()
                .or_else(|| self.read_disk(&path));
            match source {
                Some(source) => bytes.extend_from_slice(source.as_bytes()),
                None => bytes.extend_from_slice(b"NotFound"),
            }
            bytes.push(0);
        }
        let fingerprint = twox_hash::XxHash3_64::oneshot(&bytes);
        self.fingerprints
            .borrow_mut()
            .insert(directory.to_path_buf(), fingerprint);
        fingerprint
    }
}
