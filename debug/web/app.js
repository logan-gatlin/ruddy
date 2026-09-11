// State, the compile loop, persistence, and everything that has to know about
// more than one region of the page.
//
// The loop is: keystroke → 120 ms → POST /compile → render. Editing the Rust
// compiler instead lands here as a `hello` on the event stream carrying a new
// build id, which re-runs the same snippet and re-renders in place.

import { createEditor } from "./editor.js";
import { createPanes, esc } from "./panel.js";
import { createDiagnostics } from "./diagnostics.js";

const COMPILE_AFTER = 120;
const SAVE_AFTER = 1000;
const UI_KEY = "ruddy-debug/ui";
const CLIENT_ID = crypto.randomUUID();
const docKey = (name) => `ruddy-debug/doc/${name}`;

/// The root file of every bundle, which the server decides and the page only
/// has to agree with: `snapshot::ROOT`. A document without one still opens —
/// the compiler says what is missing better than a refusal here would.
const ROOT = "main.rud";

const state = {
  doc: "demo",
  docs: [],
  /// Every file of the open document, root first, exactly as the compile
  /// request carries them.
  files: [],
  /// Bundle identity supplied independently of source, as a project manifest
  /// would supply it to the command-line driver.
  name: "demo",
  version: "0.1.0",
  root: ROOT,
  kind: "library",
  target: null,
  platform: null,
  integers: null,
  /// Runtime configuration is not used by snapshots, but is preserved when
  /// debugger documents are loaded and saved.
  run: {},
  /// Standard library: null uses the default Git source, false disables it,
  /// and a dependency specification selects a custom source.
  std: null,
  /// Last custom standard-library source, retained while std is off so "on"
  /// restores the user's selection instead of silently switching to default.
  /// This is debugger UI state, not part of the project manifest.
  stdPrevious: null,
  /// Dependency project specs keyed by source module alias.
  dependencies: {},
  /// Which of them is on screen. The editor holds one file at a time; the
  /// compiler is always given all of them.
  active: 0,
  /// Caret and scroll per file path, so switching back puts the reader where
  /// they were rather than at the top. In memory only: it is where you were
  /// looking a second ago, not something worth outliving the tab.
  where: {},
  revision: 0,
  /// Optimistic generation shared by browser compiles and agent edits.
  sessionRevision: 0,
  snapshot: null,
  build: null,
  buildError: null,
  notice: "",
  link: "down",

  hover: null,
  selection: null,
  follow: true,

  split: false,
  pane: 0,
  tabs: ["ir", "ast"],
  views: {},
  width: 46,

  collapsed: {},
  offsets: null,
  lines: null,
};

const listeners = new Map();
const el = (id) => document.getElementById(id);

const app = {
  state,

  on(event, fn) {
    listeners.set(event, [...(listeners.get(event) ?? []), fn]);
  },

  emit(event, payload) {
    for (const fn of listeners.get(event) ?? []) fn(payload);
  },

  setHover(mark) {
    // `mouseover` fires again for every child span of a row; repainting the
    // editor for a hover that has not moved is pure waste.
    const same =
      mark && state.hover
        ? mark.origin === state.hover.origin &&
          mark.stage === state.hover.stage &&
          mark.node === state.hover.node
        : mark === state.hover;
    if (same) return;
    state.hover = mark;
    app.emit("highlight");
  },

  setSelection(mark) {
    state.selection = mark;
    scheduleSharedView();
    // A span names a file as well as a range now, and revealing a range in a
    // file that is not on screen reveals nothing — so the file comes first.
    if (mark?.span && mark.origin !== "editor") {
      showFile(mark.span);
      editor.reveal(mark.span);
    }
    app.emit("highlight");
  },

  /// The stage with this id in the current snapshot, if it has one.
  stage(id) {
    return state.snapshot?.stages.find((stage) => stage.id === id);
  },

  /// Every source range that mentions one symbol, across every stage. This is
  /// what turns a click on an identifier into "show me this binding and all of
  /// its uses" everywhere at once.
  ///
  /// `node.symbol` and not `node.owner`: a node claims `symbol` only when its
  /// own span is somewhere the name was written, which is the whole reason the
  /// two fields are separate. A solve step is about a definition but is
  /// spanned by a sub-expression, and painting that as an occurrence would
  /// highlight `p.x` as a use of `fst`.
  spansOfSymbol(symbol) {
    const found = [];
    const visit = (nodes) => {
      for (const node of nodes) {
        if (node.symbol === symbol && node.span) found.push(node.span);
        if (node.children) visit(node.children);
      }
    };
    for (const stage of state.snapshot?.stages ?? []) visit(stage.nodes);
    return found;
  },

  collapsed(stageId) {
    state.collapsed[stageId] ??= new Set();
    return state.collapsed[stageId];
  },

  /// Where the file on screen sits in the snapshot's file list, or `-1` when
  /// the loader never read it: a file no `module` declaration names is an
  /// orphan, and nothing the compiler said is about it.
  fileIndex() {
    const path = active()?.path;
    return state.snapshot?.files.findIndex((file) => file.path === path) ?? -1;
  },

  /// The path a span was written in, or `""` when the snapshot is older than
  /// the file list it indexes into.
  pathOf(span) {
    return state.snapshot?.files[span?.file]?.path ?? "";
  },

  /// Whether a span is in the file on screen. Everything the editor paints has
  /// to pass this: the buffer holds one file, and a range from another would
  /// land at whatever offset it happened to name.
  here(span) {
    return !!span && span.file === app.fileIndex();
  },

  byteToChar: (byte) => offsets().toChar(byte),
  charToByte: (char) => offsets().toByte(char),

  /// Line and column of a byte offset in the buffer as it is right now — the
  /// gutter has to be right between keystrokes, when the snapshot is stale.
  lineCol(byte) {
    const starts = lines();
    let low = 0;
    let high = starts.length - 1;
    while (low < high) {
      const mid = (low + high + 1) >> 1;
      starts[mid] <= byte ? (low = mid) : (high = mid - 1);
    }
    return { line: low + 1, col: byte - starts[low] + 1 };
  },

  /// `line:col` for a span of the *snapshot*, which is what the panels and the
  /// diagnostics describe. Uses the line table the server sent for the file the
  /// span names, so a label can never disagree with the tree it labels — and a
  /// span in another file of the bundle is numbered in that file's lines rather
  /// than in the buffer's.
  spanLabel(span) {
    if (!span) return "";
    const starts = state.snapshot?.files[span.file]?.line_starts ?? lines();
    const at = (byte) => {
      let low = 0;
      let high = starts.length - 1;
      while (low < high) {
        const mid = (low + high + 1) >> 1;
        starts[mid] <= byte ? (low = mid) : (high = mid - 1);
      }
      return `${low + 1}:${byte - starts[low] + 1}`;
    };
    const from = at(span.range[0]);
    const to = at(span.range[1]);
    return from === to ? from : `${from}-${to}`;
  },

  save: saveUi,
  shareView: scheduleSharedView,
};

