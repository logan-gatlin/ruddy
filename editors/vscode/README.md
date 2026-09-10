# Ruddy for VS Code

Syntax highlighting for `.rud` files using the bundled Ruddy Tree-sitter parser,
plus diagnostics, hover, completion, go-to-definition, and document formatting
through `ruddy lsp`.

## Install

From a Ruddy checkout, `just vscode-install` regenerates and checks the grammar,
builds the extension, and installs it with `code --install-extension --force`.
The `code` command must be on PATH. An alternative executable can be selected
with `just vscode-install code-insiders`. Use `just vscode` to build the VSIX
without installing it; this writes `editors/vscode/ruddy.vsix`.

The [website download page](https://ruddy.logan.md/download.html) provides a
prebuilt VSIX and a one-line download/install command.

In VS Code, run **Extensions: Install from VSIX…** and select the downloaded or
locally built `.vsix` file.
Highlighting works immediately, including standalone files and Restricted Mode.
It uses VS Code semantic tokens, enabled by default for Ruddy. If you override
`editor.semanticHighlighting.enabled` to `false`, highlighting is disabled;
there is no TextMate fallback.

For language server features, install the Ruddy CLI. From a checkout of
[Ruddy](https://github.com/logan-gatlin/ruddy), with Rust and `just` installed:

```sh
just install
```

Make sure `ruddy` is on the PATH visible to VS Code, or set **Ruddy › Server: Path**
(`ruddy.server.path`) to the executable. Enter a path, not a shell command;
the extension supplies the `lsp` argument. Relative paths resolve from the
project workspace folder. Restart VS Code after changing your shell's PATH.

Open the folder containing `Ruddy.toml`, then open a `.rud` file and trust the
workspace to start the language server. A multi-root workspace gets one server
per folder with a `Ruddy.toml` directly at its root. Add nested projects as
workspace folders explicitly; the extension does not search for them. Files
outside those project folders have highlighting only.

Use **Format Document** for formatting. Format-on-save follows your own VS Code
settings. Use **Ruddy: Restart Language Server** to recover a stopped server;
details appear in the **Ruddy** output channel. Changing the executable setting
automatically restarts affected servers.

Desktop VS Code and remote workspaces are supported. For SSH, WSL, or containers,
install the extension and Ruddy on the remote host; the executable setting and
PATH are resolved there. Browser-only VS Code is not supported.

## Develop and package

Requires Node.js 20 or newer and npm. From `editors/vscode`:

```sh
npm ci
npm run build
npm run typecheck
npm run test:unit
npm run package
```

The build compiles `treesitter/src/parser.c` to WASM, copies the existing
`treesitter/queries/highlights.scm` and Tree-sitter runtime, and bundles the
extension's JavaScript and dependency licenses. Tree-sitter's build tool fetches
wasi-sdk on its first build. End users need none of these build dependencies.
After grammar changes, run `just grammar` at the repository root before rebuilding
the extension. Generated extension assets and VSIX packages are not committed.

For the full extension tests, first run `cargo build -p ruddy --bin ruddy` at the
repository root, then `npm test` here. On headless Linux, use `xvfb-run -a npm test`.
The tests download VS Code 1.96.4, the minimum supported release, and exercise the
extension in an isolated profile with temporary projects and the real server.
Set `RUDDY_TEST_BINARY` to use another compiler binary, or `VSCODE_TEST_VERSION`
to test another editor release. Rust tests must run through `just test`.

The highlighter keeps a parser and syntax tree per requested document. Each edit
updates tree coordinates before the next incremental parse, including multicursor
edits and UTF-16 positions. Highlight queries currently run over the resulting
whole tree; incremental parsing does not imply incremental query evaluation.
