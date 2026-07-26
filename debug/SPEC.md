# `ruddy-debug` — compiler debugging webapp

A local webapp for inspecting every stage of the `ruddy` compiler on a snippet of
source you edit in the browser. It exists to make the **change → observe** loop as
short as possible, for two different kinds of change:

- **change the program** — type in the editor, see tokens/AST/IR/errors update within
  a frame or two, with no button to press;
- **change the compiler** — edit `src/*.rs`, and the running page rebuilds, restarts,
  and re-renders the same snippet by itself.

Everything in this document is scoped to a single-file, localhost-only tool. It is
not a language server, not multi-file, and not intended to be deployed.

---

## 1. Shape of the thing

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ ruddy-debug   demo.hc ▾    ● 3 errors   lex 0.08ms · parse 0.21ms · ir 0.14ms │
├──────────────────────────────────┬────────────────────────────────────────────┤
│                                  │ Tokens  AST  IR  Symbols │ tree display raw│
│  1  let id = fn x => x           ├────────────────────────────────────────────┤
│  2                               │ ▾ type Pair  demo::Pair            7:6-7:10│
│  3  let compose = fn f g x =>    │   ▾ Struct  { first: <error>, … }  7:13-7:36│
│  4      f (g x)                  │     ▾ first: <error>               7:15-7:20│
│  5                               │         Error  <error>             7:22-7:23│
│  6  let bad = @                  │ ▾ let id  demo::id                 1:5-1:7 │
│         ~~~~~~~^                 │   ▾ Fn  fn x => x                  1:9-1:18│
│  7                               │       Arg  x                       1:12-1:13│
│  8  type = missingName           │     ▾ Ident  x                     1:17-1:18│
│         ^                        │                                            │
│                                  │                                            │
├──────────────────────────────────┴────────────────────────────────────────────┤
│ ✕ 6:12  lex    unrecognized character `@`                                     │
│ ✕ 8:6   parse  expected identifier, found `=`                                 │
│ ✕ 5:12  ir     undefined term `origin`                                        │
└───────────────────────────────────────────────────────────────────────────────┘
```

Three regions, all always visible:

- **Editor** (left, resizable) — the only input. Syntax-highlighted by the compiler's
  own token stream, with inline diagnostics.
- **Stage panels** (right) — one tab per compiler stage, optionally split into two
  panes so two stages are visible at once.
- **Diagnostic strip** (bottom) — every error from every stage, in source order,
  always visible, never scrolled out of reach.

### 1.1 The one idea that ties it together

Every piece of data the tool renders — a token, an AST node, an IR node, a symbol, a
diagnostic, and later a type or an instruction — carries the `Span` it came from.
That makes one interaction work everywhere:

> **Hover or select anything, anywhere, and every other panel highlights the thing it
> corresponds to.**

- Put the caret in the editor → the deepest AST node and the deepest IR node
  containing that offset are selected and scrolled into view in their panels.
- Hover an IR node → its source range lights up in the editor, and the AST node it
  was lowered from lights up too.
- Click a node that carries a `Symbol` → *every* occurrence of that symbol in every
  panel is highlighted, plus its binding site, plus its row in the Symbols panel.
- Click a diagnostic → editor jumps to the primary span, related spans (e.g. the
  `previous` span of `ErrorKind::Duplicate`) are highlighted in a secondary colour.

Correspondence is computed purely from span containment, so it works for a stage the
frontend has never heard of. This is what makes the tool cheap to extend.

---

## 2. Repository layout

The tool lives in `debug/` and is a separate crate; the compiler crate gains a
library target so the tool can call it.

```
Cargo.toml                  # + [workspace] members = ["debug"]
justfile                    # `just dev` — the supervisor lives here
src/
  lib.rs                    # NEW: pub mod ir/parse/symbol/token/tracking
  main.rs                   # thin bin, `use ruddy::...`
