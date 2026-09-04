// Do blocks, end to end: every form the language accepts, built by the
// compiler and read back by the host, with the order statements run in
// observed through a host function that logs its calls.
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
};

const app = await import(pathToFileURL(generated).href);

// Records are built without a prototype; copying into a plain object lets
// strict deep equality compare fields alone.
const fields = (record) => ({ ...record });

test("a block is what its return carries, and unit without one", () => {
  assert.deepEqual(fields(app.several), { first: 1, second: 1 });
  assert.deepEqual(fields(app.empty), {});
  assert.equal(app.only, 7);
  assert.equal(app.nested, 2);
  assert.deepEqual(fields(app.unit_block(9)), {});
});

test("a binding sees the ones before it, itself, and shadows an earlier name", () => {
  assert.equal(app.self_recursive, "done");
  assert.deepEqual(fields(app.taken_apart), { x: 1, y: 2, v: 3 });
  assert.deepEqual(fields(app.shadowed), { was: 1 });
});

test("statements run top to bottom, whether or not their values are kept", () => {
  log.length = 0;
  assert.equal(app.ordered(0), 3);
  assert.deepEqual(log, [1, 2, 3]);
  app.unit_block(9);
  assert.deepEqual(log, [1, 2, 3, 9]);
});

test("effects performed by a block's statements reach the handler in order", () => {
  log.length = 0;
  assert.equal(app.handled(0), 30);
  assert.deepEqual(log, [10, 20]);
});
