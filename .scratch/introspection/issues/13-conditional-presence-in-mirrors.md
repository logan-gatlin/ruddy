# 13 Conditional presence through mirrors and builders

Status: open
Type: task
Blocked by: 03, 04

Spec: "Descriptions include conditional field presence, region and presence
dependencies"; acceptance: "Joint conditional presences ... Builders validate
the joint presence formula."

`src/reification.rs` rejects a type whose presence is not settled, so
`std::reflect::Presence`'s `#Optional` case is never produced, a field view's
`read` always answers `#Some`, and `BuildError` has no case for a presence
that was not satisfied. `src/reification/interface.rs` still excludes presence
parameters from ordinary descriptor demands, which the spec's section 5 asked
to change, and `conventions::Shape` gained no presence slot.

The same gap covers a sum case's admission: `SomeCase.inject` is an `Option`
that is always `#Some`, where the spec says "A case exposes a total injector
only when its admission is proved ... Otherwise `inject` is absent."
