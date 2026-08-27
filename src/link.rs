//! Static linking of portable artifacts.
//!
//! Compilation deliberately produces one artifact per project. This final phase
//! consumes those artifacts in dependency-first order and makes the root
//! artifact self-contained. It performs no optimization: every function and
//! global is copied, in graph order, and only function-table indices change.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};

use crate::{
    artifact::{self, Artifact, Callee, Op},
    symbol::{Bundle, Version},
};

/// A malformed or conflicting input to static linking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    EmptyGraph,
    MalformedIdentity {
        identity: String,
    },
    DuplicateIdentity {
        identity: String,
    },
    DuplicateDependency {
        owner: String,
        dependency: String,
    },
    MissingDependency {
        owner: String,
        dependency: String,
    },
    DependencyAfterOwner {
        owner: String,
        dependency: String,
    },
    UnreachableArtifact {
        identity: String,
        root: String,
    },
    MalformedGlobal {
        name: String,
    },
    WrongGlobalOwner {
        name: String,
        owner: String,
    },
    DuplicateGlobal {
        name: String,
    },
    MalformedDeclaration {
        namespace: DeclarationNamespace,
        name: String,
    },
    WrongDeclarationOwner {
        namespace: DeclarationNamespace,
        name: String,
        owner: String,
    },
    DuplicateDeclaration {
        namespace: DeclarationNamespace,
        name: String,
    },
    MissingGlobal {
        owner: String,
        target: String,
    },
    UndeclaredGlobalDependency {
        owner: String,
        target: String,
    },
    FunctionIndexOutOfBounds {
        owner: String,
        index: u64,
        functions: usize,
    },
}

/// One public interface namespace validated by the linker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationNamespace {
    Value,
    Type,
    Effect,
}

impl fmt::Display for DeclarationNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Value => "value",
            Self::Type => "type",
            Self::Effect => "effect",
        })
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGraph => f.write_str("cannot link an empty artifact graph"),
            Self::MalformedIdentity { identity } => {
                write!(f, "artifact graph contains malformed identity `{identity}`")
            }
            Self::DuplicateIdentity { identity } => {
                write!(f, "artifact graph contains duplicate identity `{identity}`")
            }
            Self::DuplicateDependency { owner, dependency } => write!(
                f,
                "artifact `{owner}` declares dependency `{dependency}` more than once"
            ),
            Self::MissingDependency { owner, dependency } => write!(
                f,
                "artifact `{owner}` depends on missing artifact `{dependency}`"
            ),
            Self::DependencyAfterOwner { owner, dependency } => write!(
                f,
                "artifact `{owner}` appears before its dependency `{dependency}`"
            ),
            Self::UnreachableArtifact { identity, root } => write!(
                f,
                "artifact `{identity}` is not reachable from root artifact `{root}`"
            ),
            Self::MalformedGlobal { name } => write!(f, "malformed qualified global name `{name}`"),
            Self::WrongGlobalOwner { name, owner } => write!(
                f,
                "global `{name}` is stored in artifact `{owner}` but has a different owner"
            ),
            Self::DuplicateGlobal { name } => {
                write!(f, "artifact graph defines global `{name}` more than once")
            }
            Self::MalformedDeclaration { namespace, name } => {
                write!(f, "malformed qualified public {namespace} name `{name}`")
            }
            Self::WrongDeclarationOwner {
                namespace,
                name,
                owner,
            } => write!(
                f,
                "public {namespace} `{name}` is stored in artifact `{owner}` but has a different owner"
            ),
            Self::DuplicateDeclaration { namespace, name } => write!(
                f,
                "artifact defines public {namespace} `{name}` more than once"
            ),
            Self::MissingGlobal { owner, target } => {
                write!(f, "artifact `{owner}` references missing global `{target}`")
            }
            Self::UndeclaredGlobalDependency { owner, target } => write!(
                f,
                "artifact `{owner}` references global `{target}` without declaring its owner as a direct dependency"
            ),
            Self::FunctionIndexOutOfBounds {
                owner,
                index,
                functions,
            } => write!(
                f,
                "artifact `{owner}` references function {index}, but contains {functions} functions"
            ),
        }
    }
}

impl Error for LinkError {}

