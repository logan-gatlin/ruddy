//! Everything the compiler says to a person.
//!
//! One file, so that the whole of it can be read at once. What a compiler
//! prints is an interface as much as what it accepts: the words it picks for a
//! complaint, the notation it writes a type in, where it puts a bracket. Spread
//! across the phases that raise them, those choices can only be audited by
//! grepping for string literals and hoping; gathered here, the surface a person
//! actually meets is one file long, and two phases wording the same thing two
//! ways is a difference you can see rather than one you have to notice.
//!
//! Nothing here decides anything. Every phase raises structured values —
//! [`token::ErrorKind`], [`Ty`], [`Symbol`] — and says nothing about them; this
//! module is the only place one becomes words. A phase that phrases its own
//! output has put a second author in the room.
//!
//! Three kinds of thing live here:
//!
//! * **Diagnostics.** One [`fmt::Display`] per error kind, saying what went
//!   wrong in a sentence, and one `code` beside it — a stable, greppable name
//!   that reporters key on so that prose stays free to be reworded. Every
//!   reporter, the CLI driver and the debugger's diagnostic strip alike, prints
//!   through these rather than matching on the variants itself, so a new
//!   variant cannot reach one reader and not the other.
//! * **The surface grammar.** [`Prec`], one table per node kind ([`Grouped`]),
//!   and one writer per position ([`write_apply`] and friends). Grouping is
//!   dropped rather than recorded — `(a) -> b` and `a -> b` parse to the same
//!   tree — so anything rendering one back as source has to work out from
//!   precedence alone where the parentheses go. That rule is said once, here.
//! * **Names and types.** How a bundle, a symbol's path, and a semantic type
//!   are spelled. Types print in the surface type grammar above, so a type in a
//!   diagnostic reads as one the user could have written.
//!
//! What is *not* here is layout: where a diagnostic goes, how a span is quoted,
//! what a row of a debugger table looks like. Those belong to whoever is doing
//! the printing, and differ between one reporter and the next on purpose.
//!
//! Rendering a whole *tree* back as source is no part of the compiler's job
//! either. That lives in `ruddy-debug`'s printers — but they write through the
//! [`Prec`] and [`Grouped`] rules re-exported from here, so the syntax the
//! debugger shows and the syntax the compiler prints cannot drift apart. The
//! sharing can only run in this direction: `ruddy-debug` depends on `ruddy`,
//! and nothing may make the dependency run back.

use std::{collections::HashSet, fmt, path::Path as FsPath};

use crate::{
    bundle,
    inference::{self, Constraint, ConstraintKind, Effect, Goal, Origin, Rule},
    ir, parse, patterns,
    symbol::{Bundle, LOCAL_SEGMENT, Mint, Namespace, Symbol},
    token::{self, Kind},
    tracking::{Anchor, SourceMap, Span},
    types::{
        Assigned, Atom, EffectId, Formula, Presence, Prim, Rest, Row, RowField, Scheme, Sense,
        Shape, Ty,
    },
};

/// The note a duplicate definition points back with, printed against the span
/// of the definition that stands. Held apart from [`ir::ErrorKind`]'s own
/// wording because it is a second line about a second place, and only a
/// reporter knows how to attach one.
pub const FIRST_DEFINITION: &str = "first defined here";
/// The earlier source item in a repeated list.
pub const FIRST_WRITTEN: &str = "first written here";
/// The earlier binder in a pattern.
pub const FIRST_BINDING: &str = "first bound here";
/// The earlier arm in a handler.
pub const FIRST_ARM: &str = "first arm here";

/// The note a clash of tails points back with, printed against the span of the
/// `..` that decided what the name stands for. [`FIRST_DEFINITION`]'s
/// counterpart for the complaints about a name that was used rather than
/// defined twice — see [`ir::ErrorKind::MixedTail`].
pub const FIRST_USE: &str = "first used here";
/// The other result boundary participating in an existential lifetime clash.
pub const FIRST_PRODUCTION_LIFETIME: &str = "another result makes a separate choice here";

/// The note a repeated variable points back with, printed against the
/// span of the declaration that stands.
///
/// [`FIRST_DEFINITION`]'s counterpart for a `where` clause, and separate from it
/// because nothing in a clause is defined: a declaration says a name will stand
/// for something, and what it stands for is the type beside it. See
/// [`ir::ErrorKind::DuplicateVariable`].
pub const FIRST_DECLARATION: &str = "first declared here";

/// The note a complaint about a variable points back with, printed
/// against the span of the name in the a variable that declared it.
///
/// [`FIRST_USE`]'s counterpart for the complaints inference raises about a
/// rigid: what went wrong is on the line the reader is being shown, and what
/// was promised is somewhere else on the page. See
/// [`inference::ErrorKind::RigidBroken`] and
/// [`inference::ErrorKind::RigidField`].
pub const DECLARED_HERE: &str = "declared here";

/// What every `@if` complaint reminds the reader of: the shape of a guard and
/// the facts it can ask about.
const CONDITIONS: &str = "`@if {target: \"js\", platform: \"web\"}` compiles a definition only when the root project is built with every fact named; a fact not named may be anything";

/// A node a printer has to parenthesize by precedence. Implemented by every
/// wrapper that prints as surface syntax, and by [`Ty`], which prints as one
/// directly.
pub trait Grouped: fmt::Display {
    fn prec(&self) -> Prec;

    /// Whether this node's surface spelling ends in a numeric projection.
    /// Another numeric projection must parenthesize such a base: `.0.0` is
    /// deliberately lexed as one malformed decimal-like field.
    fn ends_in_numeric_projection(&self) -> bool {
        false
    }
}

/// A reference groups as what it points at, so a printer can hand out borrowed
/// nodes without every caller wrapping them first.
impl<T: Grouped + ?Sized> Grouped for &T {
    fn prec(&self) -> Prec {
        (**self).prec()
    }

    fn ends_in_numeric_projection(&self) -> bool {
        (**self).ends_in_numeric_projection()
    }
}

/// A name is an atom: one word, however many arrows are behind it. Written
/// here so that the head of an application can be a name in one printer and a
/// whole written type in another, and both go through the same rule.
impl Grouped for str {
    fn prec(&self) -> Prec {
        Prec::Atom
    }
}

/// How tightly a printed node binds. Grouping is dropped rather than recorded,
/// so the printers reconstruct it from this alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    /// `fn a => body` — the body extends as far right as it can, so a lambda
    /// needs parentheses anywhere anything may follow it.
    Lambda,
    /// `cell := value`, associating to the right.
    Assignment,
    /// `value |> function`.
    Pipeline,
    /// `left or right`.
    Or,
    /// `left xor right`.
    Xor,
    /// `left and right`.
    And,
    /// `left + right`.
    Addition,
    /// `left * right`.
    Multiplication,
    /// `-value`.
    Unary,
    /// `from -> to`.
    Arrow,
    /// `#A Nat | #B` — a sum extends rightward by case, so it needs
    /// parentheses anywhere a case could follow it. Above the arrow, which is
    /// what makes `#A Nat | #B -> Nat` a function *from* the sum rather
    /// than a sum whose last case carries an arrow.
    Sum,
    /// `#A` with nothing after it — a tag takes the next atom as its
    /// payload, so a bare one needs parentheses anywhere one could follow.
    ///
    /// Below [`Prec::Apply`] rather than at it, which is the whole difference
    /// between the two: a tag that already carries something groups as the
    /// application it reads as and may head one, `#A 1 2` being `#A 1`
    /// applied to `2`, while a bare tag heading one would swallow the argument
    /// it was applied to — `f (#A) 1` printed without the parentheses reads
    /// back as `f (#A 1)`.
    Tag,
    /// `func arg`.
    Apply,
    /// A form nothing can be appended to: a name, a literal, `()`, a braced
    /// struct, or a projection off one of those.
    Atom,
}

/// How tightly a written expression binds, for whoever prints the parse tree
/// back as source. One table for the debugger's printer and the formatter, so
/// the two cannot come to disagree about where a parenthesis goes.
pub fn expr_prec(kind: &parse::ExprKind) -> Prec {
    use parse::{BinaryOp, ExprKind};
    match kind {
        // The body runs as far right as it can, so anything appended after
        // a bare lambda would be read as part of it.
        ExprKind::Function { .. } | ExprKind::MatchFunction { .. } => Prec::Lambda,
        // Self-delimiting on the right — the `end` closes it — so it may
        // head an application and be projected from; but it is not an
        // application *argument* by grammar, so an argument position
        // brackets it. Below `Atom` is exactly that split.
        ExprKind::If { .. }
        | ExprKind::Match { .. }
        | ExprKind::Handle { .. }
        | ExprKind::Do { .. } => Prec::Apply,
        // The body runs as far right as it can, so anything appended after
        // a `raise` would be read as part of what it carries.
        ExprKind::Raise(_) => Prec::Lambda,
        ExprKind::Pipe { .. } => Prec::Pipeline,
        ExprKind::Binary { op, .. } => match op {
            BinaryOp::Write => Prec::Assignment,
            BinaryOp::Or => Prec::Or,
            BinaryOp::Xor => Prec::Xor,
            BinaryOp::And => Prec::And,
            BinaryOp::Add | BinaryOp::Sub => Prec::Addition,
            BinaryOp::Mul | BinaryOp::Div => Prec::Multiplication,
        },
        ExprKind::Unary { .. } => Prec::Unary,
        // A tag carrying something groups as the application it reads as:
        // anything appended to `#A x` would be read as applying the
        // case rather than as a second argument to it. Carrying nothing it
        // is not a word but a word still waiting for one, so it groups
        // below an application — see [`Prec::Tag`].
        ExprKind::Tag {
            payload: Some(_), ..
        } => Prec::Apply,
        ExprKind::Tag { payload: None, .. } => Prec::Tag,
        ExprKind::Apply { .. } => Prec::Apply,
        // Self-delimiting: each ends at a token of its own, so nothing that
        // follows can be drawn into it. An operation is written like a
        // projection and closes itself the same way.
        ExprKind::Project { .. }
        | ExprKind::Operation { .. }
        | ExprKind::Struct { .. }
        | ExprKind::Tuple(_)
        | ExprKind::Array(_)
        | ExprKind::Ident { .. }
        | ExprKind::Natural(_)
        | ExprKind::Integer(_)
        | ExprKind::Fixed(_)
        | ExprKind::Real(_)
        | ExprKind::String(_)
        | ExprKind::Boolean(_)
        | ExprKind::Unit => Prec::Atom,
    }
}

/// Whether a written expression's surface spelling ends in a numeric
/// projection; see [`Grouped::ends_in_numeric_projection`].
pub fn expr_ends_in_numeric_projection(kind: &parse::ExprKind) -> bool {
    matches!(
        kind,
        parse::ExprKind::Project { field, .. } if canonical_tuple_index(&field.tracked).is_some()
    )
}

/// [`expr_prec`]'s twin for a written type.
pub fn type_prec(kind: &parse::TypeKind) -> Prec {
    use parse::TypeKind;
    match kind {
        TypeKind::Arrow { .. } => Prec::Arrow,
        // A row of effects binds as a sum does: it is written with the same
        // labels and the same tail, and needs the same brackets around it.
        TypeKind::Sum { .. } | TypeKind::Effects(_) => Prec::Sum,
        TypeKind::Apply { .. } | TypeKind::Mut(..) => Prec::Apply,
        TypeKind::Struct { .. }
        | TypeKind::Tuple(_)
        | TypeKind::Array(_)
        | TypeKind::Ident { .. }
        | TypeKind::Variable { .. }
        | TypeKind::Hole
        | TypeKind::Unit => Prec::Atom,
    }
}

/// [`expr_prec`]'s twin for a written pattern.
pub fn pattern_prec(kind: &parse::PatternKind) -> Prec {
    use parse::PatternKind;
    match kind {
        PatternKind::Tag {
            payload: Some(_), ..
        } => Prec::Apply,
        PatternKind::Tag { payload: None, .. } => Prec::Tag,
        PatternKind::Ident { .. }
        | PatternKind::Wildcard
        | PatternKind::Natural(_)
        | PatternKind::Integer(_)
        | PatternKind::Fixed(_)
        | PatternKind::Real(_)
        | PatternKind::String(_)
        | PatternKind::Boolean(_)
        | PatternKind::Unit
        | PatternKind::Struct { .. }
        | PatternKind::Tuple(_)
        | PatternKind::Array { .. } => Prec::Atom,
    }
}

/// Renders a symbol as `bundle::module::name`. Kept separate from [`Symbol`]
/// because printing one needs the mint that made it; [`Mint::path`] is how one
/// is made.
pub struct Path<'a> {
    mint: &'a Mint,
    symbol: Symbol,
}

impl<'a> Path<'a> {
    pub(crate) fn new(mint: &'a Mint, symbol: Symbol) -> Self {
        Self { mint, symbol }
    }
}

/// An effect row lifted out of its arrow, in the same notation the arrow uses.
/// Constraints need this wrapper because a bare [`Row`] has no shape of its
/// own and therefore falls back to struct notation.
struct EffectLabels<'a> {
    row: &'a Row,
}

/// One label of a written row, as [`write_row`] and [`write_sum`] render it:
/// written out with what it holds, or explicitly absent — the `\name`
/// spelling, which writes no type, no payload and no mark.
///
/// `V` is what follows a written label: a field's type for [`write_row`], and
/// a case's optional payload for [`write_sum`]. An absent label follows with
/// nothing whichever shape it is in, which is why the variant carries only the
/// name.
#[derive(Debug)]
pub enum Entry<K, V> {
    Written {
        name: K,
        mark: Option<Mark>,
        holds: V,
    },
    Absent {
        name: K,
    },
}

/// How a label wears a presence that is neither certainly there nor certainly
/// absent.
///
/// Two answers where there used to be one. The `?` retired as *syntax* — no
/// source spells it any more — but it survives as the one thing the printer
/// still has to be able to say about a presence a failure abandoned. Everything
/// else a presence can be has a name, and prints as the `when` clause that
/// names it.
#[derive(Debug, Clone)]
pub enum Mark {
    /// `?` — the undecided-presence failure artifact. Nothing parses it, so a
    /// type wearing one does not read back, and that is exactly what it is
    /// reporting.
    Undecided,
    /// `when a`, or `when ?3` for one the solve still owns: the presence has a
    /// name, and the `where` clause beside the type can talk about it.
    When(String),
}

/// A formula written in the reader's own nouns.
///
/// The `where` clause of a *scheme* names presence variables, because that is
/// what the type's `when` clauses bound. A complaint about a use site names the
/// labels instead — "this value needs `x != y` among its fields" — because the
/// reader wrote `x` and `y` and never saw the variable that decides them. Same
/// formula, two vocabularies, one writer.
struct Named<'a> {
    formula: &'a Formula,
    /// Which label each presence decides, where a label decides one. Empty
    /// spells every atom as the presence itself, which is what a scheme's
    /// clause wants.
    labels: &'a [(String, Presence)],
}

/// One piece of source text explained by a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub span: Span,
    pub message: String,
}

/// A compiler complaint before a terminal, browser, or editor lays it out.
///
/// Compiler phases keep deciding facts, this module turns those facts into
/// reader-facing words, and reporters only decide presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub title: String,
    pub primary: Annotation,
    pub related: Vec<Annotation>,
    pub help: Vec<String>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    fn new(code: &'static str, title: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            title: title.into(),
            primary: Annotation {
                span,
                message: String::new(),
            },
            related: Vec::new(),
            help: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn label(mut self, message: impl Into<String>) -> Self {
        self.primary.message = message.into();
        self
    }

    fn related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related.push(Annotation {
            span,
            message: message.into(),
        });
        self
    }

    fn help(mut self, message: impl Into<String>) -> Self {
        self.help.push(message.into());
        self
    }

    fn note(mut self, message: impl Into<String>) -> Self {
        self.notes.push(message.into());
        self
    }
}

impl token::ErrorKind {
    /// A stable, greppable name for this kind of error. Reporters key on it
    /// rather than on the message, which is prose and may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            token::ErrorKind::InvalidCharacter { .. } => "character-not-used",
            token::ErrorKind::MalformedTag => "tag-needs-name",
            token::ErrorKind::MalformedEffectLabel => "effect-needs-name",
            token::ErrorKind::MalformedVariable => "variable-needs-name",
            token::ErrorKind::MalformedAttribute => "attribute-needs-name",
            token::ErrorKind::NumberFollowedByName => "number-joined-to-name",
            token::ErrorKind::DecimalWithWholeSuffix { .. } => "decimal-marked-whole",
            token::ErrorKind::MalformedNumericField => "invalid-field-number",
            token::ErrorKind::NegativeNatural => "negative-natural",
            token::ErrorKind::NaturalTooLarge => "whole-number-too-large",
            token::ErrorKind::FixedOutOfRange { .. } => "fixed-integer-out-of-range",
            token::ErrorKind::IntegerTooLarge => "integer-too-large",
            token::ErrorKind::RealTooLarge => "number-too-large",
            token::ErrorKind::NumericFieldTooLarge => "field-number-too-large",
            token::ErrorKind::UnknownStringEscape { .. } => "unknown-string-escape",
            token::ErrorKind::MissingClosingQuote => "missing-closing-quote",
            token::ErrorKind::MissingClosingComment => "missing-closing-comment",
        }
    }
}

impl token::Error {
    pub fn diagnostic(&self) -> Diagnostic {
        use token::ErrorKind as E;
        match self.kind {
            E::InvalidCharacter { character } => Diagnostic::new(
                self.kind.code(),
                format!("Ruddy does not use `{character}`"),
                self.span,
            )
            .label("remove this character"),
            E::MalformedTag => Diagnostic::new(
                self.kind.code(),
                "`#` must be followed by a name or quoted text",
                self.span,
            )
            .label("the name cannot start here"),
            E::MalformedEffectLabel => Diagnostic::new(
                self.kind.code(),
                "`!` must be followed by a name",
                self.span,
            )
            .label("the name cannot start here"),
            E::MalformedVariable => Diagnostic::new(
                self.kind.code(),
                "`'` must be followed by a name",
                self.span,
            )
            .label("the name cannot start here"),
            E::MalformedAttribute => Diagnostic::new(
                self.kind.code(),
                "`@` must be followed by a name",
                self.span,
            )
            .label("the name cannot start here")
            .help("metadata is written `@key` or `@key value` in front of a definition"),
            E::NumberFollowedByName => Diagnostic::new(
                self.kind.code(),
                "a number cannot run directly into a name",
                self.span,
            )
            .label("the number and name are joined")
            .help("add a space, or start the whole name with a letter"),
            E::DecimalWithWholeSuffix { suffix } => Diagnostic::new(
                self.kind.code(),
                format!("`{suffix}` is only used with whole numbers"),
                self.span,
            )
            .label("this number has a decimal point")
            .help(format!("remove the decimal part or remove `{suffix}`")),
            E::MalformedNumericField => Diagnostic::new(
                self.kind.code(),
                "a numbered field can contain only digits",
                self.span,
            )
            .label("this is not a field number")
            .help("remove the suffix or decimal part"),
            E::NegativeNatural => Diagnostic::new(
                self.kind.code(), "a natural literal cannot have a minus sign", self.span,
            ).help("remove the minus sign or use an integer suffix (`i`, `i8`, `i16`, `i32`, or `i64`)"),
            E::NaturalTooLarge => Diagnostic::new(
                self.kind.code(),
                "this whole number is too large",
                self.span,
            )
            .help("the largest allowed value is `18446744073709551615n`"),
            E::FixedOutOfRange { kind } => Diagnostic::new(
                self.kind.code(),
                format!("this literal is outside the range of `{}`", kind.name()),
                self.span,
            )
            .help(format!(
                "allowed values run from `{}{}` to `{}{}`",
                kind.min(),
                kind.suffix(),
                kind.max(),
                kind.suffix()
            )),
            E::IntegerTooLarge => {
                Diagnostic::new(self.kind.code(), "this integer is too large", self.span)
                    .help("allowed values run from `-9223372036854775808i` to `9223372036854775807i`")
            }
            E::RealTooLarge => {
                Diagnostic::new(self.kind.code(), "this number is too large", self.span)
            }
            E::NumericFieldTooLarge => Diagnostic::new(
                self.kind.code(),
                "this field number is too large",
                self.span,
            ),
            E::UnknownStringEscape { escape } => Diagnostic::new(
                self.kind.code(),
                format!("`\\{escape}` has no special meaning in a string"),
                self.span,
            )
            .label("this escape is not supported")
            .help("use `\\\"`, `\\\\`, `\\n`, `\\r`, or `\\t`"),
            E::MissingClosingQuote => {
                let mut diagnostic = Diagnostic::new(
                    self.kind.code(),
                    "this string is missing its closing `\"`",
                    self.span.file_id.span(self.span.end(), 0),
                )
                .label("add `\"` here")
                .help(
                    "strings must start and finish on the same line; \
                     for one that spans lines, begin each line with `\\\\`",
                );
                diagnostic.related.push(Annotation {
                    span: self.span.file_id.span(self.span.start, 1),
                    message: "the string starts here".into(),
                });
                diagnostic
            }
            E::MissingClosingComment => {
                let mut diagnostic = Diagnostic::new(
                    self.kind.code(),
                    "this comment is missing its closing `*)`",
                    self.span.file_id.span(self.span.end(), 0),
                )
                .label("add `*)` here")
                .help("every `(*` needs a `*)` of its own, including ones nested inside it");
                diagnostic.related.push(Annotation {
                    span: self.span.file_id.span(self.span.start, 2),
                    message: "the comment starts here".into(),
                });
                diagnostic
            }
        }
    }
}

