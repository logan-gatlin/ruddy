//! Tests for the public JavaScript backend API and its generated ESM boundary.

use std::{fs, process::Command};

use ruddy::{
    artifact::{self, Artifact},
    backend::js::{self, Error},
    compile, inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};

fn retag_numeric_global(
    artifact: &mut artifact::UncheckedArtifact,
    name: &str,
    rep: artifact::Rep,
) {
    let global = artifact
        .lir
        .globals
        .iter_mut()
        .find(|global| global.name.ends_with(&format!("::{name}")))
        .unwrap_or_else(|| panic!("no global named {name}"));
    for instr in artifact.lir.functions[global.initializer as usize]
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instrs)
    {
        if instr.rep == artifact::Rep::Real {
            instr.rep = rep;
        }
        if let artifact::Op::Const(artifact::Literal::Real(bits)) = &mut instr.op {
            let value = f64::from_bits(*bits);
            instr.op = artifact::Op::Const(match rep {
                artifact::Rep::Nat => artifact::Literal::Natural(value as u64),
                artifact::Rep::Int => artifact::Literal::Integer(value as i64),
                _ => panic!("numeric retag requires an integer representation"),
            });
        }
    }
}

fn split_record_into_merge(artifact: &mut artifact::UncheckedArtifact, name: &str) {
    let global = artifact
        .lir
        .globals
        .iter_mut()
        .find(|global| global.name.ends_with(&format!("::{name}")))
        .unwrap_or_else(|| panic!("no global named {name}"));
    let function = &mut artifact.lir.functions[global.initializer as usize];
    let body = &mut function.blocks[function.entry as usize];
    let combined = body.instrs.pop().expect("record constructor");
    let artifact::Op::Struct(fields) = &combined.op else {
        panic!("{name} does not end in a record")
    };
    let fields = fields.clone();
    assert_eq!(fields.len(), 2);
    let left_temp = combined.temp;
    let right_temp = left_temp + 1;
    let merged_temp = left_temp + 2;
    body.instrs.push(artifact::Instr {
        temp: left_temp,
        rep: artifact::Rep::Struct,
        op: artifact::Op::Struct(vec![fields[0].clone()]),
    });
    body.instrs.push(artifact::Instr {
        temp: right_temp,
        rep: artifact::Rep::Struct,
        op: artifact::Op::Struct(vec![fields[1].clone()]),
    });
    body.instrs.push(artifact::Instr {
        temp: merged_temp,
        rep: artifact::Rep::Struct,
        op: artifact::Op::Merge(vec![left_temp, right_temp]),
    });
    let artifact::End::Continue { value, .. } = &mut body.end else {
        panic!("record initializer must return")
    };
    *value = merged_temp;
}

fn retag_primitive_cases(
    artifact: &mut artifact::UncheckedArtifact,
    name: &str,
    rep: artifact::Rep,
) {
    for function in artifact
        .lir
        .functions
        .iter_mut()
        .filter(|f| f.name.contains(name) && !f.params.is_empty())
    {
        for param in &mut function.params {
            if param.rep == artifact::Rep::Real {
                param.rep = rep;
            }
        }
        let input = function.params[0].temp;
        for block in &mut function.blocks {
            for param in &mut block.params {
                if param.temp == input {
                    param.rep = rep;
                }
            }
            if let artifact::End::Branch {
                test: artifact::Test::Literal { value, .. },
                ..
            } = &mut block.end
            {
                let artifact::Literal::Real(bits) = value else {
                    panic!("expected a real case")
                };
                let number = f64::from_bits(*bits);
                *value = match rep {
                    artifact::Rep::Nat => artifact::Literal::Natural(number as u64),
                    artifact::Rep::Int => artifact::Literal::Integer(number as i64),
                    _ => panic!("primitive retag requires an integer representation"),
                };
            }
        }
    }
}

fn compiled(source: &str) -> Artifact {
    let mut files = FileManager::new();
    let file = files.register_new_file("<javascript-test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("app", Version::new(1, 0, 0)).unwrap();
    compile::compile(Mint::new(bundle), parsed.stmts, inference::Trace::Off)
        .unwrap_or_else(|partial| panic!("{:#?}", partial.errors))
        .artifact()
        .clone()
}

#[test]
fn reification_preserves_the_boxed_type_instead_of_the_javascript_numeric_representation() {
    let artifact = compiled(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
let boxed = upcast 42n
let natural: Option Nat = downcast boxed
let integer: Option Int = downcast boxed
let recovered = match natural with | #Some n => n | #None => 0n end
let rejected = match integer with | #Some _ => false | #None => true end
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("any.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.recovered, 42); assert.equal(app.rejected, true);",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_concrete_artifact_row_extensions_normalize_before_identity_comparison() {
    let mut artifact = compiled(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let unbox: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let saved = box { left: 1n, right: 2n }
@private let restored: Option { left: Nat, right: Nat } = unbox saved
let matched = match restored with | #Some _ => true | #None => false end
"#,
    )
    .to_unchecked();
    let descriptor = artifact.lir.functions.iter_mut()
        .flat_map(|f| &mut f.blocks).flat_map(|b| &mut b.instrs)
        .find_map(|instruction| match &mut instruction.op {
            artifact::Op::TypeDescriptor { template, arguments } if arguments.is_empty()
                && matches!(&template.nodes[0], ruddy::reification::Node::Struct(fields) if fields.len() == 2) => Some(template),
            _ => None,
        }).unwrap();
    let ruddy::reification::Node::Struct(fields) = descriptor.nodes[0].clone() else {
        unreachable!()
    };
    let base = descriptor.nodes.len() as u32;
    descriptor.nodes[0] = ruddy::reification::Node::Extend([base, base + 1]);
    descriptor
        .nodes
        .push(ruddy::reification::Node::Struct(vec![fields[0].clone()]));
    descriptor
        .nodes
        .push(ruddy::reification::Node::Struct(vec![fields[1].clone()]));
    let artifact = artifact
        .validate()
        .expect("a concrete guarded row extension is valid");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let output = Command::new("node").args(["--input-type=module", "--eval", &format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.matched, true);",
        serde_json::to_string(path.to_str().unwrap()).unwrap(),
    )]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_generalized_initializers_preserve_eager_allocation_identity() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private extern same: Any -> Any -> Bool = "a => b => a === b"
@private let make: 'a -> { boxed: Any, token: Any } = do
  let token = box 1n
  return fn x => { boxed: box x, token: token }
end
@private let first = make 2n
@private let second = make false
let shared = same first.token second.token
"#,
        "assert.equal(app.shared, true);",
    );
}

#[test]
fn reification_generalized_partial_application_evaluates_its_argument_once() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private extern same: Any -> Any -> Bool = "a => b => a === b"
@private let pair = fn token => fn x => { boxed: box x, token: token }
@private let partial = pair (box 1n)
@private let first = partial 2n
@private let second = partial false
let shared = same first.token second.token
"#,
        "assert.equal(app.shared, true);",
    );
}

#[test]
fn shared_rows_generic_descriptors_keep_structs_and_sums_distinct() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
type Both 'r = { product: { ..'r }, choice: | ..'r }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let unbox: Any -> Option 'a = fn any => match any with
| hide 'x { mirror: witness, value } => match same_pair (witness, mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let pack: Both { ..'r } -> { product: Any, choice: Any } = fn p => { product: box p.product, choice: box p.choice }
@private let packed = pack { product: { A: 7n }, choice: #A 8n }
@private let product: Option { A: Nat } = unbox packed.product
@private let choice: Option (#A Nat) = unbox packed.choice
@private let wrong: Option { A: Nat } = unbox packed.choice
let field = match product with | #Some p => p.A | #None => 0n end
let payload = match choice with | #Some (#A n) => n | #None => 0n end
let distinct = match wrong with | #Some _ => false | #None => true end
"#,
        "assert.equal(app.field, 7); assert.equal(app.payload, 8); assert.equal(app.distinct, true);",
    );
}

