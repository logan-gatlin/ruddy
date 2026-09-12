---
doc: true
bookNavigation:
  previous:
    path: "/book/getting-started.html"
    title: "1. Getting started with Ruddy"
  next:
    path: "/book/functions.html"
    title: "3. Functions and application"
---

# 2. Values, expressions, and bindings

Moving a game object needs a speed, an elapsed time, and a rule for finding its next position.
Writing the calculation as an [expression](../dictionary.md#expression) gives it a result that can be passed directly to another calculation.
A [binding](../dictionary.md#binding) names an input or intermediate result so its role is clear.
[Values](../dictionary.md#value) such as `3`, `true`, and `"ready"` need no further calculation.

## Evaluation and types

The following definitions calculate a displacement from two named inputs:

```ruddy
let speed = 12.5
let seconds = 4
let displacement = speed * seconds
```

The multiplication evaluates to `50`.
Unsuffixed numeric literals have type `Real`, and the arithmetic operators operate on that type.
Natural numbers such as `4n` have type `Nat`; signed integers such as `-4i` have type `Int`.
Their arithmetic uses functions from [Nat](../std/nat.md) and [Int](../std/int.md).
The [numbers appendix](runtime.md#numbers) explains their domains and conversions.

[Evaluation](../dictionary.md#evaluation) describes what an expression does.
Typing describes which uses of its result are permitted before execution.
These are the [dynamic semantics](../dictionary.md#dynamic-semantics) and [static semantics](../dictionary.md#static-semantics) of the expression.
Type inference determines constraints from the operations used; it does not run the program to guess a type.

## Conditional expressions

A dash adds a bonus to the distance an object moves.
If the conditional itself produces that bonus, the program can add it directly instead of creating a variable and assigning to it in both branches.
This definition chooses a dash bonus and adds it to the ordinary displacement:

```ruddy
let dash_distance = fn walking displacement =>
  displacement + (if walking then 0 else 5 end)
```

With arguments `false` and `50`, the conditional evaluates to `5` and the result is `55`.
Only the selected branch is evaluated.
The condition must have type `Bool`, and both branches must have compatible result types even if a particular call selects only one.
The addition must work whichever branch is selected.
That is why replacing the second branch with `"five"` fails type checking: an unselected branch in one call may be selected in another.

The general form is `if condition then consequence else alternative end`.
Both result branches are required.
A chain of `else if` branches shares one final `end`.

## Blocks and scope

A long calculation is easier to explain when its intermediate results have names.
A `do` block supplies those names locally, so they do not become unrelated parts of the surrounding module.
Its final `return` supplies the block's value:

```ruddy
let travel_distance = fn speed seconds => do
  let displacement = speed * seconds
  let dash_bonus = 5
  return displacement + dash_bonus
end
```

For `travel_distance 12.5 4`, the local `displacement` is `50`, `dash_bonus` is `5`, and the block result is `55`.
Those local names are not visible outside the block.
This is [lexical scope](../dictionary.md#lexical-scope): the source structure determines where a name is meaningful.

`return` belongs to its enclosing block and does not exit a function.
A nested block can supply one input to a surrounding calculation:

```ruddy
let distance_with_bonus = fn displacement =>
  displacement + (do return 5 end)
```

A block without `return` produces `()`.
The form `_ = expression` evaluates an expression and discards its result; it will be useful when [sequencing effects](effects.md).

## Bindings and mutation

The normal movement speed should remain available when another calculation derives the speed for a slowed character.
Ordinary bindings keep those values stable; changing state requires an explicit [mutable cell](state.md).
An inner scope can introduce a new binding with the same name, called [shadowing](../dictionary.md#shadowing), without updating the outer binding:

```ruddy
let combined_speeds = do
  let speed = 10
  let slowed = do
    let speed = 8
    return speed
  end
  return speed + slowed
end
```

The result is `18`.
The inner `speed` is `8`; the outer `speed` remains `10`.
Distinct names would make this example clearer in ordinary application code.
[Mutable cells](state.md) provide explicit state when it is needed.

## Summary and exercises

An expression has both evaluation behavior and type constraints.
A block creates a scope, and its `return` provides a value to the enclosing expression.

1. Trace `dash_distance true 30` without executing it.
2. Explain why an invalid unselected branch can still cause a type error.
3. Rewrite `combined_speeds` with distinct names and justify why the result is unchanged.

[Selected answers](answers.md#expressions) give the evaluation and typing arguments.