// ── boot ─────────────────────────────────────────────────────────────────

loadUi();
const editor = createEditor(el("editor-host"), app);
const panes = createPanes(el("panes"), app);
const diagnostics = createDiagnostics(el("diagnostics"), app);

wireTitlebar();
wireFiles();
wireKeys();
wireDrag();
connect();

app.on("input", (text) => {
  const file = active();
  if (!file) return;
  file.source = text;
  state.offsets = null;
  state.lines = null;
  cacheLocally();
  scheduleCompile();
  scheduleSave();
});

app.on("caret", (byte) => {
  scheduleSharedView();
  if (!state.follow) return;
  const span = { file: app.fileIndex(), range: [byte, byte] };
  app.setSelection({ origin: "editor", span, symbol: null, stage: null, node: null });
});

app.on("snapshot", renderTitlebar);
app.on("status", renderTitlebar);

// Opening the first document is the last thing in the file: a top-level `await`
// suspends module evaluation, and everything declared below it would still be
// uninitialised when the compile it triggers runs.

// ── documents ────────────────────────────────────────────────────────────

async function refreshDocs() {
  state.docs = await get("/docs").catch(() => []);
}

async function openDoc(name) {
  const server = await get(`/docs/${encodeURIComponent(name)}`).catch(() => null);
  const cached = readCache(name);

  // The server copy wins, except when this browser holds an edit that never
  // made it there — a crash, a lost connection, a closed tab mid-keystroke.
  let files = server?.files ?? cached?.files ?? [];
  state.notice = "";
  const recovered = cached &&
    !sameDocumentConfiguration(cached, server, name) &&
    (!server || cached.at > server.modified_ms);
  if (recovered) {
    files = cached.files ?? [];
    state.notice = `restored unsaved changes to ${name}`;
  }
  // A document nobody has written yet — the switcher creates one by opening a
  // name that does not exist — starts as the one file a bundle cannot do
  // without.
  if (!files.length) files = [{ path: ROOT, source: "" }];

  state.doc = name;
  state.files = files;
  const configured = recovered ? cached : server ?? cached;
  state.name = configured?.bundle_name ?? configured?.name ?? name;
  state.version = configured?.version ?? "0.1.0";
  state.root = configured?.root ?? ROOT;
  state.kind = configured?.kind ?? "library";
  state.target = configured?.target ?? null;
  state.platform = configured?.platform ?? null;
  state.integers = configured?.integers ?? null;
  state.run = configured?.run ?? {};
  state.std = configured?.std ?? null;
  // Older caches have no stdPrevious. A currently configured custom source is
  // itself the best compatible value; otherwise retain the recovery cache's
  // UI-only value across a server round trip.
  state.stdPrevious = state.std !== null && state.std !== false
    ? state.std
    : cached?.stdPrevious ?? null;
  state.dependencies = configured?.dependencies ?? {};
  state.snapshot = null;
  state.active = 0;
  state.where = {};
  state.offsets = null;
  state.lines = null;
  state.selection = null;
  editor.setValue(files[0].source, { caret: 0, top: 0, left: 0 });
  renderFiles();
  saveUi();
  renderTitlebar();
  // Dependency compilation resolves from the saved document directory. Create
  // it before the first compile so a new or locally recovered document can use
  // its standard library immediately.
  if (!server) await saveNow();
  await compileNow();
  editor.focus();
}

/// The file on screen, and its text. Everything that says "the buffer" means
/// this one; the rest of the bundle is only ever a compile away.
function active() {
  return state.files[state.active] ?? null;
}

function source() {
  return active()?.source ?? "";
}

/// Whether two file lists hold the same bundle. Paths and contents, in order:
/// the server hands them back in the order it wants the strip shown in, so a
/// list that only differs in order is a different list.
function sameFiles(a, b) {
  return (
    a?.length === b?.length &&
    a.every((file, i) => file.path === b[i].path && file.source === b[i].source)
  );
}

