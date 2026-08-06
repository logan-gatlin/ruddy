//! Tests for [`ruddy::types`].

use std::rc::Rc;

use ruddy::types::{
    Assigned, Core, ParamKind, Presence, Prim, Rest, Row, RowField, Sense, Shape, Ty,
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

/// Unit is one type with one spelling, and so is the sum nothing inhabits. A
/// second way to build either would be a second empty type for the solver to
/// find not quite equal to the first, which is the whole reason the
/// constructors exist rather than the shapes being written out at each site.
#[test]
fn each_empty_type_has_one_constructor() {
    let unit = Ty::unit();
    assert!(matches!(unit.core, Core::Unit));
    assert!(unit.fields.is_trivial());
    assert_eq!(unit.to_string(), "{}");
    // The default type is the undecided one, which is what a term that has not
    // been inferred yet carries.
    assert!(matches!(Ty::default().core, Core::Undecided));

    let empty = Ty::empty_sum();
    let Core::Sum(cases) = &empty.core else {
        panic!("the empty sum is a sum: {empty:?}");
    };
    assert!(cases.is_trivial());
    assert!(empty.fields.is_trivial());
    assert_eq!(empty.to_string(), "|");

    // And a plain core is that core carrying nothing, which is every type the
    // language can currently write.
    assert!(Ty::plain(Core::Nat).fields.is_trivial());
}

/// A row says nothing at all exactly when it names no labels and allows none.
/// That is the test a printed type wears its `with` by, the test unification
/// skips a second row by, and the test a variable is bare enough to take a
/// whole type by — so it is one function rather than three readings.
#[test]
fn a_row_is_trivial_only_when_it_says_nothing() {
    assert!(Row::closed().is_trivial());
    assert!(
        !Row {
            labels: [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }
        .is_trivial()
    );
    for rest in [Rest::Var(0), Rest::Bound(0), Rest::Undecided] {
        assert!(
            !Row {
                labels: Default::default(),
                rest,
            }
            .is_trivial()
        );
    }
}

/// The row a written argument stands for at a row parameter: a struct's own
/// fields, a sum's cases, and — for the argument lowering already refused —
/// nothing decided at all, which is what an erased one has always left behind.
#[test]
fn a_written_argument_reads_as_the_row_of_its_position() {
    let field = || {
        [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
            .into_iter()
            .collect()
    };
    let strukt = Ty {
        core: Core::Unit,
        fields: Row {
            labels: field(),
            rest: Rest::Closed,
        },
    };
    assert_eq!(strukt.row(Shape::Struct).labels.len(), 1);

    let sum = Ty::plain(Core::Sum(Row {
        labels: field(),
        rest: Rest::Closed,
    }));
    assert_eq!(sum.row(Shape::Sum).labels.len(), 1);

    // Read at the shape it is not, and read when it is neither: undecided,
    // never closed. A closed row would quietly say the type has no more
    // labels, which is a claim nobody made.
    for (ty, shape) in [
        (&strukt, Shape::Sum),
        (&sum, Shape::Struct),
        (&Ty::default(), Shape::Struct),
    ] {
        let row = ty.row(shape);
        assert!(row.labels.is_empty());
        assert!(matches!(row.rest, Rest::Undecided), "{row:?}");
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

    // Each at its own sort.
    assert_eq!(nat.as_ty().to_string(), "Nat");
    assert_eq!(row.as_row(Shape::Struct).labels.len(), 1);
    assert!(matches!(presence.as_presence(), Presence::Absent));

    // A type at a row or a presence position is read for what it carries: the
    // row of that shape, and — for a bare variable, which is what
    // instantiating a scheme hands over — the variable itself.
    let fresh = Assigned::Ty(Rc::new(Ty::plain(Core::Var(7))));
    assert!(matches!(fresh.as_row(Shape::Struct).rest, Rest::Var(7)));
    assert!(matches!(fresh.as_presence(), Presence::Var(7)));
    assert!(matches!(nat.as_row(Shape::Struct).rest, Rest::Undecided));
    assert!(matches!(nat.as_presence(), Presence::Undecided));

    // And the pairs no position can produce say nothing rather than inventing
    // an answer.
    assert!(matches!(row.as_ty().core, Core::Undecided));
    assert!(matches!(presence.as_ty().core, Core::Undecided));
    assert!(matches!(presence.as_row(Shape::Sum).rest, Rest::Closed));
    assert!(matches!(row.as_presence(), Presence::Undecided));
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

/// What an argument written at a parameter has to be, asked in one place. A
/// type parameter takes anything and says so with `None`; a row parameter
/// carries the shape and the labels it may not repeat.
#[test]
fn a_parameter_says_what_an_argument_has_to_be() {
    assert_eq!(ParamKind::Type.row(), None);
    assert_eq!(ParamKind::Type.sense(), Sense::Type);

    let lacks: indexmap::IndexSet<String> = ["x".to_string()].into_iter().collect();
    for shape in [Shape::Struct, Shape::Sum] {
        let kind = ParamKind::Row {
            shape,
            lacks: lacks.clone(),
        };
        let (found, names) = kind.row().expect("a row parameter takes a row");
        assert_eq!(found, shape);
        assert_eq!(names, &lacks);
        assert_eq!(kind.sense(), Sense::Row(shape));
    }
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
