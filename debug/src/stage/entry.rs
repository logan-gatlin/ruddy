//! Target-neutral executable entry validation, separate from artifact creation.

use crate::{
    stage::{Cx, Spec},
    wire::Stage,
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let mut stage = match cx.entry {
        Some(Ok(artifact)) => return super::artifact::render(spec, cx, artifact, cx.micros.entry),
        Some(Err(error)) => super::failed(spec, &error.to_string()),
        None if cx.entry_panicked => super::panicked(spec),
        None => {
            return super::skipped(
                spec,
                "entry validation did not run (libraries have no entry contract)",
            );
        }
    };
    stage.micros = Some(cx.micros.entry);
    stage
}
