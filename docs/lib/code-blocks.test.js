import assert from "node:assert/strict";
import test from "node:test";
import MarkdownIt from "markdown-it";
import { addCodeFilenames } from "./code-blocks.js";

function render(source) {
  const markdown = new MarkdownIt();
  addCodeFilenames(markdown);
  return markdown.render(source);
}

test("filename bars preserve code and language independently of the label", () => {
  const html = render('```ruddy filename="src/main.rud"\nlet main = fn _ => ()\n```');
  assert.match(html, /class="code-filename">src\/main\.rud<\/span>/);
  assert.match(html, /<code class="language-ruddy">let main = fn _ =&gt; \(\)\n<\/code>/);
  assert.equal((html.match(/copyable-code/g) ?? []).length, 1);
});

test("filenames are escaped text, including quotes and markup", () => {
  const html = render('```text filename=\'<img src=x onerror="bad"> & file\'\n<source>\n```');
  assert.ok(html.includes('&lt;img src=x onerror=&quot;bad&quot;&gt; &amp; file'));
  assert.ok(html.includes('&lt;source&gt;'));
  assert.ok(!html.includes('<img'));
});

test("ordinary and unsupported fences retain their default rendering", () => {
  const source = '```sh\necho hello\n```\n\n```unrecognized filename="notes.txt"\nplain text\n```';
  const html = render(source);
  assert.ok(html.startsWith('<pre><code class="language-sh">echo hello\n</code></pre>'));
  assert.match(html, /class="code-filename">notes\.txt/);
  assert.match(html, /class="language-unrecognized">plain text/);
});
