# 01 Reserve `hide` and parse hidden types and patterns

Status: resolved
Type: task

Spec section: "Source syntax: `hide`" and the first acceptance bullet.

`hide` becomes an unconditionally reserved keyword in the Rust lexer and the
Tree-sitter grammar. Two forms parse:

- `hide 'a => <type>`: a hidden type. The body extends to the right; parentheses
  delimit one used as a type argument. Allowed wherever a full type is written,
  including an arrow's result and a nested `hide`.
- `hide 'a <pattern>`: a hidden pattern. The payload pattern is taken greedily,
  as a tag pattern's is.

Surface support: parser AST, formatter, the debugger's AST printer and token/AST
tabs, `ui` precedence tables, Tree-sitter grammar/highlights/locals/corpus, docs
(`grammar.md`, `dictionary.md`). Quoted labels (`{ "hide": v }`, `record."hide"`),
sigilled tokens (`'hide`, `#hide`, `!hide`, `@hide`) and longer/case-distinct
identifiers (`hidden`, `Hide`) keep their lexical rules; printers quote a
structural field labelled `hide`.

The IR lowers both forms to a diagnosed error until ticket 02 gives them a
semantics, so a source using them still compiles to a coherent partial result.

Seams: `tests/src/token.rs`, `tests/src/parse.rs`, `tests/src/format.rs`,
`tests/src/print.rs`, `tests/src/ui.rs`, Tree-sitter corpus.
