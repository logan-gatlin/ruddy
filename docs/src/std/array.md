---
doc: true
layout: std.njk
stdReference: true
---

# [std](bundle.md)::array

`array` provides immutable operations on arrays.

Available items can be kept in their original order.

```ruddy
let available_items = std::array::filter is_available inventory
```

## Values

### all

```ruddy
let all: ('a -> Bool + ..'effects) -> ['a] -> Bool + ..'effects
```

Whether the predicate returns `true` for every value.

### any

```ruddy
let any: ('a -> Bool + ..'effects) -> ['a] -> Bool + ..'effects
```

Whether the predicate returns `true` for at least one value.

### concat

```ruddy
extern concat: ['a] -> ['a] -> ['a]
```

The values of the first array followed by the values of the second.

### enumerate

```ruddy
let enumerate: ['a] -> [(Nat, 'a)]
```

Pairs every value with its zero-based position.

### filter

```ruddy
let filter: ('a -> Bool + ..'effects) -> ['a] -> ['a] + ..'effects
```

The values for which the predicate returns `true`, in their original order.

### filter_map

```ruddy
let filter_map: ('a -> Option 'b + ..'effects) -> ['a] -> ['b] + ..'effects
```

The present results of applying the function, in their original order.

### find

```ruddy
let find: ('a -> Bool + ..'effects) -> ['a] -> Option 'a + ..'effects
```

The first value for which the predicate returns `true`.

### find_index

```ruddy
let find_index: ('a -> Bool + ..'effects) -> ['a] -> Option Nat + ..'effects
```

The zero-based position of the first value for which the predicate returns `true`.

### flat_map

```ruddy
let flat_map: ('a -> ['b] + ..'effects) -> ['a] -> ['b] + ..'effects
```

Concatenates the arrays produced by applying the function to every value.

### fold

```ruddy
let fold:
  ('state -> 'a -> 'state + ..'effects) -> 'state -> ['a] -> 'state + ..'effects
```

Combines the values from left to right, beginning with the supplied state.

### fold_right

```ruddy
let fold_right:
  ('a -> 'state -> 'state + ..'effects) -> 'state -> ['a] -> 'state + ..'effects
```

Combines the values from right to left, beginning with the supplied state.

### get

```ruddy
extern get: ['a] -> Nat -> Option 'a
```

The value at the zero-based position, or `#None` when the position is outside the array.

### len

```ruddy
extern len: ['a] -> Nat
```

The number of values in the array.

### map

```ruddy
let map: ('a -> 'b + ..'effects) -> ['a] -> ['b] + ..'effects
```

The results of applying the function to every value in order.

### partition

```ruddy
let partition: ('a -> Bool + ..'effects) -> ['a] -> (['a], ['a]) + ..'effects
```

Separates values by the predicate while preserving their order.

### pop

```ruddy
extern pop: ['a] -> Option ('a, ['a])
```

The last value and the remaining prefix, or `#None` for an empty array.

### prepend

```ruddy
extern prepend: ['a] -> 'a -> ['a]
```

A copy with the value added at the beginning.

### push

```ruddy
extern push: ['a] -> 'a -> ['a]
```

A copy with the value added at the end.

### reduce

```ruddy
let reduce: ('a -> 'a -> 'a + ..'effects) -> ['a] -> Option 'a + ..'effects
```

Combines a nonempty array from left to right, using its first value as the initial state.

### reverse

```ruddy
let reverse: ['a] -> ['a]
```

The values in reverse order.

### sequence_option

```ruddy
let sequence_option: [Option 'a] -> Option ['a]
```

Collects present values into an array, or returns `#None` when any value is absent.

### sequence_result

```ruddy
let sequence_result: [Result 'a 'failure] -> Result ['a] 'failure
```

Collects successful values into an array, or returns the first error.

### set

```ruddy
extern set: ['a] -> Nat -> 'a -> Option ['a]
```

A copy with the value at the zero-based position replaced, or `#None` when the position is outside the array.

### slice

```ruddy
extern slice: ['a] -> Nat -> Nat -> Option ['a]
```

The values from `start` up to `end`, or `#None` when either position is invalid.

### sort_by

```ruddy
let sort_by: ('a -> 'a -> order::Order + ..'effects) -> ['a] -> ['a] + ..'effects
```

The values in order, stable: values the ordering calls equal keep their order.

### traverse_option

```ruddy
let traverse_option:
  ('a -> Option 'b + ..'effects) -> ['a] -> Option ['b] + ..'effects
```

Applies the function in order and returns all results when every result is present.

### traverse_result

```ruddy
let traverse_result:
  ('a -> Result 'b 'failure + ..'effects)
  -> ['a]
  -> Result ['b] 'failure + ..'effects
```

Applies the function in order and returns all values or the first error.

### try_fold_option

```ruddy
let try_fold_option:
  ('state -> 'a -> Option 'state + ..'effects)
  -> 'state
  -> ['a]
  -> Option 'state + ..'effects
```

Combines values from left to right until the step returns `#None`.

### try_fold_result

```ruddy
let try_fold_result:
  ('state -> 'a -> Result 'state 'failure + ..'effects)
  -> 'state
  -> ['a]
  -> Result 'state 'failure + ..'effects
```

Combines values from left to right until the step returns an error.

### zip

```ruddy
let zip: ['a] -> ['b] -> [('a, 'b)]
```

Pairs corresponding values until either array ends.

<!-- Generated by ruddy doc for std. -->
