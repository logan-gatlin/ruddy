---
doc: false
---

# Bundled code font

Cousine regular, italic, and bold are unmodified WOFF2 files from [googlefonts/cousine](https://github.com/googlefonts/cousine/tree/c0fbdb438443968c884a5c13f5f9bee916a7f89b/fonts/webfonts), commit `c0fbdb438443968c884a5c13f5f9bee916a7f89b`.
The upstream SIL Open Font License is included as `OFL.txt`.
These files have no glyphs in the Powerline range U+E0A0–U+E0D7, verified from their character maps with `fc-query`.
The website disables ligatures and contextual alternates so sequences such as `=>`, `->`, and `|>` retain separate characters.
CSS loads these assets directly without selecting a locally installed font variant.
