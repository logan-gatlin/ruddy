# 21 Make arbitrary host lifting effects explicit

Status: resolved
Type: task
Priority: P1

Review baseline: `cd906cf`. Reproduced in both reviews; still open.

The [foreign conversion spec](../spec.md#4-foreign-conversion-and-invocation)
requires arbitrary host observation to be effectful. A getter or proxy trap
can run code; only an authenticated inert snapshot can subsequently be read
purely.

## Reproduction and impact

Compile this JavaScript-target module with the standard library dependency:

```ruddy
let lift_record:
  std::js::Value -> std::result::Result { count: Nat } std::js::Error =
  std::js::lift
```

Call its generated export from JavaScript:

```javascript
let reads = 0;
const input = { get count() { reads += 1; return 7; } };
const result = await app.lift_record(input);
// Current result: { tag: "Some", value: { count: 7 } }; reads === 1.
```

The explicitly pure Ruddy function compiles and performs the host mutation.
Expected: arbitrary `Value` lifting requires the appropriate host observation
effect, or pure lifting accepts only a distinct, authenticated inert input.
Returning conversion failures as `Result` does not account for those effects.

## Work

Audit [std/js.rud](../../../std/js.rud), including both `lift` and
`Adapter.lift`, the underlying [FFI API](../../../std/ffi.rud), and structural
property reads in [type-runtime.js](../../../src/backend/type-runtime.js).

- Choose and consistently implement the effectful arbitrary-value API, an
  authenticated snapshot API for pure conversion, or both. Update signatures,
  evidence plumbing, callers, and API documentation together.
- Do not treat the existing generic `js::Value` type or an ordinary object
  shape as proof of inertness. `snapshot` currently returns the same `Value`
  type, so it provides no static distinction from arbitrary host values.
- Any pure snapshot path must prevent callers from substituting live host
  objects, forging the snapshot capability, or regaining mutable host aliases
  that make later pure reads observable. Cover nested records, arrays, and
  proxies, not only direct fields.
- Preserve the [selected foreign trust policy](../../region-mutability/ffi.md).
  Fix ordinary source effect checking without adding mandatory FFI revocation,
  thread guards, or restrictions on declared foreign lifetime contracts.

## Acceptance

The pure arbitrary-`Value` signature above must no longer permit observation.
Exercise direct `js::lift`, `Adapter.lift`, and any remaining public conversion
entry point with getters and proxy traps. Prove that an effectful conversion
declares its effect and that any supported pure snapshot conversion cannot
accept the live input shown above. Preserve normal inert-data round trips.
Extend [adapter tests](../../../tests/src/js_adapters.rs) and run Rust tests
only through `just test`.

## Answer

Both readings exist, and the pure one accepts only input that justifies it.

`std::js::lift` reads an arbitrary host value and carries `!Host`, so the
ticket's signature — a pure `Value -> Result { count: Nat } Error` bound to
`lift` — no longer compiles: the effect is required by the use and excluded by
the annotation. It observes for real, not by declaration: it asks the host what
the value is before converting it.

`std::js::read` is the pure reading. It accepts a host primitive, which carries
no getter, and a snapshot, which is a copy this program took; anything else is
refused with `a snapshot this program took`. Membership decides, not shape: the
runtime keeps the copies `snapshot` made, so a live host object of exactly the
right shape, and a proxy that answers every property, are both refused. Nothing
outside `snapshot` can put a value in that set, so the capability cannot be
forged, and because a snapshot is a fresh tree with no getters anywhere,
reading it runs no host code at any depth.

`Adapter` carries all three, so a specialized adapter writes both readings and
goes wherever the structural one goes.

Regression: `host_observation_reads_values_on_node_and_web` in
`tests/src/js_adapters.rs` calls the pure reading on a live object with a
getter, on one with a nested getter, and on a proxy, and asserts no getter ran
at all — the proxy's catch-all trap sees only the symbol probe the shared
calling convention makes of every extern argument, which is counted separately
and is not a field read. It then reads a primitive purely, snapshots the live
object, reads the copy purely, and only then does the getter run, once.

Fixed on the way: the host export adapter was compiled with the default
integer domains, so a bundle binding another could not link its own exports.
It now binds the root's.
