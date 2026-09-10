//! Tests for the filesystem-facing CLI compiler API.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ruddy::artifact::Artifact;
use ruddy_cli::{
    FormatReport, Lockfile, Outcome, build_project, check_project, clean_project, compile,
    execute_javascript_module, format_paths, new_project, run, run_project,
};
use ruddy_debug::{
    snapshot::{ROOT as DEBUG_ROOT, compile as debug_compile},
    wire::{CompileRequest, FileSpec, StdConfig},
};
use tempfile::TempDir;

fn project() -> TempDir {
    let directory = tempfile::tempdir().expect("a temporary project");
    fs::write(directory.path().join("main.rud"), "let id = fn x => x\n").expect("write the root");
    directory
}

fn disable_std(directory: &Path) {
    let path = directory.join("Ruddy.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        manifest.replace("\n[dependencies]", "\n[dependencies]\nstd = false"),
    )
    .unwrap();
}

fn write_project(directory: &Path, name: &str, version: &str, dependencies: &[(&str, &str)]) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("main.rud"), "let value = 0n\n").unwrap();
    let mut manifest = format!(
        "name = {name:?}\nversion = {version:?}\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n"
    );
    for (dependency, path) in dependencies {
        manifest.push_str(&format!("{dependency} = {path:?}\n"));
    }
    fs::write(directory.join("Ruddy.toml"), manifest).unwrap();
}

fn executable_project(directory: &Path, source: &str, target: Option<&str>) {
    fs::create_dir_all(directory).unwrap();
    let target = target
        .map(|target| format!("target = {target:?}\n"))
        .unwrap_or_default();
    fs::write(directory.join("Ruddy.toml"), format!(
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"executable\"\nroot = \"main.rud\"\n{target}[dependencies]\nstd = false\n"
    )).unwrap();
    fs::write(directory.join("main.rud"), source).unwrap();
}

fn prepare_execution(directory: &Path) {
    let manifest = fs::read_to_string(directory.join("Ruddy.toml")).unwrap();
    fs::write(
        directory.join("Ruddy.toml"),
        manifest.replace("kind = \"library\"", "kind = \"executable\""),
    )
    .unwrap();
    let source = fs::read_to_string(directory.join("main.rud")).unwrap();
    fs::write(
        directory.join("main.rud"),
        format!("{source}\nlet main = fn _ => ()\n"),
    )
    .unwrap();
}

#[test]
fn executable_main_drains_console_output_and_saturates_exit_codes() {
    let directory = tempfile::tempdir().unwrap();
    executable_project(
        directory.path(),
        "let main = fn _ => do\n\
           let _ = std::console::print \"hello\"\n\
           let _ = std::console::write_error \"goodbye\"\n\
           let _ = std::process::exit 999n\n\
           return std::console::print \"unreachable\"\n\
         end",
        None,
    );
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let manifest = fs::read_to_string(directory.path().join("Ruddy.toml")).unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        manifest.replace("std = false", &format!("std = {standard:?}")),
    )
    .unwrap();
    check_project(directory.path()).expect("the standard platform effects are accepted");
    let artifact = build_project(directory.path()).expect("the executable builds");
    let output = Command::new("node")
        .arg(artifact.with_extension("js"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(255),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"hello\n");
    assert_eq!(output.stderr, b"goodbye");
}

#[test]
fn nonreturning_main_preserves_in_range_exit_codes_and_saturates_large_naturals() {
    for (code, expected) in [
        (0_u64, 0),
        (17, 17),
        (255, 255),
        (256, 255),
        (u64::MAX, 255),
    ] {
        let project = tempfile::tempdir().unwrap();
        executable_project(
            project.path(),
            &format!(
                "type Never = |\neffect Process = {{ exit: Nat -> Never }}\nlet main : () -> Never + !Process = fn _ => !Process.exit {code}n"
            ),
            None,
        );
        let built = build_project(project.path()).unwrap_or_else(|error| panic!("{code}: {error}"));
        let output = Command::new("node")
            .arg(built.with_extension("js"))
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        let launched = run_project(project.path());
        if expected == 0 {
            launched.unwrap();
        } else {
            assert_eq!(launched.unwrap_err().exit_code(), expected as u8);
        }
    }
}

#[test]
fn artifact_executable_defers_platform_support_without_running_a_backend() {
    let project = tempfile::tempdir().unwrap();
    executable_project(
        project.path(),
        "effect Custom = () -> ()\nextern backend_only : Nat = \"?\"\nlet main = fn _ => !Custom ()",
        Some("artifact"),
    );
    check_project(project.path()).expect("artifact checks are target-neutral");
    assert!(!project.path().join("build").exists());
    let built = build_project(project.path()).unwrap();
    let artifact = fs::read_to_string(&built).unwrap();
    assert_eq!(
        Artifact::try_parse(&artifact)
            .unwrap()
            .validate()
            .unwrap()
            .header()
            .kind,
        ruddy::artifact::Kind::Executable
    );
    assert!(!built.with_extension("js").exists());
    assert!(!project.path().join("build/package.json").exists());
    assert!(run_project(project.path()).is_err());
    let manifest = fs::read_to_string(project.path().join("Ruddy.toml")).unwrap();
    fs::write(
        project.path().join("Ruddy.toml"),
        manifest.replace("target = \"artifact\"", "target = \"js\""),
    )
    .unwrap();
    for error in [
        check_project(project.path()).unwrap_err(),
        build_project(project.path()).unwrap_err(),
    ] {
        assert!(
            error.to_string().contains("unsupported-entry-effects"),
            "{error}"
        );
    }
    assert_eq!(fs::read_to_string(built).unwrap(), artifact);
}

#[test]
fn executable_can_use_a_dependency_function_with_a_named_never_result() {
    let project = tempfile::tempdir().unwrap();
    let dependency = project.path().join("dep");
    write_project(&dependency, "dep", "1.0.0", &[]);
    fs::write(dependency.join("main.rud"), "type Never = |\neffect Process = { exit: Nat -> Never }\nlet stop : () -> Never + !Process = fn _ => !Process.exit 17n").unwrap();
    let app = project.path().join("app");
    executable_project(&app, "let main = dep::stop", None);
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        format!("{manifest}dep = \"../dep\"\n"),
    )
    .unwrap();
    let artifact =
        build_project(&app).expect("the entry adapter understands linked dependency types");
    let output = Command::new("node")
        .arg(artifact.with_extension("js"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_platform_handlers_override_runtime_behavior_and_can_escape_exit() {
    let project = tempfile::tempdir().unwrap();
    executable_project(
        project.path(),
        r#"
        effect Console = { write: String -> (), write_error: String -> () }
        effect Process = { exit: Nat -> | }
        let locally_handled = fn _ => handle do
            let _ = !Console.write "hidden"
            return !Process.exit 999n
          end with
          | !Console.write _ => ()
          | !Console.write_error _ => ()
          | !Process.exit code => raise code
          | return _ => 0n
          end
        let main = fn _ => do
          let code = locally_handled ()
          let _ = !Console.write "continued"
          return ()
        end
    "#,
        None,
    );
    let artifact = build_project(project.path()).unwrap();
    let output = Command::new("node")
        .arg(artifact.with_extension("js"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"continued");
    assert!(output.stderr.is_empty());
}

#[test]
fn node_rejects_effects_that_only_match_a_standard_effects_name() {
    let project = tempfile::tempdir().unwrap();
    executable_project(
        project.path(),
        "effect Console = { write: Nat -> (), write_error: String -> () }\nlet main = fn _ => !Console.write 42n",
        None,
    );
    let error = check_project(project.path()).unwrap_err();
    assert!(
        error.to_string().contains("unsupported-entry-effects"),
        "{error}"
    );
    assert!(!project.path().join("build").exists());
}

#[test]
fn executable_dependencies_are_rejected_before_building_any_project() {
    let root = tempfile::tempdir().unwrap();
    let dependency = root.path().join("dep");
    executable_project(&dependency, "let main = fn _ => ()", Some("artifact"));
    let consumer = root.path().join("consumer");
    write_project(&consumer, "consumer", "1.0.0", &[("app", "../dep")]);
    let error = build_project(&consumer).unwrap_err();
    assert!(
        error.to_string().contains("executable-dependency"),
        "{error}"
    );
    assert!(!consumer.join("build").exists());
    assert!(!dependency.join("build").exists());
}

#[test]
fn exit_drains_backpressured_stdout_and_stderr_before_terminating() {
    let project = tempfile::tempdir().unwrap();
    executable_project(
        project.path(),
        r#"
        effect Console = { write: String -> (), write_error: String -> () }
        effect Process = { exit: Nat -> | }
        extern text : String = "'x'.repeat(1000000)"
        let main = fn _ => do
          let _ = !Console.write text
          let _ = !Console.write_error text
          return !Process.exit 23n
        end
    "#,
        None,
    );
    let built = build_project(project.path()).unwrap();
    let output = Command::new("node")
        .arg(built.with_extension("js"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(output.stdout, vec![b'x'; 1_000_000]);
    assert_eq!(output.stderr, vec![b'x'; 1_000_000]);
}

#[test]
fn platform_handlers_do_not_make_top_level_initialization_effectful() {
    let project = tempfile::tempdir().unwrap();
    executable_project(
        project.path(),
        "effect Console = { write: String -> (), write_error: String -> () }\nlet value = !Console.write \"unhandled\"\nlet main = fn _ => ()",
        None,
    );
    let path = project.path().join("Ruddy.toml");
    let executable = fs::read_to_string(&path).unwrap();
    for manifest in [
        executable.clone(),
        executable.replace("kind = \"executable\"", "kind = \"library\""),
    ] {
        fs::write(&path, manifest).unwrap();
        let error = check_project(project.path()).unwrap_err();
        assert!(error.to_string().contains("unhandled-effect"), "{error}");
    }
    assert!(!project.path().join("build").exists());
}

#[test]
fn entry_adapter_avoids_extern_only_dependency_name_collisions() {
    let project = tempfile::tempdir().unwrap();
    let dependency = project.path().join("dep");
    write_project(&dependency, "ruddy-entry", "0.0.0", &[]);
    fs::write(
        dependency.join("main.rud"),
        "extern write : String = \"'dependency'\"",
    )
    .unwrap();
    let app = project.path().join("app");
    executable_project(
        &app,
        "effect Console = { write: String -> (), write_error: String -> () }\nlet main = fn _ => !Console.write dep::write",
        None,
    );
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        format!("{manifest}dep = {{ bundle = \"ruddy-entry\", path = \"../dep\" }}\n"),
    )
    .unwrap();
    let built = build_project(&app).unwrap();
    let output = Command::new("node")
        .arg(built.with_extension("js"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"dependency");
}

fn error(directory: &TempDir) -> String {
    compile(directory.path())
        .expect_err("compilation fails")
        .to_string()
}

#[test]
fn inference_diagnostics_keep_structured_parity_across_real_consumers() {
    let source = include_str!("../diagnostics/inference/rigid-field-struct.rud");
    let directory = tempfile::tempdir().unwrap();
    write_project(directory.path(), "diagnostics", "0.1.0", &[]);
    fs::write(directory.path().join("main.rud"), source).unwrap();

    let failure = compile(directory.path()).expect_err("the fixture does not type-check");
    let [cli] = failure.diagnostics() else {
        panic!("expected one CLI diagnostic: {failure}");
    };
    let snapshot = debug_compile(
        &CompileRequest {
            kind: ruddy::artifact::Kind::Library,
            target: None,
            platform: None,
            name: "diagnostics".into(),
            version: "0.1.0".into(),
            root: DEBUG_ROOT.into(),
            document: "diagnostics".into(),
            files: vec![FileSpec {
                path: DEBUG_ROOT.into(),
                source: source.into(),
            }],
            std: StdConfig::Disabled,
            dependencies: indexmap::IndexMap::new(),
            revision: 0,
        },
        1,
    );
    let [debugger] = snapshot.diagnostics.as_slice() else {
        panic!(
            "expected one debugger diagnostic: {:#?}",
            snapshot.diagnostics
        );
    };

    assert_eq!(cli.stage(), debugger.stage);
    assert_eq!(cli.code(), debugger.code);
    assert_eq!(cli.message(), debugger.message);
    assert_eq!(cli.help(), debugger.help);
    assert_eq!(cli.notes(), debugger.notes);
    for prose in std::iter::once(debugger.message.as_str())
        .chain(std::iter::once(debugger.label.as_str()))
        .chain(debugger.help.iter().map(String::as_str))
        .chain(
            debugger
                .related
                .iter()
                .map(|related| related.message.as_str()),
        )
    {
        assert!(!prose.to_lowercase().contains("rigid"), "{prose}");
    }
    let explanation = debugger
        .inference_explanation
        .as_ref()
        .expect("caller-choice diagnostics are structured");
    assert_eq!(explanation.contradiction.kind, "caller-choice");
    assert!((2..=4).contains(&explanation.abridged.len()));
    assert!(
        explanation
            .full
            .iter()
            .any(|fact| fact.payload == "caller-choice-declaration")
    );
    assert!(
        explanation
            .full
            .iter()
            .any(|fact| fact.payload == "caller-choice-use")
    );

    // The CLI's public adapter renders labels while the debugger keeps them as
    // wire fields. Ensure those fields came through both real consumers too.
    let rendered = cli.render(false);
    assert!(rendered.contains(&debugger.label), "{rendered}");
    for related in &debugger.related {
        assert!(rendered.contains(&related.message), "{rendered}");
    }
}

#[test]
fn shared_std_configuration_accepts_only_false_or_dependency_syntax() {
    let disabled: ruddy_cli::StdConfig = toml::Value::Boolean(false).try_into().unwrap();
    assert!(disabled.is_disabled());

    let path: ruddy_cli::StdConfig = toml::Value::String("vendor/std".into()).try_into().unwrap();
    assert_eq!(
        path.dependency().and_then(ruddy_cli::DependencySpec::path),
        Some(Path::new("vendor/std"))
    );

    let enabled = toml::Value::Boolean(true)
        .try_into::<ruddy_cli::StdConfig>()
        .unwrap_err()
        .to_string();
    assert!(enabled.contains("`std = true` is invalid"), "{enabled}");
}

#[test]
fn std_manifest_forms_are_strict_and_contextual() {
    let directory = project();
    for (setting, expected) in [
        ("1", "invalid type"),
        ("[]", "invalid type"),
        ("{}", "[dependency-source-missing] Error"),
        (
            "{ path = \"std\", git = \"https://example.test/std\" }",
            "both `path` and `git`",
        ),
        (
            "{ path = \"std\", unknown = true }",
            "unknown field `unknown`",
        ),
        ("{ path = \"std\", bundle = 1 }", "invalid type"),
    ] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!(
                "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = {setting}\n"
            ),
        )
        .unwrap();
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
        assert!(!directory.path().join("Ruddy.lock").exists());
    }

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\nstd = false\n[dependencies]\n",
    )
    .unwrap();
    let found = error(&directory);
    assert!(found.contains("unknown field `std`"), "{found}");
}

#[test]
fn automatic_std_environment_behavior_is_isolated() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");

    for mode in ["default", "relative", "custom", "disabled"] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "cli::automatic_std_environment_child",
            ])
            .env("RUDDY_TEST_STD_MODE", mode)
            .env("RUDDY_TEST_STD_ROOT", parent.path())
            .current_dir(parent.path())
            .env_remove("HOME")
            .env_remove("RUDDY_HOME");
        if mode == "default" {
            command.env("HOME", &home);
        } else if mode == "relative" {
            command.env("RUDDY_HOME", "relative-home");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{mode} child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "run in isolation with controlled standard-library environment variables"]
