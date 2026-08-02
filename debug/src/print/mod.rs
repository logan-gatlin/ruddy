//! Rendering compiler trees back as surface syntax.
//!
//! The compiler does not print its own trees. Lowering and parsing produce
//! structures; turning one back into something a person reads is a debugging
//! concern, so all of it lives here — the [`ast`] tree as written and the [`ir`]
//! tree as lowered.
//!
//! Both print the *same* surface grammar, and so does the compiler when it
//! prints a type into a diagnostic. That rule therefore lives one crate down,
//! in [`ruddy::grammar`] — [`Prec`], one table per node kind ([`Grouped`]), and
//! one writer per position ([`write_apply`] and friends) — and is re-exported
//! here for the two printers that read it. It cannot live in this crate: the
//! compiler would have to depend on the debugger to reach it, and the
//! dependency only runs the other way.
//!
//! Neither printer can implement [`std::fmt::Display`] on a compiler type
//! directly: the types are foreign to this crate and so is `Display`. Each
//! submodule therefore wraps a node before printing it, and it is the wrapper,
//! not the node, that implements [`Grouped`].

pub use ruddy::grammar::{Grouped, Prec, write_apply, write_arrow, write_project, write_struct};

pub mod ast;
pub mod ir;
