# Relocatable CPS LIR and Transparent Async Interoperation

Status: ready-for-agent

## Problem Statement

Ruddy currently lowers calls to ordinary host calls and implements handler exits through native exceptions. Recursive Ruddy execution therefore grows the JavaScript stack, including in pure functions. Pending computation and handler extent depend on host stack activations, making it difficult to suspend a computation for asynchronous JavaScript work and resume it with the same semantics.

The language should support ordinary sequential code whether an operation completes immediately or later. Programmers should not need separate synchronous and asynchronous functions, Promise-shaped result types, or an Async effect merely to use asynchronous host implementations. JavaScript interoperation must also support retained callbacks, repeated invocations, and overlapping invocations without confusing a callback with the continuation of the call that registered it.

These capabilities must be established during independent bundle compilation. Reconstructing control flow after linking, or independently in every backend, would duplicate semantic decisions and make portable artifacts incomplete descriptions of execution. The user explicitly prefers breaking the artifact format now to introducing a required transformation of fully linked artifacts.

## Solution

Make continuation-passing style the native form of portable LIR. Lower each accepted bundle directly into functions containing parameterized basic blocks, explicit continuation values, and explicit handler control. Serialize that representation before linking. Linking combines artifacts and relocates references; it does not discover continuations, transform calling conventions, or infer suspension behavior.

Give all Ruddy functions one internal calling convention that can deliver results immediately or after suspension. Preserve ordinary source-level function types and current handler behavior. Foreign bindings describe host completion protocols, and adapters translate immediate returns, Promises, completion callbacks, and notifications into the shared execution model.

Implement JavaScript execution with generated step functions, explicit continuation frames, and a central driver. Potentially unbounded Ruddy transfers return through the driver. Tail calls reuse return continuations; non-tail recursion may retain pending work on the heap while keeping the host stack bounded.

Keep callbacks alive through ownership of their function, captures, required handler evidence, and runtime. Give every invocation fresh completion and execution state. Check handler-exit validity at runtime, including logical ancestry, and defer general static capture/lifetime inference. This provides transparent suspension and robust callback execution without expanding the migration into a second inference system.

## User Stories