/// Whether a decoded label has the lexical shape of a sigilled name.
/// Keywords are allowed behind `#`: the sigil has already told the lexer that
/// `#let` is a tag rather than the `let` token.
fn bare_tag(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_alphabetic()) && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Whether a decoded label can be written as an ordinary identifier token.
/// This mirrors the lexer's Unicode identifier rule and excludes every word
/// that lexes as a keyword or another dedicated token.
fn bare_identifier(name: &str) -> bool {
    bare_tag(name)
        && !matches!(
            name,
            "_" | "let"
                | "extern"
                | "do"
                | "return"
                | "if"
                | "then"
                | "else"
                | "type"
                | "end"
                | "with"
                | "match"
                | "fn"
                | "effect"
                | "handle"
                | "raise"
                | "and"
                | "or"
                | "xor"
                | "not"
                | "module"
                | "true"
                | "false"
        )
}

/// Write one decoded Ruddy string with the escapes accepted by the lexer.
pub fn write_string(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in value.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("\"")
}

/// Write a field label in its shortest unambiguous source spelling.
pub fn write_field_label(f: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    let canonical_numeric = name
        .parse::<u64>()
        .is_ok_and(|value| value.to_string() == name);
    if bare_identifier(name) || canonical_numeric {
        f.write_str(name)
    } else {
        write_string(f, name)
    }
}

/// Write a union tag, including its sigil, in canonical source spelling.
pub fn write_tag_label(f: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    f.write_str("#")?;
    if bare_tag(name) {
        f.write_str(name)
    } else {
        write_string(f, name)
    }
}

impl fmt::Display for token::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let error = token::Error {
            span: Span::default(),
            kind: *self,
        };
        f.write_str(&error.diagnostic().title)
    }
}

/// Every token has a spelling, and a token prints as the one it was written
/// with. Injective on the fixed kinds, so a printed stream re-lexes to the
/// tokens it was printed from.
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Let => f.write_str("let"),
            Kind::Extern => f.write_str("extern"),
            Kind::Do => f.write_str("do"),
            Kind::Return => f.write_str("return"),
            Kind::Type => f.write_str("type"),
            Kind::End => f.write_str("end"),
            Kind::With => f.write_str("with"),
            Kind::Match => f.write_str("match"),
            Kind::If => f.write_str("if"),
            Kind::Then => f.write_str("then"),
            Kind::Else => f.write_str("else"),
            Kind::Fn => f.write_str("fn"),
            Kind::Effect => f.write_str("effect"),
            Kind::Handle => f.write_str("handle"),
            Kind::Raise => f.write_str("raise"),
            Kind::And => f.write_str("and"),
            Kind::Or => f.write_str("or"),
            Kind::Xor => f.write_str("xor"),
            Kind::Mut => f.write_str("mut"),
            Kind::Tilde => f.write_str("~"),
            Kind::Assign => f.write_str(":="),
            Kind::Not => f.write_str("not"),
            Kind::Module => f.write_str("module"),
            Kind::Equal => f.write_str("="),
            Kind::FatArrow => f.write_str("=>"),
            Kind::Arrow => f.write_str("->"),
            Kind::Colon => f.write_str(":"),
            Kind::ColonColon => f.write_str("::"),
            Kind::Comma => f.write_str(","),
            Kind::Semicolon => f.write_str(";"),
            Kind::Dot => f.write_str("."),
            Kind::DotDot => f.write_str(".."),
            Kind::NotEqual => f.write_str("!="),
            Kind::Plus => f.write_str("+"),
            Kind::Minus => f.write_str("-"),
            Kind::Star => f.write_str("*"),
            Kind::Slash => f.write_str("/"),
            Kind::Backslash => f.write_str("\\"),
            Kind::Pipe => f.write_str("|"),
            Kind::PipeForward => f.write_str("|>"),
            // The sigil is written back on: it is how the token was spelled,
            // and either of these printing as a bare name would re-lex as an
            // identifier.
            Kind::Tag(name) => write_tag_label(f, name),
            Kind::EffectLabel(name) => write!(f, "!{name}"),
            Kind::Variable(name) => write!(f, "'{name}"),
            Kind::Attribute(name) => write!(f, "@{name}"),
            Kind::LeftBrace => f.write_str("{"),
            Kind::RightBrace => f.write_str("}"),
            Kind::LeftBracket => f.write_str("["),
            Kind::RightBracket => f.write_str("]"),
            Kind::LeftParen => f.write_str("("),
            Kind::RightParen => f.write_str(")"),
            Kind::Identifier(name) => f.write_str(name),
            Kind::Underscore => f.write_str("_"),
            Kind::Natural(value) => write!(f, "{value}n"),
            Kind::Fixed(value) => write!(f, "{value}"),
            Kind::Integer(value) => write!(f, "{value}i"),
            Kind::NumericField(value) => write!(f, "{value}"),
            Kind::Real(value) => write!(f, "{value}"),
            Kind::String(value) => write_string(f, value),
            Kind::Boolean(value) => write!(f, "{value}"),
            // The delimiters are written back on, as a string's quotes are:
            // the text alone would re-lex as code.
            Kind::LineComment(text) => write!(f, "--{text}"),
            Kind::BlockComment(text) => write!(f, "(*{text}*)"),
            // Invalid input is never printed as source: its original spelling
            // remains in the file and its diagnostic owns the explanation.
            Kind::Invalid => f.write_str("<invalid>"),
        }
    }
}

impl parse::Expected {
    fn words(self) -> String {
        match self {
            Self::Statement => "a definition".into(),
            Self::Name => "a name".into(),
            Self::Value => "a value".into(),
            Self::Type => "a type".into(),
            Self::Pattern => "a name or shape to take apart".into(),
            Self::Field => "a field name".into(),
            Self::Argument => "at least one argument name".into(),
            Self::Clause => "a condition".into(),
            Self::Effect => "an effect name".into(),
            Self::Case => "another case".into(),
            Self::FunctionType => "a function type such as `Nat -> Nat`".into(),
            Self::EndOfClause => "the end of this condition".into(),
            Self::ExternTarget => "a quoted JavaScript expression such as `\"host.log\"`".into(),
            Self::Keyword(word) | Self::Punctuation(word) => format!("`{word}`"),
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Statement => "expected-definition",
            Self::Name => "expected-name",
            Self::Value => "expected-value",
            Self::Type => "expected-type",
            Self::Pattern => "expected-name-or-shape",
            Self::Field => "expected-field-name",
            Self::Argument => "expected-argument",
            Self::Clause => "expected-condition",
            Self::Effect => "expected-effect",
            Self::Case => "expected-case",
            Self::FunctionType => "expected-function-type",
            Self::EndOfClause => "expected-condition-end",
            Self::ExternTarget => "expected-extern-target",
            Self::Keyword("=") | Self::Punctuation("=") => "expected-equals",
            Self::Keyword("=>") | Self::Punctuation("=>") => "expected-function-arrow",
            Self::Keyword("end") => "expected-end",
            Self::Keyword("with") => "expected-with",
            Self::Keyword("then") => "expected-then",
            Self::Keyword("else") => "expected-else",
            Self::Keyword(_) => "expected-word",
            Self::Punctuation(")") => "expected-closing-parenthesis",
            Self::Punctuation("}") => "expected-closing-brace",
            Self::Punctuation(_) => "expected-punctuation",
        }
    }
}

impl parse::Error {
    /// A stable, greppable name for this kind of error.
    pub fn code(&self) -> &'static str {
        match self.kind {
            parse::ErrorKind::Expected { expected, .. } => expected.code(),
            parse::ErrorKind::Wildcard { .. } => "misplaced-discard",
            parse::ErrorKind::SecondArrayRest { .. } => "second-array-rest",
            parse::ErrorKind::DiscardedArrayRest => "discarded-array-rest",
            parse::ErrorKind::SecondStructSpread { .. } => "second-struct-spread",
            parse::ErrorKind::FieldAfterSpread { .. } => "field-after-spread",
            parse::ErrorKind::StatementAfterReturn { .. } => "statement-after-return",
            parse::ErrorKind::BareReturn => "bare-return",
            parse::ErrorKind::ReturnOutsideBlock => "return-outside-block",
            parse::ErrorKind::DeclarationInBlock { .. } => "declaration-in-block",
            parse::ErrorKind::AttributeWithoutDefinition => "metadata-without-definition",
            parse::ErrorKind::MetadataNotLiteral => "metadata-not-literal",
            parse::ErrorKind::AttributeInBlock => "metadata-in-block",
            parse::ErrorKind::AttributeInExpression => "metadata-in-expression",
        }
    }

    pub fn diagnostic(&self) -> Diagnostic {
        match self.kind {
            parse::ErrorKind::Expected {
                expected,
                found,
                related,
                context,
            } => {
                let words = expected.words();
                let title = match (expected, related, context) {
                    (
                        parse::Expected::Punctuation("|"),
                        _,
                        Some(parse::Related {
                            kind: parse::RelatedKind::Construct("match"),
                            ..
                        }),
                    ) => "each match case must begin with `|`".into(),
                    (
                        parse::Expected::Keyword("end"),
                        _,
                        Some(parse::Related {
                            kind: parse::RelatedKind::Construct(name),
                            ..
                        }),
                    ) => format!("this {name} needs a closing `end`"),
                    (
                        parse::Expected::Punctuation(mark),
                        Some(parse::Related {
                            kind: parse::RelatedKind::Opener,
                            ..
                        }),
                        _,
                    ) => format!("add `{mark}` to close this part"),
                    (
                        _,
                        Some(parse::Related {
                            kind: parse::RelatedKind::Operator | parse::RelatedKind::Separator,
                            ..
                        }),
                        _,
                    ) => format!("write {words} after this"),
                    _ => format!("expected {words} here"),
                };
                let mut diagnostic = Diagnostic::new(expected.code(), title, self.span);
                diagnostic.primary.message = match (expected, found, context) {
                    (parse::Expected::Punctuation("|"), parse::Found::Token, Some(_)) => {
                        "add `|` before this case".into()
                    }
                    (parse::Expected::Keyword("end"), parse::Found::Token, Some(_)) => {
                        "write `end` before this".into()
                    }
                    (parse::Expected::Punctuation(mark), parse::Found::Token, _) => {
                        format!("write `{mark}` before this")
                    }
                    (_, parse::Found::End, _) => format!("write {words} here"),
                    (_, parse::Found::Token, _) => "this cannot be used here".into(),
                };
                if let Some(related) = related
                    && !(matches!(expected, parse::Expected::Keyword("end"))
                        && context.is_some()
                        && matches!(related.kind, parse::RelatedKind::Anchor))
                {
                    diagnostic.related.push(Annotation {
                        span: related.span,
                        message: match related.kind {
                            parse::RelatedKind::Opener => "opened here".into(),
                            parse::RelatedKind::Separator => {
                                "this mark promises more after it".into()
                            }
                            parse::RelatedKind::Operator => {
                                "this operation needs a value after it".into()
                            }
                            parse::RelatedKind::Anchor => "the missing part belongs here".into(),
                            parse::RelatedKind::Construct(name) => {
                                format!("this {name} starts here")
                            }
                        },
                    });
                }
                if let Some(context) = context {
                    diagnostic.related.push(Annotation {
                        span: context.span,
                        message: match context.kind {
                            parse::RelatedKind::Construct(name) => {
                                format!("this {name} starts here")
                            }
                            _ => "this part starts here".into(),
                        },
                    });
                }
                diagnostic
            }
            parse::ErrorKind::Wildcard { place } => {
                let title = match place {
                    parse::Place::Value => "`_` throws a value away, so it cannot be read here",
                    parse::Place::Field => "a field needs a name other than `_`",
                    parse::Place::Pun => "write a field name, or give the field a value after `:`",
                    parse::Place::Projection => "write the name of the field to read",
                    parse::Place::Type => "this place needs a name rather than `_`",
                };
                Diagnostic::new("misplaced-discard", title, self.span)
                    .label("`_` does not provide a name here")
            }
            parse::ErrorKind::SecondArrayRest { previous } => Diagnostic::new(
                "second-array-rest",
                "an array pattern can only have one `..`",
                self.span,
            )
            .label("a second `..`")
            .related(previous, "the first `..`")
            .help("keep one `..` and name the elements on either side of it"),
            parse::ErrorKind::DiscardedArrayRest => Diagnostic::new(
                "discarded-array-rest",
                "write `..` on its own to skip the rest",
                self.span,
            )
            .label("`..` already skips the rest; the `_` adds nothing"),
            parse::ErrorKind::SecondStructSpread { previous } => Diagnostic::new(
                "second-struct-spread",
                "a struct can only spread one value",
                self.span,
            )
            .label("a second `..`")
            .related(previous, "the first `..`")
            .help("keep one `..` and write the other value's fields by name"),
            parse::ErrorKind::FieldAfterSpread { spread } => Diagnostic::new(
                "field-after-spread",
                "a struct's `..` comes after its last field",
                self.span,
            )
            .label("this field comes after the `..`")
            .related(spread, "the `..`")
            .help("move the `..` after the last field"),
            parse::ErrorKind::StatementAfterReturn { returned } => Diagnostic::new(
                "statement-after-return",
                "`return` must be the last thing in its `do` block",
                self.span,
            )
            .label("this comes after the `return`")
            .related(returned, "the `return`")
            .help("move this above the `return`, or take it out of the block"),
            parse::ErrorKind::BareReturn => {
                Diagnostic::new("bare-return", "`return` needs a value after it", self.span)
                    .label("nothing follows this `return`")
                    .help("write the value to return, or leave the `return` out to return `()`")
            }
            parse::ErrorKind::ReturnOutsideBlock => Diagnostic::new(
                "return-outside-block",
                "`return` can only end a `do` block",
                self.span,
            )
            .label("this `return` is not at the end of a `do` block")
            .help("write it as the last thing in a `do ... end` block, or leave the `return` out"),
            parse::ErrorKind::DeclarationInBlock { keyword } => Diagnostic::new(
                "declaration-in-block",
                "a `do` block can only hold `let` definitions",
                self.span,
            )
            .label(format!("a `{keyword}` cannot be written inside a block"))
            .help(format!("move this `{keyword}` out to the file or module")),
            parse::ErrorKind::AttributeWithoutDefinition => Diagnostic::new(
                "metadata-without-definition",
                "this metadata has no definition to describe",
                self.span,
            )
            .label("nothing that this could describe follows it")
            .help("write a `let`, `extern`, `type`, `effect`, or `module` after it, or remove it"),
            parse::ErrorKind::MetadataNotLiteral => Diagnostic::new(
                "metadata-not-literal",
                "a metadata value must be a literal",
                self.span,
            )
            .label("this is not a literal")
            .help(
                "write a string, number, boolean, or tag, or a tuple, array, or struct of those; \
                 metadata is data and cannot be computed",
            ),
            parse::ErrorKind::AttributeInBlock => Diagnostic::new(
                "metadata-in-block",
                "metadata belongs to a top-level definition",
                self.span,
            )
            .label("a `let` inside a `do` block cannot carry metadata")
            .help("remove this, or move the definition out to the file or module"),
            parse::ErrorKind::AttributeInExpression => Diagnostic::new(
                "metadata-in-expression",
                "metadata goes in front of a definition",
                self.span,
            )
            .label("this is written where a value goes")
            .help("move it in front of the `let`, `extern`, `type`, `effect`, or `module` it describes"),
        }
    }
}

impl fmt::Display for parse::Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.diagnostic().title)
    }
}

/// How much of the surface grammar a pattern can be, on the same ladder the
/// expressions it mirrors use: a bare tag is still waiting for its payload, a
/// tag carrying one reads as the application it looks like, and everything
/// else closes itself.
impl Grouped for parse::PatternKind {
    fn prec(&self) -> Prec {
        match self {
            parse::PatternKind::Tag {
                payload: Some(_), ..
            } => Prec::Apply,
            parse::PatternKind::Tag { payload: None, .. } => Prec::Tag,
            parse::PatternKind::Ident { .. }
            | parse::PatternKind::Wildcard
            | parse::PatternKind::Natural(_)
            | parse::PatternKind::Fixed(_)
            | parse::PatternKind::Integer(_)
            | parse::PatternKind::Real(_)
            | parse::PatternKind::String(_)
            | parse::PatternKind::Boolean(_)
            | parse::PatternKind::Unit
            | parse::PatternKind::Struct { .. }
            | parse::PatternKind::Tuple(_)
            | parse::PatternKind::Array { .. } => Prec::Atom,
        }
    }
}

/// A pattern prints as it was written, so a printed match or pattern `let`
/// re-parses to the tree it was printed from. A pun stays a pun — the field's
/// name alone — and a tag's payload is grouped by the same rule a tag
/// expression's is.
impl fmt::Display for parse::PatternKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            parse::PatternKind::Ident { name } => f.write_str(&name.tracked),
            parse::PatternKind::Wildcard => f.write_str("_"),
            parse::PatternKind::Natural(value) => write!(f, "{value}n"),
            parse::PatternKind::Fixed(value) => write!(f, "{value}"),
            parse::PatternKind::Integer(value) => write!(f, "{value}i"),
            parse::PatternKind::Real(value) => write!(f, "{value}"),
            parse::PatternKind::String(value) => write_string(f, value),
            parse::PatternKind::Boolean(value) => write!(f, "{value}"),
            parse::PatternKind::Unit => f.write_str("()"),
            parse::PatternKind::Tuple(elements) => {
                write_tuple(f, elements.iter().map(|element| &element.tracked))
            }
            parse::PatternKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_deref().map(|payload| &payload.tracked),
            ),
            parse::PatternKind::Struct { fields, rest } => {
                if fields.is_empty() && rest.is_none() {
                    return f.write_str("()");
                }
                if rest.is_none()
                    && let Some(order) =
                        tuple_field_order(fields.keys().map(|name| name.tracked.as_str()))
                    && fields.values().all(Option::is_some)
                {
                    return write_tuple(
                        f,
                        order.into_iter().map(|insertion| {
                            &fields
                                .get_index(insertion)
                                .expect("tuple field index")
                                .1
                                .as_ref()
                                .expect("tuple fields were checked as explicit")
                                .tracked
                        }),
                    );
                }
                f.write_str("{ ")?;
                let mut first = true;
                for (name, sub) in fields {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    write_field_label(f, &name.tracked)?;
                    if let Some(sub) = sub {
                        write!(f, ": {}", sub.tracked)?;
                    }
                }
                // The `..` that makes the pattern open, last as it was
                // written: what it stands for has no order among the named
                // fields to claim.
                if rest.is_some() {
                    if !first {
                        f.write_str(", ")?;
                    }
                    f.write_str("..")?;
                }
                f.write_str(" }")
            }
            parse::PatternKind::Array {
                before,
                rest,
                after,
            } => write_array_pattern(
                f,
                before.iter().map(|element| &element.tracked),
                rest.as_ref()
                    .map(|rest| rest.name.as_ref().map(|name| name.tracked.as_str())),
                after.iter().map(|element| &element.tracked),
            ),
        }
    }
}

/// A witness groups by what it prints as: a case carrying something reads as
/// the application it looks like, a bare one is still waiting for a payload,
/// and the two prose forms — "anything", "anything other than …" — group
/// lowest, so a payload position always brackets them and they never read as
/// part of what follows.
impl Grouped for ir::Witness {
    fn prec(&self) -> Prec {
        match self {
            ir::Witness::Tag {
                payload: Some(_), ..
            } => Prec::Apply,
            ir::Witness::Tag { payload: None, .. } => Prec::Tag,
            ir::Witness::Natural(_)
            | ir::Witness::Literal(_)
            | ir::Witness::Struct(_)
            | ir::Witness::Array { .. } => Prec::Atom,
            ir::Witness::Any | ir::Witness::Other(_) => Prec::Lambda,
        }
    }
}

