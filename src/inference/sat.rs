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
//! What does *not* go through the solver is the shape of an answer:
//! [`project`] existentially eliminates variables and puts what is left in a
//! canonical form, by enumeration. That is Shannon expansion with the
//! worst-case size the spec accepts, and it is right to keep it here rather
//! than in the solver: a satisfying assignment is a solver's business, and a
//! minimal formula a person is going to read is not.
//!
//! Staging is what keeps inference terminating. Nothing here is called from
//! unification: the solver runs at generalization boundaries, at the use-site
//! and annotation checks, and in the patterns phase, over formulas that are
//! already finite.

use std::collections::HashMap;

use varisat::{ExtendFormula, Lit, Solver, Var};

use crate::types::{Atom, Formula};

/// The widest projection [`project`] enumerates.
///
/// Shannon expansion costs one evaluation per row of a `2^(kept + dropped)`
/// truth table, and the minimization that reads the table off is worse than
/// that in the kept side alone. The spec takes the trade on the understanding
/// that these formulas are tiny — a store names only the presences some match,
/// use or clause related, and one definition relates few of them — and this is
/// the width at which the understanding stops holding.
///
/// It is a correctness bound and not only a budget. A row index is a `u32`, so
/// a formula naming 32 atoms shifts past the end of one: a panic where that is
/// checked and, where it is not, a table enumerated over a single row and a
/// formula that is not the one asked about. Answering wide is the alternative
/// to answering wrong.
const WIDTH: usize = 20;

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
/// Two jobs in one walk because they are one walk: eliminating a variable is
/// asking, for each assignment of the ones that stay, whether *any* assignment
/// of the ones that go satisfies the formula — which is exactly the truth table
/// the canonical form is read off. Doing them separately would enumerate twice.
///
/// The canonical form is a minimal sum of products over the kept atoms in
/// first-appearance order, with the two-variable `a = b` and `a != b` cases
/// recognized first because those are what a reader wrote and what R12 asks to
/// see. Deterministic throughout: the atom order fixes the literal order inside
/// a product, and the products are sorted by it.
///
/// Bounded by [`WIDTH`], past which it answers without enumerating.
pub fn project(formula: &Formula, keep: &[Atom]) -> Formula {
    let mut named = Vec::new();
    formula.atoms(&mut named);
    let kept: Vec<Atom> = named
        .iter()
        .copied()
        .filter(|atom| keep.contains(atom))
        .collect();
    let dropped: Vec<Atom> = named
        .iter()
        .copied()
        .filter(|atom| !keep.contains(atom))
        .collect();
    // Past [`WIDTH`] the table is not enumerated at all, and what comes back
    // instead errs the one way it may: `true` says less about the kept atoms
    // than the truth does, so a caller loses a complaint it might have made and
    // never invents one. A projection with nothing to eliminate is already its
    // own answer, so a wide formula that only wanted canonicalizing keeps
    // everything it said and loses the minimization alone.
    if kept.len() + dropped.len() > WIDTH {
        return match dropped.is_empty() {
            true => formula.clone(),
            false => Formula::True,
        };
    }
    let minterms = table(formula, &kept, &dropped);
    rebuild(&kept, &minterms)
}

/// Which assignments of `kept` some assignment of `dropped` satisfies the
/// formula under, as bitmasks: bit `i` set means `kept[i]` is there.
fn table(formula: &Formula, kept: &[Atom], dropped: &[Atom]) -> Vec<u32> {
    let mut minterms = Vec::new();
    for outer in 0..(1u32 << kept.len()) {
        let holds = (0..(1u32 << dropped.len())).any(|inner| {
            formula.eval(&|atom| match kept.iter().position(|known| *known == atom) {
                Some(at) => outer & (1 << at) != 0,
                // Every atom the formula names is in one list or the other, so
                // the position always resolves; a name in neither would be a
                // formula this walk never collected.
                None => {
                    let at = dropped
                        .iter()
                        .position(|known| *known == atom)
                        .expect("every atom the formula names was collected");
                    inner & (1 << at) != 0
                }
            })
        });
        if holds {
            minterms.push(outer);
        }
    }
    minterms
}

