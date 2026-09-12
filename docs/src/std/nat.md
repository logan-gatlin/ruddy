---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::nat

`nat` provides arithmetic, comparison, and conversion for natural numbers.

A quantity can be kept within inclusive bounds.

```ruddy
let table_size = std::nat::clamp (requested_size, 1n, 12n)
```

## Values

### add

```ruddy
extern add: fn(Nat, Nat) -> Nat
```

The sum of two `Nat` values.

### add16

```ruddy
extern add16: fn(Nat16, Nat16) -> Nat16
```

The sum of two `Nat16` values, wrapping to 16 bits.

### add32

```ruddy
extern add32: fn(Nat32, Nat32) -> Nat32
```

The sum of two `Nat32` values, wrapping to 32 bits.

### add64

```ruddy
extern add64: fn(Nat64, Nat64) -> Nat64
```

The sum of two `Nat64` values, wrapping to 64 bits.

### add8

```ruddy
extern add8: fn(Nat8, Nat8) -> Nat8
```

The sum of two `Nat8` values, wrapping to 8 bits.

### clamp

```ruddy
extern clamp: fn(Nat, Nat, Nat) -> Nat
```

The `Nat` value limited to the inclusive lower and upper bounds.

### clamp16

```ruddy
extern clamp16: fn(Nat16, Nat16, Nat16) -> Nat16
```

The `Nat16` value limited to the inclusive lower and upper bounds.

### clamp32

```ruddy
extern clamp32: fn(Nat32, Nat32, Nat32) -> Nat32
```

The `Nat32` value limited to the inclusive lower and upper bounds.

### clamp64

```ruddy
extern clamp64: fn(Nat64, Nat64, Nat64) -> Nat64
```

The `Nat64` value limited to the inclusive lower and upper bounds.

### clamp8

```ruddy
extern clamp8: fn(Nat8, Nat8, Nat8) -> Nat8
```

The `Nat8` value limited to the inclusive lower and upper bounds.

### compare

```ruddy
let compare: order::Ordering Nat
```

The three-way comparison of two `Nat` values.

### compare16

```ruddy
let compare16: order::Ordering Nat16
```

The three-way comparison of two `Nat16` values.

### compare32

```ruddy
let compare32: order::Ordering Nat32
```

The three-way comparison of two `Nat32` values.

### compare64

```ruddy
let compare64: order::Ordering Nat64
```

The three-way comparison of two `Nat64` values.

### compare8

```ruddy
let compare8: order::Ordering Nat8
```

The three-way comparison of two `Nat8` values.

### divide

```ruddy
extern divide: fn(Nat, Nat) -> Nat
```

The first `Nat` value divided by the second.

### divide16

```ruddy
extern divide16: fn(Nat16, Nat16) -> Nat16
```

The first `Nat16` value divided by the second.

### divide32

```ruddy
extern divide32: fn(Nat32, Nat32) -> Nat32
```

The first `Nat32` value divided by the second.

### divide64

```ruddy
extern divide64: fn(Nat64, Nat64) -> Nat64
```

The first `Nat64` value divided by the second.

### divide8

```ruddy
extern divide8: fn(Nat8, Nat8) -> Nat8
```

The first `Nat8` value divided by the second.

### equal

```ruddy
let equal: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### equal16

```ruddy
let equal16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### equal32

```ruddy
let equal32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### equal64

```ruddy
let equal64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### equal8

```ruddy
let equal8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### from_int

```ruddy
extern from_int: fn(Int) -> Nat
```

Converts an `Int` to `Nat`.

### from_int16

```ruddy
extern from_int16: fn(Int) -> Nat16
```

Converts an `Int` to `Nat16`.

### from_int32

```ruddy
extern from_int32: fn(Int) -> Nat32
```

Converts an `Int` to `Nat32`.

### from_int64

```ruddy
extern from_int64: fn(Int) -> Nat64
```

Converts an `Int` to `Nat64`.

### from_int8

```ruddy
extern from_int8: fn(Int) -> Nat8
```

Converts an `Int` to `Nat8`.

### from_nat16

```ruddy
extern from_nat16: fn(Nat) -> Nat16
```

Converts a `Nat` to `Nat16`.

### from_nat32

```ruddy
extern from_nat32: fn(Nat) -> Nat32
```

Converts a `Nat` to `Nat32`.

### from_nat64

```ruddy
extern from_nat64: fn(Nat) -> Nat64
```

Converts a `Nat` to `Nat64`.

### from_nat8

```ruddy
extern from_nat8: fn(Nat) -> Nat8
```

Converts a `Nat` to `Nat8`.

### from_real

```ruddy
extern from_real: fn(Real) -> Nat
```

Converts a `Real` to `Nat`.

### greater_than

```ruddy
let greater_than: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### greater_than16

