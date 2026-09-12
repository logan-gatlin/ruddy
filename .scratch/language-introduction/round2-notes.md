# Round 2: rows and presence

The chapter now develops row polymorphism from familiar subtyping interfaces, then derives presence constraints from exact and open struct patterns. It connects input/output fields, field-to-case constraints, shared struct/sum rows, and a presence spanning options, sum cases, and effects. Selected answers, the dictionary, and reference cross-links have been expanded with it.

## Primary-source basis

- [TypeScript's structural compatibility rules](https://www.typescriptlang.org/docs/handbook/type-compatibility.html) establish the comparison with a smaller structural interface. [Generic type variables](https://www.typescriptlang.org/docs/handbook/2/generics.html) support the distinction between basic compatibility and an interface preserving an input/output relationship. The chapter does not claim that subtyping languages lack generics.
- [`the_motivating_programs_infer_their_constraints` and `a_presence_match_refines_annotated_and_inferred_results`](../../tests/src/inference.rs) verify XOR, equality, OR, and swapped input/output relationships. `a_struct_spread_never_drops_a_field_to_fit_an_expected_type` verifies closed-row rejection.
- [`a_required_case_combination_uses_sum_vocabulary`](../../tests/src/inference.rs) demonstrates that case presence concerns a type's admitted alternatives, rather than one value's runtime tag.
- [Shared rows spec](../shared-rows/spec.md) specifies the shared struct/sum row kind and separate effect-row kind.

## Validation

- All extracted book Ruddy snippets compile and execute with the existing `target/debug/ruddy`, Node, and working-tree std dependency in a disposable project. All 60 runtime assertions passed, including both contact alternatives, paired/open patterns, swapped payload types, inferred/annotated sum results, shared-row fields/cases, and logging.
- Generated API output for the disposable rows project confirms the inferred XOR, equality, OR, swapped-field signatures, and the inferred field-to-case implications. No repository standard-library documentation was regenerated.
- Eight invalid programs were rejected with their expected semantic diagnostics: closed-row extra fields, neither/both inputs to XOR, a single field for equality, no fields for OR, an over-permissive annotation, XOR over a sum type admitting both cases, and missing logging metadata for a logging callback.
- Documentation build: 57 pages. All local links/anchors resolve and all pages are reachable. Generated std Markdown hashes equal the snapshot taken before Round 2 edits.
- No Rust source changes, compiler rebuild, or Rust test invocation was needed for the documentation work. The runtime probes demonstrate examples, not a proof of the type system.
