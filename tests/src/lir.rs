//! Tests for [`ruddy::lir`].

use ruddy::{
    artifact as a, inference, ir, lir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};
use ruddy_debug::print;

/// One source, taken all the way to LIR. Every earlier phase has to be silent:
/// lowering runs on accepted programs alone, so a source that does not type is
/// a test about nothing.
fn lowered(source: &str) -> lir::Output {
    lowered_labelled(source).0
}

fn lowered_labelled(source: &str) -> (lir::Output, print::lir::Labels) {
    lowered_with_dependencies(source, &[])
}

/// One accepted pipeline, dependencies included, taken to LIR through the
/// public compilation seam. Tests that corrupt accepted state to reach LIR's
/// defensive branches are crate-private and live beside the lowering.
fn lowered_with_dependencies(
    source: &str,
    dependencies: &[a::Artifact],
) -> (lir::Output, print::lir::Labels) {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    let bundle = Bundle::new("tests", Version::new(0, 1, 0)).expect("the bundle name is valid");
    let imports: Vec<_> = dependencies
        .iter()
        .map(|artifact| ruddy::compile::Dependency {
            alias: Some(&artifact.header().identity.name),
            artifact: ruddy::compile::DependencyArtifact::Checked(artifact),
        })
        .collect();
    let accepted = ruddy::compile::compile_with_dependencies(
        Mint::new(bundle),
        parsed.stmts,
        &imports,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{source}: {partial:#?}"));
    accepted
        .artifact()
        .to_unchecked()
        .validate()
        .expect("lowered control flow validates");
    let labels = print::lir::Labels::new(accepted.ir());
    (accepted.lower(), labels)
}

fn local_effect_interface(source: &str) -> String {
    let lexed = token::lex(source, ruddy::tracking::FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("interface", Version::new(0, 1, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{:#?}", built.errors);
    match built.program.effect_ids.values().next().unwrap() {
        ruddy::types::EffectId::Structural { interface, .. } => interface.clone(),
        ruddy::types::EffectId::Pending(_) => panic!("effect identity was not finalized"),
    }
}

/// The canonical listing of one source, which is what most of these tests read:
/// the printed form carries the temps, the representations and the nesting all
/// at once, and it is the form the spec pins.
fn listing(source: &str) -> String {
    let (output, labels) = lowered_labelled(source);
    print::lir::program(&output, &labels)
}

/// One section of a listing, by its header line.
fn section(source: &str, header: &str) -> String {
    let whole = listing(source);
    let header = header
        .strip_prefix("global ")
        .map(|name| format!("fn {}#init(", name.trim_end_matches(':')))
        .unwrap_or_else(|| header.to_string());
    whole
        .split("\n\n")
        .find(|part| part.starts_with(&header))
        .unwrap_or_else(|| panic!("`{header}` is a section of:\n{whole}"))
        .trim_end()
        .to_string()
}

#[test]
fn an_if_uses_the_existing_boolean_primitive_dispatch() {
    let printed = section(
        "let choose = fn p => if p then 1n else 2n end",
        "fn choose(",
    );
    assert_eq!(printed.matches("branch_prim").count(), 2, "{printed}");
    assert!(printed.contains("true =>"), "{printed}");
    assert!(printed.contains("false =>"), "{printed}");
    assert!(!printed.contains("else =>"), "{printed}");
    assert!(printed.contains("const 1n"), "{printed}");
    assert!(printed.contains("const 2n"), "{printed}");

    let chained = section(
        "let choose = fn p q => if p then 1n else if q then 2n else 3n end",
        "fn choose(",
    );
    assert_eq!(chained.matches("branch_prim").count(), 4, "{chained}");
}

#[test]
fn pipeline_lowers_as_an_application() {
    let printed = listing("let f = fn x => x\nlet value = 1 |> f");
    assert!(
        printed.contains("call f"),
        "pipeline did not call f:\n{printed}"
    );
}

#[test]
fn real_number_operators_lower_to_native_instructions() {
    let printed = listing("let value = - 1 + 2 - 3 * 4 / 5");
    for op in ["neg", "add", "sub", "mul", "div"] {
        assert!(printed.contains(op), "{op} missing from:\n{printed}");
    }
}

#[test]
fn boolean_operators_lower_to_native_instructions() {
    let printed = listing("let value = not true and false xor true or false");
    for op in ["not", "and", "xor", "or"] {
        assert!(printed.contains(op), "{op} missing from:\n{printed}");
    }
}

/// Every nested expression becomes one temp assignment, in the order the source
/// evaluates it: the value of a `let` before its body, a struct's fields as
/// written, a projection's base before the read.
#[test]
fn a_nested_expression_flattens_in_source_order() {
    let printed = section(
        "let f = do let p : { a: Nat } = { a: 1n } return #Some { b: p.a, c: 2n } end",
        "global f",
    );
    assert_eq!(
        printed,
        r#"fn f#init(; %6: cont) entry b0 [f0, Synchronous]:
  b0(%6: cont):
    %0: nat = const 1n
    %1: struct = struct { a: %0 }
    %2: nat = project %1, "a"
    %3: nat = const 2n
    %4: struct = struct { b: %2, c: %3 }
    %5: sum = tag #Some, %4
    continue %6, %5"#
    );
}

/// A binding is a name for a temp and nothing else. Nothing is emitted for the
/// `let` itself, and the annotation beside it has nothing to say to a backend —
/// so a body that only names what was bound emits no instruction at all.
#[test]
fn a_binding_emits_no_instruction_of_its_own() {
    assert_eq!(
        section("let f = do let p : Nat = 1n return p end", "global f"),
        r#"fn f#init(; %1: cont) entry b0 [f0, Synchronous]:
  b0(%1: cont):
    %0: nat = const 1n
    continue %1, %0"#
    );
}

/// A name a binding shadows goes back to standing for what it did.
#[test]
fn a_shadowed_name_is_put_back_afterwards() {
    assert_eq!(
        section(
            "let f = do let p = 1n return { one: do let p = 2n return p end, two: p } end",
            "global f"
        ),
        r#"fn f#init(; %3: cont) entry b0 [f0, Synchronous]:
  b0(%3: cont):
    %0: nat = const 1n
    %1: nat = const 2n
    %2: struct = struct { one: %1, two: %0 }
    continue %3, %2"#
    );
}

/// One case per representation. The primitive representations come off the
/// core; the empty struct is `unit` and a struct with fields is `struct`; and
/// anything a scheme quantified is `any`, since monomorphization is deferred.
#[test]
fn forwarded_empty_struct_rows_have_unit_representation_everywhere() {
    let source = "type RowId 'r = { ..'r }\n\
                  let id : RowId {} -> RowId {} = fn x => x\n\
                  let called = id {}";
    let function = section(source, "fn id(");
    assert!(function.starts_with("fn id(%0: unit;"), "{function}");
    let call = section(source, "global called");
    assert!(call.contains(": unit = struct {}"), "{call}");
    assert!(call.contains("call id"), "{call}");

    let absent = section(
        "type NoY 'r = { \\y, ..'r }\nlet value : NoY {} = {}",
        "global value",
    );
    assert!(absent.contains(": unit = struct {}"), "{absent}");

    let open = section(
        "type RowId 'r = { ..'r }\n\
         let open : RowId { ..'s } -> RowId { ..'s } = fn x => x",
        "fn open(",
    );
    assert!(open.starts_with("fn open(%0: struct;"), "{open}");
    let nonempty = section(
        "type RowId 'r = { ..'r }\n\
         let point : RowId { x: Nat } = { x: 1n }",
        "global point",
    );
    assert!(nonempty.contains(": struct = struct { x:"), "{nonempty}");
}

#[test]
fn existential_packages_are_transparent_to_container_lowering() {
    let source = "let build: Nat ->\n\
                  { left when 'p: Nat, also when 'p: Nat, right when 'q: Nat }\n\
                  where 'p != 'q = fn n => { left: n, also: n }\n\
                  extern choose: Nat ->\n\
                  { left when 'p: Nat, also when 'p: Nat, right when 'q: Nat }\n\
                  where 'p != 'q = \"host.choose\"\n\
                  let value = choose 7n\n\
                  let projected = (choose 8n).also\n\
                  let matched = match value with\n\
                  | { left, .. } => left\n\
                  | { right, .. } => right\n\
                  end";

    let function = section(source, "fn build(");
    assert!(
        function.contains(": struct = struct { left:"),
        "a packaged struct literal keeps its runtime representation:\n{function}"
    );

    let value = section(source, "global value");
    assert!(
        value.contains("global choose") && value.contains("call %"),
        "a packaged call result keeps its runtime representation:\n{value}"
    );

    let projected = section(source, "global projected");
    assert!(
        projected.contains(": nat = project") && projected.contains(": struct) continuation"),
        "a packaged container still declares its member representation:\n{projected}"
    );

    let matched = section(source, "global matched");
    assert!(
        matched.contains("branch_presence") && matched.contains(": nat = project"),
        "a packaged struct still widens into presence and field tests:\n{matched}"
    );
}

#[test]
fn sibling_row_substitutions_have_identical_unit_representations() {
    let source = "type Empty = {}\n\
                  type Dup 'r = { a: { ..'r }, b: { ..'r } }\n\
                  type Rev 'r = { b: { ..'r }, a: { ..'r } }\n\
                  let dup : Dup Empty -> { a: Empty, b: Empty } = fn p => { a: p.a, b: p.b }\n\
                  let rev : Rev Empty -> { a: Empty, b: Empty } = fn p => { a: p.a, b: p.b }";
    for name in ["dup", "rev"] {
        let function = section(source, &format!("fn {name}("));
        assert_eq!(
            function.matches(": unit = project").count(),
            2,
            "both independent fields must open identically regardless of order:\n{function}"
        );
    }
}

#[test]
fn imported_forwarding_cycles_recover_before_lir_representation() {
    std::thread::Builder::new()
        .name("forwarding-cycle-lir".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let scheme = |count, body| a::Scheme {
                callable: None,
                representations: Vec::new(),
                count,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body,
            };
            let dependency = a::UncheckedArtifact {
                header: a::Header {
                    kind: ruddy::artifact::Kind::Library,
                    compiler: ruddy::artifact::Stamp::current(),
                    domains: ruddy::types::Domains::default(),
                    modules: Vec::new(),
                    identity: a::Identity {
                        name: "dep".into(),
                        version: "1.0.0".into(),
                    },
                    dependencies: Vec::new(),
                    values: Vec::new(),
                    types: vec![
                        a::DeclaredType {
                            exported: true,
                            metadata: Default::default(),
                            name: "dep@1.0.0::A".into(),
                            params: Vec::new(),
                            scheme: scheme(
                                0,
                                a::Type::Named {
                                    name: "dep@1.0.0::Id".into(),
                                    args: vec![a::Type::Named {
                                        name: "dep@1.0.0::A".into(),
                                        args: Vec::new(),
                                    }],
                                },
                            ),
                        },
                        a::DeclaredType {
                            exported: true,
                            metadata: Default::default(),
                            name: "dep@1.0.0::Id".into(),
                            params: vec![a::Parameter {
                                sense: a::Sense::Type,
                                lacks: Vec::new(),
                                relevant: true,
                            }],
                            scheme: scheme(1, a::Type::Bound(0)),
                        },
                    ],
                    effects: Vec::new(),
                },
                lir: a::Lir {
                    externs: Vec::new(),
                    functions: Vec::new(),
                    globals: Vec::new(),
                },
            }
            .validate()
            .expect("a hand-built dependency validates");

            let source = "let id : dep::A -> dep::A = fn x => x";
            let (output, labels) = lowered_with_dependencies(source, &[dependency]);
            let printed = print::lir::program(&output, &labels);
            assert!(printed.contains("fn id(%0: any;"), "{printed}");
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("a forwarding cycle reaches LIR without hanging");
}

#[test]
fn every_representation_comes_off_the_solved_type() {
    let source = "let n = 1n\nlet u = {}\nlet s = { x: 1n }\nlet c = #A\nlet i = fn x => x\n\
                  let int : Int -> Int = fn x => x\n\
                  let real : Real -> Real = fn x => x\n\
                  let string : String -> String = fn x => x\n\
                  let boolean : Bool -> Bool = fn x => x";
    assert!(section(source, "global n").contains("%0: nat = const 1n"));
    assert!(section(source, "global u").contains(": unit = struct {}"));
    assert!(section(source, "global s").contains(": struct = struct { x:"));
    assert!(section(source, "global c").contains(": sum = tag #A"));
    assert!(section(source, "global i").contains(": fn = closure i#1"));
    // The polymorphic identity's argument is held as anything at all.
    assert!(section(source, "fn i(").starts_with("fn i(%5: any;"));
    assert!(section(source, "fn int(").contains(": int;"));
    assert!(section(source, "fn real(").contains(": real64;"));
    assert!(section(source, "fn string(").contains(": string;"));
    assert!(section(source, "fn boolean(").contains(": boolean;"));
}

/// A `fn` lifts to a top-level function whose parameters are its captures and
/// then its own argument, and a `closure` at the site it was written pairs the
/// two. The captures are the free variables in the order the body first uses
/// them, which is why `b` comes before `a` here and not the other way round.
#[test]
fn a_lifted_function_captures_in_order_of_first_use() {
    let source = "let two = fn a => fn b => { g: fn z => { p: b, q: a } }";
    assert!(
        section(source, "fn two(").contains("closure two#3, [%1, %0]"),
        "{}",
        listing(source)
    );
    assert!(section(source, "fn two#3").starts_with("fn two#3(%3: any, %4: any, %2: any;"));
}

/// A lambda that closes over nothing carries no captures.
#[test]
fn a_lambda_that_closes_over_nothing_captures_nothing() {
    let source = "let k = fn w => { f: fn x => x }";
    assert!(section(source, "fn k(").contains("closure k#2, []"));
    assert!(section(source, "fn k#2").starts_with("fn k#2(%1: any;"));
}

/// A recursive local function reconstructs its own closure inside the lifted
/// body. Its name is therefore a local closure read, never a bogus global.
#[test]
fn a_local_recursive_function_reconstructs_its_closure() {
    let source =
        "let main = do let loop = fn n => do let again = loop n return loop n end return loop end";
    assert_eq!(
        section(source, "fn main#1"),
        r#"fn main#1(%0: any; %5: cont) entry b1 [f0, Synchronous]:
  b0(%0: any, %1: fn, %5: cont, %2: any) continuation:
    call %1, %0 -> %5
  b1(%0: any, %5: cont):
    %1: fn = closure main#1, []
    %6: cont = continuation main#1:b0, [%0, %1, %5] [f0]
    call %1, %0 -> %6"#
    );
    assert_eq!(
        section(source, "global main"),
        r#"fn main#init(; %7: cont) entry b0 [f1, Synchronous]:
  b0(%7: cont):
    %4: fn = closure main#1, []
    continue %7, %4"#
    );
}

/// A reconstructed recursive closure closes over the lifted function's capture
/// parameters, corresponding exactly to the outer values captured at the site.
#[test]
fn a_local_recursive_function_keeps_its_outer_captures() {
    let source = "let make = fn seed => do let loop = fn n => seed + loop n return loop end";
    let outer = section(source, "fn make(");
    assert!(outer.contains("closure make#2, [%0]"), "{outer}");
    let recursive = section(source, "fn make#2");
    assert!(
        recursive.starts_with("fn make#2(%2: real64, %1: any;"),
        "{recursive}"
    );
    assert!(
        recursive.contains("%3: fn = closure make#2, [%2]"),
        "{recursive}"
    );
    assert!(recursive.contains("call %3, %1"), "{recursive}");
}

/// Multi-argument syntax becomes nested lambdas. The inner lambda may still
/// capture the reconstructed closure of the whole recursively bound function.
#[test]
fn a_curried_local_recursive_function_captures_its_outer_self() {
    let source = "let main = do let loop = fn a b => loop a b return loop end";
    let outer = section(source, "fn main#2");
    assert!(outer.contains("%4: fn = closure main#2, []"), "{outer}");
    assert!(outer.contains("closure main#1, [%4, %1]"), "{outer}");
    let inner = section(source, "fn main#1");
    assert!(inner.contains("call %5, %2, %6"), "{inner}");
    assert!(inner.contains("call %7, %2, %3"), "{inner}");
}

/// A top-level definition is not a free variable: naming one is a `global` read
/// inside the function that names it, so nothing is captured for it.
#[test]
fn a_top_level_name_is_read_rather_than_captured() {
    let source = "let c = 1n\nlet r = fn w => { f: fn x => c }";
    assert_eq!(
        section(source, "fn r#2"),
        r#"fn r#2(%2: any; %11: cont) entry b0 [f2, Synchronous]:
  b0(%11: cont):
    %3: nat = global c
    continue %11, %3"#
    );
    assert!(section(source, "fn r(").contains("closure r#2, []"));
}

/// Applying a known 2-ary function to two arguments is one direct call of the
/// uncurried function, not two unary calls.
#[test]
fn a_full_application_is_one_direct_call() {
    let source = "let add = fn a => fn b => a\nlet main = add 1n 2n";
    assert_eq!(
        section(source, "global main"),
        r#"fn main#init(; %15: cont) entry b0 [f4, Synchronous]:
  b0(%15: cont):
    %8: nat = const 1n
    %9: nat = const 2n
    call add, %8, %9 -> %15 [f0]"#
    );
    assert_eq!(
        section(source, "fn add("),
        r#"fn add(%0: any, %1: any; %11: cont) entry b0 [f0, Synchronous]:
  b0(%0: any, %11: cont):
    continue %11, %0"#
    );
}

/// Short of a full application, the arguments so far become the captures of the
/// wrapper that takes the next one.
#[test]
fn a_partial_application_is_a_wrapper_closure() {
    let source = "let two = fn a => fn b => a\nlet part = two 1n";
    assert_eq!(
        section(source, "global part"),
        r#"fn part#init(; %14: cont) entry b0 [f4, Synchronous]:
  b0(%14: cont):
    %8: nat = const 1n
    %9: fn = closure two#2, [%8]
    continue %14, %9"#
    );
    // The wrapper it names takes what was captured and then the argument left,
    // and hands the lot to the uncurried function.
    assert_eq!(
        section(source, "fn two#2"),
        r#"fn two#2(%4: any, %5: any; %12: cont) entry b0 [f2, Synchronous]:
  b0(%4: any, %5: any, %12: cont):
    call two, %4, %5 -> %12 [f0]"#
    );
}

/// Naming a known function without applying it is the closure of its outermost
/// wrapper, which is exactly what its global holds — so a name and a call reach
/// the same function by two routes that cannot disagree.
#[test]
fn a_known_function_used_as_a_value_is_its_global() {
    let source = "let two = fn a => fn b => a\nlet bare = two";
    assert_eq!(
        section(source, "global bare"),
        r#"fn bare#init(; %13: cont) entry b0 [f4, Synchronous]:
  b0(%13: cont):
    %8: fn = global two
    continue %13, %8"#
    );
    assert_eq!(
        section(source, "global two"),
        r#"fn two#init(; %12: cont) entry b0 [f3, Synchronous]:
  b0(%12: cont):
    %7: fn = closure two#1, []
    continue %12, %7"#
    );
    assert_eq!(
        section(source, "fn two#1"),
        r#"fn two#1(%2: any; %10: cont) entry b0 [f1, Synchronous]:
  b0(%2: any, %10: cont):
    %3: fn = closure two#2, [%2]
    continue %10, %3"#
    );
}

/// More arguments than the function has levels: the first n go into the direct
/// call, and what is left is applied to the result one at a time.
#[test]
fn an_over_application_calls_direct_and_then_indirectly() {
    let source = "let id = fn x => x\nlet two = fn a => fn b => a\nlet over = two id 1n 2n";
    assert_eq!(
        section(source, "global over"),
        r#"fn over#init(; %24: cont) entry b1 [f7, MaySuspend]:
  b0(%24: cont, %14: fn) continuation:
    %15: nat = const 2n
    call %14, %15 -> %24
  b1(%24: cont):
    %12: fn = global id
    %13: nat = const 1n
    %25: cont = continuation over#init:b0, [%24] [f7]
    call two, %12, %13 -> %25 [f2]"#
    );
}

/// A callee that is not a known top-level definition stays one unary call per
/// argument: nothing here knows its arity, and working one out is deferred.
#[test]
fn an_unknown_callee_stays_unary() {
    let source = "let go = fn f => fn x => f x";
    assert_eq!(
        section(source, "fn go#1"),
        r#"fn go#1(%5: fn, %1: type_descriptor, %2: type_descriptor, %3: struct, %4: any; %9: cont) entry b0 [f0, MaySuspend]:
  b0(%1: type_descriptor, %2: type_descriptor, %3: struct, %4: any, %5: fn, %9: cont):
    call %5, %1, %2, %3, %4 -> %9"#
    );
}

/// Globals come out in group order — earliest group first, source order within
/// one — which is the order a backend initializes them in.
#[test]
fn globals_are_emitted_in_group_order() {
    let output = lowered("let a = 1n\nlet b = a\nlet c = 2n");
    let names: Vec<&str> = output
        .globals
        .iter()
        .map(|global| global.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert_eq!(output.functions.len(), output.globals.len());
}

/// Two functions in one group call each other directly: the slot a function's
/// name stands for exists before either body is built, so a call can be written
/// before the callee is.
#[test]
fn mutual_recursion_calls_direct() {
    let source = "let ping = fn n => pong n\nlet pong = fn n => ping n";
    assert_eq!(
        section(source, "fn ping("),
        r#"fn ping(%0: any; %10: cont) entry b0 [f0, Synchronous]:
  b0(%0: any, %10: cont):
    call pong, %0 -> %10 [f2]"#
    );
    assert_eq!(
        section(source, "fn pong("),
        r#"fn pong(%5: any; %12: cont) entry b0 [f2, Synchronous]:
  b0(%5: any, %12: cont):
    call ping, %5 -> %12 [f0]"#
    );
}

/// A plain value's global is the lowering of its body and nothing else.
#[test]
fn a_plain_value_global_is_its_own_initializer() {
    assert_eq!(
        section("let v = { x: 1n }", "global v"),
        r#"fn v#init(; %2: cont) entry b0 [f0, Synchronous]:
  b0(%2: cont):
    %0: nat = const 1n
    %1: struct = struct { x: %0 }
    continue %2, %1"#
    );
}

/// The decision tree tests each discriminant once along any path: the tag once,
/// the payload read out once, and the numbers under it — with the `else` the
/// naturals always need, since they never run out.
#[test]
fn a_match_tests_each_position_once() {
    assert_eq!(
        section(
            "let pick = fn v => match v with | #Some 0n => 100n | #Some n => n | #None => 0n end",
            "fn pick("
        ),
        r#"fn pick(%0: sum; %9: cont) entry b8 [f0, Synchronous]:
  b0():
    unreachable
  b1(%9: cont):
    %4: nat = const 0n
    continue %9, %4
  b2(%0: sum, %9: cont):
    branch_tag %0, #None => b1(%9), otherwise b0()
  b3(%1: nat, %9: cont):
    continue %9, %1
  b4(%9: cont):
    %2: nat = const 100n
    continue %9, %2
  b5(%1: nat, %9: cont):
    branch_prim %1, 0n => b4(%9), otherwise b3(%1, %9)
  b6(%0: sum, %9: cont):
    %1: nat = payload %0
    jump b5(%1, %9)
  b7(%0: sum, %9: cont):
    branch_tag %0, #Some => b6(%0, %9), otherwise b2(%0, %9)
  b8(%0: sum, %9: cont):
    jump b7(%0, %9)"#
    );
}

/// An array position dispatches on length once: one case per length up to the
/// longest an arm names and an `else` for the lengths beyond, each element
/// read out once where an arm tests or binds it, and a rest read out as the
/// slice between the fixed elements only where an arm binds it.
#[test]
fn an_array_match_dispatches_on_length_once() {
    assert_eq!(
        section(
            "let len = fn arr => match arr with | [] => 0n | [_, ..rest] => 1n end",
            "fn len("
        ),
        r#"fn len(%0: array; %10: cont) entry b5 [f0, Synchronous]:
  b0(%0: array, %10: cont):
    %4: array = slice %0, 1, 0
    %5: nat = const 1n
    continue %10, %5
  b1(%0: array, %10: cont):
    %2: array = slice %0, 1, 0
    %3: nat = const 1n
    continue %10, %3
  b2(%0: array, %10: cont):
    branch_len %0, 1 => b1(%0, %10), otherwise b0(%0, %10)
  b3(%10: cont):
    %1: nat = const 0n
    continue %10, %1
  b4(%0: array, %10: cont):
    branch_len %0, 0 => b3(%10), otherwise b2(%0, %10)
  b5(%0: array, %10: cont):
    jump b4(%0, %10)"#
    );
    // A rest with nothing on either side is the array itself: no slice, and
    // with no arm naming an element, no length to dispatch on either.
    assert_eq!(
        section(
            "let same = fn arr => match arr with | [..all] => all end",
            "fn same("
        ),
        r#"fn same(%0: array; %4: cont) entry b0 [f0, Synchronous]:
  b0(%0: array, %4: cont):
    continue %4, %0"#
    );
}

/// Under an exact length every element is counted from the front, the ones
/// after a rest included; beyond the named lengths the ones after a rest are
/// counted from the back, and the rest is the slice between.
#[test]
fn array_elements_are_read_from_whichever_end_the_length_fixes() {
    assert_eq!(
        section(
            "let ends = fn arr => match arr with | [first, ..middle, last] => middle | [..rest] => rest end",
            "fn ends("
        ),
        r#"fn ends(%0: array; %11: cont) entry b7 [f0, Synchronous]:
  b0(%0: array, %11: cont):
    %4: any = nth %0, 0
    %5: any = nth_back %0, 0
    %6: array = slice %0, 1, 1
    continue %11, %6
  b1(%0: array, %11: cont):
    %1: any = nth %0, 0
    %2: any = nth %0, 1
    %3: array = slice %0, 1, 1
    continue %11, %3
  b2(%0: array, %11: cont):
    branch_len %0, 2 => b1(%0, %11), otherwise b0(%0, %11)
  b3(%0: array, %11: cont):
    continue %11, %0
  b4(%0: array, %11: cont):
    branch_len %0, 1 => b3(%0, %11), otherwise b2(%0, %11)
  b5(%0: array, %11: cont):
    continue %11, %0
  b6(%0: array, %11: cont):
    branch_len %0, 0 => b5(%0, %11), otherwise b4(%0, %11)
  b7(%0: array, %11: cont):
    jump b6(%0, %11)"#
    );
    assert_eq!(
        section(
            "let pick = fn arr => match arr with | [1n, .., 2n] => 3n | [.., last] => last | [] => 0n end",
            "fn pick("
        ),
        r#"fn pick(%0: array; %19: cont) entry b19 [f0, Synchronous]:
  b0(%0: array, %19: cont):
    %13: nat = nth_back %0, 0
    continue %19, %13
  b1(%10: nat, %19: cont):
    continue %19, %10
  b2(%19: cont):
    %11: nat = const 3n
    continue %19, %11
  b3(%10: nat, %19: cont):
    branch_prim %10, 2n => b2(%19), otherwise b1(%10, %19)
  b4(%0: array, %19: cont):
    %10: nat = nth_back %0, 0
    jump b3(%10, %19)
  b5(%0: array, %9: nat, %19: cont):
    branch_prim %9, 1n => b4(%0, %19), otherwise b0(%0, %19)
  b6(%0: array, %19: cont):
    %9: nat = nth %0, 0
    jump b5(%0, %9, %19)
  b7(%0: array, %19: cont):
    %7: nat = nth %0, 1
    continue %19, %7
  b8(%4: nat, %19: cont):
    continue %19, %4
  b9(%19: cont):
    %5: nat = const 3n
    continue %19, %5
  b10(%4: nat, %19: cont):
    branch_prim %4, 2n => b9(%19), otherwise b8(%4, %19)
  b11(%0: array, %19: cont):
    %4: nat = nth %0, 1
    jump b10(%4, %19)
  b12(%0: array, %3: nat, %19: cont):
    branch_prim %3, 1n => b11(%0, %19), otherwise b7(%0, %19)
  b13(%0: array, %19: cont):
    %3: nat = nth %0, 0
    jump b12(%0, %3, %19)
  b14(%0: array, %19: cont):
    branch_len %0, 2 => b13(%0, %19), otherwise b6(%0, %19)
  b15(%0: array, %19: cont):
    %2: nat = nth %0, 0
    continue %19, %2
  b16(%0: array, %19: cont):
    branch_len %0, 1 => b15(%0, %19), otherwise b14(%0, %19)
  b17(%19: cont):
    %1: nat = const 0n
    continue %19, %1
  b18(%0: array, %19: cont):
    branch_len %0, 0 => b17(%19), otherwise b16(%0, %19)
  b19(%0: array, %19: cont):
    jump b18(%0, %19)"#
    );
}

/// An arm accepting the whole array sits beside the length cases as the row
/// that takes every length: it binds the array itself under each case, and
/// reads no element.
#[test]
fn an_arm_binding_the_whole_array_takes_every_length() {
    assert_eq!(
        section(
            "let f = fn arr => match arr with | [] => 0n | other => 1n end",
            "fn f("
        ),
        r#"fn f(%0: array; %7: cont) entry b3 [f0, Synchronous]:
  b0(%7: cont):
    %2: nat = const 1n
    continue %7, %2
  b1(%7: cont):
    %1: nat = const 0n
    continue %7, %1
  b2(%0: array, %7: cont):
    branch_len %0, 0 => b1(%7), otherwise b0(%7)
  b3(%0: array, %7: cont):
    jump b2(%0, %7)"#
    );
    assert_eq!(
        section(
            "let g = fn arr => match arr with | [x, ..] => x | _ => 0n end",
            "fn g("
        ),
        r#"fn g(%0: array; %8: cont) entry b5 [f0, Synchronous]:
  b0(%0: array, %8: cont):
    %3: nat = nth %0, 0
    continue %8, %3
  b1(%0: array, %8: cont):
    %2: nat = nth %0, 0
    continue %8, %2
  b2(%0: array, %8: cont):
    branch_len %0, 1 => b1(%0, %8), otherwise b0(%0, %8)
  b3(%8: cont):
    %1: nat = const 0n
    continue %8, %1
  b4(%0: array, %8: cont):
    branch_len %0, 0 => b3(%8), otherwise b2(%0, %8)
  b5(%0: array, %8: cont):
    jump b4(%0, %8)"#
    );
}

/// A spread literal is the runs of plain items, each one literal, joined with
/// the arrays it spreads, in order; a literal with no spread is one array.
#[test]
fn a_spread_literal_joins_its_pieces_in_order() {
    let listing = listing("let a = [1n]\nlet b = [..a, 2n, 3n, ..a]\nlet c = [..a]");
    assert!(listing.contains("concat %"), "{listing}");
    let joined = section("let a = [1n]\nlet b = [..a, 2n, 3n, ..a]", "global b");
    assert!(joined.contains("array [%"), "{joined}");
    assert_eq!(joined.matches("concat").count(), 1, "{joined}");
}

/// A spread struct is the value it spreads under a record of the fields it
/// names, laid over it so the named ones win wherever the two share a field.
/// The value it spreads runs after every named field, which is where it was
/// written — and once, whatever the fields do with it.
#[test]
fn a_struct_spread_lays_the_named_fields_over_the_value() {
    assert_eq!(
        section("let c = { y: 2n }\nlet a = { x: 1n, ..c }", "global a"),
        r#"fn a#init(; %7: cont) entry b0 [f1, Synchronous]:
  b0(%7: cont):
    %2: nat = const 1n
    %3: struct = global c
    %4: struct = struct { x: %2 }
    %5: struct = merge %3, %4
    continue %7, %5"#
    );
    let output = lowered("let f = fn n => { y: n }\nlet a = { x: f 1n, ..f 2n }");
    let global = output.globals.iter().find(|g| g.name == "a").unwrap();
    let function = &output.functions[global.initializer];
    let next = |block: &lir::Block| {
        let lir::End::Call { continuation, .. } = block.end.kind else {
            panic!("expected a call")
        };
        block
            .instrs
            .iter()
            .find_map(|i| match &i.op {
                lir::Op::Continuation { code, .. } if i.temp == continuation => {
                    Some(&function.blocks[code.block])
                }
                _ => None,
            })
            .expect("call has a saved return destination")
    };
    let first = &function.blocks[function.entry];
    let second = next(first);
    let merged = next(second);
    assert!(
        first
            .instrs
            .iter()
            .any(|i| matches!(i.op, lir::Op::Const(ir::Literal::Natural(1))))
    );
    assert!(
        second
            .instrs
            .iter()
            .any(|i| matches!(i.op, lir::Op::Const(ir::Literal::Natural(2))))
    );
    assert!(
        merged
            .instrs
            .iter()
            .any(|i| matches!(i.op, lir::Op::Merge(_)))
    );
    assert!(matches!(merged.end.kind, lir::End::Continue { .. }));
}

/// A case no listed one covers needs somewhere 'to go: an arm that accepts
/// anything gives the dispatch its `else`. With every case of a closed row
/// listed there is nothing left over, and no `else` is written.
#[test]
fn a_tag_dispatch_marks_impossible_paths_unreachable() {
    assert!(
        !section(
            "let f = fn v => match v with | #A x => x | b => 0n end",
            "fn f("
        )
        .contains("unreachable")
    );
    assert!(
        section(
            "let f = fn v => match v with | #A x => x | #B => 0n end",
            "fn f("
        )
        .contains("unreachable")
    );
}

/// A field the solved type leaves undecided is tested at run time; one it proves
/// present is read straight out, above the test, since nothing about it is in
/// question.
#[test]
fn an_optional_field_becomes_a_presence_test() {
    assert_eq!(
        section(
            "let f = fn s => match s with | { x, y } => y | { x } => x end",
            "fn f("
        ),
        r#"fn f(%0: struct; %7: cont) entry b2 [f0, Synchronous]:
  b0(%0: struct, %7: cont):
    %2: any = project %0, "y"
    continue %7, %2
  b1(%1: any, %7: cont):
    continue %7, %1
  b2(%0: struct, %7: cont):
    %1: any = project %0, "x"
    branch_presence %0, "y" => b0(%0, %7), otherwise b1(%1, %7)"#
    );
}

#[test]
fn a_refined_swap_uses_the_existing_presence_switch() {
    for source in [
        "let swap : { a when 'a: Nat, b when 'b: Nat } -> \
         { a when 'b: Nat, b when 'a: Nat } where 'a != 'b = fn v =>\n\
         match v with | {a} => { b: a } | {b} => { a: b } end",
        "let swap = fn v => match v with \
         | {a} => { b: a } | {b} => { a: b } end",
    ] {
        let printed = section(source, "fn swap(");
        assert_eq!(
            printed.matches("branch_presence").count(),
            1,
            "the second presence is entailed on each path:\n{printed}"
        );
        assert!(printed.contains("project"), "{printed}");
        assert!(printed.contains("struct { b:"), "{printed}");
        assert!(printed.contains("struct { a:"), "{printed}");
    }

    let nested = section(
        "let nested = fn v => match v with\n\
         | {left} => match left with | {x} => 1n | {y} => 2n end\n\
         | {right} => 3n end",
        "fn nested(",
    );
    assert_eq!(nested.matches("branch_presence").count(), 2, "{nested}");
}

/// An exact pattern against a type that is open has to ask whether the value
/// carries anything beyond the fields the type names — which is the one thing
/// separating it from the open pattern beside it.
#[test]
fn an_exact_pattern_over_an_open_type_tests_the_rest() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x} => 1n | {x, ..} => 2n end",
            "fn f("
        ),
        r#"fn f(%0: struct; %8: cont) entry b2 [f0, Synchronous]:
  b0(%8: cont):
    %2: nat = const 1n
    continue %8, %2
  b1(%8: cont):
    %3: nat = const 2n
    continue %8, %3
  b2(%0: struct, %8: cont):
    %1: any = project %0, "x"
    branch_rest %0, ["x"] => b0(%8), otherwise b1(%8)"#
    );
}

/// A match every arm of which accepts everything tests nothing at all: the
/// first arm wins, and there is no dispatch to write.
#[test]
fn a_wildcard_match_tests_nothing() {
    assert_eq!(
        section("let w = fn v => match v with | _ => 1n end", "fn w("),
        r#"fn w(%0: any; %5: cont) entry b0 [f0, Synchronous]:
  b0(%5: cont):
    %1: nat = const 1n
    continue %5, %1"#
    );
}

/// A binder arm binds the temp the position it sits at already holds, rather
/// than reading the value out again.
#[test]
fn a_binder_arm_binds_the_scrutinee_temp() {
    assert_eq!(
        section(
            "let f = fn n => match n with | 0n => 1n | m => m end",
            "fn f("
        ),
        r#"fn f(%0: nat; %6: cont) entry b3 [f0, Synchronous]:
  b0(%0: nat, %6: cont):
    continue %6, %0
  b1(%6: cont):
    %1: nat = const 1n
    continue %6, %1
  b2(%0: nat, %6: cont):
    branch_prim %0, 0n => b1(%6), otherwise b0(%0, %6)
  b3(%0: nat, %6: cont):
    jump b2(%0, %6)"#
    );
}

/// Every scalar literal uses the one primitive dispatch, retaining its source
/// spelling in the case so a backend can compare the value at its own
/// representation.
#[test]
fn every_primitive_pattern_uses_branch_prim() {
    for (literal, case) in [
        ("1n", "1n =>"),
        ("1i", "1i =>"),
        ("1.5", "1.5 =>"),
        ("\"text\"", "\"text\" =>"),
        ("true", "true =>"),
    ] {
        let printed = section(
            &format!("let f = fn x => match x with | {literal} => {literal} | _ => {literal} end"),
            "fn f(",
        );
        assert!(printed.contains("branch_prim"), "{printed}");
        assert!(printed.contains(case), "{printed}");
    }
}

#[test]
fn a_complete_boolean_and_typed_wildcards_take_both_dispatch_paths() {
    let boolean = section(
        "let f = fn x => match x with | false => 0n | true => 1n end",
        "fn f(",
    );
    assert!(boolean.contains("branch_prim"), "{boolean}");
    assert!(!boolean.contains("else =>"), "{boolean}");

    for primitive in ["Nat", "Int", "Real", "String", "Bool"] {
        let printed = section(
            &format!("let f : {primitive} -> {primitive} = fn x => match x with | y => y end"),
            "fn f(",
        );
        assert!(!printed.contains("branch_prim"), "{primitive}: {printed}");
    }
}

/// A match with no arms constrained its scrutinee to the empty sum: a dispatch
/// with no case to take, and nothing left over for it to fall through to.
#[test]
fn a_match_with_no_arms_dispatches_over_nothing() {
    assert_eq!(
        section("let e = fn v => match v with end", "fn e("),
        r#"fn e(%0: sum; %5: cont) entry b1 [f0, Synchronous]:
  b0():
    unreachable
  b1():
    jump b0()"#
    );
}

/// Performing an operation is an ordinary call: the function's own hidden
/// evidence parameter holds the record the handler passed down, the operation is
/// read out of it, and what comes back is what the arm answered.
#[test]
fn performing_an_operation_reads_it_out_of_the_evidence() {
    assert_eq!(
        section(
            "effect Log = { write: Nat -> () }\nlet shout = fn x => !Log.write x",
            "fn shout("
        ),
        r#"fn shout(%0: struct, %1: nat; %8: cont) entry b0 [f0, MaySuspend]:
  b0(%0: struct, %1: nat, %8: cont):
    %2: fn = project %0, "write"
    call %2, %1 -> %8"#
    );
}

#[test]
fn unnamed_operations_use_the_private_evidence_slot_without_leaking_it() {
    let printed = section(
        "effect Log = Nat -> ()\n\
         let main = fn n => handle !Log n with | !Log value => () end",
        "fn main(",
    );
    assert!(printed.contains("<unnamed>"), "{printed}");
    assert!(!printed.contains("ruddy:unnamed-operation"), "{printed}");
}

/// A handler mints an identity, builds one record of operation closures per
/// effect it discharges, and runs its body under a `catch` on that identity —
/// with the direct call out of the body handed the record.
#[test]
fn a_handler_builds_evidence_and_catches_its_own_tag() {
    assert_eq!(
        section(
            "effect Log = { write: Nat -> () }\n\
             let shout = fn x => !Log.write x\n\
             let main = fn w => handle shout 5n with | !Log.write n => {} end",
            "fn main("
        ),
        r#"fn main(%8: any; %22: cont) entry b2 [f2, MaySuspend]:
  b0(%9: handler, %23: any) continuation:
    leave %9, %23
  b1(%9: handler, %13: struct):
    %14: nat = const 5n
    %24: cont = continuation main:b0, [%9] [f2]
    call shout, %13, %14 -> %24 [f0]
  b2(%22: cont):
    %9: handler = new_tag
    %12: fn = closure main#2, []
    %13: struct = struct { write: %12 }
    enter %9, b1(%9, %13), %22"#
    );
}

#[test]
fn structurally_equivalent_handler_arms_build_one_complete_record() {
    let source = "module Foo =\n  effect Log = { write: Nat -> (), flush: () -> () }\nend\n\
                  module Bar =\n  effect Log = { write: Nat -> (), flush: () -> () }\nend\n\
                  let main = fn n => handle Foo::!Log.write n with\n\
                    | Foo::!Log.write value => ()\n\
                    | Bar::!Log.flush unit => ()\n\
                  end";
    let printed = section(source, "fn main(");
    assert!(printed.contains("struct { write:"), "{printed}");
    assert!(printed.contains(", flush:"), "{printed}");
    assert_eq!(printed.matches("struct = struct {").count(), 1, "{printed}");
    assert!(printed.contains("project %"), "{printed}");
    assert!(printed.contains("\"write\""), "{printed}");
}

#[test]
fn imported_and_local_handler_arms_build_one_complete_evidence_record() {
    let declaration = "effect Log = { write: Nat -> (), flush: () -> () }";
    let interface = local_effect_interface(declaration);
    let plain = |ty| ty;
    let unit = || {
        a::Type::Struct(a::Row {
            labels: Vec::new(),
            rest: a::Rest::Closed,
        })
    };
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            kind: ruddy::artifact::Kind::Library,
            compiler: ruddy::artifact::Stamp::current(),
            domains: ruddy::types::Domains::default(),
            modules: Vec::new(),
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: vec![a::DeclaredEffect {
                exported: true,
                metadata: Default::default(),
                name: "dep@1.0.0::Log".into(),
                params: Vec::new(),
                identity: Some(a::EffectIdentity {
                    name: "Log".into(),
                    interface,
                }),
                kind: a::EffectKind::Operations(vec![
                    a::Operation {
                        selector: a::OperationSelector::Named("write".into()),
                        from: plain(a::Type::Nat),
                        to: unit(),
                    },
                    a::Operation {
                        selector: a::OperationSelector::Named("flush".into()),
                        from: unit(),
                        to: unit(),
                    },
                ]),
            }],
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
    .validate()
    .expect("a hand-built dependency validates");
    let source = format!(
        "{declaration}\n\
         let main = fn n => handle dep::!Log.write n with\n\
           | dep::!Log.write value => ()\n\
           | !Log.flush unit => ()\n\
         end"
    );
    let (output, labels) = lowered_with_dependencies(&source, &[dependency]);
    let printed = print::lir::program(&output, &labels);
    let main = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn main("))
        .unwrap_or_else(|| panic!("missing main in:\n{printed}"));
    assert!(main.contains("struct { write:"), "{main}");
    assert!(main.contains(", flush:"), "{main}");
    assert_eq!(main.matches("struct = struct {").count(), 1, "{main}");
    assert!(main.contains("project %"), "{main}");
    assert!(main.contains("\"write\""), "{main}");
}

/// The `return` arm is applied inline to the body's value on the normal path:
/// it binds that value, and the catch yields what it answers.
#[test]
fn a_return_arm_is_applied_on_the_normal_path() {
    let printed = section(
        "effect Log = { write: Nat -> () }\n\
         let main = fn w => handle 1n with | !Log.write n => {} | return r => { got: r } end",
        "fn main(",
    );
    assert!(printed.contains("%6: nat = const 1n"), "{printed}");
    assert!(printed.contains("struct { got: %6 }"), "{printed}");
}

/// A `raise` ends its arm with a throw to the identity the arm captured, and the
/// code after it is unreachable and is not emitted. The `catch` around the body
/// yields the thrown value directly, so the `return` arm never sees it.
#[test]
fn a_raise_throws_to_the_tag_its_arm_captured() {
    let source = "effect Fail = { oops: () -> Nat }\n\
         let recover = fn w =>\n\
           handle !Fail.oops () with | !Fail.oops z => raise 0n | return r => r end";
    assert_eq!(
        section(source, "fn recover#2"),
        r#"fn recover#2(%4: any, %2: unit; %18: cont) entry b0 [f2, Synchronous]:
  b0(%4: any):
    %3: nat = const 0n
    abort %4, %3"#
    );
    // The arm captures the very tag the `catch` beside it was minted with.
    let recover = section(source, "fn recover(");
    assert!(recover.contains("%1: handler = new_tag"), "{recover}");
    assert!(recover.contains("closure recover#2, [%1]"), "{recover}");
    assert!(recover.contains("enter %1,"), "{recover}");
}

/// Two handlers of one effect, one inside the other: each mints its own
/// identity, and the perform inside the inner body reaches the inner record.
#[test]
fn nested_handlers_of_one_effect_shadow_and_stay_apart() {
    let printed = section(
        "effect Log = { write: Nat -> () }\n\
         let nest = fn w =>\n\
           handle (handle !Log.write 1n with | !Log.write a => {} end)\n\
           with | !Log.write b => {} end",
        "fn nest(",
    );
    assert_eq!(printed.matches("new_tag").count(), 2, "{printed}");
    assert!(printed.contains("%1: handler = new_tag"), "{printed}");
    assert!(printed.contains("%6: handler = new_tag"), "{printed}");
    // The inner record — the one built beside the inner tag — is what the
    // perform reads from.
    assert!(
        printed.contains("%10: struct = struct { write: %9 }"),
        "{printed}"
    );
    assert!(
        printed.contains("%11: fn = project %10, \"write\""),
        "{printed}"
    );
}

/// A function whose effects are a bare row variable takes one bundle standing
/// for that variable, and hands it straight on to a callee whose row is the
/// very same variable rather than building a second one.
#[test]
fn an_effect_polymorphic_call_forwards_its_bundle() {
    assert_eq!(
        section(
            "let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n",
            "fn piped("
        ),
        r#"fn piped(%0: fn, %1: struct, %2: nat; %11: cont) entry b0 [f0, MaySuspend]:
  b0(%0: fn, %1: struct, %2: nat, %11: cont):
    call %0, %1, %2 -> %11"#
    );
}

/// A call whose callee's variable part is not the caller's own gets a bundle
/// built at the site, holding evidence for every effect the scope can handle —
/// and the function handed over, which takes a record of its own rather than a
/// bundle, is wrapped in an adapter that reads its record back out.
#[test]
fn a_bundle_is_built_where_none_can_be_forwarded() {
    let source = "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => n\n\
         let use = fn w => handle piped logger 1n with | !Log.write s => {} end";
    let printed = section(source, "fn use(");
    assert!(
        printed.contains("%22: struct = struct { write: %21 }"),
        "{printed}"
    );
    assert!(
        printed.contains("%31: struct = struct { Log: %22 }"),
        "{printed}"
    );
    assert!(
        printed.contains("%30: fn = closure use#3, [%23]"),
        "{printed}"
    );
    assert!(printed.contains("call piped, %30, %31, %24"), "{printed}");
    assert_eq!(
        section(source, "fn use#3"),
        r#"fn use#3(%25: fn, %26: struct, %27: nat; %47: cont) entry b0 [f8, MaySuspend]:
  b0(%25: fn, %26: struct, %27: nat, %47: cont):
    %28: struct = project %26, "Log"
    call %25, %28, %27 -> %47"#
    );
}

/// The evidence a function is handed is the evidence it was compiled to read.
///
/// A function performing `Log` takes one record of `Log`'s operations; a
/// parameter polymorphic in its effects is handed one bundle keyed by effect
/// name. Passing the first into the second puts an adapter between them, so
/// that the record the body projects `"write"` out of is the record the handler
/// built — rather than the bundle holding it.
#[test]
fn a_function_receives_the_evidence_it_projects() {
    let source = "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => do let z = !Log.write n return n end\n\
         let use = fn w => handle piped logger 1n with | !Log.write s => {} end";
    // The record `logger` reads is its first parameter, and the wrapper that
    // stands for `logger` as a value passes its own first parameter straight on.
    assert!(
        section(source, "fn logger(").contains("%13: fn = project %11, \"write\""),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn logger#1"),
        r#"fn logger#1(%15: struct, %16: nat; %44: cont) entry b0 [f4, MaySuspend]:
  b0(%15: struct, %16: nat, %44: cont):
    call logger, %15, %16 -> %44 [f3]"#
    );
    // The adapter reads that record out of the bundle `piped` was declared to
    // take, and the bundle the call built holds the handler's record under the
    // effect's name.
    assert_eq!(
        section(source, "fn use#3"),
        r#"fn use#3(%27: fn, %28: struct, %29: nat; %50: cont) entry b0 [f8, MaySuspend]:
  b0(%27: fn, %28: struct, %29: nat, %50: cont):
    %30: struct = project %28, "Log"
    call %27, %30, %29 -> %50"#
    );
    let use_ = section(source, "fn use(");
    assert!(
        use_.contains("%24: struct = struct { write: %23 }"),
        "{use_}"
    );
    assert!(use_.contains("%33: struct = struct { Log: %24 }"), "{use_}");
}

/// An operation used as a value becomes a wrapper taking the effect's evidence
/// and the payload — which is what puts the effect in the wrapper's own arrow
/// row — and the call site hands that evidence in like any other.
#[test]
fn an_operation_used_as_a_value_gets_a_wrapper() {
    let source = "effect Log = { write: Nat -> () }\n\
         let op = !Log.write\n\
         let go = fn w => handle op 1n with | !Log.write n => {} end";
    assert_eq!(
        section(source, "fn op#1"),
        r#"fn op#1(%0: struct, %1: nat; %22: cont) entry b0 [f2, MaySuspend]:
  b0(%0: struct, %1: nat, %22: cont):
    %2: fn = project %0, "write"
    call %2, %1 -> %22"#
    );
    assert_eq!(
        section(source, "global op"),
        r#"fn op#init(; %24: cont) entry b0 [f4, Synchronous]:
  b0(%24: cont):
    %4: fn = closure op#1, []
    continue %24, %4"#
    );
    assert!(section(source, "fn go(").contains("call %11, %10, %12"));
}

/// An effect the row proves performed earns a parameter of its own, and a row
/// variable beside it still earns the bundle — so a function can both perform
/// something and forward whatever its caller performs.
#[test]
fn a_definite_effect_and_an_open_rest_are_both_passed() {
    assert_eq!(
        section(
            "effect Log = { write: Nat -> () }\n\
             let both = fn g => do let z = !Log.write 1n return g 2n end",
            "fn both#1"
        ),
        r#"fn both#1(%0: type_descriptor, %1: struct, %2: struct, %3: fn; %10: cont) entry b1 [f0, MaySuspend]:
  b0(%0: type_descriptor, %1: struct, %2: struct, %3: fn, %10: cont, %6: unit) continuation:
    %7: nat = const 2n
    call %3, %0, %1, %2, %7 -> %10
  b1(%0: type_descriptor, %1: struct, %2: struct, %3: fn, %10: cont):
    %4: fn = project %1, "write"
    %5: nat = const 1n
    %11: cont = continuation both#1:b0, [%0, %1, %2, %3, %10] [f0]
    call %4, %5 -> %11"#
    );
}

/// The whole listing of a small program, exactly: one section per function and
/// per global, a header line, one instruction per line as `%N: rep = op
/// operands`, and the terminator written bare. This is the format the tab and
/// every test above read, so it is pinned in one place.
#[test]
fn the_listing_is_the_canonical_format() {
    assert_eq!(
        listing("let add = fn a => fn b => a\nlet main = add 1n 2n"),
        r#"fn add(%0: any, %1: any; %11: cont) entry b0 [f0, Synchronous]:
  b0(%0: any, %11: cont):
    continue %11, %0

fn add#1(%2: any; %12: cont) entry b0 [f1, Synchronous]:
  b0(%2: any, %12: cont):
    %3: fn = closure add#2, [%2]
    continue %12, %3

fn add#2(%4: any, %5: any; %13: cont) entry b0 [f2, Synchronous]:
  b0(%4: any, %5: any, %13: cont):
    call add, %4, %5 -> %13 [f0]

fn add#init(; %14: cont) entry b0 [f3, Synchronous]:
  b0(%14: cont):
    %7: fn = closure add#1, []
    continue %14, %7

fn main#init(; %15: cont) entry b0 [f4, Synchronous]:
  b0(%15: cont):
    %8: nat = const 1n
    %9: nat = const 2n
    call add, %8, %9 -> %15 [f0]

global add:
  initializer f3 (add#init)

global main:
  initializer f4 (main#init)
"#
    );
}

/// A program with nothing in it lowers to nothing, rather than to a listing of
/// one empty thing.
#[test]
fn an_empty_program_lowers_to_an_empty_output() {
    let (output, labels) = lowered_labelled("");
    assert!(output.globals.is_empty());
    assert!(output.functions.is_empty());
    assert_eq!(print::lir::program(&output, &labels), "");
}

/// A declared sum applied to more cases is one row by the time the dispatch is
/// built: the tail a use site handed the declaration is flattened into the
/// cases beside it, so both are ordinary entries of one `branch_tag`.
#[test]
fn a_declared_sums_cases_are_flattened_before_dispatch() {
    assert_eq!(
        section(
            "type Fallible 'r = #Err Nat | ..'r\n\
             let h : Fallible (#Ok Nat) -> Nat =\n\
               fn t => match t with | #Err n => n | #Ok n => n end",
            "fn h("
        ),
        r#"fn h(%0: sum; %7: cont) entry b5 [f0, Synchronous]:
  b0():
    unreachable
  b1(%0: sum, %7: cont):
    %2: nat = payload %0
    continue %7, %2
  b2(%0: sum, %7: cont):
    branch_tag %0, #Ok => b1(%0, %7), otherwise b0()
  b3(%0: sum, %7: cont):
    %1: nat = payload %0
    continue %7, %1
  b4(%0: sum, %7: cont):
    branch_tag %0, #Err => b3(%0, %7), otherwise b2(%0, %7)
  b5(%0: sum, %7: cont):
    jump b4(%0, %7)"#
    );
}

/// `()` is the exact struct naming no fields, and over the unit type there is
/// nothing about it left to ask.
#[test]
fn a_unit_pattern_tests_nothing() {
    assert_eq!(
        section("let u = fn v => match v with | () => 1n end", "fn u("),
        r#"fn u(%0: unit; %5: cont) entry b0 [f0, Synchronous]:
  b0(%5: cont):
    %1: nat = const 1n
    continue %5, %1"#
    );
}

/// A capture is one parameter however often the body uses what it stands for.
#[test]
fn a_value_captured_twice_is_one_parameter() {
    let source = "let d = fn y => { g: fn x => { p: y, q: y } }";
    assert!(section(source, "fn d(").contains("closure d#2, [%0]"));
    assert_eq!(
        section(source, "fn d#2"),
        r#"fn d#2(%2: any, %1: any; %11: cont) entry b0 [f2, Synchronous]:
  b0(%2: any, %11: cont):
    %3: struct = struct { p: %2, q: %2 }
    continue %11, %3"#
    );
}

/// An open pattern says nothing about a field it does not mention, so it goes
/// down both halves of that field's test — while the exact pattern beside it
/// still has to ask what lies beyond the fields it names.
#[test]
fn an_open_pattern_says_nothing_about_a_field_it_omits() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x, ..} => 1n | {y} => 2n | _ => 3n end",
            "fn f("
        ),
        r#"fn f(%0: struct; %13: cont) entry b6 [f0, Synchronous]:
  b0(%0: struct, %13: cont):
    %1: any = project %0, "x"
    %2: nat = const 1n
    continue %13, %2
  b1(%13: cont):
    %4: nat = const 2n
    continue %13, %4
  b2(%13: cont):
    %5: nat = const 3n
    continue %13, %5
  b3(%0: struct, %13: cont):
    %3: any = project %0, "y"
    branch_rest %0, ["x", "y"] => b1(%13), otherwise b2(%13)
  b4(%13: cont):
    %7: nat = const 3n
    continue %13, %7
  b5(%0: struct, %13: cont):
    branch_presence %0, "y" => b3(%0, %13), otherwise b4(%13)
  b6(%0: struct, %13: cont):
    branch_presence %0, "x" => b0(%0, %13), otherwise b5(%0, %13)"#
    );
}

/// A field the type names but no arm asks about is neither tested nor read: an
/// annotation can put one there, and the dispatch has nothing to do with it.
#[test]
fn a_field_no_arm_asks_about_is_never_read() {
    assert_eq!(
        section(
            "let f : { x: Nat, y: Nat } -> Nat = fn s => match s with | {x, ..} => x end",
            "fn f("
        ),
        r#"fn f(%0: struct; %5: cont) entry b0 [f0, Synchronous]:
  b0(%0: struct, %5: cont):
    %1: nat = project %0, "x"
    continue %5, %1"#
    );
}

#[test]
fn a_field_every_arm_ignores_is_tested_but_not_read() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x: _, y: _} => 1n | {x: _} => 2n end",
            "fn f("
        ),
        r#"fn f(%0: struct; %7: cont) entry b2 [f0, Synchronous]:
  b0(%7: cont):
    %1: nat = const 1n
    continue %7, %1
  b1(%7: cont):
    %2: nat = const 2n
    continue %7, %2
  b2(%0: struct, %7: cont):
    branch_presence %0, "y" => b0(%7), otherwise b1(%7)"#
    );
}

