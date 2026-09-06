//! Tests for [`ruddy::bundle`].
//!
//! Almost everything here compiles a bundle held in a [`HashMap`] rather than on
//! a disk, which is the whole reason [`Files`] is a trait: what is being checked
//! is the walk — which files it reads, in which order, and what it says when one
//! is missing — and a directory tree would only make each case harder to read.
//! The fixtures under `tests/bundles/` are checked in so that [`Disk`] is
//! exercised against a real filesystem too, since it is the one implementation
//! nothing else can stand in for.

use std::{collections::HashMap, path::PathBuf};

use ruddy::{
    bundle::{self, Disk, Environment, ErrorKind, Files, Output},
    inference,
    ir::{self, TypeKind},
    parse::StmtKind,
    symbol::{Bundle, Mint, Version},
    tracking::{FileID, FileManager},
};

/// A bundle held in memory: a path per file, exactly as [`Files`] wants them —
/// relative to the root's directory and `/`-separated.
struct Memory(HashMap<String, String>);

impl Files for Memory {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(path).cloned()
    }
}

/// A build for the named target on Node, the platform every build had until
/// there was a choice. The platform tests spell theirs out.
fn environment(target: &str) -> Environment {
    Environment::new([("target", target), ("platform", "node")])
}

/// The build most tests load for: a JavaScript one. Only the guard tests care
/// which, and they say so.
fn js() -> Environment {
    environment("js")
}

/// Load an in-memory bundle rooted at `main.hc` for a JavaScript build, with
/// the file manager the spans name their files through.
fn load(files: &[(&str, &str)]) -> (FileManager, Output) {
    load_for("js", files)
}

/// [`load`], for a build of the named target.
fn load_for(target: &str, files: &[(&str, &str)]) -> (FileManager, Output) {
    load_in(&environment(target), files)
}

/// [`load`], for whatever build `environment` describes.
fn load_in(environment: &Environment, files: &[(&str, &str)]) -> (FileManager, Output) {
    let fs = Memory(
        files
            .iter()
            .map(|(path, source)| ((*path).to_string(), (*source).to_string()))
            .collect(),
    );
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc", environment);
    (manager, out)
}

/// One file's worth of bundle, which is what most of the tests below want.
fn one(source: &str) -> Output {
    load(&[("main.hc", source)]).1
}

/// The paths the loader read, in the order it read them.
fn paths(out: &Output) -> Vec<&str> {
    out.loaded.iter().map(|file| file.path.as_str()).collect()
}

/// The names of the top-level statements, so a splice can be checked without
/// the whole tree being written out.
fn names(stmts: &[ruddy::parse::Stmt]) -> Vec<String> {
    stmts
        .iter()
        .map(|stmt| match &stmt.kind {
            StmtKind::Let { pattern, .. } => format!("let {}", pattern.tracked),
            StmtKind::Extern { name, .. } => format!("extern {}", name.tracked),
            StmtKind::Type { name, .. } => format!("type {}", name.tracked),
            StmtKind::Effect { name, .. } => format!("effect {}", name.tracked),
            StmtKind::Module { name, .. } => format!("module {}", name.tracked),
        })
        .collect()
}

/// The body of the module named `name`, wherever it sits in the top level.
fn body<'a>(stmts: &'a [ruddy::parse::Stmt], name: &str) -> &'a [ruddy::parse::Stmt] {
    stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            StmtKind::Module { name: at, body } if at.tracked == name => body.as_deref(),
            _ => None,
        })
        .expect("the module is declared at the top level")
}

/// The repository's own fixture directory, which the two [`Disk`] tests read.
fn fixture(name: &str) -> Disk {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("bundles");
    root.push(name);
    Disk::new(root)
}

#[test]
fn a_sandbox_that_cannot_be_resolved_reads_nothing() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("main.hc"), "let main = 0n\n").unwrap();
    let disk = Disk::sandboxed(directory.path(), directory.path().join("missing"));
    assert!(disk.read("main.hc").is_none());
}

