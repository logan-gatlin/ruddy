# Construction evidence requirements for Mirror

Status: implemented

## Problem Statement

An inferred return type can currently obtain a mirror even when no value of that
type can be constructed. Generic producers built on reflection must therefore
discover empty types, inaccessible constructors, and recursive types without a
finite value at runtime. This makes otherwise ordinary operations, such as
structural random generation, fallible because their result type is unsupported.

Inhabitation alone does not solve the problem. An opaque resource can have values
without giving a generic library permission or operations to create one. Equally,
an inhabited aggregate can contain an uninhabited part: an option of an empty type
has its absent value, and an array of an empty type has its empty value. Rejecting
such aggregates would unnecessarily restrict useful generic code.

Users want the familiar inferred mirror request, with a stronger guarantee:
receiving a mirror means accessible, finite construction evidence is available.
Unsupported requests should fail at compilation rather than during construction.
Describing types, comparing exact type identities, and observing existing values
must remain possible without granting construction authority.

## Solution

Make Mirror an authenticated capability for typed reflection with accessible,
finite construction evidence. Keep reflect::mirror () as the inferred entry
point. The expected type, an annotation, a use, or a generic caller determines the
reflected type; the compiler supplies the required evidence or diagnoses its
absence. Generic functions retain and forward that requirement to their callers.

Constructive views expose mirrors only for constructible parts. Proven impossible
variant cases remain in the full description but do not appear as constructible
cases. Arrays with a proven empty element type expose an explicit empty-only
view. Missing evidence for a possibly inhabited part is an error, not permission
to silently discard that part.

Every mirror provides a pure operation that executes a finite construction of its
type, in addition to typed structural constructors. General descriptions remain
available for empty and unsupported types through separately authenticated
description-only evidence. The implementation may share the underlying type
graph, but description-only evidence cannot be promoted into a mirror without
satisfying the construction requirement.

This spec establishes the reflection contract. It does not implement a generic
random generator or promise that every operation consuming a mirror is total.

## User Stories

