//! Tests for `std::js`, the JavaScript foreign-adapter layer.
//!
//! Each test builds a small bundle against the standard library as a path
//! dependency, runs it under Node, and asserts from JavaScript, because what
//! is under test is exactly what the host sees and what the host can hand
//! back. The reference interpreter provides no JavaScript host, so the
//! cross-backend case for the pure data layer lives in `interp.rs` instead.

use std::{fs, path::Path, process::Command};

/// A bundle that depends on the standard library by path.
fn project(source: &str, platform: &str, integers: Option<u32>) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut manifest = format!(
        "name = \"js-adapter-test\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\nplatform = {platform:?}\n"
    );
    if let Some(integers) = integers {
        manifest.push_str(&format!("integers = {integers}\n"));
    }
    manifest.push_str(&format!("\n[dependencies]\nstd = {standard:?}\n"));
    fs::write(project.path().join("Ruddy.toml"), manifest).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

/// Build the bundle and run `script` against its exports under Node.
fn run(project: &Path, script: &str) {
    ruddy_cli::check_project(project).expect("the adapter consumer checks");
    let artifact = ruddy_cli::build_project(project).expect("the adapter consumer builds");
    let script = format!(
        "import assert from 'node:assert/strict'; import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({}));\nconst show = value => JSON.stringify(value, (_, item) => typeof item === 'bigint' ? item.toString() : item);\nconst some = result => {{ assert.equal(result.tag, 'Some', show(result)); return result.value; }};\nconst failure = result => {{ assert.equal(result.tag, 'Error', show(result)); return result.value; }};\nconst data = value => JSON.parse(JSON.stringify(value));\nconst tag = value => value.tag;\n{script}",
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

#[test]
fn lower_and_lift_carry_records_arrays_sums_and_nesting() {
    for platform in ["node", "web"] {
        let project = project(
            r#"
type Role = #Admin | #User Nat
type Home = { city: String, zip: Nat }
type Person = { name: String, roles: [Role], home: Home }
@private
let read: std::js::Value -> std::result::Result Person std::js::Error + std::js::!Host = std::js::lift
@private
let write: Person -> std::result::Result std::js::Value std::js::Error = std::js::lower
let lift_person = read
let lower_person = write
let round_trip: std::js::Value -> std::result::Result std::js::Value std::js::Error
  + std::js::!Host = fn raw => match read raw with
  | #Some person => write person
  | #Error error => #Error error
  end
let name_of: std::js::Value -> String + std::js::!Host =
  fn raw => match read raw with
| #Some person => person.name
| #Error error => error.message
end
let roles_of: std::js::Value -> Nat + std::js::!Host =
  fn raw => match read raw with
| #Some person => std::array::len person.roles
| #Error _ => 0n
end
"#,
            platform,
            None,
        );
        run(
            project.path(),
            r#"
const person = {
  name: 'Ada',
  roles: [{ tag: 'Admin' }, { tag: 'User', value: 7 }],
  home: { city: 'London', zip: 1 },
};
// A record, an array, a sum with and without a payload, and a nested record.
const written = data(some(await app.round_trip(person)));
assert.equal(written.name, 'Ada');
assert.deepEqual(written.home, { city: 'London', zip: 1 });
assert.deepEqual(written.roles.map(role => role.tag), ['Admin', 'User']);
assert.equal(written.roles[1].value, 7);
// What one direction writes the other reads again.
assert.equal(await app.name_of(written), 'Ada');
assert.equal(await app.roles_of(written), 2);
assert.equal(await app.name_of(person), 'Ada');

// The same two directions, each on its own.
const value = some(await app.lower_person(person));
assert.equal(data(value).home.city, 'London');
assert.equal(data(some(await app.lift_person(value))).name, 'Ada');

// A position that is not what the type says names itself.
const wrong = failure(await app.lift_person({ ...person, home: { city: 'London', zip: 'no' } }));
assert.equal(wrong.path, '$.home.zip');
assert.equal(wrong.expected, 'Nat');
assert.equal(failure(await app.lift_person({ ...person, roles: [{ tag: 'Ghost' }] })).path, '$.roles[0]');
assert.equal(failure(await app.lift_person({ name: 'Ada', roles: [] })).path, '$.home');
"#,
        );
    }
}

#[test]
fn conversion_checks_numbers_and_text() {
    let project = project(
        r#"
@private
let read_nat: std::js::Value -> std::result::Result Nat std::js::Error + std::js::!Host = std::js::lift
@private
let read_real: std::js::Value -> std::result::Result Real std::js::Error + std::js::!Host = std::js::lift
@private
let read_text: std::js::Value -> std::result::Result String std::js::Error + std::js::!Host = std::js::lift
@private
let read_callable: std::js::Value -> std::result::Result (Nat -> Nat) std::js::Error + std::js::!Host = std::js::lift
@private
let write_callable: (Nat -> Nat) -> std::result::Result std::js::Value std::js::Error =
  std::js::lower
let as_nat = read_nat
let as_real = read_real
let as_text = read_text
let scalars = std::str::len
let refused_by_lift: std::js::Value -> String + std::js::!Host =
  fn value => match read_callable value with
| #Some _ => "accepted"
| #Error error => error.expected
end
let refused_by_lower: String = match write_callable (fn n => n) with
| #Some _ => "written"
| #Error error => error.expected
end
"#,
        "node",
        None,
    );
    run(
        project.path(),
        r#"
// An integer position takes only an integer inside the target's domain.
assert.equal(some(await app.as_nat(7)), 7);
assert.equal(failure(await app.as_nat(1.5)).expected, 'Nat');
assert.equal(failure(await app.as_nat(-1)).expected, 'Nat');
assert.equal(failure(await app.as_nat(Number.MAX_SAFE_INTEGER + 1)).expected, 'Nat');
assert.equal(failure(await app.as_nat('7')).expected, 'Nat');
assert.equal(failure(await app.as_nat(NaN)).expected, 'Nat');

// Integers have one zero; a Real keeps the sign the host gave it.
const zero = some(await app.as_nat(-0));
assert.equal(zero, 0);
assert.equal(Object.is(zero, -0), false);
assert.equal(Object.is(some(await app.as_real(-0)), -0), true);

// Text must really be text, and an astral scalar survives as one scalar.
assert.equal(failure(await app.as_text(5)).expected, 'String');
assert.equal(failure(await app.as_text(['a'])).expected, 'String');
assert.equal(some(await app.as_text('a\u{1F600}')), 'a\u{1F600}');
assert.equal(await app.scalars('a\u{1F600}'), 2);

// Neither direction of the data layer carries a function contract.
const message = 'a verifiable data type; a function contract needs an adapter';
assert.equal(await app.refused_by_lift(() => 1), message);
assert.equal(app.refused_by_lower, message);
"#,
    );
}

/// The bundle binds `Nat` to 32 bits, so what a host number may be narrows
/// with it: the same token the default JS domain accepts is out of range here.
#[test]
fn a_bound_integer_domain_decides_what_lifts() {
    let project = project(
        r#"
let as_nat: std::js::Value -> std::result::Result Nat std::js::Error + std::js::!Host = std::js::lift
let as_nat64: std::js::Value -> std::result::Result Nat64 std::js::Error + std::js::!Host =
  std::js::lift
"#,
        "node",
        Some(32),
    );
    run(
        project.path(),
        r#"
assert.equal(some(await app.as_nat(4294967295)), 4294967295);
assert.equal(failure(await app.as_nat(4294967296)).expected, 'Nat');
assert.equal(some(await app.as_nat64(4294967296n)), 4294967296n);
"#,
    );
}

#[test]
fn a_specialized_adapter_replaces_the_structural_one() {
    let project = project(
        r#"
@private
let millis: std::js::Value -> std::result::Result Nat std::js::Error + std::js::!Host =
  std::js::lift
@private
let pure_millis: std::js::Value -> std::result::Result Nat std::js::Error = std::js::read
@doc "Seconds on this side, milliseconds on the host's."
let seconds: std::js::Adapter Nat = {
  lower: fn count => std::js::lower (std::nat::multiply count 1000n),
  lift: fn value => match millis value with
  | #Some elapsed => #Some (std::nat::divide elapsed 1000n)
  | #Error error => #Error error
  end,
  read: fn value => match pure_millis value with
  | #Some elapsed => #Some (std::nat::divide elapsed 1000n)
  | #Error error => #Error error
  end,
}
@private
let through: std::js::Adapter 'a -> 'a -> std::result::Result 'a std::js::Error
  + std::js::!Host = fn adapter value => match adapter.lower value with
  | #Some raw => adapter.lift raw
  | #Error error => #Error error
  end
let specialized: Nat -> std::result::Result Nat std::js::Error + std::js::!Host =
  through seconds
let structural: Nat -> std::result::Result Nat std::js::Error + std::js::!Host =
  through (std::js::adapter ())
let lowered = seconds.lower
let lifted = seconds.lift
"#,
        "node",
        None,
    );
    run(
        project.path(),
        r#"
// The hand-written adapter stands where the structural one does: the same
// function takes either, and only what reaches the host differs.
assert.equal(some(await app.specialized(7)), 7);
assert.equal(some(await app.structural(7)), 7);
assert.equal(some(await (await app.lowered)(7)), 7000);
assert.equal(some(await (await app.lifted)(7000)), 7);
assert.equal(failure(await (await app.lifted)('soon')).expected, 'Nat');
"#,
    );
}

#[test]
fn host_observation_reads_values_on_node_and_web() {
    for platform in ["node", "web"] {
        let project = project(
            r#"
let kind_of: std::js::Value -> String + std::js::!Host = fn value =>
  match std::js::kind value with
  | #Null => "null"
  | #Undefined => "undefined"
  | #Bool => "bool"
  | #Number => "number"
  | #String => "string"
  | #Array => "array"
  | #Object => "object"
  | #Function => "function"
  | #Other => "other"
  end
let field = std::js::field
let element = std::js::element
let length = std::js::length
let keys = std::js::keys
let snapshot = std::js::snapshot
let call = std::js::apply
@doc "Forwarding a host value hands the host back the value it gave, not a copy."
let forward: std::js::Value -> std::js::Value = fn value => value
@private
@doc "Reading a snapshot is pure: this type carries no `!Host`."
let inert: std::js::Value -> std::result::Result { total: Nat } std::js::Error = std::js::read
let copy_then_read = fn value => match std::js::snapshot value with
| #Some copy => inert copy
| #Error error => #Error error
end
@doc "A live host value is refused by the pure reading, however plain it looks."
let read_live: std::js::Value -> String = fn value => match inert value with
| #Some _ => "read"
| #Error error => error.expected
end
@doc "A host primitive carries no getter, so the pure reading takes it."
let read_number: std::js::Value -> String = fn value => do
  let count: std::js::Value -> std::result::Result Nat std::js::Error = std::js::read
  return match count value with
  | #Some value => std::str::from_nat value
  | #Error error => error.expected
  end
end
let deep_total: std::js::Value -> std::result::Result Nat std::js::Error + std::js::!Host =
  fn value => match std::js::field "counts" value with
  | #Some counts => match std::js::element 1n counts with
    | #Some second => do
      let read: std::js::Value -> std::result::Result Nat std::js::Error + std::js::!Host = std::js::lift
      return read second
    end
    | #Error error => #Error error
    end
  | #Error error => #Error error
  end
"#,
            platform,
            None,
        );
        run(
            project.path(),
            r#"
assert.equal(await app.kind_of(null), 'null');
assert.equal(await app.kind_of(undefined), 'undefined');
assert.equal(await app.kind_of(true), 'bool');
assert.equal(await app.kind_of(1.5), 'number');
assert.equal(await app.kind_of('text'), 'string');
assert.equal(await app.kind_of([1]), 'array');
assert.equal(await app.kind_of({}), 'object');
assert.equal(await app.kind_of(() => 1), 'function');
assert.equal(await app.kind_of(Symbol('s')), 'other');
assert.equal(await app.kind_of(1n), 'other');

// Reading a live host value purely is refused whatever its shape, and a
// getter on it never runs. Taking a snapshot first is what makes the read
// pure, and a primitive was never observable.
let touched = 0;
const watched = { get total() { touched += 1; return 3; } };
assert.equal(await app.read_live(watched), 'a snapshot this program took');
assert.equal(touched, 0, 'plain getter');
const deep = { get counts() { touched += 1; return [1, 2]; } };
assert.equal(await app.read_live(deep), 'a snapshot this program took');
assert.equal(touched, 0, 'nested getter');
// A proxy with a catch-all trap also sees the shared calling convention ask
// whether an argument is one of this program's closures, which is a symbol
// key. That probe belongs to every extern call, not to reading; what matters
// here is that no field of the value is read.
let trapped = 0;
assert.equal(await app.read_live(new Proxy({ total: 1 }, { get(target, key) { if (typeof key === 'string') trapped += 1; return 1; } })), 'a snapshot this program took');
assert.equal(touched, 0, 'after proxy');
assert.equal(trapped, 0, 'no string key was read');
assert.equal(await app.read_number(7), '7');
assert.equal(some(await app.copy_then_read(watched)).total, 3);
assert.equal(touched, 1);

const record = { counts: [4, 9], name: 'Ada' };
assert.equal(some(await (await app.field('name'))(record)), 'Ada');
assert.equal(await app.kind_of(some(await (await app.field('missing'))(record))), 'undefined');
assert.equal(some(await (await app.element(1))([7, 8])), 8);
assert.equal(some(await app.length([7, 8])), 2);
assert.deepEqual(some(await app.keys(record)), ['counts', 'name']);
assert.equal(some(await app.deep_total(record)), 9);

// A call with the arguments it was given, and nothing bound as a receiver.
assert.equal(some(await (await app.call([2, 3]))((a, b) => a + b)), 5);
const target = {};
assert.equal(some(await (await app.call([target]))(value => value)), target);

// Forwarding keeps the host's own identity.
assert.equal(await app.forward(target), target);
assert.equal(await app.forward(record), record);

// A snapshot is a copy: the host can change the original afterwards.
const live = { total: 1, nested: { deep: [1, 2] } };
const copy = some(await app.snapshot(live));
assert.notEqual(copy, live);
assert.deepEqual(data(copy), data(live));
live.total = 2;
assert.equal(copy.total, 1);
assert.equal(data(some(await app.copy_then_read(live))).total, 2);
"#,
        );
    }
}

#[test]
fn host_failures_arrive_as_errors_rather_than_exceptions() {
    let project = project(
        r#"
let kind = std::js::kind
let text = std::js::text
let lift_text: std::js::Value -> std::result::Result String std::js::Error + std::js::!Host = std::js::lift
let field = std::js::field
let element = std::js::element
let length = std::js::length
let keys = std::js::keys
let snapshot = std::js::snapshot
let call = std::js::apply
"#,
        "node",
        None,
    );
    run(
        project.path(),
        r#"
// Reading a property runs whatever the host put there, so it can fail.
const raising = { get boom() { throw new Error('the getter said no'); } };
const raised = failure(await (await app.field('boom'))(raising));
assert.equal(raised.path, '$.boom');
assert.equal(raised.expected, 'a readable property');
assert.equal(raised.message, 'the getter said no');
assert.equal(failure(await (await app.field('a'))(null)).message, 'Cannot read a of null');
assert.equal(failure(await (await app.element(0))(undefined)).path, '$[0]');

// A proxy trap is observed, and its refusal is data too.
const names = [];
const proxy = new Proxy({ a: 1, hidden: 2 }, {
  get(target, name) { if (typeof name === 'string') names.push(name); if (name === 'hidden') throw new Error('trap refused'); return Reflect.get(target, name); },
  ownKeys() { return ['a']; },
  getOwnPropertyDescriptor(target, name) { return { value: target[name], enumerable: true, configurable: true }; },
});
assert.equal(some(await (await app.field('a'))(proxy)), 1);
assert.deepEqual(names, ['a']);
assert.equal(failure(await (await app.field('hidden'))(proxy)).message, 'trap refused');
assert.deepEqual(some(await app.keys(proxy)), ['a']);

// A length must be a natural number this target can hold.
assert.equal(some(await app.length('ab')), 2);
assert.equal(failure(await app.length({})).expected, 'a finite non-negative integer length');
assert.equal(failure(await app.length({ length: -1 })).expected, 'a finite non-negative integer length');
assert.equal(failure(await app.length({ length: 1.5 })).expected, 'a finite non-negative integer length');
assert.equal(failure(await app.length({ length: Infinity })).expected, 'a finite non-negative integer length');
assert.equal(failure(await app.keys(4)).message, 'A number has no keys');

// A snapshot copies plain data and refuses everything else.
assert.deepEqual(data(some(await app.snapshot({ a: [1, { b: 'c' }], d: null }))), { a: [1, { b: 'c' }], d: null });
const shared = { s: 1 };
assert.deepEqual(data(some(await app.snapshot([shared, shared]))), [{ s: 1 }, { s: 1 }]);
assert.equal(failure(await app.snapshot(() => 1)).message, 'A function at $ is not inert data');
assert.equal(failure(await app.snapshot({ at: () => 1 })).message, 'A function at $.at is not inert data');
const cyclic = { name: 'self' };
cyclic.again = cyclic;
assert.equal(failure(await app.snapshot(cyclic)).message, 'The value at $.again refers to itself');
assert.equal(failure(await app.snapshot(new Date())).message, 'The value at $ is a host object, not plain data');
assert.equal(failure(await app.snapshot({ when: new Map() })).message, 'The value at $.when is a host object, not plain data');
assert.equal(failure(await app.snapshot(Symbol('s'))).message, 'A symbol at $ is not inert data');

// A call refuses a value that is not callable, and reports a host throw.
assert.equal(failure(await (await app.call([]))({})).message, 'This value is not callable');
assert.equal(failure(await (await app.call([]))(() => { throw new Error('called and failed'); })).message, 'called and failed');

// A value that refuses to be read at all. A revoked proxy answers `typeof`
// and nothing else, so even deciding whether it is one of this program's
// functions would throw. Every operation still answers with data, and `kind`
// answers at all, which is what it promises.
const revoked = (() => { const handle = Proxy.revocable([], {}); handle.revoke(); return handle.proxy; })();
const revokedCallable = (() => { const handle = Proxy.revocable(function () {}, {}); handle.revoke(); return handle.proxy; })();
assert.equal(tag(await app.kind(revoked)), 'Other');
assert.equal(tag(await app.kind(revokedCallable)), 'Function');
assert.match(failure(await (await app.field('a'))(revoked)).message, /revoked/);
assert.match(failure(await app.keys(revoked)).message, /revoked/);
assert.match(failure(await app.snapshot(revoked)).message, /revoked/);
assert.match(failure(await (await app.call([]))(revokedCallable)).message, /revoked/);

// A thrown value that refuses to describe itself. Reading it to write the
// message is another observation, and it must not become a second failure
// outside the boundary that promised a recoverable one.
const silent = { get message() { throw new Error("message lookup failed"); } };
const hiding = { get boom() { throw silent; } };
const quiet = failure(await (await app.field("boom"))(hiding));
assert.equal(quiet.path, "$.boom");
assert.equal(quiet.expected, "a readable property");
assert.equal(quiet.message, "the host threw a value that cannot be read");

const uncoercible = { get message() { return undefined; }, [Symbol.toPrimitive]() { throw new Error("no"); } };
const coercing = { get boom() { throw uncoercible; } };
assert.equal(
  failure(await (await app.field("boom"))(coercing)).message,
  "the host threw a value that cannot be read"
);

// Host text is Unicode scalar values. An unpaired half is refused where text
// is taken in strictly, and repaired only where a caller asks for that.
const lone = "a" + String.fromCharCode(0xd800) + "b";
assert.equal(failure(await app.lift_text(lone)).expected, "String");
assert.equal(some(await app.lift_text("a😀b")), "a😀b");
const repaired = some(await app.text(lone));
assert.equal(repaired, "a\ufffdb");
assert.equal([...repaired].length, 3);
assert.equal(some(await app.text("a😀b")), "a😀b");
assert.equal(failure(await app.text(7)).message, "This value is not a string");

// An error message the host wrote with an unpaired half is repaired too: a
// message has to exist, and it has to be text.
const shouting = { get boom() { throw new Error("bad " + String.fromCharCode(0xdc00)); } };
const shouted = failure(await (await app.field("boom"))(shouting)).message;
assert.equal(shouted, "bad \ufffd");
assert.equal([...shouted].length, 5);
"#,
    );
}

#[test]
fn callbacks_keep_their_declared_effects_and_completion() {
    let project = project(
        r#"
@private
@doc "A Ruddy function the host calls, with the whole per-arrow type declared."
extern each: fn(fn(Nat) -> Nat, [Nat]) -> [Nat] = "(step, values) => values.map(step)"
let stepped: [Nat] -> [Nat] = fn values => each (fn value => std::nat::add value 1n) values

@private
effect Log = { note: String -> () }
@private
@doc "The callback's declared effect stays in the type, so a caller must handle it."
extern run_with: fn(fn(Nat) -> Nat + !Log) -> Nat + !Log = "step => step(21)"
let doubled: () -> String = fn _ => do
  let notes = mut ""
  let total = handle run_with (fn value => do
    let _ = !Log.note "doubling"
    return std::nat::multiply value 2n
  end) with
  | !Log.note text => do
    _ = notes := text
    return ()
  end
  end
  return std::str::concat (~notes) (std::str::from_nat total)
end

@private
@doc "Completion is a promise; the Ruddy call site still reads as an ordinary call."
@async
extern later: fn(fn(Nat) -> Nat, Nat) -> Nat =
  "(step, value) => new Promise(resolve => setTimeout(() => resolve(step(value)), 0))"
let eventually: Nat -> Nat = fn value => std::nat::add (later (fn n => std::nat::multiply n 2n) value) 1n

@private
@doc "A host function becomes a Ruddy one only through a declaration like this."
extern make_adder: fn(Nat) -> fn(Nat) -> Nat = "base => extra => base + extra"
let add_three: Nat -> Nat = make_adder 3n

@private
let lift_callable: std::js::Value -> std::result::Result (Nat -> Nat) std::js::Error
  + std::js::!Host = std::js::lift
let decoder_refuses: std::js::Value -> String + std::js::!Host =
  fn value => match lift_callable value with
| #Some _ => "accepted"
| #Error error => error.expected
end
"#,
        "node",
        None,
    );
    run(
        project.path(),
        r#"
// The host calls a Ruddy function it was handed.
assert.deepEqual(await app.stepped([1, 2, 3]), [2, 3, 4]);

// The callback performed its declared effect inside the caller's handler.
assert.equal(await app.doubled(), 'doubling42');

// The extern answers with a promise and the call site is ordinary.
assert.equal(await app.eventually(5), 11);

// A declaration accepts a callable in the other direction as well.
assert.equal(await (await app.add_three)(4), 7);

// The data decoder refuses the same callable the declarations accept.
assert.equal(await app.decoder_refuses(value => value), 'a verifiable data type; a function contract needs an adapter');
"#,
    );
}
