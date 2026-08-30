---
name: ruddy-debug-session
description: Collaboratively debug Ruddy compiler behavior in the user's live browser debugger. Use when investigating Ruddy source, diagnostics, compiler stages, inference/solve traces, or when the user asks to debug together.
---

# Ruddy live debugger

Work in the same session as the user's browser instead of reconstructing its state from files or running a separate compiler command.

## Connect

The debugger must be running (`just dev` is preferred) and open in the user's browser. The API defaults to `http://127.0.0.1:7878`; set `RUDDY_DEBUG_URL` if `--port` changed.

Use the helper from the repository root:

```bash
python3 .pi/skills/ruddy-debug-session/scripts/session.py context
```

A 404 means no browser has joined yet. Ask the user to open the debugger, then retry.

## Inspect what the user sees

```bash
# Active document/file/caret, visible panes, diagnostics, stage summaries
python3 .pi/skills/ruddy-debug-session/scripts/session.py context

# Exact active source (or another file)
python3 .pi/skills/ruddy-debug-session/scripts/session.py file
python3 .pi/skills/ruddy-debug-session/scripts/session.py file Math.hc

# Complete structured data for one compiler stage
python3 .pi/skills/ruddy-debug-session/scripts/session.py stage solve

# Entire protocol response, including every source, stage, node and diagnostic
python3 .pi/skills/ruddy-debug-session/scripts/session.py dump
```

Treat `view.active_file`, `view.caret`, `view.panes` (including filter, solve-step cursor, scroll, and visible node ids), and `view.selection` as the user's current context. The full `snapshot` is the exact compilation behind their panels. Locations are UTF-8 byte ranges; `snapshot.files[*].line_starts` maps them to lines.

Prefer structured fields over scraping rendered HTML:

- `snapshot.diagnostics`: codes, messages, labels/help/notes, and spans
- `snapshot.stages`: status, summary, timings, canonical text/raw debug output, and nodes
- node `span`, `symbol`, `owner`, and `link`: source and semantic relationships
- `snapshot.panic`: guarded compiler panic details and backtrace

## Edit together

Read the current source, produce the complete replacement file, then send it on stdin:

```bash
python3 .pi/skills/ruddy-debug-session/scripts/session.py file main.hc > /tmp/main.hc
# edit /tmp/main.hc with the normal coding tools
python3 .pi/skills/ruddy-debug-session/scripts/session.py write main.hc < /tmp/main.hc
```

`write` uses an optimistic session revision. On success the browser receives the edit immediately, its source and panels update together, and the scratch document is persisted. On HTTP 409, the human changed the session after it was read: inspect again, merge with their current source, and retry. Never blindly retry a stale replacement.

After an edit, run `context` or inspect the relevant stage again before deciding the result. Do not edit `debug/scratch` directly: that bypasses live synchronization and conflict detection.

## Collaboration etiquette

- Start by reporting the active file, caret line, visible stages, and relevant diagnostic so the user knows context is shared.
- Make small, explainable edits and inspect the new snapshot after each logical change.
- Preserve concurrent human changes. A conflict is a request to re-read and merge, not an error to override.
- Use ordinary repository editing tools for compiler implementation files under `src/` or `debug/`; `just dev` rebuilds those and the browser reconnects automatically. Use the session API only for the live Ruddy scratch program.
- The server is loopback-only and intentionally unauthenticated. Do not expose it on a network interface.

See [references/api.md](references/api.md) for the HTTP contract.