/// Compare every editable part of a document. The server calls the configured
/// identity `bundle_name` because its `name` is the storage key; the browser
/// cache calls it `name`, so normalize that one wire-format difference first.
function sameDocumentConfiguration(cache, server, documentName) {
  if (!cache || !server) return false;
  const cacheName = cache.bundle_name ?? cache.name ?? documentName;
  const serverName = server.bundle_name ?? server.name ?? documentName;
  const cacheDependencies = cache.dependencies ?? {};
  const serverDependencies = server.dependencies ?? {};
  return (
    cacheName === serverName &&
    (cache.version ?? "0.1.0") === (server.version ?? "0.1.0") &&
    (cache.root ?? ROOT) === (server.root ?? ROOT) &&
    (cache.kind ?? "library") === (server.kind ?? "library") &&
    (cache.target ?? null) === (server.target ?? null) &&
    (cache.platform ?? null) === (server.platform ?? null) &&
    (cache.integers ?? null) === (server.integers ?? null) &&
    JSON.stringify(cache.run ?? {}) === JSON.stringify(server.run ?? {}) &&
    JSON.stringify(cache.std ?? null) === JSON.stringify(server.std ?? null) &&
    sameFiles(cache.files ?? [], server.files ?? []) &&
    JSON.stringify(cacheDependencies) === JSON.stringify(serverDependencies)
  );
}

let cacheTimer = null;

/// Debounced off the keystroke: `localStorage` is synchronous, and there is no
/// reason for a write to sit between a character and the screen.
function cacheLocally() {
  clearTimeout(cacheTimer);
  cacheTimer = setTimeout(() => {
    try {
      const at = Date.now();
      localStorage.setItem(
        docKey(state.doc),
        JSON.stringify({
          files: state.files,
          name: state.name,
          version: state.version,
          root: state.root,
          kind: state.kind,
          target: state.target,
          platform: state.platform,
          integers: state.integers,
          run: state.run,
          std: state.std,
          stdPrevious: state.stdPrevious,
          dependencies: state.dependencies,
          at,
        }),
      );
    } catch {
      // A full quota is not worth interrupting the loop over; the server copy
      // is the durable one anyway.
    }
  }, 200);
}

function readCache(name) {
  try {
    return JSON.parse(localStorage.getItem(docKey(name)) ?? "null");
  } catch {
    return null;
  }
}

let saveTimer = null;
function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(saveNow, SAVE_AFTER);
}

async function saveNow() {
  clearTimeout(saveTimer);
  try {
    await fetch(`/docs/${encodeURIComponent(state.doc)}`, {
      method: "PUT",
      body: JSON.stringify({
        name: state.name,
        version: state.version,
        root: state.root,
        kind: state.kind,
        target: state.target,
        platform: state.platform,
        integers: state.integers,
        run: state.run,
        ...(state.std === null ? {} : { std: state.std }),
        dependencies: state.dependencies,
        files: state.files,
      }),
    });
  } catch {
    // Offline: the local copy already has it, and the next save will catch up.
  }
}

// ── compile loop ─────────────────────────────────────────────────────────

let compileTimer = null;
let inflight = null;

function scheduleCompile() {
  clearTimeout(compileTimer);
  compileTimer = setTimeout(compileNow, COMPILE_AFTER);
}

async function compileNow() {
  clearTimeout(compileTimer);
  const revision = ++state.revision;
  inflight?.abort();
  const controller = new AbortController();
  inflight = controller;

  try {
    const response = await fetch("/compile", {
      method: "POST",
      signal: controller.signal,
      body: JSON.stringify({
        name: state.name,
        version: state.version,
        root: state.root,
        kind: state.kind,
        target: state.target,
        platform: state.platform,
        integers: state.integers,
        document: state.doc,
        files: state.files,
        ...(state.std === null ? {} : { std: state.std }),
        dependencies: state.dependencies,
        revision,
        session_revision: state.sessionRevision,
        client_id: CLIENT_ID,
        view: sharedView(),
      }),
    });
    const snapshot = await response.json();
    // The source changed after this request left the browser. Pull the winner
    // instead of letting an in-flight compile undo a human or agent edit.
    if (response.headers.get("X-Session-Accepted") === "false") {
      await syncSharedSession();
      return;
    }
    const sharedRevision = Number(response.headers.get("X-Session-Revision"));
    if (Number.isFinite(sharedRevision)) state.sessionRevision = sharedRevision;
    // A slower earlier compile must never overwrite a newer one.
    if (snapshot.revision !== state.revision) return;
    state.snapshot = snapshot;
    state.build = snapshot.build;
    setLink("live");
    app.emit("snapshot");
  } catch (error) {
    if (error.name !== "AbortError") setLink("down");
  }
}

// ── live rebuild ─────────────────────────────────────────────────────────

