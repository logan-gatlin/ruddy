//! The checked core compilation pipeline.
//!
//! This is deliberately the only public seam that joins IR building, inference
//! and pattern checking.  Individual phases remain public for tooling, but a
//! caller cannot accidentally hand LIR facts from different runs.

use std::borrow::Cow;

use crate::{
    artifact, externs,
    inference::{self, Trace},
    ir, lir, parse, patterns,
    symbol::Mint,
    tracking::SourceMap,
};

/// Every checking error produced by a completed compiler phase.
///
/// An inference error is far larger than the others, and boxing it would
/// touch every reader of one for a value that is only ever built on failure.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Error {
    Ir(ir::Error),
    Inference(inference::Error),
    Patterns(patterns::Error),
}

/// A compilation which has not established the all-phases-accepted invariant.
/// Completed phases stay available for diagnostics and tooling.
///
/// Returned whole in the `Err` of the compile entry points: it is built once
/// per failed compile, so its size is no cost worth a box.
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
/// Both roles point at the same artifact collection, so source interfaces and
/// linked implementations cannot drift into separate inputs.
#[derive(Debug, Clone, Copy)]
pub struct Dependency<'a> {
    pub alias: Option<&'a str>,
    pub artifact: DependencyArtifact<'a>,
}

/// How a dependency arrives at the seam: as an artifact that has already been
/// validated, admitted as it is, or as portable data, admitted through
/// recovery with any repairs published as diagnostics.
///
/// A driver that compiled or parsed a dependency itself holds the validated
/// form, and handing it over unchecked would only have it copied three times
/// and decoded once more for a proof it already has. The unchecked form is
/// for data whose provenance is not this compiler's own output.
#[derive(Debug, Clone, Copy)]
pub enum DependencyArtifact<'a> {
    Checked(&'a artifact::Artifact),
    Unchecked(&'a artifact::UncheckedArtifact),
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
    /// Where every anchor in the program was written.
    pub fn source(&self) -> &SourceMap {
        &self.ir.source
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
#[allow(clippy::result_large_err)]
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
#[allow(clippy::result_large_err)]
pub fn compile_with_dependencies(
    mint: Mint,
    stmts: Vec<parse::Stmt>,
    dependencies: &[Dependency<'_>],
    trace: Trace,
) -> Result<AcceptedProgram, PartialCompilation> {
    let recovered: Vec<(Cow<'_, artifact::Artifact>, Vec<artifact::RecoveryFact>)> = dependencies
        .iter()
        .map(|dependency| match dependency.artifact {
            DependencyArtifact::Checked(artifact) => (Cow::Borrowed(artifact), Vec::new()),
            DependencyArtifact::Unchecked(artifact) => {
                let (artifact, facts) = artifact.clone().recover();
                (Cow::Owned(artifact), facts)
            }
        })
        .collect();
    let facts: Vec<_> = recovered
        .iter()
        .flat_map(|(_, facts)| facts.iter().cloned())
        .collect();
    let imports: Vec<_> = dependencies
        .iter()
        .zip(&recovered)
        .filter_map(|(dependency, (artifact, _))| {
            dependency.alias.map(|alias| ir::DependencyImport {
                alias,
                artifact: artifact.as_ref(),
            })
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
    let linked: Vec<&artifact::Artifact> = recovered
        .iter()
        .map(|(artifact, _)| artifact.as_ref())
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

#[allow(clippy::result_large_err)]
fn compile_with(
    mut mint: Mint,
    stmts: Vec<parse::Stmt>,
    trace: Trace,
    artifact_dependencies: Vec<artifact::Dependency>,
    build: impl FnOnce(&mut Mint, Vec<parse::Stmt>) -> ir::Output,
) -> Result<AcceptedProgram, PartialCompilation> {
    let mut ir = build(&mut mint, stmts);
    let inference = inference::infer(&mint, &ir.program, trace);
    // Inference reads the program and answers with typed copies of its
    // declarations; the program every later phase reads is the one with
    // those written in.
    inference.apply_types(&mut ir.program);
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
