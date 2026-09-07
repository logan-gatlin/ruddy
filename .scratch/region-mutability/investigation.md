# Region-based mutability in Ruddy

Status: investigation; no implementation approved or started.

## Current direction

The authoritative [specification](spec.md) consolidates the selected
**single-region mutability** scheme. This investigation retains background
findings; deferred alternatives do not expand the implementation scope.

Requirements:

- Mutation is for ergonomics and safety.
- Effect-less functions remain pure.
- Region variables have a distinct kind.
- Explicit cell types use `mut 'r 'a`; access effects use `!mut 'r`.
  Type and effect aliases declare their region parameter, as in
  `type t 'r = mut 'r Nat` and `effect e 'r = !Log + !mut 'r`.
  `type t = mut Nat` is invalid; regions remain inferred in expressions.
- Selected read/write syntax: `~a` reads a cell; `a := b` writes its contents
  and returns the value just stored. Assignment is right-associative, so
  `a := b := c := ~d` writes to `c`, then `b`, then `a`, returning the stored
  value. Reads follow existing unary precedence; assignment evaluates its target
  first. Allocation uses the primitive keyword expression `mut value`.
- Regions and isolation scopes are inferred; explicit region syntax is not
  required. Isolation candidates are function bodies only.
- Escaping state uses the caller's region (option 2): factories retain `!mut r`,
  and returned stateful closures retain their latent access effect. Region
  existential packages are out of scope.
- A computation can access one mutation region through ordinary `!mut r`.
- Regions do not overlap in the effects of one computation; different region
  arguments for `mut` clash under the existing effect-row rule.
- Reads and writes use the same effect and are confined to the creating thread.
- Discarded writes use `let _ =`; there are no bare expression statements.
- Let generalization requires a proven effect-less initializer, subject to
  existing scope restrictions. Effectful initializers retain shared unknowns;
  covariance and dependency-based relaxations are deferred.
- Foreign callers and implementations are trusted to preserve invariants,
  without extra Ruddy-side FFI enforcement.
- Separate read/write permissions, region transfers, and shared-reader modes
  are out of scope.

A single region can contain many aliased cells of different element types.
Checked isolation can hide fresh local mutation and return a pure result.
The finalized syntax and acceptance criteria are recorded in the specification.

## Repository findings that still apply

Typed effect parameters already share one application per constructor in a
row. The solver unifies corresponding arguments, and handler arms share the
handled instance. This is suitable for enforcing the selected single-region
restriction. [Effect constraints](../../src/inference/constrain.rs),
[argument unification](../../src/inference/solve.rs),
[typed-effect design](../typed-effect-parameters/design.md)

The existing variable kinds do not include regions. A new region kind needs
representation through inference, substitution, named type arguments, and
artifacts. A region identity is not an ordinary value type, and kinding alone
does not establish freshness or non-escape. [Types and kinds](../../src/types.rs)

Let generalization is currently unrestricted, with a test explicitly justified
by the absence of mutation. A mutable cell must not be independently
instantiated at incompatible types. Fresh local region identities must also
not generalize into apparently caller-selectable regions on captured cells.
[Inference tests](../../tests/src/inference.rs),
[local binding solver](../../src/inference/solve.rs)

Existing rigid variables and escape diagnostics provide foundations, but their
current role is annotation checking. Region scope checking must permit local
sharing and check escape through result types, latent effects, residual effects,
and the outer environment. [Inference](../../src/inference/mod.rs)

An ordinary handler's ability to discharge an effect is not proof of state
isolation. Reference operations and the region runner need compiler-controlled
semantics. Reads of external mutable state stay effectful. Keeping the cell
parameter in the cell type `mut r a` and only the region in the effect `!mut r` permits heterogeneous
cells in one region. [Handler generation](../../src/inference/constrain.rs)

Ruddy supports suspension and retained, repeated, and overlapping foreign
callbacks. Future thread checks must cover captures and evidence, not merely
immediate effects. Suspended region access must stay on its owning thread.
Thread confinement does not promise atomicity against same-thread reentrancy.
[Runtime contracts](../../docs/cps.md)

## Immutable results

Existing arrays remain persistent and immutable. A future mutable buffer could
return a copied snapshot; elements must still satisfy escape checks. Zero-copy
freezing needs an additional account of surviving mutable aliases and is not
required for this ergonomics-focused investigation.
[Array interface](../../std/array.rud),
[JavaScript representation](../../src/backend/js.rs)

## References and validation

[Prior art](prior-art.md) compares Haskell ST, Koka, and Effekt using primary
sources. Their broader multi-region and concurrency mechanisms are reference
material rather than requirements for this sketch.

No compiler/runtime changes or tests were made for this investigation.
