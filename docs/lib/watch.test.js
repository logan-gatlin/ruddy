import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { test } from "node:test";
import Eleventy from "@11ty/eleventy";

test("editing Markdown rebuilds its content with external grammar watch targets", async () => {
  const input = await mkdtemp(path.resolve(".watch-test-"));
  const output = await mkdtemp(path.join(tmpdir(), "ruddy-docs-watch-"));
  const source = path.join(input, "index.md");
  const page = path.join(output, "index.html");
  const eleventy = new Eleventy(path.relative(process.cwd(), input), output, {
    configPath: "./eleventy.config.js",
    quietMode: true,
    source: "cli",
  });
  try {
    await writeFile(source, "---\ndoc: true\n---\n# Before edit\n");
    await eleventy.init();
    await eleventy.watch();
    assert.match(await readFile(page, "utf8"), /Before edit/);
    await writeFile(source, "---\ndoc: true\n---\n# After edit\n");
    const deadline = Date.now() + 5000;
    while (!(await readFile(page, "utf8")).includes("After edit") && Date.now() < deadline) {
      await delay(100);
    }
    assert.match(await readFile(page, "utf8"), /After edit/);
  } finally {
    await eleventy.stopWatch();
    await rm(input, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
});
