# CPS LIR implementation and verification

Implemented on `feature/cps-lir`, starting from `9f4b8ea`.

- `50fff86`: portable CPS LIR, managed JS execution, foreign completion protocols,
  callbacks, ordered initialization, debugger support, documentation, and tests.
- `25d4c9f`: review fixes for indirect-call proofs and ordinary completion data.

The public LIR and serialized artifacts contain flat parameterized blocks,
explicit continuations and captures, handler transfers, initializer functions,
and conservative callable summaries. Linking relocates references without a
required control-flow transformation. See [the execution and FFI contracts](../../docs/cps.md).

## Verification

- `just test`: 1,645 passed, 0 failed, 9 ignored, including workspace doctests.
- `just fmt-check`: passed.
- `just clippy`: passed without warnings.
- Instrumented suite: 1,645 passed, 0 failed, 9 ignored.
- Compiler coverage: 36,038 / 37,481 lines (**96.15%**) and 3,803 / 4,264
  branches (**89.19%**). **The repository's 100% coverage requirement is unmet.**
  Misses span existing inference/parser code and new validation paths; these
  figures do not establish a baseline comparison or a coverage regression.
  The final report excludes stale binaries and profiles from an older checkout.

The runtime tests exercise deep direct, mutual, higher-order and non-tail
recursion, recursive handlers, a two-million-call tail recursion case under a
48 MB JS heap limit, independently compiled bundles, suspended handlers,
retained and overlapping callbacks, returned callbacks, stale and unrelated
handler exits, fixed adapters, duplicate and late settlement, notification
reporting, suspendable initialization and Process output draining. Node stress
and failure subprocesses have deadlines.

## Standards review

Three maintainability findings were addressed: shared continuation result
layout, shared branch construction, and explicit protocol ABI encodings.
Coverage against the repository's 100% line-and-branch requirement is recorded
above; it must not be inferred from the passing test suite.

## Spec review

Two implementation defects were reproduced and fixed:

- Completion notifications and initializer readiness no longer assimilate
  ordinary thenable data. Only a declared Promise-facing adapter delivers its
  result through a host Promise.
- Global reads preserve the producer's callable proof. Artifact validation
  rejects synchronous unknown indirect calls and invalid proof propagation
  through block or continuation environments.

The reviewers rechecked these changes. The linker's existing contract still
requires semantically valid compiler-produced inputs; it does not authenticate
arbitrary imported proof claims against a replacement dependency graph.

## Coverage tooling

`just cov` now executes tests through `just test`, preserving the memory-limited
runner. It cleans the dedicated coverage build directory, including artifacts left by
other compiler layouts, and
supports `RUDDY_COVERAGE_TOOLCHAIN` (default `nightly`).

The installed Rust 1.100 nightly compiler grew beyond practical memory while
compiling `varisat`, including with dependency optimization disabled. Verification
therefore used the working Rust 1.98.1 toolchain with its unstable branch
coverage support enabled:

```sh
RUDDY_COVERAGE_TOOLCHAIN=stable RUSTC_BOOTSTRAP=1 just cov
```
