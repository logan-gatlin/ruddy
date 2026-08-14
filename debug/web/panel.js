// Stage panels.
//
// One renderer for every stage, driven entirely by the snapshot: tabs, columns,
// filtering and cross-highlighting all come from `stages[]`. Nothing here knows
// what "tokens" or "ir" are, which is what makes a new stage a backend-only
// change.

const INDENT = 12;
const NOTHING_YET = `<div class="step-item none">nothing yet</div>`;

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
    step: (to) => panes[app.state.pane]?.step(to),
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
  // The `.tok` elements currently lit, for the same reason.
  let lit = new Set();
  // The stepped view as it stands on screen, when one is up: see `buildSteps`.
  // `null` whenever the pane's rows have been replaced by anything else.
  let stepper = null;
  // Where the reader has got to in each stepped stage, by stage id. Per pane,
  // because two panes showing one stage are two readers; kept across compiles,
  // because an edit should not throw away where you were.
  const cursors = new Map();

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
    lightTokens(event.target.closest(".tok")?.dataset.tok ?? null, row);
    if (row) app.setHover(mark(row));
  });
  rows.addEventListener("mouseleave", () => {
    lightTokens(null);
    app.setHover(null);
  });

  rows.addEventListener("click", (event) => {
    const nav = event.target.closest("[data-step]");
    if (nav) return step(nav.dataset.step);

    const row = event.target.closest(".row");
    if (!row) return;
    if (event.target.closest(".twisty") && row.dataset.kids === "1") {
      const collapsed = app.collapsed(stage.id);
      const key = row.dataset.key;
      collapsed.has(key) ? collapsed.delete(key) : collapsed.add(key);
      render();
      return;
    }
    // Read the row before moving the cursor: stepping re-renders, and the
    // element this came from is gone with it.
    const at = row.dataset.at;
    const selection = mark(row);
    if (at !== undefined) step(at, true);
    app.setSelection(selection);
  });

  /// Light every occurrence of one name in the rows on screen — the same
  /// name, spelled the same way, wherever the stage's text uses it. Which runs
  /// count as a name is the stage's `highlight` pattern; this knows only that
  /// two matching strings are the same thing.
  ///
  /// How far "wherever" reaches is the stage's too. A `?4` is one entry of one
  /// program-wide variable table, so it means the same thing in every row; a
  /// scheme's `'a` is numbered from `'a` again for every definition, so it
  /// means something only inside the row it is in. A stage that says it is
  /// `scoped` gets the second reading.
  ///
  /// Rows only, and only this pane's: a collapsed or filtered-out row has
  /// nothing on screen to light, and it lights again the moment it is back.
  function lightTokens(name, row) {
    const wanted = new Set();
    if (name != null) {
      const within = stage?.scoped ? (row ?? rows) : rows;
      for (const el of within.querySelectorAll(".tok")) {
        if (el.dataset.tok === name) wanted.add(el);
      }
    }
    for (const el of lit) if (!wanted.has(el)) el.classList.remove("tok-on");
    for (const el of wanted) if (!lit.has(el)) el.classList.add("tok-on");
    lit = wanted;
  }

  /// Move the cursor of a stepped stage. `to` is a signed delta, one of the
  /// ends, or — with `absolute` — the step to stop in front of.
  ///
  /// The cursor sits *between* steps: everything before it has happened, and
  /// the step at it is the one about to. So it runs to `length`, one past the
  /// last step, which is the state the solve actually ended in.
  function step(to, absolute = false) {
    if (!stage || viewOf(app, stage) !== "steps") return;
    const last = stage.nodes.length;
    const at = cursors.get(stage.id) ?? 0;
    const wanted =
      to === "start" ? 0 : to === "end" ? last : absolute ? Number(to) : at + Number(to);
    cursors.set(stage.id, Math.max(0, Math.min(last, wanted)));
    render();
  }

  function mark(row) {
    const start = row.dataset.start;
    // A step row carries no `data-link`, so an absent one reads the same as an
    // empty one: this row is in no group.
    const link = row.dataset.link ?? "";
    return {
      origin: `pane${index}`,
      stage: stage?.id,
      node: Number(row.dataset.node),
      span: start === "" ? null : [Number(start), Number(row.dataset.end)],
      symbol: row.dataset.symbol === "" ? null : Number(row.dataset.symbol),
      link: link === "" ? null : Number(link),
    };
  }

  /// Replace the pane's rows wholesale. Every mark and every lit token was on
  /// an element that goes with them, and so is anything the stepper was holding
  /// on to, so the sets that tracked those are emptied here rather than
  /// hopefully at each call site.
  function setRows(html) {
    rows.innerHTML = html;
    marked = new Set();
    highlighted = new Set();
    lit = new Set();
    stepper = null;
  }

  function render() {
    const snapshot = app.state.snapshot;
    if (!snapshot) {
      setRows(`<div class="pane-note">waiting for the first compile…</div>`);
      return;
    }

    stage = snapshot.stages.find((s) => s.id === app.state.tabs[index]) ?? snapshot.stages[0];
    app.state.tabs[index] = stage.id;
    renderTabs(snapshot);
    renderViews();

    const view = viewOf(app, stage);
    if (view === "raw") {
      setRows(`<pre class="raw">${esc(stage.debug || "(empty)")}</pre>`);
      visible = [];
      return;
    }

    if (stage.status === "panicked") {
      const panic = snapshot.panic;
      setRows(
        `<div class="pane-note bad"><b>${esc(stage.title)} panicked</b> — ${esc(
          panic?.message ?? "no message",
        )}<br />at ${esc(panic?.location ?? "?")}<pre>${esc(panic?.backtrace ?? "")}</pre></div>`,
      );
      visible = [];
      return;
    }
    if (stage.status === "skipped" || !stage.nodes.length) {
      const why = stage.status === "skipped" ? stage.summary : "nothing to show";
      setRows(`<div class="pane-note">${esc(why)}</div>`);
      visible = [];
      return;
    }

    // A stepped stage is a timeline rather than a shape, so it owns the pane
    // instead of going through the tree walk: none of the collapsing,
    // filtering or caret-following below means anything to it.
    if (view === "steps") {
      renderSteps();
      visible = [];
      return;
    }

    const query = filter.value.trim().toLowerCase();
    const collapsed = app.collapsed(stage.id);
    const columns = stage.view === "list" ? columnsOf(stage) : [];
    const scroll = rows.scrollTop;
    const badges = badgesFor(snapshot, stage.id);

    visible = [];
    walk(stage.nodes, 0, "", collapsed, query, visible);

    const pattern = names(stage);
    setRows(visible.map((row) => rowHtml(row, columns, app, badges, pattern)).join(""));
    rows.scrollTop = scroll;
    for (const [i, row] of visible.entries()) row.el = rows.children[i];
    markRows();
  }

  /// Show the stepped stage: the timeline, a cursor into it, and the state
  /// everything before the cursor built up.
  ///
  /// A new snapshot is a new timeline and is laid out from scratch. Everything
  /// else that reaches here — a cursor move, a tab returned to — is the same
  /// timeline, and only the cursor has moved, so `moveCursor` repaints what the
  /// move crossed. Rebuilding the list and replaying its prefix for a one-step
  /// delta made each press of `.` cost the whole solve, twice over.
  function renderSteps() {
    const steps = stage.nodes;
    const at = Math.min(cursors.get(stage.id) ?? 0, steps.length);
    cursors.set(stage.id, at);

    // The stepper is only reusable if what it was built from is still what is
    // being shown *and* is still on screen: any other view replaces the rows
    // out from under it, which `setRows` is what records.
    const showing = stepper?.steps === steps && stepper.root.parentNode === rows;
    showing ? moveCursor(at) : buildSteps(steps, at);
  }

  /// Lay out the whole timeline and take hold of the parts a cursor move has to
  /// reach: the rows, the two state panels, and the two lines that say where
  /// the cursor is.
  ///
  /// The state panels are accumulated from the steps rather than sent per step:
  /// a step declares what it *added* to each — a `_bind`, an `_error` — and the
  /// prefix up to the cursor is the state. That keeps the wire linear in the
  /// number of steps instead of quadratic, and keeps the page ignorant of what
  /// a binding or an error actually is.
  function buildSteps(steps, at) {
    const pattern = names(stage);

    const solution = [];
    const errors = [];
    for (const done of steps.slice(0, at)) {
      const bind = field(done, "_bind");
      const failed = field(done, "_error");
      if (bind !== undefined) solution.push(bind);
      if (failed !== undefined) errors.push(failed);
    }

    // Solving runs one definition at a time, and a step says which one it was
    // solving. A rule wherever that changes is where one solve ends and the
    // next begins — which the numbers alone never showed, since the timeline
    // is continuous but the solves are not.
    let solving = null;
    const list = steps
      .map((one, i) => {
        const where = i < at ? "done" : i === at ? "at" : "ahead";
        const depth = Number(field(one, "_depth") ?? 0);
        const definition = field(one, "_def") ?? "";
        // Not on the first row: the top of the list is already an edge.
        const starts = i > 0 && definition !== solving ? " starts" : "";
        solving = definition;
        return (
          `<div class="row step-row ${where}${starts}${one.error ? " bad" : ""}" data-at="${i}" ` +
          `data-node="${one.id}" data-kids="0" data-key="" ` +
          `data-start="${one.span ? one.span[0] : ""}" data-end="${one.span ? one.span[1] : ""}" ` +
          `data-symbol="${symbolOf(one) ?? ""}">` +
          // The rule name is a fixed column and does not indent: eleven names
          // read as a column only if they all start in the same place. What
          // nests is the goal, so that is what carries the depth.
          `<span class="label">${esc(one.label)}</span>` +
          `<span class="text" style="margin-left:${depth * INDENT}px">` +
          `${escNames(one.text, pattern)}</span>` +
          `<span class="step-effect">${escNames(field(one, "_effect") ?? "", pattern)}</span>` +
          `<span class="step-def">${esc(definition)}</span>` +
          `</div>`
        );
      })
      .join("");

    const panel = (title, items, bad) =>
      `<div class="step-panel"><div class="step-head">${title}</div>` +
      `<div class="step-items">${itemsHtml(items, bad, pattern)}</div></div>`;

    setRows(
      `<div class="stepper">` +
        `<div class="step-bar">` +
        `<button class="chip" data-step="start" title="Back to the start">⏮</button>` +
        `<button class="chip" data-step="-1" title="Back one step">◀</button>` +
        `<button class="chip" data-step="1" title="Forward one step">▶</button>` +
        `<button class="chip" data-step="end" title="Run to the end">⏭</button>` +
        `<span class="step-count"></span>` +
        `</div>` +
        `<div class="step-next"></div>` +
        `<div class="step-body"><div class="step-list">${list}</div>` +
        `<div class="step-state">${panel("solution", solution, "")}${panel("errors", errors, " bad")}</div>` +
        `</div></div>`,
    );

    const items = rows.querySelectorAll(".step-items");
    stepper = {
      steps,
      at,
      pattern,
      root: rows.firstElementChild,
      // An array rather than the live NodeList: the rows never change in
      // number, and indexing one by step is the whole access pattern.
      stepRows: [...rows.querySelectorAll(".step-row")],
      list: rows.querySelector(".step-list"),
      count: rows.querySelector(".step-count"),
      next: rows.querySelector(".step-next"),
      solution: { el: items[0], count: solution.length, bad: "" },
      errors: { el: items[1], count: errors.length, bad: " bad" },
    };
    renderCursor();
  }

  /// Move the cursor over a timeline already on screen. Everything below the
  /// cursor has happened and everything from it on has not, so a move repaints
  /// exactly the rows it crossed and adds or drops what those rows declared —
  /// work proportional to the move rather than to the solve.
  function moveCursor(to) {
    const { steps, stepRows } = stepper;
    const from = stepper.at;
    if (to !== from) {
      for (let i = Math.min(from, to); i <= Math.max(from, to); i++) {
        const el = stepRows[i];
        if (!el) continue;
        el.classList.remove("done", "at", "ahead");
        el.classList.add(i < to ? "done" : i === to ? "at" : "ahead");
      }
      if (to > from) for (let i = from; i < to; i++) addState(steps[i]);
      else for (let i = to; i < from; i++) dropState(steps[i]);
      stepper.at = to;
    }
    renderCursor();
  }

  /// The two lines that say where the cursor is — the count in the bar and the
  /// rule about to fire — and the scroll that keeps it in sight. One line each,
  /// so they are rewritten rather than patched.
  function renderCursor() {
    const { steps, at, pattern, stepRows } = stepper;
    stepper.count.textContent = `${at} of ${steps.length} applied`;

    const next = steps[at];
    stepper.next.innerHTML = next
      ? `<span class="step-head">next</span>` +
        `<span class="label">${esc(next.label)}</span>` +
        `<span class="text">${escNames(next.text, pattern)}</span>` +
        `<div class="step-why">${esc(field(next, "_rule") ?? "")}</div>`
      : `<span class="step-head">next</span>` +
        `<span class="step-why">nothing left; this is what the solve concluded</span>`;

    if (stepRows[at]) scrollIntoView(stepper.list, stepRows[at]);
  }

  /// What one step added to each state panel, appended. Absent and empty are
  /// different: a step carrying no `_bind` changed no binding, which is not the
  /// same as binding nothing.
  function addState(step) {
    const bind = field(step, "_bind");
    const failed = field(step, "_error");
    if (bind !== undefined) pushItem(stepper.solution, bind);
    if (failed !== undefined) pushItem(stepper.errors, failed);
  }

  /// The same, undone. Stepping backwards is dropping the tail of each list,
  /// which is only sound because a step adds to the state and never rewrites
  /// it — the compiler side is tested for exactly that.
  function dropState(step) {
    if (field(step, "_bind") !== undefined) popItem(stepper.solution);
    if (field(step, "_error") !== undefined) popItem(stepper.errors);
  }

  function pushItem(panel, text) {
    // The placeholder is not an item, so the first real one replaces it.
    if (panel.count === 0) panel.el.innerHTML = "";
    panel.el.insertAdjacentHTML(
      "beforeend",
      `<div class="step-item${panel.bad}">${escNames(text, stepper.pattern)}</div>`,
    );
    panel.count += 1;
  }

  function popItem(panel) {
    panel.el.lastElementChild?.remove();
    panel.count -= 1;
    if (panel.count === 0) panel.el.innerHTML = NOTHING_YET;
  }

  /// The tab strip.
  ///
  /// A tab is a selector and nothing more: its shortcut and its name. What a
  /// stage counted hangs off its tab as the tooltip rather than printing in
  /// the bar, where a long count crowded the tabs themselves out.
  ///
  /// A stage that annotates another owns no tab; its content shows up as
  /// badges on the stage it decorates.
  ///
  /// One of the counts can be a warning — `3 demangle mismatch`, a check the
  /// stage ran and failed. That has to be findable without visiting every
  /// tab, so a stage reporting anything short of `ok` marks its tab.
  function renderTabs(snapshot) {
    tabs.innerHTML = snapshot.stages
      .filter((s) => !s.annotates)
      .map((s, i) => {
        const on = s.id === stage.id ? " on" : "";
        const bad = s.status === "panicked" ? " bad" : s.status === "partial" ? " warn" : "";
        return (
          `<button class="tab${on}${bad}" data-stage="${s.id}" title="${esc(s.summary)}">` +
          `<span class="key">${i + 1}</span>${esc(s.title)}</button>`
        );
      })
      .join("");
  }

  function renderViews() {
    const current = viewOf(app, stage);
    // A stage's own view, or the raw dump every stage has. Naming the first
    // button after `stage.view` is what lets a list stage stop offering a
    // "tree" it never rendered.
    views.innerHTML = [stage.view, "raw"]
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
    // A row that names something no symbol table holds — a `where let` variable
    // and its uses — carries a group id instead, and lights its group the same
    // way a symbol lights its own rows.
    const link = focus?.link ?? null;

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
      // A row is the symbol's whether it names it or is merely about it: what
      // `owner` costs a node is the editor painting its span, not its place in
      // the group.
      if (symbol != null && symbolOf(row.node) === symbol) symbolised.add(row.el);
      if (link != null && row.node.link === link) symbolised.add(row.el);
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
    step,
    focusFilter: () => filter.focus(),
    selectTab(n) {
      const snapshot = app.state.snapshot;
      // Numbered the way the tab strip is: annotating stages have no tab.
      const target = snapshot?.stages.filter((s) => !s.annotates)[n];
      if (!target) return;
      app.state.tabs[index] = target.id;
      app.save();
      render();
    },
  };
}

