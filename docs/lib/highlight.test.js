import assert from "node:assert/strict";
import { test } from "node:test";
import { createHighlighter } from "./highlight.js";

const { highlight } = createHighlighter();

function textContent(html) {
  return html.replace(/<[^>]*>/g, "").replace(
    /&(amp|lt|gt|quot|#39);/g,
    (_, entity) => ({ amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'" })[entity],
  );
}

test("uses the grammar's contextual and local-name captures", () => {
  const source = "let identity = fn item => item\nlet name = fn person => person.name\n";
  const html = highlight(source, "ruddy");
  assert.match(html, /class='keyword'>let<\/span>/);
  assert.match(html, /class='function'>identity<\/span>/);
  assert.equal([...html.matchAll(/class='variable parameter'>item<\/span>/g)].length, 2);
  assert.match(html, /class='property'>name<\/span>/);
  assert.equal(textContent(html), source);
});

test("preserves Unicode, blank lines, multiline strings, and HTML-like source", () => {
  const source = 'let café = "<script>&</script>"\n\nlet message =\n  \\\\Hello\n  \\\\世界\n';
  const html = highlight(source, "rud");
  assert.equal(textContent(html), source);
  assert.ok(!html.includes("<script>"));
  assert.match(html, /class='string'/);
});

test("supports incomplete snippets and leaves unsupported fence languages to Markdown", () => {
  assert.equal(textContent(highlight("let unfinished =\n", "ruddy")), "let unfinished =\n");
  assert.equal(highlight("<script>", "text"), "");
  assert.equal(highlight("", "ruddy"), "");
});

test("primes several snippets for highlighting together", () => {
  const first = "let first = 1n\n";
  const second = "let second = 2n\n";
  const highlighter = createHighlighter();

  highlighter.prime([first, second]);

  assert.equal(textContent(highlighter.highlight(first, "ruddy")), first);
  assert.equal(textContent(highlighter.highlight(second, "ruddy")), second);
});

test("highlights shell commands, variables, and strings through all shell aliases", () => {
  const source = 'export NAME="世界 <script>&"\n\necho "$NAME"\n';
  for (const language of ["sh", "bash", "shell"]) {
    const html = highlight(source, language);
    assert.match(html, /class='keyword'>export<\/span>/);
    assert.match(html, /class='function'>echo<\/span>/);
    assert.match(html, /class='property'>NAME<\/span>/);
    assert.match(html, /class='string'/);
    assert.equal(textContent(html), source);
    assert.ok(!html.includes("<script>"));
  }
});

test("highlights TOML tables, properties, and values", () => {
  const source = '[package]\nname = "café <script>&"\n\nenabled = true\ncount = 42\n';
  const html = highlight(source, "toml");
  assert.match(html, /class='type'>package<\/span>/);
  assert.match(html, /class='property'>name /);
  assert.match(html, /class='boolean'>true<\/span>/);
  assert.match(html, /class='number'>42<\/span>/);
  assert.equal(textContent(html), source);
  assert.ok(!html.includes("<script>"));
});

test("highlights JSON keys and literals", () => {
  const source = '{\n  "name": "世界 <script>&",\n\n  "count": 42, "enabled": true, "empty": null\n}\n';
  const html = highlight(source, "json");
  assert.match(html, /class='string'>&quot;name&quot;<\/span>/);
  assert.match(html, /class='number'>42<\/span>/);
  assert.match(html, /class='constant builtin'>true<\/span>/);
  assert.match(html, /class='constant builtin'>null<\/span>/);
  assert.equal(textContent(html), source);
  assert.ok(!html.includes("<script>"));
});

test("keeps primed snippets separate by language and can clear them", () => {
  const highlighter = createHighlighter();
  const source = 'echo "hello"\n';
  highlighter.prime([source, source], "sh");
  highlighter.prime([source], "ruddy");
  const shell = highlighter.highlight(source, "sh");
  const ruddy = highlighter.highlight(source, "ruddy");
  assert.notEqual(shell, ruddy);
  assert.equal(textContent(shell), source);
  assert.equal(textContent(ruddy), source);
  highlighter.clear();
  assert.equal(highlighter.highlight(source, "bash"), shell);
  for (const language of ["text", "", "unknown", "constructor", "__proto__"]) {
    highlighter.prime([source], language);
    assert.equal(highlighter.highlight(source, language), "");
  }
});
