# Typed Effect Parameters

Status: ready-for-agent

## Problem Statement

Ruddy effects currently have one fixed operation interface. An effect declaration cannot bind parameters, and an operation signature containing a type variable or open row is rejected. This prevents reusable effects whose operation types depend on the computation using them, such as an `Ask` effect that returns a caller-selected type or a `State` effect whose operations share one state type.

Treating every applied interface as an unrelated effect would lose an important invariant. Within one computation, two occurrences of the same effect constructor must describe one coherent instantiation. If independently inferred occurrences select incompatible arguments, the program must be rejected rather than represented as two separately handled effects.

Generic effect aliases must participate in the same model. They need to forward parameters and produce effect rows, including open rows, without becoming runtime effects themselves. The compiler must preserve these semantics through parsing, kind inference, row unification, handler checking, diagnostics, structural identity, artifacts, dependency imports, printing, and lowering.

## Solution

Allow concrete effects and effect aliases to declare ordered parameters using the existing declaration parameter syntax. An effect used in a type-level effect row is fully applied using the existing type-argument grammar, such as `!Ask Nat`. Parameters may have any of the four existing declaration kinds: an ordinary type, struct fields, sum cases, or effects. Presence remains annotation-only and is not an effect parameter kind.

Represent an applied concrete effect as a structural effect constructor plus its ordered arguments. A row may contain at most one application of a constructor. Independently inferred applications of the same constructor unify their corresponding arguments and coalesce when successful; incompatible arguments produce a causal type error. In contrast, directly writing the same constructor more than once in one row is always a duplicate-effect error, even if its arguments could unify.

Instantiate an effect's parameters freshly whenever an operation is referenced. Term syntax remains unchanged: operation arguments are runtime values, while effect type arguments are inferred. Every operation of a multi-operation effect, every contributing inference path, and every arm of a handler constrain the same application carried by the surrounding effect row. Effect arguments are erased before runtime dispatch.

Treat parameterized effect aliases as checked compile-time row macros. An alias may forward parameters, combine concrete effects, and splice an open effects parameter into its output row. Expansion preserves and propagates constructor-level lacks constraints. Aliases never acquire runtime identity and cannot be performed directly.

Publish parameterized concrete effects and unexpanded generic aliases in a new artifact schema. Backward compatibility with artifacts using the previous effect representation is not required.

## User Stories

