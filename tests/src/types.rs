//! Tests for [`ruddy::types`].

use std::rc::Rc;

use indexmap::IndexMap;
use ruddy::types::{
    Assigned, Atom, Core, Formula, ParamKind, Presence, Prim, Rest, Row, RowField, Scheme, Sense,
    Shape, Ty,
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
    assert_eq!(Prim::ALL.len(), 5);
    assert_eq!(Prim::Int.name(), "Int");
    assert_eq!(Prim::Real.name(), "Real");
    assert_eq!(Prim::String.name(), "String");
    assert_eq!(Prim::Boolean.name(), "Boolean");
    assert!(matches!(Core::from(Prim::Nat), Core::Nat));
    assert!(matches!(Core::from(Prim::Int), Core::Int));
    assert!(matches!(Core::from(Prim::Real), Core::Real));
    assert!(matches!(Core::from(Prim::String), Core::String));
    assert!(matches!(Core::from(Prim::Boolean), Core::Boolean));
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
    assert_eq!(sum.to_string(), "#A | ..?2");
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
    // A struct's `..'r` is a type parameter with fields it may not name, which is
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
    assert_eq!(cases.cases(), Some((Shape::Sum, &lacks)));
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
        (a.clone(), "'a"),
        (a.clone().not(), "not 'a"),
        (a.clone().and(b.clone()), "'a and 'b"),
        (a.clone().or(b.clone()), "'a or 'b"),
        (a.clone().iff(b.clone()), "'a = 'b"),
        (a.clone().xor(b.clone()), "'a != 'b"),
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
    assert_eq!(renamed.to_string(), "?10 and 'b");
}

/// Opening a scheme's formula hands each variable it quantified whatever
/// instantiation minted for it — and a presence already decided folds to the
/// constant it is, which is what makes a use of a settled label claim nothing.
#[test]
fn opening_a_formula_substitutes_what_was_minted() {
    let formula = Formula::bound(0).xor(Formula::bound(1));
    assert_eq!(
        formula
            .open(&[
                Assigned::Presence(Presence::Var(7)),
                Assigned::Presence(Presence::Var(8)),
            ])
            .to_string(),
        "?7 != ?8"
    );
    // A variable the scheme did not quantify is left standing.
    assert_eq!(
        Formula::var(3)
            .open(&[Assigned::Presence(Presence::Present)])
            .to_string(),
        "?3"
    );
    // Each presence reads as the literal it is.
    for (presence, printed) in [
        (Presence::Present, "always"),
        (Presence::Absent, "never"),
        (Presence::Var(2), "?2"),
        (Presence::Bound(1), "'b"),
        // A failure abandoned the question, so there is nothing to require.
        (Presence::Undecided, "always"),
    ] {
        assert_eq!(presence.formula().to_string(), printed, "{presence}");
    }
}

/// A third reading of the same machinery, and the only new thing it says is
/// where it lives: an arrow carries a row beside its two sides, and a bare
/// `A -> B` is that row closed and empty — which is what pure means, and what
/// the printer writes as nothing at all.
#[test]
fn an_arrow_carries_the_effects_calling_it_may_perform() {
    let nat = || Rc::new(Ty::plain(Core::Nat));
    // The constructor every position with no effects to put on an arrow goes
    // through, so the empty row is one value rather than six literals.
    let pure = Ty::plain(Core::pure(nat(), nat()));
    assert_eq!(pure.to_string(), "Nat -> Nat");
    let Core::Arrow(_, _, effects) = &pure.core else {
        panic!("expected an arrow");
    };
    assert!(effects.labels.is_empty());
    assert!(matches!(effects.rest, Rest::Closed));

    let performing = Ty::plain(Core::Arrow(
        nat(),
        nat(),
        Row {
            labels: [("Log".to_string(), RowField::present(Rc::new(Ty::unit())))]
                .into_iter()
                .collect(),
            rest: Rest::Var(3),
        },
    ));
    assert_eq!(performing.to_string(), "Nat -> Nat + !Log + ..?3");
}

/// The third shape reads in its own noun and writes its labels its own way: an
/// effect wears the `!` that makes it one, so a message about `!Log` never asks
/// the reader to look for `Log` — and never reads like one about the case
/// `#Log` either.
#[test]
fn an_effect_row_is_read_in_effects() {
    assert_eq!(Shape::Effect.to_string(), "function");
    assert_eq!(ruddy::ui::label(Shape::Effect, "Log"), "!Log");
    assert_eq!(ruddy::ui::label(Shape::Sum, "Log"), "#Log");
    assert_eq!(ruddy::ui::label(Shape::Struct, "x"), "x");
}

