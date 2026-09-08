import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

const events = [];
globalThis.host = { record: n => { events.push(n); return {}; } };
const app = await import(pathToFileURL(process.argv[2]));
assert.equal(await app.run({}), 42);
assert.deepEqual(events, [1, 2]);
assert.equal(await app.pure({}), 7);
