//! Tests for the filesystem-facing CLI compiler API.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ruddy::artifact::Artifact;
use ruddy_cli::{Lockfile, Outcome, build_project, compile, new_project, run};
use tempfile::TempDir;

fn project() -> TempDir {
    let directory = tempfile::tempdir().expect("a temporary project");
    fs::write(directory.path().join("main.hc"), "let id = fn x => x\n").expect("write the root");
    directory
}

fn write_project(directory: &Path, name: &str, version: &str, dependencies: &[(&str, &str)]) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("main.hc"), "let value = 0n\n").unwrap();
    let mut manifest =
        format!("name = {name:?}\nversion = {version:?}\nroot = \"main.hc\"\n[dependencies]\n");
    for (dependency, path) in dependencies {
        manifest.push_str(&format!("{dependency} = {path:?}\n"));
    }
    fs::write(directory.join("Ruddy.toml"), manifest).unwrap();
}

fn error(directory: &TempDir) -> String {
    compile(directory.path())
        .expect_err("compilation fails")
        .to_string()
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nzeta = \"zeta\"\nalpha = \"alpha\"\n",
    )
    .expect("write the manifest");

    let graph = ruddy_cli::compile_graph(directory.path()).expect("compile the project graph");
    let identities: Vec<_> = graph
        .projects
        .last()
        .unwrap()
        .artifact
        .header
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency.version.as_str()))
        .collect();
    assert_eq!(identities, [("zeta", "2.0.0"), ("alpha", "1.2.3-beta.1")]);
    let built = compile(directory.path()).unwrap();
    assert_eq!(built.header.identity.name, "app");
    assert!(built.header.dependencies.is_empty());
    assert!(!directory.path().join("zeta/build/zeta.artifact").exists());
    assert!(!directory.path().join("alpha/build/alpha.artifact").exists());
}

#[test]
fn direct_dependency_exports_resolve_and_keep_their_artifact_owner() {
    let directory = project();
    let dependency = directory.path().join("std");
    write_project(&dependency, "std", "0.1.0", &[]);
    fs::write(
        dependency.join("main.hc"),
        "module Nested =\n  type Number = Nat\n  effect Read = get : {} -> Nat\n  let foo = 1n\nend\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let value : std::Nested::Number = std::Nested::foo\nlet operation = std::Nested::!Read.get\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();

    let built = compile(directory.path()).expect("dependency members resolve");
    let printed = built.print();
    assert!(printed.contains("std@0.1.0::Nested::foo"), "{printed}");
    assert!(printed.contains("std@0.1.0::Nested::Number"), "{printed}");
    assert_eq!(built.header.values.len(), 2);
    assert!(built.header.types.is_empty());
    assert!(built.header.effects.is_empty());
}

#[test]
fn detailed_dependencies_alias_hyphenated_bundle_identities() {
    let directory = project();
    let dependency = directory.path().join("http-core");
    write_project(&dependency, "http-core", "1.0.0", &[]);
    fs::write(dependency.join("main.hc"), "let status = 200n\n").unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let main = http_core::status\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nhttp_core = { bundle = \"http-core\", path = \"http-core\" }\n",
    )
    .unwrap();

    let graph = ruddy_cli::compile_graph(directory.path()).expect("the source alias resolves");
    assert_eq!(
        graph.projects.last().unwrap().artifact.header.dependencies[0].name,
        "http-core"
    );
    assert!(
        compile(directory.path())
            .unwrap()
            .header
            .dependencies
            .is_empty()
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nhttp_core = { package = \"http-core\", path = \"http-core\" }\n",
    )
    .unwrap();
    let old_field_error = error(&directory);
    assert!(
        old_field_error.contains("unknown field `package`"),
        "{old_field_error}"
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nhttp-core = \"http-core\"\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(
        error.contains("not a valid Ruddy source identifier"),
        "{error}"
    );
}

#[test]
fn transitive_dependencies_are_linkable_but_not_source_visible() {
    let directory = project();
    write_project(&directory.path().join("base"), "base", "1.0.0", &[]);
    fs::write(
        directory.path().join("base/main.hc"),
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
        directory.path().join("std/main.hc"),
        "let foo = base::foo\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let main : Nat = std::foo\n",
    )
    .unwrap();
    let built = compile(directory.path()).expect("direct export backed by transitive global");
    assert!(built.print().contains("std@1.0.0::foo"));

    fs::write(directory.path().join("main.hc"), "let main = base::foo\n").unwrap();
    let error = error(&directory);
    assert!(error.contains("undefined module"), "{error}");
}

#[test]
fn missing_dependency_paths_report_the_requested_namespace() {
    let directory = project();
    write_project(&directory.path().join("std"), "std", "1.0.0", &[]);
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let a = std::missing\nlet b : std::Missing = 1n\nlet c = std::!MissingEffect.op\nlet d = std::NoModule::x\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(error.contains("undefined term"), "{error}");
    assert!(error.contains("undefined type"), "{error}");
    assert!(error.contains("undefined effect"), "{error}");
    assert!(error.contains("undefined module"), "{error}");
}

#[test]
fn a_local_module_cannot_shadow_a_direct_dependency_root() {
    let directory = project();
    write_project(&directory.path().join("std"), "std", "1.0.0", &[]);
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"std\"\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "module std = let local = 1n end\nlet main = 0n\n",
    )
    .unwrap();
    let error = error(&directory);
    assert!(error.contains("duplicate module"), "{error}");
}

#[test]
fn the_configured_root_is_resolved_relative_to_the_manifest() {
    let directory = project();
    fs::create_dir(directory.path().join("src")).expect("create source directory");
    fs::rename(
        directory.path().join("main.hc"),
        directory.path().join("src/app.hc"),
    )
    .expect("move the root");
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"src/app.hc\"\n[dependencies]\n",
    )
    .expect("write the manifest");

    let built = compile(directory.path()).expect("compile the configured root");
    assert!(built.header.dependencies.is_empty());
    assert_eq!(Artifact::try_parse(&built.print()).unwrap(), built);
}

