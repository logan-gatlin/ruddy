// State, the compile loop, persistence, and everything that has to know about
// more than one region of the page.
//
// The loop is: keystroke → 120 ms → POST /compile → render. Editing the Rust
// compiler instead lands here as a `hello` on the event stream carrying a new
// build id, which re-runs the same snippet and re-renders in place.

import { createEditor } from "./editor.js";
import { createPanes } from "./panel.js";
import { createDiagnostics } from "./diagnostics.js";

const COMPILE_AFTER = 120;
const SAVE_AFTER = 1000;
const UI_KEY = "ruddy-debug/ui";
const docKey = (name) => `ruddy-debug/doc/${name}`;

const state = {
  doc: "demo",
  docs: [],
  source: "",
  revision: 0,
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
  bundle: { name: "demo", version: "0.1.0" },
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
    if (mark?.span && mark.origin !== "editor") editor.reveal(mark.span);
    app.emit("highlight");
  },

  /// The stage with this id in the current snapshot, if it has one.
  stage(id) {
    return state.snapshot?.stages.find((stage) => stage.id === id);
  },

  /// Every source range that mentions one symbol, across every stage. This is
  /// what turns a click on an identifier into "show me this binding and all of
  /// its uses" everywhere at once.
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
  /// diagnostics describe. Uses the line table the server sent, so a label can
  /// never disagree with the tree it labels.
  spanLabel(span) {
    if (!span) return "";
    const starts = state.snapshot?.line_starts ?? lines();
    const at = (byte) => {
      let low = 0;
      let high = starts.length - 1;
      while (low < high) {
        const mid = (low + high + 1) >> 1;
        starts[mid] <= byte ? (low = mid) : (high = mid - 1);
      }
      return `${low + 1}:${byte - starts[low] + 1}`;
    };
    const from = at(span[0]);
    const to = at(span[1]);
    return from === to ? from : `${from}-${to}`;
  },

  save: saveUi,
};

// ── boot ─────────────────────────────────────────────────────────────────

loadUi();
const editor = createEditor(el("editor-pane"), app);
const panes = createPanes(el("panes"), app);
const diagnostics = createDiagnostics(el("diagnostics"), app);

wireTitlebar();
wireKeys();
wireDrag();
connect();

app.on("input", (text) => {
  state.source = text;
  state.offsets = null;
  state.lines = null;
  cacheLocally();
  scheduleCompile();
  scheduleSave();
});

app.on("caret", (byte) => {
  if (!state.follow) return;
  app.setSelection({ origin: "editor", span: [byte, byte], symbol: null, stage: null, node: null });
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
  let source = server?.source ?? cached?.source ?? "";
  state.notice = "";
  if (cached && cached.source !== source && (!server || cached.at > server.modified_ms)) {
    source = cached.source;
    state.notice = `restored unsaved changes to ${name}`;
  }

  state.doc = name;
  state.source = source;
  state.offsets = null;
  state.lines = null;
  state.selection = null;
  editor.setValue(source);
  saveUi();
  renderTitlebar();
  await compileNow();
  editor.focus();
}

let cacheTimer = null;

/// Debounced off the keystroke: `localStorage` is synchronous, and there is no
/// reason for a write to sit between a character and the screen.
function cacheLocally() {
  clearTimeout(cacheTimer);
  cacheTimer = setTimeout(() => {
    try {
      const at = Date.now();
      localStorage.setItem(docKey(state.doc), JSON.stringify({ source: state.source, at }));
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
      body: JSON.stringify({ source: state.source }),
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
      body: JSON.stringify({ source: state.source, revision, bundle: state.bundle }),
    });
    const snapshot = await response.json();
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

      for (const { event } of update.events) {
        if (event === "rebuilding") rebuilding = true;
        if (event === "reload-web") location.reload();
      }
      setLink(rebuilding ? "busy" : "live");

      if (first || update.build !== state.build) {
        await refreshStatus();
        rebuilding = false;
        setLink("live");
        if (!first && state.build !== null) await compileNow();
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

function setLink(link) {
  if (state.link === link) return;
  state.link = link;
  renderTitlebar();
}

// ── title bar ────────────────────────────────────────────────────────────

function wireTitlebar() {
  el("doc-button").addEventListener("click", openSwitcher);
  el("follow").addEventListener("click", () => toggleFollow());
  el("split").addEventListener("click", () => toggleSplit());

  for (const [id, field] of [
    ["bundle-name", "name"],
    ["bundle-version", "version"],
  ]) {
    el(id).addEventListener("change", () => {
      state.bundle[field] = el(id).value.trim();
      saveUi();
      compileNow();
    });
  }
}

function renderTitlebar() {
  el("doc-name").textContent = state.doc;
  el("bundle-name").value = state.bundle.name;
  el("bundle-version").value = state.bundle.version;
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
    // An annotating stage does its work inside another compile phase; a
    // second chip for it would double-count the same time.
    .filter((stage) => !stage.annotates)
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
      state.source = "";
      openDoc(doc.name).then(saveNow);
    } else {
      openDoc(doc.name);
    }
  }
});

el("overlay").addEventListener("mousedown", (event) => {
  if (event.target === el("overlay")) closeSwitcher();
});

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
  const { doc, follow, split, tabs, views, bundle, width } = state;
  localStorage.setItem(UI_KEY, JSON.stringify({ doc, follow, split, tabs, views, bundle, width }));
}

// ── offsets ──────────────────────────────────────────────────────────────

/// Spans are UTF-8 byte offsets and JavaScript strings are UTF-16, so every
/// span has to be converted before it can index the buffer. Pure-ASCII source
/// — nearly always — takes the identity path.
function offsets() {
  if (state.offsets) return state.offsets;
  const text = state.source;
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
  const text = state.source;
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
state.buildError = startup?.build_error ?? null;
await refreshDocs();
await openDoc(startup?.doc ?? state.doc);
