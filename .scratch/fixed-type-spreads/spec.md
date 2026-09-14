# Fixed type spreads

Extend existing generic row spreading with fixed spreads in struct and sum
types, including type declarations and annotations.

```ruddy
type V2 = { x: Real, y: Real }
type Circle = { ..V2, radius: Real }
```

## Agreed behavior

- Fixed operands are named aliases or parenthesized applications of named
  aliases: `..V2` and `..(Pair Real)`. Applications require parentheses.
- Resolve alias chains and forward references, including transitive spreads.
- A fixed operand must have a closed, known outer row. Its payload types can
  contain generic parameters or open rows. An unresolved outer tail is not a
  fixed spread.
- Allow any number of fixed spreads among explicit entries, in any order.
- Allow the existing single generic spread alongside fixed spreads, last.
  Its collision constraints include all fields contributed by fixed spreads.
- Reject all duplicate labels, including duplicates with identical payload
  types and collisions between two fixed operands.
- Expand fields/cases structurally, exactly as though written explicitly;
  introduce no additional compatibility or inheritance relationship.
- Preserve the shared struct/sum row semantics of existing generic composition,
  including cross-shape row sources. Effect rows remain separate.
- Support fixed spreads wherever struct/sum type syntax is accepted, retaining
  the existing restrictions of each context.
- Fixed struct spreads use ordinary entry delimiters and allow trailing commas.
  Generic tails retain their existing delimiter restrictions.
- Sums use the existing separators and leading bar when starting with a spread:
  `type S = | ..Base | #Extra`.
- Empty operands contribute no entries. Field order has no semantic effect.
- Reject cycles through spread expansion while retaining ordinary recursion
  through payload types.

## Implementation and validation

Update compiler parsing, row expansion and diagnostics, formatter, debugger,
tree-sitter grammar and generated parser, and language documentation.

Confirmed test boundaries: existing
source-program compiler tests, formatter round trips, and tree-sitter corpus.
Run Rust tests exclusively through `just test`; run the full suite once at the
end. Review both repository standards and this spec, then commit on the current
branch. The starting commit is `749292651f5d5ba79cdd7cb4e62d7ec6abb31188`.

## Validation results

- Full instrumented Rust suite: 2,052 passed, 10 ignored, run through `just test`
  with `RUST_TEST_THREADS=2`. Default concurrency hit the 4 GiB memory cap;
  the serial retry hit the runner's 30-minute limit.
- All 18 fixed-spread compiler, formatter, and debugger tests pass, including
  two additional coverage-driven cases added after the full run.
- Tree-sitter: 138 corpus cases pass; spread highlighting has 9 passing assertions.
- Formatting passes. Clippy completes with warnings, including three enum-size
  warnings from the additional type syntax and existing CLI warnings.
- Compiler coverage: 96.29% lines, 88.75% branches. The repository's required
  100% coverage is not met; gaps include both changed and unchanged modules.
- Standards review: no remaining code findings; coverage requirement remains
  unmet. Spec review: all reported findings resolved.