1. As a Ruddy programmer, I want to declare an effect with an ordinary type parameter, so that one effect interface can operate over a caller-selected type.
2. As a Ruddy programmer, I want an effect parameter to appear in an operation input, so that performing the operation constrains the effect application from the supplied value.
3. As a Ruddy programmer, I want an effect parameter to appear in an operation result, so that using the result constrains the effect application.
4. As a Ruddy programmer, I want several operations of one effect to share its parameters, so that operations such as state reads and writes agree on one state type.
5. As a Ruddy programmer, I want effects to accept struct-fields parameters, so that an operation interface can be generic over a record remainder.
6. As a Ruddy programmer, I want effects to accept sum-cases parameters, so that an operation interface can be generic over a choice remainder.
7. As a Ruddy programmer, I want effects to accept effects parameters, so that an operation interface can describe callbacks with caller-selected effects.
8. As a Ruddy programmer, I want effect parameter kinds inferred from their uses, so that effect declarations use the same concise parameter syntax as type declarations.
9. As a Ruddy programmer, I want a parameter forwarded to another declaration to inherit that declaration's kind, so that generic interfaces compose without redundant kind annotations.
10. As a Ruddy programmer, I want mixed uses of one parameter reported at its declaration, so that I can correct an incoherent generic interface at its source.
11. As a Ruddy programmer, I want unused effect parameters to default to the ordinary type kind, so that their meaning is deterministic.
12. As a Ruddy programmer, I want every declared effect parameter to remain semantically relevant, so that even phantom applications with incompatible arguments cannot silently coalesce.
13. As a Ruddy programmer, I want effect applications to be fully applied, so that an effect row never contains a constructor with unknown arity.
14. As a Ruddy programmer, I want clear arity errors for missing or extra effect arguments, so that malformed applications are easy to repair.
15. As a Ruddy programmer, I want clear kind errors at effect arguments, so that a type cannot accidentally be supplied where a particular row kind is required.
16. As a Ruddy programmer, I want effect arguments to use familiar type-argument syntax, so that generic types and generic effects feel consistent.
17. As a Ruddy programmer, I want compound effect arguments to obey normal application precedence, so that parsing remains predictable.
18. As a Ruddy programmer, I want operation references to infer their effect arguments, so that runtime application syntax is not confused with type application syntax.
19. As a Ruddy programmer, I want an operation reference to receive fresh parameters, so that a polymorphic operation value can be instantiated at each use.
20. As a Ruddy programmer, I want normal let generalization to include inferred effect arguments, so that reusable operation values remain polymorphic.
21. As a Ruddy programmer, I want separately inferred occurrences of one effect to coalesce when their arguments unify, so that ordinary inference does not create duplicate effects.
22. As a Ruddy programmer, I want separately inferred occurrences of one effect to fail when their arguments do not unify, so that one computation cannot use incompatible versions of the same effect.
23. As a Ruddy programmer, I want incompatible occurrences on different branches to remain an error, so that effect instantiation is not flow-sensitive.
24. As a Ruddy programmer, I want conditional occurrences of one constructor to unify their arguments, so that conditional presence does not split effect identity.
25. As a Ruddy programmer, I want negative occurrences of a constructor to participate in the same uniqueness rules, so that absence and presence cannot disagree about which application they name.
26. As a Ruddy programmer, I want directly repeated effect constructors rejected even when their arguments agree, so that redundant written rows are always diagnosed.
27. As a Ruddy programmer, I want directly repeated effect constructors rejected when their arguments differ, so that duplicate syntax takes precedence over a secondary unification failure.
28. As a Ruddy programmer, I want aliases that expand to overlapping constructors rejected, so that indirect duplication is not silently accepted.
29. As a Ruddy programmer, I want an inferred argument mismatch distinguished from a written duplicate, so that the diagnostic identifies the mistake I can act on.
30. As a Ruddy programmer, I want handler arms to infer the handled effect's arguments from the computation and arm bodies, so that handlers need no explicit type arguments.
31. As a Ruddy programmer, I want every arm for a multi-operation effect to share one instantiation, so that a handler cannot implement mutually inconsistent operations.
32. As a Ruddy programmer, I want a handler to remove the inferred applied effect from the handled computation, so that the remaining effect row is accurate.
33. As a Ruddy programmer, I want nested function types in operation interfaces to carry declared open effects, so that an operation can accept effect-polymorphic callbacks.
34. As a Ruddy programmer, I want declared field and case tails accepted in operation interfaces, so that a generic operation interface is considered fixed by its declaration.
35. As a Ruddy programmer, I want anonymous open rows and holes in operation interfaces rejected, so that every accepted generic interface has explicit parameters.
36. As a Ruddy programmer, I want an operation's outer arrow to remain pure, so that performing one operation introduces only its declared effect application.
37. As a Ruddy programmer, I want to declare parameterized effect aliases, so that reusable groups of effects can forward type and row information.
38. As a Ruddy programmer, I want an effect alias to splice an effects parameter, so that it can extend an arbitrary caller-selected effect row.
39. As a Ruddy programmer, I want open alias parameters to carry constructor-level lacks constraints, so that an argument cannot reintroduce an effect supplied by the alias.
40. As a Ruddy programmer, I want lacks constraints propagated through nested aliases and open arguments, so that overlap is rejected at the most useful use site.
41. As a Ruddy programmer, I want alias applications expanded before row unification and handler coverage, so that aliases have exactly the semantics of the rows they produce.
42. As a Ruddy programmer, I want aliases to remain unperformable, so that only concrete effects declare operations and runtime identities.
43. As a Ruddy programmer, I want a forwarding identity alias to be valid, so that an alias may return its effects argument unchanged.
44. As a Ruddy programmer, I want recursive alias-expansion cycles rejected, so that expansion always terminates.
45. As a Ruddy programmer, I want growing alias cycles diagnosed distinctly, so that a cycle that adds row structure is not mistaken for ordinary forwarding.
46. As a Ruddy programmer, I want a negative closed alias application to make every expanded effect absent, so that modifiers distribute predictably.
47. As a Ruddy programmer, I want a conditional closed alias application to apply its condition to every expanded effect, so that an alias behaves as one written group.
48. As a Ruddy programmer, I want modified aliases that retain an unknown open tail rejected, so that the compiler does not invent unrepresentable conditional or negative tails.
49. As a Ruddy programmer, I want recursive concrete effect interfaces to remain valid, so that an operation type may refer back to its parameterized effect without being treated as an alias cycle.
50. As a Ruddy programmer, I want structurally equivalent generic effects to coalesce across modules, so that the current structural effect model remains intact.
51. As a Ruddy programmer, I want alpha-renamed parameter declarations to have the same structural identity, so that parameter spelling does not affect semantics.
52. As a Ruddy programmer, I want differently kinded or differently shaped generic interfaces to remain distinct, so that accidental name matches do not merge incompatible constructors.
53. As a Ruddy programmer, I want applied effect arguments shown in diagnostics, so that an error identifies the actual instantiation involved.
54. As a Ruddy programmer, I want causal details for inferred effect-argument mismatches, so that I can see which corresponding arguments failed to unify and why.
55. As a Ruddy programmer, I want parameterized effects printed canonically in inferred types and debugger views, so that compiler output can be read and recompiled consistently.
56. As a library author, I want exported effects to preserve parameter kinds and generic interfaces, so that consumers kind-check applications exactly as producers do.
57. As a library author, I want exported generic aliases to preserve their parameterized row expressions, so that open aliases expand using consumer arguments.
58. As a library consumer, I want imported effects and aliases to behave exactly like local declarations, so that package boundaries do not change type inference.
59. As a compiler caller, I want malformed imported effect applications diagnosed or recovered safely, so that invalid foreign data cannot crash compilation.
60. As a runtime user, I want effect parameters erased before dispatch, so that typed effects require no runtime type representation or new handler key.
61. As a compiler maintainer, I want one shared effect-application path for annotations, type arguments, extern declarations, imports, negative labels, and conditional labels, so that language features cannot drift apart.
62. As a compiler maintainer, I want the artifact format to represent generic effects directly, so that structural identity is not reconstructed from lossy data.
63. As a compiler maintainer, I want the artifact schema change to be explicit and breaking, so that compatibility code does not obscure the new invariants.
64. As a tooling author, I want effect application navigation to target the constructor declaration, so that added arguments do not weaken source navigation.

