# 17 Symbolic artifacts and distinct host identities

Status: open
Type: task
Blocked by: 07, 09

Two places where a decision is made earlier than the spec asks.

Spec: "Portable artifacts retain symbolic target primitives until binding them
once to a target; specialized artifacts record their concrete domains."
`types::Domains` has no symbolic case and defaults to the 53-bit domain, so a
library compiled without a target is already specialized and the linker
refuses it under a 32- or 64-bit root.

Spec step 6: "Compiler-owned opaque host-type identities, initially for JS
values, with distinct identities available to native/Wasm handle adapters as
those integrations are implemented. Today's single `ForeignValue` cannot
express distinct environments using aliases alone." There is one
`Ty::ForeignValue`, and `std::js::Value` is an alias of it. That is correct
while JavaScript is the only host with an integration, and it is what blocks
a second one: a `wasm::Resource` alias would be the same type as
`js::Value`.

Artifact equality of two hidden types also compares their binder numbers
directly, where `types.rs` says "the number itself is no part of a type's
identity"; two alpha-equivalent hidden types from different bundles can
therefore disagree.
