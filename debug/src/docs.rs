//! Scratch documents: the bundles you keep around while debugging.
//!
//! A document is a directory under `debug/scratch/<doc>/` holding plain `.hc`
//! files, so anything else — the CLI compiler, `grep`, an editor — can read one
//! too. `main.hc` is the root; the rest are whatever its modules name. The page
//! keeps its own copy in `localStorage`; this is the durable one.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{
    snapshot::ROOT,
    wire::{Doc, DocMeta, FileSpec},
};

const EXTENSION: &str = "hc";

/// The longest path a file inside a document may have. Long enough for a module
/// nested deeper than anyone will nest one, short enough that no request can ask
/// the filesystem to think about a path of its own devising.
const MAX_PATH: usize = 128;

/// A document name is a single path segment of a restricted alphabet, so no
/// request can name a directory outside the scratch directory. This and
/// [`valid_file_path`] are the only checks standing between a URL and the
/// filesystem; keep both total.
pub fn valid_name(name: &str) -> bool {
    name.len() <= 64 && segment(name)
}

/// One segment of a path inside the scratch directory: non-empty, and drawn
/// from the alphabet a document's own name is.
///
/// That alphabet holds no `.` and no separator of any platform's, so an empty
/// segment, a `.`, a `..` and anything else that could climb out of the
/// directory it is joined to are all refused by this one rule rather than by a
/// list of special cases — which is what makes it possible to be sure the list
/// is complete.
fn segment(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether a path names a file *inside* a document.
///
/// A module's file path mirrors its logical path, so this is more than one
/// segment and cannot be [`valid_name`]. What it must still be is a relative,
/// `/`-separated path of segments the alphabet above allows, ending in `.hc`,
/// with nothing in it that could climb out of the directory it is joined to: no
/// empty segment, no `.`, no `..`, no leading or trailing `/`, and no leading
/// drive or root of any other shape a platform might read.
///
/// Total, and deliberately answering `false` rather than normalising: a path
/// this refuses is one the reader can retype, and a path it rewrote would be a
/// second place where what was asked for and what was written differ.
pub fn valid_file_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH {
        return false;
    }
    // The extension comes off first, so what is left is a path of plain names
    // and every one of them is held to the same rule. A leading or trailing
    // `/`, a doubled one, a `.` and a `..` all become a segment [`segment`]
    // refuses, so none of them needs a case of its own.
    let Some(names) = path.strip_suffix(&format!(".{EXTENSION}")) else {
        return false;
    };
    names.split('/').all(segment)
}

/// The directory a document lives in.
pub fn path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_name(name).then(|| root.join(name))
}

/// Where one file of a document sits on disk.
///
/// Built one segment at a time from a path [`valid_file_path`] has accepted, so
/// nothing platform-specific about separators can smuggle a component in — and
/// checked once more afterwards, because a path that reached outside the
/// document is the one mistake here that would not be a mistake anywhere else.
pub fn file_path(root: &Path, name: &str, file: &str) -> Option<PathBuf> {
    if !valid_file_path(file) {
        return None;
    }
    let mut at = path(root, name)?;
    for segment in file.split('/') {
        at.push(segment);
    }
    at.components()
        .all(|component| !matches!(component, Component::ParentDir))
        .then_some(at)
}

pub fn list(root: &Path) -> io::Result<Vec<DocMeta>> {
    let mut docs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        // Directories only: a document is a bundle now, and a flat `.hc` file
        // left over from before is neither read nor migrated.
        if !meta.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let files = collect(&dir, "")?;
        docs.push(DocMeta {
            name: name.to_string(),
            bytes: files.iter().map(|file| file.source.len() as u64).sum(),
            modified_ms: modified_ms(&meta),
        });
    }
    // Most recently touched first: the one you were just working on.
    docs.sort_by_key(|doc| std::cmp::Reverse(doc.modified_ms));
    Ok(docs)
}

pub fn read(root: &Path, name: &str) -> io::Result<Doc> {
    let dir = path(root, name).ok_or_else(bad_name)?;
    let mut files = collect(&dir, "")?;
    // The root first, then the rest by path: a stable order the page can show
    // its file strip in without sorting it again.
    files.sort_by_key(|file| (file.path != ROOT, file.path.clone()));
    Ok(Doc {
        name: name.to_string(),
        files,
        modified_ms: modified_ms(&fs::metadata(&dir)?),
    })
}

/// Replace a document's contents with `files`.
///
/// Every file in the request is written and every `.hc` file on disk that is
/// not in it is deleted, so what comes back from [`read`] is what was sent. A
/// file the page renamed is a write and a delete rather than a move, which is
/// the same thing from here and one fewer operation to get wrong.
pub fn write(root: &Path, name: &str, files: &[FileSpec]) -> io::Result<u128> {
    let dir = path(root, name).ok_or_else(bad_name)?;
    fs::create_dir_all(&dir)?;
    for file in files {
        let at = file_path(root, name, &file.path).ok_or_else(bad_name)?;
        if let Some(parent) = at.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&at, &file.source)?;
    }
    for existing in collect(&dir, "")? {
        if files.iter().any(|file| file.path == existing.path) {
            continue;
        }
        if let Some(at) = file_path(root, name, &existing.path) {
            fs::remove_file(at)?;
        }
    }
    modified(&dir)?;
    modified_of(&dir)
}

pub fn delete(root: &Path, name: &str) -> io::Result<()> {
    let dir = path(root, name).ok_or_else(bad_name)?;
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Create the scratch directory, seeding it from the repository's `demo.hc` the
/// first time so a fresh checkout opens onto something worth compiling.
///
/// A flat `debug/scratch/*.hc` from before documents were directories is left
/// exactly where it is: the scratch directory is gitignored working state, and
/// a migration that guessed which snippets were bundles would be a worse answer
/// than leaving them alone.
pub fn ensure(root: &Path, seed: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let demo = root.join("demo");
    if !demo.exists()
        && let Ok(source) = fs::read_to_string(seed)
    {
        fs::create_dir_all(&demo)?;
        fs::write(demo.join(ROOT), source)?;
    }
    Ok(())
}

/// Every `.hc` file under `dir`, with its path relative to the document root.
/// Anything else in the directory — a stray file, a directory holding no `.hc`
/// at all — is ignored rather than reported, the way an orphan module file is.
fn collect(dir: &Path, prefix: &str) -> io::Result<Vec<FileSpec>> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = match prefix.is_empty() {
            true => name.clone(),
            false => format!("{prefix}/{name}"),
        };
        if entry.metadata()?.is_dir() {
            files.extend(collect(&entry.path(), &path)?);
            continue;
        }
        if !valid_file_path(&path) {
            continue;
        }
        files.push(FileSpec {
            path,
            source: fs::read_to_string(entry.path())?,
        });
    }
    Ok(files)
}

/// Touch the directory, so a document that only had a file deleted still reads
/// back as the most recently changed one.
fn modified(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
}

fn modified_of(dir: &Path) -> io::Result<u128> {
    Ok(modified_ms(&fs::metadata(dir)?))
}

fn modified_ms(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_millis())
        .unwrap_or(0)
}

fn bad_name() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid document name")
}
