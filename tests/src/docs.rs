//! Tests for [`ruddy_debug::docs`].

use std::path::{Path, PathBuf};

use ruddy_debug::docs::{path, valid_name};

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
