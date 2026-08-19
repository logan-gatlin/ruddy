//! Reading a whole bundle: the root file, and every file its modules name.
//!
//! A bundle is described entirely by its source. The root file opens with
//! `bundle <name> <version>`, `module A = ... end` nests a module inline, and
//! `module A` alone says the body is in another file — which is looked for at
//! the path the module's own position in the tree spells. There is no build
//! metadata anywhere, and nothing outside a `.hc` file decides what is in one.
//!
//! What comes out is one spliced statement list, in which every
//! [`StmtKind::Module`] carries a body, so [`ir::build`](crate::ir::build) sees
//! a uniform tree and needs no notion of files at all. Every file is registered
//! with the [`FileManager`] on the way, so every span in the program names the
//! file it was written in.
//!
//! Files are reached through the [`Files`] trait rather than through
//! [`std::fs`], so the debugger compiles what is in its editor and a test
//! compiles what is in its own map — and neither has to invent a directory to
//! do it. [`Disk`] is the implementation that reads a real one.

use std::{path::PathBuf, time::Instant};

use crate::{
    parse::{self, Stmt, StmtKind},
    symbol::{Bundle, Version},
    token::{self, Kind, Token},
    tracking::{FileID, FileManager, Span},
};

/// The extension every file of a bundle wears.
const EXTENSION: &str = "hc";

/// The file a module's directory holds its own body in, when its body is not
/// beside the directory instead.
const DIRECTORY_FILE: &str = "module";

/// Where a bundle's files come from.
///
/// One method, because that is the whole of what loading needs: nothing here
/// lists a directory — an orphan `.hc` file no module declares is ignored — and
/// nothing writes.
pub trait Files {
    /// The contents of `path`, relative to the bundle root's directory and
    /// always `/`-separated, or `None` when there is no such file.
    fn read(&self, path: &str) -> Option<String>;
}

/// [`Files`] over a real directory.
pub struct Disk {
    root: PathBuf,
}

/// One file, as it was read.
///
/// Deliberately without the file's statements: they are reachable through the
/// spliced tree, and every span already carries its [`FileID`], so a second
/// copy here would be one more thing that can disagree with the first.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub id: FileID,
    /// The path it was read from, relative to the bundle root's directory.
    pub path: String,
    pub tokens: Vec<Token>,
    pub lex_errors: Vec<token::Error>,
    pub parse_errors: Vec<parse::Error>,
    /// How long lexing this file took, in microseconds. Kept per file so a
    /// reporter showing one figure for the phase can add them up; the load as a
    /// whole is the caller's to time.
    pub lex_micros: u64,
    /// How long parsing it took, in microseconds. [`Loaded::lex_micros`]'s
    /// twin.
    pub parse_micros: u64,
}

#[derive(Debug, Clone)]
pub struct Output {
    /// `None` when the header was missing or [`Bundle::new`] refused it; the
    /// caller mints under [`fallback`] so the later phases still run.
    pub bundle: Option<Bundle>,
    /// Every file's statements, spliced: each [`StmtKind::Module`] carries a
    /// body.
    pub stmts: Vec<Stmt>,
    /// Root first, then depth-first in declaration order.
    pub loaded: Vec<Loaded>,
    pub errors: Vec<Error>,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

/// Everything loading can refuse. Every one of them is recoverable: the loader
/// reports it, substitutes an empty body or a fallback identity, and keeps
/// going, so one missing file does not hide every other complaint in the
/// program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A file module whose file is at neither of the two paths it could be at.
    /// Both are carried, because naming them is the whole of the fix.
    ModuleFileMissing { beside: String, inside: String },
    /// A file module whose file is at *both* paths. Which one was meant is not
    /// the loader's to guess.
    ModuleFileAmbiguous { beside: String, inside: String },
    /// A `bundle` header in a file that is not the root.
    MisplacedBundleDeclaration,
    /// A root file that does not open with one.
    MissingBundleDeclaration,
    /// A header [`Bundle::new`] refused. Only the name can be refused: build
    /// metadata is the other reason, and the header grammar cannot spell one.
    BadBundleIdentity,
}

/// The whole load, in progress.
struct Loader<'a> {
    files: &'a mut FileManager,
    fs: &'a dyn Files,
    out: Output,
}

impl Disk {
    /// Rooted at the directory holding the bundle's root file.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Files for Disk {
    fn read(&self, path: &str) -> Option<String> {
        // `/`-separated by contract, which is what a `PathBuf` built one
        // component at a time turns into the platform's own separator.
        let mut file = self.root.clone();
        for segment in path.split('/') {
            file.push(segment);
        }
        std::fs::read_to_string(file).ok()
    }
}

/// The identity a bundle compiles under when its header is missing or refused.
///
/// Written once, here, rather than invented by each driver: a program whose
/// first line is still being typed should mangle the same way in the debugger
/// as it does at the command line, and two fallbacks would be two answers to
/// one question.
pub fn fallback() -> Bundle {
    Bundle::new("fallback", Version::new(0, 0, 0)).expect("the fallback identity is valid")
}

