use std::{fs, path::Path, process::Command};

fn project(source: &str, platform: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("std");
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"host-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.hc\"\ntarget = \"js\"\nplatform = {platform:?}\n\n[dependencies]\nstd = {standard:?}\n"
    )).unwrap();
    fs::write(project.path().join("main.hc"), source).unwrap();
    project
}

fn run(project: &Path, script: &str) -> std::process::Output {
    ruddy_cli::check_project(project).unwrap();
    let artifact = ruddy_cli::build_project(project).unwrap();
    let script = format!(
        "import assert from 'node:assert/strict'; import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({}));\n{script}",
        serde_json::to_string(artifact.with_extension("js").to_str().unwrap()).unwrap()
    );
    Command::new("node")
        .current_dir(project)
        .args(["--input-type=module", "--eval", &script])
        .output()
        .unwrap()
}

fn success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn host_exports_handle_filesystem_aliases_currying_and_returned_functions() {
    let project = project(
        r#"
let read_file = std::fs::read_text
let write_file = std::fs::write_text
let exists = std::fs::exists
let read_or_empty = fn path => std::result::unwrap_or "" (read_file path)
let reader = fn _ => read_file
module files = let read = read_or_empty end
let print = std::console::print
let capture_print = fn text => handle print text with
  | std::!Console.write output => raise output
  | std::!Console.write_error _ => ()
  | return _ => ""
  end
"#,
        "node",
    );
    let output = run(
        project.path(),
        r#"
const write = await app.write_file('text');
assert.equal(typeof write, 'function');
await write('host contents');
assert.equal(await app.read_or_empty('text'), 'host contents');
assert.equal(await app.files.read('text'), 'host contents');
const reader = await app.reader({});
assert.equal(typeof reader, 'function');
assert.deepEqual(await reader('text'), await app.read_file('text'));
await app.exists('text');
assert.equal(await app.capture_print('local'), 'local\n');
await app.print('host output');
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"host output\n");
    assert_eq!(
        fs::read_to_string(project.path().join("text")).unwrap(),
        "host contents"
    );
}

#[test]
fn host_exports_reject_unsupported_effects_at_every_curried_arrow() {
    for body in ["fn _ => !Ask.get ()", "fn _ => fn _ => !Ask.get ()"] {
        let project = project(
            &format!("effect Ask = {{ get: () -> String }}\nlet read_file = {body}"),
            "node",
        );
        let error = ruddy_cli::check_project(project.path())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported-export-effects") && error.contains("read_file"),
            "{error}"
        );
        assert!(ruddy_cli::build_project(project.path()).is_err());
        assert!(!project.path().join("build").exists());
    }
}

