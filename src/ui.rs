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

use std::fmt;

use indexmap::IndexMap;

use crate::{
    bundle,
    inference::{self, Constraint, ConstraintKind, Effect, Goal, Origin, Rule},
    ir, parse, patterns,
    symbol::{Bundle, LOCAL_SEGMENT, Mint, Namespace, Symbol},
    token::{self, Kind},
    types::{
        Assigned, Atom, Core, Formula, Presence, Prim, Rest, Row, RowField, Scheme, Sense, Shape,
        Ty,
    },
};

/// The note a duplicate definition points back with, printed against the span
/// of the definition that stands. Held apart from [`ir::ErrorKind`]'s own
/// wording because it is a second line about a second place, and only a
/// reporter knows how to attach one.
pub const FIRST_DEFINITION: &str = "first defined here";

/// The note a clash of tails points back with, printed against the span of the
/// `..` that decided what the name stands for. [`FIRST_DEFINITION`]'s
/// counterpart for the complaints about a name that was used rather than
/// defined twice — see [`ir::ErrorKind::MixedTail`].
pub const FIRST_USE: &str = "first used here";

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

/// A node a printer has to parenthesize by precedence. Implemented by every
/// wrapper that prints as surface syntax, and by [`Ty`], which prints as one
/// directly.
pub trait Grouped: fmt::Display {
    fn prec(&self) -> Prec;
}

/// A reference groups as what it points at, so a printer can hand out borrowed
/// nodes without every caller wrapping them first.
impl<T: Grouped + ?Sized> Grouped for &T {
    fn prec(&self) -> Prec {
        (**self).prec()
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
    /// `value |> function`.
    Pipeline,
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
    /// `<core> with { ... }` — the fields extend rightward, so a `with` needs
    /// parentheses anywhere a field list could be mistaken for something
    /// else's.
    ///
    /// Above the arrow and the sum, which is what leaves a `with` type bare on
    /// either side of an arrow and puts the parentheses round an arrow or a sum
    /// used as the core of one. Below [`Prec::Tag`], and so below an argument
    /// position: a `with` type handed to a type constructor or carried by a tag
    /// is bracketed, because the fields would otherwise read as the next thing
    /// along.
    With,
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

/// A row read as one of the two shapes: labels in braces, or cases with their
/// `#`s.
///
/// What a spliced tail prints as, and the one place a row's notation is chosen
/// by something other than the position the row sits in. A tail is written by
/// the row it ends — see [`tail_of`] — so the shape travels down from the type
/// whose row it is, and `#Err Nat | ..#Ok Nat` never comes out spelled with
/// a colon. See [`Display for Labels`](Labels) for the two writers it picks
/// between, and [`Display for Row`](Row) for what a row with no shape to hand
/// down falls back to.
struct Labels<'a> {
    shape: Shape,
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

impl token::ErrorKind {
    /// A stable, greppable name for this kind of error. Reporters key on it
    /// rather than on the message, which is prose and may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            token::ErrorKind::Unrecognized => "unrecognized-character",
            token::ErrorKind::MalformedNatural => "malformed-natural",
            token::ErrorKind::NaturalTooLarge => "natural-too-large",
        }
    }
}

impl fmt::Display for token::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            token::ErrorKind::Unrecognized => "unrecognized character",
            token::ErrorKind::MalformedNatural => "malformed natural number",
            // The bound is worth naming: it is the one limit here that a
            // person can do arithmetic against.
            token::ErrorKind::NaturalTooLarge => "natural number too large to fit in 64 bits",
        })
    }
}

/// Every token has a spelling, and a token prints as the one it was written
/// with. Injective on the fixed kinds, so a printed stream re-lexes to the
/// tokens it was printed from.
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Let => f.write_str("let"),
            Kind::In => f.write_str("in"),
            Kind::Type => f.write_str("type"),
            Kind::End => f.write_str("end"),
            Kind::With => f.write_str("with"),
            Kind::Match => f.write_str("match"),
            Kind::Fn => f.write_str("fn"),
            Kind::Effect => f.write_str("effect"),
            Kind::Handle => f.write_str("handle"),
            Kind::Raise => f.write_str("raise"),
            Kind::Bundle => f.write_str("bundle"),
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
            Kind::Tag(name) => write!(f, "#{name}"),
            Kind::EffectLabel(name) => write!(f, "!{name}"),
            Kind::Variable(name) => write!(f, "'{name}"),
            Kind::LeftBrace => f.write_str("{"),
            Kind::RightBrace => f.write_str("}"),
            Kind::LeftParen => f.write_str("("),
            Kind::RightParen => f.write_str(")"),
            Kind::Identifier(name) => f.write_str(name),
            Kind::Underscore => f.write_str("_"),
            Kind::Natural(value) => write!(f, "{value}n"),
            Kind::Integer(value) => write!(f, "{value}i"),
            Kind::Real(value) => write!(f, "{value}"),
        }
    }
}

impl parse::Error {
    /// A stable, greppable name for this kind of error, the way every other
    /// phase's are coded. One code for the wildcard wherever it landed: what
    /// went wrong is the `_`, and the position only polishes the wording.
    pub fn code(&self) -> &'static str {
        match self.kind {
            parse::ErrorKind::Unexpected => "unexpected-token",
            parse::ErrorKind::Wildcard { .. } => "misplaced-wildcard",
        }
    }
}

