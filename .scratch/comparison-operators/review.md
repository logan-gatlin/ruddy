# Comparison operator review

Baseline: `79e6ffa0b504f3336f7710551de3f9cb0c28b061`.
Reviewed against [the agreed spec](spec.md), the user's clarification requiring mirrors for opened existential types, and repository instructions.

## Standards

Static review found no documented code-structure breaches and one low-priority maintainability suggestion: `src/inference/defaults.rs` repeats the immutable child traversal as a mutable traversal. Consider sharing traversal infrastructure when adding future term variants; no current omission was found.

The repository's 100% compiler coverage target remains unmet: **96.07% lines and 88.76% branches**. Earlier reviews also recorded a repository-wide shortfall, including [shared rows](../shared-rows/review.md). The new defaulting module measures 95.35% lines and 80.77% branches; passing tests do not establish full coverage compliance.

## Spec

No remaining findings. Review prompted additional dual-backend tests for locally bound open sums and empty arrays, which pass. The user clarified that opened existential types require mirrors, including when nested inside function types. Tests cover rejection without evidence and successful comparison with a carried mirror.

Full-suite verification found a host-boundary regression: exact reflection descriptors were being accepted where a native conversion contract was required. Validation now checks native contracts for polymorphic external slots, preserving rejection of effectful callbacks. Updated the diagnostic golden to its more precise pure-callable-contract error and extended reflection fixtures for Cell descriptors.

## Validation

- Full instrumented workspace suite: **2,002 passed, 10 ignored, 0 failed** (the main suite accounts for 1,929 passing tests).
- Coverage used the `just cov` environment. After correcting the diagnostic fixture, resumed its test/report steps without cleaning compiled dependencies; the complete test phase ran through `just test`.
- `cargo check --workspace`, `just clippy`, and `just fmt-check` passed.
- All 137 tree-sitter corpus tests and highlight checks passed.
- Comparison behavior is exercised on both the JavaScript backend and reference interpreter: numeric domains, Unicode ordering, arrays, alphabetical records/sums, recursion, NaNs, signed zeros, generic calls, cells, functions, operand evaluation order, and existential evidence.

Detailed coverage and final test output are available locally under `target/comparison-coverage.json` and `target/comparison-coverage-final.log`.
