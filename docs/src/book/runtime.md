---
doc: true
---

# D. Numbers, strings, and runtime behavior

Runtime behavior includes properties a program can observe even when its types are valid.
An introduction can use small values while still stating the boundaries that larger inputs encounter.
The [syntax reference](../grammar.md#literals) lists literal forms; generated module pages document individual operations.

## Numbers

Ruddy distinguishes real numbers, natural numbers, and signed integers.
Their literal spelling makes that choice explicit:

| Literal | Type | Meaning |
| --- | --- | --- |
| `3`, `3.5` | `Real` | IEEE binary64 floating-point values. |
| `3n` | `Nat` | A natural number in the target's domain. |
| `-3i` | `Int` | A signed integer in the target's domain. |
| `3n8`, `3n64` | `Nat8`, `Nat64` | A natural number with an explicit fixed width. |
| `-3i8`, `3i64` | `Int8`, `Int64` | A signed integer with an explicit fixed width. |

The JavaScript default uses 53-bit integer precision for `Nat` and `Int`.
`Nat` ranges from zero through `9007199254740991`; `Int` ranges from `-9007199254740991` through `9007199254740991`.
A manifest can select `integers = 32` for conventional 32-bit domains.
Fixed-width integer types retain their explicit domains.
An out-of-domain literal is rejected during compilation.

The operators `+`, `-`, `*`, and `/` operate on `Real`.
[Real](../std/real.md), [Nat](../std/nat.md), and [Int](../std/int.md) provide named operations and conversions.
An integer type does not imply arbitrary-precision arithmetic, and a type-correct total can still exceed its domain.
A spawn counter or accumulated score therefore needs a bound on its total as well as on each input.
Programs requiring a wider domain must choose a suitable representation and check boundary behavior.

`Real` includes infinities, NaNs, and signed zero.
A frame interval or movement speed need not be represented exactly, so a computation such as `0.1 + 0.2` should not be assumed to equal a decimal fraction represented with unlimited precision.
Conversion between numeric types is a separate operation and should be chosen explicitly.
Typed decoding adds representation checks; it is not equivalent to silently narrowing a host number.

## Strings

A `String` is a sequence of Unicode scalar values.
Positions and lengths count scalars, not UTF-8 bytes or JavaScript UTF-16 code units.
The following values demonstrate the distinction:

```ruddy
let symbol_count = std::str::len "😀"
let combined_count = std::str::len "é"
```

`symbol_count` is `1n`.
The second string contains `e` followed by a combining accent, so `combined_count` is `2n` even though it may display as one visible character.
A visible character and a scalar position are not interchangeable concepts.
A dialogue typewriter effect must account for that difference when revealing player-facing text.
[String](../std/str.md) documents slicing, searching, and conversion operations using the same scalar convention.

Quoted strings support the escape sequences listed in the syntax reference.
Raw multiline strings preserve their text without interpreting those escapes.
Bytes have a separate representation, such as `[Nat8]`, and require an encoding decision when converted to text.

## Stack safety and memory

Ruddy calls do not overflow the call stack, whether recursion is direct, mutual, higher-order, tail-recursive, or non-tail-recursive.
Foreign calls and host callback chains are outside that guarantee.
[The recursion chapter](functions.md#tail-calls-and-loops) explains why a tail call behaves like advancing a loop and avoids retaining pending return work.

Non-tail recursion can still require memory proportional to unfinished work.
Accumulated output, immutable versions retained for history, and state retained by closures all consume memory independently of stack safety.
The absence of stack overflow is not a guarantee of bounded total memory or termination.

## Execution and the environment

Effects describe which operations a function requires, not a promise that the environment will succeed.
Filesystem and HTTP operations can return errors under valid types.
The selected runtime supplies platform handlers, and local handlers can change the behavior of supported operations.
Immediate foreign observations have a separate contract described in [interoperability](interoperability.md).

An operation can complete immediately or after suspension while retaining ordinary Ruddy call syntax.
A foreign declaration must accurately state its completion contract.
A program should not infer parallel execution from the mere absence of an explicit waiting keyword.

[HTTP](../std/http.md) buffers responses and enforces configured limits.
The [platform guide](../platform-apis.md) records additional host contracts and limitations.
The [array cost table](data.md#arrays-are-immutable-trees) describes immutable tree operations.
The [traversal discussion](higher-order.md#the-cost-of-a-traversal) accounts for those costs in the current standard-library implementations.

---

[Book contents](../index.md) · [Standard-library reference](../std/bundle.md)
