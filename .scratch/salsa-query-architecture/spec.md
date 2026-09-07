# Salsa query architecture and responsive LSP

Status: Implemented; final sustained measurements and implementation review in progress.

## Objective

Transition the compiler to a Salsa query architecture that supports a responsive
LSP. Deliver a working editor experience, not only an incremental compiler API.
CLI compilation, the browser debugger, and the LSP ultimately consume one shared
semantic query layer.

## Change authority

- Compiler internals, Rust-facing APIs, ownership, and representations may change
  freely. Compatibility wrappers and preservation of existing internal seams are
  not requirements.
- The CLI must retain its existing user-facing behavior. Its implementation may
  change completely.
- Adapt the debugger to the final compiler API. Any debugger changes necessary
  for that adaptation are permitted; its detailed solver traces remain available
  on demand.
- Complete the CLI and debugger migrations before declaring the work finished.
  Remove superseded semantic paths after validation rather than maintaining two
  implementations permanently.
- Preserve language behavior and artifact meaning initially. Preserve the
  invariant that lowering consumes accepted compilation state. These invariants
  do not require retaining today's API shapes or artifact byte layout.
- Routine implementation choices follow this specification. Bring proposed
  language changes and evidence that performance targets cannot be met back to
  the user for a decision.

## Performance contract

Use a fixed, representative 10,000-line project on the user's development
machine. Record the machine, build profile, corpus, dependency state, and timing
method so results can be reproduced. Include realistic dependencies, structural
types, effects, and recursive groups rather than only independent trivial
definitions.

| Scenario | Target |
| --- | --- |
| Warm hover and completion after an ordinary body edit | p95 below 100 ms, including scheduling |
| Updated active-file frontend diagnostics after an ordinary body edit | p95 below 200 ms, including scheduling |
| Initial active-file frontend diagnostics with dependencies locally available | Below two seconds |
| Total LSP process memory for the reference workspace | Below 1 GiB during sustained editing |

Frontend diagnostics include syntax, resolution, type/effect, and pattern checks.
Whole-project and lowering-dependent checks run in the background and eventually
agree with CLI results for the same sources and build configuration. Hover and
completion must not wait for these checks.

Widely propagating interface edits have a separate measurement category. They
must preserve cancellation and service responsiveness, but the first milestone
does not promise the ordinary-edit diagnostics deadline for them. Measure their
cost explicitly. These targets are acceptance criteria, not measured current
performance or promised speedups from Salsa.

## First LSP deliverable

- Diagnostics, hover, completion, and go-to-definition.
- Completion for names in scope, qualified module members, fields when the
  receiver's type is available, and context-appropriate type/effect names.
- Incomplete prefixes, missing expressions, and unsaved buffers are normal
  inputs. Unknown types reduce available suggestions without disabling unrelated
  completion.
- Defer automatic imports, sophisticated expected-type ranking, references, and
  rename from this first slice.

Analyze the current buffer with recovery. Preserve useful semantic results for
unaffected definitions and whatever current information survives inside a broken
definition. Represent unresolved information explicitly. Do not silently present
the last valid version's types or locations as current results.

## Query boundaries

- Parse per file; separate declaration information from individual body queries.
- Infer mutually dependent groups in explicit compiler-owned queries, with a
  local solver inside each query. The compiler remains responsible for recursion
  and generalization.
- Publish each definition's semantic interface separately from its body,
  diagnostics, explanatory provenance, source locations, and optional replay data.
- A body edit that preserves the semantic interface must not force consumers to
  repeat inference. Changes in locations or explanatory provenance still update
  the affected presentation and diagnostic results.
- Track the declarations and interfaces actually consumed. File and bundle
  boundaries alone do not establish semantic independence.
- Handle edits that merge or split recursive groups correctly. Investigate
  whether complete annotations can safely create smaller inference boundaries
  without changing their meaning.
- Do not substitute Salsa's cycle iteration for recursive inference merely
  because both involve cycles. Any use of query fixpoints needs a demonstrated
  convergence argument appropriate to that computation.
- Keep backend incrementality out of the first milestone unless measurements
  show it is necessary. CLI builds additionally request lowering and artifacts;
  editor queries do not inherently request those outputs.

## Workspace and inputs

Initially analyze one active root/build configuration, including unsaved buffers
throughout its reachable local path dependencies. Support navigation into Git
dependencies and installed std. Defer simultaneous analysis under multiple root
configurations.

Track source contents, manifests, dependency selection, build configuration, and
file creation/deletion as inputs. Module discovery must account for missing files
and competing module-file candidates, not only changes to existing file text.
The root's target/platform governs conditional compilation throughout the
dependency graph; each bundle's std configuration also contributes to its inputs.

