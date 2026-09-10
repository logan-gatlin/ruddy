const copyIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1"/></svg>';
const checkIcon = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m5 12 4 4L19 6"/></svg>';

for (const pre of document.querySelectorAll("main pre")) {
  const code = pre.querySelector("code");
  if (!code) continue;
  const wrapper = document.createElement("div");
  wrapper.className = "copyable-code";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "copy-code";
  button.setAttribute("aria-label", "Copy code");
  button.title = "Copy code";
  button.innerHTML = copyIcon;
  const status = document.createElement("span");
  status.className = "copy-feedback";
  status.setAttribute("role", "status");
  pre.before(wrapper);
  wrapper.append(pre, button, status);
  let reset;
  button.addEventListener("click", async () => {
    clearTimeout(reset);
    try {
      await navigator.clipboard.writeText(code.textContent);
      button.innerHTML = checkIcon;
      button.setAttribute("data-copied", "");
      status.textContent = "Copied";
    } catch {
      const range = document.createRange();
      range.selectNodeContents(code);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      status.textContent = "Code selected; copy with Ctrl+C or ⌘C.";
    }
    reset = setTimeout(() => {
      button.innerHTML = copyIcon;
      button.removeAttribute("data-copied");
      status.textContent = "";
    }, 2500);
  });
}