/// Long-poll the server forever. Each response carries the build id, so a
/// restart is noticed by the first poll that gets through afterwards: the same
/// snippet is re-run and the panels update in place, with the caret, the tabs
/// and the tree state where they were.
async function connect() {
  let seq = null;
  let rebuilding = false;
  let backoff = 250;

  for (;;) {
    try {
      const response = await fetch(seq === null ? "/events" : `/events?since=${seq}`);
      const update = await response.json();
      const first = seq === null;
      seq = update.seq;

      for (const { event, data } of update.events) {
        if (event === "rebuilding") rebuilding = true;
        if (event === "reload-web") location.reload();
        if (event === "session-changed" && data.revision > state.sessionRevision) {
          await syncSharedSession();
        }
      }
      setLink(rebuilding ? "busy" : "live");

      if (first || update.build !== state.build) {
        await refreshStatus();
        rebuilding = false;
        setLink("live");
        if (!first && state.build !== null) {
          // A restarted server has no in-memory session yet. Rejoin from zero;
          // the scratch document and this browser still hold the source.
          state.sessionRevision = 0;
          await compileNow();
        }
        state.build = update.build;
        renderTitlebar();
      }
      backoff = 250;
    } catch {
      // The server is down — most likely rebuilding itself. Back off a little
      // so a long `cargo build` is not thirty failed requests in the console,
      // but stay fast enough that the page is back before you look up.
      setLink(rebuilding ? "busy" : "down");
      await new Promise((done) => setTimeout(done, backoff));
      backoff = Math.min(800, Math.round(backoff * 1.5));
    }
  }
}

async function refreshStatus() {
  const status = await get("/status").catch(() => null);
  state.buildError = status?.build_error ?? null;
  app.emit("status");
}

/// Pull an agent edit into the live editor. The server response carries the
/// exact snapshot the agent saw after editing, so source and every panel move
/// together rather than waiting for a second compile.
async function syncSharedSession(shared = null) {
  shared ??= await get("/session").catch(() => null);
  if (!shared || shared.session_revision < state.sessionRevision) return;

  const request = shared.request;
  const view = shared.view ?? {};
  state.sessionRevision = shared.session_revision;
  state.doc = request.document;
  state.name = request.name;
  state.version = request.version;
  state.root = request.root;
  state.kind = request.kind ?? "library";
  state.target = request.target ?? null;
  state.platform = request.platform ?? null;
  state.integers = request.integers ?? null;
  state.std = request.std ?? null;
  state.dependencies = request.dependencies ?? {};
  state.files = request.files;
  state.snapshot = shared.snapshot;
  state.revision = Math.max(state.revision, shared.snapshot.revision);
  state.build = shared.snapshot.build;

  const wanted = state.files.findIndex((file) => file.path === view.active_file);
  state.active = wanted >= 0 ? wanted : 0;
  state.tabs = view.tabs?.length ? view.tabs : state.tabs;
  state.views = view.views ?? state.views;
  state.split = view.split ?? state.split;
  state.pane = view.pane ?? 0;
  state.selection = view.selection ?? null;
  state.offsets = null;
  state.lines = null;

  const file = active();
  editor.setValue(file?.source ?? "", {
    caret: app.byteToChar(view.caret ?? 0),
    top: state.where[file?.path]?.top ?? 0,
    left: state.where[file?.path]?.left ?? 0,
  });
  renderFiles();
  cacheLocally();
  saveUi();
  app.emit("layout");
  app.emit("file");
  app.emit("snapshot");
}

function setLink(link) {
  if (state.link === link) return;
  state.link = link;
  renderTitlebar();
}

// ── title bar ────────────────────────────────────────────────────────────

function parseDependencies(input, allowStd = false) {
  if (!input.trim()) return {};
  const dependencies = {};
  for (const [index, raw] of input.split(",").entries()) {
    const part = raw.trim();
    const equals = part.indexOf("=");
    const left = part.slice(0, equals).trim();
    let source = part.slice(equals + 1).trim();
    const at = left.indexOf("@");
    const alias = (at < 0 ? left : left.slice(0, at)).trim();
    const bundleName = (at < 0 ? alias : left.slice(at + 1)).trim();
    if (equals <= 0 || !alias || !bundleName || !source) {
      throw new Error(`Dependency ${index + 1} must be written as alias=folder, alias@bundle=folder, or alias=https://git-url[#branch=name|#tag=name|#rev=commit].`);
    }
    if (alias === "std" && !allowStd) {
      throw new Error("Dependency alias std is reserved; use the standard-library control instead.");
    }
    if (Object.hasOwn(dependencies, alias)) throw new Error(`Dependency ${alias} is declared more than once.`);

    if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(source) && !source.startsWith("https://")) {
      throw new Error(`Dependency ${alias} uses an unsupported URL scheme; Git dependencies must use HTTPS.`);
    }
    if (source.startsWith("https://")) {
      const detail = { git: source };
      const selector = source.match(/#(branch|tag|rev)=([^#]+)$/);
      if (selector) {
        detail.git = source.slice(0, selector.index);
        detail[selector[1]] = selector[2].trim();
        if (!detail[selector[1]]) throw new Error(`Dependency ${alias} has an empty Git selector.`);
      }
      if (alias !== bundleName) detail.bundle = bundleName;
      dependencies[alias] = detail;
    } else {
      dependencies[alias] = alias === bundleName ? source : { bundle: bundleName, path: source };
    }
  }
  return dependencies;
}

function printDependency(alias, specification) {
  if (typeof specification === "string") return `${alias}=${specification}`;
  const bundle = specification.bundle && specification.bundle !== alias
    ? `@${specification.bundle}`
    : "";
  if (specification.git) {
    const selector = ["branch", "tag", "rev"].find((kind) => specification[kind]);
    const suffix = selector ? `#${selector}=${specification[selector]}` : "";
    return `${alias}${bundle}=${specification.git}${suffix}`;
  }
  return `${alias}${bundle}=${specification.path}`;
}

