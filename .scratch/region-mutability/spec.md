# Inferred single-region mutability

Status: ready-for-agent

## Problem Statement

Ruddy programmers need local state for counters, accumulators, and stateful
helpers without manually threading updated values through every call. They
also need factories that return cells or closures while preserving the language's
invariant that effect-less functions are pure.

The design must make state dependencies visible to inference, prevent one cell
from being used at incompatible types, and support future thread confinement.
It must fit the existing effect rows without overlapping mutation regions,
duplicate effect labels, or separate read and write permissions.

## Solution

Provide built-in cell allocation, reading, and writing. Infer the region used
by expressions, while requiring explicit region parameters in written cell
types and mutation effects. Each computation has at most one accessible
mutation region. Cells in that region may alias and may hold different types.

The source surface is:

| Form | Meaning |
| --- | --- |
| `mut value` | Allocate a fresh cell containing the evaluated value |
| `~cell` | Read a cell |
| `cell := value` | Write a cell and return the value just stored |
| `mut 'r 'a` | Cell type with region `'r` and element type `'a` |
| `!mut 'r` | Effect required for allocation, reads, and writes in `'r` |
| `type t 'r = mut 'r Nat` | Type alias with an explicit region parameter |
| `effect e 'r = !Log + !mut 'r` | Effect alias preserving the region argument |

Assignment associates to the right. The expression `a := b := c := ~d` means
`a := (b := (c := ~d))`: the value read from `d` is written to `c`, then `b`,
then `a`, and is the result of the entire expression. Discarded results continue
to use `let _ =`; there are no bare expression statements.

Factories allocate in the caller's inferred region and retain their mutation
effect. The compiler may isolate fresh local state at a function body when the
region cannot escape through the result, latent effects, remaining effects, or
surrounding environment. That removes only the local mutation effect. Ordinary
helpers can return state; an isolated scope cannot expose usable state after
its effect has been removed.

For example:

```text
let make_counter = fn _ => do
  let count = mut 0n
  return fn _ => count := nat::add (~count) 1n
end

let example = fn _ => do
  let first = make_counter ()
  let second = make_counter ()
  let _ = first ()
  return second ()
end
```

The factory has the inferred type
`() -> (() -> Nat + !mut 'r) + !mut 'r`. Each call allocates a different cell.
The enclosing example returns `1n` and can have the pure type `() -> Nat`.

## User Stories

