use std::{env, path::PathBuf, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env::var_os("CARGO_MANIFEST_DIR")?)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=RUDDY_BUILD_REVISION");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    // A source archive nested inside an unrelated Git repository has no
    // compiler revision. Do not accidentally embed that outer repo's HEAD.
    let in_repo = git(&["rev-parse", "--show-toplevel"]).is_some_and(|root| {
        PathBuf::from(root).canonicalize().ok() == manifest.parent().unwrap().canonicalize().ok()
    });
    // Watch HEAD plus its symbolic ref (including worktrees and packed refs).
    for name in [
        Some("HEAD".to_owned()),
        git(&["symbolic-ref", "-q", "HEAD"]),
        Some("packed-refs".to_owned()),
    ]
    .into_iter()
    .flatten()
    {
        if in_repo && let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let mut path = manifest.join(path);
            // Missing watched files make Cargo rebuild on every invocation.
            // Watch the nearest existing directory until the ref is created.
            while !path.exists() && path.pop() {}
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let revision = env::var("RUDDY_BUILD_REVISION")
        .ok()
        .or_else(|| in_repo.then(|| git(&["rev-parse", "HEAD"])).flatten());
    let revision = match revision {
        Some(revision) => {
            assert!(
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "RUDDY_BUILD_REVISION must be a full 40-character Git commit SHA"
            );
            revision.to_ascii_lowercase()
        }
        None => {
            println!(
                "cargo:warning=No source revision available; implicit std requires RUDDY_BUILD_REVISION at build time"
            );
            String::new()
        }
    };
    println!("cargo:rustc-env=RUDDY_BUILD_REVISION={revision}");
    println!(
        "cargo:rustc-env=RUDDY_LONG_VERSION={} ({})",
        env::var("CARGO_PKG_VERSION").unwrap(),
        if revision.is_empty() {
            "unknown revision"
        } else {
            &revision
        }
    );
}