/// The formula a truth table came from, minimized and written in the canonical
/// order.
fn rebuild(atoms: &[Atom], minterms: &[u32]) -> Formula {
    if minterms.is_empty() {
        return Formula::False;
    }
    if minterms.len() == 1usize << atoms.len() {
        return Formula::True;
    }
    // The two shapes a reader wrote, recognized before any normal form is
    // taken: `a != b` is the one assignment-pair where exactly one is there,
    // and `a = b` the pair where they agree. Printing either as a sum of
    // products would be correct and unreadable.
    if atoms.len() == 2 {
        let left = Formula::Atom(atoms[0]);
        let right = Formula::Atom(atoms[1]);
        if minterms == [0b01, 0b10] {
            return left.xor(right);
        }
        if minterms == [0b00, 0b11] {
            return left.iff(right);
        }
    }
    let mut cover = cover(minterms, atoms.len());
    // By the atoms' own order, which is the order they first appear in the
    // type: the products, and the literals inside each of them.
    cover.sort_by_key(|implicant| literals(*implicant, atoms.len()));
    Formula::any(cover.into_iter().map(|implicant| {
        Formula::all(
            literals(implicant, atoms.len())
                .into_iter()
                .map(|(at, there)| match there {
                    true => Formula::Atom(atoms[at]),
                    false => Formula::Atom(atoms[at]).not(),
                }),
        )
    }))
}

/// One implicant of a truth table: which positions it fixes, and to what.
///
/// `care` has a bit per position the implicant constrains, and `mask` says
/// whether that position is there. A position outside `care` is one the
/// implicant does not look at, which is what combining two implicants that
/// differ in exactly one place produces.
type Implicant = (u32, u32);

/// The literals of one implicant, in position order: the whole of what decides
/// the printed order, and so written once.
fn literals(implicant: Implicant, width: usize) -> Vec<(usize, bool)> {
    let (mask, care) = implicant;
    (0..width)
        .filter(|at| care & (1 << at) != 0)
        .map(|at| (at, mask & (1 << at) != 0))
        .collect()
}

/// A set of implicants covering exactly `minterms`: the essential prime
/// implicants, and then greedily whichever of the rest covers most of what is
/// left.
///
/// Deterministic, which is the property R12 needs: the primes come out of
/// Quine–McCluskey in a fixed order, the essentials are forced, and the greedy
/// step breaks every tie by that order. Not guaranteed minimum where the
/// essentials leave a genuine choice — the spec asks for a deterministic
/// canonical form rather than for the smallest one — and on the formulas a
/// match's arms can write, the essentials already decide it.
fn cover(minterms: &[u32], width: usize) -> Vec<Implicant> {
    let primes = primes(minterms, width);
    let covers = |implicant: Implicant, minterm: u32| {
        let (mask, care) = implicant;
        (minterm & care) == (mask & care)
    };
    let mut chosen: Vec<Implicant> = Vec::new();
    let mut left: Vec<u32> = minterms.to_vec();
    // Essential first: a minterm only one prime covers leaves no choice, so
    // taking it is not a decision at all.
    for minterm in minterms {
        let mut only = primes.iter().filter(|prime| covers(**prime, *minterm));
        if let (Some(prime), None) = (only.next(), only.next())
            && !chosen.contains(prime)
        {
            chosen.push(*prime);
        }
    }
    left.retain(|minterm| !chosen.iter().any(|prime| covers(*prime, *minterm)));
    while !left.is_empty() {
        let best = primes
            .iter()
            .max_by_key(|prime| left.iter().filter(|m| covers(**prime, **m)).count())
            .expect("the primes cover every minterm they were built from");
        chosen.push(*best);
        left.retain(|minterm| !covers(*best, *minterm));
    }
    chosen
}

/// Every prime implicant of a truth table: Quine–McCluskey, combining
/// implicants that differ in one cared position until nothing more combines.
fn primes(minterms: &[u32], width: usize) -> Vec<Implicant> {
    // Every position cared about. `rebuild` answers the no-atom cases before
    // it gets here, so the shift is over a width the enumeration already had to
    // fit in a `u32`.
    let full = (1u32 << width) - 1;
    let mut layer: Vec<Implicant> = minterms.iter().map(|minterm| (*minterm, full)).collect();
    let mut out: Vec<Implicant> = Vec::new();
    while !layer.is_empty() {
        let mut next: Vec<Implicant> = Vec::new();
        let mut used = vec![false; layer.len()];
        for (at, one) in layer.iter().enumerate() {
            for (also, other) in layer.iter().enumerate().skip(at + 1) {
                if one.1 != other.1 {
                    continue;
                }
                let differ = (one.0 ^ other.0) & one.1;
                if differ.count_ones() != 1 {
                    continue;
                }
                used[at] = true;
                used[also] = true;
                let combined = (one.0 & !differ, one.1 & !differ);
                if !next.contains(&combined) {
                    next.push(combined);
                }
            }
        }
        // A layer holds no duplicates — the minterms it starts from are
        // distinct, and each round deduplicates what it combines — so an
        // implicant nothing combined with is a prime nothing has recorded yet.
        for (at, implicant) in layer.iter().enumerate() {
            if !used[at] {
                out.push(*implicant);
            }
        }
        layer = next;
    }
    out
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