/// An operation whose result is itself a function: the perform is the first
/// call, and what is left is applied to the result one argument at a time.
#[test]
fn an_operation_returning_a_function_keeps_applying() {
    let printed = section(
        "effect Mk = { mk: Nat -> (Nat -> Nat) }\n\
         let go = fn w => handle !Mk.mk 1n 2n with | !Mk.mk n => fn z => z end",
        "fn go(",
    );
    assert!(printed.contains("%7: fn = project %6, \"mk\""), "{printed}");
    assert!(printed.contains("call %7, %8"), "{printed}");
    assert!(printed.contains("call %9, %10"), "{printed}");
}

/// A nested binding whose own effects reach no further than itself still gets
/// its bundle: the row it performs at is a variable, so the evidence for it has
/// to travel, and the lifted function takes it beside what it captured.
#[test]
fn a_nested_binding_takes_a_bundle_of_its_own() {
    assert_eq!(
        section(
            "let outer = fn z => do let g = fn h => h z return 0n end",
            "fn outer#2"
        ),
        r#"fn outer#2(%5: any, %1: type_descriptor, %2: type_descriptor, %3: struct, %4: fn; %14: cont) entry b0 [f2, MaySuspend]:
  b0(%1: type_descriptor, %2: type_descriptor, %3: struct, %4: fn, %5: any, %14: cont):
    call %4, %1, %2, %3, %5 -> %14"#
    );
}

