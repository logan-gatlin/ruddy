import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { Edit, Language, Parser, Query, type Tree } from 'web-tree-sitter';

export const tokenTypes = [
  'variable', 'type', 'namespace', 'function', 'parameter', 'property',
  'keyword', 'comment', 'string', 'number', 'operator',
  'ruddyTag', 'ruddyAttribute', 'ruddyPunctuation',
];
export const tokenModifiers = ['declaration', 'defaultLibrary'];

export interface HighlightToken {
  line: number;
  start: number;
  length: number;
  type: string;
  modifiers: number;
}

export interface TextEdit {
  rangeOffset: number;
  rangeLength: number;
  text: string;
}

const captureTypes: Record<string, string> = {
  variable: 'variable', type: 'type', module: 'namespace', namespace: 'namespace',
  function: 'function', property: 'property', keyword: 'keyword', comment: 'comment',
  string: 'string', number: 'number', boolean: 'keyword', operator: 'operator',
  constructor: 'ruddyTag', attribute: 'ruddyAttribute', punctuation: 'ruddyPunctuation',
  'variable.parameter': 'parameter', 'keyword.operator': 'operator',
};

export class HighlightEngine {
  private constructor(private language: Language, private query: Query) {}

  static async load(directory: string): Promise<HighlightEngine> {
    await Parser.init({ locateFile: () => path.join(directory, 'web-tree-sitter.wasm') });
    const language = await Language.load(path.join(directory, 'tree-sitter-ruddy.wasm'));
    const query = new Query(language, await readFile(path.join(directory, 'highlights.scm'), 'utf8'));
    return new HighlightEngine(language, query);
  }

  open(text: string): HighlightDocument {
    const parser = new Parser();
    parser.setLanguage(this.language);
    return new HighlightDocument(parser, this.query, text);
  }

  dispose(): void { this.query.delete(); }
}

/** Owns a document's parser and syntax tree; all offsets are UTF-16 code units. */
export class HighlightDocument {
  private tree: Tree;
  private text: string;

  constructor(private parser: Parser, private query: Query, text: string) {
    this.text = text;
    const tree = parser.parse(text);
    if (!tree) { parser.delete(); throw new Error('Tree-sitter could not parse the document'); }
    this.tree = tree;
  }

  update(edits: readonly TextEdit[]): void {
    if (!edits.length) return;
    // VS Code supplies all ranges against the pre-change document. Applying
    // them from right to left keeps those offsets valid for multicursor edits.
    for (const edit of [...edits].sort((a, b) => b.rangeOffset - a.rangeOffset)) {
      const startIndex = edit.rangeOffset;
      const oldEndIndex = startIndex + edit.rangeLength;
      const startPosition = pointAt(this.text, startIndex);
      const insertedLines = edit.text.split('\n');
      this.tree.edit(new Edit({
        startIndex, oldEndIndex, newEndIndex: startIndex + edit.text.length,
        startPosition, oldEndPosition: pointAt(this.text, oldEndIndex),
        newEndPosition: {
          row: startPosition.row + insertedLines.length - 1,
          column: insertedLines.length === 1 ? startPosition.column + edit.text.length : insertedLines.at(-1)!.length,
        },
      }));
      this.text = this.text.slice(0, startIndex) + edit.text + this.text.slice(oldEndIndex);
    }
    const previous = this.tree;
    // web-tree-sitter takes UTF-16 indices AND columns, matching VS Code.
    // Passing the edited tree is what enables Tree-sitter subtree reuse.
    const next = this.parser.parse(this.text, previous);
    if (!next) throw new Error('Tree-sitter could not update the document');
    this.tree = next;
    previous.delete();
  }

  tokens(): HighlightToken[] {
    const lines = this.text.split('\n');
    const byLine = new Map<number, Array<{ start: number; end: number; type: string; modifiers: number; priority: number }>>();
    for (const capture of this.query.captures(this.tree.rootNode)) {
      const parts = capture.name.split('.');
      const type = captureTypes[capture.name] ?? captureTypes[parts[0]!];
      if (!type) continue;
      const modifiers = (parts.includes('definition') ? 1 : 0) | (parts.includes('builtin') ? 2 : 0);
      const { startPosition, endPosition } = capture.node;
      for (let line = startPosition.row; line <= endPosition.row; line++) {
        // Exclude CR as well as LF: semantic tokens cannot span line endings.
        const lineLength = (lines[line] ?? '').replace(/\r$/, '').length;
        const start = line === startPosition.row ? startPosition.column : 0;
        const end = Math.min(line === endPosition.row ? endPosition.column : lineLength, lineLength);
        if (end <= start) continue;
        const spans = byLine.get(line) ?? [];
        spans.push({ start, end, type, modifiers, priority: capture.patternIndex });
        byLine.set(line, spans);
      }
    }

    const tokens: HighlightToken[] = [];
    for (const [line, spans] of [...byLine].sort(([a], [b]) => a - b)) {
      // A sweep resolves nested/overlapping captures using the queries' rule:
      // later patterns win. Emitted semantic tokens never overlap.
      const events = spans.flatMap(span => [{ at: span.start, start: true, span }, { at: span.end, start: false, span }])
        .sort((a, b) => a.at - b.at);
      const active = new Set<(typeof spans)[number]>();
      let previous = 0;
      for (const event of events) {
        if (event.at > previous && active.size) {
          let winner: (typeof spans)[number] | undefined;
          for (const span of active) if (!winner || span.priority >= winner.priority) winner = span;
          const last = tokens.at(-1);
          if (last && last.line === line && last.start + last.length === previous && last.type === winner!.type && last.modifiers === winner!.modifiers) {
            last.length += event.at - previous;
          } else {
            tokens.push({ line, start: previous, length: event.at - previous, type: winner!.type, modifiers: winner!.modifiers });
          }
        }
        if (event.start) active.add(event.span); else active.delete(event.span);
        previous = event.at;
      }
    }
    return tokens;
  }

  dispose(): void { this.tree.delete(); this.parser.delete(); }
}

function pointAt(text: string, index: number): { row: number; column: number } {
  const prefix = text.slice(0, index);
  return { row: prefix.split('\n').length - 1, column: index - prefix.lastIndexOf('\n') - 1 };
}
