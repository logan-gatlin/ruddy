//! Scratch documents: the snippets you keep around while debugging.
//!
//! They are plain `.hc` files under `debug/scratch/`, so anything else — the
//! CLI compiler, `grep`, an editor — can read them too. The page keeps its own
//! copy in `localStorage`; this is the durable one.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::wire::{Doc, DocMeta};

const EXTENSION: &str = "hc";

/// A document name is a single path segment of a restricted alphabet, so no
/// request can name a file outside the scratch directory. This is the only
/// check standing between a URL and the filesystem; keep it total.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn path(root: &Path, name: &str) -> Option<PathBuf> {
    valid_name(name).then(|| root.join(format!("{name}.{EXTENSION}")))
}

pub fn list(root: &Path) -> io::Result<Vec<DocMeta>> {
    let mut docs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file = entry.path();
        if file.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
            continue;
        }
        let Some(name) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let meta = entry.metadata()?;
        docs.push(DocMeta {
            name: name.to_string(),
            bytes: meta.len(),
            modified_ms: modified_ms(&meta),
        });
    }
    // Most recently touched first: the one you were just working on.
    docs.sort_by_key(|doc| std::cmp::Reverse(doc.modified_ms));
    Ok(docs)
}

pub fn read(root: &Path, name: &str) -> io::Result<Doc> {
    let path = path(root, name).ok_or_else(bad_name)?;
    let source = fs::read_to_string(&path)?;
    let modified_ms = modified_ms(&fs::metadata(&path)?);
    Ok(Doc {
        name: name.to_string(),
        source,
        modified_ms,
    })
}

pub fn write(root: &Path, name: &str, source: &str) -> io::Result<u128> {
    let path = path(root, name).ok_or_else(bad_name)?;
    fs::write(&path, source)?;
    Ok(modified_ms(&fs::metadata(&path)?))
}

pub fn delete(root: &Path, name: &str) -> io::Result<()> {
    let path = path(root, name).ok_or_else(bad_name)?;
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Create the scratch directory, seeding it from the repository's `demo.hc` the
/// first time so a fresh checkout opens onto something worth compiling.
pub fn ensure(root: &Path, seed: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let demo = root.join(format!("demo.{EXTENSION}"));
    if !demo.exists()
        && let Ok(source) = fs::read_to_string(seed)
    {
        fs::write(demo, source)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_a_single_safe_segment() {
        assert!(valid_name("demo"));
        assert!(valid_name("regress-42_b"));

        // Everything that could reach outside the scratch directory.
        assert!(!valid_name(""));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
        assert!(!valid_name("../../etc/passwd"));
        assert!(!valid_name("a.hc"));
        assert!(!valid_name(&"x".repeat(65)));
    }

    #[test]
    fn a_rejected_name_never_becomes_a_path() {
        assert!(path(Path::new("/tmp"), "..").is_none());
        assert_eq!(
            path(Path::new("/tmp"), "demo"),
            Some(PathBuf::from("/tmp/demo.hc"))
        );
    }
}
