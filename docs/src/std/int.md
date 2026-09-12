---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::int

`int` provides arithmetic, comparison, conversion, and bounded operations for integers.

A signed difference can be converted to a nonnegative distance.

```ruddy
let distance_from_zero = std::int::abs (-14i)
```

## Values

### abs

```ruddy
extern abs: fn(Int) -> Int
```

The absolute value of an `Int` value.

### abs16

```ruddy
extern abs16: fn(Int16) -> Int16
```

The absolute value of an `Int16` value.

### abs32

```ruddy
extern abs32: fn(Int32) -> Int32
```

The absolute value of an `Int32` value.

### abs64

```ruddy
extern abs64: fn(Int64) -> Int64
```

The absolute value of an `Int64` value.

### abs8

```ruddy
extern abs8: fn(Int8) -> Int8
```

The absolute value of an `Int8` value.

### add

```ruddy
extern add: fn(Int, Int) -> Int
```

The sum of two `Int` values.

### add16

```ruddy
extern add16: fn(Int16, Int16) -> Int16
```

The sum of two `Int16` values, wrapping to 16 bits.

### add32

```ruddy
extern add32: fn(Int32, Int32) -> Int32
```

The sum of two `Int32` values, wrapping to 32 bits.

### add64

```ruddy
extern add64: fn(Int64, Int64) -> Int64
```

The sum of two `Int64` values, wrapping to 64 bits.

### add8

```ruddy
extern add8: fn(Int8, Int8) -> Int8
```

The sum of two `Int8` values, wrapping to 8 bits.

### clamp

```ruddy
extern clamp: fn(Int, Int, Int) -> Int
```

The `Int` value limited to the inclusive lower and upper bounds.

### clamp16

```ruddy
extern clamp16: fn(Int16, Int16, Int16) -> Int16
```

The `Int16` value limited to the inclusive lower and upper bounds.

### clamp32

```ruddy
extern clamp32: fn(Int32, Int32, Int32) -> Int32
```

The `Int32` value limited to the inclusive lower and upper bounds.

### clamp64

```ruddy
extern clamp64: fn(Int64, Int64, Int64) -> Int64
```

The `Int64` value limited to the inclusive lower and upper bounds.

### clamp8

```ruddy
extern clamp8: fn(Int8, Int8, Int8) -> Int8
```

The `Int8` value limited to the inclusive lower and upper bounds.

### compare

```ruddy
let compare: order::Ordering Int
```

The three-way comparison of two `Int` values.

### compare16

```ruddy
let compare16: order::Ordering Int16
```

The three-way comparison of two `Int16` values.

### compare32

```ruddy
let compare32: order::Ordering Int32
```

The three-way comparison of two `Int32` values.

### compare64

```ruddy
let compare64: order::Ordering Int64
```

The three-way comparison of two `Int64` values.

### compare8

```ruddy
let compare8: order::Ordering Int8
```

The three-way comparison of two `Int8` values.

### divide

```ruddy
extern divide: fn(Int, Int) -> Int
```

The first `Int` value divided by the second.

### divide16

```ruddy
extern divide16: fn(Int16, Int16) -> Int16
```

The first `Int16` value divided by the second.

### divide32

```ruddy
extern divide32: fn(Int32, Int32) -> Int32
```

The first `Int32` value divided by the second.

### divide64

```ruddy
extern divide64: fn(Int64, Int64) -> Int64
```

The first `Int64` value divided by the second.

### divide8

```ruddy
extern divide8: fn(Int8, Int8) -> Int8
```

The first `Int8` value divided by the second.

### equal

```ruddy
let equal: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### equal16

```ruddy
let equal16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### equal32

```ruddy
let equal32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### equal64

```ruddy
let equal64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### equal8