#[test]
fn nested_root_diagnostics_preserve_root_and_module_paths() {
    let directory = project();
    fs::create_dir(directory.path().join("src")).expect("create source directory");
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"src/app.hc\"\n[dependencies]\n",
    )
    .expect("write the manifest");
    fs::write(
        directory.path().join("src/app.hc"),
        "let bad : Nat = fn x => x\n",
    )
    .expect("write an invalid root");

    let root_error = error(&directory).replace('\\', "/");
    assert!(root_error.contains("src/app.hc:1:"), "{root_error}");

    fs::write(directory.path().join("src/app.hc"), "module Child\n").expect("replace the root");
    fs::write(
        directory.path().join("src/Child.hc"),
        "let bad : Nat = fn x => x\n",
    )
    .expect("write an invalid module");

    let module_error = error(&directory).replace('\\', "/");
    assert!(module_error.contains("src/Child.hc:1:"), "{module_error}");

    fs::remove_file(directory.path().join("src/Child.hc")).expect("remove the module file");
    let missing = error(&directory).replace('\\', "/");
    assert!(
        missing.contains("create `src/Child.hc` or `src/Child/module.hc`"),
        "{missing}",
    );

    fs::write(directory.path().join("src/Child.hc"), "let beside = 1n\n")
        .expect("write the beside candidate");
    fs::create_dir(directory.path().join("src/Child")).expect("create module directory");
    fs::write(
        directory.path().join("src/Child/module.hc"),
        "let inside = 1n\n",
    )
    .expect("write the inside candidate");
    let ambiguous = error(&directory).replace('\\', "/");
    assert!(
        ambiguous.contains("delete one of `src/Child.hc` or `src/Child/module.hc`"),
        "{ambiguous}",
    );
}