#[test]
fn reification_generic_boxing_helpers_and_higher_order_calls_preserve_each_instantiation() {
    let artifact = compiled(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let box: 'a -> Any = fn value => upcast value
@private let apply = fn f => fn x => f x
@private let forward = fn value => apply box value
@private let first: Option Nat = downcast (forward 17n)
let second: Option String = downcast (forward "seventeen")
let number = match first with | #Some n => n | #None => 0n end
let text = match second with | #Some s => s | #None => "failed" end
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generic-any.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.number, 17); assert.equal(app.text, 'seventeen');",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_survives_artifact_import_and_separate_compilation() {
    let producer = compiled(
        "type Any = hide 'a => { mirror: Mirror 'a, value: 'a }\n@private extern type_of: 'a -> Mirror 'a = \"$typeOf\"\n@private extern fresh_mirror: () -> Mirror 'a = \"$mirror\"\n@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = \"$sameMirror\"\ntype Option 'a = #Some 'a | #None\nlet box: 'a -> Any = fn value => { mirror: type_of value, value: value }\nlet unbox: Any -> Option 'a = fn any => match any with\n| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with\n  | #Some { forward, backward } => #Some (forward value)\n  | #None => #None\n  end\nend\nlet wrap = fn x => box x",
    );
    let producer = artifact::parse(&producer.print())
        .validate()
        .expect("the producer artifact round trips");
    let source = "let first: dep::Option Nat = dep::unbox (dep::wrap 23n)\nlet result = match first with | #Some n => n | #None => 0n end";
    let mut files = FileManager::new();
    let file = files.register_new_file("consumer.rud".into(), source.into());
    let parsed = parse::parse(token::lex(source, file).tokens);
    let consumer = compile::compile_with_dependencies(
        Mint::new(Bundle::new("consumer", Version::new(1, 0, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: compile::DependencyArtifact::Checked(&producer),
        }],
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{:#?}", partial.errors));
    let linked = ruddy::link::link(&[producer, consumer.artifact().clone()]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.mjs");
    fs::write(&path, js::generate(&linked).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.result, 23);",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A closure the library stores in a record field, hands to the consumer, and
/// receives back is invoked with the convention the library lowered, even
/// though the library never resolved the callback port the closure feeds.
#[test]
fn reification_sealed_callback_records_survive_separate_compilation() {
    let producer = compiled(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
effect Tick = () -> ()
type Box 'a = { run: 'a -> Any + !Tick }
type Box2 'a = { run: () -> Option 'a + !Tick }
type Maker 'a = hide 'm => { mirror: Mirror 'm, seed: 'm, make: 'm -> 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
let unbox: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
let map: ('a -> 'b) -> Option 'a -> Option 'b = fn f o => match o with | #Some x => #Some (f x) | #None => #None end
let make: Option () -> Option (Box 'a) = fn o => map (fn _ => { run: fn value => do let _ = !Tick () return box value end }) o
let run_box: Box 'a -> 'a -> Any = fn b value => handle b.run value with | !Tick _ => () end
let nat_maker: Maker Nat = { mirror: type_of 41n, seed: 41n, make: fn n => n }
let make_box: Maker 'a -> Box2 'a = fn maker => match maker with
| hide 'm { mirror, seed, make } => { run: fn _ => do let _ = !Tick () return map make (#Some seed) end }
end
let run_box2: Box2 'a -> Option 'a = fn b => handle b.run () with | !Tick _ => () end
"#,
    );
    let producer = artifact::parse(&producer.print())
        .validate()
        .expect("the producer artifact round trips");
    let source = r#"
let simple: () -> Nat = fn _ => match dep::make (#Some ()) with
| #Some b => match dep::unbox (dep::run_box b 23n) with | #Some n => n | #None => 0n end
| #None => 1n
end
let hidden: () -> Nat = fn _ => match dep::run_box2 (dep::make_box dep::nat_maker) with | #Some n => n | #None => 0n end
"#;
    let mut files = FileManager::new();
    let file = files.register_new_file("consumer.rud".into(), source.into());
    let parsed = parse::parse(token::lex(source, file).tokens);
    let consumer = compile::compile_with_dependencies(
        Mint::new(Bundle::new("consumer", Version::new(1, 0, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: compile::DependencyArtifact::Checked(&producer),
        }],
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{:#?}", partial.errors));
    let linked = ruddy::link::link(&[producer, consumer.artifact().clone()]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.mjs");
    fs::write(&path, js::generate(&linked).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(await app.simple(), 23); assert.equal(await app.hidden(), 41);",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A record holding a closure with a conditional descriptor slot passes
/// through a generalized definition's bare type parameter unchanged: the
/// definition only forwards it, and the callback receiving it plans the same
/// convention the producer lowered.
#[test]
fn reification_forwards_callables_through_bare_type_parameters() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
effect Tick = () -> ()
type Box 'a = { run: () -> Option 'a + !Tick }
type Pair 'a = { first: Nat, box: Box 'a }
type Maker 'a = hide 'm => { mirror: Mirror 'm, seed: 'm, make: 'm -> 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private let map: ('a -> 'b) -> Option 'a -> Option 'b = fn f o => match o with | #Some x => #Some (f x) | #None => #None end
@private let and_then: ('a -> Option 'b) -> Option 'a -> Option 'b = fn f o => match o with | #Some x => f x | #None => #None end
@private let nat_maker: Maker Nat = { mirror: type_of 41n, seed: 41n, make: fn n => n }
@private let make_box: Maker 'a -> Option (Box 'a) = fn maker => match maker with
| hide 'm { mirror, seed, make } => #Some { run: fn _ => do let _ = !Tick () return map make (#Some seed) end }
end
@private let make_pair: Maker 'a -> Option (Pair 'a) = fn maker =>
  and_then (fn first => map (fn box => { first: first, box: box }) (make_box maker)) (#Some 7n)
@private let run_box: Box 'a -> Option 'a = fn b => handle b.run () with | !Tick _ => () end
let direct = match make_box nat_maker with | #Some b => match run_box b with | #Some n => n | #None => 0n end | #None => 1n end
let paired = match make_pair nat_maker with | #Some p => match run_box p.box with | #Some n => n | #None => 0n end | #None => 1n end
"#,
        "assert.equal(app.direct, 41); assert.equal(app.paired, 41);",
    );
}

#[test]
fn reification_imported_recursive_descriptors_close_forwarding_arguments_and_reject_growth() {
    let original = compiled("type Id 'a = 'a\ntype Loop 'a = { next: Loop 'a }\nlet ready = true");
    for growing in [false, true] {
        let mut changed = original.clone().to_unchecked();
        let alias = changed
            .header
            .types
            .iter_mut()
            .find(|ty| ty.name.ends_with("::Loop"))
            .unwrap();
        let artifact::Type::Struct(row) = &mut alias.scheme.body else {
            panic!("Loop record")
        };
        let artifact::Type::Named { args, .. } = &mut row.labels[0].1.ty else {
            panic!("recursive application")
        };
        args[0] = if growing {
            artifact::Type::Array(Box::new(artifact::Type::Bound(0)))
        } else {
            artifact::Type::Named {
                name: "app@1.0.0::Id".into(),
                args: vec![artifact::Type::Bound(0)],
            }
        };
        let producer = changed
            .validate()
            .expect("a structurally checked portable alias graph");
        let source = "type Option 'a = #Some 'a | #None\ntype Any = hide 'a => { mirror: Mirror 'a, value: 'a }\n@private extern type_of: 'a -> Mirror 'a = \"$typeOf\"\n@private extern mirror: () -> Mirror 'a = \"$mirror\"\n@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = \"$sameMirror\"\n@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }\nlet wrap: dep::Loop Nat -> Any = fn x => box x";
        let mut files = FileManager::new();
        let file = files.register_new_file("consumer.rud".into(), source.into());
        let parsed = parse::parse(token::lex(source, file).tokens);
        let result = compile::compile_with_dependencies(
            Mint::new(Bundle::new("consumer", Version::new(1, 0, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Checked(&producer),
            }],
            inference::Trace::Off,
        );
        if growing {
            let failed = result.expect_err("a growing imported runtime identity is rejected");
            assert!(format!("{:?}", failed.errors).contains("RuntimeTypeInformation"));
        } else {
            let accepted = result.unwrap_or_else(|failed| panic!("{:?}", failed.errors));
            assert!(accepted.artifact().print().len() < 30_000);
        }
    }
}

#[test]
fn reification_generic_extern_identity_converts_native_arrays_in_both_directions() {
    let artifact = compiled(
        r#"
@private extern id: 'a -> 'a = "a => { if (!Array.isArray(a)) throw Error('expected a native array'); return a; }"
let original = [31n, 47n]
let returned = id original
let result = match returned with | [first, second] => first | _ => 0n end
let texts = id ["native", "array"]
let text = match texts with | [first, ..] => first | _ => "failed" end
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-array.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.result, 31); assert.equal(app.text, 'native');",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn execute_reification(source: &str, assertions: &str) {
    let artifact = compiled(source);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reification.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; {assertions}",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_native_arrays_are_recursive_snapshots_with_delayed_completion() {
    execute_reification(
        r#"
extern change: fn({ items: [[Nat]] }) -> { items: [[Nat]] } = "a => { globalThis.ruddySavedArray = a.items; a.items[0][0] = 7; return a; }"
extern mutate: fn() -> () = "() => { globalThis.ruddySavedArray[0][0] = 99; }"
@async
@private extern delayed: 'a -> 'a = "async a => a"
let original = { items: [[3n]] }
let result = change original
let changed = mutate ()
let later = delayed [11n]
let original_value = match original.items with | [[n]] => n | _ => 0n end
let imported_value = match result.items with | [[n]] => n | _ => 0n end
let delayed_value = match later with | [n] => n | _ => 0n end
"#,
        "assert.equal(app.original_value, 3); assert.equal(app.imported_value, 7); assert.equal(app.delayed_value, 11);",
    );
}

#[test]
fn reification_checked_decoding_reports_nested_paths_and_retains_opaque_values() {
    execute_reification(
        r#"
type Result 'a 'e = #Some 'a | #Error 'e
type DecodeError = { path: String, expected: String, message: String }
@private extern decode: ForeignValue -> Result 'a DecodeError = "$ffiDecode"
extern unknown: ForeignValue = "({ items: [1, 2, 3] })"
extern invalid: ForeignValue = "({ items: [1, 'bad'] })"
extern cyclic: ForeignValue = "(() => { const a = []; a.push(a); return a; })()"
let decoded: Result { items: [Nat] } DecodeError = decode unknown
let value = match decoded with | #Some r => match r.items with | [n, ..] => n | _ => 0n end | #Error _ => 0n end
let failed: Result { items: [Nat] } DecodeError = decode invalid
let path = match failed with | #Some _ => "failed" | #Error error => error.path end
let cycle: Result [[Nat]] DecodeError = decode cyclic
let rejected = match cycle with | #Some _ => false | #Error _ => true end
"#,
        "assert.equal(app.value, 1); assert.equal(app.path, '$.items[1]'); assert.equal(app.rejected, true);",
    );
}

#[test]
fn reification_local_helpers_independent_parameters_and_partial_application() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let pair = fn a b => (upcast a, upcast b)
@private let first = pair 5n
let both = first "second"
@private let local = fn x => do let box = fn value => upcast value return (box x, box "local") end
let boxed = local 9n
let number: Option Nat = downcast both.0
let text: Option String = downcast boxed.1
let result = match number with | #Some n => n | #None => 0n end
let label = match text with | #Some s => s | #None => "failed" end
"#,
        "assert.equal(app.result, 5); assert.equal(app.label, 'local');",
    );
}

#[test]
fn reification_structural_rows_recursive_aliases_and_callable_payloads() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
type Left = #Cons (Nat, Left) | #Nil
type Right = #Cons (Nat, Right) | #Nil
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let row: { value: 'a, ..'rest } -> Any = fn r => upcast r
@private let data: Left = #Cons (6n, #Nil)
@private let boxed = upcast data
@private let recovered: Option Right = downcast boxed
let recursive = match recovered with | #Some (#Cons (n, _)) => n | _ => 0n end
@private let record: Option { value: Nat, name: String } = downcast (row { value: 7n, name: "seven" })
let field = match record with | #Some r => r.name | #None => "failed" end
@private extern identity: 'a -> 'a = "a => a"
@private let function: Nat -> Nat = identity (fn n => n)
let called = function 8n
@private let roundtrip: Option (Nat -> Nat) = downcast (upcast function)
let boxed_function = match roundtrip with | #Some f => f 9n | #None => 0n end
"#,
        "assert.equal(app.recursive, 6); assert.equal(app.field, 'seven'); assert.equal(app.called, 8); assert.equal(app.boxed_function, 9);",
    );
}

#[test]
fn reification_recursive_generic_functions_and_captured_descriptors() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private extern dec: Nat -> Nat = "n => n - 1"
@private let recur = fn value n => match n with | 0n => upcast value | _ => other value (dec n) end
@private let other = fn value n => recur value n
@private let closure: 'a -> (() -> Any) = fn value => fn unit => upcast value
@private let local = fn value => do
  let again = fn x n => match n with | 0n => upcast x | _ => again x 0n end
  return again value 1n
end
@private let first: Option Nat = downcast (recur 12n 20000n)
@private let second: Option String = downcast ((closure "captured") ())
@private let third: Option Nat = downcast (local 14n)
let a = match first with | #Some n => n | #None => 0n end
let b = match second with | #Some s => s | #None => "failed" end
let c = match third with | #Some n => n | #None => 0n end
"#,
        "assert.equal(app.a, 12); assert.equal(app.b, 'captured'); assert.equal(app.c, 14);",
    );
}

#[test]
fn reification_js_exports_require_concrete_interfaces() {
    for source in [
        r#"type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
let box = fn value => upcast value"#,
        "let identity = fn value => value",
        "let nested = { factory: fn x => fn y => (x, y) }",
        "let callbacks = [fn value => value]",
    ] {
        let artifact = compiled(source);
        assert!(!artifact.header().values.is_empty());
        let error = js::check_exports(&artifact, &[], js::Platform::Node).unwrap_err();
        assert!(
            error.to_string().contains("runtime type information"),
            "{error}"
        );
        assert!(js::generate(&artifact).is_err());
    }
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
let dynamic: ForeignValue -> Any = fn value => upcast value
let native: [Nat] -> [Nat] = fn value => value
let identity: ForeignValue -> ForeignValue = fn value => value
"#,
        "const value = {}; assert.equal(app.identity(value), value); assert.deepEqual(app.native([1, 2]), [1, 2]); for (const value of [null, undefined, Symbol('s'), () => {}, 123n, {}]) assert.equal(app.identity(value), value); assert.throws(() => app.native([1, 'bad']), /Nat/);",
    );
}

#[test]
fn reification_opaque_transport_preserves_payloads_and_rejects_forgeries() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
type Result 'a 'e = #Some 'a | #Error 'e
type DecodeError = { path: String, expected: String, message: String }
@private let upcast: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private extern decode: ForeignValue -> Result 'a DecodeError = "$ffiDecode"
@private extern store: Any -> () = "value => { globalThis.savedAny = value; }"
@private extern load: () -> Any = "() => globalThis.savedAny"
@private extern forged: () -> Any = "() => ({ descriptor: 'Nat', value: 42 })"
@private extern unknown: ForeignValue = "({ descriptor: 'Nat', value: 42 })"
@private let stored = store (upcast [19n])
@private let returned: Option [Nat] = downcast (load stored)
let result = match returned with | #Some [n] => n | _ => 0n end
@private let checked: Result Any DecodeError = decode unknown
let rejected = match checked with | #Error _ => true | #Some _ => false end
@private let width: Option Int8 = downcast (upcast 1n8)
let different_width = match width with | #None => true | #Some _ => false end
let bad: () -> Any = fn _ => forged ()
"#,
        "assert.equal(app.result, 19); assert.equal(app.rejected, true); assert.equal(app.different_width, true); assert.throws(() => app.bad({}), /package/);",
    );
}

