//! The final self-contained artifact produced by static linking.

use crate::{
    stage::{Cx, Spec},
    wire::Stage,
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(linked) = cx.linked else {
        return missing(spec, cx.link_panicked, cx.link_error);
    };
    super::artifact::render(spec, cx, linked, cx.micros.link)
}

pub fn missing(spec: &Spec, panicked: bool, error: Option<&str>) -> Stage {
    if panicked {
        crate::stage::panicked(spec)
    } else if let Some(error) = error {
        crate::stage::failed(spec, error)
    } else {
        crate::stage::skipped(spec, "static linking did not run")
    }
}
