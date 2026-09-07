# Implementation and validation record

Status: Implemented and reviewed. Performance criteria met; the repository-wide
100% coverage target remains unmet.

The shared Salsa inference layer, source workspace driver, `ruddy-lsp`, CLI and
debugger migrations are implemented. See [architecture notes](../../docs/query-architecture.md)
for query boundaries, cancellation, semantic investigation, and the benchmark method.
No language semantics were changed. Both review axes and the fixes are recorded
in [the review](review.md).

## Performance

Measured on the Intel Core i5-9600K at 3.70 GHz, Linux
`7.2.3-ogc3.1.fc44.x86_64`, using Rust 1.98.1 and a release build.
The fixed 10,000-line corpus includes one local path dependency, structural
records and effects, and recursive groups. Std is disabled; no downloads are
included. All compiler and test processes had finished before measurement.

300 ordinary edits rotate through the 98 module files after initial full checking.
Times include client serialization, scheduling, and stdio response parsing.

| Measurement | Result | Acceptance target |
| --- | ---: | ---: |
| Warm hover p95 | 84.091 ms | <100 ms |
| Warm completion p95 | 88.324 ms | <100 ms |
| Active-file frontend diagnostics p95 | 85.296 ms | <200 ms |
| Cold active-file diagnostics | 123.283 ms | <2,000 ms |
| Initial full background checks | 596.215 ms | Measured separately |
| Broad dependency-interface edit: hover | 95.493 ms | Measured separately |
| Broad dependency-interface edit: background | 546.674 ms | Measured separately |
| Ordinary editing RSS: first / last | 317.33 / 330.25 MiB | <1,024 MiB |
| Ordinary editing maximum sampled RSS | 330.31 MiB | <1,024 MiB |
| Kernel peak RSS, including the interface edit | 518.71 MiB | <1,024 MiB |

[Full results and individual latency samples](performance.json) include the
corpus SHA-256 and measurement paths. The final corpus hash is
`6bddd6a4d94bbee19220ff15460ec0c413f634615cf5b0890c792616066bb154`.

The benchmark project is **preserved** at
[`benchmarks/workspace-6mjk4qgd/root/`](benchmarks/workspace-6mjk4qgd/root/Ruddy.toml),
with its sibling `dep/` source dependency. A byte-for-byte copy was kept outside
the build tree; the original measurement workspace was also retained. Both
copies were checked for exactly 10,000 source lines and the recorded hash.
Future script runs retain new workspaces under `benchmarks/`; generated copies
are ignored by Git and are never automatically deleted by this script.

These measurements cover ordinary body edits in this reference graph. Broad
interface changes, network acquisition, backend incrementality, and simultaneous
root configurations do not carry the ordinary-edit deadline.

## Validation

- Final instrumented full workspace suite: **1,725 passed, 9 ignored**, via
  `just cov --summary-only`, which invokes `just test` under the repository's
  memory-limited runner. Earlier uninstrumented full and focused suites also passed.
- `cargo clippy --workspace --all-targets`: passed without warnings.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Release `ruddy-lsp` build and the complete retained benchmark run: passed.
- Edit/fresh comparisons cover interfaces, SCC merge/split/undo, current diagnostic
  evidence, recovery, names/fields/type/effect completion, unsaved dependencies,
  configuration, missing/competing files, SAT cancellation, and session reuse.
- LSP tests cover UTF-16 incremental changes, rapid versions, stale-publication
  prevention, disk creation, unopened-file diagnostics and clearing, symlink URI
  navigation, and shutdown.
- Seeded offline Git tests cover acquired-selection reuse under cache-lock
  contention and cancellation/retry of a fresh acquisition. The acquisition
  thread and gix share the cancellation signal.
- Debugger tests cover recovered semantic stages, requested traces, and skipped
  backend stages for rejected input. Existing CLI, artifacts, and generated-code
  tests passed in the full suite.

Compiler coverage is **95.96% lines and 88.54% branches**; see the complete
[coverage report](coverage.txt). This does **not** meet `CONTRIBUTING.md`'s 100%
requirement. Gaps include both existing compiler code and new editor paths;
these figures are not a controlled baseline comparison. The coverage recipe now
excludes the LSP crate and vendored solver from its compiler-library report.