/// A state panel's items, or the note that stands in for none of them.
function itemsHtml(items, bad, pattern) {
  if (!items.length) return NOTHING_YET;
  return items
    .map((item) => `<div class="step-item${bad}">${escNames(item, pattern)}</div>`)
    .join("");
}

/// The badges every annotating stage paints, grouped by the stage they land on.
///
/// An annotator's nodes carry the *annotated* stage's ids, so reading them is a
/// join across two stages' node lists — work proportional to the whole
/// snapshot. `render` runs on every keystroke in the filter box and on every
/// tab click, and the answer only changes when the snapshot does, so it is
/// worked out once per snapshot and kept.
let badgeCache = { snapshot: null, byStage: new Map() };

function badgesFor(snapshot, stageId) {
  if (badgeCache.snapshot !== snapshot) {
    const byStage = new Map();
    for (const stage of snapshot.stages) {
      if (!stage.annotates) continue;
      let onto = byStage.get(stage.annotates);
      if (!onto) byStage.set(stage.annotates, (onto = new Map()));
      for (const node of stage.nodes) onto.set(node.id, node.text);
    }
    badgeCache = { snapshot, byStage };
  }
  return badgeCache.byStage.get(stageId);
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

/// Which of a stage's two views is showing. Only `raw` is a deviation — every
/// stage has one — so anything else means the view the stage was registered
/// with, and a name left in storage by an older build falls back to it rather
/// than to a blank pane.
function viewOf(app, stage) {
  return app.state.views[stage.id] === "raw" ? "raw" : stage.view;
}

/// Which symbol a row belongs to, whether it names it (`symbol`) or is only
/// about it (`owner`). Selecting the row hands this to everyone, which is what
/// makes clicking a solve step light the definition being solved — the editor
/// then paints the spans of that symbol's real occurrences, which this row's
/// own span is not one of.
function symbolOf(node) {
  return node.symbol ?? node.owner ?? null;
}

/// One of a node's fields by name, or `undefined`. Absent and empty are
/// different: a step with no `_bind` changed no binding, which is not the same
/// as binding nothing.
function field(node, name) {
  return (node.fields ?? []).find((f) => f.name === name)?.value;
}

/// The stage's `highlight` pattern, compiled. A stage declares the notation it
/// uses; a pattern this build cannot compile is treated as no pattern rather
/// than as a broken panel, since the rest of the stage is still worth reading.
function names(stage) {
  if (!stage.highlight) return null;
  try {
    return new RegExp(stage.highlight, "g");
  } catch {
    return null;
  }
}

/// Escape `text`, wrapping every run the pattern matches so it can be lit
/// alongside its other occurrences. The name goes in the dataset verbatim:
/// matching is string equality, and nothing here parses what it matched.
function escNames(text, pattern) {
  if (!pattern) return esc(text);
  const source = String(text);
  let out = "";
  let last = 0;
  for (const match of source.matchAll(pattern)) {
    const name = esc(match[0]);
    out += esc(source.slice(last, match.index));
    out += `<span class="tok" data-tok="${name}">${name}</span>`;
    last = match.index + match[0].length;
  }
  return out + esc(source.slice(last));
}

function rowHtml(row, columns, app, badges, pattern) {
  const { node, depth, kids, open, dim } = row;
  const twisty = kids ? (open ? "▾" : "▸") : "";
  const badge = badges?.get(node.id);
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
    `data-end="${node.span ? node.span[1] : ""}" data-symbol="${symbolOf(node) ?? ""}" ` +
    `data-link="${node.link ?? ""}" ` +
    `style="padding-left:${8 + depth * INDENT}px">` +
    `<span class="twisty${kids ? "" : " leaf"}">${twisty}</span>` +
    `<span class="label">${esc(node.label)}</span>` +
    `<span class="text">${escNames(node.text, pattern)}</span>` +
    cells +
    (badge ? `<span class="badge">${esc(badge)}</span>` : "") +
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
