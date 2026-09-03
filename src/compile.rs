//! The checked core compilation pipeline.
//!
//! This is deliberately the only public seam that joins IR building, inference
//! and pattern checking.  Individual phases remain public for tooling, but a
//! caller cannot accidentally hand LIR facts from different runs.

use crate::{
    artifact, externs,
    inference::{self, Trace},
    ir, lir, parse, patterns,
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

/// One member of the dependency graph admitted by core compilation.
///
/// An alias makes this artifact directly source-visible. An absent alias keeps
/// it available solely to resolve the interfaces named by direct dependencies.
/// Both roles point at the same portable artifact collection, so source
/// interfaces and linked implementations cannot drift into separate inputs.
#[derive(Debug, Clone, Copy)]
pub struct Dependency<'a> {
    pub alias: Option<&'a str>,
    pub artifact: &'a artifact::UncheckedArtifact,
}

/// The coherent program that has passed every checking phase.
#[derive(Debug)]
pub struct AcceptedProgram {
    mint: Mint,
    ir: ir::Output,
    inference: inference::Output,
    patterns: patterns::Output,
    externs: externs::ExternPlan,
    artifact: artifact::Artifact,
}

impl AcceptedProgram {
    pub fn mint(&self) -> &Mint {
        &self.mint
    }
    pub fn ir(&self) -> &ir::Program {
        &self.ir.program
    }
    pub fn semantics(&self) -> &inference::Semantics {
        self.inference.semantics()
    }
    pub fn checks(&self) -> &patterns::Output {
        &self.patterns
    }
    pub fn externs(&self) -> &externs::ExternPlan {
        &self.externs
    }

    /// Lowering is infallible because this type is the proof of its precondition.
    pub fn lower(&self) -> lir::Output {
        lir::lower(self)
    }

    /// The validated target-neutral artifact produced by core compilation.
    pub fn artifact(&self) -> &artifact::Artifact {
        &self.artifact
    }

    /// Consume accepted state for phase-specific debugger/test inspection.
    /// It cannot be passed back to lowering or artifact construction.
    pub fn into_parts(self) -> (Mint, ir::Output, inference::Output, patterns::Output) {
        (self.mint, self.ir, self.inference, self.patterns)
    }
}

/// Compile a parsed source bundle with no dependency interfaces.
pub fn compile(
    mint: Mint,
    stmts: Vec<parse::Stmt>,
    trace: Trace,
) -> Result<AcceptedProgram, PartialCompilation> {
    compile_with(mint, stmts, trace, Vec::new(), |mint, stmts| {
        ir::build(mint, stmts)
    })
}

/// Compile a parsed source bundle against one coherent dependency graph.
///
/// Every direct dependency is represented by a [`Dependency`] with an alias;
/// transitive dependencies are represented by entries without one. This keeps
/// the source-visible interface and the linked implementation set together at
/// the public compilation seam.
pub fn compile_with_dependencies(
    mint: Mint,
    stmts: Vec<parse::Stmt>,
    dependencies: &[Dependency<'_>],
    trace: Trace,
) -> Result<AcceptedProgram, PartialCompilation> {
    let recovered: Vec<_> = dependencies
        .iter()
        .map(|dependency| dependency.artifact.clone().recover())
        .collect();
    let facts: Vec<_> = recovered
        .iter()
        .flat_map(|(_, facts)| facts.iter().cloned())
        .collect();
    let imports: Vec<_> = dependencies
        .iter()
        .zip(&recovered)
        .filter_map(|(dependency, (artifact, _))| {
            dependency
                .alias
                .map(|alias| ir::DependencyImport { alias, artifact })
        })
        .collect();
    let artifact_dependencies = recovered
        .iter()
        .zip(dependencies)
        .filter(|(_, dependency)| dependency.alias.is_some())
        .map(|((artifact, _), _)| artifact::Dependency {
            name: artifact.header().identity.name.clone(),
            version: artifact.header().identity.version.clone(),
        })
        .collect();
    let linked: Vec<_> = recovered
        .iter()
        .map(|(artifact, _)| artifact.clone())
        .collect();
    let mut result = compile_with(mint, stmts, trace, artifact_dependencies, |mint, stmts| {
        ir::build_with_dependency_imports(mint, stmts, &imports, &linked)
    });
    match &mut result {
        Ok(accepted) => accepted.inference.publish_recovery_facts(facts),
        Err(partial) => partial.inference.publish_recovery_facts(facts),
    }
    result
}

fn compile_with(
    mut mint: Mint,
    stmts: Vec<parse::Stmt>,
    trace: Trace,
    artifact_dependencies: Vec<artifact::Dependency>,
    build: impl FnOnce(&mut Mint, Vec<parse::Stmt>) -> ir::Output,
) -> Result<AcceptedProgram, PartialCompilation> {
    let ir = build(&mut mint, stmts);
    let mut program = ir.program.clone();
    let inference = inference::infer(&mint, &mut program, trace);
    // Inference writes solved types into the program, so publish that coherent
    // program rather than the pre-inference IR output.
    let ir = ir::Output {
        program,
        errors: ir.errors,
    };
    let patterns = patterns::check(&ir.program, &inference);
    let mut errors = Vec::new();
    errors.extend(ir.errors.iter().cloned().map(Error::Ir));
    errors.extend(inference.errors().iter().cloned().map(Error::Inference));
    errors.extend(patterns.errors.iter().cloned().map(Error::Patterns));
    if !errors.is_empty() {
        return Err(PartialCompilation {
            mint,
            ir,
            inference,
            patterns,
            errors,
        });
    }
    let externs = externs::plan(inference.semantics());
    // Construct the accepted proof before producing the artifact: lowering can
    // only consume that proof. The artifact is then retained in the same
    // coherent result, so the public compile seam runs all the way to the
    // validated target-neutral persistence boundary.
    let mut accepted = AcceptedProgram {
        mint,
        ir,
        inference,
        patterns,
        externs,
        artifact: artifact::empty(),
    };
    accepted.artifact = artifact::build_with_dependencies(&accepted, artifact_dependencies);
    Ok(accepted)
}
