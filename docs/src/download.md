---
doc: true
---

# Download

## Install the CLI

Installation requires a current stable [Rust toolchain](https://www.rust-lang.org/tools/install).
The following command builds and installs Ruddy from source:

```sh
cargo install --git https://ruddy.logan.md --locked ruddy
```

[Node.js](https://nodejs.org/en/download) is optional and is needed for `ruddy run` to run the JavaScript target.

## Set up an editor

Choose an editor for syntax highlighting, diagnostics, completion, hover, go-to-definition, and formatting.

::: editor-tabs
::: tab VS Code
Install [Ruddy from the Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=lgatlin.ruddy).
The extension includes syntax highlighting and starts the Ruddy language server when you open and trust a folder containing `Ruddy.toml`.

Language server features require `ruddy` on VS Code's `PATH`; alternatively, set `ruddy.server.path` to the executable.
Remote workspaces require the extension and Ruddy CLI on the remote host.
::: tab Helix
Add the following to `~/.config/helix/languages.toml`.
It fetches Ruddy's Tree-sitter grammar from GitHub, starts the installed `ruddy` executable as a language server, and formats with `ruddy fmt --stdin` on save.

```toml
[language-server.ruddy]
command = "ruddy"
args = ["lsp"]

[[language]]
name = "ruddy"
scope = "source.ruddy"
injection-regex = "ruddy"
file-types = ["rud"]
roots = ["Ruddy.toml"]
indent = { tab-width = 2, unit = "  " }
grammar = "ruddy"
language-servers = ["ruddy"]
formatter = { command = "ruddy", args = ["fmt", "--stdin"] }
auto-format = true

# Ruddy uses ' as its ML-style type-parameter prefix, not as a delimiter.
[language.auto-pairs]
'(' = ')'
'{' = '}'
'[' = ']'
'"' = '"'
'`' = '`'
'<' = '>'

[[grammar]]
name = "ruddy"
source = { git = "https://github.com/logan-gatlin/ruddy", rev = "main", subpath = "treesitter" }
```

Fetch and compile the grammar after saving the configuration:

```sh
hx --grammar fetch
hx --grammar build
```

Open a `.rud` file, or verify the setup with `hx --health ruddy`.
:::

## Run a first project

With Node.js installed, the following commands create and run a project:

```sh
ruddy new hello
cd hello
ruddy run
```

## Source code
The Ruddy mono-repo is hosted on [GitHub](https://github.com/logan-gatlin/ruddy).
