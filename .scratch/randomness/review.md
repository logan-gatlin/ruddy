# Randomness implementation review

Baseline: `0cc67ca084be5ed1df6c196219795689f1d279fe`.
Implementation: `78e29fd`, plus the documentation correction reviewed below.
Two independent reviewers examined the standards and spec axes.

## Standards

No confirmed new documented-standard violations in the reviewed diff.
Deferred mutation regions appear in the shared constraint display used by the
debugger. No grammar or compiler phase was added. New tests use `tests/`;
existing inline compiler tests predate this change.

One optional **Duplicated Code** smell: direct and row-carried mutation
isolation in `src/inference/solve.rs` repeat escape checks and rigidification.
They intentionally inspect different residual effect rows. Kept these paths
explicit rather than expanding this feature with an additional solver refactor.
Compiler coverage is recorded with the validation results below.

## Spec

The reviewer found one P3 documentation gap: missing ambient and custom-handler
examples. Added both to `docs/src/platform-apis.md`; the reviewer confirmed the
finding resolved. The examples compile.

No remaining spec findings. Reviewed primitive independence, eager outer seed
consumption, private shared state, exact numeric conversions, rejection sampling,
effect forwarding, and stable seed assignment across controlled async schedules.

Standards: 0 hard findings, 1 optional duplication smell. Spec: 0 remaining
findings; the original P3 documentation gap is resolved.

## Validation

- Independent SplitMix64 C reference vectors agree for seeds zero, one, and
  maximum Nat64, including wraparound.
- Focused `just test` runs passed for all 18 mutation tests, the generic local
  handler compiler regression, reference sequences, sampling, local handlers,
  Node/web adapters, opposite async completion schedules, and Node entry points.
- Sampling and reference tests cover interpreter integer domains 32/53/64 and
  supported JavaScript domains 32/53.
- `just fmt-check`, `just clippy`, and `git diff --check` passed.
- Documentation examples compiled with `ruddy check`; documentation tests passed
  (16 tests), and the site build passed after adding the missing examples.
- `RUST_TEST_THREADS=2 just cov --json --output-path
  /tmp/ruddy-random-coverage.json` passed. It invokes the required memory-limited
  `just test` recipe. The main test executable reported 1,969 passed and 10
  ignored; the other workspace executables added 73 passing tests (2,042 total).
  The standard-library runner also reported all 13 tests passing.
- Compiler coverage: 50,372/52,399 lines (96.13%) and 6,009/6,766 branches
  (88.81%). The repository's overall 100% requirement is **not met**.
  Comparing LCOV entries against added lines in `git diff --unified=0
  0cc67ca HEAD -- src/` found all 84 instrumented changed lines covered and all
  14 branches on those lines covered. Uncovered entries lie outside the changed
  lines; no baseline coverage run was performed, so no aggregate coverage delta
  is claimed. This feature does not expand into unrelated compiler coverage.
- Raw full-run log: `/tmp/ruddy-random-full.log`; JSON and LCOV reports:
  `/tmp/ruddy-random-coverage.json`, `/tmp/ruddy-random-coverage.lcov`.