/// The ordinary case: one file and nothing to splice. Everything after it is
/// a variation on this, so it is worth pinning that the plain program costs no
/// complaints at all.
#[test]
fn a_single_file_bundle_loads_with_no_errors() {
    let out = one("let id = fn x => x\n");

    assert_eq!(paths(&out), ["main.hc"]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(names(&out.stmts), ["let id"]);
}

/// The whole point of the phase: a module declared with no body gets one from
/// the file its logical path names, and that file's own modules get theirs, all
/// the way down. Root first, then depth-first in declaration order — which is
/// the order the debugger shows its file strip in, so it is not an accident to
/// be rediscovered later.
#[test]
fn a_nested_bundle_splices_every_file_into_one_tree() {
    let (manager, out) = load(&[
        ("main.hc", "module Math\nlet four = Math::double 2n\n"),
        ("Math.hc", "module Vec\nlet double = fn x => x\n"),
        ("Math/Vec.hc", "let zero = 0n\n"),
    ]);

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math.hc", "Math/Vec.hc"]);
    assert_eq!(names(&out.stmts), ["module Math", "let four"]);

    let math = body(&out.stmts, "Math");
    assert_eq!(names(math), ["module Vec", "let double"]);
    assert_eq!(names(body(math, "Vec")), ["let zero"]);

    // Three files, three ids, and every span pointing at the file it was
    // written in. This is what lets a diagnostic be traced back to a file at
    // all, so it is checked directly rather than through anything downstream.
    let ids: Vec<FileID> = out.loaded.iter().map(|file| file.id).collect();
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_ne!(ids[0], ids[2]);
    let mut manager = manager;
    for file in &out.loaded {
        assert_eq!(manager.get_file(file.id).path, file.path);
        for token in &file.tokens {
            assert_eq!(token.span.file_id, file.id, "{}", file.path);
        }
    }
    assert_eq!(out.stmts[0].span.file_id, ids[0]);
    assert_eq!(math[0].span.file_id, ids[1]);
    assert_eq!(body(math, "Vec")[0].span.file_id, ids[2]);
}

/// File-backed modules are ordinary modules to the compiler too: equivalent
/// effects declared in two files coalesce, and an operation from either one
/// satisfies the other's row annotation.
#[test]
fn file_modules_share_structural_effects() {
    let out = load(&[
        (
            "main.hc",
            "module Foo\nmodule Bar\nlet cross : Nat -> {} + Foo::!Log = fn n => do let _ = Bar::!Log.write n return {} end\n",
        ),
        ("Foo.hc", "effect Log = { write: Nat -> () }\n"),
        ("Bar.hc", "effect Log = { write: Nat -> () }\n"),
    ])
    .1;
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut mint = Mint::new(Bundle::new("demo", Version::new(0, 1, 0)).expect("valid bundle"));
    let mut lowered = ir::build(&mut mint, out.stmts);
    assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
    let inferred = inference::infer(&mint, &lowered.program, inference::Trace::Off);
    inferred.apply_types(&mut lowered.program);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    let cross = lowered
        .program
        .terms
        .iter()
        .find(|(symbol, _)| mint.name(**symbol) == "cross")
        .map(|(_, decl)| decl)
        .expect("cross declaration");
    let TypeKind::Arrow { effects, .. } =
        &cross.annotation.as_ref().expect("annotation").ty.anchored
    else {
        panic!("cross has an arrow annotation");
    };
    assert_eq!(effects.effects.len(), 1);
    assert_eq!(effects.effects.keys().next().expect("effect").name(), "Log");
}

/// The second spelling. `A/module.hc` and `A.hc` are the same module written
/// two ways, so the tree they produce has to be the same tree — only the path
/// the loader read differs.
#[test]
fn a_module_directory_serves_in_place_of_a_file_beside_it() {
    let out = load(&[
        ("main.hc", "module Math\n"),
        ("Math/module.hc", "let double = fn x => x\n"),
    ])
    .1;

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math/module.hc"]);
    assert_eq!(names(body(&out.stmts, "Math")), ["let double"]);
}

/// A module's file path mirrors its *logical* path, and an inline module
/// contributes a component exactly as a file module does. So `B` inside `A` is
/// looked for under `A/`, and never beside the root — which is the one thing
/// about this rule a reader could reasonably guess wrong.
#[test]
fn a_file_module_inside_an_inline_module_is_looked_for_under_it() {
    let out = load(&[
        ("main.hc", "module A =\n  module B\nend\n"),
        ("A/B.hc", "let x = 1n\n"),
        // Beside the root, where it must *not* be found: a loader that dropped
        // the enclosing module from the path would read this one and pass.
        ("B.hc", "let wrong = 1n\n"),
    ])
    .1;

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "A/B.hc"]);
    let a = body(&out.stmts, "A");
    assert_eq!(names(body(a, "B")), ["let x"]);
}

