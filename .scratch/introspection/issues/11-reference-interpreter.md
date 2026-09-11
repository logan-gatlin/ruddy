# 11 Independent reference interpreter

Status: resolved
Type: task
Blocked by: 06, 07

Spec step 6: execute the shared portable IR with a different value
representation, run the same reflection/codec programs on it and on JS,
exercise 32-/64-bit and 53-bit integer domains, reject malformed artifacts.

## Answer

`interp/` is a second execution of the same linked artifact, with a different
value representation: `interp/src/value.rs` holds integers as Rust integers of
their own width, records as ordered maps, sums as tagged data, and functions,
continuations, handler identities and runtime type descriptors as distinct
variants — none of the JavaScript backend's "a callable with marker
properties" encoding. `interp/src/machine.rs` runs the continuation-passing
LIR directly: blocks, direct and indirect calls, handler entry and exit with
run ownership, cells, and the pattern tests. `interp/src/types.rs`,
`convert.rs` and `reflect.rs` re-implement descriptor instantiation,
projection, the checked foreign conversions, and the reflection intrinsics
over those values. `interp/src/prim.rs` implements the portable primitive
table, and `interp/src/number.rs` reproduces ECMAScript number formatting.
Nothing reads a JavaScript extern string: an extern outside the table is
reported as unsupported, naming it.

`tests/src/interp.rs` runs the same reflection and codec source programs
through Node and through the interpreter and asserts the two agree, covering
mirrors and descriptions, typed views and checked builders, JSON encoding and
strict decoding, the canonical profile, the positional binary format, and both
directions of the foreign boundary. The integer domains are exercised at 32,
53, and 64 bits — the last on the interpreter alone, since a JavaScript
manifest refuses it. Malformed artifacts are refused: an unlinked artifact, and
one naming a definition it does not define. `tests/src/interp_prim.rs` checks
the primitive table itself against the JavaScript in `src/backend/primitives.js`.

Also added: `std::ffi::encode`, the outgoing counterpart of `ffi::decode`, as
the `$ffiEncode` intrinsic in both runtimes; and a Portable panel in the
debugger that holds a program's host values against the primitive table.

