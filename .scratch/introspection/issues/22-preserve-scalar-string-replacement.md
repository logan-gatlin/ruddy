# 22 Preserve scalar string replacement semantics across backends

Status: open
Type: task
Priority: P1

Review baseline: `cd906cf`. Reproduced in both reviews; still open.

The [primitive contracts](../spec.md#portable-primitive-contracts) define
`String` as valid Unicode scalar values and require portable observable
semantics. Current replacement operations can create invalid strings and give
different results in JavaScript and the reference interpreter.

## Reproduction and impact

Run the same Ruddy source through both backends:

```ruddy
let empty_search = std::str::replace_all "😀" "" "."
let first_literal = std::str::replace_first "ab" "a" "$&"
let all_literal = std::str::replace_all "aba" "a" "$&"
```

| Expression | Current JavaScript result | Current interpreter result / required literal contract |
| --- | --- | --- |
| `empty_search` | `".\ud83d.\ude00."` | `".😀."` |
| `first_literal` | `"ab"` | `"$&b"` |
| `all_literal` | `"aba"` | `"$&b$&"` |

JavaScript inserts between UTF-16 surrogate halves for the empty search,
breaking the valid-scalar invariant even when every input is valid. It also
interprets replacement text as JavaScript substitution templates, whereas the
interpreter uses literal substring replacement.

## Work

Define and document one portable literal-substring replacement contract for
`replace_first` and `replace_all` in [std/str.rud](../../../std/str.rud).
Replacement text should be inserted literally, including `$&`, `$$`, and the
JavaScript prefix/suffix substitution spellings.

Fix [JavaScript primitives](../../../src/backend/primitives.js) to honor this
contract and scalar boundaries. With an empty search, `replace_all` inserts at
each scalar boundary, including before the first and after the last scalar;
`replace_first` inserts once at the beginning. Specify non-overlapping matches
for nonempty searches. Keep the
[interpreter primitives](../../../interp/src/prim.rs) aligned and remove their
comment treating the replacement mismatch as an intentional departure.

Do not weaken the Unicode contract to accommodate JavaScript's built-ins.
Changing replacement to a callback prevents substitution-template expansion,
but does not by itself fix empty-search insertion between surrogate halves.

## Acceptance

Run shared-source conformance cases on both backends for the examples above,
empty input, empty/nonempty searches, repeated or overlapping matches, and
replacement strings containing dollar substitution spellings. Assert scalar
validity and `str::len` on the astral-character output, not merely matching
rendered text. Run Rust tests only through `just test`.
