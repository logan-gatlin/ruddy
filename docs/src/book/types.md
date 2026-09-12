---
doc: true
bookNavigation:
  previous:
    path: "/book/data.html"
    title: "4. Modeling data and matching its shape"
  next:
    path: "/book/higher-order.html"
    title: "6. Higher-order programming"
---

# 5. Understanding inferred and structural types

A targeting system needs the distance from a player to an enemy.
The player also has health, while the enemy has a species and an attack cooldown.
Making those objects inherit from a common class, or copying both into temporary vectors, would add work unrelated to the geometry.
[Structural typing](../dictionary.md#structural-typing) lets a function require the fields it actually uses.
[Type inference](../dictionary.md#type-inference) discovers those requirements from the implementation.

## Following a requirement

A distance function needs the two coordinates of each object:

```ruddy
let distance_to = fn from to => do
  let dx = to.x - from.x
  let dy = to.y - from.y
  return std::real::sqrt (dx * dx + dy * dy)
end

let player = { x: 0, y: 0, health: 100n }
let enemy = { x: 3, y: 4, species: "slime", cooldown: 0.5 }
let beacon = { x: -3, y: 4, radius: 2 }

let enemy_distance = distance_to player enemy
let beacon_distance = distance_to player beacon
```

Both distances are `5`.
Reading `.x` and `.y` requires those fields on each parameter.
Subtraction requires their payloads to be `Real`, and [square root](../std/real.md) returns a `Real`.
Nothing in the body mentions health, species, cooldown, or radius, so those fields do not become requirements.

The two arguments need not have the same complete type.
Each has its own additional fields, and the same function works for all three object shapes through their shared coordinate fields.
No conversion or declared inheritance relationship is needed at the call.
An explicit annotation can express that requirement:

```ruddy
let distance_to_checked:
  { x: Real, y: Real, .. } -> { x: Real, y: Real, .. } -> Real = distance_to
```

Each `..` permits additional fields independently.
A closed type `{ x: Real, y: Real }` instead describes exactly those two fields, so it would reject the extra health field.
Structural typing does not mean that every extra field is accepted by every annotation.
The [rows chapter](rows.md#from-subtyping-to-row-polymorphism) compares this rule with subtyping and shows how a movement function preserves an object's additional fields in its result.

This interface checks that usable coordinates exist; it does not check coordinate conventions.
Passing a screen-space position and a world-space position can still be a gameplay error even when both have `Real` fields named `x` and `y`.

## Names describe structure

Two type names can describe the same structure:

```ruddy
type WorldPosition = { x: Real, y: Real }
type ScreenPosition = { x: Real, y: Real }
let world_position: WorldPosition = { x: 3, y: 4 }
let screen_position: ScreenPosition = world_position
```

This assignment is accepted: the names communicate intended roles but do not establish distinct type identities.
Renaming a type is therefore insufficient to prevent mixing coordinate spaces.
When that distinction matters, different tags can represent it:

```ruddy
type WorldPoint = #World { x: Real, y: Real }
type ScreenPoint = #Screen { x: Real, y: Real }

let to_screen: { x: Real, y: Real } -> WorldPoint -> ScreenPoint =
  fn camera point => match point with
  | #World position => #Screen {
      x: position.x - camera.x,
      y: position.y - camera.y,
    }
  end

let projected = to_screen { x: 1, y: 2 } (#World { x: 3, y: 4 })
```

`projected` is `#Screen { x: 2, y: 2 }`.
The conversion makes a translation-only camera rule explicit, and a screen point cannot be supplied where a world point is required.
The types separate coordinate spaces; they do not prove that the camera formula is correct or supply scaling and rotation automatically.

## Generic functions

A helper that returns its input needs no separate implementation for a health value, a position, or an enemy state.
It does need to preserve the relationship between its input and output.
A [type variable](../dictionary.md#type-variable) names that relationship without fixing one particular type:

```ruddy
let identity: 'a -> 'a = fn value => value
let enemy_name = identity "slime"
let lives = identity 3n
```

The repeated `'a` states that the input and output have the same type for a particular use.
It does not mean that every call must use one global type.
A generic function cannot assume operations that its parameter's type does not support.

Parameterized type definitions describe repeated data patterns.
A replay can attach a simulation tick to values of different types:

```ruddy
type AtTick 'a = { tick: Nat, value: 'a }
let recorded_position: AtTick WorldPosition = {
  tick: 12n,
  value: { x: 3, y: 4 },
}
```

`AtTick` preserves the payload's type while giving every record the same timing field.
A type parameter is useful when that relationship recurs throughout an API, rather than only in one function body.

## Investigating a mismatch

An enemy with `x: "east"` cannot be used by `distance_to`: its producer supplies text, while subtraction requires a real number.
An annotation can state the intended boundary and move the disagreement closer to its cause.
Changing the annotation alone cannot make those uses compatible.

A useful investigation follows the value from its producer to each consumer.
The producer establishes a shape; operations and annotations establish further requirements.
The repair depends on the intended contract: a level loader may need to reject an invalid coordinate, or an explicit conversion may belong at the input boundary.
A cast that merely claims the data has another type would not establish that the coordinate is meaningful.

## Summary and exercises

Functions can operate on the shared fields of different game objects.
Type names describe structure, while type variables and rows preserve relationships across operations.
Distinct tags can express distinctions that alias names alone do not enforce.

1. Add an object with `x`, `y`, and `light_radius` and predict its distance from the player.
2. Explain why the two arguments to `distance_to` do not need the same complete type.
3. Give an input that has correctly typed coordinates but uses the wrong coordinate space.
4. Extend `to_screen` with a scale factor and specify a check for a camera at the origin.
5. Define a generic function that attaches a tick to an arbitrary value and give its type.

[Selected answers](answers.md#types) trace the requirements and coordinate-space distinction.