fn automatic_std_environment_child() {
    let mode = std::env::var("RUDDY_TEST_STD_MODE").unwrap();
    let root = PathBuf::from(std::env::var_os("RUDDY_TEST_STD_ROOT").unwrap()).join(&mode);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.rud"), "let main = 0n\n").unwrap();

    match mode.as_str() {
        "default" | "relative" => {
            let home = ruddy_cli::ruddy_home().unwrap();
            let standard = crate::git_fixture::cache_std(&home, "let value = 0n\n");
            assert!(standard.is_absolute());
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\n",
            )
            .unwrap();

            let graph = ruddy_cli::compile_graph(&root).unwrap();
            assert_eq!(
                graph
                    .projects
                    .iter()
                    .map(|project| project.artifact.header().identity.name.as_str())
                    .collect::<Vec<_>>(),
                ["std", "app"]
            );
            assert_eq!(graph.projects[0].source, ruddy_cli::ProjectSource::GitCache);
            let artifact = build_project(&root).unwrap();
            assert_eq!(artifact, root.join("build/app.artifact"));
            assert!(!standard.join("build").exists());
            assert!(!home.join("std").exists());
            let lock: Lockfile =
                toml::from_str(&fs::read_to_string(root.join("Ruddy.lock")).unwrap()).unwrap();
            assert_eq!(lock.entries[0].url, ruddy_cli::DEFAULT_STD_GIT);
            // A new project reuses the selection and checkout without a lockfile.
            fs::remove_file(root.join("Ruddy.lock")).unwrap();
            compile(&root).unwrap();
            assert!(root.join("Ruddy.lock").exists());
        }
        "custom" => {
            write_project(&root.join("standard"), "std", "2.0.0", &[]);
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"standard\"\n",
            )
            .unwrap();
            assert_eq!(compile(&root).unwrap().header().identity.name, "app");
        }
        "disabled" => {
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
            )
            .unwrap();
            assert_eq!(compile(&root).unwrap().header().identity.name, "app");
        }
        _ => panic!("unexpected mode {mode}"),
    }
}

#[test]
fn automatic_std_is_independent_transitive_deduplicated_and_cycle_checked() {
    let parent = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "cli::automatic_std_graph_child"])
        .env("RUDDY_TEST_STD_ROOT", parent.path())
        .env("RUDDY_HOME", parent.path().join("ruddy-home"))
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with a dedicated default standard library"]
fn automatic_std_graph_child() {
    let root = PathBuf::from(std::env::var_os("RUDDY_TEST_STD_ROOT").unwrap());
    let home = PathBuf::from(std::env::var_os("RUDDY_HOME").unwrap());
    let standard = crate::git_fixture::cache_std(&home, "let value = 0n\n");

    let dedup = root.join("dedup");
    write_project(&dedup.join("dep"), "dep", "1.0.0", &[]);
    fs::write(
        dedup.join("dep/Ruddy.toml"),
        "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\n",
    )
    .unwrap();
    fs::create_dir_all(&dedup).unwrap();
    fs::write(dedup.join("main.rud"), "let main = 0n\n").unwrap();
    fs::write(
        dedup.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\ndep = \"dep\"\n",
    )
    .unwrap();
    let graph = ruddy_cli::compile_graph(&dedup).unwrap();
    assert_eq!(
        graph
            .projects
            .iter()
            .map(|project| project.artifact.header().identity.name.as_str())
            .collect::<Vec<_>>(),
        ["std", "dep", "app"]
    );
    assert_eq!(
        graph.projects[1].artifact.header().dependencies[0].name,
        "std"
    );
    assert_eq!(
        graph.projects[2]
            .artifact
            .header()
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect::<Vec<_>>(),
        ["std", "dep"]
    );

    let versions = root.join("versions");
    write_project(&versions.join("std2"), "std", "2.0.0", &[]);
    write_project(&versions.join("dep"), "dep", "1.0.0", &[]);
    fs::write(
        versions.join("dep/Ruddy.toml"),
        "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"../std2\"\n",
    )
    .unwrap();
    fs::write(versions.join("main.rud"), "let main = 0n\n").unwrap();
    fs::write(
        versions.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\ndep = \"dep\"\n",
    )
    .unwrap();
    let graph = ruddy_cli::compile_graph(&versions).unwrap();
    let std_versions = graph
        .projects
        .iter()
        .filter(|project| project.artifact.header().identity.name == "std")
        .map(|project| project.artifact.header().identity.version.as_str())
        .collect::<Vec<_>>();
    assert_eq!(std_versions, ["1.0.0", "2.0.0"]);

    fs::write(
        versions.join("std2/Ruddy.toml"),
        "name = \"std\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let found = compile(&versions).unwrap_err().to_string();
    assert!(
        found.contains("[project-identity-conflict] Error") && found.contains("`std@1.0.0`"),
        "{found}"
    );

    let cycle = root.join("cycle");
    let local_std = root.join("cycle-std");
    write_project(
        &local_std,
        "std",
        "1.0.0",
        &[("app", cycle.to_str().unwrap())],
    );
    write_project(&cycle, "app", "1.0.0", &[]);
    let manifest = fs::read_to_string(cycle.join("Ruddy.toml")).unwrap();
    fs::write(
        cycle.join("Ruddy.toml"),
        manifest.replace("std = false", &format!("std = {local_std:?}")),
    )
    .unwrap();
    let found = compile(&cycle).unwrap_err().to_string();
    assert!(found.contains("dependency cycle"), "{found}");
    assert!(standard.is_dir());
}

#[test]
fn configured_std_is_injected_first_and_is_source_visible() {
    let directory = project();
    write_project(
        &directory.path().join("standard"),
        "foundation",
        "2.1.0",
        &[],
    );
    fs::write(
        directory.path().join("standard/main.rud"),
        "module prelude =\n  let answer = 42n\nend\n",
    )
    .unwrap();
    fs::write(directory.path().join("main.rud"), "let main = answer\n").unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = { path = \"standard\", bundle = \"foundation\" }\n",
    )
    .unwrap();

    let graph = ruddy_cli::compile_graph(directory.path()).unwrap();
    assert_eq!(
        graph
            .projects
            .iter()
            .map(|project| project.artifact.header().identity.name.as_str())
            .collect::<Vec<_>>(),
        ["foundation", "app"]
    );
    assert_eq!(
        graph.projects[1].artifact.header().dependencies[0].name,
        "foundation"
    );
    assert!(
        compile(directory.path())
            .unwrap()
            .print()
            .contains("foundation@2.1.0::prelude::answer")
    );
}

#[test]
fn disabled_std_does_not_open_an_implicit_prelude() {
    let directory = project();
    fs::write(directory.path().join("main.rud"), "let main = answer\n").unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let found = error(&directory);
    assert!(found.contains("[undefined-term] Error"), "{found}");
    assert!(found.contains("cannot find value `answer`"), "{found}");
}

#[test]
fn duplicate_std_settings_have_a_focused_error() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nstd = \"vendor/std\"\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(error.contains("duplicate key"), "{error}");
}

#[test]
fn manifest_dependencies_reach_the_artifact_in_declaration_order() {
    let directory = project();
    write_project(&directory.path().join("zeta"), "zeta", "2.0.0", &[]);
    write_project(
        &directory.path().join("alpha"),
        "alpha",
        "1.2.3-beta.1",
        &[],
    );
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nzeta = \"zeta\"\nalpha = \"alpha\"\n",
    )
    .expect("write the manifest");

    let graph = ruddy_cli::compile_graph(directory.path()).expect("compile the project graph");
    let identities: Vec<_> = graph
        .projects
        .last()
        .unwrap()
        .artifact
        .header()
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency.version.as_str()))
        .collect();
    assert_eq!(identities, [("zeta", "2.0.0"), ("alpha", "1.2.3-beta.1")]);
    let built = compile(directory.path()).unwrap();
    assert_eq!(built.header().identity.name, "app");
    assert!(built.header().dependencies.is_empty());
    assert!(!directory.path().join("zeta/build/zeta.artifact").exists());
    assert!(!directory.path().join("alpha/build/alpha.artifact").exists());
}

