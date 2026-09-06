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
        Node::new(ids.next(), "kind", artifact.header().kind.to_string()),
        Node::new(
            ids.next(),
            "dependencies",
            format!("{} declared", artifact.header().dependencies.len()),
        )
        .children(dependency_nodes),
    ];
    for value in &artifact.header().values {
        interface.push(
            Node::new(ids.next(), "value", &value.name)
                .children(metadata(&mut ids, &value.metadata)),
        );
    }
    for ty in &artifact.header().types {
        interface.push(
            Node::new(ids.next(), "type", &ty.name)
                .field("visibility", if ty.exported { "public" } else { "private" })
                .children(metadata(&mut ids, &ty.metadata)),
        );
    }
    for effect in &artifact.header().effects {
        let mut node = Node::new(ids.next(), "effect", &effect.name)
            .field(
                "visibility",
                if effect.exported { "public" } else { "private" },
            )
            .children(metadata(&mut ids, &effect.metadata));
        for param in &effect.params {
            node = node.child(Node::new(
                ids.next(),
                "param",
                format!("{:?}", param.sense).to_lowercase(),
            ));
        }
        match &effect.kind {
            ruddy::artifact::EffectKind::Operations(operations) => {
                node = node.children(
                    operations
                        .iter()
                        .map(|operation| {
                            let selector = match &operation.selector {
                                ruddy::artifact::OperationSelector::Unnamed => {
                                    "unnamed".to_string()
                                }
                                ruddy::artifact::OperationSelector::Named(name) => {
                                    format!("named {name}")
                                }
                            };
                            Node::new(ids.next(), "selector", selector)
                        })
                        .collect::<Vec<_>>(),
                );
            }
            // An alias: the effects it applies, each with how many arguments,
            // and the parameter it ends in.
            ruddy::artifact::EffectKind::Alias(row) => {
                for case in &row.cases {
                    node = node.child(Node::new(
                        ids.next(),
                        "case",
                        match case.args.len() {
                            0 => case.name.clone(),
                            n => format!("{} ({n} arguments)", case.name),
                        },
                    ));
                }
                if let Some(index) = row.tail {
                    node = node.child(Node::new(ids.next(), "tail", format!("parameter {index}")));
                }
            }
        }
        interface.push(node);
    }
    for module in &artifact.header().modules {
        interface.push(
            Node::new(ids.next(), "module", &module.name)
                .children(metadata(&mut ids, &module.metadata)),
        );
    }
    let header = Node::new(
        ids.next(),
        "header",
        format!(
            "{}@{}",
            artifact.header().identity.name,
            artifact.header().identity.version
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
    for (function_id, function) in artifact.lir().functions.iter().enumerate() {
        let mut blocks = Vec::new();
        for (block_id, block) in function.blocks.iter().enumerate() {
            let mut instructions = Vec::new();
            for instruction in &block.instrs {
                instructions.push(Node::new(
                    ids.next(),
                    "value",
                    format!(
                        "%{}: {:?} = {:?}",
                        instruction.temp, instruction.rep, instruction.op
                    ),
                ));
            }
            instructions.push(Node::new(
                ids.next(),
                "transfer",
                format!("{:?}", block.end),
            ));
            blocks.push(
                Node::new(ids.next(), "block", format!("f{function_id}:b{block_id}"))
                    .field("parameters", format!("{:?}", block.params))
                    .field("result", format!("{:?}", block.result))
                    .children(instructions),
            );
        }
        lowered.push(
            Node::new(
                ids.next(),
                "function",
                format!("f{function_id} {}", function.name),
            )
            .field("entry", format!("b{}", function.entry))
            .field("suspension", format!("{:?}", function.suspension))
            .children(blocks),
        );
    }
    for global in &artifact.lir().globals {
        lowered.push(
            Node::new(ids.next(), "global", &global.name)
                .field("initializer", format!("f{}", global.initializer))
                .field("callable", format!("{:?}", global.callable))
                .field("adapter", format!("{:?}", global.adapter)),
        );
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
                "{} · {} values · {} types · {} effects · {} modules · {} externs · {} functions · {} globals",
                dependencies,
                artifact.header().values.len(),
                artifact.header().types.len(),
                artifact.header().effects.len(),
                artifact.header().modules.len(),
                artifact.lir().externs.len(),
                artifact.lir().functions.len(),
                artifact.lir().globals.len(),
            ),
        )
    }
}

/// The published metadata of one declaration, one row per entry, written as
/// the source it stands for rather than as the artifact's own text.
fn metadata(ids: &mut Ids, metadata: &ruddy::artifact::Metadata) -> Vec<Node> {
    metadata
        .iter()
        .map(|(key, value)| {
            Node::new(
                ids.next(),
                "metadata",
                crate::print::artifact::attribute(key, value).to_string(),
            )
        })
        .collect()
}
