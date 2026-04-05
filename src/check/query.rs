//! Public query-style entry points for type checking.

use crate::engine::Source;
use crate::lower::{ir as lir, lower_text};
use crate::reporting::Diagnostic;
use crate::resolver::{FilesystemResolver, Resolver};
use crate::ty::store::TypeStore;
use crate::ty::typed_ir;

use super::infer::check_lowered_source;

/// Result of type checking a lowered or parsed source.
#[derive(Debug)]
pub struct CheckedSource {
    /// Type-annotated IR after zonking solved meta-variables.
    pub source: typed_ir::Source,
    /// Type diagnostics produced during checking.
    pub diagnostics: Vec<Diagnostic>,
    /// Canonical type arena used by `source` type ids.
    pub type_store: TypeStore,
}

/// Type check an already-lowered source graph.
pub fn check_lowered(_db: &dyn salsa::Database, lowered: lir::LoweredSource) -> CheckedSource {
    let result = check_lowered_source(&lowered);
    CheckedSource {
        source: result.source,
        diagnostics: result.diagnostics,
        type_store: result.type_store,
    }
}

/// Lower with resolver `R`, then type check the result.
pub fn check_text<R: Resolver>(db: &dyn salsa::Database, source: Source) -> CheckedSource {
    let lowered = lower_text::<R>(db, source);
    check_lowered(db, lowered)
}

/// Convenience wrapper over [`check_text`] using filesystem resolution.
pub fn check_text_fs(db: &dyn salsa::Database, source: Source) -> CheckedSource {
    check_text::<FilesystemResolver>(db, source)
}

/// Lower with resolver `R`, then return only checker diagnostics.
pub fn check_diagnostics<R: Resolver>(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    check_text::<R>(db, source).diagnostics
}
