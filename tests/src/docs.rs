//! Tests for [`ruddy_debug::docs`].

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use ruddy_debug::{
    docs::{delete, dependency_path, path, read, valid_file_path, valid_name, write},
    wire::{DependencySpec, FileSpec, RunConfig, StdConfig},
};

#[test]
fn names_are_a_single_safe_segment() {
    assert!(valid_name("demo"));
    assert!(valid_name("regress-42_b"));

    // Everything that could reach outside the scratch directory.
    assert!(!valid_name(""));
    assert!(!valid_name("3demo"));
    assert!(!valid_name("_demo"));
    assert!(!valid_name("-demo"));
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
        file("main.hc", "module Math\n"),
        file("Math.hc", "module Vec\nlet double = fn x => x\n"),
        file("Math/Vec.hc", "let zero = 0n\n"),
    ];
    let dependencies = IndexMap::from([("base".into(), "../base".into())]);
    let run = RunConfig {
        js: Some("node".into()),
    };
    write(
        &root,
        "demo",
        "configured",
        "1.2.3",
        "main.hc",
        &run,
        &Default::default(),
        &dependencies,
        &files,
    )
    .expect("the document is written");

    let doc = read(&root, "demo").expect("the document is read back");
    assert_eq!(doc.name, "demo");
    assert_eq!(doc.bundle_name, "configured");
    assert_eq!(doc.version, "1.2.3");
    assert_eq!(doc.root, "main.hc");
    assert_eq!(doc.run, run);
    assert_eq!(doc.std, StdConfig::default());
    assert!(matches!(
        &doc.dependencies["base"],
        ruddy_debug::wire::DependencySpec::Path(path)
            if path == Path::new("../base")
    ));
    let back: Vec<(&str, &str)> = doc
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.source.as_str()))
        .collect();
    assert_eq!(
        back,
        [
            ("main.hc", "module Math\n"),
            ("Math.hc", "module Vec\nlet double = fn x => x\n"),
            ("Math/Vec.hc", "let zero = 0n\n"),
        ]
    );
    assert!(doc.modified_ms > 0);

    delete(&root, "demo").expect("the document is deleted");
}

#[test]
fn the_reserved_std_alias_is_never_persisted_as_a_declared_dependency() {
    let root = scratch("reserved-std");
    let dependencies = IndexMap::from([("std".into(), DependencySpec::from("../standard"))]);
    let found = write(
        &root,
        "demo",
        "demo",
        "0.1.0",
        "main.hc",
        &RunConfig::default(),
        &StdConfig::default(),
        &dependencies,
        &[file("main.hc", "")],
    )
    .unwrap_err();
    assert_eq!(found.kind(), std::io::ErrorKind::InvalidInput);
    assert!(found.to_string().contains("alias `std` is reserved"));
    assert!(!root.join("demo").exists());

    std::fs::create_dir_all(root.join("demo")).unwrap();
    std::fs::write(root.join("demo/main.hc"), "").unwrap();
    std::fs::write(
        root.join("demo/Ruddy.toml"),
        "name = \"demo\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nstd = \"../standard\"\n",
    )
    .unwrap();
    let found = read(&root, "demo").unwrap_err();
    assert_eq!(found.kind(), std::io::ErrorKind::InvalidData);
    assert!(found.to_string().contains("duplicate key"));
}

#[test]
fn the_configured_root_is_ordered_before_other_files() {
    let root = scratch("configured-root-order");
    write(
        &root,
        "demo",
        "demo",
        "0.1.0",
        "start.hc",
        &RunConfig::default(),
        &Default::default(),
        &IndexMap::new(),
        &[file("main.hc", ""), file("start.hc", "")],
    )
    .unwrap();

    let doc = read(&root, "demo").unwrap();
    assert_eq!(
        doc.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["start.hc", "main.hc"]
    );
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
        "demo",
        "0.1.0",
        "main.hc",
        &RunConfig::default(),
        &Default::default(),
        &IndexMap::new(),
        &[
            file("main.hc", "module Math\n"),
            file("Math.hc", "let double = fn x => x\n"),
        ],
    )
    .expect("the document is written");

    write(
        &root,
        "demo",
        "demo",
        "0.1.0",
        "main.hc",
        &RunConfig::default(),
        &Default::default(),
        &IndexMap::new(),
        &[file("main.hc", "")],
    )
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

#[test]
fn standard_library_configuration_round_trips_and_defaults_to_installed() {
    let root = scratch("std-configuration");
    let files = [file("main.hc", "")];
    for configuration in [
        StdConfig::Disabled,
        StdConfig::Dependency(DependencySpec::from("../custom-std")),
    ] {
        write(
            &root,
            "demo",
            "demo",
            "0.1.0",
            "main.hc",
            &RunConfig::default(),
            &configuration,
            &IndexMap::new(),
            &files,
        )
        .unwrap();
        assert_eq!(read(&root, "demo").unwrap().std, configuration);
    }

    let source = std::fs::read_to_string(root.join("demo/Ruddy.toml")).unwrap();
    assert!(
        source.contains("[dependencies]\nstd = \"../custom-std\""),
        "{source}"
    );
    std::fs::write(
        root.join("demo/Ruddy.toml"),
        "name = \"demo\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    assert_eq!(read(&root, "demo").unwrap().std, StdConfig::Default);
}

#[test]
fn dependency_paths_are_canonical_and_sandboxed() {
    let root = scratch("dependencies");
    let app = root.join("app");
    let base = root.join("base");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&base).unwrap();
    assert_eq!(
        dependency_path(&root, &app, Path::new("../base")).unwrap(),
        std::fs::canonicalize(&base).unwrap()
    );
    assert!(dependency_path(&root, &app, Path::new("/tmp")).is_err());
    assert!(dependency_path(&root, &app, Path::new("../../")).is_err());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp", root.join("outside")).unwrap();
        assert!(dependency_path(&root, &app, Path::new("../outside")).is_err());
    }
}

#[test]
fn missing_and_invalid_manifests_are_not_replaced_by_json_defaults() {
    let root = scratch("manifest-errors");
    std::fs::create_dir(root.join("demo")).unwrap();
    std::fs::write(root.join("demo/.ruddy-debug.json"), r#"{"name":"old"}"#).unwrap();
    assert_eq!(
        read(&root, "demo").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    std::fs::write(root.join("demo/Ruddy.toml"), "not toml =").unwrap();
    assert_eq!(
        read(&root, "demo").unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

fn file(path: &str, source: &str) -> FileSpec {
    FileSpec {
        path: path.to_string(),
        source: source.to_string(),
    }
}
