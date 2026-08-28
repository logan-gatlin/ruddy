# Audited root-defect plan

- [x] Route deep equal and unequal composite type goals through one explicit continuation stack, preserving nominal congruence rollback and row diagnostics.
- [x] Retain one solver-table checkpoint for the outermost nominal transaction so rollback memory is O(depth + variables), including refinement/store/trace restoration.
- [x] Make recursive public artifact types stack-safe to clone, compare, and drop independently of `Artifact`.
- [x] Add 30,000-depth, 256 KiB stack regressions for unequal named/row composites, large solver tables, and standalone artifact ownership.
- [x] Verify formatting, workspace checks/tests, Clippy with warnings denied, and 100% line/branch coverage with `just cov`.
