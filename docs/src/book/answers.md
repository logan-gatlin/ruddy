---
doc: true
---

# E. Selected exercise answers

These answers explain the rule being exercised rather than only giving a final value.
Implementation exercises can have several valid solutions with the same specified behavior.
Examples below are independent unless a chapter definition is explicitly mentioned.

## Getting started

Changing `player.health` leaves the result unchanged because `display_name` reads only `name`.
Changing its body to `display entity.health` changes the required field and prints `100n`.
`display` preserves the natural-number suffix; `std::str::from_nat` produces plain decimal text when that is the desired format.
Removing that field makes the supplied struct unable to satisfy the access in the function body.

## Expressions

`dash_distance true 30` selects the zero bonus and returns `30`.
The branches must still agree in type because the function is checked for its possible calls, not evaluated at one chosen argument during checking.
Renaming the inner `speed` in `combined_speeds` preserves the binding each use refers to and therefore preserves `18`.

## Functions

`add_bonus 3` is a function of type `Real -> Real`.
`add_bonus 3 20` is a `Real` value equal to `23`.
`attack_damage (5 + 5)` passes `10` and produces `8`, whereas `attack_damage 5 + 5` produces `5.5 + 5`, or `10.5`.
The grouping determines which calculation supplies the function argument.
In `spawn_total_loop`, the parameters carry the loop state into the next iteration.
The original `spawn_total` instead retains an addition for each unfinished recursive call.
Both are stack-safe in Ruddy; their pending-work memory requirements differ.

## Data

The first two elements can be named by one array pattern, while the fallback handles every shorter array:

```ruddy
let second: ['a] -> Option 'a = fn values => match values with
| [_, value, ..] => #Some value
| _ => #None
end
```

The payload type of `#Some` is the array's element type.
Neither an empty array nor a one-element array contains a second element.
`array::get values 1n` provides the same selection through the library's checked indexing operation.
For targeting, `#Some 0n` identifies entity zero, while `#None` supplies no entity ID.

## Types

`distance_to` requires `Real` coordinates on each argument and gives each its own additional fields.
A player with health and a beacon with a light radius therefore satisfy the same coordinate requirements without having identical complete types.
A string-valued `x` violates the arithmetic requirement.

`WorldPosition` and `ScreenPosition` are aliases for the same structure, so their names alone do not reject a mixed-space calculation.
Distinct `#World` and `#Screen` tags make the spaces different types and put the conversion at an explicit boundary.
A translation-only camera at the origin should preserve the coordinate numbers.
A scale-aware conversion should specify whether scaling happens before or after translation and check the intended formula.

A generic recording helper preserves its payload type:

```ruddy
let at_tick: Nat -> 'a -> { tick: Nat, value: 'a } =
  fn tick value => { tick: tick, value: value }
```

The repeated `'a` relates the stored value to the input; no runtime conversion is involved.

## Higher-order programming

Counting selected values needs a natural-number state, not a retained array of matching elements:

```ruddy
let count_living = fn healths => std::array::fold
  (fn count health =>
    if std::real::greater_than health 0 then std::nat::add count 1n else count end)
  0n healths
```

The state changes only when a health satisfies the condition.
For subtraction, left and right folds compute different parenthesizations, producing `-13` and `7` in the chapter's example.
Addition hides that distinction for the small exact values used there; floating-point rounding can introduce further differences on other inputs.

## Modules

`src/spawn.rud` contains the definitions previously inside `Spawn`.
The root file declares `module spawn`, and callers use `spawn::message`.
A definition marked `@private` remains accessible within the same bundle, so this attribute does not establish privacy between sibling modules.

## Effects

In `next_time`, the operation returns `100n` to the handled expression; the normal-result arm then returns `101n`.
In `availability`, `raise "unavailable"` ends the handled expression before either conditional branch produces its result.
Returning `true` normally instead resumes the condition and selects `"ready"`.
The forwarding output handler still needs an outer output handler because it performs that operation in its own arm.
Reversing the prefix nesting produces `ai: match: target acquired`: the innermost handler adds its prefix first.
`at_time` and `without_logs` handle all their callbacks' operations without introducing observable effects.
`to_console` replaces `Log` with `IO`, so it remains effectful.
Top-level effects would make declaration order observable; prohibiting escaping initialization effects allows mutually referring definitions to remain unordered.
With `Cancel = () -> ()`, returning unit from the operation arm resumes the handled block, which then returns `"turn resolved"`.
With the original `() -> |`, no value can satisfy the operation's result type, and `raise "turn cancelled"` exits the block instead.
`Immediate` has no operation for a handler to answer: the foreign function observes the clock directly.
A fixed `Clock` handler can supply a time without that observation, while a real handler that calls the host retains `Immediate`.

