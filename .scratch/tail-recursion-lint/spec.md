# Tail-recursion opportunities and LSP quick fixes

Status: ready-for-agent

## Problem Statement

Ruddy programmers cannot readily see which recursive functions retain work after recursive calls but could use a tail-recursive accumulator. Recognizing and rewriting these functions manually requires understanding evaluation order, effect rows, arithmetic semantics, binding identity, and the compiler's continuation behavior.

Ruddy already lowers tail-position calls by forwarding their continuation, and its runtime drives Ruddy transfers through a loop. The opportunity is to eliminate avoidable pending continuations, not to promise that ordinary recursion otherwise overflows the host call stack. A source rewrite should yield a real tail-recursive computation rather than move the same chain of deferred work into closures.

## Solution

Publish an informational LSP diagnostic, “This function can be rewritten as tail recursion,” for supported direct self-recursive accumulator patterns. Offer an associated quick fix titled “Convert to tail recursion.” Applying it introduces a local accumulator helper and preserves the original callable interface.

Only report an opportunity when the implementation can produce an edit that eliminates the recognized post-recursive work while preserving bindings, types, and the required effect ordering. This is an optional, user-applied refactoring: numeric boundary and rounding differences from regrouping do not disqualify a candidate, and the user decides whether to apply it. Unsupported recursion patterns remain unchanged. Already-tail-recursive functions receive no diagnostic.

## User Stories

1. As a Ruddy programmer, I want supported tail-recursion opportunities highlighted in my editor, so that I can find avoidable pending continuations.
2. As a Ruddy programmer, I want the diagnostic to explain the remaining work after recursion, so that I understand the proposed improvement.
3. As a Ruddy programmer, I want an informational diagnostic, so that an optional optimization does not make valid code fail compilation.
4. As a Ruddy programmer, I want a quick fix attached to the diagnostic, so that I can perform the supported rewrite without implementing an accumulator manually.
5. As a Ruddy programmer, I want to inspect the proposed source edit before applying it, so that I retain control of the refactoring.
6. As a library author, I want the original function name, parameters, visibility, and callable type preserved, so that callers need no edits.
7. As a Ruddy programmer, I want base cases preserved, so that empty and terminating inputs retain their results.
8. As a Ruddy programmer, I want every recognized recursive branch converted consistently, so that the helper does not retain the original chain of deferred work.
9. As a Ruddy programmer, I want generated bindings to avoid collisions and capture, so that local names do not change program meaning.
10. As a Ruddy programmer, I want comments and unrelated source preserved, so that the quick fix remains easy to review.
11. As a Ruddy programmer, I want empty, closed effect rows accepted as proof that evaluation may be reordered, so that safe rewrites are not rejected unnecessarily.
12. As a Ruddy programmer, I want open effect rows treated as insufficient proof of reorderability, so that effect-polymorphic code is not silently changed unsafely.
13. As a Ruddy programmer, I want the effects of evaluating expressions and invoking functions distinguished, so that a pure function value does not hide an effectful call.
14. As a Ruddy programmer, I want effectful operations whose order is preserved to remain in that order, so that otherwise-safe rewrites can preserve observable behavior.
15. As a Ruddy programmer, I want ordinary numeric accumulator rewrites offered without numerical boundary proofs, so that I can judge whether applying the optional refactoring suits my program.
16. As a Ruddy programmer, I want recursive references resolved by binding identity, so that shadowed names cannot trigger an incorrect rewrite.
17. As a Ruddy programmer, I want unsupported recursion patterns left alone, so that diagnostics represent actionable opportunities.
18. As a Ruddy programmer, I want incomplete or ill-typed candidates skipped safely, so that editing does not produce misleading quick fixes.
19. As a Ruddy programmer, I want diagnostics refreshed after edits, so that a successfully converted function no longer appears as an opportunity.
20. As a Ruddy programmer, I want stale edits prevented, so that a quick fix cannot overwrite newer source.
21. As an editor user, I want accurate ranges with Unicode and different line endings, so that the quick fix edits the intended text.
22. As a Ruddy programmer, I want the resulting program checked through normal compilation and execution, so that the quick fix produces valid code with expected results on representative inputs and preserves effect ordering.
23. As a maintainer, I want detection and rewriting to share one eligibility decision, so that the editor never advertises a fix the compiler analysis cannot support.
24. As a maintainer, I want coverage through the existing LSP and runtime boundaries, so that tests protect user-visible behavior without coupling to private traversal details.

## Implementation Decisions

