//! The public entry validation and portable artifact boundaries.

use ruddy::{
    artifact::{Artifact, Kind},
    compile, entry, inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

fn compiled(source: &str) -> Artifact {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("app", Version::new(1, 0, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{partial:?}"))
    .artifact()
    .clone()
}

#[test]
fn executable_entry_specializes_polymorphism_and_persists_bundle_kind() {
    let library = compiled("let main = fn x => x");
    assert_eq!(library.header().kind, Kind::Library);
    let executable =
        entry::executable(&library, &[]).expect("identity specializes to unit -> unit");
    let restored = Artifact::try_parse(&executable.print())
        .unwrap()
        .validate()
        .unwrap();
    assert_eq!(restored.header().kind, Kind::Executable);
    assert_eq!(restored, executable);
    let linked = ruddy::link::link(&[restored]).unwrap();
    assert_eq!(linked.header().kind, Kind::Executable);
}

#[test]
fn executable_requires_a_root_function_compatible_with_unit() {
    for source in [
        "let value = ()",
        "module Nested = let main = fn _ => () end",
    ] {
        assert_eq!(
            entry::executable(&compiled(source), &[]),
            Err(entry::Error::MissingMain)
        );
    }
    for source in [
        "let main = 0n",
        "let main : Nat -> () = fn _ => ()",
        "let main = fn _ => 42n",
        "let main = fn _ => fn _ => ()",
    ] {
        assert!(
            matches!(
                entry::executable(&compiled(source), &[]),
                Err(entry::Error::InvalidMain(_))
            ),
            "{source}"
        );
    }
}

#[test]
fn executable_artifacts_cannot_be_imported_or_linked_as_dependencies() {
    let executable = entry::executable(&compiled("let main = fn _ => ()"), &[]).unwrap();
    let source = parse::parse(token::lex("let value = ()", FileID::GENERATED).tokens);
    let result = compile::compile_with_dependencies(
        Mint::new(Bundle::new("consumer", Version::new(1, 0, 0)).unwrap()),
        source.stmts,
        &[compile::Dependency {
            alias: Some("app"),
            artifact: compile::DependencyArtifact::Checked(&executable),
        }],
        inference::Trace::Off,
    )
    .expect_err("an executable cannot cross the import boundary");
    assert!(result.ir.errors.iter().any(|error| matches!(
        error.kind,
        ruddy::ir::ErrorKind::ExecutableDependency { .. }
    )));
    assert!(matches!(
        ruddy::link::link(&[executable, compiled("let value = ()")]),
        Err(ruddy::link::LinkError::ExecutableDependency(_))
    ));
}

#[test]
fn node_filesystem_support_requires_the_complete_structural_interface() {
    let library = compiled(
        "effect FileSystem = { exists: String -> Bool }\n\
         let main = fn _ => do let _ = !FileSystem.exists \"file\" return () end",
    );
    let executable = entry::executable(&library, &[]).unwrap();
    assert!(ruddy::backend::js::check_entry(&executable, &[]).is_err());
}
