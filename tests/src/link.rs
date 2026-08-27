use ruddy::{
    artifact as a,
    link::{self, LinkError},
};

fn artifact(
    name: &str,
    dependencies: &[(&str, &str)],
    functions: Vec<a::Function>,
    globals: Vec<a::Global>,
) -> a::Artifact {
    a::Artifact {
        header: a::Header {
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
            values: Vec::new(),
            types: Vec::new(),
            effects: Vec::new(),
        },
        lir: a::Lir { functions, globals },
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

#[test]
fn links_every_item_and_recursively_relocates_function_indices() {
    let leaf = || {
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
    };
    let root_body = block(vec![
        a::Op::Catch {
            tag: 0,
            body: Box::new(leaf()),
        },
        a::Op::SwitchTag {
            on: 0,
            cases: vec![a::TagCase {
                name: "A".into(),
                block: leaf(),
            }],
            fallback: Some(Box::new(leaf())),
        },
        a::Op::SwitchPrim {
            on: 0,
            cases: vec![a::PrimCase {
                value: a::Literal::Boolean(true),
                block: leaf(),
            }],
            fallback: Some(Box::new(leaf())),
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
            present: Box::new(leaf()),
            absent: Box::new(leaf()),
        },
        a::Op::SwitchRest {
            on: 0,
            fields: vec![],
            none: Box::new(leaf()),
            some: Box::new(leaf()),
        },
        a::Op::Global {
            target: "dep@1.0.0::value".into(),
        },
    ]);
    let dep = artifact(
        "dep",
        &[],
        vec![function("dep-f", leaf())],
        vec![global(
            "dep@1.0.0::value",
            block(vec![a::Op::Closure {
                func: 0,
                captures: vec![],
            }]),
        )],
    );
    let mut root = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![function("app-f", root_body)],
        vec![global(
            "app@1.0.0::main",
            block(vec![a::Op::Call {
                callee: a::Callee::Direct(0),
                args: vec![],
            }]),
        )],
    );
    root.header.values.push(a::Value {
        name: "app@1.0.0::main".into(),
        scheme: a::Scheme {
            count: 0,
            presences: 0,
            formula: a::Formula::True,
            body: a::Type {
                core: a::Core::Unit,
                fields: vec![],
            },
        },
    });

    let linked = link::link(&[dep, root.clone()]).unwrap();
    assert_eq!(linked.header.identity, root.header.identity);
    assert_eq!(linked.header.values, root.header.values);
    assert!(linked.header.dependencies.is_empty());
    assert_eq!(linked.lir.functions.len(), 2);
    assert_eq!(linked.lir.globals.len(), 2);

    fn assert_relocated(block: &a::Block) {
        for instr in &block.instrs {
            match &instr.op {
                a::Op::Closure { func, .. } => assert_eq!(*func, 1),
                a::Op::Call {
                    callee: a::Callee::Direct(func),
                    ..
                } => assert_eq!(*func, 1),
                a::Op::Catch { body, .. } => assert_relocated(body),
                a::Op::SwitchTag {
                    cases, fallback, ..
                } => {
                    for case in cases {
                        assert_relocated(&case.block);
                    }
                    if let Some(fallback) = fallback {
                        assert_relocated(fallback);
                    }
                }
                a::Op::SwitchPrim {
                    cases, fallback, ..
                } => {
                    for case in cases {
                        assert_relocated(&case.block);
                    }
                    if let Some(fallback) = fallback {
                        assert_relocated(fallback);
                    }
                }
                a::Op::SwitchPresence {
                    present, absent, ..
                } => {
                    assert_relocated(present);
                    assert_relocated(absent);
                }
                a::Op::SwitchRest { none, some, .. } => {
                    assert_relocated(none);
                    assert_relocated(some);
                }
                _ => {}
            }
        }
    }
    assert_relocated(&linked.lir.functions[1].body);
    assert_relocated(&linked.lir.globals[1].body);
    // Dependency-local references retain their original zero offset.
    let a::Op::Closure { func, .. } = linked.lir.functions[0].body.instrs[0].op else {
        panic!()
    };
    assert_eq!(func, 0);
}

#[test]
fn rejects_invalid_graph_structure() {
    let empty: Vec<a::Artifact> = vec![];
    assert_eq!(link::link(&empty), Err(LinkError::EmptyGraph));

    for (name, version) in [
        ("dep", "bad"),
        ("dep", "1.0.0+build"),
        ("not.a.bundle", "1.0.0"),
    ] {
        let mut malformed_identity = artifact(name, &[], vec![], vec![]);
        malformed_identity.header.identity.version = version.into();
        assert!(matches!(
            link::link(&[malformed_identity]),
            Err(LinkError::MalformedIdentity { .. })
        ));
    }

    let malformed_dependency = artifact("app", &[("dep", "bad")], vec![], vec![]);
    assert!(matches!(
        link::link(&[malformed_dependency]),
        Err(LinkError::MalformedIdentity { .. })
    ));

    let dep = artifact("dep", &[], vec![], vec![]);
    let duplicate = artifact("dep", &[], vec![], vec![]);
    assert!(matches!(
        link::link(&[dep.clone(), duplicate]),
        Err(LinkError::DuplicateIdentity { .. })
    ));

    let missing = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
    assert!(matches!(
        link::link(&[missing]),
        Err(LinkError::MissingDependency { .. })
    ));

    let before = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
    assert!(matches!(
        link::link(&[before, dep.clone()]),
        Err(LinkError::DependencyAfterOwner { .. })
    ));

    let repeated = artifact("app", &[("dep", "1.0.0"), ("dep", "1.0.0")], vec![], vec![]);
    assert!(matches!(
        link::link(&[dep, repeated]),
        Err(LinkError::DuplicateDependency { .. })
    ));

    let orphan = artifact("orphan", &[], vec![], vec![]);
    let root = artifact("app", &[], vec![], vec![]);
    assert!(matches!(
        link::link(&[orphan, root]),
        Err(LinkError::UnreachableArtifact { .. })
    ));

    let first_root = artifact("first", &[], vec![], vec![]);
    let dep = artifact("dep", &[], vec![], vec![]);
    let final_root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
    assert!(matches!(
        link::link(&[first_root, dep, final_root]),
        Err(LinkError::UnreachableArtifact { .. })
    ));
}

#[test]
fn rejects_bad_global_definitions_and_references() {
    for name in [
        "plain",
        "plain::x",
        "@1::x",
        "app@::x",
        "app@1::",
        "app@1.0.0::",
        "app@1::x::",
        "app@1@2::x",
    ] {
        let input = artifact("app", &[], vec![], vec![global(name, block(vec![]))]);
        assert!(
            matches!(link::link(&[input]), Err(LinkError::MalformedGlobal { .. })),
            "{name}"
        );
    }
    let wrong = artifact(
        "app",
        &[],
        vec![],
        vec![global("other@1.0.0::x", block(vec![]))],
    );
    assert!(matches!(
        link::link(&[wrong]),
        Err(LinkError::WrongGlobalOwner { .. })
    ));

    let dep = artifact(
        "dep",
        &[],
        vec![],
        vec![global("dep@1.0.0::x", block(vec![]))],
    );
    let duplicate = artifact(
        "app",
        &[("dep", "1.0.0")],
        vec![],
        vec![global("dep@1.0.0::x", block(vec![]))],
    );
    // Owner validation wins before the cross-artifact collision.
    assert!(matches!(
        link::link(&[dep, duplicate]),
        Err(LinkError::WrongGlobalOwner { .. })
    ));

    let duplicated_within = artifact(
        "app",
        &[],
        vec![],
        vec![
            global("app@1.0.0::x", block(vec![])),
            global("app@1.0.0::x", block(vec![])),
        ],
    );
    assert!(matches!(
        link::link(&[duplicated_within]),
        Err(LinkError::DuplicateGlobal { .. })
    ));

    let missing = artifact(
        "app",
        &[],
        vec![],
        vec![global(
            "app@1.0.0::x",
            block(vec![a::Op::Global {
                target: "app@1.0.0::nope".into(),
            }]),
        )],
    );
    assert!(matches!(
        link::link(&[missing]),
        Err(LinkError::MissingGlobal { .. })
    ));
    let malformed = artifact(
        "app",
        &[],
        vec![],
        vec![global(
            "app@1.0.0::x",
            block(vec![a::Op::Global {
                target: "bad".into(),
            }]),
        )],
    );
    assert!(matches!(
        link::link(&[malformed]),
        Err(LinkError::MalformedGlobal { .. })
    ));

    let base = artifact(
        "base",
        &[],
        vec![],
        vec![global("base@1.0.0::x", block(vec![]))],
    );
    let middle = artifact("middle", &[("base", "1.0.0")], vec![], vec![]);
    let bypass = artifact(
        "app",
        &[("middle", "1.0.0")],
        vec![],
        vec![global(
            "app@1.0.0::x",
            block(vec![a::Op::Global {
                target: "base@1.0.0::x".into(),
            }]),
        )],
    );
    assert!(matches!(
        link::link(&[base, middle, bypass]),
        Err(LinkError::UndeclaredGlobalDependency { .. })
    ));
}

#[test]
fn every_link_error_has_a_user_facing_message() {
    let errors = [
        LinkError::EmptyGraph,
        LinkError::MalformedIdentity {
            identity: "bad".into(),
        },
        LinkError::DuplicateIdentity {
            identity: "a@1.0.0".into(),
        },
        LinkError::DuplicateDependency {
            owner: "a".into(),
            dependency: "b".into(),
        },
        LinkError::MissingDependency {
            owner: "a".into(),
            dependency: "b".into(),
        },
        LinkError::DependencyAfterOwner {
            owner: "a".into(),
            dependency: "b".into(),
        },
        LinkError::UnreachableArtifact {
            identity: "a@1.0.0".into(),
            root: "b@1.0.0".into(),
        },
        LinkError::MalformedGlobal { name: "bad".into() },
        LinkError::WrongGlobalOwner {
            name: "b@1.0.0::x".into(),
            owner: "a".into(),
        },
        LinkError::DuplicateGlobal {
            name: "a@1.0.0::x".into(),
        },
        LinkError::MissingGlobal {
            owner: "a".into(),
            target: "b@1.0.0::x".into(),
        },
        LinkError::UndeclaredGlobalDependency {
            owner: "a".into(),
            target: "b@1.0.0::x".into(),
        },
        LinkError::FunctionIndexOutOfBounds {
            owner: "a".into(),
            index: 2,
            functions: 1,
        },
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
        let _: &dyn std::error::Error = &error;
    }
}

#[test]
fn rejects_out_of_range_function_indices_in_functions_and_globals() {
    let bad_closure = artifact(
        "app",
        &[],
        vec![function(
            "f",
            block(vec![a::Op::Closure {
                func: 1,
                captures: vec![],
            }]),
        )],
        vec![],
    );
    assert!(matches!(
        link::link(&[bad_closure]),
        Err(LinkError::FunctionIndexOutOfBounds { .. })
    ));
    let bad_call = artifact(
        "app",
        &[],
        vec![],
        vec![global(
            "app@1.0.0::x",
            block(vec![a::Op::Call {
                callee: a::Callee::Direct(0),
                args: vec![],
            }]),
        )],
    );
    assert!(matches!(
        link::link(&[bad_call]),
        Err(LinkError::FunctionIndexOutOfBounds { .. })
    ));
}