#[test]
fn public_generate_is_deterministic_and_emits_nested_esm_exports() {
    let artifact = compiled(
        "module Math =\n  let answer = 42n\n  let identity: ForeignValue -> ForeignValue = fn x => x\nend\nlet ready = true\n",
    );
    let first = js::generate(&artifact).unwrap();
    let second = js::generate(&artifact).unwrap();

    assert_eq!(first, second);
    assert!(first.starts_with("// Generated by Ruddy.\n"), "{first}");
    assert!(first.contains("export {"), "{first}");
    assert!(
        first.contains("const $e0 = $namespace([[\"answer\","),
        "{first}"
    );
    assert!(first.contains("[\"identity\","), "{first}");
    assert!(first.contains(" as ready"), "{first}");
}

#[test]
fn generated_module_executes_values_functions_records_and_sums_in_node() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let artifact = compiled(
        "let answer = 42n\n\
         let apply: (Nat -> Nat) -> Nat -> Nat = fn f => fn x => f x\n\
         let identity: Nat -> Nat = fn n => n\n\
         let record = { value: answer }\n\
         let tagged: #Ready Nat = #Ready answer\n",
    );
    let module = js::generate(&artifact).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, module).unwrap();

    let probe = format!(
        "import * as app from {}; console.log(JSON.stringify([app.answer, await (await app.apply(app.identity))(2), app.record.value, typeof app.tagged]));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[42,2,42,\"object\"]"
    );
}

