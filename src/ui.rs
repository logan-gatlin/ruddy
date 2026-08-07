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
    inference::{self, Constraint, ConstraintKind, Effect, Goal, Rule},
    ir, parse,
    symbol::{Bundle, LOCAL_SEGMENT, Mint, Namespace, Symbol},
    token::{self, Kind},
    types::{Assigned, Core, Presence, Prim, Rest, Row, RowField, Scheme, Sense, Shape, Ty},
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

/// A row read as one of the two shapes: labels in braces, or cases with their
/// backticks.
///
/// What a spliced tail prints as, and the one place a row's notation is chosen
/// by something other than the position the row sits in. A tail is written by
/// the row it ends — see [`tail_of`] — so the shape travels down from the type
/// whose row it is, and `` `Err Nat | ..`Ok Nat `` never comes out spelled with
/// a colon. See [`Display for Labels`](Labels) for the two writers it picks
/// between, and [`Display for Row`](Row) for what a row with no shape to hand
/// down falls back to.
struct Labels<'a> {
    shape: Shape,
    row: &'a Row,
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
            ir::ErrorKind::NotARow => "not-a-row",
            ir::ErrorKind::RepeatedRowField { .. } => "repeated-row-field",
            ir::ErrorKind::EndlessFields => "endless-fields",
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
                noun(*shape),
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
                "this stands for {first} in one place and for {second} in another",
            ),
            ir::ErrorKind::MixedParameter { first, second } => write!(
                f,
                "this stands for {first} in one place and for {second} in another",
            ),
            ir::ErrorKind::NotARow => {
                write!(f, "{} goes here, and this is not that", Sense::Cases)
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

/// What a parameter, or a named tail, stands for, as the phrase a complaint
/// drops into a sentence. Read as "this stands for …", which is what
/// [`ir::ErrorKind::MixedParameter`] and [`ir::ErrorKind::MixedTail`] each say
/// twice and [`ir::ErrorKind::NotARow`] once.
///
/// The rest of a struct is a whole type, so it has no phrase of its own: `..r`
/// in a struct puts whatever is written for `r` in the type's core, and there is
/// nothing narrower to call that.
impl fmt::Display for Sense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Sense::Type => "a whole type",
            Sense::Cases => "the rest of a sum's cases",
        })
    }
}

/// What one label of a row of this shape is called.
fn noun(shape: Shape) -> &'static str {
    match shape {
        Shape::Sum => "case",
        Shape::Struct => "field",
    }
}

/// The two words a complaint about one label of a type needs: what to call it,
/// and how it is written there.
///
/// Both come off the shape the solver carried rather than off the type beside
/// them, because the type no longer answers the question. Every type has fields
/// *and* may have cases, so a base can be a sum-cored type that is missing a
/// `field` — `` (`A 1).x `` is exactly that — and reading the word off the base
/// would call it a case. The solver knows which row it was deciding at the
/// moment it failed, and that is the only place the answer is not a guess.
fn about(shape: Shape, name: &str) -> (&'static str, String) {
    (noun(shape), label(shape, name))
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
            (false, Core::Unit | Core::Var(_) | Core::Bound(_) | Core::Undecided) => Prec::Atom,
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
            | Core::Named { .. }
            | Core::Var(_)
            | Core::Bound(_)
            | Core::Undecided => Prec::Atom,
        }
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as `'a`, and an unsolved or
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
/// type it was printed from: `'b with { x: 'a }` is not something the parser
/// could read back, and `{ x: 'a, ..'b }` is.
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
        Core::Bound(index) => Some(Some(Core::Bound(*index).to_string())),
        Core::Undecided => Some(Some(String::new())),
        Core::Nat | Core::Arrow(..) | Core::Sum(_) | Core::Named { .. } => None,
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
            Core::Arrow(from, to) => write_arrow(f, &**from, &**to),
            Core::Sum(cases) => write_cases(f, cases),
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
            Core::Bound(index) => letter(f, *index),
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
            Shape::Sum => write_cases(f, self.row),
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
        .filter_map(|(name, field)| match &field.presence {
            Presence::Absent => None,
            Presence::Present => Some((name, false, &field.ty)),
            _ => Some((name, true, &field.ty)),
        });
    write_row(f, entries, tail)
}

