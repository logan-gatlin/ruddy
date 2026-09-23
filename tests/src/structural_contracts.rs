//! Structural contracts through the public compiler interface.

use ruddy::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token::lex,
    tracking::FileID,
};

#[test]
fn shared_contract_graph_debug_and_equality_stay_linear() {
    use ruddy::contracts::Expr;
    use std::sync::Arc;

    let graph = || {
        let mut node = Arc::new(Expr::Input(0));
        for _ in 0..100 {
            node = Arc::new(Expr::Apply {
                function: node.clone(),
                argument: node,
            });
        }
        node
    };
    let (left, right) = (graph(), graph());
    assert_eq!(left, right);
    assert!(format!("{left:?}").len() < 10_000);
}

const IMAGE_OPERATIONS: &str = r#"
let get_channel = fn channel image => match channel with
| #Red => image.r
| #Green => image.g
| #Blue => image.b
| #Alpha => image.a
end

let set_channel = fn channel value image => match channel with
| #Red => { r: value, ..image }
| #Green => { g: value, ..image }
| #Blue => { b: value, ..image }
| #Alpha => { a: value, ..image }
end

let try_get_set = fn get set image => match (get, set) with
| (#None, _) => image
| (_, #None) => image
| (get, set) => set_channel set (get_channel get image) image
end

let extend4 = fn
| () => (#None, #None, #None, #None)
| (a,) => (a, #None, #None, #None)
| (a, b) => (a, b, #None, #None)
| (a, b, c) => (a, b, c, #None)
| (a, b, c, d) => (a, b, c, d)

let swizzle = fn in_channels out_channels image => do
  let (in_a, in_b, in_c, in_d) = extend4 in_channels
  let (out_a, out_b, out_c, out_d) = extend4 out_channels
  return image
    |> try_get_set in_a out_a
    |> try_get_set in_b out_b
    |> try_get_set in_c out_c
    |> try_get_set in_d out_d
end
"#;

fn infer_with_mint(source: &str) -> (Mint, inference::Output) {
    let lexed = lex(source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = Mint::new(Bundle::new("contracts", Version::new(0, 0, 0)).unwrap());
    let lowered = ir::build(&mut mint, parsed.stmts);
    assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
    let output = inference::infer(&mint, &lowered.program, inference::Trace::Off);
    (mint, output)
}

fn infer(source: &str) -> inference::Output {
    infer_with_mint(source).1
}

fn accepts(source: &str) {
    let inferred = infer(source);
    assert!(
        inferred.errors().is_empty(),
        "source:\n{source}\nerrors: {:#?}",
        inferred.errors()
    );
}

fn rejects(source: &str) {
    let inferred = infer(source);
    assert!(
        !inferred.errors().is_empty(),
        "accepted invalid source:\n{source}"
    );
}

#[test]
fn channel_readers_are_polymorphic_over_compatible_channel_payloads() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let red: Nat = get_channel #Red {{r: 1n, g: 2n, b: 3n}}\n\
         let green: String = get_channel #Green {{r: \"red\", g: \"green\"}}\n\
         let blue: Bool = get_channel #Blue {{r: false, b: true}}"
    ));
}

#[test]
fn channel_updates_preserve_shared_payload_types_and_unrelated_fields() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let changed: {{r: Nat, g: Nat, flag: Bool}} =\n\
           set_channel #Red 1n {{r: 0n, g: 2n, flag: true}}\n\
         let added: {{a: Bool, name: String}} =\n\
           set_channel #Alpha true {{name: \"image\"}}"
    ));
}

#[test]
fn tuple_padding_preserves_channel_tags_and_none() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let zero: (#None, #None, #None, #None) = extend4 ()\n\
         let one: (#Red, #None, #None, #None) = extend4 (#Red,)\n\
         let two: (#Red, #Green, #None, #None) = extend4 (#Red, #Green)\n\
         let three: (#Red, #Green, #Blue, #None) = extend4 (#Red, #Green, #Blue)\n\
         let four: (#Red, #Green, #Blue, #Alpha) =\n\
           extend4 (#Red, #Green, #Blue, #Alpha)"
    ));
    rejects(&format!(
        "{IMAGE_OPERATIONS}\nlet five = extend4 (#Red, #Green, #Blue, #Alpha, #Other)"
    ));
    rejects(&format!("{IMAGE_OPERATIONS}\nlet scalar = extend4 (1n,)"));
}

#[test]
fn transfer_noop_arms_have_no_channel_requirements() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let neither: {{}} = try_get_set #None #None {{}}\n\
         let no_source: {{}} = try_get_set #None #Red {{}}\n\
         let no_destination: {{}} = try_get_set #Red #None {{}}\n\
         let compatible: {{r: Nat, g: Nat}} =\n\
           try_get_set #None #None {{r: 1n, g: 2n}}"
    ));
}

#[test]
fn swizzle_composes_updates_in_source_order() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let added: {{r: Nat, g: Nat}} =\n\
           swizzle (#Red,) (#Green,) {{r: 1n}}\n\
         let sequential: {{r: Nat, g: Nat, b: Nat}} =\n\
           swizzle (#Red, #Green) (#Green, #Blue) {{r: 1n, g: 2n, b: 3n}}"
    ));
}

