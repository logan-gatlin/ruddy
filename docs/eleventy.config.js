import path from "node:path";
import MarkdownIt from "markdown-it";
import anchor from "markdown-it-anchor";
import { slug } from "github-slugger";
import { createHighlighter } from "./lib/highlight.js";

function addEditorTabs(markdown) {
  const marker = /^:::\s*editor-tabs\s*$/;
  const tab = /^:::\s*tab\s+(.+?)\s*$/;

  markdown.block.ruler.before("fence", "editor_tabs", (state, startLine, endLine, silent) => {
    const line = (number) => state.src
      .slice(state.bMarks[number] + state.tShift[number], state.eMarks[number])
      .trim();
    if (!marker.test(line(startLine))) return false;

    const panels = [];
    let current;
    let closeLine;
    for (let number = startLine + 1; number < endLine; number += 1) {
      const match = line(number).match(tab);
      if (match) {
        if (current) current.body = state.src.slice(state.bMarks[current.start], state.bMarks[number]);
        current = { label: match[1], start: number + 1, body: "" };
        panels.push(current);
      } else if (line(number) === ":::") {
        if (current) current.body = state.src.slice(state.bMarks[current.start], state.bMarks[number]);
        closeLine = number;
        break;
      }
    }
    if (closeLine === undefined || panels.length === 0) return false;
    if (silent) return true;

    const token = state.push("editor_tabs", "div", 0);
    token.block = true;
    token.map = [startLine, closeLine + 1];
    token.meta = { panels, line: startLine + 1 };
    state.line = closeLine + 1;
    return true;
  });

  markdown.renderer.rules.editor_tabs = (tokens, index, _options, env) => {
    const { panels, line } = tokens[index].meta;
    const escape = markdown.utils.escapeHtml;
    const tabs = panels.map((panel, panelIndex) => {
      const id = `editor-tabs-${line}-${panelIndex + 1}`;
      return `<button type="button" role="tab" id="${id}-tab" aria-controls="${id}" aria-selected="${panelIndex === 0}" tabindex="${panelIndex === 0 ? 0 : -1}">${escape(panel.label)}</button>`;
    }).join("");
    const content = panels.map((panel, panelIndex) => {
      const id = `editor-tabs-${line}-${panelIndex + 1}`;
      return `<section role="tabpanel" id="${id}" aria-labelledby="${id}-tab">${markdown.render(panel.body, env)}</section>`;
    }).join("");
    return `<div class="editor-tabs" data-editor-tabs><div class="editor-tab-list" role="tablist" aria-label="Editor">${tabs}</div>${content}</div>`;
  };
}

export default function (eleventyConfig) {
  const highlighter = createHighlighter();
  eleventyConfig.on("eleventy.before", highlighter.clear);
  // Absolute paths avoid Eleventy 3.1 remapping all watch events to the parent directory.
  eleventyConfig.addWatchTarget(path.resolve("../treesitter/src/parser.c"));
  eleventyConfig.addWatchTarget(path.resolve("../treesitter/queries"));
  const markdown = new MarkdownIt({ html: false, highlight: highlighter.highlight }).use(anchor, {
    slugify: slug,
  });
  addEditorTabs(markdown);
  const renderLink = markdown.renderer.rules.link_open;

  // Keep source links useful in Markdown and in the generated site.
  markdown.renderer.rules.link_open = (tokens, index, options, env, renderer) => {
    const token = tokens[index];
    const href = token.attrGet("href");
    if (href && !/^(?:[a-z][a-z\d+.-]*:|\/\/|#)/i.test(href)) {
      token.attrSet("href", href.replace(/\.md(?=[?#]|$)/i, ".html"));
    }
    return renderLink
      ? renderLink(tokens, index, options, env, renderer)
      : renderer.renderToken(tokens, index, options);
  };
  eleventyConfig.setLibrary("md", markdown);
  eleventyConfig.addGlobalData("eleventyComputed", {
    permalink: (data) => `${data.page.filePathStem}.html`,
  });

  eleventyConfig.addPreprocessor("language-docs", "md", (data, content) => {
    if (data.doc !== true) return false;
    data.layout = "page.njk";
    data.title = content.match(/^#\s+(.+)$/m)?.[1] ?? data.page.fileSlug;
  });

  // Relative URLs allow hosting at either a domain root or a subdirectory.
  eleventyConfig.addFilter("relativeUrl", (target, current) =>
    path.posix.relative(path.posix.dirname(current), target),
  );
  eleventyConfig.addPassthroughCopy({ assets: "assets" });

  return {
    dir: { input: "src", output: "_site", includes: "../_includes" },
    templateFormats: ["md"],
    markdownTemplateEngine: false,
    htmlTemplateEngine: "njk",
  };
}
