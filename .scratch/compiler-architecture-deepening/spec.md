# Compiler Architecture Deepening

Status: ready-for-agent

## Problem Statement

Ruddy's compiler phases are individually substantial, but several high-change concepts are published through shallow or leaking seams. Inference returns one wide result containing accepted semantic facts, pattern facts, errors, and complete solver replay data. Semantic schemes crossing the artifact seam repeat validation, conversion, recovery, package ownership, and stack-safety knowledge in several places. Extern declarations are independently interpreted for inference diagnostics and LIR adapter construction. Finally, callers must manually preserve phase ordering and keep the program, inference result, pattern result, LIR, and artifact inputs coherent.

This spreads knowledge across callers and reduces locality. Changes to existential packages, extern ABIs, or inference diagnostics repeatedly require coordinated edits across several modules and test harnesses. Publicly mutable intermediate values also allow callers to construct combinations that violate lowering assumptions, while routine integration tests duplicate the same compiler orchestration.

## Solution

Deepen four areas of the compiler in independently verified refactors:

1. Deepen inference publication by separating accepted semantic facts from a supported, read-only diagnostic view.
2. Deepen semantic artifact translation under the artifact module, with explicit unchecked and validated states plus strict validation and tolerant recovery.
3. Deepen extern interoperability with target-neutral reviewed facts and an in-memory adapter plan produced after inference.
4. Deepen core compilation around partial and accepted states, owning the parsed-bundle-to-artifact sequence while retaining direct access to individual phases.

The resulting interfaces must make invalid lowering calls unrepresentable, retain useful partial results after errors, keep rich errors available without requiring a complete trace, and let the debugger opt into detailed compiler tracing. Linking and backends must consume only validated artifacts. JavaScript-specific target validation remains in the JavaScript adapter.

## User Stories

1. As a compiler caller, I want to compile a parsed bundle through one core interface, so that I do not need to reproduce phase ordering.
2. As a compiler caller, I want successful compilation to produce an explicitly accepted program, so that I cannot accidentally lower invalid input.
3. As a compiler caller, I want failed compilation to return partial results, so that I can inspect every phase that completed.
4. As a compiler caller, I want structured errors from every completed checking phase, so that I can present all useful failures in one run.
5. As a CLI maintainer, I want filesystem discovery to remain outside core compilation, so that filesystem policy does not leak into compiler semantics.
6. As a CLI maintainer, I want core compilation to stop at a validated target-neutral artifact, so that target generation remains independently selectable.
7. As a debugger maintainer, I want individual compiler phases to remain publicly callable, so that the debugger can inspect intermediate representations.
8. As a debugger maintainer, I want to opt into a complete diagnostic trace, so that constraints, solver steps, reasons, variables, and refinements remain inspectable.
9. As a normal compiler caller, I want complete tracing to be optional, so that ordinary compilation does not retain unnecessary replay data.
10. As a language user, I want rich causal error explanations even when tracing is disabled, so that lower memory use does not degrade diagnostics.
11. As a tooling author, I want inference diagnostics exposed through a read-only structured view, so that tools can analyze them without mutating inference state.
12. As a lowering author, I want only accepted semantic facts at the LIR seam, so that lowering does not need defensive checks for ordinary callers.
13. As an artifact producer, I want an explicit unchecked artifact representation, so that construction and decoding do not falsely claim validity.
14. As an artifact consumer, I want a validated artifact representation, so that linking and backend generation can rely on its invariants.
15. As a dependency consumer, I want malformed manually constructed artifacts to receive tolerant recovery, so that invalid foreign state cannot crash trusted compiler paths.
16. As a dependency consumer, I want recovery facts published through the diagnostic seam, so that repairs are visible without becoming compilation errors.
17. As an artifact reader, I want textual artifacts validated strictly, so that malformed persistent data is rejected rather than silently accepted.
18. As a compiler maintainer, I want package ownership and bound-position rules localized in semantic artifact translation, so that import, export, and validation agree.
19. As a compiler maintainer, I want deep semantic trees handled with one stack-safe discipline, so that each adapter does not invent a separate traversal policy.
20. As an extern author, I want target-neutral ABI errors reported during inference, so that source-level failures retain precise semantic explanations.
21. As an extern author, I want callback capability checks and lowering to share reviewed facts, so that an accepted extern cannot produce a contradictory adapter plan.
22. As a lowering author, I want an infallible target-neutral extern plan for accepted input, so that ABI proof is not repeated in LIR.
23. As a backend author, I want the extern plan absorbed into LIR rather than serialized separately, so that the artifact has one executable meaning.
24. As a JavaScript backend author, I want JavaScript snippet validation to remain in the JavaScript adapter, so that target syntax does not leak into inference.
25. As a future backend author, I want target-neutral compiler artifacts, so that adding a target does not require JavaScript knowledge in shared modules.
26. As a test author, I want integration tests to use the same compilation interface as production callers, so that tests exercise the real orchestration seam.
27. As a test author, I want phase-specific tests to call their phase directly, so that focused semantic behavior remains easy to isolate.
28. As a test author, I want mutation hooks for defensive LIR cases to be crate-private, so that unusual test setup does not weaken the public interface.
29. As a maintainer, I want redundant tests of shallow orchestration removed once higher-seam tests exist, so that internal refactors do not cause unrelated test churn.
30. As a downstream Rust caller, I want architecture-driven interface changes made deliberately, so that the compiler gains depth without gratuitous behavioral changes.
31. As a maintainer, I want each architectural refactor independently testable and revertible, so that failures can be isolated to one seam.
32. As an AI coding agent, I want semantic ownership and acceptance invariants concentrated in named modules, so that relevant behavior can be found without traversing unrelated compiler internals.