function wireTitlebar() {
  el("doc-button").addEventListener("click", openSwitcher);
  el("bundle").addEventListener("click", () => {
    const entered = window.prompt("Bundle identity (name@version)", `${state.name}@${state.version}`);
    if (entered === null) return;
    try {
      const at = entered.lastIndexOf("@");
      if (at <= 0 || entered.includes(",") || !entered.slice(at + 1).trim()) {
        throw new Error("A bundle identity must be written as name@version.");
      }
      state.name = entered.slice(0, at).trim();
      state.version = entered.slice(at + 1).trim();
    } catch (error) {
      window.alert(error.message);
      return;
    }
    state.snapshot = null;
    cacheLocally();
    scheduleSave();
    scheduleCompile();
    renderTitlebar();
  });
  el("root").addEventListener("click", () => {
    const entered = window.prompt("Root source path", state.root);
    if (entered === null || !entered.trim()) return;
    state.root = entered.trim();
    cacheLocally();
    scheduleSave();
    scheduleCompile();
    renderTitlebar();
  });
  el("kind").addEventListener("change", event => {
    state.kind = event.target.value;
    cacheLocally();
    scheduleSave();
    scheduleCompile();
  });
  el("target").addEventListener("change", event => {
    state.target = event.target.value || null;
    cacheLocally();
    scheduleSave();
    scheduleCompile();
  });
  el("integers").addEventListener("change", event => {
    state.integers = event.target.value ? Number(event.target.value) : null;
    cacheLocally();
    scheduleSave();
    scheduleCompile();
  });
  el("platform").addEventListener("change", event => {
    state.platform = event.target.value || null;
    cacheLocally();
    scheduleSave();
    scheduleCompile();
  });
  el("std").addEventListener("click", () => {
    const current = state.std === null
      ? "default"
      : state.std === false
        ? "off"
        : printDependency("std", state.std)
            .slice("std".length)
            .replace(/^=/, "");
    const entered = window.prompt("Standard library (on, default, off, folder, @bundle=folder, or https://url#branch=name)", current);
    if (entered === null) return;
    const value = entered.trim();
    try {
      if (value === "on" || value === "true") state.std = state.stdPrevious ?? null;
      else if (!value || value === "default") state.std = null;
      else if (value === "off" || value === "false") state.std = false;
      else {
        state.std = parseDependencies(value.startsWith("@") ? `std${value}` : `std=${value}`, true).std;
        state.stdPrevious = state.std;
      }
    } catch (error) {
      window.alert(error.message);
      return;
    }
    cacheLocally();
    scheduleSave();
    scheduleCompile();
    renderTitlebar();
  });
  el("dependencies").addEventListener("click", () => {
    const current = Object.entries(state.dependencies).map(([alias, specification]) => printDependency(alias, specification)).join(", ");
    const entered = window.prompt("Dependencies (alias=folder or alias=https://url#branch=name; @bundle aliases, comma-separated)", current);
    if (entered === null) return;
    try {
      state.dependencies = parseDependencies(entered);
    } catch (error) {
      window.alert(error.message);
      return;
    }
    cacheLocally();
    scheduleSave();
    scheduleCompile();
    renderTitlebar();
  });
  el("follow").addEventListener("click", () => toggleFollow());
  el("split").addEventListener("click", () => toggleSplit());
}

function renderTitlebar() {
  el("doc-name").textContent = state.doc;

  el("bundle").textContent = `${state.name}@${state.version}`;
  el("bundle").classList.toggle("none", state.snapshot && !state.snapshot.bundle);
  el("root").textContent = state.root;
  el("kind").value = state.kind;
  el("target").value = state.target ?? "";
  el("platform").value = state.platform ?? "";
  el("integers").value = state.integers === null ? "" : String(state.integers);
  el("std").textContent = state.std === null
    ? "std: default"
    : state.std === false
      ? "std: off"
      : "std: custom";

  const dependencies = el("dependencies");
  const dependencyCount = Object.keys(state.dependencies).length;
  dependencies.textContent = dependencyCount === 1
    ? "1 dependency"
    : `${dependencyCount || "no"} dependencies`;

  el("follow").classList.toggle("on", state.follow);
  el("split").classList.toggle("on", state.split);

  const link = el("link");
  link.className = `chip status ${state.link}`;
  link.textContent =
    state.link === "live"
      ? `build ${state.build ?? "?"}`
      : state.link === "busy"
        ? "rebuilding…"
        : "disconnected";

  const snapshot = state.snapshot;
  const errors = snapshot?.diagnostics.length ?? 0;
  const timings = (snapshot?.stages ?? [])
    // One chip per phase, not per tab. A stage whose work happened inside
    // another stage's phase — an annotator, or a second view of a phase's
    // output — carries no timing at all, and a second chip for it would be the
    // same microseconds counted twice.
    //
    // Owning a phase is what the stage says, not what its number came out as:
    // `micros > 0` read a measurement as that fact, and the measurement is
    // truncated to whole microseconds, so a phase quick enough to round to zero
    // lost its chip.
    .filter((stage) => stage.micros !== null)
    .map((stage) => {
      const ms = stage.micros / 1000;
      const slow = ms > 5 ? " slow" : "";
      return `<b>${stage.title}</b> <span class="${slow.trim()}">${ms.toFixed(2)}ms</span>`;
    })
    .join(" · ");
  el("timings").innerHTML =
    (errors ? `<span style="color:var(--error)">● ${errors} error${errors === 1 ? "" : "s"}</span> ` : "") +
    timings;
}