/// What the parser could not read, in a phrase. The wildcard's five wordings
/// are one meaning — `_` stands for a value being thrown away, so it can't be
/// *used* — said in the noun of the position it landed in.
impl fmt::Display for parse::Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            parse::ErrorKind::Unexpected => f.write_str("unexpected token"),
            parse::ErrorKind::Wildcard { place } => f.write_str(match place {
                parse::Place::Value => {
                    "`_` stands for a value being thrown away, so it can't be used as a value here"
                }
                parse::Place::Field => {
                    "`_` stands for a value being thrown away, so it can't be a field's name"
                }
                // A pun binds a field to its own name, so what is missing here
                // is the name — the value never gets a say.
                parse::Place::Pun => {
                    "`_` stands for a value being thrown away, and a field written bare binds to its own name, so there is no name here to bind"
                }
                parse::Place::Projection => {
                    "`_` stands for a value being thrown away, so it can't name a field to read"
                }
                parse::Place::Type => {
                    "`_` stands for a value being thrown away, so it can't be used as a type"
                }
            }),
        }
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
            | parse::PatternKind::Unit
            | parse::PatternKind::Struct { .. } => Prec::Atom,
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
            parse::PatternKind::Unit => f.write_str("()"),
            parse::PatternKind::Tag { name, payload } => write_tag(
                f,
                &name.tracked,
                None,
                payload.as_deref().map(|payload| &payload.tracked),
            ),
            parse::PatternKind::Struct { fields, rest } => {
                if fields.is_empty() && rest.is_none() {
                    return f.write_str("{}");
                }
                f.write_str("{ ")?;
                let mut first = true;
                for (name, sub) in fields {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    match sub {
                        Some(sub) => write!(f, "{}: {}", name.tracked, sub.tracked)?,
                        None => f.write_str(&name.tracked)?,
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
            ir::Witness::Natural(_) | ir::Witness::Struct(_) => Prec::Atom,
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
                    match witness {
                        ir::Witness::Any => f.write_str(name)?,
                        witness => write!(f, "{name}: {witness}")?,
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
                    write!(f, "#{case}")?;
                }
                Ok(())
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
            bundle::ErrorKind::MisplacedBundleDeclaration => "misplaced-bundle-declaration",
            bundle::ErrorKind::MissingBundleDeclaration => "missing-bundle-declaration",
            bundle::ErrorKind::BadBundleIdentity => "bad-bundle-identity",
        }
    }
}

/// What loading could not do, in a phrase.
///
/// The two about a module's file name the exact paths that were looked for,
/// because that is the whole of the fix: a reader who is told "no file" still
/// has to work out where one would have gone, and the loader already knows.
impl fmt::Display for bundle::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            bundle::ErrorKind::ModuleFileMissing { beside, inside } => write!(
                f,
                "this module has no file; create `{beside}` or `{inside}`",
            ),
            // Which of the two was meant is not the compiler's to guess, so it
            // says what to delete rather than which one it took.
            bundle::ErrorKind::ModuleFileAmbiguous { beside, inside } => write!(
                f,
                "this module has two files; delete one of `{beside}` or `{inside}`",
            ),
            bundle::ErrorKind::MisplacedBundleDeclaration => {
                f.write_str("remove this bundle declaration; only the root file has one")
            }
            bundle::ErrorKind::MissingBundleDeclaration => {
                f.write_str("add `bundle <name> <version>` at the top of this file")
            }
            // The name is the only thing that can be refused: build metadata is
            // the other reason [`Bundle::new`] says no, and the header grammar
            // has no way to spell one.
            bundle::ErrorKind::BadBundleIdentity => f.write_str(
                "a bundle name must start with a letter and use only letters, digits, `-` and `_`",
            ),
        }
    }
}

impl ir::ErrorKind {
    /// A stable, greppable name for this kind of error. The namespace is part
    /// of the code rather than only of the message: a reporter that wants to
    /// treat an undefined type differently from an undefined term should not
    /// have to re-inspect the variant to tell them apart.
    pub fn code(&self) -> &'static str {
        match self {
            ir::ErrorKind::Undefined { namespace } => match namespace {
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
            ir::ErrorKind::DuplicateField => "duplicate-field",
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
            ir::ErrorKind::VariableInDeclaration => "variable-in-declaration",
            ir::ErrorKind::HoleInDeclaration => "hole-in-declaration",
            // The name is not part of the code, only of the wording: what went
            // wrong is that the clause names something no label wears, and
            // which name it was is the span's to show. The two below say the
            // same about theirs.
            ir::ErrorKind::UnboundPresence { .. } => "unbound-presence",
            ir::ErrorKind::Arity { .. } => "wrong-argument-count",
            ir::ErrorKind::NotAConstructor => "not-a-type-constructor",
            ir::ErrorKind::ParameterApplied => "applied-parameter",
            ir::ErrorKind::DuplicateParameter { .. } => "duplicate-parameter",
            ir::ErrorKind::GrowingRecursion => "growing-recursion",
            ir::ErrorKind::DuplicateCase => "duplicate-case",
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
            ir::ErrorKind::DuplicateOperation => "duplicate-operation",
            ir::ErrorKind::NotAnOperation { .. } => "not-an-operation",
            ir::ErrorKind::ImpureOperation => "impure-operation",
            ir::ErrorKind::MixedEffectForm => "mixed-effect-form",
            ir::ErrorKind::EffectsOutsideRow => "effects-outside-row",
            ir::ErrorKind::OperationOnAlias { .. } => "operation-on-alias",
            ir::ErrorKind::UnknownOperation { .. } => "unknown-operation",
            ir::ErrorKind::PartialHandler { .. } => "partial-handler",
            ir::ErrorKind::DuplicateArm { .. } => "duplicate-arm",
            ir::ErrorKind::DuplicateReturn { .. } => "duplicate-return-arm",
            ir::ErrorKind::RaiseOutsideArm => "raise-outside-arm",
            ir::ErrorKind::RaiseInFunction => "raise-in-function",
        }
    }
}

