//! Tests for the parsed-bundle core compilation seam.

use ruddy::{
    compile::{self, Error},
    inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

#[test]
fn failed_compilation_preserves_completed_phases_and_aggregates_checking_errors() {
    let source = "let bad = 1n 2n\nlet missing = fn truth => match truth with | true => 1n end";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);

    let partial = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .expect_err("the malformed call and incomplete match must not be accepted");

    // Each completed phase remains available, even though no accepted program
    // (and therefore no LIR) can be obtained from this result.
    assert_eq!(partial.ir.program.terms.len(), 2);
    assert_eq!(partial.inference.semantics().schemes().len(), 2);
    assert_eq!(partial.patterns.reports.len(), 1);
    assert!(!partial.inference.errors().is_empty(), "{partial:#?}");
    assert!(!partial.patterns.errors.is_empty(), "{partial:#?}");

    // Core compilation publishes the errors from every checking phase together
    // rather than making callers re-run a later phase to learn its failures.
    assert!(
        partial
            .errors
            .iter()
            .any(|error| matches!(error, Error::Inference(_)))
    );
    assert!(
        partial
            .errors
            .iter()
            .any(|error| matches!(error, Error::Patterns(_)))
    );
}

/// The core seam preserves rich errors independently of trace retention. Only
/// a debugger that asks for a complete trace receives the solver replay data.
#[test]
fn compilation_trace_retention_does_not_change_causal_errors() {
    let source = "let bad = 1n + \"text\"\n\
                  let inspect = fn value => match value with | { field } => field | {} => 0n end";
    let compile = |trace| {
        let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
        assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
        compile::compile(
            Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            trace,
        )
        .expect_err("the type mismatch must leave a partial compilation")
    };

    let off = compile(inference::Trace::Off);
    let complete = compile(inference::Trace::Complete);

    assert_eq!(
        off.inference.errors().len(),
        complete.inference.errors().len()
    );
    for (without_trace, with_trace) in off
        .inference
        .errors()
        .iter()
        .zip(complete.inference.errors())
    {
        assert_eq!(without_trace.explanation, with_trace.explanation);
        assert_eq!(
            without_trace.diagnostic().title,
            with_trace.diagnostic().title
        );
    }
    assert!(
        off.inference
            .errors()
            .iter()
            .any(|error| error.explanation.is_some()),
        "the public compilation error keeps its causal explanation: {off:#?}"
    );

    let diagnostics = off.inference.diagnostics();
    assert_eq!(diagnostics.trace(), inference::Trace::Off);
    assert!(diagnostics.constraints().is_empty());
    assert!(diagnostics.steps().is_empty());
    assert!(diagnostics.reasons().is_empty());
    assert!(diagnostics.variables().is_empty());
    assert!(diagnostics.refinements().is_empty());

    let diagnostics = complete.inference.diagnostics();
    assert_eq!(diagnostics.trace(), inference::Trace::Complete);
    assert!(!diagnostics.constraints().is_empty());
    assert!(!diagnostics.steps().is_empty());
    assert!(!diagnostics.reasons().is_empty());
    assert!(!diagnostics.variables().is_empty());
    assert!(!diagnostics.refinements().is_empty());
}

/// Compile one source that every phase accepts.
fn accepted(source: &str) -> compile::AcceptedProgram {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{source}: {:#?}", partial.errors))
}

/// Compile one source some phase refuses, for the errors it publishes.
fn rejected(source: &str) -> compile::PartialCompilation {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .expect_err("the source is refused")
}

/// The published scheme of one top-level definition, printed.
fn scheme(accepted: &compile::AcceptedProgram, name: &str) -> String {
    accepted
        .semantics()
        .schemes()
        .iter()
        .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
        .map(|(_, scheme)| scheme.to_string())
        .unwrap_or_else(|| panic!("no definition named {name}"))
}

/// The codes of every error a refused compilation published, in order.
fn codes(partial: &compile::PartialCompilation) -> Vec<&'static str> {
    partial
        .errors
        .iter()
        .map(|error| match error {
            Error::Ir(error) => error.kind.code(),
            Error::Inference(error) => error.kind.code(),
            Error::Patterns(error) => error.kind.code(),
        })
        .collect()
}

