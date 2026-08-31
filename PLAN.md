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
