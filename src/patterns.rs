//! The pattern checks, run once inference has answered: exhaustiveness,
//! reachability, and the misplaced catch-all, per match.
//!
//! The phase runs after [`inference::infer`](crate::inference::infer) and
//! reads the solved types the column rule shaped, so the checks are *typed*: a
//! field the scrutinee's type proves present needs no absent case, one it
//! proves absent makes arms requiring it unreachable, and one whose presence
//! is still a variable — or a quantified `?` — must have both its cases
//! covered, independently per field. The catch-all placement rule alone stays
//! syntactic: the dedicated wording is reserved for arms that are irrefutable
//! on their face, and an exact arm the type happens to make total is reported
//! as the unreachable arm it leaves behind instead.
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

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::{
    inference::{self, unfold},
    ir::{Pattern, PatternKind, Program, Term, TermKind, Witness},
    symbol::Symbol,
    tracking::Span,
    types::{Core, Presence, Rest, Row, Scheme, Ty},
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
    /// above it — a second bare `` `A `` after `` `A x ``, a duplicate
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
    Natural(u128),
    /// A case, with what it carries — a bare tag's payload is the unit it
    /// demands, which is [`Cell::Struct`] with no fields, exact.
    Tag {
        name: String,
        payload: Box<Cell>,
    },
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
    /// Past widening only: the field must not be there.
    Absent,
}

/// One column of the typed matrix: a whole position with its solved type, or —
/// past widening — one field's presence.
#[derive(Clone)]
enum Col {
    Whole(Rc<Ty>),
    Field { presence: Presence, ty: Rc<Ty> },
}

/// The walk's context: the aliases, for looking through a declared name to
/// the shape the checks need, and the solver's complaints, for the one part
/// of the cascade rule the solved types cannot carry — a scrutinee the solver
/// already complained about is a scrutinee whose type is not to be reasoned
/// from, whatever it says.
struct Check<'a> {
    aliases: &'a IndexMap<Symbol, Scheme>,
    errors: &'a [inference::Error],
}

