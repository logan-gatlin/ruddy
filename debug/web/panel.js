// Stage panels.
//
// One renderer for every stage, driven entirely by the snapshot: tabs, columns,
// filtering and cross-highlighting all come from `stages[]`. Nothing here knows
// what "tokens" or "ir" are, which is what makes a new stage a backend-only
// change.

const INDENT = 12;

export function createPanes(root, app) {
  let panes = [];

  function build() {
    root.innerHTML = "";
    panes = [];
    const count = app.state.split ? 2 : 1;
    for (let index = 0; index < count; index++) {
      panes.push(createPane(root, app, index));
    }
    render();
  }

  function render() {
    for (const pane of panes) pane.render();
  }

  app.on("snapshot", render);
  app.on("highlight", () => panes.forEach((pane) => pane.mark()));
  app.on("layout", build);
  build();

  return {
    focusFilter: () => panes[app.state.pane]?.focusFilter(),
    selectTab: (n) => panes[app.state.pane]?.selectTab(n),
  };
}

function createPane(root, app, index) {
  const pane = document.createElement("div");
  pane.className = "pane";
  pane.innerHTML = `
    <div class="pane-bar">
      <span class="tabs"></span>
      <span class="grow"></span>
      <span class="views"></span>
      <button class="chip" data-act="collapse" title="Collapse to top level">⊟</button>
      <button class="chip" data-act="expand" title="Expand all">⊞</button>
      <input class="filter" placeholder="filter  /" spellcheck="false" />
    </div>
    <div class="rows"></div>`;
  root.appendChild(pane);

  const tabs = pane.querySelector(".tabs");
  const views = pane.querySelector(".views");
  const filter = pane.querySelector(".filter");
  const rows = pane.querySelector(".rows");

  // Every rendered row, so highlighting and caret-following can work over the
  // rows on screen without walking the tree again.
  let visible = [];
  let stage = null;
  // The elements currently carrying each highlight class, so marking touches
  // only what changed rather than every row.
  let marked = new Set();
  let highlighted = new Set();
  let symbolised = new Set();

  pane.addEventListener("mousedown", () => (app.state.pane = index), true);

  tabs.addEventListener("click", (event) => {
    const id = event.target.closest(".tab")?.dataset.stage;
    if (id) {
      app.state.tabs[index] = id;
      app.save();
      render();
    }
  });

  views.addEventListener("click", (event) => {
    const view = event.target.closest("button")?.dataset.view;
    if (view && stage) {
      app.state.views[stage.id] = view;
      app.save();
      render();
    }
  });

  pane.querySelector('[data-act="collapse"]').addEventListener("click", () => {
    if (!stage) return;
    const collapsed = app.collapsed(stage.id);
    collapsed.clear();
    for (const row of allKeys(stage.nodes, "")) {
      if (row.depth === 0) collapsed.add(row.key);
    }
    render();
  });

  pane.querySelector('[data-act="expand"]').addEventListener("click", () => {
    if (!stage) return;
    app.collapsed(stage.id).clear();
    render();
  });

  filter.addEventListener("input", render);
  filter.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      filter.value = "";
      filter.blur();
      render();
    }
  });

  rows.addEventListener("mouseover", (event) => {
    const row = event.target.closest(".row");
    if (row) app.setHover(mark(row));
  });
  rows.addEventListener("mouseleave", () => app.setHover(null));

  rows.addEventListener("click", (event) => {
    const row = event.target.closest(".row");
    if (!row) return;
    if (event.target.closest(".twisty") && row.dataset.kids === "1") {
      const collapsed = app.collapsed(stage.id);
      const key = row.dataset.key;
      collapsed.has(key) ? collapsed.delete(key) : collapsed.add(key);
      render();
      return;
    }
    app.setSelection(mark(row));
  });

  function mark(row) {
    const start = row.dataset.start;
    return {
      origin: `pane${index}`,
      stage: stage?.id,
      node: Number(row.dataset.node),
      span: start === "" ? null : [Number(start), Number(row.dataset.end)],
      symbol: row.dataset.symbol === "" ? null : Number(row.dataset.symbol),
    };
  }

  function render() {
    // Every render replaces the rows, so the elements the marks were on are
    // gone with it.
    marked = new Set();
    highlighted = new Set();

    const snapshot = app.state.snapshot;
    if (!snapshot) {
      rows.innerHTML = `<div class="pane-note">waiting for the first compile…</div>`;
      return;
    }

    stage = snapshot.stages.find((s) => s.id === app.state.tabs[index]) ?? snapshot.stages[0];
    app.state.tabs[index] = stage.id;
    renderTabs(snapshot);
    renderViews();

    const view = app.state.views[stage.id] ?? "tree";
    if (view !== "tree") {
      const body = view === "display" ? stage.display : stage.debug;
      rows.innerHTML = `<pre class="raw">${esc(body || "(empty)")}</pre>`;
      visible = [];
      return;
    }

    if (stage.status === "panicked") {
      const panic = snapshot.panic;
      rows.innerHTML = `<div class="pane-note bad"><b>${esc(stage.title)} panicked</b> — ${esc(
        panic?.message ?? "no message",
      )}<br />at ${esc(panic?.location ?? "?")}<pre>${esc(panic?.backtrace ?? "")}</pre></div>`;
      visible = [];
      return;
    }
    if (stage.status === "skipped" || !stage.nodes.length) {
      const why = stage.status === "skipped" ? stage.summary : "nothing to show";
      rows.innerHTML = `<div class="pane-note">${esc(why)}</div>`;
      visible = [];
      return;
    }

    const query = filter.value.trim().toLowerCase();
    const collapsed = app.collapsed(stage.id);
    const columns = stage.view === "list" ? columnsOf(stage) : [];
    const scroll = rows.scrollTop;

    visible = [];
    walk(stage.nodes, 0, "", collapsed, query, visible);

    rows.innerHTML = visible.map((row) => rowHtml(row, columns, app)).join("");
    rows.scrollTop = scroll;
    for (const [i, row] of visible.entries()) row.el = rows.children[i];
    markRows();
  }

  function renderTabs(snapshot) {
    tabs.innerHTML = snapshot.stages
      .map((s, i) => {
        const on = s.id === stage.id ? " on" : "";
        const bad = s.status === "panicked" ? " bad" : "";
        return `<button class="tab${on}${bad}" data-stage="${s.id}"><span class="key">${
          i + 1
        }</span>${esc(s.title)}<span class="count">${esc(s.summary)}</span></button>`;
      })
      .join("");
  }

  function renderViews() {
    const current = app.state.views[stage.id] ?? "tree";
    views.innerHTML = ["tree", "display", "raw"]
      .map(
        (view) =>
          `<button data-view="${view}" class="${view === current ? "on" : ""}">${view}</button>`,
      )
      .join("");
  }

  /// Paint selection, symbol and caret correspondence onto the rows already on
  /// screen. Nothing is re-rendered: highlighting happens on every mouse move,
  /// and rebuilding a tree for that would be felt.
  function markRows() {
    const { selection, hover } = app.state;
    const focus = selection ?? hover;
    const symbol = focus?.symbol ?? null;

    // A caret in the editor selects the deepest row covering it; a click in
    // another panel selects the row that came from the same source text.
    let deepest = null;
    if (focus && focus.origin !== `pane${index}` && focus.span) {
      const [from, to] = focus.span;
      for (const row of visible) {
        const span = row.node.span;
        if (!span || span[0] > from || span[1] < to) continue;
        if (!deepest || span[1] - span[0] <= deepest.node.span[1] - deepest.node.span[0]) {
          deepest = row;
        }
      }
    }

    // Only the rows whose state changes are touched. Toggling a class on every
    // row of a large tree costs a style recalculation of the whole pane, and
    // this runs on every caret move and every mouse move.
    const wanted = new Set();
    for (const row of visible) {
      if (!row.el) continue;
      const selected =
        (focus?.origin === `pane${index}` && focus.stage === stage.id && focus.node === row.node.id) ||
        row === deepest;
      if (selected) wanted.add(row.el);
      if (symbol != null && row.node.symbol === symbol) symbolised.add(row.el);
    }

    for (const el of marked) if (!wanted.has(el)) el.classList.remove("sel");
    for (const el of wanted) if (!marked.has(el)) el.classList.add("sel");
    marked = wanted;

    for (const el of highlighted) if (!symbolised.has(el)) el.classList.remove("sym");
    for (const el of symbolised) if (!highlighted.has(el)) el.classList.add("sym");
    highlighted = symbolised;
    symbolised = new Set();

    if (deepest?.el && app.state.follow && focus?.origin === "editor") {
      scrollIntoView(rows, deepest.el);
    }
  }

  return {
    render,
    mark: markRows,
    focusFilter: () => filter.focus(),
    selectTab(n) {
      const snapshot = app.state.snapshot;
      const target = snapshot?.stages[n];
      if (!target) return;
      app.state.tabs[index] = target.id;
      app.save();
      render();
    },
  };
}