/// A module with no file is reported at the declaration, and the two paths it
/// could have been at are named — which is the whole of the fix. The rest of
/// the bundle is still returned: one missing file must not hide every other
/// complaint in the program.
#[test]
fn a_module_with_no_file_is_reported_and_the_rest_still_loads() {
    let out = one("module Gone\nlet x = 1n\n");

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(
        out.errors[0].kind,
        ErrorKind::ModuleFileMissing {
            beside: "Gone.hc".to_string(),
            inside: "Gone/module.hc".to_string(),
        }
    );
    // At the declaration, which is the name the reader wrote.
    let source = "module Gone\nlet x = 1n\n";
    let at = source.find("Gone").expect("the module is declared");
    assert_eq!(out.errors[0].span.start, at);
    // An empty body rather than none, so nothing downstream has to know the
    // file was missing.
    assert_eq!(names(body(&out.stmts, "Gone")), [] as [String; 0]);
    assert_eq!(names(&out.stmts), ["module Gone", "let x"]);
}

/// Repeating a file module is one module error, not a second import of that
/// module's source. Otherwise every definition in the file would be reported
/// as a duplicate even though it was written only once.
#[test]
fn a_repeated_file_module_does_not_splice_its_body_twice() {
    let out = load(&[
        ("main.hc", "module Math\nmodule Math\n"),
        ("Math.hc", "let double = fn x => x\n"),
    ])
    .1;

    assert_eq!(paths(&out), ["main.hc", "Math.hc"]);
    let mut mint = Mint::new(Bundle::new("demo", Version::new(0, 1, 0)).expect("valid bundle"));
    let built = ruddy::ir::build(&mut mint, out.stmts);
    assert_eq!(
        built
            .errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-module"],
        "{:#?}",
        built.errors,
    );
}

/// Two files for one module is refused rather than resolved. Which of them was
/// meant is not the compiler's to guess, so it says what to delete and leaves
/// the module empty.
#[test]
fn a_module_with_two_files_is_reported_and_neither_is_read() {
    let out = load(&[
        ("main.hc", "module Math\nlet x = 1n\n"),
        ("Math.hc", "let beside = 1n\n"),
        ("Math/module.hc", "let inside = 1n\n"),
    ])
    .1;

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(
        out.errors[0].kind,
        ErrorKind::ModuleFileAmbiguous {
            beside: "Math.hc".to_string(),
            inside: "Math/module.hc".to_string(),
        }
    );
    assert_eq!(paths(&out), ["main.hc"]);
    assert_eq!(names(body(&out.stmts, "Math")), [] as [String; 0]);
    assert_eq!(names(&out.stmts), ["module Math", "let x"]);
}

