//! End-to-end runtime tests executed by Node's test runner.
//!
//! The checked-in bundle owns the Ruddy program and its JavaScript assertions.
//! This Rust test only copies it to a temporary project so build output never
//! lands in the fixture, then exercises the same build-and-run path as the CLI.

use std::{fs, path::Path, process::Command};

use ruddy_cli::build_project;

/// Bound generated-program regressions while draining both pipes concurrently.
fn run_node(command: &mut Command) -> std::process::Output {
    use std::{
        io::Read,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Node starts");
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate a stuck generated program");
            child.wait().unwrap();
            panic!("generated program exceeded 60 seconds");
        }
        thread::sleep(Duration::from_millis(20));
    };
    std::process::Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn run_assertions(project: &Path, script: &str) -> std::path::PathBuf {
    let generated = build_project(project)
        .expect("the library builds")
        .with_extension("js");
    let output = run_node(
        Command::new("node")
            .arg(script)
            .arg(&generated)
            .current_dir(project),
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    generated
}

const FILES: &[&str] = &["Ruddy.toml", "main.rud", "Math.rud", "runtime.test.mjs"];

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
            "name = \"{fixture}\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n\n[run]\njs = \"node {script}\"\n\n[dependencies]\nstd = {:?}\n",
            root
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

#[test]
fn rethrowing_arms_and_return_arms_use_surrounding_handlers() {
    run_bundle("effect-rethrowing", "rethrowing.test.mjs");
}

/// Definition metadata compiled through the CLI's own path and read back by
/// Node: every attribute form on every kind of definition, including a
/// module whose body is another file, and a program that computes exactly
/// what it would without them.
#[test]
fn definition_metadata_leaves_the_program_unchanged() {
    run_bundle("definition-metadata", "metadata.test.mjs");
}

#[test]
fn cps_execution_preserves_results_without_growing_the_host_stack() {
    run_bundle("cps", "cps.test.mjs");
}

#[test]
fn cps_handlers_resume_after_foreign_promise_completion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundles/cps");
    let project = tempfile::tempdir().unwrap();
    fs::copy(root.join("async.rud"), project.path().join("main.rud")).unwrap();
    fs::copy(
        root.join("async.test.mjs"),
        project.path().join("async.test.mjs"),
    )
    .unwrap();
    fs::write(project.path().join("Ruddy.toml"), "name = \"async-test\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    run_assertions(project.path(), "async.test.mjs");
}

#[test]
fn independently_built_cps_bundles_suspend_in_order_and_resume_callers() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundles/cps-multibundle");
    let project = tempfile::tempdir().unwrap();
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    fs::copy(fixture.join("dependency.rud"), dependency.join("main.rud")).unwrap();
    fs::write(dependency.join("Ruddy.toml"), "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    let portable = build_project(&dependency).expect("dependency builds before the caller exists");
    let text = fs::read_to_string(&portable).unwrap();
    let artifact = ruddy::artifact::Artifact::try_parse(&text)
        .unwrap()
        .validate()
        .unwrap();
    assert!(
        artifact
            .lir()
            .functions
            .iter()
            .any(|f| f.suspension == ruddy::lir::Suspension::MaySuspend)
    );
    fs::copy(fixture.join("main.rud"), project.path().join("main.rud")).unwrap();
    fs::copy(
        fixture.join("multibundle.test.mjs"),
        project.path().join("multibundle.test.mjs"),
    )
    .unwrap();
    fs::write(project.path().join("Ruddy.toml"), "name = \"root\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\ndep = \"./dep\"\n").unwrap();
    run_assertions(project.path(), "multibundle.test.mjs");
}

#[test]
fn rethrowing_survives_independent_artifacts_and_structural_aliases() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundles/rethrowing-imports");
    let project = tempfile::tempdir().unwrap();
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    fs::copy(fixture.join("dependency.rud"), dependency.join("main.rud")).unwrap();
    fs::write(dependency.join("Ruddy.toml"), "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    let (graph, _) = ruddy_cli::compile_dependency_graph([("dep", &dependency)])
        .expect("forwarding library compiles independently of a host entry point");
    let serialized = graph.projects[0].artifact.print();
    let artifact = ruddy::artifact::Artifact::try_parse(&serialized)
        .unwrap()
        .validate()
        .unwrap();
    assert_eq!(artifact.header().values.len(), 2);
    fs::copy(fixture.join("main.rud"), project.path().join("main.rud")).unwrap();
    fs::copy(
        fixture.join("imports.test.mjs"),
        project.path().join("imports.test.mjs"),
    )
    .unwrap();
    fs::write(project.path().join("Ruddy.toml"), "name = \"root\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\ndep = \"./dep\"\n").unwrap();
    run_assertions(project.path(), "imports.test.mjs");
}

#[test]
fn executable_main_waits_for_async_initialization_and_preserves_process_exit() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("Ruddy.toml"), "name = \"async-main\"\nversion = \"1.0.0\"\nkind = \"executable\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    fs::write(project.path().join("main.rud"), r#"
        effect IO = { write: String -> (), write_error: String -> () }
        effect Exit = Nat -> |
        @async
        extern wait : String -> String = "value => new Promise(resolve => queueMicrotask(() => { console.log(value); resolve(value); }))"
        let initialized = wait "initialized"
        let main = fn _ => do
          let _ = wait "main"
          let _ = !IO.write initialized
          let _ = !IO.write_error "error"
          return !Exit 23n
        end
    "#).unwrap();
    let artifact = build_project(project.path()).unwrap();
    let output = run_node(Command::new("node").arg(artifact.with_extension("js")));
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"initialized\nmain\ninitialized");
    assert_eq!(output.stderr, b"error");
}

#[test]
fn async_initialization_does_not_assimilate_ordinary_thenable_data() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("Ruddy.toml"), "name = \"async-data\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    fs::write(project.path().join("main.rud"), r#"
        @async
        extern wait : () -> () = "() => Promise.resolve(null)"
        extern data : () -> ForeignValue = "() => ({ get then() { globalThis.inspected++; return () => {}; } })"
        let value = do let _ = wait () return data () end
    "#).unwrap();
    fs::write(
        project.path().join("data.test.mjs"),
        r#"
        import assert from 'node:assert/strict';
        import { pathToFileURL } from 'node:url';
        globalThis.inspected = 0;
        const app = await import(pathToFileURL(process.argv[2]));
        assert.equal(globalThis.inspected, 0);
        assert.equal(typeof Object.getOwnPropertyDescriptor(app.value, 'then').get, 'function');
    "#,
    )
    .unwrap();
    run_assertions(project.path(), "data.test.mjs");
}
