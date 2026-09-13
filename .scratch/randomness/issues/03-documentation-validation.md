# Randomness documentation and complete contract verification

Status: resolved
Blocked by: 01, 02

Finish the module's `@doc` text and standard-library/platform documentation
following [the spec](../spec.md). Show an ambient draw, a repeatable seeded
computation, nested parent-seeded `local` scopes, and a custom handler supplying
the primitive operations. Explain seed allocation before scheduling tasks and
why a callback must establish its handler when invoked. Distinguish isolation
after seeding from scheduling-dependent assignment of seeds.

Check all acceptance criteria, especially exact cross-backend sequences,
unrelated-effect forwarding, source-quality limits, and the noncryptographic
scope. Compile examples, run `just test`, relevant format checks, and the docs
build. Record actual validation results; do not introduce statistical pass/fail
tests or broaden this feature into secure randomness.

## Answer

Published the module reference, library index entry, and platform examples for
ambient, seeded, local, custom-handler, and prepared concurrent use. Examples
compile; docs tests and build pass. The full workspace test suite passes.
All instrumented changed compiler lines and branches are covered; aggregate
compiler coverage remains below the repository target. See the exact results
and separate standards/spec findings in [review](../review.md).
