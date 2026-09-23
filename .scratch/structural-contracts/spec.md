# Structural contracts

Status: implemented
Resolution: policy correction completed, 2026-09-22

The first implementation and its validation are recorded below as historical
work. The user subsequently rejected general unions and
input-shape-dependent ordinary result types. The completed correction keeps
conditional fields and variants while enforcing one compatible ordinary
result type across every reachable branch.

## Objective

Infer readable structural contracts from ordinary unannotated Ruddy code, and
make those contracts govern checking and application. This is a semantic
extension, not a display-only rendering of existing presence schemes.

The branch was rebased onto `origin/main` at `fd50b2e` before implementation.
The user authorized planning and executing the full rework.

## Required behavior

- Infer the five functions in `~/code/hc/img/src/main.rud` without annotations,
  including the final swizzle over empty channel buffers. Preserve the actual
  source, manifest, lock, and dependency when validating that project.
- Preserve input/output relationships involving conditional fields and tagged
  variants. Ordinary branch result types must remain compatible at definition
  time, including every shared field and tag payload position.
- Reject `fn | #Natural => 1n | #Text => "hello"` even if it is never called or
  every known call supplies a singleton selector. An annotation, wrapper,
  partial application, or imported contract cannot bypass that rule.
- Permit different payload types under different field names or different
  tags, as ordinary row typing already does. Do not allow one shared field or
  tag to acquire incompatible payload types depending on an input shape.
- Select image channel fields with a common ordinary payload type. Add or
  replace a selected channel and preserve unrelated fields, subject to the
  same payload consistency rules as other branches.
- Pad tuples of zero through four tagged channel values with nullary `#None`.
  The occupied slots may carry compatible sum types; padding does not promise
  to accept bare `Nat`, `String`, or other types incompatible with `#None`.
- Preserve the actual no-op branches of `try_get_set`, including on `{}`, while
  retaining all ordinary constraints established by the complete function.
- Compose the four sequential swizzle operations without expanding a table of
  all selector combinations.
- Check every reachable case of a broad selector. Join only compatible ordinary
  result types and retain conditional presence relationships; never create an
  untagged union of incompatible results.
- Preserve contracts and their restrictions across aliases, partial
  applications, closures, higher-order applications, record fields, and
  compiled dependency interfaces, with no required user annotations.
- Provide writable structural annotations, useful hovers and documentation,
  formatting, editor grammar support, and debugger visibility. Keep named
  pattern binders and the single-argument `fn | pattern => result` shorthand.
- Reject invalid uncalled function bodies and incorrect annotations. Preserve
  effects, existential ownership, capture identity, and ordinary recursion.

## Design constraints

Automatic inference is mandatory. The user is willing to relax guaranteed
termination, but has not authorized replacing inference with required
annotations. General untagged union types are outside the language design:
there is no `Nat or String`, `Nat | String`, or hidden semantic equivalent.
The word `or` remains available in Boolean expressions and presence formulas.

Contracts describe structural computations over arguments, captured types,
field selection/update, tuple/tag construction, ordered shape matches, and
function application. They are first-class semantic types. Runtime
representation is tracked separately; erasure must not affect checking. Shared
ordinary payload constraints are part of a function's checked type and must
remain in force when a contract selects a more precise shape for a call.

The current implementation uses finite graphs, symbolic unknown inputs,
bounded normalization, and active-contract detection. It does not unfold
recursive value bodies. These are current implementation boundaries, not a
user requirement that every future inference algorithm have a termination
proof. A work limit must report unfinished or rejected checking; it must not
silently accept an unchecked contract or discard its obligations. Shared
representations and bounded presentation work remain required for interactive
use.

Ordinary types, row/presence inference, and existing annotations remain valid.
Presence variables may remain internal. This correction does not require a
replacement presence solver or new recursive type execution. Every change in
accepted programs must follow the structural contract rules; there must be no
special cases for image-library function names.

## Correction plan

1. Remove general whole-type alternatives from parsing, semantic types,
   traversals, checking, artifacts, runtime representation, printers, debugger,
   editor grammar, and public documentation.
2. Restore ordinary branch result and shared-payload consistency during
   inference, while retaining structural presence relationships and precise
   application for the supported record/tag cases.
3. Preserve writable named patterns and match-function shorthand. Ensure
   annotations, higher-order calls, partial applications, and imported
   contracts retain the same restrictions as inferred definitions.