#[test]
fn contracts_survive_aliases_partial_calls_and_higher_order_values() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let alias = get_channel\n\
         let red = alias #Red\n\
         let number: Nat = red {{r: 1n, g: 2n}}\n\
         let text: String = red {{r: \"red\", g: \"green\"}}\n\
         let invoke = fn operation channel image => operation channel image\n\
         let through_callback: String = invoke get_channel #Green {{r: \"red\", g: \"green\"}}\n\
         let operations = {{read: get_channel, write: set_channel}}\n\
         let through_field: Bool = operations.read #Blue {{r: false, b: true}}"
    ));
}

#[test]
fn broad_selectors_require_every_reachable_input_case() {
    accepts(&format!(
        "{IMAGE_OPERATIONS}\n\
         let selector: #Red | #Green = #Red\n\
         let value: Nat = get_channel selector {{r: 1n, g: 2n}}\n\
         let updated = set_channel selector 1n {{}}"
    ));
    rejects(&format!(
        "{IMAGE_OPERATIONS}\n\
         let selector: #Red | #Green = #Red\n\
         let missing = get_channel selector {{r: 1n}}"
    ));
    rejects(&format!(
        "{IMAGE_OPERATIONS}\n\
         let selector: #Red | #Green = #Red\n\
         let updated = set_channel selector 1n {{}}\n\
         let unsafe = updated.r"
    ));
}

#[test]
fn unused_bad_bodies_and_bad_annotations_are_rejected() {
    rejects(
        "let unused = fn selector image => match selector with\n\
         | #Red => 1n + \"bad\"\n\
         | #Green => image.g\n\
         end",
    );
    rejects(
        "let wrong: match | #Red => {r: Nat} -> Nat end =\n\
         fn selector image => match selector with | #Red => \"wrong\" end",
    );
}

#[test]
fn structural_definitions_require_one_ordinary_result_family() {
    for (left, right) in [
        ("1n", "\"hello\""),
        ("1n", "fn value => value"),
        ("1n", "()"),
        ("{value: 1n}", "{value: \"hello\"}"),
        ("#Value 1n", "#Value \"hello\""),
        ("[1n]", "[\"hello\"]"),
        ("fn _ => 1n", "fn _ => \"hello\""),
    ] {
        let definition = format!("let choose = fn | #Natural => {left} | #Text => {right}\n");
        // Validation happens at the definition, even without a caller, and
        // forwarding through another contract must not conceal the failure.
        for use_site in [
            "",
            "let result = choose #Natural",
            "let wrapped = fn input => choose input",
            "let invoke = fn operation input => operation input\nlet wrapped = invoke choose",
        ] {
            rejects(&format!("{definition}{use_site}"));
        }
    }
}

