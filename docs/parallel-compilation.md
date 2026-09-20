# Parallel compilation

The compiler's default `parallel` Cargo feature enables concurrent inference of
independent recursive groups. Each group still uses a local sequential solver;
Salsa coordinates dependencies and shares query results across database clones.
Parsing, IR construction, declaration inference, result assembly, pattern checks,
and lowering remain sequential.

## Execution policy

The default lazily creates one compiler-owned Rayon pool shared by all sessions
in the process, including CLI, LSP, and debugger sessions. It uses the available
CPU count, capped at four workers to limit simultaneous solver memory. It does
not initialize Rayon's global pool. If worker creation fails, default execution
falls back to the calling thread.

Embedders can select sequential execution, including in a parallel-enabled build:

```rust
use ruddy::execution::Execution;

Execution::sequential().run(|| {
    // New inference sessions and ordinary compilation calls run on this thread.
});
```

For a configurable private pool, call `Execution::with_threads(NonZeroUsize)` and
share clones of the returned policy. A count of one creates no pool. Use
`Session::with_execution` or `Host::with_execution` for persistent sessions, or
`Execution::run` to select defaults for sessions created within a call. Scoped
defaults apply on the calling thread; callers creating their own threads should
explicitly pass the policy to those threads. Existing sessions retain their
original policy.

Query database clones live only for one request. Workers inherit the request's
cancellation token; failure cancels sibling jobs, and the caller waits for worker
state to be dropped before updating inputs. Ordinary panic payloads are preserved.
Results are read from the shared cache and assembled in their original order.
Focused editor requests evaluate their selected dependency closure directly on
the calling thread, avoiding pool dispatch and redundant cache reads for small,
mostly cached requests. Full and background checks use the configured executor.
Dispatch happens outside tracked queries: worker queries record their own
dependencies, so this executor must not be used to implicitly collect the
dependencies of a calling Salsa query.

## Builds without threads

Disable default features on the compiler dependency:

```toml
ruddy = { package = "ruddy-compiler", path = "path/to/ruddy", default-features = false }
```

This removes Rayon and all compiler pool code. The same Salsa queries run directly
on the calling thread. Explicit requests for more than one worker return an
`Unsupported` error. Cargo features are additive: another dependency enabling
`parallel` re-enables it for the shared compiler crate.

The workspace's CLI, debugger, interpreter, and test crates forward their own
default `parallel` features without forcing compiler defaults. Validate both
configurations with `just test` and `just test --no-default-features`. Native
services such as filesystem watching still use threads; this feature controls
core compiler query execution, not every native application service.

Wasm cross-compilation is best-effort. Unrelated target compatibility failures
are outside the parallel compilation change's acceptance criteria.

## Measuring performance

`examples/inference-bench.rs` compares cold inference, unchanged rechecks, a body
edit, an interface edit, and full compilation. It reports median/p95 milliseconds
and actual group-solve counts as JSON. For example:

```sh
cargo run --release --example inference-bench -- 1 32 7
cargo run --release --example inference-bench -- 4 32 7
cargo run --release --example inference-bench -- 4 256 7 independent
```

The optional last argument selects `independent`, `chain`, or `recursive`;
otherwise all three run. Chains and one large recursive group deliberately
exercise limits on available parallelism. Compare one, two, and four workers
on an otherwise idle machine and record peak RSS alongside wall time.

`scripts/bench-lsp.py` measures editor latency and RSS on its retained 10,000-line
workspace. Compare native release executables built with default features and
with `--no-default-features`, keeping the source corpus and iteration count fixed.

## Validation and measurements (2026-09-20)

Both full release suites passed through the repository's memory-limited runner:

```sh
just test --release -- --test-threads=2
just test --release --no-default-features -- --test-threads=2
```

These passed 2,099 and 2,097 tests respectively, with ten ignored tests in each
configuration. Subsequent dispatch refinements passed all 35 analysis tests in
both configurations. Focused execution tests cover overlapping workers,
cancellation, panic propagation, pool reuse, and dropping database contexts;
equivalence tests cover incremental edits, recursion, artifacts, and structured
debugger snapshots. Formatting passed. Clippy reported only the three existing
large-enum warnings in the IR/parser.

The core library also passed
`cargo check -p ruddy-compiler --lib --no-default-features --target wasm32-unknown-unknown`.
This is a compile check, not a browser execution test. The workspace dependency
tree without default features contains no Rayon.

Full instrumented coverage exceeded the test runner's 4 GiB memory limit and was
killed; the limit was not raised. Focused coverage reached every executor
function, about 96% of its lines and 71% of branches. Platform/resource fallback
paths remain uncovered; this is not a claim of complete coverage.

Release measurements used an Intel i5-9600K with six CPUs on a shared Linux host.
Repeated nine-sample runs with 256 independent groups measured cold inference at
38–40 ms with one worker versus 30–34 ms with four, and full compilation at
53–54 ms versus 44–47 ms. Unchanged rechecks gained little. Small dependency-chain
and single-recursive-group workloads were slightly slower with four workers:
parallelism helps only when enough independent solver work is available.

The retained 10,000-line LSP corpus, measured over 50 edits, gave:

| Measurement | Threads disabled | Default parallel |
| --- | ---: | ---: |
| Initial background check | 1,314 ms | 1,113 ms |
| Interface-edit background check | 1,240 ms | 1,180 ms |
| Hover p95 | 111 ms | 117 ms |
| Completion p95 | 116 ms | 120 ms |
| Diagnostics p95 | 112 ms | 117 ms |
| Peak RSS | 682 MiB | 704 MiB |

The sequential executable predates the final focused-dispatch shortcut, so this
comparison also includes removal of a redundant cache read in the parallel
executable. Shared-host load introduces variability. These measurements support
a background throughput improvement, not a foreground latency improvement;
neither configuration met a 100 ms foreground p95 target on this host.