#[test]
fn the_manifest_is_required_and_must_be_valid_and_supported() {
    let directory = project();
    let missing = error(&directory);
    assert!(missing.contains("could not read manifest"), "{missing}");
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
            "name = 1\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = 1\nroot = \"main.hc\"\n[dependencies]\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = 1\n[dependencies]\n",
            "invalid type",
        ),
        ("[dependencies", "could not parse manifest"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\ntitle = \"app\"\n[dependencies]\n",
            "unknown field `title`",
        ),
        ("title = \"app\"\n", "unknown field `title`"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { source = \"base.artifact\" }\n",
            "unknown field `source`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { version = 1, source = \"base.artifact\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\", source = 1 }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\", source = \"base.artifact\", registry = \"x\" }\n",
            "unknown field `version`",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.toml"), manifest).expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn git_dependency_manifest_validation_is_strict_and_contextual() {
    let directory = project();
    for (specification, expected) in [
        ("{ git = \"http://example.test/repo\" }", "must use HTTPS"),
        ("{ git = \"ssh://example.test/repo\" }", "must use HTTPS"),
        (
            "{ path = \"dep\", git = \"https://example.test/repo\" }",
            "both `path` and `git`",
        ),
        ("{ bundle = \"base\" }", "either `path` or `git`"),
        (
            "{ path = \"dep\", branch = \"main\" }",
            "selectors require a `git`",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"\" }",
            "must not be empty",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"main\", tag = \"v1\" }",
            "conflicting",
        ),
        (
            "{ git = \"https://example.test/repo\", branch = \"bad..name\" }",
            "valid Git reference names",
        ),
        (
            "{ git = \"https://example.test/repo\", unknown = true }",
            "unknown field `unknown`",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = {specification}\n")).unwrap();
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
        if !expected.starts_with("unknown field") {
            assert!(found.contains("dependency `base`"), "{found}");
        }
        assert!(!directory.path().join("Ruddy.lock").exists());
    }
}

#[test]
fn ambient_git_configuration_cannot_rewrite_https_to_an_unsafe_transport() {
    let parent = tempfile::tempdir().unwrap();
    let app = parent.path().join("app");
    let home = parent.path().join("home");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(app.join("main.hc"), "let main = 0n\n").unwrap();
    fs::write(app.join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { git = \"https://example.invalid/repository\" }\n").unwrap();
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
        found.contains("effective Git remote URL must use HTTPS"),
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
                .contains("set RUDDY_HOME")
        ),
    }
}

#[test]
fn https_fetch_failures_are_contextual_and_do_not_create_a_lockfile() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { git = \"https://127.0.0.1:9/repository\", branch = \"main\" }\n").unwrap();
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
        .write_blob(b"name = \"base\"\nversion = \"2.0.0\"\nroot = \"main.hc\"\n[dependencies]\n")
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
                    filename: "main.hc".into(),
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
    fs::write(app.join("main.hc"), "let main = base::value\n").unwrap();
    fs::write(app.join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = {{ git = {url:?}, branch = \"main\" }}\n")).unwrap();
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
    assert!(first.header.dependencies.is_empty());

    // Every use restores both tracked and untracked cache contents while the
    // cross-process cache lock remains held for compilation.
    fs::write(
        checkout.join("Ruddy.toml"),
        "name = \"poison\"\nversion = \"9.9.9\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        checkout.join("main.hc"),
        "mod injected\nlet value = injected::value\n",
    )
    .unwrap();
    fs::write(checkout.join("injected.hc"), "let value = false\n").unwrap();
    let second = compile(&app).unwrap();
    assert_eq!(first, second);
    assert!(!checkout.join("injected.hc").exists());
    assert_eq!(
        fs::read_to_string(checkout.join("main.hc")).unwrap(),
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

    // Cache provenance follows the canonical location, even when a local path
    // specification reaches an already-seeded checkout instead of a Git spec.
    let path_app = home.join("path-app");
    fs::create_dir(&path_app).unwrap();
    fs::write(path_app.join("main.hc"), "let main = base::value\n").unwrap();
    fs::write(
        path_app.join("Ruddy.toml"),
        format!(
            "name = \"path-app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = {{ path = {:?} }}\n",
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
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = { git = \"https://127.0.0.1:9/repository\", rev = \"abc123\" }\n").unwrap();
    let found = error(&directory);
    assert!(found.contains("7 to 40 hexadecimal digits"), "{found}");
    assert!(!directory.path().join("Ruddy.lock").exists());
}

#[test]
fn a_successful_build_removes_stale_git_entries_from_an_existing_lockfile() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    for source in [
        "not toml =",
        "version = 2\n",
        "version = 1\n[[git]]\nurl = \"https://example.test/repo\"\ncommit = \"short\"\n",
        "version = 1\n[[git]]\nurl = \"https://example.test/repo\"\nbranch = \"main\"\ntag = \"v1\"\ncommit = \"0000000000000000000000000000000000000000\"\n",
    ] {
        fs::write(directory.path().join("Ruddy.lock"), source).unwrap();
        let found = error(&directory);
        assert!(found.contains("lockfile"), "{found}");
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
        ("app", "not-semver", "invalid semantic version"),
        ("not.a.name", "1.0.0", "not a valid Ruddy bundle name"),
        ("app", "1.0.0+local", "unsupported build metadata"),
    ] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!("name = {name:?}\nversion = {version:?}\nroot = \"main.hc\"\n[dependencies]\n"),
        )
        .expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn dependency_projects_must_exist_compile_and_match_the_table_key() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = \"missing\"\n").unwrap();
    assert!(error(&directory).contains("dependency `base`"));

    write_project(&directory.path().join("child"), "other", "1.0.0", &[]);
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nbase = \"child\"\n").unwrap();
    assert!(error(&directory).contains("contains project `other` instead"));

    write_project(&directory.path().join("child"), "base", "1.0.0", &[]);
    fs::write(
        directory.path().join("child/main.hc"),
        "let bad : Nat = fn x => x\n",
    )
    .unwrap();
    let found = error(&directory);
    assert!(found.contains("dependency `base`"));
    assert!(found.contains("error[types/"));
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

    let graph = ruddy_cli::compile_graph(&app).unwrap();
    let names: Vec<_> = graph
        .projects
        .iter()
        .map(|project| project.artifact.header.identity.name.as_str())
        .collect();
    assert_eq!(names, ["shared", "left", "right", "app"]);
    assert_eq!(graph.projects[1].artifact.header.dependencies.len(), 1);
    assert_eq!(
        graph.projects[3]
            .artifact
            .header
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
            .map(|project| project.artifact.header.identity.name.as_str())
            .collect::<Vec<_>>(),
        ["shared", "left", "right"]
    );
    assert!(dependencies.projects[1].artifact.header.dependencies.len() == 1);
    assert!(dependencies.projects[2].artifact.header.dependencies.len() == 1);
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
    assert_eq!(linked.lir.globals.len(), 4);
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
        base.join("main.hc"),
        "effect Read = get : {} -> Nat\n\
         type Cases 'r = #A Nat | ..'r\n\
         let read : {} -> Nat + !Read = fn _ => !Read.get {}\n",
    )
    .unwrap();
    write_project(&middle, "middle", "1.0.0", &[("base", "../base")]);
    fs::write(
        middle.join("main.hc"),
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
        app.join("main.hc"),
        "type F = {} -> Nat + base::!Read\n\
         let direct : F = base::read\n\
         let linked : {} -> Nat + middle::!Services = middle::read\n",
    )
    .unwrap();
    compile(&app)
        .expect("imported effects remain in declared types and aliases close transitively");

    fs::write(app.join("main.hc"), "type Bad = base::Cases Nat\n").unwrap();
    let error = compile(&app).unwrap_err().to_string();
    assert!(error.contains("not-a-row"), "{error}");

    fs::write(app.join("main.hc"), "type Bad = base::Cases (#A Nat)\n").unwrap();
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
    assert!(error.contains("both declare bundle same@1.0.0"), "{error}");
}

