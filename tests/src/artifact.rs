//! Tests for the span-free, canonical bundle artifact.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};
use ruddy::{
    artifact::{
        self, Artifact, Block, Callee, End, Formula, Global, Instr, Lir, Literal, Op, Param,
        Presence, Rep, Rest, Row, RowField, Scheme, Type,
    },
    inference, ir, lir, parse, patterns,
    symbol::{Bundle, Mint, Namespace, Version},
    token,
    tracking::{FileManager, Span},
    types,
};

fn compiled(source: &str) -> (Mint, ir::Program, inference::Output, lir::Output) {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("tests", Version::new(0, 1, 0)).unwrap();
    let mut mint = Mint::new(bundle);
    let mut program = ir::build(&mut mint, parsed.stmts).program;
    let inferred = inference::infer(&mint, &mut program, inference::Trace::Off);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    let checked = patterns::check(&program, &inferred);
    assert!(checked.errors.is_empty(), "{:#?}", checked.errors);
    let lowered = lir::lower(&mint, &program, inferred.semantics());
    (mint, program, inferred, lowered)
}

fn built(source: &str) -> Artifact {
    let (mint, program, inferred, lowered) = compiled(source);
    Artifact::build(&mint, &program, inferred.semantics(), &lowered)
}

/// An artifact exporting one hand-built semantic scheme as its only value, for
/// semantic states no source program can conveniently reach.
fn exporting(mint: &Mint, scheme: &types::Scheme) -> Artifact {
    let name = mint.bundle().name().to_string();
    let version = mint.bundle().version().to_string();
    Artifact {
        header: artifact::Header {
            identity: artifact::Identity {
                name: name.clone(),
                version: version.clone(),
            },
            dependencies: Vec::new(),
            values: vec![artifact::Value {
                name: format!("{name}@{version}::value"),
                scheme: artifact::export_scheme(mint, scheme),
            }],
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
}

fn plain(ty: Type) -> Type {
    ty
}
fn unit() -> Type {
    Type::Struct(Row {
        labels: Vec::new(),
        rest: Rest::Closed,
    })
}

fn field(presence: Presence, ty: Type) -> RowField {
    RowField { presence, ty }
}

fn rich(ty: Type) -> Type {
    ty
}

fn row(rest: Rest) -> Row {
    Row {
        labels: vec![("label".to_string(), field(Presence::Present, Type::Boolean))],
        rest,
    }
}

/// A hand-built artifact covers semantic states that a source program cannot
/// conveniently preserve through inference, such as every normalized variable
/// form and every LIR representation.
fn model_artifact() -> Artifact {
    let formulas = vec![
        Formula::True,
        Formula::False,
        Formula::Var(0),
        Formula::Bound(1),
        Formula::Not(Box::new(Formula::Var(2))),
        Formula::And(Box::new(Formula::True), Box::new(Formula::False)),
        Formula::Or(Box::new(Formula::Var(3)), Box::new(Formula::Bound(4))),
        Formula::Iff(Box::new(Formula::True), Box::new(Formula::Var(5))),
        Formula::Xor(Box::new(Formula::False), Box::new(Formula::Bound(6))),
    ];
    let types = vec![
        Type::Struct(Row {
            labels: vec![
                (
                    "present".to_string(),
                    field(
                        Presence::Present,
                        Type::Struct(Row {
                            labels: Vec::new(),
                            rest: Rest::Closed,
                        }),
                    ),
                ),
                ("absent".to_string(), field(Presence::Absent, Type::Nat)),
                ("var".to_string(), field(Presence::Var(1), Type::Int)),
                ("bound".to_string(), field(Presence::Bound(2), Type::Real)),
                (
                    "undecided".to_string(),
                    field(Presence::Undecided, Type::String),
                ),
            ],
            rest: Rest::Closed,
        }),
        rich(Type::Nat),
        rich(Type::Int),
        rich(Type::Real),
        rich(Type::String),
        rich(Type::Boolean),
        rich(Type::Arrow(
            Box::new(plain(Type::Var(7))),
            Box::new(plain(Type::Bound(8))),
            row(Rest::More(Box::new(row(Rest::Rigid {
                id: 9,
                name: "tail".to_string(),
            })))),
        )),
        rich(Type::Sum(row(Rest::Closed))),
        rich(Type::Sum(row(Rest::Var(10)))),
        rich(Type::Sum(row(Rest::Bound(11)))),
        rich(Type::Sum(row(Rest::Undecided))),
        rich(Type::Var(12)),
        rich(Type::Bound(13)),
        rich(Type::Rigid {
            id: 14,
            name: "rigid".to_string(),
        }),
        rich(Type::Named {
            name: "other@1.2.3::T".to_string(),
            args: vec![plain(Type::Nat), plain(Type::Undecided)],
        }),
        rich(Type::Undecided),
    ];
    let values = types
        .into_iter()
        .enumerate()
        .map(|(index, body)| artifact::Value {
            name: format!("bundle@1.0.0::value-{index}"),
            scheme: Scheme {
                count: 15,
                presences: 7,
                existentials: Vec::new(),
                formula: formulas[index % formulas.len()].clone(),
                body,
            },
        })
        .collect();
    let yielded = Block {
        instrs: Vec::new(),
        end: End::Yield(98),
    };
    let thrown = Block {
        instrs: Vec::new(),
        end: End::Throw { tag: 97, value: 96 },
    };
    let ops = vec![
        Op::Const(Literal::Natural(u64::MAX)),
        Op::Const(Literal::Integer(i64::MIN)),
        Op::Const(Literal::Real(f64::NAN.to_bits())),
        Op::Const(Literal::String("quote \" and newline\n".to_string())),
        Op::Const(Literal::Boolean(true)),
        Op::Neg(1),
        Op::Not(2),
        Op::And { left: 1, right: 2 },
        Op::Or { left: 1, right: 2 },
        Op::Xor { left: 1, right: 2 },
        Op::Add { left: 1, right: 2 },
        Op::Sub { left: 1, right: 2 },
        Op::Mul { left: 1, right: 2 },
        Op::Div { left: 1, right: 2 },
        Op::Struct(vec![
            (artifact::FieldKey::Named("first".to_string()), 1),
            (artifact::FieldKey::Named("second".to_string()), 2),
            (artifact::FieldKey::UnnamedOperation, 3),
        ]),
        Op::Merge(vec![1, 2]),
        Op::Project {
            base: 1,
            field: artifact::FieldKey::Named("field".to_string()),
        },
        Op::Tag {
            name: "Case".to_string(),
            payload: None,
        },
        Op::Tag {
            name: "Other".to_string(),
            payload: Some(3),
        },
        Op::Payload(3),
        Op::Closure {
            func: 0,
            captures: vec![1, 2],
        },
        Op::Call {
            callee: Callee::Direct(0),
            args: vec![1],
        },
        Op::Call {
            callee: Callee::Indirect(2),
            args: vec![3],
        },
        Op::Global {
            target: "dep@1.0.0::value".to_string(),
        },
        Op::NewTag,
        Op::Catch {
            tag: 1,
            body: Box::new(thrown.clone()),
        },
        Op::SwitchTag {
            on: 1,
            cases: vec![artifact::TagCase {
                name: "A".to_string(),
                block: yielded.clone(),
            }],
            fallback: Some(Box::new(thrown.clone())),
        },
        Op::SwitchPrim {
            on: 1,
            cases: vec![artifact::PrimCase {
                value: Literal::Boolean(false),
                block: yielded.clone(),
            }],
            fallback: None,
        },
        Op::SwitchPresence {
            on: 1,
            field: "x".to_string(),
            present: Box::new(yielded.clone()),
            absent: Box::new(thrown.clone()),
        },
        Op::SwitchRest {
            on: 1,
            fields: vec!["x".to_string(), "y".to_string()],
            none: Box::new(yielded.clone()),
            some: Box::new(thrown.clone()),
        },
    ];
    let reps = [
        Rep::Nat,
        Rep::Int,
        Rep::Real,
        Rep::String,
        Rep::Boolean,
        Rep::Unit,
        Rep::Struct,
        Rep::Sum,
        Rep::Fn,
        Rep::Any,
    ];
    Artifact {
        header: artifact::Header {
            identity: artifact::Identity {
                name: "bundle".to_string(),
                version: "1.0.0".to_string(),
            },
            dependencies: vec![
                artifact::Dependency {
                    name: "base".to_string(),
                    version: "2.1.0".to_string(),
                },
                artifact::Dependency {
                    name: "support".to_string(),
                    version: "3.0.0-beta.1".to_string(),
                },
            ],
            values,
            types: vec![
                artifact::DeclaredType {
                    name: "bundle@1.0.0::Type".to_string(),
                    params: vec![artifact::Parameter {
                        sense: artifact::Sense::Type,
                        lacks: vec!["field".to_string()],
                        relevant: true,
                    }],
                    scheme: Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: Formula::True,
                        body: plain(unit()),
                    },
                },
                artifact::DeclaredType {
                    name: "bundle@1.0.0::Fields".to_string(),
                    params: vec![artifact::Parameter {
                        sense: artifact::Sense::Fields,
                        lacks: vec!["field".to_string()],
                        relevant: true,
                    }],
                    scheme: Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: Formula::True,
                        body: rich(unit()),
                    },
                },
                artifact::DeclaredType {
                    name: "bundle@1.0.0::Cases".to_string(),
                    params: vec![artifact::Parameter {
                        sense: artifact::Sense::Cases,
                        lacks: vec!["Case".to_string()],
                        relevant: false,
                    }],
                    scheme: Scheme {
                        count: 2,
                        presences: 1,
                        existentials: Vec::new(),
                        formula: Formula::Var(0),
                        body: plain(Type::Sum(row(Rest::Closed))),
                    },
                },
                artifact::DeclaredType {
                    name: "bundle@1.0.0::Effects".to_string(),
                    params: vec![artifact::Parameter {
                        sense: artifact::Sense::Effects,
                        lacks: vec!["Log".to_string()],
                        relevant: true,
                    }],
                    scheme: Scheme {
                        count: 3,
                        presences: 2,
                        existentials: Vec::new(),
                        formula: Formula::Bound(1),
                        body: plain(Type::Arrow(
                            Box::new(plain(Type::Nat)),
                            Box::new(plain(unit())),
                            row(Rest::Closed),
                        )),
                    },
                },
            ],
            effects: vec![
                artifact::DeclaredEffect {
                    name: "bundle@1.0.0::Log".to_string(),
                    identity: Some(artifact::EffectIdentity {
                        name: "Log".to_string(),
                        interface: "write".to_string(),
                    }),
                    kind: artifact::EffectKind::Operations(vec![artifact::Operation {
                        selector: artifact::OperationSelector::Named("write".to_string()),
                        from: plain(Type::Nat),
                        to: plain(unit()),
                    }]),
                },
                artifact::DeclaredEffect {
                    name: "bundle@1.0.0::Alias".to_string(),
                    identity: None,
                    kind: artifact::EffectKind::Alias(vec!["bundle@1.0.0::Log".to_string()]),
                },
            ],
        },
        lir: Lir {
            externs: vec![artifact::Extern {
                name: "bundle@1.0.0::consoleLog".to_string(),
                target: "console.log".to_string(),
                rep: Rep::Fn,
            }],
            functions: vec![artifact::Function {
                name: "f".to_string(),
                params: reps
                    .iter()
                    .enumerate()
                    .map(|(temp, rep)| Param {
                        temp: temp as u32,
                        rep: *rep,
                    })
                    .collect(),
                body: Block {
                    instrs: ops
                        .into_iter()
                        .enumerate()
                        .map(|(temp, op)| Instr {
                            temp: temp as u32,
                            rep: reps[temp % reps.len()],
                            op,
                        })
                        .collect(),
                    end: End::Ret(0),
                },
            }],
            globals: vec![Global {
                name: "bundle@1.0.0::g".to_string(),
                body: thrown,
            }],
        },
    }
}

