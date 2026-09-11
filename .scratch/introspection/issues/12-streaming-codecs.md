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