#[test]
fn imported_signature_types_participate_in_effect_identity() {
    let root = tempfile::tempdir().unwrap();
    let dep = root.path().join("dep");
    let app = root.path().join("app");
    write_project(&dep, "dep", "1.0.0", &[]);
    fs::write(dep.join("main.hc"), "type A = Nat\ntype B = String\n").unwrap();
    write_project(&app, "app", "1.0.0", &[("dep", "../dep")]);
    fs::write(
        app.join("main.hc"),
        "module X = effect Same = op : dep::A -> {} end\n\
         module Y = effect Same = op : dep::B -> {} end\n",
    )
    .unwrap();
    let artifact = compile(&app).unwrap();
    let identities: Vec<_> = artifact
        .header
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"missing.hc\"\n[dependencies]\n",
    )
    .unwrap();
    let root_error = compile(missing.path())
        .expect_err("the configured root is missing")
        .to_string();
    assert!(
        root_error.contains("could not read bundle root"),
        "{root_error}"
    );

    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let bad : Nat = fn x => x\n",
    )
    .unwrap();
    let compiler = error(&directory);
    assert!(compiler.contains("error[types/"), "{compiler}");
    assert!(compiler.contains("main.hc:1:"), "{compiler}");

    let diagnostics = compile(directory.path()).expect_err("the program has a type error");
    assert_eq!(diagnostics.messages().len(), 1, "{diagnostics}");
}

