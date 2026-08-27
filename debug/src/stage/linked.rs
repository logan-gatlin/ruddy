//! The final self-contained artifact produced by static linking.

use crate::{
    stage::{Cx, Spec},
    wire::Stage,
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(linked) = cx.linked else {
        return if cx.link_panicked {
            crate::stage::panicked(spec)
        } else {
            crate::stage::skipped(spec, "static linking did not run")
        };
    };
    super::artifact::render(spec, cx, linked, cx.micros.link)
}