/// Each CPS block closes over explicitly declared parameters; definitions are
/// unique within the block even when a temp is forwarded to several blocks.
#[test]
fn every_block_defines_each_temp_once() {
    for source in [
        "let three = fn a => fn b => fn c => a\nlet p = three 1n",
        "effect Log = { write: Nat -> () }\n\
         let shout = fn x => fn y => !Log.write x\n\
         let go = fn w => handle shout 1n with | !Log.write n => {} end",
        "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => do let z = !Log.write n return n end\n\
         let use = fn w => handle piped logger 1n with | !Log.write s => {} end",
    ] {
        let output = lowered(source);
        for function in &output.functions {
            for block in &function.blocks {
                let mut defined = std::collections::HashSet::new();
                for temp in block
                    .params
                    .iter()
                    .map(|p| p.temp)
                    .chain(block.instrs.iter().map(|i| i.temp))
                {
                    assert!(defined.insert(temp), "duplicate %{temp} in {function:?}");
                }
            }
        }
    }
}

/// A value takes the evidence of the row it was written with, and the position
/// it is handed to passes the evidence of the row *that* was declared with. The
/// two agree here — both name `Log` and nothing else — so the function goes
/// across as it is, with no adapter between.
#[test]
fn evidence_of_the_same_shape_is_handed_straight_over() {
    let source = "effect Log = { write: Nat -> () }\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1n\n\
         let noisy = fn n => do let z = !Log.write n return n end\n\
         let same = fn w => handle takes noisy with | !Log.write s => {} end";
    let printed = section(source, "fn same(");
    assert!(printed.contains("%22: fn = global noisy"), "{printed}");
    assert!(printed.contains("call takes, %21, %22"), "{printed}");
}

