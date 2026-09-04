//! End-to-end runtime tests executed by Node's test runner.
//!
//! The checked-in bundle owns the Ruddy program and its JavaScript assertions.
//! This Rust test only copies it to a temporary project so build output never
//! lands in the fixture, then exercises the same build-and-run path as the CLI.

use std::{fs, path::Path, process::Command};

use ruddy_cli::run_project;

const FILES: &[&str] = &["Ruddy.toml", "main.hc", "Math.hc", "runtime.test.mjs"];

#[test]
fn generated_javascript_passes_the_node_runtime_suite() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundles/runtime");
    let project = tempfile::tempdir().expect("a temporary runtime-test project");
    for name in FILES {
        fs::copy(fixture.join(name), project.path().join(name))
            .unwrap_or_else(|error| panic!("could not copy runtime fixture {name}: {error}"));
    }

    let generated = run_project(project.path()).expect("the Node runtime suite passes");
    assert_eq!(generated, project.path().join("build/runtime-tests.js"));
    assert!(generated.is_file());
}

/// The array runtime, checked against a plain JavaScript array by a seeded
/// sequence of every operation the standard library exposes. The fixture
/// owns the program and the assertions; the manifest is written here because
/// it has to name the standard library by an absolute path.
#[test]
fn the_array_runtime_agrees_with_a_plain_array() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundles/arrays");
    let project = tempfile::tempdir().expect("a temporary array-test project");
    for name in ["main.hc", "arrays.test.mjs"] {
        fs::copy(fixture.join(name), project.path().join(name))
            .unwrap_or_else(|error| panic!("could not copy array fixture {name}: {error}"));
    }
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"array-tests\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\ntarget = \"js\"\n\n[run]\njs = \"node arrays.test.mjs\"\n\n[dependencies]\nstd = {:?}\n",
            root.join("std")
        ),
    )
    .unwrap();

    let generated = run_project(project.path()).expect("the array differential suite passes");
    assert!(generated.is_file());
}
