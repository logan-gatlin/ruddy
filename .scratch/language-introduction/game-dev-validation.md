# Game-development revision validation

Validated 2026-09-12 against the current working tree with the existing `target/debug/ruddy` executable and local standard library.
The [current outline](book-outline.md) describes the revised reading path; [game-development research](game-dev-research.md) records the source-backed investigation and complete runnable type-system candidates.

## Changes

Reworked all fifteen chapters, the tooling/runtime appendices, selected answers, and the front-page roadmap around game logic and tools.
Updated the handwritten syntax reference and the platform-reference decoder shape to keep supporting examples consistent.
The complete program is now a combat replay with pure damage rules and a separate executable boundary.
Its existing `book/report.html` URL is retained so earlier links continue to work.

The type-system progression now includes independent coordinate rows, shared movement rows, distinct coordinate-space tags, inferred state/component relationships, state-preserving transitions, equipment/selection row sharing, conditional audio contracts, and hidden actor-state packages.
Each explanation identifies both the relationship established by the types and relevant gameplay rules that remain the implementation's responsibility.

## Executable checks

Disposable validation projects and scripts are under `/tmp/ruddy-game-book-s5meal3g`.
The research candidate and its negative checks are under `/tmp/ruddy-game-types-research`.
No compiler, standard-library source, or generated standard-library documentation was changed in this revision.

- Extracted the Ruddy fences from the book into isolated chapter modules and compiled them together with `ruddy run`, excluding the complete replay and tooling setup fragments that have their own file layouts.
- Passed 52 runtime assertions covering expression results, currying, 100,000 tail-recursive iterations, immutable updates, coordinate operations, transforms/folds, modules, handlers, cancellation, cells, JSON, presence-sensitive state, actor updates, and host callbacks.
- Verified the composed log output as `match: ai: target acquired` followed by the successful assertion marker.
- Built and ran the replay directly from its two documented source files and sample JSON; output was exactly `Slime: 0` and `Golem: 13`.
- Passed 13 additional replay scenarios: empty data, no hits, zero damage, exact damage, repeated excess damage, invalid target, negative damage, missing field, extra field, wrong field type, malformed JSON, missing file, and excess arguments.
- Passed four direct pure-replay assertions for the expected result, repeatability, preserved initial roster, and empty-hit identity.
- The research candidate passed its five expected output checks and nine rejected-program checks: missing coordinate, both/neither session states, wrong-state consumer, half velocity, path without target, absent AI plan, unavailable weapon, and escaping hidden state.
- Four further checks against the actual chapter definitions rejected mixed world/screen coordinates, a string coordinate, an overpermissive state annotation, and an incompatible audio callback/configuration.

The Rust test suite was not run because this revision changes documentation only.
All program checks used the documented CLI against disposable projects.

## Website checks

`npm run build` in `docs/` generated 57 pages successfully.
A rendered-HTML audit checked local destinations and heading anchors and verified that every page is reachable from the book index.
A SHA-256 comparison against the start-of-revision snapshot confirmed that the entire generated `docs/src/std/*.md` file set is unchanged.
The snapshot is specific to this revision; pre-existing standard-library changes in the working tree were preserved.
