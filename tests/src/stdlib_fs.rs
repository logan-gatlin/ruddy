use std::{fs, path::Path, process::Command};

const ASSERTIONS: &str = r#"
extern assert: fn(Bool, String) -> () = "(ok, message) => { if (!ok) throw new Error(message); }"
extern crash: String -> | = "message => { throw new Error(message); }"
let fail = fn message => match crash message with end
let expect: std::result::Result 'a std::fs::Error -> 'a = fn result => match result with
  | #Some value => value
  | #Error error => fail error.message
end
let kind_name = fn kind => match kind with
  | #NotFound => "NotFound"
  | #PermissionDenied => "PermissionDenied"
  | #AlreadyExists => "AlreadyExists"
  | #NotDirectory => "NotDirectory"
  | #IsDirectory => "IsDirectory"
  | #DirectoryNotEmpty => "DirectoryNotEmpty"
  | #InvalidPath => "InvalidPath"
  | #InvalidEncoding => "InvalidEncoding"
  | #Unsupported => "Unsupported"
  | #Other => "Other"
end
let expect_error: String -> std::result::Result 'a std::fs::Error -> () = fn expected result => match result with
  | #Some _ => fail (std::str::concat "Expected error: " expected)
  | #Error error => do
      let actual = kind_name error.kind
      let _ = assert (std::str::equal expected actual) (std::str::concat "Unexpected error: " actual)
      return assert (std::nat::greater_than (std::str::len error.message) 0n) "Empty diagnostic"
    end
end
"#;

fn project(source: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"fs-test\"\nversion = \"0.1.0\"\nkind = \"executable\"\nroot = \"main.rud\"\ntarget = \"js\"\n\n[dependencies]\nstd = {standard:?}\n"
        ),
    )
    .unwrap();
    fs::write(
        project.path().join("main.rud"),
        format!("{ASSERTIONS}\n{source}"),
    )
    .unwrap();
    project
}

