# VS Code extension review

Baseline: `66ba3f2ffb445724405932a6250bdb03129a4a80`.
Standards and Spec were reviewed independently by parallel agents.

## Standards

No documented-standard violations or actionable baseline smells found.

`AGENTS.md` requires Rust tests through `just test`; the extension scripts
introduce no direct Rust test invocation. `CONTRIBUTING.md`'s test placement and
coverage rules concern the Rust workspace/compiler, so separate TypeScript
extension tests are appropriate. The Tree-sitter comment fix includes regenerated
parser artifacts and a corpus regression test.

The implementation separates parsing/highlight generation, VS Code integration,
and server lifecycle responsibilities without unnecessary abstractions.

Generated parser internals and the dependency lockfile were excluded from manual
review.

## Spec

No actionable findings against [the agreed spec](spec.md).

- Missing or partial requirements: none found. The implementation includes bundled
  WASM and existing queries, incremental tree edits and reuse, independent
  highlighting, trusted per-folder LSP startup, configuration changes, recovery
  controls, and VSIX packaging.
- Scope creep: none material. The nested-comment grammar correction directly
  supports accurate bundled highlighting and is documented in the spec.
- Implementation bugs against requirements: none identified. Incremental edits
  use UTF-16 coordinates and descending original-document offsets; overlapping
  captures resolve deterministically. Workspace ownership middleware prevents
  nested projects from sharing document requests.

Windows and remote execution were inspected statically; editor integration tests
ran on Linux with VS Code 1.96.4, in trusted and untrusted workspaces. The same
integration tests also passed against the extracted VSIX, without the development
checkout's runtime dependencies.

Standards: 0 findings, no worst issue. Spec: 0 findings, no worst issue.

## Validation

- `just test`: full Rust workspace suite passed.
- `just grammar`: 130 corpus cases and 51 highlight assertions passed.
- `xvfb-run -a npm test` in `editors/vscode`: TypeScript typecheck, WASM build,
  four highlighter tests (including 50 successive edits of the language demo),
  and trusted/untrusted VS Code integration tests passed.
- `npm run package`: produced `editors/vscode/ruddy-0.1.0.vsix` (about 264 KB).
- The trusted/untrusted integration tests also passed with
  `RUDDY_TEST_EXTENSION_PATH` pointing to the extracted VSIX.

## CLI installation follow-up

Baseline: `09b16556db283bbc545d581d145bc01af5797299`.

### Standards

No findings. The docs follow the writing guide, and the recipes compose grammar
validation and packaging without introducing direct Rust test invocations.

### Spec

No findings. The install recipe, stable artifact filename, website build, direct
download link, and one-line command agree with the follow-up requirements.

Validation: website build and all eight docs tests passed; `just vscode-install`
installed into an isolated real VS Code profile. Downloading the generated site's
VSIX over local HTTP and installing it through the documented CLI flow also
succeeded (`ruddy-lang.ruddy@0.1.0`). The public site was not deployed.

Standards: 0 findings, no worst issue. Spec: 0 findings, no worst issue.
