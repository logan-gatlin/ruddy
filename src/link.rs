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
    MissingGlobal {
        owner: String,
        target: String,
    },
    FunctionIndexOutOfBounds {
        owner: String,
        index: u64,
        functions: usize,
    },
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
            Self::MalformedGlobal { name } => write!(f, "malformed qualified global name `{name}`"),
            Self::WrongGlobalOwner { name, owner } => write!(
                f,
                "global `{name}` is stored in artifact `{owner}` but has a different owner"
            ),
            Self::DuplicateGlobal { name } => {
                write!(f, "artifact graph defines global `{name}` more than once")
            }
            Self::MissingGlobal { owner, target } => {
                write!(f, "artifact `{owner}` references missing global `{target}`")
            }
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

    let mut globals = HashSet::new();
    for artifact in artifacts {
        let owner = identity(artifact);
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
        // Artifact indices are u64 and Rust vectors cannot exceed u64::MAX
        // entries on supported targets.
        let offset = functions.len() as u64;
        let count = artifact.lir.functions.len();
        for function in &artifact.lir.functions {
            let mut function = function.clone();
            relocate_block(&mut function.body, offset, count, &owner, &globals)?;
            functions.push(function);
        }
        for global in &artifact.lir.globals {
            let mut global = global.clone();
            relocate_block(&mut global.body, offset, count, &owner, &globals)?;
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
        .filter(|version| version.build.is_empty())
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
    let Some((owner, path)) = name.split_once("::") else {
        return Err(LinkError::MalformedGlobal {
            name: name.to_string(),
        });
    };
    let Some((bundle, version)) = owner.split_once('@') else {
        return Err(LinkError::MalformedGlobal {
            name: name.to_string(),
        });
    };
    if !valid_identity(bundle, version) || path.split("::").any(str::is_empty) {
        return Err(LinkError::MalformedGlobal {
            name: name.to_string(),
        });
    }
    Ok(owner)
}

fn relocate_block(
    block: &mut artifact::Block,
    offset: u64,
    count: usize,
    owner: &str,
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
                qualified_owner(target)?;
                if !globals.contains(target) {
                    return Err(LinkError::MissingGlobal {
                        owner: owner.to_string(),
                        target: target.clone(),
                    });
                }
            }
            Op::Catch { body, .. } => relocate_block(body, offset, count, owner, globals)?,
            Op::SwitchTag {
                cases, fallback, ..
            } => {
                for case in cases {
                    relocate_block(&mut case.block, offset, count, owner, globals)?;
                }
                if let Some(block) = fallback {
                    relocate_block(block, offset, count, owner, globals)?;
                }
            }
            Op::SwitchPrim {
                cases, fallback, ..
            } => {
                for case in cases {
                    relocate_block(&mut case.block, offset, count, owner, globals)?;
                }
                if let Some(block) = fallback {
                    relocate_block(block, offset, count, owner, globals)?;
                }
            }
            Op::SwitchPresence {
                present, absent, ..
            } => {
                relocate_block(present, offset, count, owner, globals)?;
                relocate_block(absent, offset, count, owner, globals)?;
            }
            Op::SwitchRest { none, some, .. } => {
                relocate_block(none, offset, count, owner, globals)?;
                relocate_block(some, offset, count, owner, globals)?;
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
