# Ruddy VS Code extension

Agreed through the grilling interview; implementation authorized by the user.

- Deliver a locally installable VSIX for desktop VS Code and remote workspaces.
- Recognize `.rud` as Ruddy. Bundle the existing Tree-sitter grammar as WASM and
  the existing highlight queries. No TextMate fallback. Use semantic tokens,
  enabled by default for Ruddy while respecting user overrides.
- Highlight independently of the CLI, including standalone files and untrusted
  workspaces. Reuse edited syntax trees for incremental parsing. Verify insertion,
  deletion, multiline, multiple simultaneous edits, and Unicode against fresh parsing.
- Launch `ruddy lsp` over stdio from PATH, with a configurable executable path.
  No compiler downloads or bundling. Explain installation when unavailable.
- Expose existing diagnostics, hover, completion, go-to-definition, and document
  formatting. Leave format-on-save to the user's settings.
- Run one server per workspace folder with a `Ruddy.toml` directly at its root.
  No automatic nested project discovery; standalone files receive highlighting.
- Only launch executables in trusted workspaces. In remote workspaces execute on
  the remote host. Browser-only support is deferred.
- Provide actionable server errors, output logs, and a restart command. Restart
  affected servers when their executable setting changes.
- Build, test, review against the pre-implementation commit
  `66ba3f2ffb445724405932a6250bdb03129a4a80`, then commit on the current branch.

Verification boundaries: highlighted documents and VS Code editor commands with
the real Ruddy language server. All Rust tests must run through `just test`.

Implementation note: highlighting tests exposed a pre-existing Tree-sitter bug
where nested block comments on separate lines consumed following definitions.
The grammar fix and regression corpus case are included so bundled highlighting
handles these valid programs correctly.

## CLI installation follow-up

The user requested a separate `just` command to build and install the grammar,
plus a convenient one-line extension install command on the website in `docs/`.

- `just vscode-install` regenerates/tests the grammar, packages the extension,
  and installs it through VS Code's CLI; an optional editor executable supports
  alternate installations. `just vscode` remains packaging-only.
- Website builds include a freshly packaged VSIX at `downloads/ruddy.vsix`.
- The download page documents a single-line `curl` plus `code --install-extension`
  command, a direct VSIX link, prerequisites, and the separate CLI requirement for
  LSP features. Publishing uses the existing docs deployment workflow.

## Distribution correction

The user rejected bundling the extension with the website and asked how to publish
to the Marketplace. This supersedes the website-distribution items above: docs
builds must not package or host a VSIX. Keep the local `just vscode-install` flow.
Marketplace account setup and publication are not yet configured; document source
installation until the actual publisher identity and listing exist.
