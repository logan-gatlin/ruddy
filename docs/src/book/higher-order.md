---
doc: true
bookNavigation:
  previous:
    path: "/book/types.html"
    title: "5. Understanding inferred and structural types"
  next:
    path: "/book/modules.html"
    title: "7. Organizing programs"
---

# 6. Higher-order programming

A [higher-order function](../dictionary.md#higher-order-function) accepts or returns a function.
Its practical purpose is often to retain the structure of a computation while allowing one part of its behavior to vary.
[Partial application](functions.md#functions-that-return-functions) provides one way to construct that varying behavior.

## Finding the repeated computation

Two array transformations can differ only in the operation applied to each element.
The following implementation converts an array of health values into an array healed by a fixed amount:

```ruddy
let heal_all = fn healths => match healths with
| [] => []
| [health, ..rest] => [health + 3, ..heal_all rest]
end
```

A difficulty setting might instead multiply every health value by a factor.
It would repeat the empty case, the first/rest decomposition, and the result construction.
The operation on `health` is the part worth making a parameter.

This generalization takes a function as that parameter:

```ruddy
let transform = fn operation values => match values with
| [] => []
| [value, ..rest] => [operation value, ..transform operation rest]
end

let healed = transform (fn health => health + 3) [10, 20]
let doubled = transform (fn health => health * 2) [10, 20]
```

The results are `[13, 23]` and `[20, 40]`.
The input and output elements need not have the same type: a debug overlay could convert each health to text.
This pattern is called mapping, and [Array](../std/array.md) provides it as `map`.

## Selecting and combining

Filtering retains input elements that satisfy a [predicate](../dictionary.md#predicate).
The predicate returns `Bool`; the retained elements keep their order.
A pipeline can express filtering followed by transformation:

```ruddy
using std::{array, real}

let heal_survivors = fn healths =>
  healths
  |> array::filter (fn health => real::greater_than health 0)
  |> array::map (fn health => health + 3)
```

The [pipeline](../dictionary.md#pipeline) operator passes its left value to its right function.
Thus `values |> f |> g` expresses `g (f values)`.
The partially applied array functions still expect their array argument.

A [fold](../dictionary.md#fold) combines elements with an accumulated state, called an [accumulator](../dictionary.md#accumulator).
This definition totals health from left to right, beginning at zero:

```ruddy
let total = fn healths =>
  std::array::fold (fn accumulated health => accumulated + health) 0 healths
```

For `[10, 20, 5]`, the successive states are `0`, `10`, `30`, and `35`.
The step function returns the next state, so its result must have the state type.

## Choosing an abstraction

Order-sensitive operations expose the difference between folds.
A left fold of subtraction over `[10, 3]`, starting at `0`, computes `(0 - 10) - 3`, which is `-13`.
A right fold with element-minus-state computes `10 - (3 - 0)`, which is `7`.
`fold` and `fold_right` also differ in the positions of the state and element arguments to their callbacks.
The API reference supplies their exact contracts.

A direct recursive function may communicate a stopping condition more clearly than a fold.
A library operation such as `find` can state the intent more directly still.
Abstraction is useful when it exposes the shared computation; brevity alone is not the criterion.
These examples use [pure functions](../dictionary.md#pure-function) as [callbacks](../dictionary.md#callback).
[Effects](effects.md#effects-in-higher-order-functions) explain what changes when a callback performs operations.

## The cost of a traversal

The recursive transformations above repeatedly bind an array's remainder and build a result from smaller arrays.
Ruddy's [tree-backed arrays](data.md#arrays-are-immutable-trees) make splitting and joining logarithmic, avoiding the full copy on every step that a naive immutable flat buffer would require.
They do not give those steps the constant-time cost of linked-list head and tail operations.

The current `map`, `filter`, and `fold` implementations use recursive array patterns and these tree operations.
For constant-time callbacks, accounting for a logarithmic operation at each of `n` steps gives an O(n log n) worst-case bound for these implementations.
The callback's own work must be added to that cost.
A fold's tail-call structure controls pending return work, but does not by itself make its array operations constant-time.

When only one matching element is needed, `find` can stop before traversing the remaining input.
When only a count or total is needed, a fold avoids retaining an intermediate array of selected values.
These changes reduce unnecessary work and retained data without assuming that a particular style is always faster.

## Summary and exercises

Mapping transforms each element, filtering selects elements, and folding carries state across elements.
All three make a varying operation explicit as a function value.

1. Replace `heal_all` with a use of `array::map`.
2. Write a function that counts living enemies from their health values without constructing an array of matching values.
3. Trace both subtraction folds and explain why addition alone is a poor example for distinguishing direction.

[Selected answers](answers.md#higher-order-programming) include a counting fold.

