//! Real shallow fetches against a local HTTPS Git server.
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn shallow_git_dependencies_and_default_std() {
    check_transport("version=2");
}

#[test]
fn legacy_git_protocol_is_rejected_before_downloading_history() {
    check_transport("");
}

fn check_transport(protocol: &str) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo.git");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("Ruddy.toml"), "name = \"std\"\nkind = \"library\"\nversion = \"1.0.0\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n").unwrap();
    let mut old = String::new();
    let mut untagged = String::new();
    for value in 0..8 {
        fs::write(repo.join("main.rud"), format!("let value = {value}n\n")).unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
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
        if value == 1 {
            untagged = git(&repo, &["rev-parse", "HEAD"]);
        }
        if value == 2 {
            old = git(&repo, &["rev-parse", "HEAD"]);
        }
    }
    git(
        &repo,
        &[
            "-c",
            "user.name=Tests",
            "-c",
            "user.email=test@example.invalid",
            "tag",
            "-a",
            "v1",
            &old,
            "-m",
            "release",
            "--cleanup=verbatim",
        ],
    );
    let tip = git(&repo, &["rev-parse", "HEAD"]);
    let mut server = Server(
        Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/git_https.py"))
            .arg(root.path())
            .env("RUDDY_TEST_GIT_PROTOCOL", protocol)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut port = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    assert!(
        !port.trim().is_empty(),
        "HTTPS fixture failed to start (requires Python 3 and OpenSSL)"
    );
    let url = format!("https://127.0.0.1:{}/repo.git", port.trim());
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "git_transport::shallow_git_child"])
        .env("RUDDY_HOME", root.path().join("home"))
        .env_remove("HOME")
        .env("RUDDY_TEST_TRANSPORT_ROOT", root.path())
        .env("RUDDY_TEST_TRANSPORT_URL", &url)
        .env("RUDDY_TEST_TRANSPORT_PROTOCOL", protocol)
        .env("RUDDY_TEST_TRANSPORT_OLD", &old)
        .env("RUDDY_TEST_TRANSPORT_UNTAGGED", &untagged)
        .env("RUDDY_TEST_TRANSPORT_TIP", &tip)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", format!("url.{url}.insteadOf"))
        .env("GIT_CONFIG_VALUE_0", ruddy_cli::DEFAULT_STD_GIT)
        .env("GIT_CONFIG_KEY_1", "http.sslCAInfo")
        .env("GIT_CONFIG_VALUE_1", root.path().join("ca.pem"))
        .env("SSL_CERT_FILE", root.path().join("ca.pem"))
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
#[ignore = "isolated HTTPS transport environment supplied by parent"]
fn shallow_git_child() {
    let root = std::path::PathBuf::from(std::env::var_os("RUDDY_TEST_TRANSPORT_ROOT").unwrap());
    let url = std::env::var("RUDDY_TEST_TRANSPORT_URL").unwrap();
    let old = std::env::var("RUDDY_TEST_TRANSPORT_OLD").unwrap();
    let untagged = std::env::var("RUDDY_TEST_TRANSPORT_UNTAGGED").unwrap();
    let tip = std::env::var("RUDDY_TEST_TRANSPORT_TIP").unwrap();
    if std::env::var("RUDDY_TEST_TRANSPORT_PROTOCOL")
        .unwrap()
        .is_empty()
    {
        let app = root.join("legacy");
        fs::create_dir(&app).unwrap();
        fs::write(app.join("main.rud"), "let value = 0n\n").unwrap();
        fs::write(
            app.join("Ruddy.toml"),
            "name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\n",
        )
        .unwrap();
        let error = ruddy_cli::compile_graph(&app).unwrap_err();
        assert!(error.to_string().contains("protocol version 2"), "{error}");
        assert!(!app.join("Ruddy.lock").exists());
        assert!(!root.join("home/cache/git/checkouts").exists());
        return;
    }
    for (name, setting, expected, shallow) in [
        ("default", String::new(), tip.clone(), true),
        ("shared", String::new(), tip.clone(), true),
        (
            "branch",
            format!("std = {{ git = {url:?}, branch = \"main\" }}"),
            tip.clone(),
            true,
        ),
        (
            "tag",
            format!("std = {{ git = {url:?}, tag = \"v1\" }}"),
            old.clone(),
            true,
        ),
        (
            "revision",
            format!("std = {{ git = {url:?}, rev = {old:?} }}"),
            old.clone(),
            true,
        ),
        (
            "prefix",
            format!("std = {{ git = {url:?}, rev = {:?} }}", &untagged[..9]),
            untagged.clone(),
            false,
        ),
    ] {
        let app = root.join(name);
        fs::create_dir(&app).unwrap();
        fs::write(app.join("main.rud"), "let value = std::value\n").unwrap();
        fs::write(app.join("Ruddy.toml"), format!("name = \"app\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\n{setting}\n")).unwrap();
        let graph =
            ruddy_cli::compile_graph(&app).unwrap_or_else(|error| panic!("{name}: {error}"));
        let checkout = &graph.projects[0].directory;
        assert_eq!(git(checkout, &["rev-parse", "HEAD"]), expected, "{name}");
        if shallow {
            assert_eq!(
                git(checkout, &["rev-parse", "--is-shallow-repository"]),
                "true",
                "{name}"
            );
            assert_eq!(
                git(checkout, &["rev-list", "--count", "HEAD"]),
                "1",
                "{name}"
            );
        }
        assert!(!root.join("home/std").exists());
        if name == "branch" {
            // A pinned lock must survive a missing checkout after the remote moves.
            fs::remove_dir_all(checkout).unwrap();
            fs::write(root.join("repo.git/main.rud"), "let value = 99n\n").unwrap();
            git(&root.join("repo.git"), &["add", "."]);
            git(
                &root.join("repo.git"),
                &[
                    "-c",
                    "user.name=Tests",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-qm",
                    "advance",
                ],
            );
            let graph = ruddy_cli::compile_graph(&app).unwrap();
            assert_eq!(
                git(&graph.projects[0].directory, &["rev-parse", "HEAD"]),
                tip
            );
            assert_eq!(
                git(
                    &graph.projects[0].directory,
                    &["rev-list", "--count", "HEAD"]
                ),
                "1"
            );
        }
    }
}