## Implementation Decisions

- The work is delivered as four separate refactors in this order: inference publication, semantic artifact translation, extern interoperability, then accepted-program compilation.
- Rust-facing public interfaces may change where needed to create meaningful depth. Compatibility forwarding that preserves a shallow interface is not required.
- Existing language behavior and persistent artifact meaning should remain stable. Small diagnostic, snapshot, or presentation changes are acceptable when they are a reasonable consequence of the new ownership.
- The compile module owns parsed-bundle-to-artifact orchestration, aggregated checking errors, partial compilation state, and accepted compilation state.
- Filesystem discovery and project loading remain driver concerns outside the compile module.
- Backend generation remains outside the compile interface. Core compilation ends with a validated target-neutral artifact.
- `PartialCompilation` is the canonical term for a failed or not-yet-accepted compilation carrying all completed phase results and structured errors.
- `AcceptedProgram` is the canonical term for the state proving that IR building, inference, and pattern checking produced no errors.
- The compile module defines `AcceptedProgram`, because no individual checking phase can establish the complete invariant.
- LIR lowering accepts only `AcceptedProgram`. Artifact construction receives coherent accepted state and lowered LIR rather than unrelated mutable values.
- Individual lexing, parsing, IR, inference, and pattern modules remain publicly callable for debugger, tooling, and phase-specific test use.
- Inference returns partial semantic information even when it reports errors.
- Inference publication separates semantic facts from diagnostic information rather than exposing one broad mutable data structure.
- `DiagnosticView` is the canonical term for the supported read-only structured diagnostic interface.
- User-facing structured errors and their causal explanations are always retained.
- Complete solver replay data is retained only when callers opt into one complete trace mode. Fine-grained trace categories are deferred until multiple real callers require them.
- The diagnostic seam continues to support constraints, solver steps, reasons, variables, presence refinements, and artifact recovery facts.
- Internal numbering, ordering, zonking, package publication, and reason-graph construction stay within the inference publication implementation.
- Tests that need to synthesize solver identifiers or mutate otherwise coherent inference state use crate-private test support.
- The artifact module owns the persistence seam for semantic schemes.
- `UncheckedArtifact` is the canonical term for portable artifact data that has not established compiler invariants.
- `Artifact` is the canonical term for validated artifact data that linking and backends may trust.
- Parsing and manual portable construction produce unchecked data. They do not implicitly claim validity.
- Strict validation and tolerant recovery both produce the same validated artifact type when successful.
- Strict textual parsing rejects malformed persistent data.
- Tolerant dependency import repairs malformed manually constructed data where safe, preserving current recovery behavior.
- `RecoveryFact` is the canonical term for a structured description of tolerant artifact repair.
- Recovery facts are always available through the diagnostic seam and do not become compilation errors by themselves.
- Semantic artifact translation owns structural validation, bound clamping, recovery, package-preorder ownership, formula ownership, absent-payload treatment, and stack-safe traversal.
- In-memory semantic types own semantic meaning only; persistence-specific validation and recovery do not move into the types module.
- Export and import remain genuine adapters at the artifact seam, backed by one deep semantic translation implementation.
- Inference owns target-neutral extern admissibility checks, including representation admissibility, ABI compatibility, and callback capability coverage.
- Accepted semantic publication carries reviewed target-neutral extern facts internally so later phases do not repeat inference walks.
- A distinct post-inference extern module transforms reviewed facts into a target-neutral in-memory adapter plan.
- `ExternPlan` is the canonical term for that plan.
- Extern planning is infallible for accepted target-neutral facts. Target adapters may still reject target-specific text.
- The extern plan is absorbed during LIR lowering and is not added to the artifact format.
- JavaScript expression validation remains in the JavaScript backend adapter.
- No validator is injected into inference. A separate injected validation seam would be hypothetical with only one current JavaScript parser adapter.
- The four refactors should deepen existing concepts without introducing pass-through modules. Apply the deletion test during implementation: removing a new module should force its complexity back across multiple callers.

