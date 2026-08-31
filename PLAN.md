# Semantic inference pivot repair plan

- [x] Give generated function-input and branch-result constraints source-semantic identities, and derive field/effect/binder/fallback pivot keys for every displayed fact.
- [x] Select only keys shared by at least two deduplicated source facts, in input/branch/field/effects/binder/fallback priority order.
- [x] Sort constraints, facts, endpoints, and post-overlay facts by source visibility, file, span, semantic role, and stable content rather than solver IDs.
- [x] Count abridgement omissions from deduplicated source candidates.
- [x] Choose deterministic collision-free explanatory labels from visible symbols without consulting solver identities.
- [x] Cover false pivots, priority selection, overlay order, omission accounting, label collisions, projection/effect serialization, and stable source ordering regressions.
- [x] Run formatting, the full workspace suite, checks, and Clippy; commit without disturbing unrelated changes.
