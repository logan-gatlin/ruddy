# 06: Add tolerant artifact recovery

**What to build:** Let malformed unchecked dependency data recover safely into a validated artifact while publishing every repair as structured diagnostic information.

**Blocked by:** 05/Add validated artifact states

**Status:** ready-for-agent

- [ ] Tolerant recovery produces the same trusted artifact type as strict validation.
- [ ] Foreign variables, invalid bounds, formulas, package ownership, and other currently recoverable semantic defects remain non-fatal.
- [ ] Every repair produces a structured `RecoveryFact` through the diagnostic seam.
- [ ] Recovery facts are available independently of complete solver tracing.
- [ ] Recovery preserves stack safety for deeply nested semantic data.
- [ ] The complete `just test` suite passes.

