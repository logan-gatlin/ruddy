//! The JSON contract between the server and the page.
//!
//! Every stage the debugger renders — today tokens, AST, IR, constraints, the
//! solve, types and symbols; tomorrow assembly — is serialized into the same
//! [`Stage`] of [`Node`]s. The page derives tabs, filtering and cross-highlighting from this
//! shape alone, so a new stage costs a backend file and nothing else.

use std::collections::HashMap;

use indexmap::IndexMap;
use ruddy::tracking::{FileID, Span};
pub use ruddy_cli::{DependencyDetail, DependencySpec, RunConfig, StdConfig};
use serde::{Deserialize, Serialize};

/// A byte range in one file, `[start, end)`. Always UTF-8 offsets, matching
/// [`ruddy::tracking::Span`]; the page converts to UTF-16 indices itself.
pub type Range = [usize; 2];

/// Where something was written: the file, and the range inside it.
///
/// A bundle is several files, so a range on its own no longer says where
/// anything is. `file` is a position in [`Snapshot::files`], which is the one
/// index the page and the server agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Loc {
    pub file: u32,
    pub range: Range,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompileRequest {
    #[serde(default)]
    pub kind: ruddy::artifact::Kind,
    #[serde(default)]
    pub target: Option<ruddy_cli::Target>,
    #[serde(default)]
    pub platform: Option<ruddy_cli::Platform>,
    /// The bundle identity supplied by project configuration in a normal
    /// compilation. Defaults keep cached requests from older debugger pages
    /// usable after identity moved out of source files.
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Root path configured by the active project's manifest.
    #[serde(default = "default_root")]
    pub root: String,
    /// Every file of the active bundle. A request without its configured root
    /// is told so rather than compiled.
    pub files: Vec<FileSpec>,
    /// Implicit standard-library configuration. Omission uses the installed std.
    #[serde(default, skip_serializing_if = "StdConfig::is_default")]
    pub std: StdConfig,
    /// Dependency project specifications keyed by source module alias.
    #[serde(default)]
    pub dependencies: IndexMap<String, DependencySpec>,
    /// Scratch document name, used to resolve saved dependency projects.
    #[serde(default = "default_name")]
    pub document: String,
    #[serde(default)]
    pub revision: u64,
}

// Dependency specifications are shared with the CLI so debugger manifests and
// browser requests accept exactly the same path and HTTPS Git forms.

fn default_name() -> String {
    "demo".to_string()
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_root() -> String {
    "main.hc".to_string()
}

/// One file of a bundle, as the page holds it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileSpec {
    /// Relative to the bundle root's directory, `/`-separated.
    pub path: String,
    pub source: String,
}

/// One file of a bundle, as the page reads it back: enough to turn any [`Loc`]
/// into a line and a column without rescanning anything.
#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub len: usize,
    /// Byte offset of each line start, so the page can map an offset to a
    /// line and column without rescanning the source.
    pub line_starts: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub build: u64,
    /// Every file the loader read, in load order: the root first, then
    /// depth-first through the modules. This is the index every [`Loc`] points
    /// into.
    pub files: Vec<FileInfo>,
    /// The externally supplied identity this compilation used, as
    /// `name@version`, or `None` when the supplied identity was invalid.
    pub bundle: Option<String>,
    pub stages: Vec<Stage>,
    pub diagnostics: Vec<Diagnostic>,
    pub panic: Option<Panic>,
}

#[derive(Debug, Serialize)]
pub struct Stage {
    pub id: &'static str,
    pub title: &'static str,
    pub view: View,
    /// Every reader-selectable rendering this stage supplies, in default-first
    /// order. `Text` stages can additionally expose their structural outline.
    pub views: &'static [View],
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

/// How a panel renders its nodes. A text stage may also offer its nodes as a
/// structural outline without giving up its canonical text rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum View {
    List,
    Tree,
    Text,
    /// ANSI-coloured terminal text, converted to themed spans by the page.
    Terminal,
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
    /// It ran normally but rejected its input.
    Error,
    Panicked,
}

#[derive(Debug, Default, Serialize)]
pub struct Node {
    pub id: u32,
    /// The node kind, rendered bold.
    pub label: String,
    /// The rendered value, dimmed.
    pub text: String,
    /// Where the node was written, as a [`Span`]: the file by [`FileID`], which
    /// has no wire form, and the range inside it.
    ///
    /// The index a [`Loc`] carries is a position in [`Snapshot::files`], which
    /// only the driver assembling the snapshot knows — so [`Node::at`] records
    /// the span here and [`locate`] turns it into a `Loc` once, in one walk
    /// over the finished stages. The alternative is a file map threaded through
    /// every tree printer in the tool, and there are ninety-odd of those.
    #[serde(skip)]
    pub at: Option<Span>,
    pub span: Option<Loc>,
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
    /// grouped by — a variable and its uses, which are scoped to
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
    /// Display identity in the source-sorted diagnostic strip.
    pub id: u32,
    /// Stable inference identity, when this came from inference. Unlike `id`,
    /// this survives debugger-wide sorting and never changes with other phases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_error_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_cause: Option<InferenceCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_explanation: Option<InferenceExplanation>,
    /// Which stage produced it: `lex`, `parse`, `ir`, …
    pub stage: &'static str,
    pub severity: Severity,
    /// Stable and greppable; the page filters on it.
    pub code: &'static str,
    pub message: String,
    /// Location-specific explanation, kept separate from the headline.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub help: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// A dependency compiler report may quote files outside the active editor
    /// bundle. Keep it server-side so the Errors tab can render that source
    /// without adding external files to the browser's editable file list.
    #[serde(skip)]
    pub report: Option<ruddy_cli::CompileDiagnostic>,
    pub span: Option<Loc>,
    /// Secondary spans, e.g. the first definition a duplicate repeats.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InferenceCause {
    Step { step_id: String },
    Batch { batch_id: String },
    Direct,
}

