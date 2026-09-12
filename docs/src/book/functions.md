---
doc: true
bookNavigation:
  previous:
    path: "/book/expressions.html"
    title: "2. Values, expressions, and bindings"
  next:
    path: "/book/data.html"
    title: "4. Modeling data and matching its shape"
---

# 3. Functions and application

A damage rule should have one implementation even when it is used by a combat simulation, a weapon preview, and an AI planner.
A function gives that rule a name and identifies the inputs allowed to vary.
Separating the changing inputs from the fixed rule also makes it possible to specialize the rule for one context.

## Reading a function

The following function computes attack damage from an attacker's strength:

```ruddy
let attack_damage = fn strength => strength * 0.5 + 3
```

`fn` introduces the parameter `strength`; `=>` separates that parameter from the body.
The arithmetic operations require `strength` to be `Real` and produce a `Real` result, so the function has type `Real -> Real`.
The arrow describes an input and an output.
An annotation can state the same constraint explicitly:

```ruddy
let attack_damage_checked: Real -> Real = fn strength => strength * 0.5 + 3
```

An annotation is checked against the implementation.
It is useful when a public contract should remain fixed or when a diagnostic needs a more precise location.

## Application and grouping

The call `attack_damage 10` evaluates to `8`.
The call `attack_damage (5 + 5)` first computes its argument and produces the same result.
Without parentheses, `attack_damage 5 + 5` adds `5` to the result of the call with argument `5`, producing `10.5`.
[Application](../dictionary.md#function-application) groups more tightly than arithmetic.

## Functions that return functions

Several parameters are shorthand for nested functions.
These definitions describe the same calculation:

```ruddy
let add_bonus = fn bonus damage => bonus + damage
let add_bonus_nested = fn bonus => fn damage => bonus + damage
```

The function type is `Real -> Real -> Real`, grouped as `Real -> (Real -> Real)`.
Applying one argument produces another function:

```ruddy
let apply_weapon_bonus = add_bonus 3
let boosted_damage = apply_weapon_bonus 20
```

`boosted_damage` is `23`.
The resulting function retains access to `bonus` through its lexical scope.
A function together with the bindings it retains is a [closure](../dictionary.md#closure).

The bonus can be chosen once when configuring a weapon, leaving the base damage to arrive with each attack.
That is the practical use of the returned function: callers can pass around an already configured operation without repeating its configuration.
Supplying fewer arguments in this way is [partial application](../dictionary.md#partial-application).
The representation of several arguments through nested functions is [currying](../dictionary.md#currying).
Function calls group leftward: `add_bonus 3 20` means `(add_bonus 3) 20`.
Function arrows group rightward because the first application returns the remaining function.

## Recursion

[Recursion](../dictionary.md#recursion) occurs when a function refers to itself.
Ruddy requires no separate recursion keyword.
A wave spawner can put one enemy in wave one, two in wave two, and so on.
This function counts all enemies scheduled through a given wave:

```ruddy
using std::nat

let spawn_total = fn number =>
  if nat::is_zero number then 0n
  else nat::add number (spawn_total (nat::subtract number 1n))
  end
```

`spawn_total 3n` adds `3n` to the result of `spawn_total 2n`, which adds `2n` to the result of `spawn_total 1n`.
The call with `0n` returns the base result.
Each recursive call makes progress toward that base case.
A well-typed recursive function can still fail to terminate, so type checking does not prove this progress argument.
The example also inherits the arithmetic domain and overflow behavior of [Nat](../std/nat.md).

## Tail calls and loops

The recursive `spawn_total` leaves an addition waiting while the smaller sum is calculated.
For `3n`, the pending work is `3n + (2n + (1n + 0n))`.
A loop would keep the sum calculated so far instead:

```text
remaining = input
total = 0
while remaining is not zero:
    total = total + remaining
    remaining = remaining - 1
result = total
```

The same progression can be expressed by making the loop state into function parameters:

```ruddy
let spawn_total_loop = fn number => do
  let step = fn remaining total =>
    if nat::is_zero remaining then total
    else step (nat::subtract remaining 1n) (nat::add total remaining)
    end
  return step number 0n
end
```

Each call to `step` supplies the state for the next iteration.
For `3n`, the pairs are `(3n, 0n)`, `(2n, 3n)`, `(1n, 5n)`, and `(0n, 6n)`.
The recursive call is the branch's result: there is no addition or other computation waiting for it to return.
A call in that position is a [tail call](../dictionary.md#tail-call).
Returning its result directly allows Ruddy to continue with the next state without retaining a chain of pending returns, just as a loop continues with updated loop variables.

## Stack safety and memory

Ruddy programs do not overflow the call stack through Ruddy calls.
This covers non-tail recursion as well as tail calls, including mutual recursion and calls through function values.
Foreign code and recursive chains through foreign calls and host callbacks are outside that guarantee.

Stack safety does not remove the need to store unfinished work.
The original `spawn_total` retains pending additions proportional to its input, using memory even though it does not consume an unbounded call stack.
`spawn_total_loop` needs the current pair of values rather than that chain.
Either version is still subject to available memory and the numeric domain; a tail-recursive loop that keeps accumulating an array can consume increasing memory too.
Tail calls are therefore useful for expressing iteration efficiently, not a requirement for avoiding stack overflow in ordinary Ruddy recursion.

## Summary and exercises

Function syntax, nested functions, and partial application describe one consistent evaluation model.
[Higher-order programming](higher-order.md) uses these values to factor out repeated behavior.

1. Give the type and result of `add_bonus 3` and `add_bonus 3 20` separately.
2. Explain the difference between `attack_damage (5 + 5)` and `attack_damage 5 + 5`.
3. Write a tail-recursive function that counts down from a remaining frame count and returns the number of simulated frames.
4. Identify the pending work in `spawn_total` and explain where the loop version keeps the equivalent information.

[Selected answers](answers.md#functions) cover application and grouping.

