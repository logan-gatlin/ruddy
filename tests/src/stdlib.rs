use std::{fs, process::Command};

#[test]
fn bundled_primitive_utilities_compile_and_run_through_the_javascript_boundary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let project = tempfile::tempdir().expect("a temporary string-utility project");
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"str-test\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\ntarget = \"js\"\n\n[dependencies]\nstd = {:?}\n",
            root.join("std")
        ),
    )
    .unwrap();
    fs::write(
        project.path().join("main.hc"),
        r#"let joined = std::str::concat "rud" "dy"
let size = std::str::len joined
let empty = std::str::is_empty ""
let nat = std::str::from_nat 42n
let int = std::str::from_int 7i
let real = std::str::from_real 1.5
let boolean = std::str::from_boolean true
let has = std::str::contains joined "udd"
let starts = std::str::starts_with joined "rud"
let ends = std::str::ends_with joined "dy"
let char = std::str::char_at joined 1n
let index = std::str::index_of joined "ddy"
let part = std::str::slice joined 1n 4n
let trimmed = std::str::trim "  ruddy  "
let left_trimmed = std::str::trim_start "  ruddy  "
let right_trimmed = std::str::trim_end "  ruddy  "
let lower = std::str::to_lowercase "RuDdY"
let upper = std::str::to_uppercase "RuDdY"
let repeated = std::str::repeat "ha" 2n
let replaced = std::str::replace_first "ruddy" "d" "b"
let replaced_all = std::str::replace_all "ruddy" "d" "b"
let left_padded = std::str::pad_start "7" 3n "0"
let right_padded = std::str::pad_end "7" 3n "0"

let nat_results = {
  added: std::nat::add 20n 22n,
  subtracted: std::nat::subtract 2n 5n,
  divided: std::nat::divide 7n 2n,
  clamped: std::nat::clamp 12n 0n 10n,
  converted: std::nat::from_real 3.9,
  zero: std::nat::is_zero 0n,
}
let negative_seven = std::int::negate 7i
let int_results = {
  subtracted: std::int::subtract 2i 5i,
  divided: std::int::divide negative_seven 2i,
  remainder: std::int::remainder negative_seven 4i,
  absolute: std::int::abs negative_seven,
  converted: std::int::from_real 3.9,
}
let real_results = {
  added: std::real::add 1.25 2.25,
  divided: std::real::divide 7 2,
  square_root: std::real::sqrt 9,
  rounded: std::real::round 3.6,
  nan: std::real::is_nan (std::real::divide 0 0),
  finite: std::real::is_finite 42,
  negative_zero_equal: std::real::equal (std::real::negate 0) 0,
}
let boolean_results = {
  negated: std::boolean::logical_not true,
  both: std::boolean::logical_and true false,
  either: std::boolean::logical_or true false,
  exclusive: std::boolean::logical_xor true false,
  equal: std::boolean::equal true true,
  implies: std::boolean::implies true false,
}
"#,
    )
    .unwrap();

    let artifact = ruddy_cli::build_project(project.path()).expect("the std consumer builds");
    let javascript = artifact.with_extension("js");
    assert!(javascript.is_file());

    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let probe = format!(
        "import {{ pathToFileURL }} from 'node:url'; const x = await import(pathToFileURL({}).href); console.log(JSON.stringify([x.joined,x.size,x.empty,x.nat,x.int,x.real,x.boolean,x.has,x.starts,x.ends,x.char,x.index,x.part,x.trimmed,x.left_trimmed,x.right_trimmed,x.lower,x.upper,x.repeated,x.replaced,x.replaced_all,x.left_padded,x.right_padded,{{...x.nat_results}},{{...x.int_results}},{{...x.real_results}},{{...x.boolean_results}}]));",
        serde_json::to_string(javascript.to_str().unwrap()).unwrap()
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
        r#"["ruddy",5,true,"42","7","1.5","true",true,true,true,"u",2,"udd","ruddy","ruddy  ","  ruddy","ruddy","RUDDY","haha","rubdy","rubby","007","700",{"added":42,"subtracted":0,"divided":3,"clamped":10,"converted":3,"zero":true},{"subtracted":-3,"divided":-3,"remainder":-3,"absolute":7,"converted":3},{"added":3.5,"divided":3.5,"square_root":3,"rounded":4,"nan":true,"finite":true,"negative_zero_equal":false},{"negated":false,"both":false,"either":true,"exclusive":true,"equal":true,"implies":false}]"#
    );
}