#[derive(Debug, Serialize)]
pub struct InferenceExplanation {
    pub abridged: Vec<ExplanationFact>,
    pub full: Vec<ExplanationFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pivot: Option<ExplanationPivot>,
    pub omitted_facts: usize,
    pub contradiction: ExplanationContradiction,
    pub cause: ExplanationCause,
}

#[derive(Debug, Serialize)]
pub struct ExplanationPivot {
    pub name: String,
    pub kind: &'static str,
    pub references: Vec<usize>,
    pub introduced_at: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplanationFact {
    pub span: Option<Loc>,
    /// Absent for an explicit direct source fact, rather than a solver fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint_id: Option<String>,
    pub direct: bool,
    pub origin: &'static str,
    pub subject: &'static str,
    pub payload: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ExplanationContradiction {
    pub kind: &'static str,
    pub left: &'static str,
    pub right: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<ExplanationRow>,
    pub repairs: [&'static str; 2],
}

#[derive(Debug, Serialize)]
pub struct ExplanationRow {
    pub shape: &'static str,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct ExplanationCause {
    pub error_id: String,
    pub seed_reason_id: Option<String>,
    pub constraint_ids: Vec<String>,
    pub reason_ids: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_reasons: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
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
    pub span: Option<Loc>,
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

/// One document: a whole bundle, which is a directory of files rather than a
/// single snippet.
#[derive(Debug, Serialize)]
pub struct Doc {
    pub kind: ruddy::artifact::Kind,
    pub target: Option<ruddy_cli::Target>,
    pub platform: Option<ruddy_cli::Platform>,
    pub name: String,
    pub bundle_name: String,
    pub version: String,
    pub root: String,
    pub run: RunConfig,
    #[serde(default, skip_serializing_if = "StdConfig::is_default")]
    pub std: StdConfig,
    pub dependencies: IndexMap<String, DependencySpec>,
    pub files: Vec<FileSpec>,
    pub modified_ms: u128,
}

/// The browser context attached to the shared debugging session. The snapshot
/// contains everything that can be inspected; this says which part the human
/// is currently inspecting.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SessionView {
    #[serde(default)]
    pub active_file: String,
    #[serde(default)]
    pub caret: usize,
    #[serde(default)]
    pub tabs: Vec<String>,
    #[serde(default)]
    pub views: HashMap<String, String>,
    #[serde(default)]
    pub split: bool,
    #[serde(default)]
    pub pane: usize,
    /// Per-pane rendered context: stage/view, filter, step cursor, scroll, and
    /// the node ids actually visible after filtering and collapsing.
    #[serde(default)]
    pub panes: Vec<serde_json::Value>,
    #[serde(default)]
    pub selection: Option<serde_json::Value>,
}

/// The body accepted by `PUT /session`: one optimistic source-file edit.
#[derive(Debug, Deserialize)]
pub struct SessionEdit {
    pub base_revision: u64,
    pub document: String,
    pub path: String,
    pub source: String,
}

/// An agent-readable copy of the live browser session.
#[derive(Debug, Serialize)]
pub struct SharedSession {
    pub protocol: u32,
    pub session_revision: u64,
    pub request: CompileRequest,
    pub view: SessionView,
    pub snapshot: Snapshot,
}

#[derive(Debug, Deserialize)]
pub struct SessionViewUpdate {
    pub session_revision: u64,
    pub view: SessionView,
}

#[derive(Debug, Deserialize)]
pub struct DocBody {
    #[serde(default)]
    pub kind: ruddy::artifact::Kind,
    #[serde(default)]
    pub target: Option<ruddy_cli::Target>,
    #[serde(default)]
    pub platform: Option<ruddy_cli::Platform>,
    /// Optional only for compatibility with saves from debugger pages opened
    /// before identity moved into document configuration.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_root")]
    pub root: String,
    #[serde(default)]
    pub run: RunConfig,
    #[serde(default)]
    pub std: StdConfig,
    #[serde(default)]
    pub dependencies: IndexMap<String, DependencySpec>,
    pub files: Vec<FileSpec>,
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
    ///
    /// Which file the span names is not resolved here; see [`Node::at`] and
    /// [`locate`].
    pub fn at(mut self, span: Span) -> Self {
        match span.is_generated() {
            true => self.generated = true,
            false => self.at = Some(span),
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

/// Where a span points, as the page reads it: the file's position in
/// [`Snapshot::files`], and the range inside it.
///
/// `None` for a generated span, and for one naming a file the loader never read
/// — neither is anywhere the reader could be shown.
pub fn loc(span: Span, files: &HashMap<FileID, u32>) -> Option<Loc> {
    match span.is_generated() {
        true => None,
        false => files.get(&span.file_id).map(|&file| Loc {
            file,
            range: [span.start, span.end()],
        }),
    }
}

/// Resolve every node's span into a [`Loc`], now that the file index is known.
///
/// One walk over the finished stages, at the one point that has both the nodes
/// and the index. See [`Node::at`] for why it happens here rather than as each
/// node is built.
pub fn locate(stages: &mut [Stage], files: &HashMap<FileID, u32>) {
    fn walk(nodes: &mut [Node], files: &HashMap<FileID, u32>) {
        for node in nodes {
            node.span = node.at.and_then(|span| loc(span, files));
            walk(&mut node.children, files);
        }
    }
    for stage in stages {
        walk(&mut stage.nodes, files);
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
