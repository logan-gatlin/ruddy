// The editor: a textarea with a highlight layer painted behind it.
//
// The colouring comes from the compiler's own token stream rather than from a
// regex, so a keyword the lexer failed to recognise is visibly wrong before any
// panel is opened. Anything no token covers is painted as "unlexed", which
// makes a gap in the lexer show up as red text.
//
// Two things follow from the text you see being the highlight layer rather than
// the textarea, whose glyphs are transparent:
//
//   1. The layer has to be repainted in the same turn as the keystroke that
//      changed it. Waiting for the compiler would put every character on screen
//      a compile behind the caret. So between a keystroke and the snapshot that
//      catches up with it, the ranges from the last snapshot are shifted by the
//      edit — exact everywhere except the few characters being typed.
//   2. The layer is one element per line, and an edit rewrites only the lines
//      it touched. Rebuilding all of them costs the length of the file on every
//      keystroke, which is fine for a demo and not fine for a real one.

import { esc } from "./panel.js";

export function createEditor(root, app) {
  root.innerHTML = `
    <div class="editor">
      <div class="gutter"><div class="gutter-inner"></div></div>
      <div class="code">
        <pre class="hl"></pre>
        <textarea spellcheck="false" wrap="off" autocomplete="off" autocapitalize="off"></textarea>
      </div>
    </div>`;

  const gutter = root.querySelector(".gutter-inner");
  const hl = root.querySelector(".hl");
  const area = root.querySelector("textarea");

  /// The text the layer currently shows, for diffing the next edit against.
  let text = "";
  /// Character offset of each line start.
  let starts = [0];
  /// Token and diagnostic ranges in character offsets, sorted by start. Rebuilt
  /// from a snapshot, shifted by every edit in between.
  let base = [];
  /// Whether the buffer has moved on from the snapshot `base` came from.
  let dirty = false;
  /// The rendered markup of each line, so a repaint can skip the lines that did
  /// not change.
  let cache = [];
  /// The gutter row the caret is on, so moving it is two class changes rather
  /// than a rebuild.
  let here = null;
  /// Source ranges of the highlighted symbol, memoised: finding them walks
  /// every node of every stage, and highlighting happens on every mouse move.
  let symbolSpans = { key: null, spans: [] };

  area.addEventListener("input", onInput);
  area.addEventListener("scroll", sync);
  area.addEventListener("keydown", onKeydown);
  for (const event of ["click", "keyup", "focus"]) {
    area.addEventListener(event, reportCaret);
  }

  app.on("snapshot", rebuild);
  // Another file of the same bundle: the snapshot still holds, and what it
  // says about this file is what the layer has to show now.
  app.on("file", rebuild);
  app.on("highlight", onHighlight);

  function sync() {
    hl.style.transform = `translate(${-area.scrollLeft}px, ${-area.scrollTop}px)`;
    gutter.style.transform = `translateY(${-area.scrollTop}px)`;
  }

  // ── edits ──────────────────────────────────────────────────────────────

  function onInput() {
    // Repaint before anything else: this is the keystroke appearing on screen,
    // and it must not queue behind saving, scheduling or a network request.
    apply(area.value);
    app.emit("input", area.value);
  }

  function onKeydown(event) {
    // Two spaces, since the language has no tabs and a literal tab in the
    // buffer would put every span offset out of step with the display.
    if (event.key === "Tab" && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      const { selectionStart: from, selectionEnd: to } = area;
      area.setRangeText("  ", from, to, "end");
      onInput();
    }
  }

  /// Move the layer to `next`, rewriting only the lines the edit covered.
  function apply(next) {
    const edit = diff(text, next);
    base = shift(base, edit);
    dirty = true;

    const firstLine = lineOf(edit.start);
    const lastOld = lineOf(edit.start + edit.removed);
    text = next;
    starts = lineStarts(text);
    const lastNew = lineOf(edit.start + edit.inserted);

    splice(firstLine, lastOld, lastNew);
    moveHere();
  }

  /// Replace the rendered lines `[first, lastOld]` with `[first, lastNew]`.
  function splice(first, lastOld, lastNew) {
    const fragment = document.createDocumentFragment();
    const fresh = [];
    for (let line = first; line <= lastNew; line++) {
      const html = render(line);
      fresh.push(html);
      const element = document.createElement("div");
      element.className = "ln";
      element.innerHTML = html;
      fragment.appendChild(element);
    }

    for (let n = lastOld - first + 1; n > 0; n--) hl.children[first]?.remove();
    hl.insertBefore(fragment, hl.children[first] ?? null);
    cache.splice(first, lastOld - first + 1, ...fresh);

    // Gutter rows carry no text — CSS numbers them — so only the count has to
    // follow, and a renumbering costs nothing.
    resizeGutter();
  }

  function resizeGutter() {
    while (gutter.children.length > starts.length) gutter.lastElementChild.remove();
    while (gutter.children.length < starts.length) {
      gutter.appendChild(document.createElement("div"));
    }
  }

  // ── painting ───────────────────────────────────────────────────────────

  /// Take the ranges from a fresh snapshot. Only here does the layer learn what
  /// the compiler actually thinks; everything in between is extrapolation.
  function rebuild() {
    const ranges = [];
    // The tokens stage groups its rows by file — one row per file, that file's
    // tokens under it — so the stream this buffer holds is one level down, and
    // the streams of the bundle's other files are not this buffer's to paint.
    for (const file of app.stage("tokens")?.nodes ?? []) {
      for (const node of file.children ?? []) {
        if (app.here(node.span)) ranges.push(range(node.span, className(node)));
      }
    }
    for (const diagnostic of app.state.snapshot?.diagnostics ?? []) {
      if (app.here(diagnostic.span)) ranges.push(range(diagnostic.span, "diag"));
    }
    ranges.sort((a, b) => a.from - b.from);
    base = ranges;
    dirty = false;
    symbolSpans = { key: null, spans: [] };
    paint();
    markBadLines();
  }

  function onHighlight() {
    // While the buffer is ahead of the compiler the overlays are spans of a
    // text that no longer exists, so there is nothing to repaint — only the
    // caret's own line has moved.
    if (dirty) return moveHere();
    paint();
  }

  /// Repaint every line, writing only the ones whose markup actually changed.
  function paint() {
    if (area.value !== text) {
      text = area.value;
      starts = lineStarts(text);
    }
    const overlays = overlay();
    if (hl.children.length !== starts.length) rebuildLines(overlays);
    else {
      for (let line = 0; line < starts.length; line++) {
        const html = render(line, overlays);
        if (html === cache[line]) continue;
        cache[line] = html;
        hl.children[line].innerHTML = html;
      }
    }
    moveHere();
    sync();
  }

  function rebuildLines(overlays) {
    cache = [];
    let html = "";
    for (let line = 0; line < starts.length; line++) {
      cache.push(render(line, overlays));
      html += `<div class="ln">${cache[line]}</div>`;
    }
    hl.innerHTML = html;
    resizeGutter();
  }

  /// One line's markup: the ranges covering it, cut into non-overlapping spans.
  function render(line, overlays = overlay()) {
    const from = starts[line];
    const to = line + 1 < starts.length ? starts[line + 1] - 1 : text.length;
    const body = text.slice(from, to);

    const covering = [];
    for (const source of [base, overlays]) {
      for (let i = firstOverlapping(source, from); i < source.length; i++) {
        if (source[i].from >= to) break;
        if (source[i].to <= from) continue;
        covering.push({
          from: source[i].from - from,
          to: source[i].to - from,
          cls: source[i].cls,
        });
      }
    }

    return segments(body, covering)
      .map(({ from: start, to: end, cls }) => {
        const chunk = esc(body.slice(start, end));
        return cls.length ? `<span class="${cls.join(" ")}">${chunk}</span>` : chunk;
      })
      .join("");
  }

  /// Selection, hover and symbol highlighting — recomputed per paint because
  /// they are few, unlike the token ranges they sit on top of.
  function overlay() {
    if (dirty) return [];
    const ranges = [];
    const { selection, hover } = app.state;

    for (const mark of [selection, hover]) {
      if (!mark) continue;
      const cls = mark === selection ? "sel" : "hov";
      if (app.here(mark.span)) ranges.push(range(mark.span, cls));
      for (const related of mark.related ?? []) {
        if (app.here(related.span)) ranges.push(range(related.span, "rel"));
      }
    }

    // A symbol's occurrences are spread across the bundle; the ones in the file
    // on screen are the ones there is anything to paint for.
    const symbol = selection?.symbol ?? hover?.symbol;
    if (symbol != null) {
      if (symbolSpans.key !== symbol) {
        symbolSpans = { key: symbol, spans: app.spansOfSymbol(symbol) };
      }
      for (const span of symbolSpans.spans) {
        if (app.here(span)) ranges.push(range(span, "sym"));
      }
    }
    return ranges.sort((a, b) => a.from - b.from);
  }

  function markBadLines() {
    const bad = new Set();
    for (const diagnostic of app.state.snapshot?.diagnostics ?? []) {
      if (app.here(diagnostic.span)) bad.add(app.lineCol(diagnostic.span.range[0]).line);
    }
    for (let line = 0; line < gutter.children.length; line++) {
      gutter.children[line].classList.toggle("bad", bad.has(line + 1));
    }
  }

  function moveHere() {
    const line = app.lineCol(app.charToByte(area.selectionStart)).line;
    const element = gutter.children[line - 1] ?? null;
    if (element === here) return;
    here?.classList.remove("here");
    element?.classList.add("here");
    here = element;
  }

  // ── plumbing ───────────────────────────────────────────────────────────

  function reportCaret() {
    app.emit("caret", app.charToByte(area.selectionStart));
  }

  /// A span of the file on screen, in the buffer's own units. Which file it is
  /// in is the caller's to have checked — see `app.here`.
  function range(span, cls) {
    return { from: app.byteToChar(span.range[0]), to: app.byteToChar(span.range[1]), cls };
  }

  function lineOf(pos) {
    let low = 0;
    let high = starts.length - 1;
    while (low < high) {
      const mid = (low + high + 1) >> 1;
      starts[mid] <= pos ? (low = mid) : (high = mid - 1);
    }
    return low;
  }

  /// Scroll `span` into view without touching focus or the caret — a panel
  /// click should show you the source, not take the cursor away from it.
  ///
  /// A span of a file that is not on screen is nothing to scroll to: the caller
  /// switches files first, and one that could not is asking about a file this
  /// bundle no longer has.
  function reveal(span) {
    if (!app.here(span)) return;
    const line = app.lineCol(span.range[0]).line - 1;
    const height = lineHeight();
    const top = line * height;
    const view = area.clientHeight;
    if (top < area.scrollTop + height || top > area.scrollTop + view - height * 2) {
      area.scrollTop = Math.max(0, top - view / 3);
      sync();
    }
  }

  function lineHeight() {
    const parsed = parseFloat(getComputedStyle(hl).lineHeight);
    return Number.isFinite(parsed) ? parsed : 18;
  }

  return {
    focus: () => area.focus(),
    value: () => area.value,
    /// Put a file in the buffer, at `where` the reader last left it. The two
    /// are one call because they are one event: a file arriving on screen
    /// arrives somewhere, and restoring the caret after a repaint that reset it
    /// is a frame of the wrong position.
    setValue(next, where = null) {
      if (area.value !== next) {
        area.value = next;
        // A different file: nothing known about the old one applies.
        text = next;
        starts = lineStarts(next);
        base = [];
        dirty = true;
        rebuildLines([]);
      }
      if (where) {
        const caret = Math.min(where.caret, next.length);
        area.setSelectionRange(caret, caret);
        area.scrollTop = where.top;
        area.scrollLeft = where.left;
        sync();
      }
      moveHere();
    },
    /// Where the reader is in the file on screen, for putting them back when
    /// they return to it.
    where: () => ({
      caret: area.selectionStart,
      top: area.scrollTop,
      left: area.scrollLeft,
    }),
    reveal,
  };
}

