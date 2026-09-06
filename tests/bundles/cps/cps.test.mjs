import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const app = await import(pathToFileURL(process.argv[2]).href);
assert.equal(await app.count(100000), 0);
assert.equal(await app.sum(20000), 200010000);
assert.equal(await app.left(100001), 2);
assert.equal(await app.higher(100000), 0);
assert.equal(await app.handled(20000), 20000);

let interleaved = false;
queueMicrotask(() => { interleaved = true; });
assert.equal(app.count(10000), 0);
assert.equal(interleaved, false, 'driver transfers alone do not yield to the event loop');

// A pending frame per tail call exceeds this heap bound long before completion.
const { spawnSync } = await import('node:child_process');
const bounded = spawnSync(process.execPath, ['--max-old-space-size=48', '--stack-size=256', '--input-type=module', '--eval',
  `const app = await import(${JSON.stringify(pathToFileURL(process.argv[2]).href)}); if (await app.count(2000000) !== 0) process.exit(2);`],
  { timeout: 30000, encoding: 'utf8' });
assert.equal(bounded.error, undefined);
assert.equal(bounded.status, 0, bounded.stderr);
assert.equal(await app.recursive_handler(20_000), 20_000, 'recursive handler evaluations have distinct exit identities');
