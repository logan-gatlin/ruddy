# Game-development examples for Ruddy's type system

Investigated 2026-09-12 against the working tree based on `79e6ffa`, using the existing `target/debug/ruddy` and the working-tree standard library. These are validated teaching candidates, not claims about engine integration, a complete ECS, or resource ownership. No generated std documentation was edited. Source snippets below formed one executable disposable project in `/tmp/ruddy-game-types-research`; `ruddy check`, `ruddy run`, and `ruddy doc` provided acceptance, behavior, and exact printed signatures.

## Recommended progression

1. **Geometry without a common base object.** `distance_to` reads only `x` and `y`. Its two independently inferred row tails let a player carrying health and a beacon carrying color participate in one operation. It neither needs nor establishes a common nominal class. Real arithmetic and square root come from [std/real.rud](../../std/real.rud); the precise inferred signature below confirms that input remainders are independent. A separate `translate` names the same remainder in input and output, preserving the caller's extra field types. Its spread preserves their values. This differs from narrowing an object to a base interface and losing the return shape; [inference/mod.rs](../../src/inference/mod.rs) explicitly describes unification rather than an ordering between types.
2. **State-sensitive interfaces.** `toggle_pause` uses exactly one of `running` and `paused`. Exact patterns require XOR, and inference tracks the state transition by swapping presences in the result. A paused-only operation accepts the resulting paused state and rejects a running one. Begin with an ordinary named sum when explaining runtime state; use presence relationships when an interface needs to preserve a caller's known state across an operation. A typed transition does not consume or invalidate the old immutable value, and does not enforce one-time use. The compiler's [presence inference overview](../../src/inference/mod.rs) explains deriving formulas from coverage and [guarded equality](../../src/inference/solve.rs) explains relationships conditional on a match arm.
3. **Component consistency.** Exact `{ vx, vy }` and `{}` arms imply that both velocity coordinates exist or neither does. `target_and_path` models an API that caches a route only together with its intended target; their types need not agree. That relation does not prove the stored route reaches the stored target. These simple functions validate the shape and return unit; a later implementation can perform calculations inside the matching arm. Explain this deliberate application contract, since some games reasonably represent a target while its path is still being found. Represent that situation with an additional explicit state rather than claiming target/path equality is a universal rule.
4. **Prioritized AI inputs.** Open `{ target, .. }` and `{ patrol, .. }` arms infer OR. One destination must exist, both may exist, and the earlier target arm wins. Because either payload is returned, the destination payload types must agree. Unlike exact patterns this helper also accepts unrelated fields. This gives a concrete reason why changing pattern openness changes the inferred interface.
5. **State as a result case.** Converting a running/paused field into a `#Running`/`#Paused` case naturally infers implications: a present input field requires its constructed result case to be admitted. An explicit shared-presence signature states the stronger input/output relationship when needed. The validated `known_paused` has only `#Paused`, so a single-case match is exhaustive. Sum-case presence describes which cases the static type admits, whereas one runtime value always has only its current tag. Do not add XOR to a usual two-case runtime state type merely because a value has one tag.
6. **Configured side effects.** An audio marker, a possible `#Sounded` result case, and the `Audio` requirement can share one presence variable. This couples the contracts of configuration, callback, result, and effects. The marker does not implement the mode switch or prove that sound actually occurred; it constrains compatible callbacks. The supplied enabled-mode example executes under a local handler. [Shared rows](../shared-rows/spec.md) establishes that struct/sum rows share a kind and effect rows have a separate kind; presence booleans can still connect their membership decisions.
7. **Loadout entries and selection cases.** Sharing a row between `{ ..'weapons }` and `| ..'weapons` makes available weapon labels and selection labels agree. `ArcherLoadout` permits `#bow` and `#dagger`, rejects `#wand`, and preserves their distinct payload types. This is a structural agreement, not a lookup: the selection's bow count need not equal the loadout's stored count. Labels keep their exact spelling and capitalization. The owning [shared-row specification](../shared-rows/spec.md) states these rules.
8. **Heterogeneous immutable actor updates.** `Actor` hides a state type while preserving that `update` accepts and returns it and `position` accepts it. A homogeneous `[Actor]` can then contain different state representations. Repacking the updated hidden value also retains an explicit `Mirror 'state`; matching that field binds the authentic runtime type evidence required by this example. This evidence behavior is specified in [introspection](../introspection/spec.md), especially the explicit pattern-bound evidence rule. The consumer cannot extract an untyped state or mix one actor's state with another actor's update. The package is an example of typed behavior and state, not a complete ECS, allocation strategy, or engine object model. If mirrors have not yet been taught, first show a consume-only package and introduce updating/repacking after the mirror section.

