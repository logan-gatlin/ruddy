# 08 Separate semantic evidence from host plans

Status: open
Type: task
Blocked by: 03

Spec: implementation step 2 and compiler work item 4. Move `NativeTemplate`,
`Direction`, and optional-field overlays out of `src/reification.rs` into the
backend's host planning; broaden semantic descriptors to carry effect rows and
callable contracts faithfully; keep exact cast behaviour and per-arrow evidence.