```ruddy
let equal8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### from_int16

```ruddy
extern from_int16: fn(Int) -> Int16
```

Converts an `Int` to `Int16`.

### from_int32

```ruddy
extern from_int32: fn(Int) -> Int32
```

Converts an `Int` to `Int32`.

### from_int64

```ruddy
extern from_int64: fn(Int) -> Int64
```

Converts an `Int` to `Int64`.

### from_int8

```ruddy
extern from_int8: fn(Int) -> Int8
```

Converts an `Int` to `Int8`.

### from_nat

```ruddy
extern from_nat: fn(Nat) -> Int
```

Converts a `Nat` to `Int`.

### from_nat16

```ruddy
extern from_nat16: fn(Nat) -> Int16
```

Converts a `Nat` to `Int16`.

### from_nat32

```ruddy
extern from_nat32: fn(Nat) -> Int32
```

Converts a `Nat` to `Int32`.

### from_nat64

```ruddy
extern from_nat64: fn(Nat) -> Int64
```

Converts a `Nat` to `Int64`.

### from_nat8

```ruddy
extern from_nat8: fn(Nat) -> Int8
```

Converts a `Nat` to `Int8`.

### from_real

```ruddy
extern from_real: fn(Real) -> Int
```

Converts a `Real` to `Int`.

### greater_than

```ruddy
let greater_than: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### greater_than16

```ruddy
let greater_than16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### greater_than32

```ruddy
let greater_than32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### greater_than64

```ruddy
let greater_than64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### greater_than8

```ruddy
let greater_than8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### greater_than_or_equal

```ruddy
let greater_than_or_equal: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### greater_than_or_equal16

```ruddy
let greater_than_or_equal16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### greater_than_or_equal32

```ruddy
let greater_than_or_equal32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### greater_than_or_equal64

```ruddy
let greater_than_or_equal64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### greater_than_or_equal8

```ruddy
let greater_than_or_equal8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### is_zero

```ruddy
let is_zero: Int -> Bool
```

Whether the `Int` value is zero.

### is_zero16

```ruddy
extern is_zero16: fn(Int16) -> Bool
```

Whether the `Int16` value is zero.

### is_zero32

```ruddy
extern is_zero32: fn(Int32) -> Bool
```

Whether the `Int32` value is zero.

### is_zero64

```ruddy
extern is_zero64: fn(Int64) -> Bool
```

Whether the `Int64` value is zero.

### is_zero8

```ruddy
extern is_zero8: fn(Int8) -> Bool
```

Whether the `Int8` value is zero.

### less_than

```ruddy
let less_than: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### less_than16

```ruddy
let less_than16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### less_than32

```ruddy
let less_than32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### less_than64

```ruddy
let less_than64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### less_than8

```ruddy
let less_than8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### less_than_or_equal

```ruddy
let less_than_or_equal: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### less_than_or_equal16

```ruddy
let less_than_or_equal16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### less_than_or_equal32

```ruddy
let less_than_or_equal32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### less_than_or_equal64

```ruddy
let less_than_or_equal64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### less_than_or_equal8

