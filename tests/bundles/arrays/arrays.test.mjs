// A seeded differential test of the array runtime against a plain JavaScript
// array: every operation the standard library exposes, at sizes that cross
// the leaf and node boundaries, with concatenations of uneven pieces to make
// the tree rebalance.
import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const generated = process.argv[2];
if (!generated) {
  throw new Error("the generated JavaScript module path was not provided");
}
const app = await import(pathToFileURL(generated).href);

// mulberry32: small, deterministic, and good enough to shuffle operations.
const seeded = (seed) => () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

const build = (values) => values.reduce((arr, value) => app.push(arr)(value), app.empty);
const contents = async (arr) => await app.contents(arr);
const same = async (arr, model, what) => {
  assert.equal(app.len(arr), model.length, `${what}: length`);
  assert.deepEqual(await contents(arr), model, `${what}: contents`);
  assert.equal(await app.get_or(arr)(model.length)(-1), -1, `${what}: past the end`);
};

test("pushes across the leaf and node boundaries", async () => {
  const model = [];
  let arr = app.empty;
  for (let i = 0; i < 33 * 33 + 5; i += 1) {
    model.push(i);
    arr = app.push(arr)(i);
    if ([31, 32, 33, 1023, 1024, 1025, 1088, 1089].includes(i)) await same(arr, model, `after push ${i}`);
  }
  await same(arr, model, "after every push");
});

test("sets, slices, prepends and pops agree with a plain array", async () => {
  const random = seeded(20260904);
  const pick = (n) => Math.floor(random() * n);
  let model = Array.from({ length: 1100 }, (_, i) => i);
  let arr = build(model);
  await same(arr, model, "the start");
  for (let step = 0; step < 300; step += 1) {
    const choice = pick(6);
    if (choice === 0 && model.length > 0) {
      const i = pick(model.length);
      const value = 100000 + step;
      const before = await contents(arr);
      arr = await app.set_or(arr)(i)(value);
      model = model.slice();
      model[i] = value;
      assert.equal(before[i] !== value, true, "set leaves the original alone");
    } else if (choice === 1) {
      const from = pick(model.length + 1);
      const to = from + pick(model.length - from + 1);
      assert.equal(await app.slice_ok(arr)(from)(to), true);
      arr = await app.slice_or(arr)(from)(to);
      model = model.slice(from, to);
    } else if (choice === 2) {
      const value = 200000 + step;
      arr = app.prepend(arr)(value);
      model = [value, ...model];
    } else if (choice === 3) {
      assert.equal(await app.pop_ok(arr), model.length > 0);
      if (model.length > 0) {
        assert.equal(await app.pop_last(arr)(-1), model[model.length - 1]);
        arr = await app.pop_rest(arr);
        model = model.slice(0, -1);
      }
    } else if (choice === 4) {
      const extra = Array.from({ length: pick(70) }, (_, i) => 300000 + step * 100 + i);
      arr = app.concat(arr)(build(extra));
      model = [...model, ...extra];
    } else {
      const value = 400000 + step;
      arr = app.push(arr)(value);
      model = [...model, value];
    }
    await same(arr, model, `step ${step} (choice ${choice})`);
  }
});

test("concatenation of uneven pieces stays correct and balanced", async () => {
  const random = seeded(7);
  const pick = (n) => Math.floor(random() * n);
  const whole = Array.from({ length: 4000 }, (_, i) => i);
  const base = build(whole);
  // Pieces cut at arbitrary points have partial leaves at both ends; joining
  // many of them is what the rebalancing exists for.
  let model = [];
  let arr = app.empty;
  for (let round = 0; round < 40; round += 1) {
    const from = pick(whole.length);
    const to = from + pick(Math.min(900, whole.length - from) + 1);
    const piece = await app.slice_or(base)(from)(to);
    await same(piece, whole.slice(from, to), `piece ${round}`);
    arr = round % 2 === 0 ? app.concat(arr)(piece) : app.concat(piece)(arr);
    model = round % 2 === 0 ? [...model, ...whole.slice(from, to)] : [...whole.slice(from, to), ...model];
    await same(arr, model, `join ${round}`);
  }
  // Everything still works after the joins: sets and pushes at the far end.
  arr = await app.set_or(arr)(0)(-7);
  model[0] = -7;
  arr = app.push(arr)(-8);
  model.push(-8);
  await same(arr, model, "after the joins");
});

