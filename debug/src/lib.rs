//! The debugger's innards, as a library.
//!
//! The binary in `main.rs` is only argument parsing and a call to
//! [`server::serve`]; everything worth testing is here, where the test crate
//! can reach it.

pub mod dependency_cache;
pub mod docs;
pub mod print;
pub mod server;
pub mod session;
pub mod snapshot;
pub mod stage;
pub mod watch;
pub mod wire;
