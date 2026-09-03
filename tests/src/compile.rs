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
