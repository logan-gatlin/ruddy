//! Tests for the filesystem-facing CLI compiler API.

use std::{fs, path::Path};

use ruddy::artifact::{Artifact, Header, Identity, Lir};
use ruddy_cli::{Outcome, build_project, compile, new_project, run};
use tempfile::TempDir;

fn artifact(name: &str, version: &str) -> Artifact {
    Artifact {
        header: Header {
            identity: Identity {
                name: name.to_string(),
                version: version.to_string(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: Lir {
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
}

fn project() -> TempDir {
    let directory = tempfile::tempdir().expect("a temporary project");
    fs::write(
        directory.path().join("main.hc"),
        "bundle app 1.0.0\nlet id = fn x => x\n",
    )
    .expect("write the root");
    directory
}

fn write_artifact(directory: &Path, file: &str, name: &str, version: &str) {
    fs::write(
        directory.join(file),
        artifact(name, version).print().as_bytes(),
    )
    .expect("write the dependency artifact");
}

fn error(directory: &TempDir) -> String {
    compile(directory.path())
        .expect_err("compilation fails")
        .to_string()
}

#[test]
fn manifest_dependencies_reach_the_artifact_in_declaration_order() {
    let directory = project();
    write_artifact(directory.path(), "zeta.artifact", "zeta", "2.0.0");
    write_artifact(directory.path(), "alpha.artifact", "alpha", "1.2.3-beta.1");
    fs::write(
        directory.path().join("Ruddy.toml"),
        "root = \"main.hc\"\n[dependencies]\n\
         zeta = { version = \"2.0.0\", source = \"zeta.artifact\" }\n\
         alpha = { version = \"1.2.3-beta.1\", source = \"alpha.artifact\" }\n",
    )
    .expect("write the manifest");

    let built = compile(directory.path()).expect("compile the project");
    let identities: Vec<_> = built
        .header
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency.version.as_str()))
        .collect();
    assert_eq!(identities, [("zeta", "2.0.0"), ("alpha", "1.2.3-beta.1")]);
    assert_eq!(built.header.identity.name, "app");
    let printed = built.print();
    assert!(printed.contains("(dependency \"zeta\" \"2.0.0\")"));
    assert!(printed.contains("(dependency \"alpha\" \"1.2.3-beta.1\")"));
    assert!(!printed.contains("zeta.artifact"));
    assert!(!printed.contains("alpha.artifact"));
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
        "root = \"src/app.hc\"\n[dependencies]\n",
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
        "root = \"src/app.hc\"\n[dependencies]\n",
    )
    .expect("write the manifest");
    fs::write(
        directory.path().join("src/app.hc"),
        "bundle app 1.0.0\nlet bad : Nat = fn x => x\n",
    )
    .expect("write an invalid root");

    let root_error = error(&directory).replace('\\', "/");
    assert!(root_error.contains("src/app.hc:2:"), "{root_error}");

    fs::write(
        directory.path().join("src/app.hc"),
        "bundle app 1.0.0\nmodule Child\n",
    )
    .expect("replace the root");
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
        ("", "missing field `root`"),
        ("[dependencies]\n", "missing field `root`"),
        ("root = 1\n[dependencies]\n", "invalid type"),
        ("[dependencies", "could not parse manifest"),
        (
            "root = \"main.hc\"\ntitle = \"app\"\n[dependencies]\n",
            "unknown field `title`",
        ),
        ("title = \"app\"\n", "unknown field `title`"),
        (
            "root = \"main.hc\"\n[dependencies]\nbase = { source = \"base.artifact\" }\n",
            "missing field `version`",
        ),
        (
            "root = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\" }\n",
            "missing field `source`",
        ),
        (
            "root = \"main.hc\"\n[dependencies]\nbase = { version = 1, source = \"base.artifact\" }\n",
            "invalid type",
        ),
        (
            "root = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\", source = 1 }\n",
            "invalid type",
        ),
        (
            "root = \"main.hc\"\n[dependencies]\nbase = { version = \"1.0.0\", source = \"base.artifact\", registry = \"x\" }\n",
            "unknown field `registry`",
        ),
    ] {
        fs::write(directory.path().join("Ruddy.toml"), manifest).expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn dependency_names_and_versions_must_be_valid_artifact_identities() {
    let directory = project();
    for (name, version, expected) in [
        ("base", "not-semver", "invalid semantic version"),
        ("not.a.name", "1.0.0", "not a valid Ruddy bundle name"),
        ("base", "1.0.0+local", "unsupported build metadata"),
    ] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!(
                "root = \"main.hc\"\n[dependencies]\n\"{name}\" = {{ version = \"{version}\", source = \"base.artifact\" }}\n"
            ),
        )
        .expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn dependency_sources_must_be_readable_canonical_matching_artifacts() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "root = \"main.hc\"\n[dependencies]\nbase = { version = \"1.2.3\", source = \"base.artifact\" }\n",
    )
    .expect("write the manifest");

    let unreadable = error(&directory);
    assert!(unreadable.contains("could not read source for dependency `base`"));
    assert!(unreadable.contains("base.artifact"));

    fs::write(directory.path().join("base.artifact"), "not an artifact")
        .expect("write malformed artifact text");
    let malformed = error(&directory);
    assert!(malformed.contains("is not a Ruddy artifact"), "{malformed}");

    fs::write(
        directory.path().join("base.artifact"),
        format!(" {}", artifact("base", "1.2.3").print()),
    )
    .expect("write noncanonical artifact text");
    let noncanonical = error(&directory);
    assert!(
        noncanonical.contains("is not in canonical artifact form"),
        "{noncanonical}"
    );

    write_artifact(directory.path(), "base.artifact", "other", "1.2.3");
    let wrong_name = error(&directory);
    assert!(
        wrong_name.contains("contains artifact `other` instead"),
        "{wrong_name}"
    );

    write_artifact(directory.path(), "base.artifact", "base", "2.0.0");
    let wrong_version = error(&directory);
    assert!(
        wrong_version.contains("requests version `1.2.3`")
            && wrong_version.contains("contains version `2.0.0`"),
        "{wrong_version}"
    );
}

#[test]
fn bundle_and_compiler_failures_are_returned_as_cli_diagnostics() {
    let missing = tempfile::tempdir().unwrap();
    fs::write(
        missing.path().join("Ruddy.toml"),
        "root = \"missing.hc\"\n[dependencies]\n",
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
        "root = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "bundle app 1.0.0\nlet bad : Nat = fn x => x\n",
    )
    .unwrap();
    let compiler = error(&directory);
    assert!(compiler.contains("error[types/"), "{compiler}");
    assert!(compiler.contains("main.hc:2:"), "{compiler}");

    let diagnostics = compile(directory.path()).expect_err("the program has a type error");
    assert_eq!(diagnostics.messages().len(), 1, "{diagnostics}");
}

#[test]
fn the_configured_root_must_name_a_file() {
    let directory = tempfile::tempdir().unwrap();
    for root in ["", ".", "..", "src/", "src/.", "/"] {
        fs::write(
            directory.path().join("Ruddy.toml"),
            format!("root = {root:?}\n[dependencies]\n"),
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
        "root = \"main.hc\"\n\n[dependencies]\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("main.hc")).unwrap(),
        "bundle my_app 0.1.0\n\nlet main = 0n\n"
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
        "bundle my_app 0.1.0\n\nlet main = 0n\n"
    );
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
    assert_eq!(fs::read_to_string(path).unwrap(), first);
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
