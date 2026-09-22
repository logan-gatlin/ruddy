//! The propositional half of inference: deciding what a [`Formula`] says.
//!
//! Three questions and nothing else. Is a store satisfiable — which is what
//! makes a batch the one that flipped it. Does one formula force another —
//! which is what an annotation's contract and generalization's fold-back both
//! ask. And what is a model of one — which is where a witness comes from.
//!
//! All three go through `batsat`, encoded by Tseitin: a fresh variable per
//! connective, defined by the clauses that make it agree with the subformula
//! under it. Rolling a solver of our own is out of scope and would be a poor
//! trade — this is a decidable question with an off-the-shelf answer.
//!
//! [`project`] existentially eliminates variables. Independent conjunctive
//! components stay factored: turning their answers into one sum of products
//! would multiply unrelated choices. Each connected component is projected by
//! enumerating satisfying products with an incremental solver, never a truth
//! table over all assignments. The term budget bounds that enumeration before
//! minimization, and is shared across the components' alternative products.
//!
//! Staging is what keeps inference terminating. Nothing here is called from
//! unification: the solver runs at generalization boundaries, at the use-site
//! and annotation checks, and in the patterns phase, over formulas that are
//! already finite.

use std::collections::HashMap;

pub use batsat::Lit;
use batsat::{Callbacks, Solver, SolverInterface, Var, lbool};

use crate::{
    cancellation::Cancellation,
    types::{Atom, Formula},
};

/// Default maximum number of product alternatives collected by projection.
/// Independent factors share this budget additively, without distributing
/// their conjunction. Singleton products can conjoin without alternatives,
/// and together require at least one budget unit. Definitions may override
/// this with `@max_sat_terms <nat>`; enumeration is bounded before minimization.
pub const DEFAULT_MAX_TERMS: usize = 256;

/// Projection could not finish within its product-term budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermLimitExceeded {
    pub max_terms: usize,
}

/// Presentation alternatives for one connected presence-constraint group.
///
/// Inference keeps its canonical sum of products; this pair exists only so a
/// human-facing printer can choose the less expensive spelling without making
/// semantic identity depend on presentation. DNF and CNF are exact whenever
/// bounded construction completes; `relationships` preserves a compact exact
/// spelling and is also the safe fallback.
#[derive(Debug, Clone)]
pub struct PresentationGroup {
    pub dnf: Formula,
    pub cnf: Formula,
    /// The connected source form, which may already expose relationships that
    /// expanding either normal form would obscure.
    pub relationships: Formula,
}

/// The most rows the exact minimizer will expand a cover into.
///
/// Quine–McCluskey reads a truth table, and a truth table is what the
/// elimination above no longer builds. Where the answer is small enough that
/// the table can be recovered from it cheaply, it is: exact minimization is
/// what R12 asks for, and the cover behind it breaks its ties by counting rows.
/// Past here the cover is minimized in place instead — every product widened as
/// far as it will go and the ones nothing needs dropped — which is prime and
/// irredundant but not always smallest.
const MINTERMS: usize = 256;

/// Presentation explores both sides of every group, so keep its exact
/// truth-table minimization smaller than inference's one-directional pass.
/// Beyond this, exact cube combination and absorption still preserve meaning
/// while avoiding a printer whose cost can dominate type checking.
const PRESENTATION_MINTERMS: usize = 32;

/// The bounded search may compare forms larger than an inferred projection is
/// allowed to publish. This is high enough for the image-channel constraints
/// that motivated the presentation pass while still making printing finite.
const PRESENTATION_MAX_TERMS: usize = 2048;

/// Pairwise relationship recovery is quadratic in atoms and linear in cover
/// size. Wider groups keep the existing bounded normal-form path.
const PRESENTATION_BINARY_ATOMS: usize = 64;

/// Keep redundancy elimination on a recovered binary cover small as well.
const PRESENTATION_BINARY_CLAUSES: usize = 256;

/// SAT-backed clause reduction and gate recognition are presentation extras.
/// Larger covers retain the bounded, purely syntactic normal-form fallback.
const PRESENTATION_REDUCTION_CLAUSES: usize = 128;
const PRESENTATION_REDUCTION_LITERALS: usize = 1024;

/// One product term over the atoms a projection kept: what each position is
/// fixed to, and `None` where the term does not look at it.
///
/// Indexed by position in the kept list, so the order the canonical form prints
/// in is the order the vector is already in. A pair of bitmasks would be denser
/// and served while a formula could not name more atoms than a `u32` has bits;
/// this holds whatever the store relates.
type Cube = Vec<Option<bool>>;

/// A solver that keeps what it has been told between questions.
///
/// Encoding a formula is most of what answering a question about it costs —
/// the formulas the store holds are a few atoms each, and building a solver
/// to decide one takes longer than deciding it — so the store's questions are
/// put to one solver that already holds every batch. What changes between
/// questions is not the formulas but which of them are in force and what
/// the variables in them have been decided to be, and both are said with
/// assumptions rather than clauses: a batch's clauses hold only under its
/// guard literal, and a variable's binding is assumed for the one question
/// it is asked in. A batch rolled back is a guard never assumed again; a
/// binding rolled back is simply not assumed next time. Nothing added is
/// ever wrong later, which is what lets the clauses accumulate.
#[derive(Default)]
pub struct Incremental {
    solver: Solver<SolverCallbacks>,
    atoms: HashMap<Atom, Var>,
    clause: Vec<Lit>,
}

/// BatSat polls this token during search. Refresh it for every solve so an
/// incremental solver follows the revision asking the current question.
#[derive(Default)]
struct SolverCallbacks(Cancellation);

impl Callbacks for SolverCallbacks {
    fn on_start(&mut self) {
        self.0 = Cancellation::current();
    }