#[test]
fn host_export_check_does_not_restrict_dependencies_or_private_definitions() {
    let project = project(
        r#"
@private let ask = dep::read
let read = fn _ => handle ask () with | dep::!Ask.get _ => "handled" end
"#,
        "node",
    );
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    fs::write(dependency.join("Ruddy.toml"), "name = \"dep\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"lib.hc\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    fs::write(
        dependency.join("lib.hc"),
        "effect Ask = { get: () -> String }\nlet read = fn _ => !Ask.get ()",
    )
    .unwrap();
    let manifest = project.path().join("Ruddy.toml");
    fs::write(
        &manifest,
        format!("{}dep = \"dep\"\n", fs::read_to_string(&manifest).unwrap()),
    )
    .unwrap();
    success(&run(
        project.path(),
        "assert.equal(await app.read({}), 'handled');",
    ));
    let dependency_error = ruddy_cli::check_project(&dependency)
        .unwrap_err()
        .to_string();
    assert!(
        dependency_error.contains("unsupported-export-effects"),
        "{dependency_error}"
    );
}

#[test]
fn host_exports_use_the_selected_platform() {
    let effectful = project("let read_file = std::fs::read_text", "web");
    let error = ruddy_cli::check_project(effectful.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unsupported-export-effects") && error.contains("read_file"),
        "{error}"
    );
    let pure = project("let identity = fn x => x", "web");
    success(&run(pure.path(), "assert.equal(app.identity(42), 42);"));
    let js = fs::read_to_string(pure.path().join("build/host-test.js")).unwrap();
    assert!(!js.contains("node:fs") && !js.contains("process.stdout"));
}

#[test]
fn host_exports_exit_drains_output_and_saturates_the_exit_code() {
    let project = project(
        r#"
let stop = fn code => do
  let _ = std::console::write "before exit"
  return std::process::exit code
end
"#,
        "node",
    );
    let output = run(
        project.path(),
        "await app.stop(999); throw new Error('exit resumed');",
    );
    assert_eq!(
        output.status.code(),
        Some(255),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"before exit");
}

#[test]
fn host_exports_handle_callable_fields_and_reject_hidden_unsupported_effects() {
    let good = project(
        r#"
let files = { read: std::fs::read_text, label: "files" }
let factory = fn _ => { print: std::console::print }
let tagged = #Printer std::console::print
let printers = [std::console::print]
let optional = fn enable => if enable then #Printer std::console::print else #Empty end
let pair = (std::console::print, 42n)
"#,
        "node",
    );
    let output = run(
        good.path(),
        r#"
assert.equal(app.files.label, 'files');
await app.files.read('missing');
const writer = await app.factory({});
await writer.print('field');
const payload = Object.getOwnPropertySymbols(app.tagged).find(symbol => symbol.description === 'sum payload');
await app.tagged[payload]('tagged');
await app.printers.tail[0]('array');
const empty = await app.optional(false);
const tag = Object.getOwnPropertySymbols(empty).find(symbol => symbol.description === 'sum tag');
assert.equal(empty[tag], 'Empty');
const some = await app.optional(true);
await some[payload]('optional');
assert.equal(app.pair[1], 42);
await app.pair[0]('tuple');
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"field\ntagged\narray\noptional\ntuple\n");
    let bad = project(
        "effect Ask = { get: () -> String }\nlet hidden = { read: !Ask.get }",
        "node",
    );
    let error = ruddy_cli::check_project(bad.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unsupported-export-effects") && error.contains("hidden"),
        "{error}"
    );
}

#[test]
fn host_exports_adapt_recursive_callables_and_preserve_returned_captures() {
    let project = project(
        r#"
@private type Printer = String -> Printer + std::!Console
@private let make : Nat -> Printer = fn count => fn text => do
  let _ = std::console::print (std::str::concat (std::str::from_nat count) text)
  return make (std::nat::add count 1n)
end
let printer = make 0n
let capture = fn _ => handle printer "local" with
  | std::!Console.write text => raise text
  | std::!Console.write_error _ => ()
  | return _ => ""
end
"#,
        "node",
    );
    let output = run(
        project.path(),
        r#"
assert.equal(await app.capture({}), '0local\n');
const first = await app.printer('a');
const second = await first('b');
await second('c');
await first('d');
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"0a\n1b\n2c\n1d\n");
}

#[test]
fn host_exports_adapt_mutual_recursion_and_rotating_type_arguments() {
    let project = project(
        r#"
type Left = String -> Right + std::!Console
type Right = Nat -> Left + std::!Console
let left : Left = fn text => do
  let _ = std::console::print text
  return right
end
@private let right : Right = fn n => do
  let _ = std::console::print (std::str::from_nat n)
  return left
end
type Id 'a = 'a
type Rotate 'a 'b = 'a -> Rotate 'b 'a + std::!Console
let rotate : Rotate (Id String) Nat = fn text => do
  let _ = std::console::print text
  return number
end
@private let number : Rotate Nat String = fn n => do
  let _ = std::console::print (std::str::from_nat n)
  return rotate
end
"#,
        "node",
    );
    let output = run(
        project.path(),
        r#"
await (await (await app.left('left'))(7))('again');
await (await (await app.rotate('rotate'))(8))('rotated');
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"left\n7\nagain\nrotate\n8\nrotated\n");
}

#[test]
fn host_exports_adapt_recursive_containers_and_async_invocations() {
    let project = project(
        r#"
type Tree = #Leaf | #Branch { print: String -> () + std::!Console, children: [Tree] }
let tree : Tree = #Branch { print: std::console::print, children: [#Leaf, #Branch { print: std::console::print, children: [] }] }
type Reader = String -> { text: String, next: Reader } + std::!FileSystem
let reader : Reader = fn path => do
  let text = std::result::unwrap_or "missing" (std::fs::read_text path)
  return { text: text, next: reader }
end
"#,
        "node",
    );
    fs::write(project.path().join("one"), "first").unwrap();
    fs::write(project.path().join("two"), "second").unwrap();
    let output = run(
        project.path(),
        r#"
const payload = Object.getOwnPropertySymbols(app.tree).find(symbol => symbol.description === 'sum payload');
await app.tree[payload].print('root');
await app.tree[payload].children.tail[1][payload].print('child');
const [one, two] = await Promise.all([app.reader('one'), app.reader('two')]);
assert.equal(one.text, 'first');
assert.equal(two.text, 'second');
assert.equal((await one.next('two')).text, 'second');
assert.equal((await two.next('one')).text, 'first');
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"root\nchild\n");
}

#[test]
fn host_exports_reject_unhandled_effects_behind_recursive_edges() {
    for platform in ["node", "web"] {
        let project = project(
            r#"
effect Ask = { get: () -> String }
type First = () -> Second
type Second = { again: First, read: () -> String + !Ask }
let first : First = fn _ => { again: first, read: !Ask.get }
"#,
            platform,
        );
        let error = ruddy_cli::check_project(project.path())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported-export-effects") && error.contains("first"),
            "{error}"
        );
        assert!(!error.contains("finite host interface"), "{error}");
        assert!(ruddy_cli::build_project(project.path()).is_err());
    }
    let pure = project(
        "type Loop = () -> Loop\nlet loop : Loop = fn _ => loop",
        "web",
    );
    success(&run(pure.path(), "assert.equal(app.loop({}), app.loop);"));
}