4. Replace union acceptance regressions with incompatible-result rejection
   coverage and compatible conditional-field/tagged-variant cases. Use tagged
   values in padding tests, matching the actual image code.
5. Validate the actual image project, core inference, annotations and hovers,
   exported interfaces, and interpreter/JavaScript behavior. Run required
   checks, then record this correction's results separately from the earlier
   implementation's historical validation.

## Validation interfaces

Use the existing public compiler, inference, analysis/hover, artifact, formatter,
and interpreter/JavaScript test interfaces in `ruddy-tests`. Regression tests
must demonstrate stronger accepted examples and unsafe rejected examples.
Run every Rust test exclusively through `just test`. Do not invoke `cargo test`
directly. Run `just grammar` for grammar changes.

## Correction result

- Compatible ordinary results are required in every function definition.
  General whole-type alternatives have been removed from syntax, semantic and
  runtime types, artifacts, presentation, editor support, and documentation.
- Structural summaries retain conditional field and tag relationships, named
  patterns, match arms, and the single-argument `fn |` shorthand. They compose
  through wrappers, partial applications, closures, higher-order calls, record
  fields, and imported interfaces without weakening ordinary payload checks.
- Regressions reject input-dependent ordinary results at definition time and
  cover conditional fields and variants, annotation round trips, hovers,
  artifacts, LIR lowering, the interpreter, JavaScript, host exports, and the
  five image operations.
- Validation passed the complete inference, structural-contract, artifact,
  compile, interpreter, LIR, JavaScript, host-export, UI, stage, pattern,
  parser, printer, analysis, and compiler-library partitions. The regenerated
  editor grammar passed 158 corpus cases and nine highlighting suites.
  Formatting and Clippy completed; Clippy reports non-fatal size warnings for
  the existing large error and syntax enums, plus an unrelated conversion
  warning in an analysis test.

## Historical implementation progress before this correction

The entries in this section describe the earlier rework. Independent ordinary
branch payloads and whole-type alternatives mentioned here are superseded by
this correction's required behavior.


- Rebase completed cleanly.
- Architecture survey identified eager arrow application, common branch
  payload unification, artifact transport, and callable lowering as required
  integration points.
- Implemented semantic contracts and whole-type alternatives, compositional
  synthesis, bounded application, writable annotations, shared presentation,
  artifact transport, debugger views, and editor grammar.
- Parser/formatter, hover, serialized dependency signature, and documentation
  example regressions pass. Interpreter and Node callback forwarding now
  preserve effect and reflection arguments.
- Structural arms now check input payload variables independently after finite
  synthesis preflight. Exact record shapes can use the same field as a scalar
  in one arm and a function in another. Shared row/presence identities and rigid
  annotation variables retain their ordinary meaning; semantic application
  checks the requirements of every reachable arm.
- A wildcard retains the constructor families required by sibling patterns,
  including nested record fields and tag payloads. Structural pattern matching
  does not introduce a runtime discriminator between records, sums, and scalars.
- Normalization uses an explicit continuation stack, bounded work, per-graph
  validation, and active-contract detection. Long chains of finite summaries
  no longer consume the host call stack.
- Focused payload, constructor-family, correlated-arm, core, artifact, syntax,
  and interpreter/Node regressions pass. The source capture-growth regression
  reports a definition-anchored complexity error and prevents dependent
  amplification; the standard scaffold check passes with the same limit.
  Standard-library, native export, interpreter, and JavaScript compatibility
  partitions pass.
- The actual `../img` project passes the latest CLI check with its pinned
  standard-library dependency. Validation used an exact temporary snapshot;
  original manifest, lock, and source hashes stayed unchanged and temporary
  project/cache files were removed. A full public-compiler regression covers
  all five operations and the final swizzle over empty arrays, including
  pattern checks and artifact validation. This exposed and fixed reachability
  treating a structural function's closed representation fallback as its
  complete input domain: wildcard alternatives now remain reachable while
  duplicate arms and unrelated closed-value checks retain their diagnostics.
- Compatibility and soundness review added regressions for symbolic residual
  annotation escapes, captured presence requirements, recursive annotated
  groups, callback convention provenance, and native export permissions.
- Recursive alias conformance retains the ordinary solver's open equality
  assumptions. Capture and returned-type obligations use its heap work stack,
  including an effect-check continuation. Both contract/contract and
  arrow/contract chains of 1,024 aliases pass on a 256 KB thread stack; divergent
  payloads, divergent effects, and recursive contract execution still reject.

