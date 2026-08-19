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
    bundle::{self, Disk, ErrorKind, Files, Output},
    inference,
    ir::{self, TypeKind},
    parse::StmtKind,
    symbol::{Mint, Version},
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

/// Load an in-memory bundle rooted at `main.hc`, with the file manager the spans
/// name their files through.
fn load(files: &[(&str, &str)]) -> (FileManager, Output) {
    let fs = Memory(
        files
            .iter()
            .map(|(path, source)| ((*path).to_string(), (*source).to_string()))
            .collect(),
    );
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc");
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
        .map(|stmt| match &stmt.tracked {
            StmtKind::Let { pattern, .. } => format!("let {}", pattern.tracked),
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
        .find_map(|stmt| match &stmt.tracked {
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

/// The ordinary case: one file, a header, and nothing to splice. Everything
/// after it is a variation on this, so it is worth pinning that the plain
/// program costs no complaints at all.
#[test]
fn a_single_file_bundle_loads_with_no_errors() {
    let out = one("bundle demo 0.1.0\n\nlet id = fn x => x\n");

    let bundle = out
        .bundle
        .as_ref()
        .expect("the header declared an identity");
    assert_eq!(bundle.name(), "demo");
    assert_eq!(*bundle.version(), Version::new(0, 1, 0));
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
        (
            "main.hc",
            "bundle demo 0.1.0\nmodule Math\nlet four = Math::double 2\n",
        ),
        ("Math.hc", "module Vec\nlet double = fn x => x\n"),
        ("Math/Vec.hc", "let zero = 0\n"),
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
            "bundle demo 0.1.0\nmodule Foo\nmodule Bar\nlet cross : Nat -> {} + Foo::!Log = fn n => let _ = Bar::!Log.write n in {}\n",
        ),
        ("Foo.hc", "effect Log = write : Nat -> ()\n"),
        ("Bar.hc", "effect Log = write : Nat -> ()\n"),
    ])
    .1;
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut mint = Mint::new(out.bundle.clone().expect("bundle identity"));
    let mut lowered = ir::build(&mut mint, out.stmts);
    assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
    let inferred = inference::infer(&mint, &mut lowered.program);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    let cross = lowered
        .program
        .terms
        .iter()
        .find(|(symbol, _)| mint.name(**symbol) == "cross")
        .map(|(_, decl)| decl)
        .expect("cross declaration");
    let TypeKind::Arrow { effects, .. } =
        &cross.annotation.as_ref().expect("annotation").ty.tracked
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
        ("main.hc", "bundle demo 0.1.0\nmodule Math\n"),
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
        (
            "main.hc",
            "bundle demo 0.1.0\nmodule A =\n  module B\nend\n",
        ),
        ("A/B.hc", "let x = 1\n"),
        // Beside the root, where it must *not* be found: a loader that dropped
        // the enclosing module from the path would read this one and pass.
        ("B.hc", "let wrong = 1\n"),
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
    let out = one("bundle demo 0.1.0\nmodule Gone\nlet x = 1\n");

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(
        out.errors[0].kind,
        ErrorKind::ModuleFileMissing {
            beside: "Gone.hc".to_string(),
            inside: "Gone/module.hc".to_string(),
        }
    );
    // At the declaration, which is the name the reader wrote.
    let source = "bundle demo 0.1.0\nmodule Gone\nlet x = 1\n";
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
        ("main.hc", "bundle demo 0.1.0\nmodule Math\nmodule Math\n"),
        ("Math.hc", "let double = fn x => x\n"),
    ])
    .1;

    assert_eq!(paths(&out), ["main.hc", "Math.hc"]);
    let mut mint = ruddy::symbol::Mint::new(out.bundle.clone().expect("valid header"));
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
        ("main.hc", "bundle demo 0.1.0\nmodule Math\nlet x = 1\n"),
        ("Math.hc", "let beside = 1\n"),
        ("Math/module.hc", "let inside = 1\n"),
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

/// Only the root file carries the identity, so a header anywhere else is a
/// second answer to a settled question. The file's own statements still load.
#[test]
fn a_header_outside_the_root_is_reported_at_the_header() {
    let out = load(&[
        ("main.hc", "bundle demo 0.1.0\nmodule Math\n"),
        ("Math.hc", "bundle other 1.0.0\nlet double = fn x => x\n"),
    ])
    .1;

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::MisplacedBundleDeclaration);
    assert_eq!(out.errors[0].span.start, 0);
    assert_eq!(out.errors[0].span.file_id, out.loaded[1].id);
    // The root's identity stands, and the file loads anyway.
    assert_eq!(
        out.bundle.as_ref().expect("the root declared one").name(),
        "demo"
    );
    assert_eq!(names(body(&out.stmts, "Math")), ["let double"]);
}

