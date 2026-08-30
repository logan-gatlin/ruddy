//! Expanded compiler diagnostics, rendered by the same Ariadne boundary as the CLI.
//!
//! The diagnostic strip stays terse and always visible. This tab is the place
//! to read the quoted source, primary highlight and related locations together.

use ruddy_cli::{DiagnosticLabel, DiagnosticSource};

use crate::{
    stage::{Cx, Spec, plural},
    wire::{Stage, Status},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    if cx.diagnostics.is_empty() {
        return Stage {
            text: Some("no errors".to_string()),
            debug: "no errors".to_string(),
            ..spec.stage(Status::Ok, "no errors")
        };
    }

    let available: Vec<_> = cx
        .files
        .iter()
        .zip(cx.sources)
        .map(|(file, source)| DiagnosticSource {
            path: &file.path,
            source,
        })
        .collect();
    let render = |color| {
        cx.diagnostics
            .iter()
            .map(|diagnostic| {
                if let Some(report) = &diagnostic.report {
                    return report.render(color);
                }
                let primary = diagnostic.span.map(|span| DiagnosticLabel {
                    source: span.file as usize,
                    range: span.range[0]..span.range[1],
                    message: &diagnostic.label,
                });
                let related: Vec<_> = diagnostic
                    .related
                    .iter()
                    .filter_map(|related| {
                        related.span.map(|span| DiagnosticLabel {
                            source: span.file as usize,
                            range: span.range[0]..span.range[1],
                            message: &related.message,
                        })
                    })
                    .collect();
                let notes: Vec<_> = diagnostic.notes.iter().map(String::as_str).collect();
                ruddy_cli::render_diagnostic_with_advice(
                    diagnostic.stage,
                    diagnostic.code,
                    &diagnostic.message,
                    &available,
                    primary.as_ref(),
                    &related,
                    diagnostic.help.first().map(String::as_str),
                    &notes,
                    color,
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let text = render(true);

    Stage {
        status: Status::Partial,
        summary: plural(cx.diagnostics.len(), "error"),
        text: Some(text),
        debug: render(false),
        ..spec.stage(Status::Partial, "")
    }
}
