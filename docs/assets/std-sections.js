const reference = document.querySelector(".std-reference");

if (reference) {
  const headings = [...reference.querySelectorAll(":scope > h2")];
  for (const [index, heading] of headings.entries()) {
    const content = document.createElement("div");
    content.className = "std-section-content";
    content.id = `${heading.id || `section-${index + 1}`}-content`;

    while (heading.nextElementSibling && heading.nextElementSibling.tagName !== "H2") {
      content.append(heading.nextElementSibling);
    }
    heading.after(content);

    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "std-section-toggle";
    toggle.setAttribute("aria-controls", content.id);
    toggle.setAttribute("aria-expanded", "true");
    while (heading.firstChild) toggle.append(heading.firstChild);
    heading.append(toggle);

    toggle.addEventListener("click", () => {
      const expanded = toggle.getAttribute("aria-expanded") === "true";
      toggle.setAttribute("aria-expanded", String(!expanded));
      content.hidden = expanded;
    });
  }
}
