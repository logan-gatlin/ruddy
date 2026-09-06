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
The repository's required coverage measurement could not complete within the
machine's memory budget, as detailed below; 100% coverage remains unverified.

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
  workspace's memory. No coverage percentage is claimed: the 100% requirement
  in `CONTRIBUTING.md` remains unverified.