/// [`write_braced`] about the other shape: the cases of a row and its tail, each
/// wearing the backtick that makes it one.
///
/// Absent not at all, undecided as `` `A? T ``, and the tail after them. The one
/// thing a sum has that a struct has not is a case carrying unit, which prints
/// as no payload at all — `` `None `` is how it was written, and `` `None {} ``
/// is the same type spelled longer.
fn write_cases(f: &mut fmt::Formatter<'_>, row: &Row) -> fmt::Result {
    let cases = row
        .labels
        .iter()
        .filter_map(|(name, field)| match &field.presence {
            Presence::Absent => None,
            Presence::Present => Some((name, false, payload(&field.ty))),
            _ => Some((name, true, payload(&field.ty))),
        });
    write_sum(f, cases, tail_of(Shape::Sum, &row.rest).as_ref().map(shown))
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
            Rest::Bound(index) => letter(f, *index),
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
/// Never part of a printed scheme either: a field wears its presence as the `?`
/// on its name, or as nothing. Where one surfaces is the solver's record, where
/// a step deciding a presence reads `?3 := absent`.
impl fmt::Display for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Presence::Present => f.write_str("present"),
            Presence::Absent => f.write_str("absent"),
            Presence::Var(var) => write!(f, "?{var}"),
            Presence::Bound(index) => letter(f, *index),
            Presence::Undecided => f.write_str("?"),
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
/// once the letters run out. One spelling for all three sorts, since they share
/// one numbering — a presence usually prints as the `?` on its label instead,
/// but the letter is what it falls back to.
fn letter(f: &mut fmt::Formatter<'_>, index: u32) -> fmt::Result {
    let letter = (b'a' + (index % 26) as u8) as char;
    match index / 26 {
        0 => write!(f, "'{letter}"),
        round => write!(f, "'{letter}{round}"),
    }
}

/// What follows a row's `..`, or `None` when the row allows nothing more and no
/// `..` is written at all: a quantified `'a`, a solver variable's `?3`, the
/// labels a tail has been decided to be, or the empty string where the rest is
/// undecided and there is nothing to report.
///
/// A tail decided to be more labels is the one part of this the shape is needed
/// for, and the reason it is passed down: those labels are the row's own,
/// written in the row's own notation, so a sum's spliced tail reads
/// `` ..`Ok Nat `` and not `..{ Ok: Nat }`. Everything else about a tail reads
/// the same either way — it stands for the labels not named, and what it stands
/// for is spelled by what it resolved to.
///
/// A splice that came to nothing is no tail. `..r` handed a closed row that
/// names nothing leaves a tail saying exactly what [`Rest::Closed`] says, and
/// the answer has to be the same for both: `∅` is the solver's mark for a row
/// with nothing more to come and is never part of a printed type, so
/// `` Fallible (`Ok Nat) `` prints its cases and no `..` — which is what it is.
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
/// Unit is what a tag with no payload lowers to — `` `None `` is `` `None () ``
/// — so this is the inverse of that lowering, and it is what makes a printed
/// sum re-lower to the sum it was printed from. A payload that is unit however
/// it was written goes the same way, `{}` and `()` alike: the two spell one
/// type, and `` `None {} `` is that type written longer. Everything else
/// prints.
fn payload(ty: &Ty) -> Option<&Ty> {
    // A field settled absent is not part of what the type says, so a type that
    // prints as `{}` is unit however many labels the solver left in the map. A
    // unit core writes no `..` after them, which is the whole of what makes this
    // the same question a printed type asks: a case carrying a struct that came
    // to nothing is still a case carrying unit, and still prints as `` `A ``.
    let empty = matches!(ty.core, Core::Unit) && ty.fields.values().all(absent);
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
            // In the reader's nouns, like everything else said about a set of
            // labels: someone who wrote braces is told about a struct and a
            // field, and someone who wrote backticks about a sum and a case.
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
            },
            Rule::Prim => f.write_str("the same primitive on both sides"),
            Rule::Arrow => {
                f.write_str("two arrows: argument against argument, result against result")
            }
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
            ConstraintKind::Let { bound, level, .. } => {
                write!(f, "{bound} generalized at level {level}")
            }
            // The name is not spelled: a symbol needs the mint to name it, and
            // nothing here has one. The type is what a reader is following
            // through the tab anyway, and the row it is a child of says which
            // binding it is a copy of.
            ConstraintKind::Instance { ty, .. } => {
                write!(f, "{ty} ~ a fresh copy of what this name was bound to")
            }
        }
    }
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
                noun(*shape),
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
    name: &str,
    ty: Option<impl fmt::Display>,
    value: &impl fmt::Display,
    body: &impl fmt::Display,
) -> fmt::Result {
    write!(f, "let {name}")?;
    if let Some(ty) = ty {
        write!(f, " : {ty}")?;
    }
    write!(f, " = {value} in {body}")
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
