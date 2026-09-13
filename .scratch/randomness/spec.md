# Standard-library randomness

Status: implemented

Add `std::random` for ordinary randomized application behavior and repeatable
tests. User-directed requirements: expose several primitive operations on the
effect, and make local handlers derive their seed from the next outer Random
handler. Implementation was requested against this revised plan.

## Design

Use a small primitive effect as the seam between callers and their source:

```ruddy
effect Random = {
  word64: () -> Nat64,
  boolean: () -> Bool,
  real: () -> Real,
}
```

Each primitive wrapper performs its corresponding effect operation directly;
`boolean` and `real` are not wrappers around the public `word64` operation.
Handlers can supply these primitives independently. The module owns sampling
rules, an explicitly seeded handler, and a parent-seeded local handler. The
JavaScript platform supplies the default adapter. This follows `std::process`
and `std::http`, keeping algorithm state and conversions inside the module.

Proposed public interface (function types below omit their common `!Random`
effect for readability):

| Function | Input → output | Contract |
| --- | --- | --- |
| `word64` | `() -> Nat64` | One source word, including both endpoints of the Nat64 domain. |
| `boolean` | `() -> Bool` | A random boolean. Built-in handlers use one private word's top bit. |
| `real` | `() -> Real` | A random Real in `[0, 1)`. Built-in handlers use one private word's top 53 bits divided by 2^53. |
| `nat_below` | `Nat64 -> Option Nat64` | Exclusive upper bound; zero returns `#None` without drawing. One returns `#Some 0n64` without drawing. |
| `choose` | `['a] -> Option 'a` | Select an array position via `nat_below`; empty returns `#None`, singleton returns its item, neither draws. Duplicates retain their positional weight. |
| `with_seed` | `Nat64 -> (() -> 'a + !Random + …) -> 'a + …` | Handle Random with fresh private state; preserve every unrelated body effect. |
| `local` | `(() -> 'a + !Random + …) -> 'a + !Random + …` | Draw one seed from the outer handler, then handle the body with its own stream. |

The handler rows are schematic: use the compiler's existing effect inference
and local mutation patterns rather than adding effect syntax. A pure body apart
from Random produces a pure result under `with_seed`; `local` retains Random
because it requests its seed from outside. Internal mutation must not escape.

Example intended usage:

```ruddy
let roll = fn _ => std::random::nat_below 6n64
let repeatable_roll = std::random::with_seed 42n64 (fn _ =>
  std::random::local roll
)

-- Proposed implementation of the convenience handler:
let local = fn body => do
  let seed = word64 ()
  return with_seed seed body
end
```

Use explicit `Nat64` bounds to avoid target-dependent `Nat` precision and
overflow. Document the suffix despite the shorter function name. Convert array
lengths to Nat64 exactly and convert the selected index back only after it is
known to fit the original array's indexing domain.

## Local scopes and reproducibility

`local body` obtains exactly one `Random.word64` from the nearest surrounding
handler before installing its own handler and invoking the body. Seeding is
eager, even if the body makes no draws. All three operations inside the body
use one shared private generator state. They must not call public Random
operations from their handler arms: those operations would reach the outer
handler. Share a private next-word implementation instead.

Nested `local` calls form a tree of streams: each child consumes one word from
its immediate parent, and subsequent child draws never advance the parent.
An explicit `with_seed` consumes nothing from its parent. On return or early
exit, local state is discarded; the initial seed draw is not rolled back.
An unrelated effect or host suspension preserves the installed handler and
its state for the resumed body. No per-draw host calls or global PRNG state.

For a fixed root seed, stable assignment of child seeds, and the same ordered
draws inside each child, interleaving *between* children does not change their
results. Child creation order still matters. If async tasks first call `local`
after racing to resume, seeds can be assigned to different tasks. Allocate
seeds in stable task order before scheduling, then invoke each task under
`with_seed` using its assigned seed. Entering `local` before a task's first
suspension is sufficient only when those entries themselves have stable order.

