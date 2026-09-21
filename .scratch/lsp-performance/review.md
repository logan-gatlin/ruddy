# LSP performance and match diagnostics

## Implementation

1. Retain declaration resolution, stable symbols, lowered bodies, reference sets,
   and SCCs across body edits. Rebuild conservatively when declaration shape,
   imports, allocation ranges, or recovery changes. Pass changed symbols into
   inference so unchanged groups avoid body fingerprinting.
2. Infer the definition requested by a cursor and its dependencies. Completion
   additionally requests matching visible candidates. Load validated dependency
   interface sidecars before executable artifacts; dependencies publish complete
   interfaces even when the editor focuses on one of their definitions.
3. Retain project/query state across cancellation. Publish inferred semantics and
   their checks together. Interrupt background work for requests and wait for a
   300 ms typing pause before starting full-project checks. Retry interrupted
   diagnostic publication independently of completed analysis.
4. Cache acquired disk inputs, identities, and directory fingerprints. Native
   filesystem events invalidate affected inputs; reconciliation recovers missed
   events. Watch missing candidates and complete symlink chains. Reacquire files
   that enter the graph after being unwatched and finalize artifact keys after
   discovering their contents.
5. Cache UTF-16 line indexes, per-file diagnostic rendering, and tail-recursion
   suggestions. Render outside the scheduler mutex; check the current revision
   before publication. Preserve foreground/background phases for open documents.
6. Share immutable syntax, program, inference semantics, and diagnostics through
   `Arc`; retain file identities across module insertion/reordering.

The implementation retains one analysis worker. Edited files are fully parsed;
an incremental body update can still copy the retained program once. Full
semantic-output assembly and uncached dependency inference remain measurable
costs.

## Match-checking follow-up

The original checkout at `9c365571` had six failing semantic tests. The same six
failed in a separately built clean baseline. Rebasing onto `203dee19` incorporates
PR #81's conditional-variant coverage fix and updated scrutinee-type expectation.
That change carries conditional case assumptions into coverage checks.

Guarded branch constraints now retain their written body spans. Family
construction carries each contributor's source reason through its iterative
type/row walk, recording the selected contributor separately from semantic
types and fingerprints. Unification consumes that evidence at the corresponding
type node, so an unrelated arm does not become an endpoint of a nested-field
conflict. The solver's reachability and type rules are unchanged.

This attribution covers nodes synthesized by family construction. Existing
wrapper-copy and type-opening paths can still omit a contributing arm in deeper
diagnostic context; propagating provenance through those paths is a separate
follow-up. Shared input type pointers are deliberately not assigned guessed
origins.

The obsolete open-sum snapshot now expects a row-closure violation: matching
only finitely many named cases cannot preserve an arbitrary caller-chosen
remainder. Regressions distinguish this error from a valid catch-all and from a
closed sum with a conditional case. Branch regressions cover three arms,
unreachable arms, nested fields, and shared values in unrelated fields.

The LSP test helper now enforces its existing 60-second limit against elapsed
request time and reports the case, method, and request on failure. It attempts
shutdown after failed assertions or timeouts, waits a bounded time for the
worker, and preserves the original panic if cleanup also fails. Both earlier
std-related debug timeouts pass with the final implementation and the original
deadline. No test-profile change or longer deadline is retained.

## Measurements

The warm measurements below were repeated after rebasing and fixing diagnostics,
at code commit `bc988e2`, using the 10,000-line corpus in `scripts/bench-lsp.py`.
Baseline was the installed binary at `9c365571`; changed was a release build.
Runs were sequential, with separate Ruddy caches and no agent builds/tests
running. Other applications remained active, and the baseline's build flags
are unknown. The std cache was warmed before the measured run.

| Measurement | Baseline | Changed |
| --- | ---: | ---: |
| No std: edit-to-hover p95, 25 edits | 107.917 ms | 33.170 ms |
| No std: edit-to-completion p95 | 113.244 ms | 35.436 ms |
| Warm std: edit-to-hover p95, 20 edits | 140.540 ms | 51.682 ms |
| Warm std: edit-to-completion p95 | 146.524 ms | 59.910 ms |
| No std: peak RSS | 700.41 MiB | 661.67 MiB |
| Warm std: peak RSS | 859.12 MiB | 779.23 MiB |

Earlier measurements, before the semantic follow-up/rebase, found empty-std-cache
first diagnostics at 6,711 ms baseline versus 6,840 ms changed, and peak RSS at
2,445 MiB versus 2,058 MiB. Cold startup did not improve.

Cold runs used fresh artifact/interface caches seeded with existing git source
checkouts; OS caches were not flushed. Increasing
the background quiet interval from 150 to 300 ms defers complete diagnostics;
foreground requests and diagnostics do not wait for it.

In the earlier run, a 40-edit burst spaced 25 ms apart cancelled 78 obsolete requests and returned
the newest hover/completion in 76/79 ms after the last edit. A 12-edit burst
spaced 300 ms apart returned all 24 responses without cancellation. These are
small synthetic samples, not statistical guarantees.

Reproduce the primary measurement with:

```sh
python3 scripts/bench-lsp.py target/release/ruddy 25 no-std
python3 scripts/bench-lsp.py target/release/ruddy 20 std
```

Detailed local traces, binary hashes, baseline assertions, and validation logs
remain alongside this review, excluded from version control.

## Validation

All six reported failures now pass. The final diagnostic/inference check passed
106 focused tests. Final complete release validation passed 2,181 tests with zero
failures and 10 intentionally ignored tests, including 2,108 passes in the main
test crate. The final default-profile LSP run passed all 31 tests with the
original 60-second deadlines. Formatting and clippy passed, with three
pre-existing enum-size warnings.

Commands used:

```sh
RUDDY_HOME=/tmp/ruddy-lsp-test-home RUST_TEST_THREADS=1 just test --release -j2
RUDDY_HOME=/tmp/ruddy-lsp-debug-test-home RUST_TEST_THREADS=1 just test -j1 --lib -- --test-threads=1 lsp:: --nocapture
RUDDY_HOME=/tmp/ruddy-lsp-coverage-home RUST_TEST_THREADS=1 CARGO_PROFILE_TEST_OPT_LEVEL=1 CARGO_BUILD_JOBS=2 just cov
just fmt-check
just clippy
```

The complete `just cov` run also passed: 2,182 tests, zero failures, and 10
intentionally ignored tests. It includes one additional test enabled by debug
assertions. Compiler coverage is **96.24% of lines and 88.34% of branches**.
The repository's existing 100% coverage requirement remains unmet; this report
does not treat that target as satisfied.