```ruddy
let greater_than16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### greater_than32

```ruddy
let greater_than32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### greater_than64

```ruddy
let greater_than64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### greater_than8

```ruddy
let greater_than8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### greater_than_or_equal

```ruddy
let greater_than_or_equal: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### greater_than_or_equal16

```ruddy
let greater_than_or_equal16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### greater_than_or_equal32

```ruddy
let greater_than_or_equal32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### greater_than_or_equal64

```ruddy
let greater_than_or_equal64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### greater_than_or_equal8

```ruddy
let greater_than_or_equal8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### is_zero

```ruddy
let is_zero: Nat -> Bool
```

Whether the `Nat` value is zero.

### is_zero16

```ruddy
extern is_zero16: fn(Nat16) -> Bool
```

Whether the `Nat16` value is zero.

### is_zero32

```ruddy
extern is_zero32: fn(Nat32) -> Bool
```

Whether the `Nat32` value is zero.

### is_zero64

```ruddy
extern is_zero64: fn(Nat64) -> Bool
```

Whether the `Nat64` value is zero.

### is_zero8

```ruddy
extern is_zero8: fn(Nat8) -> Bool
```

Whether the `Nat8` value is zero.

### less_than

```ruddy
let less_than: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### less_than16

```ruddy
let less_than16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### less_than32

```ruddy
let less_than32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### less_than64

```ruddy
let less_than64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### less_than8

```ruddy
let less_than8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### less_than_or_equal

```ruddy
let less_than_or_equal: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### less_than_or_equal16

```ruddy
let less_than_or_equal16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### less_than_or_equal32

```ruddy
let less_than_or_equal32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### less_than_or_equal64

```ruddy
let less_than_or_equal64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### less_than_or_equal8