The existing language invokes effectful callbacks under the handler active at
invocation; merely constructing a callback inside `local` does not capture its
stream. A scheduled callback must establish its handler when invoked. Document
this explicitly in the async example; do not imply a returned closure retains
the local handler.

Future multithreaded tasks should each own their handler state. Sharing one
mutable stream between threads would still make draw assignment depend on
scheduling even with locking. Thread spawning/inheritance is outside this
feature; preserve the per-task ownership rule when that facility is designed.
Derived seeds do not promise distinct or statistically independent streams.

## Sampling and seeded implementation

- Implement `nat_below b` by rejection sampling. For `b > 1`, calculate
  `threshold = (0 - b) % b` with wrapping Nat64 subtraction. Draw until
  `word >= threshold`, then return `word % b`. This removes reduction bias
  assuming uniformly distributed input words. Expected draws are below two
  under that assumption; a custom handler that only supplies rejected words
  can fail to terminate. Do not impose a retry cap that biases results.
- Use private SplitMix64 state for `with_seed`, following the
  [author's reference implementation](https://prng.di.unimi.it/splitmix64.c).
  Initialize state directly from the supplied seed, including zero; increment
  before mixing the first word. Preserve wrapping 64-bit arithmetic exactly.
- Keep the seeded algorithm and sampling consumption rules stable across
  interpreter, Node, and web builds. Identical seeds and ordered calls must
  yield identical results. Changing these sequences is a compatibility change.
  No serialized generator-state format is introduced.
- `word64`, `boolean`, and `real` each consume one private generator word in
  built-in seeded handlers. Helpers such as `nat_below` and `choose` draw through
  the public word operation. Custom handlers must honor each operation's value
  contract but need not implement boolean/real draws in terms of words.
- Implement the seeded algorithm in Ruddy using existing Nat64 arithmetic and
  private `xor64`. Constant logical right shifts can use unsigned division by
  powers of two, avoiding a new primitive. Check the exact conversion path for
  the top 53 bits; add a narrow pure conversion primitive only if existing
  conversions cannot preserve those values across targets.

## Default host adapter

For the first release, use the JavaScript host's `Math.random` to obtain two
32-bit chunks and assemble a Nat64 using BigInt, without passing a full word
through a JavaScript Number. Draw high chunk first, then low chunk. Implement
all three primitive operations using this private word source and the same
conversions as the seeded handler; no operation forwards to an outer Random.
The
[ECMAScript contract](https://tc39.es/ecma262/multipage/numbers-and-dates.html#sec-math.random)
specifies approximately uniform output and an implementation-defined strategy;
the default adapter therefore offers no exact uniformity or sequence guarantee.
Rejection sampling avoids introducing modulo bias but cannot improve its source.

Install the adapter in both Node entry points and Node/web library exports.
Perform ambient draws only inside the platform handler, with the existing
Immediate effect discipline. `local` touches this adapter once for its seed
when it is the nearest outer handler; `with_seed` does not touch it at all.
The reference interpreter supports seeded and caller-handled computations;
this feature does not add an ambient host capability system to the interpreter.

This entire module is noncryptographic. Defer secure bytes, passwords, tokens,
UUIDs, weighted sampling, distributions, shuffle, signed ranges, public generator
objects, and explicit jump/split algorithms beyond parent-seeded scopes.
A later secure facility should have its own
fallible effect and host entropy contract so seeded overrides cannot silently
weaken a security-sensitive call.

## Repository integration

- Add `std/random.rud` and its documented export in `std/lib.rud`.
- Follow the structural platform declarations in
  `src/backend/web-platform.rud`, handler clauses in
  `src/backend/web-handler.rud`, and runtime helpers in
  `src/backend/web-apis.js`. Extend the explicit Node effect allowance in
  `src/backend/js.rs`; check `src/backend/host.rs` for library export handling.
- Avoid parser/type-system changes and new dependencies. If a pure conversion
  primitive is needed, cover both `src/backend/primitives.js` and
  `interp/src/prim.rs`, plus primitive validation/registration as applicable.
- Add public-interface tests under `std/tests/` and register them in
  `std/tests.rud`; add backend integration coverage alongside
  `tests/src/stdlib.rs`, `tests/src/stdlib_apis.rs`, and interpreter tests.
- Add `@doc` descriptions, `docs/src/std/random.md`, and appropriate entries in
  `docs/src/standard-library.md` and `docs/src/platform-apis.md`, following the
  existing documentation generation conventions.

## Acceptance criteria

1. A compiled ordinary application can draw with the default handler; Node and
   web library exports discharge Random correctly. A local handler takes
   precedence and pure imports never draw or initialize hidden random state.
   All three effect operations are independently replaceable; overriding
   boolean or real must not require the corresponding draw to call word64.
2. Independent reference vectors cover seeds zero, one, and maximum Nat64,
   including state wraparound. The interpreter and JavaScript agree on exact
   word sequences and a mixed sequence of helper calls.
3. Reference seeded draws and controlled host words exercise boolean outcomes
   and Real endpoints; scripted Random.word64 responses exercise bounded
   sampling. Cover Real endpoints
   `0` and `1 - 2^-53`, and bounded sampling at zero, one, a power of two,
   a non-power-of-two, above 2^63, and maximum Nat64. Force rejection followed
   by acceptance and verify exact draw counts. Never use histogram thresholds
   as a correctness test.
4. Array choice covers empty/singleton arrays, duplicates and generic values;
   validates first/last index selection and leaves its input unchanged.
5. Seeded runs repeat; `local` requests exactly one outer word before executing
   its body, including an empty or aborting body. Nested locals consume one
   word from their immediate parent only; explicit seeds consume none.
   Boolean/real handler arms never escape to the parent. Unrelated effects
   remain handleable and internal mutation does not leak in inferred types.
   A seeded root containing local children works when host randomness throws.
   An ambient-root local is expected to request its seed from the host.
6. With fixed child seed assignment, two controlled suspension/resumption
   schedules produce identical per-child results despite different draw counts
   between children. A sibling's extra draws do not affect the other child or
   the parent. Verify resumed bodies retain local state. Use the existing host
   async test machinery; do not require a new threading or scheduler facility.
7. Documentation states exclusive bounds, source-quality assumptions, fixed
   seeded sequences, parent seed consumption, stable seed assignment before
   concurrent work, callback invocation semantics, and noncryptographic scope.
   All examples compile.
8. During implementation run Rust tests exclusively through `just test`, then
   relevant format checks and documentation checks. Planning itself requires
   no test-suite run.

## Implementation sequence

See [01](issues/01-seeded-core.md), [02](issues/02-platform-adapters.md), and
[03](issues/03-documentation-validation.md). Implement and validate the seeded
core first, then host integration, then complete documentation and acceptance
checks.

Review baseline: `0cc67ca084be5ed1df6c196219795689f1d279fe`.
Tests exercise the public std::random interface and compiled/interpreted
consumer programs, as specified in the acceptance criteria.

Implementation prerequisites: the existing compiler joins direct local mutation
to an open callback effect remainder before checking region escape. This makes
the planned pure, effect-polymorphic seeded handler impossible. Isolate direct
cell operations before joining callback effects, while retaining mutation's
value restriction on local initializers and all escaping-region checks. Cover
the prerequisite through compiler consumer tests and existing mutation tests;
expose deferred state regions in debugger constraint rendering. No new syntax
or runtime primitive is required.

The shared runner takes a seed callback whose Random presence determines
whether seeding needs an outer handler. The backend must omit a conditional
capability's dictionary entry when the use excludes it, or when a pure call
leaves it unconstrained and no evidence exists. Cover this through pure seeded
roots, parent-seeded locals, and calls under custom/host handlers. This fixes
the conditional-evidence lowering panic encountered during implementation.