#[test]
fn existential_presence_ownership_round_trips_and_is_validated() {
    let mut artifact = model_artifact();
    let scheme = &mut artifact.header.values[0].scheme;
    scheme.existentials = vec![0, 3];
    scheme.body = Type::Package(Box::new(Type::Struct(Row {
        labels: vec![("a".into(), field(Presence::Bound(0), Type::Nat))],
        rest: Rest::More(Box::new(Row {
            labels: vec![("b".into(), field(Presence::Bound(3), Type::Nat))],
            rest: Rest::Closed,
        })),
    })));
    scheme.formula = Formula::Owned(
        0,
        Box::new(Formula::Iff(
            Box::new(Formula::Bound(1)), // a universal in this scheme
            Box::new(Formula::Or(
                Box::new(Formula::Bound(0)),
                Box::new(Formula::Bound(3)),
            )),
        )),
    );
    let printed = assert_round_trip(&artifact);
    assert!(printed.contains("(existentials 0 3)"), "{printed}");

    assert_malformed(&printed.replacen("(existentials 0 3)", "(existentials 3 0)", 1));
    assert_malformed(&printed.replacen("(existentials 0 3)", "(existentials 0 0)", 1));
    assert_malformed(&printed.replacen("(existentials 0 3)", "(existentials 0 7)", 1));
    assert_malformed(&printed.replacen("(owned 0", "(owned 1", 1));
    assert_malformed(&printed.replacen("(bound 1)", "(bound 7)", 1));
    assert_malformed(&printed.replacen("(owned 0", "", 1).replacen(")", "", 1));
    // Presence positions share the scheme's quantifier space; accepting more
    // presence slots than total slots would let malformed bounds pass all
    // subsequent per-presence checks.
    assert_malformed(&printed.replacen("(scheme 15 7", "(scheme 6 7", 1));
}

#[test]
fn owned_disjunction_with_nested_conjunction_round_trips() {
    let mut artifact = model_artifact();
    let scheme = &mut artifact.header.values[0].scheme;
    scheme.existentials = vec![0, 1];
    scheme.body = Type::Package(Box::new(Type::Struct(Row {
        labels: vec![
            ("a".into(), field(Presence::Bound(0), Type::Nat)),
            ("b".into(), field(Presence::Bound(1), Type::Nat)),
        ],
        rest: Rest::Closed,
    })));
    scheme.formula = Formula::Owned(
        0,
        Box::new(Formula::Or(
            Box::new(Formula::And(
                Box::new(Formula::Bound(0)),
                Box::new(Formula::Bound(1)),
            )),
            Box::new(Formula::Not(Box::new(Formula::Bound(0)))),
        )),
    );

    let printed = assert_round_trip(&artifact);
    assert!(printed.contains("(owned 0 (or (and"), "{printed}");

    // The same conjunction directly beneath Owned is partitionable and is not
    // compiler-canonical, unlike the conjunction nested in the disjunction.
    let malformed = printed.replacen("(or (and", "(and (and", 1);
    assert_malformed(&malformed);
}

#[test]
fn broad_existential_package_validation_scales() {
    const WIDTH: u32 = 4_096;
    let mut artifact = model_artifact();
    let scheme = &mut artifact.header.values[0].scheme;
    scheme.count = WIDTH + 1;
    scheme.presences = WIDTH + 1;
    scheme.existentials = (0..WIDTH).collect();
    scheme.body = Type::Package(Box::new(Type::Struct(Row {
        labels: (0..WIDTH)
            .map(|index| {
                (
                    format!("slot{index}"),
                    field(Presence::Bound(index), Type::Nat),
                )
            })
            .collect(),
        rest: Rest::Closed,
    })));

    // Include every existential and one universal in an indivisible owned
    // proposition. This exercises both broad body-owner membership checks and
    // the mixed formula scan without creating a recursively deep formula.
    let mut formulas: Vec<_> = (0..=WIDTH).map(Formula::Bound).collect();
    while formulas.len() > 1 {
        formulas = formulas
            .chunks(2)
            .map(|pair| match pair {
                [left, right] => Formula::Or(Box::new(left.clone()), Box::new(right.clone())),
                [only] => only.clone(),
                _ => unreachable!(),
            })
            .collect();
    }
    scheme.formula = Formula::Owned(0, Box::new(formulas.pop().unwrap()));

    let printed = artifact.print();
    let parsed = Artifact::try_parse(&printed).expect("broad package remains valid");
    assert_eq!(
        parsed.header.values[0].scheme.existentials.len(),
        WIDTH as usize
    );
}