```ruddy
let less_than_or_equal8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### max

```ruddy
extern max: fn(Nat, Nat) -> Nat
```

The greater of two `Nat` values.

### max16

```ruddy
extern max16: fn(Nat16, Nat16) -> Nat16
```

The greater of two `Nat16` values.

### max32

```ruddy
extern max32: fn(Nat32, Nat32) -> Nat32
```

The greater of two `Nat32` values.

### max64

```ruddy
extern max64: fn(Nat64, Nat64) -> Nat64
```

The greater of two `Nat64` values.

### max8

```ruddy
extern max8: fn(Nat8, Nat8) -> Nat8
```

The greater of two `Nat8` values.

### max_value16

```ruddy
let max_value16: Nat16
```

The greatest `Nat16` value.

### max_value32

```ruddy
let max_value32: Nat32
```

The greatest `Nat32` value.

### max_value64

```ruddy
let max_value64: Nat64
```

The greatest `Nat64` value.

### max_value8

```ruddy
let max_value8: Nat8
```

The greatest `Nat8` value.

### min

```ruddy
extern min: fn(Nat, Nat) -> Nat
```

The lesser of two `Nat` values.

### min16

```ruddy
extern min16: fn(Nat16, Nat16) -> Nat16
```

The lesser of two `Nat16` values.

### min32

```ruddy
extern min32: fn(Nat32, Nat32) -> Nat32
```

The lesser of two `Nat32` values.

### min64

```ruddy
extern min64: fn(Nat64, Nat64) -> Nat64
```

The lesser of two `Nat64` values.

### min8

```ruddy
extern min8: fn(Nat8, Nat8) -> Nat8
```

The lesser of two `Nat8` values.

### min_value16

```ruddy
let min_value16: Nat16
```

The least `Nat16` value.

### min_value32

```ruddy
let min_value32: Nat32
```

The least `Nat32` value.

### min_value64

```ruddy
let min_value64: Nat64
```

The least `Nat64` value.

### min_value8

```ruddy
let min_value8: Nat8
```

The least `Nat8` value.

### multiply

```ruddy
extern multiply: fn(Nat, Nat) -> Nat
```

The product of two `Nat` values.

### multiply16

```ruddy
extern multiply16: fn(Nat16, Nat16) -> Nat16
```

The product of two `Nat16` values, wrapping to 16 bits.

### multiply32

```ruddy
extern multiply32: fn(Nat32, Nat32) -> Nat32
```

The product of two `Nat32` values, wrapping to 32 bits.

### multiply64

```ruddy
extern multiply64: fn(Nat64, Nat64) -> Nat64
```

The product of two `Nat64` values, wrapping to 64 bits.

### multiply8

```ruddy
extern multiply8: fn(Nat8, Nat8) -> Nat8
```

The product of two `Nat8` values, wrapping to 8 bits.

### not_equal

```ruddy
let not_equal: Nat -> Nat -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat` values.

### not_equal16

```ruddy
let not_equal16: Nat16 -> Nat16 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat16` values.

### not_equal32

```ruddy
let not_equal32: Nat32 -> Nat32 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat32` values.

### not_equal64

```ruddy
let not_equal64: Nat64 -> Nat64 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat64` values.

### not_equal8

```ruddy
let not_equal8: Nat8 -> Nat8 -> Bool
```

The Boolean comparisons derived from the three-way ordering of `Nat8` values.

### remainder

```ruddy
extern remainder: fn(Nat, Nat) -> Nat
```

The remainder after dividing the first `Nat` value by the second.

### remainder16

```ruddy
extern remainder16: fn(Nat16, Nat16) -> Nat16
```

The remainder after dividing the first `Nat16` value by the second.

### remainder32

```ruddy
extern remainder32: fn(Nat32, Nat32) -> Nat32
```

The remainder after dividing the first `Nat32` value by the second.

### remainder64

```ruddy
extern remainder64: fn(Nat64, Nat64) -> Nat64
```

The remainder after dividing the first `Nat64` value by the second.

### remainder8

```ruddy
extern remainder8: fn(Nat8, Nat8) -> Nat8
```

The remainder after dividing the first `Nat8` value by the second.

### subtract

```ruddy
extern subtract: fn(Nat, Nat) -> Nat
```

The first `Nat` value minus the second.

### subtract16

```ruddy
extern subtract16: fn(Nat16, Nat16) -> Nat16
```

The first `Nat16` value minus the second, wrapping to 16 bits.

### subtract32

```ruddy
extern subtract32: fn(Nat32, Nat32) -> Nat32
```

The first `Nat32` value minus the second, wrapping to 32 bits.

### subtract64

```ruddy
extern subtract64: fn(Nat64, Nat64) -> Nat64
```

The first `Nat64` value minus the second, wrapping to 64 bits.

### subtract8

```ruddy
extern subtract8: fn(Nat8, Nat8) -> Nat8
```

The first `Nat8` value minus the second, wrapping to 8 bits.

### to_nat16

```ruddy
extern to_nat16: fn(Nat16) -> Nat
```

Converts a `Nat16` value to `Nat`.

### to_nat32

```ruddy
extern to_nat32: fn(Nat32) -> Nat
```

Converts a `Nat32` value to `Nat`.

### to_nat64

```ruddy
extern to_nat64: fn(Nat64) -> Nat
```

Converts a `Nat64` value to `Nat`.

### to_nat8

```ruddy
extern to_nat8: fn(Nat8) -> Nat
```

Converts a `Nat8` value to `Nat`.

### to_string16

```ruddy
extern to_string16: fn(Nat16) -> String
```

The decimal text of a `Nat16` value.

### to_string32

```ruddy
extern to_string32: fn(Nat32) -> String
```

The decimal text of a `Nat32` value.

### to_string64

```ruddy
extern to_string64: fn(Nat64) -> String
```

The decimal text of a `Nat64` value.

### to_string8

```ruddy
extern to_string8: fn(Nat8) -> String
```

The decimal text of a `Nat8` value.

<!-- Generated by ruddy doc for std. -->
