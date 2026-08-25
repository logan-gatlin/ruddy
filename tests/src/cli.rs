//! Tests for the filesystem-facing CLI compiler API.

use std::{fs, path::Path};

use ruddy::artifact::{Artifact, Header, Identity, Lir};
use ruddy_cli::compile;
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
    fs::write(
        directory.path().join("Ruddy.toml"),
        "root = \"/\"\n[dependencies]\n",
    )
    .unwrap();
    let error = compile(directory.path())
        .expect_err("the root does not name a file")
        .to_string();
    assert!(error.contains("field `root` must name a file"), "{error}");
}