/// What lowering could not resolve, in a phrase. [`ir::ErrorKind::Duplicate`]
/// says nothing here about the definition it repeats, and
/// [`ir::ErrorKind::MixedTail`] nothing about the first `..` it clashes with:
/// each is a second span in another place, and pointing at one is layout — see
/// [`FIRST_DEFINITION`] and [`FIRST_USE`].
impl fmt::Display for ir::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ir::ErrorKind::Undefined { namespace } => write!(f, "undefined {namespace}"),
            ir::ErrorKind::Duplicate { namespace, .. } => write!(f, "duplicate {namespace}"),
            ir::ErrorKind::DuplicateField => f.write_str("duplicate field"),
            ir::ErrorKind::DuplicateCase => f.write_str("duplicate case"),
            // Said as what the type already does rather than as the mark that
            // repeats it: a `\` rules a label out of the `..` beside it, and a
            // type with no `..` has already ruled out everything it does not
            // name. The label is quoted the way it was written — `#` and all
            // for a case — like every label a complaint quotes.
            ir::ErrorKind::AbsentInClosed { shape, label } => write!(
                f,
                "a type with no `..` already says `{}` is not there",
                self::label(*shape, label),
            ),
            // Not "recursive": a definition is welcome to lead back to itself,
            // and what is wrong here is that there is nothing in the way when
            // it does. Said as what the reader can change — give it a shape —
            // rather than as the loop the compiler noticed. Two wordings for
            // the one rule, because a reader who wrote `let` is being told
            // about a value rather than about a type.
            ir::ErrorKind::Circular { namespace } => match namespace {
                Namespace::Types => f.write_str("type defined only as another name"),
                Namespace::Terms | Namespace::Effects | Namespace::Modules => {
                    f.write_str("this definition is never given a value of its own")
                }
            },
            // The noun follows the shape that was written: someone who wrote
            // `#`s is told about cases, and the `when` and the `..` are the
            // same two marks either way.
            ir::ErrorKind::OpenDeclaredType { shape } => write!(
                f,
                "a declared type must list its {}s exactly; `..` and `when` belong in annotations",
                noun(*shape),
            ),
            // The same refusal about the clause beside the type rather than
            // about a label inside it, so it has no noun to be worded in.
            ir::ErrorKind::ClauseInDeclaration => f.write_str(
                "a declared type says the same thing wherever it is used, so there is nothing here for a `where` clause to decide; it belongs in an annotation",
            ),
            // The same refusal about a variable rather than a comparison, and
            // said as what a declaration's variables already are: writing one
            // in the header is the fix, and naming it is shorter than
            // describing it.
            ir::ErrorKind::VariableInDeclaration => f.write_str(
                "a declared type's variables are its parameters, so this one has to be written in its header; write `type T 'a = ...`",
            ),
            // And the same about a `_`, which leaves open the one thing a
            // declaration has no way to leave open.
            ir::ErrorKind::HoleInDeclaration => f.write_str(
                "a declared type says the same thing wherever it is used, so there is nothing here for `_` to leave open",
            ),
            // Said as what the type would have to do rather than as what the
            // clause failed to find: a formula is about presences, a presence
            // is what a `when` puts on a label, and putting the name on one is
            // the fix whether it was declared or not.
            ir::ErrorKind::UnboundPresence { name } => write!(
                f,
                "this clause names `'{name}`, but no `when` in the type beside it gives it a label",
            ),
            // Counted in words, and said as what the type takes rather than as
            // what the reader failed to supply — the count is the fact, and
            // which side is short of it follows from the two numbers.
            ir::ErrorKind::Arity { expected, found } => write!(
                f,
                "this type takes {}, and {} written",
                arguments(*expected),
                supplied(*found),
            ),
            ir::ErrorKind::NotAConstructor => {
                f.write_str("only a declared type can be given arguments")
            }
            ir::ErrorKind::ParameterApplied => {
                f.write_str("this stands for one type, so there is nothing to give arguments to")
            }
            ir::ErrorKind::DuplicateParameter { .. } => {
                f.write_str("this type already takes something of this name")
            }
            // Said as the rule rather than as the loop: what the reader can
            // change is the arguments at this one mention, and naming the whole
            // cycle would point at declarations they got right.
            ir::ErrorKind::GrowingRecursion => f.write_str(
                "types that lead back to each other may hand on the names they take, but not types built out of them, which get bigger every time round",
            ),
            // Said as the readings rather than as "kind", which names a thing
            // this language does not otherwise have and the reader has never
            // been shown. One sentence for the two, because it is one thing
            // gone wrong: a name that has to stand for one thing was given two.
            // Which name it is, the span already says; where the other reading
            // was is a second place, and pointing at one is layout — see
            // [`FIRST_USE`].
            ir::ErrorKind::MixedTail { first, second, .. } => write!(
                f,
                "this is used as {second} here and as {first} before it, and a name can only stand for one of them",
            ),
            ir::ErrorKind::MixedParameter { first, second } => write!(
                f,
                "this stands for {first} in one place and for {second} in another",
            ),
            ir::ErrorKind::NotARow { sense } => {
                write!(f, "{sense} goes here, and this is not that")
            }
            // Said as what the argument does rather than as what goes here: a
            // struct's `..` takes any type at all, so there is no reading to
            // open with, and what is wrong is that the label would be named
            // twice. The reader can change the field they wrote, and the type
            // the declaration would end up with is not one anybody put on the
            // page.
            ir::ErrorKind::RepeatedRowField { shape, field } => write!(
                f,
                "this names `{}`, which the {} it goes into already has",
                label(*shape, field),
                shape,
            ),
            // Said as what the type does rather than as the loop the compiler
            // noticed, the way [`ir::ErrorKind::Circular`] is: a type that adds
            // fields to itself has more of them every time round, so there is no
            // finite set of fields for it to have.
            ir::ErrorKind::EndlessFields => {
                f.write_str("this type adds fields to itself, so it never has all of them")
            }
            // Said as what a binding has to do — take whatever arrives — with
            // the tag or number that breaks the promise quoted the way it was
            // written. Two sentences for the two, because a case is something
            // a value might not be and a number is something it might not
            // equal.
            ir::ErrorKind::RefutableBinding { found } => match found {
                ir::Refuter::Case(name) => write!(
                    f,
                    "this binding has to accept every value, but a value here might not be `{}`",
                    label(Shape::Sum, name),
                ),
                ir::Refuter::Number(value) => write!(
                    f,
                    "this binding has to accept every value, but the number `{value}` makes it able to fail",
                ),
            },
            ir::ErrorKind::DuplicateBinding { name } => {
                write!(f, "this binds `{name}` twice")
            }
            ir::ErrorKind::DuplicateOperation => f.write_str("duplicate operation"),
            // Said as what to write instead, since the shape of the fix is the
            // whole of what a reader needs: performing an operation is applying
            // it, so an operation has to be something that can be applied.
            ir::ErrorKind::NotAnOperation { name } => write!(
                f,
                "an operation must be a function: write `{name} : Nat -> ()`",
            ),
            ir::ErrorKind::ImpureOperation => f.write_str(
                "an operation's signature must be plain; `+`, `..` and `when` belong in annotations",
            ),
            ir::ErrorKind::MixedEffectForm => f.write_str(
                "an effect either declares operations or names other effects, not both",
            ),
            // Said as where effects do go, since the reader has written
            // something that means one thing and put it where nothing means
            // it: what is missing is an arrow to carry them.
            ir::ErrorKind::EffectsOutsideRow => f.write_str(
                "effects belong on an arrow, or at a parameter a type uses as its own",
            ),
            ir::ErrorKind::OperationOnAlias { effect } => write!(
                f,
                "an alias names effects and declares no operations, so `{}` has none to perform",
                label(Shape::Effect, effect),
            ),
            ir::ErrorKind::UnknownOperation { effect, op } => write!(
                f,
                "no operation `{op}` on effect `{}`",
                label(Shape::Effect, effect),
            ),
            // Named rather than counted: the reader has to write an arm for
            // each of them, and the list is the whole of what they have to do.
            ir::ErrorKind::PartialHandler { effect, missing } => write!(
                f,
                "handling `{}` needs an arm for {} too",
                label(Shape::Effect, effect),
                listed(missing),
            ),
            ir::ErrorKind::DuplicateArm { effect, op } => write!(
                f,
                "duplicate arm for `{}.{op}`",
                label(Shape::Effect, effect),
            ),
            ir::ErrorKind::DuplicateReturn { .. } => f.write_str("duplicate return arm"),
            ir::ErrorKind::RaiseOutsideArm => f.write_str("raise belongs in a handler arm"),
            ir::ErrorKind::RaiseInFunction => f.write_str(
                "raise may not be written inside a function: it answers the handler around it, and a function can outlive one",
            ),
        }
    }
}

