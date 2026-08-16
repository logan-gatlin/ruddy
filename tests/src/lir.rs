//! Tests for [`ruddy::lir`].

use ruddy::{
    inference, ir, lir, parse, patterns,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileManager,
};
use ruddy_debug::print;

/// One source, taken all the way to LIR. Every earlier phase has to be silent:
/// lowering runs on accepted programs alone, so a source that does not type is
/// a test about nothing.
fn lowered(source: &str) -> lir::Output {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    let bundle = Bundle::new("tests", Version::new(0, 1, 0)).expect("the bundle name is valid");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);
    let inferred = inference::infer(&mint, &mut built.program);
    assert!(
        inferred.errors.is_empty(),
        "{source}: {:#?}",
        inferred.errors
    );
    let checked = patterns::check(&built.program, &inferred);
    assert!(checked.errors.is_empty(), "{source}: {:#?}", checked.errors);

    lir::lower(&mint, &built.program, &inferred)
}

/// The canonical listing of one source, which is what most of these tests read:
/// the printed form carries the temps, the representations and the nesting all
/// at once, and it is the form the spec pins.
fn listing(source: &str) -> String {
    print::lir::program(&lowered(source))
}

/// One section of a listing, by its header line.
fn section(source: &str, header: &str) -> String {
    let whole = listing(source);
    whole
        .split("\n\n")
        .find(|part| part.starts_with(header))
        .unwrap_or_else(|| panic!("`{header}` is a section of:\n{whole}"))
        .trim_end()
        .to_string()
}

/// Every nested expression becomes one temp assignment, in the order the source
/// evaluates it: the value of a `let` before its body, a struct's fields as
/// written, a projection's base before the read.
#[test]
fn a_nested_expression_flattens_in_source_order() {
    let printed = section(
        "let f = let p : { a: Nat } = { a: 1 } in #Some { b: p.a, c: 2 }",
        "global f",
    );
    assert_eq!(
        printed,
        "global f:\n\
         \x20 %0: nat = const 1\n\
         \x20 %1: struct = struct { a: %0 }\n\
         \x20 %2: nat = project %1, \"a\"\n\
         \x20 %3: nat = const 2\n\
         \x20 %4: struct = struct { b: %2, c: %3 }\n\
         \x20 %5: sum = tag #Some, %4\n\
         \x20 ret %5"
    );
}

/// A binding is a name for a temp and nothing else. Nothing is emitted for the
/// `let` itself, and the annotation beside it has nothing to say to a backend —
/// so a body that only names what was bound emits no instruction at all.
#[test]
fn a_binding_emits_no_instruction_of_its_own() {
    assert_eq!(
        section("let f = let p : Nat = 1 in p", "global f"),
        "global f:\n  %0: nat = const 1\n  ret %0"
    );
}

/// A name a binding shadows goes back to standing for what it did.
#[test]
fn a_shadowed_name_is_put_back_afterwards() {
    assert_eq!(
        section(
            "let f = let p = 1 in { one: let p = 2 in p, two: p }",
            "global f"
        ),
        "global f:\n\
         \x20 %0: nat = const 1\n\
         \x20 %1: nat = const 2\n\
         \x20 %2: struct = struct { one: %1, two: %0 }\n\
         \x20 ret %2"
    );
}

/// One case per representation. `nat`, `sum` and `fn` come off the core; the
/// empty struct is `unit` and a struct with fields is `struct`; and anything a
/// scheme quantified is `any`, since monomorphization is deferred.
#[test]
fn every_representation_comes_off_the_solved_type() {
    let source = "let n = 1\nlet u = {}\nlet s = { x: 1 }\nlet c = #A\nlet i = fn x => x";
    assert!(section(source, "global n").contains("%0: nat = const 1"));
    assert!(section(source, "global u").contains(": unit = struct {}"));
    assert!(section(source, "global s").contains(": struct = struct { x:"));
    assert!(section(source, "global c").contains(": sum = tag #A"));
    assert!(section(source, "global i").contains(": fn = closure i#1"));
    // The polymorphic identity's argument is held as anything at all.
    assert!(section(source, "fn i(").starts_with("fn i(%5: any):"));
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
    assert!(section(source, "fn two#3").starts_with("fn two#3(%3: any, %4: any, %2: any):"));
}

/// A lambda that closes over nothing carries no captures.
#[test]
fn a_lambda_that_closes_over_nothing_captures_nothing() {
    let source = "let k = fn w => { f: fn x => x }";
    assert!(section(source, "fn k(").contains("closure k#2, []"));
    assert!(section(source, "fn k#2").starts_with("fn k#2(%1: any):"));
}

/// A top-level definition is not a free variable: naming one is a `global` read
/// inside the function that names it, so nothing is captured for it.
#[test]
fn a_top_level_name_is_read_rather_than_captured() {
    let source = "let c = 1\nlet r = fn w => { f: fn x => c }";
    assert_eq!(
        section(source, "fn r#2"),
        "fn r#2(%2: any):\n  %3: nat = global c\n  ret %3"
    );
    assert!(section(source, "fn r(").contains("closure r#2, []"));
}

/// Applying a known 2-ary function to two arguments is one direct call of the
/// uncurried function, not two unary calls.
#[test]
fn a_full_application_is_one_direct_call() {
    let source = "let add = fn a => fn b => a\nlet main = add 1 2";
    assert_eq!(
        section(source, "global main"),
        "global main:\n\
         \x20 %8: nat = const 1\n\
         \x20 %9: nat = const 2\n\
         \x20 %10: nat = call add, %8, %9\n\
         \x20 ret %10"
    );
    assert_eq!(
        section(source, "fn add("),
        "fn add(%0: any, %1: any):\n  ret %0"
    );
}

/// Short of a full application, the arguments so far become the captures of the
/// wrapper that takes the next one.
#[test]
fn a_partial_application_is_a_wrapper_closure() {
    let source = "let two = fn a => fn b => a\nlet part = two 1";
    assert_eq!(
        section(source, "global part"),
        "global part:\n  %8: nat = const 1\n  %9: fn = closure two#2, [%8]\n  ret %9"
    );
    // The wrapper it names takes what was captured and then the argument left,
    // and hands the lot to the uncurried function.
    assert_eq!(
        section(source, "fn two#2"),
        "fn two#2(%4: any, %5: any):\n  %6: any = call two, %4, %5\n  ret %6"
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
        "global bare:\n  %8: fn = global two\n  ret %8"
    );
    assert_eq!(
        section(source, "global two"),
        "global two:\n  %7: fn = closure two#1, []\n  ret %7"
    );
    assert_eq!(
        section(source, "fn two#1"),
        "fn two#1(%2: any):\n  %3: fn = closure two#2, [%2]\n  ret %3"
    );
}

/// More arguments than the function has levels: the first n go into the direct
/// call, and what is left is applied to the result one at a time.
#[test]
fn an_over_application_calls_direct_and_then_indirectly() {
    let source = "let id = fn x => x\nlet two = fn a => fn b => a\nlet over = two id 1 2";
    assert_eq!(
        section(source, "global over"),
        "global over:\n\
         \x20 %12: fn = global id\n\
         \x20 %13: nat = const 1\n\
         \x20 %14: fn = call two, %12, %13\n\
         \x20 %15: nat = const 2\n\
         \x20 %16: nat = call %14, %15\n\
         \x20 ret %16"
    );
}

