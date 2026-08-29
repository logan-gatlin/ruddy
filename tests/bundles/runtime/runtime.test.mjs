import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const generated = process.argv[2];
if (!generated) {
  throw new Error("the generated JavaScript module path was not provided");
}

globalThis.host = {
  counter: {
    base: 40,
    next(value) {
      return this.base + value;
    },
  },
};

const app = await import(pathToFileURL(generated).href);

test("exports values, closures, and file-backed modules", () => {
  assert.equal(app.answer, 42);
  assert.equal(app.identity("value"), "value");
  assert.equal(app.apply(app.identity)(7), 7);
  assert.equal(app.captured(20)(22), 42);
  assert.equal(app.Math.answer, 42);
  assert.equal(app.Math.identity(false), false);
  assert.equal(Object.getPrototypeOf(app.Math), null);
  assert.ok(Object.isFrozen(app.Math));
});

test("preserves records and pattern matching", () => {
  assert.equal(Object.getPrototypeOf(app.record), null);
  assert.deepEqual({ ...app.record }, { answer: 42, ready: true });
  assert.equal(app.classify_record(app.record), 42);
  assert.equal(app.classify_record({ ready: true }), 0);
  assert.equal(app.read_tag(app.tagged), 42);
  assert.equal(app.read_tag({}), 0);
});

test("runs effects and binds extern methods to their receiver", () => {
  assert.equal(app.bump(41), 42);
  assert.equal(app.next(2), 42);
});