#[test]
fn direct_dependency_exports_resolve_and_keep_their_artifact_owner() {
    let directory = project();
    let dependency = directory.path().join("std");
    write_project(&dependency, "std", "0.1.0", &[]);
    fs::write(
        dependency.join("main.rud"),
        "module Nested =\n  type Number = Nat\n  effect Read = { get: {} -> Nat }\n  extern runtime : Nat = \"host.runtime\"\n  let foo = 1n\nend\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "let value : std::Nested::Number = std::Nested::foo\nlet imported = std::Nested::runtime\nlet operation = std::Nested::!Read.get\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();

    let built = compile(directory.path()).expect("dependency members resolve");
    let printed = built.print();
    assert!(printed.contains("std@0.1.0::Nested::foo"), "{printed}");
    assert!(printed.contains("std@0.1.0::Nested::Number"), "{printed}");
    assert_eq!(built.lir().externs.len(), 1);
    assert_eq!(built.lir().externs[0].name, "std@0.1.0::Nested::runtime");
    assert_eq!(built.lir().externs[0].target, "host.runtime");
    assert_eq!(built.header().values.len(), 3);
    assert_eq!(built.header().types.len(), 1);
    assert_eq!(built.header().types[0].name, "std@0.1.0::Nested::Number");
    assert!(!built.header().types[0].exported);
    assert_eq!(built.header().effects.len(), 1);
    assert_eq!(built.header().effects[0].name, "std@0.1.0::Nested::Read");
    assert!(!built.header().effects[0].exported);
    Artifact::try_parse(&printed).unwrap().validate().unwrap();
}

#[test]
fn detailed_dependencies_alias_hyphenated_bundle_identities() {
    let directory = project();
    let dependency = directory.path().join("http-core");
    write_project(&dependency, "http-core", "1.0.0", &[]);
    fs::write(dependency.join("main.rud"), "let status = 200n\n").unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "let main = http_core::status\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nhttp_core = { bundle = \"http-core\", path = \"http-core\" }\n",
    )
    .unwrap();

    let graph = ruddy_cli::compile_graph(directory.path()).expect("the source alias resolves");
    assert_eq!(
        graph
            .projects
            .last()
            .unwrap()
            .artifact
            .header()
            .dependencies[0]
            .name,
        "http-core"
    );
    assert!(
        compile(directory.path())
            .unwrap()
            .header()
            .dependencies
            .is_empty()
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nhttp_core = { package = \"http-core\", path = \"http-core\" }\n",
    )
    .unwrap();
    let old_field_error = error(&directory);
    assert!(
        old_field_error.contains("unknown field `package`"),
        "{old_field_error}"
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nhttp-core = \"http-core\"\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(
        error.contains("[dependency-alias-invalid] Error"),
        "{error}"
    );
}

#[test]
fn transitive_dependencies_are_linkable_but_not_source_visible() {
    let directory = project();
    write_project(&directory.path().join("base"), "base", "1.0.0", &[]);
    fs::write(
        directory.path().join("base/main.rud"),
        "type Number = Nat\nlet foo : Number = 1n\n",
    )
    .unwrap();
    write_project(
        &directory.path().join("std"),
        "std",
        "1.0.0",
        &[("base", "../base")],
    );
    fs::write(
        directory.path().join("std/main.rud"),
        "let foo = base::foo\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "let main : Nat = std::foo\n",
    )
    .unwrap();
    let built = compile(directory.path()).expect("direct export backed by transitive global");
    assert!(built.print().contains("std@1.0.0::foo"));

    fs::write(directory.path().join("main.rud"), "let main = base::foo\n").unwrap();
    let error = error(&directory);
    assert!(error.contains("[undefined-module] Error"), "{error}");
    assert!(error.contains("cannot find module `base`"), "{error}");
}

#[test]
fn missing_dependency_paths_report_the_requested_namespace() {
    let directory = project();
    write_project(&directory.path().join("std"), "std", "1.0.0", &[]);
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "let a = std::missing\nlet b : std::Missing = 1n\nlet c = std::!MissingEffect.op\nlet d = std::NoModule::x\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(
        error.contains("[undefined-term] Error: cannot find value `missing`"),
        "{error}"
    );
    assert!(
        error.contains("[undefined-type] Error: cannot find type `Missing`"),
        "{error}"
    );
    assert!(
        error.contains("[undefined-effect] Error: cannot find effect `MissingEffect`"),
        "{error}"
    );
    assert!(
        error.contains("[undefined-module] Error: cannot find module `NoModule`"),
        "{error}"
    );
}

#[test]
fn a_local_module_cannot_shadow_a_direct_dependency_root() {
    let directory = project();
    write_project(&directory.path().join("std"), "std", "1.0.0", &[]);
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "module std = let local = 1n end\nlet main = 0n\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(error.contains("[duplicate-module] Error"), "{error}");
    assert!(error.contains("`std` is defined more than once"), "{error}");
}

#[test]
fn the_configured_root_is_resolved_relative_to_the_manifest() {
    let directory = project();
    fs::create_dir(directory.path().join("src")).expect("create source directory");
    fs::rename(
        directory.path().join("main.rud"),
        directory.path().join("src/app.rud"),
    )
    .expect("move the root");
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"src/app.rud\"\n[dependencies]\nstd = false\n",
    )
    .expect("write the manifest");

    let built = compile(directory.path()).expect("compile the configured root");
    assert!(built.header().dependencies.is_empty());
    assert_eq!(
        Artifact::try_parse(&built.print())
            .unwrap()
            .validate()
            .unwrap(),
        built
    );
}

#[test]
fn nested_root_diagnostics_preserve_root_and_module_paths() {
    let directory = project();
    fs::create_dir(directory.path().join("src")).expect("create source directory");
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"src/app.rud\"\n[dependencies]\nstd = false\n",
    )
    .expect("write the manifest");
    fs::write(
        directory.path().join("src/app.rud"),
        "let bad : Nat = fn x => x\n",
    )
    .expect("write an invalid root");

    let root_error = error(&directory).replace('\\', "/");
    assert!(root_error.contains("src/app.rud:1:"), "{root_error}");

    fs::write(directory.path().join("src/app.rud"), "module Child\n").expect("replace the root");
    fs::write(
        directory.path().join("src/Child.rud"),
        "let bad : Nat = fn x => x\n",
    )
    .expect("write an invalid module");

    let module_error = error(&directory).replace('\\', "/");
    assert!(module_error.contains("src/Child.rud:1:"), "{module_error}");

    fs::remove_file(directory.path().join("src/Child.rud")).expect("remove the module file");
    let missing = error(&directory).replace('\\', "/");
    assert!(missing.contains("[module-file-missing] Error"), "{missing}");
    assert!(missing.contains("this module needs a file"), "{missing}");
    assert!(
        missing.contains("no file was found for this module"),
        "{missing}"
    );
    assert!(
        missing.contains("create `src/Child.rud` or `src/Child/module.rud`"),
        "{missing}",
    );

    fs::write(directory.path().join("src/Child.rud"), "let beside = 1n\n")
        .expect("write the beside candidate");
    fs::create_dir(directory.path().join("src/Child")).expect("create module directory");
    fs::write(
        directory.path().join("src/Child/module.rud"),
        "let inside = 1n\n",
    )
    .expect("write the inside candidate");
    let ambiguous = error(&directory).replace('\\', "/");
    assert!(
        ambiguous.contains("[module-file-ambiguous] Error")
            && ambiguous.contains("this module has two possible files"),
        "{ambiguous}"
    );
    assert!(
        ambiguous
            .contains("keep one of `src/Child.rud` or `src/Child/module.rud` and delete the other"),
        "{ambiguous}",
    );
}

#[test]
fn the_manifest_is_required_and_must_be_valid_and_supported() {
    let directory = project();
    let missing = error(&directory);
    assert!(missing.contains("[manifest-unreadable] Error"), "{missing}");
    assert!(missing.contains("Ruddy.toml"), "{missing}");

    for (manifest, expected) in [
        ("", "missing field `name`"),
        ("[dependencies]\n", "missing field `name`"),
        ("name = \"app\"\n", "missing field `version`"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\n",
            "missing field `root`",
        ),
        (
            "name = 1\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = 1\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = 1\n[dependencies]\n",
            "invalid type",
        ),
        ("[dependencies", "[manifest-invalid] Error"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntitle = \"app\"\n[dependencies]\nstd = false\n",
            "unknown field `title`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[run]\njs = 1\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[run]\njs = \"node\"\nnative = \"app\"\n[dependencies]\nstd = false\n",
            "unknown field `native`",
        ),
        ("title = \"app\"\n", "unknown field `title`"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { source = \"base.artifact\" }\n",
            "unknown field `source`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { version = 1, source = \"base.artifact\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\", source = 1 }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\", source = \"base.artifact\", registry = \"x\" }\n",
            "unknown field `version`",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.toml"), manifest).expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = [\"https://user:secret@example.test/repo\"] }\n",
    )
    .unwrap();
    let redacted = error(&directory);
    assert!(redacted.contains("[manifest-invalid] Error"), "{redacted}");
    assert!(!redacted.contains("user:secret"), "{redacted}");
}

#[test]
fn git_dependency_manifest_validation_is_strict_and_contextual() {
    let directory = project();
    for (specification, expected) in [
        (
            "{ git = \"http://example.test/repo\" }",
            "[dependency-git-not-https] Error",
        ),
        (
            "{ git = \"ssh://example.test/repo\" }",
            "[dependency-git-not-https] Error",
        ),
        (
            "{ path = \"dep\", git = \"https://example.test/repo\" }",
            "[dependency-source-conflict] Error",
        ),
        ("{ bundle = \"base\" }", "[dependency-source-missing] Error"),
        (
            "{ path = \"dep\", branch = \"main\" }",
            "[dependency-selector-without-git] Error",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"\" }",
            "[dependency-selector-empty] Error",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"main\", tag = \"v1\" }",
            "[dependency-selectors-conflict] Error",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"bad..name\" }",
            "[dependency-selector-invalid] Error",
        ),
        (
            "{ git = \"https://example.test/repo\", unknown = true }",
            "unknown field `unknown`",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = {specification}\n")).unwrap();
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
        if !expected.starts_with("unknown field") {
            assert!(found.contains("dependency `base`"), "{found}");
        }
        assert!(!directory.path().join("Ruddy.lock").exists());
    }

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = \"http://user:secret@example.test/repo?token=hidden\" }\n",
    )
    .unwrap();
    let redacted = error(&directory);
    assert!(!redacted.contains("user:secret"), "{redacted}");
    assert!(!redacted.contains("token=hidden"), "{redacted}");
    assert!(
        redacted.contains("this Git dependency URL must use HTTPS"),
        "{redacted}"
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = \"user:secret@example.test/repo\" }\n",
    )
    .unwrap();
    let malformed = error(&directory);
    assert!(!malformed.contains("user:secret"), "{malformed}");
    assert!(
        malformed.contains("[dependency-git-not-https] Error"),
        "{malformed}"
    );
}

#[test]
fn ambient_git_configuration_cannot_rewrite_https_to_an_unsafe_transport() {
    let parent = tempfile::tempdir().unwrap();
    let app = parent.path().join("app");
    let home = parent.path().join("home");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(app.join("main.rud"), "let main = 0n\n").unwrap();
    fs::write(app.join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = \"https://example.invalid/repository\" }\n").unwrap();
    let config = home.join("hostile.gitconfig");
    fs::write(
        &config,
        "[url \"file:///tmp/hostile/\"]\n\tinsteadOf = https://example.invalid/\n",
    )
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "cli::hostile_git_configuration_child",
        ])
        .env("RUDDY_TEST_GIT_APP", &app)
        .env("RUDDY_HOME", parent.path().join("ruddy-home"))
        .env("HOME", &home)
        .env("GIT_CONFIG_GLOBAL", &config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in an isolated process with hostile Git configuration"]
fn hostile_git_configuration_child() {
    let app = PathBuf::from(std::env::var_os("RUDDY_TEST_GIT_APP").unwrap());
    let found = compile(app).unwrap_err().to_string();
    assert!(
        found.contains("rewrote the dependency URL to a non-HTTPS address"),
        "{found}"
    );
}

#[test]
fn ruddy_home_uses_an_explicit_override_or_defaults_to_dot_ruddy() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");
    let explicit = parent.path().join("explicit-ruddy-home");
    let default = home.join(".ruddy");
    let xdg = parent.path().join("xdg-must-be-ignored");
    for (override_home, expected) in [
        (Some(explicit.as_path()), explicit.as_path()),
        (Some(Path::new("")), default.as_path()),
        (None, default.as_path()),
    ] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", "cli::ruddy_home_layout_child"])
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", &xdg)
            .env("RUDDY_TEST_EXPECTED_HOME", expected);
        match override_home {
            Some(path) => {
                command.env("RUDDY_HOME", path);
            }
            None => {
                command.env_remove("RUDDY_HOME");
            }
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "cli::ruddy_home_layout_child"])
        .env("RUDDY_HOME", "")
        .env_remove("HOME")
        .env("XDG_CACHE_HOME", &xdg)
        .env_remove("RUDDY_TEST_EXPECTED_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with controlled home environment variables"]
fn ruddy_home_layout_child() {
    match std::env::var_os("RUDDY_TEST_EXPECTED_HOME") {
        Some(expected) => assert_eq!(ruddy_cli::ruddy_home().unwrap(), PathBuf::from(expected)),
        None => assert!(
            ruddy_cli::ruddy_home()
                .unwrap_err()
                .to_string()
                .contains("[ruddy-home-unavailable] Error")
        ),
    }
}

#[test]
fn https_fetch_failures_are_contextual_and_do_not_create_a_lockfile() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = \"https://127.0.0.1:9/repository\", branch = \"main\" }\n").unwrap();
    let found = error(&directory);
    assert!(found.contains("dependency `base`"), "{found}");
    assert!(
        found.contains("could not fetch or check out Git dependency"),
        "{found}"
    );
    assert!(!directory.path().join("Ruddy.lock").exists());
}

