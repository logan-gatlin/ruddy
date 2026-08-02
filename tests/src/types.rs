//! Tests for [`ruddy::types`].

use ruddy::types::Prim;

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