/// Link a dependency-first artifact graph into one self-contained root artifact.
///
/// The root is the final input. Its identity and public interface are retained;
/// its dependency list is cleared because all dependency code is copied into
/// the returned LIR.
pub fn link(artifacts: &[Artifact]) -> Result<Artifact, LinkError> {
    let Some(root) = artifacts.last() else {
        return Err(LinkError::EmptyGraph);
    };

    let mut positions = HashMap::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        let identity = identity(artifact);
        if !valid_identity(
            &artifact.header.identity.name,
            &artifact.header.identity.version,
        ) {
            return Err(LinkError::MalformedIdentity { identity });
        }
        if positions.insert(identity.clone(), index).is_some() {
            return Err(LinkError::DuplicateIdentity { identity });
        }
    }
    for (index, artifact) in artifacts.iter().enumerate() {
        let owner = identity(artifact);
        let mut dependencies = HashSet::new();
        for specification in &artifact.header.dependencies {
            let dependency = format!("{}@{}", specification.name, specification.version);
            if !valid_identity(&specification.name, &specification.version) {
                return Err(LinkError::MalformedIdentity {
                    identity: dependency,
                });
            }
            if !dependencies.insert(dependency.clone()) {
                return Err(LinkError::DuplicateDependency { owner, dependency });
            }
            match positions.get(&dependency) {
                None => return Err(LinkError::MissingDependency { owner, dependency }),
                Some(position) if *position >= index => {
                    return Err(LinkError::DependencyAfterOwner { owner, dependency });
                }
                Some(_) => {}
            }
        }
    }

    let root_identity = identity(root);
    let mut reachable = HashSet::from([root_identity.clone()]);
    let mut pending = vec![root_identity.clone()];
    while let Some(owner) = pending.pop() {
        let artifact = &artifacts[positions[&owner]];
        for dependency in &artifact.header.dependencies {
            let dependency = format!("{}@{}", dependency.name, dependency.version);
            if reachable.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    if let Some(artifact) = artifacts
        .iter()
        .find(|artifact| !reachable.contains(&identity(artifact)))
    {
        return Err(LinkError::UnreachableArtifact {
            identity: identity(artifact),
            root: root_identity,
        });
    }

    let mut globals = HashSet::new();
    for artifact in artifacts {
        let owner = identity(artifact);
        validate_declarations(artifact, &owner)?;
        for global in &artifact.lir.globals {
            let parsed_owner = qualified_owner(&global.name)?;
            if parsed_owner != owner {
                return Err(LinkError::WrongGlobalOwner {
                    name: global.name.clone(),
                    owner,
                });
            }
            if !globals.insert(global.name.clone()) {
                return Err(LinkError::DuplicateGlobal {
                    name: global.name.clone(),
                });
            }
        }
    }

    let mut functions = Vec::new();
    let mut linked_globals = Vec::new();
    for artifact in artifacts {
        let owner = identity(artifact);
        let dependencies: HashSet<String> = artifact
            .header
            .dependencies
            .iter()
            .map(|dependency| format!("{}@{}", dependency.name, dependency.version))
            .collect();
        // Artifact indices are u64 and Rust vectors cannot exceed u64::MAX
        // entries on supported targets.
        let offset = functions.len() as u64;
        let count = artifact.lir.functions.len();
        for function in &artifact.lir.functions {
            let mut function = function.clone();
            relocate_block(
                &mut function.body,
                offset,
                count,
                &owner,
                &dependencies,
                &globals,
            )?;
            functions.push(function);
        }
        for global in &artifact.lir.globals {
            let mut global = global.clone();
            relocate_block(
                &mut global.body,
                offset,
                count,
                &owner,
                &dependencies,
                &globals,
            )?;
            linked_globals.push(global);
        }
    }

    let mut header = root.header.clone();
    header.dependencies.clear();
    Ok(Artifact {
        header,
        lir: artifact::Lir {
            functions,
            globals: linked_globals,
        },
    })
}

fn valid_identity(name: &str, version: &str) -> bool {
    Version::parse(version)
        .ok()
        .filter(|parsed| parsed.build.is_empty() && parsed.to_string() == version)
        .and_then(|version| Bundle::new(name, version))
        .is_some()
}

fn identity(artifact: &Artifact) -> String {
    format!(
        "{}@{}",
        artifact.header.identity.name, artifact.header.identity.version
    )
}

fn qualified_owner(name: &str) -> Result<&str, LinkError> {
    parse_qualified_owner(name).ok_or_else(|| LinkError::MalformedGlobal {
        name: name.to_string(),
    })
}

fn parse_qualified_owner(name: &str) -> Option<&str> {
    let (owner, path) = name.split_once("::")?;
    let (bundle, version) = owner.split_once('@')?;
    (valid_identity(bundle, version) && path.split("::").all(canonical_component)).then_some(owner)
}

fn canonical_component(name: &str) -> bool {
    source_identifier(name) || name == "_" || name.strip_prefix('%').is_some_and(source_identifier)
}

fn source_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !matches!(
            name,
            "_" | "let"
                | "in"
                | "type"
                | "end"
                | "with"
                | "match"
                | "fn"
                | "effect"
                | "handle"
                | "raise"
                | "and"
                | "or"
                | "xor"
                | "not"
                | "module"
                | "true"
                | "false"
        )
}