/// A callee that is not a known top-level definition stays one unary call per
/// argument: nothing here knows its arity, and working one out is deferred.
#[test]
fn an_unknown_callee_stays_unary() {
    let source = "let go = fn f => fn x => f x";
    assert_eq!(
        section(source, "fn go("),
        "fn go(%0: fn, %1: struct, %2: any):\n  %3: any = call %0, %1, %2\n  ret %3"
    );
}

/// Globals come out in group order — earliest group first, source order within
/// one — which is the order a backend initializes them in.
#[test]
fn globals_are_emitted_in_group_order() {
    let output = lowered("let a = 1\nlet b = a\nlet c = 2");
    let names: Vec<&str> = output
        .globals
        .iter()
        .map(|global| global.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert!(output.functions.is_empty());
}

/// Two functions in one group call each other directly: the slot a function's
/// name stands for exists before either body is built, so a call can be written
/// before the callee is.
#[test]
fn mutual_recursion_calls_direct() {
    let source = "let ping = fn n => pong n\nlet pong = fn n => ping n";
    assert_eq!(
        section(source, "fn ping("),
        "fn ping(%0: any):\n  %1: any = call pong, %0\n  ret %1"
    );
    assert_eq!(
        section(source, "fn pong("),
        "fn pong(%5: any):\n  %6: any = call ping, %5\n  ret %6"
    );
}

/// A plain value's global is the lowering of its body and nothing else.
#[test]
fn a_plain_value_global_is_its_own_initializer() {
    assert_eq!(
        section("let v = { x: 1 }", "global v"),
        "global v:\n  %0: nat = const 1\n  %1: struct = struct { x: %0 }\n  ret %1"
    );
}

/// The decision tree tests each discriminant once along any path: the tag once,
/// the payload read out once, and the numbers under it — with the `else` the
/// naturals always need, since they never run out.
#[test]
fn a_match_tests_each_position_once() {
    assert_eq!(
        section(
            "let pick = fn v => match v with | #Some 0 => 100 | #Some n => n | #None => 0 end",
            "fn pick("
        ),
        "fn pick(%0: sum):\n\
         \x20 %5: nat = switch_tag %0:\n\
         \x20   #Some =>\n\
         \x20     %1: nat = payload %0\n\
         \x20     %3: nat = switch_nat %1:\n\
         \x20       0 =>\n\
         \x20         %2: nat = const 100\n\
         \x20         yield %2\n\
         \x20       else =>\n\
         \x20         yield %1\n\
         \x20     yield %3\n\
         \x20   #None =>\n\
         \x20     %4: nat = const 0\n\
         \x20     yield %4\n\
         \x20 ret %5"
    );
}

/// A case no listed one covers needs somewhere 'to go: an arm that accepts
/// anything gives the dispatch its `else`. With every case of a closed row
/// listed there is nothing left over, and no `else` is written.
#[test]
fn a_tag_dispatch_has_an_else_only_where_something_is_left_over() {
    assert!(
        section(
            "let f = fn v => match v with | #A x => x | b => 0 end",
            "fn f("
        )
        .contains("else =>")
    );
    assert!(
        !section(
            "let f = fn v => match v with | #A x => x | #B => 0 end",
            "fn f("
        )
        .contains("else =>")
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
        "fn f(%0: struct):\n\
         \x20 %1: any = project %0, \"x\"\n\
         \x20 %3: any = switch_presence %0, \"y\":\n\
         \x20   present =>\n\
         \x20     %2: any = project %0, \"y\"\n\
         \x20     yield %2\n\
         \x20   absent =>\n\
         \x20     yield %1\n\
         \x20 ret %3"
    );
}

/// An exact pattern against a type that is open has to ask whether the value
/// carries anything beyond the fields the type names — which is the one thing
/// separating it from the open pattern beside it.
#[test]
fn an_exact_pattern_over_an_open_type_tests_the_rest() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x} => 1 | {x, ..} => 2 end",
            "fn f("
        ),
        "fn f(%0: any):\n\
         \x20 %1: any = project %0, \"x\"\n\
         \x20 %4: nat = switch_rest %0, [\"x\"]:\n\
         \x20   none =>\n\
         \x20     %2: nat = const 1\n\
         \x20     yield %2\n\
         \x20   some =>\n\
         \x20     %3: nat = const 2\n\
         \x20     yield %3\n\
         \x20 ret %4"
    );
}

/// A match every arm of which accepts everything tests nothing at all: the
/// first arm wins, and there is no dispatch to write.
#[test]
fn a_wildcard_match_tests_nothing() {
    assert_eq!(
        section("let w = fn v => match v with | _ => 1 end", "fn w("),
        "fn w(%0: any):\n  %1: nat = const 1\n  ret %1"
    );
}

/// A binder arm binds the temp the position it sits at already holds, rather
/// than reading the value out again.
#[test]
fn a_binder_arm_binds_the_scrutinee_temp() {
    assert_eq!(
        section(
            "let f = fn n => match n with | 0 => 1 | m => m end",
            "fn f("
        ),
        "fn f(%0: nat):\n\
         \x20 %2: nat = switch_nat %0:\n\
         \x20   0 =>\n\
         \x20     %1: nat = const 1\n\
         \x20     yield %1\n\
         \x20   else =>\n\
         \x20     yield %0\n\
         \x20 ret %2"
    );
}

/// A match with no arms constrained its scrutinee to the empty sum: a dispatch
/// with no case to take, and nothing left over for it to fall through to.
#[test]
fn a_match_with_no_arms_dispatches_over_nothing() {
    assert_eq!(
        section("let e = fn v => match v with end", "fn e("),
        "fn e(%0: sum):\n  %1: any = switch_tag %0:\n  ret %1"
    );
}

/// Performing an operation is an ordinary call: the function's own hidden
/// evidence parameter holds the record the handler passed down, the operation is
/// read out of it, and what comes back is what the arm answered.
#[test]
fn performing_an_operation_reads_it_out_of_the_evidence() {
    assert_eq!(
        section(
            "effect Log = write : Nat -> ()\nlet shout = fn x => !Log.write x",
            "fn shout("
        ),
        "fn shout(%0: struct, %1: nat):\n\
         \x20 %2: fn = project %0, \"write\"\n\
         \x20 %3: unit = call %2, %1\n\
         \x20 ret %3"
    );
}

/// A handler mints an identity, builds one record of operation closures per
/// effect it discharges, and runs its body under a `catch` on that identity —
/// with the direct call out of the body handed the record.
#[test]
fn a_handler_builds_evidence_and_catches_its_own_tag() {
    assert_eq!(
        section(
            "effect Log = write : Nat -> ()\n\
             let shout = fn x => !Log.write x\n\
             let main = fn w => handle shout 5 with | !Log.write n => {} end",
            "fn main("
        ),
        "fn main(%8: any):\n\
         \x20 %9: any = new_tag\n\
         \x20 %12: fn = closure main#2, []\n\
         \x20 %13: struct = struct { write: %12 }\n\
         \x20 %16: unit = catch %9:\n\
         \x20   %14: nat = const 5\n\
         \x20   %15: unit = call shout, %13, %14\n\
         \x20   yield %15\n\
         \x20 ret %16"
    );
}

