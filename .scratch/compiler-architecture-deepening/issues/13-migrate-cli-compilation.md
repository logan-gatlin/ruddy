# 13: Migrate the CLI compilation path

**What to build:** Make CLI checking and building use core accepted compilation while retaining driver ownership of files, projects, dependencies, and target execution.

**Blocked by:** 12/Add accepted compilation states

**Status:** ready-for-agent

- [ ] CLI check and build commands use the core parsed-bundle compilation interface.
- [ ] CLI diagnostics preserve useful aggregation and source attribution from partial compilation.
- [ ] Filesystem discovery, manifest handling, dependency resolution, and installation remain in the CLI driver.
- [ ] JavaScript generation and execution remain backend work after target-neutral compilation.
- [ ] Existing CLI behavior remains stable except for reasonable diagnostic presentation changes.
- [ ] The complete `just test` suite passes.

