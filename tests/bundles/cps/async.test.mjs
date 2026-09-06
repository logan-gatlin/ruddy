import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
const pending = [];
const completions = [];
const reports = [];
globalThis[Symbol.for('ruddy.runtime')] = { onUnhandledError: error => reports.push(error) };
globalThis.observations = 0;
globalThis.host = {
  keepFactory: callback => { globalThis.factoryCallback = callback; },
  keepSync: callback => { globalThis.syncCallback = callback; },
  observed: value => { globalThis.observations++; return value; },
  keepExit: callback => { globalThis.exitCallback = callback; },
  keepReader: callback => { globalThis.readerCallback = callback; },
  runExit: callback => callback(7),
  remember: callback => { globalThis.saved = callback; },
  watch: callback => { globalThis.watched = callback; },
  keepCompleted: callback => { globalThis.completedCallback = callback; },
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

const immediate = app.via_callback(0);
assert.ok(immediate instanceof Promise, 'fixed Promise export even for immediate completion');
assert.equal(await immediate, 11);
await assert.rejects(app.via_callback(-1), /first failure/);
const later = app.via_callback(1);
const delivery = completions.pop();
delivery.ok(30); delivery.bad(new Error('late')); delivery.ok(40);
assert.equal(await later, 31);

app.notify(10);
assert.equal(globalThis.watched(1), undefined);
pending.pop().bad(new Error('notification failure'));
// Observe the reporter explicitly, without arbitrary timer delays.
await new Promise(resolve => {
  globalThis[Symbol.for('ruddy.runtime')].onUnhandledError = error => { reports.push(error); resolve(); };
});
assert.equal(reports[0].message, 'notification failure');
app.save_completed(20);
const delivered = new Promise((resolve, reject) => globalThis.completedCallback(2, resolve, reject));
pending.pop().ok();
assert.equal(await delivered, 23);

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
app.notify_failure(null);
assert.equal(globalThis.watched(0), undefined);
assert.equal(reports.at(-1).message, 'immediate failure');
assert.equal(await app.translation(3), 13);
assert.ok(app.fixed_promise(3) instanceof Promise);
assert.equal(await app.fixed_promise(4), 4);
assert.equal(typeof app.immediate_data(null), 'object', 'immediate data is never probed for then');
assert.equal(await app.registrations(100_000), 0, 'synchronous registrations do not recursively reenter the driver');

for (const delayed of [false, true]) {
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', `
    globalThis.host = {
      delayed: () => Promise.reject(new Error('uncaught delayed notification')),
      watch: callback => { globalThis.watched = callback; }
    };
    const app = await import(${JSON.stringify(pathToFileURL(process.argv[2]).href)});
    app.${delayed ? 'notify' : 'notify_failure'}(0);
    globalThis.watched(1);
  `], { encoding: 'utf8', timeout: 10_000 });
  assert.ifError(child.error);
  assert.notEqual(child.status, 0, 'default notification reporter is an uncaught host failure');
  assert.match(child.stderr, delayed ? /uncaught delayed notification/ : /immediate failure/);
}

const nestedHandlers = app.nested_handlers(2);
const secondOperation = new Promise(resolve => { globalThis.onRegistration = resolve; });
pending.pop().ok();
await secondOperation;
globalThis.onRegistration = undefined;
pending.pop().ok();
assert.equal(await nestedHandlers, 14, 'resumption restores evidence across nested handlers');
