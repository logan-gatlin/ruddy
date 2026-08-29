//! Tests for the filesystem-facing CLI compiler API.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ruddy::artifact::Artifact;
use ruddy_cli::{
    Lockfile, Outcome, build_project, check_project, clean_project, compile,
    execute_javascript_module, new_project, run, run_project,
};
use tempfile::TempDir;

fn project() -> TempDir {
    let directory = tempfile::tempdir().expect("a temporary project");
    fs::write(directory.path().join("main.hc"), "let id = fn x => x\n").expect("write the root");
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
    fs::write(directory.join("main.hc"), "let value = 0n\n").unwrap();
    let mut manifest = format!(
        "name = {name:?}\nversion = {version:?}\nroot = \"main.hc\"\n[dependencies]\nstd = false\n"
    );
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
        ("{}", "either `path` or `git`"),
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
                "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = {setting}\n"
            ),
        )
        .unwrap();
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
        assert!(!directory.path().join("Ruddy.lock").exists());
    }

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\nstd = false\n[dependencies]\n",
    )
    .unwrap();
    let found = error(&directory);
    assert!(found.contains("unknown field `std`"), "{found}");
}

#[cfg(unix)]
#[test]
fn bundled_std_installer_first_install_does_not_require_gnu_mv_flags() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "").unwrap();

    let real_mv = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v mv"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fake_mv = bin.join("mv");
    fs::write(
        &fake_mv,
        format!(
            "#!/bin/sh\ncase ${{1-}} in -*) echo 'nonportable mv option' >&2; exit 97;; esac\nexec {} \"$@\"\n",
            real_mv.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o755)).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("PATH", path)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join("std/Ruddy.toml").is_file());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_replaces_only_source_files() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    fs::create_dir_all(source.join("Nested")).unwrap();
    fs::create_dir_all(source.join("build")).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "").unwrap();
    fs::write(source.join("Nested/module.hc"), "let value = 0n\n").unwrap();
    fs::write(source.join("build/app.artifact"), "artifact").unwrap();
    fs::write(source.join("notes.txt"), "notes").unwrap();
    fs::write(home.join("std/old.hc"), "old").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(&script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "manifest"
    );
    assert!(home.join("std/main.hc").is_file());
    assert!(home.join("std/Nested/module.hc").is_file());
    assert!(!home.join("std/old.hc").exists());
    assert!(!home.join("std/build").exists());
    assert!(!home.join("std/notes.txt").exists());

    fs::remove_file(source.join("main.hc")).unwrap();
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(home.join("std/Nested/module.hc").is_file());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_releases_lock_when_old_tree_cleanup_fails() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "new main").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();
    fs::write(home.join("std/main.hc"), "old main").unwrap();

    let real_rm = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v rm"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fake_rm = bin.join("rm");
    fs::write(
        &fake_rm,
        "#!/bin/sh\nfor arg do\n  case $arg in\n    \"$FAIL_HOME\"/.std.install.*)\n      if [ -f \"$arg/Ruddy.toml\" ] && grep -qx old \"$arg/Ruddy.toml\"; then\n        echo 'injected old-tree removal failure' >&2\n        exit 88\n      fi\n      ;;\n  esac\ndone\nexec \"$REAL_RM\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_rm, fs::Permissions::from_mode(0o755)).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("FAIL_HOME", &home)
        .env("REAL_RM", real_rm.trim())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("HOME")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "new"
    );
    assert!(!home.join(".std.install.lock").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("installed the new standard library but could not remove previous tree"),
        "{stderr}"
    );
    assert!(fs::read_dir(&home).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".std.install.")
            && fs::read_to_string(path.join("Ruddy.toml")).is_ok_and(|contents| contents == "old")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_discovery_failure_preserves_existing_installation() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(source.join("Nested")).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "new main").unwrap();
    fs::write(source.join("Nested/module.hc"), "new nested").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();
    fs::write(home.join("std/main.hc"), "old main").unwrap();

    let real_find = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v find"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fake_find = bin.join("find");
    fs::write(
        &fake_find,
        format!("#!/bin/sh\n{} \"$@\"\nexit 73\n", real_find.trim()),
    )
    .unwrap();
    fs::set_permissions(&fake_find, fs::Permissions::from_mode(0o755)).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(73));
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(home.join("std/main.hc")).unwrap(),
        "old main"
    );
    assert!(fs::read_dir(&home).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".std.install.")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_reports_missing_exchange_capability_before_copying() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();
    let fake_mv = bin.join("mv");
    fs::write(&fake_mv, "#!/bin/sh\necho 'minimal mv'\n").unwrap();
    fs::set_permissions(&fake_mv, fs::Permissions::from_mode(0o755)).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires Linux and GNU mv with --exchange"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "old"
    );
    assert!(!fs::read_dir(&home).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".std.install.")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_never_hides_an_existing_installation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::{thread, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let barrier = root.path().join("commit-barrier");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let mut child = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_BARRIER", &barrier)
        .env_remove("HOME")
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if barrier.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(barrier.exists(), "installer did not finish staging");

    let stop = Arc::new(AtomicBool::new(false));
    let observations = Arc::new(AtomicUsize::new(0));
    let observer_home = home.clone();
    let observer_stop = Arc::clone(&stop);
    let observer_observations = Arc::clone(&observations);
    let observer = thread::spawn(move || {
        while !observer_stop.load(Ordering::Acquire) {
            let manifest = fs::read_to_string(observer_home.join("std/Ruddy.toml"))
                .expect("std must remain visible throughout replacement");
            assert!(manifest == "old" || manifest == "new", "{manifest:?}");
            observer_observations.fetch_add(1, Ordering::Relaxed);
        }
    });
    while observations.load(Ordering::Relaxed) == 0 {
        thread::yield_now();
    }
    fs::remove_file(&barrier).unwrap();
    let status = child.wait().unwrap();
    stop.store(true, Ordering::Release);
    observer.join().unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "new"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_interruption_preserves_existing_installation() {
    use std::{thread, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let barrier = root.path().join("commit-barrier");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let mut child = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_BARRIER", &barrier)
        .env_remove("HOME")
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if barrier.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(barrier.exists(), "installer did not finish staging");
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(!child.wait().unwrap().success());

    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "old"
    );
    assert!(fs::read_dir(&home).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".std.install.")
    }));
}

