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
    assert!(partial.inference.errors().len() >= 1, "{partial:#?}");
    assert!(partial.patterns.errors.len() >= 1, "{partial:#?}");

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
