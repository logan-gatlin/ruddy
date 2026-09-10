import assert from 'node:assert/strict';
import path from 'node:path';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { HighlightEngine } from '../src/highlighting';

test('bundled grammar highlights Ruddy roles without a language server', async () => {
  const engine = await HighlightEngine.load(path.resolve('dist'));
  const document = engine.open('let greet = fn name => name\nlet value = #Some 42n');
  try {
    const tokens = document.tokens();
    assert.ok(tokens.some(t => t.type === 'keyword' && t.line === 0 && t.start === 0 && t.length === 3));
    assert.ok(tokens.some(t => t.type === 'function' && t.line === 0 && t.start === 4 && t.length === 5));
    assert.ok(tokens.some(t => t.type === 'parameter' && t.line === 0 && t.start === 15 && t.length === 4));
    assert.ok(tokens.some(t => t.type === 'ruddyTag' && t.line === 1 && t.start === 12 && t.length === 5));
  } finally { document.dispose(); engine.dispose(); }
});

test('nested comments and quoted fields produce non-overlapping, themeable tokens', async () => {
  const engine = await HighlightEngine.load(path.resolve('dist'));
  const source = '(* outer\r\n (* inner *)\r\n*)\r\nlet record = { "😀": 1n }\r\n@private let value = !Log\r\n';
  const document = engine.open(source);
  try {
    const tokens = document.tokens();
    assert.deepEqual(tokens.filter(t => t.line < 3).map(t => t.type), ['comment', 'comment', 'comment']);
    assert.ok(tokens.some(t => t.line === 3 && t.start === 15 && t.length === 4 && t.type === 'property'), 'quoted field overrides the general string capture using UTF-16 columns');
    assert.ok(tokens.some(t => t.line === 4 && t.type === 'ruddyAttribute'));
    assert.ok(tokens.some(t => t.line === 4 && t.type === 'namespace'));
    for (const [index, token] of tokens.entries()) {
      assert.ok(token.length > 0);
      assert.ok(token.start + token.length <= source.split('\n')[token.line]!.replace(/\r$/, '').length);
      const previous = tokens[index - 1];
      if (previous?.line === token.line) assert.ok(previous.start + previous.length <= token.start);
    }
  } finally { document.dispose(); engine.dispose(); }
});

test('repeated edits of the language demo preserve fresh-parse highlighting, including error recovery', async () => {
  const engine = await HighlightEngine.load(path.resolve('dist'));
  let source = await readFile('../../demo.rud', 'utf8');
  const document = engine.open(source);
  let seed = 17;
  try {
    for (let iteration = 0; iteration < 50; iteration++) {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      const offset = seed % source.length;
      const length = Math.min(iteration % 5, source.length - offset);
      const text = ['\n', '(*', '*)', '😀', 'let x = 42n\n', ''][iteration % 6]!;
      document.update([{ rangeOffset: offset, rangeLength: length, text }]);
      source = source.slice(0, offset) + text + source.slice(offset + length);
      const fresh = engine.open(source);
      try { assert.deepEqual(document.tokens(), fresh.tokens(), `edit ${iteration} at ${offset}`); }
      finally { fresh.dispose(); }
    }
  } finally { document.dispose(); engine.dispose(); }
});

test('incremental insertions, deletions, multiline and simultaneous Unicode edits match a fresh parse', async () => {
  const engine = await HighlightEngine.load(path.resolve('dist'));
  let source = 'let café = "😀"\r\nlet value = #Some 42n\r\n(* outer (* inner *) comment *)\r\n';
  const document = engine.open(source);
  const batches = [
    [{ rangeOffset: source.indexOf('42n'), rangeLength: 3, text: '123n' }],
    [{ rangeOffset: 0, rangeLength: 0, text: 'let identity = fn x => x\n' }],
    [{ rangeOffset: 0, rangeLength: 24, text: '' }],
    [{ rangeOffset: 4, rangeLength: 4, text: '名前' }, { rangeOffset: 12, rangeLength: 2, text: '🌳🌳' }],
    [{ rangeOffset: 0, rangeLength: 0, text: '(*\n' }],
    [{ rangeOffset: 0, rangeLength: 3, text: '' }],
  ];
  try {
    for (const edits of batches) {
      document.update(edits);
      for (const edit of [...edits].sort((a, b) => b.rangeOffset - a.rangeOffset)) {
        source = source.slice(0, edit.rangeOffset) + edit.text + source.slice(edit.rangeOffset + edit.rangeLength);
      }
      const fresh = engine.open(source);
      try { assert.deepEqual(document.tokens(), fresh.tokens(), `after ${JSON.stringify(edits)}`); }
      finally { fresh.dispose(); }
    }
    document.update([{ rangeOffset: 0, rangeLength: source.length, text: '' }]);
    assert.deepEqual(document.tokens(), []);
  } finally { document.dispose(); engine.dispose(); }
});
