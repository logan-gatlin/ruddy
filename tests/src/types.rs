//! Tests for [`ruddy::types`].

use std::sync::Arc;

use indexmap::IndexMap;
use ruddy::symbol::{Bundle, Mint, Namespace, Version};
use ruddy::types::{
    Assigned, Atom, EffectId, Formula, ParamKind, Presence, Prim, Rest, Row, RowField, Scheme,
    Sense, Shape, Ty, open_hidden, same_finite_syntax,
};

#[test]
fn effect_row_keys_are_unambiguous_and_read_legacy_artifacts() {
    let first = EffectId::structural("A\u{1f}B".into(), "C:1".into()).row_key();
    let second = EffectId::structural("A".into(), "B\u{1f}C:1".into()).row_key();
    assert_ne!(first, second);
    assert_eq!(EffectId::parse_row_key(&first), Some(("A\u{1f}B", "C:1")));
    assert_eq!(EffectId::parse_row_key(&second), Some(("A", "B\u{1f}C:1")));
    assert_eq!(
        EffectId::parse_row_key("A\u{1f}legacy"),
        Some(("A", "legacy"))
    );
    assert_eq!(EffectId::parse_row_key("\u{1e}e1:A1:xjunk"), None);
    assert_eq!(
        EffectId::parse_row_key("\u{1e}e1:A1:xjunk\u{1f}legacy"),
        Some(("\u{1e}e1:A1:xjunk", "legacy"))
    );
    assert_eq!(
        EffectId::parse_canonical_row_key(&first),
        Some(("A\u{1f}B", "C:1"))
    );
    assert_eq!(EffectId::parse_canonical_row_key("A\u{1f}legacy"), None);
    assert_eq!(EffectId::parse_row_key("\u{1e}e"), None);
    assert_eq!(EffectId::parse_row_key("\u{1e}eA"), None);
    assert_eq!(EffectId::parse_row_key("\u{1e}e1A"), None);
    assert_eq!(
        EffectId::parse_row_key("\u{1e}e18446744073709551616:A1:x"),
        None
    );
    assert_eq!(EffectId::parse_row_key("ordinary"), None);
}

fn semantic_name(symbol: ruddy::symbol::Symbol, ty: Arc<Ty>) -> Arc<Ty> {
    Arc::new(Ty::Named {
        symbol,
        name: Arc::from("Layer"),
        args: vec![ty].into(),
    })
}