/// A witness prints as the example value it is, in source syntax, so the
/// unhandled-values complaint shows the reader something they could write an
/// arm for. The two forms no value literal spells — a position anything
/// serves for, and an open row's "anything else" — print as the plain English
/// they mean.
impl fmt::Display for ir::Witness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ir::Witness::Any => f.write_str("anything"),
            ir::Witness::Natural(value) => write!(f, "{value}n"),
            ir::Witness::Literal(value) => match value {
                ir::Literal::Natural(value) => write!(f, "{value}n"),
                ir::Literal::Fixed(value) => write!(f, "{value}"),
                ir::Literal::Integer(value) => write!(f, "{value}i"),
                ir::Literal::Real(value) => write!(f, "{value}"),
                ir::Literal::String(value) => write_string(f, value),
                ir::Literal::Boolean(value) => write!(f, "{value}"),
            },
            ir::Witness::Tag { name, payload } => write_tag(f, name, None, payload.as_deref()),
            // A field held to be present with any value at all prints
            // pun-style — under exactness the presence *is* the information,
            // so `{a}` says what `{}` would deny.
            ir::Witness::Struct(fields) => {
                if fields.is_empty() {
                    return f.write_str("{}");
                }
                f.write_str("{ ")?;
                for (at, (name, witness)) in fields.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    write_field_label(f, name)?;
                    if !matches!(witness, ir::Witness::Any) || !bare_identifier(name) {
                        write!(f, ": {witness}")?;
                    }
                }
                f.write_str(" }")
            }
            ir::Witness::Other(cases) => {
                f.write_str("anything other than ")?;
                for (at, case) in cases.iter().enumerate() {
                    if at > 0 {
                        f.write_str(" or ")?;
                    }
                    write_tag_label(f, case)?;
                }
                Ok(())
            }
            // An element any value serves for prints as the `_` an arm for it
            // would be written with, and the lengths past the named ones as
            // the `..` that would take them.
            ir::Witness::Array { elements, open } => {
                f.write_str("[")?;
                for (at, element) in elements.iter().enumerate() {
                    if at > 0 {
                        f.write_str(", ")?;
                    }
                    match element {
                        ir::Witness::Any => f.write_str("_")?,
                        element => write!(f, "{element}")?,
                    }
                }
                if *open {
                    if !elements.is_empty() {
                        f.write_str(", ")?;
                    }
                    f.write_str("..")?;
                }
                f.write_str("]")
            }
        }
    }
}

impl bundle::ErrorKind {
    /// A stable, greppable name for this kind of error, the way every other
    /// phase's are coded.
    pub fn code(&self) -> &'static str {
        match self {
            bundle::ErrorKind::ModuleFileMissing { .. } => "module-file-missing",
            bundle::ErrorKind::ModuleFileAmbiguous { .. } => "module-file-ambiguous",
            bundle::ErrorKind::ConditionMissing => "condition-missing",
            bundle::ErrorKind::ConditionNotStruct => "condition-not-struct",
            bundle::ErrorKind::ConditionUnknownField { .. } => "condition-unknown-field",
            bundle::ErrorKind::ConditionNotString { .. } => "condition-not-string",
        }
    }
}

impl bundle::Error {
    /// Describe a module-file failure using paths relative to the bundle root.
    pub fn diagnostic(&self) -> Diagnostic {
        self.diagnostic_in(FsPath::new(""))
    }

    /// Describe a module-file failure using paths relative to `directory`.
    /// Filesystem reporters use this to name paths from the project root while
    /// in-memory reporters keep the loader's bundle-relative spellings.
    pub fn diagnostic_in(&self, directory: &FsPath) -> Diagnostic {
        let path = |candidate: &str| directory.join(candidate).display().to_string();
        let convention = "a module written without a body uses either `Name.rud` or `Name/module.rud`, and exactly one of them must exist";
        match &self.kind {
            bundle::ErrorKind::ModuleFileMissing { beside, inside } => {
                Diagnostic::new(self.kind.code(), "this module needs a file", self.span)
                    .label("no file was found for this module")
                    .help(format!("create `{}` or `{}`", path(beside), path(inside)))
                    .note(convention)
            }
            bundle::ErrorKind::ModuleFileAmbiguous { beside, inside } => Diagnostic::new(
                self.kind.code(),
                "this module has two possible files",
                self.span,
            )
            .label("Ruddy cannot choose which file defines this module")
            .help(format!(
                "keep one of `{}` or `{}` and delete the other",
                path(beside),
                path(inside)
            ))
            .note(convention),
            bundle::ErrorKind::ConditionMissing => {
                Diagnostic::new(self.kind.code(), "`@if` needs a condition", self.span)
                    .label("nothing here says when the definition is compiled")
                    .help("write a struct of conditions, such as `@if {target: \"js\"}`")
                    .note(CONDITIONS)
            }
            bundle::ErrorKind::ConditionNotStruct => Diagnostic::new(
                self.kind.code(),
                "an `@if` condition is a struct",
                self.span,
            )
            .label("this value is not a struct")
            .help("write the condition as a field, such as `@if {target: \"js\"}`")
            .note(CONDITIONS),
            bundle::ErrorKind::ConditionUnknownField { name, known } => {
                Diagnostic::new(self.kind.code(), "this condition is not known", self.span)
                    .label(format!("`{name}` is not a fact of the build"))
                    .help(format!(
                        "the facts are {}",
                        known
                            .iter()
                            .map(|fact| format!("`{fact}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .note(CONDITIONS)
            }
            bundle::ErrorKind::ConditionNotString { name } => Diagnostic::new(
                self.kind.code(),
                "a condition's value is a string",
                self.span,
            )
            .label("this value is not a string")
            .help(format!("write `{name}` as a string, in quotes"))
            .note(CONDITIONS),
        }
    }
}

impl fmt::Display for bundle::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            bundle::ErrorKind::ModuleFileMissing { .. } => "this module needs a file",
            bundle::ErrorKind::ModuleFileAmbiguous { .. } => "this module has two possible files",
            bundle::ErrorKind::ConditionMissing => "`@if` needs a condition",
            bundle::ErrorKind::ConditionNotStruct => "an `@if` condition is a struct",
            bundle::ErrorKind::ConditionUnknownField { .. } => "this condition is not known",
            bundle::ErrorKind::ConditionNotString { .. } => "a condition's value is a string",
        })
    }
}

impl ir::ErrorKind {
    /// A stable, greppable name for this kind of error. The namespace is part
    /// of the code rather than only of the message: a reporter that wants to
    /// treat an undefined type differently from an undefined term should not
    /// have to re-inspect the variant to tell them apart.
    pub fn code(&self) -> &'static str {
        match self {
            ir::ErrorKind::ForeignProtocol { .. } => "foreign-protocol",
            ir::ErrorKind::ArrayInExtern => "array-in-extern",
            ir::ErrorKind::InvalidDependencyAlias { .. } => "invalid-dependency-alias",
            ir::ErrorKind::ExecutableDependency { .. } => "executable-dependency",
            ir::ErrorKind::DuplicateDependencyAlias { .. } => "duplicate-dependency-alias",
            ir::ErrorKind::DuplicateDependency { .. } => "duplicate-dependency",
            ir::ErrorKind::Undefined { namespace, .. } => match namespace {
                Namespace::Types => "undefined-type",
                Namespace::Effects => "undefined-effect",
                Namespace::Modules => "undefined-module",
                Namespace::Terms => "undefined-term",
            },
            ir::ErrorKind::Duplicate { namespace, .. } => match namespace {
                Namespace::Types => "duplicate-type",
                Namespace::Effects => "duplicate-effect",
                Namespace::Modules => "duplicate-module",
                Namespace::Terms => "duplicate-term",
            },
            ir::ErrorKind::DuplicateField { .. } => "duplicate-field",
            ir::ErrorKind::DuplicateAttribute { .. } => "duplicate-metadata-key",
            ir::ErrorKind::MetadataTooDeep => "metadata-too-deep",
            ir::ErrorKind::InvalidPrivateValue => "invalid-private-value",
            // The shape is not part of the code, for the reason a repeated row
            // field's is not: the wording quotes the label the way it was
            // written, and that already says which kind of row it sits in.
            ir::ErrorKind::AbsentInClosed { .. } => "absent-in-closed",
            // An effect never reaches here: nothing about one leads back to
            // itself through names alone, and a circular alias stands for a
            // perfectly good set of effects.
            ir::ErrorKind::Circular { namespace } => match namespace {
                Namespace::Types => "circular-type",
                Namespace::Terms | Namespace::Effects | Namespace::Modules => "circular-term",
            },
            ir::ErrorKind::OpenDeclaredType { .. } => "open-declared-type",
            ir::ErrorKind::ClauseInDeclaration => "declared-where-clause",
            ir::ErrorKind::VariableInDeclaration { .. } => "variable-in-declaration",
            ir::ErrorKind::HoleInDeclaration => "hole-in-declaration",
            ir::ErrorKind::HoleInOperation => "hole-in-operation",
            // The name is not part of the code, only of the wording: what went
            // wrong is that the clause names something no label wears, and
            // which name it was is the span's to show. The two below say the
            // same about theirs.
            ir::ErrorKind::UnboundPresence { .. } => "unbound-presence",
            ir::ErrorKind::IncompatiblePresenceOwnership { .. } => {
                "incompatible-presence-ownership"
            }
            ir::ErrorKind::Arity { .. } => "wrong-argument-count",
            ir::ErrorKind::EffectArity { .. } => "effect-arity",
            ir::ErrorKind::AliasCycle { growing: false, .. } => "alias-cycle",
            ir::ErrorKind::AliasCycle { growing: true, .. } => "growing-alias-cycle",
            ir::ErrorKind::ModifiedOpenAlias { .. } => "modified-open-alias",
            ir::ErrorKind::TwoTails { .. } => "two-tails",
            ir::ErrorKind::NotAConstructor => "not-a-type-constructor",
            ir::ErrorKind::ParameterApplied { .. } => "applied-parameter",
            ir::ErrorKind::DuplicateParameter { .. } => "duplicate-parameter",
            ir::ErrorKind::GrowingRecursion => "growing-recursion",
            ir::ErrorKind::DuplicateCase { .. } => "duplicate-case",
            // The shape is not part of these codes, only of their wording: a
            // reporter that treats a struct's row differently from a sum's is
            // reading the type, not the complaint. The namespace on
            // [`ir::ErrorKind::Undefined`] is the other way round, and says so.
            ir::ErrorKind::MixedTail { .. } => "mixed-tail",
            ir::ErrorKind::MixedParameter { .. } => "mixed-parameter",
            ir::ErrorKind::NotARow { .. } => "not-a-row",
            ir::ErrorKind::RepeatedRowField { .. } => "repeated-row-field",
            ir::ErrorKind::EndlessFields => "endless-fields",
            // What refuted the binding is not part of the code, only of the
            // wording: what went wrong is the binding, and the quoted tag or
            // number only points at where.
            ir::ErrorKind::RefutableBinding { .. } => "binding-can-fail",
            ir::ErrorKind::DuplicateBinding { .. } => "duplicate-binding",
            ir::ErrorKind::DuplicateOperation { .. } => "duplicate-operation",
            ir::ErrorKind::NotAnOperation { .. } => "not-an-operation",
            ir::ErrorKind::ImpureOperation { .. } => "impure-operation",
            ir::ErrorKind::EffectsOutsideRow => "effects-outside-row",
            ir::ErrorKind::OperationOnAlias { .. } => "operation-on-alias",
            ir::ErrorKind::UnknownOperation { .. } => "unknown-operation",
            ir::ErrorKind::BareOperationUnavailable { .. } => "bare-operation-unavailable",
            ir::ErrorKind::NamedOperationOnUnnamed { .. } => "named-operation-on-unnamed",
            ir::ErrorKind::PartialHandler { .. } => "partial-handler",
            ir::ErrorKind::DuplicateArm { .. } => "duplicate-arm",
            ir::ErrorKind::DuplicateReturn { .. } => "duplicate-return-arm",
            ir::ErrorKind::RaiseOutsideArm => "raise-outside-arm",
            ir::ErrorKind::RaiseInFunction { .. } => "raise-in-function",
        }
    }
}

impl ir::Error {
    /// Turn a lowering failure into reporter-independent words and annotations,
    /// with `source` saying where its anchors point.
    pub fn diagnostic(&self, source: &SourceMap) -> Diagnostic {
        use ir::ErrorKind as E;

        let code = self.kind.code();
        let span = source.span(self.at);
        match &self.kind {
            E::ForeignProtocol { message } => Diagnostic::new(self.kind.code(), message.clone(), source.span(self.at)),
            E::ArrayInExtern => Diagnostic::new(
                code,
                "arrays cannot cross an extern boundary yet",
                span,
            )
            .label("this array uses Ruddy's private persistent representation")
            .help("convert the value at a Ruddy boundary, or keep this function in Ruddy code"),
            E::ExecutableDependency { name } => Diagnostic::new(
                code,
                format!("executable bundle `{name}` cannot be a dependency"),
                span,
            ).help("only library bundles can be imported"),
            E::InvalidDependencyAlias { alias } => Diagnostic::new(
                code,
                format!("`{alias}` cannot be used as a module name"),
                span,
            )
            .label("this name is not valid in Ruddy source")
            .help("start with a letter or `_`, use only letters, digits, and `_`, and do not use a reserved word"),
            E::DuplicateDependencyAlias { alias } => Diagnostic::new(
                code,
                format!("dependency alias `{alias}` was imported more than once"),
                span,
            )
            .label("this alias is already in use")
            .help("remove one import or give it a different alias"),
            E::DuplicateDependency { name, version } => Diagnostic::new(
                code,
                format!("dependency `{name}@{version}` was imported more than once"),
                span,
            )
            .label("this exact dependency is already imported")
            .help("remove one of the duplicate dependencies"),
            E::Undefined { namespace, name } => {
                let kind = namespace_name(*namespace);
                Diagnostic::new(code, format!("cannot find {kind} `{name}`"), span)
                    .label(format!("no {kind} of this name is in scope"))
            }
            E::Duplicate {
                namespace,
                name,
                previous,
            } => {
                let kind = namespace_name(*namespace);
                Diagnostic::new(code, format!("`{name}` is defined more than once"), span)
                    .label(format!("this {kind} is defined again"))
                    .related(source.span(*previous), FIRST_DEFINITION)
            }
            E::DuplicateField { name, previous } => {
                Diagnostic::new(code, format!("field `{name}` is written more than once"), span)
                    .label("written again here")
                    .related(source.span(*previous), FIRST_WRITTEN)
            }
            E::DuplicateAttribute { name, previous } => Diagnostic::new(
                code,
                format!("metadata key `{name}` is written more than once"),
                span,
            )
            .label("written again here")
            .related(source.span(*previous), FIRST_WRITTEN)
            .help(format!(
                "a definition's metadata has one value per key; keep one `@{name}`"
            )),
            E::InvalidPrivateValue => Diagnostic::new(code, "`@private` requires unit", span)
                .label("this value is not unit")
                .help("write `@private` or `@private ()`"),
            E::MetadataTooDeep => Diagnostic::new(
                code,
                "this metadata value is nested too deeply",
                span,
            )
            .label("this sits below the deepest level allowed")
            .help(format!(
                "metadata may nest at most {} levels of arrays, structs, and tag payloads",
                ir::METADATA_DEPTH_LIMIT
            )),
            E::DuplicateCase {
                shape,
                name,
                previous,
            } => {
                let name = label(*shape, name);
                Diagnostic::new(
                    code,
                    format!("{} `{name}` is included more than once", noun(*shape)),
                    span,
                )
                .label("included again here")
                .related(source.span(*previous), FIRST_WRITTEN)
            }
            E::AbsentInClosed { shape, label: name } => Diagnostic::new(
                code,
                format!(
                    "a type with no `..` already says `{}` is not there",
                    label(*shape, name)
                ),
                span,
            )
            .label("this mark repeats what the closed type already says")
            .help("remove this `\\` mark; a closed type already excludes labels it does not list"),
            E::Circular { namespace } => match namespace {
                Namespace::Types => {
                    Diagnostic::new(code, "type defined only as another name", span)
                        .label("following these names never reaches an actual type")
                        .help("put a struct, sum, or arrow between the type and itself")
                }
                Namespace::Terms | Namespace::Effects | Namespace::Modules => Diagnostic::new(
                    code,
                    "this definition is never given a value of its own",
                    span,
                )
                .label("following these names leads back here")
                .help("give the definition a value instead of only another name"),
            },
            E::OpenDeclaredType { shape } => Diagnostic::new(
                code,
                format!("a declared type must list its {}s exactly", noun(*shape)),
                span,
            )
            .label("this leaves part of the declared type undecided")
            .help("list every label, use one of the declaration's parameters, or move this type to an annotation"),
            E::ClauseInDeclaration => {
                Diagnostic::new(code, "a declared type cannot have a `where` clause", span)
                    .label("there is nothing in a declaration for this clause to decide")
                    .help("move the `where` clause to an annotation")
            }
            E::VariableInDeclaration { name } => Diagnostic::new(
                code,
                format!("`'{name}` is not declared in this type's header"),
                span,
            )
            .label("this name is not one of the type's parameters")
            .help(format!("add `'{name}` to the header, or use an existing parameter")),
            E::HoleInDeclaration => {
                Diagnostic::new(code, "a declared type cannot contain `_`", span)
                    .label("a declaration cannot leave its type open")
                    .help("write the type that every use of this declaration should have")
            }
            E::HoleInOperation => {
                Diagnostic::new(code, "an effect function's type cannot contain `_`", span)
                    .label("an effect function must have one fixed type")
                    .help("write the type that every call to this function should have")
            }
            E::UnboundPresence { name } => Diagnostic::new(
                code,
                format!("`'{name}` does not control any field or case"),
                span,
            )
            .label(format!("used here, but no label has `when '{name}`"))
            .help(format!("add `when '{name}` to the intended label, or correct the name")),
            E::IncompatiblePresenceOwnership { name, previous } => Diagnostic::new(
                code,
                format!("`'{name}` cannot describe choices made by two separate results"),
                span,
            )
            .label("this result makes its own choice")
            .related(source.span(*previous), FIRST_PRODUCTION_LIFETIME)
            .help("use a different name for each result's choice"),
            E::Arity {
                name,
                expected,
                found,
            } => Diagnostic::new(
                code,
                format!(
                    "`{name}` expects {}, but {} written",
                    arguments(*expected),
                    supplied(*found)
                ),
                span,
            )
            .label(if found < expected {
                "not enough arguments are supplied"
            } else {
                "too many arguments are supplied"
            })
            .help(if found < expected {
                "add the missing type arguments"
            } else {
                "remove the extra type arguments"
            }),
            E::EffectArity {
                name,
                expected,
                found,
            } => Diagnostic::new(
                code,
                format!(
                    "effect `!{name}` expects {}, but {} written",
                    arguments(*expected),
                    supplied(*found)
                ),
                span,
            )
            .label(if found < expected {
                "not enough arguments are supplied"
            } else {
                "too many arguments are supplied"
            })
            .help(if found < expected {
                "add the missing effect arguments"
            } else {
                "remove the extra effect arguments"
            }),
            E::AliasCycle { name, growing } => Diagnostic::new(
                code,
                match growing {
                    true => format!("effect `!{name}` grows every time it stands for itself"),
                    false => format!("effect `!{name}` only ever stands for itself"),
                },
                span,
            )
            .label(match growing {
                true => "this alias adds effects and then names itself again",
                false => "this alias leads back to itself through other aliases",
            })
            .help("name the effects the alias stands for without going through itself"),
            E::ModifiedOpenAlias { name } => Diagnostic::new(
                code,
                format!("`!{name}` leaves some effects unnamed here, so it cannot be marked"),
                span,
            )
            .label("this mark would have to apply to effects that are not named yet")
            .help("give the alias every effect it stands for, or mark each effect yourself"),
            E::TwoTails { previous } => Diagnostic::new(
                code,
                "these effects are left open twice",
                span,
            )
            .label("this `..` is the second")
            .related(source.span(*previous), "the first `..` is here")
            .help("keep one `..`: write the other's effects out, or drop one"),
            E::NotAConstructor => Diagnostic::new(
                code,
                "this type cannot take arguments",
                span,
            )
            .label("arguments are applied here")
            .help("apply a named type declaration that has parameters"),
            E::ParameterApplied { name } => Diagnostic::new(
                code,
                format!("type parameter `'{name}` cannot take arguments"),
                span,
            )
            .label("this parameter already represents one complete type")
            .help("apply a declared type with parameters instead"),
            E::DuplicateParameter { name, previous } => {
                Diagnostic::new(code, format!("parameter `'{name}` is declared more than once"), span)
                    .label("declared again here")
                    .related(source.span(*previous), FIRST_DECLARATION)
            }
            E::GrowingRecursion => {
                Diagnostic::new(code, "recursive type arguments grow without bound", span)
                    .label("each trip around this recursion builds a larger type")
                    .help("pass recursive parameters through unchanged, use a fixed type, or break the recursion")
            }
            E::MixedTail {
                first,
                second,
                previous,
            } => Diagnostic::new(
                code,
                format!("one variable cannot stand for both {first} and {second}"),
                span,
            )
            .label(format!("used as {second} here"))
            .related(source.span(*previous), format!("first used as {first} here"))
            .help("use a different name for each purpose"),
            E::MixedParameter { first, second } => Diagnostic::new(
                code,
                format!("this parameter is used as {first} and as {second}"),
                span,
            )
            .label(format!("some uses need {first}, while others need {second}"))
            .help("use a separate parameter name for each purpose"),
            E::NotARow { sense } => Diagnostic::new(
                code,
                format!("this argument must provide {sense}"),
                span,
            )
            .label(format!("this type cannot provide {sense}")),
            E::RepeatedRowField { shape, field } => Diagnostic::new(
                code,
                format!(
                    "`{}` would appear twice in this {shape}",
                    label(*shape, field)
                ),
                span,
            )
            .label("the declaration and this argument both provide this name")
            .help("remove or rename the repeated field or case"),
            E::EndlessFields => {
                Diagnostic::new(code, "this recursive type adds more fields on every cycle", span)
                    .label("the fields never reach a finite end")
                    .help("place the recursion inside a field, or stop extending the `..` part")
            }
            E::RefutableBinding { found } => {
                let message = match found {
                    ir::Refuter::Case(name) => {
                        format!("a value here might not be `{}`", label(Shape::Sum, name))
                    }
                    ir::Refuter::Literal(value) => {
                        format!("a value here might not equal `{}`", SourceLiteral(value))
                    }
                    ir::Refuter::Length => {
                        "a value here might have a different number of elements".to_string()
                    }
                };
                Diagnostic::new(code, "this pattern can fail, but a binding must accept every value", span)
                    .label(message)
                    .help("use `match` for this case or literal, or bind a name instead")
            }
            E::DuplicateBinding { name, previous } => {
                Diagnostic::new(code, format!("this pattern binds `{name}` more than once"), span)
                    .label("bound again here")
                    .related(source.span(*previous), FIRST_BINDING)
                    .help("rename one binding or replace it with `_`")
            }
            E::DuplicateOperation { name, previous } => {
                Diagnostic::new(code, format!("effect function `{name}` is declared more than once"), span)
                    .label("declared again here")
                    .related(source.span(*previous), FIRST_DECLARATION)
            }
            E::NotAnOperation { name } => {
                Diagnostic::new(code, format!("`{name}` is not a function"), span)
                    .label("what an effect declares must be a function type")
                    .help(format!("write a signature such as `{name} : Nat -> ()`"))
            }
            E::ImpureOperation { found } => match found {
                ir::OperationTypeProblem::Effects => Diagnostic::new(
                    code,
                    "an effect function cannot declare effects of its own",
                    span,
                )
                .label("calling it already performs this effect")
                .help("remove this `+` effect list"),
                ir::OperationTypeProblem::OpenPart => Diagnostic::new(
                    code,
                    "an effect function must have one fixed function type",
                    span,
                )
                .label("this leaves part of the function's type undecided")
                .help("write this part explicitly; use `..` and `when` in annotations instead"),
                ir::OperationTypeProblem::Variable(name) => Diagnostic::new(
                    code,
                    format!("`'{name}` is not declared by this effect function"),
                    span,
                )
                .label("an effect function cannot introduce type variables")
                .help("replace it with a fixed type or a declared type application"),
            },
            E::EffectsOutsideRow => Diagnostic::new(
                code,
                "effects cannot be used as a type by themselves",
                span,
            )
            .label("there is no function arrow here to carry these effects")
            .help("write them after a function result, as in `Nat -> Nat + !Log`"),
            E::OperationOnAlias { effect } => Diagnostic::new(
                code,
                format!(
                    "effect alias `{}` declares nothing to perform",
                    label(Shape::Effect, effect)
                ),
                span,
            )
            .label("an alias groups effects but declares nothing of its own")
            .help("perform one of the functions declared by the concrete effects this alias names"),
            E::UnknownOperation { effect, op } => Diagnostic::new(
                code,
                format!(
                    "effect `{}` does not declare `{op}`",
                    label(Shape::Effect, effect)
                ),
                span,
            )
            .label("the effect does not declare this name")
            .help("use one of the names the effect declares, or add this one to its declaration"),
            E::BareOperationUnavailable { effect, suggestion } => {
                let effect = label(Shape::Effect, effect);
                match suggestion {
                    Some(op) => Diagnostic::new(
                        code,
                        format!("performing `{effect}` requires a function name"),
                        span,
                    )
                    .label("this effect declares only named functions")
                    .help(format!("write `{effect}.{op}`")),
                    None => Diagnostic::new(
                        code,
                        format!("effect `{effect}` declares nothing to perform"),
                        span,
                    )
                    .label("there is nothing in this effect to perform")
                    .help("remove this use, or declare a function on the effect"),
                }
            }
            E::NamedOperationOnUnnamed { effect, op } => {
                let effect = label(Shape::Effect, effect);
                Diagnostic::new(
                    code,
                    format!("effect `{effect}` declares one unnamed function"),
                    span,
                )
                .label(format!("`{op}` does not name anything this effect declares"))
                .help(format!("write `{effect}` instead"))
            }
            E::PartialHandler { effect, missing } => Diagnostic::new(
                code,
                format!(
                    "this handler does not cover effect `{}`",
                    label(Shape::Effect, effect)
                ),
                span,
            )
            .label(format!("missing {}", operation_arms(missing)))
            .help(format!("add {}", operation_arms(missing))),
            E::DuplicateArm {
                effect,
                selector,
                previous,
            } => Diagnostic::new(
                code,
                format!(
                    "duplicate arm for `{}{selector}`",
                    label(Shape::Effect, effect)
                ),
                span,
            )
            .label("handled again here")
            .related(source.span(*previous), FIRST_ARM),
            E::DuplicateReturn { previous } => Diagnostic::new(
                code,
                "this handler has more than one `return` arm",
                span,
            )
            .label("second return arm")
            .related(source.span(*previous), FIRST_ARM),
            E::RaiseOutsideArm => Diagnostic::new(
                code,
                "`raise` can be used only directly inside a handler arm",
                span,
            )
            .label("there is no enclosing arm for this `raise`")
            .help("move it into a handler arm, or remove it"),
            E::RaiseInFunction { function } => Diagnostic::new(
                code,
                "`raise` cannot cross a function boundary",
                span,
            )
            .label("this `raise` is separated from its handler arm")
            .related(source.span(*function), "the intervening function starts here")
            .help("move `raise` directly into the handler arm, or return a value from the function"),
        }
    }
}

