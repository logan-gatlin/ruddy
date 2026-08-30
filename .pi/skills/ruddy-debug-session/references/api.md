# Shared debugger HTTP API

Base URL: `http://127.0.0.1:7878` by default.

## `GET /session`

Returns protocol version `1`:

```json
{
  "protocol": 1,
  "session_revision": 3,
  "request": { "document": "demo", "files": [], "revision": 18 },
  "view": {
    "active_file": "main.hc",
    "caret": 42,
    "tabs": ["ir", "solve"],
    "views": { "ir": "tree" },
    "split": true,
    "pane": 0,
    "panes": [{ "stage": "ir", "view": "tree", "filter": "", "step": null, "scroll": 0, "visible_nodes": [0, 1] }],
    "selection": null
  },
  "snapshot": { "files": [], "stages": [], "diagnostics": [] }
}
```

`session_revision` advances for every accepted browser compile and agent source edit; it is the optimistic concurrency token. `request.revision` identifies browser compiles. A 404 means no browser has compiled in this server process.

## `PUT /session`

Optimistically replaces one source file in the open document:

```json
{
  "base_revision": 3,
  "document": "demo",
  "path": "main.hc",
  "source": "let answer = 42n\n"
}
```

Returns the same full shape as `GET /session`, including the post-edit snapshot. It also persists the scratch document and emits `session-changed` to the browser.

Responses:

- `400`: invalid JSON, document name, or source path
- `404`: no live session, or file is not in the bundle
- `409`: stale revision or browser changed documents; re-read and merge
- `500`: persistence failure; the in-memory edit is rolled back

## `PUT /session/view`

The browser publishes its current context:

```json
{ "session_revision": 3, "view": { "active_file": "main.hc", "caret": 42 } }
```

Agents normally only read this endpoint's state through `GET /session`.

## Existing useful endpoints

- `POST /compile`: stateless snapshot plus publication into the shared session when its `session_revision` is current
- `GET /events?since=N`: long poll; `session-changed` data contains the new revision
- `GET /docs`, `GET|PUT|DELETE /docs/:name`: durable scratch document API; use `/session` for live edits
- `GET /status`: build/watch status and rustc rebuild errors
