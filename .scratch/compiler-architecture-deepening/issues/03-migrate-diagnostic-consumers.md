# 03: Migrate diagnostic consumers

**What to build:** Move user-facing diagnostics and debugger views onto `DiagnosticView` while preserving rich ordinary errors and opt-in solver replay.

**Blocked by:** 01/Add inference publication views

**Status:** ready-for-agent

- [ ] Error rendering consumes supported structured diagnostic data without mutating inference state.
- [ ] Debugger type, constraint, solve, and presence views read only the diagnostic information each view owns.
- [ ] Complete tracing retains constraints, solver steps, reasons, variables, and refinements.
- [ ] Disabling tracing retains equivalent user-facing causal explanations without retaining unrelated replay data.
- [ ] Relevant debugger snapshots and the complete `just test` suite pass.

