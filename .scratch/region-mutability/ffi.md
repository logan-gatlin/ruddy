# Region mutability at the FFI boundary

Status: selected trust policy; concrete ABI details remain implementation work.

The user selected trusting foreign implementations and callers to respect
invariants. This replaces the earlier proposal for scoped-only callbacks,
revocation, and FFI-specific retention/thread checks. Ordinary source type and
effect checking still applies, but Ruddy adds no enforcement of foreign region
lifetime, retention, or thread behavior in this version.

## Trusted invariants

- Reading and writing region-owned state require `!mut r` in the declared
  effects. Allocation must obey the caller-region rule.
- Aliases preserve storage identity and the cell's fixed element type.
- Foreign callers do not access state after its region lifetime ends or from
  another thread.
- Retained references and callbacks may only be used while their region remains
  valid and on its owning thread. Returning from one foreign call is not
  necessarily the region's end.
- Foreign global state, I/O, and other observable behavior must be accounted
  for in declared effects. Declaring only `!mut r` must not conceal unrelated
  effects that would incorrectly disappear during local isolation.
- Foreign code does not present an already shared host object as fresh local
  storage or mutate the backing storage of Ruddy immutable values.

These are trusted contracts, not facts the compiler proves about foreign code.
Violating them invalidates the language guarantees across that boundary. No
runtime revocation tokens, callback lifetime guards, or owning-thread checks
are required by the selected FFI design.

## Examples

Proposed types:

```text
extern increment : mut 'r Nat -> () + !mut 'r = "host.increment"
extern twice : fn(fn(()) -> Nat + !mut 'r) -> Nat + !mut 'r
  = "host.twice"
```

A foreign operation may discard the value produced by an internal write and
return unit, as `increment` does. `twice` promises that the supplied callback's
state access and its own behavior obey the declared region effect and all
relevant lifetime/thread invariants. It must not secretly publish observations
into unrelated host state.

Callbacks and mutable arguments are not restricted to immediate scoped calls
by this policy. Retention and asynchronous calls are trusted to obey the same
invariants; `@async` continues to describe completion timing only. The concrete
representation of a mutable cell in the foreign ABI still needs specification,
including identity and conversion, but an opaque guarded-handle interface is
not a requirement. [Current FFI contracts](../../docs/cps.md),
[reviewed conversions](../../src/externs.rs)

## Isolation and exports

The compiler must not knowingly erase a region dependency exposed by source
signatures, results, or captured Ruddy values. Foreign code is responsible for
not creating hidden escape paths that violate inferred isolation. Exported
region-dependent values likewise require the foreign caller to respect the
region's lifetime and thread restrictions; the compiler does not infer safety
of arbitrary retained host uses.

No FFI implementation changes have been made.
