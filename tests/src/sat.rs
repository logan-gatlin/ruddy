//! Tests for [`ruddy::inference::sat`].

use ruddy::{
    inference::sat,
    types::{Atom, Formula},
};

fn var(index: u32) -> Formula {
    Formula::var(index)
}

/// Exactly one of `n` presences there: what an exact match over `n` fields
/// writes, and the shape whose answer is small however wide it is.
fn exactly_one(n: u32) -> Formula {
    Formula::any((0..n).map(|there| {
        Formula::all((0..n).map(|at| match at == there {
            true => var(at),
            false => var(at).not(),
        }))
    }))
}

/// Every atom a formula names, as a canonical form is read off them: in the
/// order the formula first names them, which is the order the printed alphabet
/// follows.
fn atoms(formula: &Formula) -> Vec<Atom> {
    let mut out = Vec::new();
    formula.atoms(&mut out);
    out
}

/// The three questions the module answers, on formulas whose answers are
/// obvious: satisfiability, entailment and a model.
#[test]
fn the_solver_answers_the_three_questions() {
    assert!(sat::satisfiable(&Formula::True));
    assert!(!sat::satisfiable(&Formula::False));
    assert!(sat::satisfiable(&var(0).xor(var(1))));
    assert!(!sat::satisfiable(&var(0).and(var(0).not())));

    // Entailment is refutation: what would have to hold for the conclusion to
    // fail has no model.
    assert!(sat::entails(&var(0).and(var(1)), &var(0)));
    assert!(!sat::entails(&var(0).or(var(1)), &var(0)));
    assert!(sat::entails(&var(0).iff(var(1)), &var(0).iff(var(1))));
    assert!(sat::entails(&Formula::False, &Formula::False));

    // A model says what each atom the formula names is; the ones it does not
    // name are absent rather than defaulted.
    let model = sat::model(&var(0).and(var(1).not())).expect("a model");
    assert!(model[&Atom::Var(0)]);
    assert!(!model[&Atom::Var(1)]);
    assert_eq!(model.len(), 2);
    assert!(sat::model(&Formula::False).is_none());
}

/// Every connective the surface grammar has, encoded and decided: each one is
/// a Tseitin definition of its own, and a formula that agreed with the wrong
/// one would answer some of these backwards.
#[test]
fn every_connective_is_encoded() {
    let cases: [(Formula, bool); 8] = [
        (var(0).and(var(0).not()), false),
        (var(0).or(var(0).not()), true),
        (var(0).not().and(var(0)), false),
        (var(0).iff(var(1)).and(var(0)).and(var(1).not()), false),
        (var(0).iff(var(1)).and(var(0)).and(var(1)), true),
        (var(0).xor(var(1)).and(var(0)).and(var(1)), false),
        (var(0).xor(var(1)).and(var(0)).and(var(1).not()), true),
        (Formula::True.and(var(0)), true),
    ];
    for (formula, holds) in cases {
        assert_eq!(sat::satisfiable(&formula), holds, "{formula}");
    }
}

/// The canonical form is the minimal sum of products over the atoms in
/// first-appearance order, with the two shapes a reader wrote recognized
/// first: a formula that *is* `a = b` prints as one rather than as the four
/// literals it expands to.
#[test]
fn the_canonical_form_is_the_shape_a_reader_wrote() {
    let canonical = |formula: Formula| -> String {
        let keep = atoms(&formula);
        sat::project(&formula, &keep).to_string()
    };

    assert_eq!(canonical(var(0).xor(var(1))), "?0 != ?1");
    assert_eq!(canonical(var(0).iff(var(1))), "?0 = ?1");
    // The long way round comes out the same: the special cases are decided by
    // what the formula *is*, not by how it was written.
    assert_eq!(
        canonical(var(0).and(var(1).not()).or(var(0).not().and(var(1)))),
        "?0 != ?1"
    );
    // Everything else is a minimal sum of products.
    assert_eq!(
        canonical(
            var(0)
                .and(var(1).not())
                .or(var(0).and(var(1)))
                .or(var(0).not().and(var(1).not()))
        ),
        "?0 or not ?1"
    );
    assert_eq!(canonical(var(0).and(var(0))), "?0");
    // A function no prime implicant is essential for: every minterm of "not all
    // three the same" is covered by two primes, so the essentials decide
    // nothing and the greedy step has to. Whatever it picks is a cover, and it
    // picks the same one every time.
    let cyclic = var(0)
        .or(var(1))
        .or(var(2))
        .and(var(0).and(var(1)).and(var(2)).not());
    assert_eq!(
        canonical(cyclic.clone()),
        "not ?0 and ?2 or ?0 and not ?1 or ?1 and not ?2"
    );
    assert_eq!(canonical(var(0).or(var(0).not())), "always");
    assert_eq!(canonical(var(0).and(var(0).not())), "never");
}

