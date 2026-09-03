# 12: Add accepted compilation states

**What to build:** Add a core compilation interface that takes a parsed bundle through checking to either inspectable partial state or an accepted program and validated artifact.

**Blocked by:** 11/Plan and lower callback externs

**Status:** ready-for-agent

- [ ] Core compilation returns `PartialCompilation` with completed phase results and aggregated structured errors when checking fails.
- [ ] Successful IR building, inference, and pattern checking establish `AcceptedProgram`.
- [ ] Only `AcceptedProgram` can use the new LIR lowering path.
- [ ] Successful compilation produces a validated target-neutral artifact.
- [ ] Complete trace collection is controlled by one opt-in setting.
- [ ] Filesystem discovery and backend generation remain outside the compilation interface.
- [ ] Legacy orchestration remains temporarily available for migration.
- [ ] The complete `just test` suite passes.