#[test]
fn broad_semantic_scheme_converts_to_artifact_linearly() {
    const WIDTH: u32 = 4_096;
    let mint = Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap());
    let labels = (0..WIDTH)
        .map(|index| {
            (
                format!("slot{index}"),
                types::RowField {
                    presence: types::Presence::Bound(index),
                    ty: Rc::new(types::Ty::Nat),
                },
            )
        })
        .collect();
    let body = Rc::new(types::Ty::Package(Rc::new(types::Ty::Struct(types::Row {
        labels,
        rest: types::Rest::Closed,
    }))));
    let formula = types::Formula::any((0..WIDTH).map(types::Formula::bound));
    let scheme = types::Scheme::existential(
        WIDTH,
        WIDTH,
        (0..WIDTH).collect::<IndexSet<_>>(),
        body,
        formula,
    );

    let artifact = exporting(&mint, &scheme);
    let value = &artifact.header.values[0];
    assert_eq!(value.scheme.existentials.len(), WIDTH as usize);
    assert!(matches!(value.scheme.formula, Formula::Owned(0, _)));
    Artifact::try_parse(&artifact.print()).expect("broad semantic scheme round trips");
}

fn assert_round_trip(value: &Artifact) -> String {
    let printed = value.print();
    let parsed = Artifact::parse(&printed);
    assert_eq!(parsed, *value);
    assert_eq!(artifact::print(&parsed), printed);
    assert_eq!(artifact::parse(&printed), parsed);
    assert_eq!(Artifact::try_parse(&printed), Ok(parsed.clone()));
    assert_eq!(artifact::try_parse(&printed), Ok(parsed.clone()));
    assert_eq!(artifact::text::try_parse(&printed), Ok(parsed));
    printed
}

fn assert_parse_error(error: &artifact::ParseError) {
    let public_error: &dyn std::error::Error = error;
    assert!(!error.message().is_empty());
    assert!(!public_error.to_string().is_empty());
}

fn assert_malformed(text: &str) {
    for error in [
        Artifact::try_parse(text).expect_err("malformed text unexpectedly parsed"),
        artifact::try_parse(text).expect_err("malformed text unexpectedly parsed"),
        artifact::text::try_parse(text).expect_err("malformed text unexpectedly parsed"),
    ] {
        assert_parse_error(&error);
    }
    assert!(
        catch_unwind(AssertUnwindSafe(|| artifact::parse(text))).is_err(),
        "{text:?} unexpectedly parsed"
    );
}

fn assert_bad_replacement(text: &str, from: &str, to: &str) {
    let malformed = text.replacen(from, to, 1);
    assert_ne!(malformed, text, "missing test path {from:?}");
    assert_malformed(&malformed);
}

