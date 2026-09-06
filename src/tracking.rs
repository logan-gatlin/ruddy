use std::collections::HashMap;

use crate::symbol::Symbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileID(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub file_id: FileID,
    pub start: usize,
    pub width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Tracked<T> {
    pub tracked: T,
    pub span: Span,
}

pub type TrackedString = Tracked<String>;

/// One node of a definition's lowered form: where it was written, counted
/// from the definition's own start rather than the file's.
///
/// The number is the node's offset from the start of its definition and its
/// width, packed, so that anchors order the way spans do — by start and then
/// by extent — and two nodes written at one place, a desugaring's pieces or
/// a tuple field's name and its value, are one anchor as they were one span.
/// Counting from the definition's start is what keeps the number the same
/// when the text above the definition moves; an edit inside the definition
/// renumbers what follows it, which is the definition being edited anyway.
/// A generated span, which belongs to no file, is numbered by its absolute
/// position under a flag that keeps it apart from written ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u64);

/// Where something in the semantic program came from, said without a byte
/// offset: which definition, and which node of it.
///
/// Everything after parsing carries one of these where it used to carry a
/// [`Span`]. A span is a fact about the file's bytes, and inserting a space
/// at the top of a file changes every span below it, which made every
/// definition's lowered form a different value whenever whitespace moved.
/// An anchor is a fact about the definition's structure, so the lowered form
/// of a definition nobody edited compares equal to what it was, and only the
/// [`SourceMap`] that turns anchors back into spans has to be read again.
/// Nothing between lowering and the rendering of a diagnostic ever sees a
/// position.
///
/// Ordered by definition and then by node, which is source order within a
/// definition and no order in particular across them, since a symbol is a
/// fingerprint. What wants source order across definitions sorts by an
/// [`Order`], which the program supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Anchor {
    pub definition: Symbol,
    pub node: NodeId,
}

/// A value and the anchor of where it was written: the semantic program's
/// counterpart to [`Tracked`], which is the parser's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchored<T> {
    pub anchored: T,
    pub at: Anchor,
}

pub type AnchoredString = Anchored<String>;

/// Where a program's definitions stand relative to each other, so that what
/// carries anchors can be put in the order a reader meets it. A definition
/// the program never declared — the compiler's own, or a dependency's — sorts
/// after every one it did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Order {
    ranks: HashMap<Symbol, usize>,
}

/// Where every anchor of a program points. Produced beside the lowered
/// program and kept apart from it, so the program is a value about meaning
/// and this is the one about position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    spans: HashMap<Anchor, Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFile {
    pub path: String,
    pub content: String,
}

pub struct FileManager {
    next_file_id: usize,
    files: HashMap<FileID, RegisteredFile>,
}

impl FileID {
    /// Represents a non-existant placeholder file for generated code
    pub const GENERATED: FileID = FileID(0);

    pub const fn is_generated(self) -> bool {
        self.0 == Self::GENERATED.0
    }

    pub const fn span(&self, start: usize, width: usize) -> Span {
        Span::new(*self, start, width)
    }
}

impl Default for Span {
    fn default() -> Self {
        Self {
            file_id: FileID::GENERATED,
            start: Default::default(),
            width: Default::default(),
        }
    }
}

impl Span {
    const fn new(file_id: FileID, start: usize, width: usize) -> Self {
        Self {
            file_id,
            start,
            width,
        }
    }

    pub const fn generated(start: usize, width: usize) -> Self {
        Self::new(FileID::GENERATED, start, width)
    }

    pub const fn is_generated(self) -> bool {
        self.file_id.is_generated()
    }

    /// The exclusive end offset of this span (`start + width`).
    pub const fn end(self) -> usize {
        self.start + self.width
    }

    /// Combine two spans into the smallest span covering both. Assumes both
    /// spans belong to the same file; the receiver's file wins.
    pub const fn merge(self, other: Span) -> Span {
        let start = if self.start < other.start {
            self.start
        } else {
            other.start
        };
        let end = if self.end() > other.end() {
            self.end()
        } else {
            other.end()
        };
        Self {
            file_id: self.file_id,
            start,
            width: end - start,
        }
    }

    pub const fn track<T>(self, t: T) -> Tracked<T> {
        Tracked {
            tracked: t,
            span: self,
        }
    }
}

impl<T> Tracked<T> {
    pub const fn new(tracked: T, span: Span) -> Self {
        Self { tracked, span }
    }

    pub fn into_tracked(self) -> T {
        self.tracked
    }
}

impl NodeId {
    /// The node of what no definition wrote. See [`Anchor::GENERATED`].
    pub const GENERATED: NodeId = NodeId(u64::MAX);

