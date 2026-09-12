---
doc: true
bookNavigation:
  previous:
    path: "/book/functions.html"
    title: "3. Functions and application"
  next:
    path: "/book/types.html"
    title: "5. Understanding inferred and structural types"
---

# 4. Modeling data and matching its shape

A data model determines which situations a program can represent and what a consumer must handle.
Ruddy combines [structs](../dictionary.md#struct), [tuples](../dictionary.md#tuple), [arrays](../dictionary.md#array), and [tags](../dictionary.md#tag) to describe those situations.

## Fields and immutable updates

A movement step should produce a new player state while leaving the previous frame available for comparison or replay.
A struct spread expresses the changed coordinate without listing every unchanged field:

```ruddy
let original = { name: "Scout", x: 0, y: 0, health: 100n }
let updated = { x: 3, ..original }
```

`updated.x` is `3`; `original.x` remains `0`.
The name, `y` coordinate, and health are preserved.
A struct permits one spread, written last, and explicit fields before it replace matching fields from the spread.
An update can also add a field or replace it with a value of another type; the result then has the corresponding new structure.
The [rows chapter](rows.md) uses that ability to preserve object-specific fields through movement and state transitions.

A tuple serves when positions carry enough meaning, such as a pair of values returned together.
Named `x` and `y` fields often communicate geometry interfaces more clearly than numeric positions.

## Alternative cases

A targeting system may have no selected entity.
Using entity ID zero to mean “no target” would reserve an otherwise valid ID and rely on every caller remembering the convention.
The prelude's [Option](../std/option.md) represents presence and absence as different tags:

```ruddy
let target_label: Option Nat -> String = fn target => match target with
| #Some id => std::str::concat "Target " (std::str::from_nat id)
| #None => "No target"
end
```

`#Some 0n` identifies entity zero; `#None` identifies no entity.
The `#Some` pattern binds its payload to `id`, while the `#None` pattern needs no payload.
Both branches produce a string.
A tag's name is case-sensitive, and its payload is grouped with parentheses when it contains an application.

Absence does not decide gameplay policy.
An enemy without a target might patrol, wait, or search; the consuming function chooses what to do.

A [sum type](../dictionary.md#sum-type) names a set of alternatives and the data each needs.
An enemy state can carry only the fields meaningful in that state:

```ruddy
type EnemyState =
  #Patrol { waypoint: Nat }
  | #Chase { target: Nat }
  | #Stunned { remaining: Real }

let state_label: EnemyState -> String = fn state => match state with
| #Patrol { waypoint } => "patrolling"
| #Chase { target } => "chasing"
| #Stunned { remaining } => "stunned"
end
```

A chase state must carry a target; a stunned state carries a remaining duration instead.
This avoids a loose collection of optional fields that could claim the enemy is both chasing and stunned, or chasing without a target.
The duration's type alone still permits negative values, so constructing a sensible timer needs a gameplay rule.
[Presence polymorphism](rows.md#presence-is-inferred-from-program-shape) later describes interfaces that also preserve knowledge of which state shape is available.

[Result](../std/result.md) distinguishes a successful `#Some value` from `#Error error`.
A level loader can return a structured error without pretending an invalid level is an empty one.
Ruddy's success tag is `#Some`, including when the result contains another optional value.

## Arrays and patterns

Arrays are immutable sequences with compatible element types.
A pattern can distinguish an empty sequence from a sequence with a first value:

```ruddy
let first_or: String -> [String] -> String = fn fallback values => match values with
| [] => fallback
| [first, ..] => first
end
```

The `..` permits the remaining elements without binding them.
The pattern `[first, ..rest]` would also bind an array of remaining elements to `rest`.
Patterns appear in `match` arms and bindings; ordinary `fn` parameters are names or `_`.
A function that only matches its argument can use the shorthand documented in the [syntax reference](../grammar.md#patterns).

## Arrays are immutable trees

> A Ruddy array is stored as an immutable tree, not a contiguous mutable buffer or an OCaml linked list.
> Familiar bracket syntax does not imply the same operation costs as either representation.

If updating one element copied every element, keeping both the old and new arrays would become expensive.
The tree allows an update to build a new path to the changed element while sharing the unchanged branches.
Neither version can modify those branches, so retaining them in both versions is safe.
This is [structural sharing](../dictionary.md#structural-sharing).

For an array of length `n`, the current performance contracts are:

| Operation | Time | What happens |
| --- | --- | --- |
| `array::len` | O(1) | Read the stored length. |
| `array::get` | O(log n) | Follow a path to an element. |
| `array::set` | O(log n) | Copy a path and share unchanged branches. |
| `array::push`, `array::prepend`, `array::pop` | O(log n) | Adjust a boundary while retaining shared structure. |
| `array::slice` | O(log n) | Retain the selected interior and adjust boundaries. |
| `array::concat` | O(log(n + m)) | Join arrays of lengths `n` and `m` with boundary rebalancing. |
| A literal of `n` elements | O(n) | Build the initial structure. |

The tree is wide, so paths are shallow, but indexing is still not a constant-time contract.
A pattern such as `[first, ..rest]` also performs tree operations when it extracts the element and remainder.
It should not be costed as taking the head and tail of a linked list.
The [higher-order chapter](higher-order.md#the-cost-of-a-traversal) returns to what that means for a recursive traversal.

Sharing avoids full copies, not all allocation.
An update needs new structure, and retaining old versions can keep old elements alive.
The [state chapter](state.md#sharing-and-the-cost-of-change) compares these costs with explicit mutation.
The [Array reference](../std/array.md) supplies the operations and result types.

## Summary and exercises

Structs combine information; tags distinguish alternatives; patterns consume the resulting structure.
Immutable updates produce new values while preserving the input.

1. Extend `EnemyState` with a sleeping state and update `state_label`.
2. Explain the difference between `#Some 0n` and `#None` in a targeting system.
3. Implement a function that returns the second array element as an option, including arrays of lengths zero and one.

[Selected answers](answers.md#data) include the short-array cases.