function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\n") starts.push(i + 1);
  }
  return starts;
}

/// The one edit between two versions of the buffer, as a common prefix and
/// suffix. Every edit a textarea can make in one event — typing, deleting,
/// pasting, replacing a selection — is one contiguous replacement.
function diff(before, after) {
  let start = 0;
  const shortest = Math.min(before.length, after.length);
  while (start < shortest && before[start] === after[start]) start++;

  let endBefore = before.length;
  let endAfter = after.length;
  while (endBefore > start && endAfter > start && before[endBefore - 1] === after[endAfter - 1]) {
    endBefore--;
    endAfter--;
  }
  return { start, removed: endBefore - start, inserted: endAfter - start };
}

/// Move ranges across an edit. Anything before it is untouched, anything after
/// it slides, and anything the edit lands inside is dropped — that text is what
/// the compiler is about to be asked about.
function shift(ranges, { start, removed, inserted }) {
  if (removed === 0 && inserted === 0) return ranges;
  const end = start + removed;
  const delta = inserted - removed;
  const out = [];
  for (const range of ranges) {
    if (range.to <= start) out.push(range);
    else if (range.from >= end) {
      out.push({ from: range.from + delta, to: range.to + delta, cls: range.cls });
    }
  }
  return out;
}

/// Index of the first range that could cover `pos`, in a list sorted by start.
/// Walks back over ranges that begin earlier and end later, which multi-line
/// diagnostics and selections do.
function firstOverlapping(ranges, pos) {
  let low = 0;
  let high = ranges.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    ranges[mid].from < pos ? (low = mid + 1) : (high = mid);
  }
  while (low > 0 && ranges[low - 1].to > pos) low--;
  return low;
}

