# 24 Check declared sequence lengths before reporting encoder success

Status: open
Type: task
Priority: P2

Reviewed at `cd906cf`; reproduced with the current compiler and generated JS.
The [spec's protocol contract](../spec.md#a-protocol-that-works-beyond-json)
requires checked child transitions and a complete declared root before a
runner reports success.

In [binary encoding](../../../std/binary.rud), `begin_sequence count` writes
the count and then replaces the frame with `count: 0n`, losing the expected
length. `end_sequence` passes the actual count as its own expected count, so
the completion comparison cannot fail. JSON also ignores the declared count.

```ruddy
@private
let incomplete: std::codec::Encoder () = {
  schema: std::reflect::describe (std::reflect::type_of [1n]),
  run: fn _ => std::result::and_then
    (fn cursor => std::codec::!Write.end_sequence cursor)
    (std::codec::!Write.begin_sequence 2n),
}

let encoded = std::binary::encode_with
  incomplete std::binary::default_limits ()
```

Actual: `#Some [2n8, 0n8, 0n8, 0n8]`, a header declaring two elements with no
payloads. Expected: `#Error (#Codec ...)` with a protocol failure. The JSON
runner currently accepts the same encoder and returns `#Some "[]"`.

Implementation requirements:

- Retain the expected count separately from the number of completed children.
  Reject an extra element and reject closing before the declared count is met.
- Keep the pending-child check: opening a child is not completing it. Nested
  frames and stale cursors must not change another frame's counts.
- Check that counts fit the binary format's length representation before
  emitting a header; do not silently truncate to 32 bits.
- Apply the shared protocol's count contract in both JSON and binary handlers.
  Keep byte/member budgets coordinated with
  [23](23-enforce-codec-resource-limits.md).

Acceptance checks cover zero-length success, exact-length success, too few and
too many elements, an unfinished final child, nested sequences, and a count
that exceeds the binary header's range. Buffered failure returns no output.
Exercise both handlers and both executable backends with custom codecs, not
only default derivation. Add tests under `tests/`; use only `just test` for
Rust test execution.
