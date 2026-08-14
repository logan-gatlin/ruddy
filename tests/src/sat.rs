//! Tests for [`ruddy::inference::sat`].

use ruddy::{
    inference::sat,
    types::{Atom, Formula},
};

fn var(index: u32) -> Formula {
    Formula::var(index)
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
/// assignment of it satisfies the formula under, which is Shannon expansion
/// said as a truth table.
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