1. As a Ruddy programmer, I want to allocate a cell with a keyword, so that mutable state has a direct language spelling.
2. As a Ruddy programmer, I want to read a cell with a prefix operator, so that state access is visible at the use site.
3. As a Ruddy programmer, I want to write a cell with an assignment operator, so that updates are concise.
4. As a Ruddy programmer, I want assignment to return the stored value, so that I can use an update as an expression.
5. As a Ruddy programmer, I want assignments to associate rightward, so that I can copy one value through a chain of cells.
6. As a Ruddy programmer, I want each operand evaluated once in a defined order, so that effectful targets and values behave predictably.
7. As a Ruddy programmer, I want discarded writes to use the existing binding syntax, so that blocks keep their current sequencing rules.
8. As a Ruddy programmer, I want reference aliases to observe the same updates, so that helpers can share state within a region.
9. As a Ruddy programmer, I want separate allocations to create separate cells, so that two counters do not accidentally share their contents.
10. As a Ruddy programmer, I want one region to contain cells of different types, so that state can model more than one kind of value.
11. As a Ruddy programmer, I want cell types and mutation effects to carry a region parameter, so that signatures describe which state they depend on.
12. As a Ruddy programmer, I want region variables to have their own kind, so that ordinary value types cannot stand in for region identities.
13. As a Ruddy programmer, I want type aliases to forward explicit region parameters, so that reusable state types retain their meaning.
14. As a Ruddy programmer, I want effect aliases to forward region parameters, so that mutation composes with ordinary declared effects.
15. As a Ruddy programmer, I want ordinary expressions to infer their regions, so that local mutation needs no explicit region block.
16. As a Ruddy programmer, I want all accesses in one computation to agree on a mutation region, so that the existing effect-row model stays simple.
17. As a Ruddy programmer, I want local mutation with no observable escape to become pure, so that implementation details do not burden callers.
18. As a Ruddy programmer, I want mutations of argument-owned state to remain effectful, so that a function cannot conceal changes to its caller's state.
19. As a Ruddy programmer, I want factories to allocate in the caller's region, so that they can return cells without existential packaging.
20. As a Ruddy programmer, I want returned stateful closures to retain their mutation effect, so that subsequent calls expose their state dependency.
21. As a Ruddy programmer, I want multiple factories to share a caller region, so that independently allocated objects compose in one computation.
22. As a Ruddy programmer, I want an enclosing function to isolate state produced by helpers, so that abstraction through a factory does not prevent purity.
23. As a Ruddy programmer, I want unrelated effects to survive local-state isolation, so that logging and foreign operations remain visible.
24. As a Ruddy programmer, I want isolation considered only at function bodies, so that its placement is predictable.
25. As a Ruddy programmer, I want a cell's element type to stay consistent across reads and writes, so that mutation cannot violate type safety.
26. As a Ruddy programmer, I want pure initializers to retain ordinary let polymorphism, so that existing pure abstractions remain reusable.
27. As a Ruddy programmer, I want effectful initializers to retain shared unknown types, so that later uses cannot instantiate one cell incompatibly.
28. As a Ruddy programmer, I want cell factories to remain polymorphic, so that different calls can create differently typed cells.
29. As a Ruddy programmer, I want aliases and captured closures to preserve shared type variables, so that they cannot circumvent the polymorphism restriction.
30. As a Ruddy programmer, I want kind, write-type, and region mismatches explained at the relevant source expressions, so that errors are actionable.
31. As a library author, I want imported signatures to preserve region kinds and effects, so that dependent bundles get the same checks as local code.
32. As a library author, I want explicit annotations to preserve their existing contract, so that inference does not silently add mutation to a promised pure function.
33. As a Ruddy programmer, I want cells to work with existing immutable aggregates and closures, so that mutation integrates with ordinary values.
34. As a Ruddy programmer, I want persistent arrays to remain immutable, so that adding cells does not change existing array behavior.
35. As a foreign binding author, I want to declare region-dependent arguments and callbacks, so that foreign implementations participate in the same effect model.
36. As a foreign binding author, I want the language to trust my lifetime and thread contracts, so that this feature does not impose new callback guards or revocation machinery.
37. As a Ruddy programmer, I want suspension to preserve cell contents and aliasing, so that delayed foreign completion does not reset state.
38. As a Ruddy programmer, I want the model to confine a region to its owning thread, so that future threading does not permit cross-thread reads or writes.
39. As a compiler user, I want formatting and editor grammar support for the new syntax, so that tools agree with the compiler.
40. As a compiler user, I want diagnostics and debugger views to display regions consistently, so that inferred state dependencies are understandable.
41. As a compiler maintainer, I want canonical artifacts to retain region information and validate it, so that imports cannot lose the feature's invariants.
42. As a compiler maintainer, I want behavior tested through existing compilation and execution interfaces, so that tests exercise the guarantees users receive.

## Implementation Decisions