1. As a library caller, I want to request a mirror using the existing inferred entry point, so that I do not need to name the type separately from its ordinary use.
2. As a library caller, I want an annotation to select the mirrored type, so that return-directed reflection remains straightforward.
3. As a library caller, I want later uses to constrain a mirror's type, so that inference remains useful without redundant annotations.
4. As a compiler user, I want a diagnostic when a mirror request remains ambiguous, so that the compiler does not select an arbitrary constructible type.
5. As a generic library author, I want construction requirements to propagate through wrappers, so that callers satisfy the requirements of the operations they use.
6. As a generic library author, I want partial applications and returned closures to retain their evidence, so that construction remains valid after the original call returns.
7. As a library publisher, I want construction requirements preserved in compiled interfaces, so that separately compiled consumers receive the same checks as local callers.
8. As a library caller, I want an empty result type rejected before execution, so that type-driven production does not need a runtime unsupported-type branch.
9. As a library caller, I want inaccessible constructors distinguished from empty types, so that diagnostics explain whether construction is impossible or merely unavailable.
10. As a generic library author, I want a mirror to provide a finite construction, so that recursive constructor enumeration does not leave me without a terminating starting value.
11. As a generic library author, I want primitive mirrors to retain exact numeric domains and typed conversions, so that construction remains portable.
12. As a generic library author, I want mirrors for every required field of a constructible record, so that I can recursively construct the record.
13. As a generic library author, I want constructible variants to expose typed injections directly, so that an advertised constructor cannot later report that its authority is absent.
14. As a generic library author, I want impossible variant cases omitted from constructive enumeration, so that I never need to manufacture their payloads.
15. As a schema author, I want impossible cases retained in the complete description, so that constructive filtering does not rewrite the exact type being described.
16. As a library caller, I want an option of an empty type to remain reflectable, so that its valid absent value remains usable.
17. As a library caller, I want a result with an empty error type to remain reflectable, so that infallible computations can still use generic libraries.
18. As a library caller, I want an array of an empty type to remain reflectable, so that its valid empty value can still be constructed and inspected.
19. As a generic library author, I want an explicit empty-only array view, so that traversing an array mirror never exposes a mirror for an empty element type.
20. As a library caller, I want a possibly inhabited but unsupported array element type diagnosed, so that generation does not silently restrict my array to the empty value.
21. As a library caller, I want valid variant alternatives preserved, so that unavailable construction support does not silently change the set of values a generic library can produce.
22. As a recursive data author, I want finite construction paths discovered through mutually recursive types, so that legitimate recursive structures remain supported.
23. As a compiler user, I want construction analysis to terminate on recursive types, so that requesting a mirror cannot hang compilation or exhaust the stack.
24. As an abstraction author, I want hidden representation and constructor authority preserved, so that reflection cannot create values my module has not authorized.
25. As an abstraction author, I want an explicitly carried mirror to remain usable after opening a hidden type, so that authorized construction can be passed through ordinary packages.
26. As a reflection user, I want descriptions of empty, callable, and opaque types, so that inspection is not restricted to types that can be generically constructed.
27. As a user of dynamic packages, I want existing values to retain exact checked casting without requiring constructors, so that strengthening Mirror does not unnecessarily narrow Any.
28. As a user of display and comparison, I want observational operations to keep their existing supported domains, so that they do not acquire unrelated construction requirements.
29. As a codec author, I want constructive views that work for empty nested types, so that valid absent cases and empty arrays can round-trip.
30. As a codec user, I want malformed input to remain a checked error, so that construction evidence is not mistaken for proof that external data is valid.
31. As a compiler user, I want errors to identify the field, case, or element that lacks evidence, so that I can fix a nested construction failure.
32. As a backend implementer, I want portable construction evidence and identical structural behavior, so that interpreter and JavaScript consumers agree.
33. As a debugger user, I want to see construction requirements distinctly from descriptive requirements, so that evidence failures are understandable.
34. As a maintainer, I want authenticated evidence validated across artifact boundaries, so that malformed compiled input cannot claim unsupported construction authority.
35. As a library author, I want the supported automatic construction domain documented, so that I can decide when to pass an explicit construction operation instead.

## Implementation Decisions

- **Entry point and type inference.** Preserve reflect::mirror () and its
  type-indexed Mirror result. Preserve annotation-directed and use-directed
  inference. Mirror construction requests add a requirement; they do not infer a
  type from runtime values or use constructibility to choose among ambiguous
  types. No new source-level constraint syntax is required for this feature.

- **One stronger Mirror contract.** Every successfully obtained Mirror must
  carry authentic exact type evidence, accessible typed construction operations,
  and a finite construction recipe. A legacy description-only descriptor is not
  a valid Mirror. Apply the same requirement to reflect::type_of while it returns
  Mirror; possession of an arbitrary value must not bypass the rule or cause
  type_of to capture that value as a default constructor.

- **Finite construction operation.** Expose a pure reflection operation taking
  a Mirror and returning a value of its indexed type without an unsupported-type
  Result. It executes a compiler-validated finite recipe. It must not inspect an
  arbitrary existing value, invoke user callbacks, allocate region-bound cells,
  request randomness, or perform host/resource effects. Normal allocation of
  immutable data is permitted. Structural recipe selection must be deterministic
  across backends and independent of descriptor allocation identity. Document
  the selection rule; it is not a claim about a statistically useful distribution.

- **Finite evidence is stronger than constructor signatures.** A cycle of
  constructors with no finite starting value cannot establish construction
  evidence. The compiler must produce a finite recipe, not merely assert a
  boolean inhabited flag or install an endlessly recursive constructor thunk.
  Executing the supplied recipe terminates for the supported structural domain,
  subject to ordinary resource limits. This is not a general termination proof
  for user programs that traverse the mirror.

- **Construction classification.** Distinguish supported construction,
  proven uninhabitation, and unavailable evidence. Unavailable includes abstract
  structure, unsupported construction, and requirements still deferred to a
  caller. Only a sound proof of uninhabitation permits removing a part from
  constructive views. An unsupported node or unresolved type variable must not
  be treated as empty.