/// Replace the balanced list beginning at `needle`. This lets malformed model
/// tests put an atom where a whole nested list normally sits without depending
/// on the pretty-printer's line wrapping.
fn replace_balanced(text: &str, needle: &str, replacement: &str) -> String {
    let start = text
        .find(needle)
        .expect("missing balanced replacement path");
    assert_eq!(text.as_bytes()[start], b'(');
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut end = None;
    for (offset, character) in text[start..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset + character.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("replacement path is not a balanced list");
    format!("{}{}{}", &text[..start], replacement, &text[end..])
}

/// Collapse layout whitespace without touching quoted strings. Malformed-input
/// tests target grammar tags rather than the pretty-printer's line choices.
fn compact(text: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut separating = false;
    for character in text.chars() {
        if quoted {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
        } else if character == '"' {
            if separating && !out.is_empty() {
                out.push(' ');
            }
            separating = false;
            quoted = true;
            out.push(character);
        } else if character.is_whitespace() {
            separating = true;
        } else {
            if separating && !out.is_empty() {
                out.push(' ');
            }
            separating = false;
            out.push(character);
        }
    }
    out
}

#[test]
fn nested_existential_result_boundaries_survive_artifact_text() {
    let artifact = built(
        "extern nested: ((Nat ->\n\
         { left when 'p: Nat, right when 'q: Nat }) -> Nat) -> Nat\n\
         where 'p != 'q = \"host.nested\"\n",
    );
    let value = artifact
        .header
        .values
        .iter()
        .find(|value| value.name.ends_with("::nested"))
        .expect("the nested extern is published");
    let Type::Arrow(outer_from, _, _) = &value.scheme.body else {
        panic!(
            "expected the outer callback arrow: {:#?}",
            value.scheme.body
        );
    };
    let Type::Arrow(callback_from, _, _) = &**outer_from else {
        panic!("expected the nested callback arrow: {outer_from:#?}");
    };
    let Type::Arrow(_, callback_result, _) = &**callback_from else {
        panic!("expected the callback-producing arrow: {callback_from:#?}");
    };
    assert!(
        matches!(&**callback_result, Type::Package(body) if matches!(&**body, Type::Struct(_))),
        "the package must stay at the innermost result: {callback_result:#?}"
    );

    let printed = assert_round_trip(&artifact);
    let reparsed = Artifact::parse(&printed);
    let reparsed = reparsed
        .header
        .values
        .iter()
        .find(|value| value.name.ends_with("::nested"))
        .expect("the nested extern survives decoding");
    assert_eq!(reparsed.scheme.existentials, value.scheme.existentials);
    assert!(
        matches!(reparsed.scheme.formula, Formula::Owned(0, _)),
        "the guarantee belongs to the exact nested result package: {:#?}",
        reparsed.scheme.formula
    );
    assert!(printed.contains("(existentials"), "{printed}");
    assert!(printed.contains("(owned 0 (xor"), "{printed}");

    let invalid = printed.replacen("(owned 0", "(owned 1", 1);
    let error = Artifact::try_parse(&invalid).expect_err("owner 1 names no package");
    assert_eq!(
        error.message(),
        "formula package owner is outside scheme body"
    );
}

#[test]
fn mixed_universal_input_to_existential_result_guarantee_round_trips() {
    let artifact = built(
        "extern relate: { input when 'u: Nat } ->\n\
         { result when 'e: Nat } where 'u = 'e = \"host.relate\"\n",
    );
    let value = artifact
        .header
        .values
        .iter()
        .find(|value| value.name.ends_with("::relate"))
        .expect("the extern is published");
    assert_eq!(value.scheme.existentials.len(), 1);
    assert!(
        matches!(
            value.scheme.formula,
            Formula::Owned(0, ref inner) if matches!(&**inner, Formula::Iff(..))
        ),
        "the indivisible mixed guarantee belongs to its result package: {:#?}",
        value.scheme.formula
    );

    let printed = assert_round_trip(&artifact);
    assert!(printed.contains("(owned 0 (iff"), "{printed}");
}

#[test]
fn a_compiled_bundle_round_trips_through_canonical_text() {
    let artifact = built(
        "type Box 'a = { value: 'a }\n\
         effect Log = { write: Nat -> () }\n\
         effect Console = !Log\n\
         let id = fn x => x\n\
         let main = fn n => handle id n with | !Log.write x => {} end\n",
    );
    assert_eq!(artifact.header.identity.name, "tests");
    assert_eq!(artifact.header.identity.version, "0.1.0");
    assert!(artifact.header.dependencies.is_empty());
    assert_eq!(artifact.header.values.len(), 2);
    assert_eq!(artifact.header.types.len(), 1);
    assert_eq!(artifact.header.effects.len(), 2);
    assert!(artifact.header.values[0].name.starts_with("tests@0.1.0::"));
    assert!(
        artifact
            .lir
            .globals
            .iter()
            .all(|global| global.name.contains('@'))
    );

    let printed = assert_round_trip(&artifact);
    assert!(
        printed.starts_with("(artifact\n  (header"),
        "artifact should use the canonical pretty layout:\n{printed}"
    );
    assert!(
        printed.lines().count() > 10,
        "a nontrivial artifact should be laid out across lines:\n{printed}"
    );
    assert!(
        !printed.contains("<test>"),
        "source paths must not cross the disk boundary"
    );
    assert!(
        printed.contains("(selector named \"write\")"),
        "named selectors use the strict selector subform: {printed}"
    );
    assert!(
        !printed.contains("Span"),
        "source spans must not cross the disk boundary"
    );
}

#[test]
fn compiler_externs_cross_the_artifact_boundary_without_symbols_or_spans() {
    let artifact = built(
        "module Host =\n\
         extern log : String -> () = \"console.log\"\n\
         end\n\
         let main = Host::log \"hello\"\n",
    );

    assert_eq!(artifact.header.values.len(), 2);
    assert_eq!(artifact.header.values[0].name, "tests@0.1.0::Host::log");
    assert_eq!(
        artifact.lir.externs,
        [artifact::Extern {
            name: "tests@0.1.0::Host::log".to_string(),
            target: "console.log".to_string(),
            rep: Rep::Fn,
        }]
    );
    let printed = assert_round_trip(&artifact);
    assert!(
        printed.contains("(extern \"tests@0.1.0::Host::log\" (target \"console.log\") fn)"),
        "{printed}"
    );
    assert!(!printed.contains("Span"), "{printed}");
    assert!(!printed.contains("Symbol"), "{printed}");
}

#[test]
fn canonical_pretty_layout_is_pinned_at_its_unicode_width_boundary() {
    let empty = |name: String| Artifact {
        header: artifact::Header {
            identity: artifact::Identity {
                name,
                version: "1".to_string(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };

    assert_eq!(
        empty("界".repeat(12)).print(),
        "(artifact\n\
         \x20 (header\n\
         \x20   (identity \"界界界界界界界界界界界界\" \"1\")\n\
         \x20   (dependencies)\n\
         \x20   (values)\n\
         \x20   (types)\n\
         \x20   (effects))\n\
         \x20 (lir (externs) (functions) (globals)))\n"
    );
    assert_eq!(
        empty("界".repeat(13)).print(),
        "(artifact\n\
         \x20 (header\n\
         \x20   (identity \"界界界界界界界界界界界界界\" \"1\")\n\
         \x20   (dependencies)\n\
         \x20   (values)\n\
         \x20   (types)\n\
         \x20   (effects))\n\
         \x20 (lir (externs) (functions) (globals)))\n"
    );
}

#[test]
fn unnamed_operation_selectors_round_trip_strictly() {
    let artifact = built(
        "effect Log = String -> ()\n\
         let main = fn message => handle !Log message with | !Log text => () end",
    );
    let printed = assert_round_trip(&artifact);
    assert!(printed.contains("(selector unnamed)"), "{printed}");
    let effect = &artifact.header.effects[0];
    let artifact::EffectKind::Operations(operations) = &effect.kind else {
        panic!("expected operations")
    };
    assert_eq!(operations[0].selector, artifact::OperationSelector::Unnamed);
}

#[test]
fn dependencies_round_trip_in_canonical_text() {
    let artifact = model_artifact();
    let printed = assert_round_trip(&artifact);

    assert!(printed.contains("(dependencies\n      (dependency \"base\" \"2.1.0\")"));
    assert_eq!(
        Artifact::parse(&printed).header.dependencies,
        artifact.header.dependencies
    );
}

#[test]
fn every_semantic_type_scheme_and_lir_variant_round_trips() {
    assert_round_trip(&model_artifact());
}

#[test]
fn building_translates_every_compiler_semantic_and_lir_variant() {
    let (mut mint, program, inferred, _) = compiled(
        "type OpenCases 'r = #A | ..'r\n\
         type Runner 'e = Nat -> Nat + ..'e\n\
         let value = 0\n",
    );
    let symbol = *program.terms.keys().next().expect("source has one term");
    let type_symbol = *program
        .types
        .keys()
        .next()
        .expect("source has declared types");
    mint.register_external(type_symbol, "dependency@1.0.0::OpenCases");
    let local_symbol = mint.local(None, Namespace::Terms, "wildcard");
    let compiler_plain = |core| Rc::new(types::Ty::plain(core));
    let compiler_field = |presence, core| types::RowField {
        presence,
        ty: compiler_plain(core),
    };

    let mut fields = IndexMap::new();
    fields.insert(
        "int".to_string(),
        compiler_field(types::Presence::Var(0), types::Ty::Int),
    );
    fields.insert(
        "real".to_string(),
        compiler_field(types::Presence::Undecided, types::Ty::Real),
    );
    fields.insert(
        "boolean".to_string(),
        compiler_field(types::Presence::Absent, types::Ty::Boolean),
    );
    fields.insert(
        "var".to_string(),
        compiler_field(types::Presence::Present, types::Ty::Var(1)),
    );
    fields.insert(
        "rigid".to_string(),
        compiler_field(
            types::Presence::Bound(2),
            types::Ty::Rigid {
                id: 3,
                name: Rc::from("rigid"),
            },
        ),
    );
    fields.insert(
        "var-rest".to_string(),
        compiler_field(
            types::Presence::Present,
            types::Ty::Sum(types::Row::of(types::Rest::Var(4))),
        ),
    );
    fields.insert(
        "bound-rest".to_string(),
        compiler_field(
            types::Presence::Present,
            types::Ty::Sum(types::Row::of(types::Rest::Bound(5))),
        ),
    );
    fields.insert(
        "undecided-rest".to_string(),
        compiler_field(
            types::Presence::Present,
            types::Ty::Sum(types::Row::of(types::Rest::Undecided)),
        ),
    );
    fields.insert(
        "named".to_string(),
        compiler_field(
            types::Presence::Present,
            types::Ty::Named {
                symbol: type_symbol,
                name: Rc::from("OpenCases"),
                args: vec![compiler_plain(types::Ty::Undecided)].into(),
            },
        ),
    );

    let mut inner_labels = IndexMap::new();
    inner_labels.insert(
        "effect".to_string(),
        compiler_field(types::Presence::Present, types::Ty::String),
    );
    let inner_row = types::Row {
        labels: inner_labels,
        rest: types::Rest::Rigid {
            id: 5,
            name: Rc::from("effects"),
        },
    };
    let effects = types::Row::of(types::Rest::More(Rc::new(inner_row)));
    fields.insert(
        "arrow".to_string(),
        compiler_field(
            types::Presence::Present,
            types::Ty::Arrow(
                compiler_plain(types::Ty::Int),
                compiler_plain(types::Ty::Real),
                effects,
            ),
        ),
    );
    let body = Rc::new(types::Ty::Struct(types::Row {
        labels: fields,
        rest: types::Rest::Closed,
    }));
    let formula = types::Formula::Iff(
        Rc::new(types::Formula::False),
        Rc::new(types::Formula::Xor(
            Rc::new(types::Formula::Atom(types::Atom::Var(6))),
            Rc::new(types::Formula::And(
                Rc::new(types::Formula::True),
                Rc::new(types::Formula::Or(
                    Rc::new(types::Formula::Atom(types::Atom::Bound(0))),
                    Rc::new(types::Formula::Not(Rc::new(types::Formula::False))),
                )),
            )),
        )),
    );
    let custom = types::Scheme::constrained(7, 3, body, formula);

    let span = Span::default();
    let nested = |kind| lir::Block {
        instrs: Vec::new(),
        end: lir::Terminator { span, kind },
    };
    let yielded = nested(lir::End::Yield(90));
    let thrown = nested(lir::End::Throw { tag: 91, value: 92 });
    let ops = vec![
        lir::Op::Const(ir::Literal::Natural(u64::MAX)),
        lir::Op::Const(ir::Literal::Integer(i64::MIN)),
        lir::Op::Const(ir::Literal::Real(-0.0)),
        lir::Op::Const(ir::Literal::String("text".to_string())),
        lir::Op::Const(ir::Literal::Boolean(false)),
        lir::Op::Neg(1),
        lir::Op::Not(2),
        lir::Op::And { left: 1, right: 2 },
        lir::Op::Or { left: 1, right: 2 },
        lir::Op::Xor { left: 1, right: 2 },
        lir::Op::Add { left: 1, right: 2 },
        lir::Op::Sub { left: 1, right: 2 },
        lir::Op::Mul { left: 1, right: 2 },
        lir::Op::Div { left: 1, right: 2 },
        lir::Op::Struct(IndexMap::from([
            (lir::FieldKey::Named("x".to_string()), 1),
            (lir::FieldKey::UnnamedOperation, 2),
        ])),
        lir::Op::Merge(vec![1, 2]),
        lir::Op::Project {
            base: 1,
            field: lir::FieldKey::Named("x".to_string()),
        },
        lir::Op::Tag {
            name: "None".to_string(),
            payload: None,
        },
        lir::Op::Tag {
            name: "Some".to_string(),
            payload: Some(2),
        },
        lir::Op::Payload(2),
        lir::Op::Closure {
            func: 0,
            captures: vec![1, 2],
        },
        lir::Op::Call {
            callee: lir::Callee::Direct(0),
            args: vec![1],
        },
        lir::Op::Call {
            callee: lir::Callee::Indirect(2),
            args: vec![3],
        },
        lir::Op::Global {
            symbol,
            name: "value".to_string(),
        },
        lir::Op::NewTag,
        lir::Op::Catch {
            tag: 1,
            body: Box::new(thrown.clone()),
        },
        lir::Op::SwitchTag {
            on: 1,
            cases: vec![lir::TagCase {
                name: "A".to_string(),
                block: yielded.clone(),
            }],
            fallback: Some(Box::new(thrown.clone())),
        },
        lir::Op::SwitchPrim {
            on: 1,
            cases: vec![lir::PrimCase {
                value: ir::Literal::Boolean(true),
                block: yielded.clone(),
            }],
            fallback: Some(Box::new(thrown.clone())),
        },
        lir::Op::SwitchPresence {
            on: 1,
            field: "x".to_string(),
            present: Box::new(yielded.clone()),
            absent: Box::new(thrown.clone()),
        },
        lir::Op::SwitchRest {
            on: 1,
            fields: vec!["x".to_string()],
            none: Box::new(yielded.clone()),
            some: Box::new(thrown.clone()),
        },
    ];
    let reps = [
        lir::Rep::Nat,
        lir::Rep::Int,
        lir::Rep::Real,
        lir::Rep::String,
        lir::Rep::Boolean,
        lir::Rep::Unit,
        lir::Rep::Struct,
        lir::Rep::Sum,
        lir::Rep::Fn,
        lir::Rep::Any,
    ];
    let lowered = lir::Output {
        externs: Vec::new(),
        functions: vec![lir::Function {
            name: "all".to_string(),
            params: reps
                .iter()
                .enumerate()
                .map(|(temp, rep)| lir::Param {
                    temp: temp as u32,
                    rep: *rep,
                })
                .collect(),
            body: lir::Block {
                instrs: ops
                    .into_iter()
                    .enumerate()
                    .map(|(temp, op)| lir::Instr {
                        temp: temp as u32,
                        rep: reps[temp % reps.len()],
                        span,
                        op,
                    })
                    .collect(),
                end: lir::Terminator {
                    span,
                    kind: lir::End::Ret(0),
                },
            },
            span,
        }],
        globals: vec![
            lir::Global {
                symbol,
                name: "value".to_string(),
                body: thrown.clone(),
                span,
            },
            lir::Global {
                symbol: local_symbol,
                name: "wildcard".to_string(),
                body: thrown,
                span,
            },
        ],
    };

    let mut artifact = Artifact::build(&mint, &program, inferred.semantics(), &lowered);
    // The exported value wears the hand-built scheme: no source program reaches
    // every semantic variant, and the export adapter is the same one building
    // used for the scheme it replaces.
    let value = artifact
        .header
        .values
        .iter_mut()
        .find(|value| value.name.ends_with("::value"))
        .expect("the value is exported");
    value.scheme = artifact::export_scheme(&mint, &custom);
    assert_round_trip(&artifact);
}

#[test]
fn building_rejects_a_pending_effect_identity() {
    let (mint, mut program, inferred, lowered) = compiled("effect Log = { write: Nat -> () }\n");
    let symbol = *program
        .effects
        .keys()
        .next()
        .expect("source has one effect");
    program
        .effect_ids
        .insert(symbol, types::EffectId::pending(symbol));

    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            Artifact::build(&mint, &program, inferred.semantics(), &lowered)
        }))
        .is_err()
    );
}

