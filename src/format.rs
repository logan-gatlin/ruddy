//! Printing a file back as canonical source: what `ruddy fmt` writes.
//!
//! An AST pretty-printer over a Wadler-style document. Every node builds a
//! [`Doc`] — text, places a line may break, groups that try to fit on one
//! line before breaking, indentation — and [`print`] lays the document out
//! against [`WIDTH`]. Precedence and parenthesization are the rules the
//! debugger's printer already reads from [`ui`], so the two cannot come to
//! disagree about where a parenthesis goes.
//!
//! Three things the parse tree does not carry come from the source text:
//!
//! - **Comments.** The lexer makes a token of each and the parser splits
//!   them off ([`parse::Output::comments`]). Each is attached to the node it
//!   sits beside — leading, trailing, or dangling at the end of a block — by
//!   walking a skeleton of the tree's spans, and the printers write them
//!   back where the node is printed.
//! - **The author's line breaks.** A construct written across lines stays
//!   across lines; one written on a line stays on a line while it fits. The
//!   signal is a newline in the source between an opener and its first
//!   element, or between elements, and every printer that can break asks
//!   the source for it.
//! - **Literals.** Strings, quoted tags and quoted labels are copied from the
//!   source by span, so the author's escapes survive. Numbers are printed
//!   normalized from their decoded value.
//!
//! Where the parser recovered from an error the tree is missing what it
//! dropped, so a statement any error or dropped region falls inside is copied
//! from the source verbatim rather than printed from the tree, and every
//! dropped region ([`parse::Output::skipped`]) is copied back between the
//! statements around it. Nothing is ever lost to a syntax error.

use std::collections::HashMap;

use crate::{
    parse::{
        self, Annotation, Arg, ArgKind, Arm, ArmHead, Attribute, BinaryOp, Clause, ClauseKind,
        Data, DataKind, EffectBody, EffectLabel, EffectRow, Expr, ExprKind, ExternType,
        ExternTypeKind, HandlerArm, Path, Pattern, PatternKind, Rest, Stmt, StmtKind, SumCase,
        Type, TypeField, TypeKind, UnaryOp, When,
    },
    token::{self, Kind},
    tracking::{FileID, Span, TrackedString},
    ui::{self, Prec},
};

/// The column a line may not run past, comments included.
pub const WIDTH: usize = 100;

/// One level of indentation, in spaces.
pub const INDENT: usize = 2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A file, formatted, with the diagnostics found on the way. The text is
/// complete whatever the errors: what the parser could not read is copied
/// from the source rather than dropped.
#[derive(Debug, Clone)]
pub struct Formatted {
    pub text: String,
    pub lex_errors: Vec<token::Error>,
    pub parse_errors: Vec<parse::Error>,
}

/// What a printer builds: text and the places it may bend.
#[derive(Debug, Clone)]
enum Doc {
    /// Literal text, never containing a newline.
    Text(String),
    /// A single space that never breaks. Its own variant so the printer can
    /// name one without owning a string.
    Space,
    /// A space when the enclosing group is flat, a newline when it breaks.
    Line,
    /// Nothing when flat, a newline when broken.
    SoftLine,
    /// A newline whatever the group decides; breaks every enclosing group.
    HardLine,
    /// Nothing at all, except that every enclosing group breaks.
    BreakParent,
    Concat(Vec<Doc>),
    /// Everything inside indents one level past the enclosing indentation
    /// at each newline.
    Nest(Box<Doc>),
    /// Printed flat when it fits in the remaining width, broken otherwise.
    /// The flag says a forced break inside has already decided it.
    Group(Box<Doc>, bool),
    /// The first when the enclosing group is broken, the second when flat.
    IfBreak(Box<Doc>, Box<Doc>),
    /// A body after `=` or `=>`. Flat, it follows on the same line. Broken,
    /// a *fluid* one moves to a line of its own, one level in, when it fits
    /// there whole; otherwise, and always for a block-shaped one, it stays
    /// on the line when its first line fits there — a match hugging its `=`
    /// lays its arms out under it — and moves down otherwise.
    /// The flag says whether the body counts toward the width of whatever
    /// is in front of it: an arm's pattern breaks to make room for a short
    /// body, while a definition's header does not.
    Hug(Box<Doc>, Layout, bool),
    /// Two layouts of one thing: the first when its first line fits where
    /// the cursor is, the second otherwise. What a call with a braced last
    /// argument uses to hug the brace to the call.
    Choice(Box<Doc>, Box<Doc>),
    /// Lines copied from the source. The first prints where the cursor is;
    /// each later one prints on a line of its own at the current indentation
    /// plus the relative indentation it had in the source.
    Verbatim(Vec<(usize, String)>),
    /// A `\\` raw string's lines, re-indented to the current indentation.
    /// Nothing may follow the last line on its line — the string runs to the
    /// end of it — so the printer moves whatever comes next down a line.
    Raw(Vec<String>),
    /// A run of own-line `--` comments, reflowed to the width when printed
    /// and written one per line. Ends without a newline.
    LineComments(Vec<String>),
    /// An own-line `(* *)` comment, its interior re-indented under the
    /// opener when it spans lines.
    BlockComment(String),
    /// A trailing `--` comment: held until the line ends, then written
    /// after whatever else is on it, wrapped if it overflows.
    LineSuffix(String),
}

/// How a body sits against the `=` or `=>` in front of it when the
/// definition does not fit on one line. Every layout first tries the body
/// whole on the same line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// A block or braced value: stays on the line when its first line fits
    /// there — a match hugging its `=` lays its arms out under it — and
    /// moves down a line otherwise.
    Block,
    /// Anything else after `=`: moves down a line when it fits there whole,
    /// stays on the line when its first line fits there, and moves down
    /// broken otherwise.
    Fluid,
    /// Anything else after an arm's `=>`: moves down a line, whole when it
    /// fits and broken otherwise.
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

#[derive(Clone, Copy)]
struct Cmd<'d> {
    indent: usize,
    mode: Mode,
    doc: &'d Doc,
}

static SPACE: Doc = Doc::Space;
static LINE: Doc = Doc::Line;

#[derive(Debug, Clone)]
struct Comment {
    span: Span,
    text: String,
    block: bool,
    /// Whether the comment prints on a line of its own in front of its node,
    /// as opposed to inline on the node's line. Decided when attached.
    own_line: bool,
}

/// One node of the tree, as a span and its children, for attaching comments.
/// A node that ends in a closing token — `end`, `}`, `)`, `]` — takes the
/// comments written after its last child as its own, to print before the
/// closer. A verbatim node is copied from the source with its comments in
/// it, so none attaches to anything inside.
struct Skel {
    span: Span,
    closer: bool,
    verbatim: bool,
    kids: Vec<Skel>,
}

/// One entry of a statement list: a statement the parser read, or a region
/// it dropped, copied back as written.
#[derive(Clone, Copy)]
enum Item<'a> {
    Stmt(&'a Stmt),
    Skipped(Span),
}

struct Printer<'a> {
    source: &'a str,
    /// Every comment's span, in source order: a newline inside one is not
    /// a break the author made around the code.
    comment_spans: Vec<Span>,
    skipped: &'a [Span],
    /// Where every error and every dropped region starts, sorted.
    problems: Vec<usize>,
    leading: HashMap<Span, Vec<Comment>>,
    trailing: HashMap<Span, Vec<Comment>>,
    dangling: HashMap<Span, Vec<Comment>>,
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

impl Formatted {
    /// Whether the source had any lexical or syntactic error.
    pub fn has_errors(&self) -> bool {
        !self.lex_errors.is_empty() || !self.parse_errors.is_empty()
    }
}

/// Lex, parse and format one file.
pub fn format(source: &str, file: FileID) -> Formatted {
    let lexed = token::lex(source, file);
    let parsed = parse::parse(lexed.tokens);
    let text = format_parsed(source, &parsed, &lexed.errors);
    Formatted {
        text,
        lex_errors: lexed.errors,
        parse_errors: parsed.errors,
    }
}

/// Format a file already parsed. `source` must be the text `parsed` was read
/// from: every span in the tree indexes it.
pub fn format_parsed(source: &str, parsed: &parse::Output, lex_errors: &[token::Error]) -> String {
    let mut problems: Vec<usize> = lex_errors
        .iter()
        .map(|error| error.span.start)
        .chain(parsed.errors.iter().map(|error| error.span.start))
        .chain(parsed.skipped.iter().map(|span| span.start))
        .collect();
    problems.sort_unstable();
    problems.dedup();
    let mut printer = Printer {
        source,
        comment_spans: parsed.comments.iter().map(|comment| comment.span).collect(),
        skipped: &parsed.skipped,
        problems,
        leading: HashMap::new(),
        trailing: HashMap::new(),
        dangling: HashMap::new(),
    };
    // One byte past the source, so no statement that fills the whole file
    // shares the root's span and takes its comments.
    let file = FileID::GENERATED.span(0, source.len() + 1);
    let root = printer.file_skeleton(&parsed.stmts, file);
    for comment in &parsed.comments {
        let (text, block) = match &comment.tracked {
            Kind::LineComment(text) => (text.clone(), false),
            Kind::BlockComment(text) => (text.clone(), true),
            _ => continue,
        };
        printer.attach(
            &root,
            Comment {
                span: comment.span,
                text,
                block,
                own_line: false,
            },
        );
    }
    let mut doc = printer.file(&parsed.stmts, file);
    propagate_breaks(&mut doc);
    let mut text = print(&doc);
    text.truncate(text.trim_end_matches('\n').len());
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// The document and its layout
// ---------------------------------------------------------------------------

fn text(text: impl Into<String>) -> Doc {
    Doc::Text(text.into())
}

fn concat(docs: Vec<Doc>) -> Doc {
    Doc::Concat(docs)
}

fn group(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc), false)
}

/// A group already decided: broken, because the source was.
fn broken(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc), true)
}

fn nest(doc: Doc) -> Doc {
    Doc::Nest(Box::new(doc))
}

fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Box::new(broken), Box::new(flat))
}

fn nil() -> Doc {
    Doc::Concat(Vec::new())
}

/// Join `docs` with `separator` between each pair.
fn join(docs: Vec<Doc>, separator: impl Fn() -> Doc) -> Doc {
    let mut out = Vec::with_capacity(docs.len() * 2);
    for (at, doc) in docs.into_iter().enumerate() {
        if at > 0 {
            out.push(separator());
        }
        out.push(doc);
    }
    concat(out)
}

/// `(doc)` when `parens`, `doc` otherwise.
fn parenthesized(parens: bool, doc: Doc) -> Doc {
    if parens {
        concat(vec![text("("), doc, text(")")])
    } else {
        doc
    }
}

/// Mark every group that contains a forced break as broken, so the printer
/// never tries to lay one out flat. Returns whether `doc` forces a break.
fn propagate_breaks(doc: &mut Doc) -> bool {
    match doc {
        Doc::Text(_)
        | Doc::Space
        | Doc::Line
        | Doc::SoftLine
        | Doc::LineSuffix(_)
        | Doc::BlockComment(_) => false,
        Doc::HardLine | Doc::BreakParent | Doc::LineComments(_) => true,
        // A raw string ends its line whatever the group decides, so the
        // group need not break for it; the printer moves what follows down.
        Doc::Raw(_) => false,
        Doc::Verbatim(lines) => lines.len() > 1,
        Doc::Concat(docs) => {
            let mut forced = false;
            for doc in docs {
                forced |= propagate_breaks(doc);
            }
            forced
        }
        Doc::Nest(inner) | Doc::Hug(inner, ..) => propagate_breaks(inner),
        // A broken group breaks the groups around it, whether a comment
        // forced it or the author wrote it across lines: nothing around it
        // can be on one line either. What stays on the line of an `=` or
        // `=>` in front of it is the hug's decision, not the group's.
        Doc::Group(inner, broken) => {
            *broken |= propagate_breaks(inner);
            *broken
        }
        Doc::IfBreak(broken, flat) | Doc::Choice(broken, flat) => {
            let a = propagate_breaks(broken);
            let b = propagate_breaks(flat);
            a || b
        }
    }
}

fn width_of(text: &str) -> usize {
    text.chars().count()
}

/// Whether the commands in `next`, then the rest of the stack, fit in
/// `width` columns before the first line break in break mode. With
/// `must_be_flat`, a group the author broke does not fit: the question is
/// whether the whole thing goes on one line.
fn fits<'d>(mut next: Vec<Cmd<'d>>, rest: &[Cmd<'d>], width: usize, must_be_flat: bool) -> bool {
    let mut remaining = width as isize;
    let mut rest_at = rest.len();
    loop {
        let Some(cmd) = next.pop() else {
            if rest_at == 0 {
                return true;
            }
            rest_at -= 1;
            next.push(rest[rest_at]);
            continue;
        };
        let Cmd { indent, mode, doc } = cmd;
        match doc {
            Doc::Text(s) => {
                remaining -= width_of(s) as isize;
                if remaining < 0 {
                    return false;
                }
            }
            Doc::Space => {
                remaining -= 1;
                if remaining < 0 {
                    return false;
                }
            }
            Doc::Line => match mode {
                Mode::Flat => {
                    remaining -= 1;
                    if remaining < 0 {
                        return false;
                    }
                }
                Mode::Break => return true,
            },
            Doc::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::HardLine | Doc::LineComments(_) | Doc::Raw(_) => return true,
            Doc::BreakParent | Doc::LineSuffix(_) => {}
            Doc::Concat(docs) => {
                for doc in docs.iter().rev() {
                    next.push(Cmd { indent, mode, doc });
                }
            }
            Doc::Nest(inner) => next.push(Cmd {
                indent: indent + INDENT,
                mode,
                doc: inner,
            }),
            Doc::Group(inner, broken) => {
                if must_be_flat && *broken {
                    return false;
                }
                next.push(Cmd {
                    indent,
                    mode: if *broken { Mode::Break } else { mode },
                    doc: inner,
                });
            }
            Doc::IfBreak(broken, flat) => next.push(Cmd {
                indent,
                mode,
                doc: if mode == Mode::Break { broken } else { flat },
            }),
            // In break mode a body may start a new line right here, so the
            // line being measured can end here — unless the body counts,
            // in which case it is measured as it would be hugged.
            Doc::Hug(inner, _, measured) => match mode {
                Mode::Break if !measured => return true,
                _ => {
                    next.push(Cmd {
                        indent,
                        mode,
                        doc: inner,
                    });
                    next.push(Cmd {
                        indent,
                        mode,
                        doc: &SPACE,
                    });
                }
            },
            // The first form is what will be printed if it fits, and the
            // question being asked is whether it does.
            Doc::Choice(first, _) => next.push(Cmd {
                indent,
                mode,
                doc: first,
            }),
            // A copied region of more than one line breaks the groups around
            // it, so only a one-line one is ever measured.
            Doc::Verbatim(lines) => {
                remaining -= lines.first().map_or(0, |(_, line)| width_of(line)) as isize;
                if remaining < 0 {
                    return false;
                }
            }
            // A block comment printed as one is on a line of its own or
            // spans lines, and either way the line being measured ends.
            Doc::BlockComment(_) => return true,
        }
    }
}