function toggleFollow() {
  state.follow = !state.follow;
  saveUi();
  renderTitlebar();
}

function toggleSplit() {
  state.split = !state.split;
  state.pane = 0;
  saveUi();
  renderTitlebar();
  app.emit("layout");
}

// ── document switcher ────────────────────────────────────────────────────

let switcherAt = 0;

async function openSwitcher() {
  await refreshDocs();
  const overlay = el("overlay");
  const input = el("overlay-input");
  overlay.hidden = false;
  input.value = "";
  switcherAt = 0;
  renderSwitcher();
  input.focus();
}

function closeSwitcher() {
  el("overlay").hidden = true;
  editor.focus();
}

function switcherMatches() {
  const query = el("overlay-input").value.trim().toLowerCase();
  const found = state.docs.filter((doc) => doc.name.toLowerCase().includes(query));
  // Typing a name nothing matches is how a document is created: no separate
  // "new" command to remember.
  const exact = state.docs.some((doc) => doc.name === query);
  if (query && !exact && /^[A-Za-z0-9_-]{1,64}$/.test(query)) {
    found.push({ name: query, create: true });
  }
  return found;
}

function renderSwitcher() {
  const found = switcherMatches();
  switcherAt = Math.max(0, Math.min(switcherAt, found.length - 1));
  el("overlay-list").innerHTML = found
    .map((doc, i) => {
      const meta = doc.create
        ? `<span class="meta new">create</span>`
        : `<span class="meta">${(doc.bytes / 1024).toFixed(1)}k</span>`;
      return `<li class="${i === switcherAt ? "on" : ""}" data-name="${doc.name}">${doc.name}${meta}</li>`;
    })
    .join("");
}

el("overlay-input").addEventListener("input", () => {
  switcherAt = 0;
  renderSwitcher();
});

el("overlay-list").addEventListener("click", (event) => {
  const name = event.target.closest("li")?.dataset.name;
  if (name) {
    closeSwitcher();
    openDoc(name);
  }
});

el("overlay-input").addEventListener("keydown", (event) => {
  const found = switcherMatches();
  if (event.key === "Escape") return closeSwitcher();
  if (event.key === "ArrowDown") {
    switcherAt = Math.min(switcherAt + 1, found.length - 1);
    renderSwitcher();
    event.preventDefault();
  } else if (event.key === "ArrowUp") {
    switcherAt = Math.max(switcherAt - 1, 0);
    renderSwitcher();
    event.preventDefault();
  } else if (event.key === "Enter" && found[switcherAt]) {
    const doc = found[switcherAt];
    closeSwitcher();
    if (doc.create) {
      openDoc(doc.name).then(saveNow);
    } else {
      openDoc(doc.name);
    }
  }
});

el("overlay").addEventListener("mousedown", (event) => {
  if (event.target === el("overlay")) closeSwitcher();
});

// ── the file strip ───────────────────────────────────────────────────────
//
// A document is a bundle, so the editor holds one file of several. The strip
// is the whole of that: which files there are, which one is on screen, and the
// three things you can do to the set. `main.rud` gets no special treatment —
// renaming or deleting it is allowed, and what happens is that the compiler
// says the bundle has lost its root, which is a better teacher than a disabled
// button.

/// The tab being renamed, or created when its index is `-1`. Renaming happens
/// in place rather than in a dialog: the name is already on screen, and the
/// only thing a dialog would add is a second place to look.
let renaming = null;

function wireFiles() {
  const strip = el("files");

  strip.addEventListener("click", (event) => {
    const act = event.target.closest("[data-act]")?.dataset.act;
    if (act === "add") return startRename(-1);

    const tab = event.target.closest(".ftab");
    if (!tab || tab.dataset.index === undefined) return;
    const index = Number(tab.dataset.index);
    if (act === "rename") return startRename(index);
    if (act === "delete") return deleteFile(index);
    selectFile(index);
    editor.focus();
  });

  // The name is what you would try to edit, so double-clicking it does.
  strip.addEventListener("dblclick", (event) => {
    const tab = event.target.closest(".ftab");
    if (tab?.dataset.index !== undefined) startRename(Number(tab.dataset.index));
  });

  strip.addEventListener("keydown", (event) => {
    if (!event.target.matches(".fname")) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commitRename(event.target);
    }
    if (event.key === "Escape") {
      event.preventDefault();
      cancelRename();
    }
  });

  // Clicking away cancels rather than commits: a half-typed name is not a file
  // anybody asked for, and the tab it came from is still there to try again.
  strip.addEventListener("focusout", (event) => {
    if (event.target.matches(".fname") && renaming) cancelRename();
  });
}

function renderFiles() {
  const strip = el("files");
  const tabs = state.files.map((file, i) => {
    if (renaming?.index === i) return nameBox(file.path);
    const on = i === state.active ? " on" : "";
    return (
      `<span class="ftab${on}" data-index="${i}">` +
      `<span class="name">${esc(file.path)}</span>` +
      `<span class="act" data-act="rename" title="Rename">✎</span>` +
      `<span class="act" data-act="delete" title="Delete">✕</span>` +
      `</span>`
    );
  });
  if (renaming?.index === -1) tabs.push(nameBox(""));
  strip.innerHTML =
    tabs.join("") + `<span class="ftab add" data-act="add" title="New file">+</span>`;

  const box = strip.querySelector(".fname");
  if (!box) return;
  box.focus();
  // Select the module name and leave the `.rud` alone: renaming a file is
  // almost always renaming the module it holds.
  box.setSelectionRange(0, Math.max(0, box.value.length - ".rud".length));
}