- **Primitive and record rules.** Automatically support the existing immutable
  primitive data kinds, unit, and records whose required fields have construction
  evidence. Retain exact target integer domains and primitive identities. Empty
  records have a finite construction. A required field proven empty makes the
  record empty; a possibly inhabited field without evidence blocks automatic
  mirror derivation. Preserve current row and presence semantics: absence is not
  a value of an empty payload type, and an unknown remainder is not an empty row.

- **Variant rules.** A reflected variant must have at least one finite
  construction, and every potentially inhabited case must have usable payload
  construction evidence. Omit only proven impossible cases from constructive
  enumeration. Each exposed case carries its payload mirror and an available
  typed injection; absence of injection authority must be diagnosed before a
  mirror is provided. Preserve projection behavior for all realizable values.
  Keep impossible cases in the full type description and exact identity.

- **Array rules.** An array with constructible elements uses the ordinary typed
  element view. An array with a proven empty element type uses a separate
  empty-only constructive view, with operations sufficient to read and make its
  sole array value and with descriptive element information, but no element
  Mirror. A possibly inhabited element type with unavailable evidence blocks
  automatic array mirror derivation, even though the empty array exists. This
  stronger rule preserves complete constructive traversal of all possible parts.

- **Recursive structural types.** Analyze the existing finite semantic type
  graph with a terminating fixed-point/worklist procedure or an equivalent
  bounded algorithm. Discover finite constructions through actual base cases,
  including empty arrays and terminating variant alternatives. Recursive edges
  alone provide no positive evidence. Within the supported data domain, classify
  types with no finite values as empty; opaque or unsupported structure must
  remain unavailable unless emptiness follows independently. Preserve the
  compiler's existing rules for admissible recursive types and polymorphic
  recursion; do not introduce unrestricted theorem proving or unfolding.

- **Abstraction and scope.** A concrete type hidden behind an opened binder
  can use a Mirror explicitly carried by its producer. Merely opening a package,
  observing a value, or receiving description-only evidence does not authorize
  constructors. Ordinary aliases remain structural and do not gain nominal
  privacy rules. Preserve region, presence-owner, and effect dependencies of
  evidence and reject their escape. Functions, foreign resources, mutable cells,
  and hidden packages do not gain automatic generic constructors in this feature.
  Any supported construction of evidence types themselves must satisfy the same
  finite-recipe and authentication rules rather than recursively assuming the
  evidence being requested.

- **Description-only evidence.** Preserve a separately authenticated,
  type-indexed route for obtaining exact type information without construction
  authority, both from an inferred type position and from an existing value's
  static type. It must work for empty types and retain existing support for
  callable, hidden, and region-sensitive descriptions within their valid scopes.
  Expose ordinary Description data from it and allow a Mirror to provide the same
  descriptive evidence. Exact typed equality may use authenticated descriptive
  evidence; it never constructs a value of an empty type. Edited Description
  records, names, hashes, and wire identifiers grant neither equality evidence
  nor construction authority. Choose consistent public names during implementation
  and document the migration; retaining reflect::mirror () is mandatory.

- **Shared representation, distinct demands.** Reuse the existing semantic
  type graph and reification machinery where possible. Descriptive evidence and
  construction evidence must be distinguishable in requirements and validation,
  without creating unrelated competing definitions of exact type identity. No
  runtime fallback may turn a failed construction requirement into a descriptive
  Mirror. Mirror operations that return nested mirrors inherit the stronger
  contract without requiring the caller to revalidate them dynamically.

- **Generic and compiled interfaces.** Infer construction demands through
  generic calls, higher-order values, curried calls, recursive groups, captures,
  imports, and effect-operation values using the existing evidence-propagation
  discipline. A generic definition is valid with a deferred requirement; the
  concrete use must supply evidence or fail compilation. Export enough demand
  information for separate compilation and include the new evidence kinds and
  recipes in artifact validation and backend lowering. Version or reject
  incompatible older artifacts rather than interpreting their mirrors under the
  stronger contract without checking them.

