# Numbers

`Nat` and `Int` have target-dependent sizes. On JavaScript they use `number`,
whose exact integer range is limited by its 53-bit significand. Their literal
suffixes remain `n` and `i`. Unsuffixed numeric literals and the arithmetic
operators `+`, `-`, `*`, `/` continue to use `Real`.

The fixed-width primitives have the same ranges on every target:

| Type | Minimum | Maximum | Example |
| --- | ---: | ---: | --- |
| `Nat8` | 0 | 255 | `255n8` |
| `Nat16` | 0 | 65535 | `65535n16` |
| `Nat32` | 0 | 4294967295 | `4294967295n32` |
| `Nat64` | 0 | 18446744073709551615 | `18446744073709551615n64` |
| `Int8` | -128 | 127 | `-128i8` |
| `Int16` | -32768 | 32767 | `-32768i16` |
| `Int32` | -2147483648 | 2147483647 | `-2147483648i32` |
| `Int64` | -9223372036854775808 | 9223372036854775807 | `-9223372036854775808i64` |

A minus immediately followed by digits is part of the literal: `-1.5`,
`-42i`, and `-128i8` are single tokens, usable in expressions, patterns, and
metadata. Real literals preserve negative zero (`-0.0`). Minus-prefixed
natural literals, including `-0n` and `-0n8`, are compile errors.

Separate an operator minus from the following digits: `a - 1` is subtraction,
while `f -1` applies `f` to a negative real literal. `- value` remains unary
real negation. Out-of-range literals are compile errors. Each width and signedness is a
separate type; annotations do not convert literals or values automatically.
The suffix is also used in literal patterns and metadata values.

## Arithmetic

The existing `std::nat` and `std::int` modules provide functions with a trailing
width. For example:

```hc
let wrapped : Nat8 = std::nat::add8 255n8 1n8 -- 0n8
let signed : Int8 = std::int::add8 127i8 1i8 -- -128i8
let exact : Nat64 = std::nat::subtract64 9007199254740993n64 9007199254740992n64 -- 1n64
```

Every width provides `add`, `subtract`, `multiply`, `divide`, `remainder`,
`equal`, `not_equal`, `less_than`, `less_than_or_equal`, `greater_than`,
`greater_than_or_equal`, `min`, `max`, `clamp`, `is_zero`, and `compare`, with
`8`, `16`, `32`, or `64` appended. The `int` module additionally provides
`negate` and `abs` at each width. `min_value8`, `max_value8`, and the other
widths expose the bounds.

Arithmetic wraps modulo 2 to the declared width, using two's complement for
signed results. Natural subtraction wraps too. Division truncates toward
zero; a signed remainder has the dividend's sign. Dividing or taking a
remainder by zero throws a runtime error. Negating or taking the absolute
value of the minimum signed value returns that minimum; dividing it by -1
also wraps to the minimum.

## Conversions

`from_nat8` and `from_int8` convert target-sized `Nat` and `Int` values to the
module's 8-bit type, wrapping to the destination width. The same names with
`16`, `32`, and `64` select the other widths. JavaScript inputs must be safe
integers; an invalid input throws a runtime error.

`nat::to_nat8` and `int::to_int8` convert back to the target-sized type, with
corresponding functions for the other widths. These conversions throw if the
result cannot be represented as a safe JavaScript integer. `to_string8` and
the other widths render every value exactly, including all 64-bit values.

## JavaScript representation

Widths up to 32 use `number`. Arithmetic reduces the result to the declared
range; 32-bit multiplication preserves the low bits with `Math.imul`.
`Nat64` and `Int64` use `bigint`, with results reduced to 64 bits. Generic
functions and containers preserve this representation. JavaScript externs
accepting or returning these types must follow these representations and
ranges, just as other externs must honor their declared types.
