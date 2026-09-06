use ruddy::{
    artifact as a, compile, inference,
    link::{self, LinkError},
    parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};

/// Hand-built link inputs cross the strict artifact boundary like a bundle
/// read from disk. Rendering canonical text recurses, and the relocation
/// regressions below nest thirty thousand blocks deep, so validation always
/// runs on a generously sized thread of its own.
fn validated(artifact: a::UncheckedArtifact) -> a::Artifact {
    std::thread::Builder::new()
        .name("artifact-validation".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || artifact.validate())
        .expect("the artifact validation thread starts")
        .join()
        .expect("artifact validation completes")
        .expect("a hand-built artifact validates")
}

fn artifact(
    name: &str,
    dependencies: &[(&str, &str)],
    functions: Vec<a::Function>,
    globals: Vec<a::Global>,
) -> a::Artifact {
    let values = globals
        .iter()
        .map(|global| a::Value {
            name: global.name.clone(),
            scheme: scheme(),
        })
        .collect();
    validated(a::UncheckedArtifact {
        header: a::Header {
            compiler: ruddy::artifact::Stamp::current(),
            identity: a::Identity {
                name: name.into(),
                version: "1.0.0".into(),
            },
            dependencies: dependencies
                .iter()
                .map(|(name, version)| a::Dependency {
                    name: (*name).into(),
                    version: (*version).into(),
                })
                .collect(),
            values,
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: vec![a::Extern {
                name: format!("{name}@1.0.0::host"),
                target: format!("{name}.host"),
                rep: a::Rep::Any,
            }],
            functions,
            globals,
        },
    })
}

fn scheme() -> a::Scheme {
    a::Scheme {
        count: 0,
        presences: 0,
        existentials: Vec::new(),
        formula: a::Formula::True,
        body: a::Type::Struct(a::Row {
            labels: vec![],
            rest: a::Rest::Closed,
        }),
    }
}

fn block(ops: Vec<a::Op>) -> a::Block {
    a::Block {
        instrs: ops
            .into_iter()
            .enumerate()
            .map(|(temp, op)| a::Instr {
                temp: temp as u32,
                rep: a::Rep::Any,
                op,
            })
            .collect(),
        end: a::End::Ret(0),
    }
}

fn function(name: &str, body: a::Block) -> a::Function {
    a::Function {
        name: name.into(),
        params: Vec::new(),
        body,
    }
}

fn global(name: &str, body: a::Block) -> a::Global {
    a::Global {
        name: name.into(),
        body,
    }
}

fn local_references() -> a::Block {
    block(vec![
        a::Op::Closure {
            func: 0,
            captures: vec![],
        },
        a::Op::Call {
            callee: a::Callee::Direct(0),
            args: vec![],
        },
    ])
}