/// The third parameter reading, beside a whole type and a sum's rest. It
/// answers the shape question a sum's does — both are spliced into a row, so
/// only a row can go there — and carries the labels an argument written at it
/// may not name.
#[test]
fn a_parameter_may_stand_for_an_arrows_effects() {
    let lacks: indexmap::IndexSet<String> = ["Log".to_string()].into_iter().collect();
    let effects = ParamKind::Effects {
        lacks: lacks.clone(),
    };
    assert_eq!(effects.sense(), Sense::Effects);
    assert_eq!(effects.lacks(), &lacks);
    assert_eq!(effects.cases(), Some((Shape::Effect, &lacks)));
}

/// A scheme has one index space, not two. The presences take the low
/// positions, `0..presences`, and everything else the rest, `presences..count`
/// — which is what lets a bare [`Ty`] be printed with no scheme beside it to
/// ask which alphabet a letter came out of.
#[test]
fn a_scheme_numbers_every_sort_in_one_space() {
    // A declaration's scheme quantifies its parameters and requires nothing, so
    // it has no presences and its count is the whole of it.
    let body = Rc::new(Ty::plain(Core::Bound(1)));
    let declaration = Scheme::new(2, body.clone());
    assert_eq!(declaration.count(), 2);
    assert_eq!(declaration.presences(), 0);
    assert!(declaration.formula().is_true());

    // A definition's may quantify both, and `count` is the total rather than
    // the types alone.
    let fields: IndexMap<String, RowField> = [(
        "x".to_string(),
        RowField {
            presence: Presence::Bound(0),
            ty: Rc::new(Ty::plain(Core::Bound(1))),
        },
    )]
    .into_iter()
    .collect();
    let body = Rc::new(Ty {
        core: Core::Bound(2),
        fields,
    });
    let scheme = Scheme::constrained(3, 1, body, Formula::bound(0));
    assert_eq!(scheme.count(), 3);
    assert_eq!(scheme.presences(), 1);

    // Opening it hands every position its value out of one list: the low one is
    // a presence, and the rest are types.
    let fresh = [
        Assigned::Presence(Presence::Var(7)),
        Assigned::Ty(Rc::new(Ty::plain(Core::Nat))),
        Assigned::Ty(Rc::new(Ty::unit())),
    ];
    let opened = scheme.body().open(&fresh);
    assert_eq!(opened.to_string(), "{ x when ?7: Nat }");
    assert_eq!(scheme.formula().open(&fresh).to_string(), "?7");
}

/// Every position of a scheme is opened at the sort the scheme reserved it for,
/// so a value of another sort never reaches one. Rather than a rule for what
/// cannot happen there is a value that says nothing, which absorbs the way
/// every other unanswerable value does.
#[test]
fn a_value_of_the_wrong_sort_opens_to_nothing() {
    assert_eq!(
        Assigned::Ty(Rc::new(Ty::plain(Core::Nat))).presence(),
        Presence::Undecided
    );
    assert_eq!(
        Assigned::Row(Rc::new(Row::closed())).presence(),
        Presence::Undecided
    );
    assert_eq!(
        Assigned::Presence(Presence::Present).presence(),
        Presence::Present
    );
}

/// A rigid is a leaf: it is not opened, because nothing supplies a value for
/// one. What a scheme quantified is a [`Core::Bound`], and a rigid is what an
/// annotation's own variable stands for while its body is being checked.
#[test]
fn a_rigid_is_a_leaf_that_opening_leaves_alone() {
    let rigid = Rc::new(Ty::plain(Core::Rigid {
        id: 4,
        name: "r".into(),
    }));
    let opened = rigid.open(&[Assigned::Ty(Rc::new(Ty::plain(Core::Nat)))]);
    assert!(matches!(opened.core, Core::Rigid { id: 4, .. }));

    // A sum's rest goes the same way, and prints as its name either side of
    // the substitution.
    let cases = Rc::new(Ty::plain(Core::Sum(Row::of(Rest::Rigid {
        id: 5,
        name: "s".into(),
    }))));
    assert_eq!(cases.to_string(), "| ..'s");
    let opened = cases.open(&[Assigned::Ty(Rc::new(Ty::plain(Core::Nat)))]);
    assert_eq!(opened.to_string(), "| ..'s");
}
