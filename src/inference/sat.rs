//! The propositional half of inference: deciding what a [`Formula`] says.
//!
//! Three questions and nothing else. Is a store satisfiable — which is what
//! makes a batch the one that flipped it. Does one formula force another —
//! which is what an annotation's contract and generalization's fold-back both
//! ask. And what is a model of one — which is where a witness comes from.
//!
//! All three go through `varisat`, encoded by Tseitin: a fresh variable per
//! connective, defined by the clauses that make it agree with the subformula
//! under it. Rolling a solver of our own is out of scope and would be a poor
//! trade — this is a decidable question with an off-the-shelf answer.
//!
//! [`project`] is the fourth thing and the only one that is not a single
//! question: it existentially eliminates variables and puts what is left in the
//! canonical form R12 prints. It goes through the solver as well, and why is
//! worth saying. Eliminating a variable *by enumeration* — Shannon expansion,
//! a truth table over every atom the formula names — costs two to the power of
//! the atom count whatever the formula happens to say, so a definition relating
//! twenty presences took seconds and one relating thirty-two overran the row
//! index counting them. Nothing about those formulas is hard. A match over
//! twenty fields says "exactly one of these" and the answer is twenty products;
//! the walk simply declined to look at it and counted out a million rows
//! instead. So the elimination asks the solver for satisfying *products* rather
//! than rows, and what it costs tracks the size of the answer.
//!
//! Staging is what keeps inference terminating. Nothing here is called from
//! unification: the solver runs at generalization boundaries, at the use-site
//! and annotation checks, and in the patterns phase, over formulas that are
//! already finite.

use std::collections::HashMap;

use varisat::{ExtendFormula, Lit, Solver, Var};

use crate::types::{Atom, Formula};

/// The most products [`project`] will collect before giving up on a formula.
///
/// A bound on the *answer* rather than on the question, which is the whole
/// difference between this and counting atoms. Projection is genuinely hard —
/// the parity of ten presences has five hundred prime implicants and no shorter
/// spelling — so something has to stop, and what stops here is a `where` clause
/// nobody could read. A formula naming any number of atoms whose answer is
/// small is answered exactly; one whose answer is this large had outrun the
/// page before it outran the budget.
const CUBES: usize = 256;

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

/// One product term over the atoms a projection kept: what each position is
/// fixed to, and `None` where the term does not look at it.
///
/// Indexed by position in the kept list, so the order the canonical form prints
/// in is the order the vector is already in. A pair of bitmasks would be denser
/// and served while a formula could not name more atoms than a `u32` has bits;
/// this holds whatever the store relates.
type Cube = Vec<Option<bool>>;

/// One Tseitin encoding in progress: the solver being filled, and which
/// variable each atom was given.
struct Encoding {
    solver: Solver<'static>,
    atoms: HashMap<Atom, Var>,
    next: usize,
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
    let mut encoding = Encoding {
        solver: Solver::new(),
        atoms: HashMap::new(),
        next: 0,
    };
    let top = encoding.encode(formula);
    encoding.solver.add_clause(&[top]);
    let solved = encoding
        .solver
        .solve()
        .expect("the solver is handed a finite formula and no assumptions");
    if !solved {
        return None;
    }
    let assignment = encoding
        .solver
        .model()
        .expect("a solver that answered yes has a model");
    let mut positive: Vec<Var> = Vec::new();
    for lit in assignment {
        if lit.is_positive() {
            positive.push(lit.var());
        }
    }
    Some(
        encoding
            .atoms
            .into_iter()
            .map(|(atom, var)| (atom, positive.contains(&var)))
            .collect(),
    )
}

/// `formula` with every atom it names but `keep` does not existentially
/// eliminated, in the canonical form R12 prints.
///
/// Two jobs in one pass because they are one job: eliminating a variable is
/// asking which assignments of the ones that stay some assignment of the ones
/// that go satisfies the formula under, and the answer to that *is* what the
/// canonical form is read off. [`eliminate`] finds it a product at a time and
/// [`minimized`] writes it as few products as it knows how.
///
/// The canonical form is a sum of products over the kept atoms in
/// first-appearance order, with the two-variable `a = b` and `a != b` cases
/// recognized first because those are what a reader wrote and what R12 asks to
/// see. Deterministic throughout: the atom order fixes the literal order inside
/// a product, and the products are sorted by it.
pub fn project(formula: &Formula, keep: &[Atom]) -> Formula {
    let mut named = Vec::new();
    formula.atoms(&mut named);
    let kept: Vec<Atom> = named
        .iter()
        .copied()
        .filter(|atom| keep.contains(atom))
        .collect();
    let Some(cover) = eliminate(formula, &kept) else {
        // More products than [`CUBES`] allows, so what comes back errs the one
        // way it may: `true` says less about the kept atoms than the truth
        // does, so a caller loses a complaint it might have made and never
        // invents one. Where nothing was being eliminated the formula is
        // already its own answer, and keeps everything it said.
        return match named.len() == kept.len() {
            true => formula.clone(),
            false => Formula::True,
        };
    };
    rebuild(&kept, minimized(&kept, cover))
}