debug/
  SPEC.md                   # this document
  Cargo.toml                # ruddy-debug
  src/
    main.rs                 # arg parsing, startup, supervisor handshake
    server.rs               # tiny_http routing, event polling, static files
    docs.rs                 # scratch document storage
    watch.rs                # source mtime polling
    snapshot.rs             # runs the pipeline, catches panics, builds Snapshot
    wire.rs                 # serde types shared with the frontend
    stage/
      mod.rs                # Stage registry — the extension point
      tokens.rs
      ast.rs
      ir.rs
      symbols.rs
  web/
    index.html
    app.js                  # state, fetch loop, keybindings
    editor.js               # textarea + highlight overlay
    panel.js                # generic list/tree/text renderer
    diagnostics.js
    style.css
  scratch/                  # gitignored; autosaved documents live here
```

**Repository edits required outside `debug/`:**

1. Add `src/lib.rs` containing the five `pub mod` declarations currently at the top of
   `main.rs`; `main.rs` keeps only `fn main`, `report`, and `line_col`, importing from
   `ruddy::`.
2. Root `Cargo.toml` gains `[workspace]\nmembers = ["debug"]`.
3. `.gitignore` gains `debug/scratch/` and `debug/.dev/`.
4. `ir.rs` gains `TermKind::display` and `TypeKind::display`, two three-line wrappers
   around the existing private `Show`, mirroring `Program::display`. The debugger
   renders one node at a time and must use the compiler's own printer to do it;
   the alternative — a second printer living in the debug crate — would drift.

The compiler crate gains **no new dependencies** and **no `serde` derives**. All
serialization lives in `debug/src/stage/*`, which walks the public AST/IR types from
the outside. The compiler is never bent to suit the debugger.

### 2.1 Conventions

The debug crate follows the same house rules as the compiler: source files are
ordered *imports, macros, types, code*; comments explain why a thing is the way it is
rather than restating it.

---

## 3. Running it

```sh
just dev                            # supervised: rebuilds on any compiler change
just debug --open                   # one-shot, no supervision
just debug --doc regress-42         # open straight onto one scratch document
just test                           # compiler tests + debugger tests
```

`just dev` is the supervisor: it builds, copies the binary to `debug/.dev/`, runs it,
and starts over when the server exits with 75. Everything is a recipe in the root
`justfile`; there are no shell scripts.

Flags: `--port <n>` (default 7878), `--open` (launch a browser), `--doc <name>`
(document to open first, overriding whatever the browser had open), `--watch` /
`--no-watch` (watching defaults to on only under the supervisor).

The server binds `127.0.0.1` only and never `0.0.0.0`. There is no auth, no CORS
header, and no path outside `debug/web/` and `debug/scratch/` is readable or
writable.

---

## 4. Backend

### 4.1 Dependencies

```toml
[dependencies]
ruddy       = { path = ".." }
tiny_http   = "0.12"
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
```

Blocking, no async runtime. `tiny_http::Server` is shared across four worker threads;
a compile of a realistic snippet is well under a millisecond, so concurrency exists
only so a held-open event poll cannot wedge the next compile.

### 4.2 Endpoints

| Method | Path             | Body / Response |
|--------|------------------|-----------------|
| `GET`  | `/`              | `web/index.html` |
| `GET`  | `/static/*`      | files under `web/`, MIME by extension |
| `POST` | `/compile`       | `CompileRequest` → `Snapshot` |
| `GET`  | `/docs`          | `[{name, bytes, modified_ms}]` |
| `GET`  | `/docs/<name>`   | `{name, source}` |
| `PUT`  | `/docs/<name>`   | `{source}` → `{modified_ms}` |
| `DELETE`| `/docs/<name>`  | `204` |
| `GET`  | `/events?since=N`| held open until something happens (§4.6) |
| `GET`  | `/status`        | `{build, watching, doc, build_error}` |

`<name>` must match `[A-Za-z0-9_-]{1,64}` and is stored as `debug/scratch/<name>.hc`.
Any other name is a `400`; there is no path traversal surface because the name is
never treated as a path fragment beyond that check.

In debug builds, `serde_json` is used for both directions. Requests are capped at
1 MiB.

### 4.3 `CompileRequest`

```jsonc
{
  "source": "let id = fn x => x\n",
  "revision": 41,                       // echoed back; client drops stale responses
  "bundle": { "name": "demo", "version": "0.1.0" },
  "stages": null                        // null = all; or ["tokens","ir"] to narrow
}
```

`bundle` is editable in the UI because bundle name and version are inputs to symbol
mangling, and being able to change them and watch every mangled name move is the
whole point of the Symbols panel.

### 4.4 `Snapshot` — the wire format

```jsonc
{
  "revision": 41,
  "build": 7,                    // server build id; changes across a restart
  "source_len": 248,
  "line_starts": [0, 19, 20, …], // client maps offset → line/col without rescanning
  "stages": [Stage, …],
  "diagnostics": [Diagnostic, …],
  "panic": null                  // or Panic, see §4.7
}
```

```jsonc
// Stage
{
  "id": "ir",                    // stable identifier, used by keybindings and layout
  "title": "IR",
  "view": "tree",                // "list" | "tree" | "text"
  "status": "ok",                // "ok" | "partial" | "skipped" | "panicked"
  "summary": "3 types · 5 terms",// shown next to the tab title
  "micros": 143,
  "nodes": [Node, …],            // roots; a "list" view has one flat level
  "text": null,                  // for view:"text" (e.g. future assembly)
  "display": "type Pair = …",    // the compiler's own Display output, verbatim
  "debug": "Program {\n    …",   // {:#?} — the escape hatch, see §7
  "annotates": null              // or another stage id, see §7.2
}
```

```jsonc
// Node — the same shape for every stage, forever
{
  "id": 12,                      // unique within the stage
  "label": "Apply",              // node kind; rendered bold
  "text": "f (g x)",             // rendered value; rendered dim
  "span": [24, 31],              // [start, end) byte offsets, or null
  "generated": false,            // Span::is_generated()
  "symbol": 4,                   // index into the Symbols stage, or null
  "error": false,                // TermKind::Error, Kind mismatch, … → red
  "fields": [{"name": "namespace", "value": "term"}],
  "children": [Node, …]
}
```

A field whose name starts with `_` is for the page rather than the reader, and the
list view hides its column. The tokens stage uses `_class` to tell the editor which
colour a token gets, which is how the editor's highlighting is driven by the real
lexer rather than by a regex.

```jsonc
// Diagnostic
{
  "id": 2,
  "stage": "ir",                 // which stage produced it
  "severity": "error",
  "code": "undefined-term",      // stable, greppable, used for filtering
  "message": "undefined term `origin`",
  "span": [77, 83],
  "related": [{"span": [12, 16], "message": "first defined here"}]
}
```

The `related` array is what makes `ErrorKind::Duplicate` legible: the repeat and the
definition it repeats are shown as one diagnostic with two highlights, rather than as
the two independent lines `main.rs` prints today.

### 4.5 Stage implementations

Each stage is a function `fn build(cx: &Cx) -> Stage` in `debug/src/stage/`, where
`Cx` holds the source, the `FileManager`, the lex/parse/build outputs, and the `Mint`.
The registry in `stage/mod.rs` is a plain array; adding a stage is one file plus one
line.

- **`tokens`** (`view: "list"`) — one node per `Token`. `label` is the `Kind` variant
  name, `text` is its `Display`, `span` is the token span. This list also drives
  editor syntax highlighting (§5.2), so what you see coloured *is* what the lexer
  produced — a miscoloured keyword is a real lexer bug, visible without opening a
  panel.
- **`ast`** (`view: "tree"`) — `parse::Output::stmts` walked into nodes. `label` is
  the variant (`Let`, `Apply`, `Function`, `Struct`, `Ident`, `Natural`, `Unit`); `text` is the
  existing `Display` impl for that subtree, so the tree and the rendered source agree
  by construction. Struct fields become child nodes labelled with the field name,
  carrying the field-name span.
- **`ir`** (`view: "tree"`) — `ir::Program`, types group then terms group, matching
  `Show<'_, Program>`. Every `Ident` and `Fn` argument node carries `symbol`. Curried
  `Fn` chains are shown as nested nodes with an optional "collapse currying" toggle
  that renders `fn a => fn b => fn c => body` as one row.
- **`symbols`** (`view: "list"`) — one row per `Mint::symbols()`, with `name`, `path`
  (`Mint::path`), `mangle`, namespace, and local/global as `fields`. Each row also
  runs `demangle(mangle(s))` and flags a mismatch as `error: true` with a diagnostic —
  a free, continuous round-trip check on the mangling scheme while you edit it.

Lowering is total, so a stage always produces a tree even when earlier stages failed;
a stage that could not run at all reports `status: "skipped"` and the panel says so
instead of going blank.

### 4.6 Live reload

The `dev` recipe in the root `justfile` is a supervisor loop:

1. `cargo build -p ruddy-debug`, teeing the output to `debug/.dev/build-error.log`.
   On failure, re-launch the **last known good binary** with
   `RUDDY_DEBUG_BUILD_ERROR` pointing at that log, so the page stays alive and shows
   the compiler's own build error in the diagnostic strip. This is the difference
   between a broken build being a dead tab and being one more thing the tool tells
   you about. With no previous binary to fall back on, the loop waits and retries.
2. On success, copy the binary to `debug/.dev/ruddy-debug` and run it with
   `RUST_BACKTRACE=1` and an incremented build id.
3. The server polls the mtimes of `src/**/*.rs`, `debug/src/**/*.rs`, and both
   `Cargo.toml`s every 250 ms. On change it emits a `rebuilding` event, gives it
   120 ms to reach the page, and exits with status `75`. The loop sees `75` and goes
   to 1.

Polling rather than `notify` keeps the dependency list at four crates; 250 ms is
imperceptible next to the `cargo build` that follows it.

**Long polling, not server-sent events.** `tiny_http` writes a chunked body through
`chunked_transfer::Encoder`, which buffers until it has 8 KiB — a stream of 40-byte
SSE frames would sit in that buffer and never reach the browser. `GET /events?since=N`
instead holds the request open for up to 20 s and returns the moment something
happens, over the ordinary response path. Each response carries the current build id,
so a page that reconnects after a restart learns about it from the first poll that
gets through. Polls are answered on their own thread, never one of the four workers.

| event | client action |
|-------|---------------|
| `rebuilding` | show a "rebuilding…" chip; keep the last snapshot on screen |
| `reload-web` | a file under `web/` changed — reload the page |

A response whose `build` differs from the page's triggers an automatic recompile and
re-render — **with panel scroll position, expanded/collapsed tree state, active tab,
filters, and editor caret preserved**. Editing `src/ir.rs` therefore updates the IR
panel in place, which is the loop this tool exists to shorten. While the server is
down the page retries with a 250 ms → 800 ms backoff, so a slow `cargo build` is not
a screenful of failed requests.

### 4.7 Panics are results, not crashes

The compiler is under active development, so it will panic — on a bad `expect`, an
`unreachable!("the surface syntax has no modules")`, or an out-of-range span. The
server installs a panic hook that captures the message, location, and backtrace, and
runs each stage inside `catch_unwind`. A panicking stage yields
`status: "panicked"`, the snapshot's `panic` field is populated, and every stage that
already succeeded is still rendered.

```jsonc
"panic": {
  "stage": "ir",
  "message": "the name table already ruled out a repeat",
  "location": "src/ir.rs:383",
  "backtrace": "…"
}
```

The panel shows the message and location prominently with the backtrace behind a
disclosure. A panic never takes the server down and never loses your editor content.

---

## 5. Frontend

Vanilla ES modules, no build step, no `npm`, no vendored framework. Files are served
straight from `debug/web/`, so editing `panel.js` and hitting reload is the whole
frontend loop.

### 5.1 State model

One store, one render pass:

```js
{ doc, source, revision, snapshot, layout, selection, hover, follow }
```

- `selection` / `hover` are `{ origin, span, symbol, stage, node }` — the single
  source of truth behind all cross-highlighting. `origin` is what stops a panel from
  re-selecting the row you just clicked in it.
- A pane re-renders by building one HTML string and setting it once. Continuity comes
  from state that outlives the DOM rather than from diffing it: scroll position is
  saved and restored across the swap, and collapsed subtrees are keyed by index path
  (`/0/1/3`) rather than by node id, so they survive a recompile that renumbers every
  node. Highlighting never re-renders — it toggles classes on the rows already there,
  because it happens on every mouse move.

### 5.2 Editor

A `<textarea>` layered over a `<pre>` highlight canvas — the standard overlay
technique, no dependency, native undo/redo, native IME and clipboard.

The text on screen is the highlight layer; the textarea's own glyphs are transparent.
Two consequences drive the whole design of `editor.js`:

- **The layer is repainted in the same turn as the keystroke**, never after the
  compile. Waiting for the server would put every character on screen a compile
  behind the caret, which reads as a laggy editor even though the compile is fast.
  Between a keystroke and the snapshot catching up, the ranges from the last
  snapshot are *shifted by the edit* — a keystroke is one contiguous replacement, so
  everything before it is untouched, everything after it slides, and only ranges the
  edit lands inside are dropped. Colour is therefore right everywhere except the few
  characters actually being typed, and correct again ~130 ms later.
- **The layer is one element per line**, and an edit rewrites only the lines it
  touched. Rebuilding every line costs the length of the file per keystroke: 17 ms on
  a 200-line file, which is felt. Line-scoped updates make it 0.8 ms. Gutter rows hold
  no text — CSS counters number them — so inserting a line renumbers the file without
  touching a row's contents.

Overlays (selection, hover, symbol) are dropped while the buffer is ahead of the
compiler: they describe spans of a text that no longer exists, and a highlight over
the wrong characters is worse than none for a tenth of a second. Highlighting also
never re-renders panel rows; it adds and removes classes on the rows whose state
actually changed, because it runs on every caret move and every mouse move.

- **Highlighting** comes from the snapshot's token list, not a regex. Tokens are
  coloured by class (keyword, punctuation, identifier, number); any byte range not covered by
  a token is rendered in the "unlexed" colour, which makes lexer gaps visible.
- **Diagnostics** render as underlines in the overlay plus a gutter marker; hovering
  shows the message.
- **Selection echo** — the span currently selected in any panel is painted as a block
  highlight behind the text.
- **Debounce** — 120 ms after the last keystroke, `POST /compile`. In-flight requests
  are aborted; responses with a stale `revision` are dropped. `Ctrl+Enter` forces an
  immediate compile.

Tab inserts two spaces. No autocomplete, no bracket matching, no formatting — this is
a debugging surface, not an IDE.

### 5.3 Persistence

Content survives sessions through two layers:

1. **`localStorage`**, written on every keystroke (debounced 200 ms) under
   `ruddy-debug/doc/<name>`. Instant, survives server restarts and crashes.
2. **`debug/scratch/<name>.hc`**, `PUT` 1 s after typing stops. Durable, survives
   browser data clears, greppable, and directly runnable by the CLI compiler.

On load the server copy is authoritative; `localStorage` is used when the server is
unreachable or when it holds an unsaved edit newer than the server's `modified_ms`,
in which case a one-line "restored unsaved changes" notice appears. On first ever
run, `scratch/demo.hc` is seeded from the repository's `demo.hc`.

UI state (layout, active tabs, editor width, follow mode, bundle name/version) is
stored separately under `ruddy-debug/ui` and restored on load.

### 5.4 Documents

Debugging a compiler means accumulating a pile of small reproducers, so the editor is
document-oriented: a title-bar dropdown lists everything in `scratch/`, `Ctrl+P` is a
fuzzy switcher, and typing a name that matches nothing offers to create it.
Each document keeps its own caret position and panel state.

### 5.5 Panels

One generic renderer handles all three views, driven by `Stage.view`:

- **`list`** — a virtualized flat table. Columns are derived from the union of node
  `fields`, so the Symbols panel gets a real table for free.
- **`tree`** — indented rows with disclosure triangles; `label` bold, `text` dim,
  span shown right-aligned as `line:col-line:col`, error nodes red, generated spans
  italic and grey.
- **`text`** — monospace block with span-mapped lines (for future stages such as
  assembly).

Per-panel controls, in a single strip:

- **View switch** — `tree` / `display` / `raw`. `display` shows the stage's `display`
  string (the compiler's own printer — the ground truth the tests round-trip against);
  `raw` shows `{:#?}`.
- **Filter** — `/` focuses a filter box; a row survives if it matches or if one of its
  descendants does. Ancestors are kept but dimmed, so a match is never shown without
  the structure it sits in, and a filtered subtree expands itself.
- **Collapse** — `⊟` collapses to the top level, `⊞` expands everything.
- **Copy** — copy the selected node's path, span, or subtree as text.

`Ctrl+B` splits the panel region into two independently-tabbed panes; the common case
is AST above, IR below, watching a change ripple through lowering.

### 5.6 Diagnostic strip

Always visible, sorted by source position, one line each:
`✕  6:12  lex  unrecognized character '@'`. Grouped counts per stage sit in the title
bar. Clicking scrolls the editor and selects the primary span; `Ctrl+J` /
`Ctrl+Shift+J` step through diagnostics without touching the mouse. When there are no
diagnostics the strip collapses to a single green `no errors` line and gives its
height back to the editor.

### 5.7 Keybindings

| Key | Action |
|-----|--------|
| `Alt+1…9` | focus stage tab *n* (browser-safe; `Ctrl+n` is taken by the browser) |
| `Ctrl+Enter` | force recompile |
| `Ctrl+S` | force save |
| `Ctrl+P` | switch document — typing a name nothing matches creates it |
| `Ctrl+B` | toggle split panes |
| `Ctrl+J` / `Ctrl+Shift+J` | next / previous diagnostic |
| `Ctrl+.` | toggle follow-caret |
| `/` | focus the active panel's filter |
| `Esc` | clear selection and filter |

### 5.8 Visual design

Dark by default, light via `prefers-color-scheme`. One monospace family throughout —
this is a tool for reading code, and proportional text in a tree of code fragments
costs more than it gains. A single accent colour for selection, a second for
symbol-scoped highlight, red reserved exclusively for errors, and grey-italic for
anything generated. Density over decoration: rows are 20 px, there are no shadows, no
animation beyond a 60 ms highlight fade, and no chrome that does not carry data.

---

## 6. Performance budget

The loop is only useful if it is imperceptible. Targets for a 200-line snippet:

| Step | Budget | Measured (200 lines / 6.7 KB) |
|------|--------|-------------------------------|
| **keystroke → character on screen** | **< 4 ms** | 0.8 ms median, 2.4 ms worst |
| lex + parse + build | < 2 ms | 0.3 ms |
| snapshot serialization + transfer | < 4 ms | — |
| **keystroke → updated panels** | **< 140 ms** | 120 ms of which is the debounce |

The first row is the one that decides whether the editor feels alive, and it is the
only one on the keystroke path — everything else happens after the debounce, off to
the side. Saving to `localStorage` is debounced for the same reason: a synchronous
write has no business sitting between a character and the screen.

Stage timings are always on screen, so a regression in the compiler's own speed shows
up as a number that moved rather than as a vague feeling.

Rows are **not** virtualized. A pane builds one HTML string and sets it once, which is
comfortably fast at the scale a debugging snippet reaches (the 248-byte demo produces
59 IR rows; a 200-line file produces a few hundred). Virtualization is the first thing
to add if a pane ever renders thousands of rows, and the renderer is structured for
it: `visible[]` is already the flat list of rows that would be windowed.

---

## 7. Extensibility

This is the requirement the format is built around. Adding a stage should be a
backend-only change.

### 7.1 A new panel

1. Add `debug/src/stage/types.rs` with `pub fn build(cx: &Cx) -> Stage`.
2. Register it in the array in `stage/mod.rs`.

The frontend generates tabs, keybindings, filters, and cross-highlighting from
`Snapshot.stages` alone — it has no hardcoded knowledge of `tokens`, `ast`, `ir`, or
`symbols` beyond default tab order. A stage that emits `Node`s with spans participates
in every interaction described in §1.1 on the day it is written.

### 7.2 Annotating an existing panel

Type information usually wants to decorate the IR rather than live in its own tab. A
stage sets `annotates: "ir"` and returns nodes whose `id`s match the target stage's
node ids; the renderer paints each node's `text` as a right-aligned badge on the
corresponding IR row. So `let compose = fn f => …` gains an inline
`(b → c) → (a → b) → a → c` without a new panel and without touching the frontend.

### 7.3 Text stages

Assembly, LLVM-style IR, or any other textual output uses `view: "text"` with a
`spans` side-table mapping output line ranges back to source spans, keeping
click-to-source working. The renderer for this view is ~40 lines and is specified now
so the wire format does not need to change later.

### 7.4 Before a stage is taught to the debugger

Every stage always carries `debug` (`{:#?}`). A new IR field is visible in the `raw`
view the moment it exists, before anyone writes a renderer for it. There is no window
where the tool is behind the compiler and shows you nothing.

---

## 8. Testing

The debug crate is a debugging tool, so its tests cover the contract, not the pixels
(`just test`, 11 tests alongside the compiler's 51):

- `docs.rs` — name validation rejects `..`, `/`, `\`, empty, and over-long names, and
  a rejected name never becomes a path at all.
- `snapshot.rs` — compiling the repository's `demo.hc` produces all four stages with
  non-empty nodes; diagnostics arrive in source order with the expected codes; a
  duplicate carries the span of what it repeats; every symbol round-trips through
  `mangle`/`demangle`; an empty buffer and an invalid bundle are results rather than
  failures.
- **Span hygiene** — every span every stage emits must satisfy
  `source.get(start..end).is_some()`. This is a check on the *compiler*, not the
  debugger: bad `merge` arithmetic or an off-by-one in a lexer span fails here
  instead of showing up as a highlight over the wrong text.
- **Panic capture** — `guard` turns a panicking stage into a recorded `Panic` with a
  message and a location, keeps the first panic rather than the last, and returns
  `None` so the stages around it still render.
- **Wire round-trip** — a `Snapshot` serializes and parses back with its stage views
  intact, including the underscored fields the editor's colouring depends on.

---

## 9. Milestones

All built.

| # | Deliverable | |
|---|-------------|---|
| **M0** | `src/lib.rs` split, workspace, `ruddy-debug` binary that serves a page | ✅ |
| **M1** | `POST /compile` with the `tokens`, `ast`, `ir` stages | ✅ |
| **M2** | Editor with token highlighting, debounced compile, `localStorage` + scratch persistence | ✅ |
| **M3** | Generic list/tree renderer, tabs, span cross-highlighting, symbol highlighting | ✅ |
| **M4** | Diagnostic strip, inline underlines, `related` spans, keyboard navigation | ✅ |
| **M5** | `just dev` supervisor, live rebuild, state-preserving re-render, panic capture | ✅ |
| **M6** | Symbols panel with demangle round-trip check, documents + `Ctrl+P`, split panes | ✅ |

---

## 10. Non-goals

- Multi-file compilation, imports, or modules — the surface syntax has none yet, and
  `FileManager` is already threaded through, so this is a later stage, not a later
  rewrite.
- Editing the compiler from the browser. Rust source is edited in a real editor.
- Remote or shared use. Localhost, single user, no auth, no TLS.
- Being a language server, formatter, or REPL.
- Any JavaScript toolchain. If a feature needs a bundler, the feature is wrong.