/// The `return` arm is applied inline to the body's value on the normal path:
/// it binds that value, and the catch yields what it answers.
#[test]
fn a_return_arm_is_applied_on_the_normal_path() {
    let printed = section(
        "effect Log = write : Nat -> ()\n\
         let main = fn w => handle 1 with | !Log.write n => {} | return r => { got: r } end",
        "fn main(",
    );
    assert!(printed.contains("%6: nat = const 1"), "{printed}");
    assert!(printed.contains("struct { got: %6 }"), "{printed}");
}

/// A `raise` ends its arm with a throw to the identity the arm captured, and the
/// code after it is unreachable and is not emitted. The `catch` around the body
/// yields the thrown value directly, so the `return` arm never sees it.
#[test]
fn a_raise_throws_to_the_tag_its_arm_captured() {
    let source = "effect Fail = oops : () -> Nat\n\
         let recover = fn w =>\n\
           handle !Fail.oops () with | !Fail.oops z => raise 0 | return r => r end";
    assert_eq!(
        section(source, "fn recover#2"),
        "fn recover#2(%4: any, %2: unit):\n  %3: nat = const 0\n  throw %4, %3"
    );
    // The arm captures the very tag the `catch` beside it was minted with.
    let recover = section(source, "fn recover(");
    assert!(recover.contains("%1: any = new_tag"), "{recover}");
    assert!(recover.contains("closure recover#2, [%1]"), "{recover}");
    assert!(recover.contains("catch %1:"), "{recover}");
}

/// Two handlers of one effect, one inside the other: each mints its own
/// identity, and the perform inside the inner body reaches the inner record.
#[test]
fn nested_handlers_of_one_effect_shadow_and_stay_apart() {
    let printed = section(
        "effect Log = write : Nat -> ()\n\
         let nest = fn w =>\n\
           handle (handle !Log.write 1 with | !Log.write a => {} end)\n\
           with | !Log.write b => {} end",
        "fn nest(",
    );
    assert_eq!(printed.matches("new_tag").count(), 2, "{printed}");
    assert!(printed.contains("%1: any = new_tag"), "{printed}");
    assert!(printed.contains("%6: any = new_tag"), "{printed}");
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
        "fn piped(%0: fn, %1: struct, %2: nat):\n  %3: nat = call %0, %1, %2\n  ret %3"
    );
}

/// A call whose callee's variable part is not the caller's own gets a bundle
/// built at the site, holding evidence for every effect the scope can handle —
/// and the function handed over, which takes a record of its own rather than a
/// bundle, is wrapped in an adapter that reads its record back out.
#[test]
fn a_bundle_is_built_where_none_can_be_forwarded() {
    let source = "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => n\n\
         let use = fn w => handle piped logger 1 with | !Log.write s => {} end";
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
        "fn use#3(%25: fn, %26: struct, %27: nat):\n\
         \x20 %28: struct = project %26, \"Log\"\n\
         \x20 %29: nat = call %25, %28, %27\n\
         \x20 ret %29"
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
    let source = "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let use = fn w => handle piped logger 1 with | !Log.write s => {} end";
    // The record `logger` reads is its first parameter, and the wrapper that
    // stands for `logger` as a value passes its own first parameter straight on.
    assert!(
        section(source, "fn logger(").contains("%13: fn = project %11, \"write\""),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn logger#1"),
        "fn logger#1(%15: struct, %16: nat):\n  %17: nat = call logger, %15, %16\n  ret %17"
    );
    // The adapter reads that record out of the bundle `piped` was declared to
    // take, and the bundle the call built holds the handler's record under the
    // effect's name.
    assert_eq!(
        section(source, "fn use#3"),
        "fn use#3(%27: fn, %28: struct, %29: nat):\n\
         \x20 %30: struct = project %28, \"Log\"\n\
         \x20 %31: nat = call %27, %30, %29\n\
         \x20 ret %31"
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
    let source = "effect Log = write : Nat -> ()\n\
         let op = !Log.write\n\
         let go = fn w => handle op 1 with | !Log.write n => {} end";
    assert_eq!(
        section(source, "fn op#1"),
        "fn op#1(%0: struct, %1: nat):\n\
         \x20 %2: fn = project %0, \"write\"\n\
         \x20 %3: unit = call %2, %1\n\
         \x20 ret %3"
    );
    assert_eq!(
        section(source, "global op"),
        "global op:\n  %4: fn = closure op#1, []\n  ret %4"
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
            "effect Log = write : Nat -> ()\n\
             let both = fn g => let z = !Log.write 1 in g 2",
            "fn both("
        ),
        "fn both(%0: struct, %1: struct, %2: fn):\n\
         \x20 %3: fn = project %0, \"write\"\n\
         \x20 %4: nat = const 1\n\
         \x20 %5: unit = call %3, %4\n\
         \x20 %6: nat = const 2\n\
         \x20 %7: any = call %2, %0, %1, %6\n\
         \x20 ret %7"
    );
}

/// The whole listing of a small program, exactly: one section per function and
/// per global, a header line, one instruction per line as `%N: rep = op
/// operands`, and the terminator written bare. This is the format the tab and
/// every test above read, so it is pinned in one place.
#[test]
fn the_listing_is_the_canonical_format() {
    assert_eq!(
        listing("let add = fn a => fn b => a\nlet main = add 1 2"),
        "fn add(%0: any, %1: any):\n\
         \x20 ret %0\n\
         \n\
         fn add#1(%2: any):\n\
         \x20 %3: fn = closure add#2, [%2]\n\
         \x20 ret %3\n\
         \n\
         fn add#2(%4: any, %5: any):\n\
         \x20 %6: any = call add, %4, %5\n\
         \x20 ret %6\n\
         \n\
         global add:\n\
         \x20 %7: fn = closure add#1, []\n\
         \x20 ret %7\n\
         \n\
         global main:\n\
         \x20 %8: nat = const 1\n\
         \x20 %9: nat = const 2\n\
         \x20 %10: nat = call add, %8, %9\n\
         \x20 ret %10\n"
    );
}

/// A program with nothing in it lowers to nothing, rather than to a listing of
/// one empty thing.
#[test]
fn an_empty_program_lowers_to_an_empty_output() {
    let output = lowered("");
    assert!(output.globals.is_empty());
    assert!(output.functions.is_empty());
    assert_eq!(print::lir::program(&output), "");
}

/// A declared sum applied to more cases is one row by the time the dispatch is
/// built: the tail a use site handed the declaration is flattened into the
/// cases beside it, so both are ordinary entries of one `switch_tag`.
#[test]
fn a_declared_sums_cases_are_flattened_before_dispatch() {
    assert_eq!(
        section(
            "type Fallible r = #Err Nat | ..r\n\
             let h : Fallible (#Ok Nat) -> Nat =\n\
               fn t => match t with | #Err n => n | #Ok n => n end",
            "fn h("
        ),
        "fn h(%0: sum):\n\
         \x20 %3: nat = switch_tag %0:\n\
         \x20   #Err =>\n\
         \x20     %1: nat = payload %0\n\
         \x20     yield %1\n\
         \x20   #Ok =>\n\
         \x20     %2: nat = payload %0\n\
         \x20     yield %2\n\
         \x20 ret %3"
    );
}

/// `()` is the exact struct naming no fields, and over the unit type there is
/// nothing about it left to ask.
#[test]
fn a_unit_pattern_tests_nothing() {
    assert_eq!(
        section("let u = fn v => match v with | () => 1 end", "fn u("),
        "fn u(%0: unit):\n  %1: nat = const 1\n  ret %1"
    );
}

