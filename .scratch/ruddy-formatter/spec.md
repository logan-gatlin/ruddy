# `ruddy fmt` — a source formatter for Ruddy

Status: accepted (2026-09-07)

## Summary

`ruddy fmt` rewrites `.rud` files into one canonical layout. It is an AST
pretty-printer with a Wadler-style document layout engine, living in the
compiler crate as `src/format.rs`, driven from the CLI and from the language
server. Zero configuration: 2-space indent, width 100, LF line endings, one
trailing newline, never hard tabs.

## Architecture

- **Layout engine.** `src/format.rs` builds a document (`text`, `line`,
  `softline`, `hardline`, `group`, `nest`, `if-break`, verbatim blocks, line
  suffixes for trailing comments) from the parse tree and prints it to the
  width. Precedence and parenthesization follow the rules already in
  `src/ui.rs` (`Prec`, the `write_*` writers); the parse-tree precedence
  tables move into `src/ui.rs` so the debugger's printer and the formatter
  read one table.
- **Not a compiler phase.** It never feeds a later stage, so no debugger tab.
  It is subject to the compiler crate's full branch coverage requirement.
- **Comments become lexer tokens.** `Kind::LineComment(String)` and
  `Kind::BlockComment(String)` carry the inner text only (after `--`, or
  between `(*` and `*)`), the way `String` tokens are stored decoded. The
  lexer's numeric-field rule looks at the last *non-comment* token. The
  debugger's tokens tab, the LSP and `bundle::Syntax` all see comment tokens.
- **Partition at the parser door.** `parse::parse` splits comment tokens off
  into `parse::Output::comments` before the parser proper sees the stream, so
  parser internals stay untouched.
- **Literals.** Strings, quoted tags and quoted field labels are copied
  verbatim from the source by span, so the author's escapes survive. Numbers
  are normalized: reals print as the shortest decimal that round-trips to the
  same `f64` (`1.0` → `1`, `1.50` → `1.5`, `007` → `7`); naturals, integers
  and fixed-width literals print their value followed by their suffix. No
  exponent notation exists in the grammar so none is produced. Positional
  field indices print as digits.

## Error recovery contract

- The parser records every region it drops during recovery (`recover()`,
  the block-drop paths, a refused block declaration) as spans in
  `parse::Output::skipped`.
- The formatter walks every statement list (file level, `do` blocks, module
  bodies) in span order, interleaving statements and skipped regions. A
  statement is **opaque** if any lex error, parse error, or skipped region
  starts within its region (from the end of the previous item to its own
  end) and is not inside one of its nested statement lists. Opaque statements
  and skipped regions are copied verbatim from the source, re-indented by
  their first line only. Everything around them formats normally.
- With errors present the file is still written, diagnostics are printed,
  and the exit code is non-zero. Stdin mode writes formatted text to stdout
  and diagnostics to stderr under the same rule.

## CLI

- `ruddy fmt`, alias `f`.
- No arguments: walk up from the current directory to the bundle's
  `Ruddy.toml`, format every `.rud` under the bundle's source directory (the
  parent of the manifest's `root`), skipping any subdirectory that contains
  its own `Ruddy.toml`. Never follows dependency edges; never touches the
  installed standard library or any dependency.
- Explicit files and directories are formatted as given, whatever bundle
  they belong to.
- `--check`: write nothing, list files that would change, exit non-zero when
  anything would change.
- `--stdin`: format one file from standard input to standard output.
- No gitignore handling.

## Style

- Type ascription is `let x: T` (no space before the colon).
- `end` aligns with the indentation of the line that opened its construct.
- Match and handle arms carry a leading `|` at the indentation of the
  `match` line, not indented further. Arm bodies that break go on the next
  line, indented one level under the `|`.
- Sum type variants get a leading `|` per line when broken.
- Trailing commas are added when a collection is broken across lines and
  removed when it fits on one line. The mandatory 1-tuple comma always stays.
- One attribute per line above the definition.
- Author blank lines between statements are preserved, collapsed to at most
  one; none at the start or end of a block.
- Every construct respects the author's break choice until width forces a
  break. The signal is a source newline in the gaps between the opener and
  the first element, or between elements, for: struct, array and tuple
  literals and types; application arguments; match and handle arms; `do`
  statements; `let` after `=`; sum type cases; pipeline and operator chains;
  module bodies. For `if` the signal is any newline between `then` and the
  chain's shared `end`. If the signal is present the construct prints broken
  even when it would fit; otherwise it prints on one line if it fits and
  broken otherwise. Formatted output preserves the signal it was printed
  with, so formatting is a fixed point.
- Broken `if` chains use the corpus style: the branch stays on the `then`
  line when it fits, `else if` and `else` sit at the `if` column, `end`
  closes at the `if` column.
- Long `let`: break after `=` first, then at `->` in the type signature
  (continuation lines at the type's indentation), then inside the body by its
  own rules. A `let`, `fn` or `return` whose body is a `match`, `handle` or
  `do` hugs: the block's first line stays on the `=`/`=>` line and its arms
  or statements follow at that line's indentation.
- Broken applications put each argument on its own line, indented one level
  under the function. Broken pipelines put each `|> step` on its own line at
  the indentation of the first operand. Broken operator chains break before
  the operator with the continuation indented one level.
- Raw `\\` strings are re-indented line by line to the current indentation,
  their content untouched, and nothing is ever inserted between consecutive
  `\\` lines. Nothing follows a raw string on its line.

## Comments

- A comment on its own line attaches as a *leading* comment to the next node
  that starts after it; at the end of a block it becomes a *dangling*
  comment of the enclosing block, printed before the closer.
- A comment on the same line as preceding code attaches as a *trailing*
  comment of the last node that ends before it on that line.
- A `--` comment breaks every group containing it. An inline `(* *)` block
  comment does not.
- Comment text is reflowed. Consecutive `--` lines at the same indentation
  form one paragraph, refilled to the width, breaking only at whitespace; a
  word longer than the remaining width goes on its own line unbroken. A
  paragraph ends at a blank comment line, at a non-comment line, at a line
  indented deeper than the paragraph's first line, or at a line starting
  with a list marker (`-`, `*`, `1.`); those lines are kept verbatim with
  only their `--` prefix and indentation normalized. Exactly one space after
  `--`. Comment indentation equals the indentation of the node the comment
  attaches to. Single-line `(* *)` comments are left alone; multi-line ones
  get their interior re-indented one level under the `(*` column with the
  same paragraph rules applied.
- A trailing comment that overflows the width wraps onto continuation `--`
  lines aligned under the original `--`.
- Blank lines between a leading comment and its node are preserved,
  collapsed to one.

## Integration

- `ruddy-ls` gains whole-document `textDocument/formatting` only.
- `just fmt` also runs `ruddy fmt` over the repository's Ruddy sources and
  `just fmt-check` runs `--check`, so `just check` enforces it.
  `treesitter/test/` and the diagnostics fixtures are excluded: their exact
  text is under test.
- The corpus reformat lands as the final commit and doubles as the
  idempotence test. Tests live in `tests/src/format.rs` and
  `tests/src/cli.rs`, run via `just test`.
