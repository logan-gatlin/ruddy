//! Resolved project dependencies for the active debugger document.

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{DependencySpec, Node, Stage, Status, StdConfig},
};

fn source(specification: &DependencySpec) -> String {
    match specification {
        DependencySpec::Path(path) => format!("path {}", path.display()),
        DependencySpec::Detailed(detail) => {
            if let Some(path) = &detail.path {
                return format!("path {}", path.display());
            }
            let mut source = format!("git {}", detail.git.as_deref().unwrap_or("<missing>"));
            if let Some((kind, value)) = detail
                .branch
                .as_deref()
                .map(|value| ("branch", value))
                .or_else(|| detail.tag.as_deref().map(|value| ("tag", value)))
                .or_else(|| detail.rev.as_deref().map(|value| ("rev", value)))
            {
                source.push_str(&format!(" ({kind} {value})"));
            }
            source
        }
    }
}

fn project(
    ids: &mut Ids,
    cx: &Cx,
    alias: &str,
    source_text: String,
    declared_path: Option<String>,
) -> Node {
    // The three slices are produced in preserved direct-request order. Alias
    // position, unlike bundle name, distinguishes different requested roots.
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
            || alias.to_string(),
            |dependency| format!("{}@{}", dependency.name, dependency.version),
        ),
    )
    .field("source alias", alias)
    .field("source", source_text)
    .field("status", if built.is_some() { "built" } else { "failed" })
    .field(
        "artifact",
        if built.is_some() { "in memory" } else { "none" },
    );
    if let Some(path) = declared_path {
        node = node.field("declared path", path);
    }
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
}

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let mut ids = Ids::default();
    let mut children = Vec::new();
    match cx.standard_library {
        StdConfig::Default => children.push(project(
            &mut ids,
            cx,
            "std",
            "installed default".to_string(),
            None,
        )),
        StdConfig::Disabled => {}
        StdConfig::Dependency(specification) => children.push(project(
            &mut ids,
            cx,
            "std",
            source(specification),
            specification.path().map(|path| path.display().to_string()),
        )),
    }
    children.extend(
        cx.dependency_declarations
            .iter()
            .map(|(alias, specification)| {
                project(
                    &mut ids,
                    cx,
                    alias,
                    source(specification),
                    specification.path().map(|path| path.display().to_string()),
                )
            }),
    );
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
                cx.dependency_declarations.len() + usize::from(!cx.standard_library.is_disabled()),
                cx.dependencies.len()
            ),
        )
    }
}