#[test]
fn opening_scrubs_imported_recovery_presences_before_solving() {
    let recovered = Ty::Struct(Row {
        labels: [(
            "x".into(),
            RowField {
                presence: Presence::Recovered(u32::MAX),
                ty: Arc::new(Ty::Nat),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    });
    let opened = recovered.open(&[]);
    let Ty::Struct(row) = &*opened else {
        panic!("opened recovery type was not a struct")
    };
    assert!(matches!(row.labels["x"].presence, Presence::Undecided));
}

#[test]
fn finite_semantic_syntax_equality_compares_every_identity_and_position() {
    let ty = |ty| Arc::new(ty);
    let same = |left, right| same_finite_syntax(&ty(left), &ty(right));

    for primitive in [Ty::Nat, Ty::Int, Ty::Real, Ty::String, Ty::Bool] {
        assert!(same(primitive.clone(), primitive));
    }
    assert!(same(Ty::Undecided, Ty::Undecided));
    assert!(!same(Ty::Nat, Ty::Int));
    assert!(same(Ty::Var(1), Ty::Var(1)));
    assert!(!same(Ty::Var(1), Ty::Var(2)));
    assert!(same(Ty::Bound(1), Ty::Bound(1)));
    assert!(!same(Ty::Bound(1), Ty::Bound(2)));
    assert!(same(
        Ty::Rigid {
            id: 1,
            name: Arc::from("left"),
        },
        Ty::Rigid {
            id: 1,
            name: Arc::from("right"),
        },
    ));
    assert!(!same(
        Ty::Rigid {
            id: 1,
            name: Arc::from("a"),
        },
        Ty::Rigid {
            id: 2,
            name: Arc::from("a"),
        },
    ));

    let row = |presence, rest| Row {
        labels: [(
            "field".into(),
            RowField {
                presence,
                ty: ty(Ty::Nat),
            },
        )]
        .into_iter()
        .collect(),
        rest,
    };
    for (left, right, expected) in [
        (Presence::Present, Presence::Present, true),
        (Presence::Absent, Presence::Absent, true),
        (Presence::Undecided, Presence::Undecided, true),
        (Presence::Var(1), Presence::Var(1), true),
        (Presence::Var(1), Presence::Var(2), false),
        (Presence::Bound(1), Presence::Bound(1), true),
        (Presence::Bound(1), Presence::Bound(2), false),
        (Presence::Recovered(1), Presence::Recovered(1), true),
        (Presence::Recovered(1), Presence::Recovered(2), false),
        (Presence::Present, Presence::Absent, false),
    ] {
        assert_eq!(
            same(
                Ty::Struct(row(left, Rest::Closed)),
                Ty::Struct(row(right, Rest::Closed))
            ),
            expected
        );
    }
    for (left, right, expected) in [
        (Rest::Closed, Rest::Closed, true),
        (Rest::Undecided, Rest::Undecided, true),
        (Rest::Var(1), Rest::Var(1), true),
        (Rest::Var(1), Rest::Var(2), false),
        (Rest::Bound(1), Rest::Bound(1), true),
        (Rest::Bound(1), Rest::Bound(2), false),
        (
            Rest::Rigid {
                id: 1,
                name: Arc::from("left"),
            },
            Rest::Rigid {
                id: 1,
                name: Arc::from("right"),
            },
            true,
        ),
        (
            Rest::Rigid {
                id: 1,
                name: Arc::from("a"),
            },
            Rest::Rigid {
                id: 2,
                name: Arc::from("a"),
            },
            false,
        ),
        (Rest::Closed, Rest::Undecided, false),
    ] {
        assert_eq!(
            same(Ty::Sum(Row::of(left)), Ty::Sum(Row::of(right))),
            expected
        );
    }
    assert!(same(
        Ty::Struct(Row::of(Rest::More(Arc::new(Row::closed())))),
        Ty::Struct(Row::of(Rest::More(Arc::new(Row::closed())))),
    ));
    let shared_row = Arc::new(row(Presence::Present, Rest::Closed));
    assert!(same(
        Ty::Struct(Row::of(Rest::More(shared_row.clone()))),
        Ty::Struct(Row::of(Rest::More(shared_row))),
    ));

    assert!(!same(
        Ty::Struct(Row::closed()),
        Ty::Struct(row(Presence::Present, Rest::Closed))
    ));
    assert!(!same(
        Ty::Struct(row(Presence::Present, Rest::Closed)),
        Ty::Struct(Row {
            labels: [("other".into(), RowField::present(ty(Ty::Nat)))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }),
    ));
    assert!(!same(
        Ty::Struct(row(Presence::Present, Rest::Closed)),
        Ty::Struct(Row {
            labels: [("field".into(), RowField::present(ty(Ty::Int)))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }),
    ));
    assert!(same(
        Ty::Struct(row(Presence::Absent, Rest::Closed)),
        Ty::Struct(Row {
            labels: [(
                "field".into(),
                RowField {
                    presence: Presence::Absent,
                    ty: ty(Ty::Int),
                },
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        }),
    ));
    assert!(same(
        Ty::Arrow(ty(Ty::Nat), ty(Ty::Int), Row::closed()),
        Ty::Arrow(ty(Ty::Nat), ty(Ty::Int), Row::closed()),
    ));

    let bundle = Bundle::new("syntax", Version::new(1, 0, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let left_symbol = mint
        .global(None, Namespace::Types, "Left")
        .expect("fresh symbol");
    let right_symbol = mint
        .global(None, Namespace::Types, "Right")
        .expect("fresh symbol");
    let named = |symbol, name: &str, args: Vec<Arc<Ty>>| Ty::Named {
        symbol,
        name: Arc::from(name),
        args: args.into(),
    };
    assert!(same(
        named(left_symbol, "First", vec![ty(Ty::Nat)]),
        named(left_symbol, "Second", vec![ty(Ty::Nat)]),
    ));
    assert!(!same(
        named(left_symbol, "Same", Vec::new()),
        named(right_symbol, "Same", Vec::new()),
    ));
    assert!(!same(
        named(left_symbol, "Same", Vec::new()),
        named(left_symbol, "Same", vec![ty(Ty::Nat)]),
    ));
    assert!(!same(
        named(left_symbol, "Same", vec![ty(Ty::Nat)]),
        named(left_symbol, "Same", vec![ty(Ty::Int)]),
    ));

    let shared_ty = ty(Ty::Arrow(ty(Ty::Nat), ty(Ty::Int), Row::closed()));
    assert!(same_finite_syntax(&shared_ty, &shared_ty));
}

#[test]
fn finite_semantic_syntax_equality_memoizes_independent_shared_dags() {
    fn binary_dag(mut leaf: Arc<Ty>, depth: usize) -> Arc<Ty> {
        for _ in 0..depth {
            leaf = Arc::new(Ty::Arrow(leaf.clone(), leaf, Row::closed()));
        }
        leaf
    }

    // These have 41 independently allocated nodes apiece but 2^40 unfolded
    // paths. Pair memoization must compare the shared presentations, not every
    // path through their infinite-tree interpretation.
    let left = binary_dag(Arc::new(Ty::Nat), 40);
    let right = binary_dag(Arc::new(Ty::Nat), 40);
    assert!(same_finite_syntax(&left, &right));

    let different = binary_dag(Arc::new(Ty::Int), 40);
    assert!(!same_finite_syntax(&left, &different));

    // Distinct type wrappers can also converge on one shared `Rest::More`
    // pair, so row pairs need the same memoization as type pairs.
    let left_row = Arc::new(Row::closed());
    let right_row = Arc::new(Row::closed());
    let wrap = |row: &Arc<Row>| Arc::new(Ty::Struct(Row::of(Rest::More(row.clone()))));
    let left = Arc::new(Ty::pure(wrap(&left_row), wrap(&left_row)));
    let right = Arc::new(Ty::pure(wrap(&right_row), wrap(&right_row)));
    assert!(same_finite_syntax(&left, &right));
}

fn pending_effect() -> EffectId {
    let bundle = Bundle::new("test", Version::new(1, 0, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let symbol = mint
        .global(None, Namespace::Effects, "Log")
        .expect("fresh effect");
    EffectId::pending(symbol)
}

#[test]
fn a_pending_effect_has_an_unresolved_name() {
    assert_eq!(pending_effect().name(), "<unresolved effect>");
}

#[test]
#[should_panic(expected = "effect row reached inference before structuralization")]
fn a_pending_effect_has_no_semantic_row_key() {
    let _ = pending_effect().row_key();
}

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
    assert_eq!(Prim::ALL.len(), 14);
    assert_eq!(Prim::Int.name(), "Int");
    assert_eq!(Prim::Real.name(), "Real");
    assert_eq!(Prim::String.name(), "String");
    assert_eq!(Prim::Bool.name(), "Bool");
    assert!(matches!(Ty::from(Prim::Nat), Ty::Nat));
    assert!(matches!(Ty::from(Prim::Int), Ty::Int));
    assert!(matches!(Ty::from(Prim::Real), Ty::Real));
    assert!(matches!(Ty::from(Prim::String), Ty::String));
    assert!(matches!(Ty::from(Prim::Bool), Ty::Bool));
}

/// Unit is one type with one spelling. A second way to build it would be a
/// second empty type for the solver to find not quite equal to the first, which
/// is the whole reason the constructor exists rather than the shape being
/// written out at each site.
#[test]
fn each_empty_type_has_one_constructor() {
    let unit = Ty::unit();
    assert!(matches!(
        unit,
        Ty::Struct(Row {
            rest: Rest::Closed,
            ..
        })
    ));
    assert!(matches!(&unit, Ty::Struct(row) if row.labels.is_empty()));
    assert_eq!(unit.to_string(), "()");
    // The default type is the undecided one, which is what a term that has not
    // been inferred yet carries.
    assert!(matches!(Ty::default(), Ty::Undecided));

    // And a plain core is that core carrying nothing, which is every type the
    // language can currently write.
    assert!(matches!(Ty::plain(Ty::Nat), Ty::Nat));
}

/// Structs own field rows; non-struct types do not expose fields.
#[test]
fn only_struct_types_carry_fields() {
    let ty = Ty::Struct(Row {
        labels: [("x".to_string(), RowField::present(Arc::new(Ty::unit())))]
            .into_iter()
            .collect(),
        rest: Rest::Var(3),
    });
    assert_eq!(ty.to_string(), "{ x: (), ..?3 }");
    assert!(ty.fields().is_some());
    assert!(Ty::Nat.fields().is_none());
}

/// [`Row`] and [`Rest`] survive for a sum's cases and reach nothing else: the
/// only place one is written into a type is inside [`Ty::Sum`].
#[test]
fn a_row_is_reachable_only_through_a_sum() {
    let cases = Row {
        labels: [("A".to_string(), RowField::present(Arc::new(Ty::unit())))]
            .into_iter()
            .collect(),
        rest: Rest::Var(2),
    };
    let sum = Ty::plain(Ty::Sum(cases));
    assert_eq!(sum.to_string(), "#A | ..?2");
    assert_eq!(sum.cases().labels.len(), 1);

    // Everything else allows no case it has not been shown, which it says as
    // the undecided tail an erased argument has always left behind — never as a
    // closed one, which would be a claim nobody made.
    for ty in [Ty::unit(), Ty::plain(Ty::Nat), Ty::default()] {
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
    let nat = Assigned::Ty(Arc::new(Ty::plain(Ty::Nat)));
    let row = Assigned::Row(Arc::new(Row {
        labels: [("x".to_string(), RowField::present(Arc::new(Ty::unit())))]
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
    let fresh = Assigned::Ty(Arc::new(Ty::plain(Ty::Var(7))));
    assert!(matches!(fresh.as_row().rest, Rest::Var(7)));
    let structure = Assigned::Ty(Arc::new(Ty::Struct(Row::closed())));
    let sum = Assigned::Ty(Arc::new(Ty::Sum(Row::closed())));
    assert!(matches!(structure.as_row().rest, Rest::Closed));
    assert!(matches!(sum.as_row().rest, Rest::Closed));
    assert!(matches!(nat.as_row().rest, Rest::Undecided));

    // And the pairs no position can produce say nothing rather than inventing
    // an answer.
    assert!(matches!(&*row.as_ty(), Ty::Undecided));
    assert!(matches!(&*presence.as_ty(), Ty::Undecided));
    assert!(matches!(presence.as_row().rest, Rest::Closed));
}

/// A refused binding abandons the variable it would have bound as well as the
/// value it refused, and both have to be said in the sort the variable was
/// minted for — a row variable pointed at a *type* would be a slot the row
/// readers could never follow.
#[test]
fn a_value_can_name_a_variable_and_a_nothing_of_its_own_sort() {
    let cases = [
        Assigned::Ty(Arc::new(Ty::plain(Ty::Nat))),
        Assigned::Row(Arc::new(Row::closed())),
        Assigned::Presence(Presence::Present),
    ];
    for value in &cases {
        match (value.variable(3), value.undecided(), value) {
            (Assigned::Ty(var), Assigned::Ty(nothing), Assigned::Ty(_)) => {
                assert!(matches!(&*var, Ty::Var(3)));
                assert!(matches!(&*nothing, Ty::Undecided));
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
    assert_eq!(plain.row(), None);

    let lacks: indexmap::IndexSet<String> = ["x".to_string()].into_iter().collect();
    // A struct's `..'r` is a type parameter with fields it may not name, which is
    // why `WithX Nat` is well-formed and `WithX { x: Nat }` is not.
    let fielded = ParamKind::Fields {
        lacks: lacks.clone(),
    };
    assert_eq!(fielded.sense(), Sense::Fields);
    assert_eq!(fielded.lacks(), &lacks);
    assert_eq!(fielded.row(), Some((Shape::Struct, &lacks)));

    let cases = ParamKind::Cases {
        lacks: lacks.clone(),
    };
    assert_eq!(cases.sense(), Sense::Cases);
    assert_eq!(cases.lacks(), &lacks);
    assert_eq!(cases.row(), Some((Shape::Sum, &lacks)));
}

/// A label written into a type is simply there. The constructor exists so that
/// the three places that build one — a struct literal, a written field, a tag's
/// one case — cannot disagree about what "there" is.
#[test]
fn a_written_label_is_present() {
    let field = RowField::present(Arc::new(Ty::plain(Ty::Nat)));
    assert!(matches!(field.presence, Presence::Present));
    assert_eq!(field.ty.to_string(), "Nat");
}

/// A primitive is a core, and the conversion is what keeps the syntactic and
/// the semantic type languages from disagreeing about which primitives exist.
#[test]
fn a_primitive_lowers_to_its_core() {
    for &prim in Prim::ALL {
        let core: Ty = prim.into();
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

#[test]
fn deep_formula_opening_and_use_site_walks_are_stack_safe() {
    std::thread::Builder::new()
        .name("deep-formula-open".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut formula = Formula::bound(0);
            for depth in 0..30_000 {
                formula = match depth % 3 {
                    0 => Formula::Not(Arc::new(formula)),
                    1 => Formula::And(Arc::new(formula), Arc::new(Formula::bound(1))),
                    _ => Formula::Or(Arc::new(Formula::bound(1)), Arc::new(formula)),
                };
            }
            let opened = formula.open(&[
                Assigned::Presence(Presence::Var(7)),
                Assigned::Presence(Presence::Var(8)),
            ]);

            let mut work = vec![&opened];
            let mut nodes = 0;
            while let Some(at) = work.pop() {
                nodes += 1;
                match at {
                    Formula::Atom(Atom::Var(7 | 8)) => {}
                    Formula::Not(inner) => work.push(inner),
                    Formula::And(left, right) | Formula::Or(left, right) => {
                        work.push(right);
                        work.push(left);
                    }
                    other => panic!("opening preserved an unexpected node: {other:?}"),
                }
            }
            assert!(nodes > 30_000);
            let mut atoms = Vec::new();
            opened.atoms(&mut atoms);
            assert_eq!(atoms, [Atom::Var(8), Atom::Var(7)]);
            let _ = opened.eval(&|atom| atom == Atom::Var(7));
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep formula opening and reads use bounded stack");
}

#[test]
fn deep_semantic_type_and_scheme_display_are_stack_safe() {
    std::thread::Builder::new()
        .name("deep-semantic-display".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let bundle = Bundle::new("deep-display", Version::new(1, 0, 0)).unwrap();
            let mut mint = Mint::new(bundle);
            let layer = mint
                .global(None, Namespace::Types, "Layer")
                .expect("a semantic name");
            let mut ty = Arc::new(Ty::Nat);
            for depth in 0..30_000 {
                ty = match depth % 3 {
                    0 => semantic_name(layer, ty),
                    1 => Arc::new(Ty::Arrow(Arc::new(Ty::Nat), ty, Row::closed())),
                    _ => Arc::new(Ty::Struct(Row {
                        labels: [(
                            "payload".into(),
                            RowField {
                                presence: Presence::Present,
                                ty,
                            },
                        )]
                        .into_iter()
                        .collect(),
                        rest: Rest::Closed,
                    })),
                };
            }
            let scheme = Scheme::new(0, ty);
            let shown = scheme.to_string();
            assert!(shown.contains("Layer"));
            assert!(shown.contains("payload"));
        })
        .expect("the bounded-stack display regression starts")
        .join()
        .expect("semantic display uses an explicit stack");
}

#[test]
fn deep_package_formatting_and_structural_accessors_are_stack_safe() {
    std::thread::Builder::new()
        .name("deep-package-reads".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;

            let mut record = Arc::new(Ty::Struct(Row {
                labels: [("marker".into(), RowField::present(Arc::new(Ty::Nat)))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }));
            for _ in 0..DEPTH {
                record = Arc::new(Ty::Package(record));
            }

            // Formatting an arrow asks its input for its precedence before the
            // explicit formatter consumes it. Both that query and the package
            // jobs themselves must remain on the heap work list.
            let arrow = Ty::pure(record.clone(), Arc::new(Ty::Nat));
            assert_eq!(arrow.to_string(), "{ marker: Nat } -> Nat");
            assert!(record.fields().unwrap().labels.contains_key("marker"));

            let mut sum = Arc::new(Ty::Sum(Row {
                labels: [("Only".into(), RowField::present(Arc::new(Ty::unit())))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }));
            for _ in 0..DEPTH {
                sum = Arc::new(Ty::Package(sum));
            }
            assert!(sum.cases().labels.contains_key("Only"));
            assert_eq!(sum.to_string(), "#Only");
        })
        .expect("the bounded-stack package regression thread starts")
        .join()
        .expect("package formatting and accessors use bounded stack");
}

#[test]
fn deep_formula_display_and_simplifying_destruction_are_stack_safe() {
    std::thread::Builder::new()
        .name("deep-formula-display-drop".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut formula = Formula::var(0);
            for _ in 0..30_000 {
                formula = Formula::Not(Arc::new(formula));
            }
            let printed = formula.to_string();
            assert!(printed.starts_with("not not not "));
            assert!(printed.ends_with("?0"));

            // Folding this conjunction discards the entire unique Arc chain.
            // Its release is part of substitution/constructor semantics and
            // must use the heap worklist rather than recursive Drop.
            assert_eq!(Formula::False.and(formula), Formula::False);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep formula printing and simplification use bounded stack");
}

#[test]
fn malformed_bound_positions_open_to_recovery() {
    assert!(matches!(&*Ty::Bound(9).open(&[]), Ty::Undecided));
    let opened = Ty::Struct(Row::of(Rest::Bound(9))).open(&[]);
    assert!(matches!(
        &*opened,
        Ty::Struct(Row {
            rest: Rest::More(more),
            ..
        }) if matches!(more.rest, Rest::Undecided)
    ));
}

#[test]
fn standalone_deep_row_destruction_is_stack_safe_for_shared_semantic_dags() {
    std::thread::Builder::new()
        .name("deep-standalone-row-drop".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;

            let mut shared = Arc::new(Row::closed());
            let bottom = Arc::downgrade(&shared);
            for _ in 0..DEPTH {
                shared = Arc::new(Row::of(Rest::More(shared)));
            }
            let top = Arc::downgrade(&shared);

            // The root reaches the same row chain directly and through a type
            // shared by two labels. Dropping one work-list edge must not steal
            // the other edge, while dropping the final edge must reclaim the
            // whole chain without recursing through Rest::More.
            let payload = Arc::new(Ty::Struct(Row::of(Rest::More(shared.clone()))));
            let payload_weak = Arc::downgrade(&payload);
            let root = Row {
                labels: [
                    ("left".into(), RowField::present(payload.clone())),
                    ("right".into(), RowField::present(payload.clone())),
                ]
                .into_iter()
                .collect(),
                rest: Rest::More(shared.clone()),
            };
            drop(payload);
            drop(shared);

            assert!(top.upgrade().is_some());
            assert!(bottom.upgrade().is_some());
            assert!(payload_weak.upgrade().is_some());
            drop(root);
            assert!(top.upgrade().is_none());
            assert!(bottom.upgrade().is_none());
            assert!(payload_weak.upgrade().is_none());
        })
        .expect("the bounded-stack row-drop regression starts")
        .join()
        .expect("standalone semantic rows are destroyed iteratively");
}

#[test]
fn deep_type_opening_is_stack_safe_for_every_nested_semantic_position() {
    std::thread::Builder::new()
        .name("deep-type-open".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let bundle = Bundle::new("deep", Version::new(1, 0, 0)).unwrap();
            let mut mint = Mint::new(bundle);
            let symbol = mint
                .global(None, Namespace::Types, "Layer")
                .expect("a type symbol");
            let mut ty = Arc::new(Ty::Bound(0));
            for depth in 0..30_000 {
                ty = match depth % 3 {
                    0 => Arc::new(Ty::Arrow(Arc::new(Ty::Nat), ty, Row::closed())),
                    1 => Arc::new(Ty::Named {
                        symbol,
                        name: Arc::from("Layer"),
                        args: vec![ty].into(),
                    }),
                    _ => Arc::new(Ty::Struct(Row {
                        labels: [("payload".into(), RowField::present(ty))]
                            .into_iter()
                            .collect(),
                        rest: Rest::Closed,
                    })),
                };
            }
            let opened = ty.open(&[Assigned::Ty(Arc::new(Ty::Nat))]);
            assert!(matches!(&*opened, Ty::Struct(_)));

            let mut row = Row::of(Rest::Bound(0));
            for _ in 0..30_000 {
                row = Row::of(Rest::More(Arc::new(row)));
            }
            let row_ty = Ty::Struct(row);
            let opened_row = row_ty.open(&[Assigned::Ty(Arc::new(Ty::unit()))]);
            assert!(matches!(&*opened_row, Ty::Struct(_)));
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep type and row opening use bounded stack");
}

/// A third reading of the same machinery, and the only new thing it says is
/// where it lives: an arrow carries a row beside its two sides, and a bare
/// `A -> B` is that row closed and empty — which is what pure means, and what
/// the printer writes as nothing at all.
#[test]
fn an_arrow_carries_the_effects_calling_it_may_perform() {
    let nat = || Arc::new(Ty::plain(Ty::Nat));
    // The constructor every position with no effects to put on an arrow goes
    // through, so the empty row is one value rather than six literals.
    let pure = Ty::plain(Ty::pure(nat(), nat()));
    assert_eq!(pure.to_string(), "Nat -> Nat");
    let Ty::Arrow(_, _, effects) = &pure else {
        panic!("expected an arrow");
    };
    assert!(effects.labels.is_empty());
    assert!(matches!(effects.rest, Rest::Closed));

    let performing = Ty::plain(Ty::Arrow(
        nat(),
        nat(),
        Row {
            labels: [("Log".to_string(), RowField::present(Arc::new(Ty::unit())))]
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
    assert_eq!(effects.row(), Some((Shape::Effect, &lacks)));
}

/// A scheme has one index space, not two. The presences take the low
/// positions, `0..presences`, and everything else the rest, `presences..count`
/// — which is what lets a bare [`Ty`] be printed with no scheme beside it to
/// ask which alphabet a letter came out of.
#[test]
fn a_scheme_numbers_every_sort_in_one_space() {
    // A declaration's scheme quantifies its parameters and requires nothing, so
    // it has no presences and its count is the whole of it.
    let body = Arc::new(Ty::plain(Ty::Bound(1)));
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
            ty: Arc::new(Ty::plain(Ty::Bound(1))),
        },
    )]
    .into_iter()
    .collect();
    let body = Arc::new(Ty::Struct(Row {
        labels: fields,
        rest: Rest::Bound(2),
    }));
    let scheme = Scheme::constrained(3, 1, body, Formula::bound(0));
    assert_eq!(scheme.count(), 3);
    assert_eq!(scheme.presences(), 1);

    // Opening it hands every position its value out of one list: the low one is
    // a presence, and the rest are types.
    let fresh = [
        Assigned::Presence(Presence::Var(7)),
        Assigned::Ty(Arc::new(Ty::plain(Ty::Nat))),
        Assigned::Ty(Arc::new(Ty::unit())),
    ];
    let opened = scheme.body().open(&fresh);
    assert_eq!(opened.to_string(), "{ x when ?7: Nat }");
    assert_eq!(scheme.formula().open(&fresh).to_string(), "?7");
}

/// The element accessor looks through a package to the array inside, and
/// answers nothing for anything that is not one.
#[test]
fn the_element_accessor_looks_through_packages() {
    let nat = Arc::new(Ty::Nat);
    let array = Arc::new(Ty::Array(nat.clone()));
    assert!(matches!(array.element(), Some(element) if matches!(**element, Ty::Nat)));
    let packaged = Ty::Package(array.clone());
    assert!(matches!(packaged.element(), Some(element) if matches!(**element, Ty::Nat)));
    assert!(nat.element().is_none());
    assert!(Ty::Package(nat.clone()).element().is_none());
}

#[test]
fn existential_presence_ownership_descends_through_arrays() {
    let element = Arc::new(Ty::Struct(Row {
        labels: [(
            "value".to_string(),
            RowField {
                presence: Presence::Bound(0),
                ty: Arc::new(Ty::Nat),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    }));
    let scheme = Scheme::existential(
        1,
        1,
        [0].into_iter().collect(),
        Arc::new(Ty::Array(element)),
        Formula::bound(0),
    );
    assert!(matches!(&**scheme.body(), Ty::Package(body) if matches!(&**body, Ty::Array(_))));
    assert!(matches!(scheme.formula(), Formula::Owned(0, _)));
}

/// Every position of a scheme is opened at the sort the scheme reserved it for,
/// so a value of another sort never reaches one. Rather than a rule for what
/// cannot happen there is a value that says nothing, which absorbs the way
/// every other unanswerable value does.
#[test]
fn a_value_of_the_wrong_sort_opens_to_nothing() {
    assert_eq!(
        Assigned::Ty(Arc::new(Ty::plain(Ty::Nat))).presence(),
        Presence::Undecided
    );
    assert_eq!(
        Assigned::Row(Arc::new(Row::closed())).presence(),
        Presence::Undecided
    );
    assert_eq!(
        Assigned::Presence(Presence::Present).presence(),
        Presence::Present
    );
}

/// A rigid is a leaf: it is not opened, because nothing supplies a value for
/// one. What a scheme quantified is a [`Ty::Bound`], and a rigid is what an
/// annotation's own variable stands for while its body is being checked.
#[test]
fn a_rigid_is_a_leaf_that_opening_leaves_alone() {
    let rigid = Arc::new(Ty::plain(Ty::Rigid {
        id: 4,
        name: "r".into(),
    }));
    let opened = rigid.open(&[Assigned::Ty(Arc::new(Ty::plain(Ty::Nat)))]);
    assert!(matches!(&*opened, Ty::Rigid { id: 4, .. }));

    // A sum's rest goes the same way, and prints as its name either side of
    // the substitution.
    let cases = Arc::new(Ty::plain(Ty::Sum(Row::of(Rest::Rigid {
        id: 5,
        name: "s".into(),
    }))));
    assert_eq!(cases.to_string(), "| ..'s");
    let opened = cases.open(&[Assigned::Ty(Arc::new(Ty::plain(Ty::Nat)))]);
    assert_eq!(opened.to_string(), "| ..'s");
}

/// Hidden types are one type up to the names of their variables: bound
/// occurrences pair through their binders, innermost first, and a free
/// occurrence is equal to itself alone. Opening replaces a binder's
/// occurrences and leaves a nested binder of the same number to itself.
#[test]
fn hidden_types_compare_through_their_binders_and_open_with_shadowing() {
    let var = |binder: u32| {
        Arc::new(Ty::HiddenVar {
            binder,
            name: "a".into(),
        })
    };
    let hidden = |binder: u32, body: Arc<Ty>| {
        Arc::new(Ty::Hidden {
            binder,
            name: "a".into(),
            body,
        })
    };
    let arrow = |from: Arc<Ty>, to: Arc<Ty>| Arc::new(Ty::Arrow(from, to, Row::closed()));

    let left = hidden(1, arrow(var(1), var(1)));
    let right = hidden(2, arrow(var(2), var(2)));
    assert!(same_finite_syntax(&left, &right));
    let mixed = hidden(3, arrow(var(3), Arc::new(Ty::Nat)));
    assert!(!same_finite_syntax(&left, &mixed));
    assert!(same_finite_syntax(&var(7), &var(7)));
    assert!(!same_finite_syntax(&var(7), &var(8)));
    // The same allocation under two binders still pairs its variables.
    let shared = arrow(var(1), var(1));
    assert!(same_finite_syntax(
        &hidden(1, shared.clone()),
        &hidden(2, arrow(var(2), var(2)))
    ));

    let nested_left = hidden(1, hidden(2, arrow(var(1), var(2))));
    let nested_right = hidden(3, hidden(4, arrow(var(3), var(4))));
    assert!(same_finite_syntax(&nested_left, &nested_right));
    let crossed = hidden(3, hidden(4, arrow(var(4), var(3))));
    assert!(!same_finite_syntax(&nested_left, &crossed));

    let body = arrow(var(1), hidden(1, arrow(var(1), Arc::new(Ty::Nat))));
    let opened = open_hidden(&body, 1, &Arc::new(Ty::String));
    assert_eq!(opened.to_string(), "String -> hide 'a => 'a -> Nat");
    assert_eq!(
        hidden(1, body.clone()).to_string(),
        "hide 'a => 'a -> hide 'a => 'a -> Nat"
    );
}

#[test]
fn target_domains_have_exact_bounds() {
    use ruddy::types::Domains;
    assert_eq!(Domains::default(), Domains::Js53);
    for domains in Domains::ALL {
        assert_eq!(Domains::from_name(domains.name()), Some(domains));
        assert_eq!(domains.nat().bits, domains.bits());
        assert!(!domains.nat().signed);
        assert!(domains.int().signed);
        assert_eq!(domains.nat().min, "0");
        assert!(domains.holds_natural(0));
        assert!(domains.holds_integer(0));
    }
    assert_eq!(Domains::from_name("48"), None);
    assert_eq!(Domains::Js53.nat().max, "9007199254740991");
    assert_eq!(Domains::Js53.int().min, "-9007199254740991");
    assert!(Domains::Js53.holds_natural(9007199254740991));
    assert!(!Domains::Js53.holds_natural(9007199254740992));
    assert!(Domains::Js53.holds_integer(-9007199254740991));
    assert!(!Domains::Js53.holds_integer(-9007199254740992));
    assert_eq!(Domains::Bits32.nat().max, "4294967295");
    assert_eq!(Domains::Bits32.int().min, "-2147483648");
    assert_eq!(Domains::Bits32.int().max, "2147483647");
    assert!(!Domains::Bits32.holds_natural(4294967296));
    assert!(!Domains::Bits32.holds_integer(2147483648));
    assert!(!Domains::Bits32.holds_integer(-2147483649));
    assert_eq!(Domains::Bits64.nat().max, "18446744073709551615");
    assert_eq!(Domains::Bits64.int().min, "-9223372036854775808");
    assert!(Domains::Bits64.holds_natural(u64::MAX));
    assert!(Domains::Bits64.holds_integer(i64::MIN));
}
