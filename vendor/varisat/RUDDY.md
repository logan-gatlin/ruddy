# Ruddy cancellation patch

Upstream: varisat 0.2.2, https://github.com/jix/varisat, MIT / Apache-2.0.

`Solver::solve_with` calls a supplied checkpoint between scheduler steps.
`solve` delegates with a no-op checkpoint and retains upstream behavior.
Cancellation unwinds and discards the local solver; no partially solved state
is published. The patch does not change clauses, assumptions, or search rules.
Upstream dev dependencies are omitted; production dependencies are unchanged. The build script no longer probes optional
external proof-checker executables, and compatibility lints from the upstream
0.2.2 sources/macros are suppressed within this vendored crate.

When upgrading, preserve the cancellation hook or replace it with an upstream
interruption API and run Ruddy's cancellation and SAT tests through `just test`.
