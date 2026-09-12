---
doc: true
bookNavigation:
  previous:
    path: "/book/rows.html"
    title: "13. Rows, presence, and richer interfaces"
  next:
    path: "/book/interoperability.html"
    title: "15. JavaScript interoperability"
---

# 14. Hidden types, mirrors, and generic operations

A debug HUD might display a mixture of remaining lives, enemy names, and state flags in one collection.
An ordinary array needs one element type, but converting everything to text immediately would discard the original values.
A package can instead keep each value together with its own display function.
A [hidden type](../dictionary.md#hidden-type) lets the producer choose the value's type while promising that the accompanying function accepts it.
The consumer only needs that relationship, not the particular type chosen for each package.

## Packaging a value with its operation

A displayable package contains a value and a function that accepts exactly that value's hidden type:

```ruddy
type Displayable = hide 'a => { value: 'a, show: 'a -> String }

let lives_display: Displayable = { value: 3n, show: std::str::from_nat }
let enemy_display: Displayable = { value: "slime", show: fn text => text }
```

The packages choose different types, but both have the public type `Displayable`.
The annotation supplies the expected hidden type at the packaging boundary.
The compiler obtains the particular hidden type from the value; it does not guess one for an ambiguous empty value.

A consumer opens the package inside a pattern:

```ruddy
let display: Displayable -> String = fn item => match item with
| hide 'item { value, show } => show value
end
```

Within the arm, `'item` names one type shared by `value` and the argument of `show`.
The consumer need not know whether that type is `Nat` or `String`.
Returning `value` directly would let an unknown type escape its scope and is rejected; returning the string produced by `show` is valid.
An array of `Displayable` packages can therefore be heterogeneous internally while retaining one array element type.

## Evidence for a type

A [mirror](../dictionary.md#mirror) is a value that supplies authentic evidence for a type.
The compiler supplies it at a use whose type has been determined.
The following annotation selects the type whose mirror is requested:

```ruddy
let count_mirror: Mirror Nat = std::reflect::mirror ()
let count_description = std::reflect::describe count_mirror
```

The description is ordinary data describing a finite graph of type structure.
It can be inspected, but editing it cannot create a new mirror or authorize a conversion.
`reflect::type_of value` obtains a mirror of the value's static type at that use; it does not discover an arbitrary new type by examining host data.
The [Reflect](../std/reflect.md) reference separates descriptions from typed views.

## Different actor states in one update list

A moving enemy and a stationary beacon can have different internal state layouts while exposing the same operations.
A hidden type can keep each actor's state paired with the update function that understands it:

```ruddy
type Actor = hide 'state => {
  witness: Mirror 'state,
  state: 'state,
  update: Real -> 'state -> 'state,
  position: 'state -> { x: Real, y: Real },
}

let moving_actor: Actor = {
  witness: std::reflect::mirror (),
  state: { x: 0, y: 0, speed: 2 },
  update: fn dt state => { x: state.x + dt * state.speed, ..state },
  position: fn state => { x: state.x, y: state.y },
}

let stationary_actor: Actor = {
  witness: std::reflect::mirror (),
  state: { point: { x: 3, y: 4 }, name: "beacon" },
  update: fn dt state => state,
  position: fn state => state.point,
}
```

The moving actor stores its coordinates beside its speed.
The beacon stores a nested point and a name.
Each package supplies operations for its own layout, so neither needs a common concrete state struct or an unchecked downcast.
The `witness` retains type evidence for repackaging updated state after the package is opened.
The ordinary `Displayable` consumer needed only to produce a string; updating an `Actor` needs to build another package.

```ruddy
let tick_actor: Real -> Actor -> Actor = fn dt actor => match actor with
| hide 'state { witness, state, update, position } => {
    witness: witness,
    state: update dt state,
    update: update,
    position: position,
  }
end

let actor_position: Actor -> { x: Real, y: Real } = fn actor => match actor with
| hide 'state { witness, state, update, position } => position state
end

let ticked_actors = std::array::map (tick_actor 0.5) [moving_actor, stationary_actor]
```

After half a second, the moving actor's position is `{ x: 1, y: 0 }` and the beacon remains at `{ x: 3, y: 4 }`.
The update's output has the same hidden state type as its input, so it can be paired with the same operations again.
The array has one public element type, `Actor`, even though the packages contain different state types.
The original packages remain available because ticking produces new values.

This representation is useful for independently defined actor behaviors or editor extensions with private state.
A sum such as `#Enemy Enemy | #Beacon Beacon` is simpler when a game has a fixed set of variants and consumers need to inspect them directly.
A homogeneous array of one component type can also suit a tight simulation loop better than packaging a function set with every actor.
The hidden type establishes compatibility; it does not choose the most efficient layout for a workload.

## Recovering a known type

[Any](../std/any.md) packages a value together with its mirror.
A consumer can attempt to recover a particular type through a checked downcast:

```ruddy
let hidden_count = std::any::upcast 3n
let recovered_count: Option Nat = std::any::downcast hidden_count
let recovered_text: Option String = std::any::downcast hidden_count
```

The first result is `#Some 3n`; the second is `#None`.
A type comparison is not a numeric or text conversion.
The mirror preserves the type contract, including effects for function types.

At a lower level, `reflect::same` returns evidence connecting two types when they are exactly the same.
Typed views from `reflect::shape` can expose operations for reading or constructing parts of a value while retaining those parts' hidden types.
This supports generic operations without unchecked casts.

## Representations and schemas

[Codecs](../std/codec.md) use type information to describe encoding and decoding.
[Binary](../std/binary.md) runs codecs over a positional representation, so [schema](../dictionary.md#schema) agreement matters even when no field name is written on the wire.
Versioned operations make a save-game or network-message schema identity explicit.
Type-directed decoding checks the selected schema; migrating an old save still needs an explicit compatibility policy.
A registry associates dynamic packages with application-selected wire identities; an identity from untrusted input cannot manufacture type evidence.

## Summary and exercises

Hidden types preserve relationships while concealing a producer's choice.
Mirrors provide evidence about types, and descriptions provide data about that evidence.

1. Construct a third `Displayable` package for a boss-defeated boolean.
2. Explain why `show value` is valid while returning the hidden `value` alone is not.
3. Predict both downcasts above and distinguish a failed downcast from conversion to text.
4. Add a vertically moving actor with its own state layout, then update a mixed actor array.
5. Explain why `tick_actor` must keep the state, its operations, and its witness together.

[Selected answers](answers.md#hidden-types) describe the scope boundary.

