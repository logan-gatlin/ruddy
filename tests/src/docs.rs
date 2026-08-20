//! Tests for [`ruddy_debug::docs`].

use std::path::{Path, PathBuf};

use ruddy_debug::{
    docs::{delete, path, read, valid_file_path, valid_name, write},
    wire::FileSpec,
};

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
    // A document is a directory now, so its path is the directory's: the files
    // inside it are what carry the extension.
    assert_eq!(
        path(Path::new("/tmp"), "demo"),
        Some(PathBuf::from("/tmp/demo"))
    );
}

/// The other half of the security boundary, and the one a request can spell
/// most of: a module's file path mirrors its logical path, so it is more than
/// one segment and cannot be [`valid_name`]. What it may never be is anything
/// that could climb out of the document it is joined to.
#[test]
fn a_file_path_is_a_relative_hc_path_and_nothing_else() {
    // The shapes a module's file really takes: beside its parent, or inside the
    // directory its parent's name spells.
    assert!(valid_file_path("main.hc"));
    assert!(valid_file_path("Math.hc"));
    assert!(valid_file_path("Math/Vec.hc"));
    assert!(valid_file_path("Math/Vec/module.hc"));
    assert!(valid_file_path("regress-42_b.hc"));

    // Everything that could reach outside the document's directory.
    assert!(!valid_file_path(".."));
    assert!(!valid_file_path("../main.hc"));
    assert!(!valid_file_path("Math/../../main.hc"));
    assert!(!valid_file_path("./main.hc"));
    assert!(!valid_file_path("/etc/passwd.hc"));
    assert!(!valid_file_path("/main.hc"));
    assert!(!valid_file_path("Math//Vec.hc"));
    assert!(!valid_file_path("Math/.hc"));
    assert!(!valid_file_path("main.hc/"));
    assert!(!valid_file_path("C:\\main.hc"));

    // A file of a bundle is a `.hc` file. Anything else in the directory is
    // somebody's stray note, and the page has no business writing one.
    assert!(!valid_file_path(""));
    assert!(!valid_file_path("main"));
    assert!(!valid_file_path("main.rs"));
    assert!(!valid_file_path(".hc"));

    // And a path long enough to be a filesystem's problem rather than a
    // module's is refused rather than truncated.
    assert!(valid_file_path(&format!(
        "{}/{}.hc",
        "a".repeat(62),
        "b".repeat(62)
    )));
    assert!(!valid_file_path(&format!(
        "{}/{}.hc",
        "a".repeat(63),
        "b".repeat(63)
    )));
}

/// A document is a bundle, so what goes in comes back out: every file, at the
/// path it was written under. The root leads, because the page shows the file
/// strip in the order it reads them in.
#[test]
fn a_document_round_trips_through_the_disk() {
    let root = scratch("round-trip");
    let files = [
        file("main.hc", "bundle demo 0.1.0\nmodule Math\n"),
        file("Math.hc", "module Vec\nlet double = fn x => x\n"),
        file("Math/Vec.hc", "let zero = 0n\n"),
    ];
    write(&root, "demo", &files).expect("the document is written");

    let doc = read(&root, "demo").expect("the document is read back");
    assert_eq!(doc.name, "demo");
    let back: Vec<(&str, &str)> = doc
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.source.as_str()))
        .collect();
    assert_eq!(
        back,
        [
            ("main.hc", "bundle demo 0.1.0\nmodule Math\n"),
            ("Math.hc", "module Vec\nlet double = fn x => x\n"),
            ("Math/Vec.hc", "let zero = 0n\n"),
        ]
    );
    assert!(doc.modified_ms > 0);

    delete(&root, "demo").expect("the document is deleted");
}

/// A write replaces a document rather than adding to one: a file the page
/// dropped is gone from the disk too. Otherwise a module deleted in the editor
/// would keep being compiled, and the reader would be shown a bundle nobody
/// wrote.
#[test]
fn a_write_deletes_a_file_dropped_from_the_set() {
    let root = scratch("dropped");
    write(
        &root,
        "demo",
        &[
            file("main.hc", "bundle demo 0.1.0\nmodule Math\n"),
            file("Math.hc", "let double = fn x => x\n"),
        ],
    )
    .expect("the document is written");

    write(&root, "demo", &[file("main.hc", "bundle demo 0.1.0\n")])
        .expect("the document is written again");

    let doc = read(&root, "demo").expect("the document is read back");
    let paths: Vec<&str> = doc.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["main.hc"]);
    assert!(!root.join("demo").join("Math.hc").exists());

    delete(&root, "demo").expect("the document is deleted");
    assert!(!root.join("demo").exists());
}

/// A scratch root of this test's own, so two tests writing documents cannot
/// read each other's — and neither can two runs of the suite.
fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ruddy-docs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("the scratch directory is created");
    root
}

fn file(path: &str, source: &str) -> FileSpec {
    FileSpec {
        path: path.to_string(),
        source: source.to_string(),
    }
}