/// The products of `∃ dropped. formula` over `kept`, or `None` where there are
/// more of them than [`CUBES`] allows.
///
/// One solver call per product, and the product is read off the model rather
/// than counted out of a table. The model says how the formula was satisfied;
/// [`justify`] narrows that to the literals that did the satisfying, which is a
/// product implying the formula; and the kept part of *that* is a product
/// implying the projection, since any assignment agreeing with it extends by
/// the eliminated literals into one the formula holds under. Blocking it and
/// asking again walks the answer a product at a time, so a formula whose answer
/// is twenty products takes twenty calls however many atoms it names.
fn eliminate(formula: &Formula, kept: &[Atom]) -> Option<Vec<Cube>> {
    let mut cover: Vec<Cube> = Vec::new();
    // What is left to account for: everything no product found so far covers.
    let mut left = Formula::True;
    loop {
        let Some(model) = model(&formula.clone().and(left.clone())) else {
            return Some(cover);
        };
        let mut needed = Vec::new();
        justify(formula, true, &|atom| model[&atom], &mut needed);
        let mut cube: Cube = vec![None; kept.len()];
        for (atom, there) in needed {
            if let Some(at) = kept.iter().position(|known| *known == atom) {
                cube[at] = Some(there);
            }
        }
        if cover.len() == CUBES {
            return None;
        }
        left = left.and(product(kept, &cube).not());
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
    match formula {
        // A constant is what it is with nothing to hold it there.
        Formula::True | Formula::False => {}
        Formula::Atom(atom) => out.push((*atom, want)),
        Formula::Not(inner) => justify(inner, !want, model, out),
        Formula::And(left, right) => match want {
            true => {
                justify(left, true, model, out);
                justify(right, true, model, out);
            }
            false => match left.eval(model) {
                false => justify(left, false, model, out),
                true => justify(right, false, model, out),
            },
        },
        Formula::Or(left, right) => match want {
            true => match left.eval(model) {
                true => justify(left, true, model, out),
                false => justify(right, true, model, out),
            },
            false => {
                justify(left, false, model, out);
                justify(right, false, model, out);
            }
        },
        Formula::Iff(left, right) | Formula::Xor(left, right) => {
            justify(left, left.eval(model), model, out);
            justify(right, right.eval(model), model, out);
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

/// Every full assignment `cover` covers, in the order counting them out would
/// give — or `None` where there are more than [`MINTERMS`] of them.
fn rows(cover: &[Cube]) -> Option<Vec<Cube>> {
    let mut rows: Vec<Cube> = Vec::new();
    for cube in cover {
        let free: Vec<usize> = (0..cube.len()).filter(|at| cube[*at].is_none()).collect();
        // Two to the power of what the product does not look at. Asked before
        // the shift rather than after it, which is the whole difference between
        // a budget and a wrapped counter.
        if free.len() > MINTERMS.trailing_zeros() as usize {
            return None;
        }
        let count = 1usize << free.len();
        if rows.len() + count > MINTERMS {
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

impl Encoding {
    /// A literal that holds exactly when `formula` does, with the clauses that
    /// make it so added to the solver.
    fn encode(&mut self, formula: &Formula) -> Lit {
        match formula {
            Formula::True => {
                let lit = self.fresh();
                self.solver.add_clause(&[lit]);
                lit
            }
            Formula::False => {
                let lit = self.fresh();
                self.solver.add_clause(&[!lit]);
                lit
            }
            Formula::Atom(atom) => {
                let next = self.next;
                let var = *self.atoms.entry(*atom).or_insert_with(|| {
                    self.next += 1;
                    Var::from_index(next)
                });
                Lit::from_var(var, true)
            }
            Formula::Not(inner) => !self.encode(inner),
            Formula::And(left, right) => {
                let (left, right) = (self.encode(left), self.encode(right));
                let out = self.fresh();
                self.solver.add_clause(&[!out, left]);
                self.solver.add_clause(&[!out, right]);
                self.solver.add_clause(&[out, !left, !right]);
                out
            }
            Formula::Or(left, right) => {
                let (left, right) = (self.encode(left), self.encode(right));
                let out = self.fresh();
                self.solver.add_clause(&[out, !left]);
                self.solver.add_clause(&[out, !right]);
                self.solver.add_clause(&[!out, left, right]);
                out
            }
            Formula::Iff(left, right) => {
                let (left, right) = (self.encode(left), self.encode(right));
                let out = self.fresh();
                self.solver.add_clause(&[!out, !left, right]);
                self.solver.add_clause(&[!out, left, !right]);
                self.solver.add_clause(&[out, left, right]);
                self.solver.add_clause(&[out, !left, !right]);
                out
            }
            Formula::Xor(left, right) => {
                let (left, right) = (self.encode(left), self.encode(right));
                let out = self.fresh();
                self.solver.add_clause(&[!out, left, right]);
                self.solver.add_clause(&[!out, !left, !right]);
                self.solver.add_clause(&[out, !left, right]);
                self.solver.add_clause(&[out, left, !right]);
                out
            }
        }
    }

    /// One more solver variable, standing for a connective rather than for an
    /// atom: nothing reads it back, so it goes into no map.
    fn fresh(&mut self) -> Lit {
        let var = Var::from_index(self.next);
        self.next += 1;
        Lit::from_var(var, true)
    }
}