## Complete runnable candidate

The sample prints `5`, `100`, `3`, `3`, and `1`: the distance between the two objects, the health retained after translating the player, the level obtained from a paused state, the level obtained from a precisely typed paused case, and the moving actor's x-coordinate after half a second. Those five outputs were compared exactly with expected output. The array example additionally checks that different actor representations can be updated through one typed array operation; [std/array.rud](../../std/array.rud) owns the generic map signature.

```ruddy
using std::real

let distance_to = fn left right => do
  let dx = left.x - right.x
  let dy = left.y - right.y
  return real::sqrt (dx * dx + dy * dy)
end
let player = { x: 0.0, y: 0.0, health: 100n }
let beacon = { x: 3.0, y: 4.0, color: "gold" }
let distance = distance_to player beacon
let translate: Real -> Real -> { x: Real, y: Real, ..'rest } -> { x: Real, y: Real, ..'rest } =
  fn dx dy object => { x: object.x + dx, y: object.y + dy, ..object }
let moved_player = translate 1.0 2.0 player

let toggle_pause = fn session => match session with
| { running } => { paused: running }
| { paused } => { running: paused }
end
let paused = toggle_pause { running: { level: 3n } }
let resumed = toggle_pause paused
let freeze: { paused: 'world } -> 'world = fn session => session.paused
let frozen = freeze paused

let session_choice = fn session => match session with
| { running } => #Running running
| { paused } => #Paused paused
end
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

let velocity_pair = fn velocity => match velocity with
| { vx, vy } => ()
| {} => ()
end
let target_and_path = fn navigation => match navigation with
| { target, path } => ()
| {} => ()
end

let chase_or_patrol = fn navigation => match navigation with
| { target, .. } => target
| { patrol, .. } => patrol
end

let toggle_sum: (#Running (when 'running) 'world | #Paused (when 'paused) 'world)
 -> (#Paused (when 'running) 'world | #Running (when 'paused) 'world)
 = fn session => match session with
 | #Running world => #Paused world
 | #Paused world => #Running world
 end
let paused_sum = toggle_sum (#Running { level: 3n })

type Actor = hide 'state => {
  witness: Mirror 'state,
  state: 'state,
  update: Real -> 'state -> 'state,
  position: 'state -> { x: Real, y: Real },
}
let moving_actor: Actor = {
  witness: std::reflect::mirror (),
  state: { x: 0.0, y: 0.0, speed: 2.0 },
  update: fn dt state => { x: state.x + dt * state.speed, ..state },
  position: fn state => { x: state.x, y: state.y },
}
let stationary_actor: Actor = {
  witness: std::reflect::mirror (),
  state: { point: { x: 3.0, y: 4.0 }, name: "beacon" },
  update: fn dt state => state,
  position: fn state => state.point,
}
let tick_actor: Real -> Actor -> Actor = fn dt actor => match actor with
| hide 'state { witness, state, update, position } => { witness: witness, state: update dt state, update: update, position: position }
end
let actor_position: Actor -> { x: Real, y: Real } = fn actor => match actor with
| hide 'state { witness, state, update, position } => position state
end
let ticked_actors = std::array::map (tick_actor 0.5) [moving_actor, stationary_actor]

effect Audio = { play: String -> () }
let run_cutscene:
  { audio when 'audio: () }
  -> (() -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio))
  -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio)
  = fn options scene => scene ()
let voiced_scene = fn _ => do
  _ = !Audio.play "gate-opening"
  return #Sounded "gate open"
end
let played_scene = handle run_cutscene { audio: () } voiced_scene with
| !Audio.play sound => ()
end

type LoadoutAndSelection 'weapons = {
  weapons: { ..'weapons },
  selection: | ..'weapons,
}
type ArcherLoadout = LoadoutAndSelection { bow: Nat, dagger: () }
let archer: ArcherLoadout = {
 weapons: { bow: 12n, dagger: () },
 selection: #bow 12n,
}

let main = fn _ => do
 _ = println (std::str::from_real distance)
 _ = println (std::str::from_nat moved_player.health)
 _ = println (std::str::from_nat frozen.level)
 _ = println (std::str::from_nat paused_level)
 _ = println (std::str::from_real (actor_position (tick_actor 0.5 moving_actor)).x)
 return ()
end
```