fn validate_declarations(artifact: &Artifact, owner: &str) -> Result<(), LinkError> {
    validate_declaration_names(
        artifact
            .header
            .values
            .iter()
            .map(|declaration| declaration.name.as_str()),
        DeclarationNamespace::Value,
        owner,
    )?;
    validate_declaration_names(
        artifact
            .header
            .types
            .iter()
            .map(|declaration| declaration.name.as_str()),
        DeclarationNamespace::Type,
        owner,
    )?;
    validate_declaration_names(
        artifact
            .header
            .effects
            .iter()
            .map(|declaration| declaration.name.as_str()),
        DeclarationNamespace::Effect,
        owner,
    )
}

fn validate_declaration_names<'a>(
    names: impl Iterator<Item = &'a str>,
    namespace: DeclarationNamespace,
    owner: &str,
) -> Result<(), LinkError> {
    let mut seen = HashSet::new();
    for name in names {
        let Some(parsed_owner) = parse_qualified_owner(name) else {
            return Err(LinkError::MalformedDeclaration {
                namespace,
                name: name.to_string(),
            });
        };
        if parsed_owner != owner {
            return Err(LinkError::WrongDeclarationOwner {
                namespace,
                name: name.to_string(),
                owner: owner.to_string(),
            });
        }
        if !seen.insert(name) {
            return Err(LinkError::DuplicateDeclaration {
                namespace,
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn relocate_block(
    block: &mut artifact::Block,
    offset: u64,
    count: usize,
    owner: &str,
    dependencies: &HashSet<String>,
    globals: &HashSet<String>,
) -> Result<(), LinkError> {
    for instruction in &mut block.instrs {
        match &mut instruction.op {
            Op::Closure { func, .. } => relocate_index(func, offset, count, owner)?,
            Op::Call {
                callee: Callee::Direct(func),
                ..
            } => relocate_index(func, offset, count, owner)?,
            Op::Global { target } => {
                let target_owner = qualified_owner(target)?;
                if target_owner != owner && !dependencies.contains(target_owner) {
                    return Err(LinkError::UndeclaredGlobalDependency {
                        owner: owner.to_string(),
                        target: target.clone(),
                    });
                }
                if !globals.contains(target) {
                    return Err(LinkError::MissingGlobal {
                        owner: owner.to_string(),
                        target: target.clone(),
                    });
                }
            }
            Op::Catch { body, .. } => {
                relocate_block(body, offset, count, owner, dependencies, globals)?
            }
            Op::SwitchTag {
                cases, fallback, ..
            } => {
                for case in cases {
                    relocate_block(&mut case.block, offset, count, owner, dependencies, globals)?;
                }
                if let Some(block) = fallback {
                    relocate_block(block, offset, count, owner, dependencies, globals)?;
                }
            }
            Op::SwitchPrim {
                cases, fallback, ..
            } => {
                for case in cases {
                    relocate_block(&mut case.block, offset, count, owner, dependencies, globals)?;
                }
                if let Some(block) = fallback {
                    relocate_block(block, offset, count, owner, dependencies, globals)?;
                }
            }
            Op::SwitchPresence {
                present, absent, ..
            } => {
                relocate_block(present, offset, count, owner, dependencies, globals)?;
                relocate_block(absent, offset, count, owner, dependencies, globals)?;
            }
            Op::SwitchRest { none, some, .. } => {
                relocate_block(none, offset, count, owner, dependencies, globals)?;
                relocate_block(some, offset, count, owner, dependencies, globals)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn relocate_index(
    index: &mut u64,
    offset: u64,
    count: usize,
    owner: &str,
) -> Result<(), LinkError> {
    if *index >= count as u64 {
        return Err(LinkError::FunctionIndexOutOfBounds {
            owner: owner.to_string(),
            index: *index,
            functions: count,
        });
    }
    // The relocated index is below the length of the combined vectors, so the
    // same representability argument as the offset applies.
    *index += offset;
    Ok(())
}