/// The configured root is loaded for its root role, not as an earlier load of
/// a same-named module body. Both module candidates must therefore still be
/// considered when the root itself has the beside spelling.
#[test]
fn a_root_name_collision_does_not_hide_ambiguous_module_files() {
    let fs = Memory(HashMap::from([
        ("Child.hc".to_string(), "module Child\n".to_string()),
        (
            "Child/module.hc".to_string(),
            "let inside = 1n\n".to_string(),
        ),
    ]));
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "Child.hc", &js());

    assert_eq!(
        out.errors
            .iter()
            .map(|error| &error.kind)
            .collect::<Vec<_>>(),
        [&ErrorKind::ModuleFileAmbiguous {
            beside: "Child.hc".to_string(),
            inside: "Child/module.hc".to_string(),
        }],
        "{:#?}",
        out.errors,
    );
    assert_eq!(paths(&out), ["Child.hc"]);
    assert_eq!(names(body(&out.stmts, "Child")), [] as [String; 0]);
}

/// Every phase's complaints ride on their own file, so a lex error in a module
/// file and a parse error in another are both reachable and both point at the
/// right place. This is the half of the load that is not the loader's own
/// complaints, and nothing else checks it.
#[test]
fn each_file_carries_its_own_lex_and_parse_errors() {
    let out = load(&[
        ("main.hc", "module A\nmodule B\n"),
        ("A.hc", "let x = @\n"),
        ("B.hc", "let = 1n\n"),
    ])
    .1;

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(out.loaded[0].lex_errors.is_empty());
    assert!(out.loaded[0].parse_errors.is_empty());
    assert_eq!(out.loaded[1].lex_errors.len(), 1);
    assert_eq!(out.loaded[1].lex_errors[0].span.file_id, out.loaded[1].id);
    assert_eq!(out.loaded[2].parse_errors.len(), 1);
    assert_eq!(out.loaded[2].parse_errors[0].span.file_id, out.loaded[2].id);
}

/// `@if {target: ...}` keeps a definition for the build it names and drops it
/// for every other, whatever kind of definition it is. What survives keeps
/// its `@if` as ordinary metadata; what is dropped leaves nothing behind.
#[test]
fn a_guard_keeps_a_definition_for_its_target_and_drops_it_for_others() {
    let source = "@if {target: \"js\"} let a = 1n\n\
                  @if {target: \"artifact\"} let b = 2n\n\
                  let c = 3n\n\
                  @if {target: \"js\"} type T = Nat\n\
                  @if {target: \"artifact\"} effect E = () -> ()\n\
                  @if {target: \"artifact\"} module M = let inner = 0n end\n\
                  @if {target: \"js\"} extern f : Nat = \"1\"\n";

    let out = load_for("js", &[("main.hc", source)]).1;
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(names(&out.stmts), ["let a", "let c", "type T", "extern f"]);
    assert_eq!(out.stmts[0].attributes[0].key.tracked, "if");
    assert!(out.stmts[1].attributes.is_empty());

    let out = load_for("artifact", &[("main.hc", source)]).1;
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(
        names(&out.stmts),
        ["let b", "let c", "effect E", "module M"]
    );
}

/// A file module guarded out is never looked for, so its file need not exist
/// for the builds it is not part of. One that is guarded in is looked for
/// exactly as an unguarded one would be, missing file and all.
#[test]
fn a_guarded_file_module_is_only_looked_for_when_its_guard_holds() {
    let files = [
        (
            "main.hc",
            "@if {target: \"js\"} module js\n@if {target: \"artifact\"} module native\nlet x = 1n\n",
        ),
        ("js.hc", "let now = 0n\n"),
    ];

    let out = load_for("js", &files).1;
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "js.hc"]);
    assert_eq!(names(&out.stmts), ["module js", "let x"]);
    assert_eq!(names(body(&out.stmts, "js")), ["let now"]);

    let out = load_for("artifact", &files).1;
    assert_eq!(paths(&out), ["main.hc"]);
    assert_eq!(names(&out.stmts), ["module native", "let x"]);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::ModuleFileMissing { .. }
    ));
}

