# Caller-choice diagnostic repair plan

- [x] Distinguish closing a caller-chosen effect remainder from restricting it with a specific performed operation.
- [x] Offer only repairs applicable to the effect-row failure: preserve/remove the remainder for closure, handle/list the operation for restriction.
- [x] Replace invalid destination-ownership advice for escaping rigids with source-annotation or value-flow repairs.
- [x] Collapse duplicate escape destination labels into one grounded fact naming the destination binding and source-level inferred type.
- [x] Add exact closure/restriction, escape-repair validity, and one-label/deep-type regressions and update structured goldens.
- [x] Run formatting, the full workspace suite, checks, and Clippy; commit without disturbing unrelated changes.

## Effect-boundary provenance follow-up

- [x] Carry resolved effect symbol, structural interface, and declaration span through `Performs` constraints and generalized aliases.
- [x] Resolve declaration evidence from exact operation provenance rather than structurally coalesced row keys.
- [x] Make the unavoidable call primary for unhandled effects and the refusing function boundary primary for disallowed effects; keep declarations related-only.
- [x] Cover coalesced and distinct same-leaf interfaces, alias propagation, CLI goldens, and debugger parity.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Bare operation-value provenance follow-up

- [x] Capture exact operation identity when an operation is resolved as a value, not only when directly applied.
- [x] Keep structured value provenance beside inferred and local/top explained schemes without changing `Ty` or `Scheme`.
- [x] Substitute higher-order parameter flow and preserve callback origins through call results, storage, and projection.
- [x] Cover bare aliases, projected/stored callbacks, forwarding, and coalesced qualified same-leaf declarations.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Scope- and path-aware effect provenance follow-up

- [x] Represent projected parameter provenance as a symbolic field path and resolve it against the exact argument value during application.
- [x] Collect performed sources per callable walk so nested closure bodies do not leak into an enclosing callable unless invoked there.
- [x] Cover projected higher-order parameters, returned versus invoked callbacks, coalesced declarations, annotations, and local/generalized flow.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Call-result effect provenance follow-up

- [x] Extend symbolic value paths with call-result and field steps, without placing provenance in inferred types.
- [x] Resolve arbitrary call-result/projection chains during argument substitution while counting only invoked values as performed effects.
- [x] Cover producer/invoker flow, nested bounded chains, coalesced declaration identity, and obtaining effectful results without invoking them.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Bounded effect-provenance substitution follow-up

- [x] Store provenance as immutable shared DAG nodes so duplicate substitutions reuse replacement nodes instead of recursively cloning them.
- [x] Prune empty branches immediately, deduplicate sources, and bound symbolic paths and substitution work with deterministic endpoint-preserving omission.
- [x] Cover forty binary duplications for empty and exact operation origins, linear unique-node growth, branch sharing, and later projected invocation.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Effect-provenance budget hardening

- [x] Rewrite substitution and selection iteratively with explicit accounting for nodes, edges, callable entries, path steps, and deduplication.
- [x] Cache bounded exact-origin summaries for deterministic exhaustion fallback and remove unresolved sources naming the substituted parameter.
- [x] Destroy unique provenance chains iteratively and cover >4096-depth small-stack, wide/edge-heavy exhaustion, retained deep origins, and stale-parameter removal.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Effect-provenance callable-admission hardening

- [x] Stream replacement callable entries through work and path budgets instead of cloning an unmetered intermediate list.
- [x] Distinguish a genuinely missing selection from bounded traversal exhaustion and retain cached exact declaration endpoints on cutoff.
- [x] Cover a 100,000-entry callable replacement and PATH_BUDGET selection exhaustion with bounded allocation and exact-origin retention.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.

## Extern-boundary structural review follow-up

- [x] Build one semantic/source boundary walk with variable provenance keyed by exact lowered identities across alias and composed-row expansion.
- [x] Preserve exact callback label formulas and open-tail relations, and evaluate coverage alongside polymorphic boundary failures.
- [x] Aggregate failures by callback path and render every missing effect, tail, and condition with shared declaration facts only once.
- [x] Cover alias presence/tails, exact multi-variable spans, simultaneous failures, complete repairs, conditional prose, and result-spine deduplication.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy; commit without disturbing unrelated changes.

## Structured extern-boundary explanations

- [x] Carry exact callback effects, callback type/path/span, extern declaration facts, and declaration context through direct and solved coverage failures.
- [x] Aggregate every failing callback path and polymorphic leaf without changing the declaration-level diagnostic count.
- [x] Preserve each symbolic conditional coverage implication and remove first-constraint fallback selection.
- [x] Walk callback arrows, effect rows, row tails, and conditional presences iteratively with exact leaf kind/span/path.
- [x] Publish direct facts explicitly and retain real solver constraint/reason provenance for solved callback failures.
- [x] Use plain host/extern vocabulary and conditional, source-applicable repair advice in CLI and debugger output.
- [x] Cover later conditional callbacks, multiple issues, row/presence kinds, 30,000 arrows, and cause resolution.
- [x] Run formatting, the full workspace suite, workspace checks, and Clippy.
