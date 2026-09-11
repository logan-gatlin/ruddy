# 23 Enforce codec resource limits before allocation

Status: open
Type: task
Priority: P2

Reviewed at `cd906cf`. This is a reproduced defect, separate from the future
streaming work in [12](12-streaming-codecs.md).

The [spec's streaming and limits contract](../spec.md#streaming-failure-and-limits)
requires byte, nesting, member, numeric-work, and schema-traversal limits to be
enforced before dangerous allocation. Several advertised limits currently do
nothing:

- [JSON encoding](../../../std/json.rud) checks depth but ignores output bytes
  and member counts; `emit` accumulates text without checking a budget.
- [Binary encoding](../../../std/binary.rud) likewise ignores bytes and member
  limits. Binary decoding never checks the input byte budget.
- JSON parsing/decoding compares `str::len input` against `limits.bytes`.
  `str::len` counts Unicode scalars, so this is not a byte limit.

These source declarations reproduce two failures with the current standard
library:

```ruddy
let json_limited = match std::codec::derive_encoder
  (std::reflect::type_of [1n, 2n]) with
| #Some encoder => std::json::encode_with encoder
    { depth: 10n, bytes: 1n, members: 0n, digits: 1n } [1n, 2n]
| #Error error => #Error (#Derive error)
end

let binary_limited = match std::codec::derive_encoder
  (std::reflect::type_of [1n, 2n]) with
| #Some encoder => std::binary::encode_with encoder
    { depth: 10n, bytes: 1n, members: 0n } [1n, 2n]
| #Error error => #Error (#Derive error)
end
```

Actual results are `#Some "[1,2]"` and `#Some` containing 20 binary bytes.
Both must fail with a limit error. Decoding those 20 bytes as `[Nat]` with
`bytes: 1n, members: 10n` also currently succeeds.

`json::parse_with { depth: 10n, bytes: 3n, members: 10n, digits: 10n }
"\"😀\""` currently succeeds: the input contains three scalars but six UTF-8
bytes. Specify the encoding used by the JSON byte budget and apply it
consistently; UTF-8 matches the portable text/binary boundary.

Implementation requirements:

- Account for input and output budgets before buffering, allocating, quoting,
  expanding number tokens, or copying the material that would exceed them.
  Avoid constructing a full extra encoded buffer merely to measure it.
- Count escaped text and structural delimiters in output bytes, and count
  fields/elements consistently with the documented member limit.
- Validate counts and arithmetic before conversions to narrower wire lengths;
  undefined ordinary arithmetic must not defeat checked codec limits.
- Include versioned binary headers and bodies in the operation's total byte
  budget. Cover document stringification and canonical buffering with explicit
  limits; coordinate schema/canonical work with
  [15](15-codec-combinators-and-schema-evolution.md).
- Preserve structured limit errors and useful paths. Buffered failure returns
  no partial output.

Acceptance checks:

- Independently exercise each limit at zero, exactly the boundary, and one
  unit beyond it. Use otherwise permissive limits to identify which budget
  caused a failure.
- Cover both writers and readers, multibyte and escaped strings, nested
  records/sequences, document helpers, and versioned binary output.
- A short token with a huge exponent must fail safely when conversion would
  exceed the numeric work budget; no unchecked large allocation first.
- Run the same portable codec programs on JS and the interpreter. Add tests
  under `tests/` and run Rust tests only through `just test`.