/// The diagnostic headline, retained for callers that only need a phrase.
impl fmt::Display for ir::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let error = ir::Error {
            at: Anchor::GENERATED,
            kind: self.clone(),
        };
        f.write_str(&error.diagnostic(&SourceMap::default()).title)
    }
}

/// A list of operations as a sentence reads them: `a`, `a and b`, `a, b and
/// c`. Every one is named, because every one is an arm the reader has to
/// write.
fn namespace_name(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::Terms => "value",
        Namespace::Types => "type",
        Namespace::Effects => "effect",
        Namespace::Modules => "module",
    }
}

/// A scalar literal written with Ruddy's own suffixes and escaping.
struct SourceLiteral<'a>(&'a ir::Literal);

impl fmt::Display for SourceLiteral<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ir::Literal::Natural(value) => write!(f, "{value}n"),
            ir::Literal::Fixed(value) => write!(f, "{value}"),
            ir::Literal::Integer(value) => write!(f, "{value}i"),
            ir::Literal::Real(value) => write!(f, "{value:?}"),
            ir::Literal::String(value) => write_string(f, value),
            ir::Literal::Boolean(value) => write!(f, "{value}"),
        }
    }
}

fn operation_arms(names: &[String]) -> String {
    match names.len() {
        1 => format!("an arm for {}", listed(names)),
        count => format!("{count} arms: {}", listed(names)),
    }
}

fn listed(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    // Indexed rather than matched for emptiness: a handler that covers every
    // operation is not partial, so a complaint about one always names at least
    // the operation that made it.
    let (last, rest) = quoted
        .split_last()
        .expect("a partial handler names an operation with no arm");
    match rest {
        [] => last.clone(),
        rest => format!("{} and {last}", rest.join(", ")),
    }
}

impl patterns::Error {
    pub fn diagnostic(&self, source: &SourceMap) -> Diagnostic {
        Diagnostic::new(
            self.kind.code(),
            self.kind.to_string(),
            source.span(self.at),
        )
    }
}

impl patterns::ErrorKind {
    /// A stable, greppable name for this kind of error — the codes the same
    /// checks had when they ran at lowering, so a reporter keyed on one keeps
    /// working across the move.
    pub fn code(&self) -> &'static str {
        match self {
            patterns::ErrorKind::MisplacedCatchAll => "misplaced-catch-all",
            patterns::ErrorKind::UnreachableArm => "unreachable-arm",
            // The witness is not part of the code, only of the wording: what
            // went wrong is the match, and the example only shows a value it
            // misses.
            patterns::ErrorKind::UnhandledValues { .. } => "unhandled-values",
            patterns::ErrorKind::UnhandledNumbers => "unhandled-numbers",
        }
    }
}

/// What the pattern checks found, in a phrase — the wordings the same checks
/// had when they ran at lowering, unchanged by the move.
impl fmt::Display for patterns::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            patterns::ErrorKind::MisplacedCatchAll => f.write_str(
                "this arm accepts everything, so the arms after it can never be reached",
            ),
            patterns::ErrorKind::UnreachableArm => {
                f.write_str("this case is already handled by the arms above it")
            }
            // The example is the complaint: a value no arm accepts, written in
            // source syntax so the reader can see what to add an arm for.
            patterns::ErrorKind::UnhandledValues { witness } => write!(
                f,
                "some values are not handled — for example `{witness}`; add an arm for them or a final arm naming the rest",
            ),
            patterns::ErrorKind::UnhandledNumbers => f.write_str(
                "numbers not listed here are not handled; add a final arm that names the rest",
            ),
        }
    }
}

/// One arm's verdict, as the word the Patterns tab prints beside it. Plain
/// English on purpose: "starved" says the arm sits after one that accepts
/// everything, and "skipped" that the checks stood aside because the typing
/// already failed.
impl fmt::Display for patterns::Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            patterns::Verdict::Reachable => "reachable",
            patterns::Verdict::Unreachable => "unreachable",
            patterns::Verdict::Starved => "starved",
            patterns::Verdict::Skipped => "skipped",
        })
    }
}

/// `first`, `second`, `third` — the position of an argument, in the word a
/// sentence wants; further along, the numeral with its ending, since an
/// effect taking a dozen arguments is already past what a word helps with.
fn ordinal(position: u32) -> String {
    let nth = position + 1;
    match nth {
        1 => "first".to_string(),
        2 => "second".to_string(),
        3 => "third".to_string(),
        n if (11..=13).contains(&(n % 100)) => format!("{n}th"),
        n if n % 10 == 1 => format!("{n}st"),
        n if n % 10 == 2 => format!("{n}nd"),
        n if n % 10 == 3 => format!("{n}rd"),
        n => format!("{n}th"),
    }
}

/// `no arguments`, `one argument`, `two arguments` — small counts in words,
/// because a message is a sentence and a sentence does not open with a numeral.
///
/// Beyond what is worth spelling out, the numeral: a type taking thirteen
/// arguments has a problem this message is not going to help with.
fn arguments(count: usize) -> String {
    match count {
        0 => "no arguments".to_string(),
        1 => "one argument".to_string(),
        2..=9 => format!("{} arguments", WORDS[count]),
        _ => format!("{count} arguments"),
    }
}

/// The same counting for the side that was written, which needs a verb that
/// agrees with it.
fn supplied(count: usize) -> String {
    match count {
        0 => "none was".to_string(),
        1 => "one was".to_string(),
        2..=9 => format!("{} were", WORDS[count]),
        _ => format!("{count} were"),
    }
}

const WORDS: [&str; 10] = [
    "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// The canonical spelling of a bundle's identity. Bundle names cannot contain
/// `@`, so this is one string per bundle and one bundle per string — which is
/// what makes it safe to fingerprint.
///
/// [`Bundle::new`] writes the same string a second time to take that
/// fingerprint, rather than calling through here, and that is on purpose: the
/// fingerprint reaches every mangled name in a build, so rewording this — the
/// one thing everything in this module is free to do — must not move it.
impl fmt::Display for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name(), self.version())
    }
}

/// What a row of this shape is called, in the one word a sentence about it
/// needs. Never "row": the word names the representation the two share, which
/// is the compiler's business, and a reader who wrote braces should be told
/// about a struct.
impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Shape::Struct => "struct",
            Shape::Sum => "sum",
            // What carries the row rather than what the row is: an effect row
            // is one of the three things a function's type says, and "row"
            // names the representation the three shapes share.
            Shape::Effect => "function",
        })
    }
}

/// What a parameter, or a named tail, stands for, as the phrase a complaint
/// drops into a sentence. Read as "this stands for …", which is what
/// [`ir::ErrorKind::MixedParameter`] and [`ir::ErrorKind::MixedTail`] each say
/// twice and [`ir::ErrorKind::NotARow`] once.
///
/// The rest of a struct is a whole type, so it has no phrase of its own: `..'r`
/// in a struct puts whatever is written for `'r` in the struct-row tail, and there is
/// nothing narrower to call that.
impl fmt::Display for Sense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Sense::Type => "a whole type",
            Sense::Fields => "the rest of a struct's fields",
            Sense::Cases => "the rest of a sum's cases",
            Sense::Effects => "the rest of an arrow's effects",
            // What a `when` puts on a label, said as the reader's own word for
            // it rather than as "a presence variable": they wrote `when a`, and
            // the variable behind it is the compiler's business.
            Sense::Region => "a region",
            Sense::Presence => "a presence",
        })
    }
}

/// What one label of a row of this shape is called.
fn noun(shape: Shape) -> &'static str {
    match shape {
        Shape::Sum => "case",
        Shape::Struct => "field",
        Shape::Effect => "effect",
    }
}

/// The two words a complaint about one label of a type needs: what to call it,
/// and how it is written there.
///
/// Both come off the shape the solver carried rather than off the type beside
/// them, because the type no longer answers the question. Only structs have fields
/// *and* may have cases, so a base can be a sum type that is missing a
/// `field` — `(#A 1).x` is exactly that — and reading the word off the base
/// would call it a case. The solver knows which row it was deciding at the
/// moment it failed, and that is the only place the answer is not a guess.
fn about(shape: Shape, name: &str) -> (&'static str, String) {
    (noun(shape), label(shape, name))
}

/// One label as it is written in a row of this shape: a field is its bare
/// name, a case wears the `#` that makes it one, and an effect the `!`.
///
/// Every complaint that quotes a label goes through here rather than
/// interpolating the string it was given, so that a message about `#Some`
/// never asks the reader to look for `Some`.
///
/// Three shapes and three spellings, which is what the effect's own sigil
/// buys: a complaint about `!Log` and one about `#Log` no longer read alike.
pub fn label(shape: Shape, name: &str) -> String {
    struct Field<'a>(&'a str);
    impl fmt::Display for Field<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_field_label(f, self.0)
        }
    }
    struct Tag<'a>(&'a str);
    impl fmt::Display for Tag<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_tag_label(f, self.0)
        }
    }
    match shape {
        Shape::Struct => Field(name).to_string(),
        Shape::Sum => Tag(name).to_string(),
        // Structural effect keys carry an opaque interface after this
        // separator. Source paths never participate in row identity, and the
        // interface is deliberately not user-facing: coalesced effects render
        // as the one bare name they share. Keep the old path spelling for the
        // diagnostic helper's explicitly path-shaped input.
        Shape::Effect => {
            let name = EffectId::parse_row_key(name).map_or(name, |pair| pair.0);
            match name.rsplit_once("::") {
                Some((modules, effect)) => format!("{modules}::!{effect}"),
                None => format!("!{name}"),
            }
        }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::Terms => f.write_str("term"),
            Namespace::Types => f.write_str("type"),
            Namespace::Effects => f.write_str("effect"),
            Namespace::Modules => f.write_str("module"),
        }
    }
}

impl fmt::Display for Path<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The version is left out: this is for diagnostics, where it would be
        // noise everywhere except the rare cross-version confusion.
        f.write_str(self.mint.bundle().name())?;
        for symbol in self.mint.chain(self.symbol) {
            // A local is not addressable where its parent's globals are, so it
            // is shown one segment further in rather than beside them. Written
            // per component instead of once at the end, so a path stays honest
            // about which of its steps was the local one.
            if self.mint.is_local(symbol) {
                write!(f, "::{LOCAL_SEGMENT}")?;
            }
            write!(f, "::{}", self.mint.name(symbol))?;
        }
        Ok(())
    }
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How much of the surface grammar a semantic type can be.
///
/// A type that carries no fields groups as its type alone, because that is all
/// it prints as. One that carries fields is either written in braces — which
/// close it, so it is an atom, and which is every row tail has a spelling
/// — or a `with`, whose field list extends rightward and so has to be kept off
/// anything that could be read as continuing it.
///
/// The type language has no lambda, so one of [`Prec`]'s levels never arises
/// here. It is still the right scale to answer on: grouping is decided by
/// comparing against the position a type is being written into, and that
/// comparison is the surface grammar's whether or not this particular tree can
/// reach every level of it.
fn unpackaged(mut ty: &Ty) -> &Ty {
    while let Ty::Package(body) = ty {
        ty = body;
    }
    ty
}