/// Flatten the tree into the rows that should be on screen, honouring the
/// collapsed set and the filter. A row survives the filter if it matches or if
/// one of its descendants does, so structure is never hidden by a search.
function walk(nodes, depth, prefix, collapsed, query, out) {
  let matchedAny = false;
  nodes.forEach((node, i) => {
    const key = `${prefix}/${i}`;
    const kids = node.children ?? [];
    const self = query ? matches(node, query) : true;

    const buffer = [];
    const open = kids.length > 0 && (!collapsed.has(key) || (query && !self));
    const below = open ? walk(kids, depth + 1, key, collapsed, query, buffer) : false;

    if (query && !self && !below) return;
    matchedAny = true;
    out.push({ node, depth, key, kids: kids.length, open, dim: !!query && !self });
    out.push(...buffer);
  });
  return matchedAny;
}

function matches(node, query) {
  if (node.label.toLowerCase().includes(query)) return true;
  if (node.text.toLowerCase().includes(query)) return true;
  return (node.fields ?? []).some((field) => field.value.toLowerCase().includes(query));
}

function allKeys(nodes, prefix, depth = 0, out = []) {
  nodes.forEach((node, i) => {
    const key = `${prefix}/${i}`;
    out.push({ key, depth });
    if (node.children?.length) allKeys(node.children, key, depth + 1, out);
  });
  return out;
}