test("slice rejects a backwards or overlong range and accepts the ends", async () => {
  const arr = build([1, 2, 3]);
  assert.equal(await app.slice_ok(arr)(2)(1), false);
  assert.equal(await app.slice_ok(arr)(0)(4), false);
  assert.equal(await app.slice_ok(arr)(3)(3), true);
  assert.deepEqual(await contents(await app.slice_or(arr)(3)(3)), []);
  assert.deepEqual(await contents(await app.slice_or(arr)(0)(3)), [1, 2, 3]);
  assert.equal(await app.pop_ok(app.empty), false);
  assert.deepEqual(await contents(app.prepend(app.empty)(9)), [9]);
  assert.deepEqual(await contents(app.concat(app.empty)(arr)), [1, 2, 3]);
  assert.deepEqual(await contents(app.concat(arr)(app.empty)), [1, 2, 3]);
});

test("array patterns take arrays apart from either end", async () => {
  const values = (n) => build(Array.from({ length: n }, (_, i) => i));
  assert.equal(await app.count(app.empty), 0);
  assert.equal(await app.count(values(1)), 1);
  assert.equal(await app.count(values(100)), 100);
  assert.equal(app.last_or(app.empty)(-1), -1);
  assert.equal(app.last_or(values(5))(-1), 4);
  assert.deepEqual(await contents(app.ends(values(5))), [0, 4]);
  assert.deepEqual(await contents(app.ends(values(2))), [0, 1]);
  assert.deepEqual(await contents(app.ends(values(1))), []);
  assert.deepEqual(await contents(app.init_of(values(5))), [0, 1, 2, 3]);
  assert.deepEqual(await contents(app.init_of(values(1))), []);
  assert.deepEqual(await contents(app.init_of(app.empty)), []);
  assert.deepEqual(await contents(app.init_of(values(40))), Array.from({ length: 39 }, (_, i) => i));
  assert.deepEqual(await contents(app.middle(values(5))), [1, 2, 3]);
  assert.deepEqual(await contents(app.middle(values(2))), []);
  assert.deepEqual(await contents(app.middle(values(1))), []);
  assert.equal(app.describe(app.empty), "empty");
  assert.equal(app.describe(build([0])), "zero");
  assert.equal(app.describe(build([1])), "one");
  assert.equal(app.describe(build([0, 0, 7])), "zeros");
  assert.equal(app.describe(build([0, 1])), "many");
  assert.equal(app.describe(build([3, 0, 0])), "many");
  // A rest bound past the tail boundary is the slice it names.
  assert.deepEqual(await contents(app.middle(values(70))), Array.from({ length: 68 }, (_, i) => i + 1));
});

test("spreads join arrays in place", async () => {
  const values = (n) => build(Array.from({ length: n }, (_, i) => i));
  assert.deepEqual(await contents(app.wrap(values(3))), [0, 1, 2, 0, 1, 2]);
  assert.deepEqual(await contents(app.around(values(2))(9)), [9, 0, 1, 9]);
  assert.deepEqual(await contents(app.around(app.empty)(9)), [9, 9]);
  assert.deepEqual(await contents(app.copy(values(40))), Array.from({ length: 40 }, (_, i) => i));
  assert.deepEqual(await contents(app.none(values(2))), [0, 1]);
  assert.deepEqual((await contents(app.wrap(values(1000)))).length, 2000);
});