    fn stop(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Whether any assignment satisfies `formula`.
pub fn satisfiable(formula: &Formula) -> bool {
    model(formula).is_some()
}

/// Whether `premise` forces `conclusion` — the entailment an annotation's
/// contract is, and the one generalization's fold-back asks of a single
/// literal. Refutation, the way entailment is always decided: what would have
/// to hold for the conclusion to fail has no model.
pub fn entails(premise: &Formula, conclusion: &Formula) -> bool {
    !satisfiable(&premise.clone().and(conclusion.clone().not()))
}

/// A model of `formula`: what each atom it names is in some assignment that
/// satisfies it, or `None` when there is none.
///
/// The atoms it does *not* name are absent rather than defaulted: a witness
/// built from this should say nothing about a label the formula says nothing
/// about.
pub fn model(formula: &Formula) -> Option<HashMap<Atom, bool>> {
    if let Some(answer) = literal_model(formula) {
        return answer;
    }
    let mut encoding = Incremental::default();
    let top = encoding.encode(formula);
    encoding.add_clause(&[top]);
    if !encoding.satisfiable(&[]) {
        return None;
    }
    Some(
        encoding
            .atoms
            .into_iter()
            .map(|(atom, var)| (atom, encoding.solver.value_var(var) == lbool::TRUE))
            .collect(),
    )
}

/// The model of a formula that is one literal or one constant, read off
/// without a solver: `Some(None)` for the unsatisfiable constant, the one-entry
/// or empty assignment otherwise, and `None` when the formula is anything
/// larger and the solver has to be asked.
///
/// Three calls in four during a compile of the standard library are of this
/// shape — the store a definition without row constraints generalizes under,
/// the fold-back of a single presence literal — and building a solver to
/// answer each of them cost more than the rest of inference's propositional
/// work put together.
fn literal_model(formula: &Formula) -> Option<Option<HashMap<Atom, bool>>> {
    let (formula, holds) = match unowned(formula) {
        Formula::Not(inner) => (unowned(inner), false),
        formula => (formula, true),
    };
    match formula {
        Formula::True => Some(holds.then(HashMap::new)),
        Formula::False => Some((!holds).then(HashMap::new)),
        Formula::Atom(atom) => Some(Some(HashMap::from([(*atom, holds)]))),
        _ => None,
    }
}

/// `formula` without its ownership wrappers. Ownership is metadata; the
/// encoder looks through it and so does the literal reading above.
fn unowned(mut formula: &Formula) -> &Formula {
    while let Formula::Owned(_, inner) = formula {
        formula = inner;
    }
    formula
}

/// Existentially eliminate atoms outside `keep`, retaining independent
/// conjunctive factors rather than expanding their Cartesian product.
///
/// Components and their product literals follow the caller's retained-atom
/// order. Each connected component has a deterministic sum-of-products answer,
/// with two-variable equivalence and exclusive-or recognized by [`rebuild`].
/// The generation budget is shared across factors
/// before any minimization: singleton products conjoin without branching,
/// while covers with alternatives contribute their full raw term counts.
pub fn project(
    formula: &Formula,
    keep: &[Atom],
    max_terms: usize,
) -> Result<Formula, TermLimitExceeded> {
    let mut order = HashMap::new();
    for atom in keep {
        let next = order.len();
        order.entry(*atom).or_insert(next);
    }
    let mut components: Vec<_> = components(formula)
        .into_iter()
        .map(|component| {
            let mut atoms = Vec::new();
            component.atoms(&mut atoms);
            (component, atoms)
        })
        .collect();
    components.sort_by_key(|(_, atoms)| {
        atoms
            .iter()
            .filter_map(|atom| order.get(atom))
            .min()
            .copied()
            .unwrap_or(usize::MAX)
    });
    // A contradiction in any component makes the whole projection false,
    // even if an earlier, independent component would exhaust the budget.
    if components.len() > 1 && !satisfiable(formula) {
        return Ok(Formula::False);
    }
    let mut answer = Formula::True;
    let mut spent = 0;
    for (component, mut kept) in components {
        crate::cancellation::checkpoint();
        kept.retain(|atom| order.contains_key(atom));
        kept.sort_by_key(|atom| order[atom]);
        let available = (max_terms - spent).max(usize::from(max_terms > 0));
        let cover =
            eliminate(&component, &kept, available).ok_or(TermLimitExceeded { max_terms })?;
        if cover.is_empty() {
            return Ok(Formula::False);
        }
        // Singleton products conjoin into one product without expansion.
        // Choices remain separate factors: charge their raw covers together,
        // before minimization, rather than their Cartesian product.
        if cover.len() > 1 {
            spent += cover.len();
        }
        answer = answer.and(rebuild(&kept, minimized(&kept, cover)));
    }
    Ok(answer)
}

/// DNF, CNF and relationship-aware alternatives for each connected component.
///
/// Each direction is bounded independently. A component whose opposite truth
/// set is too broad to enumerate falls back to its existing exact formula;
/// presentation must never make a successfully inferred scheme unprintable.
pub fn presentation_groups(formula: &Formula) -> Vec<PresentationGroup> {
    components(formula)
        .into_iter()
        .map(|component| {
            let mut atoms = Vec::new();
            component.atoms(&mut atoms);
            let positions = atoms
                .iter()
                .enumerate()
                .map(|(at, atom)| (*atom, at))
                .collect();
            let cover = formula_cover(&component, true, &positions, PRESENTATION_MAX_TERMS)
                .or_else(|| eliminate(&component, &atoms, PRESENTATION_MAX_TERMS));
            let binary = cover
                .as_ref()
                .and_then(|cover| binary_cnf(&component, &atoms, cover));
            let dnf = cover
                .map(|cover| readable_dnf(&atoms, presentation_minimized(cover)))
                .unwrap_or_else(|| component.clone());
            let cnf = binary
                .or_else(|| {
                    formula_cover(&component, false, &positions, PRESENTATION_MAX_TERMS)
                        .map(|cover| presentation_cnf(&atoms, cover))
                })
                .or_else(|| {
                    eliminate(&component.clone().not(), &atoms, PRESENTATION_MAX_TERMS)
                        .map(|cover| presentation_cnf(&atoms, cover))
                })
                .unwrap_or_else(|| component.clone());
            PresentationGroup {
                dnf,
                cnf,
                relationships: component,
            }
        })
        .collect()
}

/// Recover unit and binary clauses without distributing the opposite normal
/// form. A one- or two-atom assignment absent from every DNF cube is forbidden
/// by the formula, giving a clause (often an implication). Looking at cubes
/// includes their free atoms without enumerating any of their full models.
///
/// These clauses are consequences, not necessarily a complete description:
/// a genuine three-way condition has no binary equivalent. Check the reverse
/// entailment before using this candidate, otherwise keep the normal-form
/// fallback. This also keeps presentation from weakening inferred types.
fn binary_cnf(formula: &Formula, atoms: &[Atom], cover: &[Cube]) -> Option<Formula> {
    if atoms.len() > PRESENTATION_BINARY_ATOMS {
        return None;
    }
    if cover.is_empty() {
        return Some(Formula::False);
    }
    let values = |value| match value {
        Some(false) => 0b01u8,
        Some(true) => 0b10,
        None => 0b11,
    };
    let allowed: Vec<_> = (0..atoms.len())
        .map(|at| cover.iter().fold(0, |seen, cube| seen | values(cube[at])))
        .collect();
    let mut forbidden = Vec::new();
    for (at, seen) in allowed.iter().enumerate() {
        if *seen != 0b11 {
            let mut cube = vec![None; atoms.len()];
            cube[at] = Some(*seen == 0b01);
            forbidden.push(cube);
        }
    }
    for left in 0..atoms.len() {
        crate::cancellation::checkpoint();
        if allowed[left] != 0b11 {
            continue; // Its unit clause already excludes every forbidden pair.
        }
        for right in left + 1..atoms.len() {
            if allowed[right] != 0b11 {
                continue;
            }
            let mut seen = 0u8;
            for cube in cover {
                let right_values = values(cube[right]);
                let left_values = values(cube[left]);
                if left_values & 0b01 != 0 {
                    seen |= right_values;
                }
                if left_values & 0b10 != 0 {
                    seen |= right_values << 2;
                }
                if seen == 0b1111 {
                    break;
                }
            }
            for assignment in 0..4 {
                if seen & (1 << assignment) == 0 {
                    if forbidden.len() == PRESENTATION_BINARY_CLAUSES {
                        return None;
                    }
                    let mut cube = vec![None; atoms.len()];
                    cube[left] = Some(assignment & 2 != 0);
                    cube[right] = Some(assignment & 1 != 0);
                    forbidden.push(cube);
                }
            }
        }
    }
    let candidate = readable_cnf(atoms, forbidden.clone());
    if !entails(&candidate, formula) {
        return None;
    }
    // Pairwise consequences also include transitive implications. Removing
    // redundant forbidden products removes redundant CNF clauses by duality;
    // otherwise even a short implication chain could lose to its old DNF.
    Some(readable_cnf(atoms, irredundant(atoms, forbidden)))
}

/// A bounded cover read directly from a formula's syntax.
///
/// Presentation does this instead of repeatedly solving the formula. Inferred
/// formulas are already normal-form shaped, so walking and distributing that
/// shape is dramatically cheaper; absorption after every operation also lets
/// a small opposite normal form emerge without first enumerating all models.
fn formula_cover(
    formula: &Formula,
    want: bool,
    positions: &HashMap<Atom, usize>,
    max_terms: usize,
) -> Option<Vec<Cube>> {
    let width = positions.len();
    match formula {
        Formula::True => Some(if want {
            vec![vec![None; width]]
        } else {
            Vec::new()
        }),
        Formula::False => Some(if want {
            Vec::new()
        } else {
            vec![vec![None; width]]
        }),
        Formula::Atom(atom) => {
            let mut cube = vec![None; width];
            cube[positions[atom]] = Some(want);
            Some(vec![cube])
        }
        Formula::Owned(_, inner) => formula_cover(inner, want, positions, max_terms),
        Formula::Not(inner) => formula_cover(inner, !want, positions, max_terms),
        Formula::And(left, right) => match want {
            true => cover_and(
                formula_cover(left, true, positions, max_terms)?,
                formula_cover(right, true, positions, max_terms)?,
                max_terms,
            ),
            false => cover_or(
                formula_cover(left, false, positions, max_terms)?,
                formula_cover(right, false, positions, max_terms)?,
                max_terms,
            ),
        },
        Formula::Or(left, right) => match want {
            true => cover_or(
                formula_cover(left, true, positions, max_terms)?,
                formula_cover(right, true, positions, max_terms)?,
                max_terms,
            ),
            false => cover_and(
                formula_cover(left, false, positions, max_terms)?,
                formula_cover(right, false, positions, max_terms)?,
                max_terms,
            ),
        },
        Formula::Iff(left, right) | Formula::Xor(left, right) => {
            let equal = matches!(formula, Formula::Iff(..)) == want;
            let left_true = formula_cover(left, true, positions, max_terms)?;
            let left_false = formula_cover(left, false, positions, max_terms)?;
            let right_true = formula_cover(right, true, positions, max_terms)?;
            let right_false = formula_cover(right, false, positions, max_terms)?;
            let (one, other) = match equal {
                true => (
                    cover_and(left_true, right_true, max_terms)?,
                    cover_and(left_false, right_false, max_terms)?,
                ),
                false => (
                    cover_and(left_true, right_false, max_terms)?,
                    cover_and(left_false, right_true, max_terms)?,
                ),
            };
            cover_or(one, other, max_terms)
        }
    }
}

fn push_cube(cover: &mut Vec<Cube>, cube: Cube, max_terms: usize) -> Option<()> {
    if cover.iter().any(|existing| covers(existing, &cube)) {
        return Some(());
    }
    cover.retain(|existing| !covers(&cube, existing));
    if cover.len() == max_terms {
        return None;
    }
    cover.push(cube);
    Some(())
}

fn cover_or(mut left: Vec<Cube>, right: Vec<Cube>, max_terms: usize) -> Option<Vec<Cube>> {
    for cube in right {
        push_cube(&mut left, cube, max_terms)?;
    }
    Some(left)
}

fn cover_and(left: Vec<Cube>, right: Vec<Cube>, max_terms: usize) -> Option<Vec<Cube>> {
    let mut product = Vec::new();
    for one in &left {
        for other in &right {
            let mut merged = one.clone();
            let mut agrees = true;
            for at in 0..merged.len() {
                match (merged[at], other[at]) {
                    (None, known) => merged[at] = known,
                    (Some(one), Some(other)) if one != other => {
                        agrees = false;
                        break;
                    }
                    _ => {}
                }
            }
            if agrees {
                push_cube(&mut product, merged, max_terms)?;
            }
        }
    }
    Some(product)
}

/// Conjuncts connected by an atom must be projected together. In particular,
/// splitting two uses of the same eliminated atom would give each an
/// independent witness and weaken the answer. Disjoint components commute
/// with existential quantification, so their projected answers stay factored.
fn components(formula: &Formula) -> Vec<Formula> {
    let mut conjuncts = Vec::new();
    let mut work = vec![formula];
    while let Some(formula) = work.pop() {
        match unowned(formula) {
            Formula::And(left, right) => {
                work.push(right);
                work.push(left);
            }
            formula => conjuncts.push(formula),
        }
    }
    let mut parents: Vec<_> = (0..conjuncts.len()).collect();
    fn root(parents: &mut [usize], mut at: usize) -> usize {
        while parents[at] != at {
            parents[at] = parents[parents[at]];
            at = parents[at];
        }
        at
    }
    let mut owners = HashMap::new();
    for (at, conjunct) in conjuncts.iter().enumerate() {
        crate::cancellation::checkpoint();
        let mut atoms = Vec::new();
        conjunct.atoms(&mut atoms);
        for atom in atoms {
            if let Some(&previous) = owners.get(&atom) {
                let previous = root(&mut parents, previous);
                let current = root(&mut parents, at);
                // Earliest conjunct wins, independently of hash iteration.
                parents[previous.max(current)] = previous.min(current);
            } else {
                owners.insert(atom, at);
            }
        }
    }
    let mut groups = vec![Formula::True; conjuncts.len()];
    for (at, conjunct) in conjuncts.into_iter().enumerate() {
        let group = root(&mut parents, at);
        groups[group] = std::mem::replace(&mut groups[group], Formula::True).and(conjunct.clone());
    }
    groups.retain(|formula| !formula.is_true());
    if groups.is_empty() {
        groups.push(Formula::True);
    }
    groups
}

/// The products of `∃ dropped. formula` over `kept`, or `None` where there are
/// more of them than `max_terms` allows.
///
/// One solver call per product, and the product is read off the model rather
/// than counted out of a table. The model says how the formula was satisfied;
/// [`justify`] narrows that to the literals that did the satisfying, which is a
/// product implying the formula; and the kept part of *that* is a product
/// implying the projection, since any assignment agreeing with it extends by
/// the eliminated literals into one the formula holds under. Blocking it and
/// asking again walks the answer a product at a time, so a formula whose answer
/// is twenty products takes twenty calls however many atoms it names.
fn eliminate(formula: &Formula, kept: &[Atom], max_terms: usize) -> Option<Vec<Cube>> {
    // Most inference projections are constants or literals. Keep them out of
    // the solver, just as ordinary satisfiability queries do.
    if let Some(model) = literal_model(formula) {
        return match model {
            None => Some(Vec::new()),
            Some(model) if max_terms > 0 => Some(vec![
                kept.iter().map(|atom| model.get(atom).copied()).collect(),
            ]),
            Some(_) => None,
        };
    }
    let mut cover: Vec<Cube> = Vec::new();
    let mut encoding = Incremental::default();
    let top = encoding.encode(formula);
    encoding.add_clause(&[top]);
    let positions: HashMap<_, _> = kept
        .iter()
        .enumerate()
        .map(|(i, atom)| (*atom, i))
        .collect();
    loop {
        if !encoding.satisfiable(&[]) {
            return Some(cover);
        }
        // Refuse the next product before allocating it or minimizing anything.
        if cover.len() == max_terms {
            return None;
        }
        let mut needed = Vec::new();
        justify(
            formula,
            true,
            &|atom| encoding.solver.value_var(encoding.atoms[&atom]) == lbool::TRUE,
            &mut needed,
        );
        let mut cube: Cube = vec![None; kept.len()];
        for (atom, there) in needed {
            if let Some(&at) = positions.get(&atom) {
                debug_assert!(cube[at].is_none_or(|known| known == there));
                cube[at] = Some(there);
            }
        }
        // Block this projected product in the existing solver. Re-encoding
        // the original formula and every previous blocker per model makes
        // even a bounded cover quadratic in encoding work.
        let blocker: Vec<_> = literals(&cube)
            .into_iter()
            .map(|(at, there)| {
                let lit = encoding.atom(kept[at]);
                if there { !lit } else { lit }
            })
            .collect();
        encoding.add_clause(&blocker);
        cover.push(cube);
    }
}

/// The literals of `model` that made `formula` come out `want`, and no others.
///
/// An implicant: fixing exactly these settles the formula whatever the atoms
/// left out are. Read off the tree rather than asked of the solver, because the
/// tree already knows — a conjunction that failed failed at an operand, a
/// disjunction that held held at one, and an equivalence is decided by both
/// sides either way. Where either operand would do the leftmost is taken, so
/// one model always narrows to one product.
fn justify(
    formula: &Formula,
    want: bool,
    model: &dyn Fn(Atom) -> bool,
    out: &mut Vec<(Atom, bool)>,
) {
    let mut work = vec![(formula, want)];
    while let Some((formula, want)) = work.pop() {
        match formula {
            // A constant is what it is with nothing to hold it there.
            Formula::True | Formula::False => {}
            Formula::Atom(atom) => out.push((*atom, want)),
            Formula::Owned(_, inner) => work.push((inner, want)),
            Formula::Not(inner) => work.push((inner, !want)),
            Formula::And(left, right) => match want {
                true => {
                    work.push((right, true));
                    work.push((left, true));
                }
                false => match left.eval(model) {
                    false => work.push((left, false)),
                    true => work.push((right, false)),
                },
            },
            Formula::Or(left, right) => match want {
                true => match left.eval(model) {
                    true => work.push((left, true)),
                    false => work.push((right, true)),
                },
                false => {
                    work.push((right, false));
                    work.push((left, false));
                }
            },
            Formula::Iff(left, right) | Formula::Xor(left, right) => {
                work.push((right, right.eval(model)));
                work.push((left, left.eval(model)));
            }
        }
    }
}

/// `cover` written in as few products as this knows how to write it.
///
/// Exactly, by the truth table the cover expands into, wherever that table is
/// small enough to be worth recovering — which is every formula a program has
/// so far produced, and is what keeps the printed clause the one R12 specifies.
/// Otherwise in place, which is prime and irredundant but not always smallest.
fn minimized(atoms: &[Atom], cover: Vec<Cube>) -> Vec<Cube> {
    match rows(&cover) {
        Some(rows) => chosen(&rows, &primes(&rows)),
        None => irredundant(atoms, expanded(atoms, cover)),
    }
}

/// Minimize a presentation cover without introducing more SAT work merely to
/// print a type. Small truth sets take the same exact path as inference. For a
/// larger set, combining two adjacent cubes is an exact local rewrite, and
/// absorption keeps applying it to a fixed point within the bounded cover.
fn presentation_minimized(mut cover: Vec<Cube>) -> Vec<Cube> {
    if let Some(rows) = rows_up_to(&cover, PRESENTATION_MINTERMS) {
        return chosen(&rows, &primes(&rows));
    }
    cover.sort_by_key(literals);
    loop {
        let mut combined = None;
        'pairs: for one in 0..cover.len() {
            for other in (one + 1)..cover.len() {
                if let Some(differ) = combines(&cover[one], &cover[other]) {
                    let mut cube = cover[one].clone();
                    cube[differ] = None;
                    combined = Some((one, other, cube));
                    break 'pairs;
                }
            }
        }
        let Some((one, other, cube)) = combined else {
            break;
        };
        cover.remove(other);
        cover.remove(one);
        // Combining cannot grow the cover, so this limit cannot be reached.
        push_cube(&mut cover, cube, PRESENTATION_MAX_TERMS)
            .expect("combining two presentation cubes leaves room for one");
        cover.sort_by_key(literals);
    }
    cover
}

/// Every full assignment `cover` covers, in the order counting them out would
/// give — or `None` where there are more than [`MINTERMS`] of them.
fn rows(cover: &[Cube]) -> Option<Vec<Cube>> {
    rows_up_to(cover, MINTERMS)
}

fn rows_up_to(cover: &[Cube], limit: usize) -> Option<Vec<Cube>> {
    let mut rows: Vec<Cube> = Vec::new();
    for cube in cover {
        let free: Vec<usize> = (0..cube.len()).filter(|at| cube[*at].is_none()).collect();
        // Two to the power of what the product does not look at. Asked before
        // the shift rather than after it, which is the whole difference between
        // a budget and a wrapped counter.
        if free.len() > limit.trailing_zeros() as usize {
            return None;
        }
        let count = 1usize << free.len();
        if rows.len() + count > limit {
            return None;
        }
        for filling in 0..count {
            let mut row = cube.clone();
            for (bit, at) in free.iter().enumerate() {
                row[*at] = Some(filling & (1 << bit) != 0);
            }
            rows.push(row);
        }
    }
    // By the atom order, least significant first, which is the order the old
    // enumeration produced and the order everything downstream breaks its ties
    // by. Two products of one cover can name the same row, so the sort is also
    // what makes the duplicates adjacent.
    rows.sort_by_key(|row| (0..row.len()).rev().map(|at| row[at]).collect::<Vec<_>>());
    rows.dedup();
    Some(rows)
}

/// Every prime implicant of a truth table: Quine–McCluskey, combining
/// implicants that differ in one cared position until nothing more combines.
fn primes(rows: &[Cube]) -> Vec<Cube> {
    let mut layer: Vec<Cube> = rows.to_vec();
    let mut out: Vec<Cube> = Vec::new();
    while !layer.is_empty() {
        let mut next: Vec<Cube> = Vec::new();
        let mut used = vec![false; layer.len()];
        for (at, one) in layer.iter().enumerate() {
            for (also, other) in layer.iter().enumerate().skip(at + 1) {
                let Some(differ) = combines(one, other) else {
                    continue;
                };
                used[at] = true;
                used[also] = true;
                let mut combined = one.clone();
                combined[differ] = None;
                if !next.contains(&combined) {
                    next.push(combined);
                }
            }
        }
        // A layer holds no duplicates — the rows it starts from are distinct,
        // and each round deduplicates what it combines — so an implicant
        // nothing combined with is a prime nothing has recorded yet.
        for (at, cube) in layer.iter().enumerate() {
            if !used[at] {
                out.push(cube.clone());
            }
        }
        layer = next;
    }
    out
}

/// The one position two products differ in, where they look at the same
/// positions and disagree about exactly one — and `None` for two products
/// nothing combines.
fn combines(one: &Cube, other: &Cube) -> Option<usize> {
    let mut differ = None;
    for at in 0..one.len() {
        match (one[at], other[at]) {
            (None, None) => {}
            (Some(there), Some(also)) if there == also => {}
            (Some(_), Some(_)) => match differ {
                None => differ = Some(at),
                // Two disagreements combine to nothing: what is left after
                // dropping either is a product covering rows neither did.
                Some(_) => return None,
            },
            // One looks where the other does not, so they are not two halves
            // of anything.
            _ => return None,
        }
    }
    differ
}

/// A set of products covering exactly `rows`: the essential prime implicants,
/// and then greedily whichever of the rest covers most of what is left.
///
/// Deterministic, which is the property R12 needs: the primes come out of
/// Quine–McCluskey in a fixed order, the essentials are forced, and the greedy
/// step breaks every tie by that order. Not guaranteed minimum where the
/// essentials leave a genuine choice — the spec asks for a deterministic
/// canonical form rather than for the smallest one — and on the formulas a
/// match's arms can write, the essentials already decide it.
fn chosen(rows: &[Cube], primes: &[Cube]) -> Vec<Cube> {
    let mut picked: Vec<Cube> = Vec::new();
    let mut left: Vec<Cube> = rows.to_vec();
    // Essential first: a row only one prime covers leaves no choice, so taking
    // it is not a decision at all.
    for row in rows {
        let mut only = primes.iter().filter(|prime| covers(prime, row));
        if let (Some(prime), None) = (only.next(), only.next())
            && !picked.contains(prime)
        {
            picked.push(prime.clone());
        }
    }
    left.retain(|row| !picked.iter().any(|prime| covers(prime, row)));
    while !left.is_empty() {
        let best = primes
            .iter()
            .max_by_key(|prime| left.iter().filter(|row| covers(prime, row)).count())
            .expect("the primes cover every row they were built from");
        picked.push(best.clone());
        left.retain(|row| !covers(best, row));
    }
    picked
}

/// Every product of `cover` widened as far as it will go.
///
/// A literal a product does not need is one the answer does not have to name,
/// and dropping it is what turns "this assignment works" into a rule about a
/// whole family of them. Read against the cover rather than against the formula
/// the cover came from, and it has to be: whether a product implies a
/// *projection* is a question with a quantifier on either side of it, while the
/// cover is the projection written out and asks only the one question a solver
/// answers.
fn expanded(atoms: &[Atom], cover: Vec<Cube>) -> Vec<Cube> {
    let whole = Formula::any(cover.iter().map(|cube| product(atoms, cube)));
    cover
        .into_iter()
        .map(|mut cube| {
            for at in 0..cube.len() {
                let Some(there) = cube[at] else {
                    continue;
                };
                cube[at] = None;
                // Put back where the wider product would say something the
                // cover does not.
                if !entails(&product(atoms, &cube), &whole) {
                    cube[at] = Some(there);
                }
            }
            cube
        })
        .collect()
}

/// `cover` with every product the others already cover dropped, in the atoms'
/// own order so that which of two interchangeable products survives is not a
/// matter of luck.
fn irredundant(atoms: &[Atom], cover: Vec<Cube>) -> Vec<Cube> {
    let mut kept = cover;
    let mut at = 0;
    while at < kept.len() {
        let rest = Formula::any(
            kept.iter()
                .enumerate()
                .filter(|(other, _)| *other != at)
                .map(|(_, cube)| product(atoms, cube)),
        );
        match entails(&product(atoms, &kept[at]), &rest) {
            true => {
                kept.remove(at);
            }
            false => at += 1,
        }
    }
    kept
}

/// Whether every row `prime` covers is one `row` is.
fn covers(prime: &Cube, row: &Cube) -> bool {
    (0..prime.len()).all(|at| match prime[at] {
        None => true,
        Some(there) => row[at] == Some(there),
    })
}

/// The literals of one product, in position order: the whole of what decides
/// the printed order, and so written once.
fn literals(cube: &Cube) -> Vec<(usize, bool)> {
    (0..cube.len())
        .filter_map(|at| cube[at].map(|there| (at, there)))
        .collect()
}

/// One product as a formula, in the atoms' own order.
fn product(atoms: &[Atom], cube: &Cube) -> Formula {
    Formula::all(literals(cube).into_iter().map(|(at, there)| match there {
        true => Formula::Atom(atoms[at]),
        false => Formula::Atom(atoms[at]).not(),
    }))
}

/// A two-literal relation represented by a pair of opposite assignments,
/// optionally under literals the two assignments share.
///
/// In DNF the assignments are the rows where the relation holds. In CNF they
/// are the rows it forbids, so equality and complement exchange places.
fn paired_relation(atoms: &[Atom], one: &Cube, other: &Cube, cnf: bool) -> Option<Formula> {
    let mut common = vec![None; one.len()];
    let mut differ = Vec::new();
    for at in 0..one.len() {
        match (one[at], other[at]) {
            (one, other) if one == other => common[at] = one,
            (Some(one), Some(other)) if one != other => differ.push((at, one)),
            _ => return None,
        }
    }
    if differ.len() != 2 {
        return None;
    }
    let left = Formula::Atom(atoms[differ[0].0]);
    let right = Formula::Atom(atoms[differ[1].0]);
    let equal_rows = differ[0].1 == differ[1].1;
    let relation = match equal_rows ^ cnf {
        true => left.iff(right),
        false => left.xor(right),
    };
    Some(match cnf {
        true => Formula::any(
            literals(&common)
                .into_iter()
                .map(|(at, there)| match there {
                    true => Formula::Atom(atoms[at]).not(),
                    false => Formula::Atom(atoms[at]),
                })
                .chain(std::iter::once(relation)),
        ),
        false => product(atoms, &common).and(relation),
    })
}

/// Pull local two-atom equality/complement relations out of a cover. Unlike
/// [`rebuild`]'s whole-component special case, this recognizes a chain of such
/// relationships inside a larger connected group.
fn relations(atoms: &[Atom], cover: &mut Vec<Cube>, cnf: bool) -> Vec<Formula> {
    cover.sort_by_key(literals);
    let mut related = Vec::new();
    let mut used = vec![false; cover.len()];
    for at in 0..cover.len() {
        if used[at] {
            continue;
        }
        for other in (at + 1)..cover.len() {
            if used[other] {
                continue;
            }
            if let Some(relation) = paired_relation(atoms, &cover[at], &cover[other], cnf) {
                used[at] = true;
                used[other] = true;
                related.push(relation);
                break;
            }
        }
    }
    let mut retained = Vec::with_capacity(cover.len());
    for (at, cube) in std::mem::take(cover).into_iter().enumerate() {
        if !used[at] {
            retained.push(cube);
        }
    }
    *cover = retained;
    related
}

fn readable_dnf(atoms: &[Atom], mut cover: Vec<Cube>) -> Formula {
    if cover.is_empty() {
        return Formula::False;
    }
    if cover.iter().any(|cube| cube.iter().all(Option::is_none)) {
        return Formula::True;
    }
    let mut parts = relations(atoms, &mut cover, false);
    parts.extend(cover.iter().map(|cube| product(atoms, cube)));
    Formula::any(parts)
}

/// One forbidden product as the clause that excludes it. When the clause has
/// one positive conclusion and otherwise-negative premises, retain that
/// familiar implication instead of spelling a flat disjunction.
fn clause(atoms: &[Atom], cube: &Cube) -> Formula {
    let literals = literals(cube);
    if literals.len() < 2 {
        return Formula::any(literals.into_iter().map(|(at, there)| match there {
            true => Formula::Atom(atoms[at]).not(),
            false => Formula::Atom(atoms[at]),
        }));
    }
    let conclusions: Vec<_> = literals
        .iter()
        .filter(|(_, there)| !there)
        .map(|(at, _)| *at)
        .collect();
    if conclusions.len() <= 1 {
        let (conclusion_at, conclusion) = match conclusions.first() {
            Some(at) => (*at, Formula::Atom(atoms[*at])),
            None => {
                let (at, _) = literals[literals.len() - 1];
                (at, Formula::Atom(atoms[at]).not())
            }
        };
        let premises = Formula::all(
            literals
                .into_iter()
                .filter(|(at, there)| *there && *at != conclusion_at)
                .map(|(at, _)| Formula::Atom(atoms[at])),
        );
        return premises.not().or(conclusion);
    }
    Formula::any(literals.into_iter().map(|(at, there)| match there {
        true => Formula::Atom(atoms[at]).not(),
        false => Formula::Atom(atoms[at]),
    }))
}

fn readable_cnf(atoms: &[Atom], mut complement: Vec<Cube>) -> Formula {
    if complement.is_empty() {
        return Formula::True;
    }
    if complement
        .iter()
        .any(|cube| cube.iter().all(Option::is_none))
    {
        return Formula::False;
    }
    let mut parts = gates(atoms, &mut complement);
    parts.extend(relations(atoms, &mut complement, true));
    parts.extend(complement.iter().map(|cube| clause(atoms, cube)));
    Formula::all(parts)
}

/// Clauses can follow from several others even when no pair combines or
/// absorbs. Remove those consequences before recognizing relationships: a
/// padding equation, for example, otherwise generates many redundant clauses
/// through the tuple's prefix constraints. The SAT pass removes a clause only
/// when the remaining clauses entail it, so each step preserves equivalence.
fn presentation_cnf(atoms: &[Atom], cover: Vec<Cube>) -> Formula {
    let mut cover = presentation_minimized(cover);
    if atoms.len() <= PRESENTATION_BINARY_ATOMS
        && cover.len() <= PRESENTATION_REDUCTION_CLAUSES
        && cover.iter().map(|cube| literals(cube).len()).sum::<usize>()
            <= PRESENTATION_REDUCTION_LITERALS
    {
        cover = irredundant(atoms, cover);
    }
    readable_cnf(atoms, cover)
}

/// Recover definitions of conjunctions and disjunctions from their exact CNF
/// encoding, with either polarity on every input. For example, three clauses
/// `a -> z`, `b -> z`, `z -> a or b` become `z = (a or b)`. A wide forbidden
/// cube supplies the last clause; each of its inputs needs a two-literal cube
/// with both polarities reversed. Consuming only this complete encoding makes
/// the rewrite exact without another SAT query.
fn gates(atoms: &[Atom], cover: &mut Vec<Cube>) -> Vec<Formula> {
    if atoms.len() > PRESENTATION_BINARY_ATOMS || cover.len() > PRESENTATION_REDUCTION_CLAUSES {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut used = vec![false; cover.len()];
    for at in 0..cover.len() {
        crate::cancellation::checkpoint();
        if used[at] {
            continue;
        }
        let terms = literals(&cover[at]);
        if terms.len() < 3 {
            continue;
        }
        for &(pivot, there) in &terms {
            let mut matching = Vec::new();
            for &(input, value) in &terms {
                if input == pivot {
                    continue;
                }
                let mut spoke = vec![None; atoms.len()];
                spoke[pivot] = Some(!there);
                spoke[input] = Some(!value);
                match cover
                    .iter()
                    .enumerate()
                    .position(|(other, cube)| !used[other] && *cube == spoke)
                {
                    Some(other) => matching.push(other),
                    None => break,
                }
            }
            if matching.len() != terms.len() - 1 {
                continue;
            }
            let inputs =
                terms
                    .iter()
                    .filter(|(input, _)| *input != pivot)
                    .map(|&(input, value)| {
                        let input = Formula::Atom(atoms[input]);
                        if value == there { input.not() } else { input }
                    });
            let definition = if there {
                Formula::any(inputs)
            } else {
                Formula::all(inputs)
            };
            parts.push(Formula::Atom(atoms[pivot]).iff(definition));
            used[at] = true;
            for other in matching {
                used[other] = true;
            }
            break;
        }
    }
    *cover = std::mem::take(cover)
        .into_iter()
        .enumerate()
        .filter(|(at, _)| !used[*at])
        .map(|(_, cube)| cube)
        .collect();
    parts
}

/// The formula a cover stands for, written in the canonical order.
fn rebuild(atoms: &[Atom], mut cover: Vec<Cube>) -> Formula {
    if cover.is_empty() {
        return Formula::False;
    }
    // A product that looks at nothing holds of everything.
    if cover.iter().any(|cube| cube.iter().all(Option::is_none)) {
        return Formula::True;
    }
    // By the atoms' own order, which is the order they first appear in the
    // type: the products, and the literals inside each of them.
    cover.sort_by_key(literals);
    // The two shapes a reader wrote, recognized before the sum of products is
    // written out: `a != b` is the pair of assignments where exactly one is
    // there, and `a = b` the pair where they agree. Printing either the long
    // way would be correct and unreadable.
    if atoms.len() == 2 {
        let left = Formula::Atom(atoms[0]);
        let right = Formula::Atom(atoms[1]);
        if cover == [vec![Some(false), Some(true)], vec![Some(true), Some(false)]] {
            return left.xor(right);
        }
        if cover == [vec![Some(false), Some(false)], vec![Some(true), Some(true)]] {
            return left.iff(right);
        }
    }
    Formula::any(cover.iter().map(|cube| product(atoms, cube)))
}

impl Incremental {
    /// The literal standing for `atom`, minted on first mention.
    pub fn atom(&mut self, atom: Atom) -> Lit {
        let var = *self
            .atoms
            .entry(atom)
            .or_insert_with(|| self.solver.new_var_default());
        Lit::new(var, true)
    }

    /// Every atom the solver has been told about, in no particular order.
    pub fn atoms(&self) -> impl Iterator<Item = Atom> + '_ {
        self.atoms.keys().copied()
    }

    /// Add `formula` to hold whenever the returned guard is assumed.
    pub fn add_guarded(&mut self, formula: &Formula) -> Lit {
        let guard = self.fresh();
        let top = self.encode(formula);
        self.add_clause(&[!guard, top]);
        guard
    }

    /// Add `left = right` to hold whenever the returned guard is assumed.
    pub fn add_equivalence(&mut self, left: Atom, right: Atom) -> Lit {
        let guard = self.fresh();
        let (left, right) = (self.atom(left), self.atom(right));
        self.add_clause(&[!guard, !left, right]);
        self.add_clause(&[!guard, left, !right]);
        guard
    }

    /// Whether everything added so far has a model under `assumptions`.
    pub fn satisfiable(&mut self, assumptions: &[Lit]) -> bool {
        crate::cancellation::checkpoint();
        let answer = self.solver.solve_limited(assumptions);
        // An interrupted search is neither SAT nor UNSAT. Let BatSat restore
        // its search state, then abandon this query through the usual unwind.
        crate::cancellation::checkpoint();
        assert_ne!(
            answer,
            lbool::UNDEF,
            "SAT search stopped without cancellation"
        );
        answer == lbool::TRUE
    }

    /// A literal that holds exactly when `formula` does, with the clauses that
    /// make it so added to the solver.
    fn encode(&mut self, formula: &Formula) -> Lit {
        enum Work<'a> {
            Formula(&'a Formula),
            Not,
            Binary(u8),
        }

        let mut work = vec![Work::Formula(formula)];
        let mut values = Vec::new();
        while let Some(part) = work.pop() {
            crate::cancellation::checkpoint();
            match part {
                Work::Formula(Formula::True) => {
                    let lit = self.fresh();
                    self.add_clause(&[lit]);
                    values.push(lit);
                }
                Work::Formula(Formula::False) => {
                    let lit = self.fresh();
                    self.add_clause(&[!lit]);
                    values.push(lit);
                }
                Work::Formula(Formula::Atom(atom)) => {
                    let lit = self.atom(*atom);
                    values.push(lit);
                }
                Work::Formula(Formula::Owned(_, inner)) => {
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::Not(inner)) => {
                    work.push(Work::Not);
                    work.push(Work::Formula(inner));
                }
                Work::Formula(Formula::And(left, right)) => {
                    work.push(Work::Binary(0));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Or(left, right)) => {
                    work.push(Work::Binary(1));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Iff(left, right)) => {
                    work.push(Work::Binary(2));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Formula(Formula::Xor(left, right)) => {
                    work.push(Work::Binary(3));
                    work.push(Work::Formula(right));
                    work.push(Work::Formula(left));
                }
                Work::Not => {
                    let inner = values.pop().expect("a visited formula literal");
                    values.push(!inner);
                }
                Work::Binary(kind) => {
                    let right = values.pop().expect("a visited right formula literal");
                    let left = values.pop().expect("a visited left formula literal");
                    let out = self.fresh();
                    match kind {
                        0 => {
                            self.add_clause(&[!out, left]);
                            self.add_clause(&[!out, right]);
                            self.add_clause(&[out, !left, !right]);
                        }
                        1 => {
                            self.add_clause(&[out, !left]);
                            self.add_clause(&[out, !right]);
                            self.add_clause(&[!out, left, right]);
                        }
                        2 => {
                            self.add_clause(&[!out, !left, right]);
                            self.add_clause(&[!out, left, !right]);
                            self.add_clause(&[out, left, right]);
                            self.add_clause(&[out, !left, !right]);
                        }
                        _ => {
                            self.add_clause(&[!out, left, right]);
                            self.add_clause(&[!out, !left, !right]);
                            self.add_clause(&[out, !left, right]);
                            self.add_clause(&[out, left, !right]);
                        }
                    }
                    values.push(out);
                }
            }
        }
        values.pop().expect("every formula has an encoding")
    }

    /// One more solver variable, standing for a connective rather than for an
    /// atom: nothing reads it back, so it goes into no map.
    fn fresh(&mut self) -> Lit {
        Lit::new(self.solver.new_var_default(), true)
    }

    /// BatSat sorts and simplifies clauses in place; keep one scratch buffer
    /// for the small Tseitin clauses. A contradiction is retained by the solver
    /// and reported by the next solve.
    fn add_clause(&mut self, clause: &[Lit]) {
        self.clause.clear();
        self.clause.extend_from_slice(clause);
        self.solver.add_clause_reuse(&mut self.clause);
    }
}