impl Grouped for Ty {
    fn prec(&self) -> Prec {
        match unpackaged(self) {
            Ty::Arrow(..) => Prec::Arrow,
            Ty::Sum(_) => Prec::Sum,
            Ty::Mut(..) => Prec::Apply,
            Ty::Named { args, .. } if !args.is_empty() => Prec::Apply,
            _ => Prec::Atom,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_semantic(f, SemanticRoot::Ty(self))
    }
}

/// One explicit-stack semantic formatter shared by types, rows, and row tails.
/// Imported artifacts are allowed to be deeply nested recovery input, so a
/// formatter used by diagnostics and debugger stages may not mirror that depth
/// with Rust calls.
enum SemanticRoot<'a> {
    Ty(&'a Ty),
    Row(&'a Row),
    Rest(&'a Rest),
    EffectLabels(&'a Row),
}

enum SemanticJob<'a> {
    Ty(&'a Ty, bool),
    Applied(&'a Ty),
    Row(&'a Row),
    Rest(&'a Rest),
    Fields(&'a Row),
    Cases(&'a Row, bool),
    Effects(&'a Row),
    Tail(&'a Rest),
    Field(&'a str, &'a RowField),
    TupleField(&'a RowField),
    Case(&'a str, &'a RowField, bool),
    Effect(&'a str, &'a RowField),
    Mark(&'a Presence),
    Text(&'static str),
}

fn format_semantic(f: &mut fmt::Formatter<'_>, root: SemanticRoot<'_>) -> fmt::Result {
    let first = match root {
        SemanticRoot::Ty(ty) => SemanticJob::Ty(ty, false),
        SemanticRoot::Row(row) => SemanticJob::Row(row),
        SemanticRoot::Rest(rest) => SemanticJob::Rest(rest),
        SemanticRoot::EffectLabels(row) => SemanticJob::Effects(row),
    };
    let mut work = vec![first];
    while let Some(part) = work.pop() {
        match part {
            SemanticJob::Text(text) => f.write_str(text)?,
            SemanticJob::Ty(ty, grouped) => {
                if grouped {
                    f.write_str("(")?;
                    work.push(SemanticJob::Text(")"));
                }
                match ty {
                    Ty::Package(body) => work.push(SemanticJob::Ty(body, false)),
                    Ty::Mut(region, element) => {
                        f.write_str("mut ")?;
                        work.push(SemanticJob::Applied(element));
                        work.push(SemanticJob::Text(" "));
                        work.push(SemanticJob::Ty(region, false));
                    }
                    Ty::Array(element) => {
                        f.write_str("[")?;
                        work.push(SemanticJob::Text("]"));
                        work.push(SemanticJob::Ty(element, false));
                    }
                    Ty::Nat => f.write_str(Prim::Nat.name())?,
                    Ty::Int => f.write_str(Prim::Int.name())?,
                    Ty::Fixed(kind) => f.write_str(kind.name())?,
                    Ty::Real => f.write_str(Prim::Real.name())?,
                    Ty::String => f.write_str(Prim::String.name())?,
                    Ty::Boolean => f.write_str(Prim::Boolean.name())?,
                    Ty::Arrow(from, to, effects) => {
                        let shown = effect_row_shown(effects);
                        if shown {
                            work.push(SemanticJob::Effects(effects));
                            work.push(SemanticJob::Text(" + "));
                        }
                        work.push(SemanticJob::Ty(
                            to,
                            shown && matches!(unpackaged(to), Ty::Arrow(..)),
                        ));
                        work.push(SemanticJob::Text(" -> "));
                        work.push(SemanticJob::Ty(from, from.prec() < Prec::Sum));
                    }
                    Ty::Struct(row) => work.push(SemanticJob::Fields(row)),
                    Ty::Sum(row) => work.push(SemanticJob::Cases(row, false)),
                    Ty::Named { name, args, .. } => {
                        for arg in args.iter().rev() {
                            work.push(SemanticJob::Applied(arg));
                            work.push(SemanticJob::Text(" "));
                        }
                        f.write_str(name)?;
                    }
                    Ty::Var(var) => write!(f, "?{var}")?,
                    Ty::Bound(index) => f.write_str(&name_at(*index))?,
                    Ty::Rigid { name, .. } => write!(f, "'{name}")?,
                    Ty::Undecided => f.write_str("?")?,
                }
            }
            SemanticJob::Applied(ty) => {
                let grouped = ty.prec() < Prec::Atom;
                if grouped {
                    f.write_str("(")?;
                    work.push(SemanticJob::Text(")"));
                }
                match unpackaged(ty) {
                    Ty::Sum(row)
                        if flattened_row(row)
                            .0
                            .iter()
                            .any(|(name, _)| EffectId::parse_canonical_row_key(name).is_some()) =>
                    {
                        work.push(SemanticJob::Cases(row, true));
                    }
                    ty => work.push(SemanticJob::Ty(ty, false)),
                }
            }
            SemanticJob::Row(row) => {
                let (fields, rest) = flattened_row(row);
                match fields.is_empty() {
                    true => work.push(SemanticJob::Rest(rest)),
                    false => work.push(SemanticJob::Fields(row)),
                }
            }
            SemanticJob::Rest(rest) => match rest {
                Rest::Closed => f.write_str("∅")?,
                Rest::Var(var) => write!(f, "?{var}")?,
                Rest::Bound(index) => f.write_str(&name_at(*index))?,
                Rest::Rigid { name, .. } => write!(f, "'{name}")?,
                Rest::Undecided => f.write_str("?")?,
                Rest::More(row) => work.push(SemanticJob::Row(row)),
            },
            SemanticJob::Fields(row) => {
                let (fields, rest) = flattened_row(row);
                let tail = semantic_tail(Shape::Struct, rest);
                let tuple = if tail.is_none()
                    && fields
                        .iter()
                        .all(|(_, field)| matches!(field.presence, Presence::Present))
                {
                    tuple_field_order(fields.iter().map(|(name, _)| *name))
                } else {
                    None
                };
                if let Some(order) = tuple {
                    f.write_str("(")?;
                    work.push(SemanticJob::Text(")"));
                    if order.len() == 1 {
                        work.push(SemanticJob::Text(","));
                    }
                    for (at, insertion) in order.into_iter().enumerate().rev() {
                        work.push(SemanticJob::TupleField(fields[insertion].1));
                        if at != 0 {
                            work.push(SemanticJob::Text(", "));
                        }
                    }
                    continue;
                }
                if fields.is_empty() && tail.is_none() {
                    f.write_str("()")?;
                    continue;
                }
                f.write_str("{ ")?;
                work.push(SemanticJob::Text(" }"));
                if let Some(tail) = tail {
                    work.push(SemanticJob::Tail(tail));
                    work.push(SemanticJob::Text(".."));
                    if !fields.is_empty() {
                        work.push(SemanticJob::Text(", "));
                    }
                }
                for (at, (name, field)) in fields.into_iter().enumerate().rev() {
                    work.push(SemanticJob::Field(name, field));
                    if at != 0 {
                        work.push(SemanticJob::Text(", "));
                    }
                }
            }
            SemanticJob::Cases(row, strip_interface) => {
                let (cases, rest) = flattened_row(row);
                let tail = semantic_tail(Shape::Sum, rest);
                if cases.is_empty() {
                    f.write_str("|")?;
                }
                if let Some(tail) = tail {
                    work.push(SemanticJob::Tail(tail));
                    work.push(SemanticJob::Text(if cases.is_empty() {
                        " .."
                    } else {
                        " | .."
                    }));
                }
                for (at, (name, field)) in cases.into_iter().enumerate().rev() {
                    work.push(SemanticJob::Case(name, field, strip_interface));
                    if at != 0 {
                        work.push(SemanticJob::Text(" | "));
                    }
                }
            }
            SemanticJob::Effects(row) => {
                let (effects, rest) = flattened_row(row);
                let tail = semantic_tail(Shape::Effect, rest);
                if effects.is_empty() && tail.is_none() {
                    f.write_str("|")?;
                    continue;
                }
                if let Some(tail) = tail {
                    work.push(SemanticJob::Tail(tail));
                    work.push(SemanticJob::Text(".."));
                    if !effects.is_empty() {
                        work.push(SemanticJob::Text(" + "));
                    }
                }
                for (at, (name, field)) in effects.into_iter().enumerate().rev() {
                    work.push(SemanticJob::Effect(name, field));
                    if at != 0 {
                        work.push(SemanticJob::Text(" + "));
                    }
                }
            }
            SemanticJob::Tail(Rest::Undecided) => {}
            SemanticJob::Tail(rest) => work.push(SemanticJob::Rest(rest)),
            SemanticJob::Field(name, field) => {
                write_field_label(f, name)?;
                write_semantic_mark(f, &field.presence, false)?;
                f.write_str(": ")?;
                work.push(SemanticJob::Ty(&field.ty, false));
            }
            SemanticJob::TupleField(field) => {
                work.push(SemanticJob::Ty(&field.ty, false));
            }
            SemanticJob::Case(name, field, strip_interface) => {
                let name = if strip_interface {
                    EffectId::parse_canonical_row_key(name).map_or(name, |pair| pair.0)
                } else {
                    name
                };
                write_tag_label(f, name)?;
                write_semantic_mark(f, &field.presence, true)?;
                if !unit_type(&field.ty) {
                    f.write_str(" ")?;
                    work.push(SemanticJob::Ty(&field.ty, field.ty.prec() < Prec::Atom));
                }
            }
            // An applied effect: the label, its arguments as a type
            // application writes them, and the mark it wears after those.
            SemanticJob::Effect(name, field) => {
                f.write_str(&label(Shape::Effect, name))?;
                work.push(SemanticJob::Mark(&field.presence));
                if let Ty::Struct(row) = unpackaged(&field.ty)
                    && matches!(row.rest, Rest::Closed)
                    && let Some(order) = tuple_field_order(row.labels.keys().map(String::as_str))
                {
                    for insertion in order.into_iter().rev() {
                        let arg = &row.labels[insertion].ty;
                        match arg.effects_argument_row() {
                            Some(effects) => {
                                work.push(SemanticJob::Text(")"));
                                work.push(SemanticJob::Effects(effects));
                                work.push(SemanticJob::Text("("));
                            }
                            None => work.push(SemanticJob::Applied(arg)),
                        }
                        work.push(SemanticJob::Text(" "));
                    }
                }
            }
            SemanticJob::Mark(presence) => write_semantic_mark(f, presence, true)?,
        }
    }
    Ok(())
}

/// Flatten a semantic row with the same outer-wins rule as inference. A name
/// is claimed before its presence is inspected, so an outer absent entry masks
/// an inner present one rather than merely disappearing beside it.
fn flattened_row(row: &Row) -> (Vec<(&str, &RowField)>, &Rest) {
    let mut names = HashSet::new();
    let mut fields = Vec::new();
    let mut row = row;
    loop {
        for (name, field) in &row.labels {
            if names.insert(name.as_str()) && !matches!(field.presence, Presence::Absent) {
                fields.push((name.as_str(), field));
            }
        }
        match &row.rest {
            Rest::More(more) => row = more,
            rest => return (fields, rest),
        }
    }
}

fn semantic_tail(shape: Shape, rest: &Rest) -> Option<&Rest> {
    match rest {
        Rest::Closed => None,
        Rest::Undecided if matches!(shape, Shape::Effect) => None,
        rest => Some(rest),
    }
}

fn effect_row_shown(row: &Row) -> bool {
    let (fields, rest) = flattened_row(row);
    !fields.is_empty() || semantic_tail(Shape::Effect, rest).is_some()
}

fn unit_type(ty: &Ty) -> bool {
    let Ty::Struct(row) = ty else {
        return false;
    };
    let (fields, rest) = flattened_row(row);
    fields.is_empty() && matches!(rest, Rest::Closed)
}

fn write_semantic_mark(
    f: &mut fmt::Formatter<'_>,
    presence: &Presence,
    parenthesized: bool,
) -> fmt::Result {
    let (open, close) = match parenthesized {
        true => (" (when ", ")"),
        false => (" when ", ""),
    };
    match presence {
        Presence::Present | Presence::Absent => Ok(()),
        Presence::Recovered(_) | Presence::Undecided => f.write_str("?"),
        Presence::Var(var) => write!(f, "{open}?{var}{close}"),
        Presence::Bound(index) => write!(f, "{open}{}{close}", name_at(*index)),
    }
}

/// A row prints as what it says about the labels it names and about the ones it
/// does not.
///
/// Written in the struct's notation, because a row reaching here has no shape to
/// be read in. Where one does is the solver's own record — what a step bound a
/// row variable to, and what two rows were asked to agree on — and a [`Row`]
/// does not say which of the two shapes it is: the shape is a reading the
/// position hands down, and a row that has been lifted out of its position has
/// nobody left to hand it one. So a sum's tail bound in the Solve tab reads
/// `?2 := { B: Nat }` rather than in cases, and the braces there are notation
/// for a set of labels rather than a claim that they are fields. Everywhere a
/// shape *is* known the containing type hands it directly to the iterative
/// semantic formatter instead.
///
/// A row that names nothing prints as its rest alone, so closing one reads
/// `?4 := ∅` rather than as an empty pair of braces standing for the same
/// thing.
impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_semantic(f, SemanticRoot::Row(self))
    }
}

/// An effect row in the notation its arrow supplies, which is the one thing
/// [`Display for Row`](Row) cannot know.
impl fmt::Display for EffectLabels<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_semantic(f, SemanticRoot::EffectLabels(self.row))
    }
}

/// What is known about the cases a row does not name.
///
/// Never part of a printed scheme on its own — a closed tail is written as no
/// `..` at all, and an open one as the `..` [`write_cases`] puts before this.
/// Where one does surface is the solver's own record, where `∅` is how a row
/// says it has nothing more to come.
impl fmt::Display for Rest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_semantic(f, SemanticRoot::Rest(self))
    }
}

/// Whether one label is there.
///
/// Never part of a printed scheme either: a field wears its presence as the
/// `when` clause on its name, or as nothing. Where one surfaces is the solver's
/// record, where a step deciding a presence reads `?3 := absent`.
impl fmt::Display for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Presence::Present => f.write_str("present"),
            Presence::Absent => f.write_str("absent"),
            Presence::Var(var) => write!(f, "?{var}"),
            Presence::Bound(index) => f.write_str(&name_at(*index)),
            Presence::Recovered(_) | Presence::Undecided => f.write_str("?"),
        }
    }
}

/// One variable a formula names, in the spelling its reading has: the solver's
/// own `?3` for one it still owns, and the bare `a` of a `when` clause for one
/// a scheme quantified.
impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Var(var) => write!(f, "?{var}"),
            Atom::Bound(index) => f.write_str(&name_at(*index)),
        }
    }
}

/// What a variable was decided to stand for, whichever sort it is. One line
/// beside the rule that decided it, so each prints as what it is and nothing
/// wraps it.
impl fmt::Display for Assigned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Assigned::Ty(ty) => ty.fmt(f),
            Assigned::Row(row) => row.fmt(f),
            Assigned::Presence(presence) => presence.fmt(f),
        }
    }
}

/// A quantified variable, by its position: `'a` through `'z`, and then `'a1`
/// once the letters run out.
///
/// One alphabet and one sigil. Every sort a scheme quantifies prints through
/// here — a type, a struct's rest after a `..`, a sum's the same, a presence
/// after a `when` — because that is how each of them is *written*, and a
/// printed scheme is meant to read back as source. The position a letter sits
/// in says which sort it is, and a scheme's one index space is what keeps two
/// of them from ever colliding.
fn name_at(index: u32) -> String {
    let letter = (b'a' + (index % 26) as u8) as char;
    match index / 26 {
        0 => format!("'{letter}"),
        round => format!("'{letter}{round}"),
    }
}

/// How tightly a printed formula binds, on the `where` grammar's own ladder:
/// `0` for the non-associative `=` and `!=`, `1` for `or`, `2` for `and`, `3`
/// for `not`, `4` for a name.
///
/// A number rather than a [`Prec`] of its own, because it is its own grammar:
/// nothing in a formula can be a type and nothing in a type can be a formula,
/// so a shared ladder would only invite one to be compared against the other —
/// and there is nothing here for a named level to disambiguate that the four
/// call sites do not already say.
fn prec(formula: &Formula) -> u8 {
    match formula {
        Formula::Owned(_, inner) => prec(inner),
        Formula::Iff(..) | Formula::Xor(..) => 0,
        Formula::Or(..) => 1,
        Formula::And(..) => 2,
        Formula::Not(_) => 3,
        Formula::True | Formula::False | Formula::Atom(_) => 4,
    }
}

/// A formula in the surface `where` grammar, with exactly the parentheses
/// re-parsing needs and no others.
///
/// The two-variable special cases are not decided here: they are decided when
/// the canonical form is taken, so what arrives is already `a = b` or `a != b`
/// where that is what the formula is.
impl fmt::Display for Named<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.at(f, self.formula, 0)
    }
}

impl Named<'_> {
    /// Write `formula` for a position that binds at least as tightly as
    /// `level`, bracketing it when it does not.
    fn at(&self, f: &mut fmt::Formatter<'_>, formula: &Formula, level: u8) -> fmt::Result {
        enum Work<'a> {
            Formula(&'a Formula, u8),
            Text(&'static str),
            Close,
        }

        let mut work = vec![Work::Formula(formula, level)];
        while let Some(part) = work.pop() {
            match part {
                Work::Text(text) => f.write_str(text)?,
                Work::Close => f.write_str(")")?,
                Work::Formula(Formula::Owned(_, inner), level) => {
                    work.push(Work::Formula(inner, level));
                }
                Work::Formula(formula, level) => {
                    let parens = prec(formula) < level;
                    if parens {
                        f.write_str("(")?;
                        work.push(Work::Close);
                    }
                    match formula {
                        // Neither constant has a spelling in the grammar, and
                        // neither has to: they are debugger/recovery readings.
                        Formula::True => f.write_str("always")?,
                        Formula::False => f.write_str("never")?,
                        Formula::Atom(atom) => f.write_str(&self.spell(*atom))?,
                        Formula::Owned(..) => unreachable!("handled before precedence"),
                        Formula::Not(inner) => {
                            f.write_str("not ")?;
                            work.push(Work::Formula(inner, 3));
                        }
                        // Left-associative, so the right side is written one
                        // level tighter and retains required parentheses.
                        Formula::And(left, right) => {
                            work.push(Work::Formula(right, 3));
                            work.push(Work::Text(" and "));
                            work.push(Work::Formula(left, 2));
                        }
                        Formula::Or(left, right) => {
                            work.push(Work::Formula(right, 2));
                            work.push(Work::Text(" or "));
                            work.push(Work::Formula(left, 1));
                        }
                        // Comparisons are non-associative, so both operands are
                        // one precedence level tighter.
                        Formula::Iff(left, right) => {
                            work.push(Work::Formula(right, 1));
                            work.push(Work::Text(" = "));
                            work.push(Work::Formula(left, 1));
                        }
                        Formula::Xor(left, right) => {
                            work.push(Work::Formula(right, 1));
                            work.push(Work::Text(" != "));
                            work.push(Work::Formula(left, 1));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// How one atom is spelled: the label it decides, where this reading has
    /// one, and the presence itself otherwise.
    fn spell(&self, atom: Atom) -> String {
        let presence = match atom {
            Atom::Var(var) => Presence::Var(var),
            Atom::Bound(index) => Presence::Bound(index),
        };
        self.labels
            .iter()
            .find(|(_, decides)| *decides == presence)
            .map(|(label, _)| label.to_string())
            .unwrap_or_else(|| atom.to_string())
    }
}

/// A formula written in the labels its presences decide, for a complaint that
/// has to quote one to a reader who never saw a presence variable.
///
/// An empty `labels` spells every atom as the presence itself, which is what a
/// scheme's own `where` clause wants.
pub fn in_labels(formula: &Formula, labels: &[(String, Presence)]) -> String {
    Named { formula, labels }.to_string()
}

/// A formula as a scheme's `where` clause writes it: its presences by the names
/// their `when` clauses gave them.
impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Named {
            formula: self,
            labels: &[],
        }
        .fmt(f)
    }
}

/// A scheme prints as its body and what it requires of its presences.
///
/// The quantifier is not written out because there is nothing to write: a
/// variable is introduced where it is used, so the letters in the body are the
/// quantifier. `let id = fn x => x` reports `'a -> 'a`, and pasting that back
/// as `let id : 'a -> 'a` re-lowers to the type it was printed from.
///
/// A scheme requiring nothing prints no `where` at all, which is every scheme
/// in a program that never wrote a `when`.
impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.body().fmt(f)?;
        if self.formula().is_true() {
            return Ok(());
        }
        write!(f, " where {}", self.formula())
    }
}

impl Rule {
    /// A stable, greppable name for this rule, the way [`inference::ErrorKind`]
    /// is coded. The debugger labels its rows with it rather than with prose
    /// that may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            Rule::Absorb => "absorb",
            Rule::Same => "same",
            Rule::Congruent => "congruent",
            Rule::Bind => "bind",
            Rule::Mut => "mut",
            Rule::Array => "array",
            Rule::Occurs => "occurs",
            // The shape is not part of these two codes, only of their wording,
            // for the reason a row error's is not part of its code: a reader
            // filtering the Solve tab for a rule is asking which act of the
            // solver ran, and the goal beside it already says which shape it
            // ran on. [`Rule::Struct`] and [`Rule::Sum`] are the other way
            // round because they are two sentences rather than one noun.
            Rule::Overlap { .. } => "overlap",
            Rule::Prim => "prim",
            Rule::Arrow => "arrow",
            Rule::Performs => "performs",
            Rule::Struct => "struct",
            Rule::Refine => "refine",
            Rule::Sum => "sum",
            Rule::Presence { .. } => "presence",
            Rule::Unfold => "unfold",
            Rule::Assume => "assume",
            Rule::Mismatch => "mismatch",
            Rule::Recover => "recover",
        }
    }
}

/// What a rule does, in a phrase. Said here rather than by whoever is showing
/// the solve, so a reader stepping through it and a reader reading the code are
/// told the same thing.
impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rule::Absorb => f.write_str("one side is undecided, which unifies with anything"),
            Rule::Same => f.write_str("already the same thing on both sides"),
            Rule::Congruent => f.write_str(
                "the same declared type on both sides, and it keeps what it takes: argument against argument",
            ),
            Rule::Bind => f.write_str("a variable takes the type it is against"),
            Rule::Mut => f.write_str("two cells: region and invariant element types must agree"),
            Rule::Array => f.write_str("two arrays: element type against element type"),
            Rule::Occurs => {
                f.write_str("the variable is inside the type it is against, so no finite type fits")
            }
            // In the reader's nouns, like everything else said about a set of
            // labels: someone who wrote braces is told about a struct and a
            // field, and someone who wrote `#`s about a sum and a case.
            // Saying it about a "row" and a "label" would name the
            // representation the two shapes share, which is the compiler's
            // business and nothing the reader wrote.
            //
            // Two sentences rather than one formula, because the two rests are
            // no longer the same kind of thing. A sum's rest is a set of cases,
            // so only a sum can be one; a struct's rest is the type its fields
            // sit on, so anything at all can be one — and what is refused is
            // whichever of them names a field the struct already has.
            Rule::Overlap { shape } => match shape {
                Shape::Struct => f.write_str(
                    "the rest of a struct cannot be a type naming a field the struct already names",
                ),
                Shape::Sum => f.write_str(
                    "the rest of a sum cannot be a sum naming a case the sum already names",
                ),
                // Written rows and tail substitutions still require one
                // entry per effect constructor.
                Shape::Effect => f.write_str(
                    "the rest of a function's effects cannot name an effect it already performs",
                ),
            },
            Rule::Prim => f.write_str("the same primitive on both sides"),
            Rule::Arrow => f.write_str(
                "two arrows: argument against argument, result against result, effects against effects",
            ),
            Rule::Performs => f.write_str(
                "what calling this may perform, opened into what may be performed where it was written",
            ),
            Rule::Struct => f.write_str(
                "two types and their fields: shared fields field against field, the rest into whatever the other leaves open",
            ),
            Rule::Refine => f.write_str(
                "presence equality inside an arm becomes an implication from that arm's assumption",
            ),
            Rule::Sum => f.write_str(
                "two sums: shared cases case against case, the rest into the other's tail",
            ),
            Rule::Presence { shape } => write!(
                f,
                "whether the {} is there must agree on both sides",
                noun(*shape),
            ),
            Rule::Unfold => f.write_str("a declared type stands for something; ask again about that"),
            Rule::Assume => {
                f.write_str("these two are already being compared, so take them as equal")
            }
            Rule::Mismatch => f.write_str("no rule applies, so the two types cannot be made equal"),
            Rule::Recover => {
                f.write_str("the abandoned result becomes undecided, so nothing echoes it")
            }
        }
    }
}

/// What a step changed, as one line: nothing, a new binding, the goals it broke
/// into, or the complaint it ended in.
impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::None => f.write_str("no change"),
            Effect::Bound { var, value, .. } => write!(f, "?{var} := {value}"),
            // Not "smaller goals": an unfolding replaces a goal with the same
            // question asked about a shape, which is a step towards an answer
            // without being any smaller.
            Effect::Decomposed => f.write_str("replaced by the goals below"),
            Effect::Guarded {
                premise,
                obligation,
            } => write!(f, "requires {obligation} when {premise}"),
            Effect::Failed(kind) => kind.fmt(f),
        }
    }
}