fn run(project: &Path, prelude: &str) {
    ruddy_cli::check_project(project).expect("filesystem effects pass entry checking");
    let artifact = ruddy_cli::build_project(project).expect("filesystem executable builds");
    let probe = format!(
        "import {{ pathToFileURL }} from 'node:url';\n{prelude}\nawait import(pathToFileURL({}));",
        serde_json::to_string(artifact.with_extension("js").to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .current_dir(project)
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn whole_file_operations_run_sequentially_through_async_platform_handlers() {
    let project = project(
        r#"
let main = fn _ => do
  let _ = expect (std::fs::create_dir "work")
  let _ = expect (std::fs::create_dir_all "work/nested/deep")
  let _ = expect (std::fs::create_dir_all "work/nested/deep")
  let empty = expect (std::fs::read_dir "work/nested/deep")
  let _ = assert (std::nat::equal (std::array::len empty) 0n) "Empty directory"
  let many = expect (std::fs::read_dir "many")
  let _ = assert (std::nat::equal (std::array::len many) 65n) "Large directory listing"
  let _ = match std::array::get many 64n with
    | #Some entry => assert (match entry.kind with | #File => true | _ => false end) "Array conversion preserves entries"
    | #None => fail "Listing lost its tail"
    end
  let write = std::fs::write_text "work/text"
  let _ = expect (write "replaced content")
  let _ = expect (write "hé")
  let _ = expect (std::fs::append_text "work/text" "llo")
  let text = expect (std::fs::read_text "work/text")
  let _ = assert (std::str::equal text "héllo") "Read/write/append round trip"
  let _ = assert (expect (std::fs::exists "work/text")) "File exists"
  let _ = assert (expect (std::fs::exists "work")) "Directory exists"
  let metadata = expect (std::fs::metadata "work/text")
  let _ = assert (std::nat::equal metadata.size 6n) "Size counts UTF-8 bytes"
  let _ = assert (match metadata.kind with | #File => true | _ => false end) "File metadata"
  let directory = expect (std::fs::metadata "work")
  let _ = assert (match directory.kind with | #Directory => true | _ => false end) "Directory metadata"
  let _ = expect (std::fs::append_text "work/new" "created")
  let _ = assert (std::str::equal (expect (std::fs::read_text "work/new")) "created") "Append creates"
  let _ = expect (std::fs::write_text "work/copied" "old destination")
  let _ = expect (std::fs::copy_file "work/text" "work/copied")
  let _ = assert (std::str::equal (expect (std::fs::read_text "work/copied")) text) "Copy replaces destination"
  let _ = expect (std::fs::write_text "work/renamed" "old destination")
  let _ = expect (std::fs::rename "work/copied" "work/renamed")
  let _ = assert (std::boolean::logical_not (expect (std::fs::exists "work/copied"))) "Rename removes source"
  let _ = assert (std::str::equal (expect (std::fs::read_text "work/renamed")) text) "Rename preserves content"
  let entries = expect (std::fs::read_dir "work/nested")
  let _ = assert (std::nat::equal (std::array::len entries) 1n) "Immediate children only"
  let _ = match std::array::get entries 0n with
    | #Some entry => do
        let _ = assert (std::str::equal entry.name "deep") "Basename entry"
        return assert (match entry.kind with | #Directory => true | _ => false end) "Entry kind"
      end
    | #None => fail "Missing entry"
    end
  let _ = expect (std::fs::remove_file "work/text")
  let _ = expect (std::fs::remove_file "work/new")
  let _ = expect (std::fs::remove_file "work/renamed")
  let _ = expect (std::fs::remove_dir "work/nested/deep")
  let _ = expect (std::fs::remove_dir "work/nested")
  let _ = expect (std::fs::remove_dir "work")
  return assert (std::boolean::logical_not (expect (std::fs::exists "work"))) "Directory removed"
end
"#,
    );
    fs::create_dir(project.path().join("many")).unwrap();
    for index in 0..65 {
        fs::write(project.path().join(format!("many/{index}")), "").unwrap();
    }
    run(project.path(), "");
    assert!(!project.path().join("work").exists());
}

#[test]
fn whole_file_errors_are_results_and_invalid_text_does_not_overwrite_files() {
    let project = project(
        r#"
extern invalid_path: String = "'bad' + String.fromCharCode(0) + 'path'"
extern invalid_text: ForeignValue = "String.fromCharCode(55296)"
let read_text_value: ForeignValue -> std::result::Result String std::ffi::DecodeError =
  std::ffi::decode
let main = fn _ => do
  let _ = expect_error "NotFound" (std::fs::read_bytes "missing")
  let _ = expect_error "NotFound" (std::fs::write_bytes "missing/child" [0n8])
  let _ = expect_error "NotFound" (std::fs::append_bytes "missing/child" [0n8])
  let _ = expect_error "IsDirectory" (std::fs::read_bytes "directory")
  let _ = expect_error "IsDirectory" (std::fs::write_bytes "directory" [0n8])
  let _ = expect_error "IsDirectory" (std::fs::append_bytes "directory" [0n8])
  let _ = expect_error "InvalidPath" (std::fs::read_bytes invalid_path)
  let _ = expect_error "InvalidPath" (std::fs::write_bytes invalid_path [0n8])
  let _ = expect_error "InvalidPath" (std::fs::append_bytes invalid_path [0n8])
  let _ = expect_error "NotFound" (std::fs::read_text "missing")
  let _ = expect_error "NotFound" (std::fs::remove_file "missing")
  let _ = expect_error "NotFound" (std::fs::remove_dir "missing")
  let _ = expect_error "NotFound" (std::fs::write_text "missing/child" "text")
  let _ = expect_error "NotFound" (std::fs::append_text "missing/child" "text")
  let _ = expect_error "AlreadyExists" (std::fs::create_dir "directory")
  let _ = expect_error "DirectoryNotEmpty" (std::fs::remove_dir "directory")
  let _ = expect_error "NotDirectory" (std::fs::read_dir "file")
  let _ = expect_error "NotDirectory" (std::fs::exists "file/child")
  let _ = expect_error "IsDirectory" (std::fs::read_text "directory")
  let _ = expect_error "InvalidEncoding" (std::fs::read_text "invalid-utf8")
  -- Text that is not Unicode scalar values never becomes a Ruddy String, so
  -- there is no such text to write to a file in the first place.
  let _ = assert (match read_text_value invalid_text with
    | #Some _ => false
    | #Error error => std::str::equal error.expected "String"
    end) "A lone surrogate is not a String"
  let _ = expect_error "InvalidPath" (std::fs::exists invalid_path)
  let _ = expect_error "InvalidPath" (std::fs::rename "file" invalid_path)
  let _ = expect_error "InvalidPath" (std::fs::copy_file "file" invalid_path)
  let _ = assert (std::str::equal (expect (std::fs::read_text "file")) "original") "Invalid writes leave file intact"
  let _ = assert (std::str::equal (expect (std::fs::read_text "bom")) "﻿hello") "UTF-8 BOM preserved"
  return match std::fs::metadata "missing" with
    | #Error error => assert (std::str::equal error.path "missing") "Error includes path"
    | #Some _ => fail "Missing path metadata succeeded"
    end
end
"#,
    );
    fs::write(project.path().join("file"), "original").unwrap();
    fs::write(project.path().join("invalid-utf8"), [0xff, 0xfe]).unwrap();
    fs::write(project.path().join("bom"), "\u{feff}hello").unwrap();
    fs::create_dir(project.path().join("directory")).unwrap();
    fs::write(project.path().join("directory/child"), "").unwrap();
    run(project.path(), "");
}

#[test]
fn whole_file_exists_preserves_permission_and_other_host_failures() {
    let project = project(
        r#"
let main = fn _ => do
  let _ = expect_error "PermissionDenied" (std::fs::exists "denied")
  let _ = expect_error "Unsupported" (std::fs::metadata "unsupported")
  let _ = expect_error "Other" (std::fs::exists "broken")
  return assert (std::boolean::logical_not (expect (std::fs::exists "missing"))) "Only not-found becomes false"
end
"#,
    );
    // Inject host failures through the public Node module. This is deterministic
    // even when tests run as root or the host filesystem lacks permission bits.
    run(
        project.path(),
        r#"
import fs from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
const stat = fs.stat;
fs.stat = path => {
  if (path === 'denied') throw Object.assign(new Error('denied'), { code: 'EACCES', path });
  if (path === 'unsupported') return Promise.reject(Object.assign(new Error('unsupported'), { code: 'ENOSYS', path }));
  if (path === 'broken') return Promise.reject(Object.assign(new Error('broken'), { code: 'EIO', path }));
  return stat(path);
};
syncBuiltinESMExports();
"#,
    );
}

#[cfg(unix)]
#[test]
fn whole_file_symlinks_and_unrepresentable_directory_entries() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let project = project(
        r#"
let main = fn _ => do
  let _ = assert (expect (std::fs::exists "links/live")) "Live symlink exists"
  let _ = assert (std::boolean::logical_not (expect (std::fs::exists "links/dangling"))) "Dangling symlink is absent"
  let _ = expect_error "NotFound" (std::fs::metadata "links/dangling")
  let target = expect (std::fs::metadata "links/live")
  let _ = assert (match target.kind with | #File => true | _ => false end) "Metadata follows links"
  let link = expect (std::fs::symlink_metadata "links/dangling")
  let _ = assert (match link.kind with | #Symlink => true | _ => false end) "Lstat preserves dangling links"
  let entries = expect (std::fs::read_dir "links")
  let _ = assert (std::nat::equal (std::array::len entries) 2n) "Both links listed"
  let _ = match std::array::get entries 0n with
    | #Some entry => assert (match entry.kind with | #Symlink => true | _ => false end) "Listing does not follow links"
    | #None => fail "Missing link entry"
    end
  let _ = expect (std::fs::remove_file "links/live")
  let _ = assert (expect (std::fs::exists "target")) "Unlink leaves target intact"
  return expect_error "InvalidPath" (std::fs::read_dir "invalid-names")
end
"#,
    );
    fs::write(project.path().join("target"), "target").unwrap();
    fs::create_dir(project.path().join("links")).unwrap();
    symlink("../target", project.path().join("links/live")).unwrap();
    symlink("../missing", project.path().join("links/dangling")).unwrap();
    fs::create_dir(project.path().join("invalid-names")).unwrap();
    fs::write(
        project
            .path()
            .join("invalid-names")
            .join(std::ffi::OsStr::from_bytes(b"\xff")),
        "",
    )
    .unwrap();
    run(project.path(), "");
}

#[test]
fn whole_file_local_handlers_receive_named_fields_and_survive_suspension() {
    let project = project(
        r#"
@async
extern pause: () -> () = "() => new Promise(resolve => setTimeout(() => resolve({}), 1))"
let main = fn _ => handle do
    let _ = pause ()
    let _ = expect (std::fs::write_bytes "virtual" [0n8, 255n8])
    let _ = expect (std::fs::append_bytes "virtual" [128n8])
    let _ = match expect (std::fs::read_bytes "virtual") with
      | [0n8, 255n8, 128n8] => ()
      | _ => fail "Local binary read"
    end
    let _ = expect (std::fs::write_text "virtual" "contents")
    let _ = expect (std::fs::append_text "virtual" "contents")
    let _ = expect (std::fs::rename "virtual" "destination")
    let _ = expect (std::fs::copy_file "virtual" "destination")
    return assert (std::str::equal (expect (std::fs::read_text "virtual")) "virtual contents") "Local read"
  end with
  | std::fs::!FileSystem.read_bytes _ => do
      let _ = pause ()
      return #Some [0n8, 255n8, 128n8]
    end
  | std::fs::!FileSystem.write_bytes request => do
      let _ = assert (std::str::equal request.path "virtual") "Named binary write path"
      return match request.bytes with
        | [0n8, 255n8] => #Some ()
        | _ => fail "Named binary write bytes"
      end
    end
  | std::fs::!FileSystem.append_bytes request => do
      let _ = assert (std::str::equal request.path "virtual") "Named binary append path"
      return match request.bytes with
        | [128n8] => #Some ()
        | _ => fail "Named binary append bytes"
      end
    end
  | std::fs::!FileSystem.read_text _ => do
      let _ = pause ()
      return #Some "virtual contents"
    end
  | std::fs::!FileSystem.write_text request => do
      let _ = assert (std::str::equal request.path "virtual") "Named write path"
      let _ = assert (std::str::equal request.text "contents") "Named write text"
      return #Some ()
    end
  | std::fs::!FileSystem.append_text request => do
      let _ = assert (std::str::equal request.text "contents") "Named append text"
      return #Some ()
    end
  | std::fs::!FileSystem.rename request => do
      let _ = assert (std::str::equal request.source "virtual") "Named rename source"
      let _ = assert (std::str::equal request.destination "destination") "Named rename destination"
      return #Some ()
    end
  | std::fs::!FileSystem.copy_file request => do
      let _ = assert (std::str::equal request.destination "destination") "Named copy destination"
      return #Some ()
    end
  | std::fs::!FileSystem.exists _ => #Some true
  | std::fs::!FileSystem.read_dir _ => #Some []
  | std::fs::!FileSystem.metadata _ => #Some { kind: #File, size: 0n }
  | std::fs::!FileSystem.symlink_metadata _ => #Some { kind: #File, size: 0n }
  | std::fs::!FileSystem.create_dir _ => #Some ()
  | std::fs::!FileSystem.create_dir_all _ => #Some ()
  | std::fs::!FileSystem.remove_file _ => #Some ()
  | std::fs::!FileSystem.remove_dir _ => #Some ()
  end
"#,
    );
    run(project.path(), "");
    assert!(!project.path().join("virtual").exists());
    assert!(!project.path().join("destination").exists());
}

#[test]
fn whole_file_binary_bytes_preserve_all_values_order_and_array_boundaries() {
    let project = project(
        r#"
let byte_at = fn bytes index => match std::array::get bytes index with
  | #Some byte => byte
  | #None => fail "Missing byte"
end
let verify = fn bytes index =>
  if std::nat::equal index (std::array::len bytes) then ()
  else do
    let expected = std::nat::from_nat8 (std::nat::remainder index 256n)
    let _ = assert (std::nat::equal8 (byte_at bytes index) expected) "Byte changed"
    return verify bytes (std::nat::add index 1n)
  end end
let main = fn _ => do
  let bytes : [Nat8] = expect (std::fs::read_bytes "input")
  let _ = assert (std::nat::equal (std::array::len bytes) 65537n) "Binary length"
  let _ = verify bytes 0n
  let _ = expect_error "InvalidEncoding" (std::fs::read_text "input")
  let write = std::fs::write_bytes "output"
  let _ = expect (write bytes)
  let _ = expect (std::fs::append_bytes "output" [0n8, 255n8, 128n8])
  let reread = expect (std::fs::read_bytes "output")
  let _ = assert (std::nat::equal (std::array::len reread) 65540n) "Append length"
  let _ = assert (std::nat::equal8 (byte_at reread 65538n) 255n8) "Append order"
  let _ = assert (std::nat::equal8 (byte_at reread 65539n) 128n8) "Append high bit"
  let _ = expect (std::fs::write_bytes "truncated" [255n8])
  let _ = expect (std::fs::write_bytes "empty" [])
  let _ = expect (std::fs::append_bytes "appended" [128n8])
  let _ = expect (std::fs::append_bytes "empty-append" [])
  let _ = match expect (std::fs::read_bytes "empty") with
    | [] => ()
    | _ => fail "Empty binary read"
  end
  let changed = match std::array::set bytes 0n 255n8 with
    | #Some changed => changed
    | #None => fail "Cannot update byte array"
  end
  let _ = expect (std::fs::write_bytes "changed" changed)
  return assert (std::nat::equal8 (byte_at bytes 0n) 0n8) "Writing mutated source array"
end
"#,
    );
    let bytes: Vec<u8> = (0..65537).map(|index| (index % 256) as u8).collect();
    fs::write(project.path().join("input"), &bytes).unwrap();
    fs::write(project.path().join("truncated"), &bytes).unwrap();
    fs::write(project.path().join("empty"), &bytes).unwrap();
    run(project.path(), "");
    let mut appended = bytes.clone();
    appended.extend([0, 255, 128]);
    assert_eq!(fs::read(project.path().join("output")).unwrap(), appended);
    assert_eq!(fs::read(project.path().join("truncated")).unwrap(), [255]);
    assert!(fs::read(project.path().join("empty")).unwrap().is_empty());
    assert!(
        fs::read(project.path().join("empty-append"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(project.path().join("appended")).unwrap(), [128]);
    let mut changed = bytes.clone();
    changed[0] = 255;
    assert_eq!(fs::read(project.path().join("changed")).unwrap(), changed);
    assert_eq!(fs::read(project.path().join("input")).unwrap(), bytes);
}

#[test]
fn whole_file_binary_writes_require_nat8_elements() {
    for literal in ["1n", "1i8", "1"] {
        let project = project(&format!(
            "let main = fn _ => match std::fs::write_bytes \"out\" [{literal}] with | #Some _ => () | #Error _ => () end"
        ));
        let error = ruddy_cli::check_project(project.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("[type-mismatch]"), "{error}");
        assert!(!project.path().join("out").exists());
    }
}