#[test]
fn links_every_item_and_recursively_relocates_function_indices() {
    let nested = block(vec![
        a::Op::Catch {
            tag: 0,
            body: Box::new(local_references()),
        },
        a::Op::SwitchTag {
            on: 0,
            cases: vec![a::TagCase {
                name: "A".into(),
                block: local_references(),
            }],
            fallback: Some(Box::new(local_references())),
        },
        a::Op::SwitchPrim {
            on: 0,
            cases: vec![a::PrimCase {
                value: a::Literal::Boolean(true),
                block: local_references(),
            }],
            fallback: Some(Box::new(local_references())),
        },
        a::Op::SwitchTag {
            on: 0,
            cases: vec![],
            fallback: None,
        },
        a::Op::SwitchPrim {
            on: 0,
            cases: vec![],
            fallback: None,
        },
        a::Op::SwitchPresence {
            on: 0,
            field: "x".into(),
            present: Box::new(local_references()),
            absent: Box::new(local_references()),
        },
        a::Op::SwitchRest {
            on: 0,
            fields: vec![],
            none: Box::new(local_references()),
            some: Box::new(local_references()),
        },
        a::Op::Global {
            target: "dep@1.0.0::value".into(),
        },
    ]);
    let dep = artifact(
        "dep",
        &[],
        vec![function("dep-f", local_references())],
        vec![global("dep@1.0.0::value", local_references())],
    );
    let mut root = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![function("app-f", nested)],
        vec![global("app@1.0.0::main", local_references())],
    )
    .to_unchecked();
    // An existential witness is a presence the value's package owns, so the
    // scheme quantifies one presence and its body packages a label wearing it.
    root.header.values[0].scheme = a::Scheme {
        count: 1,
        presences: 1,
        existentials: vec![0],
        formula: a::Formula::True,
        body: a::Type::Package(Box::new(a::Type::Struct(a::Row {
            labels: vec![(
                "choice".into(),
                a::RowField {
                    presence: a::Presence::Bound(0),
                    ty: a::Type::Nat,
                },
            )],
            rest: a::Rest::Closed,
        }))),
    };
    root.header.types.push(a::DeclaredType {
        name: "app@1.0.0::Public".into(),
        params: vec![],
        scheme: scheme(),
    });

    let root = validated(root);
    let linked = link::link(&[dep, root.clone()]).unwrap();
    assert_eq!(linked.header().identity, root.header().identity);
    assert_eq!(linked.header().values, root.header().values);
    assert_eq!(linked.header().values[0].scheme.existentials, vec![0]);
    assert_eq!(linked.header().types, root.header().types);
    assert!(linked.header().dependencies.is_empty());
    assert_eq!(linked.lir().externs.len(), 2);
    assert_eq!(linked.lir().externs[0].name, "dep@1.0.0::host");
    assert_eq!(linked.lir().externs[1].name, "app@1.0.0::host");
    assert_eq!(linked.lir().functions.len(), 2);
    assert_eq!(linked.lir().globals.len(), 2);
    assert_eq!(linked.lir().globals[0].name, "dep@1.0.0::value");

    fn assert_relocated(block: &a::Block, expected: u64) {
        for instr in &block.instrs {
            match &instr.op {
                a::Op::Closure { func, .. } => assert_eq!(*func, expected),
                a::Op::Call {
                    callee: a::Callee::Direct(func),
                    ..
                } => assert_eq!(*func, expected),
                a::Op::Catch { body, .. } => assert_relocated(body, expected),
                a::Op::SwitchTag {
                    cases, fallback, ..
                } => {
                    for case in cases {
                        assert_relocated(&case.block, expected);
                    }
                    if let Some(fallback) = fallback {
                        assert_relocated(fallback, expected);
                    }
                }
                a::Op::SwitchPrim {
                    cases, fallback, ..
                } => {
                    for case in cases {
                        assert_relocated(&case.block, expected);
                    }
                    if let Some(fallback) = fallback {
                        assert_relocated(fallback, expected);
                    }
                }
                a::Op::SwitchPresence {
                    present, absent, ..
                } => {
                    assert_relocated(present, expected);
                    assert_relocated(absent, expected);
                }
                a::Op::SwitchRest { none, some, .. } => {
                    assert_relocated(none, expected);
                    assert_relocated(some, expected);
                }
                _ => {}
            }
        }
    }
    assert_relocated(&linked.lir().functions[0].body, 0);
    assert_relocated(&linked.lir().globals[0].body, 0);
    assert_relocated(&linked.lir().functions[1].body, 1);
    assert_relocated(&linked.lir().globals[1].body, 1);
}

#[test]
fn relocates_thirty_thousand_nested_catches_and_switches_iteratively_in_order() {
    const DEPTH: usize = 30_000;

    let mut nested = local_references();
    for depth in (0..DEPTH).rev() {
        nested = match depth % 5 {
            0 => block(vec![a::Op::Catch {
                tag: depth as u32,
                body: Box::new(nested),
            }]),
            1 => block(vec![a::Op::SwitchTag {
                on: depth as u32,
                cases: vec![
                    a::TagCase {
                        name: "nested".into(),
                        block: nested,
                    },
                    a::TagCase {
                        name: "sibling".into(),
                        block: local_references(),
                    },
                ],
                fallback: Some(Box::new(local_references())),
            }]),
            2 => block(vec![a::Op::SwitchPrim {
                on: depth as u32,
                cases: vec![
                    a::PrimCase {
                        value: a::Literal::Natural(depth as u64),
                        block: nested,
                    },
                    a::PrimCase {
                        value: a::Literal::Boolean(true),
                        block: local_references(),
                    },
                ],
                fallback: Some(Box::new(local_references())),
            }]),
            3 => block(vec![a::Op::SwitchPresence {
                on: depth as u32,
                field: "field".into(),
                present: Box::new(nested),
                absent: Box::new(local_references()),
            }]),
            _ => block(vec![a::Op::SwitchRest {
                on: depth as u32,
                fields: vec!["field".into()],
                none: Box::new(nested),
                some: Box::new(local_references()),
            }]),
        };
    }

    let dep = artifact(
        "dep",
        &[],
        vec![function("dep", local_references())],
        vec![],
    );
    let root = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![function("root", nested)],
        vec![],
    );
    let linked = link::link(&[dep, root]).unwrap();

    fn assert_references(block: &a::Block) {
        let a::Op::Closure { func, .. } = &block.instrs[0].op else {
            panic!("expected closure")
        };
        assert_eq!(*func, 1);
        let a::Op::Call {
            callee: a::Callee::Direct(func),
            ..
        } = &block.instrs[1].op
        else {
            panic!("expected direct call")
        };
        assert_eq!(*func, 1);
    }

    let mut current = &linked.lir().functions[1].body;
    for depth in 0..DEPTH {
        assert_eq!(current.instrs.len(), 1);
        current = match (&current.instrs[0].op, depth % 5) {
            (a::Op::Catch { tag, body }, 0) => {
                assert_eq!(*tag, depth as u32);
                body
            }
            (
                a::Op::SwitchTag {
                    on,
                    cases,
                    fallback,
                },
                1,
            ) => {
                assert_eq!(*on, depth as u32);
                assert_eq!(cases[0].name, "nested");
                assert_eq!(cases[1].name, "sibling");
                assert_references(&cases[1].block);
                assert_references(fallback.as_deref().unwrap());
                &cases[0].block
            }
            (
                a::Op::SwitchPrim {
                    on,
                    cases,
                    fallback,
                },
                2,
            ) => {
                assert_eq!(*on, depth as u32);
                assert_eq!(cases[0].value, a::Literal::Natural(depth as u64));
                assert_eq!(cases[1].value, a::Literal::Boolean(true));
                assert_references(&cases[1].block);
                assert_references(fallback.as_deref().unwrap());
                &cases[0].block
            }
            (
                a::Op::SwitchPresence {
                    on,
                    field,
                    present,
                    absent,
                },
                3,
            ) => {
                assert_eq!(*on, depth as u32);
                assert_eq!(field, "field");
                assert_references(absent);
                present
            }
            (
                a::Op::SwitchRest {
                    on,
                    fields,
                    none,
                    some,
                },
                4,
            ) => {
                assert_eq!(*on, depth as u32);
                assert_eq!(fields, &["field"]);
                assert_references(some);
                none
            }
            _ => panic!("unexpected nested operation at depth {depth}"),
        };
    }
    assert_references(current);
}