/// A list of operations as a sentence reads them: `a`, `a and b`, `a, b and
/// c`. Every one is named, because every one is an arm the reader has to
/// write.
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
/// in a struct puts whatever is written for `'r` in the type's core, and there is
/// nothing narrower to call that.
impl fmt::Display for Sense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Sense::Type => "a whole type",
            Sense::Cases => "the rest of a sum's cases",
            Sense::Effects => "the rest of an arrow's effects",
            // What a `when` puts on a label, said as the reader's own word for
            // it rather than as "a presence variable": they wrote `when a`, and
            // the variable behind it is the compiler's business.
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
/// them, because the type no longer answers the question. Every type has fields
/// *and* may have cases, so a base can be a sum-cored type that is missing a
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
    match shape {
        Shape::Struct => name.to_string(),
        Shape::Sum => format!("#{name}"),
        // Structural effect keys carry an opaque interface after this
        // separator. Source paths never participate in row identity, and the
        // interface is deliberately not user-facing: coalesced effects render
        // as the one bare name they share. Keep the old path spelling for the
        // diagnostic helper's explicitly path-shaped input.
        Shape::Effect => {
            let name = name.split('\u{1f}').next().unwrap_or(name);
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
/// A type that carries no fields groups as its core alone, because that is all
/// it prints as. One that carries fields is either written in braces — which
/// close it, so it is an atom, and which is every core a `..` has a spelling for
/// — or a `with`, whose field list extends rightward and so has to be kept off
/// anything that could be read as continuing it.
///
/// The type language has no lambda, so one of [`Prec`]'s levels never arises
/// here. It is still the right scale to answer on: grouping is decided by
/// comparing against the position a type is being written into, and that
/// comparison is the surface grammar's whether or not this particular tree can
/// reach every level of it.
impl Grouped for Ty {
    fn prec(&self) -> Prec {
        match (self.fields.is_empty(), &self.core) {
            (true, core) => core.prec(),
            // Braces close it, however many fields are inside and whatever the
            // `..` after them says.
            (
                false,
                Core::Unit | Core::Var(_) | Core::Bound(_) | Core::Rigid { .. } | Core::Undecided,
            ) => Prec::Atom,
            (false, _) => Prec::With,
        }
    }
}

/// How much of the surface grammar a core can be. The arrow extends rightward,
/// an application extends rightward by argument, and a sum extends rightward by
/// case; everything else — a primitive, unit, a variable — is a form nothing
/// can be appended to.
impl Grouped for Core {
    fn prec(&self) -> Prec {
        match self {
            Core::Arrow(..) => Prec::Arrow,
            // A sum is written as its cases with nothing around them, so
            // anything that could follow a case has to be kept off it — an
            // argument, a payload, another case.
            Core::Sum(_) => Prec::Sum,
            // Applied to something, a declared type groups as the application
            // it is: `Pair Nat Nat` needs parentheses wherever an argument
            // could follow it.
            Core::Named { args, .. } if !args.is_empty() => Prec::Apply,
            // Applied to nothing it is an atom whatever it stands for: it
            // prints as its name, and a name is one word however many arrows
            // are behind it. Unit is one too: its braces close it.
            Core::Unit
            | Core::Nat
            | Core::Int
            | Core::Real
            | Core::String
            | Core::Boolean
            | Core::Named { .. }
            | Core::Var(_)
            | Core::Bound(_)
            | Core::Rigid { .. }
            | Core::Undecided => Prec::Atom,
        }
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as the bare letter its
/// a variable declares it as, and an unsolved or
/// undecided type as `?` — inference's way of saying it has nothing to report.
///
/// Three forms, and which one a type takes is decided by its two halves rather
/// than by a variant. A type carrying no fields prints as its core alone, which
/// is every type the language had before fields were a property of all of them.
/// A type carrying fields whose core is one a `..` can be written with —
/// [`Core::Unit`], which writes no `..` at all, a variable, a quantified
/// variable, or the undecided type — prints as its fields in braces, which is
/// how a struct has always printed and is what makes `{ x: 'a, ..'b }` come out
/// as something a reader could have written. And anything else carrying fields
/// prints as `<core> with { ... }` — a form inference can build and no source
/// syntax can write, which is why it exists here and nowhere in the parser, and
/// which is reachable only through a declaration whose `..` was handed a known
/// type.
///
/// The `..` spelling is [`tail_of`]'s table read off the core rather than off a
/// tail, and that is the whole of what keeps a printed type re-lowerable to the
/// type it was printed from: `b with { x: a }` is not something the parser
/// could read back, and `{ x: 'a, ..'b }` is.
///
/// The grouping comes from [`write_arrow`] and the braces from [`write_row`]
/// below, both of which the debugger's two tree printers also write through.
/// So the punctuation a type is written with is one rule rather than two
/// copies of a rule agreeing: where a diagnostic puts a parenthesis, a comma
/// or a `when` clause, the debugger's IR tab puts one too.
///
/// Not the whole string, though, and deliberately not. A tail is written by
/// whoever knows what it stands for, and the two readers know different
/// things: the IR tab is showing a type as it was written, so it spells a
/// named tail `..'r`, while a scheme is showing what the definition was
/// inferred to be, so it spells the same tail `..'a`. `tests/src/print.rs`
/// pins both.
impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.fields.is_empty() {
            return self.core.fmt(f);
        }
        match core_tail(&self.core) {
            Some(tail) => write_fields(f, &self.fields, tail.as_ref().map(shown)),
            None => {
                write_grouped(f, self.core.prec() < Prec::With, &self.core)?;
                f.write_str(" with ")?;
                write_fields(f, &self.fields, None)
            }
        }
    }
}

/// What follows a fielded type's `..`, or `None` when the core is not one a
/// `..` can be written with at all and the type has to wear a `with` instead.
///
/// [`tail_of`]'s table moved from a row's tail to a type's core, which is where
/// a struct's `..` now lives. The four it answers about are exactly the four a
/// written `..` could stand for: nothing further at all, which writes no `..`;
/// a solver variable; one a scheme quantified; and the undecided type, which
/// writes the bare `..` a reader would have written.
///
/// `Some(None)` and `None` are two different answers and the difference matters:
/// the first is unit, which closes the braces with no `..` in them, and the
/// second is a `Nat`, an arrow, a sum or a declared name, none of which a `..`
/// has a spelling for.
fn core_tail(core: &Core) -> Option<Option<String>> {
    match core {
        Core::Unit => Some(None),
        Core::Var(var) => Some(Some(format!("?{var}"))),
        Core::Bound(index) => Some(Some(name_at(*index))),
        // A struct's rest declared by a variable prints as the name it was
        // declared with, which is what the reader wrote and what re-lowers to
        // the same rest.
        Core::Rigid { name, .. } => Some(Some(format!("'{name}"))),
        Core::Undecided => Some(Some(String::new())),
        Core::Nat
        | Core::Int
        | Core::Real
        | Core::String
        | Core::Boolean
        | Core::Arrow(..)
        | Core::Sum(_)
        | Core::Named { .. } => None,
    }
}

/// A core prints as the whole type would if it carried no fields, which is what
/// [`Display for Ty`](Ty) writes it as.
impl fmt::Display for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Unit falls out of this as `{}` rather than `()`, on purpose:
            // there is one type here, and one spelling for it. See
            // [`Core::Unit`].
            Core::Unit => write_fields(f, &IndexMap::new(), None),
            Core::Nat => f.write_str(Prim::Nat.name()),
            Core::Int => f.write_str(Prim::Int.name()),
            Core::Real => f.write_str(Prim::Real.name()),
            Core::String => f.write_str(Prim::String.name()),
            Core::Boolean => f.write_str(Prim::Boolean.name()),
            Core::Arrow(from, to, effects) => {
                let row = effects_of(effects);
                write_arrow(f, &**from, &**to, row.as_ref().map(shown))
            }
            Core::Sum(cases) => write_cases(f, cases, Shape::Sum),
            // A declared type prints as what the user called it rather than as
            // what it stands for, applied to whatever it was given. It is
            // shorter, it is what they wrote, and it is the only way a type
            // that names itself can be printed at all.
            Core::Named { name, args, .. } if args.is_empty() => f.write_str(name),
            Core::Named { name, args, .. } => {
                write_applied(f, &**name, args.iter().map(|arg| &**arg))
            }
            // A solver variable has no name, only an index; it is numbered so
            // that two different unknowns in one message stay distinguishable.
            Core::Var(var) => write!(f, "?{var}"),
            Core::Bound(index) => f.write_str(&name_at(*index)),
            // A rigid prints as the name its a variable gave it: the reader
            // wrote it, and it is what tells two of them apart in one message.
            Core::Rigid { name, .. } => write!(f, "'{name}"),
            Core::Undecided => f.write_str("?"),
        }
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
/// shape *is* known the row goes through [`Labels`] instead, which is every row
/// printed as part of a type.
///
/// A row that names nothing prints as its rest alone, so closing one reads
/// `?4 := ∅` rather than as an empty pair of braces standing for the same
/// thing.
impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.labels.is_empty() {
            true => self.rest.fmt(f),
            false => write_braced(f, self),
        }
    }
}