/// Fitting compares two evidence shapes, and a type that does not pin its
/// value down to a function has none to compare — so neither side is walked
/// into and nothing is repacked.
///
/// `idv` names `id` without applying it, so the call goes through a value
/// rather than a known chain, and what `id` gives back is the very variable
/// its scheme quantified. The value crosses as it stands; the call that
/// eventually reaches it passes the evidence the row at *that* call asks for,
/// which is `go`'s own bundle.
#[test]
fn a_level_no_type_pins_down_is_crossed_rather_than_repacked() {
    let source = "let id = fn x => x\nlet idv = id\nlet go = fn f => (idv f) 1n";
    assert_eq!(
        section(source, "fn go#3"),
        r#"fn go#3(%5: type_descriptor, %6: struct, %7: fn; %28: cont) entry b1 [f4, MaySuspend]:
  b0(%5: type_descriptor, %6: struct, %28: cont, %20: fn) continuation:
    %21: nat = const 1n
    call %20, %5, %6, %21 -> %28
  b1(%5: type_descriptor, %6: struct, %7: fn, %28: cont):
    %8: fn = global idv
    %13: fn = closure go#1, [%8, %5]
    %19: fn = closure go#2, [%7]
    %29: cont = continuation go#3:b0, [%5, %6, %28] [f4]
    call %13, %19 -> %29"#
    );
}

