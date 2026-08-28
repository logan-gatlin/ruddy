//! JavaScript emitted from the final, statically linked artifact.
//!
//! Generation is deliberately a debugger phase of its own: it runs from the
//! same in-memory linked artifact as the command-line backend, but does not
//! depend on the active project's manifest target.

use crate::{
    stage::{Cx, Spec},
    wire::{Stage, Status},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(source) = cx.javascript else {
        return missing_timed(
            spec,
            cx.javascript_panicked,
            cx.javascript_error,
            cx.micros.javascript,
        );
    };

    Stage {
        status: Status::Ok,
        micros: Some(cx.micros.javascript),
        summary: format!("{} bytes", source.len()),
        text: Some(source.to_string()),
        debug: source.to_string(),
        ..spec.stage(Status::Ok, "")
    }
}

/// Render the distinction between a phase that never ran and one that failed.
pub fn missing(spec: &Spec, panicked: bool, error: Option<&str>) -> Stage {
    missing_timed(spec, panicked, error, 0)
}

fn missing_timed(spec: &Spec, panicked: bool, error: Option<&str>, micros: u64) -> Stage {
    let mut stage = if panicked {
        crate::stage::panicked(spec)
    } else if let Some(error) = error {
        crate::stage::failed(spec, error)
    } else {
        return crate::stage::skipped(spec, "JavaScript generation did not run");
    };
    stage.micros = Some(micros);
    stage
}