- **Consumer migration.** Update reflection, dynamic packages, structural
  display/comparison/hashing, codecs, and foreign adaptation according to the
  evidence each operation needs. Observation and exact casting must not acquire
  constructor requirements merely because they previously shared Mirror
  machinery. In particular, migrate Any's identity evidence and its checked cast
  path so currently supported function values remain packageable and recoverable.
  Preserve existing region restrictions. Update hidden-package examples and
  evidence recognition where the distinction affects consumers.

- **Codec behavior.** Adapt constructive codec traversal to the revised variant
  and array views. Accept valid empty-only arrays and realizable variant cases;
  reject nonempty input for an empty-only array and input selecting an impossible
  case. Retain record-builder validation for duplicate, missing, unknown, and
  mismatched bindings. Constructor evidence does not eliminate malformed input,
  format limits, numeric range errors, or codec-specific unsupported policies.
  Constructive filtering must not silently renumber existing wire tags or change
  schema identities; preserve existing identities or perform an explicit,
  documented compatibility migration.

- **Diagnostics and tools.** Report the requested type and the path through
  fields, cases, or elements to the unavailable requirement. Distinguish proven
  emptiness, missing construction support, insufficient scoped evidence, and
  ambiguity. Show the concrete calling use when a generic requirement fails.
  Extend debugger evidence/constraint views and artifact diagnostics alongside
  compiler changes. Regenerate the grammar only if implementation introduces a
  grammar change; no new grammar is required by this spec.

- **Public contract documentation.** Update the reflection reference, inferred
  type/evidence explanations, dynamic package examples, codec guidance, and
  debugger documentation together. Explain the difference between a plain
  description, authenticated descriptive evidence, and a constructive Mirror.
  State the supported automatic construction domain and how producers can carry
  existing authorized mirrors through hidden packages. Do not present an
  arbitrary resource factory as a supported way to forge Mirror evidence.

## Testing Decisions

- The user confirmed the primary seam: compiled consumer programs using the
  public reflection interface, with interpreter/JavaScript agreement and
  compile-failure cases for missing construction evidence. Test observable
  results, type acceptance, diagnostics, and capability restrictions. Avoid tests
  tied to descriptor node numbering, dictionary layouts, solver passes, caches,
  or the particular fixed-point implementation.
- Reuse the existing reflection parity, generic callable evidence, hidden
  package, separate-compilation, and standard-library consumer test harnesses.
  Test reflection directly and through dynamic packages and codecs at this same
  consumer seam. Use artifact loading as an additional existing seam only where
  malformed external evidence cannot be expressed by a well-typed source program.
- Check annotation-directed and use-directed mirror inference; unresolved result
  ambiguity; generic wrappers; partial applications; returned closures; nested
  callbacks; recursive forwarding; and compiled producer/consumer dependencies.
  Unsupported concrete instantiations must fail at the appropriate use while
  generic definitions can retain a requirement.
- Exercise pure finite construction of every supported primitive category,
  unit, records, variants, ordinary arrays, and recursive data on both backends.
  Cover supported target integer domains and exact type distinctions. Verify
  construction does not require a Random or host handler and leaves unrelated
  caller state untouched.
- Cover an empty variant, a record with an empty required field, an option of an
  empty type, a result with an empty error type, an array of an empty type, and
  nested combinations. Assert which mirror requests compile, which constructors
  are exposed, what finite values are returned, and that descriptions still
  contain the complete original structure.
- Pair every relevant proven-empty case with an unsupported-but-possibly-
  inhabited case. A variant with an opaque alternative must not silently omit
  that alternative; an array of an opaque element must not silently become
  empty-only. Unresolved generic evidence must not be classified as empty.
- Exercise mutually recursive variants with and without a finite base case,
  recursion terminating through an empty collection, and deeper existing
  recursive-type fixtures. Demonstrate successful finite construction where
  supported and bounded compilation with a useful diagnostic otherwise. Do not
  claim to prove that arbitrary user traversal terminates.