function nameBox(value) {
  return (
    `<span class="ftab editing">` +
    `<input class="fname" spellcheck="false" autocomplete="off" placeholder="Name.rud" ` +
    `value="${esc(value)}" size="${Math.max(8, value.length + 1)}" />` +
    `</span>`
  );
}

function startRename(index) {
  renaming = { index };
  renderFiles();
}

function cancelRename() {
  renaming = null;
  renderFiles();
  editor.focus();
}

/// Take the name in the box, if it is one the server would accept. A refusal
/// leaves the box exactly as it is, since the reader is one keystroke from a
/// legal name and throwing away what they typed would not help them find it.
function commitRename(box) {
  const path = box.value.trim();
  const { index } = renaming;
  const clash = state.files.some((file, i) => i !== index && file.path === path);
  if (!validPath(path) || clash) {
    box.classList.add("bad");
    state.notice = clash
      ? `this bundle already has a ${path}`
      : `${path || "a file"} is not a path a file can have: ` +
        "letters, digits, `_` and `-`, `/` between folders, ending in `.rud`";
    app.emit("status");
    return;
  }

  state.notice = "";
  renaming = null;
  if (index === -1) {
    state.files.push({ path, source: "" });
    state.active = state.files.length - 1;
    state.offsets = null;
    state.lines = null;
    editor.setValue("", { caret: 0, top: 0, left: 0 });
    renderFiles();
    app.emit("file");
  } else {
    const file = state.files[index];
    state.where[path] = state.where[file.path];
    delete state.where[file.path];
    file.path = path;
    renderFiles();
  }
  editor.focus();
  saveNow();
  compileNow();
}

function deleteFile(index) {
  const file = state.files[index];
  if (!file) return;
  // The last file cannot go: a document with no files is not an empty bundle,
  // it is a deleted document, and deleting one is not something the strip does.
  if (state.files.length === 1) {
    state.notice = "a document keeps at least one file; Ctrl+P opens another one";
    return app.emit("status");
  }
  if (file.source.trim() && !confirm(`delete ${file.path}?`)) return;

  state.files.splice(index, 1);
  delete state.where[file.path];
  state.notice = "";
  const after = state.active > index ? state.active - 1 : state.active;
  state.active = Math.min(after, state.files.length - 1);
  state.offsets = null;
  state.lines = null;
  const shown = active();
  editor.setValue(shown.source, state.where[shown.path] ?? { caret: 0, top: 0, left: 0 });
  renderFiles();
  app.emit("file");
  editor.focus();
  saveNow();
  compileNow();
}

/// Put a file on screen, remembering where the reader was in the one leaving.
function selectFile(index) {
  const file = state.files[index];
  if (!file || index === state.active) return;
  remember();
  state.active = index;
  state.offsets = null;
  state.lines = null;
  editor.setValue(file.source, state.where[file.path] ?? { caret: 0, top: 0, left: 0 });
  renderFiles();
  // Not a new snapshot — the same one, read for a different file. The editor
  // repaints from it and the strip tells the diagnostics which paths to name.
  app.emit("file");
  scheduleSharedView();
}

function remember() {
  const file = active();
  if (file) state.where[file.path] = editor.where();
}

/// Switch to the file a span was written in, when it is not the one on screen.
function showFile(span) {
  const path = app.pathOf(span);
  if (!path || path === active()?.path) return;
  const index = state.files.findIndex((file) => file.path === path);
  if (index >= 0) selectFile(index);
}

/// The shape `docs::valid_file_path` accepts, checked where the name was typed:
/// a relative `/`-separated path of segments drawn from a small alphabet,
/// ending in `.rud`. The server refuses anything else with a 400, and a 400 the
/// reader never sees is a file that silently did not appear.
function validPath(path) {
  if (!path.endsWith(".rud") || path.length > 128) return false;
  const segments = path.split("/");
  return segments.every((segment, i) => {
    const last = i === segments.length - 1;
    const name = last ? segment.slice(0, -".rud".length) : segment;
    return /^[A-Za-z0-9_-]+$/.test(name);
  });
}

// ── keys ─────────────────────────────────────────────────────────────────

