//! Rendering compiler structures for a reader.
//!
//! The compiler does not print its own trees. Lowering and parsing produce
//! structures; turning one into something a person reads is a debugging
//! concern, so all of it lives here — the [`ast`] tree as written, the [`ir`]
//! tree as lowered, and the [`lir`] instruction stream underneath both.
//!
//! The first two print the *same* surface grammar, and so does the compiler when
//! it prints a type into a diagnostic. That rule therefore lives one crate down,
//! in [`ruddy::ui`] — [`Prec`], one table per node kind ([`Grouped`]), and one
//! writer per position ([`write_apply`] and friends) — and is re-exported here
//! for the two printers that read it. It cannot live in this crate: the
//! compiler would have to depend on the debugger to reach it, and the
//! dependency only runs the other way. [`lir`] shares none of it: LIR has no
//! surface syntax to be printed back as, so its listing is a notation of its
//! own.
//!
//! No printer can implement [`std::fmt::Display`] on a compiler type directly:
//! the types are foreign to this crate and so is `Display`. The surface
//! printers therefore wrap a node before printing it, and it is the wrapper,
//! not the node, that implements [`Grouped`]; [`lir`] writes plain functions
//! instead, having no grouping rules to share.

pub use ruddy::{
    types::Shape,
    ui::{
        Entry, Grouped, Mark, Prec, label, write_applied, write_apply, write_arrow, write_effects,
        write_let, write_match, write_project, write_row, write_struct, write_sum, write_tag,
    },
};

pub mod ast;
pub mod ir;
pub mod lir;
