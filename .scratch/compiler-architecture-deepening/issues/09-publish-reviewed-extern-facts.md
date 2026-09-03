# 09: Publish reviewed extern facts

**What to build:** Publish target-neutral proof of each accepted extern declaration from inference so later phases can rely on one semantic review.

**Blocked by:** 08/Contract the legacy artifact interface

**Status:** ready-for-agent

- [ ] Inference publishes reviewed ABI shape, representation admissibility, and callback capability facts for accepted externs.
- [ ] Representation-polymorphic, ABI-incompatible, and uncovered callback cases remain structured inference errors.
- [ ] Reviewed facts are part of semantic publication rather than diagnostic replay data.
- [ ] Existing LIR extern derivation remains available during the expansion.
- [ ] Tests cover acceptance and rejection through the inference interface.
- [ ] The complete `just test` suite passes.

