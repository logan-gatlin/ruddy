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

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(artifact) = cx.artifact else {
        return crate::stage::skipped(spec, "artifact construction did not run");
    };
    let text = Artifact::print(artifact);
    let mut ids = Ids::default();

    let dependency_nodes: Vec<_> = artifact
        .header
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
            format!("{} declared", artifact.header.dependencies.len()),
        )
        .children(dependency_nodes),
    ];
    for value in &artifact.header.values {
        interface.push(Node::new(ids.next(), "value", &value.name));
    }
    for ty in &artifact.header.types {
        interface.push(Node::new(ids.next(), "type", &ty.name));
    }
    for effect in &artifact.header.effects {
        interface.push(Node::new(ids.next(), "effect", &effect.name));
    }
    let header = Node::new(
        ids.next(),
        "header",
        format!(
            "{}@{}",
            artifact.header.identity.name, artifact.header.identity.version
        ),
    )
    .children(interface);

    let mut lowered = Vec::new();
    for function in &artifact.lir.functions {
        lowered.push(Node::new(ids.next(), "function", &function.name));
    }
    for global in &artifact.lir.globals {
        lowered.push(Node::new(ids.next(), "global", &global.name));
    }
    let lir = Node::new(
        ids.next(),
        "lir",
        format!(
            "{} functions · {} globals",
            artifact.lir.functions.len(),
            artifact.lir.globals.len()
        ),
    )
    .children(lowered);

    let dependencies = match artifact.header.dependencies.len() {
        1 => "1 dependency".to_string(),
        count => format!("{count} dependencies"),
    };

    Stage {
        micros: Some(cx.micros.artifact),
        nodes: vec![header, lir],
        text: Some(text),
        debug: format!("{artifact:#?}"),
        ..spec.stage(
            cx.status(),
            format!(
                "{} · {} values · {} types · {} effects · {} functions · {} globals",
                dependencies,
                artifact.header.values.len(),
                artifact.header.types.len(),
                artifact.header.effects.len(),
                artifact.lir.functions.len(),
                artifact.lir.globals.len(),
            ),
        )
    }
}