#[test]
fn generated_module_erases_existential_packages_around_structs() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let artifact = compiled(
        "@private let build: Nat ->\n\
         { left when 'p: Nat, also when 'p: Nat, right when 'q: Nat }\n\
         where 'p != 'q = fn n => { left: n, also: n }\n\
         @private let choose: Nat ->\n\
         { left when 'p: Nat, also when 'p: Nat, right when 'q: Nat }\n\
         where 'p != 'q = fn n => { left: n, also: n }\n\
         @private let built = build 7n\n\
         let built_left = built.left\n\
         let projected = (choose 8n).also\n\
         let matched = match choose 9n with\n\
         | { left, .. } => left\n\
         end\n",
    );
    let module = js::generate(&artifact).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, module).unwrap();

    let probe = format!(
        "globalThis.host = {{ choose: n => n === 9 ? {{ right: n }} : {{ left: n, also: n }} }}; const app = await import({}); console.log(JSON.stringify([app.built_left, app.projected, app.matched]));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "[7,8,9]");
}

#[test]
fn generated_module_executes_tuple_values_projections_and_patterns() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let artifact = compiled(
        "let pair : { 0: Nat, 1: String } = { 000: 42n, 1: \"answer\" }\n\
         let singleton = (true,)\n\
         let first = pair.0\n\
         let second = pair.1\n\
         let swap : (Nat, String) -> (String, Nat) = fn value => match value with \
         | { 0: number, 1: text } => (text, number) end\n",
    );
    let module = js::generate(&artifact).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tuples.mjs");
    fs::write(&path, module).unwrap();

    let probe = format!(
        "const app = await import({}); const swapped = app.swap(app.pair); console.log(JSON.stringify([app.pair[0], app.pair[1], app.singleton[0], app.first, app.second, swapped[0], swapped[1]]));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[42,\"answer\",true,42,\"answer\",\"answer\",42]"
    );
}

#[test]
fn generated_runtime_preserves_arithmetic_switch_record_effect_and_literal_semantics() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    // Arithmetic operators currently surface as Real in source inference. Retag the
    // three focused initializers and two primitive cases to exercise the backend's
    // Nat/Int representations exactly as linked artifact LIR records them.
    let mut artifact = compiled(
        "let nat_sub = 2.0 - 5.0\n\
         let int_div = -7.0 / 2.0\n\
         let int_negative_zero = 0.0 / -1.0\n\
         let real_div = 1.0 / 0.0\n\
         let real_literal = 1.5\n\
         let classify_nat = fn n => match n with | 0.0 => \"zero\" | 1.0 => \"one\" | _ => \"other\" end\n\
         let classify_int = fn n => match n with | 0.0 => \"zero\" | _ => \"other\" end\n\
         let classify_real = fn n => match n with | 0.0 => \"positive zero\" | _ => \"other\" end\n\
         @private let unpack = fn value => match value with | #Value n => n | #Empty => 0n | _ => 99n end\n\
         let payload = unpack (#Value 8n)\n\
         let tag_fallback = unpack #Other\n\
         @private let shape = fn value => match value with | { x } => 1n | { x, .. } => 2n | _ => 3n end\n\
         let shape_first = shape { x: 4n }\n\
         let shape_more = shape { x: 4n, y: 5n }\n\
         let shape_other = shape { y: 5n }\n\
         let merged = { left: 1n, right: 2n }\n\
         effect Bump = Real -> Real\n\
         let bump = fn n => handle !Bump n with | !Bump value => value + 1.0 end\n\
         effect Read = { get: () -> Nat }\n\
         let read: () -> Nat = fn _ => handle !Read.get () with | !Read.get _ => 42n end\n\
         effect Plus = { apply: Real -> Real }\n\
         effect Times = { apply: Real -> Real }\n\
         @private let calculate : Real -> Real + !Plus + !Times = fn n => do let x = !Plus.apply n return !Times.apply x end\n\
         let calculated = fn n => handle (handle calculate n with | !Plus.apply value => value + 1.0 end) with | !Times.apply value => value * 2.0 end\n\
         effect Ask 'a = { get: () -> 'a }\n\
         let asked = fn n => handle do let x = !Ask.get () return x + n end with | !Ask.get _ => 1.0 end\n",
    )
    .to_unchecked();
    retag_numeric_global(&mut artifact, "nat_sub", artifact::Rep::Nat);
    retag_numeric_global(&mut artifact, "int_div", artifact::Rep::Int);
    retag_numeric_global(&mut artifact, "int_negative_zero", artifact::Rep::Int);
    retag_primitive_cases(&mut artifact, "classify_nat", artifact::Rep::Nat);
    retag_primitive_cases(&mut artifact, "classify_int", artifact::Rep::Int);
    split_record_into_merge(&mut artifact, "merged");
    let artifact = artifact
        .validate()
        .expect("the retagged artifact validates");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("semantics.mjs");
    let module = js::generate(&artifact).unwrap();
    assert!(
        module.contains("Object.assign(Object.create(null)"),
        "artifact fixture did not exercise record merging"
    );
    fs::write(&path, module).unwrap();
    let probe = format!(
        "const app = await import({}); const values = [app.nat_sub, app.int_div, Object.is(app.int_negative_zero, -0), Number.isFinite(app.real_div), app.real_literal, app.classify_nat(-0), app.classify_nat(7), app.classify_int(app.int_negative_zero), app.classify_real(0), app.classify_real(-0), app.payload, app.tag_fallback, app.shape_first, app.shape_more, app.shape_other, app.merged.left + app.merged.right, await app.bump(4), await app.read({{}}), await app.calculated(4), await app.asked(4)]; console.log(JSON.stringify(values));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[0,-3,true,false,1.5,\"zero\",\"other\",\"zero\",\"positive zero\",\"other\",8,99,1,2,3,3,5,42,10,5]"
    );
}

