use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileID(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