/// A row that names `Log` and carries a variable part beside it is two pieces
/// of evidence, and a position declaring only `Log` passes one. The adapter
/// packs what it was given into the bundle the value expects, keyed by the
/// effect's name, and hands the record itself over unchanged.
#[test]
fn recursive_arrow_fitting_reuses_one_guarded_adapter() {
    let source = "effect Log = { write: Nat -> () }\n\
         type Runs 'r = Nat -> Runs 'r + ..'r\n\
         let poly : Runs (..'r) = fn n => poly\n\
         let takes : Runs (!Log) -> Nat + !Log = fn f => 1n\n\
         let go = fn w => handle takes poly with | !Log.write n => {} end";
    let adapter = section(source, "fn go#3");
    assert!(adapter.contains("struct { Log:"), "{adapter}");
    assert!(adapter.contains("call %"), "{adapter}");
    assert_eq!(adapter.matches("closure go#3").count(), 1, "{adapter}");
}

#[test]
fn an_adapter_packs_a_record_into_the_bundle_a_value_expects() {
    let source = "effect Log = { write: Nat -> () }\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1n\n\
         let openish : Nat -> Nat + !Log + ..'r = fn n => do let z = !Log.write n return n end\n\
         let packed = fn w => handle takes openish with | !Log.write s => {} end";
    assert_eq!(
        section(source, "fn openish("),
        r#"fn openish(%8: struct, %9: struct, %10: nat; %38: cont) entry b1 [f2, MaySuspend]:
  b0(%10: nat, %38: cont, %12: unit) continuation:
    continue %38, %10
  b1(%8: struct, %10: nat, %38: cont):
    %11: fn = project %8, "write"
    %39: cont = continuation openish:b0, [%10, %38] [f2]
    call %11, %10 -> %39"#
    );
    assert_eq!(
        section(source, "fn packed#3"),
        r#"fn packed#3(%25: fn, %26: struct, %27: nat; %46: cont) entry b0 [f7, MaySuspend]:
  b0(%25: fn, %26: struct, %27: nat, %46: cont):
    %28: struct = struct { Log: %26 }
    call %25, %26, %28, %27 -> %46"#
    );
    let printed = section(source, "fn packed(");
    assert!(
        printed.contains("%30: fn = closure packed#3, [%24]"),
        "{printed}"
    );
    assert!(printed.contains("call takes, %23, %30"), "{printed}");
}