function wireKeys() {
  window.addEventListener("keydown", (event) => {
    if (!el("overlay").hidden) return;
    const typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName ?? "");

    // Alt rather than Ctrl for the tabs: the browser owns Ctrl+number.
    if (event.altKey && /^[1-9]$/.test(event.key)) {
      panes.selectTab(Number(event.key) - 1);
      return event.preventDefault();
    }
    if (event.ctrlKey && event.key === "Enter") {
      compileNow();
      return event.preventDefault();
    }
    if (event.ctrlKey && event.key === "s") {
      saveNow();
      return event.preventDefault();
    }
    if (event.ctrlKey && event.key === "p") {
      openSwitcher();
      return event.preventDefault();
    }
    if (event.ctrlKey && event.key === "b") {
      toggleSplit();
      return event.preventDefault();
    }
    if (event.ctrlKey && event.key === ".") {
      toggleFollow();
      return event.preventDefault();
    }
    if (event.ctrlKey && (event.key === "j" || event.key === "J")) {
      diagnostics.step(event.shiftKey ? -1 : 1);
      return event.preventDefault();
    }
    if (event.key === "/" && !typing) {
      panes.focusFilter();
      return event.preventDefault();
    }
    // Stepping is the whole interaction of a stepped panel, so it gets the
    // cheapest keys left. Held with Alt they work with the caret in the editor
    // too — the way the diagnostic stepper's Ctrl+J does — because otherwise
    // the binding is only as reliable as the reader's guess about what has
    // focus, and guessing wrong types a `.` into their program. Shifted, they
    // are the two ends: `<` and `>` sit on the same keys.
    if ((event.key === "," || event.key === "<") && (event.altKey || !typing)) {
      panes.step(event.shiftKey ? "start" : -1);
      return event.preventDefault();
    }
    if ((event.key === "." || event.key === ">") && (event.altKey || !typing)) {
      panes.step(event.shiftKey ? "end" : 1);
      return event.preventDefault();
    }
    if (event.key === "Escape") {
      app.setSelection(null);
      app.setHover(null);
    }
  });
}

// ── layout ───────────────────────────────────────────────────────────────

function wireDrag() {
  const drag = el("drag");
  const pane = el("editor-pane");
  pane.style.width = `${state.width}%`;

  drag.addEventListener("mousedown", (event) => {
    event.preventDefault();
    const move = (moved) => {
      const percent = (moved.clientX / window.innerWidth) * 100;
      state.width = Math.max(15, Math.min(80, percent));
      pane.style.width = `${state.width}%`;
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      saveUi();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
}

// ── persistence of the workspace itself ──────────────────────────────────

function loadUi() {
  try {
    const saved = JSON.parse(localStorage.getItem(UI_KEY) ?? "null");
    if (saved) Object.assign(state, saved, { collapsed: {}, snapshot: null });
  } catch {
    // A corrupt UI blob is not worth a broken page; the defaults are fine.
  }
}

function saveUi() {
  const { doc, follow, split, tabs, views, width } = state;
  localStorage.setItem(UI_KEY, JSON.stringify({ doc, follow, split, tabs, views, width }));
  scheduleSharedView();
}

function sharedView() {
  const where = editor.where();
  return {
    active_file: active()?.path ?? "",
    caret: app.charToByte(where.caret),
    tabs: state.tabs,
    views: state.views,
    split: state.split,
    pane: state.pane,
    panes: panes.view(),
    selection: state.selection,
  };
}

let sharedViewTimer = null;
function scheduleSharedView() {
  clearTimeout(sharedViewTimer);
  sharedViewTimer = setTimeout(async () => {
    if (!state.snapshot) return;
    await fetch("/session/view", {
      method: "PUT",
      body: JSON.stringify({
        session_revision: state.sessionRevision,
        view: sharedView(),
      }),
    }).catch(() => null);
  }, 80);
}

// ── offsets ──────────────────────────────────────────────────────────────

/// Spans are UTF-8 byte offsets and JavaScript strings are UTF-16, so every
/// span has to be converted before it can index the buffer. Pure-ASCII source
/// — nearly always — takes the identity path.
///
/// Of the file on screen: a span from another file of the bundle is never
/// converted, because there is nothing in the buffer for it to point at.
function offsets() {
  if (state.offsets) return state.offsets;
  const text = source();
  let ascii = true;
  for (let i = 0; i < text.length; i++) {
    if (text.charCodeAt(i) > 127) {
      ascii = false;
      break;
    }
  }
  if (ascii) {
    state.offsets = { toChar: (byte) => byte, toByte: (char) => char };
    return state.offsets;
  }

  const toChar = new Int32Array(byteLength(text) + 1);
  const toByte = new Int32Array(text.length + 1);
  let byte = 0;
  for (let i = 0; i < text.length; ) {
    const point = text.codePointAt(i);
    const width = point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    const units = point > 0xffff ? 2 : 1;
    for (let k = 0; k < width; k++) toChar[byte + k] = i;
    for (let k = 0; k < units; k++) toByte[i + k] = byte;
    byte += width;
    i += units;
  }
  toChar[byte] = text.length;
  toByte[text.length] = byte;
  state.offsets = {
    toChar: (b) => toChar[Math.max(0, Math.min(b, toChar.length - 1))],
    toByte: (c) => toByte[Math.max(0, Math.min(c, toByte.length - 1))],
  };
  return state.offsets;
}

function byteLength(text) {
  return new TextEncoder().encode(text).length;
}

/// Byte offset of every line start in the live buffer.
function lines() {
  if (state.lines) return state.lines;
  const starts = [0];
  const text = source();
  const map = offsets();
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\n") starts.push(map.toByte(i + 1));
  }
  state.lines = starts;
  return starts;
}

// ── plumbing ─────────────────────────────────────────────────────────────

async function get(path) {
  const response = await fetch(path);
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return response.json();
}

// ── go ───────────────────────────────────────────────────────────────────

// A document named on the command line beats the one this browser had open.
const startup = await get("/status").catch(() => null);
const shared = await get("/session").catch(() => null);
state.buildError = startup?.build_error ?? null;
state.sessionRevision = shared?.session_revision ?? 0;
await refreshDocs();
await openDoc(startup?.doc ?? shared?.request?.document ?? state.doc);
if (shared && (!startup?.doc || startup.doc === shared.request.document)) {
  await syncSharedSession(shared);
}
