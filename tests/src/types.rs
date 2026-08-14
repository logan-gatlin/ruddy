//! Tests for [`ruddy::types`].

use std::rc::Rc;

use ruddy::types::{
    Assigned, Atom, Core, Formula, ParamKind, Presence, Prim, Rest, Row, RowField, Sense, Ty,
};

#[test]
fn every_primitive_round_trips_through_its_name() {
    for &prim in Prim::ALL {
        assert_eq!(Prim::from_name(prim.name()), Some(prim));
    }
    // Only the exact spelling; a near miss is an ordinary undefined name.
    assert_eq!(Prim::from_name("Natural"), None);
    assert_eq!(Prim::from_name("nat"), None);
    assert_eq!(Prim::from_name("Unit"), None);
}

#[test]
fn distinct_primitives_are_spelled_differently() {
    // Printing is only safe while this holds: two primitives sharing a
    // spelling would make one of them unreachable through `from_name`.
    let names: std::collections::HashSet<_> = Prim::ALL.iter().map(|prim| prim.name()).collect();
    assert_eq!(names.len(), Prim::ALL.len());
}

/// Unit is one type with one spelling. A second way to build it would be a
/// second empty type for the solver to find not quite equal to the first, which
/// is the whole reason the constructor exists rather than the shape being
/// written out at each site.
#[test]
fn each_empty_type_has_one_constructor() {
    let unit = Ty::unit();
    assert!(matches!(unit.core, Core::Unit));
    assert!(unit.fields.is_empty());
    assert_eq!(unit.to_string(), "{}");
    // The default type is the undecided one, which is what a term that has not
    // been inferred yet carries.
    assert!(matches!(Ty::default().core, Core::Undecided));

    // And a plain core is that core carrying nothing, which is every type the
    // language can currently write.
    assert!(Ty::plain(Core::Nat).fields.is_empty());
}

