use std::{fs, process::Command};

#[test]
fn reification_standard_modules_compile_decode_and_downcast_across_artifacts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!("name = \"any-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = {:?}\n", root)).unwrap();
    fs::write(
        project.path().join("main.rud"),
        r#"
@private extern unknown: ForeignValue = "({ tag: 'Some', value: [21, 22] })"
@private let decoded: std::result::Result (std::option::Option [Nat]) std::ffi::DecodeError = std::ffi::decode unknown
@private let boxed = std::any::upcast [23n]
@private let recovered: std::option::Option [Nat] = std::any::downcast boxed
let a = match decoded with | #Some (#Some [n, ..]) => n | _ => 0n end
let b = match recovered with | #Some [n] => n | _ => 0n end
"#,
    )
    .unwrap();
    let artifact = ruddy_cli::build_project(project.path()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import {{ pathToFileURL }} from 'node:url'; const app = await import(pathToFileURL({}).href); assert.equal(app.a, 21); assert.equal(app.b, 23);",
        serde_json::to_string(artifact.with_extension("js").to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