#[test]
fn a_locked_cached_git_dependency_builds_offline_and_reuses_its_commit() {
    let parent = tempfile::tempdir().unwrap();
    let app = parent.path().join("app");
    let home = parent.path().join("ruddy-home");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "cli::locked_cached_git_dependency_child",
        ])
        .env("RUDDY_TEST_GIT_APP", &app)
        .env("RUDDY_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with a dedicated RUDDY_HOME"]
fn locked_cached_git_dependency_child() {
    let app = PathBuf::from(std::env::var_os("RUDDY_TEST_GIT_APP").unwrap());
    let home = PathBuf::from(std::env::var_os("RUDDY_HOME").unwrap());
    let url = "https://offline.invalid/base.git";
    let seed = home.join("seed");
    fs::create_dir_all(&seed).unwrap();
    let repository = gix::init(&seed).unwrap();
    let manifest = repository
        .write_blob(b"name = \"base\"\nversion = \"2.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n")
        .unwrap()
        .detach();
    let source = repository.write_blob(b"let value = 2n\n").unwrap().detach();
    let tree = repository
        .write_object(gix::objs::Tree {
            entries: vec![
                gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: "Ruddy.toml".into(),
                    oid: manifest,
                },
                gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: "main.rud".into(),
                    oid: source,
                },
            ],
        })
        .unwrap()
        .detach();
    let signature = gix::actor::Signature {
        name: "Ruddy Tests".into(),
        email: "test@example.invalid".into(),
        time: gix::date::Time::default(),
    };
    let id = repository
        .write_object(gix::objs::Commit {
            tree,
            parents: Default::default(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: "fixture".into(),
            extra_headers: Vec::new(),
        })
        .unwrap()
        .detach();
    repository
        .reference(
            "HEAD",
            id,
            gix::refs::transaction::PreviousValue::Any,
            "test pin",
        )
        .unwrap();
    drop(repository);
    let commit = id.to_hex().to_string();
    let key = format!("{url}{:?}", ruddy_cli::GitSelector::Branch("main"));
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let checkout = home
        .join("cache/git/checkouts")
        .join(format!("{hash:016x}"))
        .join(&commit);
    fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    fs::rename(seed, &checkout).unwrap();
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("main.rud"), "let main = base::value\n").unwrap();
    fs::write(app.join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = {{ git = {url:?}, branch = \"main\" }}\n")).unwrap();
    let uppercase = commit.to_ascii_uppercase();
    let lock = format!(
        "version = 1\n\n[[git]]\nurl = {url:?}\nbranch = \"main\"\ncommit = {uppercase:?}\n"
    );
    fs::write(app.join("Ruddy.lock"), &lock).unwrap();
    let stale = home
        .join("cache/git/tmp")
        .join(format!("{hash:016x}-stale-crashed-process"));
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("partial"), "incomplete clone").unwrap();
    fs::write(home.join("cache/git/cache.lock"), "left behind by a crash").unwrap();
    let first = compile(&app).unwrap();
    assert!(!stale.exists());
    assert!(first.header().dependencies.is_empty());

    // Every use restores both tracked and untracked cache contents while the
    // cross-process cache lock remains held for compilation.
    fs::write(
        checkout.join("Ruddy.toml"),
        "name = \"poison\"\nversion = \"9.9.9\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(
        checkout.join("main.rud"),
        "mod injected\nlet value = injected::value\n",
    )
    .unwrap();
    fs::write(checkout.join("injected.rud"), "let value = false\n").unwrap();
    let second = compile(&app).unwrap();
    assert_eq!(first, second);
    assert!(!checkout.join("injected.rud").exists());
    assert_eq!(
        fs::read_to_string(checkout.join("main.rud")).unwrap(),
        "let value = 2n\n"
    );
    assert!(
        fs::read_to_string(checkout.join("Ruddy.toml"))
            .unwrap()
            .starts_with("name = \"base\"")
    );
    assert!(
        fs::read_to_string(app.join("Ruddy.lock"))
            .unwrap()
            .contains(&format!("commit = {commit:?}"))
    );

    let concurrent: Vec<_> = (0..8)
        .map(|_| {
            let app = app.clone();
            std::thread::spawn(move || compile(app).unwrap())
        })
        .collect();
    for handle in concurrent {
        assert_eq!(handle.join().unwrap(), first);
    }
    assert!(
        fs::read_dir(home.join("cache/git/tmp"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    );

    let graph = ruddy_cli::compile_graph(&app).unwrap();
    assert_eq!(graph.projects.len(), 2);
    assert_eq!(graph.projects[0].source, ruddy_cli::ProjectSource::GitCache);
    assert_eq!(graph.projects[1].source, ruddy_cli::ProjectSource::Local);
    let artifact = ruddy_cli::build_project(&app).unwrap();
    assert!(artifact.is_file());
    assert!(!checkout.join("build").exists());

    // Editor sessions reuse acquired source selections and can abandon a new
    // acquisition while another compiler holds the global cache lock.
    let mut workspace = ruddy_cli::workspace::Workspace::new(app.clone());
    workspace.refresh().unwrap();
    let cache_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.join("cache/git/cache.lock"))
        .unwrap();
    cache_lock.lock().unwrap();
    let cancel = ruddy::cancellation::Cancellation::default();
    let timer = cancel.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        timer.cancel();
    });
    workspace.set_overlay(
        &app.join("main.rud"),
        Some("let main = base::value\nlet extra = false".into()),
    );
    assert!(
        cancel.run(|| workspace.refresh()).unwrap().is_ok(),
        "ordinary edits must not reacquire the Git cache lock"
    );
    timer.join().unwrap();
    let mut fresh = ruddy_cli::workspace::Workspace::new(app.clone());
    let cancel = ruddy::cancellation::Cancellation::default();
    let timer = cancel.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        timer.cancel();
    });
    let started = std::time::Instant::now();
    assert!(cancel.run(|| fresh.refresh()).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    timer.join().unwrap();
    drop(cache_lock);
    fresh.refresh().unwrap();
    assert!(fresh.check_background().is_empty());

    // Cache provenance follows the canonical location, even when a local path
    // specification reaches an already-seeded checkout instead of a Git spec.
    let path_app = home.join("path-app");
    fs::create_dir(&path_app).unwrap();
    fs::write(path_app.join("main.rud"), "let main = base::value\n").unwrap();
    fs::write(
        path_app.join("Ruddy.toml"),
        format!(
            "name = \"path-app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = {{ path = {:?} }}\n",
            checkout
        ),
    )
    .unwrap();
    let graph = ruddy_cli::compile_graph(&path_app).unwrap();
    assert_eq!(graph.projects[0].source, ruddy_cli::ProjectSource::GitCache);
    assert_eq!(graph.projects[1].source, ruddy_cli::ProjectSource::Local);
    assert!(ruddy_cli::build_project(&path_app).unwrap().is_file());
    assert!(!checkout.join("build").exists());

    // An explicitly invoked root remains buildable, even when it is located
    // beneath the cache root.
    let checkout_artifact = ruddy_cli::build_project(&checkout).unwrap();
    assert_eq!(checkout_artifact, checkout.join("build/base.artifact"));
    assert!(checkout_artifact.is_file());
}

#[test]
fn exact_revisions_require_unambiguous_hex_prefixes_before_network_access() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = { git = \"https://127.0.0.1:9/repository\", rev = \"abc123\" }\n").unwrap();
    let found = error(&directory);
    assert!(found.contains("7 to 40 hexadecimal characters"), "{found}");
    assert!(!directory.path().join("Ruddy.lock").exists());
}

#[test]
fn a_successful_build_removes_stale_git_entries_from_an_existing_lockfile() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.lock"),
        "version = 1\n\n[[git]]\nurl = \"https://example.test/repo\"\nbranch = \"main\"\ncommit = \"0123456789abcdef0123456789abcdef01234567\"\n",
    )
    .unwrap();
    compile(directory.path()).unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join("Ruddy.lock")).unwrap(),
        "version = 1\n"
    );
}

#[test]
fn abbreviated_revision_lock_entries_accept_the_matching_full_commit() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.lock"),
        "version = 1\n\n[[git]]\nurl = \"https://example.test/repo\"\nrev = \"0123456\"\ncommit = \"0123456789abcdef0123456789abcdef01234567\"\n",
    )
    .unwrap();
    compile(directory.path()).unwrap();
}

#[test]
fn malformed_and_unsupported_lockfiles_are_diagnosed_without_replacement() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    for (source, code) in [
        ("not toml =", "lockfile-invalid"),
        ("version = 2\n", "lockfile-version-unsupported"),
        (
            "version = 1\n[[git]]\nurl = \"https://example.test/repo\"\ncommit = \"short\"\n",
            "lockfile-invalid-commit",
        ),
        (
            "version = 1\n[[git]]\nurl = \"https://example.test/repo\"\nbranch = \"main\"\ntag = \"v1\"\ncommit = \"0000000000000000000000000000000000000000\"\n",
            "lockfile-selectors-conflict",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.lock"), source).unwrap();
        let found = error(&directory);
        assert!(found.contains(&format!("[{code}] Error")), "{found}");
        assert_eq!(
            fs::read_to_string(directory.path().join("Ruddy.lock")).unwrap(),
            source
        );
    }
}

#[test]
fn public_lockfile_format_round_trips_deterministically() {
    let source = "version = 1\n\n[[git]]\nurl = \"https://example.test/repo\"\nbranch = \"main\"\ncommit = \"0123456789abcdef0123456789abcdef01234567\"\n";
    let lock: Lockfile = toml::from_str(source).unwrap();
    assert_eq!(lock.version, 1);
    assert_eq!(lock.entries[0].selector.branch.as_deref(), Some("main"));
    assert_eq!(
        toml::from_str::<Lockfile>(&toml::to_string_pretty(&lock).unwrap()).unwrap(),
        lock
    );
}

#[test]
fn manifest_bundle_identity_must_be_valid() {
    let directory = project();
    for (name, version, expected) in [
        ("app", "not-semver", "[project-version-invalid] Error"),
        ("not.a.name", "1.0.0", "[project-name-invalid] Error"),
        ("app", "1.0.0+local", "[project-version-build-suffix] Error"),
    ] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!("name = {name:?}\nversion = {version:?}\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n"),
        )
        .expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn dependency_projects_must_exist_compile_and_match_the_table_key() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = \"missing\"\n").unwrap();
    assert!(error(&directory).contains("dependency `base`"));

    write_project(&directory.path().join("child"), "other", "1.0.0", &[]);
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nbase = \"child\"\n").unwrap();
    let mismatch = error(&directory);
    assert!(
        mismatch.contains("[dependency-name-mismatch] Error")
            && mismatch.contains("declares `other`"),
        "{mismatch}"
    );

    write_project(&directory.path().join("child"), "base", "1.0.0", &[]);
    fs::write(
        directory.path().join("child/main.rud"),
        "let bad : Nat = fn x => x\n",
    )
    .unwrap();
    let diagnostics = compile(directory.path()).expect_err("the dependency does not type-check");
    assert_eq!(diagnostics.diagnostics().len(), 1, "{diagnostics}");
    assert_eq!(diagnostics.diagnostics()[0].code(), "type-mismatch");
    assert!(
        diagnostics.diagnostics()[0]
            .notes()
            .iter()
            .any(|note| note.contains("dependency `base`"))
    );
    let found = diagnostics.to_string();
    assert!(found.contains("dependency `base`"));
    assert!(found.contains("[type-mismatch] Error"), "{found}");
    assert!(!found.contains("error: dependency"), "{found}");
}

