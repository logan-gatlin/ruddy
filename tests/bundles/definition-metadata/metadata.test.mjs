// Definition metadata, end to end: every attribute form the language accepts,
// on every kind of definition, compiled by the compiler and read back by the
// host — which sees exactly the program it would have seen without them.
import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const generated = process.argv[2];
if (!generated) {
  throw new Error("the generated JavaScript module path was not provided");
}

const app = await import(pathToFileURL(generated).href);

// Records are built without a prototype; copying into a plain object lets
// strict deep equality compare fields alone.
const fields = (record) => ({ ...record });

test("annotated definitions compute what unannotated ones would", async () => {
  assert.equal(app.add(2)(3), 5);
  assert.deepEqual(fields(app.pair), { first: 3, second: "three" });
  assert.equal(app.left, 8);
  assert.equal(app.right, "hello");
  assert.equal(await app.Inline.doubled(21), 42);
  assert.equal(app.Notes.greeting, "hello");
  assert.equal(app.quiet, 7);
});

test("metadata leaves no trace in the generated module", async () => {
  const exported = Object.keys(app).sort();
  assert.deepEqual(exported, [
    "Inline",
    "Notes",
    "add",
    "left",
    "pair",
    "quiet",
    "right",
  ]);
  for (const key of ["doc", "since", "stable", "owner", "k", "test", "metadata"]) {
    assert.equal(key in app, false, key);
  }
});
