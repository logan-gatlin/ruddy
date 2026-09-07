# Region mutability implementation

Implemented against [the specification](spec.md). User documentation is in
[mutable cells](../../docs/mutability.md).

The change covers syntax, region kinds, function-body isolation, effect-based
let generalization, artifacts and imports, CPS operations, JavaScript execution,
diagnostics, debugger views, source printers, and tree-sitter support.

## Standards

Review compared the worktree with `27e2364fa39126b3e9f7f7d75f5f71d4c4edb418`.

- **Debugger completeness:** ordinary user effects initially instantiated Region
  parameters as Type variables. Fixed by minting Region variables explicitly,
  with an inference regression observing the published debugger variable sort.
- **Misleading documentation:** new helpers had inherited neighboring methods'
  comments, and Assignment had the Pipeline comment. Restored each comment to
  its method and documented the new operations.
- **Optional duplication cleanup:** the JavaScript acceptance tests repeat the
  existing temporary-module/Node harness pattern. Left consistent with the
  surrounding tests; this is a minor maintainability suggestion, not a contract
  failure.

Two actionable review findings resolved; one optional harness cleanup remains.
The coverage build initially hit a dependency/compiler memory regression,
diagnosed and fixed below. Coverage measurement now completes, but the
repository's 100% coverage requirement remains unmet.

## Spec

- **Builtin identity protection:** a forged imported effect could claim the
  mutation identity under another name and expose a handler that hid external
  reads. Validation now checks both the reserved name and any declaration
  claiming that identity. A regression verifies strict rejection and that
  dependency recovery cannot expose the forged handler. This enforces the
  requirement that metadata cannot redefine or discharge builtin mutation.
- **Imported region forwarding:** kind inference initially seeded imported types
  but omitted imported effect parameters. Both kind fixpoints now include them,
  and region parameters remain semantically relevant. Regression coverage
  includes effect aliases, type aliases, and an annotated dependent function.
  Direct imported-effect signatures also round-trip without a local alias:
  artifact validation keeps an unavailable dependency argument kind unknown
  instead of interpreting its argument tuple as a value record.
  This enforces kind and region relevance across aliases and imports.

Both findings resolved. The reviewer reran both reproductions against the fixed
library and confirmed that the exploit is rejected and the valid alias consumer
compiles.

Unannotated recursive definitions can conservatively retain mutation effects;
this is documented. Thread creation and FFI invariant enforcement remain outside
this feature's agreed scope.

## Validation

- 17 focused mutation tests pass through `just test mutation_`.
- `cargo check --workspace`, `just clippy`, and `just fmt-check` pass.
- `just grammar` passes all 120 corpus cases and highlighting tests.
- The full `just test` run passed 1,668 tests, with 9 ignored. The subsequent
  direct-import artifact fix passes its focused regression.
- `just cov` was attempted twice. The first instrumented compiler build nearly
  exhausted available memory. A retry with `CARGO_PROFILE_DEV_DEBUG=0`,
  `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_BUILD_JOBS=2` still grew past 23 GiB
  for rustc on a 32 GiB machine. Both were stopped before exhausting the
  workspace's memory. The dependency/compiler cause and fix are recorded below.
  With that fix, `just cov` passes all 1,668 tests (9 ignored) and reports
  96.09% line coverage and 89.39% branch coverage. The 100% requirement in
  `CONTRIBUTING.md` remains unmet.

## Coverage build memory diagnosis

The runaway allocation occurs while Rust nightly's new trait solver compiles
`varisat 0.2.2` against `partial_ref 0.3.3`. It reproduces without coverage
instrumentation, without compiling Ruddy, and with metadata-only output (no
machine-code generation). The compiler was `1.100.0-nightly (f248f4038 2026-09-05)`.

This matches [Rust issue #161575](https://github.com/rust-lang/rust/issues/161575).
Unconstrained lifetimes in `partial_ref` cause exponential growth in the new
solver. The [upstream fix](https://github.com/jix/partial_ref/pull/6) removes those
lifetimes and was released in `partial_ref 0.3.4`.

Controlled metadata-only compilations of Varisat, with the same nightly and
compiler arguments apart from the indicated change:

| Dependency / solver | Result | Peak sampled rustc RSS | Elapsed |
| --- | --- | --- | --- |
| `partial_ref 0.3.3`, default solver | Stopped at memory budget | 1,096 MiB and growing | 3.38 s |
| `partial_ref 0.3.3`, `-Znext-solver=coherence` | Passed | 233 MiB | 1.81 s |
| `partial_ref 0.3.4`, default solver | Passed | 188 MiB | 0.85 s |

The fix changes only `partial_ref`'s version and checksum in `Cargo.lock`, using
`cargo update -p partial_ref --precise 0.3.4`. No compiler flags or Ruddy source
changes are necessary. A subsequent nightly library build with the default
solver passed: Varisat used 253 MiB in 1.4 seconds and Ruddy used 915 MiB in
9.4 seconds (debug information disabled, one build job).

The regression is in compilation of an upstream dependency, so a Ruddy runtime
unit test would not exercise it. The regression check is the actual nightly
build, followed by `just cov`. Diagnostic replay scripts and timing logs are
kept outside the repository in `/tmp/ruddy-coverage-diagnosis`.

The original coverage command now completes with the dependency update and no
solver override:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 just cov
```

For this verification, an outer systemd scope also capped compilation at 12 GiB
with swap disabled; tests still used the repository's `just test` runner.
Compilation finished in 5m 24s and all 1,668 tests passed (9 ignored). The report
measured 96.09% line coverage and 89.39% branch coverage. The last observed build
scope peak was 2.43 GiB; the completed scope no longer exposed its final peak.