#[cfg(unix)]
#[test]
fn bundled_std_installer_resolves_symlinked_metacharacter_source_roots() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source[odd]*?");
    let source_link = root.path().join("source-link");
    let home = root.path().join("home");
    fs::create_dir_all(source.join("Nested")).unwrap();
    fs::create_dir_all(source.join("build")).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "main").unwrap();
    fs::write(source.join("Nested/module.hc"), "nested").unwrap();
    fs::write(source.join("build/generated.hc"), "generated").unwrap();
    symlink(&source, &source_link).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(source_link)
        .env("RUDDY_HOME", &home)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(home.join("std/main.hc")).unwrap(),
        "main"
    );
    assert_eq!(
        fs::read_to_string(home.join("std/Nested/module.hc")).unwrap(),
        "nested"
    );
    assert!(!home.join("std/build").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_serializes_concurrent_first_installs() {
    use std::{thread, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let home = root.path().join("home");
    let barrier = root.path().join("first-barrier");
    for (source, contents) in [(&first, "first"), (&second, "second")] {
        fs::create_dir_all(source).unwrap();
        fs::write(source.join("Ruddy.toml"), contents).unwrap();
        fs::write(source.join("main.hc"), contents).unwrap();
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let mut first_child = Command::new(&script)
        .arg(&first)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_BARRIER", &barrier)
        .env_remove("HOME")
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if barrier.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(barrier.exists(), "first installer did not finish staging");

    let mut second_child = Command::new(&script)
        .arg(&second)
        .env("RUDDY_HOME", &home)
        .env_remove("HOME")
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(second_child.try_wait().unwrap().is_none());
    assert!(!home.join("std").exists());

    fs::remove_file(&barrier).unwrap();
    assert!(first_child.wait().unwrap().success());
    assert!(second_child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "second"
    );
    assert!(!home.join("std/.std.install.lock").exists());
    assert!(fs::read_dir(&home).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.starts_with(".std.install.") && !name.starts_with(".std.exchange.")
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_preserves_a_stale_lock_for_manual_recovery() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join(".std.install.lock")).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "main").unwrap();
    fs::write(home.join(".std.install.lock/owner"), "999999999\n").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(&script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_LOCK_ATTEMPTS", "1")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out waiting"), "{stderr}");
    assert!(
        stderr.contains("after verifying that no installer is running"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(home.join(".std.install.lock/owner")).unwrap(),
        "999999999\n"
    );
    assert!(!home.join("std").exists());

    // Manual recovery is intentionally separate from observation, so the
    // installer can never unlink a newer claimant based on stale metadata.
    fs::remove_dir_all(home.join(".std.install.lock")).unwrap();
    let retry = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(home.join("std/main.hc").is_file());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_never_reaps_a_new_owner_after_waiting() {
    use std::{thread, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let lock = home.join(".std.install.lock");
    let barrier = root.path().join("lock-wait-barrier");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&lock).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "main").unwrap();
    fs::write(lock.join("owner"), "stale owner\n").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let mut child = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_LOCK_ATTEMPTS", "1")
        .env("_RUDDY_INSTALL_STD_TEST_LOCK_WAIT_BARRIER", &barrier)
        .env_remove("HOME")
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if barrier.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        barrier.exists(),
        "installer did not reach lock wait barrier"
    );

    // Atomically publish replacement metadata while the contender is paused at
    // exactly the point where the former implementation had observed an owner.
    fs::write(lock.join("replacement"), "new owner\n").unwrap();
    fs::rename(lock.join("replacement"), lock.join("owner")).unwrap();
    fs::remove_file(&barrier).unwrap();

    assert!(!child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(lock.join("owner")).unwrap(),
        "new owner\n"
    );
    assert!(!home.join("std").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_does_not_reap_an_empty_lock() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let lock = home.join(".std.install.lock");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&lock).unwrap();
    fs::write(source.join("Ruddy.toml"), "manifest").unwrap();
    fs::write(source.join("main.hc"), "main").unwrap();
    fs::write(lock.join("owner"), "").unwrap();
    fs::write(lock.join("candidate.abandoned"), "999999 abandoned\n").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env("_RUDDY_INSTALL_STD_TEST_LOCK_ATTEMPTS", "1")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("remove it with: rm -rf"));
    assert_eq!(fs::read_to_string(lock.join("owner")).unwrap(), "");
    assert_eq!(
        fs::read_to_string(lock.join("candidate.abandoned")).unwrap(),
        "999999 abandoned\n"
    );
    assert!(!home.join("std").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn bundled_std_installer_cleans_a_probe_interrupted_during_creation() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(home.join("std")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(source.join("Ruddy.toml"), "new").unwrap();
    fs::write(source.join("main.hc"), "new").unwrap();
    fs::write(home.join("std/Ruddy.toml"), "old").unwrap();
    let real_mktemp = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v mktemp"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fake_mktemp = bin.join("mktemp");
    fs::write(
        &fake_mktemp,
        format!(
            "#!/bin/sh\nresult=$({} \"$@\") || exit\nprintf '%s\\n' \"$result\"\nkill -TERM \"$PPID\"\n",
            real_mktemp.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_mktemp, fs::Permissions::from_mode(0o755)).unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/install-std.sh");
    let output = Command::new(script)
        .arg(&source)
        .env("RUDDY_HOME", &home)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(home.join("std/Ruddy.toml")).unwrap(),
        "old"
    );
    assert!(fs::read_dir(&home).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.starts_with(".std.exchange.") && !name.starts_with(".std.install.")
    }));
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
    fs::write(root.join("main.hc"), "let main = 0n\n").unwrap();

    match mode.as_str() {
        "default" | "relative" => {
            let standard = ruddy_cli::ruddy_home().unwrap().join("std");
            assert!(standard.is_absolute());
            write_project(&standard, "std", "1.0.0", &[]);
            fs::create_dir(standard.join("build")).unwrap();
            fs::write(standard.join("build/sentinel"), "keep").unwrap();
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
            )
            .unwrap();

            let graph = ruddy_cli::compile_graph(&root).unwrap();
            assert_eq!(
                graph
                    .projects
                    .iter()
                    .map(|project| project.artifact.header.identity.name.as_str())
                    .collect::<Vec<_>>(),
                ["std", "app"]
            );
            assert_eq!(
                graph.projects[0].source,
                ruddy_cli::ProjectSource::InstalledStd
            );
            let artifact = build_project(&root).unwrap();
            assert_eq!(artifact, root.join("build/app.artifact"));
            assert_eq!(
                fs::read_to_string(standard.join("build/sentinel")).unwrap(),
                "keep"
            );

            fs::remove_file(standard.join("Ruddy.toml")).unwrap();
            let found = compile(&root).unwrap_err().to_string();
            assert!(
                found.contains("help: install the Ruddy standard library"),
                "{found}"
            );
            assert!(found.contains("configure `[dependencies].std`"), "{found}");
            assert!(found.contains("set it to `false`"), "{found}");
        }
        "custom" => {
            write_project(&root.join("standard"), "std", "2.0.0", &[]);
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"standard\"\n",
            )
            .unwrap();
            assert_eq!(compile(&root).unwrap().header.identity.name, "app");
        }
        "disabled" => {
            fs::write(
                root.join("Ruddy.toml"),
                "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
            )
            .unwrap();
            assert_eq!(compile(&root).unwrap().header.identity.name, "app");
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
    let standard = PathBuf::from(std::env::var_os("RUDDY_HOME").unwrap()).join("std");
    write_project(&standard, "std", "1.0.0", &[]);

    let dedup = root.join("dedup");
    write_project(&dedup.join("dep"), "dep", "1.0.0", &[]);
    fs::write(
        dedup.join("dep/Ruddy.toml"),
        "name = \"dep\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    fs::create_dir_all(&dedup).unwrap();
    fs::write(dedup.join("main.hc"), "let main = 0n\n").unwrap();
    fs::write(
        dedup.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\ndep = \"dep\"\n",
    )
    .unwrap();
    let graph = ruddy_cli::compile_graph(&dedup).unwrap();
    assert_eq!(
        graph
            .projects
            .iter()
            .map(|project| project.artifact.header.identity.name.as_str())
            .collect::<Vec<_>>(),
        ["std", "dep", "app"]
    );
    assert_eq!(
        graph.projects[1].artifact.header.dependencies[0].name,
        "std"
    );
    assert_eq!(
        graph.projects[2]
            .artifact
            .header
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
        "name = \"dep\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = \"../std2\"\n",
    )
    .unwrap();
    fs::write(versions.join("main.hc"), "let main = 0n\n").unwrap();
    fs::write(
        versions.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\ndep = \"dep\"\n",
    )
    .unwrap();
    let graph = ruddy_cli::compile_graph(&versions).unwrap();
    let std_versions = graph
        .projects
        .iter()
        .filter(|project| project.artifact.header.identity.name == "std")
        .map(|project| project.artifact.header.identity.version.as_str())
        .collect::<Vec<_>>();
    assert_eq!(std_versions, ["1.0.0", "2.0.0"]);

    fs::write(
        versions.join("std2/Ruddy.toml"),
        "name = \"std\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let found = compile(&versions).unwrap_err().to_string();
    assert!(found.contains("both declare bundle std@1.0.0"), "{found}");

    let cycle = root.join("cycle");
    fs::create_dir_all(&cycle).unwrap();
    fs::write(cycle.join("main.hc"), "let main = 0n\n").unwrap();
    fs::write(
        cycle.join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\n",
    )
    .unwrap();
    fs::write(
        standard.join("Ruddy.toml"),
        format!(
            "name = \"std\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\napp = {{ path = {:?} }}\n",
            cycle
        ),
    )
    .unwrap();
    let found = compile(&cycle).unwrap_err().to_string();
    assert!(found.contains("dependency cycle"), "{found}");
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
        directory.path().join("standard/main.hc"),
        "let answer = 42n\n",
    )
    .unwrap();
    fs::write(directory.path().join("main.hc"), "let main = std::answer\n").unwrap();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = { path = \"standard\", bundle = \"foundation\" }\n",
    )
    .unwrap();

    let graph = ruddy_cli::compile_graph(directory.path()).unwrap();
    assert_eq!(
        graph
            .projects
            .iter()
            .map(|project| project.artifact.header.identity.name.as_str())
            .collect::<Vec<_>>(),
        ["foundation", "app"]
    );
    assert_eq!(
        graph.projects[1].artifact.header.dependencies[0].name,
        "foundation"
    );
    assert!(
        compile(directory.path())
            .unwrap()
            .print()
            .contains("foundation@2.1.0::answer")
    );
}

#[test]
fn duplicate_std_settings_have_a_focused_error() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nstd = \"vendor/std\"\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nzeta = \"zeta\"\nalpha = \"alpha\"\n",
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
        "module Nested =\n  type Number = Nat\n  effect Read = { get: {} -> Nat }\n  extern runtime : Nat = host.runtime\n  let foo = 1n\nend\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("main.hc"),
        "let value : std::Nested::Number = std::Nested::foo\nlet imported = std::Nested::runtime\nlet operation = std::Nested::!Read.get\n",
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
    assert_eq!(built.lir.externs.len(), 1);
    assert_eq!(built.lir.externs[0].name, "std@0.1.0::Nested::runtime");
    assert_eq!(built.lir.externs[0].target, ["host", "runtime"]);
    assert_eq!(built.header.values.len(), 3);
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nhttp_core = { bundle = \"http-core\", path = \"http-core\" }\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nhttp_core = { package = \"http-core\", path = \"http-core\" }\n",
    )
    .unwrap();
    let old_field_error = error(&directory);
    assert!(
        old_field_error.contains("unknown field `package`"),
        "{old_field_error}"
    );

    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nhttp-core = \"http-core\"\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"src/app.hc\"\n[dependencies]\nstd = false\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"src/app.hc\"\n[dependencies]\nstd = false\n",
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
            "name = 1\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = 1\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = 1\n[dependencies]\n",
            "invalid type",
        ),
        ("[dependencies", "could not parse manifest"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\ntitle = \"app\"\n[dependencies]\nstd = false\n",
            "unknown field `title`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[run]\njs = 1\n[dependencies]\nstd = false\n",
            "invalid type",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[run]\njs = \"node\"\nnative = \"app\"\n[dependencies]\nstd = false\n",
            "unknown field `native`",
        ),
        ("title = \"app\"\n", "unknown field `title`"),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { source = \"base.artifact\" }\n",
            "unknown field `source`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { version = 1, source = \"base.artifact\" }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\", source = 1 }\n",
            "unknown field `version`",
        ),
        (
            "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { version = \"1.0.0\", source = \"base.artifact\", registry = \"x\" }\n",
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
        fs::write(directory.path().join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = {specification}\n")).unwrap();
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
    fs::write(app.join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { git = \"https://example.invalid/repository\" }\n").unwrap();
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
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { git = \"https://127.0.0.1:9/repository\", branch = \"main\" }\n").unwrap();
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
        .write_blob(b"name = \"base\"\nversion = \"2.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n")
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
    fs::write(app.join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = {{ git = {url:?}, branch = \"main\" }}\n")).unwrap();
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
        "name = \"poison\"\nversion = \"9.9.9\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
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
            "name = \"path-app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = {{ path = {:?} }}\n",
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
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = { git = \"https://127.0.0.1:9/repository\", rev = \"abc123\" }\n").unwrap();
    let found = error(&directory);
    assert!(found.contains("7 to 40 hexadecimal digits"), "{found}");
    assert!(!directory.path().join("Ruddy.lock").exists());
}

#[test]
fn a_successful_build_removes_stale_git_entries_from_an_existing_lockfile() {
    let directory = project();
    fs::write(
        directory.path().join("Ruddy.toml"),
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
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
            format!("name = {name:?}\nversion = {version:?}\nroot = \"main.hc\"\n[dependencies]\nstd = false\n"),
        )
        .expect("replace the manifest");
        let found = error(&directory);
        assert!(found.contains(expected), "`{expected}` in:\n{found}");
    }
}

#[test]
fn dependency_projects_must_exist_compile_and_match_the_table_key() {
    let directory = project();
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = \"missing\"\n").unwrap();
    assert!(error(&directory).contains("dependency `base`"));

    write_project(&directory.path().join("child"), "other", "1.0.0", &[]);
    fs::write(directory.path().join("Ruddy.toml"), "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\nbase = \"child\"\n").unwrap();
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
    for (project, declaration) in [
        (&shared, "extern host : Nat = shared.host\nlet value = 0n\n"),
        (&left, "extern host : Nat = left.host\nlet value = 0n\n"),
        (&right, "extern host : Nat = right.host\nlet value = 0n\n"),
        (&app, "extern host : Nat = app.host\nlet value = 0n\n"),
    ] {
        fs::write(project.join("main.hc"), declaration).unwrap();
    }

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
    assert_eq!(
        linked
            .lir
            .externs
            .iter()
            .map(|external| (external.name.as_str(), external.target.join(".")))
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
        base.join("main.hc"),
        "effect Read = { get: {} -> Nat }\n\
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
        "module X = effect Same = { op: dep::A -> {} } end\n\
         module Y = effect Same = { op: dep::B -> {} } end\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"missing.hc\"\n[dependencies]\nstd = false\n",
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
        "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\n[dependencies]\nstd = false\n",
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
            format!("name = \"app\"\nversion = \"1.0.0\"\nroot = {root:?}\n[dependencies]\nstd = false\n"),
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
    let scaffold_manifest = fs::read_to_string(destination.join("Ruddy.toml")).unwrap();
    assert_eq!(
        scaffold_manifest,
        "name = \"my_app\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\n\n[dependencies]\n"
    );
    assert!(
        !scaffold_manifest
            .lines()
            .any(|line| line.starts_with("std ="))
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
    disable_std(&destination);
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
    disable_std(&project);

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
fn manifest_targets_select_root_javascript_output() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    new_project(&app).unwrap();
    disable_std(&app);

    // Omission and an explicit library target retain artifact-only behavior.
    let artifact = build_project(&app).unwrap();
    assert!(artifact.is_file());
    assert!(!app.join("build/app.js").exists());
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("root = \"main.hc\"", "root = \"main.hc\"\ntarget = \"lib\""),
    )
    .unwrap();
    build_project(&app).unwrap();
    assert!(!app.join("build/app.js").exists());

    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("target = \"lib\"", "target = \"js\""),
    )
    .unwrap();
    assert_eq!(build_project(&app).unwrap(), artifact);
    let javascript = fs::read_to_string(app.join("build/app.js")).unwrap();
    assert!(!javascript.is_empty());

    // Switching back does not implicitly clean a stale sibling product.
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace("target = \"js\"", "target = \"lib\""),
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
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
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
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
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
                "name = \"app\"\nversion = \"1.0.0\"\nroot = \"main.hc\"\ntarget = {target}\n[dependencies]\nstd = false\n"
            ),
        )
        .unwrap();
        let error = compile(directory.path()).unwrap_err().to_string();
        assert!(error.contains("could not parse manifest"), "{error}");
    }
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
fn run_builds_and_evaluates_a_javascript_module_without_a_main_entrypoint() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    write_project(&app, "app", "1.0.0", &[]);
    fs::write(app.join("main.hc"), "let initialized = 1n\n").unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    let expected = app.join("build/app.js");
    assert_eq!(run_project(&app).unwrap(), expected);
    assert_eq!(run(["run"], &app).unwrap(), Outcome::Ran(expected.clone()));
    assert!(expected.is_file());
    assert!(app.join("build/app.artifact").is_file());

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
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n\n[run]\njs = \"sh runner.sh marker.txt\"\n\n[dependencies]",
        ),
    )
    .unwrap();
    fs::write(
        app.join("runner.sh"),
        "#!/bin/sh\nprintf '%s' \"$2\" > \"$1\"\n",
    )
    .unwrap();

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
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n\n[run]\njs = \"false\"\n\n[dependencies]",
        ),
    )
    .unwrap();

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
    assert!(app.join("build/app.artifact").is_file());
}

