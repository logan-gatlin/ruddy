# 08 Separate semantic evidence from host plans

Status: resolved
Type: task
Blocked by: 03

Spec: implementation step 2 and compiler work item 4. Move `NativeTemplate`,
`Direction`, and optional-field overlays out of `src/reification.rs` into the
backend's host planning; broaden semantic descriptors to carry effect rows and
callable contracts faithfully; keep exact cast behaviour and per-arrow evidence.

Resolution: `Direction`, `NativeTemplate`, and the optional-field overlay
live in `backend::host`; `reification` keeps the exact descriptor graph and
the policy that builds either. Descriptor arrows carry their effect row as
an `Effects` node (identities, payloads, arguments), so exact identity and
`describe` see effect contracts; a conversion shape still needs a pure
contract, and an exact mirror needs a closed, decided row.
