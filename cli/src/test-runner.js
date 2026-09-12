let passed = 0;
let failed = 0;
for (const [name, path] of tests) {
  try {
    const test = path.reduce((module, key) => module[key], bundle);
    await test({});
    passed += 1;
    console.log(`PASS ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL ${name}: ${error instanceof Error ? error.stack : String(error)}`);
  }
}
console.log(`${tests.length} tests: ${passed} passed; ${failed} failed`);
if (failed !== 0) process.exitCode = 1;
