use std::{fs, path::Path, process::Command};

fn project(source: &str, platform: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"host-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\nplatform = {platform:?}\n\n[dependencies]\nstd = {standard:?}\n"
    )).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
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
let reader: () -> _ = fn _ => read_file
module files = let read = read_or_empty end
let print = std::io::println
let capture_print = fn text => handle print text with
  | std::io::!IO.write output => raise output
  | std::io::!IO.write_error _ => ()
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
    for (body, signature) in [
        ("fn _ => !Ask.get ()", "() -> String + !Ask"),
        ("fn _ => fn _ => !Ask.get ()", "() -> () -> String + !Ask"),
    ] {
        let project = project(
            &format!("effect Ask = {{ get: () -> String }}\nlet read_file: {signature} = {body}"),
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
let read: () -> _ = fn _ => handle ask () with | dep::!Ask.get _ => "handled" end
"#,
        "node",
    );
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    fs::write(dependency.join("Ruddy.toml"), "name = \"dep\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"lib.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n").unwrap();
    fs::write(
        dependency.join("lib.rud"),
        "effect Ask = { get: () -> String }\nlet read: () -> String + !Ask = fn _ => !Ask.get ()",
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
    let pure = project("let identity: Nat -> Nat = fn x => x", "web");
    success(&run(pure.path(), "assert.equal(app.identity(42), 42);"));
    let js = fs::read_to_string(pure.path().join("build/host-test.js")).unwrap();
    assert!(!js.contains("node:fs") && !js.contains("process.stdout"));
}

#[test]
fn root_effect_contract_rejects_unhandled_standard_effects_and_wrong_exit_shapes() {
    for source in [
        "let main = fn unit => do let _: () = unit return std::effects::!Halt \"stopped\" end",
        "let main = fn unit => do let _: () = unit return do let _: String = std::effects::!Ask () return () end end",
        "let main = fn unit => do let _: () = unit return std::effects::!Advise \"notice\" end",
        "effect Immediate = () -> ()\nlet main = fn unit => do let _: () = unit return !Immediate () end",
        "effect Exit = String -> |\nlet main = fn unit => do let _: () = unit return !Exit \"wrong argument\" end",
        "effect Exit = { exit: Nat -> | }\nlet main = fn unit => do let _: () = unit return !Exit.exit 1n end",
    ] {
        for kind in ["library", "executable"] {
            let project = project(source, "node");
            let manifest = project.path().join("Ruddy.toml");
            fs::write(
                &manifest,
                fs::read_to_string(&manifest)
                    .unwrap()
                    .replace("kind = \"library\"", &format!("kind = {kind:?}")),
            )
            .unwrap();
            let error = ruddy_cli::check_project(project.path())
                .unwrap_err()
                .to_string();
            let diagnostic = if kind == "executable" {
                "unsupported-entry-effects"
            } else {
                "unsupported-export-effects"
            };
            assert!(error.contains(diagnostic), "{kind}: {source}\n{error}");
        }
    }
}

#[test]
fn immediate_effects_run_at_root_calls_but_not_during_initialization() {
    let foreign = r#"@private extern foreign: String -> String + std::ffi::!Immediate = "text => { console.log(text); return text; }""#;
    for platform in ["node", "web"] {
        let library = project(
            &format!(
                "{foreign}\nlet call = foreign\nlet calls = [foreign]\nlet record = {{ call: foreign }}\nlet curried = fn prefix => fn text => foreign (std::str::concat prefix text)\ntype Loop = String -> Loop + std::ffi::!Immediate\nlet loop: Loop = fn text => do _ = foreign text return loop end"
            ),
            platform,
        );
        let output = run(
            library.path(),
            "assert.equal(await app.call('direct'), 'direct'); assert.equal(await app.calls[0]('array'), 'array'); assert.equal(await app.record.call('record'), 'record'); const call = await app.curried('curried'); assert.equal(await call(' call'), 'curried call'); const loop = await app.loop('first'); await loop('second');",
        );
        success(&output);
        assert_eq!(
            output.stdout,
            b"direct\narray\nrecord\ncurried call\nfirst\nsecond\n"
        );

        let invalid = project(
            &format!("{foreign}\nlet value = foreign \"initializing\""),
            platform,
        );
        let error = ruddy_cli::check_project(invalid.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("unhandled-effect"), "{error}");
        assert!(!invalid.path().join("build").exists());
    }

    for initialize in [false, true] {
        let source = if initialize {
            format!("{foreign}\nlet value = foreign \"initializing\"\nlet main = fn _ => ()")
        } else {
            format!(
                "{foreign}\nlet main = fn _ => do _ = foreign \"main\" return std::io::println \"handled\" end"
            )
        };
        let executable = project(&source, "node");
        let manifest = executable.path().join("Ruddy.toml");
        fs::write(
            &manifest,
            fs::read_to_string(&manifest)
                .unwrap()
                .replace("kind = \"library\"", "kind = \"executable\""),
        )
        .unwrap();
        if initialize {
            let error = ruddy_cli::check_project(executable.path())
                .unwrap_err()
                .to_string();
            assert!(error.contains("unhandled-effect"), "{error}");
        } else {
            ruddy_cli::check_project(executable.path()).unwrap();
            let artifact = ruddy_cli::build_project(executable.path()).unwrap();
            let output = Command::new("node")
                .arg(artifact.with_extension("js"))
                .output()
                .unwrap();
            success(&output);
            assert_eq!(output.stdout, b"main\nhandled\n");
        }
    }
}

#[test]
fn host_exports_exit_drains_output_and_saturates_the_exit_code() {
    let project = project(
        r#"
let stop = fn code => do
  let _ = std::io::print "before exit"
  return std::process::!Exit code
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
let factory: () -> _ = fn _ => { print: std::io::println }
let tagged: #Printer (String -> () + !IO) = #Printer std::io::println
let printers = [std::io::println]
let optional: Bool -> (#Printer (String -> () + std::io::!IO) | #Empty) = fn enable => if enable then #Printer std::io::println else #Empty end
let pair = (std::io::println, 42n)
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
await app.tagged.value('tagged');
await app.printers[0]('array');
const empty = await app.optional(false);
assert.equal(empty.tag, 'Empty');
const some = await app.optional(true);
await some.value('optional');
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
@private type Printer = String -> Printer + std::io::!IO
@private let make : Nat -> Printer = fn count => fn text => do
  let _ = std::io::println (std::str::concat (std::str::from_nat count) text)
  return make (std::nat::add count 1n)
end
let printer = make 0n
let capture: () -> _ = fn _ => handle printer "local" with
  | std::io::!IO.write text => raise text
  | std::io::!IO.write_error _ => ()
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
type Left = String -> Right + std::io::!IO
type Right = Nat -> Left + std::io::!IO
let left : Left = fn text => do
  let _ = std::io::println text
  return right
end
@private let right : Right = fn n => do
  let _ = std::io::println (std::str::from_nat n)
  return left
end
type Id 'a = 'a
type Rotate 'a 'b = 'a -> Rotate 'b 'a + std::io::!IO
let rotate : Rotate (Id String) Nat = fn text => do
  let _ = std::io::println text
  return number
end
@private let number : Rotate Nat String = fn n => do
  let _ = std::io::println (std::str::from_nat n)
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
type Tree = #Leaf | #Branch { print: String -> () + std::io::!IO, children: [Tree] }
let tree : Tree = #Branch { print: std::io::println, children: [#Leaf, #Branch { print: std::io::println, children: [] }] }
type Reader = String -> { text: String, next: Reader } + !FileSystem
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
await app.tree.value.print('root');
await app.tree.value.children[1].value.print('child');
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
let fields : Fields { print: String -> () + std::io::!IO } = { print: std::io::println }
let tagged : Cases (#Printer (String -> () + std::io::!IO)) = #Printer std::io::println
let loop : Loop (std::io::!IO) = fn text => do
  let _ = std::io::println text
  return loop
end
"#,
        "node",
    );
    let dependency = project.path().join("dep");
    fs::create_dir(&dependency).unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(dependency.join("Ruddy.toml"), format!("name = \"dep\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"lib.rud\"\n[dependencies]\nstd = {standard:?}\n")).unwrap();
    fs::write(dependency.join("lib.rud"), "@private type Printer = String -> Printer + std::io::!IO\nlet printer : Printer = fn text => do let _ = std::io::println text return printer end").unwrap();
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
await app.tagged.value('cases');
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
type Cursor 'r = () -> { next: Cursor 'r, ..'r } + std::io::!IO
let optional : { label when 'p: String } -> Cursor { label when 'p: String } = fn fields => do
  let next : Cursor { .. } = fn _ => do
    let _ = std::io::println "step"
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
