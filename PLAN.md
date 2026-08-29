# Standard-library installer audit plan

- [x] Keep the contract scoped to atomic replacement of the installed standard-library namespace; clarify that it is not a multi-file reader transaction.
- [x] Serialize installers with a portable atomic `mkdir` claim.
- [x] Never automatically reclaim a lock from PID/owner metadata: a pathname-based reaper cannot prove it is removing the object it inspected.
- [x] Time out with an actionable stale-lock diagnostic and explicit manual-recovery procedure after abnormal death.
- [x] Add regressions for concurrent serialization and preservation of stale, malformed, and interrupted owner state.
- [x] Run formatting, focused tests, workspace checks, Clippy, and coverage validation.
