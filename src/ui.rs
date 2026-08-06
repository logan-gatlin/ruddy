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

use crate::{
    inference::{self, Constraint, ConstraintKind, Effect, Rule},
    ir, parse,
    symbol::{Bundle, LOCAL_SEGMENT, Mint, Namespace, Symbol},
    token::{self, Kind},
    types::{Prim, Scheme, Sense, Shape, Ty},
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
    /// `from -> to`.
    Arrow,
    /// `` `A Nat | `B `` — a sum extends rightward by case, so it needs
    /// parentheses anywhere a case could follow it. Above the arrow, which is
    /// what makes `` `A Nat | `B -> Nat `` a function *from* the sum rather
    /// than a sum whose last case carries an arrow.
    Sum,
    /// `` `A `` with nothing after it — a tag takes the next atom as its
    /// payload, so a bare one needs parentheses anywhere one could follow.
    ///
    /// Below [`Prec::Apply`] rather than at it, which is the whole difference
    /// between the two: a tag that already carries something groups as the
    /// application it reads as and may head one, `` `A 1 2 `` being `` `A 1 ``
    /// applied to `2`, while a bare tag heading one would swallow the argument
    /// it was applied to — `` f (`A) 1 `` printed without the parentheses reads
    /// back as `` f (`A 1) ``.
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
            token::ErrorKind::NaturalTooLarge => "natural number too large to fit in 128 bits",
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
            Kind::Fn => f.write_str("fn"),
            Kind::Equal => f.write_str("="),
            Kind::FatArrow => f.write_str("=>"),
            Kind::Arrow => f.write_str("->"),
            Kind::Colon => f.write_str(":"),
            Kind::Comma => f.write_str(","),
            Kind::Dot => f.write_str("."),
            Kind::DotDot => f.write_str(".."),
            Kind::Question => f.write_str("?"),
            Kind::Pipe => f.write_str("|"),
            // The backtick is written back on: it is how the token was
            // spelled, and a `Tag` printing as a bare name would re-lex as an
            // identifier.
            Kind::Tag(name) => write!(f, "`{name}"),
            Kind::LeftBrace => f.write_str("{"),
            Kind::RightBrace => f.write_str("}"),
            Kind::LeftParen => f.write_str("("),
            Kind::RightParen => f.write_str(")"),
            Kind::Identifier(name) => f.write_str(name),
            Kind::Natural(value) => write!(f, "{value}"),
        }
    }
}

impl parse::Error {
    /// The parser raises one kind of error, so this is a constant — but it is
    /// still spelled out here, so that a reporter reaches every phase's errors
    /// the same way and a second kind has somewhere to go.
    pub fn code(&self) -> &'static str {
        "unexpected-token"
    }
}

impl fmt::Display for parse::Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unexpected token")
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
                Namespace::Terms | Namespace::Modules => "undefined-term",
            },
            ir::ErrorKind::Duplicate { namespace, .. } => match namespace {
                Namespace::Types => "duplicate-type",
                Namespace::Terms | Namespace::Modules => "duplicate-term",
            },
            ir::ErrorKind::DuplicateField => "duplicate-field",
            ir::ErrorKind::Circular { namespace } => match namespace {
                Namespace::Types => "circular-type",
                Namespace::Terms | Namespace::Modules => "circular-term",
            },
            ir::ErrorKind::OpenDeclaredType { .. } => "open-declared-type",
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
            // Not "recursive": a definition is welcome to lead back to itself,
            // and what is wrong here is that there is nothing in the way when
            // it does. Said as what the reader can change — give it a shape —
            // rather than as the loop the compiler noticed. Two wordings for
            // the one rule, because a reader who wrote `let` is being told
            // about a value rather than about a type.
            ir::ErrorKind::Circular { namespace } => match namespace {
                Namespace::Types => f.write_str("type defined only as another name"),
                Namespace::Terms | Namespace::Modules => {
                    f.write_str("this definition is never given a value of its own")
                }
            },
            // The noun follows the shape that was written: someone who wrote
            // backticks is told about cases, and the `?` and the `..` are the
            // same two marks either way.
            ir::ErrorKind::OpenDeclaredType { shape } => write!(
                f,
                "a declared type must list its {}s exactly; `..` and `?` belong in annotations",
                noun(Some(*shape)),
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
            // Said as the two readings rather than as "kind", which names a
            // thing this language does not otherwise have and the reader has
            // never been shown.
            // One sentence for the two, because it is one thing gone wrong:
            // a name that has to be one rest was given two. Which name it is,
            // the span already says.
            ir::ErrorKind::MixedTail { first, second, .. } => write!(
                f,
                "this stands for {} in one place and for {} in another",
                Sense::Row(*first),
                Sense::Row(*second),
            ),
            ir::ErrorKind::MixedParameter { first, second } => write!(
                f,
                "this stands for {first} in one place and for {second} in another",
            ),
            ir::ErrorKind::NotARow { shape } => {
                write!(f, "{} goes here, and this is not that", Sense::Row(*shape))
            }
            // Said as what the `..` covers, the way the solver's version of
            // this complaint is: the reader can change the field they wrote,
            // and the row the declaration would end up with is not a type
            // anyone put on the page.
            ir::ErrorKind::RepeatedRowField { shape, field } => write!(
                f,
                "{} goes here, and this names `{}`, which that {} already has",
                Sense::Row(*shape),
                label(*shape, field),
                shape,
            ),
        }
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
        })
    }
}

