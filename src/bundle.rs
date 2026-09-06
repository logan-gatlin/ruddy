//! Reading a whole bundle: the root file, and every file its modules name.
//!
//! A bundle's source consists of a configured root file and the files reached
//! through its module declarations. `module A = ... end` nests a module inline,
//! and `module A` alone says the body is in another file — which is looked for
//! at the path the module's own position in the tree spells. Bundle identity is
//! supplied by the driver from project configuration.
//!
//! What comes out is one spliced statement list, in which every
//! [`StmtKind::Module`] carries a body, so [`ir::build`](crate::ir::build) sees
//! a uniform tree and needs no notion of files at all. Every file is registered
//! with the [`FileManager`] on the way, so every span in the program names the
//! file it was written in.
//!
//! A definition guarded by `@if` is judged here too, against the
//! [`Environment`] the driver supplies: one whose conditions do not hold is
//! dropped before anything under it is read and before lowering mints a name,
//! so two definitions of one name guarded for two targets never meet.
//!
//! Files are reached through the [`Files`] trait rather than through
//! [`std::fs`], so the debugger compiles what is in its editor and a test
//! compiles what is in its own map — and neither has to invent a directory to
//! do it. [`Disk`] is the implementation that reads a real one.

use std::{path::PathBuf, time::Instant};

use crate::{
    parse::{self, Attribute, DataKind, Stmt, StmtKind},
    token::{self, Token},
    tracking::{FileID, FileManager, Span},
};

/// The extension every file of a bundle wears.
const EXTENSION: &str = "hc";

/// The file a module's directory holds its own body in, when its body is not
/// beside the directory instead.
const DIRECTORY_FILE: &str = "module";

/// The metadata key that guards a definition: `@if {target: "js"}`.
const CONDITION_KEY: &str = "if";

/// The one condition a guard may name today.
const TARGET_FIELD: &str = "target";

/// Where a bundle's files come from.
///
/// One method, because that is the whole of what loading needs: nothing here
/// lists a directory — an orphan `.hc` file no module declares is ignored — and
/// nothing writes.
pub trait Files {
    /// The contents of `path` in the caller's logical source tree, always
    /// `/`-separated, or `None` when there is no such file. Module files use
    /// the same coordinate space as the configured root: a root at
    /// `src/main.hc` looks for `src/Math.hc`.
    fn read(&self, path: &str) -> Option<String>;
}

/// [`Files`] over a real directory.
pub struct Disk {
    root: PathBuf,
    sandbox: Option<PathBuf>,
}

/// One file, as it was read.
///
/// Deliberately without the file's statements: they are reachable through the
/// spliced tree, and every span already carries its [`FileID`], so a second
/// copy here would be one more thing that can disagree with the first.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub id: FileID,
    /// The path it was read from in the [`Files`] coordinate space.
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
/// reports it, substitutes an empty body, and keeps going, so one missing file
/// does not hide every other complaint in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A file module whose file is at neither of the two paths it could be at.
    /// Both are carried, because naming them is the whole of the fix.
    ModuleFileMissing { beside: String, inside: String },
    /// A file module whose file is at *both* paths. Which one was meant is not
    /// the loader's to guess.
    ModuleFileAmbiguous { beside: String, inside: String },
    /// `@if` with nothing to judge: no value, unit, or an empty struct. A guard
    /// that guards nothing is far more likely a forgotten condition than a
    /// deliberate truth.
    ConditionMissing,
    /// `@if` whose value is not a struct, such as `@if "js"`.
    ConditionNotStruct,
    /// A condition field the loader does not know. Refused rather than ignored,
    /// so that a condition added later cannot change what a guard written
    /// today means, and so that a misspelling cannot drop a definition.
    ConditionUnknownField { name: String },
    /// A `target` condition whose value is not a string.
    ConditionTargetNotString,
}

/// What a build is for, as far as source can ask: the facts a definition's
/// `@if` is judged against. Supplied by the driver from project configuration,
/// as bundle identity is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// The target of the root project being built, spelled as its manifest
    /// spells it — `js` or `artifact`. A dependency is judged against the
    /// root's target, not its own manifest's, so a library sees the target its
    /// consumer is built for. A name no backend answers to matches nothing,
    /// which lets a guard be written for a backend before it exists.
    pub target: String,
}

/// The whole load, in progress.
struct Loader<'a> {
    files: &'a mut FileManager,
    fs: &'a dyn Files,
    /// Directory of the configured root, including its trailing `/`. Module
    /// paths are relative to this point even when the caller names the root as
    /// `src/main.hc` rather than rooting its [`Files`] there first.
    directory: String,
    environment: &'a Environment,
    out: Output,
}

impl Disk {
    /// Rooted at the base directory paths passed to [`Files::read`] use.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            sandbox: None,
        }
    }

    /// Rooted at `root`, refusing every read whose canonical file is outside
    /// `sandbox`. This is intended for callers compiling untrusted project
    /// configuration: the check happens for each module candidate at the point
    /// it is read, including candidates reached through symlinks.
    pub fn sandboxed(root: impl Into<PathBuf>, sandbox: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            sandbox: Some(sandbox.into()),
        }
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
        if let Some(sandbox) = &self.sandbox {
            let sandbox = std::fs::canonicalize(sandbox).ok()?;
            let canonical = std::fs::canonicalize(&file).ok()?;
            if !canonical.starts_with(&sandbox) {
                return None;
            }
            return std::fs::read_to_string(canonical).ok();
        }
        std::fs::read_to_string(file).ok()
    }
}

