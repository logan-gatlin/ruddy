# Standard-library installer audit plan

- [x] Keep the contract scoped to atomic replacement of the installed standard-library namespace; clarify that it is not a multi-file reader transaction.
- [x] Serialize installers with a portable atomic `mkdir` claim.
- [x] Never automatically reclaim a lock from PID/owner metadata: a pathname-based reaper cannot prove it is removing the object it inspected.
- [x] Time out with an actionable stale-lock diagnostic and explicit manual-recovery procedure after abnormal death.
- [x] Add regressions for concurrent serialization and preservation of stale, malformed, and interrupted owner state.
- [x] Make EXIT cleanup explicitly best-effort while preserving the incoming status, always attempt owned-lock release, and fail a successful replacement with a manual-removal warning when its old tree cannot be deleted.
- [x] Add a deterministic injected-`rm` regression proving old-tree cleanup failure cannot strand the installer lock.
- [x] Run formatting, focused tests, workspace checks, Clippy, and coverage validation.
