//! A small, immutable standard-library checkout shared by ordinary consumers.
//!
//! Pointing std at the repository makes every dependency fingerprint walk
//! `target` and editor dependencies. Keep only the manifest and std sources,
//! with a content-derived path so compiled dependencies survive test reruns.

use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub fn path() -> &'static Path {
    static STANDARD: OnceLock<PathBuf> = OnceLock::new();
    STANDARD.get_or_init(|| {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        materialize(repository, &repository.join("target/test-fixtures"))
    })
}

fn materialize(repository: &Path, cache: &Path) -> PathBuf {
    fn read_sources(repository: &Path, relative: &Path, sources: &mut Vec<(PathBuf, Vec<u8>)>) {
        let path = repository.join(relative);
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                read_sources(
                    repository,
                    &relative.join(entry.unwrap().file_name()),
                    sources,
                );
            }
        } else {
            sources.push((relative.to_owned(), fs::read(path).unwrap()));
        }
    }

    // Hash and publish the same bytes, even if a source is edited while a
    // test process starts. Unrelated repository files never enter the key.
    let mut sources = Vec::new();
    read_sources(repository, Path::new("Ruddy.toml"), &mut sources);
    read_sources(repository, Path::new("std"), &mut sources);
    sources.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut digest = DefaultHasher::new();
    sources.hash(&mut digest);
    let directory = cache.join(format!("std-{:016x}", digest.finish()));
    if directory.is_dir() {
        return directory;
    }

    fs::create_dir_all(cache).unwrap();
    let temporary = tempfile::tempdir_in(cache).unwrap();
    for (relative, contents) in sources {
        let destination = temporary.path().join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, contents).unwrap();
    }
    // Separate test processes may publish the same snapshot concurrently.
    // Atomic rename exposes only complete trees; the losing copy is dropped.
    if let Err(error) = fs::rename(temporary.path(), &directory) {
        assert!(directory.is_dir(), "publish std fixture: {error}");
    }
    directory
}

#[test]
fn source_changes_publish_a_new_fixture_without_changing_existing_consumers() {
    let repository = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(repository.path().join("Ruddy.toml"), "root = 'std/lib.rud'").unwrap();
    fs::create_dir(repository.path().join("std")).unwrap();
    let source = repository.path().join("std/lib.rud");
    fs::write(&source, "let answer = 1n").unwrap();

    let original = materialize(repository.path(), cache.path());
    fs::create_dir(repository.path().join("target")).unwrap();
    fs::write(repository.path().join("target/unrelated.rud"), "ignored").unwrap();
    assert_eq!(materialize(repository.path(), cache.path()), original);
    assert!(!original.join("target").exists());

    fs::write(source, "let answer = 2n").unwrap();
    let updated = materialize(repository.path(), cache.path());
    assert_ne!(updated, original);
    assert_eq!(
        fs::read_to_string(original.join("std/lib.rud")).unwrap(),
        "let answer = 1n"
    );
    assert_eq!(
        fs::read_to_string(updated.join("std/lib.rud")).unwrap(),
        "let answer = 2n"
    );
}

#[test]
fn concurrent_consumers_receive_the_same_complete_fixture() {
    let repository = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(repository.path().join("Ruddy.toml"), "root = 'std/lib.rud'").unwrap();
    fs::create_dir_all(repository.path().join("std/nested")).unwrap();
    fs::write(repository.path().join("std/lib.rud"), "module nested").unwrap();
    fs::write(
        repository.path().join("std/nested/lib.rud"),
        "let answer = 1n",
    )
    .unwrap();
    let barrier = std::sync::Barrier::new(4);
    let paths = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let path = materialize(repository.path(), cache.path());
                    assert_eq!(
                        fs::read_to_string(path.join("std/nested/lib.rud")).unwrap(),
                        "let answer = 1n"
                    );
                    path
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(paths.iter().all(|path| path == &paths[0]));
}
