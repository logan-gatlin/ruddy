# Structured recursive-cycle explanation plan

- [x] Replace flattening occurs detection with a graph-safe iterative raw walk that retains only the exact binding-reason route back to the assigned variable.
- [x] Bound shared `Rc` DAGs and malformed unrelated binding cycles with visited node/continuation state while preserving deterministic sibling order.
- [x] Attach `Recursive` failures to the ordinary structured causal explanation path, including direct and shared-row cycle-closing constraints.
- [x] Derive call-input, containment, and neutral recursive repair advice from the causal source shape, with no solver terminology.
- [x] Expose the 2–4 fact abridged view and complete causal path through debugger snapshots.
- [x] Add self-application, nested function/row, row-tail/presence/shared-tail exact paths, sibling exclusion, post-failure recovery, exponential-DAG, cyclic-binding, and 30,000-depth regressions.
- [x] Run formatting, focused tests, full workspace tests/checks, and Clippy.
- [x] Commit without disturbing unrelated workspace changes.

# Phase 3: cross-definition inference explanations

- [x] Keep semantic `Scheme` span/prose-free and pair it internally with a compact causal skeleton.
- [x] Close provenance at generalization and reopen it with the same scheme instantiation, including closed contracts and nested lets.
- [x] Carry original call/projection/body facts through later definitions; treat imported contracts as authoritative local-use fallbacks.
- [x] Keep causal walks iterative and cap scheme roots/full reason slices without changing inference semantics.
- [x] Make the 2–4 fact selection source-ordered and independent of unrelated definition identities; never introduce more than one shared causal pivot.
- [x] Cover accessors, repeated uses, long definition chains, imported fallback, and unrelated-definition determinism.
- [x] Run formatting, focused regressions, full workspace tests/checks, and Clippy.
- [x] Commit without disturbing unrelated workspace changes.
