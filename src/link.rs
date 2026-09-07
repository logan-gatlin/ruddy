//! Static linking of portable artifacts.
//!
//! Compilation deliberately produces one artifact per project. This final phase
//! consumes valid compiler-produced artifacts in dependency-first order and
//! makes the root artifact self-contained. It performs no optimization: every
//! extern, function, and global is copied in graph order, and only
//! artifact-local function-table indices change.

use std::{error::Error, fmt};

use crate::artifact::{self, Artifact, Callee, Op};

/// A failure to satisfy the linker's minimal input precondition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    EmptyGraph,
    ExecutableDependency(String),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGraph => f.write_str("cannot link an empty artifact graph"),
            Self::ExecutableDependency(name) => {
                write!(f, "executable bundle `{name}` cannot be a dependency")
            }
        }
    }
}

impl Error for LinkError {}

/// Link a dependency-first graph of valid compiler-produced artifacts into one
/// self-contained root artifact.
///
/// The root is the final input. Its identity and public interface are retained;
/// its dependency list is cleared because all dependency code is copied into
/// the returned LIR. Dependency type and effect declarations remain available
/// to interpret the root's schemes, including recursive outgoing callables.
/// Semantically malformed artifacts are outside this API's
/// contract and are not validated here.
pub fn link(artifacts: &[Artifact]) -> Result<Artifact, LinkError> {
    let Some(root) = artifacts.last() else {
        return Err(LinkError::EmptyGraph);
    };
    for dependency in &artifacts[..artifacts.len() - 1] {
        if dependency.header().kind == artifact::Kind::Executable {
            return Err(LinkError::ExecutableDependency(
                dependency.header().identity.name.clone(),
            ));
        }
    }

    let mut externs = Vec::new();
    let mut functions = Vec::new();
    let mut globals = Vec::new();
    for artifact in artifacts {
        externs.extend(artifact.lir().externs.iter().cloned());
        // Artifact indices are u64 and Rust vectors cannot exceed u64::MAX
        // entries on supported targets.
        let offset = functions.len() as u64;
        for function in &artifact.lir().functions {
            let mut function = function.clone();
            for block in &mut function.blocks {
                relocate_block(block, offset);
            }
            functions.push(function);
        }
        for global in &artifact.lir().globals {
            let mut global = global.clone();
            global.initializer += offset;
            globals.push(global);
        }
    }

    let mut header = root.header().clone();
    header.dependencies.clear();
    let mut types: std::collections::HashSet<_> =
        header.types.iter().map(|ty| ty.name.clone()).collect();
    let mut effects: std::collections::HashSet<_> = header
        .effects
        .iter()
        .map(|effect| effect.name.clone())
        .collect();
    for dependency in &artifacts[..artifacts.len() - 1] {
        for ty in &dependency.header().types {
            if types.insert(ty.name.clone()) {
                let mut ty = ty.clone();
                ty.exported = false;
                header.types.push(ty);
            }
        }
        for effect in &dependency.header().effects {
            if effects.insert(effect.name.clone()) {
                let mut effect = effect.clone();
                effect.exported = false;
                header.effects.push(effect);
            }
        }
    }
    Ok(Artifact::from_validated_parts(
        header,
        artifact::Lir {
            externs,
            functions,
            globals,
        },
    ))
}

fn relocate_block(block: &mut artifact::Block, offset: u64) {
    for instruction in &mut block.instrs {
        match &mut instruction.op {
            Op::Closure { func, .. } => *func += offset,
            Op::Continuation { code, .. } => code.function += offset,
            _ => {}
        }
    }
    if let artifact::End::Call {
        callee: Callee::Direct(func),
        ..
    } = &mut block.end
    {
        *func += offset;
    }
}