/// A capture is one parameter however often the body uses what it stands for.
#[test]
fn a_value_captured_twice_is_one_parameter() {
    let source = "let d = fn y => { g: fn x => { p: y, q: y } }";
    assert!(section(source, "fn d(").contains("closure d#2, [%0]"));
    assert_eq!(
        section(source, "fn d#2"),
        "fn d#2(%2: any, %1: any):\n  %3: struct = struct { p: %2, q: %2 }\n  ret %3"
    );
}

/// An open pattern says nothing about a field it does not mention, so it goes
/// down both halves of that field's test — while the exact pattern beside it
/// still has to ask what lies beyond the fields it names.
#[test]
fn an_open_pattern_says_nothing_about_a_field_it_omits() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x, ..} => 1 | {y} => 2 | _ => 3 end",
            "fn f("
        ),
        "fn f(%0: any):\n\
         \x20 %9: nat = switch_presence %0, \"x\":\n\
         \x20   present =>\n\
         \x20     %1: any = project %0, \"x\"\n\
         \x20     %2: nat = const 1\n\
         \x20     yield %2\n\
         \x20   absent =>\n\
         \x20     %8: nat = switch_presence %0, \"y\":\n\
         \x20       present =>\n\
         \x20         %3: any = project %0, \"y\"\n\
         \x20         %6: nat = switch_rest %0, [\"x\", \"y\"]:\n\
         \x20           none =>\n\
         \x20             %4: nat = const 2\n\
         \x20             yield %4\n\
         \x20           some =>\n\
         \x20             %5: nat = const 3\n\
         \x20             yield %5\n\
         \x20         yield %6\n\
         \x20       absent =>\n\
         \x20         %7: nat = const 3\n\
         \x20         yield %7\n\
         \x20     yield %8\n\
         \x20 ret %9"
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
        "fn f(%0: struct):\n  %1: nat = project %0, \"x\"\n  ret %1"
    );
}

/// A field whose presence decides the arm but whose value nothing looks at is
/// tested and not read — where a pun would have bound it, and so read it.
#[test]
fn a_field_every_arm_ignores_is_tested_but_not_read() {
    assert_eq!(
        section(
            "let f = fn s => match s with | {x: _, y: _} => 1 | {x: _} => 2 end",
            "fn f("
        ),
        "fn f(%0: struct):\n\
         \x20 %3: nat = switch_presence %0, \"y\":\n\
         \x20   present =>\n\
         \x20     %1: nat = const 1\n\
         \x20     yield %1\n\
         \x20   absent =>\n\
         \x20     %2: nat = const 2\n\
         \x20     yield %2\n\
         \x20 ret %3"
    );
}

/// An operation whose result is itself a function: the perform is the first
/// call, and what is left is applied to the result one argument at a time.
#[test]
fn an_operation_returning_a_function_keeps_applying() {
    let printed = section(
        "effect Mk = mk : Nat -> (Nat -> Nat)\n\
         let go = fn w => handle !Mk.mk 1 2 with | !Mk.mk n => fn z => z end",
        "fn go(",
    );
    assert!(printed.contains("%7: fn = project %6, \"mk\""), "{printed}");
    assert!(printed.contains("%9: fn = call %7, %8"), "{printed}");
    assert!(printed.contains("%11: nat = call %9, %10"), "{printed}");
}

/// A nested binding whose own effects reach no further than itself still gets
/// its bundle: the row it performs at is a variable, so the evidence for it has
/// to travel, and the lifted function takes it beside what it captured.
#[test]
fn a_nested_binding_takes_a_bundle_of_its_own() {
    assert_eq!(
        section("let outer = fn z => let g = fn h => h z in 0", "fn outer#2"),
        "fn outer#2(%3: any, %1: struct, %2: fn):\n  %4: any = call %2, %1, %3\n  ret %4"
    );
}

/// Every temp in a listing is defined exactly once, which is what lets the tab
/// light `%17` wherever it appears without asking which function it is in. The
/// curried wrappers are where that is easiest to lose: each one receives what
/// the one before it captured, and names it with a temp of its own rather than
/// borrowing the number it arrived under.
#[test]
fn every_temp_is_defined_once_in_the_whole_listing() {
    for source in [
        "let three = fn a => fn b => fn c => a\nlet p = three 1",
        "effect Log = write : Nat -> ()\n\
         let shout = fn x => fn y => !Log.write x\n\
         let go = fn w => handle shout 1 with | !Log.write n => {} end",
        "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let use = fn w => handle piped logger 1 with | !Log.write s => {} end",
    ] {
        let printed = listing(source);
        let defined = definitions(&printed);
        let mut once = defined.clone();
        once.sort();
        once.dedup();
        assert!(!defined.is_empty(), "{printed}");
        assert_eq!(once.len(), defined.len(), "{printed}");
    }
}

/// Every temp a listing *defines*: the parameters of each header line, and the
/// target of each instruction. A use is never at the head of a line, so what is
/// left out is exactly the uses.
fn definitions(printed: &str) -> Vec<String> {
    let mut defined = Vec::new();
    for line in printed.lines() {
        let line = line.trim_start();
        match line.strip_prefix("fn ") {
            Some(signature) => {
                let (_, params) = signature
                    .split_once('(')
                    .expect("a header line names its parameters");
                for param in params.trim_end_matches("):").split(", ") {
                    if let Some((temp, _)) = param.split_once(':') {
                        defined.push(temp.to_string());
                    }
                }
            }
            None => {
                if let Some((temp, _)) = line.split_once(':')
                    && temp.starts_with('%')
                {
                    defined.push(temp.to_string());
                }
            }
        }
    }
    defined
}

/// A value takes the evidence of the row it was written with, and the position
/// it is handed to passes the evidence of the row *that* was declared with. The
/// two agree here — both name `Log` and nothing else — so the function goes
/// across as it is, with no adapter between.
#[test]
fn evidence_of_the_same_shape_is_handed_straight_over() {
    let source = "effect Log = write : Nat -> ()\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1\n\
         let noisy = fn n => let z = !Log.write n in n\n\
         let same = fn w => handle takes noisy with | !Log.write s => {} end";
    let printed = section(source, "fn same(");
    assert!(printed.contains("%22: fn = global noisy"), "{printed}");
    assert!(
        printed.contains("%23: nat = call takes, %21, %22"),
        "{printed}"
    );
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
    let source = "let id = fn x => x\nlet idv = id\nlet go = fn f => (idv f) 1";
    assert_eq!(
        section(source, "fn go("),
        "fn go(%5: struct, %6: fn):\n\
         \x20 %7: fn = global idv\n\
         \x20 %8: fn = call %7, %6\n\
         \x20 %9: nat = const 1\n\
         \x20 %10: any = call %8, %5, %9\n\
         \x20 ret %10"
    );
}