- Allocation is a compiler/runtime primitive exposed through the reserved mut keyword. It is not implemented by ordinary immutable Ruddy code, and a standard-library allocation wrapper is not required. The builtin cell type and effect are available independently of the standard library.
- Allocation and reading are prefix expressions at the existing unary precedence. Function application and projection bind more tightly; grouping is required to pass a read or allocation expression as an application argument. Nested prefix expressions follow the existing unary recursion rule.
- Assignment is the lowest-precedence expression operator, below pipeline and the existing Boolean and numeric operators, and associates rightward. This completes the previously unspecified infix placement while preserving the selected unary precedence.
- An assignment evaluates its target first and its right operand second, each exactly once, then stores and returns that value. It does not re-evaluate the target or re-run a source-level read. For a chain with computed targets, targets are evaluated from the outer assignment inward, before the writes complete from the innermost assignment outward.
- The target may be any expression producing a cell. Assignment updates contents, preserves cell identity, and requires agreement with the fixed element type. It does not rebind the target's source name or implicitly convert ordinary fields or array elements into cells.
- Allocation evaluates its initializer once and creates a fresh cell. Reads and writes preserve the existing value representation and aliasing of stored values; they do not deep-copy mutable values reachable through the contents. Writes are ordinary sequential state changes, not transactions.
- Cell type syntax takes a region argument and an element-type argument using the existing type-argument grouping conventions. Effect syntax takes exactly one region argument and keeps its effect sigil. Written aliases bind and apply region parameters explicitly; region omission in a written cell type is not inferred.
- Add the Region kind alongside the existing kinds. Region positions accept region variables, not ordinary types, effect rows, or presence variables. Kind forwarding through aliases and imports preserves the region's semantic relevance.
- Regions have flexible inference variables, bound scheme variables, and fresh rigid identities at inferred isolation scopes. Instantiating a helper selects an existing region; it does not allocate a fresh independent region. Region identity must survive substitution without being erased by type aliases.
- The mutation effect uses one ordinary constructor-level row identity with a region argument. Repeated accesses unify that argument. Distinct rigid regions cannot be used jointly by one computation. Ordinary effect arguments, presence conditions, row tails, and lacks behavior retain their existing contracts; there are no region-variable row keys or duplicate mutation entries.
- A cell's element parameter is invariant. Different cells in one region may have different element types. A cell's region remains fixed across aliases, writes, captures, imports, and calls.
- Isolate state only at function bodies, including bodies of nested functions. A block is not independently an isolation scope. Isolation introduces fresh state only when the region is independent of the function's inputs and surrounding environment and cannot escape through its result, latent effects, or residual effects.
- Isolation removes only the local mutation effect. Access to input-owned or captured state remains effectful. A pure helper with its own fully isolated state may be called inside a mutation-using function without exposing a second region to that caller.
- If a function returns state or a stateful closure, allocate that state in the caller's inferred region and retain the corresponding immediate allocation effect and latent access effects. Multiple calls allocate distinct cells in the same caller region. There is no runtime promotion of an already isolated store and no inferred existential region package.
- Decide isolation coherently with constraint solving and scheme publication. Do not publish a state factory as pure and repair its signature at later use sites. Explicit annotations remain promises: an omitted mutation effect cannot be silently added when the annotation requires a pure arrow.
- Generalize eligible variables only when evaluating the binding initializer is proven effect-less, subject to existing environment and level restrictions. For an effectful or not-proven-effect-less initializer, retain shared unknowns at a scope that prevents later aliases from generalizing them. This rule applies to ordinary let bindings, including destructuring and bindings whose effects have no mutable result.
- Creating a function is distinct from invoking it. A factory may generalize even when its function type contains latent mutation effects; variables shared with captured state still cannot generalize. Do not retroactively generalize internal mutable bindings because an enclosing function's state is later isolated.
- Preserve existing producer-owned presence packages and their guarantees. They are not converted into caller-chosen variables by the new generalization rule. The feature does not extend existential packages to region variables.
- Allocation, read, write, and isolation have compiler-controlled semantics. Ordinary user effect handlers, aliases, declarations, or metadata cannot redefine the builtin's meaning or discharge access to externally observable mutable state as though it were isolated.
- Pure global initialization remains the existing requirement. An initializer that retains an escaping cell cannot hide its mutation effect. Normal function-body isolation can handle local state in the executable entry function while preserving platform effects; no new root-region surface construct is required.
- Integrate the semantic forms through parsing, IR, inference, accepted compilation, CPS lowering, artifacts, linking, and the JavaScript backend. Keep cell operations in the existing sequential execution model, preserve state over suspension, and preserve the existing bounded-stack execution contract.
- Persist and validate region kinds, quantifiers, cell types, mutation effects, and executable operations. Import and export must preserve type sharing, builtin identity, and scheme meaning. Update canonical artifact handling and cache invalidation as needed without introducing new public compiler entry points.
- Update source/type printers, diagnostics, debugger views, and tree-sitter grammar/highlighting. Diagnostics must distinguish missing operands, incorrect builtin arity, kind mismatches, non-cell accesses, incompatible writes, region disagreement, and an annotation that incorrectly promises purity, reusing existing diagnostic categories where appropriate.
- A region is confined to its creating thread; both reading and writing on another thread violate the language contract. This feature adds no thread creation, region transfer, lending, or shared-reader permissions. Future Ruddy threading support must enforce confinement transitively through values and captures. No host-thread runtime tracking is required for the current JavaScript implementation.
- Foreign declarations and foreign callers are trusted to respect declared effects, fixed element types, aliasing, region lifetime, and thread confinement. Preserve ordinary source type/effect checking, but add no FFI-specific retention rejection, revocation, lifetime guards, or owning-thread checks. Retained and asynchronous uses are not categorically banned; the foreign side must keep them within the valid region contract.
- Direct foreign cell arguments and returned cells must preserve cell identity and the existing conversions of their contents. Document the backend's cell representation with its implementation. Foreign code must not present externally shared state as fresh isolated storage or omit observable host effects from a declaration. Existing callback and completion contracts otherwise remain unchanged.

## Testing Decisions