#[test]
fn canonical_text_escapes_and_parses_every_control_character() {
    let controls: String = (0..=0x1f)
        .chain(0x7f..=0x9f)
        .map(|code| char::from_u32(code).unwrap())
        .collect();
    let escaped = format!("{controls}\\");
    let mut artifact = model_artifact();
    artifact.header.identity.name = escaped.clone();
    artifact.header.values[0].scheme.body = plain(Type::Arrow(
        Box::new(plain(unit())),
        Box::new(plain(unit())),
        Row {
            labels: vec![(controls.clone(), field(Presence::Present, unit()))],
            rest: Rest::Closed,
        },
    ));
    artifact.lir.functions[0].body.instrs[3].op = Op::Const(Literal::String(escaped));

    let printed = assert_round_trip(&artifact);
    let text = printed.strip_suffix('\n').unwrap();
    assert!(
        !text
            .chars()
            .any(|character| character.is_control() && character != '\n'),
        "artifact strings must not contain raw control characters"
    );
    for control in controls.chars() {
        let escape = match control {
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            control => format!("\\u{:04x}", control as u32),
        };
        assert!(text.contains(&escape), "missing escape {escape:?}");
    }
    assert!(text.contains("\\u001f"));
    assert!(text.contains("\\\\"), "a literal backslash is escaped");
    assert_malformed(&printed.replacen("\\u001f", "\u{001f}", 1));
}

#[test]
fn fallible_parse_errors_distinguish_syntax_from_structure() {
    let syntax = artifact::try_parse("").unwrap_err();
    assert_eq!(syntax.message(), "truncated artifact text");
    assert_eq!(syntax.offset(), Some(0));
    assert_eq!(syntax.to_string(), "truncated artifact text at byte 0");

    let structure = Artifact::try_parse("(artifact)").unwrap_err();
    assert_eq!(structure.message(), "bad `artifact` arity");
    assert_eq!(structure.offset(), None);
    assert_eq!(structure.to_string(), "bad `artifact` arity");
}

