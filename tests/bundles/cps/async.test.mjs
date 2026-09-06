import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const pending = [];
const completions = [];
globalThis.observations = 0;
globalThis.dataInspections = 0;
globalThis.host = {
  useNested: callback => {
    globalThis.nestedCallback = callback;
  },
  keepData: callback => { globalThis.dataCallback = callback; },
  keepFactory: callback => { globalThis.factoryCallback = callback; },
  keepSync: callback => { globalThis.syncCallback = callback; },
  observed: value => { globalThis.observations++; return value; },
  keepExit: callback => { globalThis.exitCallback = callback; },
  keepReader: callback => { globalThis.readerCallback = callback; },
  runExit: callback => callback(7),
  remember: callback => { globalThis.saved = callback; },
  delayed: value => new Promise((resolve, reject) => {
    pending.push({ ok: () => resolve(value + 1), bad: reject });
    globalThis.onRegistration?.();
  }),
  completed: (value, ok, bad) => {
    if (value === 0) { ok(10); ok(20); bad(new Error('late')); throw new Error('later'); }
    else if (value < 0) { bad(new Error('first failure')); ok(20); }
    else completions.push({ ok, bad });
  },
};
const app = await import(pathToFileURL(process.argv[2]));
const first = app.read(4);
assert.ok(first instanceof Promise);
assert.equal(pending.length, 1);
pending.shift().ok();
assert.equal(await first, 6);

app.save(10);
const left = globalThis.saved(1);
const right = globalThis.saved(2);
assert.ok(left instanceof Promise);
assert.ok(right instanceof Promise);
assert.equal(pending.length, 2);
pending.pop().ok();
assert.equal(await right, 13);
pending.pop().ok();
assert.equal(await left, 12);
const again = globalThis.saved(3);
pending.pop().ok();
assert.equal(await again, 14);

const aborted = app.abort(4);
pending.pop().ok();
assert.equal(await aborted, 5, 'raise bypasses the return arm');
const normal = app.normal(4);
pending.pop().ok();
assert.equal(await normal, 105, 'normal completion runs the return arm');
const rejected = app.normal(4);
const rejection = assert.rejects(rejected, /delayed failure/);
pending.pop().bad(new Error('delayed failure'));
await rejection;

// Callback-based host APIs use an explicit JS Promise wrapper at the extern.
const immediate = app.via_wrapped_callback(0);
assert.ok(immediate instanceof Promise, 'Promise export even for an already settled Promise');
assert.equal(await immediate, 11);
await assert.rejects(app.via_wrapped_callback(-1), /first failure/);
const later = app.via_wrapped_callback(1);
const delivery = completions.pop();
delivery.ok(30); delivery.bad(new Error('late')); delivery.ok(40);
assert.equal(await later, 31);

app.save(10);
const failedCallback = globalThis.saved(1);
assert.ok(failedCallback instanceof Promise);
const callbackRejection = assert.rejects(failedCallback, /callback failure/);
pending.pop().bad(new Error('callback failure'));
await callbackRejection;

assert.equal(await app.expired(null), 99);
await assert.rejects(globalThis.exitCallback(7), /invalid handler exit/);
const living = app.live(null);
await assert.rejects(globalThis.exitCallback(8), /invalid handler exit/);
pending.pop().ok();
assert.equal(await living, 2, 'independent callback cannot abort its suspended origin');
assert.equal(await app.inline_exit(null), 7, 'inline sync callback may exit its enclosing handler');
await app.ordinary_evidence(20);
assert.equal(await globalThis.readerCallback(2), 22, 'ordinary captured evidence outlives the handler');
assert.equal(await globalThis.readerCallback(3), 23);
const nested = app.record.run(40);
pending.pop().ok();
assert.equal(await nested, 41, 'functions stored inside exported values remain callable');

app.fresh(10);
const freshLeft = globalThis.saved(1);
const freshRight = globalThis.saved(2);
pending.pop().ok();
assert.equal(await freshRight, 13);
pending.pop().ok();
assert.equal(await freshLeft, 12, 'each callback owns a distinct live handler');

app.factory(10);
const returned = globalThis.factoryCallback(2);
assert.equal(typeof returned, 'function');
const result = returned(3);
assert.ok(result instanceof Promise);
pending.pop().ok();
assert.equal(await result, 16);
const reused = returned(4);
pending.pop().ok();
assert.equal(await reused, 17);

app.unsafe_sync(null);
assert.throws(() => globalThis.syncCallback(9), /synchronous callback attempted to suspend/);
pending.pop().ok();
await new Promise(resolve => queueMicrotask(() => queueMicrotask(resolve)));
assert.equal(globalThis.observations, 0, 'late completion cannot restart a failed invocation');
app.save_failure(null);
const immediateFailure = globalThis.saved(0);
assert.ok(immediateFailure instanceof Promise, 'synchronous failure still returns a Promise');
await assert.rejects(immediateFailure, /immediate failure/);
assert.equal(await app.translation(3), 13);
assert.ok(app.fixed_promise(3) instanceof Promise);
assert.equal(await app.fixed_promise(4), 4);
assert.equal(typeof app.immediate_data(null), 'object', 'immediate data is never probed for then');
assert.equal(await app.settled_promises(100_000), 0, 'already settled Promises do not grow the driver stack');

const nestedHandlers = app.nested_handlers(2);
const secondOperation = new Promise(resolve => { globalThis.onRegistration = resolve; });
pending.pop().ok();
await secondOperation;
globalThis.onRegistration = undefined;
pending.pop().ok();
assert.equal(await nestedHandlers, 14, 'resumption restores evidence across nested handlers');

app.save_data(null);
const ordinaryData = globalThis.dataCallback(1);
assert.equal(typeof Object.getOwnPropertyDescriptor(ordinaryData, 'then').get, 'function',
  'synchronous callbacks never assimilate ordinary returned data');
assert.equal(globalThis.dataInspections, 0, 'runtime must not inspect thenable data');

const reader = app.returned_host(20);
assert.equal(typeof reader, 'function', 'factory itself completes immediately');
const readResult = reader(2);
assert.ok(readResult instanceof Promise);
assert.equal(await readResult, 22);

app.nested_callbacks(10);
const factoryResult = globalThis.nestedCallback(n => Promise.resolve(n * 2));
assert.ok(factoryResult instanceof Promise);
const nestedCallback = await factoryResult;
const nestedResult = nestedCallback(3);
assert.ok(nestedResult instanceof Promise);
assert.equal(await nestedResult, 26, 'annotations follow each direction through nested callbacks');
await assert.rejects((await globalThis.nestedCallback(() => Promise.reject(new Error('nested failure'))))(1), /nested failure/);

const syncFactoryResult = app.returned_sync(30);
assert.ok(syncFactoryResult instanceof Promise);
const syncResult = await syncFactoryResult;
assert.equal(syncResult(4), 34, 'async metadata does not propagate into returned functions');