1. As a Ruddy programmer, I want deep pure recursion to execute without exhausting the JavaScript stack, so that stack safety does not depend on whether a function performs effects.
2. As a Ruddy programmer, I want mutually recursive functions to remain stack-safe, so that splitting an algorithm across functions does not change its execution guarantee.
3. As a Ruddy programmer, I want higher-order and indirect calls to share the stack-safety guarantee, so that passing a function as a value does not disable it.
4. As a Ruddy programmer, I want tail calls to reuse return continuations, so that an unbounded tail-recursive computation does not accumulate unnecessary return frames.
5. As a Ruddy programmer, I want non-tail recursion to store pending work outside the host stack, so that algorithms with pending results can run beyond host recursion limits.
6. As a Ruddy programmer, I want evaluation order preserved, so that the migration does not reorder calls or observable effects.
7. As a Ruddy programmer, I want a domain operation to have the same source type for immediate and asynchronous implementations, so that its implementation can change without rewriting its callers.
8. As a Ruddy programmer, I want to consume an asynchronous operation's ordinary result directly, so that I do not need an await keyword or a Promise result type.
9. As a library author, I want higher-order functions to work with immediate and suspending callbacks through one internal convention, so that I do not need duplicate library functions.
10. As a handler author, I want an operation arm to return once to the operation as it does today, so that CPS does not change handler syntax or resumption behavior.
11. As a handler author, I want an arm to suspend and later finish normally, so that asynchronous host work composes with existing handlers.
12. As a handler author, I want normal completion to run the return arm exactly as before, so that result transformation remains predictable.
13. As a handler author, I want raise to bypass the return arm, so that early handler exit retains its meaning.
14. As a handler author, I want nested and recursive handlers to retain distinct dynamic identities, so that an operation exits the correct handler.
15. As a handler author, I want suspension to preserve live handler context, so that raise after resumption still reaches the correct destination.
16. As a Ruddy programmer, I want domain effects to continue describing capabilities, so that asynchronous execution does not require an additional source-level Async effect.
17. As a Ruddy programmer, I want uninterrupted execution between actual suspension points, so that introducing a trampoline does not silently add scheduling interleavings.
18. As a Ruddy programmer, I want sequential source execution preserved across suspension, so that later expressions run only after earlier results are available.
19. As a Ruddy programmer, I want the interleaving implications of suspending host operations documented, so that I do not mistake sequential code for an atomic transaction relative to JavaScript.
20. As a foreign-binding author, I want to declare an immediate host completion protocol, so that ordinary host functions retain straightforward adapters.
21. As a foreign-binding author, I want to adapt Promise completion to an ordinary Ruddy result, so that JavaScript asynchronous operations integrate without changing Ruddy callers.
22. As a foreign-binding author, I want to adapt callback-based completion, so that APIs without Promises can participate in the same execution model.
23. As a foreign-binding author, I want immediate completion during callback registration supported, so that an API may complete either inline or later without corrupting execution state.
24. As a foreign-binding author, I want only the first completion of a pending operation accepted, so that duplicate callbacks cannot resume one continuation twice.
25. As a foreign-binding author, I want host values distinguished from control-protocol results, so that returning a Promise-like object as data does not accidentally suspend Ruddy.
26. As a foreign-binding author, I want synchronous host exceptions and asynchronous rejection to follow one failure policy, so that timing does not change error meaning.
27. As a foreign-binding author, I want to translate host failures into Ruddy results or domain effects explicitly, so that a binding can expose a useful language-level error contract.
28. As a JavaScript caller, I want a synchronous adapter to return an immediate value or throw, so that its return contract is fixed.
29. As a JavaScript caller, I want a Promise adapter always to return a Promise, so that immediate Ruddy completion does not change the interface shape.
30. As a foreign-binding author, I want synchronous callbacks accepted only with proof or a trusted non-suspension contract, so that a host operation expecting an immediate result cannot receive a suspended computation.
31. As a JavaScript caller, I want a callback kept after registration to retain its captured values and runtime, so that its ordinary callable state remains available.
32. As a JavaScript caller, I want to invoke the same retained callback repeatedly, so that a function value is not consumed by its first invocation.
33. As a JavaScript caller, I want overlapping callback invocations to have independent continuations, so that completion of one cannot settle another.
34. As a JavaScript caller, I want overlapping invocations to share captured values according to ordinary closure semantics, so that the runtime does not silently clone state.
35. As a foreign-binding author, I want callback result delivery selected by the host contract, so that the same internal Ruddy callable can support synchronous, Promise, callback-result, and notification interfaces.
36. As a notification-binding author, I want failures reported even when JavaScript ignores callback results, so that errors after suspension are not lost.
37. As an embedder, I want to provide an unhandled-error reporter, so that notification failures integrate with my application's error reporting.
38. As a Ruddy programmer, I want a callback to establish aborting handlers inside each invocation, so that each handler exit belongs to that invocation and can survive its suspension.
39. As a Ruddy programmer, I want invalid exits through retained evidence to fail explicitly, so that the runtime never resumes an already completed computation.
40. As a Ruddy programmer, I want an independent callback prevented from aborting another suspended invocation, so that keeping evidence does not grant implicit cross-invocation cancellation.
41. As a Ruddy programmer, I want synchronous callbacks nested in a foreign call to preserve logical ancestry, so that valid existing handler exits continue to work.
42. As a runtime maintainer, I want terminal invocation states to reject late completion, so that finished or failed work cannot restart.
43. As a library author, I want global initialization to permit handled suspension, so that top-level computation does not require a second calling convention.
44. As a library consumer, I want initializers to run in the established dependency and declaration order, so that suspension does not reorder initialization.
45. As a JavaScript module consumer, I want module readiness to wait for initialization, so that I do not receive a successfully initialized module with incomplete globals.
46. As an executable author, I want main invoked once after initialization completes, so that program entry remains predictable.
47. As an executable author, I want existing Console and Process behavior preserved, so that asynchronous execution does not change output draining or exit semantics.
48. As a bundle author, I want my artifact to contain executable CPS control flow before linking, so that compilation does not depend on the final application graph.
49. As a bundle consumer, I want continuations to cross calls into dependency code, so that a dependency can return or suspend without knowing its caller's continuation layout.
50. As a linker maintainer, I want function references relocated mechanically, so that linking remains independent of control analysis and inference.
51. As a linker maintainer, I want function-local block references left unchanged, so that local control flow does not require global renumbering.
52. As an artifact consumer, I want malformed control destinations and capture layouts rejected, so that validated artifacts establish the invariants needed by backends.
53. As an artifact consumer, I want compiler-produced suspension summaries preserved, so that synchronous adapter eligibility can be checked without whole-program analysis after linking.
54. As a compiler maintainer, I want unknown suspension behavior treated conservatively, so that missing information never becomes an unsound synchronous guarantee.
55. As a backend author, I want portable continuation destinations and captures without prescribed object layouts, so that each target can choose its storage representation.
56. As a JavaScript backend maintainer, I want ordinary CPS execution to avoid a Promise per call, so that suspension support does not impose unnecessary Promise scheduling.
57. As a compiler maintainer, I want accepted-program and validated-artifact interfaces retained, so that the migration does not reopen invalid phase combinations.
58. As a debugger user, I want CPS blocks, continuations, captures, and handler control visible in the existing compiler views, so that I can inspect generated execution flow.
59. As a debugger user, I want source navigation and temporary identity to remain useful, so that a new control representation does not make diagnostics harder to understand.
60. As a test author, I want behavior verified through the normal project build and execution path, so that acceptance tests exercise what users actually run.
61. As a maintainer, I want old incompatible artifacts rebuilt or rejected deliberately, so that stale cached output cannot silently execute with the new convention.
62. As a future runtime author, I want execution state owned by individual invocations, so that task scheduling can be added without reconstructing hidden host-stack state.