#[test]
fn malformed_text_returns_errors_while_trusted_api_panics() {
    let valid = compact(&assert_round_trip(&model_artifact()));
    for text in [
        "",
        "(",
        ")",
        "\"",
        "(artifact)",
        "(artifact (header) (lir))",
        "(artifact (header (identity \"x\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)) trailing)",
        "(artifact (header (identity \"x\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)) extra",
        "(artifact (header (identity \"\\u001\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (identity \"\\u00gg\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (identity \"\\u0041\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (identity \"\\u000a\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (identity \"\\ud800\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (identity \"\\q\" \"1\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
    ] {
        assert_malformed(text);
    }
    assert_malformed(&"(".repeat(257));

    for (from, to) in [
        ("(artifact ", "(bundle "),
        ("(header ", "(heading "),
        ("(identity ", "(owner "),
        ("(dependencies ", "(requires "),
        ("(dependency ", "(requirement "),
        ("(values ", "(exports "),
        ("(value ", "(export "),
        ("(types ", "(declared-types "),
        ("(type ", "(declared-type "),
        ("(params ", "(parameters "),
        ("(param type", "(param presence"),
        ("(lacks ", "(without "),
        ("(effects ", "(declared-effects "),
        ("(effect ", "(side-effect "),
        ("(identity none)", "(identity nil)"),
        ("(operations ", "(ops "),
        ("(operation ", "(op-signature "),
        ("(alias ", "(aliases "),
        ("(scheme ", "(polytype "),
        ("(ty ", "(type-value "),
        ("(struct ", "(record-type "),
        ("(arrow ", "(function-type "),
        ("(sum ", "(variant-type "),
        ("(named ", "(external-type "),
        ("(row ", "(effect-row "),
        ("(labels ", "(row-labels "),
        ("(field ", "(row-field "),
        ("(more ", "(row-tail "),
        ("(rigid 9 ", "(tail 9 "),
        ("closed))", "open))"),
        ("field present", "field missing"),
        ("(not ", "(negation "),
        ("(and ", "(conjunction "),
        ("(or ", "(disjunction "),
        ("(iff ", "(equivalence "),
        ("(xor ", "(inequality "),
        ("(lir ", "(lowered "),
        ("(externs ", "(imports "),
        ("(extern ", "(import "),
        ("(target ", "(foreign-path "),
        ("(functions ", "(function-list "),
        ("(function ", "(fn "),
        ("(param 0 nat)", "(param 0 bogus)"),
        ("(globals ", "(global-list "),
        ("(global ", "(external "),
        ("(block ", "(basic-block "),
        ("(instrs ", "(instructions "),
        ("(instr ", "(instruction "),
        ("(const ", "(constant "),
        ("(direct ", "(static "),
        ("(captures ", "(closed-over "),
        ("(args ", "(arguments "),
        ("new-tag", "unknown-op"),
        ("(switch-tag ", "(tag-switch "),
        ("(cases ", "(branches "),
        ("(fallback ", "(otherwise "),
        ("(switch-prim ", "(primitive-switch "),
        ("(switch-presence ", "(presence-switch "),
        ("(switch-rest ", "(rest-switch "),
        ("(ret ", "(return "),
        ("(yield ", "(suspend "),
        ("(throw ", "(raise "),
        ("(bool ", "(boolean-literal "),
        ("(bool true)", "(bool maybe)"),
    ] {
        assert_bad_replacement(&valid, from, to);
    }

    for (from, to) in [
        ("(artifact ", "(artifact extra "),
        ("(identity \"bundle\" \"1.0.0\")", "(identity \"bundle\")"),
        ("(dependency \"base\" \"2.1.0\")", "(dependency \"base\")"),
        ("(scheme 15 7", "(scheme 15"),
        ("(ty nat)", "(ty nat extra)"),
        ("(param 0 nat)", "(param 0 nat extra)"),
        ("(tag \"Case\")", "(tag \"Case\" 1 2)"),
        ("(direct 0)", "(direct 0 1)"),
        (
            "(fallback)",
            "(fallback (block (instrs) (ret 0)) (block (instrs) (ret 0)))",
        ),
        ("(ret 0)", "(ret 0 1)"),
        ("(nat 18446744073709551615)", "(nat -1)"),
    ] {
        assert_bad_replacement(&valid, from, to);
    }
}

#[test]
fn malformed_text_exercises_every_parser_and_reader_error_shape() {
    let valid = compact(&model_artifact().print());

    // A parsed root can itself be a string or atom, and text after a complete
    // root is a distinct syntax error from text inside that root.
    assert_malformed("\"root\"");
    assert_malformed("root");
    assert_malformed(&format!("{valid} trailing"));
    assert_malformed("\"truncated\\");
    assert_malformed("\"\\u001");

    // Invalid values of each S-expression shape reach reader paths that a tag
    // miss or arity error does not.
    assert_bad_replacement(&valid, "(param 0 nat)", "(param 0 ())");
    assert_bad_replacement(&valid, "(target \"console.log\")", "(target)");
    assert_bad_replacement(
        &valid,
        "(target \"console.log\")",
        "(target \"console\" \"log\")",
    );
    assert_bad_replacement(&valid, "(selector named \"write\")", "\"write\"");
    assert_bad_replacement(
        &valid,
        "(selector named \"write\")",
        "(selector unnamed extra)",
    );
    assert_bad_replacement(&valid, "(selector named \"write\")", "(selector named)");
    assert_bad_replacement(&valid, "(selector named \"write\")", "(selector mystery)");
    assert_malformed(&replace_balanced(
        &valid,
        "(operations (operation",
        "operations",
    ));
    assert_malformed(&replace_balanced(&valid, "(named \"other@", "(named)"));
    assert_bad_replacement(&valid, "(ty nat)", "(ty \"wrong\")");
    assert_bad_replacement(&valid, "field present", "field (wrong)");
    assert_bad_replacement(
        &valid,
        " 7 (existentials) true ",
        " 7 (existentials) \"wrong\" ",
    );
    assert_bad_replacement(
        &valid,
        " 7 (existentials) true ",
        " 7 (existentials) wrong ",
    );

    // Recursive LIR collections distinguish malformed entries, both optional
    // block cardinalities, and a non-list call target.
    let fallback_block = "(block (instrs) (throw 97 96))";
    assert_bad_replacement(
        &valid,
        &format!("(fallback {fallback_block})"),
        &format!("(fallback {fallback_block} {fallback_block})"),
    );
    let without_tag_fallback =
        valid.replacen(&format!("(fallback {fallback_block})"), "(fallback)", 1);
    assert_ne!(without_tag_fallback, valid);
    assert!(Artifact::try_parse(&without_tag_fallback).is_ok());
    assert_malformed(&replace_balanced(&valid, "(\"A\" (block", "bad-tag-case"));
    assert_malformed(&replace_balanced(
        &valid,
        "((bool false) (block",
        "bad-primitive-case",
    ));
    assert_bad_replacement(&valid, "(direct 0)", "bad-call-target");
    assert_bad_replacement(&valid, "(tag \"Case\")", "(tag)");
    assert_malformed(&replace_balanced(
        &valid,
        "(\"present\" (field",
        "bad-type-field",
    ));
    assert_malformed(&replace_balanced(
        &valid,
        "(\"label\" (field",
        "bad-row-label",
    ));
    assert_malformed(&replace_balanced(
        &valid,
        "((field-key named \"first\") 1)",
        "bad-struct-entry",
    ));
    assert_bad_replacement(
        &valid,
        "(field-key named \"first\")",
        "\"legacy-string-key\"",
    );
    assert_bad_replacement(
        &valid,
        "(field-key unnamed-operation)",
        "(field-key unnamed-operation extra)",
    );
    assert_bad_replacement(&valid, "(field-key named \"first\")", "(field-key named)");
    assert_bad_replacement(&valid, "(field-key named \"first\")", "(field-key mystery)");
    assert_bad_replacement(
        &valid,
        "(field-key named \"first\")",
        "(wrong-key named \"first\")",
    );
    assert_bad_replacement(&valid, "new-tag", "(())");
}

#[test]
fn deep_formula_conversion_and_artifact_writing_are_stack_safe() {
    std::thread::Builder::new()
        .name("deep-artifact-formula-write".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            let mint = Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap());
            let mut formula = types::Formula::var(9)
                .and(types::Formula::bound(0))
                .or(types::Formula::True.iff(types::Formula::False))
                .xor(types::Formula::var(10));
            for _ in 0..DEPTH {
                formula = types::Formula::Not(Rc::new(formula));
            }
            let scheme = types::Scheme::constrained(1, 1, Rc::new(types::Ty::Nat), formula);
            let artifact = exporting(&mint, &scheme);
            let printed = artifact.print();
            assert!(printed.contains("(not"));
            assert!(printed.contains("(bound 0)"));
            drop(artifact);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep artifact formula conversion and writing use bounded stack");
}

