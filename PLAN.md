# Scheme provenance structural alignment plan

- [x] Replace flat semantic preorder provenance with an explicit arena whose child edges mirror zonked type/row/presence structure.
- [x] Replay package bodies, absent payload pruning, rows, and presences against their exact semantic children and attach roots to opened nodes/variables.
- [x] Preserve quantified Type/Row/Presence sorts when instantiating schemes.
- [x] Pass each binding's exact annotation evidence into authoritative provenance instead of searching unrelated nested contextual checks.
- [x] Bound roots per semantic position and globally, retaining earliest/latest endpoints and accounting for omitted parents/reasons.
- [x] Add package/absent sibling structure, competing annotation, row-sort, long-chain, and >16k reason-budget regressions.
- [x] Run formatting, full workspace tests/checks, and Clippy; commit without disturbing unrelated changes.