/// The other way round: a position polymorphic in its effects passes one
/// bundle, and the value wants a record of its own *and* a bundle beside it.
/// The adapter reads the record out of the bundle by the effect's name and
/// forwards the bundle as it stands — it is already the whole of the variable
/// part. The function-typed argument crosses the same boundary the other way
/// round — it arrives shaped for the bundle alone, and `both` calls it with a
/// record and a bundle — so a second adapter stands between the two of them.
///
/// That second adapter is handed both pieces and has to give back one: the
/// record `both` passes for `Log` is laid over the bundle it passes for the
/// rest, so what the value reads through its tail is everything the adapter
/// was given rather than the named part alone.
#[test]
fn an_adapter_forwards_the_bundle_it_was_handed() {
    let source = "effect Log = { write: Nat -> () }\n\
         let hof2 : ((Nat -> Nat + ..'r) -> Nat + ..'r) -> Nat + ..'r =\n\
           fn k => k (fn m => m)\n\
         let both : (Nat -> Nat + !Log + ..'s) -> Nat + !Log + ..'s =\n\
           fn g => do let z = !Log.write 1n return g 2n end\n\
         let fwd : {} -> Nat + !Log + ..'t = fn w => hof2 both";
    assert!(
        section(source, "fn both(").starts_with("fn both(%10: struct, %11: struct, %12: fn;"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn fwd#3"),
        r#"fn fwd#3(%27: fn, %28: struct, %29: fn; %56: cont) entry b0 [f8, MaySuspend]:
  b0(%27: fn, %28: struct, %29: fn, %56: cont):
    %30: struct = project %28, "Log"
    %38: fn = closure fwd#2, [%29]
    call %27, %30, %28, %38 -> %56"#
    );
    assert_eq!(
        section(source, "fn fwd#2"),
        r#"fn fwd#2(%31: fn, %32: struct, %33: struct, %34: nat; %55: cont) entry b0 [f7, MaySuspend]:
  b0(%31: fn, %32: struct, %33: struct, %34: nat, %55: cont):
    %35: struct = struct { Log: %32 }
    %36: struct = merge %33, %35
    call %31, %36, %34 -> %55"#
    );
}

/// Applying a known definition short of its arity hands over a wrapper, and a
/// wrapper takes the evidence the *definition* was compiled with — not the
/// evidence the use site's own type asks for.
///
/// `two 1` reads as `Nat -> Nat + !Log`, one record and no bundle; the wrapper
/// it becomes takes a record and a bundle both, because that is the row `two`
/// was written with. So the partial application is fitted like any other
/// mismatch: an adapter that takes what `takes` passes and rebuilds the bundle
/// the wrapper expects.
#[test]
fn a_partial_application_is_fitted_to_the_shape_its_wrapper_takes() {
    let source = "effect Log = { write: Nat -> () }\n\
         let two : Nat -> Nat -> Nat + !Log + ..'s =\n\
           fn a => fn b => do let z = !Log.write a return b end\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1n\n\
         let go = fn w => handle takes (two 1n) with | !Log.write s => {} end";
    // The wrapper the partial application becomes takes a record and a bundle.
    assert!(
        section(source, "fn two#2")
            .starts_with("fn two#2(%8: nat, %9: struct, %10: struct, %11: nat;"),
        "{}",
        listing(source)
    );
    // `takes` passes one record and the argument, so the adapter's parameters
    // after its capture are exactly those two.
    assert_eq!(
        section(source, "fn go#3"),
        r#"fn go#3(%30: fn, %31: struct, %32: nat; %52: cont) entry b0 [f8, MaySuspend]:
  b0(%30: fn, %31: struct, %32: nat, %52: cont):
    %33: struct = struct { Log: %31 }
    call %30, %31, %33, %32 -> %52"#
    );
    let go = section(source, "fn go(");
    assert!(go.contains("%29: fn = closure two#2, [%28]"), "{go}");
    assert!(go.contains("%35: fn = closure go#3, [%29]"), "{go}");
    assert!(go.contains("call takes, %27, %35"), "{go}");
}

/// A *full* application is a value like any other: the wrappers are behind it,
/// not in front, so what it stands for is the type it came to and nothing is
/// fitted. `pick 1 2` is already the function `takes` was declared to take, and
/// it goes straight across with no adapter between them.
#[test]
fn a_full_application_stands_for_what_it_came_to() {
    let source = "effect Log = { write: Nat -> () }\n\
         let noisy : Nat -> Nat + !Log = fn n => do let z = !Log.write n return n end\n\
         let pick : Nat -> Nat -> (Nat -> Nat + !Log) = fn a => fn b => noisy\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1n\n\
         let go = fn w => handle takes (pick 1n 2n) with | !Log.write s => {} end";
    assert_eq!(
        section(source, "fn go("),
        r#"fn go(%25: any; %47: cont) entry b3 [f7, MaySuspend]:
  b0(%26: handler, %48: any) continuation:
    leave %26, %48
  b1(%26: handler, %30: struct, %33: fn) continuation:
    %49: cont = continuation go:b0, [%26] [f7]
    call takes, %30, %33 -> %49 [f5]
  b2(%26: handler, %30: struct):
    %31: nat = const 1n
    %32: nat = const 2n
    %50: cont = continuation go:b1, [%26, %30] [f7]
    call pick, %31, %32 -> %50 [f2]
  b3(%47: cont):
    %26: handler = new_tag
    %29: fn = closure go#2, []
    %30: struct = struct { write: %29 }
    enter %26, b2(%26, %30), %47"#
    );
}

/// A definition is fitted where its value is *read*, not where it happens to be
/// passed, so a binding between the read and the call changes nothing: the temp
/// `h` names already holds the shape `h`'s own type promises, and handing it to
/// `run` is handing over a function `run` can call.
#[test]
fn a_definition_read_through_a_binding_arrives_callable() {
    let source = "effect A = { a: Nat -> () }\n\
         effect B = { b: Nat -> () }\n\
         let poly : Nat -> Nat + ..'e = fn n => n\n\
         let run : (Nat -> Nat + !A + !B) -> Nat + !A + !B = fn f => f 1n\n\
         let go = fn w => handle handle\n\
           (do let h : Nat -> Nat + !A + !B = poly return run h end)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    // `poly` was compiled with one tail bundle before its argument, and `run`
    // calls what it is given with two records before it — so the adapter packs
    // the two into the bundle, keyed by effect name.
    assert!(
        section(source, "fn poly(").starts_with("fn poly(%0: struct, %1: nat;"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn go#4"),
        r#"fn go#4(%28: fn, %29: struct, %30: struct, %31: nat; %53: cont) entry b0 [f8, MaySuspend]:
  b0(%28: fn, %29: struct, %30: struct, %31: nat, %53: cont):
    %32: struct = struct { A: %29, B: %30 }
    call %28, %32, %31 -> %53"#
    );
    // The adapter is built where the global is read, and it is the adapter that
    // travels through the binding into the call.
    let go = section(source, "fn go(");
    assert!(go.contains("%27: fn = global poly"), "{go}");
    assert!(go.contains("%34: fn = closure go#4, [%27]"), "{go}");
    assert!(go.contains("call run, %26, %21, %34"), "{go}");
    // Which is what makes the indirect call inside `run` reach a function of as
    // many parameters as it passes arguments.
    assert_eq!(
        section(source, "fn run("),
        r#"fn run(%6: struct, %7: struct, %8: fn; %43: cont) entry b0 [f2, MaySuspend]:
  b0(%6: struct, %7: struct, %8: fn, %43: cont):
    %9: nat = const 1n
    call %8, %6, %7, %9 -> %43"#
    );
}

/// The same, with a function return in place of the binding: `get` hands back
/// what `poly` stands for at `get`'s own declared type, so the fitting happens
/// inside `get` and the caller has nothing left to do.
#[test]
fn a_definition_returned_from_a_function_arrives_callable() {
    let source = "effect A = { a: Nat -> () }\n\
         effect B = { b: Nat -> () }\n\
         let poly : Nat -> Nat + ..'e = fn n => n\n\
         let get : {} -> (Nat -> Nat + !A + !B) = fn u => poly\n\
         let run : (Nat -> Nat + !A + !B) -> Nat + !A + !B = fn f => f 1n\n\
         let go = fn w => handle handle run (get {})\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    assert_eq!(
        section(source, "fn get("),
        r#"fn get(%6: unit; %49: cont) entry b0 [f2, Synchronous]:
  b0(%49: cont):
    %7: fn = global poly
    %14: fn = closure get#2, [%7]
    continue %49, %14"#
    );
    assert_eq!(
        section(source, "fn get#2"),
        r#"fn get#2(%8: fn, %9: struct, %10: struct, %11: nat; %60: cont) entry b0 [f8, MaySuspend]:
  b0(%8: fn, %9: struct, %10: struct, %11: nat, %60: cont):
    %12: struct = struct { A: %9, B: %10 }
    call %8, %12, %11 -> %60"#
    );
    assert_eq!(
        section(source, "fn run("),
        r#"fn run(%18: struct, %19: struct, %20: fn; %51: cont) entry b0 [f4, MaySuspend]:
  b0(%18: struct, %19: struct, %20: fn, %51: cont):
    %21: nat = const 1n
    call %20, %18, %19, %21 -> %51"#
    );
}

/// An effect-polymorphic value called through an *unannotated* binding is
/// called at the shape it holds, not at the shape this use instantiates it to.
///
/// `h` names the very value `app`'s global holds — a closure of `app#1`, which
/// takes one bundle and one argument — while the use reads `h` at a row naming
/// `A` and `B` outright. The call goes by what the value holds: one bundle
/// built at the site, keyed by effect name, and `both` adapted from its two
/// records down to the bundle `app` hands it — exactly the call `direct`
/// makes without the binding in the way.
#[test]
fn a_polymorphic_value_bound_by_a_let_is_called_at_the_shape_it_holds() {
    let source = "effect A = { a: Nat -> () }\n\
         effect B = { b: Nat -> () }\n\
         let both : Nat -> Nat + !A + !B = fn n => do let x = !A.a n let y = !B.b n return n end\n\
         let app = fn g => g 1n\n\
         let direct = fn w => handle handle app both\n\
           with | !A.a s => {} end with | !B.b s => {} end\n\
         let bound = fn w => handle handle (do let h = app return h both end)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    execute_effect_layout(
        source,
        "{ direct: direct (), bound: bound () }",
        "assert.equal(app.result.direct, 1); assert.equal(app.result.bound, 1);",
    );
}

/// A lambda parameter applied at two different instantiations in one body is
/// called both times at the shape the parameter holds — the declared type it
/// arrived at — so both calls pass exactly as many arguments as the lifted
/// function has parameters, and the argument whose own shape differs gets an
/// adapter.
#[test]
fn a_parameter_is_called_at_one_shape_however_the_uses_instantiate_it() {
    let source = "effect Log = { write: Nat -> () }\n\
         let noisy : Nat -> Nat + !Log = fn n => do let z = !Log.write n return n end\n\
         let pure = fn n => n\n\
         let go = fn w => handle (do let h = fn g => g 1n return { a: h noisy, b: h pure } end)\n\
           with | !Log.write s => {} end";
    execute_effect_layout(
        source,
        "go ()",
        "assert.equal(app.result.a, 1); assert.equal(app.result.b, 1);",
    );
}

/// An effect-polymorphic value returned from a call is called at the shape it
/// holds: what `pick {}` gives back is shaped by `pick`'s own last arrow, so
/// the call of the result builds the bundle keyed by effect name — not a raw
/// per-effect record — and adapts the argument, exactly as if `app` had been
/// named on the spot.
#[test]
fn a_value_returned_from_a_call_is_called_at_the_shape_it_holds() {
    let source = "effect A = { a: Nat -> () }\n\
         effect B = { b: Nat -> () }\n\
         let both : Nat -> Nat + !A + !B = fn n => do let x = !A.a n let y = !B.b n return n end\n\
         let app = fn g => g 1n\n\
         let pick = fn u => app\n\
         let late = fn w => handle handle (pick {}) both\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    execute_effect_layout(source, "late ()", "assert.equal(app.result, 1);");
}

/// A function value stored in a struct field keeps its shape across the
/// container: the field was stored at the type the *definition* built the
/// struct with, so the projection reads it at that type — one tail bundle
/// before the argument — however the use instantiates the container. The
/// adapters stand at the projection, and the function-typed argument crossing
/// into the stored value goes through one of its own, unpacking the bundle
/// back into the records `both` was compiled to take.
#[test]
fn a_function_read_out_of_a_struct_field_is_called_at_the_shape_the_field_holds() {
    let source = "effect Log = { write: Nat -> () }\n\
         effect Fail = { oops: Nat -> () }\n\
         let s = { f: fn g => fn n => g n }\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           do let a = !Log.write n let b = !Fail.oops n return n end\n\
         let go = fn w => handle handle s.f both 1n\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    execute_effect_layout(source, "go ()", "assert.equal(app.result, 1);");
}

