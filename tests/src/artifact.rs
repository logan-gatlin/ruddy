//! Tests for the span-free, canonical bundle artifact.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use indexmap::{IndexMap, IndexSet};
use ruddy::{
    artifact::{
        self, Artifact, Block, End, Formula, Global, Instr, Lir, Literal, Op, Param, Presence,
        RecoveryFact, Rep, Rest, Row, RowField, Scheme, Type, UncheckedArtifact,
    },
    compile, inference, ir, lir, parse,
    symbol::{Bundle, Mint, Namespace, Version},
    token,
    tracking::FileManager,
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
    let accepted = compile::compile(Mint::new(bundle), parsed.stmts, inference::Trace::Off)
        .unwrap_or_else(|partial| panic!("{partial:#?}"));
    let lowered = accepted.lower();
    let (mint, ir, inferred, _) = accepted.into_parts();
    (mint, ir.program, inferred, lowered)
}

fn built(source: &str) -> Artifact {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let parsed = parse::parse(token::lex(source, file).tokens);
    compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{partial:#?}"))
    .artifact()
    .clone()
}

/// An artifact exporting one hand-built semantic scheme as its only value, for
/// semantic states no source program can conveniently reach.
fn exporting(mint: &Mint, scheme: &types::Scheme) -> Artifact {
    let name = mint.bundle().name().to_string();
    let version = mint.bundle().version().to_string();
    UncheckedArtifact {
        header: artifact::Header {
            kind: ruddy::artifact::Kind::Library,
            compiler: ruddy::artifact::Stamp::current(),
            modules: Vec::new(),
            identity: artifact::Identity {
                name: name.clone(),
                version: version.clone(),
            },
            dependencies: Vec::new(),
            values: vec![artifact::Value {
                metadata: Default::default(),
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
    .validate()
    .expect("the exported scheme validates")
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

fn with_cps_text(artifact: &Artifact, payload: &str) -> String {
    replace_balanced(
        &artifact.print(),
        "(cps-lir",
        &format!("(cps-lir {})", serde_json::to_string(payload).unwrap()),
    )
}
fn cps_json(artifact: &Artifact) -> serde_json::Value {
    serde_json::to_value(artifact.lir()).unwrap()
}

/// A long control-flow graph stays flat during persistence, cloning and destruction.
fn flat_function(name: &str, length: usize) -> artifact::Function {
    artifact::Function {
        suspension: ruddy::lir::Suspension::MaySuspend,
        name: name.into(),
        params: vec![],
        continuation: 99,
        entry: 0,
        blocks: (0..length)
            .map(|index| Block {
                params: vec![Param {
                    temp: 99,
                    rep: Rep::Cont,
                }],
                result: None,
                instrs: vec![Instr {
                    temp: 0,
                    rep: Rep::Unit,
                    op: Op::Struct(vec![]),
                }],
                end: if index + 1 == length {
                    End::Continue {
                        continuation: 99,
                        value: 0,
                    }
                } else {
                    End::Jump(artifact::Edge {
                        block: (index + 1) as u64,
                        args: vec![99],
                    })
                },
            })
            .collect(),
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
            metadata: Default::default(),
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
            captures: vec![0, 1],
        },
        Op::Global {
            callable: None,
            target: "dep@1.0.0::value".to_string(),
        },
        Op::NewTag,
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
        Rep::Array,
        Rep::Cont,
        Rep::Handler,
    ];
    UncheckedArtifact {
        header: artifact::Header {
            kind: ruddy::artifact::Kind::Library,
            compiler: ruddy::artifact::Stamp::current(),
            modules: Vec::new(),
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
                    exported: true,
                    metadata: Default::default(),
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
                    exported: true,
                    metadata: Default::default(),
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
                    exported: true,
                    metadata: Default::default(),
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
                    exported: true,
                    metadata: Default::default(),
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
                    exported: true,
                    metadata: Default::default(),
                    name: "bundle@1.0.0::Log".to_string(),
                    params: Vec::new(),
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
                    exported: true,
                    metadata: Default::default(),
                    name: "bundle@1.0.0::Alias".to_string(),
                    params: Vec::new(),
                    identity: None,
                    kind: artifact::EffectKind::Alias(artifact::AliasRow::naming(vec![
                        "bundle@1.0.0::Log".to_string(),
                    ])),
                },
            ],
        },
        lir: Lir {
            externs: vec![artifact::Extern {
                name: "bundle@1.0.0::consoleLog".to_string(),
                target: "console.log".to_string(),
                rep: Rep::Fn,
            }],
            functions: vec![
                artifact::Function {
                    suspension: ruddy::lir::Suspension::MaySuspend,
                    name: "f".into(),
                    params: reps
                        .iter()
                        .enumerate()
                        .map(|(temp, rep)| Param {
                            temp: temp as u32,
                            rep: *rep,
                        })
                        .collect(),
                    continuation: 99,
                    entry: 0,
                    blocks: vec![Block {
                        params: reps
                            .iter()
                            .enumerate()
                            .map(|(temp, rep)| Param {
                                temp: temp as u32,
                                rep: *rep,
                            })
                            .chain([Param {
                                temp: 99,
                                rep: Rep::Cont,
                            }])
                            .collect(),
                        result: None,
                        instrs: ops
                            .into_iter()
                            .enumerate()
                            .map(|(temp, op)| Instr {
                                temp: temp as u32 + 100,
                                rep: if matches!(op, Op::NewTag) {
                                    Rep::Handler
                                } else if matches!(op, Op::Closure { .. }) {
                                    Rep::Fn
                                } else {
                                    reps[temp % reps.len()]
                                },
                                op,
                            })
                            .collect(),
                        end: End::Continue {
                            continuation: 99,
                            value: 100,
                        },
                    }],
                },
                flat_function("g#init", 1),
            ],
            globals: vec![Global {
                adapter: None,
                callable: None,
                name: "bundle@1.0.0::g".into(),
                initializer: 1,
            }],
        },
    }
    .validate()
    .expect("the model artifact validates")
}

#[test]
fn existential_presence_ownership_round_trips_and_is_validated() {
    let mut artifact = model_artifact().to_unchecked();
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
    let artifact = artifact
        .validate()
        .expect("existential ownership validates");
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
    let mut artifact = model_artifact().to_unchecked();
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

    let artifact = artifact
        .validate()
        .expect("the owned disjunction validates");
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
    let mut artifact = model_artifact().to_unchecked();
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

    let parsed = artifact.validate().expect("broad package remains valid");
    assert_eq!(
        parsed.header().values[0].scheme.existentials.len(),
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
    let value = &artifact.header().values[0];
    assert_eq!(value.scheme.existentials.len(), WIDTH as usize);
    assert!(matches!(value.scheme.formula, Formula::Owned(0, _)));
    Artifact::try_parse(&artifact.print()).expect("broad semantic scheme round trips");
}

fn assert_round_trip(value: &Artifact) -> String {
    let printed = value.print();
    let parsed = Artifact::parse(&printed)
        .validate()
        .expect("printed artifact validates");
    assert_eq!(parsed, *value);
    assert_eq!(artifact::print(&parsed), printed);
    assert_eq!(artifact::parse(&printed).validate().unwrap(), parsed);
    assert_eq!(
        Artifact::try_parse(&printed).map(|value| value.validate().unwrap()),
        Ok(parsed.clone())
    );
    assert_eq!(
        artifact::try_parse(&printed).map(|value| value.validate().unwrap()),
        Ok(parsed.clone())
    );
    assert_eq!(artifact::text::try_parse(&printed), Ok(parsed));
    printed
}

#[test]
fn array_types_representations_and_constructors_round_trip() {
    let artifact = built("let values : [Nat] = [1n, 2n]");
    let printed = assert_round_trip(&artifact);
    assert!(printed.contains("(ty (array (ty nat)))"), "{printed}");
    assert!(
        artifact
            .lir()
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.instrs)
            .any(|i| matches!(&i.op, Op::Array(values) if values == &[0, 1]))
    );
}

#[test]
fn tolerant_recovery_reports_each_repair_it_applies() {
    let mut unchecked = built("let kept = 1n\nlet discarded = 2n").to_unchecked();
    unchecked.header.identity.name.clear();
    unchecked.header.identity.version.clear();
    unchecked.header.values[0].name.clear();
    unchecked.header.values[1].scheme.body = Type::Bound(99);

    let (recovered, facts) = unchecked.recover();
    assert_eq!(recovered.header().values.len(), 1);
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::IdentityNameReplaced { replacement } if replacement == "<recovered>"
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::IdentityVersionReplaced { replacement } if replacement == "0"
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::ValueNameReplaced { index: 0, replacement }
            if replacement == "<recovered>::value-0"
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::ValueDiscarded { index: 1, name, reason }
            if name.ends_with("::discarded") && !reason.is_empty()
    )));
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
        .header()
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
        .header()
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
    assert_eq!(artifact.header().identity.name, "tests");
    assert_eq!(artifact.header().identity.version, "0.1.0");
    assert!(artifact.header().dependencies.is_empty());
    assert_eq!(artifact.header().values.len(), 2);
    assert_eq!(artifact.header().types.len(), 1);
    assert_eq!(artifact.header().effects.len(), 2);
    assert!(
        artifact.header().values[0]
            .name
            .starts_with("tests@0.1.0::")
    );
    assert!(
        artifact
            .lir()
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

    assert_eq!(artifact.header().values.len(), 2);
    assert_eq!(artifact.header().values[0].name, "tests@0.1.0::Host::log");
    assert_eq!(
        artifact.lir().externs,
        [artifact::Extern {
            name: "tests@0.1.0::Host::log".to_string(),
            target: "console.log".to_string(),
            rep: Rep::Fn,
        }]
    );
    let printed = assert_round_trip(&artifact);
    assert!(!printed.contains("Span"), "{printed}");
    assert!(!printed.contains("Symbol"), "{printed}");
}

#[test]
fn canonical_pretty_layout_is_pinned_at_its_unicode_width_boundary() {
    let empty = |name: String| {
        UncheckedArtifact {
            header: artifact::Header {
                kind: ruddy::artifact::Kind::Library,
                compiler: ruddy::artifact::Stamp::current(),
                modules: Vec::new(),
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
        }
        .validate()
        .expect("an empty artifact validates")
    };

    let compiler = ruddy::artifact::COMPILER_HASH;
    let empty_lir = format!(
        "(cps-lir {})",
        serde_json::to_string(r#"{"externs":[],"functions":[],"globals":[]}"#).unwrap()
    );
    assert_eq!(
        empty("界".repeat(12)).print(),
        format!(
            "(artifact\n\
         \x20 (header\n\
         \x20   (kind library)\n\
         \x20   (identity \"界界界界界界界界界界界界\" \"1\")\n\
         \x20   (compiler \"{compiler}\")\n\
         \x20   (dependencies)\n\
         \x20   (values)\n\
         \x20   (types)\n\
         \x20   (effects)\n\
         \x20   (modules))\n\
         \x20 {empty_lir})\n"
        )
    );
    assert_eq!(
        empty("界".repeat(13)).print(),
        format!(
            "(artifact\n\
         \x20 (header\n\
         \x20   (kind library)\n\
         \x20   (identity \"界界界界界界界界界界界界界\" \"1\")\n\
         \x20   (compiler \"{compiler}\")\n\
         \x20   (dependencies)\n\
         \x20   (values)\n\
         \x20   (types)\n\
         \x20   (effects)\n\
         \x20   (modules))\n\
         \x20 {empty_lir})\n"
        )
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
    let effect = &artifact.header().effects[0];
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
        Artifact::parse(&printed)
            .validate()
            .expect("printed artifact validates")
            .header()
            .dependencies,
        artifact.header().dependencies
    );
}

#[test]
fn every_semantic_type_scheme_and_lir_variant_round_trips() {
    assert_round_trip(&model_artifact());
}

#[test]
fn building_translates_every_compiler_semantic_variant() {
    let (mut mint, program, _inferred, _) = compiled(
        "type OpenCases 'r = #A | ..'r\n\
         type Runner 'e = Nat -> Nat + ..'e\n\
         let value = 0\n",
    );
    let type_symbol = *program
        .types
        .keys()
        .next()
        .expect("source has declared types");
    mint.register_external(type_symbol, "dependency@1.0.0::OpenCases");
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

    let mut artifact = built("let value = 0n").to_unchecked();
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
    assert_round_trip(&artifact.validate().expect("the custom scheme validates"));
}

#[test]
fn canonical_text_escapes_and_parses_every_control_character() {
    let controls: String = (0..=0x1f)
        .chain(0x7f..=0x9f)
        .map(|code| char::from_u32(code).unwrap())
        .collect();
    let escaped = format!("{controls}\\");
    let mut artifact = model_artifact().to_unchecked();
    artifact.header.identity.name = escaped.clone();
    artifact.header.values[0].scheme.body = plain(Type::Arrow(
        Box::new(plain(unit())),
        Box::new(plain(unit())),
        Row {
            labels: vec![(controls.clone(), field(Presence::Present, unit()))],
            rest: Rest::Closed,
        },
    ));
    artifact.lir.functions[0].blocks[0].instrs[3].op = Op::Const(Literal::String(escaped));

    let artifact = artifact
        .validate()
        .expect("escaped control characters validate");
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

/// The length dispatch, the element and slice reads, and the join a spread
/// makes all print and parse back, and every misspelling or misshaping of
/// them is refused as the malformed text it is.
#[test]
fn array_dispatch_and_reads_round_trip_and_reject_malformed_spellings() {
    let artifact = built(
        "let a = [1n]\nlet b = [..a, 2n, ..a]\n\
         let pick = fn arr => match arr with \
         | [1n, .., 2n] => [] | [first, ..middle, last] => middle | [..rest] => rest end",
    );
    let valid = compact(&assert_round_trip(&artifact));
    let blocks = artifact
        .lir()
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .collect::<Vec<_>>();
    assert!(blocks.iter().any(|b| matches!(
        b.end,
        End::Branch {
            test: artifact::Test::Length { .. },
            ..
        }
    )));
    for name in ["Nth", "NthBack", "Slice", "Concat", "Length"] {
        let json = serde_json::to_string(artifact.lir()).unwrap();
        assert!(
            json.contains(&format!("\"{name}\"")),
            "missing {name}: {valid}"
        );
        assert_malformed(&with_cps_text(
            &artifact,
            &json.replace(&format!("\"{name}\""), "\"UnknownInstruction\""),
        ));
    }
    let mut json = cps_json(&artifact);
    json["functions"][0]["entry"] = serde_json::json!("zero");
    assert_malformed(&with_cps_text(&artifact, &json.to_string()));
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
        "(artifact (header (kind library) (identity \"x\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)) trailing)",
        "(artifact (header (kind library) (identity \"x\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)) extra",
        "(artifact (header (kind library) (identity \"\\u001\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (kind library) (identity \"\\u00gg\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (kind library) (identity \"\\u0041\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (kind library) (identity \"\\u000a\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (kind library) (identity \"\\ud800\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
        "(artifact (header (kind library) (identity \"\\q\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects)) (lir (functions) (globals)))",
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
        ("(cps-lir ", "(lowered "),
    ] {
        assert_bad_replacement(&valid, from, to);
    }

    for (from, to) in [
        ("(artifact ", "(artifact extra "),
        ("(identity \"bundle\" \"1.0.0\")", "(identity \"bundle\")"),
        ("(dependency \"base\" \"2.1.0\")", "(dependency \"base\")"),
        ("(scheme 15 7", "(scheme 15"),
        ("(ty nat)", "(ty nat extra)"),
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
    assert_malformed(&replace_balanced(&valid, "(cps-lir", "(cps-lir)"));
    assert_malformed(&replace_balanced(&valid, "(cps-lir", "(cps-lir 0)"));
    assert_malformed(&replace_balanced(
        &valid,
        "(cps-lir",
        "(cps-lir \"{}\" extra)",
    ));
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
            let value = &artifact.header().values[0];
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
            let reparsed = Artifact::try_parse(&printed)
                .expect("deep semantics parse")
                .validate()
                .expect("deep semantics validate");
            assert_eq!(reparsed.print(), printed);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("artifact type, row, field and rest conversion uses bounded stack");
}

#[test]
fn deeply_nested_artifact_semantics_decode_on_a_small_stack() {
    const DEPTH: usize = 400;
    let mut value = UncheckedArtifact {
        header: artifact::Header {
            kind: ruddy::artifact::Kind::Library,
            compiler: ruddy::artifact::Stamp::current(),
            modules: Vec::new(),
            identity: artifact::Identity {
                name: "deep".to_string(),
                version: "1".to_string(),
            },
            dependencies: Vec::new(),
            values: vec![artifact::Value {
                metadata: Default::default(),
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
            functions: vec![flat_function("value#init", DEPTH)],
            globals: vec![Global {
                adapter: None,
                callable: None,
                name: "deep@1::value".into(),
                initializer: 0,
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
    }

    // Printing remains the compiler side of the round trip. Parsing and all
    // three recursive semantic families must fit a deliberately tiny stack.
    let (value, printed) = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let value = value.validate().expect("the deep artifact validates");
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
            let deep_blocks = || flat_function("deep", DEPTH).blocks;

            let artifact = UncheckedArtifact {
                header: artifact::Header {
                    kind: ruddy::artifact::Kind::Library,
                    compiler: ruddy::artifact::Stamp::current(),
                    modules: Vec::new(),
                    identity: artifact::Identity {
                        name: "deep".into(),
                        version: "1".into(),
                    },
                    dependencies: Vec::new(),
                    values: vec![artifact::Value {
                        metadata: Default::default(),
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
                        exported: true,
                        metadata: Default::default(),
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
                        suspension: ruddy::lir::Suspension::MaySuspend,
                        name: "deep".into(),
                        params: Vec::new(),
                        continuation: 99,
                        entry: 0,
                        blocks: deep_blocks(),
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
                metadata: Default::default(),
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
                exported: true,
                metadata: Default::default(),
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

            let block = deep_blocks();
            let cloned = block.clone();
            assert_eq!(cloned, block);
            assert_ne!(
                End::Unreachable,
                End::Continue {
                    continuation: 99,
                    value: 0
                }
            );
            assert_ne!(Op::Neg(0), Op::Neg(1));
            drop(cloned);
            drop(block);

            let instr = Instr {
                temp: 0,
                rep: Rep::Any,
                op: Op::Continuation {
                    code: artifact::CodeRef {
                        function: 0,
                        block: 1,
                    },
                    captures: (0..DEPTH as u32).collect(),
                },
            };
            let cloned = instr.clone();
            assert_eq!(cloned, instr);
            drop(cloned);
            drop(instr);

            let op = Op::Continuation {
                code: artifact::CodeRef {
                    function: 0,
                    block: 1,
                },
                captures: (0..DEPTH as u32).collect(),
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
    let empty_lir = format!(
        "(cps-lir {})",
        serde_json::to_string(r#"{"externs":[],"functions":[],"globals":[]}"#).unwrap()
    );
    let valid = format!(
        "(artifact (header (kind library) (identity \"deep\" \"1\") (compiler \"0000000000000000\") (dependencies) \
         (values (value \"deep@1::value\" (scheme 0 0 (existentials) {formula} (ty nat)) (metadata))) \
         (types) (effects) (modules)) {empty_lir})"
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
    let header = "(header (kind library) (identity \"deep\" \"1\") (compiler \"0000000000000000\") (dependencies) (values) (types) (effects))";
    let lir = "(lir (externs) (functions) (globals))";

    // The first input is structurally balanced but puts an arbitrarily deep
    // list where a dependency string belongs. The second puts the same value
    // in an extra root slot, which `exact` must discard after reporting arity.
    // Both exercise destruction of fully built parser data on semantic error.
    let malformed = [
        format!(
            "(artifact (header (kind library) (identity \"deep\" \"1\") (dependencies (dependency {nested} \"1\")) (values) (types) (effects)) {lir})"
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
        "(artifact (header (kind library) (identity \"deep\" \"1\") (compiler \"0000000000000000\") (dependencies) \
         (values (value \"deep@1::value\" (scheme 0 0 (existentials) {formula} (ty (struct (row (labels) closed)))) (metadata))) \
         (types) (effects) (modules)) (cps-lir wrong))"
    );

    let error = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || Artifact::try_parse(&malformed).expect_err("malformed trailing LIR"))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(error.message(), "expected artifact string");
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
    let mut value = model_artifact().to_unchecked();
    let closure = value.lir.functions[0].blocks[0]
        .instrs
        .iter_mut()
        .find_map(|instr| match &mut instr.op {
            Op::Closure { func, .. } => Some(func),
            _ => None,
        })
        .expect("model has a closure");
    let fixed_width: &mut u64 = closure;
    *fixed_width = u64::MAX;
    // TODO: an out-of-table function index can no longer be printed through
    // the public API: `UncheckedArtifact::validate` rejects it before any
    // `Artifact` exists and there is no other way to build one. The boundary
    // is asserted directly and the fixed-width text is exercised by
    // substituting the index into the model's canonical text instead.
    let error = value
        .validate()
        .expect_err("a dangling function index is rejected");
    assert!(
        error.message().contains("outside its function table"),
        "{error}"
    );

    let artifact = model_artifact();
    let mut json = cps_json(&artifact);
    let instructions = json["functions"][0]["blocks"][0]["instrs"]
        .as_array_mut()
        .unwrap();
    let closure = instructions
        .iter_mut()
        .find(|i| i["op"].get("Closure").is_some())
        .unwrap();
    closure["op"]["Closure"]["func"] = serde_json::json!(u64::MAX);
    let printed = with_cps_text(&artifact, &json.to_string());
    let parsed = Artifact::try_parse(&printed).expect("a fixed-width index parses");
    let index = parsed.lir.functions[0].blocks[0]
        .instrs
        .iter()
        .find_map(|instr| match &instr.op {
            Op::Closure { func, .. } => Some(*func),
            _ => None,
        })
        .expect("the parsed model has a closure");
    assert_eq!(index, u64::MAX);

    let overflow = printed.replacen(&u64::MAX.to_string(), "18446744073709551616", 1);
    assert_malformed(&overflow);
}

/// A parameterized effect publishes its parameters and its generic interface,
/// and an alias publishes the row it writes, unexpanded, over its own
/// parameters — and both come back as themselves.
#[test]
fn parameterized_effects_and_generic_aliases_round_trip() {
    let artifact = built(
        "effect Log = { write: Nat -> () }\n\
         effect Ask 'a = { get: () -> 'a }\n\
         effect State 'r = { get: () -> { x: Nat, ..'r } }\n\
         effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
         let f : () -> Nat + !Both Nat (..'e) = fn _ => 0n",
    );
    let effect = |name: &str| {
        artifact
            .header()
            .effects
            .iter()
            .find(|effect| effect.name == format!("tests@0.1.0::{name}"))
            .unwrap_or_else(|| panic!("no effect {name}"))
    };
    let ask = effect("Ask");
    assert_eq!(ask.params.len(), 1);
    assert_eq!(ask.params[0].sense, artifact::Sense::Type);
    assert!(ask.params[0].relevant);
    let artifact::EffectKind::Operations(operations) = &ask.kind else {
        panic!("Ask declares operations");
    };
    assert!(matches!(operations[0].to, Type::Bound(0)));
    let state = effect("State");
    assert_eq!(state.params[0].sense, artifact::Sense::Fields);
    assert_eq!(state.params[0].lacks, ["x"]);
    let both = effect("Both");
    assert_eq!(
        both.params
            .iter()
            .map(|param| param.sense)
            .collect::<Vec<_>>(),
        [artifact::Sense::Type, artifact::Sense::Effects]
    );
    assert!(both.identity.is_none());
    let artifact::EffectKind::Alias(row) = &both.kind else {
        panic!("Both is an alias");
    };
    assert_eq!(row.tail, Some(1));
    assert_eq!(row.cases.len(), 2);
    assert_eq!(row.cases[0].name, "tests@0.1.0::Ask");
    assert!(matches!(row.cases[0].args.as_slice(), [Type::Bound(0)]));
    assert!(row.cases[1].args.is_empty());

    let printed = assert_round_trip(&artifact);
    // The row a use writes carries the arguments the label was applied to.
    assert!(
        compact(&printed).contains("(effect \"tests@0.1.0::Both\" true (params (param type true (lacks)) (param effects true (lacks"),
        "{printed}"
    );

    // And every part of the new schema is held to its shape: a bound outside
    // the effect's parameters, a tail outside them, an alias applying an
    // effect to the wrong number of arguments, and a ring of aliases.
    assert_bad_replacement(&printed, "(ty (bound 0))", "(ty (bound 3))");
    assert_bad_replacement(&printed, "(tail 1)", "(tail 2)");
    assert_bad_replacement(
        &printed,
        "(case \"tests@0.1.0::Log\")",
        "(case \"tests@0.1.0::Log\" (ty nat))",
    );
    assert_bad_replacement(
        &printed,
        "(case \"tests@0.1.0::Log\")",
        "(case \"tests@0.1.0::Both\" (ty nat) (ty nat))",
    );
}

/// An artifact records the compiler that wrote it, and the stamp survives the
/// text round trip: this compiler recognizes its own, and nothing else.
#[test]
fn artifacts_are_stamped_with_their_compiler() {
    let artifact = model_artifact();
    assert!(artifact.header().compiler.is_current());
    let printed = artifact.print();
    assert!(printed.contains(&format!(
        "(compiler \"{}\")",
        ruddy::artifact::COMPILER_HASH
    )));
    let parsed = Artifact::try_parse(&printed).unwrap().validate().unwrap();
    assert!(parsed.header().compiler.is_current());

    let other = printed.replace(ruddy::artifact::COMPILER_HASH, "0000000000000000");
    let parsed = Artifact::try_parse(&other).unwrap().validate().unwrap();
    assert!(!parsed.header().compiler.is_current());
    assert_eq!(parsed.header().compiler.as_str(), "0000000000000000");
    assert_eq!(ruddy::artifact::COMPILER_HASH.len(), 16);
}

/// Printing and reading an artifact cost memory rather than stack: a block
/// nested thousands deep and a header listing thousands of values both go to
/// text and back on a thread with a stack no traversal could recurse down.
/// The depth is bounded by the text rather than the printer — every level
/// indents every line under it, so the text of a nesting grows with its
/// square — and is many times what a recursive layout survives on this
/// stack.
#[test]
fn deep_and_wide_artifacts_print_and_parse_on_a_small_stack() {
    const DEPTH: usize = 3_000;
    const WIDTH: usize = 4_096;
    let mut artifact = model_artifact().to_unchecked();
    artifact
        .lir
        .functions
        .push(flat_function("deep@1.0.0::deep", DEPTH));
    for index in 0..WIDTH {
        artifact.header.values.push(artifact::Value {
            name: format!("deep@1.0.0::value{index}"),
            scheme: artifact.header.values[0].scheme.clone(),
            metadata: artifact::Metadata::new(),
        });
    }
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let artifact = artifact.validate().expect("the deep artifact validates");
            let printed = artifact.print();
            let parsed = Artifact::try_parse(&printed)
                .expect("the deep artifact parses")
                .validate()
                .expect("the deep artifact validates again");
            assert_eq!(parsed.lir().functions.len(), artifact.lir().functions.len());
            assert_eq!(parsed.header().values.len(), artifact.header().values.len());
            assert_eq!(parsed.print(), printed);
            drop(parsed);
            drop(artifact);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn meta(entries: &[(&str, artifact::Data)]) -> artifact::Metadata {
    entries
        .iter()
        .map(|(key, data)| (key.to_string(), data.clone()))
        .collect()
}

fn unit_data() -> artifact::Data {
    artifact::Data::Struct(IndexMap::new())
}

/// Every exported value, declared type, declared effect, and module publishes
/// the metadata written in front of it: a struct of literal data, empty when
/// none was written. Each name a pattern binds carries the statement's, and
/// a hidden definition publishes nothing at all.
#[test]
fn metadata_is_published_for_every_declaration() {
    let artifact = built(
        "@doc \"Adds.\" @since 2n @flag let add = 1n\n\
         @host \"m\" extern sqrt : Real -> Real = \"Math.sqrt\"\n\
         @shape (1n, \"two\") type Pair = { first: Nat }\n\
         @level #Debug effect Log = Nat -> ()\n\
         @owner { team: \"core\" } module M = @inner [1i, 2i] let y = 1.5 end\n\
         @k let (a, b) = (1n, 2n)\n\
         @gone let _ = 3n\n\
         let plain = 0n",
    );
    let header = artifact.header();
    let value = |suffix: &str| {
        header
            .values
            .iter()
            .find(|value| value.name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no value ending in {suffix}: {:#?}", header.values))
    };
    assert_eq!(
        value("::add").metadata,
        meta(&[
            ("doc", artifact::Data::String("Adds.".into())),
            ("since", artifact::Data::Natural(2)),
            ("flag", unit_data()),
        ])
    );
    assert_eq!(
        value("::sqrt").metadata,
        meta(&[("host", artifact::Data::String("m".into()))])
    );
    assert_eq!(
        value("::M::y").metadata,
        meta(&[(
            "inner",
            artifact::Data::Array(vec![artifact::Data::Integer(1), artifact::Data::Integer(2)])
        )])
    );
    assert_eq!(value("::a").metadata, meta(&[("k", unit_data())]));
    assert_eq!(value("::b").metadata, meta(&[("k", unit_data())]));
    assert_eq!(value("::plain").metadata, meta(&[]));
    assert!(
        header
            .values
            .iter()
            .all(|value| !value.metadata.contains_key("gone")),
        "a `let _` publishes nothing: {:#?}",
        header.values
    );

    assert_eq!(header.types.len(), 1);
    assert_eq!(
        header.types[0].metadata,
        meta(&[(
            "shape",
            artifact::Data::Struct(
                [
                    ("0".to_string(), artifact::Data::Natural(1)),
                    ("1".to_string(), artifact::Data::String("two".into())),
                ]
                .into_iter()
                .collect()
            )
        )])
    );
    assert_eq!(header.effects.len(), 1);
    assert_eq!(
        header.effects[0].metadata,
        meta(&[(
            "level",
            artifact::Data::Tag {
                name: "Debug".into(),
                payload: Box::new(unit_data()),
            }
        )])
    );
    assert_eq!(header.modules.len(), 1);
    assert_eq!(header.modules[0].name, "tests@0.1.0::M");
    assert_eq!(
        header.modules[0].metadata,
        meta(&[(
            "owner",
            artifact::Data::Struct(
                [("team".to_string(), artifact::Data::String("core".into()))]
                    .into_iter()
                    .collect()
            )
        )])
    );
}

/// Metadata survives the artifact's text exactly: the three numeric kinds
/// stay distinct, strings keep their escapes and newlines, tags keep their
/// payloads, and nested values keep their order.
#[test]
fn metadata_round_trips_through_artifact_text() {
    let artifact = built(
        "@n 1n @i 1i @r 1.5 @whole 1 @b false\n\
         @s \"a\\\"b\\\\c\\nd\\te\"\n\
         @raw \\\\ first line\n     \\\\ second line\n\
         @t #Tag (1n, [true, false])\n\
         @o { \"x y\": 0n, plain: (), nested: { deep: #A }, 0: 1n }\n\
         let x = 1n\n\
         @m module M\n\
         @e effect E = Nat -> ()\n\
         @ty type T = Nat",
    );
    let printed = assert_round_trip(&artifact);
    for expected in [
        "(entry \"n\" (nat 1))",
        "(entry \"i\" (int 1))",
        "(entry \"r\" (real 1.5))",
        "(entry \"whole\" (real 1))",
        "(entry \"b\" (bool false))",
        "(entry \"s\" (string \"a\\\"b\\\\c\\nd\\te\"))",
        "(entry \"raw\" (string \" first line\\n second line\"))",
        "\"Tag\"",
        "(field \"0\" (nat 1))",
        "(field \"1\" (array (bool true) (bool false)))",
        "(field \"plain\" (struct))",
        "(field \"deep\" (tag \"A\" (struct)))",
        "(module \"tests@0.1.0::M\" (metadata (entry \"m\" (struct))))",
    ] {
        assert!(printed.contains(expected), "{expected}\n{printed}");
    }
    let header = artifact.header();
    let value = &header.values[0];
    assert!(matches!(value.metadata["whole"], artifact::Data::Real(value) if value == 1.0));
    assert!(matches!(value.metadata["n"], artifact::Data::Natural(1)));
    assert!(matches!(value.metadata["i"], artifact::Data::Integer(1)));
    assert_eq!(header.effects[0].metadata, meta(&[("e", unit_data())]));
    assert_eq!(header.types[0].metadata, meta(&[("ty", unit_data())]));
}

/// Every misspelling or misshaping of metadata text is refused by the
/// reader: an unknown data kind, wrong arities, an atom where a list goes,
/// a repeated key or field, and nesting past the depth the compiler admits.
#[test]
fn malformed_metadata_text_is_refused() {
    let printed = built("@k { a: #T 1n } let x = 1n").print();
    for (present, replacement) in [
        ("(nat 1)", "(bogus 1)"),
        ("(nat 1)", "(nat)"),
        ("(nat 1)", "nat"),
        ("(tag \"T\" (nat 1))", "(tag \"T\")"),
        ("(field \"a\"", "(fld \"a\""),
        ("(field \"a\" ", "(field "),
        ("(metadata", "(meta"),
        ("(entry \"k\"", "(entry"),
        ("(entry \"k\"", "(key \"k\""),
        ("(struct (field", "(struct field"),
        ("(metadata (entry \"k\"", "(metadata k (entry \"k\""),
        (
            "(struct (field \"a\" (tag \"T\" (nat 1))))",
            "(struct (field \"a\" (nat 1)) (field \"a\" (nat 2)))",
        ),
        (
            "(metadata (entry \"k\" (struct (field \"a\" (tag \"T\" (nat 1)))))",
            "(metadata (entry \"k\" (nat 1)) (entry \"k\" (nat 2)))",
        ),
        ("(modules)", "(modules (module \"tests@0.1.0::M\"))"),
        ("(modules)", "(modules (mod \"tests@0.1.0::M\" (metadata)))"),
    ] {
        assert!(printed.contains(present), "{present}\n{printed}");
        assert_malformed(&printed.replacen(present, replacement, 1));
    }
    assert_malformed(&printed.replacen("(modules)", "", 1));

    let limit = ruddy::ir::METADATA_DEPTH_LIMIT;
    let deep = format!("{}(nat 1){}", "(array ".repeat(limit), ")".repeat(limit));
    let too_deep = printed.replacen("(nat 1)", &deep, 1);
    let error = Artifact::try_parse(&too_deep).expect_err("nesting past the limit is refused");
    assert_eq!(error.message(), "metadata nested too deeply");
    // The `(nat 1)` sits inside a struct field inside a tag, three levels down
    // already, so one array fewer than the limit fits exactly.
    let just_fits = format!(
        "{}(nat 1){}",
        "(array ".repeat(limit - 3),
        ")".repeat(limit - 3)
    );
    Artifact::try_parse(&printed.replacen("(nat 1)", &just_fits, 1))
        .expect("nesting at the limit is admitted")
        .validate()
        .expect("and validates");
}

/// In-memory metadata is held to the same limit as text, and recovery
/// discards a value or module whose metadata breaks it, leaving the rest.
#[test]
fn recovery_discards_declarations_whose_metadata_is_too_deep() {
    let mut deep = unit_data();
    for _ in 0..ruddy::ir::METADATA_DEPTH_LIMIT {
        deep = artifact::Data::Array(vec![deep]);
    }
    let mut unchecked =
        built("let kept = 1n\nlet discarded = 2n\nmodule M\nmodule Gone").to_unchecked();
    unchecked.header.values[1]
        .metadata
        .insert("deep".into(), deep.clone());
    unchecked.header.modules[1]
        .metadata
        .insert("deep".into(), deep);
    let error = unchecked
        .clone()
        .validate()
        .expect_err("too deep to validate");
    assert_eq!(error.message(), "metadata nested too deeply");

    let (recovered, facts) = unchecked.recover();
    assert_eq!(recovered.header().values.len(), 1);
    assert!(recovered.header().values[0].name.ends_with("::kept"));
    assert_eq!(recovered.header().modules.len(), 1);
    assert!(recovered.header().modules[0].name.ends_with("::M"));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::ValueDiscarded { index: 1, name, reason }
            if name.ends_with("::discarded") && reason == "metadata nested too deeply"
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        RecoveryFact::ModuleDiscarded { index: 1, name, reason }
            if name.ends_with("::Gone") && reason == "metadata nested too deeply"
    )));
}

/// Two data values are the same exactly when they would print the same: a
/// real compares by its representation, so signed zero is told apart, and
/// every other kind by value.
#[test]
fn metadata_data_compares_by_representation() {
    use artifact::Data;
    assert_eq!(Data::Real(1.5), Data::Real(1.5));
    assert_ne!(Data::Real(0.0), Data::Real(-0.0));
    assert_ne!(Data::Natural(1), Data::Integer(1));
    assert_ne!(Data::Real(1.0), Data::Natural(1));
    assert_eq!(
        Data::Tag {
            name: "A".into(),
            payload: Box::new(unit_data())
        },
        Data::Tag {
            name: "A".into(),
            payload: Box::new(unit_data())
        }
    );
    assert_ne!(
        Data::Array(vec![Data::Boolean(true)]),
        Data::Array(vec![Data::Boolean(false)])
    );
    assert_ne!(Data::String("a".into()), Data::Boolean(true));
}

#[test]
fn cps_artifacts_reject_invalid_destinations_environments_and_summaries() {
    let artifact = built(
        "let sum = fn n => match n with | 0.0 => 0.0 | _ => n + sum (n - 1.0) end\nlet value = sum 5.0",
    );
    let (function, block, instruction, code) = artifact
        .lir()
        .functions
        .iter()
        .enumerate()
        .find_map(|(f, function)| {
            function.blocks.iter().enumerate().find_map(|(b, block)| {
                block
                    .instrs
                    .iter()
                    .enumerate()
                    .find_map(|(i, instruction)| match instruction.op {
                        Op::Continuation { code, .. } => Some((f, b, i, code)),
                        _ => None,
                    })
            })
        })
        .expect("non-tail recursion saves a continuation");
    type Corruption = (&'static str, Box<dyn Fn(&mut Lir)>);
    let edits: Vec<Corruption> = vec![
        (
            "entry",
            Box::new(move |lir| lir.functions[function].entry = u64::MAX),
        ),
        (
            "initializer",
            Box::new(|lir| lir.globals[0].initializer = u64::MAX),
        ),
        (
            "continuation function",
            Box::new(move |lir| {
                let Op::Continuation { code, .. } =
                    &mut lir.functions[function].blocks[block].instrs[instruction].op
                else {
                    unreachable!()
                };
                code.function = u64::MAX;
            }),
        ),
        (
            "continuation block",
            Box::new(move |lir| {
                let Op::Continuation { code, .. } =
                    &mut lir.functions[function].blocks[block].instrs[instruction].op
                else {
                    unreachable!()
                };
                code.block = u64::MAX;
            }),
        ),
        (
            "capture arity",
            Box::new(move |lir| {
                let Op::Continuation { captures, .. } =
                    &mut lir.functions[function].blocks[block].instrs[instruction].op
                else {
                    unreachable!()
                };
                captures.pop();
            }),
        ),
        (
            "capture availability",
            Box::new(move |lir| {
                let Op::Continuation { captures, .. } =
                    &mut lir.functions[function].blocks[block].instrs[instruction].op
                else {
                    unreachable!()
                };
                captures[0] = u32::MAX;
            }),
        ),
        (
            "entry kind",
            Box::new(move |lir| {
                lir.functions[code.function as usize].blocks[code.block as usize].result = None
            }),
        ),
        (
            "result convention",
            Box::new(move |lir| {
                lir.functions[code.function as usize].blocks[code.block as usize].result =
                    Some(u32::MAX)
            }),
        ),
        (
            "duplicate parameter",
            Box::new(move |lir| {
                let b = &mut lir.functions[function].blocks[block];
                b.params.push(b.params[0]);
            }),
        ),
        (
            "continuation representation",
            Box::new(move |lir| {
                lir.functions[function].blocks[block].instrs[instruction].rep = Rep::Fn
            }),
        ),
        (
            "unproved synchronous export",
            Box::new(|lir| {
                lir.globals[0].adapter = Some(ruddy::externs::Callback::Sync);
                lir.globals[0].callable = None;
            }),
        ),
    ];
    for (name, edit) in edits {
        let mut unchecked = artifact.to_unchecked();
        edit(&mut unchecked.lir);
        assert!(
            unchecked.clone().validate().is_err(),
            "accepted invalid {name}"
        );
        let (recovered, facts) = unchecked.recover();
        assert!(
            recovered.lir().functions.is_empty(),
            "retained invalid {name}"
        );
        assert!(matches!(
            facts.as_slice(),
            [RecoveryFact::ExecutableDiscarded { .. }]
        ));
    }
    let mut asynchronous = built("@async extern wait : Nat -> Nat = \"host.wait\"").to_unchecked();
    for f in &mut asynchronous.lir.functions {
        f.suspension = ruddy::lir::Suspension::Synchronous;
    }
    assert!(
        asynchronous
            .validate()
            .unwrap_err()
            .message()
            .contains("synchronous function")
    );
}

#[test]
fn cps_artifacts_reject_forged_callable_summaries_and_handler_operands() {
    let mut forged = built("let invoke = fn f => f ()").to_unchecked();
    let global = forged
        .lir
        .globals
        .iter_mut()
        .find(|g| g.callable == Some(ruddy::lir::Suspension::MaySuspend))
        .unwrap();
    global.callable = Some(ruddy::lir::Suspension::Synchronous);
    global.adapter = Some(ruddy::externs::Callback::Sync);
    assert!(
        forged.validate().is_err(),
        "callable summary must agree with its closure"
    );

    let original = built(
        "effect Exit = Nat -> Nat\nlet run = fn n => handle !Exit n with | !Exit value => raise value end",
    );
    let mut forged = original.to_unchecked();
    for instr in forged
        .lir
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instrs)
    {
        if matches!(instr.op, Op::NewTag) {
            instr.rep = Rep::Cont;
        }
    }
    assert!(
        forged.validate().is_err(),
        "handler identities and continuations are distinct"
    );
}

#[test]
fn cps_artifacts_reject_synchronous_summaries_for_unknown_indirect_calls() {
    let mut unchecked = built("let apply = fn f => f ()").to_unchecked();
    for f in &mut unchecked.lir.functions {
        f.suspension = ruddy::lir::Suspension::Synchronous;
    }
    for g in &mut unchecked.lir.globals {
        g.callable = Some(ruddy::lir::Suspension::Synchronous);
    }
    assert!(
        unchecked.validate().is_err(),
        "an unknown higher-order call cannot certify itself synchronous"
    );
}
