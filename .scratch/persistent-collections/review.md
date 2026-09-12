# Persistent collections review

Fixed base: `6f850b107ca5978b5a4462a6ce0491d7e269c0a0`.
Reviewed the implementation, public tests, and documentation in the working
tree against `spec.md`, repository standards, and the code-review smell baseline.

## Standards

No actionable findings from the initial independent review or its follow-up.
Tests exercise public interfaces and use an independent `BTreeMap` reference.
Shared HAMT storage avoids separate map and set implementations. No compiler,
grammar, or debugger changes are involved.

## Spec

The initial review found one P2 validation gap: shared hash-prefix tests did
not exercise genuine full-hash collision buckets. Added an unequal string pair
whose full hashes match, verified through the public hashing interface in both
runtimes. The test covers lookup, replacement, deletion of either bucket entry,
persistence, content equality, branch compaction, and set algebra.

The follow-up review closed that finding and reported no remaining spec gaps.
Intersection and difference retain the original left collection and remove
excluded entries, preserving unchanged paths and reusing cached hashes.

Standards: 0 findings. Spec: 0 remaining findings; one P2 resolved.

## Validation

- Public map/set tests run through `just test`, comparing JavaScript and the
  reference interpreter. Include forgiving/strict errors, nested bulk error
  paths, unrestricted values, callback effects, persistence, traversal, algebra,
  shared hash prefixes, full collisions, and mixed updates against a Rust model.
- `target/debug/ruddy check`, `just fmt-check`, and `just clippy` passed.
- Generated public API documentation with `target/debug/ruddy doc`.
- A separate JavaScript run with 128 initial entries and 256 mixed updates
  matched an independent model for sorted keys, values, size, the retained
  snapshot, and complete deletion. The committed cross-runtime regression uses
  32 initial entries and 64 mixed updates to bound reference-interpreter cost.
  The earlier oversized focused/full runs were stopped and superseded by a
  final full run of the committed workload.
- Final `just test` passed across the workspace: 2,028 passed, 10 ignored,
  zero failures. The main test crate reported 1,955 passed and 10 ignored
  in 305.95 seconds; all six new collection tests passed in that run.