## Implementation Decisions

- Concrete effect declarations and effect aliases gain ordered declaration parameters.
- Effect parameters use the same sigil and declaration style as type parameters.
- The supported parameter kinds are ordinary types, struct fields, sum cases, and effects. Presence remains annotation-only.
- Parameter kinds are inferred from all uses by a shared fixpoint spanning local type declarations, concrete effects, aliases, imported declarations, and recursive references.
- A parameter passed directly to another declaration inherits the destination parameter's kind and lacks constraints.
- A parameter used at incompatible kinds invalidates its declaration and receives a mixed-parameter diagnostic at the declaration parameter.
- An unused parameter defaults to the ordinary type kind.
- Every effect parameter is semantically relevant, including a parameter absent from all operation signatures or alias branches.
- Effect occurrences in type-level rows must be fully applied. Partial application, default arguments, higher-kinded parameters, and named arguments are not introduced.
- Effect application arguments use the existing type-argument grammar and are checked against ordered parameter kinds.
- Applied concrete effects are represented by a structural constructor identity plus ordered, kinded arguments.
- Structural constructor identity contains the effect's leaf name, arity, ordered parameter kinds, and alpha-normalized open operation interface.
- Parameter names do not participate in structural identity. Parameter positions and their uses do.
- Two same-named declarations with equivalent generic interfaces and parameter metadata have the same constructor identity, including across modules.
- Two applications share a row label when their constructor identities match. Their corresponding arguments are then unified according to their kinds.
- Ordinary type arguments use type unification. Fields, cases, and effects arguments use their corresponding row unification and lacks rules.
- Independently inferred compatible applications coalesce into one effect-row entry.
- Independently inferred incompatible applications produce a dedicated effect-argument type error with the underlying unification failure retained as causal detail.
- The one-application-per-constructor invariant applies regardless of control-flow branch or conditional presence; inference is not flow-sensitive by effect argument.
- A single explicitly written row may name a constructor only once, regardless of arguments, qualification, presence, or whether the arguments would unify.
- Written duplicate detection happens before argument unification and emits only the duplicate diagnostic for that occurrence.
- Alias branches are checked after transitive generic expansion. Branches whose expansions overlap on a constructor are written duplicates.
- Constructor-level lacks constraints prevent an open row tail from containing any application of a constructor already supplied by the surrounding row.
- Operation references instantiate every effect parameter freshly and add the resulting applied effect to the operation's inferred arrow.
- Term-level operation and handler syntax does not accept explicit effect type arguments because the existing following term is the runtime operation argument.
- Ordinary inference, annotation checking, and let generalization constrain and quantify fresh effect arguments.
- Every operation of one effect declaration uses the same ordered declaration parameters.
- A handler infers one applied effect from its handled computation and all matching operation arms. Handling removes that application and preserves the remaining row.
- Effect arguments are erased for lowering and runtime dispatch. Runtime identity continues to use the structural effect constructor and operation selector.
- Declared parameters may open field, case, or nested effect rows inside an operation's input and result types.
- Anonymous open tails, holes, conditional presences, and undeclared variables remain invalid in operation interfaces.
- An operation declaration's outer arrow cannot declare effects. Effectful arrows nested in its input or result are allowed when otherwise well formed.
- A parameterized alias is a compile-time function from checked arguments to an effect row. It has no row-label or runtime identity of its own.
- Alias bodies may contain concrete or alias applications and may splice one declared effects parameter as an open tail.
- Open alias rows infer and propagate constructor-level lacks constraints through applications and retained tails.
- Alias expansion occurs before effect-row unification, duplicate checking, handler coverage, and semantic publication.
- An identity alias that returns only its effects parameter is valid.
- Every recursive alias-expansion cycle is invalid. Diagnostics distinguish pure forwarding cycles from cycles that grow or transform row structure.
- Recursive references to concrete effects within operation interfaces remain valid and use graph backreferences during canonicalization.
- A positive, unconditional alias application may expand to an open row.
- A negative or conditional alias application is valid only when argument substitution produces a closed row.
- For a valid modified alias, the outer presence is distributed to every concrete expanded label and combined with inner presences using normal presence rules.
- Parameterized concrete effects and aliases share one application and expansion implementation across all effect-row consumers, including annotations, declared type arguments, extern declarations, imports, absent labels, and conditional labels.
- User-facing diagnostics render applied arguments when known. Unknown arguments use normal inferred-variable rendering.
- Arity and kind diagnostics name the effect, parameter position, expected kind, and offending argument. Lacks diagnostics identify the first prohibited constructor.
- Navigation from an application continues to target the source constructor or alias declaration.
- Canonical type and debugger printers render effect applications with normal type-application precedence and enough grouping to round-trip.
- Artifact declarations publish ordered effect parameter metadata and the normalized generic concrete interface or generic alias row expression.
- Generic aliases remain unexpanded in artifacts because open applications require use-site substitution.
- The artifact schema change is breaking. Readers are not required to accept artifacts from the prior zero-parameter effect schema.
- Artifact validation checks effect arity, parameter kinds, bound positions, alias rows, lacks metadata, structural identities, and cycle safety before imported declarations become trusted.
- There are no user-written type-equality constraints, parameter defaults, partial effect applications, higher-kinded effect parameters, or first-class presence arguments.

