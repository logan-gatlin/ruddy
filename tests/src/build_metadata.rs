//! Exercise the CLI build script as Cargo does, without changing this checkout.
use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn compiler_revision_metadata_handles_archives_refs_and_explicit_builds() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join(format!("build-script{}", std::env::consts::EXE_SUFFIX));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/build.rs");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = temp.path().join("repo");
    let cli = root.join("cli");
    fs::create_dir_all(&cli).unwrap();
    let run = |manifest: &Path, revision: Option<&str>| {
        let mut command = Command::new(&executable);
        command
            .env("CARGO_MANIFEST_DIR", manifest)
            .env("CARGO_PKG_VERSION", "0.1.0")
            .env_remove("RUDDY_BUILD_REVISION");
        if let Some(revision) = revision {
            command.env("RUDDY_BUILD_REVISION", revision);
        }
        command.output().unwrap()
    };
    let text = |output: std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(text(run(&cli, None)).contains("cargo:rustc-env=RUDDY_BUILD_REVISION=\n"));
    assert!(!run(&cli, Some("bad")).status.success());
    assert!(
        text(run(&cli, Some(&"A".repeat(40))))
            .contains(&format!("RUDDY_BUILD_REVISION={}", "a".repeat(40)))
    );

    git(&root, &["init", "-q"]);
    fs::write(cli.join("file"), "fixture").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Tests",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    let revision = git(&root, &["rev-parse", "HEAD"]);
    let expected = format!("cargo:rustc-env=RUDDY_BUILD_REVISION={revision}");
    let output = text(run(&cli, None));
    assert!(output.contains(&expected));
    assert!(output.contains("HEAD"));
    assert!(output.contains("refs/heads"));

    // Never mistake a containing repository's revision for an archive's.
    let archive = cli.join("archive/cli");
    fs::create_dir_all(&archive).unwrap();
    assert!(text(run(&archive, None)).contains("cargo:rustc-env=RUDDY_BUILD_REVISION=\n"));

    git(&root, &["pack-refs", "--all"]);
    assert!(text(run(&cli, None)).contains("packed-refs"));
    git(&root, &["checkout", "--detach", "-q", &revision]);
    assert!(text(run(&cli, None)).contains(&expected));

    let worktree = temp.path().join("worktree");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            &revision,
        ],
    );
    assert!(text(run(&worktree.join("cli"), None)).contains(&expected));
}
