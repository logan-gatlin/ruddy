//! Tests for the span-free, canonical bundle artifact.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ruddy::{
    artifact::{
        self, Artifact, Block, Callee, Core, End, Formula, Global, Instr, Lir, Literal, Op, Param,
        Presence, Rep, Row, RowField, Scheme, Type,
    },
    inference, ir, lir, parse, patterns,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};

fn built(source: &str) -> Artifact {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("tests", Version::new(0, 1, 0)).unwrap();
    let mut mint = Mint::new(bundle);
    let mut program = ir::build(&mut mint, parsed.stmts).program;
    let inferred = inference::infer(&mint, &mut program);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    let checked = patterns::check(&program, &inferred);
    assert!(checked.errors.is_empty(), "{:#?}", checked.errors);
    let lowered = lir::lower(&mint, &program, &inferred);
    Artifact::build(&mint, &program, &inferred, &lowered)
}

#[test]
fn a_compiled_bundle_round_trips_through_canonical_text() {
    let artifact = built(
        "type Box 'a = { value: 'a }\n\
         effect Log = write : Nat -> ()\n\
         effect Console = !Log\n\
         let id = fn x => x\n\
         let main = fn n => handle id n with | !Log.write x => {} end\n",
    );
    assert_eq!(artifact.header.identity.name, "tests");
    assert_eq!(artifact.header.identity.version, "0.1.0");
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

    let printed = artifact::print(&artifact);
    assert!(
        !printed.contains("<test>"),
        "source paths must not cross the disk boundary"
    );
    assert!(
        !printed.contains("Span"),
        "source spans must not cross the disk boundary"
    );
    let parsed = artifact::parse(&printed);
    assert_eq!(parsed, artifact);
    assert_eq!(artifact::print(&parsed), printed);
}

/// The model test deliberately reaches branches which a small source program
/// cannot conveniently force (for example every literal bit pattern and every
/// formula constructor).  It also proves parser/printer coverage is not coupled
/// to compiler source spans.
#[test]
fn every_model_variant_has_a_lossless_text_form() {
    let ty = |core| Type {
        core,
        fields: vec![(
            "optional".to_string(),
            RowField {
                presence: Presence::Bound(0),
                ty: Type {
                    core: Core::Bound(1),
                    fields: Vec::new(),
                },
            },
        )],
    };
    let body = ty(Core::Arrow(
        Box::new(ty(Core::Rigid {
            id: 4,
            name: "a".to_string(),
        })),
        Box::new(ty(Core::Sum(Row {
            labels: vec![(
                "Case".to_string(),
                RowField {
                    presence: Presence::Var(5),
                    ty: ty(Core::Named {
                        name: "other@1.2.3::T".to_string(),
                        args: vec![ty(Core::Var(6))],
                    }),
                },
            )],
            rest: artifact::Rest::More(Box::new(Row {
                labels: Vec::new(),
                rest: artifact::Rest::Rigid {
                    id: 7,
                    name: "tail".to_string(),
                },
            })),
        }))),
        Row {
            labels: Vec::new(),
            rest: artifact::Rest::Bound(2),
        },
    ));
    let nested = Block {
        instrs: Vec::new(),
        end: End::Yield(99),
    };
    let ops = vec![
        Op::Const(Literal::Natural(1)),
        Op::Const(Literal::Integer(-2)),
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
        Op::Struct(vec![("first".to_string(), 1), ("second".to_string(), 2)]),
        Op::Merge(vec![1, 2]),
        Op::Project {
            base: 1,
            field: "field".to_string(),
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
            body: Box::new(nested.clone()),
        },
        Op::SwitchTag {
            on: 1,
            cases: vec![artifact::TagCase {
                name: "A".to_string(),
                block: nested.clone(),
            }],
            fallback: Some(Box::new(nested.clone())),
        },
        Op::SwitchPrim {
            on: 1,
            cases: vec![artifact::PrimCase {
                value: Literal::Boolean(false),
                block: nested.clone(),
            }],
            fallback: None,
        },
        Op::SwitchPresence {
            on: 1,
            field: "x".to_string(),
            present: Box::new(nested.clone()),
            absent: Box::new(nested.clone()),
        },
        Op::SwitchRest {
            on: 1,
            fields: vec!["x".to_string(), "y".to_string()],
            none: Box::new(nested.clone()),
            some: Box::new(nested.clone()),
        },
    ];
    let formula = Formula::And(
        Box::new(Formula::Iff(
            Box::new(Formula::Bound(0)),
            Box::new(Formula::Not(Box::new(Formula::Var(4)))),
        )),
        Box::new(Formula::Xor(
            Box::new(Formula::True),
            Box::new(Formula::Or(
                Box::new(Formula::False),
                Box::new(Formula::Bound(0)),
            )),
        )),
    );
    let artifact = Artifact {
        header: artifact::Header {
            identity: artifact::Identity {
                name: "bundle".to_string(),
                version: "1.0.0".to_string(),
            },
            values: vec![artifact::Value {
                name: "bundle@1.0.0::value".to_string(),
                scheme: Scheme {
                    count: 3,
                    presences: 1,
                    formula,
                    body,
                },
            }],
            types: vec![artifact::DeclaredType {
                name: "bundle@1.0.0::T".to_string(),
                params: vec![artifact::Parameter {
                    sense: artifact::Sense::Effects,
                    lacks: vec!["Log".to_string()],
                    relevant: false,
                }],
                scheme: Scheme {
                    count: 0,
                    presences: 0,
                    formula: Formula::True,
                    body: Type {
                        core: Core::Undecided,
                        fields: Vec::new(),
                    },
                },
            }],
            effects: vec![
                artifact::DeclaredEffect {
                    name: "bundle@1.0.0::Log".to_string(),
                    identity: Some(artifact::EffectIdentity {
                        name: "Log".to_string(),
                        interface: "write".to_string(),
                    }),
                    kind: artifact::EffectKind::Operations(vec![artifact::Operation {
                        name: "write".to_string(),
                        from: Type {
                            core: Core::Nat,
                            fields: Vec::new(),
                        },
                        to: Type {
                            core: Core::Unit,
                            fields: Vec::new(),
                        },
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
            functions: vec![artifact::Function {
                name: "f".to_string(),
                params: vec![Param {
                    temp: 0,
                    rep: Rep::Any,
                }],
                body: Block {
                    instrs: ops
                        .into_iter()
                        .enumerate()
                        .map(|(temp, op)| Instr {
                            temp: temp as u32,
                            rep: Rep::Any,
                            op,
                        })
                        .collect(),
                    end: End::Throw { tag: 1, value: 2 },
                },
            }],
            globals: vec![Global {
                name: "bundle@1.0.0::g".to_string(),
                body: Block {
                    instrs: Vec::new(),
                    end: End::Ret(0),
                },
            }],
        },
    };
    let printed = artifact::print(&artifact);
    assert_eq!(artifact::parse(&printed), artifact);
    assert_eq!(artifact::print(&artifact::parse(&printed)), printed);
}

#[test]
fn malformed_trusted_text_panics() {
    for text in [
        "",
        "(artifact)",
        "(artifact (header) (lir))",
        "(artifact (header (identity \"x\" \"1\") (values) (types) (effects)) (lir (functions) (globals)) trailing)",
    ] {
        assert!(
            catch_unwind(AssertUnwindSafe(|| artifact::parse(text))).is_err(),
            "{text:?}"
        );
    }
}
