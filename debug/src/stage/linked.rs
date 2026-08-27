//! The final self-contained artifact produced by static linking.

use crate::{
    stage::{Cx, Spec},
    wire::Stage,
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(linked) = cx.linked else {
        return missing_timed(spec, cx.link_panicked, cx.link_error, cx.micros.link);
    };
    super::artifact::render(spec, cx, linked, cx.micros.link)
}

pub fn missing(spec: &Spec, panicked: bool, error: Option<&str>) -> Stage {
    missing_timed(spec, panicked, error, 0)
}

fn missing_timed(spec: &Spec, panicked: bool, error: Option<&str>, micros: u64) -> Stage {
    let mut stage = if panicked {
        crate::stage::panicked(spec)
    } else if let Some(error) = error {
        crate::stage::failed(spec, error)
    } else {
        return crate::stage::skipped(spec, "static linking did not run");
    };
    stage.micros = Some(micros);
    stage
}