#[test]
fn conditional_field_and_variant_results_survive_wrappers() {
    accepts(
        "let fields = fn | #Natural => {natural: 1n} | #Text => {text: \"hello\"}\n\
         let variants = fn | #Natural => #Natural 1n | #Text => #Text \"hello\"\n\
         let shared = fn | #Natural => {value: 1n} | #Text => {value: 2n}\n\
         let choose = fn | #Natural => 1n | #Text => 2n\n\
         let wrapped = fn input => choose input\n\
         let nested = fn input => wrapped input\n\
         let identity = fn value => value\n\
         let invoke = fn operation input => operation input\n\
         let first: Nat = nested #Natural\n\
         let second: Nat = invoke (identity wrapped) #Text\n\
         let selector: #Natural | #Text = #Natural\n\
         let broad: Nat = invoke wrapped selector\n\
         let record = invoke fields selector\n\
         let variant: #Natural Nat | #Text String = invoke variants selector\n\
         let payload: {value: Nat} = invoke shared selector\n\
         let read_record = fn value => match value with\n\
         | {natural} => natural | {text} => 0n end\n\
         let read: Nat = read_record record",
    );
}

#[test]
fn ordinary_recursive_functions_keep_monomorphic_checking() {
    accepts(
        "let loop = fn remaining => if remaining then loop false else 1n end\n\
         let result: Nat = loop true",
    );
}

const TWO_CHANNELS: &str = "let read = fn selector image => match selector with\n\
    | #Red => image.r | #Green => image.g end\n";

#[test]
fn annotated_local_bindings_retain_deferred_argument_requirements() {
    let checked = format!(
        "{TWO_CHANNELS}\n\
         let checked = fn selector image => do\n\
           let value: Nat = read selector image\n\
           return value\n\
         end\n"
    );
    accepts(&format!(
        "{checked}let good: Nat = checked #Red {{r: 1n, g: 2n}}"
    ));
    rejects(&format!(
        "{checked}let bad = checked #Green {{r: 1n, g: \"green\"}}"
    ));
    rejects(&format!("{checked}let missing = checked #Red {{}}"));
}

#[test]
fn arithmetic_and_arrays_keep_requirements_of_deferred_calls() {
    let arithmetic = format!(
        "{TWO_CHANNELS}\n\
         let increment = fn selector image => read selector image + 1\n"
    );
    let arrays = format!(
        "{TWO_CHANNELS}\n\
         let collect = fn selector image => [read selector image]\n"
    );
    accepts(&format!(
        "{arithmetic}\n\
         let number: Real = increment #Red {{r: 1, g: 2}}"
    ));
    accepts(&format!(
        "{arrays}\n\
         let array: [String] = collect #Green {{r: \"red\", g: \"green\"}}\n\
         let numbers: [Nat] = collect #Red {{r: 1n, g: 2n}}"
    ));
    rejects(&format!(
        "{arithmetic}let bad = increment #Green {{r: 1, g: \"green\"}}"
    ));
    rejects(&format!("{arrays}let missing = collect #Red {{}}"));
    accepts(&format!(
        "{TWO_CHANNELS}\n\
         let prepend = fn selector image rest => [read selector image, ..rest]\n\
         let result: [String] = prepend #Green {{g: \"green\"}} [\"tail\"]"
    ));
    rejects(&format!(
        "{TWO_CHANNELS}\n\
         let prepend = fn selector image rest => [read selector image, ..rest]\n\
         let bad = prepend #Green {{g: \"green\"}} [1n]"
    ));
}

#[test]
fn unsupported_handlers_cannot_erase_deferred_input_obligations() {
    rejects(&format!(
        "{TWO_CHANNELS}\n\
         let wrapped = fn selector image =>\n\
           handle read selector image with | return value => value end\n\
         let missing = wrapped #Red {{}}"
    ));
}

#[test]
fn unsupported_hidden_scopes_cannot_erase_deferred_input_obligations() {
    rejects(&format!(
        "{TWO_CHANNELS}\n\
         type Sealed = hide 'a => {{value: 'a}}\n\
         let seal: Sealed = {{value: 1n}}\n\
         let wrapped = fn package selector image => match package with\n\
         | hide 'a _ => read selector image\n\
         end\n\
         let missing = wrapped seal #Red {{}}"
    ));
}

#[test]
fn optional_open_rows_do_not_hide_reachable_callback_effects() {
    rejects(
        "effect Tick = () -> Nat\n\
         let choose = fn action row => match row with\n\
         | {x, ..} => action ()\n\
         | _ => 0n\n\
         end\n\
         let leak: (() -> Nat + !Tick) -> {..'rest} -> Nat = choose",
    );
}