## Implementation Decisions

- The compile module continues to own bundle compilation and accepted/partial compilation states. LIR lowering consumes front-end accepted input, and core compilation publishes a validated portable artifact. Backend generation remains outside core compilation.
- The persisted LIR is CPS from its first publication. No direct-style LIR artifact or required post-link CPS transformation is introduced. Internal construction, capture determination, summary computation, and validation steps may run during bundle compilation.
- Existing pattern compilation, ordinary closure conversion, currying adaptation, representation erasure, structural effect identity, and evidence plumbing remain shared lowering responsibilities. Their observable behavior is preserved while their control representation changes.
- Functions contain an entry block and a flat collection of parameterized basic blocks. Every block contains ordinary instructions followed by exactly one explicit control terminator.
- Local branches, loops, and joins use block destinations and arguments. Nested block-valued switches and implicit child-block yields are replaced by explicit local control flow. Local joins do not require allocating continuation values.
- Ruddy function calls are control transfers carrying explicit return continuations, rather than value-producing instructions with implicit host returns. Returning invokes the supplied continuation. Tail calls pass through the caller's return continuation when no intervening semantic work is required.
- All Ruddy functions, ordinary closures, generated wrappers, and effect operation arms use the shared CPS convention, including pure functions. Restricting CPS conversion to effectful functions does not satisfy the stack-safety requirement.
- Continuation values are distinct from ordinary Ruddy function values. A portable continuation specifies its code destination, explicit captures, and invocation convention. A resumed entry can access prior execution values only through its declared captures and parameters.
- A code destination consists of an artifact-local function identity and a function-local block identity. Ordinary function references use the same artifact-local function table. Local block arguments and temporary availability have explicit ownership rules.
- Physical frame layout, allocation strategy, code dispatch, and host object representation are backend decisions. Portable control metadata does not prescribe JavaScript objects, Promises, machine addresses, or a worker layout.
- Linking retains the existing dependency-first combination model. It relocates artifact-local function references in direct calls, ordinary closures, continuation destinations, initializer references, and other code references. Function-local block references remain unchanged.
- Cross-bundle references retain the existing qualified-symbol mechanism where applicable. Calling dependency code with a continuation from the caller's artifact requires only the uniform continuation invocation convention; the dependency does not inspect the caller's captures.
- Linking performs no continuation discovery, calling-convention transformation, suspension inference, or required optimization. Backend emission receives already explicit control flow and established calling-convention facts.
- The artifact schema changes deliberately. Update artifact construction, canonical rendering, parsing, validation, recovery where applicable, linking, compiler/cache compatibility information, and consumers together. Backward compatibility with the previous LIR schema is not required, and stale artifacts must not be silently accepted as compatible.
- Artifact validation establishes reference validity, function/block ownership, argument and capture convention agreement, representation compatibility, temporary availability, and valid block termination. Continuation entries and ordinary function entries cannot be interchanged merely because both have code addresses.
- The existing distinction between UncheckedArtifact and Artifact remains meaningful. Any recovery of incomplete metadata is conservative; it must not manufacture a synchronous guarantee or an executable control destination that was not established.
- Runtime effects continue to use evidence to select operation implementations. CPS does not replace capability selection with global dynamic lookup or change structural effect identity.
- Each dynamic evaluation of a handler establishes a distinct identity and exit context. Normal completion of the handled body goes through its return arm; raise exits the matching handler and bypasses that arm. Preserve existing evidence scoping in handled bodies, operation arms, and return arms.
- An operation arm's ordinary result resumes its operation once. Its evaluation may suspend before producing that result. This does not add source-visible resume, continuation capture, or multi-shot handling.
- Live handler context and pending continuations survive suspension independently of native exception-stack activations. Native try/catch extent cannot be the sole source of handler lifetime.
- Handler exits are managed runtime destinations with validity and logical-ancestry checks. Completing their extent invalidates the destination. Suspension alone does not invalidate it.
- A raise may target a handler only when that handler remains active on the current execution's logical continuation chain. An independently invoked retained callback cannot abort another invocation, even if that invocation is suspended and its handler is still active.
- Synchronous callbacks nested within an active foreign call preserve the caller's logical ancestry. Foreign reentry support must preserve valid existing handler aborts across that call while keeping unrelated invocations separate.
- Invalid or expired handler exits fail explicitly at runtime. Keeping the handler's identity or captured data alive does not resurrect its completed exit. A handler answer must not be silently substituted for a callback result, since their types can differ.
- Ordinary callback captures and required evidence remain owned and usable according to their runtime resource lifetimes. General static capture/lifetime inference, retainability certification, and a new scoped-capability type system are not prerequisites for this migration.
- A retained callback holds its Ruddy function, captured values, required evidence, and owning runtime. It does not retain the registration call's return continuation as its reusable completion destination. Evidence may still contain handler-exit references, which are subject to the runtime checks above.
- Every callback invocation receives fresh arguments, completion state, and execution state. Repeated invocation of a function is distinct from repeated resumption of one pending operation.
- An invocation may begin while a previous invocation of the same callback is suspended. The invocations have separate continuation chains and completion state, and share captured values according to normal closure semantics. No implicit serialization or cloning is introduced.
- A callback can establish a fresh handler inside its own body on each invocation. That handler belongs to the invocation and remains valid across that invocation's suspension.
- There is no required async keyword, await syntax, Async effect, Promise result type, or source-level distinction between synchronous and asynchronous Ruddy functions. Domain effect rows continue to describe capabilities rather than host completion timing.
- All Ruddy calls use the same continuation convention whether completion is immediate or delayed. Evaluation remains sequential within an invocation: dependent expressions do not run until their inputs complete.
- Foreign binding declarations or implementations explicitly describe completion protocols. Support immediate return, Promise completion, and callback-based completion. Protocol information is a foreign-interface concern and does not propagate as a new ordinary source-language effect.
- Extend reviewed extern facts and ExternPlan to carry the required conversion and completion decisions. Absorb executable adapter behavior and required metadata into portable LIR/artifacts rather than persisting a second executable extern-plan representation. Target-specific source validation remains with the target adapter.
- Do not infer a suspension request by inspecting arbitrary returned values for Promise-like properties. A binding identifies whether a foreign result is data or part of its completion protocol.
- Foreign callback result delivery has a fixed convention chosen by the binding: synchronous return, Promise return, delivery through a supplied completion callback, or notification with no consumed result. Ordinary Ruddy callers need not select separate source-language functions for these conventions.
- A synchronous JS adapter returns a value or throws. A Promise adapter always returns a Promise, even when execution completes immediately. A callback must not dynamically alternate between a raw result and a Promise under one declared JS contract.
- Synchronous exports and callbacks require a compiler-established non-suspension guarantee or a trusted foreign contract. An empty escaping effect row is not sufficient evidence, because locally handled operations may suspend. Trusted contract violations must not be converted into a successful synchronous result.
- Compute conservative suspension information during independent bundle compilation using dependency information. Preserve callable summaries needed by consumers in artifacts, separately from source-language types. Summary production and adapter-eligibility diagnostics belong to bundle compilation, not link-time analysis or backend rediscovery.
- Initially treat unresolved indirect calls, unknown handler implementations, and insufficient imported callable information as potentially suspending. More precise parameter-dependent or higher-order summaries are a later optimization; unknown must never default to synchronous.
- Preserve the coherence of accepted compiler states when introducing summary computation and synchronous-adapter checking. Invalid adapter requests must produce supported diagnostics through compilation or the existing target-validation interface as appropriate, rather than leave inconsistent artifacts or require callers to orchestrate new checks manually.
- Ordinary call and completion transfers proceed through the execution driver without recursively invoking the next unbounded step. A host completion callback that fires during registration must record or hand off completion safely without recursively reentering that driver.
- Each pending host operation has one completion state. The first success or failure delivery wins; duplicate deliveries cannot resume it again. This restriction does not prevent a registered event callback from being invoked repeatedly as separate work.
- Finished and failed invocations are terminal. Late completion cannot restart them. Establish these states now without adding user-visible cancellation or promising cancellation of the underlying foreign operation.
- Synchronous host exceptions and Promise/callback failures use a binding's common failure policy. A binding may explicitly translate failure into a Ruddy result or domain effect. Otherwise, the invocation fails as a host failure: a synchronous adapter throws and a Promise adapter rejects.
- Host failures are not automatically interpreted as Ruddy raise, which has a specific handler identity and answer destination. Invalid handler-exit failures likewise must not be delivered as ordinary callback success values.
- Notification adapters route failures to a runtime-level unhandled-error reporter, including failures after suspension. Embedders may supply the reporter; the target default reports an uncaught host error. The adapter must not rely on JavaScript observing an ignored returned Promise.
- JavaScript execution uses generated step functions, explicit continuation frames, and a central driver. Local control flow may remain inside generated loops and branches. Potentially unbounded calls and continuation transfers return through the driver.
- A trampoline alone is not an event-loop yield. Immediate computations run synchronously until completion or an actual suspension request. Do not add automatic budget-based yielding or a Promise/microtask for every call.
- A Promise adapter or notification adapter ordinarily begins executing immediately and runs until completion or suspension, preserving synchronous effects before the first suspension. Host protocol timing still governs later completion.
- The stack-safety contract covers pure, effectful, direct, indirect, higher-order, mutual, and non-tail Ruddy recursion. Tail calls must not accumulate unnecessary return frames; non-tail recursion may retain heap state proportional to pending work. This is not a constant-total-memory guarantee.
- Recursive alternation through synchronous foreign calls and host callback frames is outside the initial bounded-host-stack guarantee. Host-internal recursion and compiler implementation recursion are also not covered by this generated-program guarantee.
- Global initializers are CPS computations run through the same machinery. They may suspend when their effects are locally handled; the existing prohibition on escaping top-level effects remains.
- Initialization preserves existing dependency and declaration order. Module readiness waits for successful initialization, and executable main is invoked exactly once afterward. Initialization failure must not be presented as successful module readiness or allow main to proceed.
- Preserve the existing executable entry type contract, structural platform-effect recognition, Console behavior, and Process exit behavior, including pending-output draining. Entry and initialization adapters must follow CPS completion rather than consume an immediate host return.
- Update ordinary standard-library externs, generated foreign adapters, JS library exports, and executable entry adapters for the new convention. Preserve existing observable synchronous behavior where a synchronous guarantee is established; uncertain JS-facing callable exports use a fixed suitable adapter rather than exposing internal step results.
- Update debugger LIR, artifact, link, and generated-JS views for the representation change. Retain useful source spans/navigation and unambiguous identities for functions, blocks, captures, and temporary values. This work replaces LIR's form rather than adding a required post-link compiler phase.
- Update language/tooling support if the chosen foreign-binding notation changes grammar. Exact notation, serialized field names, concrete Rust data structures, and JS frame layout are implementation choices constrained by this contract; no new source-language async construct is authorized by those choices.
- Implement in verifiable stages: portable CPS and synchronous behavior preservation; managed handler execution across suspension; complete foreign adapters and callback behavior; then documentation, debugger integration, and full acceptance verification. The feature is not complete at the synchronous-trampoline stage.

