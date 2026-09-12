import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const cli = require.resolve("tree-sitter-cli/cli.js");
const grammars = {
  ruddy: { directory: fileURLToPath(new URL("../../treesitter/", import.meta.url)), extension: "rud" },
  sh: { directory: path.dirname(require.resolve("tree-sitter-bash/package.json")), extension: "sh" },
  json: { directory: path.dirname(require.resolve("tree-sitter-json/package.json")), extension: "json" },
  toml: { directory: path.dirname(require.resolve("@tree-sitter-grammars/tree-sitter-toml/package.json")), extension: "toml" },
};
const aliases = { rud: "ruddy", bash: "sh", shell: "sh" };

function grammarFor(language) {
  const name = Object.hasOwn(aliases, language) ? aliases[language] : language;
  return Object.hasOwn(grammars, name) ? grammars[name] : undefined;
}

function configContents(grammar) {
  const query = readFileSync(path.join(grammar, "queries/highlights.scm"), "utf8");
  // Register the query's capture names; the website supplies their colors.
  const captures = [...query.matchAll(/@([\w.]+)/g)].map((match) => match[1]);
  return JSON.stringify({
    "parser-directories": [grammar],
    theme: Object.fromEntries(captures.map((name) => [name, "#000000"])),
  });
}

function highlightedLines(table) {
  const lines = [...table.matchAll(/<td class=line>([\s\S]*?)<\/td>/g)];
  if (!lines.length) throw new Error("Tree-sitter returned no highlighted code");
  return lines.map((match) => match[1]).join("");
}

export function createHighlighter() {
  const caches = new Map();

  function prime(sources, language = "ruddy") {
    const grammar = grammarFor(language);
    if (!grammar) return;
    if (!caches.has(grammar)) caches.set(grammar, new Map());
    const cache = caches.get(grammar);
    const missing = [...new Set(sources)].filter((source) => source && !cache.has(source));
    if (!missing.length) return;

    const temporary = mkdtempSync(path.join(tmpdir(), "ruddy-highlight-"));
    try {
      const config = path.join(temporary, "config.json");
      writeFileSync(config, configContents(grammar.directory));
      const inputs = missing.map((source, index) => {
        const input = path.join(temporary, `${index}.${grammar.extension}`);
        writeFileSync(input, source);
        return input;
      });
      const paths = path.join(temporary, "paths.txt");
      writeFileSync(paths, inputs.join("\n"));
      const document = execFileSync(process.execPath, [
        cli, "highlight", "--html", "--css-classes",
        "--config-path", config, "--paths", paths,
        // Bash omits the query path from its manifest; supply it explicitly.
        ...(grammar === grammars.sh ? ["--query-paths", path.join(grammar.directory, "queries/highlights.scm")] : []),
      ], { cwd: grammar.directory, encoding: "utf8", maxBuffer: 50 * 1024 * 1024 });
      const tables = [...document.matchAll(/<table>([\s\S]*?)<\/table>/g)];
      if (tables.length !== missing.length) {
        throw new Error(`Tree-sitter returned ${tables.length} highlighted documents for ${missing.length} snippets`);
      }
      missing.forEach((source, index) => cache.set(source, highlightedLines(tables[index][1])));
    } finally {
      rmSync(temporary, { recursive: true, force: true });
    }
  }

  function highlight(source, language) {
    const grammar = grammarFor(language);
    if (!grammar || !source) return "";
    prime([source], language);
    return caches.get(grammar).get(source);
  }

  return { highlight, prime, clear: () => caches.clear() };
}