## Testing Decisions

- Tests assert observable source-language behavior and public compiler results rather than private representation layout, traversal order, or helper calls.
- The primary test seam is core parsed-bundle compilation. Source snippets are parsed and passed through the production compilation interface; successful cases inspect accepted semantic types and artifacts, while failure cases inspect structured partial-compilation errors.
- Core compilation tests cover ordinary type parameters in operation inputs and results, shared parameters across several operations, inferred operation applications, polymorphic operation values, handlers, branches, annotations, and extern declarations.
- Core compilation tests cover all four parameter kinds, kind propagation through type and effect declarations, unused parameters, mixed kinds, arity errors, kind errors, and lacks violations.
- Core compilation tests distinguish compatible independently inferred duplicates, incompatible independently inferred applications, and every form of explicitly written duplicate.
- Core compilation tests cover duplicates introduced by qualified structurally equivalent effects, alias expansion, alias arguments, negative labels, and conditional labels.
- Core compilation tests cover parameterized aliases, effect-row tails, propagated lacks constraints, identity aliases, nested aliases, closed modified aliases, rejected modified open aliases, forwarding cycles, and growing cycles.
- Core compilation tests cover handler inference across all operations, mismatch reporting between arms, removal of the handled application, and preservation of unrelated effects.
- Core compilation tests cover allowed declared open rows in operation interfaces and continued rejection of holes, anonymous tails, conditional presences, undeclared variables, and outer-arrow effects.
- Existing IR effect-declaration, type-parameter kind inference, row-lacks, alias expansion, structural identity, operation lookup, and handler-coverage tests are prior art for these cases.
- Existing inference row-unification, scheme generalization, causal explanation, handler masking, effect-boundary, and callback-effect tests are prior art for inferred application behavior.
- Parser and canonical-printer round-trip tests are limited to the new effect declaration parameters, effect applications, alias tails, precedence, grouping, absent labels, conditional labels, and qualified names.
- Existing parser effect declaration and type application tests provide prior art for the surface syntax seam.
- Artifact tests round-trip parameterized concrete effects and generic open aliases, validate ordered parameter metadata and bound positions, and reject malformed arity, kind, identity, alias-tail, and cycle data under the new schema.
- Dependency compilation tests export an artifact and compile a consumer against it, covering structural coalescing, alpha-renamed parameters, generic alias substitution, open tails, and diagnostics involving imported applications.
- Existing artifact semantic round-trip, strict validation, tolerant recovery, and dependency effect-identity tests provide prior art for the persistence seam.
- One representative compile-to-JavaScript execution test performs and handles a parameterized effect, proving that type arguments are erased and existing runtime dispatch behavior is unchanged.
- Existing stage and standard-library execution tests provide prior art for the runtime seam.
- UI tests pin diagnostic codes, titles, primary and secondary spans, causal details, help text, and instantiated effect rendering for duplicate, arity, kind, lacks, alias-cycle, modifier, and inferred-argument errors.
- Existing UI inventory, diagnostic prose, semantic type rendering, and print-round-trip tests provide prior art for the presentation seam.
- Deep and mutually recursive generic interfaces and alias chains receive bounded-stack stress tests at the artifact and compilation seams.
- The complete Rust test suite is run only through the repository's `just test` recipe.