/// Lay a document out against [`WIDTH`].
fn print(doc: &Doc) -> String {
    let mut out = String::new();
    let mut column = 0usize;
    let mut suffixes: Vec<String> = Vec::new();
    let mut pending_newline: Option<usize> = None;
    let mut stack = vec![Cmd {
        indent: 0,
        mode: Mode::Break,
        doc,
    }];

    fn newline(
        out: &mut String,
        column: &mut usize,
        suffixes: &mut Vec<String>,
        indent: usize,
        preserve: bool,
    ) {
        for suffix in suffixes.drain(..) {
            write_suffix(out, column, &suffix);
        }
        // Trailing spaces are never wanted, except on a raw string's line,
        // where they are the string's.
        while !preserve && out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', indent));
        *column = indent;
    }

    while let Some(Cmd { indent, mode, doc }) = stack.pop() {
        let after_raw = pending_newline.is_some();
        if let Some(at) = pending_newline
            && !matches!(
                doc,
                Doc::Line
                    | Doc::SoftLine
                    | Doc::HardLine
                    | Doc::Concat(_)
                    | Doc::Nest(_)
                    | Doc::Group(..)
                    | Doc::IfBreak(..)
                    | Doc::Hug(..)
                    | Doc::Choice(..)
                    | Doc::BreakParent
                    | Doc::LineSuffix(_)
            )
        {
            newline(&mut out, &mut column, &mut suffixes, at, true);
            pending_newline = None;
        }
        match doc {
            Doc::Text(s) => {
                out.push_str(s);
                column += width_of(s);
            }
            Doc::Space => {
                out.push(' ');
                column += 1;
            }
            Doc::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    column += 1;
                }
                Mode::Break => {
                    newline(&mut out, &mut column, &mut suffixes, indent, after_raw);
                    pending_newline = None;
                }
            },
            Doc::SoftLine => {
                if mode == Mode::Break {
                    newline(&mut out, &mut column, &mut suffixes, indent, after_raw);
                    pending_newline = None;
                }
            }
            Doc::HardLine => {
                newline(&mut out, &mut column, &mut suffixes, indent, after_raw);
                pending_newline = None;
            }
            Doc::BreakParent => {}
            Doc::LineSuffix(s) => suffixes.push(s.clone()),
            Doc::Concat(docs) => {
                for doc in docs.iter().rev() {
                    stack.push(Cmd { indent, mode, doc });
                }
            }
            Doc::Nest(inner) => stack.push(Cmd {
                indent: indent + INDENT,
                mode,
                doc: inner,
            }),
            Doc::Group(inner, broken) => {
                let flat = !*broken
                    && (mode == Mode::Flat
                        || fits(
                            vec![Cmd {
                                indent,
                                mode: Mode::Flat,
                                doc: inner,
                            }],
                            &stack,
                            WIDTH.saturating_sub(column),
                            false,
                        ));
                stack.push(Cmd {
                    indent,
                    mode: if flat { Mode::Flat } else { Mode::Break },
                    doc: inner,
                });
            }
            Doc::IfBreak(broken, flat) => stack.push(Cmd {
                indent,
                mode,
                doc: if mode == Mode::Break { broken } else { flat },
            }),
            Doc::Hug(inner, layout, _) => {
                if mode == Mode::Flat {
                    stack.push(Cmd {
                        indent,
                        mode,
                        doc: inner,
                    });
                    stack.push(Cmd {
                        indent,
                        mode,
                        doc: &SPACE,
                    });
                    continue;
                }
                let below = indent + INDENT;
                // Whole on this line first; then, for a fluid body, whole on
                // the next; then its first line here; then down it goes.
                let flat_here = fits(
                    vec![
                        Cmd {
                            indent,
                            mode: Mode::Flat,
                            doc: inner,
                        },
                        Cmd {
                            indent,
                            mode: Mode::Flat,
                            doc: &SPACE,
                        },
                    ],
                    &stack,
                    WIDTH.saturating_sub(column),
                    true,
                );
                if flat_here {
                    stack.push(Cmd {
                        indent,
                        mode: Mode::Flat,
                        doc: inner,
                    });
                    stack.push(Cmd {
                        indent,
                        mode: Mode::Flat,
                        doc: &SPACE,
                    });
                    continue;
                }
                let flat_below = *layout != Layout::Block
                    && fits(
                        vec![Cmd {
                            indent: below,
                            mode: Mode::Flat,
                            doc: inner,
                        }],
                        &stack,
                        WIDTH.saturating_sub(below),
                        true,
                    );
                if flat_below {
                    stack.push(Cmd {
                        indent: below,
                        mode: Mode::Flat,
                        doc: inner,
                    });
                    stack.push(Cmd {
                        indent: below,
                        mode: Mode::Break,
                        doc: &LINE,
                    });
                    continue;
                }
                let hugged = *layout != Layout::Down
                    && fits(
                        vec![
                            Cmd {
                                indent,
                                mode: Mode::Break,
                                doc: inner,
                            },
                            Cmd {
                                indent,
                                mode: Mode::Break,
                                doc: &SPACE,
                            },
                        ],
                        &stack,
                        WIDTH.saturating_sub(column),
                        false,
                    );
                if hugged {
                    stack.push(Cmd {
                        indent,
                        mode,
                        doc: inner,
                    });
                    stack.push(Cmd {
                        indent,
                        mode,
                        doc: &SPACE,
                    });
                } else {
                    stack.push(Cmd {
                        indent: below,
                        mode: Mode::Break,
                        doc: inner,
                    });
                    stack.push(Cmd {
                        indent: below,
                        mode: Mode::Break,
                        doc: &LINE,
                    });
                }
            }
            Doc::Choice(first, second) => {
                let chosen = if mode == Mode::Flat
                    || fits(
                        vec![Cmd {
                            indent,
                            mode: Mode::Break,
                            doc: first,
                        }],
                        &stack,
                        WIDTH.saturating_sub(column),
                        false,
                    ) {
                    first
                } else {
                    second
                };
                stack.push(Cmd {
                    indent,
                    mode,
                    doc: chosen,
                });
            }
            Doc::Verbatim(lines) => {
                for (at, (relative, line)) in lines.iter().enumerate() {
                    if at > 0 {
                        newline(
                            &mut out,
                            &mut column,
                            &mut suffixes,
                            if line.is_empty() {
                                0
                            } else {
                                indent + relative
                            },
                            after_raw,
                        );
                    }
                    out.push_str(line);
                    column += width_of(line);
                }
            }
            Doc::Raw(lines) => {
                for (at, line) in lines.iter().enumerate() {
                    if at > 0 {
                        newline(&mut out, &mut column, &mut suffixes, indent, true);
                    }
                    out.push_str(line);
                    column += width_of(line);
                }
                pending_newline = Some(indent);
            }
            Doc::LineComments(lines) => {
                let available = WIDTH.saturating_sub(indent + 3);
                for (at, line) in reflow(lines, available).into_iter().enumerate() {
                    if at > 0 {
                        newline(&mut out, &mut column, &mut suffixes, indent, after_raw);
                    }
                    let line = if line.is_empty() {
                        "--".to_string()
                    } else {
                        format!("-- {line}")
                    };
                    column += width_of(&line);
                    out.push_str(&line);
                }
            }
            Doc::BlockComment(comment) => {
                let lines = block_comment_lines(comment, WIDTH.saturating_sub(indent + INDENT));
                for (at, (relative, line)) in lines.iter().enumerate() {
                    if at > 0 {
                        newline(
                            &mut out,
                            &mut column,
                            &mut suffixes,
                            if line.is_empty() {
                                0
                            } else {
                                indent + relative
                            },
                            after_raw,
                        );
                    }
                    out.push_str(line);
                    column += width_of(line);
                }
            }
        }
    }
    for suffix in suffixes.drain(..) {
        write_suffix(&mut out, &mut column, &suffix);
    }
    if pending_newline.is_none() {
        out.truncate(out.trim_end_matches(' ').len());
    }
    out
}

/// Write a trailing comment at the end of the current line, wrapping onto
/// continuation lines aligned under its `--` when it runs past the width.
fn write_suffix(out: &mut String, column: &mut usize, suffix: &str) {
    let at = *column + 1;
    let available = WIDTH.saturating_sub(at + 3);
    let text = suffix.trim();
    let lines = if width_of(text) <= available {
        vec![text.to_string()]
    } else {
        fill(text, available)
    };
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', at));
        } else {
            out.push(' ');
        }
        out.push_str("--");
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
    }
    *column = at + 3 + lines.last().map_or(0, |line| width_of(line));
}

/// Fill `text` into lines of at most `width` columns, breaking only at
/// whitespace. A word wider than the width stands alone on its line.
fn fill(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if width_of(&line) + 1 + width_of(word) <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Whether a comment line is kept as written rather than refilled: a blank
/// line, a list item, or a rule.
fn verbatim_comment_line(trimmed: &str) -> bool {
    trimmed.is_empty()
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.chars().all(|c| c == '-' || c == '*' || c == '=')
        || trimmed.split_once(' ').is_some_and(|(marker, _)| {
            marker.len() > 1
                && marker.ends_with('.')
                && marker[..marker.len() - 1]
                    .bytes()
                    .all(|b| b.is_ascii_digit())
        })
}

/// Reflow the text of a run of comment lines. Each input line is the text
/// after the comment's `--`; each output line is text to print after `-- `,
/// or empty for a blank comment line.
///
/// Consecutive unindented lines are a paragraph, refilled to `width`. A
/// paragraph ends at a blank line, at an indented line, or at a list item
/// or rule, each of which is kept verbatim.
fn reflow(lines: &[String], width: usize) -> Vec<String> {
    let normalized: Vec<(usize, String)> = lines
        .iter()
        .map(|line| {
            let line = line.strip_prefix(' ').unwrap_or(line);
            let line = line.trim_end();
            let indent = line.len() - line.trim_start().len();
            (indent, line.trim_start().to_string())
        })
        .collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < normalized.len() {
        let (indent, ref first) = normalized[at];
        // An indented line is a code sample or a diagram: kept as written.
        if indent > 0 || verbatim_comment_line(first) {
            out.push(if first.is_empty() {
                String::new()
            } else {
                format!("{}{first}", " ".repeat(indent))
            });
            at += 1;
            continue;
        }
        let mut paragraph = vec![first.as_str()];
        let mut end = at + 1;
        while end < normalized.len() {
            let (next_indent, ref next) = normalized[end];
            if next_indent > 0 || verbatim_comment_line(next) {
                break;
            }
            paragraph.push(next);
            end += 1;
        }
        out.extend(fill(&paragraph.join(" "), width));
        at = end;
    }
    out
}

/// A block comment on a line with code: as written when it fits on the
/// line, and re-indented under its opener when it spans lines, since a text
/// may not hold a newline.
fn inline_block_comment(comment: &str) -> Doc {
    if comment.contains('\n') {
        Doc::BlockComment(comment.to_string())
    } else {
        text(format!("(*{comment}*)"))
    }
}

/// The lines of an own-line block comment as printed: `(*` and `*)` at the
/// comment's own indentation, the interior one level in and reflowed, or the
/// whole thing untouched when it was written on one line. Each line carries
/// its indentation relative to the comment's.
fn block_comment_lines(comment: &str, width: usize) -> Vec<(usize, String)> {
    if !comment.contains('\n') {
        return vec![(0, format!("(*{comment}*)"))];
    }
    let raw: Vec<&str> = comment
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let first = raw[0].trim();
    // The text on the closer's line is interior text like any other, so a
    // comment closed at the end of its last line reads as one closed on a
    // line of its own. The interior is re-indented under the opener, so
    // whatever indentation it had in common is not content.
    let mut interior: Vec<&str> = raw[1..].to_vec();
    if interior.last().is_some_and(|last| last.trim().is_empty()) {
        interior.pop();
    }
    let common = interior
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let interior: Vec<String> = interior
        .iter()
        .map(|line| format!(" {}", line.get(common..).unwrap_or("").trim_end()))
        .collect();
    let mut lines = vec![(
        0,
        if first.is_empty() {
            "(*".to_string()
        } else {
            format!("(* {first}")
        },
    )];
    for line in reflow(&interior, width) {
        lines.push((INDENT, line));
    }
    lines.push((0, "*)".to_string()));
    lines
}

// ---------------------------------------------------------------------------
// Spans, skeletons and comment attachment
// ---------------------------------------------------------------------------

impl Skel {
    fn leaf(span: Span) -> Self {
        Self::new(span, Vec::new())
    }

    fn new(span: Span, kids: Vec<Skel>) -> Self {
        Self {
            span,
            closer: false,
            verbatim: false,
            kids,
        }
    }

    fn closed(mut self) -> Self {
        self.closer = true;
        self
    }

    fn verbatim(span: Span) -> Self {
        Self {
            span,
            closer: false,
            verbatim: true,
            kids: Vec::new(),
        }
    }
}

impl Item<'_> {
    fn span(&self) -> Span {
        match self {
            Item::Stmt(stmt) => stmt_span(stmt),
            Item::Skipped(span) => *span,
        }
    }
}

