---
doc: false
---

# Doc Writing Guide
## What the docs are for
This documentation is for the Ruddy language, not the compiler itself.
Divergences between this spec and the compiler's behavior are bugs in the compiler.
Compiler implementation details are out of scope, but runtime guarantees are relevant.
For example; 
* Doesn't belong - "the compiler backend uses CPS architecture"
* Does belong - "the compiler emits code which is stack safe and async agnostic"

Usage of the compiler CLI and other tooling is also relevant.

## The Reader
The primary audience of the docs are developers learning Ruddy for the first time.
Assume an undergraduate degree level of knowledge in computer science.

## Organization
The first time a related topic is referenced in a doc, it should have an inline markdown link to the relevant doc.
Every Markdown file in `docs/` starts with YAML frontmatter containing `doc: true` for language documentation or `doc: false` for supporting guidance.
The [index](src/index.md) doc must (at least indirectly) connect to every other file marked `doc: true`.
Within a doc concepts should flow from easiest to most difficult to understand, progressively building understanding.
After the frontmatter, files start with a level 1 header that is the title, and may contain level 2 and 3 headers.

## Prose
Strictly one sentence per line in a file so git diffs are readable.
Vary sentence width to avoid monotony.
Tables and lists are great ways to to visually break up files for readability, and convey structured information.
If a word, sentence, or paragraph can be removed without ambiguity, do so.
Write in the third person only. 
Do not use jargon, dense, or flowery language, - be concious of ESL readers.
When a new term is introduced, add it to the [dictionary](src/dictionary.md) - this file is the source of truth for terms and what they mean.
Never use a synonym for a word in the dictionary.

## Code Examples
Code examples are helpful for understanding and should be included, but not sufficient to explain concepts on their own. 
A code example should demonstrate exactly one thing, and that thing should immediately preceed it in the text.
Examples must be coherent, practical code that a real person would write.
An individual code example need not be a valid Ruddy program, term and type definitions may be implied by context.
All keywords, variable names, or syntax snippets inside of prose must be inside inline code blocks.
Ruddy code blocks are formatted like:

```ruddy
let id = fn a => a
```

A fence may include a filename when it represents a file or a precise edit to one:

````text
```ruddy filename="src/main.rud"
let main = fn _ => println "Hello"
```
````

The filename appears in a separate bar and is excluded from copied code.
Fragments without a file context omit this metadata.

## Website preview

The documentation website uses [Eleventy](https://www.11ty.dev/docs/) and requires Node.js 22 or newer and a C compiler for Tree-sitter.
The following commands run from `docs/`:

```sh
npm ci
npm run dev
```

The preview reloads after edits and is available at the URL printed by Eleventy, normally `http://localhost:8080`.
The command `npm run build` creates a clean static website in `docs/_site/`, ready to serve from a domain root or subdirectory.
Language documentation lives in `docs/src/`; only Markdown files marked `doc: true` become pages.
Relative `.md` links become `.html` links, and headings receive anchors compatible with the dictionary links.
The shared layout is `_includes/page.njk`, the generated standard-library reference extends it through `_includes/std.njk`, and the stylesheet is `assets/style.css`.

Code fences marked `ruddy` or `rud` are highlighted during the build using the existing Tree-sitter parser and the highlight and local-name queries in `treesitter/`.
Shell fences (`sh`, `bash`, or `shell`), `toml`, and `json` use the corresponding Tree-sitter grammars installed with the website dependencies.
Fences marked `text`, unlabelled fences, and unsupported languages remain plain text.
The Tree-sitter CLI is installed with the website dependencies; syntax highlighting adds no browser JavaScript.
The preview rebuilds when the generated parser or queries change.
After edits to `treesitter/grammar.js`, `just grammar` from the repository root regenerates the parser and checks the grammar.
The command `npm test` from `docs/` checks highlighting, source preservation, and HTML escaping.

## Website deployment

The website is deployed to [ruddy.logan.md](https://ruddy.logan.md) through the Cloudflare Worker `ruddy-docs`.
The configuration is in `wrangler.jsonc`.
The command `just deploy` from the repository root builds and deploys the website.
After Cloudflare authentication with `npx wrangler login`, the following commands run from `docs/`:

```sh
npm run deploy:check
npm run deploy
```

Both commands rebuild the website; `deploy:check` validates the deployment locally, and `deploy` publishes it.
The generated files in `_site/` and the routing code in `worker.js` are uploaded.
The Worker redirects Git discovery and fetch requests to the GitHub repository while serving the documentation normally.