/// Existential elimination: an atom the caller does not keep is one some
/// assignment of it satisfies the formula under.
#[test]
fn projection_eliminates_what_it_does_not_keep() {
    // `a and b`, with `b` eliminated, is `a`: there is a `b` for every `a`.
    let formula = var(0).and(var(1));
    assert_eq!(sat::project(&formula, &[Atom::Var(0)]).to_string(), "?0");
    // `a != b`, with `b` eliminated, says nothing about `a` at all.
    let formula = var(0).xor(var(1));
    assert!(sat::project(&formula, &[Atom::Var(0)]).is_true());
    // And an atom the formula never names cannot be kept into existence.
    assert!(sat::project(&Formula::True, &[Atom::Var(9)]).is_true());
    // The formula nothing satisfies projects to itself, whatever is kept.
    assert_eq!(sat::project(&Formula::any([]), &[]), Formula::False);
}

/// How many atoms a formula names is not what makes it hard, so it is not what
/// the projection charges for. Each of these names more of them than a truth
/// table's row index has bits, and each is answered exactly.
#[test]
fn a_wide_projection_is_answered_exactly() {
    // Every one of them there. One product, and keeping only the first is `?0`
    // rather than a shrug: the others are eliminated, not lost.
    let wide = Formula::all((0..34).map(var));
    assert_eq!(sat::project(&wide, &atoms(&wide)), wide);
    assert_eq!(sat::project(&wide, &[Atom::Var(0)]).to_string(), "?0");

    // Any one of them there, which is thirty-four products and two to the
    // thirty-four rows. The old walk counted the rows.
    let any = Formula::any((0..34).map(var));
    assert_eq!(sat::project(&any, &atoms(&any)), any);

    // Exactly one of them there — what an exact match over thirty-four fields
    // writes — read down to what it says about the first two, which is what R10
    // quotes at an annotation whose clause covers only those.
    let one = exactly_one(34);
    assert_eq!(
        sat::project(&one, &[Atom::Var(0), Atom::Var(1)]).to_string(),
        "not ?0 or not ?1"
    );

    // The width is the formula's own, not the caller's: keeping atoms it never
    // names widens nothing.
    let keep: Vec<Atom> = (0..34).map(Atom::Var).collect();
    assert_eq!(
        sat::project(&var(0).and(var(1)), &keep).to_string(),
        "?0 and ?1"
    );
}

/// What bounds a projection is the size of its answer and not the size of its
/// question — the one formula here whose answer really is exponential is the
/// only one given up on.
#[test]
fn the_answer_is_what_bounds_a_projection() {
    // The parity of ten presences: five hundred products and no shorter sum of
    // them. With nothing to eliminate the formula is its own answer, so what
    // comes back still says everything it said.
    let parity = (1..10).fold(var(0), |sum, i| sum.xor(var(i)));
    assert_eq!(sat::project(&parity, &atoms(&parity)), parity);

    // With something to eliminate there is no such shortcut, and the answer
    // errs the way that costs a missed complaint rather than an invented one.
    let spare = parity.clone().and(var(10).or(var(10).not()));
    let keep: Vec<Atom> = (0..10).map(Atom::Var).collect();
    assert!(sat::project(&spare, &keep).is_true());

    // And eliminating enough of it leaves an answer that fits, which is
    // answered rather than given up on: a parity says nothing about any two of
    // its presences.
    assert!(sat::project(&parity, &[Atom::Var(0), Atom::Var(1)]).is_true());
}