#[test]
fn transitive_diamond_graphs_are_unique_dependency_first_and_direct_only() {
    let root = tempfile::tempdir().unwrap();
    let shared = root.path().join("shared");
    let left = root.path().join("left");
    let right = root.path().join("right");
    let app = root.path().join("app");
    write_project(&shared, "shared", "3.0.0", &[]);
    write_project(&left, "left", "2.0.0", &[("shared", "../shared")]);
    write_project(&right, "right", "2.1.0", &[("shared", "../shared")]);
    write_project(
        &app,
        "app",
        "1.0.0",
        &[("left", "../left"), ("right", "../right")],
    );
    for (project, declaration) in [
        (
            &shared,
            "extern host : Nat = \"shared.host\"\nlet value = 0n\n",
        ),
        (&left, "extern host : Nat = \"left.host\"\nlet value = 0n\n"),
        (
            &right,
            "extern host : Nat = \"right.host\"\nlet value = 0n\n",
        ),
        (&app, "extern host : Nat = \"app.host\"\nlet value = 0n\n"),
    ] {
        fs::write(project.join("main.rud"), declaration).unwrap();
    }

    let graph = ruddy_cli::compile_graph(&app).unwrap();
    let names: Vec<_> = graph
        .projects
        .iter()
        .map(|project| project.artifact.header().identity.name.as_str())
        .collect();
    assert_eq!(names, ["shared", "left", "right", "app"]);
    assert_eq!(graph.projects[1].artifact.header().dependencies.len(), 1);
    assert_eq!(
        graph.projects[3]
            .artifact
            .header()
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );
    let (dependencies, direct) =
        ruddy_cli::compile_dependency_graph([("left", &left), ("right", &right)]).unwrap();
    assert_eq!(dependencies.projects.len(), 3);
    assert_eq!(
        dependencies
            .projects
            .iter()
            .map(|project| project.artifact.header().identity.name.as_str())
            .collect::<Vec<_>>(),
        ["shared", "left", "right"]
    );
    assert!(
        dependencies.projects[1]
            .artifact
            .header()
            .dependencies
            .len()
            == 1
    );
    assert!(
        dependencies.projects[2]
            .artifact
            .header()
            .dependencies
            .len()
            == 1
    );
    assert_eq!(
        direct
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );

    let path = build_project(&app).unwrap();
    assert_eq!(path, app.join("build/app.artifact"));
    let linked = Artifact::try_parse(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(linked.header.dependencies.is_empty());
    assert_eq!(linked.lir.globals.len(), 8);
    assert_eq!(
        linked
            .lir
            .externs
            .iter()
            .map(|external| (external.name.as_str(), external.target.clone()))
            .collect::<Vec<_>>(),
        [
            ("shared@3.0.0::host", "shared.host".to_string()),
            ("left@2.0.0::host", "left.host".to_string()),
            ("right@2.1.0::host", "right.host".to_string()),
            ("app@1.0.0::host", "app.host".to_string()),
        ]
    );
    for (dir, name) in [
        (&shared, "shared"),
        (&left, "left"),
        (&right, "right"),
        (&app, "app"),
    ] {
        assert!(dir.join(format!("build/{name}.artifact")).exists());
    }
}

#[test]
fn dependency_effects_aliases_and_constructor_kinds_survive_import() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("base");
    let middle = root.path().join("middle");
    let app = root.path().join("app");
    write_project(&base, "base", "1.0.0", &[]);
    fs::write(
        base.join("main.rud"),
        "effect Read = { get: {} -> Nat }\n\
         type Cases 'r = #A Nat | ..'r\n\
         let read : {} -> Nat + !Read = fn _ => !Read.get {}\n",
    )
    .unwrap();
    write_project(&middle, "middle", "1.0.0", &[("base", "../base")]);
    fs::write(
        middle.join("main.rud"),
        "effect Console = base::!Read\n\
         effect Services = !Console\n\
         let read : {} -> Nat + !Services = base::read\n",
    )
    .unwrap();
    write_project(
        &app,
        "app",
        "1.0.0",
        &[("base", "../base"), ("middle", "../middle")],
    );
    fs::write(
        app.join("main.rud"),
        "type F = {} -> Nat + base::!Read\n\
         let direct : F = base::read\n\
         let linked : {} -> Nat + middle::!Services = middle::read\n",
    )
    .unwrap();
    compile(&app)
        .expect("imported effects remain in declared types and aliases close transitively");

    fs::write(app.join("main.rud"), "type Bad = base::Cases Nat\n").unwrap();
    let error = compile(&app).unwrap_err().to_string();
    assert!(error.contains("not-a-row"), "{error}");

    fs::write(app.join("main.rud"), "type Bad = base::Cases (#A Nat)\n").unwrap();
    let error = compile(&app).unwrap_err().to_string();
    assert!(error.contains("repeated-row-field"), "{error}");
}

#[test]
fn distinct_projects_cannot_claim_one_bundle_identity() {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("left");
    let right = root.path().join("right");
    write_project(&left, "same", "1.0.0", &[]);
    write_project(&right, "same", "1.0.0", &[]);
    // Two separate direct roots allow the same manifest name to be requested
    // twice while preserving the dependency-key invariant.
    let error = ruddy_cli::compile_dependency_graph([("same", &left), ("same", &right)])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("[project-identity-conflict] Error") && error.contains("`same@1.0.0`"),
        "{error}"
    );
}

#[test]
fn imported_signature_types_participate_in_effect_identity() {
    let root = tempfile::tempdir().unwrap();
    let dep = root.path().join("dep");
    let app = root.path().join("app");
    write_project(&dep, "dep", "1.0.0", &[]);
    fs::write(dep.join("main.rud"), "type A = Nat\ntype B = String\n").unwrap();
    write_project(&app, "app", "1.0.0", &[("dep", "../dep")]);
    fs::write(
        app.join("main.rud"),
        "module X = effect Same = { op: dep::A -> {} } end\n\
         module Y = effect Same = { op: dep::B -> {} } end\n",
    )
    .unwrap();
    let artifact = compile(&app).unwrap();
    let identities: Vec<_> = artifact
        .header()
        .effects
        .iter()
        .filter_map(|effect| effect.identity.as_ref())
        .collect();
    assert_eq!(identities.len(), 2);
    assert_ne!(identities[0].interface, identities[1].interface);
}

#[test]
fn dependency_cycles_are_reported_and_failed_graphs_write_nothing() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    write_project(&a, "a", "1.0.0", &[("b", "../b")]);
    write_project(&b, "b", "1.0.0", &[("a", "../a")]);
    let found = ruddy_cli::compile(&a).unwrap_err().to_string();
    assert!(found.contains("dependency cycle"), "{found}");
    assert!(found.contains("a") && found.contains("b"), "{found}");
    assert!(!a.join("build").exists());
    assert!(!b.join("build").exists());

    fs::create_dir_all(a.join("build")).unwrap();
    fs::write(a.join("build/a.artifact"), "old").unwrap();
    assert!(build_project(&a).is_err());
    assert_eq!(
        fs::read_to_string(a.join("build/a.artifact")).unwrap(),
        "old"
    );
}

#[test]
fn bundle_and_compiler_failures_are_returned_as_cli_diagnostics() {
    let missing = tempfile::tempdir().unwrap();
    fs::write(
        missing.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"missing.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let root_error = compile(missing.path())
        .expect_err("the configured root is missing")
        .to_string();
    assert!(
        root_error.contains("[project-root-missing] Error") && root_error.contains("missing.rud"),
        "{root_error}"
    );

    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "let bad : Nat = fn x => x\n",
    )
    .unwrap();
    let compiler = error(&directory);
    assert!(compiler.contains("[type-mismatch] Error"), "{compiler}");
    assert!(compiler.contains("main.rud:1:"), "{compiler}");

    let diagnostics = compile(directory.path()).expect_err("the program has a type error");
    assert_eq!(diagnostics.messages().len(), 1, "{diagnostics}");
}

#[test]
fn frontend_errors_stop_compilation_without_parse_or_semantic_cascades() {
    let directory = project();
    write_project(directory.path(), "app", "1.0.0", &[]);
    fs::write(directory.path().join("main.rud"), "let n = 1x\n").unwrap();

    let diagnostics = compile(directory.path()).expect_err("the joined name is invalid");
    assert_eq!(diagnostics.messages().len(), 1, "{diagnostics}");
    let rendered = diagnostics.to_string();
    assert!(
        rendered.contains("[number-joined-to-name] Error"),
        "{rendered}"
    );
    assert!(!rendered.contains("expected-"), "{rendered}");
    assert!(!rendered.contains("[ir/"), "{rendered}");
    assert!(!rendered.contains("[types/"), "{rendered}");
}

#[test]
fn rendering_without_color_has_no_ansi_and_omits_the_internal_phase() {
    let sources = [ruddy_cli::DiagnosticSource {
        path: "main.rud",
        source: "let n = 1x\n",
    }];
    let primary = ruddy_cli::DiagnosticLabel {
        source: 0,
        range: 8..10,
        message: "the number and name are joined",
    };
    let rendered = ruddy_cli::render_diagnostic_with_advice(
        "lex",
        "number-joined-to-name",
        "a number cannot run directly into a name",
        &sources,
        Some(&primary),
        &[],
        Some("add a space"),
        &[],
        false,
    );

    assert!(!rendered.contains('\x1b'), "{rendered:?}");
    assert!(
        rendered.contains("[number-joined-to-name] Error"),
        "{rendered}"
    );
    assert!(!rendered.contains("[lex/"), "{rendered}");
    assert!(
        rendered.contains("a number cannot run directly into a name"),
        "{rendered}"
    );
    assert!(
        rendered.contains("the number and name are joined"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Help: add a space") || rendered.contains("help: add a space"),
        "{rendered}"
    );

    let plain = ruddy_cli::render_diagnostic_with_advice(
        "dependencies",
        "dependency-source-missing",
        "a dependency needs either a local `path` or a `git` URL",
        &[],
        None,
        &[],
        Some("add one source"),
        &["declared in `Ruddy.toml`"],
        false,
    );
    assert!(
        plain.starts_with("[dependency-source-missing] Error:"),
        "{plain}"
    );
    assert!(!plain.contains("error["), "{plain}");
    assert!(plain.contains("help: add one source"), "{plain}");
    assert!(plain.contains("note: declared in `Ruddy.toml`"), "{plain}");
}

#[test]
fn no_color_overrides_forced_cli_color() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "cli::no_color_child"])
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with controlled color environment variables"]
fn no_color_child() {
    let directory = project();
    write_project(directory.path(), "app", "1.0.0", &[]);
    fs::write(directory.path().join("main.rud"), "let n = 1x\n").unwrap();
    let rendered = error(&directory);
    assert!(!rendered.contains('\x1b'), "{rendered:?}");
    assert!(
        rendered.contains("[number-joined-to-name] Error"),
        "{rendered}"
    );
}

#[test]
fn compiler_diagnostics_treat_spans_as_byte_offsets() {
    let directory = project();
    write_project(directory.path(), "app", "1.0.0", &[]);
    fs::write(
        directory.path().join("main.rud"),
        "let café = 1n\nlet bad : Nat = false\n",
    )
    .unwrap();

    let compiler = error(&directory);
    assert!(compiler.contains("main.rud:2:11"), "{compiler}");
    assert!(compiler.contains("2 │let bad : Nat = false"), "{compiler}");
}

#[test]
fn the_configured_root_must_name_a_file() {
    let directory = tempfile::tempdir().unwrap();
    for root in ["", ".", "..", "src/", "src/.", "/"] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!("name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = {root:?}\n[dependencies]\nstd = false\n"),
        )
        .unwrap();
        let error = compile(directory.path())
            .expect_err("the root does not name a file")
            .to_string();
        assert!(
            error.contains("[project-root-invalid] Error"),
            "root `{root}`: {error}"
        );
    }
}

#[test]
fn new_scaffolds_a_compilable_project_without_overwriting() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("nested/my_app");

    new_project(&destination).expect("create the project");
    let scaffold_manifest = fs::read_to_string(destination.join("Ruddy.toml")).unwrap();
    assert_eq!(
        scaffold_manifest,
        "name = \"my_app\"\nversion = \"0.1.0\"\nkind = \"executable\"\nroot = \"main.rud\"\ntarget = \"js\"\n\n[dependencies]\n"
    );
    assert!(
        !scaffold_manifest
            .lines()
            .any(|line| line.starts_with("std ="))
    );
    assert_eq!(
        fs::read_to_string(destination.join("main.rud")).unwrap(),
        "let main = fn _ => ()\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join(".gitignore")).unwrap(),
        "/build/\n"
    );
    let repository = gix::open(&destination).expect("open the generated Git repository");
    assert_eq!(
        fs::canonicalize(repository.workdir().unwrap()).unwrap(),
        fs::canonicalize(&destination).unwrap()
    );
    assert_eq!(
        fs::canonicalize(repository.git_dir()).unwrap(),
        fs::canonicalize(destination.join(".git")).unwrap()
    );
    disable_std(&destination);
    assert_eq!(
        compile(&destination).unwrap().header().identity,
        ruddy::artifact::Identity {
            name: "my_app".into(),
            version: "0.1.0".into(),
        }
    );

    let error = new_project(&destination).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not create project directory")
    );
    assert_eq!(
        fs::read_to_string(destination.join("main.rud")).unwrap(),
        "let main = fn _ => ()\n"
    );
}

