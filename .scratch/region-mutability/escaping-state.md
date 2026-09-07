# Escaping state: implementation choices

Status: option 2 selected by the user; other approaches retained for comparison.

Decision: escaping state allocates in the caller's inferred region and retains
`!mut r`. Fresh cells and stateful closures may leave ordinary helper functions;
usable state may not escape a scope whose mutation effect has been isolated
away. Existential region packages are not part of the selected design.

The user accepted implicit region inference. Other constraints remain: one
accessible mutation region per computation, ordinary `!mut r` effect identity,
a region variable kind, thread confinement, and purity for effect-less functions.

## What escape means

Returning a cell or closure from a function is not necessarily escaping its
region. A helper can allocate in a caller-owned region and return values that
remain usable there. What must never happen is removing an observable mutation
dependency through isolation while retaining access to its state.

The ranking below estimates added implementation complexity beyond basic
reference operations, region kinds, safe let generalization, and thread
confinement. It is a design judgment based on the current compiler, not a timing
estimate. [Current type/scheme representation](../../src/types.rs),
[generalization](../../src/inference/mod.rs)

## Ranked approaches

| Rank | Approach | Semantics | Added complexity |
| --- | --- | --- | --- |
| 1 | Reject escaping fresh state | Fresh allocations must remain within an inferred isolated scope; a state factory is rejected | Lowest: failed isolation is a diagnostic; still needs complete escape checking |
| 2 | Allocate escaping state in the caller's region | A factory retains `!mut r` and returns `mut r a` or a closure with latent `!mut r`; enclosing callers may eventually isolate the whole computation | Low to moderate: region/effect propagation and an isolation decision before publication; no new package type or heap promotion |
| 3 | Explicit packages owning private regions | A returned package hides a fresh region identity and exposes operations whose state dependence stays tracked | Moderate to high: reuse existing package infrastructure, extend it to regions, and establish state-effect and lifetime semantics |
| 4 | Infer private region packages and captures automatically | The compiler invents hidden region binders and capture information for escaping state, inserting their packing/opening | Highest among these: reuse presence-package inference patterns, but add region-specific abstraction and capture rules to approach 3 |

Approach 1 rejects factories of fresh state, not necessarily returning an
unchanged reference supplied by the caller. Approaches 3 and 4 also fit poorly
with using several independently owned state packages in one computation under
the single-`!mut` rule. The ranking does not assume they solve that limitation.

## Selected semantics: caller-region allocation

Illustrative inferred types:

```text
make_cell : 'a -> mut 'r 'a + !mut 'r
make_counter : () -> (() -> Nat + !mut 'r) + !mut 'r
```

The factory's outer effect records allocation. The returned counter's arrow
records access on later calls. A factory's region is chosen at its use site;
each factory invocation allocates a fresh cell in that region, not a fresh
independent region. Two counters made and used by one computation can therefore
coexist and have independent contents while sharing one `!mut r` effect.

A captured cell's region remains fixed. It cannot be re-generalized at each
use of the returned closure. Generalizing a factory over the caller's region is
different from generalizing a cell allocated by calling that factory.

An enclosing function can create a counter, use it, and return a `Nat`. If no
region dependency remains in its environment, result, or residual effects, the
compiler can isolate that whole computation and publish a pure function. If it
returns the counter instead, the mutation effect stays visible and the region
is supplied by its own caller. No existing rigid local region is extended or
merged: the decision to allocate in the ambient region precedes committing to
a fresh isolated binder.

Region inference should solve dependencies, decide whether local isolation is
valid, and apply safe generalization in a coordinated order. Do not first mark
a factory pure, then repair it when a caller observes the escaping reference.
Inference rules and higher-order examples still need to establish that this
ordering is coherent.

Stateful global initialization remains rejected under Ruddy's existing pure
initializer contract. Ordinary implicit isolation may suffice for `main`, since
its result cannot expose a cell or stateful closure: local `!mut r` can be removed
while platform effects remain. A special root runner is not established as a
requirement. Foreign-retained state would need a separate entry/export lifetime
contract, not permission for arbitrary handlers to erase `mut`.
[Current entry contract](../../README.md)

## Why private packages cost more

Inspection of the existing presence existentials shows meaningful reusable
infrastructure. The ranking above has been revised to reflect that option 3
does not start from scratch:

- `Ty::Package` already marks producer-owned result boundaries, with traversal
  through nested functions, aggregates, and named types. `Scheme` tracks
  existential positions. Region binders could extend this framework.
  [Representation and sealing](../../src/types.rs)
- `instantiate_local`, `instantiate_scoped`, and `open_package` distinguish
  coherent lexical values from fresh views of produced results. They provide
  a model for opening and propagating abstract witnesses, though their current
  implementation collects and substitutes presence variables specifically.
  [Package opening](../../src/inference/mod.rs)
- Artifact encoding, ownership validation, and alias/call regression tests
  already exercise package boundaries. These can be extended for regions.
  [Artifact validation](../../src/artifact.rs),
  [Existential regression tests](../../tests/src/inference.rs)

The implementation is not yet kind-general: `Scheme::existential` asserts that
every existential position is below the presence count, and artifact reading
rejects positions outside that range. Presence guarantees use Boolean formulas;
region identities require their own equality, scope, and thread-confinement
semantics. Package ownership here means ownership of abstract variables, not
ownership of mutable runtime storage.

The existing alias rule deliberately opens separately bound aliases with fresh
abstract views. That can be a conservative abstraction policy for regions, but
it cannot establish that aliases refer to physically distinct stores. Reopening
one mutable package does not allocate new state or justify isolating away its
effect. Under the single-region rule, separately opened views may also make
otherwise related references impossible to use together; preserve a coherent
opening or diagnose that limitation rather than claiming fresh views prove
independence.

Conceptually, `exists r: Region. (() -> Nat + !mut r)` hides a stateful closure's
region name while retaining its state dependency. It must not collapse to
`() -> Nat`. Designing how a caller executes such a package and accounts for
its surviving state effect remains necessary even if packaging is reused.
An explicit fresh-region existential also still limits composition of multiple
packages under the one-`!mut` rule. Caller-region allocation avoids that issue.

All approaches retain the thread restriction: escaping a function does not
authorize state access on another thread. Existing region captures and their
transitive contents remain confined.