#[test]
fn deep_semantic_artifact_building_is_stack_safe_in_every_position() {
    std::thread::Builder::new()
        .name("deep-artifact-types".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            let mut mint = Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap());
            let layer = mint
                .global(None, Namespace::Types, "Layer")
                .expect("a type symbol");

            let mut arrow = Rc::new(types::Ty::Nat);
            let mut named = Rc::new(types::Ty::Nat);
            let mut payload = Rc::new(types::Ty::Nat);
            let mut more = types::Row::closed();
            for index in 0..DEPTH {
                arrow = Rc::new(types::Ty::Arrow(
                    Rc::new(types::Ty::Nat),
                    arrow,
                    types::Row::closed(),
                ));
                named = Rc::new(types::Ty::Named {
                    symbol: layer,
                    name: "Layer".into(),
                    args: vec![named].into(),
                });
                payload = Rc::new(types::Ty::Struct(types::Row {
                    labels: [(format!("field{index}"), types::RowField::present(payload))]
                        .into_iter()
                        .collect(),
                    rest: types::Rest::Closed,
                }));
                more = types::Row::of(types::Rest::More(Rc::new(more)));
            }
            let body = Rc::new(types::Ty::Struct(types::Row {
                labels: [
                    ("arrow".into(), types::RowField::present(arrow)),
                    ("named".into(), types::RowField::present(named)),
                    ("payload".into(), types::RowField::present(payload)),
                    (
                        "more".into(),
                        types::RowField::present(Rc::new(types::Ty::Sum(more))),
                    ),
                    (
                        "boolean".into(),
                        types::RowField::present(Rc::new(types::Ty::Boolean)),
                    ),
                ]
                .into_iter()
                .collect(),
                rest: types::Rest::Closed,
            }));
            let scheme = types::Scheme::new(0, body);

            let artifact = exporting(&mint, &scheme);
            let value = &artifact.header.values[0];
            let Type::Struct(row) = &value.scheme.body else {
                panic!("semantic root changed schema")
            };
            assert_eq!(
                row.labels
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
                ["arrow", "named", "payload", "more", "boolean"]
            );
            let printed = artifact.print();
            assert!(printed.contains("(more (row"));
            let reparsed = Artifact::try_parse(&printed).expect("deep semantics print readably");
            assert_eq!(reparsed.print(), printed);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("artifact type, row, field and rest conversion uses bounded stack");
}

#[test]
fn deeply_nested_artifact_semantics_decode_on_a_small_stack() {
    const DEPTH: usize = 400;
    let mut value = Artifact {
        header: artifact::Header {
            identity: artifact::Identity {
                name: "deep".to_string(),
                version: "1".to_string(),
            },
            dependencies: Vec::new(),
            values: vec![artifact::Value {
                name: "deep@1::value".to_string(),
                scheme: Scheme {
                    count: 0,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: Formula::True,
                    body: plain(unit()),
                },
            }],
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: vec![Global {
                name: "deep@1::value".to_string(),
                body: Block {
                    instrs: Vec::new(),
                    end: End::Ret(0),
                },
            }],
        },
    };
    for _ in 0..DEPTH {
        value.header.values[0].scheme.formula = Formula::Not(Box::new(std::mem::replace(
            &mut value.header.values[0].scheme.formula,
            Formula::True,
        )));
        value.header.values[0].scheme.body = plain(Type::Named {
            name: "deep@1::Layer".to_string(),
            args: vec![std::mem::replace(
                &mut value.header.values[0].scheme.body,
                plain(unit()),
            )],
        });
        value.lir.globals[0].body = Block {
            instrs: vec![Instr {
                temp: 0,
                rep: Rep::Unit,
                op: Op::Catch {
                    tag: 0,
                    body: Box::new(std::mem::replace(
                        &mut value.lir.globals[0].body,
                        Block {
                            instrs: Vec::new(),
                            end: End::Ret(0),
                        },
                    )),
                },
            }],
            end: End::Ret(0),
        };
    }

    // Printing remains the compiler side of the round trip. Parsing and all
    // three recursive semantic families must fit a deliberately tiny stack.
    let (value, printed) = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let printed = value.print();
            (value, printed)
        })
        .unwrap()
        .join()
        .unwrap();
    drop(value);
    let parsed = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let parsed = Artifact::try_parse(&printed).expect("deep artifact parses");
            assert_eq!(parsed.header.values.len(), 1);
            assert_eq!(parsed.lir.globals.len(), 1);
            parsed
        })
        .unwrap()
        .join()
        .unwrap();
    drop(parsed);
}

#[test]
fn iterative_artifact_equality_preserves_every_derived_distinction() {
    assert_ne!(Type::Var(0), Type::Var(1));
    assert_ne!(Type::Bound(0), Type::Bound(1));
    assert_ne!(
        Type::Rigid {
            id: 0,
            name: "same".into(),
        },
        Type::Rigid {
            id: 1,
            name: "same".into(),
        }
    );
    assert_ne!(
        Type::Rigid {
            id: 0,
            name: "left".into(),
        },
        Type::Rigid {
            id: 0,
            name: "right".into(),
        }
    );
    assert_ne!(
        Type::Named {
            name: "Left".into(),
            args: vec![Type::Nat],
        },
        Type::Named {
            name: "Right".into(),
            args: vec![Type::Nat],
        }
    );
    assert_ne!(
        Type::Named {
            name: "Same".into(),
            args: vec![Type::Nat],
        },
        Type::Named {
            name: "Same".into(),
            args: Vec::new(),
        }
    );
    assert_ne!(Type::Nat, Type::Int);

    let row = |name: &str, presence, rest| Row {
        labels: vec![(name.into(), field(presence, Type::Nat))],
        rest,
    };
    let same = row("field", Presence::Present, Rest::Closed);
    assert_eq!(same.clone(), same);
    assert_ne!(
        same,
        Row {
            labels: Vec::new(),
            rest: Rest::Closed
        }
    );
    assert_ne!(
        row("left", Presence::Present, Rest::Closed),
        row("right", Presence::Present, Rest::Closed)
    );
    assert_ne!(
        row("field", Presence::Present, Rest::Closed),
        row("field", Presence::Absent, Rest::Closed)
    );
    assert_ne!(
        row("field", Presence::Present, Rest::Closed),
        row("field", Presence::Present, Rest::Undecided)
    );

    assert_ne!(Rest::Var(0), Rest::Var(1));
    assert_ne!(Rest::Bound(0), Rest::Bound(1));
    assert_ne!(
        Rest::Rigid {
            id: 0,
            name: "same".into(),
        },
        Rest::Rigid {
            id: 1,
            name: "same".into(),
        }
    );
    assert_ne!(
        Rest::Rigid {
            id: 0,
            name: "left".into(),
        },
        Rest::Rigid {
            id: 0,
            name: "right".into(),
        }
    );
    assert_ne!(Rest::Closed, Rest::Undecided);
    assert_ne!(
        Rest::More(Box::new(Row {
            labels: Vec::new(),
            rest: Rest::Closed,
        })),
        Rest::More(Box::new(Row {
            labels: Vec::new(),
            rest: Rest::Undecided,
        }))
    );

    assert_ne!(Formula::Var(0), Formula::Var(1));
    assert_ne!(Formula::Bound(0), Formula::Bound(1));
    assert_ne!(Formula::True, Formula::False);
    assert_ne!(
        Formula::Not(Box::new(Formula::True)),
        Formula::Not(Box::new(Formula::False))
    );
}

