//! The JSON contract between the server and the page.
//!
//! Every stage the debugger renders — today tokens, AST, IR, constraints, the
//! solve, types and symbols; tomorrow assembly — is serialized into the same
//! [`Stage`] of [`Node`]s. The page derives tabs, filtering and cross-highlighting from this
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
    /// A regular expression over this stage's row text, naming the runs that
    /// mean the same thing wherever they appear — a solver variable, say.
    /// Hovering one lights every other occurrence in the panel.
    ///
    /// The pattern lives here rather than in the page because the notation is
    /// the stage's: the page matches what it is given without knowing what a
    /// match means. `None` for a stage whose text names nothing twice.
    pub highlight: Option<&'static str>,
    /// Whether a match of `highlight` names the same thing only within the row
    /// it appears in. Also the stage's to say: a `?4` is one variable of one
    /// program-wide table, but a scheme's letters are numbered from scratch
    /// per scheme, so two rows spelling `a` are two unrelated variables.
    pub scoped: bool,
    pub status: Status,
    /// Shown next to the tab title, e.g. `3 types · 5 terms`.
    pub summary: String,
    /// How long the compiler phase this stage owns took, in microseconds, or
    /// `None` for a stage that owns none.
    ///
    /// The distinction is carried rather than left to be read off the number.
    /// Most stages own a phase; an annotator, or a second view of a phase's
    /// output — `Constraints` and `Solve` both read the one `infer` call — owns
    /// nothing, and a chip for it would be the same microseconds counted twice.
    /// That used to be inferred from `micros > 0`, which is a measurement
    /// standing in for a fact about the registry: the figure is truncated to
    /// whole microseconds, so a phase that genuinely ran in under one reported
    /// zero and lost its chip. `Option` says which it is and cannot disagree
    /// with itself.
    pub micros: Option<u64>,
    pub nodes: Vec<Node>,
    /// Populated for [`View::Text`] stages only.
    pub text: Option<String>,
    /// `{:#?}`, so a field the renderer has not learned about yet is still
    /// visible the moment it exists. This is the panel's `raw` view.
    pub debug: String,
    /// When set, this stage decorates another stage's rows instead of owning a
    /// tab of its own.
    pub annotates: Option<&'static str>,
}

/// How a panel renders its nodes. `Text` has no producer yet; it is part of the
/// contract so that a stage emitting assembly can be added without changing the
/// wire format or the page.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum View {
    List,
    Tree,
    Text,
    /// A timeline with a cursor: the nodes are a sequence the reader walks one
    /// at a time rather than a shape they read all at once.
    Steps,
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
    /// Index into the `symbols` stage, when this node's span is an occurrence
    /// of that symbol's name. The page paints every carrier's span in the
    /// editor as a use of the symbol, so a node whose span is somewhere else
    /// belongs in `owner` instead.
    pub symbol: Option<u32>,
    /// Index into the `symbols` stage, when this node is *about* a symbol
    /// without being an occurrence of its name — a solve step, say, which is
    /// part of one definition's solve but is spanned by whatever
    /// sub-expression the constraint came from. The panel groups these with
    /// the symbol's other rows; the editor does not paint them, because there
    /// is nothing at that span the user wrote the name at.
    pub owner: Option<u32>,
    /// A group id for rows that name the same thing but have no `Symbol` to be
    /// grouped by — a `where let` variable and its uses, which are scoped to
    /// one annotation and reach no name table. The page paints every row of the
    /// focused row's group the way it paints a symbol's, and paints no editor
    /// range for them, since the group is the stage's own reading rather than
    /// something the name table can be asked about.
    pub link: Option<u32>,
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

    pub fn owner(mut self, index: u32) -> Self {
        self.owner = Some(index);
        self
    }

    pub fn link(mut self, group: u32) -> Self {
        self.link = Some(group);
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