## Retained implementation boundaries

- A contract is a finite expression graph, not an arbitrary source program.
  Each graph has size/depth bounds; normalization has a decreasing global work
  budget and rejects re-entry into an active contract. Evaluation uses a heap
  work stack rather than a cross-call depth limit. It never unfolds value
  definitions.
- Ordinary checked arrows remain the representation witness and validate
  unsummarized code. An unresolved written contract cannot use an unknown
  representation witness as proof of its promised type.
- A deferred structural call must be retained by an enclosing summary; an
  operation that cannot retain its requirements rejects the deferred call.
- Captured functions with presence clauses require their full obligations to
  survive, including an outer clause on a written structural contract.
  Type-only capture synthesis declines any nontrivial scheme formula and
  retains ordinary checking; an unresolved deferred call then rejects rather
  than discarding the clause.
- A structural summary can be published only after its function body has
  passed ordinary payload consistency checks. Unsupported wrappers and
  recursive groups retain ordinary checking; an unsupported operation cannot
  silently discard a deferred structural requirement.
- An ordinary representation arrow does not by itself prove the complete
  structural contract. A flexible higher-order domain may be refined from a
  known input shape, but the complete contract must establish the promised
  result and effects without discarding shared ordinary payload constraints.
- Captured semantic graphs are bounded independently: a finite local expression
  graph alone does not bound expanded captured types.
  Top-level and local publication reject structural capture graphs above 8,192
  unique type allocations or 32,768 edges, counting fallbacks and delayed
  arguments as well as captures. Their smaller limit bounds the instantiation
  and source-evidence metadata carried alongside these graphs. Ordinary types
  retain the 131,072-node / 262,144-edge allowance. Rejected bindings publish
  explicit error recovery to stop dependent definitions amplifying the rejected
  graph. Independently quantified
  copies are never merged merely because their printed forms are alike.

- Diagnostic publication retains at most 4,096 canonical semantic paths with
  explicit omitted-subtree evidence. These stable routes keep cached caller
  explanations aligned with fresh inference even when producer allocation
  sharing changes; the complete semantic type graph remains shared and intact.

- Extern annotation scopes close after callback coverage: solved anonymous
  rows/effects are preserved and remaining holes are quantified before an
  independent caller table opens them. Representation-only structural
  parameters are closed separately from the semantic capture obligations.

- Native JavaScript exports use a concrete ordinary calling shape only after
  direct conformance checking against the semantic contract. The proof must
  preserve universally offered presence choices. Structural exports with
  optional incoming record fields are conservatively rejected because native
  callers choose those fields independently; a concrete ordinary annotation
  provides an explicit supported boundary. Exact reflection identities retain
  contracts rather than their runtime representation.

## Historical validation before the policy correction

These results validate the earlier implementation, including behavior now being
removed. They do not establish that the current correction is complete. Its
final validation results must be recorded separately when the current checks
finish.

Rust tests run exclusively through `just test`, in memory-limited batches.
The integration-test inventory is audited against the union of those batches;
ignored subprocess entry points are exercised by their parent tests.

- Complete inventory: 2,238 integration tests passed and ten subprocess entry
  points were intentionally ignored by the top-level harness and exercised by
  their parent tests. That completed inventory belongs to the earlier build.
- Latest core conformance/inference/syntax/pattern partition: 545 passed.
- Native exports, JavaScript, and host adapters: 84 passed.
- Editor and standard-library partition: 40 passed; tail-recursion editor
  coverage separately passed all 15 tests.
- Interpreter coverage passed, including image-style structural operations,
  effects, reflection, persistent collections, and numeric domains.
- Workspace tests outside the integration crate: 88 passed.
- `just fmt-check`, `just clippy`, and `git diff --check` pass. Clippy reports
  nonfatal size advisories and the existing test conversion warning.
- `just grammar`: 155 corpus cases and all eight highlighting suites passed.
- Documentation build and all 16 documentation tests passed.
- The final frozen binary checks the exact `~/code/hc/img` snapshot successfully
  with its original manifest, lock, and pinned dependency. All original file
  hashes remain unchanged, and the temporary snapshot/cache were removed.
- Final frozen-build CLI, Git transport, and standard-library snapshot batch:
  99 passed, nine subprocess entry points ignored; no failures. These checks
  ran after all Rust builds finished so child processes could reliably execute
  the same test binary.