#[test]
fn recursive_artifact_ownership_clones_and_drops_on_a_small_stack() {
    std::thread::Builder::new()
        .name("deep-artifact-ownership".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;

            let deep_formula = || {
                let mut value = Formula::True;
                for _ in 0..DEPTH {
                    value = Formula::Not(Box::new(value));
                }
                value
            };
            let deep_type = || {
                let mut value = Type::Nat;
                for _ in 0..DEPTH {
                    value = Type::Named {
                        name: "deep@1::Layer".into(),
                        args: vec![value],
                    };
                }
                value
            };
            let deep_row_type = || {
                let mut row = Row {
                    labels: Vec::new(),
                    rest: Rest::Closed,
                };
                for _ in 0..DEPTH {
                    row = Row {
                        labels: Vec::new(),
                        rest: Rest::More(Box::new(row)),
                    };
                }
                Type::Struct(row)
            };
            let empty_block = || Block {
                instrs: Vec::new(),
                end: End::Ret(0),
            };
            let deep_block = || {
                let mut block = empty_block();
                for depth in 0..DEPTH {
                    let op = match depth % 5 {
                        0 => Op::Catch {
                            tag: 0,
                            body: Box::new(block),
                        },
                        1 => Op::SwitchTag {
                            on: 0,
                            cases: vec![artifact::TagCase {
                                name: "A".into(),
                                block,
                            }],
                            fallback: Some(Box::new(empty_block())),
                        },
                        2 => Op::SwitchPrim {
                            on: 0,
                            cases: vec![artifact::PrimCase {
                                value: Literal::Boolean(true),
                                block,
                            }],
                            fallback: Some(Box::new(empty_block())),
                        },
                        3 => Op::SwitchPresence {
                            on: 0,
                            field: "x".into(),
                            present: Box::new(block),
                            absent: Box::new(empty_block()),
                        },
                        _ => Op::SwitchRest {
                            on: 0,
                            fields: vec!["x".into()],
                            none: Box::new(empty_block()),
                            some: Box::new(block),
                        },
                    };
                    block = Block {
                        instrs: vec![Instr {
                            temp: depth as u32,
                            rep: Rep::Any,
                            op,
                        }],
                        end: End::Ret(depth as u32),
                    };
                }
                block
            };

            let artifact = Artifact {
                header: artifact::Header {
                    identity: artifact::Identity {
                        name: "deep".into(),
                        version: "1".into(),
                    },
                    dependencies: Vec::new(),
                    values: vec![artifact::Value {
                        name: "deep@1::value".into(),
                        scheme: Scheme {
                            count: 0,
                            presences: 0,
                            existentials: Vec::new(),
                            formula: deep_formula(),
                            body: deep_type(),
                        },
                    }],
                    types: vec![artifact::DeclaredType {
                        name: "deep@1::Rows".into(),
                        params: Vec::new(),
                        scheme: Scheme {
                            count: 0,
                            presences: 0,
                            existentials: Vec::new(),
                            formula: Formula::True,
                            body: deep_row_type(),
                        },
                    }],
                    effects: Vec::new(),
                },
                lir: Lir {
                    externs: Vec::new(),
                    functions: vec![artifact::Function {
                        name: "deep".into(),
                        params: Vec::new(),
                        body: deep_block(),
                    }],
                    globals: Vec::new(),
                },
            };
            let cloned = artifact.clone();
            assert_eq!(cloned, artifact);
            drop(cloned);
            drop(artifact);

            // Public header components can also outlive their containing
            // artifact, and must not depend on `Artifact::drop` for safety.
            drop(artifact::Value {
                name: "deep@1::standalone".into(),
                scheme: Scheme {
                    count: 0,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: deep_formula(),
                    body: deep_type(),
                },
            });
            drop(artifact::DeclaredType {
                name: "deep@1::StandaloneType".into(),
                params: Vec::new(),
                scheme: Scheme {
                    count: 0,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: Formula::True,
                    body: deep_row_type(),
                },
            });
            drop(deep_type());
            drop(deep_row_type());
            drop(deep_formula());

            let block = deep_block();
            let cloned = block.clone();
            assert_eq!(cloned, block);
            assert_ne!(
                empty_block(),
                Block {
                    instrs: Vec::new(),
                    end: End::Yield(0),
                }
            );
            assert_ne!(Op::Neg(0), Op::Neg(1));
            drop(cloned);
            drop(block);

            let instr = Instr {
                temp: 0,
                rep: Rep::Any,
                op: Op::Catch {
                    tag: 0,
                    body: Box::new(deep_block()),
                },
            };
            let cloned = instr.clone();
            assert_eq!(cloned, instr);
            drop(cloned);
            drop(instr);

            let op = Op::Catch {
                tag: 0,
                body: Box::new(deep_block()),
            };
            let cloned = op.clone();
            assert_eq!(cloned, op);
            drop(cloned);
            drop(op);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("recursive artifact ownership uses bounded stack");
}

#[test]
fn valid_deep_artifact_parses_and_drops_on_a_small_stack() {
    const DEPTH: usize = 30_000;
    let formula = format!("{}true{}", "(not ".repeat(DEPTH), ")".repeat(DEPTH));
    let valid = format!(
        "(artifact (header (identity \"deep\" \"1\") (dependencies) \
         (values (value \"deep@1::value\" (scheme 0 0 (existentials) {formula} (ty nat)))) \
         (types) (effects)) (lir (externs) (functions) (globals)))"
    );

    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let parsed = Artifact::try_parse(&valid).expect("valid deep artifact parses");
            assert_eq!(parsed.header.values.len(), 1);
            drop(parsed);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn malformed_deep_syntax_fails_without_exhausting_the_stack() {
    let malformed = "(".repeat(50_000);
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || Artifact::try_parse(&malformed).expect_err("unterminated input"))
        .unwrap();
    let error = handle.join().unwrap();
    assert_eq!(error.message(), "unterminated artifact list");
    assert_eq!(error.offset(), Some(50_000));
}

#[test]
fn balanced_malformed_deep_values_fail_on_a_small_stack() {
    const DEPTH: usize = 30_000;
    let nested = format!("{}wrong{}", "(wrong ".repeat(DEPTH), ")".repeat(DEPTH));
    let header = "(header (identity \"deep\" \"1\") (dependencies) (values) (types) (effects))";
    let lir = "(lir (externs) (functions) (globals))";

    // The first input is structurally balanced but puts an arbitrarily deep
    // list where a dependency string belongs. The second puts the same value
    // in an extra root slot, which `exact` must discard after reporting arity.
    // Both exercise destruction of fully built parser data on semantic error.
    let malformed = [
        format!(
            "(artifact (header (identity \"deep\" \"1\") (dependencies (dependency {nested} \"1\")) (values) (types) (effects)) {lir})"
        ),
        format!("(artifact {header} {lir} {nested})"),
    ];
    let handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            for text in malformed {
                Artifact::try_parse(&text).expect_err("wrong deep structural value");
            }
        })
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn rejected_deep_semantic_model_is_destroyed_on_a_small_stack() {
    const DEPTH: usize = 30_000;
    let formula = format!("{}true{}", "(not ".repeat(DEPTH), ")".repeat(DEPTH));
    let malformed = format!(
        "(artifact (header (identity \"deep\" \"1\") (dependencies) \
         (values (value \"deep@1::value\" (scheme 0 0 (existentials) {formula} (ty (struct (row (labels) closed)))))) \
         (types) (effects)) (lir (externs) (functions) wrong))"
    );

    let error = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || Artifact::try_parse(&malformed).expect_err("malformed trailing LIR"))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(error.message(), "expected `globals` list");
}

#[test]
fn non_struct_fields_are_not_representable_in_artifact_text() {
    let valid = compact(&model_artifact().print());
    assert_bad_replacement(
        &valid,
        "(ty nat)",
        "(ty nat (fields (\"x\" (field present (ty nat)))))",
    );
}

#[test]
fn artifact_function_indices_are_fixed_width_and_checked_by_the_parser() {
    let mut value = model_artifact();
    let closure = value.lir.functions[0]
        .body
        .instrs
        .iter_mut()
        .find_map(|instr| match &mut instr.op {
            Op::Closure { func, .. } => Some(func),
            _ => None,
        })
        .expect("model has a closure");
    let fixed_width: &mut u64 = closure;
    *fixed_width = u64::MAX;
    let printed = assert_round_trip(&value);
    assert!(printed.contains(&u64::MAX.to_string()));

    let overflow = printed.replacen(&u64::MAX.to_string(), "18446744073709551616", 1);
    assert_malformed(&overflow);
}