/// What a parameter stands for, as the phrase a complaint drops into a
/// sentence. Read as "this stands for …", which is what
/// [`ir::ErrorKind::MixedParameter`] says twice and
/// [`ir::ErrorKind::NotARow`] once.
impl fmt::Display for Sense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Sense::Type => "a whole type",
            Sense::Row(Shape::Struct) => "the rest of a struct's fields",
            Sense::Row(Shape::Sum) => "the rest of a sum's cases",
        })
    }
}

/// What one label of a row of this shape is called, for the complaints that
/// read a shape off a type rather than being handed one. A type that is
/// somehow not a row gets `field`, which is what every complaint of this kind
/// said before there were sums.
fn noun(shape: Option<Shape>) -> &'static str {
    match shape {
        Some(Shape::Sum) => "case",
        Some(Shape::Struct) | None => "field",
    }
}

/// What kind of row a type is, or `None` when it is not one.
fn shape_of(ty: &Ty) -> Option<Shape> {
    match ty {
        Ty::Row { shape, .. } => Some(*shape),
        _ => None,
    }
}

/// The two words a complaint about one label of `base` needs: what to call it,
/// and how it is written there.
///
/// Both come off the type rather than out of a second payload: `base` *is* the
/// row that went wrong, so reading it here is the one way the word and the type
/// beside it cannot disagree. A base that is somehow not a row is described the
/// way a struct is, which is what every complaint of this kind said before
/// there were sums.
fn about(base: &Ty, name: &str) -> (&'static str, String) {
    let shape = shape_of(base);
    (noun(shape), label(shape.unwrap_or(Shape::Struct), name))
}

