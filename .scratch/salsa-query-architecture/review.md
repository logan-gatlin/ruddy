# Implementation review

Fixed point: `5075031d8e7cd406c44b92691c9a2416fc5eb833`.
Initial implementation: `2e5953a`.

The code-review skill's independent standards and spec reviewers inspected
`git diff 5075031d8e7cd406c44b92691c9a2416fc5eb833...HEAD`, then the spec reviewer
checked the corrective working-tree changes. Reviewers did not run builds or
tests while the parent collected measurements.

## Standards

No additional hard violation was established in source review. New tests live in
the tests crate, debugger recovery was adapted, and there were no grammar changes.
The repository's 100% compiler coverage requirement remains unmet: 95.96% lines
and 88.54% branches. This remaining validation gap is recorded separately from
the corrected source-review findings.

One heuristic finding: **Duplicated Code / Repeated Switches** in dependency
expansion and validation shared by `Workspace::visit` and `GraphCompiler::visit`.
Resolution: extracted common std/dependency expansion used by CLI, debugger,
and editor, and reused the dependency-name diagnostic. Acquisition and artifact
compilation strategies remain separate.

## Spec

Three initial findings were confirmed and corrected:

1. **Repeated Git acquisition on ordinary edits.** The workspace now retains
   successfully acquired selections while the specification and root lock inputs
   remain applicable, and prunes unreachable selections after refresh.
2. **Uncancellable foreground Git work.** Acquisition owns its resolver on a
   helper thread. The editor's wait is cancellable; gix receives the shared
   cancellation signal, and cache-lock contention checks cancellation. Shutdown
   interrupts an active request. Resolver/cache-lock ownership is dropped on
   cancellation without retaining a global lock for the editor session.
3. **Inconsistent symlinked document identity.** Overlays, focus, and file lookup
   resolve existing path prefixes, including for new unsaved buffers. Watchers
   retain the original acquisition paths.

The follow-up review found two related details: gix can report interrupted
checkout as success, and definition results could replace an open symlink URI
with its canonical spelling. Cancellation now checks before index/HEAD state is
published; definitions prefer the existing document URI. Closing a replaced URI
alias does not remove the current document's overlay.

The final bounded review verified both follow-ups and reported no remaining
findings. Focused Git tests exercise acquired-selection reuse while a cache lock
is held and cancellation/retry for a fresh workspace under that contention.
Symlink tests cover unsaved/missing files and definition URI preservation.

Initial findings: Standards 1 heuristic; Spec 3 correctness issues, plus 2
follow-up details. All reviewed findings were addressed. Coverage is reported
separately in [the validation record](progress.md).
