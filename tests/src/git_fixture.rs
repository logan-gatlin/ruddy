//! Local Git cache fixtures for tests that must not contact the public std host.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn cache_legacy_std(home: &Path, source: &str) -> PathBuf {
    let seed = home.join("seed");
    fs::create_dir_all(&seed).unwrap();
    fs::write(seed.join("main.rud"), source).unwrap();
    fs::write(seed.join("Ruddy.toml"), "name = \"std\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Tests",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(&seed)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&seed)
        .output()
        .unwrap();
    assert!(output.status.success());
    let commit = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let key = format!(
        "{}{:?}",
        ruddy_cli::DEFAULT_STD_GIT,
        ruddy_cli::GitSelector::Default
    );
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let cache = home.join("cache/git");
    let checkout = cache
        .join("checkouts")
        .join(format!("{hash:016x}"))
        .join(&commit);
    fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    fs::rename(seed, &checkout).unwrap();
    fs::create_dir_all(cache.join("selections")).unwrap();
    fs::write(
        cache.join("selections").join(format!("{hash:016x}")),
        commit,
    )
    .unwrap();
    checkout
}

/// Install the actual compiler commit, not a floating synthetic std revision.
pub fn cache_std(home: &Path) -> PathBuf {
    let revision = ruddy_cli::BUILD_REVISION;
    assert!(
        !revision.is_empty(),
        "default std tests need compiler revision metadata"
    );
    let key = format!(
        "{}{:?}",
        ruddy_cli::DEFAULT_STD_GIT,
        ruddy_cli::GitSelector::Rev(revision)
    );
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let checkout = home
        .join("cache/git/checkouts")
        .join(format!("{hash:016x}"))
        .join(revision);
    fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for args in [
        vec![
            "clone",
            "--quiet",
            "--no-checkout",
            repo.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
        vec![
            "-C",
            checkout.to_str().unwrap(),
            "checkout",
            "--quiet",
            "--detach",
            revision,
        ],
    ] {
        let output = Command::new("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let selections = home.join("cache/git/selections");
    fs::create_dir_all(&selections).unwrap();
    fs::write(selections.join(format!("{hash:016x}")), revision).unwrap();
    checkout
}