/// One label as it is written in a row of this shape: a field is its bare
/// name, a case wears the backtick that makes it one.
///
/// Every complaint that quotes a label goes through here rather than
/// interpolating the string it was given, so that a message about `` `Some ``
/// never asks the reader to look for `Some`.
pub fn label(shape: Shape, name: &str) -> String {
    match shape {
        Shape::Struct => name.to_string(),
        Shape::Sum => format!("`{name}"),
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::Terms => f.write_str("term"),
            Namespace::Types => f.write_str("type"),
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

/// How much of the surface grammar a semantic type can be. The arrow extends
/// rightward, an application extends rightward by argument, and a sum extends
/// rightward by case; everything else — a primitive, a braced struct, a
/// variable — is a form nothing can be appended to.
///
/// The type language has no lambda, so one of [`Prec`]'s levels never arises
/// here. It is still the right scale to answer on: grouping is decided by
/// comparing against the position a type is being written into, and that
/// comparison is the surface grammar's whether or not this particular tree can
/// reach every level of it.
impl Grouped for Ty {
    fn prec(&self) -> Prec {
        match self {
            Ty::Arrow(..) => Prec::Arrow,
            // A sum is written as its cases with nothing around them, so
            // anything that could follow a case has to be kept off it — an
            // argument, a payload, another case.
            Ty::Row {
                shape: Shape::Sum, ..
            } => Prec::Sum,
            // Applied to something, a declared type groups as the application
            // it is: `Pair Nat Nat` needs parentheses wherever an argument
            // could follow it.
            Ty::Named { args, .. } if !args.is_empty() => Prec::Apply,
            // Applied to nothing it is an atom whatever it stands for: it
            // prints as its name, and a name is one word however many arrows
            // are behind it. A struct is one too: its braces close it.
            Ty::Nat
            | Ty::Row { .. }
            | Ty::Named { .. }
            | Ty::Var(_)
            | Ty::Bound(_)
            | Ty::Undecided
            | Ty::Present
            | Ty::Absent
            | Ty::Empty => Prec::Atom,
        }
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as `'a`, and an unsolved or
/// undecided type as `?` — inference's way of saying it has nothing to report.
///
/// The grouping comes from [`write_arrow`] and the braces from [`write_row`]
/// below, both of which the debugger's two tree printers also write through.
/// So the punctuation a type is written with is one rule rather than two
/// copies of a rule agreeing: where a diagnostic puts a parenthesis, a comma
/// or a `?`, the debugger's IR tab puts one too.
///
/// Not the whole string, though, and deliberately not. A tail is written by
/// whoever knows what it stands for, and the two readers know different
/// things: the IR tab is showing a type as it was written, so it spells a
/// named tail `..r`, while a scheme is showing what the definition was
/// inferred to be, so it spells the same tail `..'a`. `tests/src/print.rs`
/// pins both.
impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Nat => f.write_str(Prim::Nat.name()),
            Ty::Arrow(from, to) => write_arrow(f, &**from, &**to),
            // Unit falls out of this as `{}` rather than `()`, on purpose:
            // there is one type here, and one spelling for it. See
            // [`Ty::Row`].
            //
            // A field prints by what its presence resolved to: there for
            // certain as `name: T`, absent not at all — a field that is not
            // there is not part of what the type says — and anything still
            // undecided as `name?: T`, the surface spelling for "may or may
            // not be there". A tail prints after the fields as `..` and
            // whatever stands for the rest: a quantified `..'a`, a solver
            // variable's `..?3`, or nothing when the rest is undecided and
            // there is nothing to report. A closed struct prints no tail,
            // which is what makes this collapse to the old notation whenever
            // nothing is open.
            Ty::Row {
                shape: Shape::Struct,
                fields,
                rest,
            } => {
                let entries = fields
                    .iter()
                    .filter_map(|(name, field)| match &*field.presence {
                        Ty::Absent => None,
                        Ty::Present => Some((name, false, &field.ty)),
                        _ => Some((name, true, &field.ty)),
                    });
                write_row(f, entries, tail_of(rest).as_ref().map(shown))
            }
            // A sum reads the same way with cases in place of fields: absent
            // not at all, undecided as `` `A? T ``, and the tail after them.
            // The one thing it has that a struct has not is a case carrying
            // unit, which prints as no payload at all — `` `None `` is how it
            // was written, and `` `None {} `` is the same type spelled longer.
            Ty::Row {
                shape: Shape::Sum,
                fields,
                rest,
            } => {
                let cases = fields
                    .iter()
                    .filter_map(|(name, field)| match &*field.presence {
                        Ty::Absent => None,
                        Ty::Present => Some((name, false, payload(&field.ty))),
                        _ => Some((name, true, payload(&field.ty))),
                    });
                write_sum(f, cases, tail_of(rest).as_ref().map(shown))
            }
            // A declared type prints as what the user called it rather than as
            // what it stands for, applied to whatever it was given. It is
            // shorter, it is what they wrote, and it is the only way a type
            // that names itself can be printed at all.
            Ty::Named { name, args, .. } if args.is_empty() => f.write_str(name),
            Ty::Named { name, args, .. } => {
                write_applied(f, &**name, args.iter().map(|arg| &**arg))
            }
            // A solver variable has no name, only an index; it is numbered so
            // that two different unknowns in one message stay distinguishable.
            Ty::Var(var) => write!(f, "?{var}"),
            Ty::Bound(index) => {
                let letter = (b'a' + (index % 26) as u8) as char;
                match index / 26 {
                    0 => write!(f, "'{letter}"),
                    round => write!(f, "'{letter}{round}"),
                }
            }
            Ty::Undecided => f.write_str("?"),
            // The presence and row constants never stand alone in a printed
            // scheme — a field wears its presence as `?` or as nothing, and a
            // closed tail prints as no tail at all. Where one does surface is
            // the solver's own record: a step that binds a presence variable
            // reads `?3 := absent`, and one that closes a row reads `?4 := ∅`.
            Ty::Present => f.write_str("present"),
            Ty::Absent => f.write_str("absent"),
            Ty::Empty => f.write_str("∅"),
        }
    }
}

/// What follows a row's `..`, or `None` when the row is closed and no `..` is
/// written at all: a quantified `'a`, a solver variable's `?3`, or the empty
/// string where the rest is undecided and there is nothing to report.
///
/// Shared by the two shapes because the tail is the one part of a row that
/// reads the same either way — it stands for the labels not named, and what it
/// stands for is spelled by what it resolved to rather than by what kind of
/// row it ends.
fn tail_of(rest: &Ty) -> Option<String> {
    match rest {
        Ty::Empty => None,
        Ty::Undecided => Some(String::new()),
        open => Some(open.to_string()),
    }
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
/// Unit is what a tag with no payload lowers to — `` `None `` is `` `None () ``
/// — so this is the inverse of that lowering, and it is what makes a printed
/// sum re-lower to the sum it was printed from. A payload that is unit however
/// it was written goes the same way, `{}` and `()` alike: the two spell one
/// type, and `` `None {} `` is that type written longer. Everything else
/// prints.
fn payload(ty: &Ty) -> Option<&Ty> {
    let Ty::Row {
        shape: Shape::Struct,
        fields,
        rest,
    } = ty
    else {
        return Some(ty);
    };
    // A field settled absent is not part of what the type says, so a row that
    // prints as `{}` is unit however many labels the solver left in the map.
    let empty = matches!(&**rest, Ty::Empty)
        && fields
            .values()
            .all(|field| matches!(&*field.presence, Ty::Absent));
    match empty {
        true => None,
        false => Some(ty),
    }
}

/// A scheme prints as its body: the quantifier is implied by the `'a`s that
/// appear, the way ML type printers have always done it.
impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.body().fmt(f)
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
            // In the reader's nouns, like everything else said about a row: the
            // rest of a struct is a struct, and what it may not name is a
            // field. Saying it about a "row" and a "label" would name the
            // representation the two shapes share, which is the compiler's
            // business and nothing the reader wrote.
            Rule::Overlap { shape } => write!(
                f,
                "the rest of a {shape} cannot be a {shape} naming a {} the {shape} already names",
                noun(Some(*shape)),
            ),
            Rule::Prim => f.write_str("the same primitive on both sides"),
            Rule::Arrow => {
                f.write_str("two arrows: argument against argument, result against result")
            }
            Rule::Struct => f.write_str(
                "two structs: shared fields field against field, the rest into the other's tail",
            ),
            Rule::Sum => f.write_str(
                "two sums: shared cases case against case, the rest into the other's tail",
            ),
            Rule::Presence { shape } => write!(
                f,
                "whether the {} is there must agree on both sides",
                noun(Some(*shape)),
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
            Effect::Bound { var, ty } => write!(f, "?{var} := {ty}"),
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
            // kind of row is reading the type rather than the complaint.
            inference::ErrorKind::MissingField { .. } => "missing-field",
            inference::ErrorKind::ExtraField { .. } => "extra-field",
            inference::ErrorKind::NotAStruct { .. } => "not-a-struct",
            inference::ErrorKind::AnnotationTooOpen => "annotation-too-open",
            inference::ErrorKind::RepeatedField { .. } => "repeated-field",
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
            inference::ErrorKind::MissingField { base, field } => {
                let (noun, field) = about(base, field);
                write!(f, "no {noun} `{field}` on `{base}`")
            }
            inference::ErrorKind::ExtraField { base, field } => {
                let (noun, field) = about(base, field);
                write!(
                    f,
                    "extra {noun} `{field}`: the type `{base}` lists every {noun} it allows",
                )
            }
            // Only the type that is not a struct: what was asked of it is a
            // shape the solver made up, and quoting that back would answer a
            // question the reader did not ask.
            inference::ErrorKind::NotAStruct { base } => {
                write!(f, "`{base}` is not a struct, so it has no fields to read")
            }
            // Said as what the reader can change — the type they wrote — and
            // not as the variable the solve bound, which is a thing the
            // annotation stood for rather than anything on the page.
            inference::ErrorKind::AnnotationTooOpen => f.write_str(
                "this type promises a `..` or a `?` that the definition does not leave open: write the type it actually has"
            ),
            // Said as what `..` means rather than as the two rows that
            // disagreed: neither of those is a type the reader wrote, and the
            // field is the whole of what they can change.
            inference::ErrorKind::RepeatedField { shape, field } => write!(
                f,
                "`..` covers only the {}s a type does not already name, and here it would have to cover `{}`",
                noun(Some(*shape)),
                label(*shape, field),
            ),
        }
    }
}

