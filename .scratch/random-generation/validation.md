# Inferred random generation validation

Baseline: `f27772c` (constructive Mirror implementation).

## Behavior

- `random::random ()` returns the inferred type directly and forwards its
  construction requirement to callers.
- `random::generate budget mirror` uses explicit Mirror evidence and a shared
  expansion budget. Zero and exhausted positions use finite pure construction.
- Consumer programs cover all primitive kinds, exact integer domains, records,
  arrays, empty-only arrays, impossible sum payloads, recursive budget exhaustion,
  seeded repeatability, local stream isolation, and missing construction evidence.
- Existing fallible sampling helpers and primitive Random operations are unchanged.

## Standards

No documented-standard violations or material smell findings. Changes are
restricted to the standard library, public consumer tests, and documentation;
there is no compiler, grammar, or debugger change. The reviewer noted the pending
reference-page refresh; that page now documents both new functions.

## Spec

No actionable spec findings or scope creep. The reviewer suggested a local-stream
isolation regression; it was added to the structural consumer test.

## Checks

- Direct inferred-return consumer: red for the missing API, then green on both backends.
- Integer consumer: red for default-only generation, then green at 32/53-bit target
  domains on both backends and the 64-bit target domain in the interpreter.
- Structural consumer: red for default-only fields and recursion, then green for
  typed construction and shared-budget termination.
- Compile failures for empty, callable, and unsupported array types, and for
  TypeInfo passed in place of Mirror: passed.
- `just fmt`, `just clippy`, and reference generation (`ruddy doc`): passed.
- Documentation tests: 16 passed; site build passed.
- The new documentation example compiled as a public consumer.
- Full workspace suite: 2,065 passed, 10 ignored fixtures, zero failures. All 2,075
  listed tests were accounted for across disjoint `just test` invocations,
  preserving the repository's per-process memory and time bounds. The two
  expensive editor tests passed in separate processes after the other groups.
- The standard-library runtime matrix passed on Node and web with both 32-bit
  and 53-bit integer domains.

Local run logs: `/tmp/ruddy-generation-group-{0,1,2,3}.log`,
`/tmp/ruddy-generation-editor-{0,1}.log`,
`/tmp/ruddy-generation-doc-tests.log`, `/tmp/ruddy-generation-doc-build.log`,
and `/tmp/ruddy-generation-doc-example.log`.

Review outcome: Standards — no findings; Spec — no findings remaining.