#[test]
fn payload_requirements_are_shared_across_structural_arms() {
    rejects(
        "let wants: Nat -> Nat = fn x => x\n\
        let choose = fn value => match value with\n\
        | {a} => wants a\n\
        | {a, b} => wants (a ())\n\
        end",
    );
    rejects(
        "let unused = fn value => match value with\n\
        | {a} => 1n + \"bad\" | {a, b} => a () end",
    );
    accepts(
        "let wants: Nat -> Nat = fn x => x\n\
        let tagged = fn value => match value with\n\
        | #A a => wants a | _ => 0n end\n\
        let number: Nat = tagged (#A 1n)\n\
        let fallback: Nat = tagged #B",
    );
}

#[test]
fn catchalls_preserve_pattern_constructor_families() {
    let record = "let choose = fn value => match value with\n\
        | () => 1n | _ => 2n end\n";
    accepts(&format!(
        "{record}let empty: Nat = choose ()\nlet nonempty: Nat = choose {{x: 1n}}"
    ));
    rejects(&format!("{record}let bad = choose 3n"));
    rejects(&format!("{record}let bad = choose #Tag"));
    let nested = "let choose = fn value => match value with\n\
        | {x: #Some ()} => 1n | _ => 2n end\n";
    accepts(&format!(
        "{nested}let good: Nat = choose {{x: #Some {{y: 1n}}}}"
    ));
    rejects(&format!("{nested}let bad = choose {{x: 3n}}"));
    rejects(&format!("{nested}let bad = choose {{x: #Some 3n}}"));
}

#[test]
fn structural_contract_capture_growth_reports_the_source_definition() {
    let mut source = "let f0 = fn x => match x with | {a, ..} => x.a end\n".to_owned();
    for index in 1..=16 {
        source.push_str(&format!(
            "let f{index} = fn x => {{a: f{} x, b: f{} x}}\n",
            index - 1,
            index - 1
        ));
    }
    let output = infer(&source);
    let limits: Vec<_> = output
        .errors()
        .iter()
        .filter(|error| {
            matches!(
                &error.kind, inference::ErrorKind::StructuralContract { message }
                if message.contains("semantic type graph budget")
            )
        })
        .collect();
    assert!(
        !limits.is_empty(),
        "capture growth must stop before publication"
    );
    assert_eq!(
        limits.len(),
        output.errors().len(),
        "recovery must prevent cascading errors: {:#?}",
        output.errors()
    );
    for error in limits {
        let definition = &output.semantics().typed()[&error.at.definition];
        assert_eq!(error.at, definition.name_at);
        assert!(matches!(
            &**output.semantics().schemes()[&error.at.definition].body(),
            ruddy::types::Ty::Undecided
        ));
    }
}

#[test]
fn constrained_structural_captures_require_preserved_scheme_obligations() {
    let source = "extern one: fn 'v => capture ({x when 'p: Nat, y when 'q: Nat, ..} -> ()) 'v where 'p != 'q = \"host.one\"\n\
        let route = fn v => match v with | {x, ..} => one v | _ => () end\n";
    // This authoritative extern isolates semantic obligations. Its structural
    // reflection layout is separately unsupported, so that backend diagnostic
    // must neither count as a semantic rejection nor hide a missing clause.
    let declaration = source.lines().next().unwrap();
    let good = infer(&format!("{declaration}\nlet good: () = one {{x: 1n}}"));
    assert!(
        good.errors().iter().all(|error| matches!(
            error.kind,
            inference::ErrorKind::RuntimeTypeInformation { .. }
        )),
        "{:#?}",
        good.errors()
    );
    let bad = infer(&format!("{source}let bad = route {{x: 1n, y: 2n}}"));
    assert!(
        bad.errors().iter().any(|error| matches!(
            error.kind,
            inference::ErrorKind::PresenceImpossible { .. }
                | inference::ErrorKind::PresenceRequired { .. }
                | inference::ErrorKind::StructuralContract { .. }
        )),
        "the outer presence clause was erased: {:#?}",
        bad.errors()
    );
    let direct_bad = infer(&format!("{declaration}\nlet bad = one {{x: 1n, y: 2n}}"));
    assert!(
        direct_bad.errors().iter().any(|error| matches!(
            error.kind,
            inference::ErrorKind::PresenceImpossible { .. }
                | inference::ErrorKind::PresenceRequired { .. }
        )),
        "{:#?}",
        direct_bad.errors()
    );
}

