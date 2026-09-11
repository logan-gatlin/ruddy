# 12 Streaming readers and writers

Status: open
Type: task
Blocked by: 06

Spec section "Streaming, failure, and limits": "Streaming readers and writers
declare their I/O effects, support incremental traversal, and need no
intermediate document tree." Acceptance asks for "streaming reduction without
a document tree" and "streaming failure ends its session and may leave
consumed input/output".

`std/codec.rud`, `std/json.rud`, and `std/binary.rud` ship buffered runners
only: every entry point takes a whole `String` or `[Nat8]` and answers with a
whole value. The `Read` and `Write` protocols are already the right shape for
a streaming handler, so what is missing is a handler that owns an incremental
source or sink, declares its I/O effects, and the session rules for a run that
ends part-way through.

## Comments

Implementer handoff at `cd906cf`: also account for custom codec effects.
`Encoder 'a` and `Decoder 'a` currently fix `run` to `!Write` and `!Read`.
The spec allows a custom codec's additional effects to remain visible through
the runner; discarding buffered output must not imply rolling those effects
back. Extend the interface/evidence forwarding so a custom effect is neither
silently erased nor rejected merely because it accompanies protocol effects.

Acceptance includes a custom codec performing an application effect, a runner
that propagates it to the caller, nested runners that leave the outer session
unchanged, and a streaming failure that ends only its codec session without
promising input rewind or transport closure. Use the same portable codec with
buffered and streaming format runners.