#[test]
fn bundled_functional_and_polymorphic_utilities_run_end_to_end() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let project = tempfile::tempdir().expect("a temporary std-utility project");
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"std-test\"\nversion = \"0.1.0\"\nroot = \"main.hc\"\ntarget = \"js\"\n\n[dependencies]\nstd = {:?}\n",
            root.join("std")
        ),
    )
    .unwrap();
    fs::write(
        project.path().join("main.hc"),
        r#"let ordering_name = fn ordering => match ordering with
  | #Less => "less"
  | #Equal => "equal"
  | #Greater => "greater"
end

let mapped_option = std::option::map (std::nat::add 1n) (#Some 41n)
let option_value = std::option::unwrap_or 0n mapped_option
let filtered_option = std::option::filter (fn value => std::nat::greater_than value 40n) mapped_option
let option_kept = std::option::is_some filtered_option
let option_row_fallback = std::option::some_or "fallback" (#Error "ignored")

let mapped_error = std::result::map_error std::str::to_uppercase (#Error "bad")
let error_text = match mapped_error with | #Some _ => "ok" | #Error error => error end
let recovered = std::result::or_else (fn error => #Some (std::str::len error)) mapped_error
let recovered_value = std::result::unwrap_or 0n recovered
let error_row_fallback = std::result::error_or "fallback" (#None)

let numbers: std::List Nat = #Cons (1n, #Cons (2n, #Cons (3n, #None)))
let mapped_numbers = std::list::map (std::nat::add 1n) numbers
let list_total = std::list::fold_left std::nat::add 0n mapped_numbers
let reversed_head = std::option::unwrap_or 0n (std::list::head (std::list::reverse mapped_numbers))
let list_has_three = std::list::contains std::nat::equal 3n mapped_numbers
let found = std::option::unwrap_or 0n (std::list::find (fn value => std::nat::greater_than value 2n) mapped_numbers)

let composed = std::function::compose (std::nat::multiply 2n) (std::nat::add 1n) 20n
let piped = std::function::pipe 41n (std::nat::add 1n)
let flipped = std::function::flip std::nat::subtract 2n 5n
let curried = std::function::curry (fn values => std::nat::add (values.0) (values.1)) 20n 22n
let uncurried = std::function::uncurry std::nat::add (20n, 22n)

let tuple_mapped = std::tuple::map_both (std::nat::add 1n) std::str::to_uppercase (1n, "ruddy")
let tuple_first = std::tuple::first { 0: 42n, 1: false, 2: "extra" }
let tuple_result = { first: tuple_mapped.0, second: tuple_mapped.1, projected: tuple_first }

let reversed_order = ordering_name (std::ordering::reverse (std::nat::compare 1n 2n))
let compared_string = ordering_name (std::str::compare "a" "b")
let compared_boolean = ordering_name (std::boolean::compare false true)
let compared_real = ordering_name (std::real::compare (std::real::negate 0) 0)

let blank = std::str::is_blank " \n\t "
let safe_char = std::option::unwrap_or "?" (std::str::char_at_option "ruddy" 1n)
let stripped = std::option::unwrap_or "?" (std::str::strip_prefix "ruddy" "rud")
let bounded = std::str::slice_with "ruddy" { start: 1n, stop: 4n }
let open_bounded = std::str::slice_with "ruddy" { start: 2n }
let split_joined = std::str::join "-" (std::str::split "a,b,c" ",")
let reversed_string = std::str::reverse "ruddy"

let trig = std::real::round (std::real::multiply 1000.0 (std::real::sin (std::real::divide std::real::pi 2.0)))
let logarithm = std::real::round (std::real::log std::real::e)
let angle = std::real::round (std::real::radians_to_degrees std::real::pi)
"#,
    )
    .unwrap();

    let artifact = ruddy_cli::build_project(project.path()).expect("the std consumer builds");
    let javascript = artifact.with_extension("js");
    assert!(javascript.is_file());

    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let probe = format!(
        "import {{ pathToFileURL }} from 'node:url'; const x = await import(pathToFileURL({}).href); console.log(JSON.stringify([x.option_value,x.option_kept,x.option_row_fallback,x.error_text,x.recovered_value,x.error_row_fallback,x.list_total,x.reversed_head,x.list_has_three,x.found,x.composed,x.piped,x.flipped,x.curried,x.uncurried,{{...x.tuple_result}},x.reversed_order,x.compared_string,x.compared_boolean,x.compared_real,x.blank,x.safe_char,x.stripped,x.bounded,x.open_bounded,x.split_joined,x.reversed_string,x.trig,x.logarithm,x.angle]));",
        serde_json::to_string(javascript.to_str().unwrap()).unwrap()
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
        r#"[42,true,"fallback","BAD",3,"fallback",9,4,true,3,42,42,3,42,42,{"first":2,"second":"RUDDY","projected":42},"greater","less","less","less",true,"u","dy","udd","ddy","a-b-c","yddur",1000,1,180]"#
    );
}
