//! The checked core compilation pipeline.
//!
//! This is deliberately the only public seam that joins IR building, inference
//! and pattern checking.  Individual phases remain public for tooling, but a
//! caller cannot accidentally hand LIR facts from different runs.

use crate::{
    artifact, externs, inference::{self, Trace}, ir, lir, parse, patterns,
    symbol::Mint,
};

/// Every checking error produced by a completed compiler phase.
#[derive(Debug)]
pub enum Error {
    Ir(ir::Error),
    Inference(inference::Error),
    Patterns(patterns::Error),
}

/// A compilation which has not established the all-phases-accepted invariant.
/// Completed phases stay available for diagnostics and tooling.
#[derive(Debug)]
pub struct PartialCompilation {
    pub mint: Mint,
    pub ir: ir::Output,
    pub inference: inference::Output,
    pub patterns: patterns::Output,
    pub errors: Vec<Error>,
}

/// The coherent program that has passed every checking phase.
#[derive(Debug)]
pub struct AcceptedProgram {
    mint: Mint,
    ir: ir::Output,
    inference: inference::Output,
    patterns: patterns::Output,
    externs: externs::ExternPlan,
}

impl AcceptedProgram {
    pub fn mint(&self) -> &Mint { &self.mint }
    pub fn ir(&self) -> &ir::Program { &self.ir.program }
    pub fn semantics(&self) -> &inference::Semantics { self.inference.semantics() }
    pub fn checks(&self) -> &patterns::Output { &self.patterns }
    pub fn externs(&self) -> &externs::ExternPlan { &self.externs }

    /// Lowering is infallible because this type is the proof of its precondition.
    pub fn lower(&self) -> lir::Output { lir::lower(self) }

    /// Produce the validated target-neutral artifact for this accepted program.
    pub fn artifact(&self) -> artifact::Artifact {
        let lir = self.lower();
        artifact::build(self, &lir)
    }

    /// Consume accepted state for phase-specific debugger/test inspection.
    /// It cannot be passed back to lowering or artifact construction.
    pub fn into_parts(self) -> (Mint, ir::Output, inference::Output, patterns::Output) {
        (self.mint, self.ir, self.inference, self.patterns)
    }
}

/// Compile a parsed source bundle with no dependency interfaces.
pub fn compile(mint: Mint, stmts: Vec<parse::Stmt>, trace: Trace) -> Result<AcceptedProgram, PartialCompilation> {
    compile_with(mint, stmts, trace, |mint, stmts| ir::build(mint, stmts))
}

/// Compile a parsed source bundle against dependency interfaces.
pub fn compile_with_dependency_imports(
    mint: Mint,
    stmts: Vec<parse::Stmt>,
    dependencies: &[ir::DependencyImport<'_>],
    linked: &[artifact::Artifact],
    trace: Trace,
) -> Result<AcceptedProgram, PartialCompilation> {
    compile_with(mint, stmts, trace, |mint, stmts| {
        ir::build_with_dependency_imports(mint, stmts, dependencies, linked)
    })
}

fn compile_with(
    mut mint: Mint,
    stmts: Vec<parse::Stmt>,
    trace: Trace,
    build: impl FnOnce(&mut Mint, Vec<parse::Stmt>) -> ir::Output,
) -> Result<AcceptedProgram, PartialCompilation> {
    let ir = build(&mut mint, stmts);
    let mut program = ir.program.clone();
    let inference = inference::infer(&mint, &mut program, trace);
    // Inference writes solved types into the program, so publish that coherent
    // program rather than the pre-inference IR output.
    let ir = ir::Output { program, errors: ir.errors };
    let patterns = patterns::check(&ir.program, &inference);
    let mut errors = Vec::new();
    errors.extend(ir.errors.iter().cloned().map(Error::Ir));
    errors.extend(inference.errors().iter().cloned().map(Error::Inference));
    errors.extend(patterns.errors.iter().cloned().map(Error::Patterns));
    if !errors.is_empty() {
        return Err(PartialCompilation { mint, ir, inference, patterns, errors });
    }
    let externs = externs::plan(&ir.program, inference.semantics());
    Ok(AcceptedProgram { mint, ir, inference, patterns, externs })
}