/// Render `func arg`. A lambda on the left would swallow the argument into its
/// own body, and anything that keeps consuming to its right would swallow
/// whatever follows the argument.
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

/// Render `from -> to`. The arrow is right-associative, so only the left side
/// can ever need grouping — an arrow there would otherwise re-parse as the outer
/// arrow's right half.
///
/// Grouped against the arrow rather than against an atom, so only what could
/// swallow the arrow is bracketed: an arrow, and a lambda. An application is
/// left alone, because it stops at the arrow of its own accord and
/// `Pair A B -> Nat` is how a person would write it — and so is a sum, whose
/// last case carries an atom and so cannot reach the arrow either.
pub fn write_arrow(
    f: &mut fmt::Formatter<'_>,
    from: &impl Grouped,
    to: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, from.prec() < Prec::Sum, from)?;
    write!(f, " -> {to}")
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
    let fields = fields.into_iter().map(|(name, value)| (name, false, value));
    write_row(f, fields, None)
}

/// Render a `{ name: value, name?: value, ..tail }` row: fields, each possibly
/// marked optional, and then whatever is known about the fields not named.
///
/// `tail` is what follows the `..` — a row variable's spelling, or nothing —
/// and `None` means the row is closed and no `..` is written at all. The `..`
/// itself is written here, so the callers agree on it by construction.
pub fn write_row<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = (K, bool, V)>,
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
    for (name, optional, value) in fields {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        let mark = if optional { "?" } else { "" };
        write!(f, "{name}{mark}: {value}")?;
    }
    if let Some(tail) = tail {
        if !first {
            f.write_str(", ")?;
        }
        write!(f, "..{tail}")?;
    }
    f.write_str(" }")
}

