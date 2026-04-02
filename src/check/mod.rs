mod infer;
mod query;
mod unify;

pub use query::{
    CheckedSource, check_diagnostics, check_diagnostics_fs, check_lowered, check_text,
    check_text_fs,
};
pub use unify::{UnificationError, UnificationTable};

#[cfg(test)]
mod tests;