/// Containers returned by both direct and indirect calls retain the type at
/// which the callee produced them. Projecting an effect-polymorphic member must
/// therefore install the same ABI adapters as projecting from a global struct.
#[test]
fn call_results_keep_container_production_metadata_for_projection() {
    let source = "effect Log = { write: Nat -> () }\n\
         effect Fail = { oops: Nat -> () }\n\
         let stored = { f: fn g => fn n => g n }\n\
         let direct = fn u => stored\n\
         let indirect = fn k => k {}\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           do let a = !Log.write n let b = !Fail.oops n return n end\n\
         let go = fn w => handle handle\n\
           do let a = (direct {}).f both 1n\n\
           return (indirect direct).f both a end\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    execute_effect_layout(source, "go ()", "assert.equal(app.result, 1);");
}

/// A struct-pattern projection has the same storage authority as an expression
/// projection. In particular, it reads the function at the bundle ABI the
/// polymorphic container was built with before fitting the pattern binding to
/// this effect-specialized use.
#[test]
fn a_function_bound_by_a_struct_pattern_keeps_the_stored_effect_abi() {
    let source = "effect Log = { write: Nat -> () }\n\
         effect Fail = { oops: Nat -> () }\n\
         let s = { f: fn g => fn n => g n }\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           do let a = !Log.write n let b = !Fail.oops n return n end\n\
         let go = fn w => handle handle\n\
           (match s with | { f } => f both 1n end)\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    execute_effect_layout(source, "go ()", "assert.equal(app.result, 1);");
}

/// The same value carried as a sum payload behaves identically: the payload
/// read takes the case's shape from the type the tag was *built* at, and the
/// match binder holds a value fitted from that shape to the use's own.
#[test]
fn a_function_carried_as_a_sum_payload_is_called_at_the_shape_the_case_holds() {
    let source = "effect Log = { write: Nat -> () }\n\
         effect Fail = { oops: Nat -> () }\n\
         let wrap = #F (fn g => fn n => g n)\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           do let a = !Log.write n let b = !Fail.oops n return n end\n\
         let go = fn w => handle handle\n\
           (match wrap with | #F h => h both 1n | _ => 0n end)\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    execute_effect_layout(source, "go ()", "assert.equal(app.result, 1);");
}

/// A match whose arms yield function values fits each one, at its own leaf,
/// from the shape it holds to what the whole match stands for — so the switch
/// temp can be called at the match's own type, and the caller's positional
/// records land on adapters that pack them into the bundle each arm's value
/// actually takes.
#[test]
fn a_match_arm_yields_a_function_fitted_to_what_the_match_stands_for() {
    let source = "effect A = { a: Nat -> () }\n\
         effect B = { b: Nat -> () }\n\
         let both : Nat -> Nat + !A + !B = fn n => do let x = !A.a n let y = !B.b n return n end\n\
         let app = fn g => g 1n\n\
         let pick = fn u => app\n\
         let go = fn c => handle handle\n\
           ((match c with | #L => pick {} | _ => pick {} end) both)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    execute_effect_layout(
        source,
        "{ left: go #L, right: go #R }",
        "assert.equal(app.result.left, 1); assert.equal(app.result.right, 1);",
    );
}

/// A value already shaped as the container's type declares goes in and comes
/// out untouched: the store fits trivially, the read fits trivially, and the
/// listing carries not one instruction beyond the ones the source wrote.
#[test]
fn a_value_already_at_the_containers_shape_goes_in_and_out_untouched() {
    let source = "effect Log = { write: Nat -> () }\n\
         let s : { f: Nat -> Nat + !Log } = { f: fn n => do let z = !Log.write n return n end }\n\
         let go = fn w => handle s.f 1n with | !Log.write x => {} end";
    assert_eq!(
        section(source, "global s"),
        r#"fn s#init(; %27: cont) entry b0 [f4, Synchronous]:
  b0(%27: cont):
    %4: fn = closure s#1, []
    %5: struct = struct { f: %4 }
    continue %27, %5"#
    );
    assert_eq!(
        section(source, "fn go("),
        r#"fn go(%6: any; %20: cont) entry b2 [f0, MaySuspend]:
  b0(%7: handler, %21: any) continuation:
    leave %7, %21
  b1(%7: handler, %11: struct):
    %12: struct = global s
    %13: fn = project %12, "f"
    %14: nat = const 1n
    %22: cont = continuation go:b0, [%7] [f0]
    call %13, %11, %14 -> %22
  b2(%20: cont):
    %7: handler = new_tag
    %10: fn = closure go#2, []
    %11: struct = struct { write: %10 }
    enter %7, b1(%7, %11), %20"#
    );
}

/// Tagging a value whose type nothing pinned down: the case's own word is
/// anything at all, which says nothing about a shape, so the payload goes in
/// exactly as it stands — no fitting, no extra instruction.
#[test]
fn a_payload_no_type_pinned_down_goes_in_as_it_stands() {
    assert_eq!(
        section("let f = fn v => #F v", "fn f("),
        r#"fn f(%0: any; %5: cont) entry b0 [f0, Synchronous]:
  b0(%0: any, %5: cont):
    %1: sum = tag #F, %0
    continue %5, %1"#
    );
}

/// A case the production type never listed — here `G`, which only the use's
/// instantiation of `w`'s open row can carry — has no authoritative payload
/// type to read, so the payload is taken at the use's own word.
#[test]
fn a_case_the_production_type_never_named_reads_at_the_uses_word() {
    assert_eq!(
        section(
            "let w = #F 1n\nlet f = fn v => match w with | #F x => x | #G 0n => 7n | _ => 0n end",
            "fn f("
        ),
        r#"fn f(%2: any; %14: cont) entry b8 [f0, Synchronous]:
  b0(%14: cont):
    %9: nat = const 0n
    continue %14, %9
  b1(%14: cont):
    %7: nat = const 0n
    continue %14, %7
  b2(%14: cont):
    %6: nat = const 7n
    continue %14, %6
  b3(%5: nat, %14: cont):
    branch_prim %5, 0n => b2(%14), otherwise b1(%14)
  b4(%3: sum, %14: cont):
    %5: nat = payload %3
    jump b3(%5, %14)
  b5(%3: sum, %14: cont):
    branch_tag %3, #G => b4(%3, %14), otherwise b0(%14)
  b6(%3: sum, %14: cont):
    %4: nat = payload %3
    continue %14, %4
  b7(%3: sum, %14: cont):
    branch_tag %3, #F => b6(%3, %14), otherwise b5(%3, %14)
  b8(%14: cont):
    %3: sum = global w
    jump b7(%3, %14)"#
    );
}

