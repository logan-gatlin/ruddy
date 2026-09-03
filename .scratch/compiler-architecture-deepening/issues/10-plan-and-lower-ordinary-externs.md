# 10: Plan and lower ordinary externs

**What to build:** Carry non-callback extern declarations from reviewed inference facts through an in-memory `ExternPlan` into equivalent LIR.

**Blocked by:** 09/Publish reviewed extern facts

**Status:** ready-for-agent

- [ ] A post-inference extern module creates target-neutral plans from reviewed facts without repeating admissibility checks.
- [ ] Ordinary extern values and calls lower through `ExternPlan`.
- [ ] Raw target strings remain opaque to target-neutral planning.
- [ ] Produced LIR and artifacts retain their established executable meaning.
- [ ] JavaScript target validation remains unchanged in the JavaScript adapter.
- [ ] The complete `just test` suite passes.