#[test]
fn run_registers_the_standard_runtime_bundle() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("runtime");
    write_project(&app, "runtime", "1.0.0", &[]);
    fs::write(
        app.join("main.hc"),
        "extern cwd : {} -> String = process.cwd\n\
         extern environment : {} = process.env\n\
         extern console_object : {} = console\n\
         extern url : {} = URL\n\
         extern encoder : {} = TextEncoder\n\
         extern decoder : {} = TextDecoder\n\
         extern base64 : String -> String = btoa\n\
         extern clone : {} -> {} = structuredClone\n\
         extern microtask : ({} -> {}) -> {} = queueMicrotask\n\
         extern timeout : ({} -> {}) -> Nat = setTimeout\n\
         extern abort_controller : {} = AbortController\n\
         extern fetch_value : String -> {} = fetch\n\
         let initialized = 0n\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    run_project(&app).expect("all documented Boa runtime globals are registered");
}

#[test]
fn run_drains_queued_jobs_and_preserves_installed_files_on_runtime_failure() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("queued");
    write_project(&app, "queued", "1.0.0", &[]);
    fs::write(
        app.join("main.hc"),
        "extern queue : ({} -> {}) -> {} = queueMicrotask\n\
         extern parse : String -> {} = JSON.parse\n\
         let queued = queue (fn _ => parse \"{\")\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(!error.is_usage());
    let rendered = error.to_string().replace('\\', "/");
    assert!(rendered.contains("JavaScript runtime"), "{rendered}");
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
fn javascript_reports_unhandled_promises_despite_recurring_jobs() {
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
    assert!(
        error.to_string().contains("unhandled promise rejection"),
        "{error}"
    );
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
    assert!(
        error.to_string().contains("unhandled promise rejection"),
        "{error}"
    );
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
        app.join("main.hc"),
        "effect Tick = Nat -> Nat\n\
         extern run : fn('a) -> Nat = host.run\n\
         let result = handle run (fn n => !Tick n) with\n\
           | !Tick n => n\n\
         end\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string();
    assert!(
        rendered.contains("polymorphic-extern-boundary"),
        "{rendered}"
    );
    assert!(
        rendered.contains("fixed runtime representation"),
        "{rendered}"
    );
    assert!(
        !app.join("build/polymorphic-callback.js").exists(),
        "an unsound module reached execution"
    );
}

#[test]
fn run_reports_missing_externs_with_javascript_source_locations() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("missing");
    write_project(&app, "missing", "1.0.0", &[]);
    fs::write(
        app.join("main.hc"),
        "extern unavailable : Nat = ruddy_runtime.unavailable\n",
    )
    .unwrap();
    let manifest = fs::read_to_string(app.join("Ruddy.toml")).unwrap();
    fs::write(
        app.join("Ruddy.toml"),
        manifest.replace(
            "root = \"main.hc\"\n[dependencies]",
            "root = \"main.hc\"\ntarget = \"js\"\n[dependencies]",
        ),
    )
    .unwrap();

    let error = run_project(&app).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    let rendered = error.to_string().replace('\\', "/");
    assert!(rendered.contains("missing Ruddy extern"), "{rendered}");
    assert!(rendered.contains("ruddy_runtime.unavailable"), "{rendered}");
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

    fs::write(app.join("main.hc"), "let bad : Nat = false\n").unwrap();
    let error = check_project(&app).unwrap_err();
    assert!(!error.is_usage());
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("error[types/"), "{error}");
    assert!(!app.join("build").exists());
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
