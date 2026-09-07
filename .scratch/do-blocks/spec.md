# Do Blocks

Status: ready-for-agent

## Problem Statement

Ruddy's only way to bind a name inside an expression is `let <pattern> = <value> in <body>`.
Every binding nests one level deeper than the one before it, so a function body
with three local bindings is three nested expressions, and the last one carries
the whole result. Sequencing effects uses the same shape: `let _ = !Log 1 in`
followed by the next step. Reading such a body means tracking where each `in`
hands off to the next expression, and writing one on a single line, as several
tests do, produces `let n = f () in let m = g () in n + m`.

The language already delimits `match`, `if`, `handle`, and inline `module`
bodies with `end`. There is no equivalent delimited block for a run of local
bindings.

## Solution

Replace `let ... in ...` with a `do ... end` block. A block is an expression
holding zero or more `let` statements, written exactly as file-level `let`
statements are, optionally followed by one `return <expr>` as the last item:

```
let f = fn a => do
  let local1 = 1
  let local2 = 2
  return local1 + local2
end
```

Each `let` binds its name for the rest of the block. Statements evaluate in
written order, so effects happen top to bottom. A block ending in `return e`
evaluates to `e`; a block with no `return` evaluates to `()`. `return` is legal
only as the final item of a block and must carry an expression.

The `let ... in` form is removed. `in` stops being a keyword and becomes an
ordinary identifier. `do` and `return` become reserved keywords; the
handler-arm `return` that was previously read by spelling now reads the
keyword.

## User Stories

1. As a Ruddy programmer, I want to write several local bindings one after another inside a `do ... end` block, so that a function body with local names reads top to bottom instead of nesting.
2. As a Ruddy programmer, I want each `let` in a block to be written exactly as a file-level `let` is, with patterns and type ascriptions, so that there is one binding syntax to learn.
3. As a Ruddy programmer, I want a later `let` to see every earlier binding in the block, so that bindings can build on one another.
4. As a Ruddy programmer, I want a bare-name `let` in a block to see itself in its own value, as `let ... in` did, so that a local recursive function still works.
5. As a Ruddy programmer, I want a block's statements evaluated in written order, so that `let _ = !Log 1` followed by `let _ = !Log 2` logs in that order.
6. As a Ruddy programmer, I want a block ending in `return e` to evaluate to `e`, so that a block can produce a value.
7. As a Ruddy programmer, I want a block without `return` to evaluate to `()`, so that a block that exists only for its effects needs no trailing ceremony.
8. As a Ruddy programmer, I want `do end` to be a valid unit expression, so that the rule for a block without `return` has no exception at zero statements.
9. As a Ruddy programmer, I want `do return e end` to be valid and equal to `e`, so that the rule for `return` has no exception at zero statements.
10. As a Ruddy programmer, I want a later `let` to shadow an earlier binding of the same name, so that a block behaves like the nested scopes it replaces rather than like a file.
11. As a Ruddy programmer, I want a `return` followed by another statement reported in plain English as a `return` that is not the last thing in its block, so that I know to move it.
12. As a Ruddy programmer, I want a `return` outside any block, such as `fn a => return a`, reported in plain English as a `return` that needs a `do` block around it, so that the fix is named.
13. As a Ruddy programmer, I want a bare `return` with no expression refused, so that the only way to write a unit block is to omit `return`.
14. As a Ruddy programmer, I want a `type`, `effect`, `module`, or `extern` inside a block refused with a message saying only `let` may appear in a block, so that I am not left guessing whether local declarations exist.
15. As a Ruddy programmer, I want a `do` block to head an application or projection and to need parentheses as an application argument, exactly as `if` and `match` do, so that its placement rules are the ones I already know.
16. As a Ruddy programmer, I want to use `in` as an ordinary name, so that removing the old form frees the word rather than reserving it for nothing.
17. As a Ruddy programmer, I want `do` and `return` refused as names everywhere, so that a block reads the same at every position.
18. As a Ruddy programmer, I want the standard library and demo program to use `do` blocks, so that the shipped code shows the current syntax.
19. As a tooling user, I want editor parsing and highlighting to recognize `do`, `return`, and the block's `end`, so that source tooling agrees with the compiler.
20. As a compiler maintainer, I want the block lowered onto the existing `Let` term, so that inference, patterns, artifacts, and the JavaScript backend need no new construct.
21. As a compiler maintainer, I want the surface and normalized printers to emit `do` blocks that reparse to the same tree, so that the fixed-point printing tests keep holding without `in`.

## Implementation Decisions