- Prefer the existing accepted-compilation seam: compile source with its dependencies, inspect published semantic types or diagnostics, then lower accepted programs and execute the emitted JavaScript. Do not introduce a separate public mutability-testing interface.
- Use the existing end-to-end bundle/CLI execution pattern for runtime acceptance. Observe returned values and effect order rather than private solver variables, cell layout, or instruction sequences. Use controlled same-thread foreign helpers only where testing the trusted FFI interface itself requires them.
- Follow existing typed-effect, nested-let, presence-package, array, do-block, artifact round-trip, and CPS callback tests as prior art. Use focused parser/printer and tree-sitter tests only for the syntax/tooling contracts that those interfaces expose directly.
- Confirm allocation freshness, aliases observing writes, heterogeneous cells in one region, reads returning contents, assignment returning the new value, copying through the derived helper, and chained assignments with aliased targets.
- Observe target-first, exactly-once operand evaluation with computed targets and values. Cover the distinction between target-evaluation order and write order in a chain, and interactions with application, projection, unary operators, arithmetic, and pipelines.
- Verify discarded writes through ordinary bindings, assignment as a returned expression, and rejection of bare expression statements. Check incomplete operators, missing type/effect arguments, wrong kinds, omitted alias region parameters, and compatibility with existing colon and module-path tokens.
- Verify inferred schemes for cell factories, stateful-closure factories, same-region helpers, and an enclosing pure computation using two independent counters. Returning argument-owned state is allowed with the appropriate dependency; reading or writing it cannot be isolated away.
- Verify a body cannot jointly access incompatible region identities. Check that calling an already pure isolated helper does not merge its private region with its caller's region. Exercise nested functions, annotations, alias types, recursive use, and effect-polymorphic callbacks through published behavior.
- Reject incompatible writes to one shared cell, including after aliasing, destructuring, or closure capture. Check that cells initially containing empty arrays or identity functions retain shared element variables, while calls to a polymorphic factory can allocate differently typed cells.
- Preserve polymorphism for eligible effect-less initializers, including calls to legitimately isolated pure helpers. Retain shared variables for effectful initializers even when their result is immutable, and for initializers whose effects are still open. Pure aliases of monomorphic state cannot regain polymorphism; existing presence-package regression behavior remains intact.
- Verify local mutation disappears only after valid function-body isolation and unrelated effects remain. Preserve the entry and global-initialization contracts. Include negative annotations that promise purity while accessing supplied or captured state.
- Round-trip and import libraries exposing region-polymorphic cells and closures, type aliases, and effect aliases. Validate malformed kinds, binder positions, builtin forms, and executable data through the existing artifact interfaces. Consumers must see the same restrictions after import as for local definitions.
- Exercise compliant foreign reads/writes, callbacks, returned aliases, and Promise completion with state on the owning thread. Do not add tests expecting runtime rejection of foreign invariant violations, since enforcement is explicitly outside this contract.
- Preserve immutable array behavior when arrays are stored in cells. Check that suspension retains contents and aliases and that recursive stateful helpers do not regress the existing stack guarantees. This does not add transaction, cancellation, or cross-thread execution tests for features the language does not yet expose.
- Run Rust tests only through `just test`; never invoke `cargo test` directly. Run the existing grammar recipe for tree-sitter changes and the repository's relevant formatting and lint checks when implementing the feature.

## Out of Scope

- Multiple simultaneously accessible mutation regions, duplicate effect rows, generative region labels, region-label inequality constraints, and read/write permission splitting.
- Explicit region-block syntax, isolation at arbitrary blocks or let initializers, and region elision in written cell types or type aliases.
- Existential region packages, automatic capture packaging, promoting escaped private stores, and mutable global initialization.
- Relaxed variance-based generalization, effect-specific exemptions, or additional dependency analysis to generalize effectful initializers.
- Bare expression statements, assignment rebinding an immutable name, mutable record-field syntax, implicit dereferencing, and additional allocation-library surface.
- Threads, workers, region transfer, borrowing permissions, locks, transactional updates, deterministic destruction, cancellation semantics, and multi-shot continuations.
- FFI lifetime enforcement, callback revocation, host-thread guards, or a scoped-only foreign callback policy.
- Mutable buffers, zero-copy freezing, cell identity comparison, and changes to persistent array semantics or performance guarantees.

## Further Notes

This spec is the authoritative consolidation of the conversation. The earlier
[investigation](investigation.md), [escape comparison](escaping-state.md),
[polymorphism discussion](polymorphism.md), [FFI contract](ffi.md), and
[prior art](prior-art.md) retain rationale and deferred alternatives; they do
not expand this spec's scope.

The final pass fixes the remaining routine grammar details: allocation uses the
mut keyword at unary precedence, and assignment is below the existing infix
levels. All effect and cell-type spellings use the latest lowercase forms with
explicit region parameters in written types and effects.

The specification was finalized before implementation. The source semantics and
JavaScript boundary are documented in [mutable cells](../../docs/mutability.md).

## Comments

Implementation and review are recorded in [implementation.md](implementation.md).