/// The product a projection collects is read off the model by walking the
/// formula: a conjunction that failed failed at an operand, a disjunction that
/// held held at one, and an equivalence is decided by both sides. A walk that
/// took the wrong branch would name literals that did not matter, and the
/// answers below would gain products or lose them.
#[test]
fn a_projection_reads_its_product_off_the_model() {
    let canonical = |formula: Formula| -> String {
        let keep = atoms(&formula);
        sat::project(&formula, &keep).to_string()
    };

    // A conjunction that has to come out false, failing on each side in turn.
    assert_eq!(
        canonical(var(0).and(var(1)).not().and(var(0).not())),
        "not ?0"
    );
    assert_eq!(
        canonical(var(0).and(var(1)).not().and(var(0))),
        "?0 and not ?1"
    );
    // A disjunction that has to come out true, holding on each side in turn.
    assert_eq!(canonical(var(0).or(var(1)).and(var(0))), "?0");
    assert_eq!(
        canonical(var(0).or(var(1)).and(var(0).not())),
        "not ?0 and ?1"
    );
    // And one that has to come out false, which needs both sides.
    assert_eq!(canonical(var(0).or(var(1)).not()), "not ?0 and not ?1");
    // A constant inside a connective that does not fold it away holds itself
    // up, so the walk asks nothing of the model about it — and decides the
    // equivalence around it either way.
    assert_eq!(canonical(Formula::True.iff(var(0))), "?0");
    assert_eq!(canonical(Formula::False.iff(var(0))), "not ?0");
    // An equivalence under a disjunction is a branch the walk has to read
    // before it can pick a side.
    assert_eq!(
        canonical(var(0).iff(var(1)).or(var(2))),
        "not ?0 and not ?1 or ?0 and ?1 or ?2"
    );
}

/// The exact minimizer reads a truth table, and the elimination no longer
/// builds one — so it is recovered from the answer where that is cheap and the
/// cover is minimized in place where it is not. Both roads lead to the same
/// place on a formula this shape; what differs is only what it cost to get
/// there.
#[test]
fn a_cover_too_broad_to_expand_is_minimized_in_place() {
    // Nine atoms: each product leaves eight positions open, so the first fills
    // the row budget by itself and the second overruns it.
    let nine = Formula::any((0..9).map(var));
    assert_eq!(sat::project(&nine, &atoms(&nine)), nine);

    // Ten, where a single product already covers more rows than the budget
    // holds and the expansion is refused before it starts.
    let ten = Formula::any((0..10).map(var));
    assert_eq!(sat::project(&ten, &atoms(&ten)), ten);

    // Widening is what makes a product redundant, so the two steps are one
    // pass: `a and b` and `a and not b` are separate products of the cover and
    // both widen to `a`, and only one of the two survives that. A presence the
    // answer turns out not to depend on is gone from it entirely.
    let split = var(0).and(var(1)).or(var(0).and(var(1).not()));
    let redundant = (2..9).fold(split, |sum, at| sum.or(var(at)));
    assert_eq!(atoms(&redundant).len(), 9);
    assert_eq!(
        sat::project(&redundant, &atoms(&redundant)).to_string(),
        "?0 or ?2 or ?3 or ?4 or ?5 or ?6 or ?7 or ?8"
    );
}

/// The constant formulas fold as they are built, so "says nothing" is the
/// value rather than a tree of trues for something later to recognize.
#[test]
fn the_constructors_fold_the_constants() {
    assert!(Formula::True.and(Formula::True).is_true());
    assert!(Formula::all([]).is_true());
    assert_eq!(Formula::any([]), Formula::False);
    assert_eq!(Formula::True.and(var(0)), var(0));
    assert_eq!(var(0).and(Formula::True), var(0));
    assert_eq!(Formula::False.and(var(0)), Formula::False);
    assert_eq!(var(0).and(Formula::False), Formula::False);
    assert_eq!(Formula::False.or(var(0)), var(0));
    assert_eq!(var(0).or(Formula::False), var(0));
    assert_eq!(Formula::True.or(var(0)), Formula::True);
    assert_eq!(var(0).or(Formula::True), Formula::True);
    assert_eq!(Formula::True.not(), Formula::False);
    assert_eq!(Formula::False.not(), Formula::True);
    // A double negative is the thing itself.
    assert_eq!(var(0).not().not(), var(0));
}