- The lexer reserves `do` and `return` and stops reserving `in`. The tree-sitter grammar's reserved list changes the same way, and the highlights query lists `do` and `return` with the other keywords.
- The handler-arm head reads the `return` keyword token instead of matching the identifier by spelling. Handler syntax and semantics are otherwise unchanged.
- The surface tree replaces `ExprKind::Let` with a `Do` expression holding the block's statements and an optional `return` expression. Statements reuse the file-level statement type so a block's `let` carries the same pattern, ascription, and `where` clause a file-level `let` does.
- The parser reads a block as `do <stmt>* [return <expr>] end`. It reads statements with the existing statement reader up to `return` or `end`. A statement whose kind is not `let` is reported at its keyword as not allowed in a block, and reading continues so one message covers one mistake.
- `return` is read only at statement position inside a block. Its expression is a full expression that extends as far right as it can and ends at `end`, the way a `fn` body does. A `return` whose next token is `end` is reported as missing its expression.
- A statement after `return` is reported at that statement's first token as following a `return`, which must be last. Recovery skips to the block's `end`.
- A `return` reached where an expression atom is expected, outside any block, is reported as needing a surrounding `do` block.
- `do` is an atom in the same sense `if` and `match` are: reachable from atom position, absent from the application-argument set, and projectable off its `end`.
- Lowering desugars a block into the existing nested `Let` terms, innermost first from the last statement: a bare-name `let` binds its symbol before lowering its value and so sees itself, and a pattern `let` lowers the value first and then binds the temporary and one projection per name. This is the existing `let ... in` lowering applied once per statement. The innermost body is the `return` expression or, when there is none, a unit literal spanning the block.
- Each desugared `Let` term keeps the span of its own written statement, so diagnostics point at the statement that caused them.
- A block's scope is released at its `end`; no binding leaks past it.
- The surface printer prints a block as `do`, its statements, the optional `return`, and `end`. The normalized printer prints a chain of `Let` terms as one block: consecutive `Let` bodies fold into one statement list, and a final unit-literal body prints as a block without `return`. Grouping follows the existing rules for `if` and `match`.
- Every `let ... in` in `std/`, `demo.rud`, tests, tree-sitter corpus files, and code comments is rewritten as a block. The doc comment on `ExprKind::Operation` that says there is no `do` form is corrected.
- No diagnostic is added for the old `let ... in` spelling. Source that still writes `in` is read as an application to a name called `in` and reported however that fails.

## Testing Decisions

- Prefer external behavior over internal representation: accepted source, inferred public schemes, diagnostics, printed source, and executed JavaScript.
- One checked-in end-to-end bundle compiled through the CLI and executed with Node covers a block with several bindings and a `return`, a block with no `return` used for its unit value, `do end`, `do return e end`, a self-recursive bare-name `let`, a pattern `let`, shadowing, and effect order across statements observed through an existing effect handler.
- Public compilation tests cover inference of a block's type with and without `return`, an ascribed `let` inside a block, and an effectful block whose effect row is the union of its statements' effects.
- Rejected compilation tests cover a statement after `return`, `return` outside a block, bare `return`, a `type`/`effect`/`module`/`extern` inside a block, and `do`/`return` used as names.
- Structured diagnostic tests pin the codes, primary spans, titles, and help of the new messages. Prose stays in plain English per the project's diagnostic style.
- Public parser tests cover every accepted block shape, a block heading an application and a projection, a block as a parenthesized argument, a nested block, and `in` used as an identifier.
- Printer fixed-point tests replace every `let ... in` case with a block and add a normalized-tree case that folds nested `Let` terms into one block, with and without a trailing `return`.
- Tree-sitter corpus tests mirror every accepted and rejected form, and highlighting tests cover `do` and `return`. Run through `just grammar`.
- All Rust tests run through `just test`.

## Out of Scope

- Bare expression statements or `;` separators inside a block. Discarding a value is written `let _ = <expr>`.
- `type`, `effect`, `module`, or `extern` declarations inside a block.
- `return` anywhere other than the final item of a block, including inside an `if` or `match` arm within a block.
- Whole-block, order-independent scoping of the kind file-level definitions have.
- A migration diagnostic or compatibility path for `let ... in`.
- Warnings for unused bindings.
- Any change to `handle` arms beyond reading `return` as a keyword.

## Further Notes

- The block is a spelling of the nested `Let` terms the language already has. Choosing a parser-only construct keeps inference, effect rows, and both backends untouched; the whole change lives in the lexer, parser, lowering, printers, and tooling grammar.
- Self-reference of a bare-name `let` is carried over unchanged from `let ... in` so local recursive functions keep working. It also means `let x = x + 1` refers to the new `x`, as it did before; that quirk is inherited, not introduced.
- The repository has no domain glossary or ADR that bears on this design.
