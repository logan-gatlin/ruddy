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
    types::{Prim, Scheme, Ty},
};

/// The note a duplicate definition points back with, printed against the span
/// of the definition that stands. Held apart from [`ir::ErrorKind`]'s own
/// wording because it is a second line about a second place, and only a
/// reporter knows how to attach one.
pub const FIRST_DEFINITION: &str = "first defined here";

/// A node a printer has to parenthesize by precedence. Implemented by every
/// wrapper that prints as surface syntax, and by [`Ty`], which prints as one
/// directly.
pub trait Grouped: fmt::Display {
    fn prec(&self) -> Prec;
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
            ir::ErrorKind::Circular => "circular-type",
        }
    }
}

/// What lowering could not resolve, in a phrase. [`ir::ErrorKind::Duplicate`]
/// says nothing here about the definition it repeats: that is a second span in
/// another place, and pointing at it is layout — see [`FIRST_DEFINITION`].
impl fmt::Display for ir::ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ir::ErrorKind::Undefined { namespace } => write!(f, "undefined {namespace}"),
            ir::ErrorKind::Duplicate { namespace, .. } => write!(f, "duplicate {namespace}"),
            ir::ErrorKind::DuplicateField => f.write_str("duplicate field"),
            // Not "recursive": a type is welcome to lead back to itself, and
            // what is wrong here is that there is nothing in the way when it
            // does. Said as what the reader can change — give the type a shape
            // — rather than as the loop the compiler noticed.
            ir::ErrorKind::Circular => f.write_str("type defined only as another name"),
        }
    }
}

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

/// How much of the surface grammar a semantic type can be. Only the arrow
/// extends rightward; everything else — a primitive, a braced struct, a
/// variable — is a form nothing can be appended to.
///
/// The type language has no lambda and no application, so two of [`Prec`]'s
/// four levels never arise here. They are still the right scale to answer on:
/// grouping is decided by comparing against the position a type is being
/// written into, and that comparison is the surface grammar's whether or not
/// this particular tree can reach every level of it.
impl Grouped for Ty {
    fn prec(&self) -> Prec {
        match self {
            Ty::Arrow(..) => Prec::Arrow,
            // A declared type is an atom whatever it stands for: it prints as
            // its name, and a name is one word however many arrows are behind
            // it.
            Ty::Nat
            | Ty::Struct(_)
            | Ty::Named { .. }
            | Ty::Var(_)
            | Ty::Bound(_)
            | Ty::Undecided => Prec::Atom,
        }
    }
}

/// Types print in the surface type grammar, so a printed type reads the same
/// as one the user could have written. The two forms with no surface spelling
/// print as what they mean: a quantified variable as `'a`, and an unsolved or
/// undecided type as `?` — inference's way of saying it has nothing to report.
///
/// The grouping and the braces come from [`write_arrow`] and [`write_struct`]
/// below, which the debugger's two tree printers also write through. A type in
/// a diagnostic and the same type on the debugger's IR tab are then the same
/// string by construction rather than by two copies of the rule agreeing.
impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Nat => f.write_str(Prim::Nat.name()),
            Ty::Arrow(from, to) => write_arrow(f, &**from, &**to),
            // Unit falls out of this as `{}` rather than `()`, on purpose:
            // there is one type here, and one spelling for it. See
            // [`Ty::Struct`].
            Ty::Struct(fields) => write_struct(f, fields),
            // A declared type prints as what the user called it rather than as
            // what it stands for. It is shorter, it is what they wrote, and it
            // is the only way a type that names itself can be printed at all.
            Ty::Named { name, .. } => f.write_str(name),
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
        }
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
            Rule::Bind => "bind",
            Rule::Occurs => "occurs",
            Rule::Prim => "prim",
            Rule::Arrow => "arrow",
            Rule::Struct => "struct",
            Rule::Unfold => "unfold",
            Rule::Assume => "assume",
            Rule::Mismatch => "mismatch",
            Rule::Project => "project",
            Rule::Defer => "defer",
            Rule::Stuck => "stuck",
            Rule::Recover => "recover",
        }
    }
}

/// What a rule does, in a phrase. Said here rather than by whoever is showing
/// the solve, so a reader stepping through it and a reader reading the code are
/// told the same thing.
impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Rule::Absorb => "one side is undecided, which unifies with anything",
            Rule::Same => "already the same thing on both sides",
            Rule::Bind => "a variable takes the type it is against",
            Rule::Occurs => "the variable is inside the type it is against, so no finite type fits",
            Rule::Prim => "the same primitive on both sides",
            Rule::Arrow => "two arrows: argument against argument, result against result",
            Rule::Struct => "two structs: field against field, matched by name",
            Rule::Unfold => "a declared type stands for something; ask again about that",
            Rule::Assume => "these two are already being compared, so take them as equal",
            Rule::Mismatch => "no rule applies, so the two types cannot be made equal",
            Rule::Project => "what is before the dot is a struct, so the field can be read off it",
            Rule::Defer => "what is before the dot is still unknown; wait for another round",
            Rule::Stuck => "nothing left will say what is before the dot",
            Rule::Recover => "the abandoned result becomes undecided, so nothing echoes it",
        })
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
            ConstraintKind::Field { .. } => "field",
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
            ConstraintKind::Field {
                base, name, result, ..
            } => write!(f, "{base}.{name} ~ {result}"),
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
            inference::ErrorKind::NotAStruct { .. } => "not-a-struct",
            inference::ErrorKind::UnknownBase => "unknown-base",
            inference::ErrorKind::MissingField { .. } => "missing-field",
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
            inference::ErrorKind::NotAStruct { base } => {
                write!(f, "`{base}` is not a struct, so it has no fields to read")
            }
            inference::ErrorKind::UnknownBase => f.write_str(
                "cannot tell what this is, so its fields are unknown; write out its type",
            ),
            inference::ErrorKind::MissingField { base, field } => {
                write!(f, "no field `{field}` on `{base}`")
            }
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

/// Render `from -> to`. The arrow is right-associative, so only the left side
/// can ever need grouping — an arrow there would otherwise re-parse as the outer
/// arrow's right half.
pub fn write_arrow(
    f: &mut fmt::Formatter<'_>,
    from: &impl Grouped,
    to: &impl Grouped,
) -> fmt::Result {
    write_grouped(f, from.prec() < Prec::Atom, from)?;
    write!(f, " -> {to}")
}

/// Render a `{ name: value, ... }` literal, shared by every position one can
/// appear in: struct expressions and struct types, in either tree, and the
/// semantic type as well. The trees reach the name and the value differently —
/// one off a spanned key, another off the map's key and a field — so the pairs
/// arrive already rendered.
pub fn write_struct<K: fmt::Display, V: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    fields: impl IntoIterator<Item = (K, V)>,
) -> fmt::Result {
    let mut fields = fields.into_iter().peekable();
    // The empty struct is unit, which reads as `{}` — the padding a struct with
    // fields gets would only be two spaces around nothing.
    if fields.peek().is_none() {
        return f.write_str("{}");
    }

    f.write_str("{ ")?;
    for (i, (name, value)) in fields.enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{name}: {value}")?;
    }
    f.write_str(" }")
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