#[test]
fn constrained_match_contracts_keep_direct_call_obligations() {
    let declaration = "extern one: match | () => ({x when 'p: Nat, y when 'q: Nat} -> ()) end where 'p != 'q = \"host.one\"";
    let good = infer(&format!("{declaration}\nlet good: () = one () {{x: 1n}}"));
    assert!(
        good.errors().iter().all(|error| matches!(
            error.kind,
            inference::ErrorKind::RuntimeTypeInformation { .. }
        )),
        "{:#?}",
        good.errors()
    );
    for source in [
        format!("{declaration}\nlet bad = one () {{x: 1n, y: 2n}}"),
        format!(
            "{declaration}\nlet route = fn v => match v with | {{x, ..}} => one () v | _ => () end\nlet bad = route {{x: 1n, y: 2n}}"
        ),
    ] {
        let bad = infer(&source);
        assert!(
            bad.errors().iter().any(|error| matches!(
                error.kind,
                inference::ErrorKind::PresenceImpossible { .. }
                    | inference::ErrorKind::PresenceRequired { .. }
                    | inference::ErrorKind::StructuralContract { .. }
            )),
            "the outer presence clause was erased: {:#?}",
            bad.errors()
        );
    }
}

#[test]
fn tag_result_joining_preserves_payload_and_record_correlations() {
    use ruddy::types::{Rest, Ty};
    let selector = "extern selector: #Red | #Blue = \"host.selector\"\n";
    let disjoint = infer(&format!(
        "{selector}let left: #Some Nat = #Some 1n\nlet right: #None = #None\nlet choose = fn tag => match tag with | #Red => left | #Blue => right end\nlet result = choose selector"
    ));
    assert!(disjoint.errors().is_empty(), "{:#?}", disjoint.errors());
    let result = disjoint.semantics().schemes().values().last().unwrap();
    let Ty::Sum(row) = &**result.body() else {
        panic!("disjoint closed tags should retain an ordinary sum layout: {result:?}");
    };
    assert!(matches!(row.rest, Rest::Closed));
    assert_eq!(row.labels.len(), 2);

    for values in [
        "let left: #Some Nat = #Some 1n\nlet right: #Some String = #Some \"blue\"",
        "let left = {a: 1n, b: \"red\"}\nlet right = {a: \"blue\", b: 2n}",
    ] {
        // Even an unused definition must reject incompatible payloads under
        // one field or tag; the failure must not wait for a broad selector.
        rejects(&format!(
            "{values}\nlet choose = fn tag => match tag with | #Red => left | #Blue => right end"
        ));
    }
}