- Verify constructors cannot be obtained by editing descriptions, retyping
  evidence, supplying a mismatched intrinsic declaration, or loading an artifact
  that claims construction authority it lacks. Preserve hidden-type and region
  escape checks. Verify an explicitly carried valid Mirror remains usable after
  opening and repackaging a hidden type.
- Verify description-only requests for unsupported and empty types, exact
  equality of independently obtained evidence, mismatch rejection, and unchanged
  effect-sensitive callable identity. Preserve dynamic packaging and exact casts
  for previously supported values, including functions. Preserve opaque display
  and region-safe comparison behavior without introducing construction demands.
- Exercise codec round trips for constructible records and recursive variants,
  absent options with empty payload types, and empty-only arrays. Check rejection
  of impossible cases and nonempty arrays of empty elements. Retain malformed
  binding and external-input errors. Verify schema identity and wire-tag behavior
  under constructive case filtering.
- Cover representative debugger rendering and caller-facing diagnostics through
  existing public test facilities. Compile all new documentation examples.
- During implementation, run Rust tests only through just test, including focused
  tests and the full workspace suite. Use the repository's format, Clippy, docs,
  and coverage recipes. Compiler changes remain subject to the documented line
  and branch coverage requirement. Record actual results and any unmet
  repository-wide requirement; do not claim a passing suite proves full coverage.
  Spec writing itself does not require running the test suite.

## Out of Scope

- Implementing inferred structural random generation, choosing distributions,
  generator sizing policy, or changing the existing Random effect to a single
  operation. Those are separate randomness changes.
- Making every reflection consumer infallible. Checked record assembly, decoding
  untrusted input, strict generation budgets, custom predicates, and application
  constraints can still require errors.
- Proving arbitrary program termination or deciding inhabitation for all possible
  function, effect, resource, or abstract types. Unsupported automatic construction
  is diagnosed without pretending the type is empty.
- Synthesizing arbitrary functions, allocating resource/cell constructors, or
  invoking effectful factories to satisfy Mirror requests.
- A general type-class system, overlapping implicit instances, process-global
  constructor registration, arbitrary user-supplied Mirror fabrication, or a new
  custom constructor-provider language. Producers may pass existing authorized
  evidence through ordinary arguments and hidden packages.
- Changing structural type equality, making aliases nominal, eliminating empty
  types from the language, or redefining types to remove impossible alternatives.
- Promising uniform random sampling over arbitrary types or a cross-release
  canonical default value. Finite construction is an existence and authority
  guarantee, with deterministic behavior across supported backends.
- Unrelated compiler refactoring or repairing unrelated coverage gaps.

## Further Notes

The confirmed design retains inferred reflect::mirror () while strengthening
Mirror to require accessible finite construction evidence. Constructive nested
views omit proven empty parts, and descriptions preserve all types. The proposed
test seam was explicitly accepted by the user during specification.

The description-only evidence route and migration of observational consumers are
integration requirements identified from the current implementation: Any packages
currently carry Mirror, function identity and display already use reflection,
and array/case views currently carry mirrors for every element or payload type.
These consumers must be migrated together rather than accidentally restricted
by changing the mirror intrinsic alone. Exact public names for the new
description-only route and finite-construction operation are implementation
choices; their required behavior is specified above.

This spec intentionally supersedes the older introspection design's weaker
Mirror contract, which admitted descriptive shapes without construction support
and optional case injections. The broader principles of authenticated evidence,
structural identity, finite type graphs, separate compilation, and explicit
foreign effects remain in force. Historical design documents should not be read
as overriding this stronger contract during implementation.

Implementation is expected to proceed through construction classification and
evidence propagation, then constructive runtime views, then consumer migration
and complete validation. These are parts of one feature; intermediate builds
must not expose a mixture of old unrestricted mirrors and new assumptions about
their construction authority.

## Implementation outcome

Implemented on the current branch. See [validation.md](validation.md) for public
consumer coverage, review findings and fixes, complete workspace test results,
and the explicitly unmet repository coverage requirement.
