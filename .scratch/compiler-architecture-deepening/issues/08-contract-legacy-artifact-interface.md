# 08: Contract the legacy artifact interface

**What to build:** Finalize `UncheckedArtifact` and trusted `Artifact` as distinct states and remove construction paths that let unvalidated data masquerade as trusted input.

**Blocked by:** 07/Migrate trusted artifact consumers

**Status:** ready-for-agent

- [ ] `UncheckedArtifact` is the explicit portable construction and decoding state.
- [ ] `Artifact` guarantees the invariants required by linking and backends.
- [ ] Import, export, strict validation, and tolerant recovery share one deep semantic translation implementation.
- [ ] Persistence-specific validation and recovery knowledge no longer leaks into in-memory semantic types.
- [ ] Ambiguous legacy construction and compatibility forwarding are removed.
- [ ] The complete `just test` suite passes.

