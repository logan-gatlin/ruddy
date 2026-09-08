# Implementation

The handler body uses a row `D + ..r`, with the handled constructors present.
The surrounding ambient is equated with `D (when p) + ..r`, using one fresh
presence per handled constructor and sharing the same argument tuples. Only
`r` lacks the handled constructors. Existing row equality therefore preserves
one application per constructor while allowing operation and return arms to
perform it in the surrounding ambient.

Fresh handler allowances and their compiler-created branch views can default
absent at generalization. Shared variables, callable inputs, and aliases of
written or instantiated presence variables remain protected. Only allowances reachable from the binding
and their formula-connected views are eligible, so a local binding cannot
default an unrelated sibling callback. For guarded relationships,
SAT projection checks that choosing absence preserves every admitted assignment
of the other presence variables. The proof uses exact projection; exhausting
its representation budget skips defaulting. This removes phantom effects from
guarded swallowing without removing actual conditional rethrows or narrowing
callers. A defaulted handler allowance disappears from canonical rows; it must not become a negative label that prohibits a caller's independent
operation. This applies both to local initializers and published signatures.
The solver snapshots both inferred and defaulted allowances when trying
structural congruence, so rollback also restores default eligibility and
canonicalization.

Queued row comparisons now re-canonicalize their rows immediately before
comparison. Callback input inference can expand a shared effect tail after the
comparison was queued; using the earlier snapshot could incorrectly erase a
callback effect from one branch of a match.

Operation arms already captured surrounding evidence. Lowering now restores
that evidence immediately after the handled computation, before lowering a
return arm. Rethrows resume through the existing continuation machinery.

Regression tests cover inference and diagnostics, source compilation and
artifact round-trips, the debugger pipeline, and Node execution of local and
dependency-imported handlers. The runtime fixtures check ordering, modified
payloads, forwarding to another operation of the same constructor, repeated
rethrows, returned callbacks, stored operations, open effect remainders, return
arms, and aborts.

## Validation

- `just test inference::`: 365 passed.
- `just test rethrowing`: 13 passed, including Node runtime and artifact imports.
- `just test ui::`: 101 passed. Diagnostic fixtures now exercise explicit
  row-tail duplication, with exact source endpoints, instead of refusing
  valid rethrows.
- `just clippy`: passed without warnings.
- `just fmt-check`: passed.
- `just cov --json --output-path /tmp/ruddy-effect-rethrowing-coverage.json`:
  passed through the required `just test` runner; 1,783 workspace tests passed,
  9 ignored, 0 failed.
- Compiler coverage: 42,701 / 44,344 lines (96.29%) and 4,797 / 5,370 branches
  (89.33%). **The 100% requirement in `CONTRIBUTING.md` remains unmet.**
  These totals do not establish a baseline comparison or a coverage regression.
- `git diff --check`: passed.

## Standards

Review against `7656326682c323283f9f5447c0ea5cf95e74b813` found no remaining
code-smell findings. Duplicate evidence restoration was removed. New tests use
public seams in `ruddy-tests`, and the debugger pipeline is covered.

One documented-standard gap remains: `CONTRIBUTING.md` requires 100% compiler
line and branch coverage, and the measured totals above do not meet it.

## Spec

No remaining confirmed spec findings. Review found and prompted regressions
for guarded phantom effects, safe projection on budget exhaustion, and sibling
callback effects. Each reported issue is fixed and its regression passes. The follow-up review
also confirmed that updated UI fixtures retain actual duplication diagnostics
and fallback wording coverage.

Review totals: Standards 1 remaining finding (coverage target); Spec 0 remaining findings.