## Testing Decisions

- Use the existing temporary-project build and Node execution interface as the primary behavioral test seam. Checked-in Ruddy programs plus JavaScript assertions already exercise the same build-and-run path as CLI users. Extend that approach rather than introduce a test-only runtime or a second interpreter.
- The previously agreed completion criteria require independently compiled bundles, linking, and generated-program execution. Complement the primary seam with existing public compile, artifact, link, and debugger interfaces only where rejection, persistence, relocation, or presentation is itself the behavior under test. No new test-only public seam is required.
- Good tests assert results, effect order, module readiness, failure delivery, and observable completion behavior. Do not make application semantics depend on generated helper names, frame object layouts, or incidental temporary numbering.
- Retain focused representation tests where representation is the interface: validated artifact invariants, canonical round trips, relocation, and supported debugger output. Avoid duplicating the same runtime semantics at every compiler phase.
- Existing compile tests provide prior art for accepted and partial compilation, aggregated diagnostics, dependency imports, and tracing-independent causal errors. Add synchronous-adapter eligibility cases through that interface, including conservative refusal when information is unknown.
- Existing artifact tests provide prior art for construction, strict parsing, validation, canonical rendering, recovery, and compatibility. Cover valid CPS round trips and malformed function/block destinations, capture/argument mismatches, wrong entry kinds, unavailable temporaries, and unsafe or incomplete metadata handling.
- Existing link tests provide prior art for concatenation, dependency order, and relocation. Compile and round-trip at least two artifacts independently, link them with nonzero function offsets, and verify continuation destinations, ordinary closures, direct calls, and initializer references relocate correctly while local block identities remain valid.
- Exercise a caller-created continuation passed into dependency code and resumed after that dependency suspends. Verify compilation of the dependency required neither the root's source nor its final function numbering. Linking must not create continuations or infer missing suspension summaries.
- Test deep pure tail recursion, mutual recursion, indirect/higher-order recursion, and non-tail recursion through generated JS, including paths through handlers and generated adapters. Use bounded workloads large enough to exceed ordinary native recursion depth and verify their results.
- Verify proper tail behavior with a bounded resource regression that distinguishes increasing pending return state from constant live return state. Keep this separate from ordinary semantic assertions and avoid brittle exact byte counts or a general performance target. Non-tail recursion is allowed proportional heap growth.
- Retain existing runtime coverage for arithmetic, arrays, records, pattern matching, ordinary closures, currying, effect evidence, unnamed operations, and extern conversion. The calling-convention migration must not weaken those behaviors.
- Test normal operation-arm completion, normal handler return arms, raise bypassing the return arm, nested handlers, recursive handlers, and distinct handler identities. Existing LIR and inference handler tests are prior art for handler answer types differing from operation results.
- For each relevant handler path, test immediate and delayed host completion. Include an arm that suspends and then returns, an arm that suspends and then raises, and nested suspension with evidence restored according to existing scoping.
- Test a retained callback whose ordinary evidence remains usable after registration, repeated calls to it, callbacks returning callbacks, and a callback establishing a fresh handler per invocation. Existing extern callback/evidence fixtures provide prior art, but their immediate invocations must be extended to retained execution.
- Test expired handler exits explicitly. A retained callback reaching raise through captured evidence must fail after the target handler finishes rather than resume old code or deliver the handler answer as its callback result.
- Distinguish liveness from ancestry: while an originating invocation is suspended, an independent callback attempting to raise to its handler must fail without aborting that invocation. Conversely, a synchronous callback nested in a foreign call must retain valid logical ancestry and existing abort behavior.
- Use externally controlled completions to invoke a callback twice before either invocation finishes, then settle them in reverse order. Assert independent results, shared ordinary capture semantics, and no implicit serialization. Repeated invocation must not be mistaken for duplicate settlement of one invocation.
- Cover fixed synchronous and Promise adapter contracts with immediate and delayed completions where permitted. A Promise adapter must return a Promise even on immediate success; a synchronous adapter must never expose an internal step or pending-operation object as the declared result.
- Test immediate-return, Promise, and completion-callback foreign protocols. Include callback completion during registration, completion after registration, duplicate success, success followed by failure, failure followed by success, and completion after an invocation is terminal. Only one result may be delivered.
- Verify immediate completion does not recursively reenter the driver or corrupt a nested foreign call. Preserve visible synchronous effects before the first actual suspension and introduce no event-loop interleaving merely for trampoline transfers.
- Test both synchronous exceptions and asynchronous rejection under the default host-failure policy and an explicit binding translation. Verify Ruddy handler exits remain distinct from host failure and Promise rejection is not silently swallowed.
- Exercise notification failures before and after suspension with an embedding-provided reporter. Test the target default in a child process where necessary so an uncaught error does not terminate the test harness. Do not assert success merely because JavaScript ignored a returned Promise.
- Test suspendable global initializers across dependency and declaration order. Assert module readiness and main invocation occur only after all required initialization, and that failure prevents successful readiness and entry. Include artifact-only compilation to preserve the separation between compilation and execution.
- Existing entry, CLI, and runtime tests provide prior art for executable versus library behavior, output draining, process exit, target validation, and temporary projects. Re-run their contracts under CPS and asynchronous completion.
- Existing debugger stage and snapshot tests provide prior art for phase visibility, artifact dependencies, diagnostics, and source navigation. Verify the existing LIR/artifact/link/JS views expose the new control form coherently without a required post-link CPS stage.
- Use controlled Promises, callbacks, and explicit event logs for asynchronous ordering tests. Avoid relying on arbitrary sleeps. Give subprocess stress and failure cases bounded execution limits so a broken driver cannot hang the suite indefinitely.
- Run the Rust test suite only through `just test`; never invoke `cargo test` directly, including focused invocations. Follow repository coverage requirements and update grammar tests if foreign notation changes. This documentation task does not itself require running the runtime suite.

