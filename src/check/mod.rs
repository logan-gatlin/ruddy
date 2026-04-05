//! Hindley-Milner type checking over lowered IR.
//!
//! This module owns the end-to-end checking pipeline that turns
//! [`crate::lower::ir::LoweredSource`] into type-annotated IR in
//! [`crate::ty::typed_ir`], while collecting type diagnostics.

mod infer;
mod query;
mod unify;

pub use query::{CheckedSource, check_diagnostics, check_lowered, check_text, check_text_fs};
pub use unify::{UnificationError, UnificationTable};

#[cfg(test)]
mod tests;
