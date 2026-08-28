//! Static linking of portable artifacts.
//!
//! Compilation deliberately produces one artifact per project. This final phase
//! consumes valid compiler-produced artifacts in dependency-first order and
//! makes the root artifact self-contained. It performs no optimization: every
//! function and global is copied, in graph order, and only artifact-local
//! function-table indices change.

use std::{error::Error, fmt};

use crate::artifact::{self, Artifact, Callee, Op};

/// A failure to satisfy the linker's minimal input precondition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    EmptyGraph,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGraph => f.write_str("cannot link an empty artifact graph"),
        }
    }
}

impl Error for LinkError {}

/// Link a dependency-first graph of valid compiler-produced artifacts into one
/// self-contained root artifact.
///
/// The root is the final input. Its identity and public interface are retained;
/// its dependency list is cleared because all dependency code is copied into
/// the returned LIR. Semantically malformed artifacts are outside this API's
/// contract and are not validated here.
pub fn link(artifacts: &[Artifact]) -> Result<Artifact, LinkError> {
    let Some(root) = artifacts.last() else {
        return Err(LinkError::EmptyGraph);
    };

    let mut functions = Vec::new();
    let mut globals = Vec::new();
    for artifact in artifacts {
        // Artifact indices are u64 and Rust vectors cannot exceed u64::MAX
        // entries on supported targets.
        let offset = functions.len() as u64;
        for function in &artifact.lir.functions {
            let mut function = function.clone();
            relocate_block(&mut function.body, offset);
            functions.push(function);
        }
        for global in &artifact.lir.globals {
            let mut global = global.clone();
            relocate_block(&mut global.body, offset);
            globals.push(global);
        }
    }

    let mut header = root.header.clone();
    header.dependencies.clear();
    Ok(Artifact {
        header,
        lir: artifact::Lir { functions, globals },
    })
}

fn relocate_block(block: &mut artifact::Block, offset: u64) {
    // Push operations in reverse so the explicit stack retains the recursive
    // walk's instruction and branch order without consuming the call stack.
    let mut pending: Vec<&mut Op> = block
        .instrs
        .iter_mut()
        .rev()
        .map(|instruction| &mut instruction.op)
        .collect();
    while let Some(op) = pending.pop() {
        match op {
            Op::Closure { func, .. }
            | Op::Call {
                callee: Callee::Direct(func),
                ..
            } => *func += offset,
            Op::Catch { body, .. } => pending.extend(
                body.instrs
                    .iter_mut()
                    .rev()
                    .map(|instruction| &mut instruction.op),
            ),
            Op::SwitchTag {
                cases, fallback, ..
            } => {
                if let Some(block) = fallback {
                    pending.extend(
                        block
                            .instrs
                            .iter_mut()
                            .rev()
                            .map(|instruction| &mut instruction.op),
                    );
                }
                for case in cases.iter_mut().rev() {
                    pending.extend(
                        case.block
                            .instrs
                            .iter_mut()
                            .rev()
                            .map(|instruction| &mut instruction.op),
                    );
                }
            }
            Op::SwitchPrim {
                cases, fallback, ..
            } => {
                if let Some(block) = fallback {
                    pending.extend(
                        block
                            .instrs
                            .iter_mut()
                            .rev()
                            .map(|instruction| &mut instruction.op),
                    );
                }
                for case in cases.iter_mut().rev() {
                    pending.extend(
                        case.block
                            .instrs
                            .iter_mut()
                            .rev()
                            .map(|instruction| &mut instruction.op),
                    );
                }
            }
            Op::SwitchPresence {
                present, absent, ..
            } => {
                pending.extend(
                    absent
                        .instrs
                        .iter_mut()
                        .rev()
                        .map(|instruction| &mut instruction.op),
                );
                pending.extend(
                    present
                        .instrs
                        .iter_mut()
                        .rev()
                        .map(|instruction| &mut instruction.op),
                );
            }
            Op::SwitchRest { none, some, .. } => {
                pending.extend(
                    some.instrs
                        .iter_mut()
                        .rev()
                        .map(|instruction| &mut instruction.op),
                );
                pending.extend(
                    none.instrs
                        .iter_mut()
                        .rev()
                        .map(|instruction| &mut instruction.op),
                );
            }
            _ => {}
        }
    }
}
