pub mod artifact;
pub mod backend;
pub mod bundle;
pub mod compile;
pub mod externs;
pub mod inference;
pub mod ir;
pub mod link;
pub mod lir;
pub mod parse;
pub mod patterns;
pub mod symbol;
#[cfg(test)]
pub(crate) mod test_support;
pub mod token;
pub mod tracking;
pub mod types;
pub mod ui;