Drivers own external input acquisition and update the query inputs. Queries must
not obtain untracked changing filesystem or configuration state. Dependency
navigation needs source information alongside artifacts, since artifacts do not
carry source paths and spans.

Retain existing dependency artifact caching. Defer persistence of Salsa's query
cache. Existing compiler-stamp invalidation can continue to discard incompatible
artifact cache entries; cross-compiler cache compatibility is not a new promise.

## Scheduling and ownership

Prioritize the newest document version. Cooperatively cancel obsolete analysis,
keep protocol handling responsive during analysis, and prevent superseded work
from publishing diagnostics.

Investigate cancellation within expensive solver operations, not only between
queries. Resolve the current thread-local `Rc` ownership as needed for this
architecture. Moving analysis to a worker thread alone does not establish the
latency contract. Retention of snapshots, memoized results, and solver traces must
stay within the sustained-session memory target.

## Semantic investigation policy

The existing `infer_with_memo` API, fingerprints, and storage are implementation
optimizations, not semantic commitments. Their replacement can be more efficient
without changing the language. In particular, blanket declaration invalidation
and coupling consumer inference to explanatory provenance are not established
language requirements.

Investigate these actual semantic dependencies while preserving their behavior:

1. Unannotated and partially annotated definitions can publish interfaces
   determined by their bodies. Mutually recursive members can share inference
   variables. Complete annotations already provide contracts; requiring more
   annotations would remove existing inference convenience.
2. Structural effect identity depends on normalized operation interfaces and
   referenced types. A nominal alternative could reduce some identity
   dependencies but would lose automatic equivalence of independently declared
   compatible effects.
3. Transparent aliases and inferred parameter kinds/variance propagate facts
   between declarations. Explicit metadata or opacity would add annotation
   requirements or change structural substitutability.
4. Private declarations can contribute structural meaning to public interfaces.
   Privacy and module boundaries therefore do not by themselves isolate changes.

None of these observations establishes a need to restrict the language. No
performance gain from changing them has been measured. For any proposed semantic
change, explain the current rule, a concrete costly case, the alternative, its
effect on language philosophy and existing programs, and measured gains or the
experiment still needed. The user will decide the tradeoff.

## Validation and completion

- Replay edit sequences and compare incremental analysis against fresh analysis
  of the same revision: semantic types/effects, diagnostics, and navigation.
- Include broken-code recovery, undo, moving definitions, import changes,
  dependency edits, configuration changes, file creation/deletion, and merging or
  splitting recursive groups.
- Verify that unchanged semantic interfaces prevent consumer inference from
  rerunning even when source locations or explanations change. Verify refreshed
  diagnostic locations and explanations as well as cache reuse.
- Exercise newest-version scheduling and cancellation, including difficult
  inference work, and ensure obsolete diagnostics are never published.
- Measure the performance contract on the fixed reference workload and track
  memory across sustained editing rather than only a short warm benchmark.
- Validate existing CLI behavior, generated-program behavior, artifact meaning,
  and the adapted debugger, including its requested traces.
- Run Rust tests only through `just test`, as required by repository instructions.

Completion requires the working LSP slice, the shared query architecture, adapted
CLI and debugger, correctness evidence, and measured performance results. A Salsa
wrapper around whole-program recompilation or an isolated memoization benchmark
does not satisfy this specification.

## Repository evidence

- `src/inference/mod.rs`: existing group solver, annotations, `GroupMemo`, and
  whole-declaration fingerprints.
- `tests/src/inference.rs`: reuse tests, including consumer invalidation caused
  by changed provenance despite a body edit preserving its semantic type.
- `src/tracking.rs` and `src/symbol.rs`: definition-relative anchors, separate
  source mapping, and path-based symbol identity.
- `src/ir.rs`: declaration building, recursive grouping, structural effect
  canonicalization, and declaration metadata inference.
- `src/compile.rs`: compilation orchestration and accepted-state checks.
- `debug/src/snapshot.rs`: current all-files-clean semantic-analysis gate and
  root-buffer overlays.
- `cli/src/lib.rs` and `cli/src/cache.rs`: dependency loading, build context, and
  artifact caching.
- `.scratch/typed-effect-parameters/spec.md`: structural identity, inferred kinds,
  and transparent effect aliases.
- `.scratch/compiler-architecture-deepening/spec.md`: existing accepted-state,
  artifact, and diagnostic ownership invariants. This migration may freely
  replace the concrete APIs and adapt the debugger, as authorized in the interview.
- [Salsa overview](https://salsa-rs.github.io/salsa/overview.html) and
  [cycle handling](https://salsa-rs.github.io/salsa/cycles.html): dependency tracking
  and the conditions on query fixpoint iteration.