/// A bundle is built out of every effect the scope can handle, and each is in
/// it once however many frames between here and the handler carry it.
#[test]
fn a_bundle_names_an_effect_once_however_many_frames_hold_it() {
    let source = "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let idn = fn n => n\n\
         let outer : Nat -> Nat + !Log = fn n =>\n\
           do let inner : Nat -> Nat + !Log = fn q => piped idn q return inner n end";
    // Both `outer` and the `inner` lifted out of it take `Log`'s record, and
    // the bundle built inside `inner` names the effect once — as the record
    // `inner` itself was handed, which is the innermost one.
    assert!(
        section(source, "fn outer(").starts_with("fn outer(%15: struct, %16: nat;"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn outer#3"),
        r#"fn outer#3(%17: struct, %18: nat; %41: cont) entry b0 [f8, MaySuspend]:
  b0(%17: struct, %18: nat, %41: cont):
    %19: fn = global idn
    %24: fn = closure outer#2, [%19]
    %25: struct = struct { Log: %17 }
    call piped, %24, %25, %18 -> %41 [f0]"#
    );
}

/// A row whose variable part is a label's own presence rather than a rest has
/// no identity to share: the bundle standing for it is nobody else's, so a call
/// out of it builds its own rather than forwarding the one in hand.
///
/// Building its own still means handing over everything the scope holds, and
/// what this scope holds is that very bundle — `maybe` names no effect
/// outright, so there is nothing to lay over it and nothing to copy it into.
/// The evidence goes across; the identity does not.
#[test]
fn a_bundle_with_no_identity_hands_over_what_it_holds() {
    let source = "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let idn = fn n => n\n\
         let maybe : Nat -> Nat + !Log (when 'a) = fn n => piped idn n";
    assert_eq!(
        section(source, "fn maybe("),
        r#"fn maybe(%15: struct, %16: nat; %33: cont) entry b0 [f5, MaySuspend]:
  b0(%15: struct, %16: nat, %33: cont):
    %17: fn = global idn
    %22: fn = closure maybe#2, [%17]
    call piped, %22, %15, %16 -> %33 [f0]"#
    );
}

/// Whether a bundle may be forwarded is read off the row at the *use*, not the
/// row the callee was declared with.
///
/// A row variable's index is its place in the scheme that quantified it, so two
/// unrelated definitions both quantify a first one and their indices compare
/// equal. `caller` quantifies a rest of its own and calls `piped`, which
/// quantifies one too — but at that use `piped`'s rest stands for the `Log` the
/// handler on the same line discharges, so the bundle has to be built there:
/// the handler's record laid over the bundle `caller` was handed, since the
/// callee may reach for either. `twice` calls `piped` at its own rest, and that
/// one really is forwarded — nothing is built for it at all.
#[test]
fn a_bundle_is_forwarded_only_where_the_use_shares_the_variable() {
    let source = "effect Log = { write: Nat -> () }\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let noisy : Nat -> Nat + !Log = fn n => do let z = !Log.write n return n end\n\
         let caller : (Nat -> Nat + ..'r) -> Nat -> Nat + ..'r =\n\
           fn k => fn n => handle piped noisy n with | !Log.write s => {} end\n\
         let twice : (Nat -> Nat + ..'f) -> Nat -> Nat + ..'f =\n\
           fn g => fn n => piped g n";
    assert_eq!(
        section(source, "fn caller("),
        r#"fn caller(%19: fn, %20: struct, %21: nat; %62: cont) entry b2 [f5, MaySuspend]:
  b0(%22: handler, %63: any) continuation:
    leave %22, %63
  b1(%20: struct, %21: nat, %22: handler, %26: struct):
    %27: fn = global noisy
    %33: fn = closure caller#4, [%27]
    %34: struct = struct { Log: %26 }
    %35: struct = merge %20, %34
    %64: cont = continuation caller:b0, [%22] [f5]
    call piped, %33, %35, %21 -> %64 [f0]
  b2(%20: struct, %21: nat, %62: cont):
    %22: handler = new_tag
    %25: fn = closure caller#3, []
    %26: struct = struct { write: %25 }
    enter %22, b1(%20, %21, %22, %26), %62"#
    );
    assert_eq!(
        section(source, "fn twice("),
        r#"fn twice(%45: fn, %46: struct, %47: nat; %67: cont) entry b0 [f8, MaySuspend]:
  b0(%45: fn, %46: struct, %47: nat, %67: cont):
    call piped, %45, %46, %47 -> %67 [f0]"#
    );
}

/// Everything after a `raise` is unreachable, and none of it is emitted —
/// neither the instructions a later expression would have assigned nor the
/// terminator a second `raise` would have ended the block with.
#[test]
fn nothing_after_a_raise_is_emitted() {
    assert_eq!(
        section(
            "effect Fail = { oops: () -> Nat }\n\
             let recover = fn w =>\n\
               handle !Fail.oops () with\n\
               | !Fail.oops z => do let q = raise 0n return raise 1n end\n\
               | return r => r end",
            "fn recover#2"
        ),
        r#"fn recover#2(%4: any, %2: unit; %19: cont) entry b0 [f2, Synchronous]:
  b0(%4: any):
    %3: nat = const 0n
    abort %4, %3"#
    );
}

/// A callee written where it is called is nobody's top-level definition, so
/// there is no arity to read off it: the closure is built and then called
/// indirectly, one argument at a time.
#[test]
fn a_callee_written_inline_is_called_indirectly() {
    assert_eq!(
        section("let main = (fn n => n) 1n", "global main"),
        r#"fn main#init(; %5: cont) entry b0 [f1, Synchronous]:
  b0(%5: cont):
    %1: fn = closure main#1, []
    %2: nat = const 1n
    call %1, %2 -> %5"#
    );
}

/// A sum position no arm looks into is not dispatched on: the one arm accepts
/// every case, so there is nothing to tell apart and no `branch_tag` to write.
#[test]
fn a_sum_no_arm_tests_is_not_dispatched_on() {
    assert_eq!(
        section("let f = fn w => match #A with | y => 0n end", "fn f("),
        r#"fn f(%0: any; %6: cont) entry b0 [f0, Synchronous]:
  b0(%6: cont):
    %1: sum = tag #A
    %2: nat = const 0n
    continue %6, %2"#
    );
}

/// One number written in two arms is one case of the dispatch, not two: the
/// second arm joins the case the first opened rather than adding one that
/// could never be reached.
#[test]
fn a_number_written_twice_in_one_column_is_one_case() {
    let printed = section(
        "let f = fn s => match s with | {a: 0n, b: 0n} => 1n | {a: 0n, b: 1n} => 2n | _ => 3n end",
        "fn f(",
    );
    assert_eq!(printed.matches("branch_prim %1,").count(), 1, "{printed}");
    assert_eq!(
        printed
            .lines()
            .filter(|line| line.contains(", 0n =>"))
            .count(),
        2,
        "{printed}"
    );
}

/// A case the solved type proves absent is not a case left over: a dispatch
/// listing every case a value may be needs no `else`, even where the arms
/// list fewer labels than the row does.
#[test]
fn a_case_the_type_proves_absent_leaves_nothing_over() {
    assert_eq!(
        section(
            "type T 'r = #A Nat | \\#B | ..'r\n\
             let f : T (#C Nat) -> Nat = fn v => match v with | #A n => n | #C n => n end",
            "fn f("
        ),
        r#"fn f(%0: sum; %7: cont) entry b5 [f0, Synchronous]:
  b0():
    unreachable
  b1(%0: sum, %7: cont):
    %2: nat = payload %0
    continue %7, %2
  b2(%0: sum, %7: cont):
    branch_tag %0, #C => b1(%0, %7), otherwise b0()
  b3(%0: sum, %7: cont):
    %1: nat = payload %0
    continue %7, %1
  b4(%0: sum, %7: cont):
    branch_tag %0, #A => b3(%0, %7), otherwise b2(%0, %7)
  b5(%0: sum, %7: cont):
    jump b4(%0, %7)"#
    );
}

/// Lowering runs on a program every earlier phase accepted, and says so rather
/// than lowering one that is not: a name that did not resolve is a term with no
/// value to compute.
/// An indirect call whose callee gives back a function hands the result on at
/// the shape the callee's own next level declares — here the two agree, so the
/// second call takes the first's temp as it stands, with nothing between.
#[test]
fn a_function_an_indirect_call_returns_is_called_in_turn() {
    assert_eq!(
        section("let main = (fn a => fn b => a) 1n 2n", "global main"),
        r#"fn main#init(; %11: cont) entry b1 [f2, MaySuspend]:
  b0(%11: cont, %6: fn) continuation:
    %7: nat = const 2n
    call %6, %7 -> %11
  b1(%11: cont):
    %4: fn = closure main#2, []
    %5: nat = const 1n
    %12: cont = continuation main#init:b0, [%11] [f2]
    call %4, %5 -> %12"#
    );
}

#[test]
fn extern_values_are_imported_once_and_converted_by_global_initializers() {
    let (output, labels) =
        lowered_labelled("extern answer : Nat = \"host\\n.answer\"\nlet next = answer");
    assert_eq!(output.externs.len(), 1);
    let external = &output.externs[0];
    assert_eq!(external.name, "answer");
    assert_eq!(external.target, "host\n.answer");
    assert_eq!((external.span.start, external.span.width), (22, 15));
    assert_eq!(external.rep, lir::Rep::Nat);
    assert_eq!(
        output.globals.len(),
        2,
        "native imports have a conversion initializer"
    );
    assert_eq!(output.globals[0].name, "answer");
    assert_eq!(output.globals[1].name, "next");
    let printed = print::lir::program(&output, &labels);
    assert!(
        printed.contains("extern answer: nat = \"host\\n.answer\""),
        "the decoded target is rendered quoted and escaped:\n{printed}"
    );
    assert!(
        printed.contains("global answer"),
        "the use reads the import:\n{printed}"
    );
}

#[test]
fn recursive_ordinary_extern_adapters_close_cycles_in_both_directions() {
    let source = "type Loop = () -> Loop\n\
         extern loop : Loop = \"host.loop\"\n\
         extern install : fn(Loop) -> () = \"host.install\"";
    let printed = listing(source);
    let adapters: Vec<_> = printed
        .lines()
        .filter(|line| line.starts_with("fn ") && line.contains("#extern#"))
        .collect();
    assert_eq!(
        adapters.len(),
        3,
        "one marked and two cyclic adapters:\n{printed}"
    );
    assert!(
        printed
            .lines()
            .filter(|line| line.contains("closure loop#extern#"))
            .any(|line| line.matches("loop#extern#").count() == 1),
        "the host-to-Ruddy result closes over its in-progress adapter:\n{printed}"
    );
    assert!(
        printed
            .lines()
            .filter(|line| line.contains("closure install#extern#callback#"))
            .any(|line| line.matches("install#extern#callback#").count() == 1),
        "the Ruddy-to-host result closes over its in-progress adapter:\n{printed}"
    );
}

#[test]
fn extern_callbacks_capture_only_the_evidence_their_result_spine_requires() {
    let source = "effect Needed = { get: () -> Nat }\n\
         effect Spare = { get: () -> Nat }\n\
         type Callback = () -> (() -> Nat + !Needed)\n\
         extern install : fn(Callback) -> () + !Needed + !Spare = \"host.install\"";
    let printed = listing(source);
    let callback = printed
        .lines()
        .find(|line| line.starts_with("fn install#extern#callback#"))
        .expect("the callback adapter is emitted");
    assert_eq!(
        callback.matches(": struct").count(),
        1,
        "only Needed crosses into the callback adapter:\n{printed}"
    );
    assert!(
        printed
            .lines()
            .filter(|line| line.contains("closure install#extern#callback#"))
            .any(|line| line.matches('%').count() == 3),
        "the returned callback retains its closure and Needed evidence:\n{printed}"
    );
}

#[test]
fn callback_evidence_joins_conditional_then_definite_occurrences() {
    let source = "effect Needed = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> (() -> () + !Needed) + !Needed (when 'needed)) -> () + !Needed = \"host.install\"";
    let printed = listing(source);
    let callback = printed
        .lines()
        .find(|line| line.starts_with("fn install#extern#callback#"))
        .expect("the callback adapter is emitted");
    assert_eq!(
        callback.matches(": struct").count(),
        1,
        "the definite occurrence promotes the repeated requirement:\n{printed}"
    );
    let first_adapter = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn install#extern#callback#1"))
        .expect("the first callback adapter is printed");
    assert!(
        first_adapter.find("struct { Needed:") < first_adapter.find("call "),
        "the promoted evidence is repacked for the outer conditional arrow before its call:\n{printed}"
    );
}

#[test]
fn callback_evidence_joins_definite_then_conditional_occurrences() {
    let source = "effect Needed = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> (() -> () + !Needed (when 'needed)) + !Needed) -> () + !Needed = \"host.install\"";
    let printed = listing(source);
    let callback = printed
        .lines()
        .find(|line| line.starts_with("fn install#extern#callback#"))
        .expect("the callback adapter is emitted");
    assert_eq!(
        callback.matches(": struct").count(),
        1,
        "the definite occurrence stays dominant:\n{printed}"
    );
    let first_adapter = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn install#extern#callback#1"))
        .expect("the first callback adapter is printed");
    assert!(
        first_adapter.contains("call %4, %5, %8")
            && first_adapter.contains("struct { Needed: %5 }"),
        "the first definite occurrence remains direct evidence at its call:\n{printed}"
    );
}

#[test]
fn conditional_callback_bundles_capture_only_their_named_possibilities() {
    let source = "effect Needed = { get: () -> Nat }\n\
         effect Spare = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> () + !Needed (when 'needed)) -> () + !Needed + !Spare + ..'effects = \"host.install\"";
    let printed = listing(source);
    let bundles: Vec<_> = printed
        .lines()
        .filter(|line| line.contains(" = struct {") && line.contains("Needed:"))
        .collect();
    assert!(
        !bundles.is_empty(),
        "the conditional bundle is built:\n{printed}"
    );
    assert!(
        bundles.iter().all(|line| !line.contains("Spare:")),
        "unrelated definite evidence was captured in a conditional bundle:\n{printed}"
    );
}

#[test]
fn conditional_named_evidence_overlays_the_same_shared_open_tail() {
    let source = "effect Needed = { get: () -> Nat }\n\
         effect Spare = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> () + !Needed (when 'needed) + ..'effects) -> () + !Needed + ..'effects = \"host.install\"\n\
         let call : () -> () + !Spare = fn _ => handle install (fn _ => do let n = !Needed.get () return {} end) with | !Needed.get _ => 0n end";
    let printed = listing(source);
    let marked = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn install#extern#0"))
        .expect("the marked extern adapter is emitted");
    assert!(
        marked.contains("%5: struct = struct { Needed: %2 }")
            && marked.contains("%6: struct = merge %3, %5")
            && marked.contains("closure install#extern#callback#1, [%4, %6]"),
        "Needed must overlay the instantiated Spare tail captured by the callback:\n{printed}"
    );
    assert!(
        !marked.contains("Spare:"),
        "Spare remains an opaque shared tail instead of becoming named overlay evidence:\n{printed}"
    );
}

#[test]
fn restricted_callback_bundles_project_shared_open_tails() {
    let source = "effect Needed = { get: () -> Nat }\n\
         effect Spare = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> () + !Needed (when 'needed)) -> () + !Needed (when 'needed) + ..'effects = \"host.install\"\n\
         let pass : (() -> () + !Needed (when 'needed)) -> () + !Needed (when 'needed) + ..'effects =
           fn callback => install callback";
    let printed = listing(source);
    let marked = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn install#extern#0"))
        .expect("the marked extern adapter is emitted");
    assert!(
        marked.contains("%4: struct = project %2, \"Needed\"")
            && marked.contains("%5: struct = struct { Needed: %4 }")
            && marked.contains("closure install#extern#callback#1, [%3, %5]"),
        "the callback must capture exactly its value and projected Needed record, not ambient %2:\n{printed}"
    );
    assert!(
        !marked.contains("Spare:") && !marked.contains(" = merge "),
        "opaque ambient tail contents must not be named or merged into the capture:\n{printed}"
    );
}

#[test]
fn restricted_callback_bundles_drop_unrelated_conditional_tail_effects() {
    let source = "effect Needed = { get: () -> Nat }\n\
         effect Spare = { get: () -> Nat }\n\
         extern install : fn(fn(()) -> () + !Needed (when 'needed)) -> () + !Needed (when 'needed) + ..'effects = \"host.install\"\n\
         let pass : (() -> () + !Needed (when 'needed)) -> () + !Needed (when 'needed) + !Spare (when 'spare) + ..'effects =
           fn callback => install callback";
    let printed = listing(source);
    let projected: Vec<_> = printed
        .lines()
        .filter(|line| line.contains("project ") && line.contains("Needed"))
        .collect();
    assert!(!projected.is_empty(), "Needed is projected:\n{printed}");
    let callback = printed
        .split("\n\n")
        .find(|part| part.starts_with("fn install#extern#callback#"))
        .expect("the callback adapter is emitted");
    assert!(callback.contains("struct { Needed:"), "{printed}");
    assert!(
        !callback.contains("Spare"),
        "the unrelated conditional effect is absent from the restricted callback bundle:\n{printed}"
    );
}

#[test]
fn struct_and_project_instructions_preserve_quoted_field_names() {
    let source = r###"let pick = fn ignored => do let record = { "field name": 1n, "let": 2n, "line\n\"quote\"\\tail": 3n } return record."line\n\"quote\"\\tail" end"###;
    let printed = section(source, "fn pick(");
    assert!(
        printed.contains(r###"struct { "field name": %"###),
        "{printed}"
    );
    assert!(printed.contains(r###", "let": %"###), "{printed}");
    assert!(
        printed.contains(r###", "line\n\"quote\"\\tail": %"###),
        "{printed}"
    );
    assert!(
        printed.contains(r###"project %4, "line\n\"quote\"\\tail""###),
        "{printed}"
    );
}

#[test]
fn conditionals_preserve_quoted_fields_alongside_effect_evidence() {
    let source = r###"effect Log = { write: Nat -> () }
let choose = fn p =>
  handle !Log.write ((if p then { "if": 1n } else { "if": 2n } end)."if") with
  | !Log.write n => ()
  | return n => ()
  end"###;
    let printed = section(source, "fn choose(");
    assert!(printed.contains("branch_prim"), "{printed}");
    assert!(printed.contains(r###"struct { "if": %"###), "{printed}");
    assert!(printed.contains(r###"project %"###), "{printed}");
    assert!(printed.contains(r###", "if""###), "{printed}");
    assert!(printed.contains(r###"struct { write: %"###), "{printed}");
}

#[test]
fn tag_and_switch_instructions_preserve_quoted_variant_names() {
    let source = r###"let tagged = #"some case" 1n
let choose : (#"some case" Nat | #"let" | #"line\n\"quote\"\\tail") -> Nat = fn value => match value with | #"some case" n => n | #"let" => 0n | #"line\n\"quote\"\\tail" => 1n end"###;
    let tagged = section(source, "global tagged");
    assert!(tagged.contains(r###"tag #"some case","###), "{tagged}");

    let switched = section(source, "fn choose(");
    assert!(switched.contains("branch_tag"), "{switched}");
    for case in [
        r###"#"some case" =>"###,
        r###"#let =>"###,
        r###"#"line\n\"quote\"\\tail" =>"###,
    ] {
        assert!(
            switched.contains(case),
            "missing {case:?} from:\n{switched}"
        );
    }
}

fn execute_effect_layout(source: &str, expression: &str, assertions: &str) {
    lowered(source);
    let mut program = source
        .lines()
        .map(|line| {
            if line.starts_with("let ") || line.starts_with("extern ") {
                format!("@private {line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    program.push_str(&format!("\nlet result = {expression}\n"));
    crate::js::execute_reification(&program, assertions);
}
