import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

const events = [];
globalThis.host = { record: message => { events.push(message); return {}; } };
const app = await import(pathToFileURL(process.argv[2]));
assert.equal(await app.answer({}), 42);
assert.equal(await app.returned({}), 42);
assert.equal(await app.ordered({}), 22);
assert.deepEqual(events.splice(0), ['inner', 'outer', 'resumed', 'body']);
assert.equal(await app.invocation({}), 42);
assert.equal(await app.repeated({}), 42);
assert.deepEqual(events.splice(0), ['log', 'log', 'log', 'log']);
assert.equal(await app.abort({}), 99);
assert.deepEqual(events.splice(0), []);
assert.equal(await app.additional({}), 42);
