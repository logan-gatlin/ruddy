use ruddy::{
    artifact as a, compile, inference,
    link::{self, LinkError},
    parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};

/// Link inputs cross the same strict artifact boundary as bundles read from disk.
fn validated(artifact: a::UncheckedArtifact) -> a::Artifact {
    artifact
        .validate()
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
            metadata: Default::default(),
            name: global.name.clone(),
            scheme: scheme(),
        })
        .collect();
    validated(a::UncheckedArtifact {
        header: a::Header {
            kind: ruddy::artifact::Kind::Library,
            compiler: ruddy::artifact::Stamp::current(),
            modules: Vec::new(),
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
        callable: None,
        representations: Vec::new(),
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

const K: u32 = 999;
fn block(ops: Vec<a::Op>) -> a::Block {
    a::Block {
        params: vec![a::Param {
            temp: K,
            rep: a::Rep::Cont,
        }],
        result: None,
        instrs: ops
            .into_iter()
            .enumerate()
            .map(|(temp, op)| a::Instr {
                temp: temp as u32,
                rep: if matches!(op, a::Op::Closure { .. }) {
                    a::Rep::Fn
                } else {
                    a::Rep::Any
                },
                op,
            })
            .collect(),
        end: a::End::Continue {
            continuation: K,
            value: 0,
        },
    }
}
fn function(name: &str, body: a::Block) -> a::Function {
    a::Function {
        suspension: ruddy::lir::Suspension::MaySuspend,
        name: name.into(),
        params: vec![],
        continuation: K,
        entry: 0,
        blocks: vec![body],
    }
}
fn local_references() -> a::Block {
    let mut b = block(vec![a::Op::Closure {
        func: 0,
        captures: vec![],
    }]);
    b.end = a::End::Call {
        callee: a::Callee::Direct(0),
        args: vec![],
        continuation: K,
    };
    b
}
fn continuation_function(name: &str, width: usize) -> a::Function {
    let mut f = function(name, local_references());
    for index in 0..width {
        let mut b = local_references();
        b.instrs.push(a::Instr {
            temp: 1,
            rep: a::Rep::Cont,
            op: a::Op::Continuation {
                code: a::CodeRef {
                    function: 0,
                    block: (index + 1) as u64,
                },
                captures: vec![K],
            },
        });
        b.end = a::End::Call {
            callee: a::Callee::Direct(0),
            args: vec![],
            continuation: 1,
        };
        f.blocks[index] = b;
        f.blocks.push(a::Block {
            params: vec![
                a::Param {
                    temp: K,
                    rep: a::Rep::Cont,
                },
                a::Param {
                    temp: 2,
                    rep: a::Rep::Any,
                },
            ],
            result: Some(2),
            instrs: vec![],
            end: a::End::Continue {
                continuation: K,
                value: 2,
            },
        });
        // Entries reached by saved continuations receive the operation result last.
        if index > 0 {
            f.blocks[index].params.push(a::Param {
                temp: 2,
                rep: a::Rep::Any,
            });
            f.blocks[index].result = Some(2);
        }
    }
    f
}

#[test]
fn links_every_item_and_relocates_calls_continuations_and_initializers() {
    let dep = artifact(
        "dep",
        &[],
        vec![continuation_function("dep-f", 2)],
        vec![a::Global {
            type_interface: None,
            adapter: None,
            callable: None,
            name: "dep@1.0.0::value".into(),
            initializer: 0,
        }],
    );
    let root = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![continuation_function("app-f", 2)],
        vec![a::Global {
            type_interface: None,
            adapter: None,
            callable: None,
            name: "app@1.0.0::main".into(),
            initializer: 0,
        }],
    );
    let mut root = root.to_unchecked();
    // An existential witness is a presence the value's package owns, so the
    // scheme quantifies one presence and its body packages a label wearing it.
    root.header.values[0].scheme = a::Scheme {
        callable: None,
        representations: Vec::new(),
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
        exported: true,
        metadata: Default::default(),
        name: "app@1.0.0::Public".into(),
        params: vec![],
        scheme: scheme(),
    });

    let root = validated(root);
    // Each bundle survives independent persistence before final numbering exists.
    let dep = a::Artifact::try_parse(&dep.print())
        .unwrap()
        .validate()
        .unwrap();
    let root = a::Artifact::try_parse(&root.print())
        .unwrap()
        .validate()
        .unwrap();
    let linked = link::link(&[dep, root.clone()]).unwrap();
    assert_eq!(linked.header().identity, root.header().identity);
    assert_eq!(linked.header().values, root.header().values);
    assert_eq!(linked.header().types, root.header().types);
    assert!(linked.header().dependencies.is_empty());
    assert_eq!(linked.lir().externs.len(), 2);
    assert_eq!(linked.lir().globals.len(), 2);
    assert_eq!(linked.lir().functions.len(), 2);
    for (offset, function) in linked.lir().functions.iter().enumerate() {
        assert_eq!(linked.lir().globals[offset].initializer, offset as u64);
        for (index, block) in function.blocks.iter().take(2).enumerate() {
            assert_eq!(
                block.instrs[0].op,
                a::Op::Closure {
                    func: offset as u64,
                    captures: vec![]
                }
            );
            assert_eq!(
                block.instrs[1].op,
                a::Op::Continuation {
                    code: a::CodeRef {
                        function: offset as u64,
                        block: (index + 1) as u64
                    },
                    captures: vec![K]
                }
            );
            assert!(
                matches!(block.end, a::End::Call { callee: a::Callee::Direct(f), .. } if f == offset as u64)
            );
        }
    }
}

#[test]
fn relocates_thirty_thousand_blocks_preserving_local_destinations() {
    const WIDTH: usize = 30_000;
    let dep = artifact(
        "dep",
        &[],
        vec![function("dep", local_references())],
        vec![],
    );
    let root = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![continuation_function("root", WIDTH)],
        vec![],
    );
    let linked = link::link(&[dep, root]).unwrap();
    let f = &linked.lir().functions[1];
    assert_eq!(f.blocks.len(), WIDTH + 1);
    for (index, b) in f.blocks.iter().take(WIDTH).enumerate() {
        assert_eq!(
            b.instrs[0].op,
            a::Op::Closure {
                func: 1,
                captures: vec![]
            }
        );
        assert_eq!(
            b.instrs[1].op,
            a::Op::Continuation {
                code: a::CodeRef {
                    function: 1,
                    block: (index + 1) as u64
                },
                captures: vec![K]
            }
        );
        assert!(matches!(
            b.end,
            a::End::Call {
                callee: a::Callee::Direct(1),
                ..
            }
        ));
    }
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
        assert_references(&function.blocks[function.entry as usize], index as u64);
    }

    fn assert_references(block: &a::Block, expected: u64) {
        let a::Op::Closure { func, .. } = &block.instrs[0].op else {
            panic!()
        };
        assert_eq!(*func, expected);
        let a::End::Call {
            callee: a::Callee::Direct(func),
            ..
        } = &block.end
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
