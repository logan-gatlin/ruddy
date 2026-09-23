//! Bound portable type expansion before publishing editor or build interfaces.
//!
//! Most semantic nodes retain DAG sharing in artifacts. Package owners are
//! occurrence scoped, so their paths must stay distinct. Reject an excessive
//! expansion while a source anchor is available; recovery interfaces publish
//! an explicit unknown instead of attempting the rejected expansion again.

use super::*;

pub(super) fn check(program: &ir::Program, output: &mut Output) {
    let mut failures = Vec::new();
    let semantics = Arc::make_mut(&mut output.semantics);
    for (symbol, declaration) in &program.terms {
        if let Some(scheme) = semantics.schemes.get_mut(symbol) {
            check_scheme(declaration.name_at, scheme, &mut failures);
        }
    }
    for (symbol, declaration) in &program.types {
        if let Some(scheme) = semantics.aliases.get_mut(symbol) {
            check_scheme(declaration.name_at, scheme, &mut failures);
        }
    }
    for (symbol, declaration) in &program.externs {
        if let Some(scheme) = semantics.externs.get_mut(symbol) {
            check_scheme(declaration.name_at, scheme, &mut failures);
        }
    }
    for ((symbol, _), (from, to)) in &mut semantics.operations {
        if let Some(declaration) = program.effects.get(symbol) {
            check_type(declaration.name_at, from, &mut failures);
            check_type(declaration.name_at, to, &mut failures);
        }
    }
    for (symbol, alias) in &mut semantics.effect_aliases {
        if let Some(declaration) = program.effects.get(symbol) {
            for (_, arguments) in &mut alias.cases {
                for argument in arguments {
                    check_type(declaration.name_at, argument, &mut failures);
                }
            }
        }
    }
    let diagnostics = Arc::make_mut(&mut output.diagnostics);
    for (at, failure) in failures {
        diagnostics.errors.push(Error {
            id: ErrorId {
                scope: Symbol::GENERATED,
                index: diagnostics.errors.len() as u32,
            },
            cause: ErrorCause::Direct,
            at,
            kind: ErrorKind::StructuralContract {
                message: failure.to_string(),
            },
            explanation: None,
        });
    }
}

fn check_scheme(
    at: Anchor,
    scheme: &mut Scheme,
    failures: &mut Vec<(Anchor, crate::artifact::ExportError)>,
) {
    if let Err(failure) = crate::artifact::check_type_export(scheme.body()) {
        failures.push((at, failure));
        *scheme = Scheme::new(scheme.count(), Arc::new(Ty::Undecided));
    }
}

fn check_type(
    at: Anchor,
    ty: &mut Arc<Ty>,
    failures: &mut Vec<(Anchor, crate::artifact::ExportError)>,
) {
    if let Err(failure) = crate::artifact::check_type_export(ty) {
        failures.push((at, failure));
        *ty = Arc::new(Ty::Undecided);
    }
}
