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

test("supports incomplete snippets and leaves other fence languages to Markdown", () => {
  assert.equal(textContent(highlight("let unfinished =\n", "ruddy")), "let unfinished =\n");
  assert.equal(highlight("<script>", "sh"), "");
  assert.equal(highlight("", "ruddy"), "");
});
