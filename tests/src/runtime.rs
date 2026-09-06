//! End-to-end runtime tests executed by Node's test runner.
//!
//! The checked-in bundle owns the Ruddy program and its JavaScript assertions.
//! This Rust test only copies it to a temporary project so build output never
//! lands in the fixture, then exercises the same build-and-run path as the CLI.

use std::{fs, path::Path, process::Command};

use ruddy_cli::build_project;

fn run_assertions(project: &Path, script: &str) -> std::path::PathBuf {
    let generated = build_project(project)
        .expect("the library builds")
        .with_extension("js");
    let output = Command::new("node")
        .arg(script)
        .arg(&generated)
        .current_dir(project)
        .output()
        .expect("Node is available");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    generated
}

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

    let generated = run_assertions(project.path(), "runtime.test.mjs");
    assert_eq!(generated, project.path().join("build/runtime-tests.js"));
    assert!(generated.is_file());
}

/// Build and run one checked-in bundle whose program and assertions live in
/// `bundles/<fixture>`, from a temporary project so no output lands in the
/// fixture. Every file of the fixture is copied, so a bundle may have module
/// files beside its root. The manifest is written here because it has to name
/// the standard library by an absolute path. Skipped where there is no Node
/// to run it.
fn run_bundle(fixture: &str, script: &str) {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("bundles")
        .join(fixture);
    let project = tempfile::tempdir().expect("a temporary bundle project");
    for entry in fs::read_dir(&source).expect("the fixture directory") {
        let entry = entry.expect("a fixture entry");
        let name = entry.file_name();
        fs::copy(entry.path(), project.path().join(&name)).unwrap_or_else(|error| {
            panic!(
                "could not copy {fixture} fixture {}: {error}",
                name.display()
            )
        });
    }
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"{fixture}\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.hc\"\ntarget = \"js\"\n\n[run]\njs = \"node {script}\"\n\n[dependencies]\nstd = {:?}\n",
            root.join("std")
        ),
    )
    .unwrap();

    let generated = run_assertions(project.path(), script);
    assert!(generated.is_file());
}

/// The array runtime, checked against a plain JavaScript array by a seeded
/// sequence of every operation the standard library exposes.
#[test]
fn the_array_runtime_agrees_with_a_plain_array() {
    run_bundle("arrays", "arrays.test.mjs");
}

/// Struct spreads compiled through the CLI's own path and read back by Node:
/// every accepted form, the fields each keeps, and the order the pieces run
/// in.
#[test]
fn struct_spreads_build_the_fields_they_promise() {
    run_bundle("struct-spreads", "spreads.test.mjs");
}

/// `do` blocks compiled through the CLI's own path and read back by Node:
/// what each form evaluates to, which names a binding sees, and the order
/// the statements run in.
#[test]
fn do_blocks_evaluate_in_order_to_what_they_return() {
    run_bundle("do-blocks", "blocks.test.mjs");
}

/// Definition metadata compiled through the CLI's own path and read back by
/// Node: every attribute form on every kind of definition, including a
/// module whose body is another file, and a program that computes exactly
/// what it would without them.
#[test]
fn definition_metadata_leaves_the_program_unchanged() {
    run_bundle("definition-metadata", "metadata.test.mjs");
}
