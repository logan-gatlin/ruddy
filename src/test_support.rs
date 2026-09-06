//! Crate-private test support.
//!
//! Every ordinary test lives in the workspace's `tests` crate and reaches the
//! compiler through its public interface. A few tests deliberately do what no
//! public caller may: fabricate solver identifiers, or corrupt an otherwise
//! coherent publication to reach a defensive branch. Those live beside the
//! module they test and start from here, so the corruption stays behind the
//! crate boundary rather than becoming a public mutation requirement.

use crate::{
    inference::{self, Trace},
    ir, parse, patterns,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

pub(crate) fn mint() -> Mint {
    Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
}

/// Lex and parse one source, which must be syntactically clean.
pub(crate) fn stmts(source: &str) -> Vec<parse::Stmt> {
    let lexed = token::lex(source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    parsed.stmts
}

/// Build and infer one source. Only the parse is required to be clean.
pub(crate) fn inferred(source: &str, trace: Trace) -> (Mint, ir::Output, inference::Output) {
    let mut mint = mint();
    let mut out = ir::build(&mut mint, stmts(source));
    let inferred = inference::infer(&mint, &out.program, trace);
    inferred.apply_types(&mut out.program);
    (mint, out, inferred)
}

/// Build, infer and pattern-check one source that every phase accepts.
pub(crate) fn accepted(source: &str) -> (Mint, ir::Output, inference::Output, patterns::Output) {
    let (mint, out, inferred) = inferred(source, Trace::Off);
    assert!(out.errors.is_empty(), "{source}: {:#?}", out.errors);
    assert!(
        inferred.errors().is_empty(),
        "{source}: {:#?}",
        inferred.errors()
    );
    let checked = patterns::check(&out.program, &inferred);
    assert!(checked.errors.is_empty(), "{source}: {:#?}", checked.errors);
    (mint, out, inferred, checked)
}
