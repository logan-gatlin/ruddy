# Native Ruddy tests

Agreed design from the grilling interview:

- A bare `@test` applies only to a single named `let`, implies `@private`, and requires an exactly monomorphic unit-to-unit function, pure or carrying only the structural `effect Assert = String -> ()`. Ordinary compilation checks placement, value and signature.
- `std::test` defines Assert, assert and fail; Assert is aliased through the prelude. Assertions notify failure only. Handlers may resume since Assert returns unit.
- Allow Assert on main and public library host calls; default platform handlers throw with the message. Initializer evaluation remains pure.
- `ruddy test` compiles and runs all root-bundle tests, including private submodules, excluding dependencies. Library and executable roots work without main. Reuse JavaScript runtime selection (`node` or `[run].js`). No file filtering or interpreter runner.
- Deterministic qualified-name ordering, sequential execution and no per-test isolation. First assertion escaping a test aborts that test; runtime exceptions also fail it. Continue remaining tests. Assertions handled inside the test do not fail it. Report individual results and totals; failures exit unsuccessfully, zero tests succeeds, compilation errors prevent execution.
- Normal builds keep tests private and use normal reachability rules.
- Std mocks handle filesystem, process, path and HTTP using supplied responses; unexpected operations perform Assert. Support recording calls for verification.
- Move eligible pure and locally handled std tests to a private std::tests module; retain host integration and configuration coverage.

Implementation validation: public compiler compilation, CLI command behavior, std assertion and mocking helpers. Rust tests run exclusively via `just test`.

Review baseline: 1b225c34db4372df952551f15149a5e86fce2b8b.

Mock API implementation: optional pure response callbacks, default None for unexpected operations, and returned `{value, calls}` with tagged request traces. These keep recording local to the handler.

## Validation

- Focused compiler, CLI, editor, debugger and std-helper tests passed. Observed failing regressions before the compiler/privacy, alias and debugger fixes.
- `just fmt-check` and `cargo clippy --workspace --all-targets` passed.
- `RUST_TEST_THREADS=4 just cov` passed the complete workspace suite through `just test`: 2,024 passed, 10 ignored.
- Compiler coverage: 96.08% lines, 88.70% branches. The repository-wide 100% target is not met; the new validator has 100% function coverage, 95.56% line coverage, and 87.50% branch coverage, with defensive row-tail paths unexercised.
- Parallel standards/spec review: no remaining findings. The standards review identified missing debugger acceptance diagnostics; fixed and verified with a regression test.