#[test]
fn generation_rejects_unlinked_and_internally_inconsistent_artifacts() {
    let mut unlinked = compiled("let value = 1n").to_unchecked();
    unlinked.header.dependencies.push(artifact::Dependency {
        name: "dep".to_string(),
        version: "1.0.0".to_string(),
    });
    let unlinked = unlinked.validate().expect("an unlinked artifact validates");
    assert_eq!(js::generate(&unlinked), Err(Error::Unlinked));
    assert_eq!(
        Error::Unlinked.to_string(),
        "the JavaScript backend requires a linked artifact"
    );

    let mut missing_public = compiled("let value = 1n").to_unchecked();
    missing_public.header.values[0].name = "app@1.0.0::missing".to_string();
    let missing_public = missing_public
        .validate()
        .expect("a missing public value validates");
    assert_eq!(
        js::generate(&missing_public),
        Err(Error::UnresolvedPublicValue(
            "app@1.0.0::missing".to_string()
        ))
    );

    let mut bad_global = compiled("let value = 1n").to_unchecked();
    bad_global.lir.functions[bad_global.lir.globals[0].initializer as usize].blocks[0].instrs[0]
        .op = artifact::Op::Global {
        callable: None,
        target: "unknown@1.0.0::value".to_string(),
    };
    let bad_global = bad_global
        .validate()
        .expect("an unknown global reference validates");
    assert_eq!(
        js::generate(&bad_global),
        Err(Error::InvalidGlobalReference(
            "unknown@1.0.0::value".to_string()
        ))
    );

    let mut bad_function =
        compiled("let identity: ForeignValue -> ForeignValue = fn x => x").to_unchecked();
    let closure = bad_function.lir.functions[bad_function.lir.globals[0].initializer as usize]
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instrs)
        .find(|instr| matches!(instr.op, artifact::Op::Closure { .. }))
        .unwrap();
    closure.op = artifact::Op::Closure {
        func: u64::MAX,
        captures: Vec::new(),
    };
    // TODO: `Error::InvalidFunctionReference` is no longer reachable through
    // the public API. A dangling function index never becomes an `Artifact`:
    // `validate` rejects it and `recover` discards the executable content, so
    // the backend cannot be handed one. The boundary is asserted instead.
    let error = bad_function
        .clone()
        .validate()
        .expect_err("a dangling function index is rejected");
    assert!(
        error.message().contains("outside its function table"),
        "{error}"
    );
    let (recovered, facts) = bad_function.recover();
    assert!(
        matches!(
            facts.as_slice(),
            [artifact::RecoveryFact::ExecutableDiscarded { .. }]
        ),
        "{facts:#?}"
    );
    assert!(recovered.lir().functions.is_empty());
    assert!(recovered.lir().globals.is_empty());

    let mut conflicting = compiled("let first = 1n\nlet second = 2n").to_unchecked();
    conflicting.header.values[0].name = "app@1.0.0::path".to_string();
    conflicting.lir.globals[0].name = "app@1.0.0::path".to_string();
    conflicting.header.values[1].name = "app@1.0.0::path::child".to_string();
    conflicting.lir.globals[1].name = "app@1.0.0::path::child".to_string();
    let conflicting = conflicting
        .validate()
        .expect("a conflicting export tree validates");
    assert_eq!(
        js::generate(&conflicting),
        Err(Error::ExportTreeConflict("path".to_string()))
    );
}

#[test]
fn generated_extern_expressions_execute_once_and_require_explicit_receiver_binding() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let artifact = compiled(
        "extern next : Nat -> Nat = \"(++globalThis.initializations, globalThis.host.next.bind(globalThis.host))\"\n\
         extern base : Nat = \"globalThis.host.base\"\n\
         extern add_base : Nat -> Nat = \"n => n + globalThis.host.base\"\n",
    );
    let module = js::generate(&artifact).unwrap();
    assert!(
        module
            .contains("(++globalThis.initializations, globalThis.host.next.bind(globalThis.host))"),
        "extern contents were not emitted as JavaScript code: {module}"
    );
    assert!(!module.contains("const $extern"), "{module}");

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("extern.mjs");
    fs::write(&path, module).unwrap();
    let probe = format!(
        "globalThis.initializations = 0; globalThis.host = {{ base: 40, next(n) {{ return this.base + n; }} }}; const app = await import({}); console.log(JSON.stringify([app.next(2), app.base, app.add_base(2), globalThis.initializations]));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "[42,40,42,1]"
    );
}

#[test]
fn invalid_empty_and_structurally_breaking_extern_expressions_are_rejected() {
    let mut artifact = compiled("extern host : Nat = \"0\"\n").to_unchecked();
    for target in [
        "(",
        "",
        "0); export const injected = 1; (2",
        "0); import 'injected'; (2",
        "0); globalThis.injected = true; (2",
    ] {
        artifact.lir.externs[0].target = target.to_string();
        let artifact = artifact
            .clone()
            .validate()
            .expect("an extern target string validates");
        let error = js::generate(&artifact).unwrap_err();
        assert!(
            matches!(error, Error::InvalidJavaScript(ref diagnostic) if !diagnostic.is_empty()),
            "accepted `{target}`: {error}"
        );
        assert!(
            error
                .to_string()
                .starts_with("generated JavaScript is invalid: "),
            "{error}"
        );
        let _: &dyn std::error::Error = &error;
    }
}

#[test]
fn trailing_line_comments_in_extern_expressions_do_not_consume_the_initializer() {
    let artifact = compiled("extern host : Nat = \"globalThis.host // trailing comment\"\n");
    let module = js::generate(&artifact).unwrap();
    assert!(
        module.contains("globalThis.host // trailing comment\n);"),
        "{module}"
    );
}

/// Uninterpreted metadata is inert: two sources that differ only in the attributes in front
/// of their definitions have the same schemes and compile to byte-identical
/// JavaScript. The artifact differs only in what it publishes about them.
#[test]
fn metadata_changes_neither_schemes_nor_generated_javascript() {
    let plain = compiled(
        "module Math =\n  let answer = 42n\nend\n\
         type Pair 'a = { first: 'a, second: 'a }\n\
         effect Log = Nat -> ()\n\
         extern sqrt : Real -> Real = \"Math.sqrt\"\n\
         let pair: Nat -> { first: Nat, second: Nat } = fn x => { first: x, second: x }\n\
         let (a, b) = (1n, 2n)\n",
    );
    let annotated = compiled(
        "@owner \"core\" module Math =\n  @answer @doc \"forty-two\" let answer = 42n\nend\n\
         @shape (1n, 2n) type Pair 'a = { first: 'a, second: 'a }\n\
         @level #Debug effect Log = Nat -> ()\n\
         @host { from: \"math\" } extern sqrt : Real -> Real = \"Math.sqrt\"\n\
         @example @tags [\"a\", \"b\"] let pair: Nat -> { first: Nat, second: Nat } = fn x => { first: x, second: x }\n\
         @k let (a, b) = (1n, 2n)\n",
    );
    assert_eq!(
        js::generate(&plain).unwrap(),
        js::generate(&annotated).unwrap()
    );
    assert_eq!(plain.lir(), annotated.lir());

    let (plain, annotated) = (plain.header(), annotated.header());
    assert_eq!(plain.values.len(), annotated.values.len());
    for (before, after) in plain.values.iter().zip(&annotated.values) {
        assert_eq!(before.name, after.name);
        assert_eq!(before.scheme, after.scheme, "{}", before.name);
    }
    for (before, after) in plain.types.iter().zip(&annotated.types) {
        assert_eq!(
            (&before.name, &before.params, &before.scheme),
            (&after.name, &after.params, &after.scheme)
        );
    }
    for (before, after) in plain.effects.iter().zip(&annotated.effects) {
        assert_eq!(
            (&before.name, &before.identity, &before.kind),
            (&after.name, &after.identity, &after.kind)
        );
    }
    assert!(plain.values.iter().all(|value| value.metadata.is_empty()));
    assert!(
        annotated
            .values
            .iter()
            .any(|value| !value.metadata.is_empty())
    );
    assert_eq!(plain.modules[0].name, annotated.modules[0].name);
}