/// Where a statement begins, its attributes included.
fn stmt_span(stmt: &Stmt) -> Span {
    match stmt.attributes.first() {
        Some(first) => first.span.merge(stmt.span),
        None => stmt.span,
    }
}

fn arm_span(arm: &Arm) -> Span {
    arm.pattern.span.merge(arm.body.span)
}

fn handler_arm_span(arm: &HandlerArm) -> Span {
    let head = match &arm.head {
        ArmHead::Operation { effect, selector } => effect.span().merge(selector.span),
        ArmHead::Return { span } => *span,
    };
    head.merge(arm.body.span)
}

fn field_span<T>(name: &TrackedString, value: Option<&crate::tracking::Tracked<T>>) -> Span {
    match value {
        Some(value) => name.span.merge(value.span),
        None => name.span,
    }
}

fn when_span(when: &Option<Box<When>>) -> Option<Span> {
    when.as_ref().map(|when| when.span)
}

fn merge_all(base: Span, spans: impl IntoIterator<Item = Span>) -> Span {
    spans.into_iter().fold(base, Span::merge)
}

fn sum_case_span(name: &TrackedString, case: &SumCase) -> Span {
    match case {
        SumCase::Written { when, payload } => merge_all(
            name.span,
            when_span(when)
                .into_iter()
                .chain(payload.as_ref().map(|payload| payload.span)),
        ),
        SumCase::Absent => name.span,
    }
}

fn effect_label_span(path: &Path, label: &EffectLabel) -> Span {
    let (args, when) = match label {
        EffectLabel::Written { args, when } => (args, when_span(when)),
        EffectLabel::Absent { args } => (args, None),
    };
    merge_all(path.span(), args.iter().map(|arg| arg.span).chain(when))
}

// ---------------------------------------------------------------------------
// The printer
// ---------------------------------------------------------------------------

impl<'a> Printer<'a> {
    // -- the source ---------------------------------------------------------