/// Guards are judged wherever a definition can carry metadata: inside an
/// inline module's body and in a module file alike, with nothing under an
/// excluded module judged or read at all.
#[test]
fn guards_are_judged_inside_module_bodies() {
    let out = load_for(
        "js",
        &[
            (
                "main.hc",
                "module A\nmodule B = @if {target: \"artifact\"} let hidden = 0n let shown = 1n end\n@if {target: \"artifact\"} module C = module D end\n",
            ),
            (
                "A.hc",
                "@if {target: \"artifact\"} let gone = 0n\nlet kept = 1n\n@if {target: \"artifact\"} module Deep\n",
            ),
        ],
    )
    .1;

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "A.hc"]);
    assert_eq!(names(&out.stmts), ["module A", "module B"]);
    assert_eq!(names(body(&out.stmts, "A")), ["let kept"]);
    assert_eq!(names(body(&out.stmts, "B")), ["let shown"]);
}

/// The reason the guard is judged here and not in lowering: two definitions
/// of one name, guarded for two targets, are one definition to everything
/// after the load. A target no backend answers to holds for no build, so a
/// guard can be written for a backend before it exists.
#[test]
fn definitions_guarded_for_different_targets_do_not_collide() {
    let source = "@if {target: \"js\"} let f = 1n\n\
                  @if {target: \"artifact\"} let f = 2n\n\
                  @if {target: \"rust\"} let f = 3n\n";
    for target in ["js", "artifact"] {
        let out = load_for(target, &[("main.hc", source)]).1;
        assert!(out.errors.is_empty(), "{target}: {:#?}", out.errors);
        assert_eq!(names(&out.stmts), ["let f"], "{target}");
        let mut mint = Mint::new(Bundle::new("demo", Version::new(0, 1, 0)).expect("valid bundle"));
        let built = ruddy::ir::build(&mut mint, out.stmts);
        assert!(built.errors.is_empty(), "{target}: {:#?}", built.errors);
    }
}

/// Every fact of the build is a condition, and a guard holds only when every
/// fact it names has the value it wants. A fact it does not name may be
/// anything: `platform` alone spans both targets, and `target` alone both
/// platforms.
#[test]
fn a_guard_holds_when_every_fact_it_names_holds() {
    let source = "@if {platform: \"web\"} let w = 1n\n\
                  @if {platform: \"node\"} let n = 2n\n\
                  @if {target: \"js\", platform: \"web\"} let jw = 3n\n\
                  @if {target: \"js\"} let j = 4n\n";
    for (target, platform, expected) in [
        ("js", "web", &["let w", "let jw", "let j"][..]),
        ("js", "node", &["let n", "let j"]),
        ("artifact", "web", &["let w"]),
        ("artifact", "node", &["let n"]),
    ] {
        let environment = Environment::new([("target", target), ("platform", platform)]);
        let out = load_in(&environment, &[("main.hc", source)]).1;
        assert!(
            out.errors.is_empty(),
            "{target}/{platform}: {:#?}",
            out.errors
        );
        assert_eq!(names(&out.stmts), expected, "{target}/{platform}");
    }
}

/// A repeated `@if`, or a repeated field inside one, is the duplicate that
/// lowering already refuses. The loader keeps the definition so that lowering
/// sees it and says so, rather than judging one of two spellings and dropping
/// the definition in silence.
#[test]
fn a_repeated_guard_or_field_is_kept_for_lowering_to_refuse() {
    for (source, code) in [
        (
            "@if {target: \"artifact\"} @if {target: \"js\"} let x = 1n\n",
            "duplicate-metadata-key",
        ),
        (
            "@if {target: \"artifact\", target: \"js\"} let x = 1n\n",
            "duplicate-field",
        ),
    ] {
        let out = one(source);
        assert!(out.errors.is_empty(), "{source:?}: {:#?}", out.errors);
        assert_eq!(names(&out.stmts), ["let x"], "{source:?}");
        let mut mint = Mint::new(Bundle::new("demo", Version::new(0, 1, 0)).expect("valid bundle"));
        let built = ruddy::ir::build(&mut mint, out.stmts);
        assert_eq!(
            built
                .errors
                .iter()
                .map(|error| error.kind.code())
                .collect::<Vec<_>>(),
            [code],
            "{source:?}: {:#?}",
            built.errors
        );
    }
}

