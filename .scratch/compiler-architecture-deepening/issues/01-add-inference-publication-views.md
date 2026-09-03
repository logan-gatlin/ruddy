# 01: Add inference publication views

**What to build:** Add read-only semantic and diagnostic inference views so callers can request complete tracing when needed without disrupting existing inference consumers.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Inference exposes a read-only semantic publication containing the facts required by downstream compiler phases.
- [ ] Inference exposes a read-only `DiagnosticView` containing structured errors and, when enabled, complete trace data.
- [ ] Rich causal error explanations remain available when complete tracing is disabled.
- [ ] Existing callers continue to work during the expansion.
- [ ] The complete `just test` suite passes.

