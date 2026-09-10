import path from "node:path";
import MarkdownIt from "markdown-it";
import anchor from "markdown-it-anchor";
import { slug } from "github-slugger";
import { createHighlighter } from "./lib/highlight.js";

export default function (eleventyConfig) {
  const highlighter = createHighlighter();
  eleventyConfig.on("eleventy.before", highlighter.clear);
  // Absolute paths avoid Eleventy 3.1 remapping all watch events to the parent directory.
  eleventyConfig.addWatchTarget(path.resolve("../treesitter/src/parser.c"));
  eleventyConfig.addWatchTarget(path.resolve("../treesitter/queries"));
  const markdown = new MarkdownIt({ html: false, highlight: highlighter.highlight }).use(anchor, {
    slugify: slug,
  });
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
