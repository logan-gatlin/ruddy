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
        .map(|(alias, specification)| {
            // The three slices are produced in preserved direct-request order.
            // Alias position, unlike bundle name, distinguishes two requests
            // for different versions of the same bundle.
            let position = cx
                .dependency_aliases
                .iter()
                .position(|resolved| resolved == alias);
            let built = position.and_then(|index| cx.dependencies.get(index));
            let interface = position.and_then(|index| cx.dependency_interfaces.get(index));
            let mut node = Node::new(
                ids.next(),
                "project",
                built.map_or_else(
                    || alias.clone(),
                    |dependency| format!("{}@{}", dependency.name, dependency.version),
                ),
            )
            .field("source alias", alias)
            .field("declared path", specification.path())
            .field("status", if built.is_some() { "built" } else { "failed" })
            .field(
                "artifact",
                if built.is_some() { "in memory" } else { "none" },
            );
            if let Some(interface) = interface {
                node = node
                    .field("imported values", interface.header.values.len().to_string())
                    .field("imported types", interface.header.types.len().to_string())
                    .field(
                        "imported effects",
                        interface.header.effects.len().to_string(),
                    );
            }
            node
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