/// Read a bundle: the root file, every file its modules name, and one spliced
/// statement list holding all of them.
///
/// The walk always descends — a module's path is strictly longer than its
/// parent's — so no file can be reached twice from below and no cycle is
/// possible. There is nothing here that checks for one, and nothing that needs
/// to be.
pub fn load(
    files: &mut FileManager,
    fs: &dyn Files,
    root: &str,
    environment: &Environment,
) -> Output {
    let mut loader = Loader {
        files,
        fs,
        environment,
        directory: root
            .rsplit_once('/')
            .map_or_else(String::new, |(directory, _)| format!("{directory}/")),
        out: Output {
            stmts: Vec::new(),
            loaded: Vec::new(),
            errors: Vec::new(),
        },
    };
    let mut stmts = loader.file(root);
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
    fn file(&mut self, path: &str) -> Vec<Stmt> {
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

    /// Drop every definition in `stmts` whose guard does not hold, fill in the
    /// body of every file module left, and walk into every module's body once
    /// it has one.
    ///
    /// Guards are judged before the walk, so a file module that is guarded
    /// out is never looked for, and nothing under an excluded inline module
    /// is judged at all.
    ///
    /// `at` is the logical module path of the statements being walked, which is
    /// also the file path a module under them is looked for at: an inline
    /// module contributes a directory component exactly as a file module does,
    /// so `module B` inside `module A = ... end` is looked for at `A/B.hc` and
    /// never at `B.hc`.
    fn splice(&mut self, stmts: &mut Vec<Stmt>, at: &mut Vec<String>) {
        stmts.retain(|stmt| self.holds(stmt));
        for stmt in stmts {
            let StmtKind::Module { name, body } = &mut stmt.kind else {
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
        let beside = format!("{}{}.{EXTENSION}", self.directory, path.join("/"));
        let inside = format!(
            "{}{}/{DIRECTORY_FILE}.{EXTENSION}",
            self.directory,
            path.join("/")
        );
        // A repeated file-module declaration is already an error in lowering,
        // but its body must not be spliced a second time: the repeated copy
        // would turn every declaration in the file into a spurious duplicate.
        // A logical module has exactly these two possible paths, so a loaded
        // *module body* candidate can only be this same module reached earlier.
        // The root is not a module body merely because its configured name
        // collides with one of these paths; it remains a real candidate below.
        if self
            .out
            .loaded
            .iter()
            .skip(1)
            .any(|file| file.path == beside || file.path == inside)
        {
            return Vec::new();
        }
        match (
            self.fs.read(&beside).is_some(),
            self.fs.read(&inside).is_some(),
        ) {
            (true, false) => self.file(&beside),
            (false, true) => self.file(&inside),
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

    /// Whether `stmt` is to be compiled: it carries no `@if`, or the
    /// conditions of its one `@if` all hold.
    ///
    /// A second `@if` is the repeated key lowering refuses; the definition is
    /// kept so that lowering sees it and says so, rather than the guard being
    /// judged by one of two spellings and the definition dropped in silence.
    fn holds(&mut self, stmt: &Stmt) -> bool {
        let mut guards = stmt
            .attributes
            .iter()
            .filter(|attribute| attribute.key.tracked == CONDITION_KEY);
        match (guards.next(), guards.next()) {
            (Some(guard), None) => self.guard_holds(guard),
            _ => true,
        }
    }

    /// Judge one `@if`. A malformed guard is reported and holds, so that the
    /// one complaint is not followed by an unresolved name for everything that
    /// used the definition it guards. A guard with a repeated field holds for
    /// the same reason, and is not reported here: lowering refuses the repeat
    /// on the definition kept, as it does in every metadata struct.
    fn guard_holds(&mut self, guard: &Attribute) -> bool {
        let Some(value) = &guard.value else {
            self.error(guard.span, ErrorKind::ConditionMissing);
            return true;
        };
        let fields = match &value.tracked {
            DataKind::Struct(fields) if !fields.is_empty() => fields,
            DataKind::Struct(_) | DataKind::Unit => {
                self.error(value.span, ErrorKind::ConditionMissing);
                return true;
            }
            _ => {
                self.error(value.span, ErrorKind::ConditionNotStruct);
                return true;
            }
        };
        if fields.keys().enumerate().any(|(at, label)| {
            fields
                .keys()
                .take(at)
                .any(|seen| seen.tracked == label.tracked)
        }) {
            return true;
        }
        let mut holds = true;
        for (label, field) in fields {
            match label.tracked.as_str() {
                TARGET_FIELD => match &field.tracked {
                    DataKind::String(target) => holds &= *target == self.environment.target,
                    _ => self.error(field.span, ErrorKind::ConditionTargetNotString),
                },
                _ => self.error(
                    label.span,
                    ErrorKind::ConditionUnknownField {
                        name: label.tracked.clone(),
                    },
                ),
            }
        }
        holds
    }

    fn error(&mut self, span: Span, kind: ErrorKind) {
        self.out.errors.push(Error { span, kind });
    }
}