## Testing Decisions

- The highest test seam is the core compilation interface from parsed bundle to validated artifact. Integration behavior should be tested there whenever phase internals are not the subject.
- Compilation tests cover successful production of `AcceptedProgram` and validated artifacts.
- Compilation tests cover failures that preserve `PartialCompilation`, all completed phase results, and aggregated structured errors.
- Compilation tests prove that LIR cannot be reached through the public interface without accepted state.
- Compilation tests cover tracing disabled and enabled, verifying that causal errors remain rich in both modes while complete replay data appears only when requested.
- Inference publication tests exercise the semantic publication interface and read-only `DiagnosticView`, rather than directly coordinating public maps.
- Existing inference tests provide prior art for inferred schemes, constraints, solver steps, reason ancestry, causal explanations, existential packages, and extern callback coverage.
- Debugger stage and snapshot tests provide prior art for distinct type, constraint, solve, and presence views. They should verify that each view reads only its supported diagnostic data.
- Semantic artifact tests use the artifact validation and recovery seam as their primary surface.
- Artifact tests cover valid semantic round trips, canonical text, deeply nested stack-safe data, strict rejection, tolerant recovery, package ownership, bound positions, formulas, row tails, and absent payloads.
- Existing artifact round-trip and malformed-input tests provide prior art, but hand-built mirrored tree tests should move behind the unchecked-to-validated seam.
- Dependency-import tests provide prior art for tolerant semantic recovery and imported effect identity behavior.
- Recovery tests assert structured `RecoveryFact` values as well as the resulting validated semantic meaning.
- Extern integration tests pair inference rejection or acceptance with the resulting LIR adapter behavior through the same reviewed-facts seam.
- Existing inference tests provide prior art for representation polymorphism, callback effect coverage, and causal extern errors.
- Existing LIR tests provide prior art for raw calls, host-to-Ruddy conversion, Ruddy-to-host conversion, callback evidence, recursive adapter handling, and adapter identity.
- JavaScript backend tests continue to cover valid and invalid target expressions at the backend seam.
- Phase-specific tests may continue using public phase interfaces where the phase behavior itself is under test.
- Defensive tests that intentionally violate accepted-state internals use crate-private support and do not establish public mutation requirements.
- Duplicated lex-parse-build-infer-check-lower orchestration in integration tests is replaced by the compile interface. Equivalent shallow test helpers are removed rather than retained alongside it.
- Tests must assert observable behavior through a module's interface. They should not depend on internal field layout, traversal strategy, helper call order, or private seams.
- The Rust test suite is run only through the repository's `just test` recipe.
- Each of the four refactors must pass its relevant focused checks and the complete `just test` suite before the next refactor begins.

## Out of Scope

- Changing Ruddy language semantics.
- Redesigning the parser, type system, pattern analysis, LIR instruction set, linker, or JavaScript generation beyond changes required by the new seams.
- Moving filesystem discovery, manifest loading, dependency resolution, or project installation into the core compile module.
- Including backend generation in core compilation.
- Serializing `ExternPlan` into artifacts.
- Moving JavaScript syntax validation into inference or the target-neutral extern module.
- Introducing dependency injection for a single JavaScript validation adapter.
- Adding fine-grained trace category selection before multiple real callers require it.
- Removing public access to individual compiler phases.
- Making tolerant artifact recovery fatal.
- Preserving public mutation of accepted compiler state.
- Adding compatibility forwarding solely to retain the existing shallow interfaces.
- Creating or revising domain glossary terms; the agreed names describe code architecture rather than the Ruddy language domain.
- Combining the four refactors into one indivisible rewrite.

## Further Notes

- Recent development is concentrated in inference, IR, semantic types, artifacts, and LIR. Existential ownership, extern ABI work, and causal diagnostics repeatedly changed several of these areas together, making them the highest-leverage scope for deepening.
- The inference publication refactor comes first because later artifact and extern work should consume a smaller semantic interface rather than the current broad result.
- Semantic artifact translation comes second because it centralizes persistent semantic invariants before extern plans and accepted compilation depend on validated artifacts.
- Extern interoperability comes third so accepted compilation can consume the final target-neutral plan shape.
- Accepted-program compilation comes last and consolidates the stabilized interfaces created by the first three refactors.
- No existing domain glossary or recorded architecture decision constrains this work.