#[test]
fn mutation_factories_preserve_freshness_aliases_and_assignment_results() {
    let artifact = compiled(
        "@private let counter = fn _ => do let cell = mut 0 return fn _ => cell := ~cell + 1 end
        let run: () -> _ = fn _ => do
          let first = counter ()
          let second = counter ()
          let _ = first ()
          let a = mut 0
          let b = a
          let stored = a := b := first ()
          return { first: ~a, second: second (), stored: stored }
        end",
    );
    let module = js::generate(&artifact).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, module).unwrap();
    let probe = format!(
        "import * as app from {}; console.log(JSON.stringify(await app.run({{}})));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "{\"first\":2,\"second\":1,\"stored\":2}"
    );
}

#[test]
fn mutation_operands_execute_once_in_target_then_value_order() {
    let artifact = compiled(
        "let run: () -> _ = fn _ => do
        let order = mut 0
        let a = mut 0
        let b = mut 0
        let target = fn pair => do let _ = order := ~order * 10 + pair.0 return pair.1 end
        let value = fn _ => do let _ = order := ~order * 10 + 3 return 9 end
        let result = target (1, a) := target (2, b) := value ()
        return { order: ~order, a: ~a, b: ~b, result: result }
      end",
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import * as app from {}; console.log(JSON.stringify(await app.run({{}})));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "{\"a\":9,\"b\":9,\"order\":123,\"result\":9}"
    );
}

#[test]
fn mutation_survives_foreign_aliases_callbacks_and_suspension() {
    let artifact = compiled(
        "@private extern alias: mut 'r Real -> mut 'r Real = \"host.alias\"
        @private extern update: mut 'r Real -> () + !mut 'r = \"host.update\"
        @private @async extern later: (() -> Real + !mut 'r) -> Real + !mut 'r = \"host.later\"
        let run: () -> _ = fn _ => do
          let cell = mut 1
          let same = alias cell
          let _ = update same
          let result = later (fn _ => cell := ~cell + 1)
          return { result: result, read: ~same }
        end",
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "globalThis.host = {{ alias: c => c, update: c => {{ c.value = 4; return {{}}; }}, later: async cb => {{ await Promise.resolve(); return await cb({{}}); }} }}; const app = await import({}); console.log(JSON.stringify(await app.run({{}})));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "{\"read\":5,\"result\":5}"
    );
}

#[test]
fn mutation_composes_with_effect_polymorphism_and_stack_safe_recursion() {
    let artifact = compiled(
        "@private let apply = fn f => fn x => f x
        @private let loop = fn pair => match pair.1 with
          | 0 => ~pair.0
          | _ => do let _ = pair.0 := ~pair.0 + 1 return loop (pair.0, pair.1 - 1) end
        end
        let run: () -> _ = fn _ => do
          let cell = mut 0
          let _ = apply (fn _ => cell := 1) ()
          return loop (cell, 20000)
        end",
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import * as app from {}; console.log(await app.run({{}}));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "20001");
}

#[test]
fn mutation_keeps_arrays_persistent_and_conditional_effects_callable() {
    let artifact = compiled(
        "@private let invoke: (() -> Real + !mut 'r (when 'p)) -> Real + !mut 'r (when 'p) = fn f => f ()
        let first = fn array => match array with | [x, ..] => x | [] => 0 end
        let run: () -> _ = fn _ => do
          let cell = mut [1, 2]
          let original = ~cell
          let _ = cell := [3, 4]
          let changed = invoke (fn _ => first (~cell))
          return { original: first original, changed: changed }
        end",
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        "import * as app from {}; console.log(JSON.stringify(await app.run({{}})));",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "{\"changed\":3,\"original\":1}"
    );
}

#[test]
fn signed_literals_run_in_arguments_patterns_and_metadata() {
    let artifact = compiled(
        "@negative -42i\nlet integer = (fn x => x) -42i\n\
         @negative_zero -0.0\nlet zero = -0.0\n\
         let real = (fn x => x) -1.5\n\
         let negated = - -1.5\n\
         let difference = 3 - 5\n\
         let int_case = match integer with | -42i => true | _ => false end\n\
         let real_case = match real with | -1.5 => true | _ => false end\n\
         let zero_case = match zero with | -0.0 => true | 0.0 => false | _ => false end\n",
    );
    let parsed = Artifact::try_parse(&artifact.print())
        .unwrap()
        .validate()
        .unwrap();
    assert_eq!(parsed, artifact);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("signed.mjs");
    fs::write(&path, js::generate(&parsed).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; const x = await import({}); assert.equal(x.integer, -42); assert.equal(x.real, -1.5); assert.ok(Object.is(x.zero, -0)); assert.equal(x.negated, 1.5); assert.equal(x.difference, -2); assert.ok(x.int_case && x.real_case && x.zero_case);",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_native_returned_functions_receive_descriptors_when_called() {
    execute_reification(
        r#"
@private extern make: () -> ('a -> 'a) = "() => value => { if (!Array.isArray(value)) throw Error('native array expected'); return value; }"
@private extern nested: () -> { apply: 'a -> 'a } = "() => ({ apply: value => { if (!Array.isArray(value)) throw Error('native array expected'); return value; } })"
@private let direct = make ()
@private let container = nested ()
let direct_numbers = direct [4n, 9n]
let direct_text = direct ["direct"]
let nested_numbers = container.apply [12n, 19n]
let nested_text = container.apply ["nested"]
"#,
        "assert.deepEqual(app.direct_numbers, [4, 9]); assert.deepEqual(app.direct_text, ['direct']); assert.deepEqual(app.nested_numbers, [12, 19]); assert.deepEqual(app.nested_text, ['nested']);",
    );
}

#[test]
fn reification_recursive_callable_adapters_are_finite() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Loop 'a = 'a -> Loop 'a
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let loop: Loop 'a = fn value => do
  let token = box value
  return loop
end
@private let first = loop 1n
@private let second = first 2n
let completed = true
"#,
        "assert.equal(app.completed, true);",
    );
}

#[test]
fn reification_native_returned_functions_keep_independent_descriptor_order() {
    execute_reification(
        r#"
@private extern make: () -> ({ first: 'a, second: 'b } -> { first: 'a, second: 'b }) = "() => value => { if (!Array.isArray(value.first) || typeof value.second !== 'string') throw Error('wrong native conversion'); return value; }"
@private let apply = make ()
let result = apply { first: [7n, 8n], second: "kept" }
"#,
        "assert.deepEqual(app.result.first, [7, 8]); assert.equal(app.result.second, 'kept');",
    );
}

#[test]
fn reification_generic_callable_payloads_seal_their_evidence_layout() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let pack = fn value => box value
@private let preserve = fn value => do let boxed = box value return value end
@private let generic = fn value => do
 let inner = fn next => do let boxed = box next let same = [value, next] return next end
 return pack inner
end
@private let recover: Any -> Nat = fn token => do
 let function: Option (Nat -> Nat) = downcast token
 return match function with
 | #Some call => call 42n
 | #None => 0n
end
end
@private let fixed: Nat -> Nat = preserve
let direct = recover (pack fixed)
let captured = recover (generic 7n)
@private let closed: 'a -> (#Call 'a) = fn value => #Call value
@private let aggregate = fn value => do
 let inner = fn next => do let token = box next let same = [value, next] return next end
 return pack { functions: [inner], tagged: closed inner }