## Out of Scope

- First-class presence parameters on effect declarations or aliases.
- User-written kind annotations for effect parameters.
- User-written type-equality constraints on effect declarations.
- Default, optional, named, or partially applied effect parameters.
- Higher-kinded parameters or passing an effect constructor itself as an argument.
- Flow-sensitive effect instantiation or permitting different applications of one constructor on mutually exclusive branches.
- Allowing a directly written duplicate merely because its arguments unify.
- Runtime reification of type or row arguments.
- Runtime dispatch by applied effect arguments.
- Making effect aliases directly performable or giving them independent runtime identities.
- Conditional or negative unknown row tails.
- Allowing effects on an operation declaration's outer arrow.
- Preserving compatibility with artifacts produced under the previous effect schema.
- Unrelated changes to value syntax, handler control flow, backend protocols, or effect-row upper-bound semantics.

## Further Notes

- The distinction between written and inferred duplication is intentional. A written row is checked for constructor uniqueness as authored; inference is allowed to discover the same constructor along independent paths and reconciles those paths by argument unification.
- Structural identity belongs to the generic constructor, not to a fully substituted interface. Applications of structurally equivalent constructors can meet and unify their arguments, while generically different constructors do not become equal merely because one particular substitution makes their closed interfaces resemble each other.
- Open aliases are row-producing applications rather than effects. Keeping them unexpanded in artifacts and expanding them at use sites is necessary for correct substitution, lacks propagation, and diagnostics.
- The branch was rebased onto the current `origin/main` before this specification was published.
