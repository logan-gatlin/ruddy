//! Tests for the reference interpreter over linked artifacts.
//!
//! The interpreter executes the same linked artifact the JavaScript backend
//! does, with its own value representation, so these tests are about what a
//! Ruddy program means rather than about what one backend happens to do.

use std::{fs, path::Path, process::Command};

use ruddy_interp::{Program, Value, render};

/// A project with the given source, and the standard library when it is asked
/// for, compiled and linked.
fn project(
    source: &str,
    standard: bool,
    integers: Option<u32>,
    javascript: bool,
) -> tempfile::TempDir {
    let project = tempfile::tempdir().expect("a temporary interpreter project");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let mut manifest = String::from(
        "name = \"interp-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\n",
    );
    if javascript {
        manifest.push_str("target = \"js\"\n");
    }
    if let Some(integers) = integers {
        manifest.push_str(&format!("integers = {integers}\n"));
    }
    manifest.push_str("\n[dependencies]\n");
    if standard {
        manifest.push_str(&format!("std = {root:?}\n"));
    } else {
        manifest.push_str("std = false\n");
    }
    fs::write(project.path().join("Ruddy.toml"), manifest).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

/// The named exports of a program, as the JavaScript backend produces them
/// and as the interpreter does, so that one can be held against the other.
fn both(source: &str, exports: &[&str], integers: Option<u32>) -> (Vec<String>, Vec<String>) {
    let project = project(source, true, integers, true);
    let artifact = ruddy_cli::build_project(project.path()).expect("the consumer builds");
    let javascript = artifact.with_extension("js");
    let reads = exports
        .iter()
        .map(|name| format!("app[{}]", serde_json::to_string(name).unwrap()))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({}));\nconsole.log(JSON.stringify([{reads}]));",
        serde_json::to_string(javascript.to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", &script])
        .output()
        .expect("Node runs");
    assert!(
        output.status.success(),
        "node: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("the probe prints JSON");
    let node = values.iter().map(|value| value.to_string()).collect();

    let linked = ruddy_cli::compile(project.path()).expect("the consumer compiles");
    let program = Program::load(&linked).expect("the linked artifact loads");
    let interpreted = exports
        .iter()
        .map(|name| render::json(&program.export(name).expect("an exported value")))
        .collect();
    (node, interpreted)
}

/// Compile, link, and load a program the interpreter can run.
fn load(source: &str, standard: bool, integers: Option<u32>) -> Program {
    let project = project(source, standard, integers, false);
    let artifact = ruddy_cli::compile(project.path()).expect("the interpreter consumer compiles");
    Program::load(&artifact).expect("the linked artifact loads")
}

fn json(program: &Program, export: &str) -> String {
    render::json(&program.export(export).expect("an exported value"))
}

#[test]
fn values_of_every_representation_survive_a_run() {
    let program = load(
        r#"
let natural = 42n
let integer = -7i
let real = 1.5
let text = "hello"
let truth = true
let unit = ()
let record = { left: 1n, right: "two" }
let items = [1n, 2n, 3n]
let tagged: #Some Nat | #None = #Some 5n
let bare: #Some Nat | #None = #None
let nested = { inner: { deep: [#Some 1n] } }
let wide = 9007199254740991n
let fixed = 255n8
let wide_fixed = 9223372036854775807i64
"#,
        false,
        None,
    );
    assert_eq!(json(&program, "natural"), "42");
    assert_eq!(json(&program, "integer"), "-7");
    assert_eq!(json(&program, "real"), "1.5");
    assert_eq!(json(&program, "text"), "\"hello\"");
    assert_eq!(json(&program, "truth"), "true");
    assert_eq!(json(&program, "unit"), "{}");
    assert_eq!(json(&program, "record"), "{\"left\":1,\"right\":\"two\"}");
    assert_eq!(json(&program, "items"), "[1,2,3]");
    assert_eq!(json(&program, "tagged"), "{\"tag\":\"Some\",\"value\":5}");
    assert_eq!(json(&program, "bare"), "{\"tag\":\"None\"}");
    assert_eq!(
        json(&program, "nested"),
        "{\"inner\":{\"deep\":[{\"tag\":\"Some\",\"value\":1}]}}"
    );
    assert_eq!(json(&program, "wide"), "9007199254740991");
    assert_eq!(json(&program, "fixed"), "255");
    assert_eq!(json(&program, "wide_fixed"), "\"9223372036854775807\"");
}

#[test]
fn functions_run_when_the_host_calls_them() {
    let mut program = load(
        r#"
let double = fn value => value + value
let pick = fn record => record.chosen
let apply_twice = fn f => fn value => f (f value)
"#,
        false,
        None,
    );
    let double = program.export("double").unwrap();
    let result = program.call(&double, Value::Real(21.0)).unwrap();
    assert_eq!(render::json(&result), "42");

    let pick = program.export("pick").unwrap();
    let record = Value::record(vec![("chosen", Value::string("yes"))]);
    assert_eq!(
        render::json(&program.call(&pick, record).unwrap()),
        "\"yes\""
    );

    let apply = program.export("apply_twice").unwrap();
    let doubler = program.call(&apply, double).unwrap();
    let result = program.call(&doubler, Value::Real(2.0)).unwrap();
    assert_eq!(render::json(&result), "8");
}

#[test]
fn matching_takes_values_apart() {
    let program = load(
        r#"
type Shape = #Circle Real | #Rect { width: Real, height: Real } | #Empty
let area = fn shape => match shape with
| #Circle radius => radius * radius * 3.0
| #Rect size => size.width * size.height
| #Empty => 0.0
end
let circle = area (#Circle 2.0)
let rectangle = area (#Rect { width: 3.0, height: 4.0 })
let empty = area #Empty
let first = match [1n, 2n, 3n] with | [head, ..rest] => head | [] => 0n end
let rest_length = match [1n, 2n, 3n] with | [_, ..rest] => rest | [] => [] end
let last = match [1n, 2n, 3n] with | [..start, tail] => tail | [] => 0n end
let literal = match "two" with | "one" => 1n | "two" => 2n | _ => 0n end
"#,
        false,
        None,
    );
    assert_eq!(json(&program, "circle"), "12");
    assert_eq!(json(&program, "rectangle"), "12");
    assert_eq!(json(&program, "empty"), "0");
    assert_eq!(json(&program, "first"), "1");
    assert_eq!(json(&program, "rest_length"), "[2,3]");
    assert_eq!(json(&program, "last"), "3");
    assert_eq!(json(&program, "literal"), "2");
}

#[test]
fn effects_run_through_handlers() {
    let program = load(
        r#"
effect Ask = { ask: () -> Real }
effect Tell = { tell: Real -> () }
let asking = fn _ => do
  let value = !Ask.ask ()
  _ = !Tell.tell value
  return value + 1.0
end
let asked = handle asking () with
| !Ask.ask _ => 41.0
| !Tell.tell _ => ()
end
let raised = handle asking () with
| !Ask.ask _ => raise 0.0
| !Tell.tell _ => ()
end
let returned = handle asking () with
| !Ask.ask _ => 1.0
| !Tell.tell _ => ()
| return value => value * 10.0
end
let nested = handle (fn _ => handle asking () with
| !Ask.ask _ => 1.0
| !Tell.tell _ => ()
end) () with
| !Ask.ask _ => 99.0
| !Tell.tell _ => ()
end
"#,
        false,
        None,
    );
    assert_eq!(json(&program, "asked"), "42");
    assert_eq!(json(&program, "raised"), "0");
    assert_eq!(json(&program, "returned"), "20");
    assert_eq!(json(&program, "nested"), "2");
}

#[test]
fn cells_hold_values_through_a_run() {
    let program = load(
        r#"
let count = fn _ => do
  let cell = mut 0.0
  _ = cell := ~cell + 1.0
  _ = cell := ~cell + 1.0
  return ~cell
end
let counted = count ()
let replace = fn _ => do
  let cell = mut "first"
  let previous = ~cell
  _ = cell := "second"
  return { previous: previous, current: ~cell }
end
let written = replace ()
"#,
        false,
        None,
    );
    assert_eq!(json(&program, "counted"), "2");
    assert_eq!(
        json(&program, "written"),
        "{\"previous\":\"first\",\"current\":\"second\"}"
    );
}

#[test]
fn mirrors_describe_types_the_same_way_on_both_backends() {
    let source = r#"
let kind: std::reflect::Node -> String = fn node => match node with
| #Nat _ => "Nat"
| #Int _ => "Int"
| #Real => "Real"
| #String => "String"
| #Bool => "Bool"
| #Foreign => "Foreign"
| #Fixed _ => "Fixed"
| #Array _ => "Array"
| #Cell _ => "Cell"
| #Function _ => "Function"
| #Effects _ => "Effects"
| #Record _ => "Record"
| #Sum _ => "Sum"
| #Alias _ => "Alias"
| #Extend _ => "Extend"
| #Parameter _ => "Parameter"
| #Mirror _ => "Mirror"
| #Hidden _ => "Hidden"
| #Variable _ => "Variable"
end
let root_of: std::reflect::Description -> String = fn description =>
  match std::array::get description.nodes description.root with
  | #Some node => kind node
  | #None => "none"
  end
let record_mirror: Mirror { name: String, count: Nat } = std::reflect::mirror ()
let record_kind = root_of (std::reflect::describe record_mirror)
let field_names = do
  let description = std::reflect::describe record_mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Record fields) =>
    std::str::join "," (std::array::map (fn field => field.name) fields)
  | _ => "none"
  end
end
let nat_max = do
  let mirror: Mirror Nat = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Nat domain) => domain.max
  | _ => "none"
  end
end
let int_min = do
  let mirror: Mirror Int = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Int domain) => domain.min
  | _ => "none"
  end
end
let fixed_max = do
  let mirror: Mirror Nat32 = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Fixed domain) => domain.max
  | _ => "none"
  end
end
let twin: Mirror { name: String, count: Nat } = std::reflect::mirror ()
let natural: Mirror Nat = std::reflect::mirror ()
let same_type = match std::reflect::same record_mirror twin with
| #Some _ => true
| #None => false
end
let other_type = match std::reflect::same record_mirror natural with
| #Some _ => true
| #None => false
end
"#;
    let exports = [
        "record_kind",
        "field_names",
        "nat_max",
        "int_min",
        "fixed_max",
        "same_type",
        "other_type",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"Record\"",
            "\"count,name\"",
            "\"9007199254740991\"",
            "\"-9007199254740991\"",
            "\"4294967295\"",
            "true",
            "false",
        ]
    );
}

#[test]
fn typed_views_read_and_build_values() {
    let source = r#"
let described = do
  let mirror: Mirror { name: String, count: Nat } = std::reflect::mirror ()
  return match std::reflect::shape mirror with
  | #Record view =>
    std::str::join "," (std::array::map (fn field => match field with
      | hide 'f { name, .. } => name
      end) view.fields)
  | _ => "none"
  end
end
let rebuilt = do
  let mirror: Mirror { name: String, count: Nat } = std::reflect::mirror ()
  let original = { name: "ruddy", count: 3n }
  return match std::reflect::shape mirror with
  | #Record view =>
    match std::array::traverse_option
      (fn field => match field with
      | hide 'f { read, bind, .. } =>
        std::option::map bind (read original)
      end)
      view.fields with
    | #Some bindings => match view.build bindings with
      | #Some value => std::str::concat value.name (std::str::from_nat value.count)
      | #Error _ => "error"
      end
    | #None => "missing"
    end
  | _ => "none"
  end
end
let projected = do
  let mirror: Mirror (#Some Nat | #None) = std::reflect::mirror ()
  return match std::reflect::shape mirror with
  | #Sum view =>
    std::str::join "," (std::array::map (fn item => match item with
      | hide 'p { name, .. } => name
      end) view.cases)
  | _ => "none"
  end
end
"#;
    let exports = ["described", "rebuilt", "projected"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec!["\"count,name\"", "\"ruddy3\"", "\"None,Some\""]
    );
}

#[test]
fn codecs_write_and_read_the_same_documents() {
    let source = r#"
let encoded = match std::json::encode { name: "ruddy", counts: [1n, 2n, 3n] } with
| #Some text => text
| #Error _ => "error"
end
let read: String -> std::result::Result { name: String, counts: [Nat] } std::json::Error =
  std::json::decode
let decoded = match read "{\"name\":\"read\",\"counts\":[4,5]}" with
| #Some value => std::str::concat value.name (std::str::from_nat (std::array::len value.counts))
| #Error _ => "error"
end
let refused = match read "{\"name\":\"read\",\"counts\":[4],\"extra\":1}" with
| #Some _ => "accepted"
| #Error _ => "refused"
end
let canonical = match std::json::encode_canonical { second: 2n, first: 1n } with
| #Some text => text
| #Error _ => "error"
end
let round_trip = do
  let write: (#Circle Real | #Square Real) -> std::result::Result String std::json::Error =
    std::json::encode
  return match write (#Circle 1.5) with
  | #Some text => text
  | #Error _ => "error"
  end
end
"#;
    let exports = ["encoded", "decoded", "refused", "canonical", "round_trip"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"{\\\"counts\\\":[1,2,3],\\\"name\\\":\\\"ruddy\\\"}\"",
            "\"read2\"",
            "\"refused\"",
            "\"{\\\"first\\\":1,\\\"second\\\":2}\"",
            "\"{\\\"tag\\\":\\\"Circle\\\",\\\"value\\\":1.5}\"",
        ]
    );
}

#[test]
fn the_bound_integer_domains_reach_both_backends() {
    let source = r#"
let nat_max = do
  let mirror: Mirror Nat = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Nat domain) => domain.max
  | _ => "none"
  end
end
let read: String -> std::result::Result Nat std::json::Error = std::json::decode
let large = match read "4294967296" with
| #Some value => std::str::from_nat value
| #Error _ => "refused"
end
let small = match read "4294967295" with
| #Some value => std::str::from_nat value
| #Error _ => "refused"
end
"#;
    let exports = ["nat_max", "large", "small"];
    let (wide_node, wide) = both(source, &exports, None);
    assert_eq!(wide_node, wide);
    assert_eq!(
        wide,
        vec!["\"9007199254740991\"", "\"4294967296\"", "\"4294967295\""]
    );

    let (narrow_node, narrow) = both(source, &exports, Some(32));
    assert_eq!(narrow_node, narrow);
    assert_eq!(
        narrow,
        vec!["\"4294967295\"", "\"refused\"", "\"4294967295\""]
    );
}

#[test]
fn a_sixty_four_bit_domain_runs_where_javascript_cannot() {
    // JavaScript cannot hold a 64-bit `Nat`, so its manifest refuses the
    // domain. The interpreter is another target, and runs it.
    let program = load(
        r#"
let nat_domain = do
  let mirror: Mirror Nat = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Nat domain) => std::str::concat domain.max (std::str::concat " " (std::str::from_nat domain.bits))
  | _ => "none"
  end
end
let int_domain = do
  let mirror: Mirror Int = std::reflect::mirror ()
  let description = std::reflect::describe mirror
  return match std::array::get description.nodes description.root with
  | #Some (#Int domain) => domain.min
  | _ => "none"
  end
end
let read: String -> std::result::Result Nat std::json::Error = std::json::decode
let large = match read "9007199254740993" with
| #Some value => std::str::from_nat value
| #Error _ => "refused"
end
"#,
        true,
        Some(64),
    );
    assert_eq!(json(&program, "nat_domain"), "\"18446744073709551615 64\"");
    assert_eq!(json(&program, "int_domain"), "\"-9223372036854775808\"");
    assert_eq!(json(&program, "large"), "\"9007199254740993\"");
}

#[test]
fn the_positional_binary_format_round_trips() {
    let source = r#"
let bytes = match std::binary::encode { name: "ruddy", count: 3n } with
| #Some written => written
| #Error _ => []
end
let width = std::array::len bytes
let read: [Nat8] -> std::result::Result { name: String, count: Nat } std::binary::Error =
  std::binary::decode
let restored = match read bytes with
| #Some value => std::str::concat value.name (std::str::from_nat value.count)
| #Error _ => "error"
end
let truncated = match std::array::slice bytes 0n 3n with
| #Some short => match read short with
  | #Some _ => "accepted"
  | #Error _ => "refused"
  end
| #None => "missing"
end
"#;
    let exports = ["width", "restored", "truncated"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(interpreted, vec!["17", "\"ruddy3\"", "\"refused\""]);
}

#[test]
fn values_cross_the_foreign_boundary_in_both_directions() {
    let source = r#"
let write: { name: String, counts: [Nat] } -> std::result::Result ForeignValue std::ffi::DecodeError =
  std::ffi::encode
let read: ForeignValue -> std::result::Result { name: String, counts: [Nat] } std::ffi::DecodeError =
  std::ffi::decode
let round_trip = match write { name: "ruddy", counts: [1n, 2n] } with
| #Some raw => match read raw with
  | #Some value => std::str::concat value.name (std::str::from_nat (std::array::len value.counts))
  | #Error error => error.message
  end
| #Error error => error.message
end
let functions_need_an_adapter = do
  let bad: (Nat -> Nat) -> std::result::Result ForeignValue std::ffi::DecodeError =
    std::ffi::encode
  return match bad (fn value => value) with
  | #Some _ => "written"
  | #Error error => error.expected
  end
end
let carries_unit_and_cases = do
  let write: { flag: (), choice: #Yes () | #No Nat } -> std::result::Result ForeignValue std::ffi::DecodeError =
    std::ffi::encode
  let read: ForeignValue -> std::result::Result { flag: (), choice: #Yes () | #No Nat } std::ffi::DecodeError =
    std::ffi::decode
  let round = fn value => match write value with
  | #Some raw => match read raw with
    | #Some back => match back.choice with
      | #Yes _ => "yes"
      | #No count => std::str::concat "no" (std::str::from_nat count)
      end
    | #Error error => error.message
    end
  | #Error error => error.message
  end
  return std::str::concat
    (round { flag: (), choice: #Yes () })
    (round { flag: (), choice: #No 7n })
end
let a_wrong_shape_says_where = do
  let narrow: ForeignValue -> std::result::Result { name: Nat } std::ffi::DecodeError =
    std::ffi::decode
  return match write { name: "ruddy", counts: [] } with
  | #Some raw => match narrow raw with
    | #Some _ => "read"
    | #Error error => error.path
    end
  | #Error error => error.path
  end
end
"#;
    let exports = [
        "round_trip",
        "functions_need_an_adapter",
        "carries_unit_and_cases",
        "a_wrong_shape_says_where",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"ruddy2\"",
            "\"a verifiable data type; a function contract needs an adapter\"",
            "\"yesno7\"",
            "\"$.name\"",
        ]
    );
}

#[test]
fn the_array_and_text_libraries_agree_on_both_backends() {
    let source = r#"
let numbers = [5n, 3n, 9n, 1n, 7n]
let total = std::str::from_nat (std::array::fold std::nat::add 0n numbers)
let sorted = std::str::join "," (std::array::map std::str::from_nat (std::array::sort_by std::nat::compare numbers))
let sliced = match std::array::slice numbers 1n 4n with
| #Some part => std::str::join "-" (std::array::map std::str::from_nat part)
| #None => "none"
end
let last = match std::array::pop numbers with
| #Some pair => std::str::from_nat pair.0
| #None => "none"
end
let joined = std::str::join "" (std::array::reverse ["c", "b", "a"])
let astral = do
  let text = "a😀b"
  return std::str::join "," [
    std::str::from_nat (std::str::len text),
    std::str::char_at text 1n,
    std::str::slice text 1n 2n,
    std::str::reverse text,
    std::str::from_int (std::str::index_of text "b"),
  ]
end
let padded = std::str::join "|" [
  std::str::pad_start "7" 3n "0",
  std::str::pad_end "7" 3n "·",
  std::str::to_uppercase "straße",
]
let ordered = std::str::join "," [
  std::str::from_boolean (std::str::less_than "a" "😀"),
  std::str::from_boolean (std::str::less_than "b" "a"),
]
"#;
    let exports = [
        "total", "sorted", "sliced", "last", "joined", "astral", "padded", "ordered",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"25\"",
            "\"1,3,5,7,9\"",
            "\"3-9-1\"",
            "\"7\"",
            "\"abc\"",
            "\"3,😀,😀,b😀a,2\"",
            "\"007|7··|STRASSE\"",
            "\"true,false\"",
        ]
    );
}

/// `std::abi` is pure data and pure arithmetic, so the reference interpreter
/// runs it unchanged and must reach the same verdict as the JavaScript backend
/// on the same plan. Neither backend calls anything: a validated plan is a
/// reading of the plan's own numbers.
#[test]
fn abi_plans_validate_the_same_way_on_both_backends() {
    let source = r#"
let owner: std::abi::Ownership = {
  allocated_by: #Caller,
  freed_by: #Some (#Caller),
  free_with: #Some "free",
  lifetime: #Call,
}
let byte: std::abi::Type = #Scalar (#Integer { bits: 8n, signed: false })
let write_all: std::abi::Type -> std::abi::Plan = fn buffer => {
  name: "write_all",
  target: { convention: #C, address_bits: 64n, scalar_alignment: #None },
  parameters: [
    { name: "buffer", shape: buffer },
    { name: "len", shape: #Scalar (#Integer { bits: 64n, signed: false }) },
  ],
  result: #None,
  obligations: [
    { duty: #Validity, note: "the caller passes a readable region of len bytes" },
    { duty: #Lifetime, note: "the region stays readable until the call returns" },
    { duty: #Provenance, note: "the region comes from malloc and goes back to free" },
  ],
}
let stated = write_all
  (#Pointer { pointee: byte, ownership: #Some owner, length: #Some (#Parameter "len"), nullable: false })
let unstated = write_all (#Pointer { pointee: byte, ownership: #None, length: #None, nullable: false })
let accepted = match std::abi::validate stated with
| #Some _ => true
| #Error _ => false
end
let accepted_faults = std::str::join " " (std::abi::report stated)
let refused_faults = std::str::join " " (std::abi::report unstated)
"#;
    let exports = ["accepted", "accepted_faults", "refused_faults"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "true",
            "\"\"",
            r#""the parameter \"buffer\" states no owner: a plan must say which side allocates the region and which side frees it. the parameter \"buffer\" states no length: a plan must say how many elements travel with the pointer, because no type says it.""#,
        ]
    );
}

/// The data layer under `std::js` is the two checked-conversion intrinsics,
/// and those are portable, so both backends must agree about them. `std::js`
/// itself is not portable: reading an arbitrary host value runs host code,
/// and the interpreter provides no JavaScript host, so those readings are
/// covered against a real host in `tests/src/js_adapters.rs` instead.
#[test]
fn the_checked_conversion_intrinsics_agree_across_backends() {
    let source = r#"
type Role = #Admin | #User Nat
type Person = { name: String, roles: [Role] }
let write: Person -> std::result::Result ForeignValue std::ffi::DecodeError = std::ffi::encode
let read: ForeignValue -> std::result::Result Person std::ffi::DecodeError = std::ffi::decode
let round_trip = match write { name: "ruddy", roles: [#Admin, #User 2n] } with
| #Some raw => match read raw with
  | #Some person => std::str::concat person.name (std::str::from_nat (std::array::len person.roles))
  | #Error error => error.message
  end
| #Error error => error.message
end
let a_callable_is_refused = do
  let bad: (Nat -> Nat) -> std::result::Result ForeignValue std::ffi::DecodeError =
    std::ffi::encode
  return match bad (fn value => value) with
  | #Some _ => "written"
  | #Error error => error.expected
  end
end
"#;
    let exports = ["round_trip", "a_callable_is_refused"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"ruddy2\"",
            "\"a verifiable data type; a function contract needs an adapter\"",
        ]
    );
}

#[test]
fn the_backends_agree_on_zero_underflow_and_effect_order() {
    let source = r#"
effect A = { a: () -> Nat }
effect B = { b: () -> Nat }

-- An integer has one zero, however it was reached.
let zeroes = std::str::join "," [
  std::str::from_int (std::real::truncate (-0.5)),
  std::str::from_int (std::real::ceil (-0.5)),
  std::str::from_int (std::real::round (-0.2)),
  std::str::from_int (std::real::floor 0.5),
]
let read_int: String -> std::result::Result Int std::json::Error = std::json::decode
let read_real: String -> std::result::Result Real std::json::Error = std::json::decode
let negative_zero = match read_int "-0" with
| #Some value => std::str::from_int value
| #Error _ => "refused"
end

-- A nonzero number that underflows to zero is refused, whichever zero it
-- reached; a real number's own zero keeps its sign.
let underflow = std::str::join "," [
  match read_real "1e-400" with | #Some _ => "read" | #Error _ => "refused" end,
  match read_real "-1e-400" with | #Some _ => "read" | #Error _ => "refused" end,
  match read_real "-0.0" with
  | #Some value => match std::json::encode value with
    | #Some text => text
    | #Error _ => "unwritable"
    end
  | #Error _ => "refused"
  end,
]

-- A row is unordered, so two arrows written with the same effects in
-- different orders are one type.
let both_ways: Mirror (Nat -> Nat + !A + !B) = std::reflect::mirror ()
let other_way: Mirror (Nat -> Nat + !B + !A) = std::reflect::mirror ()
let pure_way: Mirror (Nat -> Nat) = std::reflect::mirror ()
let reordered = match std::reflect::same both_ways other_way with
| #Some _ => "same"
| #None => "different"
end
let effects_count = match std::reflect::same both_ways pure_way with
| #Some _ => "same"
| #None => "different"
end
"#;
    let exports = [
        "zeroes",
        "negative_zero",
        "underflow",
        "reordered",
        "effects_count",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"0,0,0,0\"",
            "\"0\"",
            "\"refused,refused,-0.0\"",
            "\"same\"",
            "\"different\"",
        ]
    );
}

/// A handle a foreign boundary hands back is authentic, which says it came
/// from this program and nothing about which type it stands for. Reading one
/// under another type would fabricate a cast, so the type is checked too.
#[test]
fn a_handle_is_read_only_under_the_type_it_was_made_at() {
    let source = r#"
type Box 'v = hide 'a => { secret: 'a, visible: 'v }

let natural: Mirror Nat = std::reflect::mirror ()
let write_mirror: Mirror Nat -> std::result::Result ForeignValue std::ffi::DecodeError =
  std::ffi::encode
let read_text_mirror: ForeignValue -> std::result::Result (Mirror String) std::ffi::DecodeError =
  std::ffi::decode
let read_natural_mirror: ForeignValue -> std::result::Result (Mirror Nat) std::ffi::DecodeError =
  std::ffi::decode

let crossed = match write_mirror natural with
| #Some raw => match read_text_mirror raw with
  | #Some other => match std::reflect::same natural other with
    | #Some _ => "cast"
    | #None => "unequal"
    end
  | #Error _ => "rejected"
  end
| #Error _ => "unwritable"
end
let kept_mirror = match write_mirror natural with
| #Some raw => match read_natural_mirror raw with
  | #Some other => match std::reflect::same natural other with
    | #Some _ => "same"
    | #None => "unequal"
    end
  | #Error _ => "rejected"
  end
| #Error _ => "unwritable"
end

let box: Box Nat = { secret: true, visible: 42n }
let write_box: Box Nat -> std::result::Result ForeignValue std::ffi::DecodeError =
  std::ffi::encode
let read_text_box: ForeignValue -> std::result::Result (Box String) std::ffi::DecodeError =
  std::ffi::decode
let read_natural_box: ForeignValue -> std::result::Result (Box Nat) std::ffi::DecodeError =
  std::ffi::decode

let opened = match write_box box with
| #Some raw => match read_text_box raw with
  | #Some package => match package with
    | hide 'a { visible, .. } => visible
    end
  | #Error _ => "rejected"
  end
| #Error _ => "unwritable"
end
let kept_box = match write_box box with
| #Some raw => match read_natural_box raw with
  | #Some package => match package with
    | hide 'a { visible, .. } => std::str::from_nat visible
    end
  | #Error _ => "rejected"
  end
| #Error _ => "unwritable"
end

-- The binder's spelling is no part of the type, so this reads the same
-- package, and an alias of the same shape reads it too.
type Renamed 'v = hide 'b => { secret: 'b, visible: 'v }
type Alias = Renamed Nat
let read_renamed: ForeignValue -> std::result::Result Alias std::ffi::DecodeError =
  std::ffi::decode
let renamed = match write_box box with
| #Some raw => match read_renamed raw with
  | #Some package => match package with
    | hide 'a { visible, .. } => std::str::from_nat visible
    end
  | #Error _ => "rejected"
  end
| #Error _ => "unwritable"
end

-- A nested record of mirrors carries each one's own type.
let pair: { left: Mirror Nat, right: Mirror String } = { left: natural, right: std::reflect::mirror () }
let write_pair: { left: Mirror Nat, right: Mirror String }
  -> std::result::Result ForeignValue std::ffi::DecodeError = std::ffi::encode
let read_swapped: ForeignValue
  -> std::result::Result { left: Mirror String, right: Mirror Nat } std::ffi::DecodeError =
  std::ffi::decode
let swapped = match write_pair pair with
| #Some raw => match read_swapped raw with
  | #Some _ => "read"
  | #Error error => error.path
  end
| #Error _ => "unwritable"
end
"#;
    let exports = [
        "crossed",
        "kept_mirror",
        "opened",
        "kept_box",
        "renamed",
        "swapped",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"rejected\"",
            "\"same\"",
            "\"rejected\"",
            "\"42\"",
            "\"42\"",
            "\"$.left\"",
        ]
    );
}

/// A type whose recursion runs through a hidden binder still has a finite
/// graph. Re-entering the binder used to give every turn of the recursion a
/// descriptor of its own, and construction never finished.
#[test]
fn recursion_through_a_hidden_binder_has_a_finite_graph() {
    let source = r#"
type Loop = hide 'a => { value: 'a, next: std::option::Option Loop }
type Renamed = hide 'z => { value: 'z, next: std::option::Option Renamed }
type Other = hide 'a => { value: 'a, next: std::option::Option Nat }
type Nested = hide 'a => hide 'b => { outer: 'a, inner: 'b, next: std::option::Option Nested }
type Both = hide 'a => {
  here: 'a,
  deeper: hide 'b => { there: 'b, back: 'a, next: std::option::Option Both },
}
type Plain = { head: Nat, tail: std::option::Option Plain }
type Even = hide 'a => { value: 'a, odd: std::option::Option Odd }
type Odd = hide 'b => { value: 'b, even: std::option::Option Even }

@private
let of_loop: Mirror Loop = std::reflect::mirror ()
@private
let of_renamed: Mirror Renamed = std::reflect::mirror ()
@private
let of_other: Mirror Other = std::reflect::mirror ()
@private
let of_nested: Mirror Nested = std::reflect::mirror ()
@private
let of_both: Mirror Both = std::reflect::mirror ()
@private
let of_plain: Mirror Plain = std::reflect::mirror ()
@private
let of_even: Mirror Even = std::reflect::mirror ()
@private
let of_odd: Mirror Odd = std::reflect::mirror ()

@private
let size: Mirror 'a -> String = fn of =>
  std::str::from_nat (std::array::len (std::reflect::describe of).nodes)
let sizes = std::str::join "," [
  size of_loop,
  size of_nested,
  size of_both,
  size of_plain,
  size of_even,
  size of_odd,
]

@private
let decided: Mirror 'x -> Mirror 'y -> String = fn left right =>
  match std::reflect::same left right with
  | #Some _ => "same"
  | #None => "different"
  end
let answers = std::str::join "," [
  decided of_loop of_renamed,
  decided of_loop of_loop,
  decided of_loop of_other,
  decided of_even of_odd,
]
"#;
    let exports = ["sizes", "answers"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    // Finite, and small: a graph that grew per turn of the recursion would
    // not get here at all.
    assert_eq!(
        interpreted,
        vec!["\"6,9,13,4,16,16\"", "\"same,same,different,different\"",]
    );
}

/// Replacement puts the replacement in as written and keeps the result
/// valid scalar text. JavaScript's own methods do neither: they expand `$&`
/// and its relatives, and with an empty search they insert between the two
/// halves of an astral character.
#[test]
fn replacement_is_literal_and_lands_on_scalar_boundaries() {
    let source = r#"
let show: String -> String = fn text =>
  std::str::concat text (std::str::concat "/" (std::str::from_nat (std::str::len text)))
let results = std::str::join "," [
  show (std::str::replace_all "😀" "" "."),
  show (std::str::replace_first "😀" "" "."),
  show (std::str::replace_first "ab" "a" "$&"),
  show (std::str::replace_all "aba" "a" "$&"),
  show (std::str::replace_all "ab" "b" "$$"),
  show (std::str::replace_all "ab" "a" "$`"),
  show (std::str::replace_all "aaa" "aa" "-"),
  show (std::str::replace_all "abab" "ab" "x"),
  show (std::str::replace_first "abab" "ab" "x"),
  show (std::str::replace_all "abc" "z" "-"),
  show (std::str::replace_first "abc" "z" "-"),
  show (std::str::replace_all "" "" "-"),
  show (std::str::replace_first "" "" "-"),
  show (std::str::replace_all "" "a" "-"),
  show (std::str::replace_all "a😀b" "" "|"),
]
"#;
    let exports = ["results"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![concat!(
            "\"",
            ".😀./3,",
            ".😀/2,",
            "$&b/3,",
            "$&b$&/5,",
            "a$$/3,",
            "$`b/3,",
            "-a/2,",
            "xx/2,",
            "xab/3,",
            "abc/3,",
            "abc/3,",
            "-/1,",
            "-/1,",
            "/0,",
            "|a|😀|b|/7",
            "\""
        )]
    );
}

/// A sequence says its length before its elements, so a writer that emits
/// fewer or more than it declared has not written the value it promised, and
/// the runner says so instead of reporting success.
#[test]
fn a_writer_meets_the_length_it_declared() {
    let source = r#"
@private
let of_length: Nat -> [Nat] -> std::codec::Encoder () = fn declared written => {
  schema: std::reflect::describe (std::reflect::type_of [1n]),
  run: fn _ => std::result::and_then
    (fn cursor => std::result::and_then
      (fn _ => std::codec::!Write.end_sequence cursor)
      (std::array::try_fold_result
        (fn _ element => std::result::and_then
          (fn _ => std::codec::!Write.write_nat element)
          (std::codec::!Write.next_element cursor))
        ()
        written))
    (std::codec::!Write.begin_sequence declared),
}
@private
let as_binary: std::codec::Encoder () -> String = fn encoder =>
  match std::binary::encode_with encoder std::binary::default_limits () with
  | #Some bytes => std::str::from_nat (std::array::len bytes)
  | #Error _ => "refused"
  end
@private
let as_json: std::codec::Encoder () -> String = fn encoder =>
  match std::json::encode_with encoder std::json::default_limits () with
  | #Some out => out
  | #Error _ => "refused"
  end
let binary_lengths = std::str::join "," [
  as_binary (of_length 2n []),
  as_binary (of_length 1n [0n, 1n]),
  as_binary (of_length 1n [0n]),
  as_binary (of_length 0n []),
  as_binary (of_length 3n [0n, 1n, 2n]),
]
let json_lengths = std::str::join "," [
  as_json (of_length 2n []),
  as_json (of_length 1n [0n, 1n]),
  as_json (of_length 1n [0n]),
  as_json (of_length 0n []),
  as_json (of_length 3n [0n, 1n, 2n]),
]
-- A count past what the binary header holds is refused before the header is
-- written, rather than silently truncated to thirty-two bits.
let too_long = as_binary (of_length 4294967296n [])
"#;
    let exports = ["binary_lengths", "json_lengths", "too_long"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"refused,refused,12,4,28\"",
            "\"refused,refused,[0],[],[0,1,2]\"",
            "\"refused\"",
        ]
    );
}

/// The limits a codec advertises are counted before the material that would
/// exceed them is built, and each one is checked on its own.
#[test]
fn codec_limits_are_counted_before_the_work() {
    let source = r#"
@private
let numbers: [Nat] = [1n, 2n]
@private
let writer: std::codec::Encoder [Nat] = match
  std::codec::derive_encoder (std::reflect::type_of numbers) with
| #Some encoder => encoder
| #Error _ => { schema: std::reflect::describe (std::reflect::type_of numbers), run: fn _ => #Some () }
end
@private
let reader: std::codec::Decoder [Nat] = match
  std::codec::derive_decoder (std::reflect::mirror ()) with
| #Some decoder => decoder
| #Error _ => { schema: std::reflect::describe (std::reflect::type_of numbers), run: fn _ => #Some [] }
end
@private
let as_text: Nat -> Nat -> Nat -> String = fn bytes members digits =>
  match std::json::encode_with
    writer
    { depth: 10n, bytes: bytes, members: members, digits: digits }
    numbers with
  | #Some out => out
  | #Error _ => "refused"
  end
@private
let as_bytes: Nat -> Nat -> String = fn bytes members =>
  match std::binary::encode_with writer { depth: 10n, bytes: bytes, members: members } numbers with
  | #Some out => std::str::from_nat (std::array::len out)
  | #Error _ => "refused"
  end
@private
let written: [Nat8] = match std::binary::encode_with writer std::binary::default_limits numbers with
| #Some out => out
| #Error _ => []
end
@private
let read_within: Nat -> String = fn bytes =>
  match std::binary::decode_with reader { depth: 10n, bytes: bytes, members: 10n } written with
  | #Some _ => "read"
  | #Error _ => "refused"
  end

-- Output bytes: `[1,2]` is five, and four is one short.
let json_bytes = std::str::join "," [as_text 5n 9n 9n, as_text 4n 9n 9n, as_text 0n 9n 9n]
-- Members: two elements need two.
let json_members = std::str::join "," [as_text 99n 2n 9n, as_text 99n 1n 9n, as_text 99n 0n 9n]
-- The binary document is twenty bytes, and nineteen is one short.
let binary_bytes = std::str::join "," [as_bytes 20n 9n, as_bytes 19n 9n, as_bytes 0n 9n]
let binary_members = std::str::join "," [as_bytes 99n 2n, as_bytes 99n 1n]
-- Input bytes, measured before any of the document is read.
let binary_input = std::str::join "," [read_within 20n, read_within 19n, read_within 0n]
-- The byte budget counts bytes, and this text is three scalars in six of them.
@private
let parsed: Nat -> String = fn bytes =>
  match std::json::parse_with
    { depth: 10n, bytes: bytes, members: 10n, digits: 10n }
    "\"😀\"" with
  | #Some _ => "read"
  | #Error _ => "refused"
  end
let json_input = std::str::join "," [parsed 6n, parsed 5n, parsed 3n]
"#;
    let exports = [
        "json_bytes",
        "json_members",
        "binary_bytes",
        "binary_members",
        "binary_input",
        "json_input",
    ];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"[1,2],refused,refused\"",
            "\"[1,2],refused,refused\"",
            "\"20,refused,refused\"",
            "\"20,refused\"",
            "\"read,refused,refused\"",
            "\"read,refused,refused\"",
        ]
    );
}

#[test]
fn a_host_value_the_interpreter_does_not_provide_is_named() {
    let mut program = load(
        r#"
let parse = fn text => match std::url::parse text with
| #Some url => url.host
| #Error error => error.message
end
"#,
        true,
        None,
    );
    let parse = program.export("parse").unwrap();
    let error = program
        .call(&parse, Value::string("https://example.com/path"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsupported: this interpreter does not provide the host value `$url.parse`"
    );
}

#[test]
fn an_artifact_that_names_an_unknown_definition_is_refused() {
    let project = project("let value = 1n\nlet other = value", false, None, false);
    let artifact = ruddy_cli::compile(project.path()).expect("the project compiles");
    let mut broken = artifact.to_unchecked();
    for block in broken
        .lir
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
    {
        for instr in &mut block.instrs {
            if let ruddy::artifact::Op::Global { target, .. } = &mut instr.op {
                *target = "nowhere@0.0.0::missing".to_owned();
            }
        }
    }
    let broken = broken
        .validate()
        .expect("a global name is not part of the executable invariant");
    let error = Program::load(&broken).map(|_| ()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid artifact: no definition named `nowhere@0.0.0::missing`"
    );
}

/// What the command line does with an artifact on disk: list what a bundle
/// exports, or render the exports it is asked for.
#[test]
fn the_command_line_reads_an_artifact_from_disk() {
    let project = project(
        "let name = \"ruddy\"\nlet count = 3n\nlet missing = ()\n",
        false,
        None,
        false,
    );
    let artifact = ruddy_cli::build_project(project.path()).expect("the project builds");
    let path = artifact.to_str().unwrap().to_owned();

    assert_eq!(
        ruddy_interp::run(std::slice::from_ref(&path)).unwrap(),
        ["count", "missing", "name"]
    );
    assert_eq!(
        ruddy_interp::run(&[path.clone(), "name".into(), "count".into()]).unwrap(),
        ["\"ruddy\"", "3"]
    );
    assert_eq!(
        ruddy_interp::run(&[path.clone(), "absent".into()])
            .unwrap_err()
            .to_string(),
        "no exported value at `absent`"
    );
    assert_eq!(
        ruddy_interp::run(&[]).unwrap_err().to_string(),
        "usage: ruddy-interp <artifact> [export...]"
    );

    let missing = project.path().join("nowhere.artifact");
    let error = ruddy_interp::run(&[missing.to_str().unwrap().into()])
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("could not read "), "{error}");

    let broken = project.path().join("broken.artifact");
    fs::write(&broken, "(artifact").unwrap();
    let error = ruddy_interp::run(&[broken.to_str().unwrap().into()])
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("invalid artifact: "), "{error}");

    let empty = project.path().join("empty.artifact");
    fs::write(&empty, "(artifact (header) (lir))").unwrap();
    let error = ruddy_interp::run(&[empty.to_str().unwrap().into()])
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("invalid artifact: "), "{error}");
}

#[test]
fn a_program_that_is_not_linked_is_refused() {
    let project = project("let value = 1n", false, None, false);
    let artifact = ruddy_cli::compile(project.path()).expect("the project compiles");
    let mut unlinked = artifact.to_unchecked();
    unlinked
        .header
        .dependencies
        .push(ruddy::artifact::Dependency {
            name: "other".into(),
            version: "0.1.0".into(),
        });
    let unlinked = unlinked.validate().expect("a dependency is valid data");
    assert_eq!(
        Program::load(&unlinked)
            .map(|_| ())
            .unwrap_err()
            .to_string(),
        "the interpreter requires a linked artifact"
    );
}

#[test]
fn persistent_map_insert_lookup_and_remove_preserve_old_versions() {
    let (node, interpreted) = both(
        r#"
@private
let initial: std::map::Map String Nat = std::map::empty ()
@private
let first = std::map::insert "a" 1n initial
@private
let second = std::map::insert "b" 2n first
@private
let replaced = std::map::insert "a" 3n second
@private
let removed = std::map::remove "a" replaced
let sizes = [std::map::len initial, std::map::len first, std::map::len second, std::map::len replaced, std::map::len removed]
let before = std::map::get "a" first == #Some 1n
let after = std::map::get "a" replaced == #Some 3n
let kept = std::map::get "b" removed == #Some 2n
let absent = std::map::get "a" removed == #None
let missing = std::map::try_get "missing" removed == #Some #None
let unchanged = std::map::len (std::map::remove "missing" removed) == 1n
let empty = std::map::is_empty initial and not std::map::contains "a" initial
"#,
        &[
            "sizes",
            "before",
            "after",
            "kept",
            "absent",
            "missing",
            "unchanged",
            "empty",
        ],
        None,
    );
    assert_eq!(
        node,
        [
            "[0,1,2,2,1]",
            "true",
            "true",
            "true",
            "true",
            "true",
            "true",
            "true"
        ]
    );
    assert_eq!(interpreted, node);
}

#[test]
fn persistent_map_forgiving_and_strict_operations_agree_on_invalid_keys() {
    let (node, interpreted) = both(
        r#"
@private
let nan = 0 / 0
@private
let entries = [(1.0, "one"), (nan, "bad"), (2.0, "two"), (1.0, "last")]
@private
let map = std::map::from_array entries
let retained = std::map::len map == 2n and std::map::get 1.0 map == #Some "last" and std::map::get 2.0 map == #Some "two"
let absent = std::map::get nan map == #None and not std::map::contains nan map
let unchanged = std::map::len (std::map::insert nan "bad" map) == 2n and std::map::len (std::map::remove nan map) == 2n
let strict = [
  std::result::is_error (std::map::try_get nan map),
  std::result::is_error (std::map::try_contains nan map),
  std::result::is_error (std::map::try_insert nan "bad" map),
  std::result::is_error (std::map::try_remove nan map),
]
let path = match std::map::try_from_array [({ point: 0.0 }, 1n), ({ point: nan }, 2n)] with
| #Error { path: [#Index 1n, #Field "point"], kind: #NaN } => true
| _ => false
end
let valid = match std::map::try_from_array [(1n, 2n), (1n, 3n)] with
| #Some result => std::map::get 1n result == #Some 3n and std::map::len result == 1n
| #Error _ => false
end
@private
let option_map: std::map::Map String (Option Nat) = std::map::insert "present" (#None) (std::map::empty ())
let option = std::map::try_get "present" option_map == #Some (#Some (#None))
@private
let callback: Nat -> Nat = fn value => std::nat::add value 1n
@private
let functions = std::map::insert "callback" callback (std::map::empty ())
let value = match std::map::get "callback" functions with | #Some f => f 4n == 5n | #None => false end
"#,
        &[
            "retained",
            "absent",
            "unchanged",
            "strict",
            "path",
            "valid",
            "option",
            "value",
        ],
        None,
    );
    assert_eq!(
        node,
        [
            "true",
            "true",
            "true",
            "[true,true,true,true]",
            "true",
            "true",
            "true",
            "true"
        ]
    );
    assert_eq!(interpreted, node);
}

#[test]
fn persistent_map_traversal_transforms_and_equality_use_contents() {
    let (node, interpreted) = both(
        r#"
@private
let original = std::map::from_array [(928n, 1n), (1089n, 2n), (1180n, 3n), (7n, 4n)]
@private
let reordered = std::map::from_array [(7n, 4n), (1180n, 3n), (1089n, 2n), (928n, 1n)]
let equal = std::map::equal_by std::nat::equal original reordered
let different = not std::map::equal_by std::nat::equal original (std::map::insert 928n 9n reordered)
let keys = std::array::sort_by std::nat::compare (std::map::keys original)
let values = std::array::sort_by std::nat::compare (std::map::values original)
let roundtrip = std::map::equal_by std::nat::equal original (std::map::from_array (std::map::to_array original))
@private
let run = fn _ => do
  let calls = mut 0n
  let changed = std::map::map_values (fn value => do
    _ = calls := std::nat::add (~calls) 1n
    return std::str::from_nat value
  end) original
  let total = std::map::fold (fn total key value => do
    _ = calls := std::nat::add (~calls) 1n
    return std::nat::add total value
  end) 0n original
  return [~calls == 8n, total == 10n, std::map::get 1180n changed == #Some "3"]
end
let effects = run ()
@private
let pruned = std::map::remove 7n (std::map::remove 928n original)
let pruning = std::map::get 1089n pruned == #Some 2n and std::map::get 1180n pruned == #Some 3n
let emptied = std::map::is_empty (std::map::remove 1180n (std::map::remove 1089n pruned))
"#,
        &[
            "equal",
            "different",
            "keys",
            "values",
            "roundtrip",
            "effects",
            "pruning",
            "emptied",
        ],
        None,
    );
    assert_eq!(
        node,
        [
            "true",
            "true",
            "[7,928,1089,1180]",
            "[1,2,3,4]",
            "true",
            "[true,true,true]",
            "true",
            "true"
        ]
    );
    assert_eq!(interpreted, node);
}

#[test]
fn persistent_set_operations_preserve_versions_and_offer_strict_errors() {
    let (node, interpreted) = both(
        r#"
@private
let nan = 0 / 0
@private
let left = std::set::from_array [1.0, 2.0, nan, 2.0, -0.0, 0.0]
@private
let right = std::set::from_array [2.0, 3.0]
let sizes = [std::set::len left, std::set::len (std::set::insert 2.0 left), std::set::len (std::set::insert nan left), std::set::len (std::set::remove nan left)]
let absent = not std::set::contains nan left and not std::set::contains 3.0 left
let removed = not std::set::contains 1.0 (std::set::remove 1.0 left) and std::set::contains 1.0 left
let strict = [
  std::result::is_error (std::set::try_contains nan left),
  std::result::is_error (std::set::try_insert nan left),
  std::result::is_error (std::set::try_remove nan left),
  std::set::try_contains 3.0 left == #Some false,
]
let path = match std::set::try_from_array [{ point: 0.0 }, { point: nan }] with
| #Error { path: [#Index 1n, #Field "point"], kind: #NaN } => true
| _ => false
end
let valid = match std::set::try_from_array [1n, 1n, 2n] with
| #Some set => std::set::len set == 2n
| #Error _ => false
end
let union = std::set::equal (std::set::union left right) (std::set::from_array [3.0, 2.0, 1.0, 0.0])
let intersection = std::set::equal (std::set::intersection left right) (std::set::from_array [2.0])
let difference = std::set::equal (std::set::difference left right) (std::set::from_array [1.0, 0.0])
let subset = std::set::is_subset (std::set::from_array [2.0]) left and not std::set::is_subset right left
let empty = std::set::is_empty (std::set::difference left left)
let array = std::array::sort_by std::real::total_compare (std::set::to_array right)
let folded = std::set::fold (fn total value => total + value) 0.0 right == 5.0
"#,
        &[
            "sizes",
            "absent",
            "removed",
            "strict",
            "path",
            "valid",
            "union",
            "intersection",
            "difference",
            "subset",
            "empty",
            "array",
            "folded",
        ],
        None,
    );
    assert_eq!(
        node,
        [
            "[3,3,3,3]",
            "true",
            "true",
            "[true,true,true,true]",
            "true",
            "true",
            "true",
            "true",
            "true",
            "true",
            "true",
            "[2,3]",
            "true"
        ]
    );
    assert_eq!(interpreted, node);
}

#[test]
fn persistent_map_mixed_updates_match_a_reference_map() {
    let mut model = std::collections::BTreeMap::new();
    for key in 0u64..32 {
        model.insert(key, key + 1);
    }
    // Exercise occupied and empty bitmap slots, replacement, deletion, and
    // reinsertion while retaining the original collection as a snapshot.
    for step in 0u64..64 {
        let key = (step * 73 + 19) % 48;
        if step % 3 == 0 {
            model.remove(&key);
        } else {
            model.insert(key, step);
        }
    }
    let source = r#"
@private
let range: Nat -> [Nat] = fn count =>
  if std::nat::is_zero count then []
  else [..range (std::nat::subtract count 1n), std::nat::subtract count 1n]
  end
@private
let original = std::map::from_array (std::array::map (fn key => (key, std::nat::add key 1n)) (range 32n))
@private
let updated = std::array::fold (fn map step => do
  let key = std::nat::remainder (std::nat::add (std::nat::multiply step 73n) 19n) 48n
  return if std::nat::is_zero (std::nat::remainder step 3n) then std::map::remove key map
  else std::map::insert key step map
  end
end) original (range 64n)
let keys = std::array::sort_by std::nat::compare (std::map::keys updated)
let values = std::array::map (fn key => std::option::unwrap_or 999n (std::map::get key updated)) keys
let size = std::map::len updated
let snapshot = std::map::len original == 32n and std::map::get 19n original == #Some 20n
let cleared = std::map::is_empty (std::array::fold (fn map key => std::map::remove key map) updated keys)
"#;
    let (node, interpreted) = both(
        source,
        &["keys", "values", "size", "snapshot", "cleared"],
        Some(32),
    );
    assert_eq!(
        node,
        [
            serde_json::to_string(&model.keys().collect::<Vec<_>>()).unwrap(),
            serde_json::to_string(&model.values().collect::<Vec<_>>()).unwrap(),
            model.len().to_string(),
            "true".to_owned(),
            "true".to_owned(),
        ]
    );
    assert_eq!(interpreted, node);
}

#[test]
fn persistent_collections_resolve_full_hash_collisions_by_key_equality() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/hamt-collision.json")).unwrap();
    let source = format!(
        r#"
@private
let a = {left}
@private
let b = {right}
let collision = a != b and std::hash::hash a == #Some {hash}n64 and std::hash::hash b == #Some {hash}n64
@private
let original = std::map::from_array [(a, 1n), (b, 2n)]
@private
let reversed = std::map::from_array [(b, 2n), (a, 1n)]
@private
let updated = std::map::insert b 3n original
@private
let removed_a = std::map::remove a updated
@private
let removed_b = std::map::remove b updated
let lookup = std::map::get a original == #Some 1n and std::map::get b original == #Some 2n and std::map::len original == 2n
let replaced = std::map::get b updated == #Some 3n and std::map::len updated == 2n
let removal = std::map::get a removed_a == #None and std::map::get b removed_a == #Some 3n and std::map::get a removed_b == #Some 1n and std::map::get b removed_b == #None
let equal = std::map::equal_by std::nat::equal original reversed
@private
let mixed = std::map::insert "other" 4n original
let branch = std::map::get b (std::map::remove "other" mixed) == #Some 2n
let sets = std::set::equal (std::set::union (std::set::from_array [a]) (std::set::from_array [b])) (std::set::from_array [b, a])
let difference = std::set::equal (std::set::difference (std::set::from_array [a, b]) (std::set::from_array [a])) (std::set::from_array [b])
"#,
        left = fixture["left"],
        right = fixture["right"],
        hash = fixture["hash"].as_str().unwrap(),
    );
    let (node, interpreted) = both(
        &source,
        &[
            "collision",
            "lookup",
            "replaced",
            "removal",
            "equal",
            "branch",
            "sets",
            "difference",
        ],
        None,
    );
    assert_eq!(node, ["true"; 8]);
    assert_eq!(interpreted, node);
}

#[test]
fn structural_hashing_is_pure_seeded_and_normalizes_signed_zero() {
    let (node, interpreted) = both(
        r#"
@private
let hash: 'a -> Result Nat64 std::hash::Error + | = fn value => std::hash::hash value
let zeros = hash -0.0 == hash 0.0
let repeat = hash "hé😀" == hash "hé😀"
let supported = std::result::is_ok (hash 18446744073709551615n64)
let explicit = std::hash::hash_with (std::reflect::mirror ()) std::hash::default_seed 42n == hash 42n
let seeded = std::hash::hash_seeded 1n64 "hello" != std::hash::hash_seeded 2n64 "hello"
let nan = match hash (0 / 0) with
| #Error { path: [], kind: #NaN } => true
| _ => false
end
"#,
        &["zeros", "repeat", "supported", "explicit", "seeded", "nan"],
        None,
    );
    assert_eq!(node, ["true"; 6]);
    assert_eq!(interpreted, node);
}

#[test]
fn structural_hashing_frames_records_arrays_and_recursive_variants() {
    let (node, interpreted) = both(
        r#"
@private
let hash = fn value => std::hash::hash value
type Tree = #Leaf Nat | #Branch [Tree]
@private
let tree: Tree = #Branch [#Leaf 1n, #Branch [#Leaf 2n]]
let recursive = std::result::is_ok (hash tree) and hash tree == hash tree
let records = hash { z: [1n, 2n], a: "é" } == hash { a: "é", z: [1n, 2n] }
let zeros = hash { points: [#Point -0.0] } == hash { points: [#Point 0.0] }
let boundaries = hash ["ab", "c"] != hash ["a", "bc"]
let order = hash [1n, 2n] != hash [2n, 1n]
let length = hash [1n] != hash [1n, 0n]
let names = hash { a: 1n } != hash { b: 1n }
let cases = hash (#A 1n) != hash (#B 1n)
@private
let empty_array: [Nat] = []
let empty = std::result::is_ok (hash ()) and std::result::is_ok (hash empty_array)
"#,
        &[
            "recursive",
            "records",
            "zeros",
            "boundaries",
            "order",
            "length",
            "names",
            "cases",
            "empty",
        ],
        None,
    );
    assert_eq!(node, ["true"; 9]);
    assert_eq!(interpreted, node);
}

#[test]
fn structural_hashing_reports_paths_without_observing_opaque_values() {
    let (node, interpreted) = both(
        r#"
@private
let hash = fn value => std::hash::hash value
let nested = match hash { items: [#Reading 1.0, #Reading (0 / 0)] } with
| #Error { path: [#Field "items", #Index 1n, #Case "Reading"], kind: #NaN } => true
| _ => false
end
@private
let callback: Nat -> Nat = fn x => x
let function = match hash { callback: callback } with
| #Error { path: [#Field "callback"], kind: #Unsupported #Function } => true
| _ => false
end
@private
let empty_functions: [Nat -> Nat] = []
type Choice = #Data Nat | #Callback (Nat -> Nat)
@private
let inactive: Choice = #Data 7n
let unvisited = std::result::is_ok (hash empty_functions) and std::result::is_ok (hash inactive)
@private
let run = fn _ => do
  let cell = mut 1n
  let before = hash cell
  _ = cell := 2n
  return match (before, hash cell) with
  | (#Error { path: [], kind: #Unsupported #Foreign }, #Error { path: [], kind: #Unsupported #Foreign }) => true
  | _ => false
  end
end
let cell = run ()
@private
let witness: Mirror Nat = std::reflect::mirror ()
let mirror = match hash witness with
| #Error { path: [], kind: #Unsupported #Mirror } => true
| _ => false
end
type Boxed = hide 'a => { mirror: Mirror 'a, value: 'a }
@private
let boxed: Boxed = { mirror: std::reflect::mirror (), value: 42n }
let hidden = match hash boxed with
| #Error { path: [], kind: #Unsupported #Hidden } => true
| _ => false
end
let opened = match boxed with
| hide 'a { mirror, value } => std::hash::hash_with mirror std::hash::default_seed value == hash 42n
end
let foreign = match std::ffi::encode 1n with
| #Some value => match hash value with
  | #Error { path: [], kind: #Unsupported #Foreign } => true
  | _ => false
  end
| #Error _ => false
end
"#,
        &[
            "nested",
            "function",
            "unvisited",
            "cell",
            "mirror",
            "hidden",
            "opened",
            "foreign",
        ],
        None,
    );
    assert_eq!(node, ["true"; 8]);
    assert_eq!(interpreted, node);
}

#[test]
fn structural_hashing_preserves_fixed_width_values_across_target_domains() {
    let source = r#"
@private
let hash = fn value => std::hash::hash value
@private
let results = [
  hash 0n, hash 4294967295n, hash -2147483648i, hash 2147483647i,
  hash 0n8, hash 255n8, hash 0n16, hash 65535n16,
  hash 0n32, hash 4294967295n32, hash 0n64, hash 18446744073709551615n64,
  hash -128i8, hash 127i8, hash -32768i16, hash 32767i16,
  hash -2147483648i32, hash 2147483647i32,
  hash -9223372036854775808i64, hash 9223372036854775807i64,
  hash false, hash true, hash "", hash "é😀", hash (1 / 0), hash (-1 / 0),
]
let supported = std::array::all std::result::is_ok results
let hashes = std::array::map
  (fn result => std::nat::to_string64 (std::result::unwrap_or 0n64 result)) results
let kinds = hash 1n != hash 1n64 and hash 1i8 != hash 1n8
let extremes = hash -9223372036854775808i64 != hash 9223372036854775807i64
let vectors = hash false == #Some 586857767355016009n64 and hash true == #Some 586856667843387798n64
"#;
    // FNV-1a known answers for the Boolean frames 04 00 and 04 01.
    // Export hashes as decimal text so Node's JSON does not narrow BigInts.
    let exports = ["supported", "hashes", "kinds", "extremes", "vectors"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node[0], "true");
    assert_eq!(&node[2..], ["true"; 3]);
    assert_eq!(interpreted, node);
    let (narrow_node, narrow_interpreted) = both(source, &exports, Some(32));
    assert_eq!(narrow_node, node);
    assert_eq!(narrow_interpreted, node);
}

#[test]
fn real_partial_comparisons_make_nan_unordered_and_signed_zeros_equal() {
    let (node, interpreted) = both(
        r#"
let nan = 0 / 0
let nan_equal = std::real::equal nan nan
let nan_unequal = std::real::not_equal nan nan
let nan_less = std::real::less_than nan 1
let nan_less_equal = std::real::less_than_or_equal nan nan
let nan_greater = std::real::greater_than 1 nan
let nan_greater_equal = std::real::greater_than_or_equal nan nan
let zeros_equal = std::real::equal -0.0 0.0
"#,
        &[
            "nan_equal",
            "nan_unequal",
            "nan_less",
            "nan_less_equal",
            "nan_greater",
            "nan_greater_equal",
            "zeros_equal",
        ],
        None,
    );
    let expected = ["false", "true", "false", "false", "false", "false", "true"];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn comparison_operators_use_reflection_through_polymorphic_functions() {
    let (node, interpreted) = both(
        r#"
@private
let equal: 'a -> 'a -> Bool = fn left right => left == right
@private
let less: 'a -> 'a -> Bool = fn left right => left < right
let natural = equal 3n 3n
let integer = less -2i 1i
let real = 1 + 2 <= 3 and 4 > 3 and 4 >= 4
let boolean = false < true
let text = "a" != "b"
let unicode = "" < "𐀀"
let unit = () == ()
let nan = 0 / 0
let unordered = [nan == nan, nan != nan, nan < nan, nan <= nan, nan > nan, nan >= nan]
let zeros = -0.0 == 0.0
let fixed = [1n8 < 2n8, 1n16 < 2n16, 1n32 < 2n32, 1n64 < 2n64, -1i8 < 0i8, -1i16 < 0i16, -1i32 < 0i32, -1i64 < 0i64]
"#,
        &[
            "natural",
            "integer",
            "real",
            "boolean",
            "text",
            "unicode",
            "unit",
            "unordered",
            "zeros",
            "fixed",
        ],
        None,
    );
    let expected = [
        "true",
        "true",
        "true",
        "true",
        "true",
        "true",
        "true",
        "[false,true,false,false,false,false]",
        "true",
        "[true,true,true,true,true,true,true,true]",
    ];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn structural_comparisons_are_alphabetical_lexicographic_and_partial() {
    let (node, interpreted) = both(
        r#"
let record = { z: 0n, a: 2n } > { a: 1n, z: 99n }
let reordered = { z: 0n, a: 2n } == { a: 2n, z: 0n }
let a: #A | #B = #A
let b: #A | #B = #B
let cases = [a == b, a < b, b > a]
let p: #Some [Nat] = #Some [1n, 2n]
let q: #Some [Nat] = #Some [1n, 3n]
let payload = p < q
let empty: [Nat] = []
let arrays = [[1n, 2n] < [1n, 3n], [1n] < [1n, 0n], [1n, 0n] > [1n], empty == empty]
type Tree = #Leaf Nat | #Branch [Tree]
let left: Tree = #Branch [#Leaf 1n, #Branch [#Leaf 2n]]
let right: Tree = #Branch [#Leaf 1n, #Branch [#Leaf 3n]]
let recursive = [left < right, left == left]
let f: Nat -> Nat = fn n => n
let functions = [f == f, f != f, f < f, f <= f, f > f, f >= f]
let earlier = { z: f, a: 1n } < { a: 2n, z: f }
let blocked = { a: f, z: 1n } < { z: 2n, a: f }
let nan = 0 / 0
let nested_nan = [[nan] == [nan], [nan] != [nan], [nan] <= [nan], [0, nan] < [1, nan]]
@private
let mirror: Mirror Nat = std::reflect::mirror ()
let mirrors = mirror != mirror
type Boxed = hide 'a => 'a
@private
let boxed: Boxed = 1n
let hidden = boxed != boxed
let direct = match std::order::compare left right with | #Less => true | _ => false end
let explicit = match std::order::compare_with (std::reflect::type_of right) right left with | #Greater => true | _ => false end
"#,
        &[
            "record",
            "reordered",
            "cases",
            "payload",
            "arrays",
            "recursive",
            "functions",
            "earlier",
            "blocked",
            "nested_nan",
            "mirrors",
            "hidden",
            "direct",
            "explicit",
        ],
        None,
    );
    let expected = [
        "true",
        "true",
        "[false,true,true]",
        "true",
        "[true,true,true,true]",
        "[true,true]",
        "[false,true,false,false,false,false]",
        "true",
        "false",
        "[false,true,false,true]",
        "true",
        "true",
        "true",
        "true",
    ];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn comparisons_instantiate_unconstrained_rows_without_changing_generic_contracts() {
    let (node, interpreted) = both(
        r#"
@private
let eq: 'a -> 'a -> Bool = fn a b => a == b
let cases = [#A == #B, #A < #B, #B > #A, eq (#A) (#B)]
let bound_cases = do
  let a = #A
  let b = #B
  return a == b
end
let bound_empty = do
  let a = []
  return a == a
end
let empty = [] == []
let payload = #Some [1n, 2n] < #Some [1n, 3n]
let functions = (fn x => x) == (fn x => x)
@private
let identity = fn x => x
let same_function = identity == identity
let local = do
  let same = fn a b => a == b
  return [same 1n 1n, same "a" "b", same (#A) (#B)]
end
"#,
        &[
            "cases",
            "bound_cases",
            "bound_empty",
            "empty",
            "payload",
            "functions",
            "same_function",
            "local",
        ],
        None,
    );
    let expected = [
        "[false,true,true,false]",
        "false",
        "true",
        "true",
        "true",
        "false",
        "false",
        "[true,false,false]",
    ];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn comparisons_evaluate_operands_once_and_leave_opaque_values_unordered() {
    let (node, interpreted) = both(
        r#"
@private
let effects_run = fn _ => do
  let count = mut 0n
  let next = fn _ => do
    _ = count := std::nat::add (~count) 1n
    return ~count
  end
  let ordered = next () < next ()
  return (ordered, ~count)
end
let effects = effects_run ()
@private
let cells_run = fn _ => do
  let cell = mut 1n
  return [cell == cell, cell != cell, cell < cell, cell <= cell, cell > cell, cell >= cell]
end
let cells = cells_run ()
@private
let effectful_functions_run = fn _ => do
  let cell = mut 1n
  let read = fn _ => ~cell
  return [read == read, read != read, read < read]
end
let effectful_functions = effectful_functions_run ()
let foreign = match std::ffi::encode 1n with
| #Some value => [value == value, value != value, value <= value]
| #Error _ => []
end
"#,
        &["effects", "cells", "effectful_functions", "foreign"],
        None,
    );
    let expected = [
        r#"{"0":true,"1":2}"#,
        "[false,true,false,false,false,false]",
        "[false,true,false]",
        "[false,true,false]",
    ];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn comparison_diagnostics_require_a_shared_type_and_standard_library() {
    let mixed = project("let result = 1n == \"one\"", true, None, false);
    let error = ruddy_cli::check_project(mixed.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("type-mismatch"), "{error}");
    let missing = project("let result = 1n == 1n", false, None, false);
    let error = ruddy_cli::check_project(missing.path())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("comparison operators require `std::order::equal`"),
        "{error}"
    );
}

#[test]
fn cell_comparison_descriptors_preserve_identity_without_exposing_reads() {
    let (node, interpreted) = both(
        r#"
@private
let compare_cell: mut 'r 'a -> Bool = fn cell => cell == cell
@private
let run = fn _ => do
  let numbers = mut 1n
  let words = mut "one"
  let mirror = std::reflect::type_of numbers
  let same = match std::reflect::same mirror mirror with | #Some _ => true | #None => false end
  let different = match std::reflect::same mirror (std::reflect::type_of words) with | #Some _ => false | #None => true end
  let opaque = match std::reflect::shape mirror with | #Foreign => true | _ => false end
  let description = std::reflect::describe mirror
  let described = match std::array::get description.nodes description.root with | #Some (#Cell _) => true | _ => false end
  return [same, different, opaque, described, compare_cell numbers, [numbers] != [numbers], { a: 1n, z: numbers } < { a: 2n, z: numbers }]
end
let result = run ()
"#,
        &["result"],
        None,
    );
    let expected = ["[true,true,true,true,false,true,true]"];
    assert_eq!(node, expected);
    assert_eq!(interpreted, expected);
}

#[test]
fn comparisons_of_opened_existentials_require_their_mirrors() {
    for expression in ["x == x", "(fn _ => x) == (fn _ => x)"] {
        let source = format!(
            "type Boxed = hide 'a => 'a\n@private\nlet boxed: Boxed = 1n\nlet result = match boxed with\n| hide 'a x => {expression}\nend"
        );
        let missing = project(&source, true, None, false);
        let error = ruddy_cli::check_project(missing.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("runtime-type-information"), "{error}");
    }
    let (node, interpreted) = both(
        r#"
type Boxed = hide 'a => { mirror: Mirror 'a, value: 'a }
@private
let boxed: Boxed = { mirror: std::reflect::mirror (), value: 1n }
@private
let eq = fn left right => left == right
let result = match boxed with
| hide 'a { mirror, value } => [
  value == value,
  eq value value,
  (fn _ => value) == (fn _ => value),
  match std::order::compare_with mirror value value with | #Equal => true | _ => false end,
]
end
"#,
        &["result"],
        None,
    );
    assert_eq!(node, ["[true,true,false,true]"]);
    assert_eq!(interpreted, node);
}