end
@private let token = aggregate 8n
@private let recovered: Option { functions: [Nat -> Nat], tagged: #Call (Nat -> Nat) } = downcast token
let aggregate_result = match recovered with
 | #Some { functions: [call], tagged: #Call other } => other (call 43n)
 | _ => 0n
end
"#,
        "assert.equal(app.direct, 42); assert.equal(app.captured, 42); assert.equal(app.aggregate_result, 43);",
    );
}

#[test]
fn reification_callable_profiles_survive_aggregate_patterns_and_joins() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let reified = fn value => do let token = box value return value end
@private let erased = fn value => value
@private let functions = [erased, reified]
@private let payload = #Function reified
@private let record = { apply: reified }
@private let choose = fn flag => match flag with | true => erased | false => reified end
let from_array = match functions with | [first, second] => second (first 11n) | _ => 0n end
let from_sum = match payload with | #Function apply => apply 12n end
let from_record = match record with | { apply } => apply 13n end
let from_join = (choose false) 14n
"#,
        "assert.equal(app.from_array, 11); assert.equal(app.from_sum, 12); assert.equal(app.from_record, 13); assert.equal(app.from_join, 14);",
    );
}

#[test]
fn reification_marked_native_currying_defers_independent_type_arguments() {
    execute_reification(
        r#"
@private extern pair: fn('a, 'b) -> { first: 'a, second: 'b } = "(first, second) => { if (!Array.isArray(first) || !Array.isArray(second)) throw Error('native arrays expected'); return { first, second }; }"
@private let first = pair [3n]
let text = first ["text"]
let flags = first [true]
"#,
        "assert.deepEqual(app.text.first, [3]); assert.deepEqual(app.text.second, ['text']); assert.deepEqual(app.flags.second, [true]);",
    );
}

#[test]
fn reification_native_generic_callable_conversion_uses_a_sealed_convention() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private extern native: 'a -> 'a = "value => value"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let make = fn value => do
 let function = fn next => do let token = box next let same = [value, next] return next end
 return native function
end
@private let apply = make [1n]
let result = apply [2n, 3n]
"#,
        "assert.deepEqual(app.result, [2, 3]);",
    );
}

#[test]
fn reification_generic_selection_joins_both_value_profiles() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let choose = fn flag first second => match flag with | true => first | false => second end
@private let erased = fn value => value
@private let reified = fn value => do let token = box value return value end
let first = (choose true erased reified) 21n
let second = (choose false erased reified) 22n
let reversed = (choose false reified erased) 23n
"#,
        "assert.equal(app.first, 21); assert.equal(app.second, 22); assert.equal(app.reversed, 23);",
    );
}

#[test]
fn reification_callback_adapters_extract_component_type_evidence() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let apply = fn call value => call value
@private let element = fn values => match values with | [value, ..] => box value | _ => box () end
@private let consume = apply element
@private let token = consume [37n]
@private let recovered: Option Nat = downcast token
let result = match recovered with | #Some n => n | #None => 0n end
"#,
        "assert.equal(app.result, 37);",
    );
}

#[test]
fn reification_mutable_callable_cells_share_their_storage_convention() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let run = fn initial => do
 let cell = mut (fn value => value)
 let assigned = cell := (fn value => do let token = box value return value end)
 let from_cell = (~cell) initial
 return assigned from_cell
end
let result = run 39n
"#,
        "assert.equal(app.result, 39);",
    );
}

#[test]
fn reification_callback_adapters_extract_record_evidence() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let apply = fn call value => call value
@private let field = fn record => box record.value
@private let consume_field = apply field
@private let a: Option Nat = downcast (consume_field {value: 43n})
let first = match a with | #Some n => n | #None => 0n end
"#,
        "assert.equal(app.first, 43);",
    );
}

#[test]
fn reification_effect_payloads_preserve_callable_evidence() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
effect Apply 'a = ('a -> 'a) -> 'a
effect Get 'a = () -> ('a -> 'a)
@private let run = fn value => handle !Apply (fn next => do let token = box next return next end) with
 | !Apply call => call value
end
@private let returned = fn value => handle (!Get ()) value with
 | !Get _ => fn next => do let token = box next return next end
end
@private let as_value = fn value => handle do
 let perform = !Apply
 return perform (fn next => do let token = box next return next end)
end with | !Apply call => call value end
let result = run 45n
let second = returned 46n
let third = as_value 47n
"#,
        "assert.equal(await app.result, 45); assert.equal(await app.second, 46); assert.equal(await app.third, 47);",
    );
}

#[test]
fn reification_raised_callables_join_normal_and_return_arm_conventions() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
effect Get = () -> Nat
@private let choose = fn abort => handle
  do let n = !Get () return fn next => next end
with | !Get _ => match abort with
  | true => raise (fn next => do let token = box next return next end)
  | false => 0n
end end
@private let opposite = fn abort => handle !Get () with
 | !Get _ => match abort with
   | true => raise (fn next => next)
   | false => 0n
 end
 | return _ => fn next => do let token = box next return next end
end
let raised = choose true 12n
let normal = choose false 13n
let erased = opposite true 14n
let returned = opposite false 15n
effect Inner = () -> Nat
@private let nested = fn _ => handle
  do let n = !Get () return {call: fn next => next} end
with | !Get _ => handle
  do let n = !Inner () return raise {call: fn next => do let token = box next return next end} end
with | !Inner _ => 0n end end
let nested_result = (nested ()).call 16n
"#,
        "assert.equal(app.raised, 12); assert.equal(app.normal, 13); assert.equal(app.erased, 14); assert.equal(app.returned, 15); assert.equal(app.nested_result, 16);",
    );
}