/// Split a line at every range boundary, so each piece carries the full set of
/// classes covering it. Overlaps — a selected identifier that also has a
/// squiggle — come out as one span with both classes rather than as nested
/// elements that would need the ranges to be well nested.
function segments(text, ranges) {
  if (!ranges.length) return [{ from: 0, to: text.length, cls: [] }];

  const clamp = (n) => Math.max(0, Math.min(text.length, n));
  const sorted = ranges
    .map((r) => ({ from: clamp(r.from), to: clamp(r.to), cls: r.cls }))
    .filter((r) => r.to > r.from)
    .sort((a, b) => a.from - b.from);

  const bounds = new Set([0, text.length]);
  for (const { from, to } of sorted) {
    bounds.add(from);
    bounds.add(to);
  }
  const cuts = [...bounds].sort((a, b) => a - b);

  // Swept rather than rescanned: a quadratic pass would be felt on a long line.
  const out = [];
  let next = 0;
  let active = [];
  for (let i = 0; i < cuts.length - 1; i++) {
    const from = cuts[i];
    const to = cuts[i + 1];
    while (next < sorted.length && sorted[next].from <= from) active.push(sorted[next++]);
    active = active.filter((r) => r.to > from);
    out.push({ from, to, cls: active.map((r) => r.cls) });
  }
  return out;
}

function className(node) {
  return node.fields?.find((field) => field.name === "_class")?.value ?? "ident";
}