- Add opportunity analysis over resolved, typed source information with source spans. Keep semantic eligibility and rewrite planning in compiler/editor analysis; keep protocol serialization in the LSP. Reuse existing analysis and document-revision facilities rather than introducing a separate inference engine.
- Start with direct self-recursion, at most one recursive invocation on each execution path, and an accumulator pattern whose combining operation and identity are known. Require all recursive branches in a candidate to fit the chosen transformation; support ordinary conditionals and matches within this restriction. Do not perform call-graph-wide transformations.
- The minimum supported family is reduction with standard numeric addition and a zero accumulator, including counting and summation over Nat, Int, Real, and fixed-width integers. Recognize operations by resolved identity and type, never spelling alone. Each base case combines the accumulated value with its original result. Additional operations need a recognized accumulator transformation and representative regression coverage.
- Do not require proofs about numeric boundaries, overflow, floating-point rounding, or exact equivalence across backend number representations. These nuances are outside this optional lint’s scope and must not restrict eligibility to fixed-width arithmetic. The user judges whether to apply the proposed rewrite; no additional numerical approval step or warning is required. This does not grant arbitrary user-defined pure combiners algebraic laws or justify unrelated transformations.
- User-provided semantic guarantee: evaluation of a function/value with an empty, closed effect row may be reordered freely. Accept inferred as well as explicitly declared empty, closed rows. An open row is insufficient even if it currently lists no concrete effects. Unresolved rows are also insufficient. Closed nonempty rows do not receive this permission.
- Apply that guarantee to every evaluation or invocation moved by the rewrite, including recursive arguments, accumulator contributions, and combiner calls as applicable. The effect of obtaining a function value is distinct from the effect of calling it. Do not reject an entire function solely because it has effects when those effectful operations retain their order and handler scope; require proof for the actual movement. Skip a candidate if this proof cannot be established.
- Use the recognized numeric accumulator patterns without requiring exact numerical reassociation proofs. Preserve data dependencies and evaluation multiplicity; the reordering guarantee does not authorize duplication or discarding evaluations. Preserve the order and scope of effectful and open-row computations; omit the diagnostic when the transformation would move them without justification.
- Generate a private local helper carrying original parameters plus an accumulator, with the public function acting as a wrapper. Preserve annotations and the externally visible type, including polymorphism and effect rows. Ensure new bindings are fresh, recursive calls target the helper, and argument values are not accidentally captured from the initial invocation. Skip cases whose original interface cannot be preserved.
- Preserve comments and unrelated source with localized edits using existing source/formatting facilities. Avoid whole-document formatting as a side effect. If a candidate cannot be rewritten without losing source information, omit the opportunity.
- Use a stable diagnostic code, `tail-recursion-opportunity`, with informational severity. Locate one diagnostic at the function binding and explain the recognized work after recursion. The lint must not become a compilation error or change existing error severities.
- Advertise LSP code-action support and handle `textDocument/codeAction`. Return a `quickfix` action associated with the diagnostic and a workspace edit for the affected document. Respect requested ranges and action-kind filters; return no action for unrelated or unsupported requests.
- Recompute or validate eligibility against the current analyzed document revision. Use versioned document edits where supported and the existing cancellation/revision barrier; never return edits computed from an obsolete snapshot as current. Do not silently apply source changes on save.
- Verify the rewrite produces actual tail-position recursive calls recognized by existing lowering. Do not use a growing list or closure chain to simulate an accumulator. The accumulator value itself may grow according to the result representation; the claim concerns deferred control work.

## Testing Decisions

- The user confirmed the existing LSP protocol harness as the primary boundary, with existing runtime checks for rewritten programs. Extend those facilities rather than introducing a public API solely for tests.
- Good tests assert externally observable diagnostics, actions, edits, successful compilation, result values, and effect traces. Avoid assertions about private traversal structure, generated temporary identifiers, or incidental helper naming.
- Follow existing in-memory LSP initialize/open/request/shutdown tests, formatting-edit tests, and diagnostic revision tests. Exercise capability advertisement, diagnostic code/severity/range, action association, range and kind filtering, applying edits, and reanalysis clearing the diagnostic.
- Positive cases cover counting and summation over ordinary Nat, Int, Real, and fixed-width integers, base cases, conditionals and matches, multiple original parameters, local captures, inferred empty closed rows, and explicitly empty closed rows.
- Negative cases cover already-tail-recursive functions, nonrecursive functions, shadowed self/combiner names, multiple recursive calls on one path, mutual recursion, unsupported combiners, malformed candidates, and rewrites that cannot preserve the callable interface.
- Pair otherwise-equivalent candidates with empty closed, empty-looking open, unresolved, and closed nonempty effect rows. Accept justified movement only in the empty-closed cases. Include a candidate that preserves existing effectful operations in place and verify its trace if supported by the matcher; purity must be checked on what actually moves.
- Apply returned edits to the original source and compile through normal project entry points. Execute original and rewritten programs using existing generated-JavaScript and interpreter test facilities. Compare results on representative inputs at base cases and several recursive depths; verify preserved effect traces and public calling conventions. Use ordinary numeric inputs suitable for these comparisons. Do not require numerical boundary tests or exact equivalence for rounding-sensitive regrouping as acceptance criteria for this lint.
- Cover name collisions, comments, Unicode positions, line endings, unrelated edits, and stale diagnostic/action requests. A second action request after conversion must not propose the same transformation again.
- A deep-recursion runtime case alone cannot prove improvement because Ruddy already drives calls through a loop. Add a focused assertion through the existing accepted-program/artifact inspection boundary that the rewritten recursive call forwards the function continuation rather than introducing a per-step continuation for the removed combining work. Reuse existing compiler-output tests; do not add runtime instrumentation just for this feature.
- Run the Rust suite only through `just test`, never by invoking `cargo test` directly.

