# Shared struct and sum rows

Implement the design agreed in the conversation:

- Struct fields and sum cases share one row kind. Effect rows remain separate.
- Structs and sums remain distinct value types; ordinary type unification does not equate them.
- At a row argument, either a struct or a sum contributes its underlying row, including through named and generic aliases.
- A row parameter or annotation variable may be shared across struct and sum tails, with inference and existing generalization rules preserved.
- Preserve label spelling, payload types, presence relationships, and open tails exactly.
- Combine duplicate-label exclusions from every use of the shared row.
- Reject parameters used both as whole types and rows; reject non-row arguments.
- Empty structs and empty sums contribute the same empty row, while their enclosing types retain their distinct meanings.
- Exported parameter metadata and importing compiled artifacts preserve the shared kind.

Examples:

```ruddy
type Sum 'r = | ..'r
type Struct 'r = { ..'r }
type T = Sum { a: (), b: () }
type T2 = Struct (#A () | #B ())
type Both 'r = { product: { ..'r }, choice: | ..'r }
```

`T` expands to `#a () | #b ()`, and `T2` to `{ A: (), B: () }`.

Approved test seams: source-level compiler tests (acceptance, inference, and diagnostics) and compiled-artifact round trips.
Run all Rust tests through `just test`. Review the implementation and commit it to the current branch.
