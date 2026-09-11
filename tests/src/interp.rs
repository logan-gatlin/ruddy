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

/// `std::js`'s data layer is the two checked-conversion intrinsics and
/// nothing else, so both backends must agree about it. Host observation is
/// left out on purpose: the interpreter provides no JavaScript host.
#[test]
fn the_javascript_data_adapter_agrees_across_backends() {
    let source = r#"
type Role = #Admin | #User Nat
type Person = { name: String, roles: [Role] }
let write: Person -> std::result::Result std::js::Value std::js::Error = std::js::lower
let read: std::js::Value -> std::result::Result Person std::js::Error = std::js::lift
let round_trip = match write { name: "ruddy", roles: [#Admin, #User 2n] } with
| #Some raw => match read raw with
  | #Some person => std::str::concat person.name (std::str::from_nat (std::array::len person.roles))
  | #Error error => error.message
  end
| #Error error => error.message
end
let through_the_adapter = do
  let adapter: std::js::Adapter Nat = std::js::adapter ()
  return match adapter.lower 7n with
  | #Some raw => match adapter.lift raw with
    | #Some value => std::str::from_nat value
    | #Error error => error.expected
    end
  | #Error error => error.expected
  end
end
let a_callable_is_refused = do
  let adapter: std::js::Adapter (Nat -> Nat) = std::js::adapter ()
  return match adapter.lower (fn value => value) with
  | #Some _ => "written"
  | #Error error => error.expected
  end
end
"#;
    let exports = ["round_trip", "through_the_adapter", "a_callable_is_refused"];
    let (node, interpreted) = both(source, &exports, None);
    assert_eq!(node, interpreted);
    assert_eq!(
        interpreted,
        vec![
            "\"ruddy2\"",
            "\"7\"",
            "\"a verifiable data type; a function contract needs an adapter\"",
        ]
    );
}

/// Cases where the two backends had answered differently, or had answered at
/// all where the specification says a value is refused.
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