    /// The flag on a node numbered by absolute position, because its span
    /// was generated rather than written into the definition's file.
    const ABSOLUTE: u64 = 1 << 63;

    /// The node of `span` in a definition starting at `base`.
    fn of(base: usize, span: Span) -> Self {
        let width = span.width.min(u32::MAX as usize - 1) as u64;
        if span.is_generated() {
            let start = span.start.min(u32::MAX as usize - 1) as u64;
            return Self(Self::ABSOLUTE | (start << 32) | width);
        }
        let start = span.start.saturating_sub(base).min(u32::MAX as usize - 1) as u64;
        Self((start << 32) | width)
    }
}

impl Anchor {
    /// The anchor of what the compiler made up or a dependency declared,
    /// which resolves to a generated span the way [`Span::default`] is one.
    pub const GENERATED: Anchor = Anchor {
        definition: Symbol::GENERATED,
        node: NodeId::GENERATED,
    };

    pub const fn is_generated(self) -> bool {
        self.node.0 == NodeId::GENERATED.0 || self.node.0 & NodeId::ABSOLUTE != 0
    }

    pub const fn anchor<T>(self, anchored: T) -> Anchored<T> {
        Anchored { anchored, at: self }
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::GENERATED
    }
}

/// An anchored string prints as the source literal that wrote it, the way
/// a tracked one does: quoted, with the escapes the lexer reads.
impl std::fmt::Display for AnchoredString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"")?;
        for character in self.anchored.chars() {
            match character {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                '\n' => f.write_str("\\n")?,
                '\r' => f.write_str("\\r")?,
                '\t' => f.write_str("\\t")?,
                character => write!(f, "{character}")?,
            }
        }
        f.write_str("\"")
    }
}

impl<T> Anchored<T> {
    pub const fn new(anchored: T, at: Anchor) -> Self {
        Self { anchored, at }
    }

    pub fn into_anchored(self) -> T {
        self.anchored
    }
}

impl Order {
    /// The definitions in the order they stand, earliest first.
    pub fn of(definitions: impl IntoIterator<Item = Symbol>) -> Self {
        Self {
            ranks: definitions
                .into_iter()
                .enumerate()
                .map(|(rank, symbol)| (symbol, rank))
                .collect(),
        }
    }

    /// Where `definition` stands: a key that sorts declared definitions the
    /// way they were declared and everything else after them.
    pub fn rank(&self, definition: Symbol) -> usize {
        self.ranks.get(&definition).copied().unwrap_or(usize::MAX)
    }

    /// A sort key for what was written at `at`: its definition's rank and then
    /// its place in the definition.
    pub fn key(&self, at: Anchor) -> (usize, NodeId) {
        (self.rank(at.definition), at.node)
    }
}

impl SourceMap {
    /// Record where a node of `definition`, whose own text starts at `base`,
    /// was written, and answer the anchor that names it.
    pub fn record(&mut self, definition: Symbol, base: usize, span: Span) -> Anchor {
        let at = Anchor {
            definition,
            node: NodeId::of(base, span),
        };
        self.spans.entry(at).or_insert(span);
        at
    }

    /// Where `at` was written: a generated span for a generated anchor, or
    /// for one this map never recorded.
    pub fn span(&self, at: Anchor) -> Span {
        self.spans.get(&at).copied().unwrap_or_default()
    }

    /// Where `at` was written, when it was written somewhere.
    pub fn located(&self, at: Anchor) -> Option<Span> {
        let span = self.span(at);
        (!span.is_generated()).then_some(span)
    }

    /// Take another map's records: the maps of two lowerings of disjoint
    /// definitions, joined.
    pub fn extend(&mut self, other: SourceMap) {
        self.spans.extend(other.spans);
    }
}

impl RegisteredFile {
    fn generated() -> Self {
        Self {
            path: "@GENERATED-CODE".into(),
            content: "".into(),
        }
    }
}

impl Default for FileManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FileManager {
    pub fn new() -> Self {
        let mut files: HashMap<_, _> = Default::default();
        files.insert(FileID::GENERATED, RegisteredFile::generated());
        Self {
            // Start at 1 to avoid dummy file id
            next_file_id: 1,
            files,
        }
    }

    pub fn register_new_file(&mut self, path: String, content: String) -> FileID {
        let rf = RegisteredFile { path, content };
        let id = FileID(self.next_file_id);
        self.next_file_id += 1;
        self.files.insert(id, rf);
        id
    }

    pub fn get_file(&mut self, file_id: FileID) -> &RegisteredFile {
        self.files.get(&file_id).expect("Non-existant FileID")
    }
}
