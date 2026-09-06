# Bundle-private definitions

## Agreed contract

- A top-level `let`, `extern`, `type`, `effect`, or `module` marked `@private` is accessible throughout its declaring bundle but its name is not exported to dependent bundles.
- A private module hides all descendants, for both inline and file-backed modules. Unmarked declarations remain public unless inside a private module. A pattern binding applies privacy to every bound name.
- The marker accepts only unit, including the equivalent `@private ()` and empty struct spelling. Other payloads produce a diagnostic.
- Public values, types, and effects may alias private definitions. Explicit and inferred public signatures may reference private types and effects, including effect operation signatures. Structural type/effect information must remain usable across artifact round trips and dependencies without exposing private names to source lookup.
- Private implementation code remains available to execute public definitions and initialization; it is omitted from JavaScript exports.
- An executable's root `main` must be public. A library's `main` remains an ordinary definition.

## Validation and review

Confirmed test boundaries: public compiler/artifact APIs for producer/consumer bundles, and CLI/JavaScript behavior for exports and executable entry validation. Rust tests run only through `just test`.

Review baseline: `6415db4c43ae083feb10ab76828697faf6c6d00b`.

## Implementation decisions

- Preserve private type/effect declarations as semantic support in artifact headers, with an explicit export flag distinct from source metadata. Dependency admission loads their semantics but excludes their names from source resolution. This supports recursive aliases and structural effects without forcing lossy expansion or changing the written metadata of descendants.
- Omit private values and modules from header exports; retain their implementation in LIR. Existing JavaScript export construction and executable entry lookup then enforce the same boundary.
- Compute visibility from the declaration and all enclosing modules at artifact construction. Source lookup inside the declaring bundle is unchanged.

## Test runner correction

The user clarified during validation that the 4 GiB limit is intended for test execution and its child processes, not compilation. `just test` now lets Cargo compile normally and uses an executable runner to put each unit-test, integration-test, and doctest process tree into its own memory-limited systemd scope. The existing zero-swap setting, timeout, exit-status propagation, and fallback when no systemd user bus exists are preserved.

## Verification results

- `cargo check --workspace --all-targets`, `just fmt-check`, and `just clippy` passed.
- The full workspace suite passed through `just test`: 1,619 passed and 9 existing tests ignored. Coverage used stable Rust 1.98.1 with `RUSTC_BOOTSTRAP=1`, the environment from `cargo llvm-cov show-env --sh --branch`, one build job, and two test threads.
- All 15 compiler-boundary tests and all 39 artifact tests passed. After the runner correction, all 7 tests matching `private_` passed through the new runner, and the workspace doctest command passed.
- A temporary independent Rust package, invoked through `just test --manifest-path`, verified that its build script was outside the 4 GiB cap while its unit test, doctest, and child processes had `memory.max = 4294967296` and `memory.swap.max = 0`. A separate runner check confirmed exit status 17 propagated unchanged.
- Coverage with the repository's filename exclusions measured 96.35% lines and 90.02% branches overall, below the documented 100% target. Every changed instrumented compiler line was covered (53/53), and all seven changed branch conditions were observed both true and false. The uncovered lines are outside the changed lines.
- Standards and Spec reviews found no issues in the privacy implementation or the test-runner correction.