## Exact printed signatures

Variable names are the compiler's generated names; their relationships matter more than their spelling. These were copied from `ruddy doc` output, with no simplification.

```ruddy
let distance_to: { x: Real, y: Real, ..'a } -> { x: Real, y: Real, ..'b } -> Real

let translate: Real -> Real -> { x: Real, y: Real, ..'rest } -> { x: Real, y: Real, ..'rest }

let toggle_pause: { running when 'a: 'c, paused when 'b: 'd } -> { paused when 'a: 'c, running when 'b: 'd } where 'a != 'b

let velocity_pair: { vx when 'a: 'c, vy when 'b: 'd } -> () where 'a = 'b

let target_and_path: { target when 'a: 'c, path when 'b: 'd } -> () where 'a = 'b

let chase_or_patrol: { target when 'a: 'c, patrol when 'b: 'c, ..'d } -> 'c where 'a or 'b

let session_choice: { running when 'a: 'e, paused when 'b: 'f } -> #Running (when 'c) 'e | #Paused (when 'd) 'f | ..'g where not 'a and 'b and 'd or 'a and not 'b and 'c

let precise_session_choice: {
  running when 'running: 'world,
  paused when 'paused: 'world,
} -> (#Running (when 'running) 'world | #Paused (when 'paused) 'world)
where 'running != 'paused

let known_paused: #Paused { level: Nat }

let run_cutscene:
  { audio when 'audio: () }
  -> (() -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio))
  -> (#Done String | #Sounded (when 'audio) String) + !Audio (when 'audio)

let tick_actor: Real -> Actor -> Actor

let ticked_actors: [Actor]
```

## Negative checks

Each expression was appended independently to the otherwise valid program. All nine `ruddy check` commands returned a nonzero exit status with the intended type diagnostic.

| Check | Diagnostic |
| --- | --- |
| `missing_coordinate` | `missing-field` |
| `both_states` | `presence-required` |
| `neither_state` | `presence-required` |
| `wrong_state_consumer` | `extra-field` |
| `half_velocity` | `presence-required` |
| `path_without_target` | `presence-required` |
| `no_ai_plan` | `presence-required` |
| `unavailable_weapon` | `extra-field` |
| `leaking_hidden_state` | `hidden-escapes` |

The tested expressions were:

```text
let bad = distance_to { x: 0.0 } beacon
let bad = toggle_pause { running: 1n, paused: 1n }
let bad = toggle_pause {}
let bad = freeze resumed
let bad = velocity_pair { vx: 1.0 }
let bad = target_and_path { path: [1n] }
let bad = chase_or_patrol {}
let bad: ArcherLoadout = { weapons: { bow: 12n, dagger: () }, selection: #wand "spark" }
let bad: Actor -> 'a = fn actor => match actor with
| hide 'state { witness, state, update, position } => state
end
```

Diagnostics are defined in [src/ui.rs](../../src/ui.rs); hidden-state escape checking is implemented in [src/inference/solve.rs](../../src/inference/solve.rs). The disposable project retains `negative.py` and `negative-results.json` for reproducibility during this editing session. No Rust test suite was required or invoked.