/// Run the checks over every match in the program. `inferred` is read for its
/// aliases — the solved types themselves were written into the terms.
pub fn check(program: &Program, inferred: &inference::Output) -> Output {
    let check = Check {
        aliases: &inferred.aliases,
        errors: &inferred.errors,
    };
    let mut out = Output {
        reports: Vec::new(),
        errors: Vec::new(),
    };
    for decl in program.terms.values() {
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
    match &term.kind {
        TermKind::Match { scrutinee, arms } => {
            check.matched(term.span, scrutinee, arms, out);
            walk(check, scrutinee, out);
            for (_, body) in arms {
                walk(check, body, out);
            }
        }
        TermKind::Apply { func, arg } => {
            walk(check, func, out);
            walk(check, arg, out);
        }
        TermKind::Fn { body, .. } => walk(check, body, out),
        TermKind::Let { value, body, .. } => {
            walk(check, value, out);
            walk(check, body, out);
        }
        TermKind::Struct(fields) => {
            for field in fields.values() {
                walk(check, &field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                walk(check, payload, out);
            }
        }
        TermKind::Project { base, .. } => walk(check, base, out),
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
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
        PatternKind::Natural(value) => Cell::Natural(*value),
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

impl Check<'_> {
    /// A type as the checks read it: the shape behind a declared name. The
    /// types are zonked, so this needs no table — only the aliases.
    fn shape(&self, ty: &Rc<Ty>) -> Rc<Ty> {
        unfold(self.aliases, ty)
    }

    /// Check one match and push its report — and whatever the checks have to
    /// say — onto the output.
    fn matched(&self, span: Span, scrutinee: &Term, arms: &[(Pattern, Term)], out: &mut Output) {
        let ty = scrutinee.ty.clone();
        // The empty match stays silent: it constrained the scrutinee to the
        // empty sum, which has no values to leave unhandled — unless the
        // constraint failed, in which case the type error already speaks.
        if arms.is_empty() {
            let shaped = self.shape(&ty);
            let empty = match &shaped.core {
                Core::Sum(row) => {
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

        // The misplaced catch-all, first in the order of ownership: one error
        // at the first arm that accepts everything and is not last. The arms
        // it starves are its to explain, so they are not additionally
        // flagged, however unreachable they are.
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
        // universes the solved scrutinee type draws.
        let cols = [Col::Whole(ty.clone())];
        let mut reported = Vec::with_capacity(arms.len());
        for (at, (pattern, _)) in arms.iter().enumerate() {
            let verdict = match misplaced {
                // The catch-all owns its own complaint, and everything after
                // it is starved.
                Some(here) if at == here => Verdict::Reachable,
                Some(here) if at > here => Verdict::Starved,
                _ => {
                    let rows: Vec<Vec<Cell>> =
                        cells[..at].iter().map(|cell| vec![cell.clone()]).collect();
                    match self.useful(&rows, &cols, &[cells[at].clone()]) {
                        Some(_) => Verdict::Reachable,
                        None => {
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
        let rows: Vec<Vec<Cell>> = cells.iter().map(|cell| vec![cell.clone()]).collect();
        let coverage = match self.useful(&rows, &cols, &[Cell::Wild]) {
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
        };

        out.reports.push(Report {
            span,
            scrutinee: ty,
            scrutinee_span: scrutinee.span,
            arms: reported,
            coverage,
        });
    }

    /// Whether one arm's tests all land on positions the solved type has an
    /// answer for. Anything else — a number where the type is not `Nat`, a
    /// case the row never acquired or settled, a position or presence left
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
            Cell::Natural(_) => matches!(ty.core, Core::Nat),
            Cell::Tag { name, payload } => match &ty.core {
                Core::Sum(row) => {
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
                fields.iter().all(|(name, sub)| match ty.fields.get(name) {
                    Some(field) => match &field.presence {
                        Presence::Absent => true,
                        Presence::Undecided => false,
                        _ => self.compatible(&field.ty, sub),
                    },
                    // A tested field the type does not name: over the
                    // fieldless unit core that is a proof of absence — a
                    // value of the type has exactly the named fields — and
                    // the arm is unreachable, which is the check's to say.
                    // Over any other core the demand failed to stick, and
                    // the type error already speaks.
                    None => matches!(ty.core, Core::Unit),
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
    fn useful(&self, rows: &[Vec<Cell>], cols: &[Col], q: &[Cell]) -> Option<Vec<Option<Witness>>> {
        let Some((col, later)) = cols.split_first() else {
            return rows.is_empty().then(Vec::new);
        };
        match col {
            Col::Whole(ty) => self.whole(rows, ty, later, q),
            Col::Field { presence, ty } => self.field(rows, presence, ty, later, q),
        }
    }

    /// One whole position. A column something reaches into fields at is
    /// widened first — one presence column per field the type names, then the
    /// core — so the rest of the walk only ever sees flat cells; a column
    /// nothing reaches into is its core alone.
    fn whole(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
    ) -> Option<Vec<Option<Witness>>> {
        let ty = self.shape(ty);
        let structs = std::iter::once(&q[0])
            .chain(rows.iter().map(|row| &row[0]))
            .any(|cell| matches!(cell, Cell::Struct { .. }));
        if structs {
            return self.widened(rows, &ty, later, q);
        }
        self.core(rows, &ty, later, q)
    }

    /// The widening step: the fields the solved type names become one
    /// presence column apiece — an exact pattern demands absence of every one
    /// it does not mention, an open one says nothing — and the core keeps a
    /// column of its own for the tags and numbers. The witness folds back the
    /// other way: the fields settled present in braces, presence kept even
    /// where any value serves, since under exactness the presence is the
    /// information.
    fn widened(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
    ) -> Option<Vec<Option<Witness>>> {
        let mut named: Vec<(String, Presence, Rc<Ty>)> = ty
            .fields
            .iter()
            .map(|(name, field)| (name.clone(), field.presence.clone(), field.ty.clone()))
            .collect();
        // A tested field the type does not name is provably absent —
        // compatibility let it through only over the fieldless unit core — so
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
                    // A struct pattern says nothing about the core beside the
                    // fields; what it demanded of the row, the types already
                    // enforced.
                    wide.push(Cell::Wild);
                }
                cell => {
                    wide.extend(std::iter::repeat_with(|| Cell::Wild).take(named.len()));
                    wide.push(cell.clone());
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
        wide_cols.push(Col::Whole(Rc::new(Ty::plain(ty.core.clone()))));
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

        let mut wits = self.useful(&wide_rows, &wide_cols, &wide_q)?;
        let after = wits.split_off(named.len() + 1);
        let core = wits.pop().flatten().unwrap_or(Witness::Any);
        let fields: IndexMap<String, Witness> = named
            .iter()
            .zip(wits)
            .filter_map(|((name, _, _), wit)| wit.map(|witness| (name.clone(), witness)))
            .collect();
        let folded = match fields.is_empty() {
            true => core,
            false => Witness::Struct(fields),
        };
        Some(std::iter::once(Some(folded)).chain(after).collect())
    }

    /// One field's presence: a little universe of at most two values — there,
    /// with whatever the field holds, and not there — filtered by what the
    /// solved presence allows. This is where the typed rules live: a field
    /// proved present has no absent case to cover, a field proved absent
    /// starves every cell demanding it, and a variable — or a quantified `?` —
    /// keeps both.
    fn field(
        &self,
        rows: &[Vec<Cell>],
        presence: &Presence,
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
    ) -> Option<Vec<Option<Witness>>> {
        // The present half of the universe: the rows demanding absence drop,
        // and the question moves into what the field holds — whose column
        // takes this one's place, so its witness is the field's own.
        let present = |sub: &Cell| -> Option<Vec<Option<Witness>>> {
            if !may_be_present(presence) {
                return None;
            }
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
            self.useful(&rows, &cols, &sub_q)
        };
        // The absent half: the rows demanding the field drop, and the field
        // contributes nothing further to the value.
        let absent = || -> Option<Vec<Option<Witness>>> {
            if !may_be_absent(presence) {
                return None;
            }
            let rows: Vec<Vec<Cell>> = rows
                .iter()
                .filter(|row| matches!(&row[0], Cell::Absent | Cell::Wild))
                .map(|row| wild_row(row))
                .collect();
            let wits = self.useful(&rows, later, &q[1..])?;
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

    /// One position's core, the fields already peeled off: numbers when it is
    /// `Nat`, cases when it is a sum, and nothing testable otherwise — a core
    /// the arms cannot reach into is covered by the wildcards that got here.
    fn core(
        &self,
        rows: &[Vec<Cell>],
        ty: &Rc<Ty>,
        later: &[Col],
        q: &[Cell],
    ) -> Option<Vec<Option<Witness>>> {
        match &ty.core {
            Core::Nat => self.naturals(rows, later, q),
            Core::Sum(row) => self.cases(rows, &flat(row), later, q),
            // Unit, an arrow, a quantified variable, the undecided type:
            // nothing tests it — compatibility said so, and the widening
            // flattened every struct — so every cell here is a wildcard, the
            // column is consumed, and any value serves.
            _ => {
                let rows: Vec<Vec<Cell>> = rows.iter().map(|row| wild_row(row)).collect();
                let wits = self.useful(&rows, later, &q[1..])?;
                Some(std::iter::once(Some(Witness::Any)).chain(wits).collect())
            }
        }
    }

    /// A `Nat` core: the listed numbers each in turn, and then some number
    /// not listed — the numbers never run out, so a column of them is covered
    /// only by a wildcard.
    fn naturals(
        &self,
        rows: &[Vec<Cell>],
        later: &[Col],
        q: &[Cell],
    ) -> Option<Vec<Option<Witness>>> {
        let narrow = |value: u128| -> Vec<Vec<Cell>> {
            rows.iter()
                .filter_map(|row| match &row[0] {
                    Cell::Natural(natural) if *natural == value => Some(wild_row(row)),
                    Cell::Wild => Some(wild_row(row)),
                    _ => None,
                })
                .collect()
        };
        match &q[0] {
            Cell::Natural(value) => {
                let wits = self.useful(&narrow(*value), later, &q[1..])?;
                Some(
                    std::iter::once(Some(Witness::Natural(*value)))
                        .chain(wits)
                        .collect(),
                )
            }
            // A wildcard — nothing else tests a `Nat` position — asks the
            // universe: each listed number, and then one that is not.
            _ => {
                let listed: IndexSet<u128> = rows
                    .iter()
                    .filter_map(|row| match &row[0] {
                        Cell::Natural(value) => Some(*value),
                        _ => None,
                    })
                    .collect();
                for value in &listed {
                    if let Some(wits) = self.useful(&narrow(*value), later, &q[1..]) {
                        return Some(
                            std::iter::once(Some(Witness::Natural(*value)))
                                .chain(wits)
                                .collect(),
                        );
                    }
                }
                let unlisted = (0..)
                    .find(|value| !listed.contains(value))
                    .expect("a finite set of naturals leaves one out");
                let rows: Vec<Vec<Cell>> = rows
                    .iter()
                    .filter(|row| matches!(&row[0], Cell::Wild))
                    .map(|row| wild_row(row))
                    .collect();
                let wits = self.useful(&rows, later, &q[1..])?;
                Some(
                    std::iter::once(Some(Witness::Natural(unlisted)))
                        .chain(wits)
                        .collect(),
                )
            }
        }
    }

    /// A sum core: the universe is the cases the solved row says a value may
    /// be — a case settled absent has no values, so an arm demanding it is
    /// never useful — plus "anything else" when the rest is still open.
    fn cases(
        &self,
        rows: &[Vec<Cell>],
        row: &Row,
        later: &[Col],
        q: &[Cell],
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
            let mut wits = self.useful(&narrow(name), &cols, &sub_q)?;
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
                    let wits = self.useful(&rows, later, &q[1..])?;
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
