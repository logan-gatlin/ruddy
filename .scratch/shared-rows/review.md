# Shared row review

Reviewed against `.scratch/shared-rows/spec.md` and repository standards, using
`74869e0b9c0f7e9edeb2cd6cc65b2c565d7cdaa5` as the baseline.

## Standards

No remaining code-review findings. The repository-wide 100% coverage requirement
is not met: the measured result is 96.36% line coverage and 88.66% branch coverage. Stale
comments and redundant shape matching were corrected. The alias-template
capture walk is iterative and reuses ordinary private type declarations for
kind and exclusion checking. The full coverage report is in `coverage.txt`.

## Spec

Review identified three gaps: nested effect arguments distinguished equivalent
row constructors in structural identities; effect alias templates could not
extract named row arguments; eager template substitution discarded duplicate
labels. All three were corrected and have source-level regression tests.
Imported generic effect aliases also have an artifact round-trip regression.

Re-review found no additional concrete correctness issue.

## Validation

- `cargo check --workspace`: passed.
- `just clippy`: passed.
- `just fmt-check`: passed; final Rust test expectation edits also pass rustfmt.
- `just cov`: completed successfully, running the full suite through `just test`:
  1,847 passed, 10 ignored, no failures.
- Focused regressions cover shared inference/generalization, exclusions,
  presence conditions, structural effect identities, effect alias templates,
  imported generic aliases, and runtime descriptors.
- The initial full run exposed six debugger tests expecting the old separate
  kinds. Their expectations were updated; the second full run passed.
