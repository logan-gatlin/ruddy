---
doc: true
bookNavigation:
  previous:
    path: "/book/report.html"
    title: "12. Worked program: a combat replay"
  next:
    path: "/book/reflection.html"
    title: "14. Hidden types, mirrors, and generic operations"
---

# 13. Rows, presence, and richer interfaces

A movement function should preserve a player's health and a projectile's damage even though it only changes coordinates.
A pause transition should preserve the world while changing which operations are permitted on the session.
Both need relationships between shapes, rather than only types for individual fields.

[Rows](../dictionary.md#row) describe collections of fields, cases, or effects.
[Presence variables](../dictionary.md#presence-variable) describe whether particular entries belong to those collections.
Together they let a function preserve information it does not inspect and state which combinations it can handle.

### Compact inferred signatures

The suffix `?` is shorthand for a fresh anonymous presence: `{ x?: Nat }`
means `{ x when _: Nat }`, `#Some? Nat` means `#Some (when _) Nat`, and
`!Log?` means `!Log (when _)`. Each occurrence is independent. It does not
add an `undefined` value to a field's type: if `x` is present, it is a `Nat`.
Inferred signatures use this shorthand only for presences occurring once and
not mentioned by a constraint. A repeated presence keeps its name, even without
a `where` clause, because its occurrences must stay linked.

Tuple types support the same markers after an element: `(Nat?, String when 'p)`
is the positional form of `{ 0?: Nat, 1 when 'p: String }`. A singleton keeps
its comma: `(Nat?,)`. Markers describe independent slots; a prefix relationship
must still be stated explicitly in `where`.

Group a sum or function type before marking the whole element, for example
`((#Red | #Blue)?, (Nat -> Nat) when 'p)`. This distinguishes an optional tuple
slot `((#Red)?,)` from a required slot containing an optional variant `(#Red?,)`.

The marker `when <= 'p` introduces a fresh anonymous presence constrained to
imply `'p`. For example, `{ x when <= 'p: Nat }` means
`{ x when 'x: Nat } where 'x -> 'p`, with a fresh name for `'x`.
Tuple slots, sum cases, and effects accept the same marker. Every occurrence
is independent: two entries bounded by `'p` can still have different presences.
The bound must name a presence elsewhere in the same annotation; that named
occurrence can come later, as in the channel accessor below.
Inferred signatures use it when a presence occurs once and its only remaining
constraint is an implication to a named presence. Repeated presences and more
complex relationships keep their names and `where` constraints.

This shorthand is available in annotations. Type definitions still require
explicit parameters for their presences; `when <= 'p` does not introduce a
new kind of type definition or implicit alias parameters.

Long signatures can also introduce ordinary `type` definitions before the
signature, or reuse an equivalent definition already in scope. For example:

```ruddy
type Fields 'p 'value 'rest = {
  first_long_field when 'p: 'value,
  second_long_field when 'p: 'value,
  ..'rest
}
let identity: Fields 'present 'a { ..'r } -> Fields 'present 'a { ..'r } = fn x => x
```

These are writable, fully parameterized definitions, not hidden display aliases.
The presentation retains the original presence constraints and uses an
abbreviation only when the complete output, including its definition, is shorter.

## From subtyping to row polymorphism

Accepting a richer object through a smaller interface is familiar from class or interface subtyping.
A targeting function might accept anything implementing a position interface.
Structural subtyping, as in [TypeScript's compatibility rules](https://www.typescriptlang.org/docs/handbook/type-compatibility.html), can establish compatibility from members without a declared inheritance relationship.

Accepting an object and preserving its type in a result are different questions.
A movement signature described as `Position -> Position` only promises coordinates in its result.
Even if its implementation retains health, that signature does not expose health to the caller.
A language with bounded generics can express a stronger relationship; basic subtyping alone is not that relationship.

Ruddy gives the additional fields a name and carries them through the operation:

```ruddy
let translate:
  Real -> Real -> { x: Real, y: Real, ..'rest }
  -> { x: Real, y: Real, ..'rest } =
  fn dx dy object => { x: object.x + dx, y: object.y + dy, ..object }

let moved_player = translate 1 2 { x: 0, y: 0, health: 100n }
let moved_projectile = translate 1 2 { x: 3, y: 4, damage: 12n }
```

For the player call, `'rest` contains `health: Nat`; for the projectile call, it contains `damage: Nat`.
The results therefore retain their different fields without either object inheriting from a common class.
This is **row polymorphism**: the function works for different rows while preserving a named relationship between them.
The type preserves field names and types; the spread in the implementation preserves their actual values.
The type alone does not promise that health remains exactly `100n`.

| Interface idea | What it tells the caller |
| --- | --- |
| A function returning a base position interface | Coordinates are available through the result type. |
| `{ x: Real, y: Real, .. } -> Real` | Additional input fields are accepted; only a number is returned. |
| The same `..'rest` in input and output | The same additional field names and types occur at both ends. |
| `{ x: Real, y: Real } -> Real` | The input shape is closed: an extra health field is not accepted. |

Ruddy does not silently treat a larger struct as a smaller closed struct.
An open row accepts additional fields, while constructing a smaller value explicitly discards fields when that is intended.
Two unrelated anonymous `..` tails do not name a connection between their remainders.
The [distance example](types.md#following-a-requirement) deliberately gives its two inputs independent remainders because they can be different objects.

## Rows describe entries, not a runtime container

A row must exclude every label already named beside any of its uses.
In `translate`, `'rest` cannot contain another `x` or `y` field.
This makes the explicit coordinates and the remainder fit together without duplicate entries.

Sum cases can also have open tails.
For example, `#Ready String | ..'rest` admits a `Ready` case and additional cases described by `'rest`.
A function consuming that type must also account for the additional cases, typically with a fallback pattern.

Structs and sums share a row kind.
One row can describe both an equipment loadout and the legal weapon selection cases:

```ruddy
type LoadoutAndSelection 'weapons = {
  weapons: { ..'weapons },
  selection: | ..'weapons,
}

type ArcherLoadout = LoadoutAndSelection { bow: Nat, dagger: () }

let archer: ArcherLoadout = {
  weapons: { bow: 12n, dagger: () },
  selection: #bow 12n,
}
```

`weapons` has both `bow: Nat` and `dagger: ()`.
`selection` has type `#bow Nat | #dagger ()`, so a sword selection is not admitted by this loadout type.
The bow payload might represent remaining ammunition; the dagger needs no payload.
The shared row preserves labels, payload types, and presence relationships, including spelling and capitalization.
It does not prove that the selected ammunition count equals the count in `weapons`; that is a value relationship the implementation must maintain.

Sharing a row does not convert a struct value into a sum value.
The surrounding struct or sum determines how values use the entries.
Effect rows have a separate kind because their entries describe operations rather than fields or cases.

## Presence is inferred from program shape

A running session can advance its world, while a paused session should expose that world for inspection without advancing it.
A state representation can make the distinction visible as a field:

```ruddy
let active_world = fn session => match session with
| { running } => running
| { paused } => paused
end
```

These struct patterns are exact: `{ running }` accepts that field alone, while `{ running, .. }` would permit others.
The first arm accepts running without paused; the second accepts paused without running.
Neither an empty struct nor a struct with both fields matches, so inference must exclude both shapes.
Both arms return a world in the same result position, which also connects their payload types.

A readable form of the inferred signature is:

```text
{ running when 'running: 'world, paused when 'paused: 'world }
  -> 'world where 'running != 'paused
```

A presence variable is a type-level boolean: it says whether a particular field, case, or effect belongs to a type.
`'running` describes the existence of the field, not a runtime boolean stored inside it.
`'running != 'paused` means exactly one presence is true.
The editor may choose different variable names, but the relationship is the same.

Presence variables and their constraints are **inferable**.
No `when` or `where` clause was needed in the implementation above.
Its patterns and uses supplied the constraints that an annotation could state explicitly.
A single sum type remains a good choice when only runtime state selection matters; presence relationships become useful when an interface must retain more precise knowledge of allowed states.

## Deriving different constraints

The patterns admitted by a function determine the combinations it can handle.
For a velocity component, supplying only one axis is likely a mistake.
An optional velocity sub-struct can contain both axes or neither:

```ruddy
let velocity_pair = fn velocity => match velocity with
| { vx, vy } => ()
| {} => ()
end
```

The first arm requires both presences and the second requires neither, giving `'vx = 'vy`.
A position-bearing game object could keep this component in a separate `velocity` field; the patterns here describe that component, not the whole object.
The function checks the combination without reading the payloads, so their types remain independent.
Adding arithmetic would require appropriate numeric payloads too.

An AI destination selector can accept a target or a patrol point, with the target taking priority when both exist:

```ruddy
let chase_or_patrol = fn navigation => match navigation with
| { target, .. } => target
| { patrol, .. } => patrol
end
```

Open patterns allow additional fields and accept both destinations.
The requirement becomes `'target or 'patrol`, and the input has an open row tail.
Both destinations need compatible types because either can be the function's result.
Arms are considered in order, so a target wins over a patrol point.

The three functions impose different relationships on their respective pairs of fields:

| Presence combination | Session: running/paused | Velocity: vx/vy | Navigation: target/patrol |
| --- | --- | --- | --- |
| Neither | Rejected | Accepted | Rejected |
| First only | Accepted | Rejected | Accepted |
| Second only | Accepted | Rejected | Accepted |
| Both | Rejected | Accepted | Accepted; target wins |
| Inferred constraint | `!=` | `=` | `or` |

These relationships come from coverage: every admitted input needs an arm that can handle it.
Inside an arm, matching supplies facts that justify field accesses.
A fallback after a target-presence test knows that the earlier test did not match.
That fact belongs to the branch, not to every use of the navigation value elsewhere.

An annotation can state the session contract explicitly:

```ruddy
let inspect_session: {
  running when 'running: 'world,
  paused when 'paused: 'world,
} -> 'world where 'running != 'paused = fn session => active_world session
```

Changing `!=` to `or` would promise to accept both fields, which `active_world` cannot handle.
The compiler rejects that mismatch instead of silently strengthening the annotation to rescue the implementation.

A `where` clause can appear on an annotation or a type definition.
A type definition must declare every free variable in its header; its constraints
apply at every use and are inherited by definitions that use it.
It can use `not`, `and`, `or`, `->`, `<=`, `=`, and `!=`; parentheses group relationships and semicolons separate constraints.
For example, `'target -> 'path` (equivalently `not 'target or 'path`) says that a target requires a path.
Equality would require a target and a path to appear together, rejecting a stale path without its target as well.

An implication chain abbreviates adjacent requirements:

```ruddy
'd <= 'c <= 'b <= 'a
```

This means `'d -> 'c; 'c -> 'b; 'b -> 'a`, as for the presences of an optional tuple suffix.
Each link must hold. It is different from `'d -> 'c -> 'b -> 'a`, which remains
right-associative nested implication: `'d -> ('c -> ('b -> 'a))`.
The inferred-type printer uses chains for linear sequences of presence requirements.

Chains bind at the same level as `=` and `!=`, below `or` and `and` but above `->`.
Use parentheses to mix a chain with equality or to use a chain as another chain's operand.
In value expressions, `<=` remains the ordinary less-than-or-equal comparison.

This differs from `{ target: Option Nat, path: Option [Nat] }`.
That struct always has both fields and independently allows `#Some` or `#None` in each payload.
A presence constraint can rule out mismatched combinations in the shape itself.
It still cannot prove that a path reaches the chosen target or that an entity ID refers to a living entity.

## Carrying a relationship into the result

A pause transition should carry the world into its next state and preserve the caller's knowledge of the transition:

```ruddy
let toggle_pause = fn session => match session with
| { running } => { paused: running }
| { paused } => { running: paused }
end

let paused = toggle_pause { running: { level: 3n } }
let resumed = toggle_pause paused

let paused_world: { paused: 'world } -> 'world = fn session => session.paused
let frozen = paused_world paused
```

The compiler connects output `paused` to input `running`, and output `running` to input `paused`.
With descriptive variable names, the inferred type is:

```text
{ running when 'running: 'a, paused when 'paused: 'b }
  -> { paused when 'running: 'a, running when 'paused: 'b }
  where 'running != 'paused
```

`frozen.level` is `3n`.
Passing `resumed` to `paused_world` is rejected because that result is known to have a running field, not a paused field.
Unlike `active_world`, `toggle_pause` does not combine both payloads in one field, so its inferred type can preserve different payload types in the two alternatives.

This is useful for APIs where saving, inspecting, or advancing a session requires a specific state.
The transition returns a new value; it does not consume the old one.
Ruddy's immutable values allow the original running session to remain available, so this pattern is not a guarantee that only one session can exist or that a state value is used exactly once.

A related function can turn those state fields into sum cases:

```ruddy
let session_choice = fn session => match session with
| { running } => #Running running
| { paused } => #Paused paused
end
```

The program implies that the input has exactly one state field and that its corresponding output case must be admitted.
Its inferred sum may remain open to additional cases, so constructing a `#Paused` value does not automatically promise that the result type excludes `#Running`.
A shared-presence annotation states that stronger relationship:

```ruddy
let precise_session_choice: {
  running when 'running: 'world,
  paused when 'paused: 'world,
} -> (#Running (when 'running) 'world | #Paused (when 'paused) 'world)
  where 'running != 'paused = fn session => match session with
| { running } => #Running running
| { paused } => #Paused paused
end

let known_paused = precise_session_choice paused
let paused_level = match known_paused with
| #Paused world => world.level
end
```

The final match needs only the paused case because that call's result type excludes the running case.
A sum-case presence says whether the **type admits a case**, not which tag one particular runtime value carries.
A value of type `#Running World | #Paused World` carries one tag while both cases belong to its type.
Constraining the two case presences with `!=` is stronger than the usual rule that a sum value carries one tag.

A match whose arms name tags can infer requirements on another argument for
each admitted case:

```ruddy
let get_channel = fn channel image => match channel with
| #Red => image.r
| #Green => image.g
end

let red = get_channel #Red { r: 1 }
let green = get_channel #Green { g: 2 }
```

The inferred relationship says that admitting `#Red` requires `r`, and admitting
`#Green` requires `g`. A channel typed as `#Red | #Green` therefore requires both
fields. The branch results still share one type; this inference relates case
and field presences, rather than selecting different scalar types at runtime.

Using inline bounds, the relationship fits in the type itself:

```ruddy
let get_channel:
  (#Red (when <= 'r) | #Green (when <= 'g))
  -> { r when 'r: 'value, g when 'g: 'value, ..'rest }
  -> 'value = fn channel image => match channel with
| #Red => image.r
| #Green => image.g
end
```

There are no named selector presences or separate `where` clauses here.
The bound only requires the corresponding field when the selector admits that
case. Having a red field does not itself require the selector to admit red.
Two selectors written with the same bounds can independently admit different
cases; preserving the exact selector type would require named shared presences.

## Parameters and effect rows

An effect can be parameterized by the type returned by its operation:

```ruddy
effect Ask 'a = () -> 'a

let requested_level: () -> Nat + !Ask Nat = fn _ => !Ask ()

let supplied_level = handle requested_level () with
| !Ask _ => 3n
end
```

`Ask Nat` connects the operation's result to the function's natural-number result.
Repeated uses of one effect must have compatible arguments; repetition does not create separate layers of that effect.

An open effect row connects a wrapper's requirements to those of its callback:

```ruddy
let apply_once: ('a -> 'b + ..'effects) -> 'a -> 'b + ..'effects =
  fn operation value => operation value
```

The wrapper cannot know every operation a future callback will use.
Naming its effect remainder allows that requirement to pass through to the caller.
A pure callback leaves no operations to handle; a clock-reading callback leaves a clock requirement.
The [Array](../std/array.md) signatures use this relationship for mapping and folding.

## Sharing presences across structs, sums, and effects

A cutscene interface may need its audio configuration, callback effect contract, and possible results to agree.
One presence can connect all three kinds of entry even though effect rows have a different kind from data rows:

```ruddy
effect Audio = { play: String -> () }

let run_cutscene:
  { audio when 'audio: () }
  -> (() -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio))
  -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio)
  = fn options scene => scene ()
```

When `'audio` is absent, the options are `{}`, the callback must be pure, and only `#Done String` is admitted.
When it is present, the options have an `audio` marker, the callback may perform `Audio`, and `#Sounded String` is also admitted.
The wrapper passes the same result and effect relationship to its caller.

The marker constrains which callback contract fits; this wrapper does not inspect it or mute audio at runtime.
The callback supplies the behavior, and the surrounding handler supplies the playback policy.
An admitted effect does not guarantee that every execution performs it, just as an admitted sum case need not be the returned tag.

```ruddy
let voiced_scene = fn _ => do
  _ = !Audio.play "gate-opening"
  return #Sounded "gate open"
end

let played_scene = handle run_cutscene { audio: () } voiced_scene with
| !Audio.play sound => ()
end
```

The result is `#Sounded "gate open"` and this handler discards the audio request, which is useful when exercising a cutscene without an audio device.
Calling `run_cutscene` with `{}` and `voiced_scene` violates the shared contract: its callback needs the audio case and effect that those options exclude.
Independent presence variables could instead be related with a `where` formula when exact agreement is too restrictive.

## Summary and exercises

Rows preserve an object's extra fields, while presences express conditional membership and relationships among fields, cases, and effects.
Inference derives constraints from patterns and uses; annotations state contracts that implementations must satisfy.
These features can preserve state knowledge and reject incompatible combinations without proving every gameplay invariant.

1. Explain how `translate` retains player health and projectile damage without requiring their complete types to agree.
2. Derive `!=`, `=`, and `or` from the three sets of patterns. Which accepts unrelated fields?
3. Change `chase_or_patrol` to prefer patrol when both fields exist. Does its coverage requirement change?
4. Explain why `paused_world resumed` is rejected, while keeping the old running session value is allowed.
5. Compare `session_choice` and `precise_session_choice`. Why can the latter support a one-arm match for `known_paused`?
6. Add a bow-only loadout and predict which selection cases it admits.
7. Explain each occurrence of `'audio` in `run_cutscene`. Does allowing `Audio` guarantee that sound is played?
8. Give one invalid shape these types reject and one invalid gameplay value they still admit.

[Selected answers](answers.md#advanced-types) work through the state and component relationships.
