# Structural hashing review

Fixed point: `a3f9fb35cdca9905a39d2ff518a6ee6163dc4b98` (the starting commit).
Reviewed the staged implementation against [the agreed spec](spec.md).

## Standards

No documented-standard violations or actionable baseline smells found.
Tests reside in `tests/`, as `CONTRIBUTING.md` requires. The change adds no
grammar or compiler phase; the debugger uses the shared JavaScript backend,
which includes the new primitive. Hash traversal follows the existing
reflection/comparison conventions.

## Spec

No spec findings. The implementation covers pure inferred/explicit mirror APIs,
fixed and explicit seeds, immutable FNV-1a accumulation, framed traversal,
alphabetical record fields, active sum cases, recursive data, signed-zero
normalization, encountered-only failures, and root-to-leaf error paths.
The equality/hash invariant matches comparison for supported values at a shared
static type. Interpreter and JavaScript XOR implementations agree. Tests cover
fixed FNV frames and hash-output parity across backends and target widths.
Documentation records the contract and accumulator/effect tradeoffs.

Standards: 0 findings. Spec: 0 findings. No outstanding review issues.

## Validation

- Standard library typecheck and documentation generation passed.
- `just fmt-check` and `just clippy` passed.
- All four public hashing tests passed on interpreter and JavaScript, including
  default and 32-bit target domains for the numeric cases.
- The first full run identified the primitive-table test's old count of 240;
  XOR adds the 241st entry. The corrected assertion passed its focused run,
  and both reviewers accepted the follow-up.
- Final `just test` passed, including 1,949 tests in `ruddy-tests` with 10
  existing ignored tests, the other workspace tests, and doctests.
