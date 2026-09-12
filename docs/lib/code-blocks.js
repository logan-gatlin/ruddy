// A filename is presentation metadata, separate from the code copied or highlighted.
export function addCodeFilenames(markdown) {
  const renderFence = markdown.renderer.rules.fence;
  markdown.renderer.rules.fence = (tokens, index, options, env, renderer) => {
    const html = renderFence(tokens, index, options, env, renderer);
    const info = tokens[index].info.trim();
    const match = info.match(/^\S+\s+filename=(?:"([^"]+)"|'([^']+)'|(\S+))\s*$/);
    if (!match) return html;
    const filename = markdown.utils.escapeHtml(match[1] ?? match[2] ?? match[3]);
    return `<div class="code-block copyable-code"><div class="code-caption"><span class="code-filename">${filename}</span></div>${html}</div>\n`;
  };
}