## Out of Scope

- General automatic conversion of arbitrary recursion, mutual recursion, branching recursion with multiple recursive calls, or higher-order recursion whose target cannot be resolved safely.
- Numerical boundary analysis, overflow and rounding equivalence proofs, and exact cross-backend numerical equivalence for regrouped calculations.
- General algebraic proofs for arbitrary pure functions, generic accumulator synthesis, constructor-based transformations, and unrestricted arithmetic reassociation.
- Compiler optimization changes for calls already in tail position, a new runtime calling convention, or general performance diagnostics.
- Project-wide fix-all, automatic changes on save, standalone CLI lint configuration, and a general-purpose lint framework.
- Claims that every recursion rewrite reduces elapsed time or total memory, or that this feature is needed to make ordinary Ruddy calls host-stack-safe.

## Further Notes

This spec records the agreed distinction between source-level opportunities and missed compiler optimizations. Empty, closed effect rows are sufficient permission for reordering under the user's guarantee; no extra purity caveat should be invented for them. Recognized numeric accumulator rewrites do not require numerical boundary or rounding equivalence proofs.

The user explicitly clarified that this is an optional lint, not an automatic compiler decision. Ordinary numeric counting and summation belong in the initial scope; numeric boundary nuances are left to the user deciding whether to apply the edit. The effect-row restrictions remain in force.

Implementation should preserve the existing architecture in which analysis serves the editor and lowering handles executable continuations. This issue specifies the feature; it does not implement it.

## Comments

Implementation adds the informational diagnostic and quick fix through shared typed analysis. Regression checks use the existing LSP harness, interpreter and generated JavaScript, plus artifact inspection of continuation forwarding. Review uses implementation starting commit `79e6ffa0` as its fixed point. Review corrections cover closed-effect handlers and complete source spans for erased `do` wrappers; both passed the follow-up spec review.

Final validation: `RUST_TEST_THREADS=2 just cov` passed 2,004 tests with 10 ignored; `cargo clippy --workspace --all-targets`, Rust formatting, and diff whitespace checks passed. Compiler coverage measured 96.09% lines and 88.70% branches; the new lint module measured 96.37% lines and 83.04% branches. The repository's required 100% compiler coverage remains unmet. Standards review also suggested an optional enum for the addition representation; spec review found no outstanding blockers after corrections.


Follow-up scope: the user requested multiplication after the product-reduction counterexample. Recognize Real `*` and standard numeric `multiply` functions (including fixed-width variants), with identity `1`. Preserve the existing effect-row, binding, source, and callable-interface restrictions. Require one consistent combiner across recursive branches; mixed addition/multiplication recurrence transformations remain unsupported. Extend the existing LSP/runtime and continuation-artifact tests to products.

Multiplication follow-up validation: `just test -- --test-threads=2` passed 2,008 tests with 10 ignored. Clippy, Rust formatting, and diff whitespace checks passed. LSP tests compile and run original and rewritten products in both backends, and artifact inspection verifies continuation forwarding for multiplication as well as addition. Coverage was not rerun; the previously reported coverage shortfall remains unresolved.

### Follow-up standards review

No new documented-standard violations found. The existing 100% coverage requirement remains unmet. Optional maintainability suggestions: share the standard numeric test setup between addition and multiplication, and model intrinsic versus standard callable representation explicitly.

### Follow-up spec review

No outstanding findings. Products use identity `1`, retain effect and source safeguards, and reject mixed recursive combiners.

Review totals: standards—one existing unmet requirement and two optional suggestions; spec—zero outstanding findings.