/// A row in the notation of the shape it belongs to, which is the one thing
/// [`Display for Row`](Row) cannot know.
impl fmt::Display for Labels<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.shape {
            Shape::Struct => write_braced(f, self.row),
            // One notation for the two, because an effect row is written the
            // way a sum's cases are — tags, bars and a `..` — minus the payload
            // an effect never carries.
            Shape::Sum | Shape::Effect => write_cases(f, self.row, self.shape),
        }
    }
}

/// A whole [`Row`] in braces: its labels, and its tail read as a struct's would
/// be. What the solver's own record of a row falls back to, and what a spliced
/// tail prints as where the shape says fields.
fn write_braced(f: &mut fmt::Formatter<'_>, row: &Row) -> fmt::Result {
    write_fields(
        f,
        &row.labels,
        tail_of(Shape::Struct, &row.rest).as_ref().map(shown),
    )
}

/// A set of labels and whatever follows their `..`, in braces.
///
/// A field prints by what its presence resolved to: there for certain as
/// `name: T`, absent not at all — a field that is not there is not part of what
/// the type says — and anything still undecided as `name?: T`, the surface
/// spelling for "may or may not be there". `tail` is what follows the `..`, and
/// `None` writes no `..` at all — which is what makes this collapse to the old
/// notation whenever nothing is open.
fn write_fields(
    f: &mut fmt::Formatter<'_>,
    labels: &IndexMap<String, RowField>,
    tail: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let entries = labels
        .iter()
        .filter_map(|(name, field)| match mark(&field.presence) {
            Absence::Absent => None,
            Absence::There(mark) => Some(Entry::Written {
                name,
                mark,
                holds: &field.ty,
            }),
        });
    write_row(f, entries, tail)
}

