//! Compiled dependency artifacts kept between runs.
//!
//! A dependency's artifact is a function of four things: the compiler, the
//! dependency's own sources, the artifacts of its dependencies, and the build
//! it was compiled for — target and platform — which its `@if` guards were
//! judged against. Each is digested into the key an artifact is stored under, so a
//! cached artifact is used only while all four are what they were — and there
//! is no versioning to keep up with, because the compiler's part of the key is
//! the [`Stamp`](ruddy::artifact::Stamp) its build script computed from its
//! source.
//!
//! An artifact read back from the cache is trusted on its stamp: it was
//! validated when this compiler wrote it, and the reader that parses its text
//! establishes every structural invariant again on the way in. What the
//! stamp excludes is an artifact another compiler wrote, which is neither
//! read nor kept: the directory of every other compiler is removed when this
//! one first stores something. A cache that only ever holds one compiler's
//! work cannot be read by the wrong one.
//!
//! Nothing here can fail a build. A cache that cannot be read is a miss, and
//! one that cannot be written is left alone.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ruddy::artifact::{Artifact, COMPILER_HASH, text};
use twox_hash::XxHash3_64;

use crate::Build;

/// One compiler's cache directory.
pub(crate) struct ArtifactCache {
    /// The directory holding every compiler's artifacts, one subdirectory
    /// each.
    root: PathBuf,
    /// This compiler's subdirectory.
    directory: PathBuf,
    /// Whether other compilers' directories have been removed this run.
    pruned: bool,
}

impl ArtifactCache {
    /// The cache rooted at `root`, which is created when first written to.
    pub(crate) fn at(root: PathBuf) -> Self {
        let directory = root.join(COMPILER_HASH);
        Self {
            root,
            directory,
            pruned: false,
        }
    }

    fn path(&self, key: u64) -> PathBuf {
        self.directory.join(format!("{key:016x}.artifact"))
    }

    /// The artifact stored under `key`, if this compiler wrote one for the
    /// bundle called `name` and it still reads back.
    pub(crate) fn load(&self, key: u64, name: &str) -> Option<Artifact> {
        let source = fs::read_to_string(self.path(key)).ok()?;
        let artifact = text::try_parse(&source).ok()?;
        let header = artifact.header();
        (header.compiler.is_current() && header.identity.name == name).then_some(artifact)
    }

    /// Keep `artifact` under `key`, and drop every other compiler's cache
    /// the first time this compiler stores anything.
    pub(crate) fn store(&mut self, key: u64, artifact: &Artifact) {
        if fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        if !self.pruned {
            self.pruned = true;
            if let Ok(entries) = fs::read_dir(&self.root) {
                for entry in entries.flatten() {
                    if entry.file_name() != COMPILER_HASH.as_ref() as &std::ffi::OsStr {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
        let _ = crate::replace_file(&self.path(key), artifact.print().as_bytes());
    }
}

/// The key of a project's artifact: this compiler, the project's manifest and
/// sources, the keys of the artifacts it was compiled against, and every fact
/// of the build it was compiled for.
pub(crate) fn key(directory: &Path, dependencies: &[u64], build: Build) -> u64 {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(COMPILER_HASH.as_bytes());
    for (fact, value) in build.environment().facts() {
        bytes.extend_from_slice(fact.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(
        &fingerprint(std::slice::from_ref(&directory.to_path_buf())).to_le_bytes(),
    );
    for dependency in dependencies {
        bytes.extend_from_slice(&dependency.to_le_bytes());
    }
    XxHash3_64::oneshot(&bytes)
}

/// A digest of every source file, manifest and lockfile under `paths` — a
/// directory is walked, a file is read — in a fixed order, with a file that
/// cannot be read digested as its error so that a directory going missing
/// reads as a change rather than as nothing.
///
/// Contents rather than modification times: the files are few and small, and
/// an editor that rewrites a file with the same text within one timestamp
/// tick is the case timestamps get wrong.
pub fn fingerprint(paths: &[PathBuf]) -> u64 {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            collect(path, &mut files);
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    let mut bytes = Vec::new();
    for file in &files {
        bytes.extend_from_slice(file.to_string_lossy().as_bytes());
        bytes.push(0);
        match fs::read(file) {
            Ok(contents) => bytes.extend_from_slice(&contents),
            Err(error) => bytes.extend_from_slice(format!("{:?}", error.kind()).as_bytes()),
        }
        bytes.push(0);
    }
    XxHash3_64::oneshot(&bytes)
}

/// Every file under `dir` the compiler could have read, skipping hidden
/// entries and build output so a Git checkout's own metadata and a project's
/// own artifacts are not walked.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        // Unreadable is a state worth digesting: the entry stands in for
        // the directory itself, and reading it records the error.
        files.push(dir.to_path_buf());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == crate::BUILD_DIRECTORY {
            continue;
        }
        if path.is_dir() {
            collect(&path, files);
        } else if name == "Ruddy.toml" || name == "Ruddy.lock" || name.ends_with(".rud") {
            files.push(path);
        }
    }
}
