# Mirror construction evidence validation

Implementation baseline: `e89d254182f779551e6aa0afbf2a41afed43e0f3`.

## Public behavior

- Inferred mirrors require finite construction and complete accessible constructors for potentially inhabited parts. Generic, curried, captured, hidden-package, and separately compiled consumer checks cover the propagated requirement.
- `TypeInfo` retains authenticated observation and exact equality without constructors. Forgetting a mirror's authority creates distinct evidence; foreign conversion cannot restore it.
- Interpreter and JavaScript consumer programs cover constructive views, deterministic finite defaults, recursive data, dynamic function packages, and codecs.
- Codec schemas retain impossible cases and original wire indices. Encoders use TypeInfo, skip validation only at proven-empty positions, and preserve derivation errors for unsupported live types. Decoders use constructive mirrors and reject impossible inputs.
- Artifact format `artifact-v2` rejects legacy artifacts and statically impossible construction evidence, including locally assembled descriptors. Unknown dynamic descriptors remain checked at the runtime intrinsic boundary.

## Standards review

Two optional clarity findings were addressed: both solvers share `Graph::dependency_index`, and construction fixed-point passes use named `Phase` variants. Follow-up reviews found no new standards or material clarity concerns, including the final row-context predicates and public consumer regressions. Final coverage results are recorded below; a passing suite does not establish the 100% line and branch requirement.

## Spec review

Two gaps were reproduced with failing regressions and fixed: codec derivation rejected unsupported structure in proven-impossible payloads, and artifact validation missed a descriptor assembled from concrete type arguments. Follow-up inspection found both resolved and no additional findings in those fixes. Full consumer validation additionally required preserving descriptive encoder derivation, avoiding invented construction predicates for unresolved callable ports, and retaining authentication on interpreter record-binding mirrors. Expanded parity testing also found and fixed JavaScript construction dropping nested sum payloads: the unit check now counts symbol keys. Concrete callable values retain impossible invocation requirements when stored or returned. Row extensions classify flattened fields under the enclosing constructor; separate record-row and sum-row predicates preserve that context through generic calls, compiled imports, and artifact descriptor assembly. Interpreter row instantiation now agrees with JavaScript for mixed fragments. Follow-up spec review found no remaining findings. The final workspace run also updated a debugger raw-artifact assertion to the intentional `artifact-v2` header.

## Checks

- `RUST_TEST_THREADS=2 just test artifact::`: 57 passed.
- `RUST_TEST_THREADS=2 just test construction`: 11 passed.
- `RUST_TEST_THREADS=2 just test runtime_type_stage_shows_invocation_ports_and_evaluation`: passed.
- Codec impossible-payload consumer: passed with interpreter/JavaScript agreement.
- Explicit function codec registry and forgotten-authority consumers: passed with interpreter/JavaScript agreement.
- `just clippy`: passed.
- Reflection chapter's Ruddy examples concatenated into a temporary consumer and compiled using CLI `ruddy check`: passed.
- Complete primitive/unit, recursive record, and raw mutually recursive variant construction: passed with interpreter/JavaScript agreement.
- `just fmt-check`, documentation generation, and `npm --prefix docs run build`: passed.
- Final workspace suite and combined coverage: running in five disjoint groups (the LSP group uses one test thread) through `just test`, preserving the per-process 4 GiB and 30-minute bounds.

## Existing limitation encountered

A nested match on mutually recursive variants can overflow the unchanged pattern usefulness checker in `src/patterns.rs`. The construction regression observes the generated value through public structural display instead; construction and both backends succeed. This feature does not change the pattern checker or claim arbitrary traversal termination.