/// [`write_braced`] about the other shape: the cases of a row and its tail, each
/// wearing the `#` that makes it one.
///
/// Absent not at all, undecided as `#A? T`, and the tail after them. The one
/// thing a sum has that a struct has not is a case carrying unit, which prints
/// as no payload at all — `#None` is how it was written, and `#None {}`
/// is the same type spelled longer.
fn write_cases(f: &mut fmt::Formatter<'_>, row: &Row, shape: Shape) -> fmt::Result {
    let marked = row
        .labels
        .iter()
        .filter_map(|(name, field)| match mark(&field.presence) {
            Absence::Absent => None,
            Absence::There(mark) => Some((name, mark, &field.ty)),
        });
    let tail = tail_of(shape, &row.rest);
    let tail = tail.as_ref().map(shown);
    // The one thing the two readings differ in: an effect carries nothing, so
    // its label writes no payload and the unit it holds is never inspected.
    // See R18.
    match shape {
        Shape::Effect => {
            let effects: Vec<Entry<String, ()>> = marked
                .map(|(name, mark, _)| Entry::Written {
                    name: name.split('\u{1f}').next().unwrap_or(name).to_string(),
                    mark,
                    holds: (),
                })
                .collect();
            let effects: Vec<Entry<&str, ()>> = effects
                .iter()
                .map(|entry| match entry {
                    Entry::Written { name, mark, .. } => Entry::Written {
                        name: name.as_str(),
                        mark: mark.clone(),
                        holds: (),
                    },
                    Entry::Absent { name } => Entry::Absent {
                        name: name.as_str(),
                    },
                })
                .collect();
            write_effects(f, &effects, tail)
        }
        _ => write_sum(
            f,
            marked.map(|(name, mark, ty)| Entry::Written {
                name,
                mark,
                holds: payload(ty),
            }),
            tail,
        ),
    }
}

/// What an arrow's effect row prints as, or `None` for the empty closed row —
/// which prints as nothing at all, so a pure arrow prints bare.
///
/// The one question R24's parenthesization turns on, and the one thing that
/// makes `A -> B` and `A -> B + |` print the same: both mean "performs
/// nothing", and there is one spelling for that.
///
/// A label settled absent counts for nothing here, on the same grounds as
/// everywhere else: an effect that is not performed is no part of what the
/// arrow says. So a row that came to nothing but absences prints as nothing,
/// exactly as one that named nothing does.
fn effects_of(row: &Row) -> Option<String> {
    let named = row.labels.values().any(|field| !absent(field));
    let tail = tail_of(Shape::Effect, &row.rest);
    if !named && tail.is_none() {
        return None;
    }
    let mut out = String::new();
    // Writing to a `String` cannot fail.
    let _ = fmt::write(
        &mut out,
        format_args!(
            "{}",
            Labels {
                shape: Shape::Effect,
                row,
            }
        ),
    );
    Some(out)
}

/// What is known about the cases a row does not name.
///
/// Never part of a printed scheme on its own — a closed tail is written as no
/// `..` at all, and an open one as the `..` [`write_cases`] puts before this.
/// Where one does surface is the solver's own record, where `∅` is how a row
/// says it has nothing more to come.
impl fmt::Display for Rest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rest::Closed => f.write_str("∅"),
            Rest::Var(var) => write!(f, "?{var}"),
            Rest::Bound(index) => f.write_str(&name_at(*index)),
            Rest::Rigid { name, .. } => write!(f, "'{name}"),
            Rest::Undecided => f.write_str("?"),
            // A tail already decided to be more labels prints as those labels,
            // so `..{ y: Nat }` says what the row has come to be without
            // pretending the splice has happened.
            Rest::More(row) => row.fmt(f),
        }
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
            Presence::Undecided => f.write_str("?"),
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

/// Whether a label is part of what its type says, and — when it is — the mark
/// it wears.
///
/// Three presences and two answers: absent is no part of the type at all, and
/// everything else is there with whatever mark says how sure that is.
enum Absence {
    Absent,
    There(Option<Mark>),
}

