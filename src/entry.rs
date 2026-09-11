//! Program entry validation through ordinary calls and effect inference.
//!
//! A generated probe specializes `main` at unit without duplicating the type
//! solver or assuming the representation of an exported polymorphic function.

use std::{error, fmt};

use crate::{
    artifact::{Artifact, Kind, Rest, Type},
    compile::{self, Dependency, DependencyArtifact},
    inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    MissingMain,
    InvalidMain(Vec<String>),
    OpenEffects,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMain => f.write_str("an executable must define public root-module `main`"),
            Self::InvalidMain(messages) => {
                write!(f, "invalid program entry point: {}", messages.join("; "))
            }
            Self::OpenEffects => {
                f.write_str("the invocation of `main ()` must have a closed effect row")
            }
        }
    }
}

impl error::Error for Error {}

/// Validate the target-neutral executable contract and record its bundle kind.
/// Dependencies supply any external type interfaces named by `main`.
pub fn executable(artifact: &Artifact, dependencies: &[&Artifact]) -> Result<Artifact, Error> {
    let probe = wrapper(
        artifact,
        dependencies,
        "let invoke : () -> () + .. = fn _ => program::main ()",
    )?;
    let Type::Arrow(_, _, effects) = &probe.header().values[0].scheme.body else {
        unreachable!("the checked probe has an explicit function annotation")
    };
    let mut row = effects;
    loop {
        match &row.rest {
            Rest::Closed => break,
            Rest::More(more) => row = more,
            _ => return Err(Error::OpenEffects),
        }
    }
    let mut executable = artifact.clone();
    executable.header.kind = Kind::Executable;
    Ok(executable)
}

/// Compile a private launch adapter against an artifact's public interface.
/// It is never a source-level dependency on an executable: only the compiler
/// uses this temporary library view to check and lower the root invocation.
pub(crate) fn wrapper(
    artifact: &Artifact,
    dependencies: &[&Artifact],
    source: &str,
) -> Result<Artifact, Error> {
    let main = format!(
        "{}@{}::main",
        artifact.header().identity.name,
        artifact.header().identity.version
    );
    if !artifact
        .header()
        .values
        .iter()
        .any(|value| value.name == main)
    {
        return Err(Error::MissingMain);
    }
    match compile_adapter(artifact, dependencies, source).map_err(Error::InvalidMain) {
        Ok(artifact) => Ok(artifact),
        Err(error) => {
            // Ruddy's empty sum is eliminated by an exhaustive empty match,
            // rather than unified with unit. This keeps entry compatibility
            // inside the ordinary type and pattern checkers even when main
            // never returns.
            let never = source.replace("program::main ()", "(match program::main () with end)");
            compile_adapter(artifact, dependencies, &never).map_err(|_| error)
        }
    }
}

/// Compile a generated host adapter without requiring a `main` export.
pub(crate) fn compile_adapter(
    artifact: &Artifact,
    dependencies: &[&Artifact],
    source: &str,
) -> Result<Artifact, Vec<String>> {
    let mut interface = artifact.clone();
    interface.header.kind = Kind::Library;
    // A linked artifact retains declarations from its implementation bundles.
    // Reconstitute their semantic import views: they are available to inference
    // but do not introduce source aliases or additional runtime dependencies.
    let mut retained = std::collections::BTreeMap::new();
    for name in interface
        .header
        .types
        .iter()
        .map(|ty| &ty.name)
        .chain(interface.header.effects.iter().map(|effect| &effect.name))
    {
        let Some((owner, _)) = name.split_once("::") else {
            continue;
        };
        let Some((name, version)) = owner.rsplit_once('@') else {
            continue;
        };
        if std::iter::once(artifact)
            .chain(dependencies.iter().copied())
            .any(|a| a.header().identity.name == name && a.header().identity.version == version)
        {
            continue;
        }
        retained.entry(owner.to_string()).or_insert_with(|| {
            let mut view = crate::artifact::empty();
            view.header.identity.name = name.into();
            view.header.identity.version = version.into();
            view.header.kind = Kind::Library;
            view
        });
    }
    for (owner, view) in &mut retained {
        let prefix = format!("{owner}::");
        view.header.types = interface
            .header
            .types
            .iter()
            .filter(|ty| ty.name.starts_with(&prefix))
            .cloned()
            .collect();
        view.header.effects = interface
            .header
            .effects
            .iter()
            .filter(|effect| effect.name.starts_with(&prefix))
            .cloned()
            .collect();
    }
    let mut imports: Vec<_> = dependencies
        .iter()
        .filter(|dependency| dependency.header().identity != artifact.header().identity)
        .map(|artifact| Dependency {
            alias: None,
            artifact: DependencyArtifact::Checked(artifact),
        })
        .collect();
    imports.extend(retained.values().map(|artifact| Dependency {
        alias: None,
        artifact: DependencyArtifact::Checked(artifact),
    }));
    imports.push(Dependency {
        alias: Some("program"),
        artifact: DependencyArtifact::Checked(&interface),
    });
    // Choose an identity outside the input graph, including bundled globals
    // of an already linked artifact.
    let mut name = String::from("ruddy-entry");
    while std::iter::once(artifact)
        .chain(dependencies.iter().copied())
        .any(|a| {
            a.header().identity.name == name
                || a.lir()
                    .globals
                    .iter()
                    .any(|g| g.name.starts_with(&format!("{name}@")))
                || a.lir()
                    .externs
                    .iter()
                    .any(|external| external.name.starts_with(&format!("{name}@")))
        })
    {
        name.push_str("-entry");
    }
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    if !parsed.errors.is_empty() {
        return Err(parsed
            .errors
            .iter()
            .map(|error| error.diagnostic().title)
            .collect());
    }
    // The adapter is linked into the artifact it adapts, so it binds the same
    // integer domains: one compiled for the default would be refused by the
    // linker under a root that binds another.
    compile::compile_bound(
        Mint::new(Bundle::new(&name, Version::new(0, 0, 0)).expect("valid internal bundle name")),
        parsed.stmts,
        &imports,
        inference::Trace::Off,
        artifact.header().domains,
    )
    .map(|accepted| accepted.artifact().clone())
    .map_err(|partial| {
        let source = &partial.ir.source;
        partial
            .errors
            .iter()
            .map(|error| match error {
                compile::Error::Ir(error) => error.diagnostic(source).title,
                compile::Error::Inference(error) => error.diagnostic(source).title,
                compile::Error::Patterns(error) => error.kind.to_string(),
            })
            .collect()
    })
}
