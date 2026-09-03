# 15: Migrate integration tests

**What to build:** Make integration tests exercise production compilation orchestration while retaining direct phase tests and isolating deliberate invariant violations in crate-private support.

**Blocked by:** 12/Add accepted compilation states

**Status:** ready-for-agent

- [ ] End-to-end compiler, artifact, link, and backend tests use the core compilation interface where phase internals are not under test.
- [ ] Phase-specific tests continue to call their public phase interface directly.
- [ ] Defensive LIR tests use crate-private mutation support rather than public mutable accepted state.
- [ ] Redundant lex-parse-build-infer-check-lower helpers and equivalent shallow tests are removed.
- [ ] Tests assert observable behavior through supported interfaces rather than private field layout.
- [ ] The complete `just test` suite passes.