/// A guard that cannot be judged is reported at the part that is wrong and
/// keeps its definition, so that the one complaint is not followed by an
/// unresolved name for everything that used it.
#[test]
fn a_malformed_guard_is_reported_and_keeps_its_definition() {
    for (source, at, expected) in [
        ("@if let a = 1n\n", "@if", ErrorKind::ConditionMissing),
        ("@if () let a = 1n\n", "()", ErrorKind::ConditionMissing),
        ("@if {} let a = 1n\n", "{}", ErrorKind::ConditionMissing),
        (
            "@if \"js\" let a = 1n\n",
            "\"js\"",
            ErrorKind::ConditionNotStruct,
        ),
        (
            "@if {taget: \"js\"} let a = 1n\n",
            "taget",
            ErrorKind::ConditionUnknownField {
                name: "taget".to_string(),
                known: vec!["target".to_string(), "platform".to_string()],
            },
        ),
        (
            "@if {target: 1n} let a = 1n\n",
            "1n",
            ErrorKind::ConditionNotString {
                name: "target".to_string(),
            },
        ),
        (
            "@if {platform: #Web} let a = 1n\n",
            "#Web",
            ErrorKind::ConditionNotString {
                name: "platform".to_string(),
            },
        ),
    ] {
        let out = one(source);
        assert_eq!(out.errors.len(), 1, "{source:?}: {:#?}", out.errors);
        assert_eq!(out.errors[0].kind, expected, "{source:?}");
        assert_eq!(
            out.errors[0].span.start,
            source.find(at).expect("the offending part"),
            "{source:?}"
        );
        assert_eq!(names(&out.stmts), ["let a"], "{source:?}");
    }
}

/// [`Disk`] against a real directory, which is the one implementation an
/// in-memory map cannot stand in for: the `/`-separated path a module names has
/// to become the platform's own, and a fixture is the only way to know it did.
#[test]
fn disk_reads_a_checked_in_fixture() {
    let fs = fixture("nested");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc", &js());

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math.hc", "Math/Vec.hc"]);
    let math = body(&out.stmts, "Math");
    assert_eq!(names(body(math, "Vec")), ["let zero"]);
}

/// The `A/module.hc` spelling, on a real filesystem, for the reason above.
#[test]
fn disk_reads_the_directory_spelling() {
    let fs = fixture("dir-form");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc", &js());

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math/module.hc"]);
}

/// A fixture whose module file is genuinely absent, so the complaint is reached
/// through the disk rather than through a map that was told to say no.
#[test]
fn disk_reports_a_fixture_whose_module_file_is_missing() {
    let fs = fixture("broken");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc", &js());

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::ModuleFileMissing { .. }
    ));
}

/// A path that is not there is `None` rather than an empty file, which is what
/// the two-candidate check leans on: "exactly one of these exists" is a
/// question only a total answer can settle.
#[test]
fn disk_answers_none_for_a_path_that_is_not_there() {
    let fs = fixture("nested");

    assert!(fs.read("main.hc").is_some());
    assert!(fs.read("Nope.hc").is_none());
    assert!(fs.read("Math/Nope.hc").is_none());
}

/// A root the loader cannot read is an empty file rather than a panic. Only the
/// root can reach this — a module's file is looked for before it is read.
#[test]
fn a_root_that_is_not_there_loads_as_an_empty_file() {
    let out = load(&[]).1;

    assert_eq!(paths(&out), ["main.hc"]);
    assert!(out.stmts.is_empty());
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}