/// A row that names `Log` and carries a variable part beside it is two pieces
/// of evidence, and a position declaring only `Log` passes one. The adapter
/// packs what it was given into the bundle the value expects, keyed by the
/// effect's name, and hands the record itself over unchanged.
#[test]
fn an_adapter_packs_a_record_into_the_bundle_a_value_expects() {
    let source = "effect Log = write : Nat -> ()\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1\n\
         let openish : Nat -> Nat + !Log + ..'r = fn n => let z = !Log.write n in n\n\
         let packed = fn w => handle takes openish with | !Log.write s => {} end";
    assert_eq!(
        section(source, "fn openish("),
        "fn openish(%8: struct, %9: struct, %10: nat):\n\
         \x20 %11: fn = project %8, \"write\"\n\
         \x20 %12: unit = call %11, %10\n\
         \x20 ret %10"
    );
    assert_eq!(
        section(source, "fn packed#3"),
        "fn packed#3(%25: fn, %26: struct, %27: nat):\n\
         \x20 %28: struct = struct { Log: %26 }\n\
         \x20 %29: nat = call %25, %26, %28, %27\n\
         \x20 ret %29"
    );
    let printed = section(source, "fn packed(");
    assert!(
        printed.contains("%30: fn = closure packed#3, [%24]"),
        "{printed}"
    );
    assert!(
        printed.contains("%31: nat = call takes, %23, %30"),
        "{printed}"
    );
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
    let source = "effect Log = write : Nat -> ()\n\
         let hof2 : ((Nat -> Nat + ..'r) -> Nat + ..'r) -> Nat + ..'r =\n\
           fn k => k (fn m => m)\n\
         let both : (Nat -> Nat + !Log + ..'s) -> Nat + !Log + ..'s =\n\
           fn g => let z = !Log.write 1 in g 2\n\
         let fwd : {} -> Nat + !Log + ..'t = fn w => hof2 both";
    assert!(
        section(source, "fn both(").starts_with("fn both(%10: struct, %11: struct, %12: fn):"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn fwd#3"),
        "fn fwd#3(%27: fn, %28: struct, %29: fn):\n\
         \x20 %30: struct = project %28, \"Log\"\n\
         \x20 %38: fn = closure fwd#2, [%29]\n\
         \x20 %39: nat = call %27, %30, %28, %38\n\
         \x20 ret %39"
    );
    assert_eq!(
        section(source, "fn fwd#2"),
        "fn fwd#2(%31: fn, %32: struct, %33: struct, %34: nat):\n\
         \x20 %35: struct = struct { Log: %32 }\n\
         \x20 %36: struct = merge %33, %35\n\
         \x20 %37: nat = call %31, %36, %34\n\
         \x20 ret %37"
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
    let source = "effect Log = write : Nat -> ()\n\
         let two : Nat -> Nat -> Nat + !Log + ..'s =\n\
           fn a => fn b => let z = !Log.write a in b\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1\n\
         let go = fn w => handle takes (two 1) with | !Log.write s => {} end";
    // The wrapper the partial application becomes takes a record and a bundle.
    assert!(
        section(source, "fn two#2")
            .starts_with("fn two#2(%8: nat, %9: struct, %10: struct, %11: nat):"),
        "{}",
        listing(source)
    );
    // `takes` passes one record and the argument, so the adapter's parameters
    // after its capture are exactly those two.
    assert_eq!(
        section(source, "fn go#3"),
        "fn go#3(%30: fn, %31: struct, %32: nat):\n\
         \x20 %33: struct = struct { Log: %31 }\n\
         \x20 %34: nat = call %30, %31, %33, %32\n\
         \x20 ret %34"
    );
    let go = section(source, "fn go(");
    assert!(go.contains("%29: fn = closure two#2, [%28]"), "{go}");
    assert!(go.contains("%35: fn = closure go#3, [%29]"), "{go}");
    assert!(go.contains("%36: nat = call takes, %27, %35"), "{go}");
}

/// A *full* application is a value like any other: the wrappers are behind it,
/// not in front, so what it stands for is the type it came to and nothing is
/// fitted. `pick 1 2` is already the function `takes` was declared to take, and
/// it goes straight across with no adapter between them.
#[test]
fn a_full_application_stands_for_what_it_came_to() {
    let source = "effect Log = write : Nat -> ()\n\
         let noisy : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let pick : Nat -> Nat -> (Nat -> Nat + !Log) = fn a => fn b => noisy\n\
         let takes : (Nat -> Nat + !Log) -> Nat + !Log = fn f => f 1\n\
         let go = fn w => handle takes (pick 1 2) with | !Log.write s => {} end";
    assert_eq!(
        section(source, "fn go("),
        "fn go(%25: any):\n\
         \x20 %26: any = new_tag\n\
         \x20 %29: fn = closure go#2, []\n\
         \x20 %30: struct = struct { write: %29 }\n\
         \x20 %35: nat = catch %26:\n\
         \x20   %31: nat = const 1\n\
         \x20   %32: nat = const 2\n\
         \x20   %33: fn = call pick, %31, %32\n\
         \x20   %34: nat = call takes, %30, %33\n\
         \x20   yield %34\n\
         \x20 ret %35"
    );
}

/// A definition is fitted where its value is *read*, not where it happens to be
/// passed, so a binding between the read and the call changes nothing: the temp
/// `h` names already holds the shape `h`'s own type promises, and handing it to
/// `run` is handing over a function `run` can call.
#[test]
fn a_definition_read_through_a_binding_arrives_callable() {
    let source = "effect A = a : Nat -> ()\n\
         effect B = b : Nat -> ()\n\
         let poly : Nat -> Nat + ..'e = fn n => n\n\
         let run : (Nat -> Nat + !A + !B) -> Nat + !A + !B = fn f => f 1\n\
         let go = fn w => handle handle\n\
           (let h : Nat -> Nat + !A + !B = poly in run h)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    // `poly` was compiled with one tail bundle before its argument, and `run`
    // calls what it is given with two records before it — so the adapter packs
    // the two into the bundle, keyed by effect name.
    assert!(
        section(source, "fn poly(").starts_with("fn poly(%0: struct, %1: nat):"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn go#4"),
        "fn go#4(%28: fn, %29: struct, %30: struct, %31: nat):\n\
         \x20 %32: struct = struct { A: %29, B: %30 }\n\
         \x20 %33: nat = call %28, %32, %31\n\
         \x20 ret %33"
    );
    // The adapter is built where the global is read, and it is the adapter that
    // travels through the binding into the call.
    let go = section(source, "fn go(");
    assert!(go.contains("%27: fn = global poly"), "{go}");
    assert!(go.contains("%34: fn = closure go#4, [%27]"), "{go}");
    assert!(go.contains("%35: nat = call run, %26, %21, %34"), "{go}");
    // Which is what makes the indirect call inside `run` reach a function of as
    // many parameters as it passes arguments.
    assert_eq!(
        section(source, "fn run("),
        "fn run(%6: struct, %7: struct, %8: fn):\n\
         \x20 %9: nat = const 1\n\
         \x20 %10: nat = call %8, %6, %7, %9\n\
         \x20 ret %10"
    );
}

/// The same, with a function return in place of the binding: `get` hands back
/// what `poly` stands for at `get`'s own declared type, so the fitting happens
/// inside `get` and the caller has nothing left to do.
#[test]
fn a_definition_returned_from_a_function_arrives_callable() {
    let source = "effect A = a : Nat -> ()\n\
         effect B = b : Nat -> ()\n\
         let poly : Nat -> Nat + ..'e = fn n => n\n\
         let get : {} -> (Nat -> Nat + !A + !B) = fn u => poly\n\
         let run : (Nat -> Nat + !A + !B) -> Nat + !A + !B = fn f => f 1\n\
         let go = fn w => handle handle run (get {})\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    assert_eq!(
        section(source, "fn get("),
        "fn get(%6: unit):\n\
         \x20 %7: fn = global poly\n\
         \x20 %14: fn = closure get#2, [%7]\n\
         \x20 ret %14"
    );
    assert_eq!(
        section(source, "fn get#2"),
        "fn get#2(%8: fn, %9: struct, %10: struct, %11: nat):\n\
         \x20 %12: struct = struct { A: %9, B: %10 }\n\
         \x20 %13: nat = call %8, %12, %11\n\
         \x20 ret %13"
    );
    assert_eq!(
        section(source, "fn run("),
        "fn run(%18: struct, %19: struct, %20: fn):\n\
         \x20 %21: nat = const 1\n\
         \x20 %22: nat = call %20, %18, %19, %21\n\
         \x20 ret %22"
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
    let source = "effect A = a : Nat -> ()\n\
         effect B = b : Nat -> ()\n\
         let both : Nat -> Nat + !A + !B = fn n => let x = !A.a n in let y = !B.b n in n\n\
         let app = fn g => g 1\n\
         let direct = fn w => handle handle app both\n\
           with | !A.a s => {} end with | !B.b s => {} end\n\
         let bound = fn w => handle handle (let h = app in h both)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    // The lifted `app` and its wrapper take a bundle and the argument: two
    // parameters, which is what every call of the value has to supply.
    assert!(
        section(source, "fn app#1").starts_with("fn app#1(%16: struct, %17: fn):"),
        "{}",
        listing(source)
    );
    // `direct` calls `app` directly: one bundle, one adapted argument.
    let direct = section(source, "fn direct(");
    assert!(
        direct.contains("%32: struct = struct { B: %25, A: %30 }"),
        "{direct}"
    );
    assert!(
        direct.contains("%39: fn = closure direct#4, [%31]"),
        "{direct}"
    );
    assert!(direct.contains("%40: nat = call app, %32, %39"), "{direct}");
    // `bound` calls the same value through `h`, and the call has the same
    // shape: the bundle keyed by effect name, the adapter over `global both`,
    // and two arguments handed to the temp the global read produced.
    let bound = section(source, "fn bound(");
    assert!(bound.contains("%57: fn = global app"), "{bound}");
    assert!(bound.contains("%58: fn = global both"), "{bound}");
    assert!(
        bound.contains("%59: struct = struct { B: %51, A: %56 }"),
        "{bound}"
    );
    assert!(
        bound.contains("%66: fn = closure bound#4, [%58]"),
        "{bound}"
    );
    assert!(bound.contains("%67: nat = call %57, %59, %66"), "{bound}");
    // The two adapters repack the same two records into the same bundle.
    assert_eq!(
        section(source, "fn bound#4"),
        "fn bound#4(%60: fn, %61: struct, %62: nat):\n\
         \x20 %63: struct = project %61, \"A\"\n\
         \x20 %64: struct = project %61, \"B\"\n\
         \x20 %65: nat = call %60, %63, %64, %62\n\
         \x20 ret %65"
    );
}

/// A lambda parameter applied at two different instantiations in one body is
/// called both times at the shape the parameter holds — the declared type it
/// arrived at — so both calls pass exactly as many arguments as the lifted
/// function has parameters, and the argument whose own shape differs gets an
/// adapter.
#[test]
fn a_parameter_is_called_at_one_shape_however_the_uses_instantiate_it() {
    let source = "effect Log = write : Nat -> ()\n\
         let noisy : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let pure = fn n => n\n\
         let go = fn w => handle (let h = fn g => g 1 in { a: h noisy, b: h pure })\n\
           with | !Log.write s => {} end";
    // The lifted `h` takes a bundle and its argument: two parameters.
    assert_eq!(
        section(source, "fn go#3"),
        "fn go#3(%18: struct, %19: fn):\n\
         \x20 %20: nat = const 1\n\
         \x20 %21: any = call %19, %18, %20\n\
         \x20 ret %21"
    );
    // Both calls of the closure pass two arguments — a bundle and the adapted
    // function — although one use reads `h` at `Log` and the other reads it
    // pure.
    let go = section(source, "fn go(");
    assert!(go.contains("%31: nat = call %22, %24, %30"), "{go}");
    assert!(go.contains("%39: nat = call %22, %33, %38"), "{go}");
    assert!(go.contains("%24: struct = struct { Log: %17 }"), "{go}");
    assert!(go.contains("%30: fn = closure go#4, [%23]"), "{go}");
    assert!(go.contains("%38: fn = closure go#5, [%32]"), "{go}");
    // `noisy`'s adapter reads its record back out of the bundle; `pure`'s
    // drops the bundle it has no use for.
    assert_eq!(
        section(source, "fn go#4"),
        "fn go#4(%25: fn, %26: struct, %27: nat):\n\
         \x20 %28: struct = project %26, \"Log\"\n\
         \x20 %29: nat = call %25, %28, %27\n\
         \x20 ret %29"
    );
    assert_eq!(
        section(source, "fn go#5"),
        "fn go#5(%34: fn, %35: struct, %36: nat):\n\
         \x20 %37: nat = call %34, %36\n\
         \x20 ret %37"
    );
}

/// An effect-polymorphic value returned from a call is called at the shape it
/// holds: what `pick {}` gives back is shaped by `pick`'s own last arrow, so
/// the call of the result builds the bundle keyed by effect name — not a raw
/// per-effect record — and adapts the argument, exactly as if `app` had been
/// named on the spot.
#[test]
fn a_value_returned_from_a_call_is_called_at_the_shape_it_holds() {
    let source = "effect A = a : Nat -> ()\n\
         effect B = b : Nat -> ()\n\
         let both : Nat -> Nat + !A + !B = fn n => let x = !A.a n in let y = !B.b n in n\n\
         let app = fn g => g 1\n\
         let pick = fn u => app\n\
         let late = fn w => handle handle (pick {}) both\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    let late = section(source, "fn late(");
    assert!(late.contains("%37: fn = call pick, %36"), "{late}");
    assert!(late.contains("%38: fn = global both"), "{late}");
    assert!(
        late.contains("%39: struct = struct { B: %30, A: %35 }"),
        "{late}"
    );
    assert!(late.contains("%46: fn = closure late#4, [%38]"), "{late}");
    assert!(late.contains("%47: nat = call %37, %39, %46"), "{late}");
    assert_eq!(
        section(source, "fn late#4"),
        "fn late#4(%40: fn, %41: struct, %42: nat):\n\
         \x20 %43: struct = project %41, \"A\"\n\
         \x20 %44: struct = project %41, \"B\"\n\
         \x20 %45: nat = call %40, %43, %44, %42\n\
         \x20 ret %45"
    );
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
    let source = "effect Log = write : Nat -> ()\n\
         effect Fail = oops : Nat -> ()\n\
         let s = { f: fn g => fn n => g n }\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           let a = !Log.write n in let b = !Fail.oops n in n\n\
         let go = fn w => handle handle s.f both 1\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    // The function lifted out of the field takes a bundle and its argument,
    // and calls its own argument with that same bundle: two parameters after
    // the capture, two arguments — never the use site's positional records.
    assert_eq!(
        section(source, "fn s#1"),
        "fn s#1(%3: fn, %1: struct, %2: any):\n\
         \x20 %4: any = call %3, %1, %2\n\
         \x20 ret %4"
    );
    // The projection is fitted where it is read, and every call downstream
    // passes exactly as many arguments as its callee has parameters.
    let go = section(source, "fn go(");
    assert!(go.contains("%31: struct = global s"), "{go}");
    assert!(go.contains("%32: fn = project %31, \"f\""), "{go}");
    assert!(go.contains("%50: fn = closure go#6, [%32]"), "{go}");
    assert!(go.contains("%52: fn = call %50, %51"), "{go}");
    assert!(go.contains("%54: nat = call %52, %30, %25, %53"), "{go}");
    // `both` goes in through an adapter that unpacks the bundle back into the
    // two records it takes.
    assert_eq!(
        section(source, "fn go#4"),
        "fn go#4(%35: fn, %36: struct, %37: any):\n\
         \x20 %38: struct = project %36, \"Log\"\n\
         \x20 %39: struct = project %36, \"Fail\"\n\
         \x20 %40: nat = call %35, %38, %39, %37\n\
         \x20 ret %40"
    );
    // And the use site's two records are packed into the one bundle the
    // stored function's next level takes.
    assert_eq!(
        section(source, "fn go#5"),
        "fn go#5(%43: fn, %44: struct, %45: struct, %46: nat):\n\
         \x20 %47: struct = struct { Log: %44, Fail: %45 }\n\
         \x20 %48: any = call %43, %47, %46\n\
         \x20 ret %48"
    );
}

/// The same value carried as a sum payload behaves identically: the payload
/// read takes the case's shape from the type the tag was *built* at, and the
/// match binder holds a value fitted from that shape to the use's own.
#[test]
fn a_function_carried_as_a_sum_payload_is_called_at_the_shape_the_case_holds() {
    let source = "effect Log = write : Nat -> ()\n\
         effect Fail = oops : Nat -> ()\n\
         let wrap = #F (fn g => fn n => g n)\n\
         let both : Nat -> Nat + !Log + !Fail = fn n =>\n\
           let a = !Log.write n in let b = !Fail.oops n in n\n\
         let go = fn w => handle handle\n\
           (match wrap with | #F h => h both 1 | _ => 0 end)\n\
           with | !Log.write x => {} end with | !Fail.oops y => {} end";
    let go = section(source, "fn go(");
    assert!(go.contains("%32: fn = payload %31"), "{go}");
    assert!(go.contains("%50: fn = closure go#6, [%32]"), "{go}");
    assert!(go.contains("%54: nat = call %52, %30, %25, %53"), "{go}");
    // The same unpacking adapter stands in front of `both` as in the struct
    // spelling: the payload's shape survived the container.
    assert_eq!(
        section(source, "fn go#4"),
        "fn go#4(%35: fn, %36: struct, %37: any):\n\
         \x20 %38: struct = project %36, \"Log\"\n\
         \x20 %39: struct = project %36, \"Fail\"\n\
         \x20 %40: nat = call %35, %38, %39, %37\n\
         \x20 ret %40"
    );
}

/// A match whose arms yield function values fits each one, at its own leaf,
/// from the shape it holds to what the whole match stands for — so the switch
/// temp can be called at the match's own type, and the caller's positional
/// records land on adapters that pack them into the bundle each arm's value
/// actually takes.
#[test]
fn a_match_arm_yields_a_function_fitted_to_what_the_match_stands_for() {
    let source = "effect A = a : Nat -> ()\n\
         effect B = b : Nat -> ()\n\
         let both : Nat -> Nat + !A + !B = fn n => let x = !A.a n in let y = !B.b n in n\n\
         let app = fn g => g 1\n\
         let pick = fn u => app\n\
         let go = fn c => handle handle\n\
           ((match c with | #L => pick {} | _ => pick {} end) both)\n\
           with | !A.a s => {} end with | !B.b s => {} end";
    // Each leaf wraps its own `pick {}` — which holds `app`'s bundle shape —
    // in an adapter of its own, and the one call after the switch passes the
    // two records and the argument the match's own type promises.
    let go = section(source, "fn go(");
    assert!(go.contains("%51: fn = closure go#5, [%37]"), "{go}");
    assert!(go.contains("%67: fn = closure go#7, [%53]"), "{go}");
    assert!(go.contains("%70: nat = call %68, %35, %30, %69"), "{go}");
    assert_eq!(
        section(source, "fn go#5"),
        "fn go#5(%38: fn, %39: struct, %40: struct, %41: fn):\n\
         \x20 %42: struct = struct { A: %39, B: %40 }\n\
         \x20 %49: fn = closure go#4, [%41]\n\
         \x20 %50: any = call %38, %42, %49\n\
         \x20 ret %50"
    );
}

/// A value already shaped as the container's type declares goes in and comes
/// out untouched: the store fits trivially, the read fits trivially, and the
/// listing carries not one instruction beyond the ones the source wrote.
#[test]
fn a_value_already_at_the_containers_shape_goes_in_and_out_untouched() {
    let source = "effect Log = write : Nat -> ()\n\
         let s : { f: Nat -> Nat + !Log } = { f: fn n => let z = !Log.write n in n }\n\
         let go = fn w => handle s.f 1 with | !Log.write x => {} end";
    assert_eq!(
        section(source, "global s"),
        "global s:\n\
         \x20 %4: fn = closure s#1, []\n\
         \x20 %5: struct = struct { f: %4 }\n\
         \x20 ret %5"
    );
    assert_eq!(
        section(source, "fn go("),
        "fn go(%6: any):\n\
         \x20 %7: any = new_tag\n\
         \x20 %10: fn = closure go#2, []\n\
         \x20 %11: struct = struct { write: %10 }\n\
         \x20 %16: nat = catch %7:\n\
         \x20   %12: struct = global s\n\
         \x20   %13: fn = project %12, \"f\"\n\
         \x20   %14: nat = const 1\n\
         \x20   %15: nat = call %13, %11, %14\n\
         \x20   yield %15\n\
         \x20 ret %16"
    );
}

/// Tagging a value whose type nothing pinned down: the case's own word is
/// anything at all, which says nothing about a shape, so the payload goes in
/// exactly as it stands — no fitting, no extra instruction.
#[test]
fn a_payload_no_type_pinned_down_goes_in_as_it_stands() {
    assert_eq!(
        section("let f = fn v => #F v", "fn f("),
        "fn f(%0: any):\n\
         \x20 %1: sum = tag #F, %0\n\
         \x20 ret %1"
    );
}

/// A case the production type never listed — here `G`, which only the use's
/// instantiation of `w`'s open row can carry — has no authoritative payload
/// type to read, so the payload is taken at the use's own word.
#[test]
fn a_case_the_production_type_never_named_reads_at_the_uses_word() {
    assert_eq!(
        section(
            "let w = #F 1\nlet f = fn v => match w with | #F x => x | #G 0 => 7 | _ => 0 end",
            "fn f("
        ),
        "fn f(%2: any):\n\
         \x20 %3: sum = global w\n\
         \x20 %10: nat = switch_tag %3:\n\
         \x20   #F =>\n\
         \x20     %4: nat = payload %3\n\
         \x20     yield %4\n\
         \x20   #G =>\n\
         \x20     %5: nat = payload %3\n\
         \x20     %8: nat = switch_nat %5:\n\
         \x20       0 =>\n\
         \x20         %6: nat = const 7\n\
         \x20         yield %6\n\
         \x20       else =>\n\
         \x20         %7: nat = const 0\n\
         \x20         yield %7\n\
         \x20     yield %8\n\
         \x20   else =>\n\
         \x20     %9: nat = const 0\n\
         \x20     yield %9\n\
         \x20 ret %10"
    );
}

/// A bundle is built out of every effect the scope can handle, and each is in
/// it once however many frames between here and the handler carry it.
#[test]
fn a_bundle_names_an_effect_once_however_many_frames_hold_it() {
    let source = "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let idn = fn n => n\n\
         let outer : Nat -> Nat + !Log = fn n =>\n\
           let inner : Nat -> Nat + !Log = fn q => piped idn q in inner n";
    // Both `outer` and the `inner` lifted out of it take `Log`'s record, and
    // the bundle built inside `inner` names the effect once — as the record
    // `inner` itself was handed, which is the innermost one.
    assert!(
        section(source, "fn outer(").starts_with("fn outer(%15: struct, %16: nat):"),
        "{}",
        listing(source)
    );
    assert_eq!(
        section(source, "fn outer#3"),
        "fn outer#3(%17: struct, %18: nat):\n\
         \x20 %19: fn = global idn\n\
         \x20 %24: fn = closure outer#2, [%19]\n\
         \x20 %25: struct = struct { Log: %17 }\n\
         \x20 %26: nat = call piped, %24, %25, %18\n\
         \x20 ret %26"
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
    let source = "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let idn = fn n => n\n\
         let maybe : Nat -> Nat + !Log (when 'a) = fn n => piped idn n";
    assert_eq!(
        section(source, "fn maybe("),
        "fn maybe(%15: struct, %16: nat):\n\
         \x20 %17: fn = global idn\n\
         \x20 %22: fn = closure maybe#2, [%17]\n\
         \x20 %23: nat = call piped, %22, %15, %16\n\
         \x20 ret %23"
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
    let source = "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let noisy : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let caller : (Nat -> Nat + ..'r) -> Nat -> Nat + ..'r =\n\
           fn k => fn n => handle piped noisy n with | !Log.write s => {} end\n\
         let twice : (Nat -> Nat + ..'f) -> Nat -> Nat + ..'f =\n\
           fn g => fn n => piped g n";
    assert_eq!(
        section(source, "fn caller("),
        "fn caller(%19: fn, %20: struct, %21: nat):\n\
         \x20 %22: any = new_tag\n\
         \x20 %25: fn = closure caller#3, []\n\
         \x20 %26: struct = struct { write: %25 }\n\
         \x20 %37: nat = catch %22:\n\
         \x20   %27: fn = global noisy\n\
         \x20   %33: fn = closure caller#4, [%27]\n\
         \x20   %34: struct = struct { Log: %26 }\n\
         \x20   %35: struct = merge %20, %34\n\
         \x20   %36: nat = call piped, %33, %35, %21\n\
         \x20   yield %36\n\
         \x20 ret %37"
    );
    assert_eq!(
        section(source, "fn twice("),
        "fn twice(%45: fn, %46: struct, %47: nat):\n\
         \x20 %48: nat = call piped, %45, %46, %47\n\
         \x20 ret %48"
    );
}

/// Everything after a `raise` is unreachable, and none of it is emitted —
/// neither the instructions a later expression would have assigned nor the
/// terminator a second `raise` would have ended the block with.
#[test]
fn nothing_after_a_raise_is_emitted() {
    assert_eq!(
        section(
            "effect Fail = oops : () -> Nat\n\
             let recover = fn w =>\n\
               handle !Fail.oops () with\n\
               | !Fail.oops z => let q = raise 0 in raise 1\n\
               | return r => r end",
            "fn recover#2"
        ),
        "fn recover#2(%4: any, %2: unit):\n  %3: nat = const 0\n  throw %4, %3"
    );
}

/// A callee written where it is called is nobody's top-level definition, so
/// there is no arity to read off it: the closure is built and then called
/// indirectly, one argument at a time.
#[test]
fn a_callee_written_inline_is_called_indirectly() {
    assert_eq!(
        section("let main = (fn n => n) 1", "global main"),
        "global main:\n\
         \x20 %1: fn = closure main#1, []\n\
         \x20 %2: nat = const 1\n\
         \x20 %3: nat = call %1, %2\n\
         \x20 ret %3"
    );
}

/// A sum position no arm looks into is not dispatched on: the one arm accepts
/// every case, so there is nothing to tell apart and no `switch_tag` to write.
#[test]
fn a_sum_no_arm_tests_is_not_dispatched_on() {
    assert_eq!(
        section("let f = fn w => match #A with | y => 0 end", "fn f("),
        "fn f(%0: any):\n  %1: sum = tag #A\n  %2: nat = const 0\n  ret %2"
    );
}

/// One number written in two arms is one case of the dispatch, not two: the
/// second arm joins the case the first opened rather than adding one that
/// could never be reached.
#[test]
fn a_number_written_twice_in_one_column_is_one_case() {
    let printed = section(
        "let f = fn s => match s with | {a: 0, b: 0} => 1 | {a: 0, b: 1} => 2 | _ => 3 end",
        "fn f(",
    );
    assert_eq!(printed.matches("switch_nat %1:").count(), 1, "{printed}");
    assert_eq!(
        printed.lines().filter(|line| line.trim() == "0 =>").count(),
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
            "type T r = #A Nat | \\#B | ..r\n\
             let f : T (#C Nat) -> Nat = fn v => match v with | #A n => n | #C n => n end",
            "fn f("
        ),
        "fn f(%0: sum):\n\
         \x20 %3: nat = switch_tag %0:\n\
         \x20   #A =>\n\
         \x20     %1: nat = payload %0\n\
         \x20     yield %1\n\
         \x20   #C =>\n\
         \x20     %2: nat = payload %0\n\
         \x20     yield %2\n\
         \x20 ret %3"
    );
}

/// Lowering runs on a program every earlier phase accepted, and says so rather
/// than lowering one that is not: a name that did not resolve is a term with no
/// value to compute.
#[test]
#[should_panic(expected = "LIR runs only on programs with no errors")]
fn a_name_that_did_not_resolve_is_refused() {
    forced("let f = q");
}

/// And a callee inference could not make a function of is a type there is no
/// arrow to read: the same refusal, from the other end.
#[test]
#[should_panic(expected = "LIR runs only on programs with no errors")]
fn a_callee_that_is_not_a_function_is_refused() {
    forced("let f = 1 2");
}

/// Lowering a source the earlier phases did complain about, which is what the
/// driver and the debugger both refuse to do. Only the refusals above use it.
fn forced(source: &str) -> lir::Output {
    let mut files = FileManager::new();
    let file = files.register_new_file("<test>".to_string(), source.to_string());
    let lexed = token::lex(source, file);
    let parsed = parse::parse(lexed.tokens);
    let bundle = Bundle::new("tests", Version::new(0, 1, 0)).expect("the bundle name is valid");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    let inferred = inference::infer(&mint, &mut built.program);
    lir::lower(&mint, &built.program, &inferred)
}

/// An indirect call whose callee gives back a function hands the result on at
/// the shape the callee's own next level declares — here the two agree, so the
/// second call takes the first's temp as it stands, with nothing between.
#[test]
fn a_function_an_indirect_call_returns_is_called_in_turn() {
    assert_eq!(
        section("let main = (fn a => fn b => a) 1 2", "global main"),
        "global main:\n\
         \x20 %4: fn = closure main#2, []\n\
         \x20 %5: nat = const 1\n\
         \x20 %6: fn = call %4, %5\n\
         \x20 %7: nat = const 2\n\
         \x20 %8: nat = call %6, %7\n\
         \x20 ret %8"
    );
}
