// The diagnostic strip.
//
// Always visible, always in source order, and never scrolled out of reach: a
// compiler debugger where you have to go looking for the errors is the wrong
// shape. When there are none it collapses to one line and gives the height
// back to the editor.

import { esc } from "./panel.js";

export function createDiagnostics(root, app) {
  let at = -1;

  app.on("snapshot", () => {
    at = -1;
    render();
  });
  app.on("status", render);
  app.on("highlight", mark);

  root.addEventListener("click", (event) => {
    const row = event.target.closest(".diag-row");
    if (!row) return;
    at = Number(row.dataset.at);
    select();
  });

  function list() {
    return app.state.snapshot?.diagnostics ?? [];
  }

  function select() {
    const diagnostic = list()[at];
    if (!diagnostic) return;
    app.setSelection({
      origin: "diag",
      span: diagnostic.span,
      related: diagnostic.related ?? [],
      symbol: null,
      stage: null,
      node: null,
    });
    mark();
  }

  function render() {
    const { snapshot, buildError, notice } = app.state;
    let html = "";

    if (buildError) {
      html += `<div class="banner"><b>compiler build failed</b> — serving the last good binary<pre>${esc(
        buildError,
      )}</pre></div>`;
    }
    if (snapshot?.panic) {
      const panic = snapshot.panic;
      html += `<div class="banner"><b>panic in ${esc(panic.stage)}</b> — ${esc(
        panic.message,
      )} <span class="rel">at ${esc(panic.location)}</span></div>`;
    }
    if (notice) {
      html += `<div class="clean">${esc(notice)}</div>`;
    }

    const diagnostics = list();
    if (!diagnostics.length) {
      html += snapshot ? `<div class="clean">no errors</div>` : `<div class="clean">…</div>`;
    } else {
      html += diagnostics
        .map((diagnostic, i) => {
          const related = (diagnostic.related ?? [])
            .map((r) => ` <span class="rel">↳ ${esc(app.spanLabel(r.span))} ${esc(r.message)}</span>`)
            .join("");
          return (
            `<div class="diag-row" data-at="${i}">` +
            `<span class="mark">✕</span>` +
            `<span class="at">${esc(app.spanLabel(diagnostic.span))}</span>` +
            `<span class="from">${esc(diagnostic.stage)}</span>` +
            `<span class="msg">${esc(diagnostic.message)}${related}</span>` +
            `<span class="code">${esc(diagnostic.code)}</span>` +
            `</div>`
          );
        })
        .join("");
    }

    root.innerHTML = html;
    mark();
  }

  function mark() {
    for (const row of root.querySelectorAll(".diag-row")) {
      row.classList.toggle("on", Number(row.dataset.at) === at);
    }
  }

  return {
    /// Step through the errors without reaching for the mouse. Wraps, because
    /// the alternative is a keystroke that silently does nothing.
    step(delta) {
      const diagnostics = list();
      if (!diagnostics.length) return;
      at = (at + delta + diagnostics.length) % diagnostics.length;
      select();
      root.querySelector(`.diag-row[data-at="${at}"]`)?.scrollIntoView({ block: "nearest" });
    },
  };
}
