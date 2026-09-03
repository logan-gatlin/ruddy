# 05: Add validated artifact states

**What to build:** Introduce explicit unchecked and validated artifact states, with strict semantic validation establishing the trusted artifact consumed by the compiler.

**Blocked by:** 04/Contract the legacy inference result

**Status:** ready-for-agent

- [ ] Portable data that has not established compiler invariants is represented as unchecked.
- [ ] Strict validation converts valid unchecked data into a trusted artifact.
- [ ] Strict textual parsing rejects malformed persistent data with structured parse failures.
- [ ] Semantic validation covers bound positions, package ownership, formulas, row tails, absent payloads, and deep stack-safe structures.
- [ ] Existing artifact consumers continue to work during the expansion.
- [ ] The complete `just test` suite passes.

