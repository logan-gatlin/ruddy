# 07: Migrate trusted artifact consumers

**What to build:** Make linking, backend generation, dependency import, and compiler artifact production consume artifacts whose invariants have been established.

**Blocked by:** 05/Add validated artifact states, 06/Add tolerant artifact recovery

**Status:** ready-for-agent

- [ ] Linking accepts only validated artifacts.
- [ ] Backend generation accepts only validated artifacts.
- [ ] Textual dependency artifacts use strict validation before import.
- [ ] Manually constructed dependency data uses explicit tolerant recovery and exposes recovery facts.
- [ ] Compiler-produced artifacts are validated by construction through the semantic artifact seam.
- [ ] The complete `just test` suite passes.

