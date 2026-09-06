import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const events = [];
const waiting = [];
let wake;
const next = () => waiting.length ? Promise.resolve(waiting.shift()) : new Promise(resolve => { wake = resolve; });
globalThis.host = {
  mark: event => events.push(event),
  delay: value => new Promise((resolve, reject) => {
    const request = { value, resolve, reject };
    if (wake) { const notify = wake; wake = null; notify(request); }
    else waiting.push(request);
  }),
};
const url = pathToFileURL(process.argv[2]);
let ready = false;
const loading = import(url).then(app => { ready = true; return app; });
const dependency = await next();
assert.equal(dependency.value, 10);
assert.deepEqual(events, ['dep:start']);
assert.equal(ready, false);
dependency.resolve(11);
const root = await next();
assert.equal(root.value, 20);
assert.deepEqual(events, ['dep:start', 'dep:ready']);
assert.equal(ready, false);
root.resolve(21);
const app = await loading;
assert.equal(app.ready, 21);
assert.deepEqual(events, ['dep:start', 'dep:ready', 'root:ready']);
const call = app.read(30);
const operation = await next();
assert.equal(operation.value, 30);
operation.resolve(31);
assert.equal(await call, 32);

const reader = app.nested(40);
assert.equal(typeof reader, 'function', 'imported factory stays synchronous');
const nestedResult = reader(2);
assert.ok(nestedResult instanceof Promise);
assert.equal(await nestedResult, 42, 'nested boundary adapters survive artifact serialization and linking');

// Initialization failure rejects readiness before later declarations run.
events.length = 0;
url.search = '?failure';
const failing = import(url);
const rejection = assert.rejects(failing, /initialization failure/);
(await next()).reject(new Error('initialization failure'));
await rejection;
assert.deepEqual(events, ['dep:start']);