#[test]
fn copies_valid_dependency_first_transitive_diamond_graph_without_pruning() {
    let base = artifact(
        "base",
        &[],
        vec![function("base", local_references())],
        vec![],
    );
    let left = artifact(
        "left",
        &[("base", "1.0.0")],
        vec![function("left", local_references())],
        vec![],
    );
    let right = artifact(
        "right",
        &[("base", "1.0.0")],
        vec![function("right", local_references())],
        vec![],
    );
    let root = artifact(
        "app",
        &[("left", "1.0.0"), ("right", "1.0.0")],
        vec![function("root", local_references())],
        vec![],
    );

    let linked = link::link(&[base, left, right, root]).unwrap();
    let names: Vec<_> = linked
        .lir()
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert_eq!(names, ["base", "left", "right", "root"]);
    assert_eq!(
        linked
            .lir()
            .externs
            .iter()
            .map(|external| external.name.as_str())
            .collect::<Vec<_>>(),
        [
            "base@1.0.0::host",
            "left@1.0.0::host",
            "right@1.0.0::host",
            "app@1.0.0::host"
        ]
    );
    for (index, function) in linked.lir().functions.iter().enumerate() {
        assert_references(&function.body, index as u64);
    }

    fn assert_references(block: &a::Block, expected: u64) {
        let a::Op::Closure { func, .. } = &block.instrs[0].op else {
            panic!()
        };
        assert_eq!(*func, expected);
        let a::Op::Call {
            callee: a::Callee::Direct(func),
            ..
        } = &block.instrs[1].op
        else {
            panic!()
        };
        assert_eq!(*func, expected);
    }
}

fn compiled(source: &str) -> a::Artifact {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".into(), source.into());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("app", Version::new(1, 0, 0)).unwrap();
    compile::compile(Mint::new(bundle), parsed.stmts, inference::Trace::Off)
        .unwrap_or_else(|partial| panic!("{partial:#?}"))
        .artifact()
        .clone()
}

#[test]
fn links_unexported_wildcard_definitions_without_aliasing() {
    let artifact = compiled("let _ = 1n\nlet _ = 2n\nlet keep = 3n\nlet _ = keep");
    assert_eq!(artifact.header().values.len(), 1);
    assert_eq!(artifact.header().values[0].name, "app@1.0.0::keep");

    let names: std::collections::HashSet<_> = artifact
        .lir()
        .globals
        .iter()
        .map(|global| global.name.as_str())
        .collect();
    assert_eq!(names.len(), 4);

    let linked = link::link(&[artifact]).unwrap();
    assert_eq!(linked.header().values.len(), 1);
    assert_eq!(linked.lir().globals.len(), 4);
}

#[test]
fn empty_input_reports_the_only_link_error() {
    let error = link::link(&[]).unwrap_err();
    assert_eq!(error, LinkError::EmptyGraph);
    assert_eq!(error.to_string(), "cannot link an empty artifact graph");
    let _: &dyn std::error::Error = &error;
}
