//! Tests for `std::abi`, the backend-neutral ABI plans.
//!
//! A plan is data describing a foreign calling contract, and `validate` is a
//! reading of that data. Nothing in this file executes a foreign call, and
//! nothing in `std/abi.rud` could: production native and WebAssembly foreign
//! integration is later work, and a plan that validates is not a claim that
//! those integrations already execute. What these tests hold the validator to
//! are real numbers — the offsets a 64-bit C compiler gives
//! `struct { char a; int b; double c; }`, and the ownership rules the
//! component model's canonical ABI states for a lifted string and list.

use std::{fs, path::Path, process::Command};

/// A bundle with the standard library as a path dependency and the given
/// source as its root module.
fn project(source: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::write(
        project.path().join("Ruddy.toml"),
        format!(
            "name = \"abi-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\nplatform = \"node\"\n\n[dependencies]\nstd = {standard:?}\n"
        ),
    )
    .unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

/// Build the bundle and assert its exports from JavaScript.
fn run(project: &Path, script: &str) {
    ruddy_cli::check_project(project).expect("the plan bundle checks");
    let artifact = ruddy_cli::build_project(project).expect("the plan bundle builds");
    let script = format!(
        "import assert from 'node:assert/strict'; import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({}));\n{script}",
        serde_json::to_string(artifact.with_extension("js").to_str().unwrap()).unwrap()
    );
    let output = Command::new("node")
        .current_dir(project)
        .args(["--input-type=module", "--eval", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn standard_module() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("std/abi.rud"),
    )
    .unwrap()
}

/// The layout a 64-bit C compiler gives `struct { char a; int b; double c; }`:
/// `a` at 0, three bytes of padding, `b` at 4, `c` at 8, and a total of 16
/// bytes aligned to 8. Every mutation below moves one of those numbers.
const C_STRUCT: &str = r#"
let wide: std::abi::Target = { convention: #C, address_bits: 64n, scalar_alignment: #None }

let sample: std::abi::Layout = {
  name: "Sample",
  size: 16n,
  alignment: 8n,
  fields: [
    { name: "a", offset: 0n, shape: #Scalar (#Integer { bits: 8n, signed: true }) },
    { name: "b", offset: 4n, shape: #Scalar (#Integer { bits: 32n, signed: true }) },
    { name: "c", offset: 8n, shape: #Scalar (#Float { bits: 64n }) },
  ],
}

let by_value: std::abi::Layout -> std::abi::Plan = fn layout => {
  name: "sample_by_value",
  target: { convention: #C, address_bits: 64n, scalar_alignment: #None },
  parameters: [{ name: "sample", shape: #Record layout }],
  result: #None,
  obligations: [],
}

let laid_out: [std::abi::Field] -> std::abi::Layout = fn fields => { fields: fields, ..sample }

let a: std::abi::Field = { name: "a", offset: 0n, shape: #Scalar (#Integer { bits: 8n, signed: true }) }
let b: std::abi::Field = { name: "b", offset: 4n, shape: #Scalar (#Integer { bits: 32n, signed: true }) }
let c: std::abi::Field = { name: "c", offset: 8n, shape: #Scalar (#Float { bits: 64n }) }

let accepted = std::abi::report (by_value sample)
let accepted_validates = match std::abi::validate (by_value sample) with
| #Some _ => true
| #Error _ => false
end

let wrong_offset = std::abi::report (by_value (laid_out [a, { offset: 2n, ..b }, c]))
let wrong_size = std::abi::report (by_value { size: 20n, ..sample })
let wrong_alignment = std::abi::report (by_value { alignment: 3n, ..sample })
let overlapping = std::abi::report (by_value (laid_out [a, { offset: 0n, ..b }, c]))
let past_the_end = std::abi::report (by_value (laid_out [a, b, { offset: 16n, ..c }]))

let checked_alone =
  std::array::map std::abi::message (std::abi::check_layout wide { alignment: 3n, ..sample })

-- A list is a pointer beside a separate count under C, so it is not something a
-- C record holds a field of.
let a_list_field = std::abi::report (by_value (laid_out [a, b, {
  name: "c",
  offset: 8n,
  shape: #List {
    element: #Scalar (#Integer { bits: 8n, signed: false }),
    ownership: #Some {
      allocated_by: #Caller,
      freed_by: #None,
      free_with: #None,
      lifetime: #Static,
    },
    length: #Some (#Fixed 4n),
  },
}]))
"#;

#[test]
fn a_c_struct_layout_is_accepted_and_every_mutation_of_it_is_named() {
    let project = project(C_STRUCT);
    run(
        project.path(),
        r#"
// The real layout passes: nothing about it is decidably wrong.
assert.deepEqual(app.accepted, []);
assert.equal(app.accepted_validates, true);

// A field at an offset its own alignment does not divide.
assert.deepEqual(app.wrong_offset, [
  'the field "b" of the record "Sample" starts at byte 2, which its own alignment of 4 bytes does not divide.',
]);

// A size the alignment does not divide: 20 is not a multiple of 8.
assert.deepEqual(app.wrong_size, [
  'the record "Sample" declares a size of 20 bytes, which its alignment of 8 bytes does not divide.',
]);

// An alignment of 3, and the two fields that cannot fit under it.
assert.deepEqual(app.wrong_alignment, [
  'the record "Sample" declares an alignment of 3 bytes, which is not a power of two.',
  'the field "b" of the record "Sample" needs an alignment of 4 bytes, more than the 3 bytes the record it is in declares.',
  'the field "c" of the record "Sample" needs an alignment of 8 bytes, more than the 3 bytes the record it is in declares.',
]);
assert.deepEqual(app.checked_alone, app.wrong_alignment);

// A field that starts inside the one before it.
assert.deepEqual(app.overlapping, [
  'the field "b" of the record "Sample" starts at byte 0, but the field "a" before it runs to byte 1.',
]);

// A field that runs past the size the record declares.
assert.deepEqual(app.past_the_end, [
  'the field "c" of the record "Sample" starts at byte 16 and is 8 bytes wide, running past the 16 bytes the record declares.',
]);

// And a field of a shape the C ABI lays out nowhere in a record. A record with
// a region in it is a plan that passes a region, so the obligations the
// by-value struct needed none of come due as well.
assert.deepEqual(app.a_list_field, [
  'the field "c" of the record "Sample" is a list, which the C ABI does not lay out inside a record; state a pointer and the field that carries its length instead.',
  'the plan "sample_by_value" passes a region of memory, so its adapter contract must supply validity, and this plan states no such obligation.',
  'the plan "sample_by_value" passes a region of memory, so its adapter contract must supply a lifetime, and this plan states no such obligation.',
]);
"#,
    );
}

/// `ssize_t write_all(int fd, const uint8_t *buffer, size_t len)`: the buffer's
/// length lives in another parameter and its owner is the caller, and neither
/// of those facts is in any type.
const C_CALL: &str = r#"
let owned: std::abi::Ownership = {
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
    { name: "fd", shape: #Scalar (#Integer { bits: 32n, signed: true }) },
    { name: "buffer", shape: buffer },
    { name: "len", shape: #Scalar (#Integer { bits: 64n, signed: false }) },
  ],
  result: #Some (#Scalar (#Integer { bits: 64n, signed: true })),
  obligations: [
    { duty: #Validity, note: "the caller passes a readable region of len bytes" },
    { duty: #Lifetime, note: "the region stays readable until the call returns" },
    { duty: #Provenance, note: "the region comes from malloc and goes back to free" },
  ],
}

let buffer: std::abi::Type =
  #Pointer { pointee: byte, ownership: #Some owned, length: #Some (#Parameter "len"), nullable: false }

let accepted = std::abi::report (write_all buffer)
let accepted_validates = match std::abi::validate (write_all buffer) with
| #Some _ => true
| #Error _ => false
end

let without_length = std::abi::report (write_all
  (#Pointer { pointee: byte, ownership: #Some owned, length: #None, nullable: false }))
let without_owner = std::abi::report (write_all
  (#Pointer { pointee: byte, ownership: #None, length: #Some (#Parameter "len"), nullable: false }))
let unknown_length = std::abi::report (write_all
  (#Pointer { pointee: byte, ownership: #Some owned, length: #Some (#Parameter "count"), nullable: false }))
let abi_counted = std::abi::report (write_all
  (#Pointer { pointee: byte, ownership: #Some owned, length: #Some (#Canonical), nullable: false }))

let without_obligations = std::abi::report {
  obligations: [],
  ..write_all buffer
}

let text: std::abi::Type =
  #Text { encoding: #Utf8, ownership: #Some owned, length: #Some (#Sentinel "a zero byte") }
let nul_terminated = std::abi::report (write_all text)
"#;

#[test]
fn a_c_call_states_its_buffer_length_and_its_owner_or_is_refused() {
    let project = project(C_CALL);
    run(
        project.path(),
        r#"
// A pointer whose length is another parameter, whose owner is written down,
// and whose obligations the adapter contract carries.
assert.deepEqual(app.accepted, []);
assert.equal(app.accepted_validates, true);

// A length cannot be read off a pointer's type, so a plan that omits it is
// refused rather than guessed at.
assert.deepEqual(app.without_length, [
  'the parameter "buffer" states no length: a plan must say how many elements travel with the pointer, because no type says it.',
]);
assert.deepEqual(app.without_owner, [
  'the parameter "buffer" states no owner: a plan must say which side allocates the region and which side frees it.',
]);

// A length that names a parameter this plan does not have.
assert.deepEqual(app.unknown_length, [
  'the parameter "buffer" takes its length from a parameter named "count", and this plan has no parameter of that name.',
]);

// The C ABI carries no count beside a pointer; that is the canonical ABI's
// rule, and a C plan may not borrow it.
assert.deepEqual(app.abi_counted, [
  'the parameter "buffer" leaves its length to the ABI, which the C ABI does not carry: name the parameter or the field beside it holding the count, the count the contract fixes, or the sentinel that ends the region.',
]);

// The plan records obligations rather than discharging them, so a plan that
// passes a region and states none is refused.
assert.deepEqual(app.without_obligations, [
  'the plan "write_all" passes a region of memory, so its adapter contract must supply validity, and this plan states no such obligation.',
  'the plan "write_all" passes a region of memory, so its adapter contract must supply a lifetime, and this plan states no such obligation.',
  'the plan "write_all" passes a region of memory, so its adapter contract must supply allocation provenance, and this plan states no such obligation.',
]);

// A NUL-terminated C string is a length too, stated as the sentinel that ends
// the region.
assert.deepEqual(app.nul_terminated, []);
"#,
    );
}

/// `log: func(message: string, tags: list<u32>, sink: borrow<output-stream>)
/// -> bool` across the component model's canonical ABI: the callee's own
/// allocator owns the copies the lift makes, and the count travels with the
/// pointer rather than in a parameter of its own.
const CANONICAL_CALL: &str = r#"
let lifted: std::abi::Ownership = {
  allocated_by: #Callee,
  freed_by: #None,
  free_with: #None,
  lifetime: #Call,
}

let message: std::abi::Type =
  #Text { encoding: #Utf8, ownership: #Some lifted, length: #Some (#Canonical) }
let tags: std::abi::Type = #List {
  element: #Scalar (#Integer { bits: 32n, signed: false }),
  ownership: #Some lifted,
  length: #Some (#Canonical),
}

let log: [std::abi::Parameter] -> std::abi::Plan = fn parameters => {
  name: "log",
  target: { convention: #CanonicalAbi, address_bits: 32n, scalar_alignment: #None },
  parameters: parameters,
  result: #Some (#Scalar #Bool),
  obligations: [
    { duty: #Validity, note: "the canonical lift copies both regions into the callee's memory" },
    { duty: #Lifetime, note: "the copies live as long as the callee keeps them" },
  ],
}

let lifting: [std::abi::Parameter] = [
  { name: "message", shape: message },
  { name: "tags", shape: tags },
  { name: "sink", shape: #Handle { resource: "output-stream", owned: false } },
]

let accepted = std::abi::report (log lifting)
let accepted_validates = match std::abi::validate (log lifting) with
| #Some _ => true
| #Error _ => false
end

let a_pointer = std::abi::report (log [{
  name: "message",
  shape: #Pointer {
    pointee: #Scalar (#Integer { bits: 8n, signed: false }),
    ownership: #Some lifted,
    length: #Some (#Fixed 8n),
    nullable: false,
  },
}])

let a_wide_integer =
  std::abi::report (log [{ name: "id", shape: #Scalar (#Integer { bits: 128n, signed: false }) }])

let a_zero_width =
  std::abi::report (log [{ name: "id", shape: #Scalar (#Integer { bits: 0n, signed: false }) }])

let counted_by_hand = std::abi::report (log [
  { name: "tags", shape: #List {
    element: #Scalar (#Integer { bits: 32n, signed: false }),
    ownership: #Some lifted,
    length: #Some (#Fixed 4n),
  } },
])

let unowned = std::abi::report (log [
  { name: "tags", shape: #List {
    element: #Scalar (#Integer { bits: 32n, signed: false }),
    ownership: #None,
    length: #Some (#Canonical),
  } },
])

let a_wrong_address_width = std::abi::report {
  target: { convention: #CanonicalAbi, address_bits: 16n, scalar_alignment: #None },
  ..log lifting
}

let a_c_character = std::abi::report {
  name: "log",
  target: { convention: #C, address_bits: 64n, scalar_alignment: #None },
  parameters: [{ name: "letter", shape: #Scalar #Char }],
  result: #None,
  obligations: [],
}

let a_c_handle = std::abi::report {
  name: "log",
  target: { convention: #C, address_bits: 64n, scalar_alignment: #None },
  parameters: [{ name: "sink", shape: #Handle { resource: "output-stream", owned: false } }],
  result: #None,
  obligations: [],
}

-- A list and a string are each one object in linear memory under the canonical
-- ABI: a pointer and a count, eight bytes aligned to four on wasm32.
let wasm: std::abi::Target = { convention: #CanonicalAbi, address_bits: 32n, scalar_alignment: #None }
let wide: std::abi::Target = { convention: #C, address_bits: 64n, scalar_alignment: #None }
let list_size = std::abi::size_of wasm tags
let list_alignment = std::abi::alignment_of wasm tags
let c_list_size = std::abi::size_of wide tags
"#;

#[test]
fn a_canonical_abi_call_lifts_a_string_and_a_list_by_its_own_rules() {
    let project = project(CANONICAL_CALL);
    run(
        project.path(),
        r#"
// Lifting a string and a list, with the handle the component model has and a
// C ABI does not.
assert.deepEqual(app.accepted, []);
assert.equal(app.accepted_validates, true);

// A raw pointer is a construct the canonical ABI does not have.
assert.deepEqual(app.a_pointer, [
  'the parameter "message" is a raw pointer, which the canonical ABI does not have; it passes lists, strings, records, and resource handles, and lowers them itself.',
]);

// Nor a 128-bit integer: the component model stops at 64.
assert.deepEqual(app.a_wide_integer, [
  'the parameter "id" is 128 bits wide, a width the canonical ABI does not have.',
]);

// A scalar of no width at all is nobody's parameter.
assert.deepEqual(app.a_zero_width, [
  'the parameter "id" is a scalar of zero width, which no calling convention passes.',
]);

// The count travels with the pointer, so a plan does not get to fix one.
assert.deepEqual(app.counted_by_hand, [
  'the parameter "tags" states a length of its own, which the canonical ABI does not leave to a plan: it passes a pointer and a count together, and that count is the length.',
]);

// Who owns a lifted copy is still the plan's to state.
assert.deepEqual(app.unowned, [
  'the parameter "tags" states no owner: a plan must say which side allocates the region and which side frees it.',
]);

// The canonical ABI is defined over 32-bit linear memory, and over 64-bit for
// memory64; 16-bit is neither.
assert.deepEqual(app.a_wrong_address_width, [
  'the plan "log" states an address width of 16 bits, which the canonical ABI is not defined over.',
]);

// And the differences run the other way too: the C ABI has neither a Unicode
// scalar value nor a resource handle.
assert.deepEqual(app.a_c_character, [
  'the parameter "letter" is a Unicode scalar value, which the C ABI does not have; a C plan states an integer of a width and the encoding its contract uses.',
]);
assert.deepEqual(app.a_c_handle, [
  'the parameter "sink" is a resource handle, which the C ABI does not have; a C plan passes an opaque pointer and states who owns it.',
]);

// A list is one object of eight bytes under the canonical ABI on wasm32, and
// no object at all under C, where it is a pointer beside a separate count.
assert.deepEqual(app.list_size, {tag: 'Some', value: 8});
assert.deepEqual(app.list_alignment, {tag: 'Some', value: 4});
assert.equal(app.c_list_size.tag, 'None');
"#,
    );
}

/// The module is data and arithmetic: it names no extern, declares and performs
/// no effect, and so cannot call, allocate, or observe anything. A plan that
/// validates says its own numbers hold together and nothing more.
#[test]
fn a_validated_plan_claims_nothing_about_execution() {
    let source = standard_module();
    assert!(
        !source
            .lines()
            .any(|line| line.trim_start().starts_with("extern ")),
        "std/abi.rud names an extern, so it could reach a host"
    );
    assert!(
        !source.contains('!'),
        "std/abi.rud spells an effect, so it could perform one"
    );

    // The leading comment says what the specification requires it to say.
    let doc = source
        .lines()
        .take_while(|line| line.starts_with("--"))
        .map(|line| line.trim_start_matches('-').trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        doc.contains(
            "Backend-neutral ABI plans are validated against representative C and Wasm layouts \
             and invocation contracts. Production native/Wasm foreign integration is later work; \
             a validated plan is not a claim that those integrations already execute."
        ),
        "the module's doc comment does not say what a validated plan is not: {doc}"
    );
    assert!(
        doc.contains(
            "https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md"
        ),
        "the module's doc comment does not link the canonical ABI specification"
    );
}

/// Three things a plan describes that no rule recovers for it: a count that
/// lives in the field beside the pointer, a target that aligns a scalar less
/// strictly than its own width, and an offset far enough out that adding a
/// size to it would leave the integer domain.
const STATED: &str = r#"
let wide: std::abi::Target = { convention: #C, address_bits: 64n, scalar_alignment: #None }
let i386: std::abi::Target = { convention: #C, address_bits: 32n, scalar_alignment: #Some 4n }

let owned: std::abi::Ownership =
  { allocated_by: #Caller, freed_by: #None, free_with: #None, lifetime: #Call }

let counted: String -> std::abi::Type = fn name => #Pointer {
  pointee: #Scalar (#Integer { bits: 8n, signed: false }),
  ownership: #Some owned,
  length: #Some (#Field name),
  nullable: false,
}

-- struct iovec { void *iov_base; size_t iov_len; }
let iovec: std::abi::Layout = {
  name: "iovec",
  size: 16n,
  alignment: 8n,
  fields: [
    { name: "iov_base", offset: 0n, shape: counted "iov_len" },
    { name: "iov_len", offset: 8n, shape: #Scalar (#Integer { bits: 64n, signed: false }) },
  ],
}
let sibling_count = std::array::map std::abi::message (std::abi::check_layout wide iovec)
let no_such_sibling = std::array::map
  std::abi::message
  (std::abi::check_layout wide {
    fields: [
      { name: "iov_base", offset: 0n, shape: counted "nowhere" },
      { name: "iov_len", offset: 8n, shape: #Scalar (#Integer { bits: 64n, signed: false }) },
    ],
    ..iovec
  })

-- i386 System V gives a double eight bytes and four-byte alignment.
let pair: std::abi::Layout = {
  name: "Pair",
  size: 12n,
  alignment: 4n,
  fields: [
    { name: "a", offset: 0n, shape: #Scalar (#Integer { bits: 32n, signed: true }) },
    { name: "b", offset: 4n, shape: #Scalar (#Float { bits: 64n }) },
  ],
}
let narrow_pair = std::array::map std::abi::message (std::abi::check_layout i386 pair)
let wide_pair = std::array::map std::abi::message (std::abi::check_layout wide pair)

let far = std::array::map
  std::abi::message
  (std::abi::check_layout wide {
    name: "Far",
    size: 16n,
    alignment: 8n,
    fields: [{ name: "only", offset: 9007199254740984n, shape: #Scalar (#Float { bits: 64n }) }],
  })
"#;

#[test]
fn a_plan_states_what_no_rule_recovers_for_it() {
    let project = project(STATED);
    run(
        project.path(),
        r#"
// A pointer counted by the field beside it is the archetypal C buffer struct,
// and a layout is checked with its own fields in scope even with no plan
// around it.
assert.deepEqual(app.sibling_count, []);
assert.deepEqual(app.no_such_sibling, [
  'the field "iov_base" of the record "iovec" takes its length from a field named "nowhere", and no field of that name is beside it.',
]);

// The same layout is right on i386 and wrong on x86-64, which is why the
// target states the cap rather than the validator deriving one.
assert.deepEqual(app.narrow_pair, []);
assert.deepEqual(app.wide_pair, [
  'the field "b" of the record "Pair" starts at byte 4, which its own alignment of 8 bytes does not divide.',
  'the field "b" of the record "Pair" needs an alignment of 8 bytes, more than the 4 bytes the record it is in declares.',
]);

// An offset near the top of the integer domain is a fault reported, not an
// addition that leaves the domain.
assert.deepEqual(app.far, [
  'the field "only" of the record "Far" starts at byte 9007199254740984 and is 8 bytes wide, running past the 16 bytes the record declares.',
]);
"#,
    );
}
