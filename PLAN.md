# Standard-library installer audit plan

- [x] Keep the contract scoped to atomic replacement of the installed standard-library namespace; clarify that it is not a multi-file reader transaction.
- [x] Publish a fully written unique lock candidate through an atomic hard-link claim rather than creating an empty owner in place.
- [x] Recover stale or malformed owners without allowing a reaper to remove a newer owner, and clean abandoned lock artifacts after eventual success.
- [x] Add regressions for concurrent serialization, stale ownership, and the interrupted empty-owner publication state.
- [x] Run formatting, focused tests, workspace checks, Clippy, and coverage validation.
