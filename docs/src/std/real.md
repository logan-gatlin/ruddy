---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::real

`real` provides arithmetic and mathematical functions for real numbers.

An angle in degrees can be converted to radians.

```ruddy
let quarter_turn = std::real::degrees_to_radians 90.0
```

## Values

### abs

```ruddy
extern abs: fn(Real) -> Real
```

The absolute value.

### acos

```ruddy
extern acos: fn(Real) -> Real
```

The arccosine in radians.

### acosh

```ruddy
extern acosh: fn(Real) -> Real
```

The inverse hyperbolic cosine.

### add

```ruddy
let add: Real -> Real -> Real
```

The sum.

### asin

```ruddy
extern asin: fn(Real) -> Real
```

The arcsine in radians.

### asinh

```ruddy
extern asinh: fn(Real) -> Real
```

The inverse hyperbolic sine.

### atan

```ruddy
extern atan: fn(Real) -> Real
```

The arctangent in radians.

### atan2

```ruddy
extern atan2: fn(Real, Real) -> Real
```

The angle in radians from the origin to the point.

### atanh

```ruddy
extern atanh: fn(Real) -> Real
```

The inverse hyperbolic tangent.

### ceil

```ruddy
extern ceil: fn(Real) -> Int
```

The least integer not less than the value.

### clamp

```ruddy
extern clamp: fn(Real, Real, Real) -> Real
```

The value limited to the inclusive lower and upper bounds.

### compare

```ruddy
let compare: order::PartialOrdering Real
```

Numeric comparison: any NaN operand is unordered, and negative and positive zero compare equal.

### cos

```ruddy
extern cos: fn(Real) -> Real
```

The cosine of an angle in radians.

### cosh

```ruddy
extern cosh: fn(Real) -> Real
```

The hyperbolic cosine.

### degrees_to_radians

```ruddy
let degrees_to_radians: Real -> Real
```

Converts an angle from degrees to radians.

### divide

```ruddy
let divide: Real -> Real -> Real
```

The first value divided by the second.

### e

```ruddy
let e: Real
```

The base of natural logarithms.

### equal

```ruddy
let equal: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### exp

```ruddy
extern exp: fn(Real) -> Real
```

`e` raised to the value.

### expm1

```ruddy
extern expm1: fn(Real) -> Real
```

`e` raised to the value, minus one.

### floor

```ruddy
extern floor: fn(Real) -> Int
```

The greatest integer not greater than the value.

### from_int

```ruddy
extern from_int: fn(Int) -> Real
```

Converts an integer to a real number.

### from_nat

```ruddy
extern from_nat: fn(Nat) -> Real
```

Converts a natural number to a real number.

### greater_than

```ruddy
let greater_than: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### greater_than_or_equal

```ruddy
let greater_than_or_equal: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### is_finite

```ruddy
extern is_finite: fn(Real) -> Bool
```

Whether the value is neither infinite nor not a number.

### is_integer

```ruddy
extern is_integer: fn(Real) -> Bool
```

Whether the value is a finite integer.

### is_nan

```ruddy
extern is_nan: fn(Real) -> Bool
```

Whether the value is not a number.

### less_than

```ruddy
let less_than: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### less_than_or_equal

```ruddy
let less_than_or_equal: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### ln_10

```ruddy
let ln_10: Real
```

The natural logarithm of 10.

### ln_2

```ruddy
let ln_2: Real
```

The natural logarithm of 2.

### log

```ruddy
extern log: fn(Real) -> Real
```

The natural logarithm.

### log10

```ruddy
extern log10: fn(Real) -> Real
```

The base-10 logarithm.

### log10_e

```ruddy
let log10_e: Real
```

The base-10 logarithm of `e`.

### log1p

```ruddy
extern log1p: fn(Real) -> Real
```

The natural logarithm of one plus the value.

### log2

```ruddy
extern log2: fn(Real) -> Real
```

The base-2 logarithm.

### log2_e

```ruddy
let log2_e: Real
```

The base-2 logarithm of `e`.

### max

```ruddy
extern max: fn(Real, Real) -> Real
```

The greater value.

### min

```ruddy
extern min: fn(Real, Real) -> Real
```

The lesser value.

### multiply

```ruddy
let multiply: Real -> Real -> Real
```

The product.

### negate

```ruddy
let negate: Real -> Real
```

The additive inverse.

### not_equal

```ruddy
let not_equal: Real -> Real -> Bool
```

The Boolean comparisons derived from the partial numeric ordering.

### pi

```ruddy
let pi: Real
```

The ratio of a circle circumference to its diameter.

### power

```ruddy
extern power: fn(Real, Real) -> Real
```

The first value raised to the second.

### radians_to_degrees

```ruddy
let radians_to_degrees: Real -> Real
```

Converts an angle from radians to degrees.

### remainder

```ruddy
extern remainder: fn(Real, Real) -> Real
```

The remainder after division.

### round

```ruddy
extern round: fn(Real) -> Int
```

The value rounded to the nearest integer.

### sin

```ruddy
extern sin: fn(Real) -> Real
```

The sine of an angle in radians.

### sinh

```ruddy
extern sinh: fn(Real) -> Real
```

The hyperbolic sine.

### sqrt

```ruddy
extern sqrt: fn(Real) -> Real
```

The square root.

### sqrt_1_2

```ruddy
let sqrt_1_2: Real
```

The square root of one half.

### sqrt_2

```ruddy
let sqrt_2: Real
```

The square root of 2.

### subtract

```ruddy
let subtract: Real -> Real -> Real
```

The first value minus the second.

### tan

```ruddy
extern tan: fn(Real) -> Real
```

The tangent of an angle in radians.

### tanh

```ruddy
extern tanh: fn(Real) -> Real
```

The hyperbolic tangent.

### tau

```ruddy
let tau: Real
```

Twice `pi`.

### total_compare

```ruddy
let total_compare: order::Ordering Real
```

A total ordering for sorting: NaNs compare equal and last, and negative zero precedes positive zero.

### truncate

```ruddy
extern truncate: fn(Real) -> Int
```

The integer portion with the fractional portion removed.

<!-- Generated by ruddy doc for std. -->
