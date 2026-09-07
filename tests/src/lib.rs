//! Every test in the workspace.
//!
//! The compiler and the debugger carry no `#[cfg(test)]` modules of their own:
//! their tests live here, one module per module under test, and reach the code
//! through the same public API any other consumer would. A test that needs an
//! item the crate does not export is telling you the item belongs in the API.
//!
//! The crate compiles to nothing outside `cargo test`, so nothing in it can
//! reach a release build.
#![cfg(test)]

mod artifact;
mod bundle;
mod cli;
mod compile;
mod docs;
mod entry;
mod fixed_integers;
mod host_exports;
mod inference;
mod ir;
mod js;
mod link;
mod lir;
mod parse;
mod patterns;
mod print;
mod runtime;
mod sat;
mod session;
mod snapshot;
mod stage;
mod stdlib;
mod stdlib_fs;
mod symbol;
mod token;
mod tracking;
mod types;
mod ui;

mod analysis;
mod lsp;
