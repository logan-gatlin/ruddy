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
    add(left, right) {
      return this.base + left + right;
    },
    nullary() {
      return this.base + 2;
    },
  },
  curriedAdd(left) {
    return (right) => left + right;
  },
  applyPair(callback, left, right) {
    return callback(left, right);
  },
  makeAdder(offset) {
    return (left, right) => offset + left + right;
  },
  tick(...args) {
    assert.equal(args.length, 1, "effect evidence crossed the foreign boundary");
    return args[0];
  },
  runTick(...args) {
    assert.equal(args.length, 2, "effect evidence crossed the foreign boundary");
    const [callback, value] = args;
    return callback(value);
  },
  runReturnedTick(...args) {
    assert.equal(args.length, 3, "effect evidence crossed the foreign boundary");
    const [callback, first, second] = args;
    return callback(first)(second);
  },
  invokeConditionalShared(...args) {
    assert.equal(args.length, 1, "conditional/shared-tail evidence crossed the foreign boundary");
    return args[0]();
  },
  loop() {
    return this.loop.bind(this);
  },
  runLoop(callback) {
    callback()();
    return 42;
  },
};

const app = await import(pathToFileURL(generated).href);

test("exports values, closures, and file-backed modules", async () => {
  assert.equal(app.answer, 42);
  assert.equal(app.identity("value"), "value");
  assert.equal(await app.apply(app.identity)(7), 7);
  assert.equal(app.captured(20)(22), 42);
  assert.equal(app.Math.answer, 42);
  assert.equal(app.Math.identity(false), false);
  assert.equal(Object.getPrototypeOf(app.Math), null);
  assert.ok(Object.isFrozen(app.Math));
});

test("preserves records and pattern matching", async () => {
  assert.equal(Object.getPrototypeOf(app.record), null);
  assert.deepEqual({ ...app.record }, { answer: 42, ready: true });
  assert.equal(app.classify_record(app.record), 42);
  assert.equal(app.classify_record({ ready: true }), 0);
  assert.equal(app.read_tag(app.tagged), 42);
  assert.equal(app.read_tag({}), 0);
});

test("runs effects and binds raw and marked extern adapters to their receiver", async () => {
  assert.equal(await app.bump(41), 42);
  assert.equal(app.next(2), 42);
  // `add` has a generated n-ary `#extern` adapter, unlike the raw unary `next`.
  assert.equal(app.added, 62);
  assert.equal(app.add_twenty(2), 62);
});

test("adapts n-ary, nullary, curried, callback, and returned extern functions", async () => {
  assert.equal(app.nullary_answer, 42);
  assert.equal(app.curried_answer, 42);
  assert.equal(app.callback_answer, 42);
  assert.equal(app.nested_result, 42);
  assert.equal(app.ticked, 42);
  assert.equal(app.callback_ticked, 42);
  assert.equal(app.returned_callback_ticked, 42);
  assert.equal(app.conditional_shared_result, 42);
  assert.equal(app.host_looped, 42);
  assert.equal(app.ruddy_looped, 42);
});