/// The root must open with one. Reported at the start of the file, which is
/// where the missing line would go, and compilation continues under the
/// fallback so every later phase still produces output.
#[test]
fn a_root_with_no_header_is_reported_at_the_start_of_the_file() {
    let out = one("let x = 1\n");

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::MissingBundleDeclaration);
    assert_eq!(out.errors[0].span.start, 0);
    assert_eq!(out.errors[0].span.width, 0);
    assert!(out.bundle.is_none());
    assert_eq!(names(&out.stmts), ["let x"]);
}

/// A name the mint cannot mangle is refused at the header, and the bundle comes
/// back as none so the caller falls back. The rest of the file still loads —
/// the whole point of the fallback is that a reader still typing the first line
/// keeps seeing every later phase.
#[test]
fn a_header_the_mint_refuses_is_reported_and_leaves_no_identity() {
    // A leading `_` is a perfectly good identifier and not a bundle name: only
    // a letter may start one. Refused by `Bundle::new`, not by the lexer.
    let out = one("bundle _demo 0.1.0\nlet x = 1\n");

    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::BadBundleIdentity);
    assert_eq!(out.errors[0].span.start, 0);
    assert!(out.bundle.is_none());
    assert_eq!(names(&out.stmts), ["let x"]);
}

/// Every phase's complaints ride on their own file, so a lex error in a module
/// file and a parse error in another are both reachable and both point at the
/// right place. This is the half of the load that is not the loader's own
/// complaints, and nothing else checks it.
#[test]
fn each_file_carries_its_own_lex_and_parse_errors() {
    let out = load(&[
        ("main.hc", "bundle demo 0.1.0\nmodule A\nmodule B\n"),
        ("A.hc", "let x = @\n"),
        ("B.hc", "let = 1\n"),
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

/// [`Disk`] against a real directory, which is the one implementation an
/// in-memory map cannot stand in for: the `/`-separated path a module names has
/// to become the platform's own, and a fixture is the only way to know it did.
#[test]
fn disk_reads_a_checked_in_fixture() {
    let fs = fixture("nested");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc");

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math.hc", "Math/Vec.hc"]);
    assert_eq!(
        out.bundle
            .as_ref()
            .expect("the fixture has a header")
            .name(),
        "nested"
    );
    let math = body(&out.stmts, "Math");
    assert_eq!(names(body(math, "Vec")), ["let zero"]);
}

/// The `A/module.hc` spelling, on a real filesystem, for the reason above.
#[test]
fn disk_reads_the_directory_spelling() {
    let fs = fixture("dir-form");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc");

    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(paths(&out), ["main.hc", "Math/module.hc"]);
}

/// A fixture whose module file is genuinely absent, so the complaint is reached
/// through the disk rather than through a map that was told to say no.
#[test]
fn disk_reports_a_fixture_whose_module_file_is_missing() {
    let fs = fixture("broken");
    let mut manager = FileManager::new();
    let out = bundle::load(&mut manager, &fs, "main.hc");

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

/// The identity a bundle compiles under when its header is missing or refused.
/// It has to be one the mint accepts, or the fallback would be a second way to
/// fail rather than a way not to.
#[test]
fn the_fallback_identity_is_a_valid_bundle() {
    let fallback = bundle::fallback();

    assert_eq!(
        ruddy::symbol::Bundle::new(fallback.name(), fallback.version().clone()),
        Some(fallback.clone())
    );
    assert!(fallback.version().pre.is_empty());
    assert!(fallback.version().build.is_empty());
}

/// A root the loader cannot read is an empty file rather than a panic. Only the
/// root can reach this — a module's file is looked for before it is read — and
/// what comes back is the complaint about the header it therefore does not
/// have.
#[test]
fn a_root_that_is_not_there_loads_as_an_empty_file() {
    let out = load(&[]).1;

    assert_eq!(paths(&out), ["main.hc"]);
    assert!(out.stmts.is_empty());
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::MissingBundleDeclaration);
}