```ruddy
let less_than_or_equal8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### max

```ruddy
extern max: fn(Int, Int) -> Int
```

The greater of two `Int` values.

### max16

```ruddy
extern max16: fn(Int16, Int16) -> Int16
```

The greater of two `Int16` values.

### max32

```ruddy
extern max32: fn(Int32, Int32) -> Int32
```

The greater of two `Int32` values.

### max64

```ruddy
extern max64: fn(Int64, Int64) -> Int64
```

The greater of two `Int64` values.

### max8

```ruddy
extern max8: fn(Int8, Int8) -> Int8
```

The greater of two `Int8` values.

### max_value16

```ruddy
let max_value16: Int16
```

The greatest `Int16` value.

### max_value32

```ruddy
let max_value32: Int32
```

The greatest `Int32` value.

### max_value64

```ruddy
let max_value64: Int64
```

The greatest `Int64` value.

### max_value8

```ruddy
let max_value8: Int8
```

The greatest `Int8` value.

### min

```ruddy
extern min: fn(Int, Int) -> Int
```

The lesser of two `Int` values.

### min16

```ruddy
extern min16: fn(Int16, Int16) -> Int16
```

The lesser of two `Int16` values.

### min32

```ruddy
extern min32: fn(Int32, Int32) -> Int32
```

The lesser of two `Int32` values.

### min64

```ruddy
extern min64: fn(Int64, Int64) -> Int64
```

The lesser of two `Int64` values.

### min8

```ruddy
extern min8: fn(Int8, Int8) -> Int8
```

The lesser of two `Int8` values.

### min_value16

```ruddy
let min_value16: Int16
```

The least `Int16` value.

### min_value32

```ruddy
let min_value32: Int32
```

The least `Int32` value.

### min_value64

```ruddy
let min_value64: Int64
```

The least `Int64` value.

### min_value8

```ruddy
let min_value8: Int8
```

The least `Int8` value.

### multiply

```ruddy
extern multiply: fn(Int, Int) -> Int
```

The product of two `Int` values.

### multiply16

```ruddy
extern multiply16: fn(Int16, Int16) -> Int16
```

The product of two `Int16` values, wrapping to 16 bits.

### multiply32

```ruddy
extern multiply32: fn(Int32, Int32) -> Int32
```

The product of two `Int32` values, wrapping to 32 bits.

### multiply64

```ruddy
extern multiply64: fn(Int64, Int64) -> Int64
```

The product of two `Int64` values, wrapping to 64 bits.

### multiply8

```ruddy
extern multiply8: fn(Int8, Int8) -> Int8
```

The product of two `Int8` values, wrapping to 8 bits.

### negate

```ruddy
extern negate: fn(Int) -> Int
```

The negation. Integers have one zero: negating zero is zero.

### negate16

```ruddy
extern negate16: fn(Int16) -> Int16
```

The additive inverse of an `Int16` value.

### negate32

```ruddy
extern negate32: fn(Int32) -> Int32
```

The additive inverse of an `Int32` value.

### negate64

```ruddy
extern negate64: fn(Int64) -> Int64
```

The additive inverse of an `Int64` value.

### negate8

```ruddy
extern negate8: fn(Int8) -> Int8
```

The additive inverse of an `Int8` value.

### not_equal

```ruddy
let not_equal: Int -> Int -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int` values.

### not_equal16

```ruddy
let not_equal16: Int16 -> Int16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int16` values.

### not_equal32

```ruddy
let not_equal32: Int32 -> Int32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int32` values.

### not_equal64

```ruddy
let not_equal64: Int64 -> Int64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int64` values.

### not_equal8

```ruddy
let not_equal8: Int8 -> Int8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Int8` values.

### remainder

```ruddy
extern remainder: fn(Int, Int) -> Int
```

The remainder after dividing the first `Int` value by the second.

### remainder16

```ruddy
extern remainder16: fn(Int16, Int16) -> Int16
```

The remainder after dividing the first `Int16` value by the second.

### remainder32

```ruddy
extern remainder32: fn(Int32, Int32) -> Int32
```

The remainder after dividing the first `Int32` value by the second.

### remainder64

```ruddy
extern remainder64: fn(Int64, Int64) -> Int64
```

The remainder after dividing the first `Int64` value by the second.

### remainder8

```ruddy
extern remainder8: fn(Int8, Int8) -> Int8
```

The remainder after dividing the first `Int8` value by the second.

### subtract

```ruddy
extern subtract: fn(Int, Int) -> Int
```

The first `Int` value minus the second.

### subtract16

```ruddy
extern subtract16: fn(Int16, Int16) -> Int16
```

The first `Int16` value minus the second, wrapping to 16 bits.

### subtract32

```ruddy
extern subtract32: fn(Int32, Int32) -> Int32
```

The first `Int32` value minus the second, wrapping to 32 bits.

### subtract64

```ruddy
extern subtract64: fn(Int64, Int64) -> Int64
```

The first `Int64` value minus the second, wrapping to 64 bits.

### subtract8

```ruddy
extern subtract8: fn(Int8, Int8) -> Int8
```

The first `Int8` value minus the second, wrapping to 8 bits.

### to_int16

```ruddy
extern to_int16: fn(Int16) -> Int
```

Converts an `Int16` value to `Int`.

### to_int32

```ruddy
extern to_int32: fn(Int32) -> Int
```

Converts an `Int32` value to `Int`.

### to_int64

```ruddy
extern to_int64: fn(Int64) -> Int
```

Converts an `Int64` value to `Int`.

### to_int8

```ruddy
extern to_int8: fn(Int8) -> Int
```

Converts an `Int8` value to `Int`.

### to_string16

```ruddy
extern to_string16: fn(Int16) -> String
```

The decimal text of a `Int16` value.

### to_string32

```ruddy
extern to_string32: fn(Int32) -> String
```

The decimal text of a `Int32` value.

### to_string64

```ruddy
extern to_string64: fn(Int64) -> String
```

The decimal text of a `Int64` value.

### to_string8

```ruddy
extern to_string8: fn(Int8) -> String
```

The decimal text of a `Int8` value.

<!-- Generated by ruddy doc for std. -->