#[test]
fn the_configured_root_must_name_a_file() {
    let directory = tempfile::tempdir().unwrap();
    for root in ["", ".", "..", "src/", "src/.", "/"] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!("name = \"app\"\nversion = \"1.0.0\"\nroot = {root:?}\n[dependencies]\n"),
        )
        .unwrap();
        let error = compile(directory.path())
            .expect_err("the root does not name a file")
            .to_string();
        assert!(
            error.contains("field `root` must name a file"),
            "root `{root}`: {error}"
        );
    }
}

#[test]
fn new_scaffolds_a_compilable_project_without_overwriting() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("nested/my_app");

    new_project(&destination).expect("create the project");
    assert_eq!(
        fs::read_to_string(destination.join("Ruddy.toml")).unwrap(),
        "name = \"my_app\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\n\n[dependencies]\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("main.hc")).unwrap(),
        "let main = 0n\n"
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
    assert_eq!(
        compile(&destination).unwrap().header.identity,
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
        fs::read_to_string(destination.join("main.hc")).unwrap(),
        "let main = 0n\n"
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
    assert!(destination.join("main.hc").is_file());
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

    let path = build_project(&project).expect("build project");
    assert_eq!(path, project.join("build/sample.artifact"));
    let first = fs::read_to_string(&path).unwrap();
    assert_eq!(Artifact::try_parse(&first).unwrap().print(), first);

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
fn build_surfaces_compile_directory_and_artifact_write_failures() {
    let missing = tempfile::tempdir().unwrap();
    let compile_error = build_project(missing.path()).unwrap_err();
    assert!(
        compile_error
            .to_string()
            .contains("could not read manifest")
    );
    assert!(!compile_error.is_usage());

    let blocked_build = tempfile::tempdir().unwrap();
    new_project(blocked_build.path().join("app")).unwrap();
    let project = blocked_build.path().join("app");
    fs::write(project.join("build"), "not a directory").unwrap();
    let error = build_project(&project).unwrap_err().to_string();
    assert!(
        error.contains("could not create build directory"),
        "{error}"
    );

    let blocked_artifact = tempfile::tempdir().unwrap();
    new_project(blocked_artifact.path().join("app")).unwrap();
    let project = blocked_artifact.path().join("app");
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
fn command_arguments_are_cargo_like_and_build_uses_the_current_directory() {
    let current = tempfile::tempdir().unwrap();
    assert_eq!(
        run(["new", "app"], current.path()).unwrap(),
        Outcome::Created(current.path().join("app"))
    );
    assert_eq!(
        run(["build"], current.path().join("app")).unwrap(),
        Outcome::Built(current.path().join("app/build/app.artifact"))
    );

    for (arguments, expected) in [
        (vec![], "expected a subcommand"),
        (vec!["new"], "requires a project path"),
        (vec!["new", "one", "two"], "exactly one project path"),
        (vec!["build", "elsewhere"], "does not accept arguments"),
        (vec!["compile"], "unknown subcommand `compile`"),
    ] {
        let error = run(arguments, current.path()).unwrap_err();
        assert!(error.is_usage());
        assert!(error.to_string().contains(expected), "{error}");
    }
}