#[test]
fn image_operations_compile_through_coverage_and_artifact_validation() {
    let source = format!(
        "type Channel = #Red | #Blue | #Green | #Alpha\n\
         type Buf = [Nat8]\n\
         type RGB = {{r: Buf, g: Buf, b: Buf}}\n\
         type RGBA = {{..RGB, a: Buf}}\n\
         {IMAGE_OPERATIONS}\n\
         let test = swizzle (#Red,) (#Green,) {{r: [], g: [], b: []}}"
    );
    let lexed = lex(&source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{:?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let mint = Mint::new(Bundle::new("image", Version::new(0, 0, 0)).unwrap());
    let accepted =
        ruddy::compile::compile_with_dependencies(mint, parsed.stmts, &[], inference::Trace::Off)
            .unwrap_or_else(|partial| panic!("{partial:#?}"));
    accepted
        .artifact()
        .to_unchecked()
        .validate()
        .expect("image artifact validates");
}

#[test]
fn inferred_image_operation_annotations_preserve_their_call_requirements() {
    let (mint, inferred) = infer_with_mint(IMAGE_OPERATIONS);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    let mut annotated = IMAGE_OPERATIONS.to_owned();
    for (symbol, scheme) in inferred.semantics().schemes() {
        let name = mint.name(*symbol);
        let signature = scheme.to_string();
        annotated = annotated.replace(
            &format!("let {name} ="),
            &format!("let {name}: {signature} ="),
        );
    }
    accepts(&format!(
        "{annotated}\n\
         let rgb: {{r: [Nat8], g: [Nat8], b: [Nat8]}} =\n\
           swizzle (#Red,) (#Green,) {{r: [], g: [], b: []}}\n\
         let red: Nat = get_channel #Red {{r: 1n}}\n\
         let green: String = get_channel #Green {{g: \"green\"}}"
    ));
    rejects(&format!("{annotated}\nlet missing = get_channel #Red {{}}"));
    rejects(&format!("{annotated}\nlet scalar = extend4 (1n,)"));
}

#[test]
fn explicit_presence_promises_reject_stronger_structural_requirements() {
    let getter = "let get = fn r => match r with | {value, ..} => r.value end";
    for annotation in [
        "{value when 'p: Nat} -> Nat where 'p = 'p",
        "W 'p where 'p = 'p",
    ] {
        for rhs in ["get", "fn r => get r"] {
            rejects(&format!(
                "{getter}\ntype W 'p = {{value when 'p: Nat}} -> Nat\nlet bad: {annotation} = {rhs}"
            ));
        }
    }
    rejects(&format!(
        "{getter}\nlet get_again: {{value when 'p: Nat}} -> Nat = fn r => get r\nlet bad: Nat = get_again {{}}"
    ));
}

#[test]
fn named_curried_annotations_check_structural_bodies_and_effects() {
    let source = "type Result 'a = #Some 'a | #Error String\n\
        type Copy = String -> Write\n\
        type Write = String -> Result ()\n\
        let read: String -> Result [Nat8] = fn path => #Some []\n\
        let write: String -> [Nat8] -> Result () = fn path bytes => #Some ()\n\
        let copy = fn source destination => match read source with\n\
        | #Some bytes => write destination bytes | #Error error => #Error error end";
    accepts(&format!("{source}\nlet checked: Copy = copy"));
    rejects(&format!(
        "{source}\ntype Wrong = String -> String -> Nat\nlet bad: Wrong = copy"
    ));
    rejects(
        "effect Tick = () -> Nat\n\
         type Pure = (() -> Nat + !Tick) -> {value: Nat} -> Nat\n\
         let run = fn action row => match row with | {value} => action () end\n\
         let bad: Pure = run",
    );
}

#[test]
fn guarded_recursive_contract_aliases_keep_conformance_assumptions() {
    accepts(
        "type A = fn 'x => capture (() -> A)\n\
         type B = fn 'x => capture (() -> B)\n\
         let convert: A -> B = fn value => value",
    );
    rejects(
        "type A = fn 'x => capture ({next: () -> A, value: Nat})\n\
         type B = fn 'x => capture ({next: () -> B, value: String})\n\
         let convert: A -> B = fn value => value",
    );
    rejects(
        "effect Tick = () -> Nat\n\
         type A = fn 'x => capture (() -> A + !Tick)\n\
         type B = fn 'x => capture (() -> B)\n\
         let convert: A -> B = fn value => value",
    );
    let application = infer(
        "type Spin = fn 'x => (capture ({next: Spin})).next 'x\n\
         extern spin: Spin = \"undefined\"\n\
         let result = spin ()",
    );
    assert!(application.errors().iter().any(|error| matches!(
        &error.kind,
        inference::ErrorKind::StructuralContract { message } if message.contains("active contract")
    )), "recursive execution must not borrow the alias equality proof: {:?}", application.errors());
}

#[test]
fn finite_contract_alias_conformance_uses_a_small_stack() {
    std::thread::Builder::new()
        .name("finite-contract-alias-conformance".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            for structural_expected in [true, false] {
                let mut source = String::new();
                for prefix in ["A", "B"] {
                    for index in 0..1024 {
                        let result = format!("{prefix}{}", index + 1);
                        let body = match (prefix, structural_expected) {
                            ("A", false) => format!("() -> {result}"),
                            ("B", false) => format!("fn 'x => capture ({result})"),
                            _ => format!("fn 'x => capture (() -> {result})"),
                        };
                        source.push_str(&format!("type {prefix}{index} = {body}\n"));
                    }
                    source.push_str(&format!("type {prefix}1024 = Nat\n"));
                }
                source.push_str("let convert: B0 -> A0 = fn value => value\n");
                accepts(&source);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