## State

The expression-only damage calculation returns the same numeric result as the cell-based calculation for the same input.
The implementations differ in the operations they perform and the state they create.
Two counters created separately each have their own cell, while two bindings of one returned counter function share its captured cell.
Interleaving calls does not merge separately created cells.
Undo histories can share unchanged array branches between immutable versions, while retaining those versions also retains their reachable data.
A cell holding an array merely identifies the current immutable version; updating an element still uses the array tree's update operation.

## Input and failure

A path-selection function can return `Result String String`:

```ruddy
let choose_path: [String] -> Result String String = fn arguments => match arguments with
| [] => #Some "level.json"
| [path] => #Some path
| _ => #Error "Expected at most one input path"
end
```

A missing environment variable may select a documented default.
A failed lookup may instead require an error report because the program has not established that the variable is absent.
The nested result and option preserve that distinction.

## External data

A syntactically valid JSON object can have the wrong fields or field values for `LevelSettings`.
An HTTP 404 can arrive through a successful transport result, and its JSON body can still be valid.
The caller must decide whether that status is acceptable before interpreting the body as a successful application response.

## The replay

Armor belongs in the pure damage rule, with explicit decisions about whether a hit can deal zero damage and whether armor applies before other modifiers.
A replay scrubber needs the initial roster and intermediate roster versions, not only the final one.
Immutable trees can share unchanged branches between snapshots, while retained versions keep their reachable data alive.

Changing file acquisition to HTTP leaves `combat::replay` intact, but adds transport and status checks before decoding.
If enemies can disappear from the array, a position is no longer a stable identity for later hits.
A revised model needs stable entity IDs and an explicit policy for stale or unknown targets.
Types can distinguish an ID from an index with separate tags; they do not prove that every ID currently exists.

## Advanced types

The shared row in `translate` carries player health or projectile damage through movement.
A return type that names only a base position interface does not promise those extra fields.
Bounded generics in a subtyping language can also preserve relationships; the comparison concerns the actual interface being written.

Exact running-only and paused-only patterns imply `!=`.
Exact paired-velocity and empty patterns imply `=`.
Open target and patrol patterns imply `or` and permit unrelated fields.
Reordering the open arms changes priority when both destinations exist without changing which combinations are accepted.

`toggle_pause` maps a known running field to a known paused field and preserves its world payload.
`paused_world` cannot accept the resumed result because that shape has no paused field.
The original running value remains usable: an immutable transition does not provide exclusive ownership or consume its input.

`session_choice` constructs a tag but can leave the result's sum open.
`precise_session_choice` shares field and case presences, allowing a known paused input to produce a type admitting only `#Paused`.
A sum type admitting both states still describes a value with one runtime tag, so those are different forms of knowledge.

A bow-only shared loadout row admits a bow selection and excludes dagger or sword cases.
It preserves payload types, not equality of ammunition counts stored in separate fields.
Likewise, paired path and target fields do not establish that the path actually reaches the target.

The repeated `'audio` connects the cutscene options marker, admitted `#Sounded` result, and allowed `Audio` effect.
An allowed effect need not occur on every execution.
The example handler discards the request rather than producing sound, so an effect contract is not a promise about physical playback.

## Hidden types

A boolean package can pair a boolean with `fn value => if value then "yes" else "no" end`.
Opening a package proves that its stored function accepts its stored value, even while the particular type remains hidden.
Returning the string forgets that private type safely; returning the raw hidden value would expose a type not named in the consumer's public result.
The `Nat` downcast succeeds, while the `String` downcast returns `#None` because conversion to text is a different operation.
`tick_actor` uses an update function whose input and output agree with its hidden state.
Retaining that state's mirror and functions lets the result be repackaged without exposing the hidden type.

## Interoperability

The uppercase operation depends on its input, while the clock observes external state.
A property of a live object may run host behavior during access, which explains the `Host` effect on observation.
A callback that prints requires an output contract in addition to its input and output value types.
Completion timing does not erase that requirement.

---

[Book contents](../index.md) · [Standard-library reference](../std/bundle.md)