#[test]
fn new_reports_git_initialization_failure_and_leaves_the_scaffold() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("blocked_git");
    let config = parent.path().join("malformed-git-config");
    fs::write(&config, "[broken\n").unwrap();

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "cli::new_project_with_malformed_git_config_child",
        ])
        .env("RUDDY_TEST_NEW_PROJECT_DESTINATION", &destination)
        .env("GIT_CONFIG_GLOBAL", config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(destination.join("Ruddy.toml").is_file());
    assert!(destination.join("main.rud").is_file());
    assert!(destination.join(".gitignore").is_file());
}

#[test]
#[ignore = "run in isolation by new_reports_git_initialization_failure_and_leaves_the_scaffold"]
fn new_project_with_malformed_git_config_child() {
    let destination = std::env::var_os("RUDDY_TEST_NEW_PROJECT_DESTINATION")
        .map(std::path::PathBuf::from)
        .expect("child project destination");
    let error = new_project(&destination).unwrap_err().to_string();
    assert!(
        error.contains("could not initialize Git repository"),
        "{error}"
    );
    assert!(error.contains("blocked_git"), "{error}");
}

#[test]
fn new_validates_the_bundle_name_and_reports_creation_failures() {
    let parent = tempfile::tempdir().unwrap();
    for name in ["3bad", "bad.name", "naïve"] {
        let error = new_project(parent.path().join(name))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a valid Ruddy bundle name"), "{error}");
        assert!(!parent.path().join(name).exists());
    }

    let nameless = new_project(Path::new("/")).unwrap_err().to_string();
    assert!(nameless.contains("has no valid bundle name"), "{nameless}");

    let blocking_file = parent.path().join("file");
    fs::write(&blocking_file, "blocking parent").unwrap();
    let error = new_project(blocking_file.join("child"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("could not create parent directory"),
        "{error}"
    );
}

#[test]
fn build_writes_and_replaces_the_named_canonical_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("sample");
    new_project(&project).unwrap();
    disable_std(&project);
    let manifest = fs::read_to_string(project.join("Ruddy.toml")).unwrap();
    fs::write(
        project.join("Ruddy.toml"),
        manifest.replace("target = \"js\"", "target = \"artifact\""),
    )
    .unwrap();

    let path = build_project(&project).expect("build project");
    assert_eq!(path, project.join("build/sample.artifact"));
    let first = fs::read_to_string(&path).unwrap();
    assert_eq!(
        Artifact::try_parse(&first)
            .unwrap()
            .validate()
            .unwrap()
            .print(),
        first
    );

    fs::write(&path, "old output").unwrap();
    assert_eq!(build_project(&project).unwrap(), path);
    assert_eq!(fs::read_to_string(&path).unwrap(), first);
    let entries: Vec<_> = fs::read_dir(project.join("build"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from("sample.artifact")]);
}

#[test]
fn manifest_targets_select_root_javascript_output() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    new_project(&app).unwrap();
    disable_std(&app);
    fs::write(app.join("main.rud"), "let main: () -> () = fn _ => ()").unwrap();

    // Omission and an explicit library target retain artifact-only behavior.
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest
            .replace("target = \"js\"\n", "")
            .replace("kind = \"executable\"", "kind = \"library\""),
    )
    .unwrap();
    let artifact = build_project(&app).unwrap();
    assert!(artifact.is_file());
    assert!(!app.join("build/app.js").exists());
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"",
            "root = \"main.rud\"\ntarget = \"artifact\"",
        ),
    )
    .unwrap();
    build_project(&app).unwrap();
    assert!(!app.join("build/app.js").exists());

    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("target = \"artifact\"", "target = \"js\""),
    )
    .unwrap();
    assert_eq!(build_project(&app).unwrap(), artifact);
    let javascript = fs::read_to_string(app.join("build/app.js")).unwrap();
    assert!(!javascript.is_empty());

    // Switching back does not implicitly clean a stale sibling product.
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("target = \"js\"", "target = \"artifact\""),
    )
    .unwrap();
    build_project(&app).unwrap();
    assert_eq!(
        fs::read_to_string(app.join("build/app.js")).unwrap(),
        javascript
    );
    clean_project(&app).unwrap();
    assert!(!app.join("build").exists());
}

#[test]
fn dependency_targets_do_not_select_backend_output_for_a_parent_build() {
    let directory = tempfile::tempdir().unwrap();
    let dependency = directory.path().join("dep");
    let app = directory.path().join("app");
    write_project(&dependency, "dep", "1.0.0", &[]);
    write_project(&app, "app", "1.0.0", &[("dep", "../dep")]);
    let manifest = fs::read_to_string(dependency.join("Ruddy.toml")).unwrap();
    fs::write(
        dependency.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    build_project(&app).unwrap();
    assert!(dependency.join("build/dep.artifact").is_file());
    assert!(!dependency.join("build/dep.js").exists());
    assert!(!app.join("build/app.js").exists());

    // A JS root with a dependency also proves generation receives the linked
    // artifact: the compiler's pre-link root artifact still has dependencies
    // and is intentionally rejected by the backend.
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();
    build_project(&app).unwrap();
    assert!(app.join("build/app.js").is_file());
    assert!(!dependency.join("build/dep.js").exists());
}

#[test]
fn unsupported_manifest_targets_use_manifest_parse_diagnostics() {
    for target in ["\"native\"", "42"] {
        let directory = project();
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!(
                "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = {target}\n[dependencies]\nstd = false\n"
            ),
        )
        .unwrap();
        let error = compile(directory.path()).unwrap_err().to_string();
        assert!(error.contains("[manifest-invalid] Error"), "{error}");
    }
}

#[test]
fn build_surfaces_compile_directory_and_artifact_write_failures() {
    let missing = tempfile::tempdir().unwrap();
    let compile_error = build_project(missing.path()).unwrap_err();
    assert!(
        compile_error
            .to_string()
            .contains("[manifest-unreadable] Error")
    );
    assert!(!compile_error.is_usage());

    let blocked_build = tempfile::tempdir().unwrap();
    new_project(blocked_build.path().join("app")).unwrap();
    let project = blocked_build.path().join("app");
    disable_std(&project);
    fs::write(project.join("build"), "not a directory").unwrap();
    let error = build_project(&project).unwrap_err().to_string();
    assert!(
        error.contains("could not create build directory"),
        "{error}"
    );

    let blocked_artifact = tempfile::tempdir().unwrap();
    new_project(blocked_artifact.path().join("app")).unwrap();
    let project = blocked_artifact.path().join("app");
    disable_std(&project);
    fs::create_dir(project.join("build")).unwrap();
    fs::create_dir(project.join("build/app.artifact")).unwrap();
    let error = build_project(&project).unwrap_err().to_string();
    assert!(error.contains("could not write artifact"), "{error}");
    let entries: Vec<_> = fs::read_dir(project.join("build"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from("app.artifact")]);
}

#[test]
fn clap_commands_support_aliases_help_and_strict_arguments() {
    let current = tempfile::tempdir().unwrap();
    assert_eq!(
        run(["n", "app"], current.path()).unwrap(),
        Outcome::Created(current.path().join("app"))
    );
    disable_std(&current.path().join("app"));
    assert_eq!(
        run(["b"], current.path().join("app")).unwrap(),
        Outcome::Built(current.path().join("app/build/app.artifact"))
    );
    assert_eq!(
        run(["check"], current.path().join("app")).unwrap(),
        Outcome::Checked(current.path().join("app"))
    );
    assert_eq!(
        run(["clean"], current.path().join("app")).unwrap(),
        Outcome::Cleaned(
            fs::canonicalize(current.path().join("app/build").parent().unwrap())
                .unwrap()
                .join("build")
        )
    );
    assert!(!current.path().join("app/build").exists());

    for arguments in [
        vec![],
        vec!["new"],
        vec!["new", "one", "two"],
        vec!["build", "elsewhere"],
        vec!["clean", "elsewhere"],
        vec!["check", "elsewhere"],
        vec!["run", "elsewhere"],
        vec!["r"],
        vec!["compile"],
    ] {
        let error = run(arguments, current.path()).unwrap_err();
        assert!(error.is_usage(), "{error}");
        assert_eq!(error.exit_code(), 2, "{error}");
        assert!(error.to_string().contains("Usage:"), "{error}");
    }

    for arguments in [vec!["--help"], vec!["new", "--help"], vec!["--version"]] {
        let information = run(arguments, current.path()).unwrap_err();
        assert!(information.is_success(), "{information}");
        assert_eq!(information.exit_code(), 0, "{information}");
        assert!(!information.is_usage(), "{information}");
    }
}

#[test]
fn run_builds_and_invokes_main_in_a_javascript_module() {
    let directory = tempfile::tempdir().unwrap();
    // The generated module remains ESM even inside an explicitly CommonJS
    // package scope.
    fs::write(
        directory.path().join("package.json"),
        "{\"type\":\"commonjs\"}\n",
    )
    .unwrap();
    let app = directory.path().join("app");
    write_project(&app, "app", "1.0.0", &[]);
    fs::write(app.join("main.rud"), "let initialized = 1n\n").unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    let expected = app.join("build/app.js");
    assert_eq!(run_project(&app).unwrap(), expected);
    assert_eq!(run(["run"], &app).unwrap(), Outcome::Ran(expected.clone()));
    assert!(expected.is_file());
    assert!(app.join("build/app.artifact").is_file());
    assert_eq!(
        fs::read_to_string(app.join("build/package.json")).unwrap(),
        "{\"type\":\"module\"}\n"
    );

    let built_javascript = fs::read_to_string(&expected).unwrap();
    let built_artifact = fs::read_to_string(app.join("build/app.artifact")).unwrap();
    clean_project(&app).unwrap();
    build_project(&app).unwrap();
    assert_eq!(fs::read_to_string(&expected).unwrap(), built_javascript);
    assert_eq!(
        fs::read_to_string(app.join("build/app.artifact")).unwrap(),
        built_artifact
    );
}

#[cfg(unix)]
#[test]
fn run_uses_the_configured_javascript_shell_runner() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    write_project(&app, "app", "1.0.0", &[]);
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n\n[run]\njs = \"sh runner.sh marker.txt\"\n\n[dependencies]",
        ),
    )
    .unwrap();
    fs::write(
        app.join("runner.sh"),
        "#!/bin/sh\nprintf '%s' \"$2\" > \"$1\"\n",
    )
    .unwrap();

    prepare_execution(&app);
    // Build configuration is inert until `run` is requested.
    build_project(&app).unwrap();
    assert!(!app.join("marker.txt").exists());

    let expected = app.join("build/app.js");
    assert_eq!(run_project(&app).unwrap(), expected);
    assert_eq!(
        fs::read_to_string(app.join("marker.txt")).unwrap(),
        expected.to_string_lossy()
    );
}

#[cfg(unix)]
#[test]
fn configured_javascript_runner_failures_preserve_build_output() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    write_project(&app, "app", "1.0.0", &[]);
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n\n[run]\njs = \"false\"\n\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(!error.is_usage());
    assert!(error.to_string().contains("runner `false`"), "{error}");
    assert!(error.to_string().contains("status: 1"), "{error}");
    assert!(app.join("build/app.js").is_file());
    assert!(app.join("build/app.artifact").is_file());
}

#[test]
fn run_rejects_library_targets_without_executing_stale_javascript() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    write_project(&app, "app", "1.0.0", &[]);
    fs::create_dir(app.join("build")).unwrap();
    let stale = app.join("build/app.js");
    fs::write(&stale, "throw new Error('stale JavaScript executed');\n").unwrap();

    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(!error.is_usage());
    assert!(error.to_string().contains("target = \"js\""), "{error}");
    assert!(!error.to_string().contains("stale JavaScript"), "{error}");
    assert_eq!(
        fs::read_to_string(&stale).unwrap(),
        "throw new Error('stale JavaScript executed');\n"
    );
    assert!(!app.join("build/app.artifact").exists());
}

#[test]
fn run_uses_node_standard_globals() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("runtime");
    write_project(&app, "runtime", "1.0.0", &[]);
    fs::write(
        app.join("main.rud"),
        "extern cwd : {} -> String = \"process.cwd\"\n\
         extern environment : {} = \"process.env\"\n\
         extern console_object : {} = \"console\"\n\
         extern url : {} = \"URL\"\n\
         extern encoder : {} = \"TextEncoder\"\n\
         extern decoder : {} = \"TextDecoder\"\n\
         extern base64 : String -> String = \"btoa\"\n\
         extern clone : {} -> {} = \"structuredClone\"\n\
         extern microtask : ({} -> {}) -> {} = \"queueMicrotask\"\n\
         extern timeout : ({} -> {}) -> Nat = \"setTimeout\"\n\
         extern abort_controller : {} = \"AbortController\"\n\
         extern fetch_value : String -> {} = \"fetch\"\n\
         let initialized = 0n\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    run_project(&app).expect("the documented Node.js globals are available");
}