#[test]
fn host_exports_adapt_recursive_dependency_types_and_row_arguments() {
    let project = project(
        r#"
let printer = dep::printer
type Fields 'r = { ..'r }
type Cases 'r = #Empty | ..'r
type Loop 'e = String -> Loop 'e + ..'e
let fields : Fields { print: String -> () + std::!Console } = { print: std::console::print }
let tagged : Cases (#Printer (String -> () + std::!Console)) = #Printer std::console::print
let loop : Loop (std::!Console) = fn text => do
  let _ = std::console::print text
  return loop
end
"#,
        "node",
    );
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("std");
    fs::write(dependency.join("Ruddy.toml"), format!("name = \"dep\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"lib.hc\"\n[dependencies]\nstd = {standard:?}\n")).unwrap();
    fs::write(dependency.join("lib.hc"), "@private type Printer = String -> Printer + std::!Console\nlet printer : Printer = fn text => do let _ = std::console::print text return printer end").unwrap();
    let manifest = project.path().join("Ruddy.toml");
    fs::write(
        &manifest,
        format!("{}dep = \"dep\"\n", fs::read_to_string(&manifest).unwrap()),
    )
    .unwrap();
    let output = run(
        project.path(),
        r#"
await (await app.printer('dependency'))('again');
await app.fields.print('fields');
const payload = Object.getOwnPropertySymbols(app.tagged).find(symbol => symbol.description === 'sum payload');
await app.tagged[payload]('cases');
await (await app.loop('effects'))('repeat');
"#,
    );
    success(&output);
    assert_eq!(
        output.stdout,
        b"dependency\nagain\nfields\ncases\neffects\nrepeat\n"
    );
}

#[test]
fn host_exports_preserve_optional_fields_in_recursive_interfaces() {
    let project = project(
        r#"
type Cursor 'r = () -> { next: Cursor 'r, ..'r } + std::!Console
let optional : { label when 'p: String } -> Cursor { label when 'p: String } = fn fields => do
  let next : Cursor { .. } = fn _ => do
    let _ = std::console::print "step"
    return { next: next, ..fields }
  end
  return next
end
"#,
        "node",
    );
    let output = run(
        project.path(),
        r#"
const some = await app.optional({label: 'kept'});
const first = await some({});
assert.equal(first.label, 'kept');
assert.equal((await first.next({})).label, 'kept');
const none = await app.optional({});
assert.equal(Object.hasOwn(await none({}), 'label'), false);
"#,
    );
    success(&output);
    assert_eq!(output.stdout, b"step\nstep\nstep\n");
}

#[test]
fn host_exports_handle_binary_files_and_curried_copies() {
    let project = project(
        r#"
let read = std::fs::read_bytes
let first_byte = fn path => match read path with
  | #Some [byte, ..] => byte
  | _ => 0n8
end
let copy_bytes = fn source destination => match read source with
  | #Some bytes => std::fs::write_bytes destination bytes
  | #Error error => #Error error
end
let append_bytes = fn path => std::fs::append_bytes path [0n8, 128n8, 255n8]
"#,
        "node",
    );
    fs::write(project.path().join("input"), [255, 0, 128, 1]).unwrap();
    let output = run(
        project.path(),
        r#"
assert.equal(await app.first_byte('input'), 255);
const copy = await app.copy_bytes('input');
assert.equal(typeof copy, 'function');
await copy('output');
await app.append_bytes('output');
"#,
    );
    success(&output);
    assert_eq!(
        fs::read(project.path().join("output")).unwrap(),
        [255, 0, 128, 1, 0, 128, 255]
    );
}
