use ruddy::{
    artifact as a, inference, ir,
    link::{self, LinkError},
    lir, parse, patterns,
    symbol::{Bundle, Mint, Version},
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
    );
    root.header.types.push(a::DeclaredType {
        name: "app@1.0.0::Public".into(),
        params: vec![],
        scheme: scheme(),
    });

    let linked = link::link(&[dep, root.clone()]).unwrap();
    assert_eq!(linked.header.identity, root.header.identity);
    assert_eq!(linked.header.values, root.header.values);
    assert_eq!(linked.header.types, root.header.types);
    assert!(linked.header.dependencies.is_empty());
    assert_eq!(linked.lir.functions.len(), 2);
    assert_eq!(linked.lir.globals.len(), 2);
    assert_eq!(linked.lir.globals[0].name, "dep@1.0.0::value");

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
    assert_relocated(&linked.lir.functions[0].body, 0);
    assert_relocated(&linked.lir.globals[0].body, 0);
    assert_relocated(&linked.lir.functions[1].body, 1);
    assert_relocated(&linked.lir.globals[1].body, 1);
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
        .lir
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert_eq!(names, ["base", "left", "right", "root"]);
    for (index, function) in linked.lir.functions.iter().enumerate() {
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
    let mut mint = Mint::new(bundle);
    let mut program = ir::build(&mut mint, parsed.stmts).program;
    let inferred = inference::infer(&mint, &mut program);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    let checked = patterns::check(&program, &inferred);
    assert!(checked.errors.is_empty(), "{:#?}", checked.errors);
    let lowered = lir::lower(&mint, &program, &inferred);
    a::Artifact::build(&mint, &program, &inferred, &lowered)
}

#[test]
fn links_compiler_produced_wildcard_definitions_without_aliasing() {
    let artifact = compiled("let _ = 1n\nlet _ = 2n\nlet keep = 3n\nlet _ = keep");
    let names: std::collections::HashSet<_> = artifact
        .lir
        .globals
        .iter()
        .map(|global| global.name.as_str())
        .collect();
    assert_eq!(names.len(), 4);

    let linked = link::link(&[artifact]).unwrap();
    assert_eq!(linked.lir.globals.len(), 4);
}

#[test]
fn empty_input_reports_the_only_link_error() {
    let error = link::link(&[]).unwrap_err();
    assert_eq!(error, LinkError::EmptyGraph);
    assert_eq!(error.to_string(), "cannot link an empty artifact graph");
    let _: &dyn std::error::Error = &error;
}