## Out of Scope

- Backward-compatible decoding or execution of the previous LIR artifact schema, a persisted direct-style-to-CPS compatibility layer, or a required whole-program transformation after linking.
- Source-visible async/await, a mandatory Async effect, separate synchronous/asynchronous Ruddy function types, or Promise-shaped ordinary Ruddy return types.
- User-visible continuation capture, explicit resume, multi-shot resumptions, continuation cloning, or changes to the source meaning of handler return and raise.
- General static capture/lifetime inference, region checking, or static certification that every retained callback is free of invalid handler exits. Runtime validity checks are required instead.
- Automatically reinstalling an escaped enclosing handler around a retained callback, reviving a completed handler, or turning an unrelated invocation's handler exit into cross-invocation cancellation.
- A universal JavaScript callback that can make arbitrary synchronous host code await an asynchronous result, or a wrapper that alternates between a value and a Promise.
- Bounded stack usage for recursive host code or unbounded synchronous alternation between foreign calls and callbacks. Constant total memory for non-tail recursion is not promised.
- User-visible cancellation, cancellation of underlying host operations, structured task scopes, and a generalized resource-cleanup protocol. Terminal-state protection against late completion is in scope.
- Automatic instruction-budget yields, fair task scheduling, implicit serialization of retained callbacks, new spawn/join interfaces, worker execution, shared-memory parallelism, or new thread-safety guarantees. Independent overlapping host callback invocations are in scope.
- A new Rust or WebAssembly backend, expanded platform support, or a new browser executable entry adapter. The portable representation must permit future backends without requiring them in this migration.
- Broad source optimization, general higher-order suspension inference, allocation elimination, direct-call specialization, and a comprehensive benchmark program. Stack safety and proper tail behavior are required; performance refinements can follow measurements.

## Further Notes

The discussion explicitly settled the representation and runtime contracts before this specification was requested. Earlier suggestions for a post-link CPS pass, a required Async effect, and prerequisite static lifetime checking were superseded by the decisions above.

The current implementation already supplies lifted functions, explicit ordinary captures, effect evidence, reviewed foreign conversion plans, validated artifacts, and mechanical function relocation. The migration replaces its implicit return and native exception control assumptions; it does not require redesigning structural types or effect identity.

The artifact compatibility break is explicitly authorized for this feature and supersedes compatibility-preserving assumptions in earlier compiler-refactoring work. Core compilation must still stop at a portable validated artifact, and linking/backend responsibilities remain distinct.

The completion criterion is an independently compiled multi-bundle program that links and executes with bounded Ruddy host-stack use, suspends in an operation arm, resumes correctly on success and failure, preserves handler return/raise behavior, and supports retained and overlapping callback invocations. A working trampoline or an isolated Promise demonstration alone does not complete the feature.

No prototype was produced during the design discussion. Proposed data layouts and internal names remain implementation choices; the observable contracts, relocation rules, and deferred scope are the agreed decisions.
