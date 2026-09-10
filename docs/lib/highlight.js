import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const cli = require.resolve("tree-sitter-cli/cli.js");
const grammar = fileURLToPath(new URL("../../treesitter/", import.meta.url));

export function createHighlighter() {
  const cache = new Map();

  function highlight(source, language) {
    if (language !== "ruddy" && language !== "rud") return "";
    if (!source) return "";
    if (cache.has(source)) return cache.get(source);

    const temporary = mkdtempSync(path.join(tmpdir(), "ruddy-highlight-"));
    try {
      const input = path.join(temporary, "snippet.rud");
      const config = path.join(temporary, "config.json");
      const query = readFileSync(path.join(grammar, "queries/highlights.scm"), "utf8");
      // Register the query's capture names; the website supplies their colors.
      const captures = [...query.matchAll(/@([\w.]+)/g)].map((match) => match[1]);
      writeFileSync(config, JSON.stringify({
        "parser-directories": [grammar],
        theme: Object.fromEntries(captures.map((name) => [name, "#000000"])),
      }));
      writeFileSync(input, source);
      const document = execFileSync(process.execPath, [
        cli, "highlight", "--html", "--css-classes",
        "--config-path", config, input,
      ], { cwd: grammar, encoding: "utf8", maxBuffer: 10 * 1024 * 1024 });

      // Tree-sitter 0.26 emits a table, with one escaped code cell per line.
      // Keep only those cells so the site's normal code-block layout is used.
      const lines = [...document.matchAll(/<td class=line>([\s\S]*?)<\/td>/g)];
      if (!lines.length) throw new Error("Tree-sitter returned no highlighted code");
      const html = lines.map((match) => match[1]).join("");
      cache.set(source, html);
      return html;
    } finally {
      rmSync(temporary, { recursive: true, force: true });
    }
  }

  return { highlight, clear: () => cache.clear() };
}
