# Structured recursive-cycle explanation plan

- [x] Replace flattening occurs detection with an iterative raw walk that retains only the exact binding-reason route back to the assigned variable.
- [x] Attach `Recursive` failures to the ordinary structured causal explanation path, including direct and shared-row cycle-closing constraints.
- [x] Render only source concepts about values, inputs, and fields containing or accepting themselves, with two-direction repairs and no solver terminology.
- [x] Expose the 2–4 fact abridged view and complete causal path through debugger snapshots.
- [x] Add self-application, nested function/row, shared-tail, cross-use, exact-path, and 30,000-depth regressions.
- [x] Run formatting, focused tests, full workspace tests/checks, and Clippy.
- [x] Commit without disturbing unrelated workspace changes.
