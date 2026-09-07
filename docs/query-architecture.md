# Incremental analysis and the language server

The compiler, CLI, debugger, and `ruddy-lsp` use the same inference implementation.
`inference::Session` owns a persistent Salsa database. `analysis::Host` adds source
inputs, recovery, editor requests, and an accepted-state boundary for background
lowering. The CLI can still compile once through `compile`; its command-line
interface and dependency artifact cache are unchanged.

## Query boundaries

File contents are explicit inputs; lexing and parsing are cached per file.
The bundle loader still discovers modules and builds declaration information for
the current bundle. This includes transparent aliases, parameter kinds/variance,
and structural effect identity. The driver supplies dependencies as source
interfaces without waiting for their backend artifacts.

The compiler computes recursive groups with an iterative strongly connected
component algorithm. Each group has an immutable body input, a projection of the
declarations and published interfaces it consumes, and a local inference solver.
Salsa tracks the acyclic graph between these groups. Recursive inference and
generalization remain compiler operations; no Salsa cycle iteration is used.

Published interface equality contains semantic types, quantification, and
constraints. Source locations and explanatory facts are separate. Stable
evidence addresses let a cached consumer refer to the producer's current causal
facts. A separate value-flow traversal reconstructs effect origins when
diagnostics or detailed replay need them. Reordering closed record fields also
preserves the interface key and its evidence routes.

Unchanged body snapshots are reused before updating Salsa inputs. Each snapshot
owns only its group's terms and referenced names, rather than retaining an old
whole-workspace name table for every edited definition. Compiler values use
`Arc`; ordinary editor results avoid copying full solver traces. Detailed traces
remain available through `Trace::Complete` and the debugger.

## Current revisions and background work

Foreground analysis requests the active file's groups and their transitive
dependencies. Hover, completion, and navigation read the current recovered
program. Syntax errors do not substitute the last successful compilation.
Requests for another file extend the same revision's inferred set.

While idle, the worker completes the remaining groups and requests lowering and
backend-dependent checks. Lowering requires complete, accepted frontend state
and matching dependency interfaces. The shared acceptance path also serves CLI
compilation. The debugger exposes recovered semantic stages and requested solver
traces, while its backend stages retain this acceptance requirement.

The protocol router remains responsive while the analysis worker runs. New
document versions cancel obsolete work; requests interrupt idle background work.
Cancellation checks occur during parsing, IR construction, inference, lowering,
and between Varisat scheduler steps. Git acquisition runs on a separate thread
with the same cancellation signal; the editor can abandon its wait even during a
blocked network read. Git cache-lock acquisition also checks cancellation. The small vendored Varisat patch is described
in [its maintenance note](../vendor/varisat/RUDDY.md). A revision barrier prevents
obsolete diagnostic publication. Open documents receive versioned diagnostics;
background checks also publish and clear diagnostics for unopened reachable files.

## Workspace inputs

The workspace driver acquires sources, manifests, dependency selection, and the
root build configuration outside Salsa queries. Overlays apply across reachable
local dependencies. Git dependencies and installed std are analyzed from source
for navigation. The existing CLI artifact cache remains in use; Salsa caches are
in-memory and are not persisted. Successfully acquired Git selections survive
ordinary edits and are reconsidered when their specification or root lock inputs
change. Document identity resolves existing symlink prefixes, including for
new unsaved files; watcher paths retain the loader's original spelling.

The driver records every requested disk input, including absent and competing
module candidates, manifests, and the root lockfile. The server polls these exact
paths every 500 ms, including external path dependencies, without scanning whole
directories or requiring client file-watcher support. Buffer changes arrive
immediately through LSP notifications. One root/build configuration is active.

## Semantic investigation

No language restrictions were needed for this milestone.

- Unannotated and partially annotated recursive definitions still share a local
  solve. Complete annotations retain their existing contract behavior. Splitting
  an annotated member out of a recursive group could change recursive inference;
  the implementation keeps the compiler's existing grouping semantics. Complete
  contracts already isolate consumers outside that group, including when a
  broken implementation is repaired.
- Structural effects still derive identity from normalized operation interfaces.
  Equivalent effects continue to interoperate. Diagnostic effect origins are
  presentation dependencies, so moving those out of consumer solve keys removes
  unnecessary work without making effects nominal.
- Transparent aliases, inferred kinds/variance, and private declarations still
  contribute structural meaning. Queries follow the consumed declaration closure
  instead of treating a module boundary or privacy as semantic independence.

Declaration construction remains a bundle-wide cost. Profiling also exposed
quadratic group discovery, repeated full name-table retention, and trace copying;
those implementation costs were reduced directly. Requiring annotations, nominal
effects, or opaque aliases has no measured justification from this workload.

## Reproducing the measurements

```sh
cargo build --release -p ruddy-lsp
python3 scripts/bench-lsp.py target/release/ruddy-lsp 300
```

The Linux benchmark creates exactly 10,000 source lines: a root, 98 module files,
and a local path dependency. It includes structural records, handled structural
effects, function application, and mutually recursive groups. Std is disabled;
dependencies are already on disk and no downloads are involved. The corpus hash
and individual request samples are recorded with the results. Generated projects
are retained under `.scratch/salsa-query-architecture/benchmarks/`, in a new directory for each run; the
JSON result includes `benchmark_project`. They are not deleted after measurement.

Cold time includes server startup and initialization through the first active-file
diagnostics. After initial whole-project checking populates the cache, ordinary
edits rotate across the 98 module files. Each timing starts before sending
`didChange` and ends after the client parses the response or current-version
diagnostics, including scheduling and transport. P95 uses the nearest-rank
definition. The final interface edit constrains a dependency function consumed by
1,568 root computations; its foreground and background costs are reported
separately. Memory includes sampled process RSS across edits and the kernel's
peak RSS through the interface edit.

Measured results and validation are recorded in the
[implementation record](../.scratch/salsa-query-architecture/progress.md).
This workload measures one locally available dependency graph; it does not promise
the ordinary-edit deadline for network acquisition or broadly propagating
interface changes. Backend incrementality and simultaneous root configurations
remain outside this milestone.