impl ConstraintKind {
    /// A stable, greppable name for this kind of constraint, the way
    /// [`inference::ErrorKind`] is coded. The debugger labels its rows with it
    /// rather than with prose that may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            ConstraintKind::Isolate { .. } => "isolate",
            ConstraintKind::Project { .. } => "project",
            ConstraintKind::Spread { .. } => "spread",
            ConstraintKind::Equal { .. } => "equal",
            ConstraintKind::Let { .. } => "let",
            ConstraintKind::Instance { .. } => "instance",
            ConstraintKind::Match { .. } => "match",
            ConstraintKind::Performs { .. } => "performs",
            ConstraintKind::CallbackCoverage { .. } => "callback-coverage",
        }
    }
}

/// A constraint prints as what it demands, with `~` for "must unify with" —
/// the notation the literature uses, and short enough to sit in a debugger row.
impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Display for ConstraintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintKind::Isolate {
                internal, external, ..
            } => write!(f, "isolate {internal} into {external}"),
            ConstraintKind::Project {
                base,
                field,
                result,
                ..
            } => {
                write!(f, "{base}.{field} -> {result}")
            }
            // The operand against what the literal asks of it, and then what
            // the literal is: the two rows share their tail, which is what
            // the reader is being shown.
            ConstraintKind::Spread {
                operand,
                demand,
                result,
                ..
            } => write!(f, "..{operand} ~ {demand} -> {result}"),
            ConstraintKind::Equal { expected, actual } => write!(f, "{expected} ~ {actual}"),
            // A header rather than a line, because a `let` carries two lists of
            // constraints and a list is not a line: what it says of itself is
            // what the name was bound to while its value was walked, and the
            // level everything the value still leaves open is quantified at. The
            // two lists are rows of their own — see the debugger's Constraints
            // tab, which is where a constraint that has children is read.
            //
            // The clause an annotation promised follows, when one was written:
            // it is what the scheme this `let` publishes requires of its
            // presences, so a reader following the tab is shown the contract
            // beside the type it is a contract about.
            ConstraintKind::Let {
                bound,
                level,
                promised,
                ..
            } => {
                write!(f, "{bound} generalized at level {level}")?;
                if promised.is_true() {
                    return Ok(());
                }
                write!(f, " where {promised}")
            }
            // The name is not spelled: a symbol needs the mint to name it, and
            // nothing here has one. The type is what a reader is following
            // through the tab anyway, and the row it is a child of says which
            // binding it is a copy of.
            ConstraintKind::Instance { ty, .. } => {
                write!(f, "{ty} ~ a fresh copy of what this name was bound to")
            }
            ConstraintKind::Match {
                scrutinee,
                result,
                arms,
                ..
            } => write!(
                f,
                "presence-refined match with {} written arms over {scrutinee} -> {result}",
                arms.len(),
            ),
            // Read as the widening it is rather than as an equation: what
            // calling this may perform has to be allowed where it was written,
            // and the ambient may allow more. The two rows print in the effect
            // notation, since that is what they are.
            ConstraintKind::Performs {
                performed, ambient, ..
            } => write!(
                f,
                "{} performed where {} is allowed",
                effects_shown(performed),
                effects_shown(ambient),
            ),
            ConstraintKind::CallbackCoverage {
                required,
                available,
                ..
            } => write!(
                f,
                "callback {} covered by {}",
                effects_shown(required),
                effects_shown(available),
            ),
        }
    }
}

/// One effect row on its own, in the notation an arrow writes it in: what a
/// [`ConstraintKind::Performs`] shows, where there is no arrow around the row
/// to hand it a shape. A row that says nothing prints as the `|` an empty one
/// is written with rather than as nothing at all, which would leave the line
/// with a gap in it.
fn effects_shown(row: &Row) -> String {
    EffectLabels { row }.to_string()
}

/// A goal prints as a constraint does, in whichever of the three sorts it is
/// about. Generation only ever equates types; the solver is what takes one
/// apart into questions about rows and about presences, and those are what the
/// other two arms show.
impl fmt::Display for Goal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Goal::Type { expected, actual } => write!(f, "{expected} ~ {actual}"),
            Goal::Row { expected, actual } => write!(f, "{expected} ~ {actual}"),
            Goal::Presence { expected, actual } => write!(f, "{expected} ~ {actual}"),
        }
    }
}

fn type_description(description: inference::TypeDescription) -> &'static str {
    use inference::TypeDescription as T;
    match description {
        T::NaturalNumber => "a natural number",
        T::Integer => "an integer",
        T::RealNumber => "a real number",
        T::Text => "text",
        T::Boolean => "a boolean",
        T::Function => "a function",
        T::Struct => "a struct",
        T::TaggedValue => "a tagged value",
        T::Mut => "a mutable cell",
        T::Array => "an array",
        T::DeclaredType => "a declared type",
        T::Undecided => "another type",
    }
}

fn explanation_fact(
    fact: &inference::ExplanationFact,
    contradiction: &inference::Contradiction,
) -> String {
    use inference::ExplanationFactPayload as P;
    if contradiction.kind == inference::ContradictionKind::RecursiveValue {
        return match (fact.origin, fact.subject) {
            (inference::ConstraintOrigin::ApplicationCallee, _)
            | (_, inference::Subject::Callee | inference::Subject::CallShape) => {
                "this call requires the value to accept an input".into()
            }
            (inference::ConstraintOrigin::ApplicationArgument, inference::Subject::Argument) => {
                "this input is required to be the value itself".into()
            }
            (inference::ConstraintOrigin::ApplicationArgument, _) => {
                "this call requires the input to accept itself".into()
            }
            (inference::ConstraintOrigin::Projection, inference::Subject::PatternDemand) => {
                "this field requires the value to contain itself".into()
            }
            (_, inference::Subject::Argument | inference::Subject::Parameter) => {
                "this input is required to contain itself".into()
            }
            _ => "this use requires the value to contain itself".into(),
        };
    }
    match fact.payload {
        P::UsedAsFunction => "this expression is called as a function".into(),
        P::SuppliesArgument => match fact.subject {
            inference::Subject::Parameter => {
                "this call fixes what type the argument must have".into()
            }
            inference::Subject::Argument => "this argument supplies a type to the call".into(),
            _ => "this call relates the argument and parameter types".into(),
        },
        P::BranchResult => match fact.subject {
            inference::Subject::MatchArm => {
                "this branch contributes its type to the match result".into()
            }
            inference::Subject::MatchResult => {
                "all branches must contribute one result type".into()
            }
            _ => "this match requires its branches to agree".into(),
        },
        P::LabelDemand => {
            if let Some(row) = &contradiction.row {
                let (noun, label) = about(row.shape, &row.label);
                format!("this use requires {noun} `{label}`")
            } else {
                "this field access requires a struct".into()
            }
        }
        P::ClosedRow => {
            if let Some(row) = &contradiction.row {
                let (noun, label) = about(row.shape, &row.label);
                format!("this use limits the type so {noun} `{label}` is unavailable")
            } else {
                "this value supplies a non-struct type here".into()
            }
        }
        P::LabelIntroduction => {
            let row = contradiction.row.as_ref().expect("overlap row metadata");
            let (noun, label) = about(row.shape, &row.label);
            format!("this use introduces {noun} `{label}`")
        }
        P::LabelForbidden => {
            let row = contradiction.row.as_ref().expect("overlap row metadata");
            let (noun, label) = about(row.shape, &row.label);
            format!("this `..` remainder already follows named {noun} `{label}`")
        }
        P::CallerChoiceDeclaration => "this annotation leaves the choice to each caller".into(),
        P::CallerChoiceUse => match contradiction.row.as_ref() {
            Some(row) => {
                let label = label(row.shape, &row.label);
                match row.shape {
                    Shape::Struct => format!("this reads field `{label}` from the caller's choice"),
                    Shape::Sum => format!("this matches case `{label}` from the caller's choice"),
                    Shape::Effect => {
                        format!("this requires effect `{label}` from the caller's choice")
                    }
                }
            }
            None => "this use fixes a type that the caller must be free to choose".into(),
        },
        P::CallerChoiceDestination => {
            "this binding's type would carry that choice outside its annotation".into()
        }
        P::EffectUse => {
            let row = contradiction.row.as_ref().expect("effect metadata");
            format!("this may perform effect `{}`", label(row.shape, &row.label))
        }
        P::EffectBoundary => {
            let row = contradiction.row.as_ref().expect("effect metadata");
            let effect = label(row.shape, &row.label);
            match contradiction.kind {
                inference::ContradictionKind::UnhandledEffect => {
                    format!("this top-level computation has no handler for effect `{effect}`")
                }
                inference::ContradictionKind::EffectNotAllowed => {
                    format!("this enclosing function does not allow effect `{effect}`")
                }
                _ => unreachable!("effect boundary payload on non-effect contradiction"),
            }
        }
        P::EffectDeclaration => {
            let row = contradiction.row.as_ref().expect("effect metadata");
            format!("effect `{}` is declared here", label(row.shape, &row.label))
        }
        P::CallbackRequirement => {
            "this callback can perform effects when host code invokes it".into()
        }
        P::ExternCapability => {
            "this extern declaration does not list those callback effects".into()
        }
        P::ExternDeclaration => "this extern declaration owns the host boundary".into(),
        P::PolymorphicExternLeaf => {
            "this variable can produce differently shaped host values".into()
        }
        P::ExternPosition => "host code needs one fixed kind of value at this position".into(),
        P::RequiresType => match fact.subject {
            inference::Subject::Binding
            | inference::Subject::TopLevelBinding
            | inference::Subject::LocalBinding => {
                "this binding keeps one type across all of its uses".into()
            }
            inference::Subject::Annotation => "the annotation fixes the type here".into(),
            inference::Subject::Argument => "this argument has to fit the call".into(),
            inference::Subject::Parameter => "the function requires one parameter type".into(),
            inference::Subject::Term => "this expression supplies its type here".into(),
            inference::Subject::Spread => "only an array can be spread into an array".into(),
            _ => "this use contributes one of the conflicting type requirements".into(),
        },
    }
}

fn displayed_explanation_fact(
    fact: &inference::ExplanationFact,
    at: usize,
    explanation: &inference::InferenceExplanation,
) -> String {
    let message = explanation_fact(fact, &explanation.contradiction);
    let Some(pivot) = explanation
        .pivot
        .as_ref()
        .filter(|pivot| pivot.references.contains(&at))
    else {
        return message;
    };
    if at == pivot.introduced_at {
        let description = match pivot.kind {
            inference::ExplanationPivotKind::WrittenValue => "written value",
            inference::ExplanationPivotKind::FunctionInput => "function input",
            inference::ExplanationPivotKind::BranchResult => "branch result",
            inference::ExplanationPivotKind::ProjectedField => "projected field",
            inference::ExplanationPivotKind::FunctionEffects => "function effects",
            inference::ExplanationPivotKind::Value => "shared value",
        };
        format!("Let’s call this {description} `{}` — {message}", pivot.name)
    } else {
        format!("{message}; this is another requirement on `{}`", pivot.name)
    }
}

fn add_abridgement_note(
    diagnostic: Diagnostic,
    explanation: &inference::InferenceExplanation,
) -> Diagnostic {
    match explanation.omitted_facts {
        0 => diagnostic,
        1 => diagnostic.note("1 intermediate use is omitted from this short explanation"),
        count => diagnostic.note(format!(
            "{count} intermediate uses are omitted from this short explanation"
        )),
    }
}

fn mismatch_title(contradiction: &inference::Contradiction) -> String {
    use inference::ContradictionKind as K;
    let left = type_description(contradiction.left);
    let right = type_description(contradiction.right);
    match contradiction.kind {
        K::ValueUsedAsFunction => {
            let value = if contradiction.left == inference::TypeDescription::Function {
                right
            } else {
                left
            };
            format!("{value} cannot be called as a function")
        }
        K::IncompatibleTypes => format!("{left} and {right} cannot be the same type"),
        K::RecursiveValue => "this value would have to contain or accept itself".into(),
        K::ProjectionOnNonStruct => format!("{left} cannot provide struct fields"),
        K::LabelUnavailable => {
            let row = contradiction
                .row
                .as_ref()
                .expect("row contradiction metadata");
            let (noun, label) = about(row.shape, &row.label);
            format!("{noun} `{label}` is required by one use but excluded by another")
        }
        K::RepeatedLabel => {
            let row = contradiction
                .row
                .as_ref()
                .expect("row contradiction metadata");
            let (noun, label) = about(row.shape, &row.label);
            format!("{noun} `{label}` cannot be both named and included by `..`")
        }
        K::CallerChoice => match &contradiction.row {
            Some(row) => {
                let (noun, label) = about(row.shape, &row.label);
                format!("the body cannot assume caller-chosen {noun} `{label}`")
            }
            None => "the body cannot fix a choice that belongs to each caller".into(),
        },
        K::CallerChoiceEscape => "a caller choice cannot cross into another binding's type".into(),
        K::UnhandledEffect => {
            let row = contradiction.row.as_ref().expect("effect metadata");
            format!(
                "effect `{}` has no enclosing handler",
                label(row.shape, &row.label)
            )
        }
        K::EffectNotAllowed => {
            let row = contradiction.row.as_ref().expect("effect metadata");
            format!(
                "effect `{}` is not allowed by the enclosing function",
                label(row.shape, &row.label)
            )
        }
        K::CallbackEffectsNotCovered => {
            "a callback can perform effects not listed by the extern declaration".into()
        }
        K::PolymorphicExternBoundary => {
            "host code needs one fixed kind of value at this extern position".into()
        }
    }
}

fn effect_row_has_specific_operation(ty: &Ty) -> bool {
    let Ty::Arrow(_, _, effects) = ty else {
        return false;
    };
    effects
        .labels
        .values()
        .any(|field| matches!(field.presence, Presence::Present))
}

fn causal_diagnostic(
    source: &SourceMap,
    mut diagnostic: Diagnostic,
    explanation: &inference::InferenceExplanation,
) -> Diagnostic {
    diagnostic.title = mismatch_title(&explanation.contradiction);
    let mut selected = explanation
        .abridged
        .iter()
        .copied()
        .filter_map(|at| explanation.full_facts.get(at).map(|fact| (at, fact)));
    if let Some((at, primary)) = selected.next() {
        diagnostic.primary.span = source.span(primary.at);
        diagnostic.primary.message = displayed_explanation_fact(primary, at, explanation);
    } else {
        diagnostic.primary.message = "these uses contribute conflicting requirements".into();
    }
    for (at, fact) in selected {
        diagnostic = diagnostic.related(
            source.span(fact.at),
            displayed_explanation_fact(fact, at, explanation),
        );
    }
    add_abridgement_note(diagnostic, explanation)
}

