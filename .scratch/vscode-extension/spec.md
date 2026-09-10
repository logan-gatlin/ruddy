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
