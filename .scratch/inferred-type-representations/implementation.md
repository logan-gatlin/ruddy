# Inferred runtime representations: implementation

Status: implemented; repository coverage target remains unmet

Worktree: `/var/home/dev/code/hc/ruddy-inferred-type-representations`
Branch: `inferred-type-representations`
Rebased main: `fdfc498` (effect rethrowing, PR59)
Spec commit: `fc52816`
Pre-rebase implementation checkpoint, rebased: `6332fe4`

## Rebase

Fetched and rebased cleanly onto `refs/remotes/origin/main`. Use the full remote
reference because a local branch named `origin/main` makes the shorthand
ambiguous. The original main worktree is untouched. Backup branch
`inferred-type-representations-before-effect-rebase` retains `09bc6b7`.
No requirement or source syntax change was needed after the effect rebase.

## Implementation

`Any` and `JsValue` are builtin types. The standard `any` module exposes `upcast`
and `downcast`; `js` exposes `decode` and structural `DecodeError`. Reflection
uses compiler-recognized intrinsics with checked signatures. Authentic `Any`
packages keep their descriptor and original payload in a runtime WeakMap;
lookalikes cannot forge packages. Structural graph equality distinguishes primitive
types and recursively compares arrays, records, sums, and supported pure functions.
Aliases and allocation addresses do not establish user type identity.

`src/reification/conventions.rs` infers a finite graph of value shapes and
per-arrow descriptor demands. Evaluation requirements are separate from invocation
requirements. Incoming callbacks quantify demand ports. Profiles follow actual
values through generalization, instantiation, partial application, captures,
recursive groups, branches, aggregate construction and patterns. An erased generic
identity retains its argument's profile without intrinsically needing descriptors.

`src/reification/interface.rs` publishes reachable finite shape graphs, normalized
parameter slots, conditional ports and solved requirements. Semantic schemes share
these immutable interfaces. Interfaces survive artifact imports and participate in
semantic fingerprints. The old binding-wide fixed-point inference is removed;
the retained flat `representations` vector is an informational summary.

`src/lir/lower/reification.rs` adapts callable layouts before ordinary type erasure.
A source lambda receives its own arrow's descriptors, effect evidence and visible
argument in that order. Initializers execute once in their actual lexical context.
Creating an adapter captures values and descriptors; it never runs the adapted
source body. Aggregate adapters preserve producer conventions through record
fields, arrays, sum payloads and patterns. Component projections decompose an
incoming compound descriptor where a callback needs a component. Effect adapters
forward the caller's evidence instead of accidentally capturing ambient handlers.

Values crossing reflection, abstract native positions, effect operations or
mutable callable storage use a canonical callable convention by capturing required
evidence. Effect operation implementations still receive only their payload;
this convention works across independently compiled perform and handler sites.
No effect or region argument becomes a type descriptor merely because it appears
in an effect row. Existing region and hidden-presence escape checks remain active.

Exact `TypeDescriptor`, `TypeProjection`, `Reflect`, `NativePlan` and `Convert`
operations survive CPS lowering, artifact translation, linking and JS emission.
Exact identity descriptors reject unavailable hidden information. Native plans
have a separate conversion policy supporting existing optional record and package
contracts, and can defer returned-function descriptors until invocation.

Ordinary externs and concrete JS exports use recursive native conversion. Arrays
are snapshots in both directions; sums use `{tag, value}`. Opaque host values and
authentic packages retain identity. Generic extern adapters are compiled once.
Marked argument groups retain earlier values and descriptors until the final
call, preserving callback effect coverage and completion protocols. Conversion
state survives Promise completion. Malformed typed input is a foreign contract
failure; explicit decoding returns structured path/expected/message errors.
See `docs/runtime-type-information.md` for the native constructor encodings.

JS roots reject unresolved conversion or invocation requirements throughout
nested/returned/recursive interfaces in both checking and building. Portable
library and private generic interfaces remain permitted. Concrete annotations,
including deliberate `JsValue` interfaces, supply host requirements.

Lowered globals retain the callable contract under which they were compiled.
Artifact validation compares complete exported interfaces against that retained
contract, including aliases, curried results and nested aggregates. It also checks
bound indices, demand ports, graph references, descriptor operand representations,
row-extension invariants and direct closure evidence arity. As with the existing
typed executable artifact, this validates compiler contracts rather than proving
arbitrary hand-written executable semantics.

Hover and diagnostics explain needs in prose. Source printers retain existing
syntax. The debugger has a Runtime types tab for finite value shapes, per-arrow
needs, demand ports and evaluation requirements, plus descriptor operations in its
existing LIR/artifact views.

## Termination

The shared IR regular-type graph retains admitted aliases as finite back edges
and rejects growing applications under existing rules. Shape construction follows
finite source occurrences and graph nodes, closes active recursive paths, and
keeps closed subgraphs erased instead of expanding shared alias DAGs.

Demand solving allocates no new types or graph nodes. It monotonically adds pairs
from finite sets of parameter indices and demand ports. Substitution maps to
finite free-parameter sets. Recursive definitions remain graph dependencies;
they do not specialize or execute source bodies. Adapter generation reserves
memo entries keyed by semantic type/profile pairs and capture layouts before
visiting recursive children. Runtime equality uses graph-pair memoization;
conversion uses an explicit work stack and cyclic-path checks.

A source-to-artifact regression compiles 128 higher-order generic forwarding
interfaces and a recursive descriptor on a 256 KiB stack, validates the printed
artifact, and bounds artifact size. Existing deep alias, imported rotation and
bounded-stack interface regressions also remain active.

## Tests and compatibility

Tests use the approved source-compilation, generated JS, artifact/import and
export-diagnostic seams. Coverage includes eager initializer identity, independently
typed curried calls, erased/reified callback joins, callable aggregate patterns,
mutable callable cells, effect payloads/results/operation values, component
evidence extraction, native callbacks, Promise completion, opaque transport,
forgery rejection, recursive structural equality and malformed public artifacts.

Eight previous effect-adapter tests now execute generated JavaScript while
retaining their source programs, rather than asserting private temporary indices.
They caught actual evidence-capture failures during integration. Existing fixture
changes annotate deliberate concrete or JsValue host interfaces and replace
observations of private arrays/sum symbols with the new native encodings. The
array model test still checks all get results; it reads contents within Ruddy and
crosses the snapshot boundary once per observation to avoid quadratic host copying.

## Review and validation

Spec review found missing effect payload conventions, weak artifact contract
validation, and missing bounded-resource coverage/documentation. Regression tests
and the changes above address these findings. Standards review requested the new
debugger phase and shared adapter-key/capture installation helpers; both are added.

- `just test`: 1,824 passed, 9 ignored, no failures (1,751 in the main test crate).
- `just cov`: the same complete suite passed; 96.12% line coverage and 88.50%
  branch coverage. This does **not** meet CONTRIBUTING.md's 100% requirement.
  The remaining coverage gap is explicitly outstanding; this is not a claim
  of complete standards compliance.
- `just clippy`: passed without warnings.
- `just fmt-check`: passed.
- `git diff --check`: passed.

All Rust tests ran only through `just test`; `just cov` delegated its instrumented
suite to that same recipe. Spec and standards reviewers verified the fixes to
their concrete findings. An additional public artifact regression rejects changing
a valid descriptor parameter index to another valid index, and generated JS tests
exercise record-row remainder and function-result descriptor projections.
