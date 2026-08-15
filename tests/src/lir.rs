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
        "let f = let p : { a: Nat } = { a: 1 } in `Some { b: p.a, c: 2 }",
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
         \x20 %5: sum = tag `Some, %4\n\
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
    let source = "let n = 1\nlet u = {}\nlet s = { x: 1 }\nlet c = `A\nlet i = fn x => x";
    assert!(section(source, "global n").contains("%0: nat = const 1"));
    assert!(section(source, "global u").contains(": unit = struct {}"));
    assert!(section(source, "global s").contains(": struct = struct { x:"));
    assert!(section(source, "global c").contains(": sum = tag `A"));
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
         \x20 %7: nat = const 1\n\
         \x20 %8: nat = const 2\n\
         \x20 %9: nat = call add, %7, %8\n\
         \x20 ret %9"
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
        "global part:\n  %7: nat = const 1\n  %8: fn = closure two#2, [%7]\n  ret %8"
    );
    // The wrapper it names takes what was captured and then the argument left,
    // and hands the lot to the uncurried function.
    assert_eq!(
        section(source, "fn two#2"),
        "fn two#2(%2: any, %4: any):\n  %5: any = call two, %2, %4\n  ret %5"
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
        "global bare:\n  %7: fn = global two\n  ret %7"
    );
    assert_eq!(
        section(source, "global two"),
        "global two:\n  %6: fn = closure two#1, []\n  ret %6"
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
         \x20 %11: fn = global id\n\
         \x20 %12: nat = const 1\n\
         \x20 %13: fn = call two, %11, %12\n\
         \x20 %14: nat = const 2\n\
         \x20 %15: nat = call %13, %14\n\
         \x20 ret %15"
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
            "let pick = fn v => match v with | `Some 0 => 100 | `Some n => n | `None => 0 end",
            "fn pick("
        ),
        "fn pick(%0: sum):\n\
         \x20 %5: nat = switch_tag %0:\n\
         \x20   `Some =>\n\
         \x20     %1: nat = payload %0\n\
         \x20     %3: nat = switch_nat %1:\n\
         \x20       0 =>\n\
         \x20         %2: nat = const 100\n\
         \x20         yield %2\n\
         \x20       else =>\n\
         \x20         yield %1\n\
         \x20     yield %3\n\
         \x20   `None =>\n\
         \x20     %4: nat = const 0\n\
         \x20     yield %4\n\
         \x20 ret %5"
    );
}

/// A case no listed one covers needs somewhere to go: an arm that accepts
/// anything gives the dispatch its `else`. With every case of a closed row
/// listed there is nothing left over, and no `else` is written.
#[test]
fn a_tag_dispatch_has_an_else_only_where_something_is_left_over() {
    assert!(
        section(
            "let f = fn v => match v with | `A x => x | b => 0 end",
            "fn f("
        )
        .contains("else =>")
    );
    assert!(
        !section(
            "let f = fn v => match v with | `A x => x | `B => 0 end",
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
            "effect Log = `write : Nat -> ()\nlet shout = fn x => Log.`write x",
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
            "effect Log = `write : Nat -> ()\n\
             let shout = fn x => Log.`write x\n\
             let main = fn w => handle shout 5 with | Log.`write n => {} end",
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
        "effect Log = `write : Nat -> ()\n\
         let main = fn w => handle 1 with | Log.`write n => {} | return r => { got: r } end",
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
    let source = "effect Fail = `oops : () -> Nat\n\
         let recover = fn w =>\n\
           handle Fail.`oops () with | Fail.`oops z => raise 0 | return r => r end";
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
        "effect Log = `write : Nat -> ()\n\
         let nest = fn w =>\n\
           handle (handle Log.`write 1 with | Log.`write a => {} end)\n\
           with | Log.`write b => {} end",
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
            "let piped : (Nat -> Nat ! ..e) -> Nat -> Nat ! ..e where let e = fn g => fn n => g n",
            "fn piped("
        ),
        "fn piped(%0: fn, %1: struct, %2: nat):\n  %3: nat = call %0, %1, %2\n  ret %3"
    );
}

/// A call whose callee's variable part is not the caller's own gets a bundle
/// built at the site, holding evidence for every effect the scope can handle.
#[test]
fn a_bundle_is_built_where_none_can_be_forwarded() {
    let printed = section(
        "effect Log = `write : Nat -> ()\n\
         let piped : (Nat -> Nat ! ..e) -> Nat -> Nat ! ..e where let e = fn g => fn n => g n\n\
         let logger : Nat -> Nat ! `Log = fn n => n\n\
         let use = fn w => handle piped logger 1 with | Log.`write s => {} end",
        "fn use(",
    );
    assert!(
        printed.contains("%21: struct = struct { write: %20 }"),
        "{printed}"
    );
    assert!(
        printed.contains("%24: struct = struct { Log: %21 }"),
        "{printed}"
    );
    assert!(printed.contains("call piped, %22, %24, %23"), "{printed}");
}

/// An operation used as a value becomes a wrapper taking the effect's evidence
/// and the payload — which is what puts the effect in the wrapper's own arrow
/// row — and the call site hands that evidence in like any other.
#[test]
fn an_operation_used_as_a_value_gets_a_wrapper() {
    let source = "effect Log = `write : Nat -> ()\n\
         let op = Log.`write\n\
         let go = fn w => handle op 1 with | Log.`write n => {} end";
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
            "effect Log = `write : Nat -> ()\n\
             let both = fn g => let z = Log.`write 1 in g 2",
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
         fn add#2(%2: any, %4: any):\n\
         \x20 %5: any = call add, %2, %4\n\
         \x20 ret %5\n\
         \n\
         global add:\n\
         \x20 %6: fn = closure add#1, []\n\
         \x20 ret %6\n\
         \n\
         global main:\n\
         \x20 %7: nat = const 1\n\
         \x20 %8: nat = const 2\n\
         \x20 %9: nat = call add, %7, %8\n\
         \x20 ret %9\n"
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
            "type Fallible r = `Err Nat | ..r\n\
             let h : Fallible (`Ok Nat) -> Nat =\n\
               fn t => match t with | `Err n => n | `Ok n => n end",
            "fn h("
        ),
        "fn h(%0: sum):\n\
         \x20 %3: nat = switch_tag %0:\n\
         \x20   `Err =>\n\
         \x20     %1: nat = payload %0\n\
         \x20     yield %1\n\
         \x20   `Ok =>\n\
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
        "effect Mk = `mk : Nat -> (Nat -> Nat)\n\
         let go = fn w => handle Mk.`mk 1 2 with | Mk.`mk n => fn z => z end",
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
