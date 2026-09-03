# 14: Migrate debugger compilation

**What to build:** Make the debugger use core compilation with complete tracing while preserving every supported intermediate phase view.

**Blocked by:** 12/Add accepted compilation states

**Status:** ready-for-agent

- [ ] Debugger compilation opts into the complete diagnostic trace.
- [ ] Tokens, syntax, IR, types, constraints, solver steps, presence facts, LIR, artifact, link, and backend views retain their appropriate partial or complete status.
- [ ] Failed compilation presents completed intermediate results through `PartialCompilation`.
- [ ] Debugger views consume read-only semantic and diagnostic interfaces.
- [ ] Snapshot payloads do not duplicate unrelated trace data between views.
- [ ] Relevant debugger snapshots and the complete `just test` suite pass.

