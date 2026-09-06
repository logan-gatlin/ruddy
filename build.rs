//! Stamp the build with a hash of the compiler's own source.
//!
//! Artifacts record the compiler that wrote them, so a cache can tell an
//! artifact this compiler would write from one an earlier compiler did. The
//! stamp is a digest of every source file of this crate rather than a version
//! number, because the compiler changes many times a day and a number nobody
//! remembers to bump is worse than none. Anything that could change what an
//! artifact contains lives in this crate's source, so that is what is hashed;
//! edits to the command line or the debugger leave the stamp alone.

use std::{env, fs, path::Path};

use twox_hash::XxHash3_64;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    files.push(root.join("Cargo.toml"));
    files.push(root.join("build.rs"));
    files.sort();

    let mut bytes = Vec::new();
    for file in &files {
        let relative = file.strip_prefix(root).unwrap_or(file);
        bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&fs::read(file).unwrap_or_default());
        bytes.push(0);
    }
    let hash = XxHash3_64::oneshot(&bytes);
    println!("cargo:rustc-env=RUDDY_COMPILER_HASH={hash:016x}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
}

fn collect(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