/// Read a bundle: the root file, every file its modules name, and one spliced
/// statement list holding all of them.
///
/// The walk always descends — a module's path is strictly longer than its
/// parent's — so no file can be reached twice from below and no cycle is
/// possible. There is nothing here that checks for one, and nothing that needs
/// to be.
pub fn load(files: &mut FileManager, fs: &dyn Files, root: &str) -> Output {
    let mut loader = Loader {
        files,
        fs,
        out: Output {
            bundle: None,
            stmts: Vec::new(),
            loaded: Vec::new(),
            errors: Vec::new(),
        },
    };
    let mut stmts = loader.file(root, true);
    loader.splice(&mut stmts, &mut Vec::new());
    loader.out.stmts = stmts;
    loader.out
}

impl Loader<'_> {
    /// Read, register, lex and parse one file, and hand back its statements.
    ///
    /// A file that is not there reads as an empty one. Only the root can reach
    /// that: a module's file is looked for before it is read, and the two ways
    /// that can go wrong are complaints of their own.
    fn file(&mut self, path: &str, root: bool) -> Vec<Stmt> {
        let source = self.fs.read(path).unwrap_or_default();
        let id = self
            .files
            .register_new_file(path.to_string(), source.clone());

        let started = Instant::now();
        let lexed = token::lex(&source, id);
        let lex_micros = started.elapsed().as_micros() as u64;

        let started = Instant::now();
        let parsed = parse::parse(lexed.tokens.clone());
        let parse_micros = started.elapsed().as_micros() as u64;

        self.header(&lexed.tokens, parsed.header.as_ref(), id, root);
        self.out.loaded.push(Loaded {
            id,
            path: path.to_string(),
            tokens: lexed.tokens,
            lex_errors: lexed.errors,
            parse_errors: parsed.errors,
            lex_micros,
            parse_micros,
        });
        parsed.stmts
    }

    /// The three rules about a header: the root must have one, no other file
    /// may, and the one the root has must be an identity [`Bundle::new`]
    /// accepts.
    ///
    /// Whether the root *has* one is asked of the tokens rather than of the
    /// parsed header, so a header that was written and did not parse is one
    /// complaint — the parser's — rather than two.
    fn header(&mut self, tokens: &[Token], header: Option<&parse::Header>, id: FileID, root: bool) {
        if !root {
            if let Some(header) = header {
                self.error(header.span, ErrorKind::MisplacedBundleDeclaration);
            }
            return;
        }
        if !matches!(tokens.first().map(|tok| &tok.tracked), Some(Kind::Bundle)) {
            // At the start of the file, which is where the missing line goes.
            self.error(id.span(0, 0), ErrorKind::MissingBundleDeclaration);
        }
        let Some(header) = header else {
            return;
        };
        match Bundle::new(&header.name.tracked, header.version.tracked.clone()) {
            Some(bundle) => self.out.bundle = Some(bundle),
            None => self.error(header.span, ErrorKind::BadBundleIdentity),
        }
    }

    /// Fill in the body of every file module in `stmts`, and walk into every
    /// module's body once it has one.
    ///
    /// `at` is the logical module path of the statements being walked, which is
    /// also the file path a module under them is looked for at: an inline
    /// module contributes a directory component exactly as a file module does,
    /// so `module B` inside `module A = ... end` is looked for at `A/B.hc` and
    /// never at `B.hc`.
    fn splice(&mut self, stmts: &mut [Stmt], at: &mut Vec<String>) {
        for stmt in stmts {
            let StmtKind::Module { name, body } = &mut stmt.tracked else {
                continue;
            };
            at.push(name.tracked.clone());
            match body {
                Some(inner) => self.splice(inner, at),
                None => {
                    let mut inner = self.body(name.span, at);
                    self.splice(&mut inner, at);
                    *body = Some(inner);
                }
            }
            at.pop();
        }
    }

    /// The statements of one file module, read from whichever of its two
    /// candidate paths holds them.
    ///
    /// Exactly one of the two must exist. Neither and both are complaints of
    /// their own, reported at the declaration, and each leaves the module with
    /// an empty body so the rest of the bundle still loads.
    fn body(&mut self, at: Span, path: &[String]) -> Vec<Stmt> {
        let beside = format!("{}.{EXTENSION}", path.join("/"));
        let inside = format!("{}/{DIRECTORY_FILE}.{EXTENSION}", path.join("/"));
        // A repeated file-module declaration is already an error in lowering,
        // but its body must not be spliced a second time: the repeated copy
        // would turn every declaration in the file into a spurious duplicate.
        // A logical module has exactly these two possible paths, so a loaded
        // candidate can only be this same module reached earlier.
        if self
            .out
            .loaded
            .iter()
            .any(|file| file.path == beside || file.path == inside)
        {
            return Vec::new();
        }
        match (
            self.fs.read(&beside).is_some(),
            self.fs.read(&inside).is_some(),
        ) {
            (true, false) => self.file(&beside, false),
            (false, true) => self.file(&inside, false),
            (true, true) => {
                self.error(at, ErrorKind::ModuleFileAmbiguous { beside, inside });
                Vec::new()
            }
            (false, false) => {
                self.error(at, ErrorKind::ModuleFileMissing { beside, inside });
                Vec::new()
            }
        }
    }

    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.out.errors.push(Error { span, kind });
    }
}