/// Referring to an operation instantiates its effect's parameters afresh:
/// what the operation is applied to, and what its result is used as,
/// constrain the application the surrounding row carries; independent
/// occurrences of one effect coalesce when their arguments agree; and a
/// reusable operation value stays polymorphic.
#[test]
fn an_operation_reference_instantiates_its_effects_parameters() {
    let source = "effect Ask 'a = { get: () -> 'a }\n\
                  let num : () -> Nat + !Ask Nat = fn _ => !Ask.get ()\n\
                  let any = fn _ => !Ask.get ()\n\
                  let twice = fn _ => let n = !Ask.get () in let m = !Ask.get () in n + m\n\
                  let get = !Ask.get\n\
                  let run = fn _ => handle !Ask.get () with | !Ask.get _ => 1n end\n\
                  let text = fn _ => handle !Ask.get () with | !Ask.get _ => \"s\" end";
    let accepted = accepted(source);
    assert_eq!(scheme(&accepted, "num"), "() -> Nat + !Ask Nat");
    assert_eq!(scheme(&accepted, "any"), "'a -> 'b + !Ask 'b");
    assert_eq!(scheme(&accepted, "twice"), "'a -> Real + !Ask Real");
    assert_eq!(scheme(&accepted, "get"), "() -> 'a + !Ask 'a");
    assert_eq!(scheme(&accepted, "run"), "'a -> Nat");
    assert_eq!(scheme(&accepted, "text"), "'a -> String");

    // Two occurrences whose arguments cannot agree are one computation using
    // incompatible versions of one effect, whichever branch each is on.
    for source in [
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad = fn _ => let n : Nat = !Ask.get () in let s : String = !Ask.get () in ()",
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad = fn b => if b then let n : Nat = !Ask.get () in () else let s : String = !Ask.get () in () end",
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad : () -> Nat + !Ask String = fn _ => !Ask.get ()",
    ] {
        let partial = rejected(source);
        assert_eq!(codes(&partial), ["effect-argument-mismatch"], "{source}");
        let Some(Error::Inference(error)) = partial.errors.first() else {
            unreachable!()
        };
        assert!(
            matches!(
                &error.kind,
                inference::ErrorKind::EffectArgument { effect, position: 0, cause }
                    if effect == "Ask" && matches!(**cause, inference::ErrorKind::Mismatch { .. })
            ),
            "{source}: {:#?}",
            error.kind
        );
        assert!(
            error.explanation.is_some(),
            "{source}: the clash keeps its causal detail"
        );
    }
}

/// Compile one source to the artifact another compilation may depend on.
fn exported(source: &str) -> ruddy::artifact::UncheckedArtifact {
    accepted(source).artifact().to_unchecked()
}

/// Compile one source against a dependency, under the alias `dep`.
fn accepted_with(
    source: &str,
    dependency: &ruddy::artifact::UncheckedArtifact,
) -> compile::AcceptedProgram {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile_with_dependencies(
        Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: dependency,
        }],
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{source}: {:#?}", partial.errors))
}

/// An imported effect keeps its parameters and generic interface, an imported
/// alias expands with the consumer's arguments, and a local declaration with
/// the same generic interface is the same effect whatever its parameters are
/// called.
#[test]
fn imported_effects_and_aliases_behave_like_local_declarations() {
    let dependency = exported(
        "effect Log = { write: Nat -> () }\n\
         effect Ask 'a = { get: () -> 'a }\n\
         effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
         let ask : () -> Nat + !Ask Nat = fn _ => !Ask.get ()",
    );
    let accepted = accepted_with(
        "effect IO = { print: Nat -> () }\n\
         effect Ask 'b = { get: () -> 'b }\n\
         let num : () -> Nat + dep::!Ask Nat = fn _ => dep::!Ask.get ()\n\
         let any = fn _ => dep::!Ask.get ()\n\
         let same : () -> Nat + !Ask Nat = fn _ => dep::!Ask.get ()\n\
         let both : () -> Nat + dep::!Both Nat (!IO) = fn _ => let _ = !IO.print 1n in dep::!Ask.get ()\n\
         let open = fn _ => let _ = dep::!Log.write 1n in dep::!Ask.get ()\n\
         let run = fn _ => handle dep::!Ask.get () with | dep::!Ask.get _ => 1n end",
        &dependency,
    );
    assert_eq!(scheme(&accepted, "num"), "() -> Nat + !Ask Nat");
    assert_eq!(scheme(&accepted, "any"), "'a -> 'b + !Ask 'b");
    assert_eq!(scheme(&accepted, "same"), "() -> Nat + !Ask Nat");
    assert_eq!(
        scheme(&accepted, "both"),
        "() -> Nat + !Ask Nat + !Log + !IO"
    );
    assert_eq!(scheme(&accepted, "open"), "'a -> 'b + !Log + !Ask 'b");
    assert_eq!(scheme(&accepted, "run"), "'a -> Nat");

    // Diagnostics involving an imported application are the local ones.
    let partial = {
        let parsed = parse::parse(
            token::lex(
                "let bad : () -> Nat + dep::!Ask = fn _ => 0n\n\
                 let worse : () -> Nat + dep::!Both Nat = fn _ => 0n",
                FileID::GENERATED,
            )
            .tokens,
        );
        compile::compile_with_dependencies(
            Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: &dependency,
            }],
            inference::Trace::Off,
        )
        .expect_err("the applications are refused")
    };
    assert_eq!(codes(&partial), ["effect-arity", "effect-arity"]);
}
