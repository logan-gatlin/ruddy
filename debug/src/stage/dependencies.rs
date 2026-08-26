//! Resolved project dependencies for the active debugger document.

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage, Status},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let mut ids = Ids::default();
    let children: Vec<Node> = cx
        .dependency_declarations
        .iter()
        .map(|(name, path)| {
            let built = cx
                .dependencies
                .iter()
                .find(|dependency| dependency.name == *name);
            Node::new(
                ids.next(),
                "project",
                built.map_or_else(
                    || name.clone(),
                    |dependency| format!("{}@{}", dependency.name, dependency.version),
                ),
            )
            .field("declared path", path)
            .field("status", if built.is_some() { "built" } else { "failed" })
            .field(
                "artifact",
                if built.is_some() { "in memory" } else { "none" },
            )
        })
        .collect();
    let root = Node::new(ids.next(), "active project", "browser source")
        .field(
            "status",
            if cx.dependencies_valid {
                "ready"
            } else {
                "blocked"
            },
        )
        .children(children);
    Stage {
        micros: Some(cx.micros.dependencies),
        nodes: vec![root],
        ..spec.stage(
            if cx.dependencies_valid {
                Status::Ok
            } else {
                Status::Partial
            },
            format!(
                "{} declared · {} built",
                cx.dependency_declarations.len(),
                cx.dependencies.len()
            ),
        )
    }
}
