# Caller-choice diagnostic repair plan

- [x] Distinguish closing a caller-chosen effect remainder from restricting it with a specific performed operation.
- [x] Offer only repairs applicable to the effect-row failure: preserve/remove the remainder for closure, handle/list the operation for restriction.
- [x] Replace invalid destination-ownership advice for escaping rigids with source-annotation or value-flow repairs.
- [x] Collapse duplicate escape destination labels into one grounded fact naming the destination binding and source-level inferred type.
- [x] Add exact closure/restriction, escape-repair validity, and one-label/deep-type regressions and update structured goldens.
- [x] Run formatting, the full workspace suite, checks, and Clippy; commit without disturbing unrelated changes.