fn callback_effects(effects: &[String]) -> String {
    effects
        .iter()
        .map(|effect| format!("`{}`", label(Shape::Effect, effect)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn callback_issue_details(
    mut diagnostic: Diagnostic,
    issues: &[inference::ExternCallbackIssue],
) -> Diagnostic {
    let mut all_effects = Vec::new();
    let mut tails = Vec::new();
    let mut conditional_repairs = Vec::new();
    for issue in issues {
        let requirements = if issue.requirements.is_empty() {
            issue
                .missing_effects
                .iter()
                .map(|effect| inference::ExternCallbackRequirement {
                    effect: effect.clone(),
                    condition: None,
                })
                .collect::<Vec<_>>()
        } else {
            issue.requirements.clone()
        };
        let mut unconditional = Vec::new();
        for requirement in requirements {
            let effect = callback_effects(std::slice::from_ref(&requirement.effect));
            if let Some(condition) = requirement.condition {
                diagnostic = diagnostic.note(format!(
                    "at {}, callback coverage requires {effect} whenever {condition}",
                    issue.callback_path
                ));
                let repair =
                    format!("list {effect} on this extern declaration whenever {condition} holds");
                if !conditional_repairs.contains(&repair) {
                    conditional_repairs.push(repair);
                }
            } else {
                if !all_effects.contains(&requirement.effect) {
                    all_effects.push(requirement.effect.clone());
                }
                unconditional.push(requirement.effect);
            }
        }
        if !unconditional.is_empty() {
            diagnostic = diagnostic.note(format!(
                "at {}, the callback can perform {} without matching extern capability",
                issue.callback_path,
                callback_effects(&unconditional),
            ));
        }
        for tail in &issue.missing_tails {
            diagnostic = diagnostic.note(format!("at {}, {tail}", issue.callback_path));
            if !tails.contains(tail) {
                tails.push(tail.clone());
            }
        }
    }
    if !all_effects.is_empty() {
        diagnostic = diagnostic.help(format!(
            "list {} on this extern declaration",
            callback_effects(&all_effects)
        ));
    }
    for repair in conditional_repairs {
        diagnostic = diagnostic.help(repair);
    }
    if !tails.is_empty() {
        diagnostic = diagnostic.help(
            "make the extern declaration's effects cover the callback remainder noted at each path",
        );
    }
    diagnostic.help("or handle those effects before the callback returns to host code")
}

impl inference::Error {
    /// Turn an inference failure into reporter-independent words and source
    /// annotations. This is deliberately the only presentation boundary for
    /// type errors: terminals and the debugger must not reconstruct evidence
    /// from the error kind themselves.
    pub fn diagnostic(&self, source: &SourceMap) -> Diagnostic {
        use inference::ErrorKind as E;

        let mut diagnostic = Diagnostic::new(
            self.kind.code(),
            self.kind.to_string(),
            source.span(self.at),
        );
        match &self.kind {
            E::NotAStruct { demand, .. } => {
                let (asked, undo) = match demand {
                    inference::StructDemand::Projection => {
                        ("field access", "change/remove the field access")
                    }
                    inference::StructDemand::Spread => ("spread", "remove the spread"),
                };
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation)
                        .help("change the value to a struct")
                        .help(format!("or {undo}"));
                } else {
                    diagnostic = diagnostic
                        .label(format!("this {asked} requires a struct"))
                        .help(format!("change this value to a struct, or {undo}"));
                }
            }
            E::EffectArgument {
                effect, position, ..
            } => {
                let title = format!(
                    "effect `!{effect}` is used with incompatible {} arguments",
                    ordinal(*position)
                );
                if let Some(explanation) = &self.explanation {
                    diagnostic.title = title;
                    let mut selected = explanation
                        .abridged
                        .iter()
                        .copied()
                        .filter_map(|at| explanation.full_facts.get(at).map(|fact| (at, fact)));
                    if let Some((at, primary)) = selected.next() {
                        diagnostic.primary.span = source.span(primary.at);
                        diagnostic.primary.message =
                            displayed_explanation_fact(primary, at, explanation);
                    } else {
                        diagnostic.primary.message =
                            "these uses of the effect disagree about this argument".into();
                    }
                    for (at, fact) in selected {
                        diagnostic = diagnostic.related(
                            source.span(fact.at),
                            displayed_explanation_fact(fact, at, explanation),
                        );
                    }
                    diagnostic = add_abridgement_note(diagnostic, explanation)
                        .note(format!(
                            "the arguments cannot agree: {}",
                            mismatch_title(&explanation.contradiction)
                        ))
                        .help("use the effect with one argument throughout this computation")
                        .help("or handle one of the uses separately");
                } else {
                    diagnostic = Diagnostic::new(self.kind.code(), title, source.span(self.at))
                        .label("these uses of the effect disagree about this argument")
                        .note(format!("the arguments cannot agree: {}", self.kind.cause()))
                        .help("use the effect with one argument throughout this computation")
                        .help("or handle one of the uses separately");
                }
            }
            E::Mismatch { .. } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic.title = mismatch_title(&explanation.contradiction);
                    let mut selected = explanation
                        .abridged
                        .iter()
                        .copied()
                        .filter_map(|at| explanation.full_facts.get(at).map(|fact| (at, fact)));
                    if let Some((at, primary)) = selected.next() {
                        diagnostic.primary.span = source.span(primary.at);
                        diagnostic.primary.message =
                            displayed_explanation_fact(primary, at, explanation);
                    } else {
                        diagnostic.primary.message =
                            "these uses contribute incompatible type requirements".into();
                    }
                    for (at, fact) in selected {
                        diagnostic = diagnostic.related(
                            source.span(fact.at),
                            displayed_explanation_fact(fact, at, explanation),
                        );
                    }
                    diagnostic = add_abridgement_note(diagnostic, explanation)
                        .help("change the first use so it agrees with the other one")
                        .help("or change the other use so it agrees with the first one");
                } else {
                    diagnostic = diagnostic
                        .label("these uses contribute incompatible type requirements")
                        .help("change either use so both require the same type");
                }
            }
            E::Recursive => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                    diagnostic = match explanation.contradiction.recursive {
                        Some(inference::RecursiveCycleShape::CallInput) => diagnostic
                            .help("change the call so a value is not passed to itself")
                            .help("or change the called value so it accepts a different input"),
                        Some(inference::RecursiveCycleShape::Containment) => diagnostic
                            .help("change the value so it does not contain itself")
                            .help("or change the field that creates the containment"),
                        Some(inference::RecursiveCycleShape::Neutral) | None => diagnostic
                            .help("change one of these uses so the type is finite")
                            .help("or separate the uses so they no longer require the same type"),
                    };
                } else {
                    diagnostic = diagnostic
                        .label("this use requires the value to contain or accept itself")
                        .help("change one of these uses so the type is finite");
                }
            }
            E::MissingField { shape, field, .. } | E::ExtraField { shape, field, .. } => {
                let (noun, field) = about(*shape, field);
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation)
                        .help(format!("allow {noun} `{field}` in the limiting use"))
                        .help(format!("or change the use that requires {noun} `{field}`"));
                } else {
                    diagnostic = diagnostic
                        .label(format!("this use conflicts over {noun} `{field}`"))
                        .help(format!(
                            "allow {noun} `{field}`, or change the use that requires it"
                        ));
                }
            }
            E::RigidBroken {
                declared,
                sense,
                found,
                ..
            } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    diagnostic = diagnostic
                        .label("this use narrows a choice that belongs to the caller")
                        .related(source.span(*declared), DECLARED_HERE);
                }
                diagnostic = match sense {
                    Sense::Region => diagnostic.help("use an explicit region variable, such as `mut 'r Nat`"),
                    Sense::Type => diagnostic
                        .help("return or pass through the annotated value instead of replacing its type")
                        .help("or change the annotation to name the concrete type the body uses"),
                    Sense::Fields => diagnostic
                        .help("preserve the caller-chosen struct remainder instead of closing it")
                        .help("or remove the open remainder from the annotation"),
                    Sense::Cases => diagnostic
                        .help("preserve the caller-chosen remaining cases instead of closing them")
                        .help("or remove the open remainder from the annotation"),
                    Sense::Effects if effect_row_has_specific_operation(found) => diagnostic
                        .help("handle the performed effect inside the body")
                        .help("or list that effect explicitly in the annotation"),
                    Sense::Effects => diagnostic
                        .help("preserve the caller-chosen effect remainder instead of closing it")
                        .help("or remove or change the open effect remainder in the annotation"),
                    Sense::Presence => diagnostic
                        .help("preserve the annotation's label condition in the body")
                        .help("or change the annotation's `where` condition"),
                };
            }
            E::RigidField {
                shape, declared, ..
            } => {
                let choices = match shape {
                    Shape::Struct => "struct fields",
                    Shape::Sum => "cases",
                    Shape::Effect => "effects",
                };
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    diagnostic = diagnostic
                        .label(format!(
                            "this assumes one of the {choices} chosen by the caller"
                        ))
                        .related(
                            source.span(*declared),
                            format!("the caller's choice of {choices} starts here"),
                        );
                }
                diagnostic = match shape {
                    Shape::Struct => diagnostic
                        .help("read caller-chosen struct fields only when named explicitly before the annotation's `..` remainder")
                        .help("or add this field explicitly to the annotation"),
                    Shape::Sum => diagnostic
                        .help("match caller-chosen cases only when named explicitly before the annotation's `..` remainder")
                        .help("or add this case explicitly to the annotation"),
                    Shape::Effect => diagnostic
                        .help("handle this effect instead of requiring it from the caller-chosen effects")
                        .help("or list this effect explicitly before the annotation's effect remainder"),
                };
            }
            E::RigidEscapes {
                destination,
                destination_name,
                destination_span,
                ..
            } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                    // The synthetic escape explanation already contributes a
                    // destination fact. Replace it rather than appending a
                    // second label for the same binding and span.
                    diagnostic
                        .related
                        .retain(|annotation| annotation.span != source.span(*destination_span));
                } else {
                    diagnostic = diagnostic
                        .label("this type carries a caller choice outside its annotation");
                }
                diagnostic = diagnostic
                    .related(
                        source.span(*destination_span),
                        format!(
                            "binding `{destination_name}` has inferred type `{destination}`, which would carry this choice outside its annotation"
                        ),
                    )
                    .help("change or remove the source annotation so its caller choice does not enter this value")
                    .help(format!(
                        "or keep this value from flowing into binding `{destination_name}`"
                    ));
            }
            E::RepeatedField { shape, field, .. } => {
                let (noun, field) = about(*shape, field);
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation)
                        .help(format!("remove {noun} `{field}` from the named side"))
                        .help("or keep it out of the `..` remainder");
                } else {
                    diagnostic = diagnostic
                        .label(format!("{noun} `{field}` is already named outside `..`"))
                        .help(format!(
                            "remove the repeated {noun}, or remove it from the remainder"
                        ));
                }
            }
            E::PresenceRequired { .. } => {
                diagnostic = diagnostic
                    .label("this value does not meet the required combination")
                    .help("change the value or loosen the required combination")
            }
            E::PresenceImpossible { .. } => {
                diagnostic = diagnostic
                    .label("the annotation and definition rule out every combination")
                    .help("change the annotation or change how the definition uses the value")
            }
            E::ClauseImpossible { .. } => {
                diagnostic = diagnostic
                    .label("this clause rules out every combination")
                    .help("remove one of the conflicting requirements in this clause")
            }
            E::AnnotationAllows { .. } => {
                diagnostic = diagnostic
                    .label("this annotation promises more combinations than the body accepts")
                    .help("strengthen the annotation or loosen the body")
            }
            E::Unhandled { effect } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    diagnostic = diagnostic.label(format!(
                        "effect `{}` has no enclosing handler",
                        label(Shape::Effect, effect)
                    ));
                }
                diagnostic = diagnostic.help("handle this effect, or perform it inside a function")
            }
            E::NotAllowed { effect } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    diagnostic = diagnostic.label(format!(
                        "effect `{}` is not listed by this function",
                        label(Shape::Effect, effect)
                    ));
                }
                diagnostic =
                    diagnostic.help("add the effect to the function type, or handle it here")
            }
            E::CallbackEffectsNotCovered {
                missing_effects,
                issues,
                ..
            } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    let effects = if missing_effects.is_empty() {
                        "callback evidence not carried by this extern".into()
                    } else {
                        callback_effects(missing_effects)
                    };
                    diagnostic = diagnostic.label(format!(
                        "this callback requires {effects}, which the extern does not permit"
                    ));
                }
                if issues.is_empty() {
                    let effects = if missing_effects.is_empty() {
                        "effects selected by a conditional callback type".into()
                    } else {
                        callback_effects(missing_effects)
                    };
                    diagnostic = diagnostic
                        .note(format!(
                            "the callback can perform {effects}, which the extern does not permit"
                        ))
                        .help(format!(
                            "list {effects} on this extern declaration whenever the callback condition holds"
                        ))
                        .help("or handle those effects before the callback returns to host code");
                } else {
                    diagnostic = callback_issue_details(diagnostic, issues);
                }
            }
            E::PolymorphicExternBoundary {
                callback_issues, ..
            } => {
                if let Some(explanation) = &self.explanation {
                    diagnostic = causal_diagnostic(source, diagnostic, explanation);
                } else {
                    diagnostic = diagnostic
                        .label("host code needs one fixed kind of value at this position");
                }
                diagnostic = diagnostic
                    .help("use a fixed type at this position")
                    .help("or expose a concrete host-facing type and convert the value in Ruddy");
                if !callback_issues.is_empty() {
                    diagnostic = callback_issue_details(diagnostic, callback_issues);
                }
            }
        }
        diagnostic
    }
}

impl inference::ErrorKind {
    /// A stable, greppable name for this kind of error. Reporters key on it
    /// rather than on the message, which is prose and may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            inference::ErrorKind::NotAStruct { .. } => "not-a-struct",
            inference::ErrorKind::Mismatch { .. } => "type-mismatch",
            inference::ErrorKind::EffectArgument { .. } => "effect-argument-mismatch",
            inference::ErrorKind::Recursive => "recursive-type",
            // A missing case and a missing field are one complaint, so they
            // are one code: what went wrong is that a row was asked for a
            // label it has not got, and a reporter that wants to know which
            // kind of row is reading the shape the complaint carries.
            inference::ErrorKind::MissingField { .. } => "missing-field",
            inference::ErrorKind::ExtraField { .. } => "extra-field",
            // The variable's name is not part of these codes, only of their
            // wording, for the reason a row error's shape is not part of its:
            // what went wrong is that a promise was broken, and which promise
            // it was is the span's — and the note's — to show.
            inference::ErrorKind::RigidBroken { .. } => "rigid-broken",
            inference::ErrorKind::RigidField { .. } => "rigid-field",
            inference::ErrorKind::RigidEscapes { .. } => "rigid-escapes",
            inference::ErrorKind::RepeatedField { .. } => "repeated-field",
            inference::ErrorKind::PresenceRequired { .. } => "presence-required",
            inference::ErrorKind::PresenceImpossible { .. } => "presence-impossible",
            inference::ErrorKind::ClauseImpossible { .. } => "clause-impossible",
            inference::ErrorKind::AnnotationAllows { .. } => "annotation-allows-more",
            inference::ErrorKind::Unhandled { .. } => "unhandled-effect",
            inference::ErrorKind::NotAllowed { .. } => "effect-not-allowed",
            inference::ErrorKind::CallbackEffectsNotCovered { .. } => {
                "callback-effects-not-covered"
            }
            inference::ErrorKind::PolymorphicExternBoundary { .. } => "polymorphic-extern-boundary",
        }
    }
}

/// What went wrong, in one sentence.
impl fmt::Display for inference::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            inference::ErrorKind::NotAStruct { base, demand } => {
                let asked = match demand {
                    inference::StructDemand::Projection => "read",
                    inference::StructDemand::Spread => "spread",
                };
                write!(
                    f,
                    "`{base}` is not a struct, so it has no fields to {asked}"
                )
            }
            inference::ErrorKind::Mismatch { expected, actual } => {
                write!(f, "type mismatch: expected `{expected}`, found `{actual}`")
            }
            inference::ErrorKind::EffectArgument {
                effect,
                position,
                cause,
            } => write!(
                f,
                "effect `!{effect}` is used with incompatible {} arguments: {cause}",
                ordinal(*position)
            ),
            inference::ErrorKind::Recursive => {
                f.write_str("this type would have to contain itself")
            }
            inference::ErrorKind::MissingField { shape, base, field } => {
                let (noun, field) = about(*shape, field);
                write!(f, "no {noun} `{field}` on `{base}`")
            }
            inference::ErrorKind::ExtraField { shape, base, field } => {
                let (noun, field) = about(*shape, field);
                write!(
                    f,
                    "extra {noun} `{field}`: the type `{base}` lists every {noun} it allows",
                )
            }
            // Said as what the expression turned out to be, beside what the
            // annotation promised it would be. Not as two types that failed to
            // agree: one of them is a name standing in for a choice the reader
            // handed to their caller, and "expected `a`, found `Nat`" says
            // nothing about why `a` cannot simply be `Nat`.
            // The effect reading quotes no type: the two rows differ only in
            // their tails there, and "this is `{} -> {} + |`" points a reader
            // at an arrow nobody wrote.
            inference::ErrorKind::RigidBroken {
                sense: Sense::Effects,
                found,
                name,
                ..
            } if effect_row_has_specific_operation(found) => write!(
                f,
                "this restricts which effect it may perform, but `'{name}` stands for whatever effects the caller allows",
            ),
            inference::ErrorKind::RigidBroken {
                sense: Sense::Effects,
                name,
                ..
            } => write!(
                f,
                "this closes the effects it may perform, but `'{name}` stands for whatever effects the caller allows",
            ),
            inference::ErrorKind::RigidBroken {
                found,
                name,
                sense: Sense::Fields,
                ..
            } => write!(
                f,
                "this is `{found}`, but `'{name}` stands for whatever the caller picks for the rest of a struct's fields",
            ),
            inference::ErrorKind::RigidBroken {
                found,
                name,
                sense: Sense::Cases,
                ..
            } => write!(
                f,
                "this is `{found}`, but `'{name}` stands for whatever the caller picks for the remaining cases",
            ),
            inference::ErrorKind::RigidBroken { found, name, .. } => write!(
                f,
                "this is `{found}`, but `'{name}` stands for whatever type the caller picks",
            ),
            inference::ErrorKind::RigidField {
                shape, field, name, ..
            } => {
                let field = label(*shape, field);
                let (action, choices) = match shape {
                    Shape::Struct => ("reads field", "other struct fields"),
                    Shape::Sum => ("matches case", "other cases"),
                    Shape::Effect => ("requires effect", "other effects"),
                };
                write!(
                    f,
                    "this {action} `{field}`, but `'{name}` stands for whatever {choices} the caller chooses, so `{field}` cannot be assumed",
                )
            }
            // Said at the declaration, because that is the line that has to
            // change: the type the variable leaked into is somewhere the reader
            // never wrote it down.
            inference::ErrorKind::RigidEscapes {
                name,
                destination,
                destination_name,
                ..
            } => write!(
                f,
                "`'{name}` stands for whatever that annotation's caller picks, but binding `{destination_name}` would publish it as `{destination}` outside that annotation",
            ),
            // Said as what `..` means rather than as the two rows that
            // disagreed: neither of those is a type the reader wrote, and the
            // field is the whole of what they can change.
            inference::ErrorKind::RepeatedField { shape, field, .. } => write!(
                f,
                "`..` covers only the {}s a type does not already name, and here it would have to cover `{}`",
                noun(*shape),
                label(*shape, field),
            ),
            // Said in the labels the reader wrote rather than in the presence
            // variables the compiler gave them: what has to change is the value
            // on this line, and the labels of this shape are the whole of what
            // they can change about it.
            inference::ErrorKind::PresenceRequired { formula, shape } => match shape {
                Some(shape) => write!(
                    f,
                    "this value needs `{formula}` among its {}s, and it does not have that",
                    noun(*shape),
                ),
                None => write!(
                    f,
                    "this value needs `{formula}` among its labels, and it does not have that",
                ),
            },
            inference::ErrorKind::PresenceImpossible { formula } => write!(
                f,
                "nothing can satisfy `{formula}`: what this definition does with the type has already ruled it out",
            ),
            // The same shape of sentence with the definition left out of it,
            // because a clause that contradicts itself does so over any body
            // at all — including one that never touches the type.
            inference::ErrorKind::ClauseImpossible { formula } => write!(
                f,
                "nothing can satisfy `{formula}`: this clause rules out every value at once",
            ),
            inference::ErrorKind::AnnotationAllows { allowed, required } => write!(
                f,
                "the annotation allows `{allowed}`, but the definition requires `{required}`",
            ),
            // Said as why there is nobody to answer it rather than as the row
            // that could not take it: outside every function there is no
            // signature to widen, and what the reader can change is where the
            // value is computed.
            inference::ErrorKind::Unhandled { effect } => write!(
                f,
                "nothing can handle `{}` here: a definition's value is computed outside every handler",
                label(Shape::Effect, effect),
            ),
            inference::ErrorKind::NotAllowed { effect } => write!(
                f,
                "this function performs `{}`, which its type does not allow",
                label(Shape::Effect, effect),
            ),
            inference::ErrorKind::CallbackEffectsNotCovered {
                missing_effects,
                callback_path,
                callback_type,
                extern_name,
                ..
            } => {
                let effects = if missing_effects.is_empty() {
                    "effects selected by a conditional callback type".into()
                } else {
                    missing_effects
                        .iter()
                        .map(|effect| format!("`{}`", label(Shape::Effect, effect)))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                write!(
                    f,
                    "callback `{callback_type}` at {callback_path} can perform {effects}, which extern `{extern_name}` does not permit"
                )
            }
            inference::ErrorKind::PolymorphicExternBoundary {
                variable,
                variable_kind,
                position,
                extern_name,
                ..
            } => {
                let varies = match variable_kind {
                    inference::ExternVariableKind::Type => "can be many different types",
                    inference::ExternVariableKind::Row => {
                        "leaves open which fields or effects the value carries"
                    }
                    inference::ExternVariableKind::Presence => {
                        "leaves open whether parts of the value are present"
                    }
                };
                write!(
                    f,
                    "host code needs one fixed kind of value at {position}, but `{variable}` in extern `{extern_name}` {varies}"
                )
            }
        }
    }
}

impl Origin {
    /// A stable, greppable name for why a batch is in the store, the way every
    /// other kind in this file is coded. The debugger's Presence tab labels its
    /// rows with it rather than with prose that may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            Origin::Coverage(_) => "match-coverage",
            Origin::Instance(_) => "use-site",
            Origin::Annotation(_) => "annotation",
            Origin::Refinement(_) => "branch-refinement",
            Origin::Guarded(_) => "guarded",
        }
    }
}

/// Why a batch is in the store, in a phrase — what the Presence tab prints
/// beside each one.
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Coverage(_) => f.write_str("what this match's arms cover between them"),
            Origin::Instance(_) => f.write_str("what this use of a name requires of its presences"),
            Origin::Annotation(_) => f.write_str("what this annotation's `where` clause promises"),
            Origin::Refinement(_) => {
                f.write_str("a structural presence relation produced in this arm")
            }
            Origin::Guarded(guarded) => write!(f, "{} when {}", guarded.origin, guarded.premise,),
        }
    }
}

/// Render `func arg`. A lambda on the left would swallow the argument into its
/// own body, and anything that keeps consuming to its right would swallow
/// whatever follows the argument.
/// Render `value |> function`.
pub fn write_pipeline(
    f: &mut fmt::Formatter<'_>,
    value: &impl Grouped,
    function: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, value.prec() < Prec::Pipeline, value)?;
    f.write_str(" |> ")?;
    write_grouped(f, function.prec() <= Prec::Pipeline, function)
}

/// Render a unary numeric operator.
pub fn write_unary(f: &mut fmt::Formatter<'_>, op: &str, value: &impl Grouped) -> fmt::Result {
    f.write_str(op)?;
    // Keep operator negation distinct from a signed literal, and never
    // join two minus signs into the start of a line comment.
    write_grouped(f, op == "-" || value.prec() < Prec::Unary, value)
}

/// Render a left-associative numeric binary operator.
pub fn write_binary(
    f: &mut fmt::Formatter<'_>,
    left: &impl Grouped,
    op: &str,
    right: &impl Grouped,
    prec: Prec,
) -> fmt::Result {
    write_grouped(
        f,
        if prec == Prec::Assignment {
            left.prec() <= prec
        } else {
            left.prec() < prec
        },
        left,
    )?;
    write!(f, " {op} ")?;
    write_grouped(
        f,
        if prec == Prec::Assignment {
            right.prec() < prec
        } else {
            right.prec() <= prec
        },
        right,
    )
}

pub fn write_apply(
    f: &mut fmt::Formatter<'_>,
    func: &impl Grouped,
    arg: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, func.prec() < Prec::Apply, func)?;
    f.write_str(" ")?;
    write_grouped(f, arg.prec() < Prec::Atom, arg)
}

/// Render `head arg arg ...` — the flat form of [`write_apply`], for the type
/// language.
///
/// Flat rather than folded pairwise because a type constructor is applied to
/// everything it takes at once: there is no half-applied thing for an
/// intermediate node to stand for, so there is none to hand to
/// [`write_apply`].
///
/// The head is grouped against an atom rather than against an application, as
/// [`write_apply`] groups its function: the form is flat, so `head arg arg`
/// says the head is one atom, and an application there would swallow the
/// arguments that follow it. The compiler's own printers never see a
/// parenthesis here — a name is an atom, and only a declared name reaches this
/// far in the IR — but the parse tree's printer renders what was written, and
/// anything at all can be *written* applied.
pub fn write_applied<H: Grouped, A: Grouped>(
    f: &mut fmt::Formatter<'_>,
    head: H,
    args: impl IntoIterator<Item = A>,
) -> fmt::Result {
    write_grouped(f, head.prec() < Prec::Atom, &head)?;
    for arg in args {
        f.write_str(" ")?;
        write_grouped(f, arg.prec() < Prec::Atom, &arg)?;
    }
    Ok(())
}

/// Render `from -> to + effects`. The arrow is right-associative, so the left
/// side needs grouping — an arrow there would otherwise re-parse as the outer
/// arrow's right half — and so, sometimes, does the result.
///
/// The left is grouped against the arrow rather than against an atom, so only
/// what could swallow the arrow is bracketed: an arrow, and a lambda. An
/// application is left alone, because it stops at the arrow of its own accord
/// and `Pair A B -> Nat` is how a person would write it — and so is a sum, whose
/// last case carries an atom and so cannot reach the arrow either.
///
/// The result is bracketed exactly when *this* arrow carries a row and the
/// result is itself an arrow, and in no other case. That is R24, and it follows
/// from where the `+` binds: a `+` attaches to the innermost arrow parsed at its
/// own level, so `A -> B -> C + E` puts `E` on `B -> C` and `A -> (B -> C) + E`
/// puts it on the outer one. Bracketing whenever the *result* carries a row —
/// the reading that looks safer — would move `E` to the wrong arrow on
/// re-reading.
///
/// `effects` is what follows the `+`, already rendered, and `None` is a pure
/// arrow, which writes no clause at all.
pub fn write_arrow(
    f: &mut fmt::Formatter<'_>,
    from: &impl Grouped,
    to: &impl Grouped,
    effects: Option<&dyn fmt::Display>,
) -> fmt::Result {
    write_grouped(f, from.prec() < Prec::Sum, from)?;
    f.write_str(" -> ")?;
    // An arrow is the one node at this level, in all three printers, so
    // comparing against it is asking exactly "is the result an arrow".
    write_grouped(f, effects.is_some() && to.prec() == Prec::Arrow, to)?;
    match effects {
        Some(effects) => write!(f, " + {effects}"),
        None => Ok(()),
    }
}

