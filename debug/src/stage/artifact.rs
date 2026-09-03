//! The portable bundle artifact as canonical text.
//!
//! Unlike LIR, artifact text is the intended disk boundary. It has no source
//! spans to cross-highlight, so the panel offers the canonical text and a
//! compact structure-only outline.

use ruddy::artifact::Artifact;

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage},
};

/// Render the distinction between a phase that never ran and one that failed.
pub fn missing(spec: &Spec, panicked: bool) -> Stage {
    if panicked {
        crate::stage::panicked(spec)
    } else {
        crate::stage::skipped(spec, "artifact construction did not run")
    }
}

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(artifact) = cx.artifact else {
        return missing(spec, cx.artifact_panicked);
    };
    render(spec, cx, artifact, cx.micros.artifact)
}

/// Render an artifact-shaped compiler boundary for artifact and link stages.
pub fn render(spec: &Spec, cx: &Cx, artifact: &Artifact, micros: u64) -> Stage {
    let text = Artifact::print(artifact);
    let mut ids = Ids::default();

    let dependency_nodes: Vec<_> = artifact
        .header()
        .dependencies
        .iter()
        .map(|dependency| {
            Node::new(
                ids.next(),
                "dependency",
                format!("{}@{}", dependency.name, dependency.version),
            )
        })
        .collect();
    let mut interface = vec![
        Node::new(
            ids.next(),
            "dependencies",
            format!("{} declared", artifact.header().dependencies.len()),
        )
        .children(dependency_nodes),
    ];
    for value in &artifact.header().values {
        interface.push(Node::new(ids.next(), "value", &value.name));
    }
    for ty in &artifact.header().types {
        interface.push(Node::new(ids.next(), "type", &ty.name));
    }
    for effect in &artifact.header().effects {
        let mut node = Node::new(ids.next(), "effect", &effect.name);
        if let ruddy::artifact::EffectKind::Operations(operations) = &effect.kind {
            node = node.children(
                operations
                    .iter()
                    .map(|operation| {
                        let selector = match &operation.selector {
                            ruddy::artifact::OperationSelector::Unnamed => "unnamed".to_string(),
                            ruddy::artifact::OperationSelector::Named(name) => {
                                format!("named {name}")
                            }
                        };
                        Node::new(ids.next(), "selector", selector)
                    })
                    .collect::<Vec<_>>(),
            );
        }
        interface.push(node);
    }
    let header = Node::new(
        ids.next(),
        "header",
        format!(
            "{}@{}",
            artifact.header().identity.name, artifact.header().identity.version
        ),
    )
    .children(interface);

    let mut lowered = Vec::new();
    for external in &artifact.lir().externs {
        lowered.push(Node::new(
            ids.next(),
            "extern",
            format!(
                "{} = {} · {:?}",
                external.name,
                crate::print::string(&external.target),
                external.rep
            ),
        ));
    }
    for function in &artifact.lir().functions {
        lowered.push(Node::new(ids.next(), "function", &function.name));
    }
    for global in &artifact.lir().globals {
        lowered.push(Node::new(ids.next(), "global", &global.name));
    }
    let lir = Node::new(
        ids.next(),
        "lir",
        format!(
            "{} externs · {} functions · {} globals",
            artifact.lir().externs.len(),
            artifact.lir().functions.len(),
            artifact.lir().globals.len()
        ),
    )
    .children(lowered);

    let dependencies = match artifact.header().dependencies.len() {
        1 => "1 dependency".to_string(),
        count => format!("{count} dependencies"),
    };

    Stage {
        micros: Some(micros),
        nodes: vec![header, lir],
        text: Some(text),
        debug: format!("{artifact:#?}"),
        ..spec.stage(
            cx.status(),
            format!(
                "{} · {} values · {} types · {} effects · {} externs · {} functions · {} globals",
                dependencies,
                artifact.header().values.len(),
                artifact.header().types.len(),
                artifact.header().effects.len(),
                artifact.lir().externs.len(),
                artifact.lir().functions.len(),
                artifact.lir().globals.len(),
            ),
        )
    }
}