#[test]
fn run_drains_queued_jobs_and_preserves_installed_files_on_runtime_failure() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("queued");
    write_project(&app, "queued", "1.0.0", &[]);
    fs::write(
        app.join("main.rud"),
        "extern queue : ({} -> {}) -> {} = \"queueMicrotask\"\n\
         extern parse : String -> {} = \"JSON.parse\"\n\
         let queued = queue (fn _ => parse \"{\")\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(!error.is_usage());
    let rendered = error.to_string().replace('\\', "/");
    assert!(rendered.contains("Node.js exited"), "{rendered}");
    assert!(rendered.contains("SyntaxError"), "{rendered}");
    assert!(rendered.contains("build/queued.js"), "{rendered}");
    assert!(rendered.contains(" at "), "{rendered}");
    assert!(app.join("build/queued.js").is_file());
    assert!(app.join("build/queued.artifact").is_file());
}

#[test]
fn javascript_evaluation_rejection_does_not_wait_for_recurring_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("rejects.mjs");
    fs::write(
        &module,
        "setInterval(() => {}, 60_000);\nthrow new Error('initialization failed');\n",
    )
    .unwrap();

    let started = std::time::Instant::now();
    let error = execute_javascript_module(&module).unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        error.to_string().contains("initialization failed"),
        "{error}"
    );
}

#[test]
fn node_reports_unhandled_promises_despite_recurring_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("unhandled.mjs");
    fs::write(
        &module,
        "Promise.reject(new Error('orphaned rejection'));\n\
         setInterval(() => {}, 60_000);\n",
    )
    .unwrap();

    let error = execute_javascript_module(&module).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("Node.js exited"), "{error}");
    assert!(error.to_string().contains("orphaned rejection"), "{error}");
}

#[test]
fn javascript_allows_a_deep_microtask_chain_to_handle_a_rejection() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("handled.mjs");
    fs::write(
        &module,
        "const promise = Promise.reject(new Error('handled eventually'));\n\
         function defer(depth) {\n\
           Promise.resolve().then(() => {\n\
             if (depth === 0) promise.catch(() => {});\n\
             else defer(depth - 1);\n\
           });\n\
         }\n\
         defer(10_000);\n",
    )
    .unwrap();

    execute_javascript_module(&module).expect("the microtask checkpoint drains to quiescence");
}

#[test]
fn javascript_reports_a_rejection_before_a_zero_delay_timer_can_handle_it() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("timer-is-too-late.mjs");
    fs::write(
        &module,
        "const promise = Promise.reject(new Error('timer was too late'));\n\
         setTimeout(() => promise.catch(() => {}), 0);\n",
    )
    .unwrap();

    let error = execute_javascript_module(&module).unwrap_err();
    assert!(error.to_string().contains("Node.js exited"), "{error}");
    assert!(error.to_string().contains("timer was too late"), "{error}");
}

#[test]
fn javascript_runs_a_microtask_checkpoint_between_timer_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("timer-microtask-order.mjs");
    fs::write(
        &module,
        "const order = [];\n\
         setTimeout(() => {\n\
           order.push('first timer');\n\
           queueMicrotask(() => order.push('microtask'));\n\
         }, 0);\n\
         setTimeout(() => {\n\
           order.push('second timer');\n\
           if (order.join(',') !== 'first timer,microtask,second timer') {\n\
             throw new Error(`wrong task order: ${order}`);\n\
           }\n\
         }, 0);\n",
    )
    .unwrap();

    execute_javascript_module(&module).expect("microtasks run between timer tasks");
}

#[test]
fn javascript_does_not_sleep_for_a_timer_cleared_by_an_earlier_timer() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("cleared-future-timer.mjs");
    fs::write(
        &module,
        "const future = setTimeout(() => {\n\
           throw new Error('cleared timer ran');\n\
         }, 2_000);\n\
         setTimeout(() => clearTimeout(future), 0);\n",
    )
    .unwrap();

    let started = std::time::Instant::now();
    execute_javascript_module(&module).expect("clearing the only future timer ends execution");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "executor slept for the canceled timer: {:?}",
        started.elapsed()
    );
}

#[test]
fn javascript_does_not_reschedule_an_interval_that_clears_itself() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("self-clearing-interval.mjs");
    fs::write(
        &module,
        "const interval = setInterval(() => clearInterval(interval), 500);\n",
    )
    .unwrap();

    let started = std::time::Instant::now();
    execute_javascript_module(&module).expect("a self-clearing interval terminates");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(850),
        "self-cleared interval was rescheduled: {:?}",
        started.elapsed()
    );
}

#[test]
fn run_rejects_an_effectful_callback_through_a_polymorphic_extern_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("polymorphic-callback");
    write_project(&app, "polymorphic-callback", "1.0.0", &[]);
    fs::write(
        app.join("main.rud"),
        "effect Tick = Nat -> Nat\n\
         extern run : fn('a) -> Nat = \"host.run\"\n\
         let result = handle run (fn n => !Tick n) with\n\
           | !Tick n => n\n\
         end\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string();
    assert!(rendered.contains("runtime-type-information"), "{rendered}");
    assert!(rendered.contains("runtime type information"), "{rendered}");
    assert!(
        !app.join("build/polymorphic-callback.js").exists(),
        "an unsound module reached execution"
    );
}

#[test]
fn run_reports_invalid_extern_expressions_with_javascript_source_locations() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("missing");
    write_project(&app, "missing", "1.0.0", &[]);
    fs::write(
        app.join("main.rud"),
        "extern unavailable : Nat = \"ruddy_runtime.unavailable\"\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.rud\"\n[dependencies]",
            "root = \"main.rud\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    prepare_execution(&app);
    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string().replace('\\', "/");
    assert!(
        rendered.contains("ruddy_runtime is not defined"),
        "{rendered}"
    );
    assert!(rendered.contains("build/missing.js"), "{rendered}");
    assert!(app.join("build/missing.js").is_file());
    assert!(app.join("build/missing.artifact").is_file());
}

#[test]
fn check_compiles_without_build_output_and_reports_failures() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    new_project(&app).unwrap();
    disable_std(&app);

    check_project(&app).unwrap();
    assert!(!app.join("build").exists());

    fs::write(app.join("main.rud"), "let bad : Nat = false\n").unwrap();
    let error = check_project(&app).unwrap_err();
    assert!(!error.is_usage());
    assert_eq!(error.exit_code(), 1);
    assert!(
        error.to_string().contains("[type-mismatch] Error"),
        "{error}"
    );
    assert!(!app.join("build").exists());
}

#[test]
fn imported_array_aliases_cross_native_extern_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let dependency = directory.path().join("dep");
    let app = directory.path().join("app");
    fs::create_dir(&dependency).unwrap();
    fs::create_dir(&app).unwrap();
    fs::write(
        dependency.join("Ruddy.toml"),
        "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"lib.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(
        dependency.join("lib.rud"),
        "type Carrier 'r = { name: String, ..'r }\ntype Numbers = Carrier { values: [Nat] }\n",
    )
    .unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\ndep = \"../dep\"\n",
    )
    .unwrap();
    fs::write(
        app.join("main.rud"),
        "extern values : dep::Numbers = \"host.values\"\n",
    )
    .unwrap();

    check_project(&app).expect("imported array aliases have a reviewed native conversion");
}

#[test]
fn clean_removes_build_directories_and_other_stale_entries_idempotently() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    fs::create_dir(&app).unwrap();
    // Cleaning needs only the project marker, not a parseable manifest.
    fs::write(app.join("Ruddy.toml"), "this is not valid TOML").unwrap();
    let expected = fs::canonicalize(&app).unwrap().join("build");

    assert_eq!(clean_project(&app).unwrap(), expected);
    fs::create_dir(&expected).unwrap();
    fs::write(expected.join("artifact"), "stale").unwrap();
    assert_eq!(clean_project(&app).unwrap(), expected);
    assert!(!expected.exists());

    fs::write(&expected, "blocked old output").unwrap();
    assert_eq!(clean_project(&app).unwrap(), expected);
    assert!(!expected.exists());

    let missing = directory.path().join("missing");
    let error = clean_project(&missing).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not resolve project folder")
    );
    assert!(!error.is_usage());
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn clean_rejects_non_projects_without_deleting_their_build_data() {
    let directory = tempfile::tempdir().unwrap();
    let build = directory.path().join("build");
    fs::create_dir(&build).unwrap();
    fs::write(build.join("keep"), "not Ruddy build data").unwrap();

    let error = clean_project(directory.path()).unwrap_err();
    assert!(error.to_string().contains("project marker"), "{error}");
    assert_eq!(
        fs::read_to_string(build.join("keep")).unwrap(),
        "not Ruddy build data"
    );

    // A path named like the marker is insufficient unless it is a regular file.
    fs::create_dir(directory.path().join("Ruddy.toml")).unwrap();
    let error = clean_project(directory.path()).unwrap_err();
    assert!(error.to_string().contains("not a regular file"), "{error}");
    assert_eq!(
        fs::read_to_string(build.join("keep")).unwrap(),
        "not Ruddy build data"
    );
}

#[cfg(unix)]
#[test]
fn clean_unlinks_build_symlinks_without_touching_their_targets() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    let output = directory.path().join("shared-output");
    fs::create_dir(&app).unwrap();
    fs::write(app.join("Ruddy.toml"), "malformed is fine").unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep"), "must remain").unwrap();
    symlink(&output, app.join("build")).unwrap();

    clean_project(&app).unwrap();
    assert!(!app.join("build").exists());
    assert_eq!(
        fs::read_to_string(output.join("keep")).unwrap(),
        "must remain"
    );
}

/// Dependency artifacts are kept under Ruddy home between builds, keyed by the
/// compiler's stamp and the sources they came from, so the second build of a
/// project reads its dependencies rather than compiling them.
#[test]
fn dependency_artifacts_are_cached_between_builds() {
    let outer = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "cli::dependency_artifacts_cache_child",
        ])
        .env("RUDDY_HOME", outer.path().join("home"))
        .env("RUDDY_TEST_CACHE_ROOT", outer.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with a dedicated Ruddy home"]
fn dependency_artifacts_cache_child() {
    let home = PathBuf::from(std::env::var_os("RUDDY_HOME").unwrap());
    let root = PathBuf::from(std::env::var_os("RUDDY_TEST_CACHE_ROOT").unwrap());
    let dep = root.join("dep");
    let app = root.join("app");
    write_project(&dep, "dep", "1.0.0", &[]);
    write_project(&app, "app", "0.1.0", &[("dep", "../dep")]);
    fs::write(app.join("main.rud"), "let main = dep::value\n").unwrap();
    build_project(&app).unwrap();

    // One entry, for the dependency and under this compiler's stamp; the
    // root is what the build is for and is not kept.
    let cache = home.join("cache/artifacts");
    let compilers: Vec<_> = fs::read_dir(&cache).unwrap().flatten().collect();
    assert_eq!(compilers.len(), 1);
    assert_eq!(
        compilers[0].file_name().to_string_lossy(),
        ruddy::artifact::COMPILER_HASH
    );
    let entries: Vec<_> = fs::read_dir(compilers[0].path())
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(entries.len(), 1);
    let entry = entries[0].path();
    let cached = ruddy::artifact::text::parse(&fs::read_to_string(&entry).unwrap());
    assert_eq!(cached.header().identity.name, "dep");
    assert!(cached.header().compiler.is_current());

    // What the cache holds is what the next build sees: an artifact for the
    // same key that exports a different name is believed over the source.
    let other = root.join("other");
    write_project(&other, "dep", "1.0.0", &[]);
    fs::write(other.join("main.rud"), "let renamed = 0n\n").unwrap();
    let planted = compile(&other).unwrap().print();
    fs::write(&entry, &planted).unwrap();
    fs::write(app.join("main.rud"), "let main = dep::renamed\n").unwrap();
    build_project(&app).unwrap();

    // Another compiler's directory is removed the moment this one stores
    // something, and an entry stamped by another compiler is not read.
    let stale = cache.join("0000000000000000");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("x.artifact"), "stale").unwrap();
    fs::write(
        &entry,
        planted.replace(ruddy::artifact::COMPILER_HASH, "0000000000000000"),
    )
    .unwrap();
    let error = build_project(&app).unwrap_err().to_string();
    assert!(error.contains("renamed"), "{error}");
    assert!(!stale.exists());
    assert!(
        ruddy::artifact::text::parse(&fs::read_to_string(&entry).unwrap())
            .header()
            .compiler
            .is_current()
    );

    // A change to the dependency's sources changes the key, so the planted
    // entry is left behind and the real dependency is compiled.
    fs::write(&entry, &planted).unwrap();
    fs::write(dep.join("main.rud"), "let value = 1n\nlet more = 2n\n").unwrap();
    let error = build_project(&app).unwrap_err().to_string();
    assert!(error.contains("renamed"), "{error}");
    fs::write(app.join("main.rud"), "let main = dep::more\n").unwrap();
    build_project(&app).unwrap();

    // The target is part of the key: the same dependency built under a
    // library root, for `artifact`, is a second entry rather than the JS
    // build's artifact read back for a build its guards were not judged for.
    let lib = root.join("lib");
    write_project(&lib, "lib", "0.1.0", &[("dep", "../dep")]);
    fs::write(lib.join("main.rud"), "let value = dep::more\n").unwrap();
    build_project(&lib).unwrap();
    let entries = fs::read_dir(compilers[0].path()).unwrap().flatten().count();
    assert_eq!(entries, 2);
}