/// How one presence prints on the label it belongs to.
///
/// A presence certainly there wears nothing, absent is not written, and
/// everything else is a `when` clause naming it — a letter for one a scheme
/// quantified, and the solver's own `?3` for one still open. The single
/// exception is the presence a failure abandoned, which keeps the `?` no syntax
/// reads: what it reports is precisely that there is nothing to write.
fn mark(presence: &Presence) -> Absence {
    match presence {
        Presence::Absent => Absence::Absent,
        Presence::Present => Absence::There(None),
        Presence::Undecided => Absence::There(Some(Mark::Undecided)),
        decided => Absence::There(Some(Mark::When(decided.to_string()))),
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
        let parens = prec(formula) < level;
        if parens {
            f.write_str("(")?;
        }
        match formula {
            // Neither constant has a spelling in the grammar, and neither has
            // to: a formula that says nothing writes no clause at all, and one
            // nothing satisfies is a complaint rather than a type. They print
            // as the words they are, for the debugger and for a message that
            // has to show one.
            Formula::True => f.write_str("always")?,
            Formula::False => f.write_str("never")?,
            Formula::Atom(atom) => f.write_str(&self.spell(*atom))?,
            Formula::Not(inner) => {
                f.write_str("not ")?;
                self.at(f, inner, 3)?;
            }
            // Left-associative, so the right side is written one level tighter
            // and a right-nested `or` inside an `or` keeps its parentheses.
            Formula::And(left, right) => {
                self.at(f, left, 2)?;
                f.write_str(" and ")?;
                self.at(f, right, 3)?;
            }
            Formula::Or(left, right) => {
                self.at(f, left, 1)?;
                f.write_str(" or ")?;
                self.at(f, right, 2)?;
            }
            // Non-associative, so both sides are written one level tighter: a
            // comparison inside a comparison takes parentheses, because the
            // grammar refuses to read one without them.
            Formula::Iff(left, right) => {
                self.at(f, left, 1)?;
                f.write_str(" = ")?;
                self.at(f, right, 1)?;
            }
            Formula::Xor(left, right) => {
                self.at(f, left, 1)?;
                f.write_str(" != ")?;
                self.at(f, right, 1)?;
            }
        }
        if parens {
            f.write_str(")")?;
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
            .map(|(label, _)| label.split('\u{1f}').next().unwrap_or(label).to_string())
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

/// What follows a row's `..`, or `None` when the row allows nothing more and no
/// `..` is written at all: a quantified `a`, a solver variable's `?3`, the
/// labels a tail has been decided to be, or the empty string where the rest is
/// undecided and there is nothing to report.
///
/// A tail decided to be more labels is the one part of this the shape is needed
/// for, and the reason it is passed down: those labels are the row's own,
/// written in the row's own notation, so a sum's spliced tail reads
/// `..#Ok Nat` and not `..{ Ok: Nat }`. Everything else about a tail reads
/// the same either way — it stands for the labels not named, and what it stands
/// for is spelled by what it resolved to.
///
/// A splice that came to nothing is no tail. `..'r` handed a closed row that
/// names nothing leaves a tail saying exactly what [`Rest::Closed`] says, and
/// the answer has to be the same for both: `∅` is the solver's mark for a row
/// with nothing more to come and is never part of a printed type, so
/// `Fallible (#Ok Nat)` prints its cases and no `..` — which is what it is.
/// A label the splice settled absent counts for nothing here, on the same
/// grounds as everywhere else: a label that is not there is not part of what the
/// type says.
///
/// A struct's `..` does not come through here at all any more: its tail is the
/// core beside its fields, and [`core_tail`] is the same table read off that.
fn tail_of(shape: Shape, rest: &Rest) -> Option<String> {
    match rest {
        Rest::Closed => None,
        Rest::More(row) if row.labels.values().all(absent) => tail_of(shape, &row.rest),
        // An abandoned effect row says nothing, so it writes nothing: the `?`
        // on the type beside it already reports the failure, and a `+ ..` would
        // claim the function may still perform something. The other two shapes
        // write the `..` a reader would have written, because there a tail is
        // part of what the type says whatever it came to.
        Rest::Undecided if matches!(shape, Shape::Effect) => None,
        Rest::Undecided => Some(String::new()),
        Rest::More(row) => Some(Labels { shape, row }.to_string()),
        open => Some(open.to_string()),
    }
}

/// Whether one label is settled not to be there, and so is no part of what the
/// row it sits in says.
fn absent(field: &RowField) -> bool {
    matches!(field.presence, Presence::Absent)
}

/// A borrowed tail as the trait object [`write_row`] and [`write_sum`] take.
/// One line, but `map(shown)` reads where the turbofished cast it replaces did
/// not.
fn shown(tail: &String) -> &dyn fmt::Display {
    tail
}

/// What a case carries, or `None` when it carries unit and so is written with
/// no payload at all.
///
/// Unit is what a tag with no payload lowers to — `#None` is `#None ()`
/// — so this is the inverse of that lowering, and it is what makes a printed
/// sum re-lower to the sum it was printed from. A payload that is unit however
/// it was written goes the same way, `{}` and `()` alike: the two spell one
/// type, and `#None {}` is that type written longer. Everything else
/// prints.
fn payload(ty: &Ty) -> Option<&Ty> {
    // A field settled absent is not part of what the type says, so a type that
    // prints as `{}` is unit however many labels the solver left in the map. A
    // unit core writes no `..` after them, which is the whole of what makes this
    // the same question a printed type asks: a case carrying a struct that came
    // to nothing is still a case carrying unit, and still prints as `#A`.
    let empty = matches!(ty.core, Core::Unit) && ty.fields.values().all(absent);
    match empty {
        true => None,
        false => Some(ty),
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
                // The masking refusal: a handler already discharges these
                // effects, so an arm re-performing one would name it twice.
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
            Effect::Bound { var, value } => write!(f, "?{var} := {value}"),
            // Not "smaller goals": an unfolding replaces a goal with the same
            // question asked about a shape, which is a step towards an answer
            // without being any smaller.
            Effect::Decomposed => f.write_str("replaced by the goals below"),
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
            ConstraintKind::Equal { .. } => "equal",
            ConstraintKind::Let { .. } => "let",
            ConstraintKind::Instance { .. } => "instance",
            ConstraintKind::Performs { .. } => "performs",
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
        }
    }
}

/// One effect row on its own, in the notation an arrow writes it in: what a
/// [`ConstraintKind::Performs`] shows, where there is no arrow around the row
/// to hand it a shape. A row that says nothing prints as the `|` an empty one
/// is written with rather than as nothing at all, which would leave the line
/// with a gap in it.
fn effects_shown(row: &Row) -> String {
    Labels {
        shape: Shape::Effect,
        row,
    }
    .to_string()
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

impl inference::ErrorKind {
    /// A stable, greppable name for this kind of error. Reporters key on it
    /// rather than on the message, which is prose and may be reworded.
    pub fn code(&self) -> &'static str {
        match self {
            inference::ErrorKind::Mismatch { .. } => "type-mismatch",
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
        }
    }
}

/// What went wrong, in one sentence.
impl fmt::Display for inference::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            inference::ErrorKind::Mismatch { expected, actual } => {
                write!(f, "type mismatch: expected `{expected}`, found `{actual}`")
            }
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
                name,
                ..
            } => write!(
                f,
                "this decides what it may perform, but `'{name}` stands for whatever effects the caller allows",
            ),
            inference::ErrorKind::RigidBroken { found, name, .. } => write!(
                f,
                "this is `{found}`, but `'{name}` stands for whatever type the caller picks",
            ),
            inference::ErrorKind::RigidField {
                shape, field, name, ..
            } => {
                let (noun, field) = about(*shape, field);
                write!(
                    f,
                    "this reads a {noun} `{field}`, but `'{name}` stands for whatever type the caller picks, so it may not have one",
                )
            }
            // Said at the declaration, because that is the line that has to
            // change: the type the variable leaked into is somewhere the reader
            // never wrote it down.
            inference::ErrorKind::RigidEscapes { name } => write!(
                f,
                "`'{name}` stands for whatever the caller picks, so it can't be part of a type outside the annotation that declared it",
            ),
            // Said as what `..` means rather than as the two rows that
            // disagreed: neither of those is a type the reader wrote, and the
            // field is the whole of what they can change.
            inference::ErrorKind::RepeatedField { shape, field } => write!(
                f,
                "`..` covers only the {}s a type does not already name, and here it would have to cover `{}`",
                noun(*shape),
                label(*shape, field),
            ),
            // Said in the labels the reader wrote rather than in the presence
            // variables the compiler gave them: what has to change is the value
            // on this line, and its fields are the whole of what they can
            // change about it.
            inference::ErrorKind::PresenceRequired { formula } => write!(
                f,
                "this value needs `{formula}` among its fields, and it does not have that",
            ),
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
        }
    }
}

