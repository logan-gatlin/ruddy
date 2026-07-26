//! The JSON contract between the server and the page.
//!
//! Every stage the debugger renders — today tokens, AST, IR and symbols;
//! tomorrow types or assembly — is serialized into the same [`Stage`] of
//! [`Node`]s. The page derives tabs, filtering and cross-highlighting from this
//! shape alone, so a new stage costs a backend file and nothing else.

use serde::{Deserialize, Serialize};

/// A byte range in the source, `[start, end)`. Always UTF-8 offsets, matching
/// [`ruddy::tracking::Span`]; the page converts to UTF-16 indices itself.
pub type Range = [usize; 2];

#[derive(Debug, Deserialize)]
pub struct CompileRequest {
    pub source: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub bundle: BundleSpec,
}

#[derive(Debug, Deserialize)]
pub struct BundleSpec {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub build: u64,
    pub source_len: usize,
    /// Byte offset of each line start, so the page can map an offset to a
    /// line and column without rescanning the source.
    pub line_starts: Vec<usize>,
    pub stages: Vec<Stage>,
    pub diagnostics: Vec<Diagnostic>,
    pub panic: Option<Panic>,
}

#[derive(Debug, Serialize)]
pub struct Stage {
    pub id: &'static str,
    pub title: &'static str,
    pub view: View,
    pub status: Status,
    /// Shown next to the tab title, e.g. `3 types · 5 terms`.
    pub summary: String,
    pub micros: u64,
    pub nodes: Vec<Node>,
    /// Populated for [`View::Text`] stages only.
    pub text: Option<String>,
    /// The compiler's own `Display` output, verbatim.
    pub display: String,
    /// `{:#?}`, so a field the renderer has not learned about yet is still
    /// visible the moment it exists.
    pub debug: String,
    /// When set, this stage decorates another stage's rows instead of owning a
    /// tab of its own.
    pub annotates: Option<&'static str>,
}

/// `Text` has no producer yet; it is part of the contract so that a stage
/// emitting assembly can be added without changing the wire format or the page.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum View {
    List,
    Tree,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    /// Produced, but from input that earlier stages reported errors for.
    Partial,
    /// Its input never arrived, so there was nothing to render.
    Skipped,
    Panicked,
}

#[derive(Debug, Default, Serialize)]
pub struct Node {
    pub id: u32,
    /// The node kind, rendered bold.
    pub label: String,
    /// The rendered value, dimmed.
    pub text: String,
    pub span: Option<Range>,
    /// The node came from a generated span rather than from written source.
    pub generated: bool,
    /// Index into the `symbols` stage, when this node names a symbol.
    pub symbol: Option<u32>,
    /// Rendered red: an error node, or a check this stage runs that failed.
    pub error: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

/// A scalar shown as a column in a list view. A name starting with `_` marks a
/// value meant for the page rather than for the reader — the list view hides
/// those columns.
#[derive(Debug, Serialize)]
pub struct Field {
    pub name: &'static str,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub id: u32,
    /// Which stage produced it: `lex`, `parse`, `ir`, …
    pub stage: &'static str,
    pub severity: Severity,
    /// Stable and greppable; the page filters on it.
    pub code: &'static str,
    pub message: String,
    pub span: Option<Range>,
    /// Secondary spans, e.g. the first definition a duplicate repeats.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

/// The compiler reports nothing but errors so far; `Warning` is here so that
/// the day it does, only the producing stage changes.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Serialize)]
pub struct Related {
    pub span: Option<Range>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Panic {
    pub stage: String,
    pub message: String,
    pub location: String,
    pub backtrace: String,
}

#[derive(Debug, Serialize)]
pub struct ServerStatus {
    pub build: u64,
    pub watching: bool,
    /// A document named on the command line, which the page opens instead of
    /// whatever it had open last.
    pub doc: Option<String>,
    /// The rustc output from a failed rebuild, when the supervisor fell back to
    /// the last good binary.
    pub build_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DocMeta {
    pub name: String,
    pub bytes: u64,
    pub modified_ms: u128,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub name: String,
    pub source: String,
    pub modified_ms: u128,
}

#[derive(Debug, Deserialize)]
pub struct DocBody {
    pub source: String,
}

impl Default for BundleSpec {
    fn default() -> Self {
        Self {
            name: "demo".into(),
            version: "0.1.0".into(),
        }
    }
}

impl Node {
    pub fn new(id: u32, label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    /// Attach the span a node was written at. A generated span carries no
    /// source range to highlight, so it is recorded as a flag instead.
    pub fn at(mut self, span: ruddy::tracking::Span) -> Self {
        match span.is_generated() {
            true => self.generated = true,
            false => self.span = Some([span.start, span.end()]),
        }
        self
    }

    pub fn symbol(mut self, index: u32) -> Self {
        self.symbol = Some(index);
        self
    }

    pub fn error(mut self) -> Self {
        self.error = true;
        self
    }

    pub fn field(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.fields.push(Field {
            name,
            value: value.into(),
        });
        self
    }

    pub fn child(mut self, node: Node) -> Self {
        self.children.push(node);
        self
    }

    pub fn children(mut self, nodes: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(nodes);
        self
    }
}

/// Byte offsets of every line start, including the first.
pub fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(i, c)| i + c.len_utf8()),
    );
    starts
}
