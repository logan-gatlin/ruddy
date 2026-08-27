use ruddy::{
    artifact as a, inference, ir,
    link::{self, DeclarationNamespace, LinkError},
    lir, parse, patterns,
    symbol::{Bundle, Mint, Namespace, Version},
    token,
    tracking::FileManager,
};

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
            values,
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

fn compiled(source: &str) -> a::Artifact {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".into(), source.into());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("app", Version::new(1, 0, 0)).unwrap();
    let mut mint = Mint::new(bundle);
    let mut program = ir::build(&mut mint, parsed.stmts).program;
    let inferred = inference::infer(&mint, &mut program);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    let checked = patterns::check(&program, &inferred);
    assert!(checked.errors.is_empty(), "{:#?}", checked.errors);
    let lowered = lir::lower(&mint, &program, &inferred);
    a::Artifact::build(&mint, &program, &inferred, &lowered)
}

fn scheme() -> a::Scheme {
    a::Scheme {
        count: 0,
        presences: 0,
        formula: a::Formula::True,
        body: a::Type {
            core: a::Core::Unit,
            fields: vec![],
        },
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
    let root = artifact(
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
fn links_multiple_top_level_wildcard_definitions_without_aliasing_them() {
    let artifact = compiled("let _ = 1n\nlet _ = 2n\nlet keep = 3n\nlet _ = keep");
    assert_eq!(artifact.header.values.len(), 4);
    assert_eq!(artifact.lir.globals.len(), 4);

    let names: std::collections::HashSet<_> = artifact
        .lir
        .globals
        .iter()
        .map(|global| global.name.as_str())
        .collect();
    assert_eq!(names.len(), 4, "every discarded initializer stays distinct");
    for value in &artifact.header.values {
        assert!(names.contains(value.name.as_str()));
    }

    let linked = link::link(&[artifact]).expect("compiler-produced wildcard names are portable");
    assert_eq!(linked.lir.globals.len(), 4);
}

#[test]
fn rejects_header_value_and_lir_global_mismatches_per_input() {
    let mut missing = artifact(
        "dep",
        &[],
        vec![],
        vec![global("dep@1.0.0::implemented", block(vec![]))],
    );
    missing.lir.globals.clear();
    let root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
    let error = link::link(&[missing, root]).unwrap_err();
    assert_eq!(
        error,
        LinkError::ValueWithoutGlobal {
            owner: "dep@1.0.0".into(),
            name: "dep@1.0.0::implemented".into(),
        }
    );
    assert_eq!(
        error.to_string(),
        "artifact `dep@1.0.0` declares public value `dep@1.0.0::implemented` without a matching LIR global"
    );

    let mut undeclared = artifact(
        "dep",
        &[],
        vec![],
        vec![global("dep@1.0.0::hidden", block(vec![]))],
    );
    undeclared.header.values.clear();
    let root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
    let error = link::link(&[undeclared, root]).unwrap_err();
    assert_eq!(
        error,
        LinkError::GlobalWithoutValue {
            owner: "dep@1.0.0".into(),
            name: "dep@1.0.0::hidden".into(),
        }
    );
    assert_eq!(
        error.to_string(),
        "artifact `dep@1.0.0` defines LIR global `dep@1.0.0::hidden` without a matching public value declaration"
    );
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

    let noncanonical_identity = artifact("app", &[], vec![], vec![]);
    let mut noncanonical_identity = noncanonical_identity;
    noncanonical_identity.header.identity.version = "1".into();
    assert!(matches!(
        link::link(&[noncanonical_identity]),
        Err(LinkError::MalformedIdentity { .. })
    ));

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
fn validates_public_declarations_in_every_namespace_and_artifact() {
    let mut valid = artifact("app", &[], vec![], vec![]);
    let shared = "app@1.0.0::Shared";
    valid.header.values.push(a::Value {
        name: shared.into(),
        scheme: scheme(),
    });
    valid.lir.globals.push(global(shared, block(Vec::new())));
    valid.header.types.push(a::DeclaredType {
        name: shared.into(),
        params: vec![],
        scheme: scheme(),
    });
    valid.header.effects.push(a::DeclaredEffect {
        name: shared.into(),
        identity: None,
        kind: a::EffectKind::Alias(vec![]),
    });
    link::link(&[valid]).expect("the same spelling is valid in distinct namespaces");

    for namespace in [
        DeclarationNamespace::Value,
        DeclarationNamespace::Type,
        DeclarationNamespace::Effect,
    ] {
        let make = |name: &str| {
            let mut artifact = artifact("dep", &[], vec![], vec![]);
            match namespace {
                DeclarationNamespace::Value => artifact.header.values.push(a::Value {
                    name: name.into(),
                    scheme: scheme(),
                }),
                DeclarationNamespace::Type => artifact.header.types.push(a::DeclaredType {
                    name: name.into(),
                    params: vec![],
                    scheme: scheme(),
                }),
                DeclarationNamespace::Effect => artifact.header.effects.push(a::DeclaredEffect {
                    name: name.into(),
                    identity: None,
                    kind: a::EffectKind::Alias(vec![]),
                }),
            }
            artifact
        };

        let malformed = make("dep@1.0.0::bad-name");
        let root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
        assert!(matches!(
            link::link(&[malformed, root]),
            Err(LinkError::MalformedDeclaration { namespace: found, .. }) if found == namespace
        ));

        let wrong = make("other@1.0.0::name");
        let root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
        assert!(matches!(
            link::link(&[wrong, root]),
            Err(LinkError::WrongDeclarationOwner { namespace: found, .. }) if found == namespace
        ));

        let mut duplicate = make("dep@1.0.0::name");
        match namespace {
            DeclarationNamespace::Value => duplicate
                .header
                .values
                .push(duplicate.header.values[0].clone()),
            DeclarationNamespace::Type => duplicate
                .header
                .types
                .push(duplicate.header.types[0].clone()),
            DeclarationNamespace::Effect => duplicate
                .header
                .effects
                .push(duplicate.header.effects[0].clone()),
        }
        let root = artifact("app", &[("dep", "1.0.0")], vec![], vec![]);
        assert!(matches!(
            link::link(&[duplicate, root]),
            Err(LinkError::DuplicateDeclaration { namespace: found, .. }) if found == namespace
        ));
    }
}

#[test]
fn qualified_validation_is_namespace_sensitive_and_synthetic_names_are_exact() {
    let valid = compiled("let _ = 1n");
    let synthetic = valid.header.values[0].name.clone();
    assert!(synthetic.contains("::%_R"), "{synthetic}");
    link::link(&[valid]).unwrap();

    for bad in [
        "app@1.0.0::_",
        "app@1.0.0::%discard",
        "app@1.0.0::Module::%discard",
        "app@1.0.0::Module::_",
    ] {
        let input = artifact("app", &[], vec![], vec![global(bad, block(vec![]))]);
        assert!(
            matches!(link::link(&[input]), Err(LinkError::MalformedGlobal { .. })),
            "{bad}"
        );
    }

    for namespace in [DeclarationNamespace::Type, DeclarationNamespace::Effect] {
        for bad in ["app@1.0.0::_", "app@1.0.0::%hidden", synthetic.as_str()] {
            let mut input = artifact("app", &[], vec![], vec![]);
            match namespace {
                DeclarationNamespace::Type => input.header.types.push(a::DeclaredType {
                    name: bad.into(),
                    params: vec![],
                    scheme: scheme(),
                }),
                DeclarationNamespace::Effect => input.header.effects.push(a::DeclaredEffect {
                    name: bad.into(),
                    identity: None,
                    kind: a::EffectKind::Alias(vec![]),
                }),
                DeclarationNamespace::Value => unreachable!(),
            }
            assert!(
                matches!(
                    link::link(&[input]),
                    Err(LinkError::MalformedDeclaration { namespace: found, .. }) if found == namespace
                ),
                "{namespace:?}: {bad}"
            );
        }
    }

    let mut bad_value = artifact("app", &[], vec![], vec![]);
    bad_value.header.values.push(a::Value {
        name: "app@1.0.0::%hidden".into(),
        scheme: scheme(),
    });
    assert!(matches!(
        link::link(&[bad_value]),
        Err(LinkError::MalformedDeclaration {
            namespace: DeclarationNamespace::Value,
            ..
        })
    ));

    // A canonical local mangling is still invalid when embedded under the
    // wrong artifact owner, even if its textual owner prefix is rewritten.
    let forged = synthetic.replacen("app@1.0.0::", "other@1.0.0::", 1);
    let input = artifact("other", &[], vec![], vec![global(&forged, block(vec![]))]);
    assert!(matches!(
        link::link(&[input]),
        Err(LinkError::MalformedGlobal { .. })
    ));

    // Exercise the semantic checks inside a canonical mangling rather than
    // treating successful decoding alone as authorization for `%...`.
    let bundle = Bundle::new("app", Version::new(1, 0, 0)).unwrap();
    let mut mint = Mint::new(bundle);
    let module = mint.module(None, "Good").unwrap();
    let local = mint.local(Some(module), Namespace::Terms, "%discard");
    let nested = format!("app@1.0.0::%{}", mint.mangle(local));
    let input = artifact("app", &[], vec![], vec![global(&nested, block(vec![]))]);
    link::link(&[input]).expect("a local under a source-valid module is canonical");

    let wrong_namespace = mint.local(None, Namespace::Types, "%discard");
    let bad_module = mint.module(None, "bad-name").unwrap();
    let under_bad_module = mint.local(Some(bad_module), Namespace::Terms, "%discard");
    let wrong_parent_namespace = nested.replacen("M4Good", "T4Good", 1);
    let local_parent = nested.replacen("M4Good", "M4Goods0", 1);
    for mangled in [
        mint.mangle(wrong_namespace),
        mint.mangle(under_bad_module),
        wrong_parent_namespace
            .strip_prefix("app@1.0.0::%")
            .unwrap()
            .into(),
        local_parent.strip_prefix("app@1.0.0::%").unwrap().into(),
    ] {
        let name = format!("app@1.0.0::%{mangled}");
        let input = artifact("app", &[], vec![], vec![global(&name, block(vec![]))]);
        assert!(matches!(
            link::link(&[input]),
            Err(LinkError::MalformedGlobal { .. })
        ));
    }
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
        LinkError::ValueWithoutGlobal {
            owner: "a@1.0.0".into(),
            name: "a@1.0.0::x".into(),
        },
        LinkError::GlobalWithoutValue {
            owner: "a@1.0.0".into(),
            name: "a@1.0.0::x".into(),
        },
        LinkError::MalformedDeclaration {
            namespace: DeclarationNamespace::Value,
            name: "bad".into(),
        },
        LinkError::WrongDeclarationOwner {
            namespace: DeclarationNamespace::Type,
            name: "b@1.0.0::T".into(),
            owner: "a@1.0.0".into(),
        },
        LinkError::DuplicateDeclaration {
            namespace: DeclarationNamespace::Effect,
            name: "a@1.0.0::E".into(),
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
