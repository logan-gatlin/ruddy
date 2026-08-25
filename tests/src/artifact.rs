//! Tests for the span-free, canonical bundle artifact.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ruddy::{
    artifact::{
        self, Artifact, Block, Callee, Core, End, Formula, Global, Instr, Lir, Literal, Op, Param,
        Presence, Rep, Rest, Row, RowField, Scheme, Type,
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

fn plain(core: Core) -> Type {
    Type {
        core,
        fields: Vec::new(),
    }
}

fn field(presence: Presence, core: Core) -> RowField {
    RowField {
        presence,
        ty: plain(core),
    }
}

fn rich(core: Core) -> Type {
    Type {
        core,
        fields: vec![
            ("present".to_string(), field(Presence::Present, Core::Unit)),
            ("absent".to_string(), field(Presence::Absent, Core::Nat)),
            ("var".to_string(), field(Presence::Var(1), Core::Int)),
            ("bound".to_string(), field(Presence::Bound(2), Core::Real)),
            (
                "undecided".to_string(),
                field(Presence::Undecided, Core::String),
            ),
        ],
    }
}

fn row(rest: Rest) -> Row {
    Row {
        labels: vec![("label".to_string(), field(Presence::Present, Core::Boolean))],
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
        rich(Core::Unit),
        rich(Core::Nat),
        rich(Core::Int),
        rich(Core::Real),
        rich(Core::String),
        rich(Core::Boolean),
        rich(Core::Arrow(
            Box::new(plain(Core::Var(7))),
            Box::new(plain(Core::Bound(8))),
            row(Rest::More(Box::new(row(Rest::Rigid {
                id: 9,
                name: "tail".to_string(),
            })))),
        )),
        rich(Core::Sum(row(Rest::Closed))),
        rich(Core::Sum(row(Rest::Var(10)))),
        rich(Core::Sum(row(Rest::Bound(11)))),
        rich(Core::Sum(row(Rest::Undecided))),
        rich(Core::Var(12)),
        rich(Core::Bound(13)),
        rich(Core::Rigid {
            id: 14,
            name: "rigid".to_string(),
        }),
        rich(Core::Named {
            name: "other@1.2.3::T".to_string(),
            args: vec![plain(Core::Nat), plain(Core::Undecided)],
        }),
        rich(Core::Undecided),
    ];
    let values = types
        .into_iter()
        .enumerate()
        .map(|(index, body)| artifact::Value {
            name: format!("bundle@1.0.0::value-{index}"),
            scheme: Scheme {
                count: 15,
                presences: 7,
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
                        formula: Formula::True,
                        body: plain(Core::Unit),
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
                        formula: Formula::Var(0),
                        body: plain(Core::Sum(row(Rest::Closed))),
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
                        formula: Formula::Bound(1),
                        body: plain(Core::Arrow(
                            Box::new(plain(Core::Nat)),
                            Box::new(plain(Core::Unit)),
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
                        name: "write".to_string(),
                        from: plain(Core::Nat),
                        to: plain(Core::Unit),
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

fn assert_round_trip(value: &Artifact) -> String {
    let printed = value.print();
    let parsed = Artifact::parse(&printed);
    assert_eq!(parsed, *value);
    assert_eq!(artifact::print(&parsed), printed);
    assert_eq!(artifact::parse(&printed), parsed);
    printed
}

fn assert_malformed(text: &str) {
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
        !printed.contains("Span"),
        "source spans must not cross the disk boundary"
    );
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
         \x20 (lir (functions) (globals)))\n"
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
         \x20 (lir (functions) (globals)))\n"
    );
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
fn canonical_text_escapes_and_parses_every_control_character() {
    let controls: String = (0..=0x1f)
        .chain(0x7f..=0x9f)
        .map(|code| char::from_u32(code).unwrap())
        .collect();
    let escaped = format!("{controls}\\");
    let mut artifact = model_artifact();
    artifact.header.identity.name = escaped.clone();
    artifact.header.values[0].scheme.body = plain(Core::Arrow(
        Box::new(plain(Core::Unit)),
        Box::new(plain(Core::Unit)),
        Row {
            labels: vec![(controls.clone(), field(Presence::Present, Core::Unit))],
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
fn malformed_trusted_text_paths_arities_and_tags_panic() {
    let valid = compact(&assert_round_trip(&model_artifact()));
    for text in [
        "",
        "(",
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
        ("(fields ", "(record-fields "),
        ("(ty unit ", "(ty void "),
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
        ("(ty unit (fields", "(ty unit extra (fields"),
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
