//! Filesystem-facing Ruddy compiler entry point used by the command-line tool.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use ruddy::{
    artifact::{Artifact, Dependency},
    bundle::{self, Disk, Files},
    inference, ir, lir, patterns,
    symbol::{Bundle, Mint, Version},
    tracking::{FileManager, Span},
};
use serde::Deserialize;

const MANIFEST: &str = "Ruddy.toml";

/// A user-facing failure while loading a manifest or compiling its bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    messages: Vec<String>,
}

impl CompileError {
    fn one(message: impl Into<String>) -> Self {
        Self {
            messages: vec![format!("error: {}", message.into())],
        }
    }

    fn diagnostics(messages: Vec<String>) -> Self {
        Self { messages }
    }

    /// The individual diagnostics in display order.
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, message) in self.messages.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            formatter.write_str(message)?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    root: PathBuf,
    dependencies: IndexMap<String, DependencySpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencySpec {
    version: String,
    source: PathBuf,
}

/// Compile the project in `directory`, whose `Ruddy.toml` names its bundle root.
///
/// The root and dependency source paths are resolved relative to the manifest.
/// Dependency artifacts are loaded and checked before compilation. Their
/// identities, but never their source paths, are recorded in the returned
/// artifact in manifest declaration order.
pub fn compile(directory: impl AsRef<Path>) -> Result<Artifact, CompileError> {
    let directory = directory.as_ref();
    let manifest = load_manifest(directory)?;
    let root = directory.join(&manifest.root);
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Some(name) = root.file_name().and_then(|name| name.to_str()) else {
        return Err(CompileError::one("manifest field `root` must name a file"));
    };

    let disk = Disk::new(parent);
    if disk.read(name).is_none() {
        return Err(CompileError::one(format!(
            "could not read bundle root {} configured by {}",
            root.display(),
            directory.join(MANIFEST).display()
        )));
    }

    let dependencies = load_dependencies(directory, manifest.dependencies)?;

    let mut files = FileManager::new();
    let loaded = bundle::load(&mut files, &disk, name);
    let mut mint = Mint::new(loaded.bundle.clone().unwrap_or_else(bundle::fallback));
    let mut built = ir::build(&mut mint, loaded.stmts);
    let inferred = inference::infer(&mint, &mut built.program);
    let checked = patterns::check(&built.program, &inferred);

    let errors = loaded
        .loaded
        .iter()
        .map(|file| file.lex_errors.len() + file.parse_errors.len())
        .sum::<usize>()
        + loaded.errors.len()
        + built.errors.len()
        + inferred.errors.len()
        + checked.errors.len();

    if errors != 0 {
        let mut diagnostics = Vec::with_capacity(errors);
        for file in &loaded.loaded {
            for error in &file.lex_errors {
                diagnostics.push(diagnostic(
                    &mut files,
                    "lex",
                    error.kind.code(),
                    error.span,
                    &error.kind,
                ));
            }
            for error in &file.parse_errors {
                diagnostics.push(diagnostic(
                    &mut files,
                    "parse",
                    error.code(),
                    error.span,
                    error,
                ));
            }
        }
        for error in &loaded.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "bundle",
                error.kind.code(),
                error.span,
                &error.kind,
            ));
        }
        for error in &built.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "ir",
                error.kind.code(),
                error.span,
                &error.kind,
            ));
        }
        for error in &inferred.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "types",
                error.kind.code(),
                error.span,
                &error.kind,
            ));
        }
        for error in &checked.errors {
            diagnostics.push(diagnostic(
                &mut files,
                "patterns",
                error.kind.code(),
                error.span,
                &error.kind,
            ));
        }
        return Err(CompileError::diagnostics(diagnostics));
    }

    let lowered = lir::lower(&mint, &built.program, &inferred);
    let mut artifact = Artifact::build(&mint, &built.program, &inferred, &lowered);
    artifact.header.dependencies = dependencies;
    Ok(artifact)
}

fn load_manifest(directory: &Path) -> Result<Manifest, CompileError> {
    let path = directory.join(MANIFEST);
    let source = fs::read_to_string(&path).map_err(|error| {
        CompileError::one(format!(
            "could not read manifest {}: {error}",
            path.display()
        ))
    })?;
    toml::from_str(&source).map_err(|error| {
        CompileError::one(format!(
            "could not parse manifest {}: {error}",
            path.display()
        ))
    })
}

fn load_dependencies(
    directory: &Path,
    dependencies: IndexMap<String, DependencySpec>,
) -> Result<Vec<Dependency>, CompileError> {
    dependencies
        .into_iter()
        .map(|(name, spec)| load_dependency(directory, name, spec))
        .collect()
}

fn load_dependency(
    directory: &Path,
    name: String,
    spec: DependencySpec,
) -> Result<Dependency, CompileError> {
    let version = Version::parse(&spec.version).map_err(|error| {
        CompileError::one(format!(
            "dependency `{name}` has invalid semantic version `{}`: {error}",
            spec.version
        ))
    })?;
    if !version.build.is_empty() {
        return Err(CompileError::one(format!(
            "dependency `{name}` version `{version}` uses unsupported build metadata"
        )));
    }
    if Bundle::new(&name, version.clone()).is_none() {
        return Err(CompileError::one(format!(
            "dependency key `{name}` is not a valid Ruddy bundle name"
        )));
    }

    let path = directory.join(&spec.source);
    let source = fs::read_to_string(&path).map_err(|error| {
        CompileError::one(format!(
            "could not read source for dependency `{name}` at {}: {error}",
            path.display()
        ))
    })?;
    let artifact = Artifact::try_parse(&source).map_err(|error| {
        CompileError::one(format!(
            "dependency `{name}` source {} is not a Ruddy artifact: {error}",
            path.display()
        ))
    })?;
    if artifact.print() != source {
        return Err(CompileError::one(format!(
            "dependency `{name}` source {} is not in canonical artifact form",
            path.display()
        )));
    }
    if artifact.header.identity.name != name {
        return Err(CompileError::one(format!(
            "dependency `{name}` source {} contains artifact `{}` instead",
            path.display(),
            artifact.header.identity.name
        )));
    }
    let requested = version.to_string();
    if artifact.header.identity.version != requested {
        return Err(CompileError::one(format!(
            "dependency `{name}` requests version `{requested}`, but source {} contains version `{}`",
            path.display(),
            artifact.header.identity.version
        )));
    }

    Ok(Dependency {
        name,
        version: requested,
    })
}

fn diagnostic(
    files: &mut FileManager,
    phase: &str,
    code: &str,
    span: Span,
    message: &impl fmt::Display,
) -> String {
    let file = files.get_file(span.file_id);
    let before = file.content.get(..span.start).unwrap_or(&file.content);
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    format!(
        "{}:{line}:{column}: error[{phase}/{code}]: {message}",
        file.path
    )
}