    fn slice(&self, span: Span) -> &'a str {
        &self.source[span.start..span.end()]
    }

    /// Whether the source has a newline in `from..to` outside any comment.
    fn newline_between(&self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        self.source[from..to]
            .match_indices('\n')
            .map(|(offset, _)| from + offset)
            .any(|at| {
                let next = self.comment_spans.partition_point(|span| span.start <= at);
                next == 0 || self.comment_spans[next - 1].end() <= at
            })
    }

    /// Whether the source has a whole blank line in `from..to`.
    fn blank_line_between(&self, from: usize, to: usize) -> bool {
        let Some(gap) = self.source.get(from..to) else {
            return false;
        };
        let mut seen_newline = false;
        for c in gap.chars() {
            match c {
                '\n' if seen_newline => return true,
                '\n' => seen_newline = true,
                c if c.is_whitespace() => {}
                _ => seen_newline = false,
            }
        }
        false
    }

    /// Whether nothing but whitespace precedes `at` on its line.
    fn starts_line(&self, at: usize) -> bool {
        self.source[..at]
            .rsplit('\n')
            .next()
            .is_some_and(|before| before.trim().is_empty())
    }

    /// The column `at` is at on its line.
    fn column_of(&self, at: usize) -> usize {
        let start = self.source[..at].rfind('\n').map_or(0, |at| at + 1);
        width_of(&self.source[start..at])
    }

    /// The indentation of the line `at` is on.
    fn line_indent(&self, at: usize) -> usize {
        let line = self.source[..at].rsplit('\n').next().unwrap_or("");
        line.chars().take_while(|c| c.is_whitespace()).count()
    }

    /// Whether any newline sits in the gaps between consecutive spans, or
    /// between `from` and the first: the author broke this construct.
    fn gaps_have_newline(&self, from: usize, spans: impl IntoIterator<Item = Span>) -> bool {
        let mut at = from;
        for span in spans {
            if self.newline_between(at, span.start) {
                return true;
            }
            at = at.max(span.end());
        }
        false
    }

    /// A region of the source, copied as written: the first line where the
    /// cursor is, the rest re-indented by however much each was indented
    /// past the first.
    fn verbatim(&self, span: Span) -> Doc {
        let base = self.line_indent(span.start);
        let lines = self
            .slice(span)
            .split('\n')
            .enumerate()
            .map(|(at, line)| {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if at == 0 {
                    return (0, line.to_string());
                }
                let indent = line.chars().take_while(|c| c.is_whitespace()).count();
                let body = line.trim_start();
                if body.is_empty() {
                    (0, String::new())
                } else {
                    (indent.saturating_sub(base), body.to_string())
                }
            })
            .collect();
        Doc::Verbatim(lines)
    }

    /// A string literal as written. A `\\` raw string comes back line by
    /// line, since its lines are re-indented rather than copied.
    fn string(&self, span: Span) -> Doc {
        let written = self.slice(span);
        if written.starts_with("\\\\") {
            Doc::Raw(
                written
                    .split('\n')
                    .enumerate()
                    .map(|(at, line)| {
                        let line = line.strip_suffix('\r').unwrap_or(line);
                        if at == 0 { line } else { line.trim_start() }.to_string()
                    })
                    .collect(),
            )
        } else {
            text(written)
        }
    }

    /// A field label as written, with a numeric one normalized.
    fn label(&self, name: &TrackedString) -> Doc {
        let written = self.slice(name.span);
        if written.bytes().all(|b| b.is_ascii_digit()) {
            text(name.tracked.clone())
        } else {
            text(written)
        }
    }

    // -- comments -------------------------------------------------------------

    fn attach(&mut self, skel: &Skel, mut comment: Comment) {
        if skel.verbatim {
            return;
        }
        if let Some(kid) = skel.kids.iter().find(|kid| {
            kid.span.start <= comment.span.start && comment.span.end() <= kid.span.end()
        }) {
            return self.attach(kid, comment);
        }
        let preceding = skel
            .kids
            .iter()
            .rev()
            .find(|kid| kid.span.end() <= comment.span.start);
        let following = skel
            .kids
            .iter()
            .find(|kid| kid.span.start >= comment.span.end());
        // A block comment with nothing but space between it and the node
        // after it on the line belongs to that node — `= (* c *) 1n` — and
        // not to whatever came before the `=`.
        let inline_before_following = following.is_some_and(|following| {
            comment.block
                && self.source[comment.span.end()..following.span.start]
                    .chars()
                    .all(|c| c.is_whitespace() && c != '\n')
        });
        if let Some(preceding) = preceding
            && !inline_before_following
            && !self.newline_between(preceding.span.end(), comment.span.start)
        {
            self.trailing
                .entry(preceding.span)
                .or_default()
                .push(comment);
        } else if let Some(preceding) = preceding
            && !comment.block
            && let Some(last) = self
                .trailing
                .get(&preceding.span)
                .and_then(|comments| comments.last())
            && !last.block
            && self.column_of(last.span.start) == self.column_of(comment.span.start)
            && !self.blank_line_between(last.span.end(), comment.span.start)
        {
            // A line comment directly under a trailing one, aligned with its
            // `--`, continues it: the way an overflowing trailing comment is
            // wrapped, read back as the one comment it was.
            let continued = format!("{} {}", last.text.trim_end(), comment.text.trim());
            let comments = self
                .trailing
                .get_mut(&preceding.span)
                .expect("the trailing comment was found above");
            let last = comments
                .last_mut()
                .expect("the trailing comment was found above");
            last.text = continued;
            last.span = last.span.merge(comment.span);
        } else if let Some(following) = following {
            comment.own_line = !comment.block
                || self.starts_line(comment.span.start)
                || self.newline_between(comment.span.end(), following.span.start);
            self.leading
                .entry(following.span)
                .or_default()
                .push(comment);
        } else {
            comment.own_line = true;
            self.dangling.entry(skel.span).or_default().push(comment);
        }
    }

    /// The comments in front of a node, each on its own line unless it was
    /// inline, with the blank lines the author left between them.
    fn leading_docs(&self, span: Span) -> Vec<Doc> {
        let Some(comments) = self.leading.get(&span) else {
            return Vec::new();
        };
        let mut parts = Vec::new();
        let mut at = 0;
        while at < comments.len() {
            let comment = &comments[at];
            if !comment.own_line {
                parts.push(inline_block_comment(&comment.text));
                parts.push(Doc::Space);
                at += 1;
                continue;
            }
            let mut last_end = comment.span.end();
            if comment.block {
                parts.push(Doc::BlockComment(comment.text.clone()));
                at += 1;
            } else {
                let mut run = vec![comment.text.clone()];
                at += 1;
                while at < comments.len()
                    && !comments[at].block
                    && comments[at].own_line
                    && !self.blank_line_between(last_end, comments[at].span.start)
                {
                    run.push(comments[at].text.clone());
                    last_end = comments[at].span.end();
                    at += 1;
                }
                parts.push(Doc::LineComments(run));
            }
            parts.push(Doc::HardLine);
            let next = comments.get(at).map_or(span.start, |next| next.span.start);
            if self.blank_line_between(last_end, next) {
                parts.push(Doc::HardLine);
            }
        }
        parts
    }

    /// The comments after a node on its line.
    fn trailing_docs(&self, span: Span) -> Vec<Doc> {
        let Some(comments) = self.trailing.get(&span) else {
            return Vec::new();
        };
        comments
            .iter()
            .flat_map(|comment| {
                if comment.block {
                    vec![Doc::Space, inline_block_comment(&comment.text)]
                } else {
                    vec![Doc::LineSuffix(comment.text.clone()), Doc::BreakParent]
                }
            })
            .collect()
    }

    /// The comments at the end of a block, in front of its closer: a line
    /// break, then each on a line of its own.
    fn dangling_docs(&self, span: Span) -> Doc {
        concat(self.dangling_parts(span))
    }

    /// [`dangling_docs`](Self::dangling_docs) as the parts it is made of.
    fn dangling_parts(&self, span: Span) -> Vec<Doc> {
        let Some(comments) = self.dangling.get(&span) else {
            return Vec::new();
        };
        let mut parts = Vec::new();
        let mut at = 0;
        while at < comments.len() {
            let comment = &comments[at];
            parts.push(Doc::HardLine);
            if comment.block {
                parts.push(Doc::BlockComment(comment.text.clone()));
                at += 1;
                continue;
            }
            let mut run = vec![comment.text.clone()];
            let mut last_end = comment.span.end();
            at += 1;
            while at < comments.len()
                && !comments[at].block
                && !self.blank_line_between(last_end, comments[at].span.start)
            {
                run.push(comments[at].text.clone());
                last_end = comments[at].span.end();
                at += 1;
            }
            parts.push(Doc::LineComments(run));
        }
        parts
    }

    fn with_comments(&self, span: Span, doc: Doc) -> Doc {
        let mut parts = self.leading_docs(span);
        parts.push(doc);
        parts.extend(self.trailing_docs(span));
        concat(parts)
    }

    /// Where a node's text begins and ends once its comments are counted,
    /// for the blank lines between it and its neighbours.
    fn extent(&self, span: Span) -> (usize, usize) {
        let start = self
            .leading
            .get(&span)
            .and_then(|comments| comments.first())
            .map_or(span.start, |comment| comment.span.start);
        let end = self
            .trailing
            .get(&span)
            .and_then(|comments| comments.last())
            .map_or(span.end(), |comment| comment.span.end());
        (start, end)
    }

    // -- statement lists ------------------------------------------------------

    /// The items of one statement list: its statements and the regions the
    /// parser dropped between them, in source order. A dropped region inside
    /// one of the statements, or inside `result`, belongs to a list further
    /// in and is left to it.
    fn items<'s>(&self, stmts: &'s [Stmt], result: Option<Span>, region: Span) -> Vec<Item<'s>> {
        let mut items: Vec<Item<'s>> = stmts.iter().map(Item::Stmt).collect();
        for &span in self.skipped {
            let inside = span.start >= region.start && span.end() <= region.end();
            let owned = stmts.iter().any(|stmt| contains(stmt_span(stmt), span))
                || result.is_some_and(|result| contains(result, span));
            if inside && !owned {
                items.push(Item::Skipped(span));
            }
        }
        items.sort_by_key(|item| item.span().start);
        items
    }

    /// Whether a statement is printed from the source rather than the tree:
    /// an error or a dropped region falls in its region — from the end of
    /// the item before it to its own end — and not inside a statement list
    /// nested in it, which answers for its own items.
    fn opaque(&self, stmt: &Stmt, region_start: usize) -> bool {
        let end = stmt.span.end();
        let first = self.problems.partition_point(|&at| at < region_start);
        let inside = &self.problems[first..];
        if inside.first().is_none_or(|&at| at >= end) {
            return false;
        }
        let nested = self.nested_lists(stmt);
        inside
            .iter()
            .take_while(|&&at| at < end)
            .any(|&at| !nested.iter().any(|(from, to)| at >= *from && at < *to))
    }

    /// The regions of every statement list nested in a statement: from
    /// where the list's items may begin to where its last one ends.
    fn nested_lists(&self, stmt: &Stmt) -> Vec<(usize, usize)> {
        let mut lists = Vec::new();
        self.collect_lists(stmt, &mut lists);
        lists
    }

    fn collect_lists(&self, stmt: &Stmt, lists: &mut Vec<(usize, usize)>) {
        match &stmt.kind {
            StmtKind::Module {
                name,
                body: Some(body),
            } => {
                let start = name.span.end();
                let items = self.items(body, None, stmt.span);
                if let Some(last) = items.last() {
                    lists.push((start, last.span().end()));
                }
                for inner in body {
                    self.collect_lists(inner, lists);
                }
            }
            StmtKind::Let { body, .. } => self.collect_expr_lists(&body.tracked, lists),
            _ => {}
        }
    }

    fn collect_expr_lists(&self, expr: &Expr, lists: &mut Vec<(usize, usize)>) {
        if let ExprKind::Do { stmts, result } = &expr.tracked {
            let items = self.items(stmts, result.as_ref().map(|result| result.span), expr.span);
            if let Some(last) = items.last() {
                lists.push((expr.span.start + 2, last.span().end()));
            }
            for inner in stmts {
                self.collect_lists(inner, lists);
            }
        }
        for child in expr_children(expr) {
            self.collect_expr_lists(child, lists);
        }
    }

    /// The docs of a statement list, separated by `separator` and by one
    /// more line wherever the author left a blank one.
    fn list(&self, items: &[Item<'_>], list_start: usize, separator: Doc) -> Vec<Doc> {
        let mut parts = Vec::new();
        let mut region_start = list_start;
        let mut previous_end: Option<usize> = None;
        for item in items {
            let span = item.span();
            let doc = match item {
                Item::Stmt(stmt) => {
                    if self.opaque(stmt, region_start) {
                        self.verbatim(span)
                    } else {
                        self.stmt(stmt)
                    }
                }
                Item::Skipped(span) => self.verbatim(*span),
            };
            let (start, end) = self.extent(span);
            if let Some(previous_end) = previous_end {
                parts.push(separator.clone());
                if self.blank_line_between(previous_end, start) {
                    parts.push(Doc::HardLine);
                }
            }
            parts.push(self.with_comments(span, doc));
            region_start = span.end();
            previous_end = Some(end);
        }
        parts
    }

    fn file(&self, stmts: &[Stmt], file: Span) -> Doc {
        let items = self.items(stmts, None, file);
        let mut parts = self.list(&items, 0, Doc::HardLine);
        let mut dangling = self.dangling_parts(file);
        // Nothing but comments: the break in front of the first is not
        // wanted, since there is nothing for it to separate from.
        if parts.is_empty() && !dangling.is_empty() {
            dangling.remove(0);
        }
        parts.extend(dangling);
        concat(parts)
    }

    // -- the skeleton ---------------------------------------------------------

    fn file_skeleton(&self, stmts: &[Stmt], file: Span) -> Skel {
        let kids = self
            .items(stmts, None, file)
            .into_iter()
            .map(|item| match item {
                Item::Stmt(stmt) => self.stmt_skeleton(stmt),
                Item::Skipped(span) => Skel::verbatim(span),
            })
            .collect();
        Skel::new(file, kids).closed()
    }

    /// The span of the keyword a statement begins with, after its
    /// attributes: what a comment between them attaches to. The `_ =`
    /// shorthand has no keyword; its pattern takes those comments instead.
    fn keyword_span(&self, stmt: &Stmt) -> Option<Span> {
        let keyword = self.source[stmt.span.start..]
            .split(|c: char| !c.is_alphabetic())
            .next()
            .unwrap_or("");
        (!keyword.is_empty()).then(|| stmt.span.file_id.span(stmt.span.start, keyword.len()))
    }

    fn stmt_skeleton(&self, stmt: &Stmt) -> Skel {
        let mut kids: Vec<Skel> = stmt.attributes.iter().map(attribute_skeleton).collect();
        kids.extend(self.keyword_span(stmt).map(Skel::leaf));
        let mut closer = false;
        match &stmt.kind {
            StmtKind::Extern {
                ty, abi, target, ..
            } => {
                kids.push(extern_type_skeleton(abi));
                kids.extend(clauses_skeleton(ty));
                kids.push(Skel::leaf(target.span));
            }
            StmtKind::Let { pattern, ty, body } => {
                kids.push(pattern_skeleton(pattern));
                if let Some(ty) = ty {
                    kids.push(type_skeleton(&ty.ty));
                    kids.extend(clauses_skeleton(ty));
                }
                kids.push(self.expr_skeleton(&body.tracked));
            }
            StmtKind::Type { params, body, .. } => {
                kids.extend(params.iter().map(|param| Skel::leaf(param.span)));
                kids.push(type_skeleton(&body.ty));
                kids.extend(clauses_skeleton(body));
            }
            StmtKind::Effect { params, body, .. } => {
                kids.extend(params.iter().map(|param| Skel::leaf(param.span)));
                match body {
                    EffectBody::Empty => {}
                    EffectBody::Alias(row) => kids.extend(row_skeleton(row)),
                    EffectBody::Unnamed { signature } => kids.push(type_skeleton(signature)),
                    EffectBody::Named(fields) => {
                        closer = true;
                        kids.extend(fields.iter().map(|(name, signature)| {
                            Skel::new(
                                field_span(name, Some(signature.as_ref())),
                                vec![type_skeleton(signature)],
                            )
                        }));
                    }
                }
            }
            StmtKind::Module { body, .. } => {
                if let Some(body) = body {
                    closer = true;
                    kids.extend(self.items(body, None, stmt.span).into_iter().map(
                        |item| match item {
                            Item::Stmt(stmt) => self.stmt_skeleton(stmt),
                            Item::Skipped(span) => Skel::verbatim(span),
                        },
                    ));
                }
            }
        }
        let skel = Skel::new(stmt_span(stmt), kids);
        if closer { skel.closed() } else { skel }
    }

    // -- statements -----------------------------------------------------------

    fn stmt(&self, stmt: &Stmt) -> Doc {
        let mut parts = Vec::new();
        for attribute in &stmt.attributes {
            parts.push(self.attribute(attribute));
            parts.push(Doc::HardLine);
        }
        // A comment between the attributes and the keyword stays there.
        if let Some(keyword) = self.keyword_span(stmt) {
            parts.extend(self.leading_docs(keyword));
        }
        parts.push(match &stmt.kind {
            StmtKind::Extern {
                name,
                ty,
                abi,
                target,
            } => {
                let mut header = vec![
                    text("extern "),
                    text(name.tracked.clone()),
                    text(": "),
                    self.extern_type(abi),
                ];
                header.extend(self.where_clause(ty));
                group(concat(vec![
                    group(concat(header)),
                    text(" ="),
                    Doc::Hug(
                        Box::new(self.with_comments(target.span, self.string(target.span))),
                        Layout::Block,
                        false,
                    ),
                ]))
            }
            StmtKind::Let { pattern, ty, body } => {
                let mut header = Vec::new();
                if stmt.span.start < pattern.span.start {
                    header.push(text("let "));
                }
                header.push(self.pattern(pattern));
                let mut header_end = pattern.span.end();
                let mut header_signal = false;
                if let Some(ty) = ty {
                    header_end = ty.span().end();
                    header_signal = self.newline_between(pattern.span.end(), ty.ty.span.start);
                    let mut ascription = vec![Doc::Line, self.ty(&ty.ty)];
                    ascription.extend(self.where_clause(ty));
                    header.push(text(":"));
                    header.push(nest(concat(ascription)));
                }
                let body_doc = self.expr(&body.tracked);
                let signal = self.newline_between(header_end, body.span.start);
                let body_part = if signal {
                    nest(concat(vec![Doc::Line, body_doc]))
                } else {
                    Doc::Hug(Box::new(body_doc), layout_of(&body.tracked), false)
                };
                let header = if header_signal {
                    broken(concat(header))
                } else {
                    group(concat(header))
                };
                let doc = concat(vec![header, text(" ="), body_part]);
                if signal { broken(doc) } else { group(doc) }
            }
            StmtKind::Type { name, params, body } => {
                let mut header = vec![text("type "), text(name.tracked.clone())];
                let mut header_end = name.span.end();
                for param in params {
                    header.push(Doc::Space);
                    header.push(self.with_comments(param.span, text(self.slice(param.span))));
                    header_end = param.span.end();
                }
                header.push(text(" ="));
                let signal = self.newline_between(header_end, body.ty.span.start);
                if hugs_type(&body.ty) && !signal && body.clause.is_none() {
                    header.push(Doc::Hug(Box::new(self.ty(&body.ty)), Layout::Block, false));
                    concat(header)
                } else {
                    let mut annotation = vec![Doc::Line, self.ty(&body.ty)];
                    annotation.extend(self.where_clause(body));
                    header.push(nest(concat(annotation)));
                    let doc = concat(header);
                    if signal { broken(doc) } else { group(doc) }
                }
            }
            StmtKind::Effect { name, params, body } => {
                let mut header = vec![text("effect "), text(name.tracked.clone())];
                let mut header_end = name.span.end();
                for param in params {
                    header.push(Doc::Space);
                    header.push(self.with_comments(param.span, text(self.slice(param.span))));
                    header_end = param.span.end();
                }
                match body {
                    EffectBody::Empty => concat(header),
                    EffectBody::Alias(row) => {
                        header.push(text(" ="));
                        let signal = self.newline_between(header_end, row.span.start);
                        header.push(nest(concat(vec![Doc::Line, self.effect_row(row)])));
                        let doc = concat(header);
                        if signal { broken(doc) } else { group(doc) }
                    }
                    EffectBody::Unnamed { signature } => {
                        header.push(text(" ="));
                        let signal = self.newline_between(header_end, signature.span.start);
                        header.push(nest(concat(vec![Doc::Line, self.ty(signature)])));
                        let doc = concat(header);
                        if signal { broken(doc) } else { group(doc) }
                    }
                    EffectBody::Named(fields) => {
                        header.push(text(" = "));
                        let open = self.source[header_end..]
                            .find('{')
                            .map_or(header_end, |at| header_end + at);
                        let entries: Vec<Doc> = fields
                            .iter()
                            .map(|(name, signature)| {
                                self.with_comments(
                                    field_span(name, Some(signature.as_ref())),
                                    concat(vec![self.label(name), text(": "), self.ty(signature)]),
                                )
                            })
                            .collect();
                        let signal = self.gaps_have_newline(
                            open,
                            fields.iter().map(|(name, signature)| {
                                field_span(name, Some(signature.as_ref()))
                            }),
                        );
                        header.push(self.braces(
                            entries,
                            true,
                            self.dangling_docs(stmt_span(stmt)),
                            signal,
                        ));
                        concat(header)
                    }
                }
            }
            StmtKind::Module { name, body } => {
                let mut parts = vec![text("module "), text(name.tracked.clone())];
                if let Some(body) = body {
                    parts.push(text(" ="));
                    let items = self.items(body, None, stmt.span);
                    let signal = self
                        .gaps_have_newline(name.span.end(), items.iter().map(|item| item.span()))
                        || self.newline_between(
                            items
                                .last()
                                .map_or(name.span.end(), |item| item.span().end()),
                            stmt.span.end(),
                        );
                    let mut inner = Vec::new();
                    for doc in self.list(&items, name.span.end(), Doc::Line) {
                        inner.push(doc);
                    }
                    let mut body_parts = Vec::new();
                    if !inner.is_empty() {
                        body_parts.push(Doc::Line);
                        body_parts.extend(inner);
                    }
                    parts.push(nest(concat(body_parts)));
                    parts.push(nest(self.dangling_docs(stmt_span(stmt))));
                    parts.push(Doc::Line);
                    parts.push(text("end"));
                    let doc = concat(parts);
                    if signal { broken(doc) } else { group(doc) }
                } else {
                    concat(parts)
                }
            }
        });
        concat(parts)
    }

    fn attribute(&self, attribute: &Attribute) -> Doc {
        let mut parts = vec![text(self.slice(attribute.key.span))];
        if let Some(value) = &attribute.value {
            parts.push(Doc::Space);
            parts.push(self.data(value));
        }
        self.with_comments(attribute.span, concat(parts))
    }

    /// The `where` clause of an annotation, on a line of its own when the
    /// annotation breaks, or nothing when none was written.
    fn where_clause(&self, annotation: &Annotation) -> Vec<Doc> {
        let Some(clause) = &annotation.clause else {
            return Vec::new();
        };
        let clauses: Vec<Doc> = clause
            .clauses
            .iter()
            .map(|clause| self.with_comments(clause.span, self.clause(&clause.tracked, 0)))
            .collect();
        vec![Doc::Line, text("where "), join(clauses, || text("; "))]
    }

    /// A `where` clause's formula, with the parentheses re-parsing needs, by
    /// the ladder the debugger's printer keeps: `0` for a comparison, `1`
    /// for `or`, `2` for `and`, `3` for `not`, `4` for a name.
    fn clause(&self, clause: &ClauseKind, level: u8) -> Doc {
        fn prec(clause: &ClauseKind) -> u8 {
            match clause {
                ClauseKind::Equal(..) | ClauseKind::NotEqual(..) => 0,
                ClauseKind::Or(..) => 1,
                ClauseKind::And(..) => 2,
                ClauseKind::Not(_) => 3,
                ClauseKind::Name(_) => 4,
            }
        }
        let parens = prec(clause) < level;
        let side = |clause: &Clause, level: u8| {
            self.with_comments(clause.span, self.clause(&clause.tracked, level))
        };
        let doc = match clause {
            ClauseKind::Name(name) => text(format!("'{name}")),
            ClauseKind::Not(inner) => concat(vec![text("not "), side(inner, 3)]),
            ClauseKind::And(left, right) => {
                concat(vec![side(left, 2), text(" and "), side(right, 3)])
            }
            ClauseKind::Or(left, right) => {
                concat(vec![side(left, 1), text(" or "), side(right, 2)])
            }
            ClauseKind::Equal(left, right) => {
                concat(vec![side(left, 1), text(" = "), side(right, 1)])
            }
            ClauseKind::NotEqual(left, right) => {
                concat(vec![side(left, 1), text(" != "), side(right, 1)])
            }
        };
        parenthesized(parens, doc)
    }

    // -- delimited lists ------------------------------------------------------

    /// `{ a, b }`, broken one entry per line with a trailing comma when the
    /// author broke it or it does not fit. `comma` says whether a trailing
    /// comma may be written.
    fn braces(&self, entries: Vec<Doc>, comma: bool, dangling: Doc, signal: bool) -> Doc {
        if entries.is_empty() {
            return match dangling {
                Doc::Concat(docs) if docs.is_empty() => text("{}"),
                dangling => concat(vec![text("{"), nest(dangling), Doc::HardLine, text("}")]),
            };
        }
        let doc = concat(vec![
            text("{"),
            nest(concat(vec![
                Doc::Line,
                join(entries, || concat(vec![text(","), Doc::Line])),
                if comma {
                    if_break(text(","), nil())
                } else {
                    nil()
                },
                dangling,
            ])),
            Doc::Line,
            text("}"),
        ]);
        if signal { broken(doc) } else { group(doc) }
    }

    /// `(a, b)` or `[a, b]`: the brackets around `entries` with nothing
    /// inside them when flat and one entry per line when broken. A
    /// singleton tuple always keeps the comma that makes it one.
    fn brackets(
        &self,
        open: &str,
        close: &str,
        entries: Vec<Doc>,
        trailing: Doc,
        dangling: Doc,
        signal: bool,
    ) -> Doc {
        if entries.is_empty() {
            return match dangling {
                Doc::Concat(docs) if docs.is_empty() => text(format!("{open}{close}")),
                dangling => concat(vec![text(open), nest(dangling), Doc::HardLine, text(close)]),
            };
        }
        let doc = concat(vec![
            text(open),
            nest(concat(vec![
                Doc::SoftLine,
                join(entries, || concat(vec![text(","), Doc::Line])),
                trailing,
                dangling,
            ])),
            Doc::SoftLine,
            text(close),
        ]);
        if signal { broken(doc) } else { group(doc) }
    }

    // -- expressions ----------------------------------------------------------

    fn expr(&self, expr: &Expr) -> Doc {
        self.with_comments(expr.span, self.expr_kind(expr))
    }

    /// `expr`, parenthesized when `parens`.
    fn expr_in(&self, expr: &Expr, parens: bool) -> Doc {
        parenthesized(parens, self.expr(expr))
    }

    fn expr_kind(&self, expr: &Expr) -> Doc {
        let prec = |expr: &Expr| ui::expr_prec(&expr.tracked);
        match &expr.tracked {
            ExprKind::Pipe { .. } => {
                // The chain is left-nested: `a |> f |> g` is `(a |> f) |> g`.
                let mut steps = Vec::new();
                let mut current = expr;
                while let ExprKind::Pipe { value, function } = &current.tracked {
                    steps.push((current, function.as_ref()));
                    current = value;
                }
                steps.reverse();
                let first = current;
                let signal = self.gaps_have_newline(
                    first.span.end(),
                    steps.iter().map(|(_, function)| function.span),
                );
                // A comment in front of an operand goes in front of its
                // operator; each inner node of the chain ends at its operand,
                // so its trailing comments go after that operand.
                let mut rest = Vec::new();
                for (at, (node, function)) in steps.iter().enumerate() {
                    rest.push(Doc::Line);
                    rest.extend(self.leading_docs(function.span));
                    rest.push(text("|> "));
                    rest.push(self.expr_after(function, prec(function) <= Prec::Pipeline));
                    if at + 1 < steps.len() {
                        rest.extend(self.trailing_docs(node.span));
                    }
                }
                let doc = concat(vec![
                    self.expr_in(first, prec(first) < Prec::Pipeline),
                    nest(concat(rest)),
                ]);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Unary { op, value } => {
                let parens = prec(value) < Prec::Unary;
                let symbol = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "not ",
                    UnaryOp::Allocate => "mut ",
                    UnaryOp::Read => "~",
                };
                // Two minus signs would lex as a comment, and a minus against
                // a digit as a signed literal, so a space keeps them apart.
                let spaced = matches!(op, UnaryOp::Neg)
                    && !parens
                    && matches!(
                        value.tracked,
                        ExprKind::Unary {
                            op: UnaryOp::Neg,
                            ..
                        } | ExprKind::Natural(_)
                            | ExprKind::Integer(_)
                            | ExprKind::Fixed(_)
                            | ExprKind::Real(_)
                    );
                concat(vec![
                    text(symbol),
                    if spaced { Doc::Space } else { nil() },
                    self.expr_in(value, parens),
                ])
            }
            ExprKind::Binary { .. } => {
                let level = prec(expr);
                let symbol = |op: &BinaryOp| match op {
                    BinaryOp::Write => ":=",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::And => "and",
                    BinaryOp::Or => "or",
                    BinaryOp::Xor => "xor",
                };
                if let ExprKind::Binary {
                    op: BinaryOp::Write,
                    left,
                    right,
                } = &expr.tracked
                {
                    let signal = self.newline_between(left.span.end(), right.span.start);
                    let mut rest = vec![Doc::Line];
                    rest.extend(self.leading_docs(right.span));
                    rest.push(text(format!("{} ", symbol(&BinaryOp::Write))));
                    rest.push(self.expr_after(right, prec(right) < level));
                    let doc = concat(vec![
                        self.expr_in(left, prec(left) <= level),
                        nest(concat(rest)),
                    ]);
                    return if signal { broken(doc) } else { group(doc) };
                }
                // A left-associative chain at one level is laid out as one
                // run, breaking before each operator.
                let mut operands = Vec::new();
                let mut current = expr;
                while let ExprKind::Binary { op, left, right } = &current.tracked
                    && prec(current) == level
                {
                    operands.push((current, *op, right.as_ref()));
                    current = left;
                }
                operands.reverse();
                let first = current;
                let signal = self.gaps_have_newline(
                    first.span.end(),
                    operands.iter().map(|(_, _, right)| right.span),
                );
                let mut rest = Vec::new();
                for (at, (node, op, right)) in operands.iter().enumerate() {
                    rest.push(Doc::Line);
                    rest.extend(self.leading_docs(right.span));
                    rest.push(text(format!("{} ", symbol(op))));
                    rest.push(self.expr_after(right, prec(right) <= level));
                    if at + 1 < operands.len() {
                        rest.extend(self.trailing_docs(node.span));
                    }
                }
                let doc = concat(vec![
                    self.expr_in(first, prec(first) < level),
                    nest(concat(rest)),
                ]);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Apply { .. } => {
                let mut args = Vec::new();
                let mut current = expr;
                while let ExprKind::Apply { func, arg } = &current.tracked {
                    args.push((current, arg.as_ref()));
                    current = func;
                }
                args.reverse();
                let head = current;
                let signal =
                    self.gaps_have_newline(head.span.end(), args.iter().map(|(_, arg)| arg.span));
                // The last argument alone may be a bare tag: nothing follows
                // an application that could be read as the tag's payload.
                let arg_doc = |at: usize, arg: &Expr| {
                    let bare_tag = at + 1 == args.len()
                        && matches!(arg.tracked, ExprKind::Tag { payload: None, .. });
                    let mut parts = self.leading_docs(arg.span);
                    parts.push(self.expr_after(arg, prec(arg) < Prec::Atom && !bare_tag));
                    concat(parts)
                };
                let head_doc = self.expr_in(head, prec(head) < Prec::Apply);
                let mut rest = Vec::new();
                for (at, (node, arg)) in args.iter().enumerate() {
                    rest.push(Doc::Line);
                    rest.push(arg_doc(at, arg));
                    if at + 1 < args.len() {
                        rest.extend(self.trailing_docs(node.span));
                    }
                }
                let expanded = concat(vec![head_doc.clone(), nest(concat(rest))]);
                if signal {
                    return broken(expanded);
                }
                // A braced last argument hugs the call, its contents one per
                // line under the head, when the head and the other
                // arguments fit on the line in front of its brace.
                let last = args.last().expect("an application has an argument").1;
                if delimited(last) {
                    let mut hugged = vec![head_doc];
                    for (at, (node, arg)) in args.iter().enumerate() {
                        hugged.push(Doc::Space);
                        hugged.push(arg_doc(at, arg));
                        if at + 1 < args.len() {
                            hugged.extend(self.trailing_docs(node.span));
                        }
                    }
                    return group(Doc::Choice(Box::new(concat(hugged)), Box::new(expanded)));
                }
                group(expanded)
            }
            ExprKind::Function { args, body } => {
                let mut header = vec![text("fn")];
                for arg in args {
                    header.push(Doc::Space);
                    header.push(self.arg(arg));
                }
                header.push(text(" =>"));
                let signal = self.newline_between(
                    args.last().map_or(expr.span.start, |arg| arg.span.end()),
                    body.span.start,
                );
                let body_doc = self.expr(body);
                let body_part = if signal {
                    nest(concat(vec![Doc::Line, body_doc]))
                } else {
                    Doc::Hug(Box::new(body_doc), layout_of(body), false)
                };
                let doc = concat(vec![concat(header), body_part]);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::MatchFunction { fn_span, arms } => {
                let signal = self.gaps_have_newline(fn_span.end(), arms.iter().map(arm_span));
                let mut parts = vec![text("fn")];
                for (at, arm) in arms.iter().enumerate() {
                    parts.push(Doc::Line);
                    // A shorthand nested at the right edge of an earlier arm
                    // would claim the next arm's bar; parentheses hand it back.
                    let parens = at + 1 < arms.len() && ends_in_match_function(&arm.body.tracked);
                    parts.push(self.arm(arm, parens));
                }
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Do { stmts, result } => {
                let items = self.items(stmts, result.as_ref().map(|result| result.span), expr.span);
                let list_start = expr.span.start + 2;
                let mut spans: Vec<Span> = items.iter().map(|item| item.span()).collect();
                if let Some(result) = result {
                    spans.push(result.span);
                }
                let signal = self.gaps_have_newline(list_start, spans.iter().copied())
                    || self.newline_between(
                        spans.last().map_or(list_start, |span| span.end()),
                        expr.span.end(),
                    );
                // A region dropped after the `return` — a stray statement —
                // is copied back after it, where it was.
                let (before, after): (Vec<Item<'_>>, Vec<Item<'_>>) =
                    items.iter().partition(|item| {
                        result
                            .as_ref()
                            .is_none_or(|result| item.span().start < result.span.start)
                    });
                let mut inner = self.list(&before, list_start, Doc::Line);
                if let Some(result) = result {
                    if !inner.is_empty() {
                        inner.push(Doc::Line);
                        if let Some(last) = before.last()
                            && self
                                .blank_line_between(self.extent(last.span()).1, result.span.start)
                        {
                            inner.push(Doc::HardLine);
                        }
                    }
                    // The break the author may have made is the one between
                    // the `return` and its value, and the keyword is the last
                    // thing before the value.
                    let keyword_end = self.source[..result.span.start]
                        .rfind("return")
                        .map_or(result.span.start, |at| at + "return".len());
                    let signal = self.newline_between(keyword_end, result.span.start);
                    inner.extend(self.leading_docs(result.span));
                    let body = self.with_trailing(result.span, self.expr_kind(result));
                    if signal {
                        inner.push(text("return"));
                        inner.push(nest(concat(vec![Doc::Line, body])));
                    } else {
                        inner.push(text("return"));
                        inner.push(Doc::Hug(Box::new(body), layout_of(result), false));
                    }
                    for item in &after {
                        inner.push(Doc::HardLine);
                        inner.push(self.with_comments(item.span(), self.verbatim(item.span())));
                    }
                }
                let mut parts = vec![text("do")];
                if !inner.is_empty() {
                    let mut body = vec![Doc::Line];
                    body.extend(inner);
                    parts.push(nest(concat(body)));
                }
                parts.push(nest(self.dangling_docs(expr.span)));
                parts.push(Doc::Line);
                parts.push(text("end"));
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Match { scrutinee, arms } => {
                let signal = self
                    .gaps_have_newline(scrutinee.span.end(), arms.iter().map(arm_span))
                    || self.newline_between(
                        arms.last()
                            .map_or(scrutinee.span.end(), |arm| arm_span(arm).end()),
                        expr.span.end(),
                    );
                let mut parts = vec![text("match "), self.expr(scrutinee), text(" with")];
                for arm in arms {
                    parts.push(Doc::Line);
                    parts.push(self.arm(arm, false));
                }
                parts.push(self.dangling_docs(expr.span));
                parts.push(Doc::Line);
                parts.push(text("end"));
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::If {
                predicate,
                consequent,
                alternative,
            } => {
                let then_end = self.source[predicate.span.end()..]
                    .find("then")
                    .map_or(predicate.span.end(), |at| {
                        predicate.span.end() + at + "then".len()
                    });
                let signal = self.newline_between(then_end, expr.span.end());
                let mut parts = Vec::new();
                let clause = |keyword: &str, predicate: &Expr, consequent: &Expr| {
                    vec![
                        group(concat(vec![
                            text(keyword),
                            self.expr(predicate),
                            text(" then"),
                            nest(concat(vec![Doc::Line, self.expr(consequent)])),
                        ])),
                        Doc::Line,
                    ]
                };
                parts.extend(clause("if ", predicate, consequent));
                // Every node of the chain spans through the one shared
                // `end`, so a comment in front of it may have attached to
                // any of them.
                let mut dangling = vec![self.dangling_docs(expr.span)];
                let mut current = alternative.as_ref();
                while let ExprKind::If {
                    predicate,
                    consequent,
                    alternative,
                } = &current.tracked
                {
                    parts.extend(self.leading_docs(current.span));
                    dangling.push(self.dangling_docs(current.span));
                    parts.extend(clause("else if ", predicate, consequent));
                    current = alternative;
                }
                // A comment on the line before the `else` is about the
                // `else`, and goes in front of it rather than under it.
                parts.extend(self.leading_docs(current.span));
                parts.push(group(concat(vec![
                    text("else"),
                    nest(concat(vec![
                        Doc::Line,
                        self.with_trailing(current.span, self.expr_kind(current)),
                    ])),
                ])));
                parts.push(concat(dangling));
                parts.push(Doc::Line);
                parts.push(text("end"));
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Struct { fields, spread } => {
                let mut entries: Vec<(Span, Doc)> = fields
                    .iter()
                    .map(|(name, value)| {
                        let span = field_span(name, Some(value));
                        (
                            span,
                            self.with_comments(
                                span,
                                concat(vec![self.label(name), text(": "), self.expr(value)]),
                            ),
                        )
                    })
                    .collect();
                if let Some(spread) = spread {
                    let span = spread.span.merge(spread.value.span);
                    entries.push((
                        span,
                        self.with_comments(
                            span,
                            concat(vec![text(".."), self.expr(&spread.value)]),
                        ),
                    ));
                }
                let signal = self
                    .gaps_have_newline(expr.span.start + 1, entries.iter().map(|(span, _)| *span));
                self.braces(
                    entries.into_iter().map(|(_, doc)| doc).collect(),
                    spread.is_none(),
                    self.dangling_docs(expr.span),
                    signal,
                )
            }
            ExprKind::Tuple(elements) => {
                let signal =
                    self.gaps_have_newline(expr.span.start + 1, elements.iter().map(|e| e.span));
                let trailing = if elements.len() == 1 {
                    text(",")
                } else {
                    if_break(text(","), nil())
                };
                self.brackets(
                    "(",
                    ")",
                    elements.iter().map(|element| self.expr(element)).collect(),
                    trailing,
                    self.dangling_docs(expr.span),
                    signal,
                )
            }
            ExprKind::Array(items) => {
                let spans: Vec<Span> = items
                    .iter()
                    .map(|item| {
                        item.spread
                            .map_or(item.value.span, |spread| spread.merge(item.value.span))
                    })
                    .collect();
                let signal = self.gaps_have_newline(expr.span.start + 1, spans.iter().copied());
                let entries = items
                    .iter()
                    .zip(&spans)
                    .map(|(item, span)| {
                        let mut parts = Vec::new();
                        if item.spread.is_some() {
                            parts.push(text(".."));
                        }
                        parts.push(self.expr(&item.value));
                        self.with_comments(*span, concat(parts))
                    })
                    .collect();
                self.brackets(
                    "[",
                    "]",
                    entries,
                    if_break(text(","), nil()),
                    self.dangling_docs(expr.span),
                    signal,
                )
            }
            ExprKind::Tag { name, payload } => {
                let mut parts = vec![text(self.slice(name.span))];
                if let Some(payload) = payload {
                    parts.push(Doc::Space);
                    parts.push(self.expr_in(payload, prec(payload) < Prec::Atom));
                }
                concat(parts)
            }
            ExprKind::Project { base, field } => {
                let index = ui::canonical_tuple_index(&field.tracked);
                let parens = prec(base) < Prec::Atom
                    || (index.is_some() && ui::expr_ends_in_numeric_projection(&base.tracked));
                concat(vec![
                    self.expr_in(base, parens),
                    text("."),
                    self.label(field),
                ])
            }
            ExprKind::Handle { body, arms } => {
                let signal = self
                    .gaps_have_newline(body.span.end(), arms.iter().map(handler_arm_span))
                    || self.newline_between(
                        arms.last()
                            .map_or(body.span.end(), |arm| handler_arm_span(arm).end()),
                        expr.span.end(),
                    );
                let mut parts = vec![text("handle "), self.expr(body), text(" with")];
                for arm in arms {
                    parts.push(Doc::Line);
                    parts.push(self.handler_arm(arm));
                }
                parts.push(self.dangling_docs(expr.span));
                parts.push(Doc::Line);
                parts.push(text("end"));
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            ExprKind::Raise(value) => concat(vec![text("raise "), self.expr(value)]),
            ExprKind::Operation { effect, selector } => concat(vec![
                self.effect_label(effect),
                text(selector.tracked.to_string()),
            ]),
            ExprKind::Ident { name } => self.path(name),
            ExprKind::Natural(value) => text(format!("{value}n")),
            ExprKind::Integer(value) => text(format!("{value}i")),
            ExprKind::Fixed(value) => text(value.to_string()),
            ExprKind::Real(_) => text(normalize_real(self.slice(expr.span))),
            ExprKind::String(_) => self.string(expr.span),
            ExprKind::Boolean(value) => text(value.to_string()),
            ExprKind::Unit => self.empty("(", expr.span, ")"),
        }
    }

    /// An empty delimited form — `()`, `{}`, `[]` — with whatever comments
    /// were written inside it on lines of their own between the brackets.
    fn empty(&self, open: &str, span: Span, close: &str) -> Doc {
        if self.dangling.contains_key(&span) {
            concat(vec![
                text(open),
                nest(self.dangling_docs(span)),
                Doc::HardLine,
                text(close),
            ])
        } else {
            text(format!("{open}{close}"))
        }
    }

    /// An expression without the comments in front of it, for a position
    /// that prints those in front of the operator or keyword before it.
    fn expr_after(&self, expr: &Expr, parens: bool) -> Doc {
        parenthesized(parens, self.with_trailing(expr.span, self.expr_kind(expr)))
    }

    /// [`expr_after`](Self::expr_after) for a type.
    fn ty_after(&self, ty: &Type, parens: bool) -> Doc {
        parenthesized(parens, self.with_trailing(ty.span, self.type_kind(ty)))
    }

    fn with_trailing(&self, span: Span, doc: Doc) -> Doc {
        let mut parts = vec![doc];
        parts.extend(self.trailing_docs(span));
        concat(parts)
    }

    fn arg(&self, arg: &Arg) -> Doc {
        let name = match &arg.tracked {
            ArgKind::Name(name) => name.as_str(),
            ArgKind::Wildcard => "_",
        };
        self.with_comments(arg.span, text(name))
    }

    /// `| pattern => body`, the body one level in on its own line when it
    /// does not fit beside the pattern.
    fn arm(&self, arm: &Arm, parens: bool) -> Doc {
        self.with_comments(
            arm_span(arm),
            group(concat(vec![
                text("| "),
                self.pattern(&arm.pattern),
                text(" =>"),
                self.arm_body(arm.pattern.span.end(), &arm.body, parens),
            ])),
        )
    }

    /// An arm's body after its `=>`: a block or braced value hugs the arrow,
    /// and anything else goes one level in on its own line when it does not
    /// fit beside the pattern. A nested `match` never hugs, since its arms
    /// would land in the column of the arms around it.
    fn arm_body(&self, arrow_before: usize, body: &Expr, parens: bool) -> Doc {
        let signal = self.newline_between(arrow_before, body.span.start);
        let doc = self.expr_in(body, parens);
        if signal || parens {
            nest(concat(vec![Doc::Line, doc]))
        } else {
            Doc::Hug(
                Box::new(doc),
                if hugs_in_arm(body) {
                    Layout::Block
                } else {
                    Layout::Down
                },
                true,
            )
        }
    }

    fn handler_arm(&self, arm: &HandlerArm) -> Doc {
        let head = match &arm.head {
            ArmHead::Operation { effect, selector } => concat(vec![
                self.effect_label(effect),
                text(selector.tracked.to_string()),
            ]),
            ArmHead::Return { .. } => text("return"),
        };
        self.with_comments(
            handler_arm_span(arm),
            group(concat(vec![
                text("| "),
                head,
                Doc::Space,
                self.arg(&arm.binder),
                text(" =>"),
                self.arm_body(arm.binder.span.end(), &arm.body, false),
            ])),
        )
    }

    fn path(&self, path: &Path) -> Doc {
        let mut out = String::new();
        for module in &path.modules {
            out.push_str(&module.tracked);
            out.push_str("::");
        }
        out.push_str(&path.name.tracked);
        text(out)
    }

    /// An effect label with the path in front of it: `Sys::!Log`.
    fn effect_label(&self, path: &Path) -> Doc {
        let mut out = String::new();
        for module in &path.modules {
            out.push_str(&module.tracked);
            out.push_str("::");
        }
        out.push_str(self.slice(path.name.span));
        text(out)
    }

    // -- patterns -------------------------------------------------------------

    fn pattern(&self, pattern: &Pattern) -> Doc {
        self.with_comments(pattern.span, self.pattern_kind(pattern))
    }

    fn pattern_in(&self, pattern: &Pattern, parens: bool) -> Doc {
        parenthesized(parens, self.pattern(pattern))
    }

    fn pattern_kind(&self, pattern: &Pattern) -> Doc {
        match &pattern.tracked {
            PatternKind::Ident { name } => text(name.tracked.clone()),
            PatternKind::Wildcard => text("_"),
            PatternKind::Natural(value) => text(format!("{value}n")),
            PatternKind::Integer(value) => text(format!("{value}i")),
            PatternKind::Fixed(value) => text(value.to_string()),
            PatternKind::Real(_) => text(normalize_real(self.slice(pattern.span))),
            PatternKind::String(_) => self.string(pattern.span),
            PatternKind::Boolean(value) => text(value.to_string()),
            PatternKind::Unit => self.empty("(", pattern.span, ")"),
            PatternKind::Struct { fields, rest } => {
                let mut entries: Vec<(Span, Doc)> = fields
                    .iter()
                    .map(|(name, value)| {
                        let span = field_span(name, value.as_ref());
                        let doc = match value {
                            Some(value) => {
                                concat(vec![self.label(name), text(": "), self.pattern(value)])
                            }
                            None => self.label(name),
                        };
                        (span, self.with_comments(span, doc))
                    })
                    .collect();
                if let Some(rest) = rest {
                    entries.push((*rest, self.with_comments(*rest, text(".."))));
                }
                let signal = self.gaps_have_newline(
                    pattern.span.start + 1,
                    entries.iter().map(|(span, _)| *span),
                );
                self.braces(
                    entries.into_iter().map(|(_, doc)| doc).collect(),
                    rest.is_none(),
                    self.dangling_docs(pattern.span),
                    signal,
                )
            }
            PatternKind::Tuple(elements) => {
                let signal =
                    self.gaps_have_newline(pattern.span.start + 1, elements.iter().map(|e| e.span));
                let trailing = if elements.len() == 1 {
                    text(",")
                } else {
                    if_break(text(","), nil())
                };
                self.brackets(
                    "(",
                    ")",
                    elements
                        .iter()
                        .map(|element| self.pattern(element))
                        .collect(),
                    trailing,
                    self.dangling_docs(pattern.span),
                    signal,
                )
            }
            PatternKind::Array {
                before,
                rest,
                after,
            } => {
                let mut entries: Vec<(Span, Doc)> = before
                    .iter()
                    .map(|element| (element.span, self.pattern(element)))
                    .collect();
                if let Some(rest) = rest {
                    entries.push((
                        rest.span,
                        self.with_comments(rest.span, text(self.slice(rest.span))),
                    ));
                }
                entries.extend(
                    after
                        .iter()
                        .map(|element| (element.span, self.pattern(element))),
                );
                let signal = self.gaps_have_newline(
                    pattern.span.start + 1,
                    entries.iter().map(|(span, _)| *span),
                );
                self.brackets(
                    "[",
                    "]",
                    entries.into_iter().map(|(_, doc)| doc).collect(),
                    if_break(text(","), nil()),
                    self.dangling_docs(pattern.span),
                    signal,
                )
            }
            PatternKind::Tag { name, payload } => {
                let mut parts = vec![text(self.slice(name.span))];
                if let Some(payload) = payload {
                    parts.push(Doc::Space);
                    parts.push(
                        self.pattern_in(payload, ui::pattern_prec(&payload.tracked) < Prec::Atom),
                    );
                }
                concat(parts)
            }
        }
    }

    // -- types ----------------------------------------------------------------

    fn ty(&self, ty: &Type) -> Doc {
        self.with_comments(ty.span, self.type_kind(ty))
    }

    fn ty_in(&self, ty: &Type, parens: bool) -> Doc {
        parenthesized(parens, self.ty(ty))
    }

    fn type_kind(&self, ty: &Type) -> Doc {
        let prec = |ty: &Type| ui::type_prec(&ty.tracked);
        match &ty.tracked {
            TypeKind::Arrow { .. } => {
                // Right-nested, and only an arrow carrying no row may fold
                // the arrow after it into the chain: `A -> (B -> C) + E` puts
                // the row on the outer arrow and needs its parentheses.
                let mut steps = Vec::new();
                let mut current = ty;
                while let TypeKind::Arrow { from, to, effects } = &current.tracked {
                    steps.push((current, from.as_ref(), effects.as_deref()));
                    current = to;
                    if effects.is_some() || !matches!(to.tracked, TypeKind::Arrow { .. }) {
                        break;
                    }
                }
                let result = current;
                let mut parts = Vec::new();
                let mut spans = Vec::new();
                for (at, (node, from, effects)) in steps.iter().enumerate() {
                    if at > 0 {
                        parts.extend(self.leading_docs(node.span));
                        parts.push(Doc::Line);
                        parts.push(text("-> "));
                    }
                    // A sum needs no parentheses in front of an arrow, but
                    // reads as the arrow's last case without them, so it
                    // keeps the pair the author almost certainly wrote.
                    parts.push(self.ty_in(from, prec(from) <= Prec::Sum));
                    spans.push(from.span);
                    if effects.is_some() {
                        // An arrow with a row is the last step; its result
                        // follows below.
                        debug_assert_eq!(at + 1, steps.len());
                    }
                }
                parts.push(Doc::Line);
                parts.extend(self.leading_docs(result.span));
                parts.push(text("-> "));
                spans.push(result.span);
                let last_effects = steps.last().and_then(|(_, _, effects)| *effects);
                parts.push(self.ty_after(
                    result,
                    last_effects.is_some() && prec(result) == Prec::Arrow,
                ));
                if let Some(row) = last_effects {
                    parts.push(text(" + "));
                    parts.push(self.effect_row(row));
                }
                let signal = self.gaps_have_newline(ty.span.start, spans);
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            TypeKind::Struct { fields, tail } => {
                let mut entries: Vec<(Span, Doc)> = fields
                    .iter()
                    .map(|(name, field)| match field {
                        TypeField::Written { when, value } => {
                            let span = merge_all(
                                name.span,
                                when_span(when).into_iter().chain([value.span]),
                            );
                            let mut parts = vec![self.label(name)];
                            if let Some(when) = when {
                                parts.push(Doc::Space);
                                parts.push(self.when(when));
                            }
                            parts.push(text(": "));
                            parts.push(self.ty(value));
                            (span, self.with_comments(span, concat(parts)))
                        }
                        TypeField::Absent => (
                            name.span,
                            self.with_comments(name.span, text(self.slice(name.span))),
                        ),
                    })
                    .collect();
                if let Some(tail) = tail {
                    entries.push((
                        tail.span,
                        self.with_comments(tail.span, self.tail(&tail.of)),
                    ));
                }
                let signal = self
                    .gaps_have_newline(ty.span.start + 1, entries.iter().map(|(span, _)| *span));
                self.braces(
                    entries.into_iter().map(|(_, doc)| doc).collect(),
                    tail.is_none(),
                    self.dangling_docs(ty.span),
                    signal,
                )
            }
            TypeKind::Sum { cases, tail } => {
                let mut entries: Vec<(Span, Doc)> = cases
                    .iter()
                    .map(|(name, case)| {
                        let span = sum_case_span(name, case);
                        let doc = match case {
                            SumCase::Written { when, payload } => {
                                let mut parts = vec![text(self.slice(name.span))];
                                if let Some(when) = when {
                                    parts.push(Doc::Space);
                                    parts.push(concat(vec![text("("), self.when(when), text(")")]));
                                }
                                if let Some(payload) = payload {
                                    parts.push(Doc::Space);
                                    parts.push(self.ty_in(payload, prec(payload) < Prec::Atom));
                                }
                                concat(parts)
                            }
                            SumCase::Absent => text(self.slice(name.span)),
                        };
                        (span, self.with_trailing(span, doc))
                    })
                    .collect();
                if let Some(tail) = tail {
                    entries.push((
                        tail.span,
                        self.with_trailing(tail.span, self.tail(&tail.of)),
                    ));
                }
                // The empty sum, and the sum that is nothing but its tail:
                // neither writes a case, so neither reads back as a sum
                // without the bar.
                if cases.is_empty() {
                    let mut parts = vec![text("|")];
                    for (span, doc) in entries {
                        parts.push(Doc::Space);
                        parts.extend(self.leading_docs(span));
                        parts.push(doc);
                    }
                    return concat(parts);
                }
                let signal =
                    self.gaps_have_newline(ty.span.start, entries.iter().map(|(span, _)| *span));
                // A comment in front of a case goes in front of its bar.
                let mut parts = Vec::new();
                for (at, (span, doc)) in entries.into_iter().enumerate() {
                    if at > 0 {
                        parts.push(if_break(Doc::Line, nil()));
                    }
                    parts.extend(self.leading_docs(span));
                    parts.push(if_break(
                        text("| "),
                        if at == 0 { nil() } else { text(" | ") },
                    ));
                    parts.push(doc);
                }
                let doc = concat(parts);
                if signal { broken(doc) } else { group(doc) }
            }
            TypeKind::Apply { head, args } => {
                let mut parts = vec![self.ty_in(head, prec(head) < Prec::Atom)];
                for arg in args {
                    parts.push(Doc::Space);
                    parts.push(self.ty_in(arg, prec(arg) < Prec::Atom));
                }
                concat(parts)
            }
            TypeKind::Tuple(elements) => {
                let signal =
                    self.gaps_have_newline(ty.span.start + 1, elements.iter().map(|e| e.span));
                let trailing = if elements.len() == 1 {
                    text(",")
                } else {
                    if_break(text(","), nil())
                };
                self.brackets(
                    "(",
                    ")",
                    elements.iter().map(|element| self.ty(element)).collect(),
                    trailing,
                    self.dangling_docs(ty.span),
                    signal,
                )
            }
            TypeKind::Array(element) => concat(vec![text("["), self.ty(element), text("]")]),
            TypeKind::Mut(region, element) => concat(vec![
                text("mut "),
                self.ty_in(region, prec(region) < Prec::Atom),
                Doc::Space,
                self.ty_in(element, prec(element) < Prec::Atom),
            ]),
            TypeKind::Ident { name } => self.path(name),
            TypeKind::Variable { name } => text(self.slice(name.span)),
            TypeKind::Effects(row) => self.effect_row(row),
            TypeKind::Hole => text("_"),
            TypeKind::Unit => self.empty("(", ty.span, ")"),
        }
    }

    fn when(&self, when: &When) -> Doc {
        self.with_comments(
            when.span,
            match &when.name {
                Some(name) => text(format!("when {}", self.slice(name.span))),
                None => text("when _"),
            },
        )
    }

    fn tail(&self, rest: &Rest) -> Doc {
        match rest {
            Rest::Anything => text(".."),
            Rest::Variable(name) => text(format!("..{}", self.slice(name.span))),
        }
    }

    /// `!Log + !Ask Nat (when 'a) + \!IO + ..'e`, or `|` for the row that
    /// allows nothing.
    fn effect_row(&self, row: &EffectRow) -> Doc {
        let mut parts = Vec::new();
        for (path, label) in &row.effects {
            let span = effect_label_span(path, label);
            let (args, when, absent) = match label {
                EffectLabel::Written { args, when } => (args, when.as_deref(), false),
                EffectLabel::Absent { args } => (args, None, true),
            };
            // An absent label's key spans from its `\` through its name,
            // path included, so it is copied whole.
            let mut doc = vec![if absent {
                text(self.slice(path.name.span))
            } else {
                self.effect_label(path)
            }];
            for arg in args {
                doc.push(Doc::Space);
                doc.push(self.ty_in(arg, ui::type_prec(&arg.tracked) < Prec::Atom));
            }
            if let Some(when) = when {
                doc.push(Doc::Space);
                doc.push(concat(vec![text("("), self.when(when), text(")")]));
            }
            parts.push(self.with_comments(span, concat(doc)));
        }
        if let Some(tail) = &row.tail {
            parts.push(self.with_comments(tail.span, self.tail(&tail.of)));
        }
        if parts.is_empty() {
            return text("|");
        }
        join(parts, || text(" + "))
    }

    fn extern_type(&self, ty: &ExternType) -> Doc {
        self.with_comments(ty.span, self.extern_type_kind(ty))
    }

    fn extern_type_kind(&self, ty: &ExternType) -> Doc {
        match &ty.tracked {
            ExternTypeKind::Annotated { attributes, inner } => {
                let mut parts = Vec::new();
                for attribute in attributes {
                    parts.push(self.attribute(attribute));
                    parts.push(Doc::Space);
                }
                parts.push(self.extern_type(inner));
                concat(parts)
            }
            ExternTypeKind::Group(inner) => {
                let dangling = self.dangling_docs(ty.span);
                let close = match dangling {
                    Doc::Concat(ref docs) if docs.is_empty() => text(")"),
                    dangling => concat(vec![nest(dangling), Doc::HardLine, text(")")]),
                };
                concat(vec![text("("), self.extern_type(inner), close])
            }
            ExternTypeKind::Ordinary(inner) => self.ty(inner),
            ExternTypeKind::Function {
                parameters,
                result,
                effects,
            } => {
                let signal = self.gaps_have_newline(
                    ty.span.start + 3,
                    parameters.iter().map(|parameter| parameter.span),
                );
                let mut parts = vec![
                    self.brackets(
                        "fn(",
                        ")",
                        parameters
                            .iter()
                            .map(|parameter| self.extern_type(parameter))
                            .collect(),
                        if_break(text(","), nil()),
                        nil(),
                        signal,
                    ),
                ];
                parts.push(text(" -> "));
                parts.push(self.extern_type(result));
                if let Some(row) = effects {
                    parts.push(text(" + "));
                    parts.push(self.effect_row(row));
                }
                concat(parts)
            }
        }
    }

    // -- metadata -------------------------------------------------------------

    fn data(&self, data: &Data) -> Doc {
        self.with_comments(data.span, self.data_kind(data))
    }

    fn data_kind(&self, data: &Data) -> Doc {
        match &data.tracked {
            DataKind::Natural(value) => text(format!("{value}n")),
            DataKind::Integer(value) => text(format!("{value}i")),
            DataKind::Fixed(value) => text(value.to_string()),
            DataKind::Real(_) => text(normalize_real(self.slice(data.span))),
            DataKind::String(_) => self.string(data.span),
            DataKind::Boolean(value) => text(value.to_string()),
            DataKind::Unit => self.empty("(", data.span, ")"),
            DataKind::Tuple(elements) => {
                let signal =
                    self.gaps_have_newline(data.span.start + 1, elements.iter().map(|e| e.span));
                let trailing = if elements.len() == 1 {
                    text(",")
                } else {
                    if_break(text(","), nil())
                };
                self.brackets(
                    "(",
                    ")",
                    elements.iter().map(|element| self.data(element)).collect(),
                    trailing,
                    self.dangling_docs(data.span),
                    signal,
                )
            }
            DataKind::Array(items) => {
                let signal =
                    self.gaps_have_newline(data.span.start + 1, items.iter().map(|e| e.span));
                self.brackets(
                    "[",
                    "]",
                    items.iter().map(|item| self.data(item)).collect(),
                    if_break(text(","), nil()),
                    self.dangling_docs(data.span),
                    signal,
                )
            }
            DataKind::Struct(fields) => {
                let entries: Vec<(Span, Doc)> = fields
                    .iter()
                    .map(|(name, value)| {
                        let span = field_span(name, Some(value));
                        (
                            span,
                            self.with_comments(
                                span,
                                concat(vec![self.label(name), text(": "), self.data(value)]),
                            ),
                        )
                    })
                    .collect();
                let signal = self
                    .gaps_have_newline(data.span.start + 1, entries.iter().map(|(span, _)| *span));
                self.braces(
                    entries.into_iter().map(|(_, doc)| doc).collect(),
                    true,
                    self.dangling_docs(data.span),
                    signal,
                )
            }
            DataKind::Tag { name, payload } => {
                let mut parts = vec![text(self.slice(name.span))];
                if let Some(payload) = payload {
                    parts.push(Doc::Space);
                    let parens =
                        matches!(
                            payload.tracked,
                            DataKind::Tag {
                                payload: Some(_),
                                ..
                            }
                        ) || matches!(payload.tracked, DataKind::Tag { payload: None, .. });
                    parts.push(parenthesized(parens, self.data(payload)));
                }
                concat(parts)
            }
        }
    }

    fn expr_skeleton(&self, expr: &Expr) -> Skel {
        let kids = match &expr.tracked {
            ExprKind::Function { args, body } => {
                let mut kids: Vec<Skel> = args.iter().map(|arg| Skel::leaf(arg.span)).collect();
                kids.push(self.expr_skeleton(body));
                kids
            }
            ExprKind::MatchFunction { arms, .. } => {
                arms.iter().map(|arm| self.arm_skeleton(arm)).collect()
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut kids = vec![self.expr_skeleton(scrutinee)];
                kids.extend(arms.iter().map(|arm| self.arm_skeleton(arm)));
                return Skel::new(expr.span, kids).closed();
            }
            ExprKind::Handle { body, arms } => {
                let mut kids = vec![self.expr_skeleton(body)];
                kids.extend(arms.iter().map(|arm| {
                    Skel::new(
                        handler_arm_span(arm),
                        vec![Skel::leaf(arm.binder.span), self.expr_skeleton(&arm.body)],
                    )
                }));
                return Skel::new(expr.span, kids).closed();
            }
            ExprKind::Do { stmts, result } => {
                let mut kids: Vec<Skel> = self
                    .items(stmts, result.as_ref().map(|result| result.span), expr.span)
                    .into_iter()
                    .map(|item| match item {
                        Item::Stmt(stmt) => self.stmt_skeleton(stmt),
                        Item::Skipped(span) => Skel::verbatim(span),
                    })
                    .collect();
                kids.extend(result.iter().map(|result| self.expr_skeleton(result)));
                return Skel::new(expr.span, kids).closed();
            }
            ExprKind::If { .. } => {
                return Skel::new(
                    expr.span,
                    expr_children(expr)
                        .into_iter()
                        .map(|child| self.expr_skeleton(child))
                        .collect(),
                )
                .closed();
            }
            ExprKind::Struct { fields, spread } => {
                let mut kids: Vec<Skel> = fields
                    .iter()
                    .map(|(name, value)| {
                        Skel::new(
                            field_span(name, Some(value)),
                            vec![self.expr_skeleton(value)],
                        )
                    })
                    .collect();
                if let Some(spread) = spread {
                    kids.push(Skel::new(
                        spread.span.merge(spread.value.span),
                        vec![self.expr_skeleton(&spread.value)],
                    ));
                }
                return Skel::new(expr.span, kids).closed();
            }
            ExprKind::Tuple(elements) => {
                return Skel::new(
                    expr.span,
                    elements
                        .iter()
                        .map(|element| self.expr_skeleton(element))
                        .collect(),
                )
                .closed();
            }
            ExprKind::Array(items) => {
                return Skel::new(
                    expr.span,
                    items
                        .iter()
                        .map(|item| {
                            let span = item
                                .spread
                                .map_or(item.value.span, |spread| spread.merge(item.value.span));
                            Skel::new(span, vec![self.expr_skeleton(&item.value)])
                        })
                        .collect(),
                )
                .closed();
            }
            ExprKind::Unit => return Skel::leaf(expr.span).closed(),
            _ => expr_children(expr)
                .into_iter()
                .map(|child| self.expr_skeleton(child))
                .collect(),
        };
        Skel::new(expr.span, kids)
    }

    fn arm_skeleton(&self, arm: &Arm) -> Skel {
        Skel::new(
            arm_span(arm),
            vec![
                pattern_skeleton(&arm.pattern),
                self.expr_skeleton(&arm.body),
            ],
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers over the tree
// ---------------------------------------------------------------------------

/// A real literal with its spelling normalized and its digits kept: no
/// leading zeros on the whole part, no trailing zeros on the fraction, and
/// no fraction at all when it was zero. Textual rather than through the
/// decoded value, so a literal wider than a double's precision keeps every
/// digit the author wrote.
fn normalize_real(written: &str) -> String {
    let (sign, digits) = match written.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", written),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end() <= outer.end()
}

/// Whether an expression hugs the `=` or `=>` in front of it rather than
/// moving to a line of its own when the definition does not fit: a block
/// that closes with its own `end` lays its arms or statements out under the
/// line it began on, a braced value opens its brace there, and a function
/// keeps its header there and lays its body out under it.
fn hugs(expr: &Expr) -> bool {
    matches!(
        expr.tracked,
        ExprKind::Match { .. }
            | ExprKind::Handle { .. }
            | ExprKind::Do { .. }
            | ExprKind::Function { .. }
    ) || delimited(expr)
}

/// The layout of an expression after the `=` or `=>` of a definition. A
/// conditional moves down whole: its `else` and `end` lines would otherwise
/// sit in the column of the `let`.
fn layout_of(expr: &Expr) -> Layout {
    if hugs(expr) {
        Layout::Block
    } else if matches!(expr.tracked, ExprKind::If { .. }) {
        Layout::Down
    } else {
        Layout::Fluid
    }
}

/// Whether an expression is written between brackets of its own.
fn delimited(expr: &Expr) -> bool {
    matches!(
        expr.tracked,
        ExprKind::Struct { .. } | ExprKind::Tuple(_) | ExprKind::Array(_)
    )
}

/// [`hugs`] for an arm's body: a `do` block or a braced value hugs the
/// `=>`, and a function whose body does the same hugs through it. A nested
/// `match` or `handle` does not, since its arms would sit in the column of
/// the arms around it.
fn hugs_in_arm(expr: &Expr) -> bool {
    match &expr.tracked {
        ExprKind::Do { .. } => true,
        ExprKind::Function { body, .. } => hugs_in_arm(body),
        _ => delimited(expr),
    }
}

/// [`hugs`] for a written type: a braced or bracketed one opens on the line
/// of the `=` in front of it.
fn hugs_type(ty: &Type) -> bool {
    matches!(
        ty.tracked,
        TypeKind::Struct { .. } | TypeKind::Tuple(_) | TypeKind::Array(_)
    )
}

/// Whether a following `|` would be claimed by a shorthand nested at the
/// right edge of this expression.
fn ends_in_match_function(expr: &ExprKind) -> bool {
    match expr {
        ExprKind::MatchFunction { .. } => true,
        ExprKind::Function { body, .. } => ends_in_match_function(&body.tracked),
        ExprKind::Raise(value) => ends_in_match_function(&value.tracked),
        _ => false,
    }
}

/// The expressions directly inside an expression.
fn expr_children(expr: &Expr) -> Vec<&Expr> {
    match &expr.tracked {
        ExprKind::Pipe { value, function } => vec![value, function],
        ExprKind::Unary { value, .. } => vec![value],
        ExprKind::Binary { left, right, .. } => vec![left, right],
        ExprKind::Apply { func, arg } => vec![func, arg],
        ExprKind::Function { body, .. } => vec![body],
        ExprKind::MatchFunction { arms, .. } => arms.iter().map(|arm| &arm.body).collect(),
        ExprKind::Do { stmts, result } => {
            let mut out: Vec<&Expr> = stmts
                .iter()
                .filter_map(|stmt| match &stmt.kind {
                    StmtKind::Let { body, .. } => Some(&body.tracked),
                    _ => None,
                })
                .collect();
            out.extend(result.as_deref());
            out
        }
        ExprKind::Match { scrutinee, arms } => {
            let mut out = vec![scrutinee.as_ref()];
            out.extend(arms.iter().map(|arm| &arm.body));
            out
        }
        ExprKind::If {
            predicate,
            consequent,
            alternative,
        } => vec![predicate, consequent, alternative],
        ExprKind::Struct { fields, spread } => {
            let mut out: Vec<&Expr> = fields.values().collect();
            out.extend(spread.as_ref().map(|spread| spread.value.as_ref()));
            out
        }
        ExprKind::Tuple(elements) => elements.iter().collect(),
        ExprKind::Array(items) => items.iter().map(|item| &item.value).collect(),
        ExprKind::Tag { payload, .. } => payload.iter().map(Box::as_ref).collect(),
        ExprKind::Project { base, .. } => vec![base],
        ExprKind::Handle { body, arms } => {
            let mut out = vec![body.as_ref()];
            out.extend(arms.iter().map(|arm| &arm.body));
            out
        }
        ExprKind::Raise(value) => vec![value],
        ExprKind::Operation { .. }
        | ExprKind::Ident { .. }
        | ExprKind::Natural(_)
        | ExprKind::Integer(_)
        | ExprKind::Fixed(_)
        | ExprKind::Real(_)
        | ExprKind::String(_)
        | ExprKind::Boolean(_)
        | ExprKind::Unit => Vec::new(),
    }
}

fn attribute_skeleton(attribute: &Attribute) -> Skel {
    Skel::new(
        attribute.span,
        attribute.value.iter().map(data_skeleton).collect(),
    )
}

fn clauses_skeleton(annotation: &Annotation) -> Vec<Skel> {
    annotation
        .clause
        .iter()
        .flat_map(|clause| clause.clauses.iter().map(clause_skeleton))
        .collect()
}

fn clause_skeleton(clause: &Clause) -> Skel {
    let kids = match &clause.tracked {
        ClauseKind::Name(_) => Vec::new(),
        ClauseKind::Not(inner) => vec![clause_skeleton(inner)],
        ClauseKind::And(left, right)
        | ClauseKind::Or(left, right)
        | ClauseKind::Equal(left, right)
        | ClauseKind::NotEqual(left, right) => {
            vec![clause_skeleton(left), clause_skeleton(right)]
        }
    };
    Skel::new(clause.span, kids)
}

fn data_skeleton(data: &Data) -> Skel {
    match &data.tracked {
        DataKind::Tuple(elements) | DataKind::Array(elements) => {
            Skel::new(data.span, elements.iter().map(data_skeleton).collect()).closed()
        }
        DataKind::Struct(fields) => Skel::new(
            data.span,
            fields
                .iter()
                .map(|(name, value)| {
                    Skel::new(field_span(name, Some(value)), vec![data_skeleton(value)])
                })
                .collect(),
        )
        .closed(),
        DataKind::Tag { payload, .. } => Skel::new(
            data.span,
            payload
                .iter()
                .map(|payload| data_skeleton(payload))
                .collect(),
        ),
        DataKind::Unit => Skel::leaf(data.span).closed(),
        _ => Skel::leaf(data.span),
    }
}

fn pattern_skeleton(pattern: &Pattern) -> Skel {
    match &pattern.tracked {
        PatternKind::Struct { fields, rest } => {
            let mut kids: Vec<Skel> = fields
                .iter()
                .map(|(name, value)| {
                    Skel::new(
                        field_span(name, value.as_ref()),
                        value.iter().map(pattern_skeleton).collect(),
                    )
                })
                .collect();
            if let Some(rest) = rest {
                kids.push(Skel::leaf(*rest));
            }
            Skel::new(pattern.span, kids).closed()
        }
        PatternKind::Tuple(elements) => Skel::new(
            pattern.span,
            elements.iter().map(pattern_skeleton).collect(),
        )
        .closed(),
        PatternKind::Array {
            before,
            rest,
            after,
        } => {
            let mut kids: Vec<Skel> = before.iter().map(pattern_skeleton).collect();
            if let Some(rest) = rest {
                kids.push(Skel::leaf(rest.span));
            }
            kids.extend(after.iter().map(pattern_skeleton));
            Skel::new(pattern.span, kids).closed()
        }
        PatternKind::Tag { payload, .. } => Skel::new(
            pattern.span,
            payload
                .iter()
                .map(|payload| pattern_skeleton(payload))
                .collect(),
        ),
        PatternKind::Unit => Skel::leaf(pattern.span).closed(),
        _ => Skel::leaf(pattern.span),
    }
}

fn type_skeleton(ty: &Type) -> Skel {
    match &ty.tracked {
        TypeKind::Struct { fields, tail } => {
            let mut kids: Vec<Skel> = fields
                .iter()
                .map(|(name, field)| match field {
                    TypeField::Written { when, value } => Skel::new(
                        merge_all(name.span, when_span(when).into_iter().chain([value.span])),
                        vec![type_skeleton(value)],
                    ),
                    TypeField::Absent => Skel::leaf(name.span),
                })
                .collect();
            if let Some(tail) = tail {
                kids.push(Skel::leaf(tail.span));
            }
            Skel::new(ty.span, kids).closed()
        }
        TypeKind::Sum { cases, tail } => {
            let mut kids: Vec<Skel> = cases
                .iter()
                .map(|(name, case)| {
                    let payload = match case {
                        SumCase::Written { payload, .. } => payload.as_ref(),
                        SumCase::Absent => None,
                    };
                    Skel::new(
                        sum_case_span(name, case),
                        payload.into_iter().map(type_skeleton).collect(),
                    )
                })
                .collect();
            if let Some(tail) = tail {
                kids.push(Skel::leaf(tail.span));
            }
            Skel::new(ty.span, kids)
        }
        TypeKind::Arrow { from, to, effects } => {
            let mut kids = vec![type_skeleton(from), type_skeleton(to)];
            if let Some(row) = effects {
                kids.extend(row_skeleton(row));
            }
            Skel::new(ty.span, kids)
        }
        TypeKind::Apply { head, args } => {
            let mut kids = vec![type_skeleton(head)];
            kids.extend(args.iter().map(type_skeleton));
            Skel::new(ty.span, kids)
        }
        TypeKind::Tuple(elements) => {
            Skel::new(ty.span, elements.iter().map(type_skeleton).collect()).closed()
        }
        TypeKind::Array(element) => Skel::new(ty.span, vec![type_skeleton(element)]).closed(),
        TypeKind::Mut(region, element) => {
            Skel::new(ty.span, vec![type_skeleton(region), type_skeleton(element)])
        }
        TypeKind::Effects(row) => Skel::new(ty.span, row_skeleton(row)),
        TypeKind::Unit => Skel::leaf(ty.span).closed(),
        TypeKind::Ident { .. } | TypeKind::Variable { .. } | TypeKind::Hole => Skel::leaf(ty.span),
    }
}

fn row_skeleton(row: &EffectRow) -> Vec<Skel> {
    let mut kids: Vec<Skel> = row
        .effects
        .iter()
        .map(|(path, label)| {
            let args = match label {
                EffectLabel::Written { args, .. } | EffectLabel::Absent { args } => args,
            };
            Skel::new(
                effect_label_span(path, label),
                args.iter().map(type_skeleton).collect(),
            )
        })
        .collect();
    if let Some(tail) = &row.tail {
        kids.push(Skel::leaf(tail.span));
    }
    kids
}

fn extern_type_skeleton(ty: &ExternType) -> Skel {
    match &ty.tracked {
        ExternTypeKind::Annotated { attributes, inner } => {
            let mut kids: Vec<Skel> = attributes.iter().map(attribute_skeleton).collect();
            kids.push(extern_type_skeleton(inner));
            Skel::new(ty.span, kids)
        }
        ExternTypeKind::Group(inner) => {
            Skel::new(ty.span, vec![extern_type_skeleton(inner)]).closed()
        }
        ExternTypeKind::Ordinary(inner) => Skel::new(ty.span, vec![type_skeleton(inner)]),
        ExternTypeKind::Function {
            parameters,
            result,
            effects,
        } => {
            let mut kids: Vec<Skel> = parameters.iter().map(extern_type_skeleton).collect();
            kids.push(extern_type_skeleton(result));
            if let Some(row) = effects {
                kids.extend(row_skeleton(row));
            }
            Skel::new(ty.span, kids)
        }
    }
}
