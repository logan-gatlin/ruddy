# 01 Parse effect parameters and applied labels

Status: open
Type: task

Parser and surface printer support for `effect Name 'a 'b = …`, applied labels `!Ask Nat` in
rows (written, absent, conditional), alias bodies with arguments and an optional `..'e` tail.

Seams: `tests/src/parse.rs` round trips, `tests/src/print.rs` AST/IR printers, tree-sitter corpus.
