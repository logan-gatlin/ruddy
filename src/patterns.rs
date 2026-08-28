//! The pattern checks, run once inference has answered: exhaustiveness,
//! reachability, and the misplaced catch-all, per match.
//!
//! The phase runs after [`inference::infer`](crate::inference::infer) and
//! reads the solved types the column rule shaped, so the checks are *typed*: a
//! field the scrutinee's type proves present needs no absent case, one it
//! proves absent makes arms requiring it unreachable, and one whose presence
//! is still a variable — or one a scheme quantified — must have both its cases
//! covered, independently per field. The catch-all placement rule alone stays
//! syntactic: the dedicated wording is reserved for arms that are irrefutable
//! on their face, and an exact arm the type happens to make total is reported
//! as the unreachable arm it leaves behind instead.
//!
//! For a match whose column inference converted to a formula, both questions
//! are asked of the constraint store instead. An arm is reachable exactly when
//! some assignment the store allows satisfies what that arm covers and none of
//! what the arms above it cover; and every value the store allows is one some
//! arm accepts, so such a match is exhaustive unless conjoining its own
//! coverage is what left the store without a model — which is the match being
//! wrong, and is reported here with a witness read off a model. Those columns
//! test nothing but which labels are there, so the two ways of asking agree
//! wherever both can be asked; a column that tests a number or a case is not
//! one of them and keeps the walk below. The walk is not left reasoning as if
//! presences were independent, though: a field whose presence is still open
//! only has the halves the definition's own clause allows it, given the halves
//! the columns above it were entered on, so `a != b` rules out the value with
//! both fields wherever the question is asked.
//!
//! One rule of ownership per match, in order: a misplaced catch-all owns its
//! own arm — the arms it starves are not additionally flagged — then an arm no
//! value of the solved type can reach is reported at itself, and then a value
//! no arm accepts is reported at the match, with a concrete example. A match
//! whose scrutinee, or any tested position, failed to solve is skipped whole:
//! the type error already speaks, and a pattern complaint about a typing that
//! never settled would be the same mistake said twice.
//!
//! Beside the errors the phase publishes a [`Report`] per match — the solved
//! scrutinee type, a verdict per arm, and the coverage — which is what the
//! debugger's Patterns tab renders.

use std::{collections::HashMap, rc::Rc};

use indexmap::IndexMap;

use crate::{
    inference::{self, Coverage as Covers, Origin, Store, effective_conditions, sat, unfold},
    ir::{Literal, Pattern, PatternKind, Program, Term, TermKind, Witness},
    symbol::Symbol,
    tracking::Span,
    types::{Atom, Formula, Presence, Rest, Row, Scheme, Ty},
};

/// What the phase found, over the whole program: one report per match, in the
/// order the matches appear, and every complaint the checks made.
#[derive(Debug, Clone)]
pub struct Output {
    pub reports: Vec<Report>,
    pub errors: Vec<Error>,
}

/// One match, as the debugger shows it: where it is, what the scrutinee
/// solved to, a verdict per written arm, and whether anything goes unhandled.
#[derive(Debug, Clone)]
pub struct Report {
    /// The whole match's span.
    pub span: Span,
    /// The solved scrutinee type, zonked — what the typed checks read.
    pub scrutinee: Rc<Ty>,
    pub scrutinee_span: Span,
    pub arms: Vec<Arm>,
    pub coverage: Coverage,
}

/// One arm of a reported match: the pattern as lowered, where the arm was
/// written, and what the checks made of it.
#[derive(Debug, Clone)]
pub struct Arm {
    /// The arm's span — pattern and body together, which is where its
    /// complaints point.
    pub span: Span,
    pub pattern: Pattern,
    pub verdict: Verdict,
}

/// What the checks made of one arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Some value of the solved scrutinee type reaches it.
    Reachable,
    /// No value of the solved type gets past the arms above it.
    Unreachable,
    /// It sits after an arm that accepts everything. The one error is at that
    /// arm; a starved arm is not additionally flagged, so the tab agrees with
    /// the single reported complaint.
    Starved,
    /// The checks stood aside: the scrutinee, or a tested position, failed to
    /// solve, and the type error already speaks.
    Skipped,
}

/// Whether a match's arms cover every value of the solved scrutinee type.
#[derive(Debug, Clone)]
pub enum Coverage {
    Exhaustive,
    /// A value no arm accepts, as a concrete example.
    Unhandled(Witness),
    /// The checks stood aside; see [`Verdict::Skipped`].
    Skipped,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    /// A syntactically irrefutable arm — a bare name, a wildcard, or an open
    /// `{..}` naming no fields — written before the last arm. The arms after
    /// it can never be reached, so the mistake is the placement, and it is
    /// reported at the arm that accepts everything rather than at each arm it
    /// starves.
    MisplacedCatchAll,
    /// An arm no value of the *solved* scrutinee type can reach past the arms
    /// above it — a second bare `#A` after `#A x`, a duplicate
    /// literal, or a `{}` under a type that proves a field present.
    UnreachableArm,
    /// A match that leaves values unhandled, with a concrete example of one
    /// no arm accepts, so the reader is shown what to add an arm for rather
    /// than told an analysis failed.
    UnhandledValues { witness: Witness },
    /// [`ErrorKind::UnhandledValues`] with the friendlier wording numbers
    /// deserve: the naturals cannot be listed in full, so a match testing
    /// them has to end in an arm that takes the rest, and no one number is
    /// worth quoting.
    UnhandledNumbers,
}

/// One cell of the typed matrix: a pattern with everything that only binds
/// flattened to the wildcard it matches as, and `()` read as the exact struct
/// naming no fields it is. Owned, because the walk builds rows no pattern
/// wrote — specializing a row appends its payload or its fields.
#[derive(Debug, Clone)]
enum Cell {
    /// Accepts everything: a binder or a wildcard.
    Wild,
    /// An exact scalar literal. Its variant carries its primitive type as
    /// well as its value, and `Literal` gives reals representation equality.
    Literal(Literal),
    /// A case, with what it carries — a bare tag's payload is the unit it
    /// demands, which is [`Cell::Struct`] with no fields, exact.
    Tag { name: String, payload: Box<Cell> },
    /// A struct pattern. Exact without the `..`: every field the type names
    /// beyond the mentioned ones must be absent. Open with it: the mentioned
    /// fields must be there, and the rest is anybody's.
    Struct {
        fields: Vec<(String, Cell)>,
        exact: bool,
    },
    /// Past widening only: the field this presence column stands for must be
    /// there, matching the carried cell.
    Present(Box<Cell>),
    /// Past widening only: the field must not be there — or, in the rest
    /// column, no field beyond the ones the type names may be, which is what
    /// an exact pattern demands of the rest.
    Absent,
}