function columnsOf(stage) {
  const seen = [];
  for (const node of stage.nodes) {
    for (const field of node.fields ?? []) {
      if (!field.name.startsWith("_") && !seen.includes(field.name)) seen.push(field.name);
    }
  }
  return seen;
}

function rowHtml(row, columns, app) {
  const { node, depth, kids, open, dim } = row;
  const twisty = kids ? (open ? "▾" : "▸") : "";
  const classes = ["row"];
  if (node.error) classes.push("bad");
  if (node.generated) classes.push("gen");
  if (dim) classes.push("dim");

  const cells = columns
    .map((name) => {
      const field = (node.fields ?? []).find((f) => f.name === name);
      return `<span class="col" style="width:${name === "mangled" ? 260 : 96}px">${esc(
        field?.value ?? "",
      )}</span>`;
    })
    .join("");

  return (
    `<div class="${classes.join(" ")}" data-key="${row.key}" data-node="${node.id}" ` +
    `data-kids="${kids ? 1 : 0}" data-start="${node.span ? node.span[0] : ""}" ` +
    `data-end="${node.span ? node.span[1] : ""}" data-symbol="${node.symbol ?? ""}" ` +
    `style="padding-left:${8 + depth * INDENT}px">` +
    `<span class="twisty${kids ? "" : " leaf"}">${twisty}</span>` +
    `<span class="label">${esc(node.label)}</span>` +
    `<span class="text">${esc(node.text)}</span>` +
    cells +
    `<span class="pos">${node.span ? esc(app.spanLabel(node.span)) : ""}</span>` +
    `</div>`
  );
}

function scrollIntoView(container, element) {
  const top = element.offsetTop;
  const height = element.offsetHeight;
  if (top < container.scrollTop) container.scrollTop = top - height;
  else if (top + height > container.scrollTop + container.clientHeight) {
    container.scrollTop = top - container.clientHeight + height * 2;
  }
}

export function esc(text) {
  return String(text).replace(
    /[&<>"]/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c],
  );
}