/// Render a `{ name: value, ..spread }` struct literal, shared by both trees:
/// they reach the name and the value differently — one off a spanned key,
/// another off the map's key and a field — so the pairs arrive already
/// rendered. The wrapper over [`write_row`] for the one position whose
/// fields have no presence: they are simply there. After them, past the
/// `..`, comes the value the literal spreads — written where a type writes
/// its tail, and by the same rule, so the two cannot drift apart — and
/// `None` is a literal with no spread, which writes no `..` at all.
///
/// The spread value is never grouped: it is a whole expression, read as far
/// as the `,` or `}` that ends the literal, and nothing an expression can
/// contain reads as either of those.
pub fn write_struct<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = (K, V)>,
    spread: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let fields = fields.into_iter().map(|(name, value)| Entry::Written {
        name,
        mark: None,
        holds: value,
    });
    write_row(f, fields, spread)
}

/// Return the insertion indices that put canonical tuple field names in
/// positional order.
///
/// A tuple lowered into a struct has exactly the decimal keys `0` through
/// `len - 1`. This check deliberately rejects leading zeroes, gaps, and every
/// other numeric-looking spelling, and returns indices rather than reordered
/// names so callers can retrieve values (and their spans) from their own map.
/// At least one field is required: the empty struct is unit, not a tuple.
///
/// The result is independent of map insertion order. That matters after rows
/// have passed through unification, where the labels remain the same but their
/// storage order is not surface syntax.
pub fn tuple_field_order<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<Vec<usize>> {
    let names: Vec<&str> = names.into_iter().collect();
    if names.is_empty() {
        return None;
    }
    let mut order = vec![usize::MAX; names.len()];
    for (insertion, name) in names.into_iter().enumerate() {
        let position = canonical_tuple_index(name)?;
        if position >= order.len() {
            return None;
        }
        if std::mem::replace(&mut order[position], insertion) != usize::MAX {
            return None;
        }
    }
    order
        .iter()
        .all(|index| *index != usize::MAX)
        .then_some(order)
}

/// Decode an exact canonical tuple field name. Numeric-looking struct labels
/// such as `00` remain ordinary quoted labels rather than changing meaning
/// when printed as projections.
pub fn canonical_tuple_index(name: &str) -> Option<usize> {
    let index = name.parse::<usize>().ok()?;
    (index.to_string() == name).then_some(index)
}

/// Render tuple elements in canonical surface syntax. A singleton keeps the
/// trailing comma that distinguishes it from grouping.
pub fn write_tuple<V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    elements: impl IntoIterator<Item = V>,
) -> fmt::Result {
    f.write_str("(")?;
    let mut count = 0;
    for element in elements {
        if count > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{element}")?;
        count += 1;
    }
    if count == 1 {
        f.write_str(",")?;
    }
    f.write_str(")")
}

/// Render an array pattern in canonical surface syntax: the elements before
/// the rest, the rest as `..` with its name when it has one, and the elements
/// after it, comma-separated inside brackets. `rest` is `None` for an exact
/// pattern, `Some(None)` for a bare `..`, and `Some(Some(name))` for a named
/// one.
pub fn write_array_pattern<V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    before: impl IntoIterator<Item = V>,
    rest: Option<Option<&str>>,
    after: impl IntoIterator<Item = V>,
) -> fmt::Result {
    f.write_str("[")?;
    let mut count = 0;
    let mut separate = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
        if count > 0 {
            f.write_str(", ")?;
        }
        count += 1;
        Ok(())
    };
    for element in before {
        separate(f)?;
        write!(f, "{element}")?;
    }
    if let Some(name) = rest {
        separate(f)?;
        f.write_str("..")?;
        if let Some(name) = name {
            f.write_str(name)?;
        }
    }
    for element in after {
        separate(f)?;
        write!(f, "{element}")?;
    }
    f.write_str("]")
}

// These pieces do not depend on `K` or `V`. Keeping them outside `write_row`
// also keeps one monomorphization from owning a form only another one renders.
fn write_row_mark(f: &mut fmt::Formatter<'_>, mark: Option<&Mark>) -> fmt::Result {
    match mark {
        Some(Mark::Undecided) => f.write_str("?"),
        // Bare between the label and the colon: the colon is what ends the
        // clause, which is why a struct's needs no parentheses.
        Some(Mark::When(name)) => write!(f, " when {name}"),
        None => Ok(()),
    }
}

fn write_row_tail(f: &mut fmt::Formatter<'_>, tail: &dyn fmt::Display) -> fmt::Result {
    write!(f, "..{tail}")
}

/// Render a `{ name: value, name?: value, \name, ..tail }` row: fields, each
/// possibly marked optional — or written `\name`, explicitly absent, with no
/// value at all — and then whatever is known about the fields not named.
///
/// `tail` is what follows the `..` — a row variable's spelling, or nothing —
/// and `None` means the row is closed and no `..` is written at all. The `..`
/// itself is written here, so the callers agree on it by construction.
pub fn write_row<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = Entry<K, V>>,
    tail: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let mut fields = fields.into_iter().peekable();
    // The empty struct is unit, which reads as `{}` — the padding a struct with
    // fields gets would only be two spaces around nothing.
    if fields.peek().is_none() && tail.is_none() {
        return f.write_str("{}");
    }

    f.write_str("{ ")?;
    let mut first = true;
    for field in fields {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        match field {
            Entry::Written { name, mark, holds } => {
                write_field_label(f, &name.to_string())?;
                write_row_mark(f, mark.as_ref())?;
                write!(f, ": {holds}")?;
            }
            Entry::Absent { name } => {
                f.write_str("\\")?;
                write_field_label(f, &name.to_string())?;
            }
        }
    }
    if let Some(tail) = tail {
        if !first {
            f.write_str(", ")?;
        }
        write_row_tail(f, tail)?;
    }
    f.write_str(" }")
}

/// Render a `#A Nat | #B? | \#C | ..tail` sum: cases, each wearing a `#` and
/// possibly a `when` clause — or a leading `\`, explicitly absent, with no
/// payload — each with a payload or without one, and then whatever is known
/// about the cases not named.
///
/// The counterpart of [`write_row`], and the same contract: `tail` is what
/// follows the `..`, `None` means the sum names every case there is, and the
/// `|`s are written here so the callers agree on them by construction.
///
/// The leading `|` the grammar allows is not written — `#A | #B` reads
/// better inline, and inside the parentheses a nested sum needs it would be
/// noise. It comes back for the one form that cannot do without it: a sum with
/// no cases written out is `|`, and `| ..'r` for one that is only a tail, since
/// a bare `..'r` begins no type the parser would read back.
pub fn write_sum<K: fmt::Display, V: Grouped>(
    f: &mut fmt::Formatter<'_>,
    cases: impl IntoIterator<Item = Entry<K, Option<V>>>,
    tail: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let mut first = true;
    for case in cases {
        if !first {
            f.write_str(" | ")?;
        }
        first = false;
        match case {
            Entry::Written { name, mark, holds } => {
                write_tag(f, &name.to_string(), mark.as_ref(), holds)?
            }
            Entry::Absent { name } => {
                f.write_str("\\")?;
                write_tag_label(f, &name.to_string())?;
            }
        }
    }
    // The empty sum, and the sum that is nothing but its tail: neither writes a
    // case, so neither would be read back as a sum without this.
    if first {
        f.write_str("|")?;
    }
    match tail {
        Some(tail) => {
            if !first {
                f.write_str(" |")?;
            }
            write!(f, " ..{tail}")
        }
        None => Ok(()),
    }
}

/// Render `!Log + !IO (when b) + ..tail` — the effects an arrow may
/// perform, after the `+` that introduces them.
///
/// [`write_sum`] minus two things, and both differences are the syntax's. An
/// effect carries nothing, so no case writes a payload; and a row that is
/// nothing but a tail is written `..'e` rather than `+ ..'e`, because the `+`
/// before it already says a row begins here and a second one would be noise.
/// The empty closed row writes nothing at all, which is what a pure arrow is —
/// so the caller leaves the whole clause off rather than calling this.
///
/// A slice rather than the `impl IntoIterator` its two neighbours take, and a
/// bare `&str` rather than a label of any shape: this is one function rather
/// than one per caller, so what a reader is shown is one rendering of an effect
/// row and not three that happen to agree. A row is a handful of labels, so
/// gathering one costs nothing worth the alternative.
pub fn write_effects(
    f: &mut fmt::Formatter<'_>,
    effects: &[Entry<&str, ()>],
    tail: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let mut first = true;
    for effect in effects {
        if !first {
            f.write_str(" + ")?;
        }
        first = false;
        match effect {
            Entry::Written { name, mark, .. } => {
                f.write_str(&label(Shape::Effect, name))?;
                match mark {
                    // The `?` no syntax reads, kept for the presence a failure
                    // abandoned — the reason [`Mark::Undecided`] survives.
                    Some(Mark::Undecided) => f.write_str("?")?,
                    // Parenthesized for the reason a sum case's is: an effect
                    // has no colon to end a bare clause.
                    Some(Mark::When(name_of)) => write!(f, " (when {name_of})")?,
                    None => {}
                }
            }
            Entry::Absent { name } => write!(f, "\\{}", label(Shape::Effect, name))?,
        }
    }
    match tail {
        Some(tail) => {
            if !first {
                f.write_str(" + ")?;
            }
            write!(f, "..{tail}")
        }
        // The empty row, which a caller only reaches by writing `+ |` and
        // meaning it: the row that allows nothing at all.
        None if first => f.write_str("|"),
        None => Ok(()),
    }
}

// Marks do not depend on the payload type, so do not duplicate their writer
// across every payload monomorphization.
fn write_tag_mark(f: &mut fmt::Formatter<'_>, mark: Option<&Mark>) -> fmt::Result {
    // A case has no colon to end a bare clause, so its `when` takes
    // parentheses — one token of lookahead would otherwise not tell
    // `#A (when a)` from `#A when` carrying a type called `when`.
    match mark {
        Some(Mark::Undecided) => f.write_str("?"),
        Some(Mark::When(name_of)) => write!(f, " (when {name_of})"),
        None => Ok(()),
    }
}

/// Render one case of a sum — `#Name`, the mark it may wear, and what it
/// carries — grouped so that a payload nothing can be appended to is left
/// bare and anything else is bracketed.
///
/// One case and one tag literal are the same thing written in two places, so
/// they are written by one function: `#Some 1` in a term and `#Some Nat`
/// in a type differ in what follows the name and in nothing else.
///
/// A payload of `None` writes nothing at all, which is how a case that carries
/// unit is spelled. Whether a given type *is* unit is the caller's to decide —
/// the term printers know it because nothing was written, and the type printer
/// works it out — so this is handed the answer rather than reaching for it.
pub fn write_tag<V: Grouped>(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    mark: Option<&Mark>,
    payload: Option<V>,
) -> fmt::Result {
    write_tag_label(f, name)?;
    write_tag_mark(f, mark)?;
    match payload {
        Some(payload) => {
            f.write_str(" ")?;
            write_grouped(f, payload.prec() < Prec::Atom, &payload)
        }
        None => Ok(()),
    }
}

/// Render `let <binder> [: <type>] = <value>` — one binding of a block, in
/// whichever tree it is being read from. The value is not grouped: it ends in
/// front of the next `let`, the `return`, or the `end` of its own accord,
/// since none of the three begins an atom.
pub fn write_let(
    f: &mut fmt::Formatter<'_>,
    binder: &impl fmt::Display,
    ty: Option<impl fmt::Display>,
    value: &impl fmt::Display,
) -> fmt::Result {
    write!(f, "let {binder}")?;
    if let Some(ty) = ty {
        write!(f, " : {ty}")?;
    }
    write!(f, " = {value}")
}

/// Render `do <stmt>* [return <value>] end` — one writer for both trees, so
/// the punctuation of a block is one rule.
///
/// Nothing here needs grouping. Each statement ends in front of the next of
/// its own accord, the `return`'s value is the last thing before the `end`,
/// and the `end` closes the whole form. A block that needs parentheses needs
/// them from whatever it was written inside, which is what [`Prec::Apply`]
/// says of it.
pub fn write_do<S: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    stmts: impl IntoIterator<Item = S>,
    result: Option<impl fmt::Display>,
) -> fmt::Result {
    f.write_str("do")?;
    for stmt in stmts {
        write!(f, " {stmt}")?;
    }
    if let Some(result) = result {
        write!(f, " return {result}")?;
    }
    f.write_str(" end")
}

/// Render `match <scrutinee> with | <pattern> => <body> ... end` — one
/// writer for both trees, so the punctuation of a match is one rule.
///
/// Nothing here needs grouping. The scrutinee ends at the `with` however far
/// right it runs, each arm's body ends at the next `|` or the `end` — none of
/// the three begins an atom — and the `end` closes the whole form. The
/// leading `|` is written on every arm, first included, as the grammar
/// requires. A match with no arms is `match <scrutinee> with end` with no bar.
pub fn write_match<P: fmt::Display, B: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    scrutinee: &dyn fmt::Display,
    arms: impl IntoIterator<Item = (P, B)>,
) -> fmt::Result {
    write!(f, "match {scrutinee} with")?;
    for (pattern, body) in arms {
        write!(f, " | {pattern} => {body}")?;
    }
    f.write_str(" end")
}

/// Render `base.field`. Projection binds tighter than everything that follows a
/// space, so only the forms that extend rightward need grouping. Consecutive
/// numeric projections are the exception: the lexer deliberately rejects
/// `.0.0` as decimal-like malformed syntax, so the base is parenthesized.
pub fn write_project(f: &mut fmt::Formatter<'_>, base: &impl Grouped, field: &str) -> fmt::Result {
    let index = canonical_tuple_index(field);
    write_grouped(
        f,
        base.prec() < Prec::Atom || (index.is_some() && base.ends_in_numeric_projection()),
        base,
    )?;
    f.write_str(".")?;
    match index {
        Some(index) => write!(f, "{index}"),
        None => write_field_label(f, field),
    }
}

/// Render `body`, wrapping it in parentheses when leaving them off would make
/// the printed source re-parse as a different tree.
fn write_grouped(f: &mut fmt::Formatter<'_>, parens: bool, body: &dyn fmt::Display) -> fmt::Result {
    match parens {
        true => write!(f, "({body})"),
        false => write!(f, "{body}"),
    }
}

#[cfg(test)]
mod tests {
    //! Display tests whose subjects carry solver identifiers. Fabricating one
    //! is crate-private, so they live here rather than in the workspace's
    //! test crate.

    use crate::tracking::Anchor;

    use std::sync::Arc;

    use crate::{
        inference::{self, Constraint, ConstraintKind, Effect, ErrorKind as TypeError},
        symbol::{Bundle, Mint, Namespace, Version},
        types::{Assigned, Formula, Presence, Rest, Row, RowField, Ty},
    };

    /// A constraint prints as what it demands, in the notation the Constraints tab
    /// shows it in. `~` is "must unify with".
    #[test]
    fn a_constraint_reads_as_what_it_demands() {
        let nat = Arc::new(Ty::plain(Ty::Nat));
        let span = Anchor::GENERATED;

        let equal = Constraint {
            id: inference::ConstraintId::synthetic(0),
            reason: inference::ReasonId::synthetic(0),
            at: span,
            origin: inference::ConstraintOrigin::ContextualCheck,
            subjects: inference::ConstraintSubjects::pair(
                inference::Subject::Context,
                inference::Subject::Term,
            ),
            kind: ConstraintKind::Equal {
                expected: nat.clone(),
                actual: Arc::new(Ty::plain(Ty::Var(0))),
            },
        };
        assert_eq!(equal.to_string(), "Nat ~ ?0");
        // The constraint prints as its kind, so the two cannot drift.
        assert_eq!(equal.to_string(), equal.kind.to_string());
    }

    /// An effect is one line beside the rule that produced it. A failure says the
    /// error and nothing else, so the row reads as the complaint rather than as a
    /// wrapper around one.
    #[test]
    fn an_effect_reads_as_the_one_thing_that_changed() {
        assert_eq!(Effect::None.to_string(), "no change");
        assert_eq!(
            Effect::Bound {
                var: 3,
                value: Assigned::Ty(Arc::new(Ty::plain(Ty::Nat))),
                by: inference::ReasonId::synthetic(0),
                because: None,
            }
            .to_string(),
            "?3 := Nat"
        );
        let failure = TypeError::Recursive;
        assert_eq!(
            Effect::Failed(failure.clone()).to_string(),
            failure.to_string()
        );
        assert_eq!(
            Effect::Guarded {
                premise: Formula::var(1),
                obligation: Formula::var(2).not(),
            }
            .to_string(),
            "requires not ?2 when ?1"
        );
    }

    /// The three sorts print on their own as well as inside a type, because the
    /// solver's own record shows them there: a step binding a row variable or a
    /// presence variable has nothing but the value to show.
    #[test]
    fn the_three_sorts_each_print_on_their_own() {
        // A row prints as what it allows. One that names nothing prints as its
        // rest alone, so closing a row reads as the nothing it closed to rather
        // than as an empty pair of braces standing for the same thing.
        assert_eq!(Row::closed().to_string(), "∅");
        assert_eq!(
            Row {
                labels: Default::default(),
                rest: Rest::Var(3),
            }
            .to_string(),
            "?3"
        );
        assert_eq!(
            Row {
                labels: [(
                    "x".to_string(),
                    RowField::present(Arc::new(Ty::plain(Ty::Nat)))
                )]
                .into_iter()
                .collect(),
                rest: Rest::Var(9),
            }
            .to_string(),
            "{ x: Nat, ..?9 }"
        );

        for (rest, printed) in [
            (Rest::Closed, "∅"),
            (Rest::Var(4), "?4"),
            (Rest::Bound(0), "'a"),
            (Rest::Undecided, "?"),
            (Rest::More(Arc::new(Row::closed())), "∅"),
        ] {
            assert_eq!(rest.to_string(), printed);
        }

        for (presence, printed) in [
            (Presence::Present, "present"),
            (Presence::Absent, "absent"),
            (Presence::Var(4), "?4"),
            // A presence is a variable like the other two, and wears the sigil a
            // `when` clause writes it with.
            (Presence::Bound(1), "'b"),
            (Presence::Undecided, "?"),
        ] {
            assert_eq!(presence.to_string(), printed);
        }

        // A binding prints as the value, whichever sort it is, so the Solve tab's
        // one column serves all three.
        for (value, printed) in [
            (Assigned::Ty(Arc::new(Ty::plain(Ty::Nat))), "?2 := Nat"),
            (Assigned::Row(Arc::new(Row::closed())), "?2 := ∅"),
            (Assigned::Presence(Presence::Absent), "?2 := absent"),
        ] {
            assert_eq!(
                Effect::Bound {
                    var: 2,
                    value,
                    by: inference::ReasonId::synthetic(0),
                    because: None,
                }
                .to_string(),
                printed.to_string()
            );
        }
    }

    /// The two kinds a nested `let` adds, read as what they say. A `let` carries
    /// two lists rather than a pair of types, so it prints as the header of the
    /// tree its children make; a use of the name it bound cannot spell the name at
    /// all, there being no mint here to spell one with, so it says what it is
    /// instead. An annotation's clause follows the header, because that is what the
    /// scheme the `let` publishes requires of its presences.
    #[test]
    fn the_scoping_constraints_read_as_what_they_do() {
        let nat = Arc::new(Ty::plain(Ty::Nat));
        let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
        let symbol = mint.local(None, Namespace::Terms, "x");

        let bound = Constraint {
            id: inference::ConstraintId::synthetic(0),
            reason: inference::ReasonId::synthetic(0),
            at: Anchor::GENERATED,
            origin: inference::ConstraintOrigin::Binding,
            subjects: inference::ConstraintSubjects::one(inference::Subject::Binding),
            kind: ConstraintKind::Let {
                symbol,
                bound: nat.clone(),
                level: 2,
                promised: Formula::True,
                rigids: Vec::new(),
                initializer_effects: Row::closed(),
                ambient: Row::closed(),
                inside: true,
                value: Vec::new(),
                body: Vec::new(),
            },
        };
        assert_eq!(bound.kind.code(), "let");
        assert_eq!(bound.to_string(), "Nat generalized at level 2");
        assert_eq!(bound.to_string(), bound.kind.to_string());

        let promised = ConstraintKind::Let {
            symbol,
            bound: nat.clone(),
            level: 2,
            promised: Formula::var(0).xor(Formula::var(1)),
            rigids: Vec::new(),
            initializer_effects: Row::closed(),
            ambient: Row::closed(),
            inside: true,
            value: Vec::new(),
            body: Vec::new(),
        };
        assert_eq!(
            promised.to_string(),
            "Nat generalized at level 2 where ?0 != ?1"
        );

        let use_site = ConstraintKind::Instance {
            symbol,
            ty: Arc::new(Ty::plain(Ty::Var(4))),
            requirement: 0,
        };
        assert_eq!(use_site.code(), "instance");
        assert_eq!(
            use_site.to_string(),
            "?4 ~ a fresh copy of what this name was bound to"
        );
    }
}