/// One column of the typed matrix: a whole position with its solved type, or —
/// past widening — one field's presence, or the position's rest: whatever
/// fields the value has beyond the ones the type names.
#[derive(Clone)]
enum Col {
    Whole(Rc<Ty>),
    Field {
        presence: Presence,
        ty: Rc<Ty>,
    },
    /// The fields beyond the named ones. `open` says whether the solved row
    /// admits any — a row tail still free or quantified does, the fieldless unit
    /// an exact column closed to does not.
    Rest {
        open: bool,
    },
}

/// Which question a usefulness walk is answering. The two differ only at the
/// rest column: reachability credits an arm with the values only its `..`
/// accepts — fields the type never named — while exhaustiveness does not
/// demand those values covered, or the open arm `{x, ..}` backed by `{}`
/// would be reported unhandled for values no pattern could name.
#[derive(Clone, Copy)]
enum Mode {
    Reachability,
    Exhaustiveness,
}

/// The primitive types accepted by [`Check::scalars`]. Keeping this narrower
/// than the explicit type variants makes the scalar witness match exhaustive without an
/// untestable fallback for types its caller can never pass.
#[derive(Clone, Copy)]
enum Scalar {
    Infinite(InfiniteScalar),
    Boolean,
}

#[derive(Clone, Copy)]
enum InfiniteScalar {
    Nat,
    Int,
    Real,
    String,
}

/// What a usefulness walk carries down its recursion: which question it is
/// answering, and what the path to this point has assumed about the presences
/// it passed through.
///
/// The assumptions are the other half of R11. A presence column's two halves
/// are not independent of the ones above it — the constraints may say `a != b`,
/// and then the value that has both is no value at all — so entering a half
/// records its literal, and every column below is asked *given* that. Every
/// walk starts assuming nothing.
#[derive(Clone)]
struct Walk {
    mode: Mode,
    assumed: Formula,
}

/// The walk's context: the aliases, for looking through a declared name to
/// the shape the checks need, and the solver's complaints, for the one part
/// of the cascade rule the solved types cannot carry — a scrutinee the solver
/// already complained about is a scrutinee whose type is not to be reasoned
/// from, whatever it says.
struct Check<'a> {
    aliases: &'a IndexMap<Symbol, Scheme>,
    errors: &'a [inference::Error],
    /// What the program requires of its presences, as the finished solve reads
    /// it. The whole of R11: for a column whose coverage was converted to a
    /// formula, reachability and exhaustiveness are questions about this rather
    /// than about the matrix — and, that column being nothing but structs and
    /// binders, everything it could be asked *is* presence.
    store: &'a Store,
    /// Which batch, if any, left the store without a model. Everything after it
    /// is suppressed: with no model at all, every arm reads unreachable and
    /// every match reads unhandled, which is one mistake said in as many places
    /// as the program has matches.
    flipped: Option<usize>,
    definition_at: usize,
    flipped_definition_at: Option<usize>,
    /// What the definition being walked may assume about its presences: the
    /// store's word about every presence its terms can name, nested bindings
    /// included. See [`inference::Output::promises`].
    ///
    /// The store itself is in the wrong alphabet to ask. Its batches are
    /// written about solver variables, and closing the definition numbered the
    /// types the walk reads — a presence still open in one of them is the
    /// *position* it was closed to, not the variable it was. The promise is
    /// that same content, renamed by the substitution that closed those types,
    /// so it is the reading of the store a column can be asked about. It keeps
    /// the cascade rule with it: a definition generalized once something had
    /// flipped the store promises nothing at all.
    promise: Formula,
}

/// What the store has to say about one match: which batch is its coverage, and
/// the arms and labels that batch carried.
struct Constrained<'a> {
    at: usize,
    coverage: &'a Covers,
    premise: Formula,
}

fn constrained_origin(origin: &Origin) -> Option<(Formula, &Covers)> {
    match origin {
        Origin::Coverage(coverage) => Some((Formula::True, coverage)),
        Origin::Guarded(guarded) => {
            let (inside, coverage) = constrained_origin(&guarded.origin)?;
            Some((guarded.premise.clone().and(inside), coverage))
        }
        Origin::Instance(_) | Origin::Annotation(_) | Origin::Refinement(_) => None,
    }
}

/// Run the checks over every match in the program. `inferred` is read for its
/// aliases — the solved types themselves were written into the terms.
pub fn check(program: &Program, inferred: &inference::Output) -> Output {
    let flipped = inferred
        .store
        .batches
        .iter()
        .position(|batch| batch.flipped);
    let mut out = Output {
        reports: Vec::new(),
        errors: Vec::new(),
    };
    let definitions: HashMap<Symbol, usize> = program
        .terms
        .keys()
        .enumerate()
        .map(|(at, symbol)| (*symbol, at))
        .collect();
    let flipped_definition_at = flipped.and_then(|at| {
        inferred.store.batches[at]
            .definition
            .and_then(|symbol| definitions.get(&symbol).copied())
    });
    // One definition at a time, because the presences its types name are its
    // own: the promise inference published for it is what the store came to
    // about them, and it is the only reading of the store the walk inside it
    // can use.
    for (definition_at, (symbol, decl)) in program.terms.iter().enumerate() {
        let check = Check {
            aliases: &inferred.aliases,
            errors: &inferred.errors,
            store: &inferred.store,
            flipped,
            definition_at,
            flipped_definition_at,
            promise: inferred
                .promises
                .get(symbol)
                .cloned()
                .unwrap_or(Formula::True),
        };
        walk(&check, &decl.value, &mut out);
    }
    // In the order the reader would meet them, whatever order the walk found
    // them in; the sort is stable, so two complaints about one span keep the
    // order the checks made them.
    out.errors.sort_by_key(|error| error.span.start);
    out
}

/// Every match in one term, outermost first, each checked where it is found.
fn walk(check: &Check, term: &Term, out: &mut Output) {
    walk_under(check, term, &Formula::True, out);
}

