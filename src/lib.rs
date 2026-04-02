mod check;
mod engine;
mod lower;
mod parser;
mod reporting;
mod resolver;
pub mod ty;
mod wasm;

pub use check::{
    CheckedSource, check_diagnostics, check_diagnostics_fs, check_lowered, check_text,
    check_text_fs,
};
pub use engine::{Eng, Source};
pub use lower::{lower_diagnostics, lower_diagnostics_fs, lower_source, lower_text, lower_text_fs};
pub use parser::{
    lex_diagnostics, lex_source, lex_text, parse_diagnostics, parse_source, parse_text,
};
pub use reporting::{Diagnostic, DiagnosticSeverity, TextRange, TextSize};
pub use resolver::{
    FailingResolver, FilesystemResolver, Resolver, ResolverDispatch, ResolverToken,
};
pub use ty::typed_ir;