/// Render a `` `A Nat | `B? | ..tail `` sum: cases, each wearing a backtick and
/// possibly a `?`, each with a payload or without one, and then whatever is
/// known about the cases not named.
///
/// The counterpart of [`write_row`], and the same contract: `tail` is what
/// follows the `..`, `None` means the sum names every case there is, and the
/// `|`s are written here so the callers agree on them by construction.
///
/// The leading `|` the grammar allows is not written — `` `A | `B `` reads
/// better inline, and inside the parentheses a nested sum needs it would be
/// noise. It comes back for the one form that cannot do without it: a sum with
/// no cases written out is `|`, and `| ..r` for one that is only a tail, since
/// a bare `..r` begins no type the parser would read back.
pub fn write_sum<K: fmt::Display, V: Grouped>(
    f: &mut fmt::Formatter<'_>,
    cases: impl IntoIterator<Item = (K, bool, Option<V>)>,
    tail: Option<&dyn fmt::Display>,
) -> fmt::Result {
    let mut first = true;
    for (name, optional, payload) in cases {
        if !first {
            f.write_str(" | ")?;
        }
        first = false;
        write_tag(f, &name.to_string(), optional, payload)?;
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

/// Render one case of a sum — `` `Name ``, the `?` it may wear, and what it
/// carries — grouped so that a payload nothing can be appended to is left
/// bare and anything else is bracketed.
///
/// One case and one tag literal are the same thing written in two places, so
/// they are written by one function: `` `Some 1 `` in a term and `` `Some Nat ``
/// in a type differ in what follows the name and in nothing else.
///
/// A payload of `None` writes nothing at all, which is how a case that carries
/// unit is spelled. Whether a given type *is* unit is the caller's to decide —
/// the term printers know it because nothing was written, and the type printer
/// works it out — so this is handed the answer rather than reaching for it.
pub fn write_tag<V: Grouped>(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    optional: bool,
    payload: Option<V>,
) -> fmt::Result {
    let mark = if optional { "?" } else { "" };
    write!(f, "`{name}{mark}")?;
    match payload {
        Some(payload) => {
            f.write_str(" ")?;
            write_grouped(f, payload.prec() < Prec::Atom, &payload)
        }
        None => Ok(()),
    }
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
