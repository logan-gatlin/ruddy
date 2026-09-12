# Ruddy Book first-draft validation

The latest game-development revision is recorded in [game-dev-validation.md](game-dev-validation.md); the checks below describe the earlier draft.


The first draft is published into the local documentation source at `docs/src/index.md` and `docs/src/book/`.
It contains 15 chapters, tooling and runtime appendices, selected exercise answers, and links to the existing syntax and dictionary appendices.
Existing public source-page paths remain available.
No deployment was performed.

## Checks performed

- `npm --prefix docs run build`: successful, producing 57 pages.
- `npm --prefix docs test`: all 9 documentation infrastructure tests passed.
- Generated HTML link audit: every local link target and fragment exists, and every page is reachable from the book index.
- Generated std Markdown audit: the complete set of files and their SHA-256 hashes match the snapshot taken before drafting.
- Chapter snippets: definitions were extracted in chapter order into separate private modules and compiled and run together, excluding the complete report and practice-project files, which were exercised separately.
- Runtime behavior: 42 assertions passed, covering expression evaluation, grouping, recursion, immutable updates, options, structural types, array operations, handlers, cells, process substitution, JSON, URL queries, rows, hidden types, downcasts, host callbacks, Unicode positions, exercise answers, and promise completion.
- Output forwarding: the local handler printed `Report: hello` before the successful completion message.
- Complete report: the documented sample produced the stated count and total; empty and all-paid inputs produced zero summaries; malformed JSON, an unexpected field, a negative amount, a missing file, and excess arguments failed as documented.
- Expected type failures: a numeric greeting, incompatible conditional branches, an escaping hidden type, and a violated presence constraint were rejected for their intended semantic reasons.
- `git diff --check` passed for the documentation changes.

## Scope of the evidence

Execution used the existing `target/debug/ruddy` binary, Node, and the working-tree standard library selected through a local dependency override in disposable projects.
The compiler was not rebuilt, installation from a published revision was not repeated, and no Rust tests were needed or run for this documentation change.
The HTTP example was type-checked but did not make a real network request.
The draft is not a performance evaluation or a reader usability study.

The generated standard-library Markdown and its existing uncommitted changes were preserved.
The initial draft renamed the navigation labels `Home` to `Book` and `Grammar` to `Syntax`; the Round 1 changes below extend the shared presentation while preserving other preexisting layout changes.


## Round 1 revision

The author clarified that banning escaping top-level initialization effects allows definitions to remain unordered, including mutually recursive definitions whose effects would otherwise have ambiguous ordering. The effects chapter now explains that rationale directly. The author also confirmed the discussion of mutable-cell allocation and access, result reuse, immutable sharing, and retained-version memory costs.

The revision leads with live LSP diagnostics, motivates concepts through their uses, relates tail calls to loops, states Ruddy's stack guarantee and foreign-call exception, explains immutable array trees and operation costs, and expands purity, pure wrappers, handler composition, and mutability tradeoffs. Presentation changes add optional filename bars, colored inline code, full-width chapter navigation buttons, and bundled Cousine fonts with no Powerline glyphs or enabled operator ligatures.

Validation after the revision:

- All 16 documentation tests passed, including filename rendering and escaping.
- The documentation build succeeded with 57 pages; all pages are reachable and local links and anchors resolve.
- Generated standard-library Markdown hashes match the snapshot taken before Round 1 edits. Existing unrelated changes remain intact.
- Extracted chapter snippets compile and execute against the existing debug compiler and working-tree standard library, including fences with filename metadata. All 47 runtime assertions pass.
- New recursion probes exercise 100,000 tail-recursive steps, 100,000 mutually recursive steps, and a 20,000-deep non-tail sum. These are example checks, not a proof of the runtime guarantee; that guarantee is documented in `.scratch/cps-lir/spec.md`.
- Pure wrapper annotations compile. Composed handlers print exactly `job: step: loaded`; the silent handler adds no output. The console wrapper explicitly distinguishes its callback's `Log` requirement from its own `IO` requirement.
- The complete invoice report passes its sample and seven additional valid/error cases.
- Browser checks at desktop and phone widths confirm the navigation matches the text width and phone navigation does not overflow. Dark and light appearances were inspected.
- The browser loads bundled Cousine. A filename block retains syntax highlighting, has one copy button, and copies exactly its source without the filename. Inline code has a distinct text color in both appearances.
- Font character maps were checked with `fc-query`; regular, italic, and bold assets contain no U+E0A0–U+E0D7 glyphs. The upstream license and provenance are bundled with them.
- Documentation whitespace checks pass. No Rust sources were edited for this revision, and no Rust tests were run.


## Special effect cases

The effects chapter now explains operations with no possible resumption result (`() -> |`) and effects with no operations that Ruddy handlers can intercept, using `std::ffi::Immediate`. It includes cancellation, forwarding, the distinction between unit and the empty sum, direct foreign observations, and the role of a replaceable application effect. Exercises, selected answers, and the interoperability cross-reference accompany the sections.

- All extracted book snippets compile and execute; the existing 60 runtime assertions still pass.
- Focused examples produce `cancelled`, `finished`, and `outer cancelled`, verifying cancellation, ordinary unit resumption, and forwarding a non-resuming operation.
- Returning unit to a `() -> |` operation is rejected with a type mismatch. An Immediate handler arm is rejected because there is no operation. A return-only handler cannot make the foreign read pure and is rejected at the effect boundary.
- The foreign clock declaration and wrapper were compiled without invoking the host clock during validation.
- The documentation build succeeds with 57 pages, and all local links and anchors resolve. Generated standard-library Markdown hashes remain unchanged.
- Checks used the existing debug compiler and disposable projects; no compiler rebuild or Rust tests were needed.
