# 04: Contract the legacy inference result

**What to build:** Remove the broad mutable inference result after all production and diagnostic consumers use the focused publication seams.

**Blocked by:** 02/Migrate semantic inference consumers, 03/Migrate diagnostic consumers

**Status:** ready-for-agent

- [ ] The legacy collection of unrelated public inference fields is removed.
- [ ] Semantic and diagnostic state is reachable only through their supported read-only interfaces.
- [ ] Synthetic identifiers and deliberate state mutation are available only through crate-private test support.
- [ ] No compatibility forwarding preserves the removed shallow interface.
- [ ] The complete `just test` suite passes.