/// A type is a core and the labels it carries, and the labels have no tail of
/// their own: what a struct says about the fields it does not name is the core
/// beside them. So `{ x: Nat }` is unit with an `x`, `{ x: Nat, .. }` is a
/// variable with an `x`, and `Nat` with an `x` is a closed type with a field.
#[test]
fn a_type_is_a_core_and_the_labels_it_carries() {
    let carrying = |core| Ty {
        core,
        fields: [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
            .into_iter()
            .collect(),
    };
    assert_eq!(carrying(Core::Unit).to_string(), "{ x: {} }");
    assert_eq!(carrying(Core::Var(3)).to_string(), "{ x: {}, ..?3 }");
    assert_eq!(carrying(Core::Nat).to_string(), "Nat with { x: {} }");
    // And the labels are a bare map: there is no second place for a `..` to
    // live, so nothing can say the same thing twice.
    assert_eq!(carrying(Core::Unit).fields.len(), 1);
}

/// [`Row`] and [`Rest`] survive for a sum's cases and reach nothing else: the
/// only place one is written into a type is inside [`Core::Sum`].
#[test]
fn a_row_is_reachable_only_through_a_sum() {
    let cases = Row {
        labels: [("A".to_string(), RowField::present(Rc::new(Ty::unit())))]
            .into_iter()
            .collect(),
        rest: Rest::Var(2),
    };
    let sum = Ty::plain(Core::Sum(cases));
    assert_eq!(sum.to_string(), "`A | ..?2");
    assert_eq!(sum.cases().labels.len(), 1);

    // Everything else allows no case it has not been shown, which it says as
    // the undecided tail an erased argument has always left behind — never as a
    // closed one, which would be a claim nobody made.
    for ty in [Ty::unit(), Ty::plain(Core::Nat), Ty::default()] {
        let cases = ty.cases();
        assert!(cases.labels.is_empty());
        assert!(matches!(cases.rest, Rest::Undecided), "{cases:?}");
    }
}

/// One value serves the three sorts a variable can have, and reading it at a
/// position it cannot reach answers with the sort's own "nothing is known"
/// rather than with a rule for something nobody can write.
#[test]
fn an_assigned_value_reads_as_the_sort_its_position_asks_for() {
    let nat = Assigned::Ty(Rc::new(Ty::plain(Core::Nat)));
    let row = Assigned::Row(Rc::new(Row {
        labels: [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
            .into_iter()
            .collect(),
        rest: Rest::Closed,
    }));
    let presence = Assigned::Presence(Presence::Absent);

    // Each at its own sort. A row position is a sum's rest and nothing else
    // now, so it takes no shape to be read at.
    assert_eq!(nat.as_ty().to_string(), "Nat");
    assert_eq!(row.as_row().labels.len(), 1);

    // A type at a row or a presence position is read for what it carries: the
    // cases it allows, and — for a bare variable, which is what instantiating a
    // scheme hands over — the variable itself.
    let fresh = Assigned::Ty(Rc::new(Ty::plain(Core::Var(7))));
    assert!(matches!(fresh.as_row().rest, Rest::Var(7)));
    assert!(matches!(nat.as_row().rest, Rest::Undecided));

    // And the pairs no position can produce say nothing rather than inventing
    // an answer.
    assert!(matches!(row.as_ty().core, Core::Undecided));
    assert!(matches!(presence.as_ty().core, Core::Undecided));
    assert!(matches!(presence.as_row().rest, Rest::Closed));
}

/// A refused binding abandons the variable it would have bound as well as the
/// value it refused, and both have to be said in the sort the variable was
/// minted for — a row variable pointed at a *type* would be a slot the row
/// readers could never follow.
#[test]
fn a_value_can_name_a_variable_and_a_nothing_of_its_own_sort() {
    let cases = [
        Assigned::Ty(Rc::new(Ty::plain(Core::Nat))),
        Assigned::Row(Rc::new(Row::closed())),
        Assigned::Presence(Presence::Present),
    ];
    for value in &cases {
        match (value.variable(3), value.undecided(), value) {
            (Assigned::Ty(var), Assigned::Ty(nothing), Assigned::Ty(_)) => {
                assert!(matches!(var.core, Core::Var(3)));
                assert!(matches!(nothing.core, Core::Undecided));
            }
            (Assigned::Row(var), Assigned::Row(nothing), Assigned::Row(_)) => {
                assert!(matches!(var.rest, Rest::Var(3)));
                assert!(matches!(nothing.rest, Rest::Undecided));
            }
            (Assigned::Presence(var), Assigned::Presence(nothing), Assigned::Presence(_)) => {
                assert!(matches!(var, Presence::Var(3)));
                assert!(matches!(nothing, Presence::Undecided));
            }
            _ => panic!("a value changed sort: {value:?}"),
        }
    }
}

/// What an argument written at a parameter has to be, asked in one place. Two
/// readings and no third: a whole type, which is what the rest of a struct is,
/// or the rest of a sum's cases. Each carries the labels it may not repeat, and
/// only the sum's answers the one question still about a shape.
#[test]
fn a_parameter_says_what_an_argument_has_to_be() {
    let empty: indexmap::IndexSet<String> = indexmap::IndexSet::new();
    let plain = ParamKind::Type {
        lacks: empty.clone(),
    };
    assert_eq!(plain.sense(), Sense::Type);
    assert_eq!(plain.lacks(), &empty);
    assert_eq!(plain.cases(), None);

    let lacks: indexmap::IndexSet<String> = ["x".to_string()].into_iter().collect();
    // A struct's `..r` is a type parameter with fields it may not name, which is
    // why `WithX Nat` is well-formed and `WithX { x: Nat }` is not.
    let fielded = ParamKind::Type {
        lacks: lacks.clone(),
    };
    assert_eq!(fielded.sense(), Sense::Type);
    assert_eq!(fielded.lacks(), &lacks);
    assert_eq!(fielded.cases(), None);

    let cases = ParamKind::Cases {
        lacks: lacks.clone(),
    };
    assert_eq!(cases.sense(), Sense::Cases);
    assert_eq!(cases.lacks(), &lacks);
    assert_eq!(cases.cases(), Some(&lacks));
}

/// A label written into a type is simply there. The constructor exists so that
/// the three places that build one — a struct literal, a written field, a tag's
/// one case — cannot disagree about what "there" is.
#[test]
fn a_written_label_is_present() {
    let field = RowField::present(Rc::new(Ty::plain(Core::Nat)));
    assert!(matches!(field.presence, Presence::Present));
    assert_eq!(field.ty.to_string(), "Nat");
}

/// A primitive is a core, and the conversion is what keeps the syntactic and
/// the semantic type languages from disagreeing about which primitives exist.
#[test]
fn a_primitive_lowers_to_its_core() {
    for &prim in Prim::ALL {
        let core: Core = prim.into();
        assert_eq!(core.to_string(), prim.name());
    }
}

/// The formula language's own walks, over every shape a formula can take.
///
/// [`Formula::substitute`] is the one of them the others are written in terms
/// of — opening a scheme's formula, reading one through what a solve decided,
/// and quantifying one into a scheme are all it — so it is what has to be right
/// about every connective.
#[test]
fn a_formula_is_walked_by_one_substitution() {
    let swap = |atom: Atom| match atom {
        Atom::Var(var) => Formula::bound(var),
        Atom::Bound(index) => Formula::var(index),
    };
    let a = Formula::var(0);
    let b = Formula::var(1);
    for (formula, swapped) in [
        (Formula::True, "always"),
        (Formula::False, "never"),
        (a.clone(), "a"),
        (a.clone().not(), "not a"),
        (a.clone().and(b.clone()), "a and b"),
        (a.clone().or(b.clone()), "a or b"),
        (a.clone().iff(b.clone()), "a = b"),
        (a.clone().xor(b.clone()), "a != b"),
    ] {
        assert_eq!(formula.substitute(&swap).to_string(), swapped);
    }
}

/// Renaming touches the solver's variables alone: a variable a scheme
/// quantified is left where it stands, because a store is written about
/// variables that exist and has nothing to say about one that does not.
#[test]
fn renaming_leaves_the_quantified_variables_alone() {
    let formula = Formula::var(0).and(Formula::bound(1));
    let renamed = formula.rename(&|var| Formula::var(var + 10));
    assert_eq!(renamed.to_string(), "?10 and b");
}

/// Opening a scheme's formula hands each variable it quantified whatever
/// instantiation minted for it — and a presence already decided folds to the
/// constant it is, which is what makes a use of a settled label claim nothing.
#[test]
fn opening_a_formula_substitutes_what_was_minted() {
    let formula = Formula::bound(0).xor(Formula::bound(1));
    assert_eq!(
        formula
            .open(&[Presence::Var(7), Presence::Var(8)])
            .to_string(),
        "?7 != ?8"
    );
    // A variable the scheme did not quantify is left standing.
    assert_eq!(Formula::var(3).open(&[Presence::Present]).to_string(), "?3");
    // Each presence reads as the literal it is.
    for (presence, printed) in [
        (Presence::Present, "always"),
        (Presence::Absent, "never"),
        (Presence::Var(2), "?2"),
        (Presence::Bound(1), "b"),
        // A failure abandoned the question, so there is nothing to require.
        (Presence::Undecided, "always"),
    ] {
        assert_eq!(presence.formula().to_string(), printed, "{presence}");
    }
}
