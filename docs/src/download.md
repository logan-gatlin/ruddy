---
doc: true
---

# Download

## Install the CLI

Installation requires a current stable [Rust toolchain](https://www.rust-lang.org/tools/install).
The following command builds Ruddy from source and installs: and installs Ruddy from the [GitHub repository]():

```sh
cargo install --git https://ruddy.logan.md --locked ruddy
```

[Node.js](https://nodejs.org/en/download) is optional and is needed for `ruddy run` to run the JavaScript target.

## Install the VS Code extension

The Ruddy extension provides syntax highlighting and connects to the installed Ruddy CLI for diagnostics, completion, hover, go-to-definition, and formatting.
The following command downloads and installs the extension on Linux or macOS, with `curl` and the [VS Code `code` command](https://code.visualstudio.com/docs/configure/command-line) available:

```sh
curl -fL https://ruddy.logan.md/downloads/ruddy.vsix -o ruddy.vsix && code --install-extension ./ruddy.vsix --force
```

The command saves `ruddy.vsix` in the current directory; the file can be deleted after installation.
Running the command again installs the latest version served by this website.
Alternatively, the [extension download](downloads/ruddy.vsix) can be installed through **Extensions: Install from VSIX…** in VS Code on any supported desktop platform.
On macOS, **Shell Command: Install 'code' command in PATH** in the Command Palette enables the CLI command.

Highlighting works without the Ruddy CLI.
Language server features require `ruddy` on VS Code's `PATH`, or an executable path configured through `ruddy.server.path`.
Opening the folder containing `Ruddy.toml` and trusting the workspace enables the language server.
Remote workspaces require the extension and Ruddy CLI on the remote host.

## Run a first project

With Node.js installed, the following commands create and run a project:

```sh
ruddy new hello
cd hello
ruddy run
```

## Source code
The Ruddy mono-repo is hosted on [GitHub](https://github.com/logan-gatlin/ruddy).