fn walk_under(check: &Check, term: &Term, assumed: &Formula, out: &mut Output) {
    match &term.kind {
        TermKind::Match { scrutinee, arms } => {
            check.matched(term.span, scrutinee, arms, assumed, out);
            walk_under(check, scrutinee, assumed, out);
            let guards = check.arm_assumptions(term.span, scrutinee);
            for (at, (_, body)) in arms.iter().enumerate() {
                let inside = guards.as_ref().map_or_else(
                    || assumed.clone(),
                    |guards| assumed.clone().and(guards[at].clone()),
                );
                walk_under(check, body, &inside, out);
            }
        }
        TermKind::Unary { value, .. } => walk_under(check, value, assumed, out),
        TermKind::Binary { left, right, .. } => {
            walk_under(check, left, assumed, out);
            walk_under(check, right, assumed, out);
        }
        TermKind::Apply { func, arg } => {
            walk_under(check, func, assumed, out);
            walk_under(check, arg, assumed, out);
        }
        TermKind::Fn { body, .. } => walk_under(check, body, assumed, out),
        TermKind::Let { value, body, .. } => {
            walk_under(check, value, assumed, out);
            walk_under(check, body, assumed, out);
        }
        TermKind::Struct(fields) => {
            for field in fields.values() {
                walk_under(check, &field.value, assumed, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                walk_under(check, payload, assumed, out);
            }
        }
        TermKind::Project { base, .. } => walk_under(check, base, assumed, out),
        // A handler's arms are not value patterns and nothing here changes for
        // them: what they hold is ordinary terms, and a match written inside
        // one is checked exactly as a match written anywhere else is.
        TermKind::Handle { body, handler } => {
            walk_under(check, body, assumed, out);
            for arm in &handler.arms {
                walk_under(check, &arm.body, assumed, out);
            }
            if let Some(ret) = &handler.ret {
                walk_under(check, &ret.body, assumed, out);
            }
        }
        TermKind::Raise(value) => walk_under(check, value, assumed, out),
        TermKind::Operation { .. }
        | TermKind::Ident(_)
        | TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// Whether an arm is irrefutable on its face: a bare name, a wildcard, or the
/// open `{..}` naming no fields. Nothing typed enters into it — an exact
/// pattern the solved type happens to make total is not one, and neither is
/// an open pattern naming a field, which a value could lack — so the
/// misplaced-catch-all wording lands only where the arm accepts everything
/// however the scrutinee solves.
fn catch_all(pattern: &Pattern) -> bool {
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Wildcard => true,
        PatternKind::Struct {
            fields,
            rest: Some(_),
        } => fields.is_empty(),
        _ => false,
    }
}

/// A pattern as the typed matrix matches it.
fn cell(pattern: &Pattern) -> Cell {
    match &pattern.tracked {
        PatternKind::Bind(_) | PatternKind::Wildcard => Cell::Wild,
        PatternKind::Natural(value) => Cell::Literal(Literal::Natural(*value)),
        PatternKind::Integer(value) => Cell::Literal(Literal::Integer(*value)),
        PatternKind::Real(value) => Cell::Literal(Literal::Real(*value)),
        PatternKind::String(value) => Cell::Literal(Literal::String(value.clone())),
        PatternKind::Boolean(value) => Cell::Literal(Literal::Boolean(*value)),
        // `()` and `{}` are one pattern: the exact struct naming no fields.
        PatternKind::Unit => Cell::Struct {
            fields: Vec::new(),
            exact: true,
        },
        PatternKind::Struct { fields, rest } => Cell::Struct {
            fields: fields
                .iter()
                .map(|(name, field)| (name.clone(), cell(&field.value)))
                .collect(),
            exact: rest.is_none(),
        },
        // A bare tag's payload demands unit; the types were told so, and the
        // cell says the same thing.
        PatternKind::Tag { name, payload } => Cell::Tag {
            name: name.tracked.clone(),
            payload: Box::new(payload.as_deref().map(cell).unwrap_or(Cell::Struct {
                fields: Vec::new(),
                exact: true,
            })),
        },
    }
}

/// A sum's cases flattened: a tail already decided to be more cases folded
/// into the labels, the outer copy of a label winning — the same read
/// inference's own flattening makes. Zonking already did this for everything
/// it published, so the loop is a defence rather than a workhorse.
fn flat(row: &Row) -> Row {
    let mut labels = row.labels.clone();
    let mut rest = row.rest.clone();
    while let Rest::More(more) = rest {
        for (name, field) in &more.labels {
            labels.entry(name.clone()).or_insert_with(|| field.clone());
        }
        rest = more.rest.clone();
    }
    Row { labels, rest }
}

/// Whether a label may be there: everything but a settled absence. A presence
/// still quantified or undecided may go either way, which is exactly why both
/// of its cases need covering.
fn may_be_present(presence: &Presence) -> bool {
    !matches!(presence, Presence::Absent)
}

/// Whether a label may be missing: everything but a settled presence.
fn may_be_absent(presence: &Presence) -> bool {
    !matches!(presence, Presence::Present)
}

/// A case's witness, with a payload any value serves for folded away — the
/// bare spelling, which is how a case carrying nothing is written.
fn tag_witness(name: &str, payload: Witness) -> Witness {
    let payload = match payload {
        Witness::Any => None,
        payload => Some(Box::new(payload)),
    };
    Witness::Tag {
        name: name.to_string(),
        payload,
    }
}

/// The unit-cell every position that carries nothing testable answers with.
fn wild_row(row: &[Cell]) -> Vec<Cell> {
    row[1..].to_vec()
}

impl Walk {
    /// A walk about to begin: the question it answers, and nothing assumed yet.
    fn start(mode: Mode, assumed: Formula) -> Self {
        Walk { mode, assumed }
    }
}

impl Check<'_> {
    /// A type as the checks read it: the shape behind a declared name. The
    /// types are zonked, so this needs no table — only the aliases.
    fn shape(&self, ty: &Rc<Ty>) -> Rc<Ty> {
        unfold(self.aliases, ty)
    }

    /// Check one match and push its report — and whatever the checks have to
    /// say — onto the output.
    fn matched(
        &self,
        span: Span,
        scrutinee: &Term,
        arms: &[(Pattern, Term)],
        assumed: &Formula,
        out: &mut Output,
    ) {
        let ty = scrutinee.ty.clone();
        // The empty match stays silent: it constrained the scrutinee to the
        // empty sum, which has no values to leave unhandled — unless the
        // constraint failed, in which case the type error already speaks.
        if arms.is_empty() {
            let shaped = self.shape(&ty);
            let empty = match &*shaped {
                Ty::Sum(row) => {
                    let row = flat(row);
                    matches!(row.rest, Rest::Closed)
                        && row
                            .labels
                            .values()
                            .all(|case| !may_be_present(&case.presence))
                }
                _ => false,
            };
            out.reports.push(Report {
                span,
                scrutinee: ty,
                scrutinee_span: scrutinee.span,
                arms: Vec::new(),
                coverage: match empty {
                    true => Coverage::Exhaustive,
                    false => Coverage::Skipped,
                },
            });
            return;
        }

        let spans: Vec<Span> = arms
            .iter()
            .map(|(pattern, body)| pattern.span.merge(body.span))
            .collect();
        let cells: Vec<Cell> = arms.iter().map(|(pattern, _)| cell(pattern)).collect();

        // The cascade rule: a match whose scrutinee — or any position the
        // arms test — failed to solve is skipped whole. The mixed match is
        // the loudest instance: its one complaint is the solver's mismatch,
        // and a pattern complaint on top would be the same mistake twice.
        // Incompatibility is read off the solved types; the solver's own
        // complaint at the scrutinee covers the failures that still left a
        // shape behind.
        let failed = self.errors.iter().any(|error| error.span == scrutinee.span);
        if cells.iter().any(|cell| !self.compatible(&ty, cell)) || failed {
            out.reports.push(Report {
                span,
                scrutinee: ty,
                scrutinee_span: scrutinee.span,
                arms: arms
                    .iter()
                    .zip(&spans)
                    .map(|((pattern, _), at)| Arm {
                        span: *at,
                        pattern: pattern.clone(),
                        verdict: Verdict::Skipped,
                    })
                    .collect(),
                coverage: Coverage::Skipped,
            });
            return;
        }

        // A column whose coverage inference converted to a formula is decided
        // by the store; anything else keeps the matrix walk it always had.
        let constrained = self.constrained(span);
        // The cascade rule applies to both paths. A converted match has a store
        // index to compare directly; for a legacy literal/tag column, source
        // position identifies one following the flipping batch in the same
        // file.
        let cascaded = self.flipped.is_some_and(|first| match &constrained {
            Some(found) => first < found.at,
            None => match self.flipped_definition_at {
                Some(flipped) if flipped < self.definition_at => true,
                Some(flipped) if flipped > self.definition_at => false,
                _ => {
                    let flipped = &self.store.batches[first];
                    matches!(
                        (
                            flipped.span.file_id == span.file_id,
                            flipped.span.start < span.start,
                        ),
                        (true, true)
                    )
                }
            },
        });
        if cascaded {
            out.reports.push(Report {
                span,
                scrutinee: ty,
                scrutinee_span: scrutinee.span,
                arms: arms
                    .iter()
                    .zip(&spans)
                    .map(|((pattern, _), at)| Arm {
                        span: *at,
                        pattern: pattern.clone(),
                        verdict: Verdict::Skipped,
                    })
                    .collect(),
                coverage: Coverage::Skipped,
            });
            return;
        }

        // The misplaced catch-all, first in the order of ownership once the
        // cascade check has allowed this match to speak. One error at the first
        // arm that accepts everything and is not last; the arms it starves are
        // not additionally flagged.
        let misplaced = arms[..arms.len() - 1]
            .iter()
            .position(|(pattern, _)| catch_all(pattern));
        if let Some(at) = misplaced {
            out.errors.push(Error {
                span: spans[at],
                kind: ErrorKind::MisplacedCatchAll,
            });
        }

        // A verdict per arm, and the unreachable complaints. Reachability is
        // typed: the arm's own usefulness against the arms above it, over the
        // universes the solved scrutinee type draws — or, where the store
        // decides the column, whether any assignment the store allows reaches
        // the arm past the ones above it.
        let cols = [Col::Whole(ty.clone())];
        let mut reported = Vec::with_capacity(arms.len());
        for (at, (pattern, _)) in arms.iter().enumerate() {
            let verdict = match misplaced {
                // The catch-all owns its own complaint, and everything after
                // it is starved.
                Some(here) if at == here => Verdict::Reachable,
                Some(here) if at > here => Verdict::Starved,
                _ => {
                    let reachable = match &constrained {
                        // The flipping batch's own match is the one being
                        // reported unhandled below; with the store already
                        // without a model, every arm of it would read dead,
                        // and the arms are not what went wrong.
                        Some(found) if self.flipped == Some(found.at) => true,
                        Some(found) => self.reaches(found, at),
                        None => {
                            let rows: Vec<Vec<Cell>> =
                                cells[..at].iter().map(|cell| vec![cell.clone()]).collect();
                            self.useful(
                                &rows,
                                &cols,
                                &[cells[at].clone()],
                                &Walk::start(Mode::Reachability, assumed.clone()),
                            )
                            .is_some()
                        }
                    };
                    match reachable {
                        true => Verdict::Reachable,
                        false => {
                            out.errors.push(Error {
                                span: spans[at],
                                kind: ErrorKind::UnreachableArm,
                            });
                            Verdict::Unreachable
                        }
                    }
                }
            };
            reported.push(Arm {
                span: spans[at],
                pattern: pattern.clone(),
                verdict,
            });
        }

        // Exhaustiveness, last: a value of the solved type no arm accepts,
        // with the example written out — worded about numbers when the
        // example is one, since no one number is worth quoting.
        let coverage = match &constrained {
            // A converted column's arms are in the store as what they cover, so
            // every value the store allows is one some arm accepts — unless
            // conjoining that very batch is what left the store without a
            // model, which is the coverage failing and is reported here, at the
            // match, with a witness read off a model of what the store allowed
            // before it and the arms do not.
            Some(found) => match self.flipped == Some(found.at) {
                false => Coverage::Exhaustive,
                true => {
                    let witness = self.witness(found);
                    out.errors.push(Error {
                        span,
                        kind: ErrorKind::UnhandledValues {
                            witness: witness.clone(),
                        },
                    });
                    Coverage::Unhandled(witness)
                }
            },
            None => {
                let rows: Vec<Vec<Cell>> = cells.iter().map(|cell| vec![cell.clone()]).collect();
                match self.useful(
                    &rows,
                    &cols,
                    &[Cell::Wild],
                    &Walk::start(Mode::Exhaustiveness, assumed.clone()),
                ) {
                    Some(mut wits) => {
                        let witness = wits.remove(0).unwrap_or(Witness::Any);
                        let kind = match &witness {
                            Witness::Natural(_) => ErrorKind::UnhandledNumbers,
                            _ => ErrorKind::UnhandledValues {
                                witness: witness.clone(),
                            },
                        };
                        out.errors.push(Error { span, kind });
                        Coverage::Unhandled(witness)
                    }
                    None => Coverage::Exhaustive,
                }
            }
        };

        out.reports.push(Report {
            span,
            scrutinee: ty,
            scrutinee_span: scrutinee.span,
            arms: reported,
            coverage,
        });
    }

    /// The local ordered arm assumptions translated from inference's solver
    /// variables into the bound presences the zonked term types use. These are
    /// carried into nested non-qualifying matches, whose literal/tag matrix
    /// still needs to know which outer presence branch it sits in.
    fn arm_assumptions(&self, span: Span, scrutinee: &Term) -> Option<Vec<Formula>> {
        let found = self.constrained(span)?;
        let raw: Vec<Formula> = found
            .coverage
            .arms
            .iter()
            .map(|arm| {
                arm.rename(&|var| {
                    found
                        .coverage
                        .paths
                        .iter()
                        .find_map(|(path, presence)| match presence {
                            Presence::Var(found) if *found == var => self
                                .presence_at(&scrutinee.ty, path)
                                .map(|presence| presence.formula()),
                            _ => None,
                        })
                        // An unnamed nested atom stays independent rather than
                        // being defaulted true or false in the bound alphabet.
                        .unwrap_or_else(|| Formula::var(var))
                })
            })
            .collect();
        Some(effective_conditions(&raw))
    }

    fn presence_at(&self, ty: &Rc<Ty>, path: &str) -> Option<Presence> {
        match path.split_once('.') {
            Some((name, below)) => {
                let shaped = self.shape(ty);
                let Ty::Struct(row) = &*shaped else {
                    return None;
                };
                let row = flat(row);
                let field = row.labels.get(name)?;
                self.presence_at(&field.ty, below)
            }
            None => match &*self.shape(ty) {
                Ty::Struct(row) => flat(row)
                    .labels
                    .get(path)
                    .map(|field| field.presence.clone()),
                _ => None,
            },
        }
    }

    /// The store's coverage batch for the match at `span`, when inference
    /// converted that match's column.
    ///
    /// Matched by span, which is the batch's own: one match writes one coverage
    /// batch, and the span is where the batch says it came from.
    fn constrained(&self, span: Span) -> Option<Constrained<'_>> {
        self.store
            .batches
            .iter()
            .enumerate()
            .find_map(|(at, batch)| {
                (batch.span == span)
                    .then(|| constrained_origin(&batch.origin))
                    .flatten()
                    .map(|(premise, coverage)| Constrained {
                        at,
                        coverage,
                        premise,
                    })
            })
    }

    /// Whether any value the store allows reaches arm `at` — that is, satisfies
    /// what that arm covers and none of what the arms above it cover.
    ///
    /// Maranget's usefulness, said propositionally. The two agree wherever both
    /// can be asked; where only this one can, it is because the column tests
    /// nothing but which labels are there, and that is what the store is about.
    fn reaches(&self, found: &Constrained<'_>, at: usize) -> bool {
        let effective = effective_conditions(&found.coverage.arms);
        sat::satisfiable(
            &self
                .known()
                .and(found.premise.clone())
                .and(effective[at].clone()),
        )
    }

    /// The walk with one more presence literal assumed, or `None` when what the
    /// definition promises leaves no room for it: a half no assignment can
    /// reach is not walked, exactly as a half the solved presence rules out is
    /// not.
    ///
    /// The other half of R11, and the half the matrix cannot answer for itself:
    /// two of a column's presences may be two the store related to each other,
    /// and then whether this half exists at all is a question for the
    /// constraints rather than for the patterns. A column carrying no literal —
    /// a presence settled either way, or one a failure abandoned — is walked
    /// unconditionally, which is what it always was.
    fn assuming(&self, walk: &Walk, literal: Option<Formula>) -> Option<Walk> {
        let Some(literal) = literal else {
            return Some(walk.clone());
        };
        let assumed = walk.assumed.clone().and(literal);
        let allowed = sat::satisfiable(&self.promise.clone().and(assumed.clone()));
        allowed.then_some(Walk {
            mode: walk.mode,
            assumed,
        })
    }

    /// Everything the store says while it still says anything: the batches up
    /// to the one that flipped it, or all of them when none did.
    ///
    /// The cascade rule, said where the queries are made. A store with no model
    /// entails everything, so asking it about an arm would answer "unreachable"
    /// for every arm of every match in the program — one contradiction reported
    /// as many times as the file has arms. The last consistent state is the one
    /// that has anything to say.
    fn known(&self) -> Formula {
        let upto = self.flipped.unwrap_or(self.store.batches.len());
        Formula::all(
            self.store.batches[..upto]
                .iter()
                .map(|batch| batch.formula.clone()),
        )
    }

    /// A value the arms of a flipped coverage batch leave unhandled, read off a
    /// model of what the store allowed before the batch and the arms do not.
    ///
    /// Such a model always exists: the store had one until this batch was
    /// conjoined, and it has none after, so something the store allows is
    /// outside what the arms cover. The value is written as the labels the
    /// model puts there — a field present with any value at all, which prints
    /// pun-style, because under this reading the presence *is* the information.
    fn witness(&self, found: &Constrained) -> Witness {
        let before = Formula::all(
            self.store.batches[..found.at]
                .iter()
                .map(|batch| batch.formula.clone()),
        );
        let covered = Formula::any(found.coverage.arms.iter().cloned());
        let model = sat::model(&before.and(found.premise.clone()).and(covered.not()))
            .expect("the store had a model before the batch that flipped it");
        let there = |presence: &Presence| match presence {
            Presence::Present => true,
            Presence::Var(var) => *model.get(&Atom::Var(*var)).unwrap_or(&false),
            _ => false,
        };
        Witness::Struct(
            found
                .coverage
                .fields
                .iter()
                .filter(|(_, presence)| there(presence))
                .map(|(name, _)| (name.clone(), Witness::Any))
                .collect(),
        )
    }

    /// Whether one arm's tests all land on positions the solved type has an
    /// answer for. Anything else — a scalar literal whose primitive type does
    /// not agree with the solved type, a case the row never acquired or
    /// settled, a position or presence left
    /// undecided — means the typing failed, and the checks stand aside.
    ///
    /// The two shapes read a settled absence differently, because inference
    /// treats them differently. A tested *case* is always demanded present,
    /// so a case the row nonetheless settles absent went through a presence
    /// mismatch — the type error already speaks, and the match skips. A
    /// tested *field's* presence is the column rule's fresh variable, which
    /// absence can settle without a word — so an absent field is compatible,
    /// and the arm demanding it is the unreachable arm the checks report.
    fn compatible(&self, ty: &Rc<Ty>, cell: &Cell) -> bool {
        let ty = self.shape(ty);
        match cell {
            Cell::Literal(literal) => matches!(
                (literal, &*ty),
                (Literal::Natural(_), Ty::Nat)
                    | (Literal::Integer(_), Ty::Int)
                    | (Literal::Real(_), Ty::Real)
                    | (Literal::String(_), Ty::String)
                    | (Literal::Boolean(_), Ty::Boolean)
            ),
            Cell::Tag { name, payload } => match &*ty {
                Ty::Sum(row) => {
                    let row = flat(row);
                    // A rest a failure abandoned: whatever the row lists, the
                    // question of what else it holds was given up, and the
                    // type error already speaks.
                    if matches!(row.rest, Rest::Undecided) {
                        return false;
                    }
                    // A case the row never acquired cannot be reached at all
                    // here: the demand put every tested case into the row, or
                    // failed trying — and the failure is the solver's
                    // complaint, caught above.
                    row.labels
                        .get(name)
                        .is_some_and(|case| match &case.presence {
                            Presence::Present => self.compatible(&case.ty, payload),
                            _ => false,
                        })
                }
                _ => false,
            },
            Cell::Struct { fields, .. } => {
                let Ty::Struct(row) = &*ty else { return false };
                let row = flat(row);
                // The struct demand follows the same cascade rule as a sum
                // demand above. Once typing abandoned the row tail, its labels
                // are recovery debris rather than facts from which reachability
                // can be proved.
                if matches!(row.rest, Rest::Undecided) {
                    return false;
                }
                fields.iter().all(|(name, sub)| match row.labels.get(name) {
                    Some(field) => match &field.presence {
                        Presence::Absent => true,
                        Presence::Undecided => false,
                        _ => self.compatible(&field.ty, sub),
                    },
                    // A tested field the type does not name: over the
                    // closed empty struct that is a proof of absence — a
                    // value of the type has exactly the named fields — and
                    // the arm is unreachable, which is the check's to say.
                    // Over any open row the demand failed to stick, and
                    // the type error already speaks.
                    None => matches!(row.rest, Rest::Closed),
                })
            }
            // A wildcard tests nothing — and the presence cells the widening
            // makes never arrive here, since compatibility reads the arms as
            // written.
            _ => true,
        }
    }

    /// Maranget's usefulness, typed: a value of the columns' types matching
    /// `q` and no row of `rows`, or `None` when every such value is covered.
    /// The universes come from the solved types — which cases a sum may be,
    /// whether a field may be absent — rather than from the written tests
    /// alone, which is the whole of what makes the checks typed.
    ///
    /// The witness comes back one entry per column: the value the position
    /// takes, or `None` where a field column settled on absence — which the
    /// struct fold above it turns into a field left out.
    fn useful(
        &self,
        rows: &[Vec<Cell>],
        cols: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        let Some((col, later)) = cols.split_first() else {
            return rows.is_empty().then(Vec::new);
        };
        match col {
            Col::Whole(ty) => self.whole(rows, ty, later, q, walk),
            Col::Field { presence, ty } => self.field(rows, presence, ty, later, q, walk),
            Col::Rest { open } => self.rest(rows, *open, later, q, walk),
        }
    }

    /// One whole position. A column something reaches into fields at is
    /// widened first — one presence column per field the type names, then the
    /// row tail — so the rest of the walk only ever sees flat cells; a column
    /// nothing reaches into is its type alone.
    fn whole(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        let ty = self.shape(ty);
        let structs = std::iter::once(&q[0])
            .chain(rows.iter().map(|row| &row[0]))
            .any(|cell| matches!(cell, Cell::Struct { .. }));
        if structs {
            return self.widened(rows, &ty, later, q, walk);
        }
        self.shape_column(rows, &ty, later, q, walk)
    }

    /// The widening step: the fields the solved type names become one
    /// presence column apiece — an exact pattern demands absence of every one
    /// it does not mention, an open one says nothing — the constructor keeps a
    /// column of its own for the tags and numbers, and a rest column carries
    /// what each pattern says about fields beyond the named ones: an exact
    /// struct demands there are none, everything else accepts any. The
    /// witness folds back the other way: the fields settled present in
    /// braces, presence kept even where any value serves, since under
    /// exactness the presence is the information.
    fn widened(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        let struct_row = flat(
            ty.fields()
                .expect("struct cells are created only for a struct-compatible column"),
        );
        let mut named: Vec<(String, Presence, Rc<Ty>)> = struct_row
            .labels
            .iter()
            .map(|(name, field)| (name.clone(), field.presence.clone(), field.ty.clone()))
            .collect();
        // A tested field the type does not name is provably absent —
        // compatibility let it through only over the closed empty struct — so
        // it still gets a column, with the one-value universe absence is.
        for cell in std::iter::once(&q[0]).chain(rows.iter().map(|row| &row[0])) {
            if let Cell::Struct { fields, .. } = cell {
                for (name, _) in fields {
                    if !named.iter().any(|(known, _, _)| known == name) {
                        named.push((name.clone(), Presence::Absent, Rc::new(Ty::default())));
                    }
                }
            }
        }
        let widen = |cell: &Cell| -> Vec<Cell> {
            let mut wide = Vec::with_capacity(named.len() + 1);
            match cell {
                Cell::Struct { fields, exact } => {
                    for (name, _, _) in &named {
                        let sub = fields.iter().find(|(field, _)| field == name);
                        wide.push(match (sub, exact) {
                            (Some((_, sub)), _) => Cell::Present(Box::new(sub.clone())),
                            (None, true) => Cell::Absent,
                            (None, false) => Cell::Wild,
                        });
                    }
                    // The rest is the pattern's to speak to: exact
                    // means no field beyond the mentioned ones, and the
                    // mentioned ones beyond the type's are provably absent
                    // columns of their own above — so beyond the *named*
                    // ones. `..` accepts whatever else there is.
                    wide.push(match exact {
                        true => Cell::Absent,
                        false => Cell::Wild,
                    });
                }
                _ => {
                    wide.extend(std::iter::repeat_with(|| Cell::Wild).take(named.len()));
                    wide.push(Cell::Wild);
                }
            }
            wide
        };
        let mut wide_cols: Vec<Col> = named
            .iter()
            .map(|(_, presence, ty)| Col::Field {
                presence: presence.clone(),
                ty: ty.clone(),
            })
            .collect();
        // Whether fields beyond the named ones exist to be had. The fieldless
        // empty struct an exact column closes the row to admits none; any open row
        // leaves the question to the value. (A non-struct type only
        // solves beside open struct patterns — an exact one would have pinned
        // it to unit or failed — so no cell demands its rest empty and the
        // flag is moot there.)
        wide_cols.push(Col::Rest {
            open: !matches!(struct_row.rest, Rest::Closed),
        });
        wide_cols.extend(later.iter().cloned());
        let wide_rows: Vec<Vec<Cell>> = rows
            .iter()
            .map(|row| {
                let mut wide = widen(&row[0]);
                wide.extend(row[1..].iter().cloned());
                wide
            })
            .collect();
        let mut wide_q = widen(&q[0]);
        wide_q.extend(q[1..].iter().cloned());

        let mut wits = self.useful(&wide_rows, &wide_cols, &wide_q, walk)?;
        let after = wits.split_off(named.len() + 1);
        // The rest entry has nothing to print: its empty half witnesses as
        // `None`, and its nonempty half is only walked judging reachability,
        // whose witness is discarded.
        wits.pop();
        let fields: IndexMap<String, Witness> = named
            .iter()
            .zip(wits)
            .filter_map(|((name, _, _), wit)| wit.map(|witness| (name.clone(), witness)))
            .collect();
        let folded = if fields.is_empty() {
            Witness::Any
        } else {
            Witness::Struct(fields)
        };
        Some(std::iter::once(Some(folded)).chain(after).collect())
    }

    /// One field's presence: a little universe of at most two values — there,
    /// with whatever the field holds, and not there — filtered by what the
    /// solved presence allows and by what the constraints allow given the
    /// halves already entered. This is where the typed rules live: a field
    /// proved present has no absent case to cover, a field proved absent
    /// starves every cell demanding it, and one still open keeps both halves —
    /// minus whatever the definition's clause has ruled out.
    fn field(
        &self,
        rows: &[Vec<Cell>],
        presence: &Presence,
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        // What this column is called in the clause the definition promised. The
        // types the walk reads are closed, so a presence still open in one is a
        // position that clause may speak of; one settled either way, or one a
        // failure abandoned, is nothing it has an opinion about, and the
        // presence rules alone decide that column's halves.
        let literal = match presence {
            Presence::Bound(index) => Some(Formula::bound(*index)),
            _ => None,
        };
        // The present half of the universe: the rows demanding absence drop,
        // and the question moves into what the field holds — whose column
        // takes this one's place, so its witness is the field's own.
        let present = |sub: &Cell| -> Option<Vec<Option<Witness>>> {
            if !may_be_present(presence) {
                return None;
            }
            let walk = &self.assuming(walk, literal.clone())?;
            let rows: Vec<Vec<Cell>> = rows
                .iter()
                .filter_map(|row| {
                    let head = match &row[0] {
                        Cell::Present(sub) => (**sub).clone(),
                        Cell::Absent => return None,
                        // A wildcard — the widening put nothing else here.
                        _ => Cell::Wild,
                    };
                    let mut kept = vec![head];
                    kept.extend(row[1..].iter().cloned());
                    Some(kept)
                })
                .collect();
            let mut cols = vec![Col::Whole(ty.clone())];
            cols.extend(later.iter().cloned());
            let mut sub_q = vec![sub.clone()];
            sub_q.extend(q[1..].iter().cloned());
            self.useful(&rows, &cols, &sub_q, walk)
        };
        // The absent half: the rows demanding the field drop, and the field
        // contributes nothing further to the value.
        let absent = || -> Option<Vec<Option<Witness>>> {
            if !may_be_absent(presence) {
                return None;
            }
            let walk = &self.assuming(walk, literal.clone().map(Formula::not))?;
            let rows: Vec<Vec<Cell>> = rows
                .iter()
                .filter(|row| matches!(&row[0], Cell::Absent | Cell::Wild))
                .map(|row| wild_row(row))
                .collect();
            let wits = self.useful(&rows, later, &q[1..], walk)?;
            Some(std::iter::once(None).chain(wits).collect())
        };
        match &q[0] {
            Cell::Present(sub) => present(sub),
            Cell::Absent => absent(),
            // A wildcard — the widening put nothing else here — accepts
            // either half; the present one is asked first, so a field that
            // could go both ways witnesses as there.
            _ => present(&Cell::Wild).or_else(absent),
        }
    }

    /// The rest of a widened position: whatever fields the value carries
    /// beyond the ones the type names. Its universe has an empty half always —
    /// no extra fields — and a nonempty half only where the row is open. An
    /// exact cell demands the empty half; a wildcard accepts either, but the
    /// nonempty half is only *asked* judging reachability: an open arm behind
    /// an exact one is reached by the values with extra fields, while
    /// exhaustiveness does not demand those values covered — no pattern could
    /// name the fields a witness for them would have to show.
    fn rest(
        &self,
        rows: &[Vec<Cell>],
        open: bool,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        // The empty half: a value with no extra fields, which the exact and
        // the indifferent rows both accept. Nothing extra means nothing to
        // print, so the witness entry is `None`.
        let empty = || -> Option<Vec<Option<Witness>>> {
            let rows: Vec<Vec<Cell>> = rows
                .iter()
                .filter(|row| matches!(&row[0], Cell::Absent | Cell::Wild))
                .map(|row| wild_row(row))
                .collect();
            let wits = self.useful(&rows, later, &q[1..], walk)?;
            Some(std::iter::once(None).chain(wits).collect())
        };
        // The nonempty half: some field beyond the named ones, which only the
        // indifferent rows accept. Its witness is never printed — the half is
        // walked judging reachability alone — so any value serves.
        let nonempty = || -> Option<Vec<Option<Witness>>> {
            if !open {
                return None;
            }
            let rows: Vec<Vec<Cell>> = rows
                .iter()
                .filter(|row| matches!(&row[0], Cell::Wild))
                .map(|row| wild_row(row))
                .collect();
            let wits = self.useful(&rows, later, &q[1..], walk)?;
            Some(std::iter::once(Some(Witness::Any)).chain(wits).collect())
        };
        match &q[0] {
            Cell::Absent => empty(),
            // A wildcard — the widening put nothing else here.
            _ => match walk.mode {
                Mode::Reachability => empty().or_else(nonempty),
                Mode::Exhaustiveness => empty(),
            },
        }
    }

    /// One position's scalar or sum shape, the fields already peeled off: scalar literals,
    /// cases when it is a sum, and nothing testable otherwise — a type the
    /// arms cannot reach into is covered by the wildcards that got here.
    fn shape_column(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        match &**ty {
            Ty::Nat => self.scalars(rows, Scalar::Infinite(InfiniteScalar::Nat), later, q, walk),
            Ty::Int => self.scalars(rows, Scalar::Infinite(InfiniteScalar::Int), later, q, walk),
            Ty::Real => self.scalars(rows, Scalar::Infinite(InfiniteScalar::Real), later, q, walk),
            Ty::String => self.scalars(
                rows,
                Scalar::Infinite(InfiniteScalar::String),
                later,
                q,
                walk,
            ),
            Ty::Boolean => self.scalars(rows, Scalar::Boolean, later, q, walk),
            Ty::Sum(row) => self.cases(rows, &flat(row), later, q, walk),
            // Unit, an arrow, a quantified variable, the undecided type:
            // nothing tests it — compatibility said so, and the widening
            // flattened every struct — so every cell here is a wildcard, the
            // column is consumed, and any value serves.
            _ => {
                let rows: Vec<Vec<Cell>> = rows.iter().map(|row| wild_row(row)).collect();
                let wits = self.useful(&rows, later, &q[1..], walk)?;
                Some(std::iter::once(Some(Witness::Any)).chain(wits).collect())
            }
        }
    }

    /// A scalar primitive type. Boolean has precisely two values, so its two
    /// exact patterns cover it. The other scalar types deliberately keep the
    /// literal-pattern rule `Nat` had: no finite list of literals is total, so
    /// a wildcard is needed to accept the value outside that list.
    fn scalars(
        &self,
        rows: &[Vec<Cell>],
        core: Scalar,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        let narrow = |value: &Literal| -> Vec<Vec<Cell>> {
            rows.iter()
                .filter_map(|row| match &row[0] {
                    Cell::Literal(literal) if literal == value => Some(wild_row(row)),
                    Cell::Wild => Some(wild_row(row)),
                    _ => None,
                })
                .collect()
        };
        let ask = |value: &Literal| -> Option<Vec<Option<Witness>>> {
            let wits = self.useful(&narrow(value), later, &q[1..], walk)?;
            let witness = match value {
                Literal::Natural(value) => Witness::Natural(*value),
                _ => Witness::Literal(value.clone()),
            };
            Some(std::iter::once(Some(witness)).chain(wits).collect())
        };
        match &q[0] {
            Cell::Literal(value) => ask(value),
            // A wildcard asks every literal explicitly named above first;
            // an uncovered one is the most useful witness where we can print
            // it (a natural), and proves usefulness for every primitive.
            _ => {
                let listed: Vec<&Literal> = rows
                    .iter()
                    .filter_map(|row| match &row[0] {
                        Cell::Literal(value) => Some(value),
                        _ => None,
                    })
                    .collect();
                for value in &listed {
                    if let Some(wits) = ask(value) {
                        return Some(wits);
                    }
                }

                let core = match core {
                    Scalar::Boolean => {
                        // There are no boolean values other than true and
                        // false. They were each considered above if written,
                        // but ask both so an empty matrix has the same finite
                        // universe.
                        for value in [Literal::Boolean(false), Literal::Boolean(true)] {
                            if let Some(wits) = ask(&value) {
                                return Some(wits);
                            }
                        }
                        return None;
                    }
                    Scalar::Infinite(core) => core,
                };

                // The value outside every listed literal is accepted only by
                // a wildcard. For naturals choose a printable one, preserving
                // the numbers-specific diagnostic; the other scalar witness
                // remains the generic `Any` described above.
                let rows: Vec<Vec<Cell>> = rows
                    .iter()
                    .filter(|row| matches!(&row[0], Cell::Wild))
                    .map(|row| wild_row(row))
                    .collect();
                let wits = self.useful(&rows, later, &q[1..], walk)?;
                let witness = match core {
                    InfiniteScalar::Nat => {
                        let unlisted = (0..)
                            .find(|value| {
                                !listed.iter().any(|literal| {
                                    matches!(literal, Literal::Natural(found) if found == value)
                                })
                            })
                            .expect("a finite set of naturals leaves one out");
                        Witness::Natural(unlisted)
                    }
                    InfiniteScalar::Int => Witness::Literal(Literal::Integer(
                        (0..)
                            .find(|value| !listed.iter().any(|literal| matches!(literal, Literal::Integer(found) if found == value)))
                            .expect("a finite set of integers leaves one out"),
                    )),
                    InfiniteScalar::Real => Witness::Literal(Literal::Real(
                        (0..)
                            .map(|value| value as f64)
                            .find(|value| !listed.iter().any(|literal| matches!(literal, Literal::Real(found) if found.to_bits() == value.to_bits())))
                            .expect("a finite set of reals leaves one out"),
                    )),
                    InfiniteScalar::String => Witness::Literal(Literal::String(
                        (0..)
                            .map(|size| "\0".repeat(size))
                            .find(|value| !listed.iter().any(|literal| matches!(literal, Literal::String(found) if found == value)))
                            .expect("a finite set of strings leaves one out"),
                    )),
                };
                Some(std::iter::once(Some(witness)).chain(wits).collect())
            }
        }
    }

    /// A sum: the universe is the cases the solved row says a value may
    /// be — a case settled absent has no values, so an arm demanding it is
    /// never useful — plus "anything else" when the rest is still open.
    fn cases(
        &self,
        rows: &[Vec<Cell>],
        row: &Row,
        later: &[Col],
        q: &[Cell],
        walk: &Walk,
    ) -> Option<Vec<Option<Witness>>> {
        let narrow = |name: &str| -> Vec<Vec<Cell>> {
            rows.iter()
                .filter_map(|row| {
                    let payload = match &row[0] {
                        Cell::Tag { name: tag, payload } if tag == name => (**payload).clone(),
                        Cell::Wild => Cell::Wild,
                        _ => return None,
                    };
                    let mut kept = vec![payload];
                    kept.extend(row[1..].iter().cloned());
                    Some(kept)
                })
                .collect()
        };
        let ask = |sub: &Cell, name: &str, ty: &Rc<Ty>| -> Option<Vec<Option<Witness>>> {
            let mut cols = vec![Col::Whole(ty.clone())];
            cols.extend(later.iter().cloned());
            let mut sub_q = vec![sub.clone()];
            sub_q.extend(q[1..].iter().cloned());
            let mut wits = self.useful(&narrow(name), &cols, &sub_q, walk)?;
            let payload = wits.remove(0).unwrap_or(Witness::Any);
            Some(
                std::iter::once(Some(tag_witness(name, payload)))
                    .chain(wits)
                    .collect(),
            )
        };
        match &q[0] {
            Cell::Tag { name, payload } => {
                // Compatibility checked the case is in the row and present —
                // a case settled absent went through a presence mismatch, and
                // that match was skipped whole.
                let case = row
                    .labels
                    .get(name)
                    .expect("compatibility checked the case is in the row");
                ask(payload, name, &case.ty)
            }
            // A wildcard — nothing else tests a sum position — asks the
            // universe: each case a value may be, and whatever an open rest
            // still allows.
            _ => {
                let universe: Vec<(&String, &Rc<Ty>)> = row
                    .labels
                    .iter()
                    .filter(|(_, case)| may_be_present(&case.presence))
                    .map(|(name, case)| (name, &case.ty))
                    .collect();
                for (name, ty) in &universe {
                    if let Some(wits) = ask(&Cell::Wild, name, ty) {
                        return Some(wits);
                    }
                }
                let open = !matches!(row.rest, Rest::Closed);
                if open {
                    let rows: Vec<Vec<Cell>> = rows
                        .iter()
                        .filter(|row| matches!(&row[0], Cell::Wild))
                        .map(|row| wild_row(row))
                        .collect();
                    let wits = self.useful(&rows, later, &q[1..], walk)?;
                    let listed: Vec<String> =
                        universe.iter().map(|(name, _)| (*name).clone()).collect();
                    // "Anything other than" needs something to be other than;
                    // a row with no case a value may be is anything at all.
                    let witness = match listed.is_empty() {
                        true => Witness::Any,
                        false => Witness::Other(listed),
                    };
                    return Some(std::iter::once(Some(witness)).chain(wits).collect());
                }
                None
            }
        }
    }
}
