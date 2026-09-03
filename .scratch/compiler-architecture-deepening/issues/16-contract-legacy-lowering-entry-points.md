# 16: Contract legacy lowering entry points

**What to build:** Remove legacy paths that let unrelated mutable compiler values cross the LIR and artifact seams, leaving accepted compilation as the only public route.

**Blocked by:** 13/Migrate CLI compilation, 14/Migrate debugger compilation, 15/Migrate integration tests

**Status:** ready-for-agent

- [ ] Public LIR lowering accepts only `AcceptedProgram`.
- [ ] Public artifact construction receives coherent accepted state and lowered LIR through the supported compilation path.
- [ ] Legacy lowering and artifact-building entry points that accept unrelated mutable values are removed.
- [ ] Individual lexing, parsing, IR, inference, and pattern interfaces remain public.
- [ ] No compatibility forwarding recreates the removed shallow orchestration interface.
- [ ] Language behavior and persistent artifact meaning remain stable.
- [ ] The complete `just test` suite passes.

