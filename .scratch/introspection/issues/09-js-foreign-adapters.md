# 09 JavaScript foreign adapters

Status: resolved
Type: task
Blocked by: 04, 08

Spec section: "4. Foreign conversion and invocation". Compiler-owned opaque
`js::Value` identity; `ForeignAdapter` lift/lower with checked numeric/text
conversion; callbacks preserving declared effects and completion; snapshots;
host-observation failures; validated (non-executing) C/Wasm ABI plans.

## Answer

`std/js.rud` is the JavaScript adapter layer. `Value` is the compiler's own
opaque host-value identity: it cannot be built out of data, and forwarding one
back to the host hands over the same value, so the host's `===` still holds.
`Adapter 'a` is the two directions of one checked conversion, `lower` and
`lift`; `adapter ()` is the structural adapter for whatever type the use site
infers, from the mirror the compiler supplies there, and a specialized adapter
is an ordinary record a caller writes and passes wherever the structural one
would go. Both directions refuse a function contract: an extern declaration
owns one, because it carries the complete per-arrow type, the effects the
callback may perform, and how the call completes.

The outgoing direction is new: `std::ffi::encode`, the `$ffiEncode` intrinsic,
matching the decode that was already there, in the JavaScript runtime and in
the reference interpreter.

Host observation is an effect. Reading a property can run a getter or a proxy
trap, so `js::Host` has `kind`, `field`, `element`, `length`, `keys`,
`snapshot`, and `apply`, handled by the platform the way `!FileSystem` and
`!Http` are. Every failure is data carrying the host's own words, and
`snapshot` copies inert data out of the host so the copy can afterwards be
read purely, while forwarding a `Value` keeps the host's identity.

`std/abi.rud` is the backend-neutral plan layer: pure data and pure functions,
no extern and no effect in the file. A `Target` names the convention, the
address width, and the largest alignment that target gives a scalar, because
no rule recovers the last of those. `validate` reports every fault a plan has
that is decidable from the plan alone, and the tests hold it to real numbers:
the 64-bit layout of `struct { char a; int b; double c; }` and five mutations
of it, `struct iovec` counted by its sibling field, the i386 layout of a
4-aligned double, `write_all` with its buffer's length and owner, and the
canonical ABI lifting a string and a list. A plan that validates is not a
claim that native or WebAssembly integration executes, which is what the
module's own doc comment says.

Two defects were found and fixed while reviewing this work: `js::kind` threw
on a value that refuses to be read at all, where its contract says such a
value is `#Other`; and the shared calling convention probed every foreign
argument for a marker property, which a revoked proxy refuses, so any extern
taking a host value directly crashed rather than answering.

Deliberately not done: distinct opaque host identities for native pointers and
WebAssembly resources. The spec asks for those "as those integrations are
implemented", and it puts production native and WebAssembly foreign
integration in later work, so `ForeignValue` remains the one host identity
this compiler has, and it is JavaScript's.