/// Why a batch is in the store, in a phrase — what the Presence tab prints
/// beside each one.
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Origin::Coverage(_) => "what this match's arms cover between them",
            Origin::Instance(_) => "what this use of a name requires of its presences",
            Origin::Annotation(_) => "what this annotation's `where` clause promises",
        })
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
    write_grouped(f, value.prec() < Prec::Unary, value)
}

/// Render a left-associative numeric binary operator.
pub fn write_binary(
    f: &mut fmt::Formatter<'_>,
    left: &impl Grouped,
    op: &str,
    right: &impl Grouped,
    prec: Prec,
) -> fmt::Result {
    write_grouped(f, left.prec() < prec, left)?;
    write!(f, " {op} ")?;
    write_grouped(f, right.prec() <= prec, right)
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

/// Render a `{ name: value, ... }` literal, shared by every position one can
/// appear in: struct expressions and struct types, in either tree, and the
/// semantic type as well. The trees reach the name and the value differently —
/// one off a spanned key, another off the map's key and a field — so the pairs
/// arrive already rendered.
///
/// The wrapper over [`write_row`] for the positions that have no presence and
/// no tail: struct expressions, whose fields are simply there.
pub fn write_struct<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = (K, V)>,
) -> fmt::Result {
    let fields = fields.into_iter().map(|(name, value)| Entry::Written {
        name,
        mark: None,
        holds: value,
    });
    write_row(f, fields, None)
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
            Entry::Written { name, mark, holds } => match mark {
                Some(Mark::Undecided) => write!(f, "{name}?: {holds}")?,
                // Bare between the label and the colon: the colon is what ends
                // the clause, which is why a struct's needs no parentheses.
                Some(Mark::When(name_of)) => write!(f, "{name} when {name_of}: {holds}")?,
                None => write!(f, "{name}: {holds}")?,
            },
            Entry::Absent { name } => write!(f, "\\{name}")?,
        }
    }
    if let Some(tail) = tail {
        if !first {
            f.write_str(", ")?;
        }
        write!(f, "..{tail}")?;
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
            Entry::Absent { name } => write!(f, "\\#{name}")?,
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
    write!(f, "#{}", name.split('\u{1f}').next().unwrap_or(name))?;
    // A case has no colon to end a bare clause, so its `when` takes
    // parentheses — one token of lookahead would otherwise not tell
    // `#A (when a)` from `#A when` carrying a type called `when`.
    match mark {
        Some(Mark::Undecided) => f.write_str("?")?,
        Some(Mark::When(name_of)) => write!(f, " (when {name_of})")?,
        None => {}
    }
    match payload {
        Some(payload) => {
            f.write_str(" ")?;
            write_grouped(f, payload.prec() < Prec::Atom, &payload)
        }
        None => Ok(()),
    }
}

/// Render `let <name> [: <type>] = <value> in <body>` — a name given a value
/// for the length of one expression, in whichever tree it is being read from.
///
/// Nothing is grouped, and nothing here has to be. `in` begins no atom, so an
/// expression written as the value ends in front of the `in` of its own accord,
/// however far right it would otherwise run; and the body is the last thing on
/// the line, so there is nothing after it to be drawn into. A `let` that needs
/// parentheses needs them from whatever it was written inside, which is what
/// [`Prec::Lambda`] says of it.
pub fn write_let(
    f: &mut fmt::Formatter<'_>,
    binder: &impl fmt::Display,
    ty: Option<impl fmt::Display>,
    value: &impl fmt::Display,
    body: &impl fmt::Display,
) -> fmt::Result {
    write!(f, "let {binder}")?;
    if let Some(ty) = ty {
        write!(f, " : {ty}")?;
    }
    write!(f, " = {value} in {body}")
}

/// Render `match <scrutinee> with | <pattern> => <body> ... end` — one
/// writer for both trees, so the punctuation of a match is one rule.
///
/// Nothing here needs grouping. The scrutinee ends at the `with` however far
/// right it runs, each arm's body ends at the next `|` or the `end` — none of
/// the three begins an atom — and the `end` closes the whole form. The
/// leading `|` is written on every arm, first included; the grammar makes it
/// optional there, so the printed form re-parses, and a match with no arms is
/// `match <scrutinee> with end` with no bar at all.
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
/// space, so only the forms that extend rightward need grouping.
pub fn write_project(f: &mut fmt::Formatter<'_>, base: &impl Grouped, field: &str) -> fmt::Result {
    write_grouped(f, base.prec() < Prec::Atom, base)?;
    write!(f, ".{field}")
}

/// Render `body`, wrapping it in parentheses when leaving them off would make
/// the printed source re-parse as a different tree.
fn write_grouped(f: &mut fmt::Formatter<'_>, parens: bool, body: &dyn fmt::Display) -> fmt::Result {
    match parens {
        true => write!(f, "({body})"),
        false => write!(f, "{body}"),
    }
}
