// Struct spreads, end to end: every form the language accepts, built by the
// compiler and read back by the host, with the evaluation order observed
// through host functions that log their calls.
import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const generated = process.argv[2];
if (!generated) {
  throw new Error("the generated JavaScript module path was not provided");
}

const log = [];
globalThis.host = {
  mark(value) {
    log.push(value);
    return value;
  },
  markBase(value) {
    log.push("base");
    return value;
  },
};

const app = await import(pathToFileURL(generated).href);

// Records are built without a prototype; copying into a plain object lets
// strict deep equality compare fields alone.
const fields = (record) => ({ ...record });

test("a spread copies, extends and replaces without dropping a field", () => {
  assert.deepEqual(fields(app.copy), { a: 1, b: "b", c: true });
  assert.notEqual(app.copy, app.base);
  assert.deepEqual(fields(app.extended), { a: 1, b: "b", c: true, d: 2 });
  assert.deepEqual(fields(app.replaced), { a: 5, b: "b", c: true });
  assert.deepEqual(fields(app.retyped), { a: "text", b: "b", c: true });
  assert.deepEqual(fields(app.several), { a: 3, b: "b", c: false, e: 9 });
  assert.deepEqual(fields(app.computed), { a: 1, b: "b", c: true, d: 4 });
  // The value spread is left as it was.
  assert.deepEqual(fields(app.base), { a: 1, b: "b", c: true });
});

test("unit, the empty struct and tuples spread like any struct", () => {
  assert.deepEqual(fields(app.from_unit), { a: 1 });
  assert.deepEqual(fields(app.from_empty), {});
  assert.deepEqual(fields(app.from_pair), { 0: 1, 1: "second" });
});

test("one updater serves a value with the field and one without it", () => {
  assert.deepEqual(fields(app.added), { a: 7, b: 1 });
  assert.deepEqual(fields(app.overwritten), { a: 7, b: 2 });
  assert.deepEqual(fields(app.set_a(app.base)), { a: 7, b: "b", c: true });
});

test("every retained field reads back", () => {
  assert.deepEqual(fields(app.read_back(app.several)), { 0: 3, 1: "b", 2: false });
  assert.deepEqual(fields(app.read_back(app.computed)), { 0: 1, 1: "b", 2: true });
  assert.deepEqual(fields(app.read_added({ ...app.several, ...app.computed })), { 0: 4, 1: 9 });
  assert.equal(app.several.e, 9);
  assert.equal(app.computed.d, 4);
  assert.equal(app.extended.d, 2);
});

test("fields run left to right, the spread last and once, and still lose to the fields", () => {
  const result = app.traced({ a: 100, b: 200 });
  assert.deepEqual(log, [10, 30, "base"]);
  assert.deepEqual(fields(result), { a: 10, b: 200, c: 30 });
  app.traced({ a: 1, b: 2 });
  assert.deepEqual(log, [10, 30, "base", 10, 30, "base"]);
});