#[test]
fn reification_deferred_components_preserve_independent_imported_callable_profiles() {
    let producer = compiled(
        r#"
type Leaf 'a = {call: 'a -> 'a}
type Pair 'a = {left: Leaf 'a, right: Leaf 'a}
let forward: Pair 'a -> Pair 'a = fn value => value
let select: Pair 'a -> ('a -> 'a) = fn value => value.right.call
"#,
    );
    let producer = artifact::parse(&producer.print()).validate().unwrap();
    let source = r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let reified = fn value => do let token = box value return value end
@private let erased = fn value => value
@private let first = dep::forward {left: {call: reified}, right: {call: erased}}
@private let second = dep::forward {left: {call: erased}, right: {call: reified}}
let a = first.left.call 51n
let b = (dep::select first) 52n
let c = second.left.call 53n
let d = (dep::select second) 54n
"#;
    let parsed = parse::parse(token::lex(source, ruddy::tracking::FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let consumer = compile::compile_with_dependencies(
        Mint::new(Bundle::new("consumer", Version::new(1, 0, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: compile::DependencyArtifact::Checked(&producer),
        }],
        inference::Trace::Off,
    )
    .unwrap_or_else(|failed| panic!("{:?}", failed.errors));
    let linked = ruddy::link::link(&[producer, consumer.artifact().clone()]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.mjs");
    fs::write(&path, js::generate(&linked).unwrap()).unwrap();
    let probe = format!(
        "import assert from 'node:assert/strict'; import * as app from {}; assert.equal(app.a, 51); assert.equal(app.b, 52); assert.equal(app.c, 53); assert.equal(app.d, 54);",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_native_callable_adapters_do_not_retain_discarded_conversions() {
    let artifact = compiled(
        r#"
@private extern native: () -> {f: Nat -> Nat} = "() => ({f: (globalThis.savedHostFunction ??= (n => n))})"
let fetch: () -> {f: Nat -> Nat} = fn _ => native ()
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("adapters.mjs");
    fs::write(&path, js::generate(&artifact).unwrap()).unwrap();
    let probe = format!(
        r#"import assert from 'node:assert/strict';
import * as app from {};
const references = [];
for (let i = 0; i < 1000; i++) {{
  const f = app.fetch().f;
  assert.equal(f(i), i);
  references.push(new WeakRef(f));
}}
// WeakRef targets survive their creation job. Collect in later jobs, while
// keeping the source host function alive throughout the probe.
for (let i = 0; i < 5; i++) {{
  await new Promise(resolve => setImmediate(resolve));
  global.gc();
}}
assert.equal(globalThis.savedHostFunction(12), 12);
assert.ok(references.filter(ref => ref.deref() === undefined).length > 900,
  'discarded native conversions must be collectible while their host function lives');
assert.equal(app.fetch().f(17), 17);
"#,
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--expose-gc", "--input-type=module", "--eval", &probe])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reification_callback_adapters_project_row_remainders_and_function_results() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let downcast: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let apply = fn call value => call value
@private let row: {head: Nat, ..'a} -> Any = fn record => box record
@private let invoke: (() -> 'a) -> Any = fn call => box (call ())
@private let consume_row = apply row
@private let consume_function = apply invoke
@private let a: Option {head: Nat, extra: String} = downcast (consume_row {head: 48n, extra: "rest"})
@private let b: Option Nat = downcast (consume_function (fn _ => 49n))
let first = match a with | #Some {head, extra} => head | #None => 0n end
let second = match b with | #Some n => n | #None => 0n end
"#,
        "assert.equal(app.first, 48); assert.equal(app.second, 49);",
    );
}

/// A hidden type is stored as its body: packaging one is no conversion,
/// opening one reads the body, and two values packaged under different
/// witnesses sit in one array and are each shown by their own function.
#[test]
fn hidden_types_package_and_open_in_generated_javascript() {
    execute_reification(
        r#"
type Box = hide 'a => { value: 'a, show: 'a -> String }
extern show_nat: fn(Nat) -> String = "n => String(n)"
extern show_bool: fn(Bool) -> String = "b => b ? \"yes\" : \"no\""
let boxes: [Box] = [{ value: 1n, show: show_nat }, { value: true, show: show_bool }]
let describe: Box -> String = fn box => match box with
| hide 'item { value, show } => show value
end
let first = match boxes with | [head, ..] => describe head | [] => "" end
let second = match boxes with | [_, next, ..] => describe next | _ => "" end
let repacked: Box -> Box = fn box => match box with
| hide 'item { value, show } => { value: value, show: fn v => show v }
end
let third = match boxes with | [head, ..] => describe (repacked head) | [] => "" end
"#,
        "assert.equal(app.first, '1'); assert.equal(app.second, 'yes'); assert.equal(app.third, '1');",
    );
}

#[test]
fn mirrors_authenticate_types_and_open_hidden_values_in_generated_javascript() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Dyn = hide 'a => { value: 'a, evidence: Mirror 'a }
let items: [Dyn] = [{ value: 1n, evidence: mirror () }, { value: "two", evidence: mirror () }]
let as_nat: Dyn -> Option Nat = fn item => match item with
| hide 'x { value, evidence } => match same_pair (evidence, mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
let echo: Dyn -> Dyn = fn item => match item with
| hide 'x { value, evidence } => { value: value, evidence: type_of value }
end
let first = match items with | [head, ..] => as_nat head | [] => #None end
let second = match items with | [_, next, ..] => as_nat next | _ => #None end
let third = match items with | [head, ..] => as_nat (echo head) | [] => #None end
let strings = same_pair (type_of "a", type_of "b")
let round_trip = match strings with
| #Some { forward, backward } => backward (forward "kept")
| #None => "lost"
end
"#,
        r#"
assert.deepEqual(app.first, { tag: 'Some', value: 1 });
assert.equal(app.second.tag, 'None');
assert.deepEqual(app.third, { tag: 'Some', value: 1 });
assert.equal(app.round_trip, 'kept');
"#,
    );
}

#[test]
fn mirrors_and_hidden_types_have_exact_identities_through_any() {
    execute_reification(
        r#"
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
type Option 'a = #Some 'a | #None
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let unbox: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
type Shown = hide 'a => { value: 'a, show: 'a -> String }
type Renamed = hide 'item => { value: 'item, show: 'item -> String }
type Wider = hide 'a => { value: 'a, show: 'a -> String, extra: Nat }
extern show_nat: fn(Nat) -> String = "n => String(n)"
let boxed_mirror = box (type_of 1n)
let mirror_of_nat: Option (Mirror Nat) = unbox boxed_mirror
let mirror_of_string: Option (Mirror String) = unbox boxed_mirror
let mirror_recovered = match mirror_of_nat with | #Some _ => true | #None => false end
let mirror_rejected = match mirror_of_string with | #Some _ => false | #None => true end
let shown: Shown = { value: 1n, show: show_nat }
let boxed_shown = box shown
let as_shown: Option Shown = unbox boxed_shown
let as_renamed: Option Renamed = unbox boxed_shown
let as_wider: Option Wider = unbox boxed_shown
let shown_recovered = match as_shown with
| #Some (hide 'a { value, show }) => show value
| #None => "lost"
end
let renamed_recovered = match as_renamed with
| #Some (hide 'a { value, show }) => show value
| #None => "lost"
end
let wider_rejected = match as_wider with | #Some _ => false | #None => true end
"#,
        r#"
assert.equal(app.mirror_recovered, true);
assert.equal(app.mirror_rejected, true);
assert.equal(app.shown_recovered, '1');
assert.equal(app.renamed_recovered, '1');
assert.equal(app.wider_rejected, true);
"#,
    );
}

#[test]
fn function_mirrors_carry_their_effect_contracts() {
    execute_reification(
        r#"
type Option 'a = #Some 'a | #None
type Any = hide 'a => { mirror: Mirror 'a, value: 'a }
effect Tick = () -> ()
effect Ask 'a = () -> 'a
@private extern type_of: 'a -> Mirror 'a = "$typeOf"
@private extern fresh_mirror: () -> Mirror 'a = "$mirror"
@private extern same_pair: (Mirror 'a, Mirror 'b) -> Option { forward: 'a -> 'b, backward: 'b -> 'a } = "$sameMirror"
@private let box: 'a -> Any = fn value => { mirror: type_of value, value: value }
@private let unbox: Any -> Option 'a = fn any => match any with
| hide 'x { mirror, value } => match same_pair (mirror, fresh_mirror ()) with
  | #Some { forward, backward } => #Some (forward value)
  | #None => #None
  end
end
@private let pure: () -> Nat = fn _ => 1n
@private let ticking: () -> Nat + !Tick = fn _ => do _ = !Tick () return 2n end
@private let asking: () -> Nat + !Ask Nat = fn _ => !Ask ()
@private let asking_text: () -> Nat + !Ask String = fn _ => do _ = !Ask () return 3n end
@private let boxed = box ticking
@private let as_pure: Option (() -> Nat) = unbox boxed
@private let as_ticking: Option (() -> Nat + !Tick) = unbox boxed
@private let as_asking: Option (() -> Nat + !Ask Nat) = unbox boxed
let pure_rejected = match as_pure with | #Some _ => false | #None => true end
let ticking_recovered = match as_ticking with
| #Some f => handle f () with | !Tick _ => () end
| #None => 0n
end
let asking_rejected = match as_asking with | #Some _ => false | #None => true end
let args_matter = match same_pair (type_of asking, type_of asking_text) with | #Some _ => false | #None => true end
let same_effect = match same_pair (type_of ticking, type_of ticking) with | #Some _ => true | #None => false end
"#,
        r#"
assert.equal(app.pure_rejected, true);
assert.equal(app.ticking_recovered, 2);
assert.equal(app.asking_rejected, true);
assert.equal(app.args_matter, true);
assert.equal(app.same_effect, true);
"#,
    );
}

#[test]
fn using_aliases_execute_without_becoming_javascript_exports() {
    execute_reification(
        "@private module Source = let value = 42n end
         using Source::{self as source, value as imported}
         let answer = imported
         let local = do using source::value as inner return inner end",
        "assert.equal(app.answer, 42); assert.equal(app.local, 42); assert.deepEqual(Object.keys(app).sort(), ['answer', 'local']);",
    );
}