/// `platform` in the manifest is the other fact a guard can name. It defaults
/// to Node, and a manifest that names a platform the compiler does not know
/// is refused where it is read.
#[test]
fn a_manifests_platform_is_judged_by_guards_and_defaults_to_node() {
    let directory = tempfile::tempdir().unwrap();
    write_project(directory.path(), "hosted", "1.0.0", &[]);
    fs::write(
        directory.path().join("main.rud"),
        "@if {platform: \"web\"} let value = 1n\nlet uses = value\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(directory.path().join("Ruddy.toml")).unwrap();

    let error = compile(directory.path()).unwrap_err().to_string();
    assert!(error.contains("undefined-term"), "{error}");

    fs::write(
        directory.path().join("Ruddy.toml"),
        manifest.replace("root =", "platform = \"web\"\nroot ="),
    )
    .unwrap();
    compile(directory.path()).expect("the web arm is compiled for a web build");

    fs::write(
        directory.path().join("Ruddy.toml"),
        manifest.replace("root =", "platform = \"deno\"\nroot ="),
    )
    .unwrap();
    let error = compile(directory.path()).unwrap_err().to_string();
    assert!(error.contains("deno"), "{error}");
}

/// A web library is an ordinary JavaScript build, with nothing of Node in its
/// output; a web executable is refused at the manifest, since the only entry
/// adapter writes to Node's streams and exits its process.
#[test]
fn a_web_library_builds_and_a_web_executable_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    write_project(directory.path(), "web_lib", "1.0.0", &[]);
    let manifest = fs::read_to_string(directory.path().join("Ruddy.toml")).unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        manifest.replace("root =", "target = \"js\"\nplatform = \"web\"\nroot ="),
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "@if {platform: \"web\"} let value = 1n\nlet main: () -> () = fn _ => ()\n",
    )
    .unwrap();
    let artifact = build_project(directory.path()).expect("a web library builds");
    let javascript = fs::read_to_string(artifact.with_extension("js")).unwrap();
    assert!(javascript.contains("value"), "{javascript}");
    assert!(!javascript.contains("process."), "{javascript}");

    let app = directory.path().join("app");
    executable_project(&app, "let main = fn _ => ()", Some("js"));
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("root =", "platform = \"web\"\nroot ="),
    )
    .unwrap();
    let error = build_project(&app).unwrap_err().to_string();
    assert!(error.contains("platform-unsupported"), "{error}");
    assert!(error.contains("`web` platform"), "{error}");
    let error = check_project(&app).unwrap_err().to_string();
    assert!(error.contains("platform-unsupported"), "{error}");
}

/// A dependency's guards are judged against the root build's target, not the
/// library's own manifest, so a library can carry one definition per target
/// and a JavaScript executable gets the JavaScript one.
#[test]
fn a_dependency_is_compiled_for_the_root_builds_target() {
    let project = tempfile::tempdir().unwrap();
    let dependency = project.path().join("dep");
    write_project(&dependency, "dep", "1.0.0", &[]);
    fs::write(
        dependency.join("main.rud"),
        "type Never = |\n\
         effect Process = { exit: Nat -> Never }\n\
         @if {target: \"js\"} module js\n\
         @if {target: \"js\"} let stop : () -> Never + !Process = fn _ => !Process.exit js::code\n\
         @if {target: \"artifact\"} let stop : () -> Never + !Process = fn _ => !Process.exit 3n\n",
    )
    .unwrap();
    fs::write(dependency.join("js.rud"), "let code = 17n\n").unwrap();
    // On its own the library is an artifact build: the `js` module is never
    // looked for and the artifact arm is the one compiled.
    let alone = compile(&dependency).unwrap();
    assert!(alone.print().contains("stop"));
    assert!(!alone.print().contains("code"));

    let app = project.path().join("app");
    executable_project(&app, "let main = dep::stop", None);
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        format!("{manifest}dep = \"../dep\"\n"),
    )
    .unwrap();
    let artifact = build_project(&app).expect("the dependency compiles for the JS root");
    let output = Command::new("node")
        .arg(artifact.with_extension("js"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn private_file_modules_execute_without_exposing_javascript_exports() {
    let directory = tempfile::tempdir().unwrap();
    write_project(directory.path(), "privacy", "1.0.0", &[]);
    let manifest = fs::read_to_string(directory.path().join("Ruddy.toml")).unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        manifest.replace("root =", "target = \"js\"\nroot ="),
    )
    .unwrap();
    fs::write(
        directory.path().join("main.rud"),
        "@private module Hidden\n\
         @private extern remember : Nat -> Nat = \"x => (globalThis.ruddyPrivateInit = x, x)\"\n\
         @private let initialized = remember 7n\n\
         @private let helper = fn _ => Hidden::Nested::answer\n\
         module Visible = @private let hidden = 1n let shown = initialized end\n\
         let answer = helper ()\n\
         let exposed: () -> Nat = helper\n\
         @private let main = fn _ => ()",
    )
    .unwrap();
    fs::write(
        directory.path().join("Hidden.rud"),
        "module Nested = let answer = 42n end",
    )
    .unwrap();
    check_project(directory.path()).expect("a library may have a private main");
    let artifact = build_project(directory.path()).expect("private module code remains executable");
    let script = format!(
        "import * as api from {};\n\
         console.log(Object.keys(api).sort().join(','));\n\
         console.log(Object.keys(api.Visible).join(','));\n\
         console.log(String(api.answer), String(api.exposed()), String(api.Visible.shown), String(globalThis.ruddyPrivateInit));",
        serde_json::to_string(&artifact.with_extension("js")).unwrap(),
    );
    let output = Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Visible,answer,exposed\nshown\n42 42 7 7\n");
}

#[test]
fn private_main_is_rejected_for_every_executable_target() {
    for target in ["artifact", "js"] {
        let directory = tempfile::tempdir().unwrap();
        executable_project(
            directory.path(),
            "@private let main = fn _ => ()",
            Some(target),
        );
        for error in [
            check_project(directory.path())
                .expect_err("check requires a public main")
                .to_string(),
            build_project(directory.path())
                .expect_err("build requires a public main")
                .to_string(),
        ] {
            assert!(error.contains("public root-module `main`"), "{error}");
        }
        fs::write(
            directory.path().join("main.rud"),
            "@private let start = fn _ => ()\nlet main = start",
        )
        .unwrap();
        check_project(directory.path()).expect("a public main may alias a private function");
        build_project(directory.path()).expect("the public main builds");
    }
}

/// A bundle with an unformatted root, a module file in a subfolder, a
/// nested bundle that is not ours, and a file outside the source folder.
fn unformatted_bundle() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("src/Math")).unwrap();
    fs::create_dir_all(root.join("src/vendor/dep")).unwrap();
    fs::write(
        root.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"src/main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rud"), "module Math\nlet   x  =  1n\n").unwrap();
    fs::write(root.join("src/Math/module.rud"), "let y = 2n\n").unwrap();
    fs::write(
        root.join("src/vendor/dep/Ruddy.toml"),
        "name = \"dep\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(root.join("src/vendor/dep/main.rud"), "let   z  =  3n\n").unwrap();
    fs::write(root.join("outside.rud"), "let   w  =  4n\n").unwrap();
    directory
}

/// `ruddy fmt` with no paths formats the bundle's own sources: everything
/// under the folder its root is in, minus any nested bundle, and nothing
/// outside it. Run from a subfolder it finds the manifest above it.
#[test]
fn fmt_formats_the_enclosing_bundle_and_nothing_else() {
    let directory = unformatted_bundle();
    let root = directory.path();
    let report = match run(["fmt"], root.join("src/Math")).unwrap() {
        Outcome::Formatted(report) => report,
        other => panic!("{other:?}"),
    };
    assert_eq!(report.changed, [root.join("src/main.rud")]);
    assert_eq!(report.unchanged, [root.join("src/Math/module.rud")]);
    assert!(report.errors.is_empty() && !report.failed() && !report.check);
    assert_eq!(
        fs::read_to_string(root.join("src/main.rud")).unwrap(),
        "module Math\nlet x = 1n\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/vendor/dep/main.rud")).unwrap(),
        "let   z  =  3n\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("outside.rud")).unwrap(),
        "let   w  =  4n\n"
    );
    // Formatted once, formatted for good.
    let again = format_paths(&[], root, false).unwrap();
    assert!(again.changed.is_empty());
    assert_eq!(again.unchanged.len(), 2);
}

/// `--check` writes nothing and fails when a file would change; explicit
/// files and folders are formatted wherever they are.
#[test]
fn fmt_check_and_explicit_paths() {
    let directory = unformatted_bundle();
    let root = directory.path();
    let before = fs::read_to_string(root.join("src/main.rud")).unwrap();
    let report = match run(["f", "--check"], root).unwrap() {
        Outcome::Formatted(report) => report,
        other => panic!("{other:?}"),
    };
    assert!(report.check && report.failed());
    assert_eq!(report.changed, [root.join("src/main.rud")]);
    assert_eq!(
        fs::read_to_string(root.join("src/main.rud")).unwrap(),
        before
    );

    // A named folder is walked, but a bundle nested in it is still another
    // bundle's; naming that bundle's folder itself formats it.
    let report = format_paths(
        &[PathBuf::from("outside.rud"), PathBuf::from("src/vendor")],
        root,
        false,
    )
    .unwrap();
    assert_eq!(report.changed, [root.join("outside.rud")]);
    let report = format_paths(&[PathBuf::from("src/vendor/dep")], root, false).unwrap();
    assert_eq!(report.changed, [root.join("src/vendor/dep/main.rud")]);
    assert_eq!(
        fs::read_to_string(root.join("outside.rud")).unwrap(),
        "let w = 4n\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/vendor/dep/main.rud")).unwrap(),
        "let z = 3n\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/main.rud")).unwrap(),
        before
    );

    let error = format_paths(&[PathBuf::from("missing.rud")], root, false).unwrap_err();
    assert!(error.to_string().contains("could not find"), "{error}");
    let nowhere = tempfile::tempdir().unwrap();
    let error = format_paths(&[], nowhere.path(), false).unwrap_err();
    assert!(error.to_string().contains("no `Ruddy.toml`"), "{error}");
    assert!(error.to_string().contains("help:"), "{error}");
    let usage = run(["fmt", "--stdin", "outside.rud"], root).unwrap_err();
    assert!(usage.is_usage(), "{usage}");
}

/// A file with a syntax error is still written, formatted around the
/// error, and the run fails with the error reported.
#[test]
fn fmt_formats_around_syntax_errors_and_reports_them() {
    let directory = unformatted_bundle();
    let root = directory.path();
    fs::write(
        root.join("src/main.rud"),
        "let   a = 1n\nlet = broken\nlet   b = 2n\n",
    )
    .unwrap();
    let report = match run(["fmt"], root).unwrap() {
        Outcome::Formatted(report) => report,
        other => panic!("{other:?}"),
    };
    assert_eq!(report.errors, [root.join("src/main.rud")]);
    assert_eq!(report.diagnostics.len(), 1);
    assert!(report.failed());
    let rendered = report.diagnostics[0].render(false);
    assert!(rendered.contains("main.rud"), "{rendered}");
    assert_eq!(
        fs::read_to_string(root.join("src/main.rud")).unwrap(),
        "let a = 1n\nlet = broken\nlet b = 2n\n"
    );
    assert!(!FormatReport::default().failed());
}
