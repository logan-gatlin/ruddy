# 02: Migrate semantic inference consumers

**What to build:** Make pattern checking, LIR preparation, and artifact construction consume the focused semantic inference publication instead of learning the full inference result layout.

**Blocked by:** 01/Add inference publication views

**Status:** ready-for-agent

- [ ] Pattern checking reads its required aliases, presence facts, promises, and refinements through the semantic publication seam.
- [ ] LIR preparation reads only accepted semantic facts through the new seam.
- [ ] Artifact construction reads exported schemes and operation types through the new seam.
- [ ] No migrated caller depends on diagnostic trace storage or mutable publication fields.
- [ ] The complete `just test` suite passes.

