use crate::engine::Source;
use crate::lower::{ir as lir, lower_text};
use crate::reporting::Diagnostic;
use crate::resolver::{FilesystemResolver, Resolver};
use crate::ty::store::TypeStore;
use crate::ty::typed_ir;

use super::infer::check_lowered_source;

#[derive(Debug)]
pub struct CheckedSource {
    pub source: typed_ir::Source,
    pub diagnostics: Vec<Diagnostic>,
    pub type_store: TypeStore,
}

pub fn check_lowered(_db: &dyn salsa::Database, lowered: lir::LoweredSource) -> CheckedSource {
    let result = check_lowered_source(&lowered);
    CheckedSource {
        source: result.source,
        diagnostics: result.diagnostics,
        type_store: result.type_store,
    }
}

pub fn check_text<R: Resolver>(db: &dyn salsa::Database, source: Source) -> CheckedSource {
    let lowered = lower_text::<R>(db, source);
    check_lowered(db, lowered)
}

pub fn check_text_fs(db: &dyn salsa::Database, source: Source) -> CheckedSource {
    check_text::<FilesystemResolver>(db, source)
}

pub fn check_diagnostics<R: Resolver>(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    check_text::<R>(db, source).diagnostics
}

pub fn check_diagnostics_fs(db: &dyn salsa::Database, source: Source) -> Vec<Diagnostic> {
    check_diagnostics::<FilesystemResolver>(db, source)
}
