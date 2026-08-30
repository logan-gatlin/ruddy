# Structured recursive-cycle explanation plan

- [x] Replace flattening occurs detection with a graph-safe iterative raw walk that retains only the exact binding-reason route back to the assigned variable.
- [x] Bound shared `Rc` DAGs and malformed unrelated binding cycles with visited node/continuation state while preserving deterministic sibling order.
- [x] Attach `Recursive` failures to the ordinary structured causal explanation path, including direct and shared-row cycle-closing constraints.
- [x] Derive call-input, containment, and neutral recursive repair advice from the causal source shape, with no solver terminology.
- [x] Expose the 2–4 fact abridged view and complete causal path through debugger snapshots.
- [x] Add self-application, nested function/row, row-tail/presence/shared-tail exact paths, sibling exclusion, post-failure recovery, exponential-DAG, cyclic-binding, and 30,000-depth regressions.
- [x] Run formatting, focused tests, full workspace tests/checks, and Clippy.
- [x] Commit without disturbing unrelated workspace changes.
