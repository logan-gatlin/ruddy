---
doc: true
bookNavigation:
  previous:
    path: "/book/external-data.html"
    title: "11. Structured data and network requests"
  next:
    path: "/book/rows.html"
    title: "13. Rows, presence, and richer interfaces"
---

# 12. Worked program: a combat replay

A combat replay makes a gameplay failure reproducible: keep an initial enemy roster and the ordered hits, then run the same rules again.
This program reads those values from JSON, applies each hit to a new roster, and prints the final health values.
Its simulation is pure, so a terminal tool, a regression check, or an engine adapter can all use the same rules.

The input contains `enemies` and `hits` arrays.
Each enemy has a `name` and natural-number `health`; each hit has a zero-based `target` index and natural-number `damage`.
A hit reduces health to a minimum of zero, and further hits on a defeated enemy leave it at zero.
The roster keeps defeated enemies in place, so indices remain stable throughout this replay.
An out-of-range target rejects the replay instead of silently ignoring the hit.

This small model assumes that every hit is already confirmed: it does not decide collisions, armor, or attack timing.
Those rules can be added as explicit transformations or recorded inputs.
Using the same pure rules and complete input sequence makes this model repeatable; it does not by itself promise cross-platform determinism for every possible game.

## Create the project

The [installation prerequisites](../download.md) apply.

```sh
ruddy new combat-replay
cd combat-replay
```

The project has two source files: `src/combat.rud` for the simulation and `src/main.rud` for file access and output.
The generated manifest already selects a Node executable.

## The simulation module

The following complete contents belong in `src/combat.rud`:

```ruddy filename="src/combat.rud"
using std::{array, nat}

type Enemy = { name: String, health: Nat }
type Hit = { target: Nat, damage: Nat }
type Replay = { enemies: [Enemy], hits: [Hit] }

@doc "Applies damage without letting health fall below zero."
let health_after_hit: Nat -> Nat -> Nat = fn health damage =>
  if nat::less_than health damage then 0n
  else nat::subtract health damage
  end

@doc "Updates one enemy, or rejects a target outside the roster."
let apply_hit: [Enemy] -> Hit -> Result [Enemy] String = fn enemies hit =>
  match array::get enemies hit.target with
  | #None => #Error "Hit target is outside the enemy roster"
  | #Some enemy => do
    let updated = { health: health_after_hit enemy.health hit.damage, ..enemy }
    return match array::set enemies hit.target updated with
    | #None => #Error "Hit target is outside the enemy roster"
    | #Some next => #Some next
    end
  end
  end

@doc "Replays hits in order, stopping at the first invalid target."
let replay: [Enemy] -> [Hit] -> Result [Enemy] String = fn enemies hits =>
  match hits with
  | [] => #Some enemies
  | [hit, ..remaining] => match apply_hit enemies hit with
    | #Error error => #Error error
    | #Some next => replay next remaining
    end
  end
```

`health_after_hit` expresses the gameplay rule explicitly instead of relying on subtraction's numeric boundary behavior.
Its comparison ensures that the subtraction is never asked to go below zero.
`Nat` rejects negative input health and damage, but the type alone does not make damage saturating.

`array::get` returns an option because a natural-number index may still be outside the roster.
The successful arm has an enemy to update; `array::set` produces a new array version and its own option, which is also handled explicitly.
The original roster remains available for comparison or a checkpoint.
The [immutable tree representation](data.md#arrays-are-immutable-trees) shares unchanged parts rather than copying every enemy on each hit.

`replay` returns the initial roster for an empty hit sequence.
After a valid hit it calls itself with the new roster and remaining hits; that call is in tail position.
An invalid target returns an error immediately, so subsequent hits are not applied.
The [array traversal costs](higher-order.md#the-cost-of-a-traversal) still matter for long replays even though recursive calls are stack-safe.

The type distinguishes decoded data from a successful simulation result.
It does not establish that a target index exists or that a replay is small enough for the available memory.
Applications accepting untrusted replay files should choose size and work limits for their own workloads.

## The executable boundary

The following complete contents replace `src/main.rud`:

```ruddy filename="src/main.rud"
module combat
using std::{array, fs, json, process, str}

let fail = fn message => do
  _ = eprintln message
  _ = process::exit 1n
  return ()
end

let read_replay: String -> Result combat::Replay json::Error = json::decode

let print_enemy = fn enemy =>
  println (str::concat enemy.name (str::concat ": " (str::from_nat enemy.health)))

let run_replay = fn path => match fs::read_text path with
| #Error error => fail (str::concat "Cannot read replay: " error.message)
| #Some text => match read_replay text with
  | #Error _ => fail "Expected enemies with name/health and hits with target/damage"
  | #Some recording => match combat::replay recording.enemies recording.hits with
    | #Error message => fail message
    | #Some enemies => array::fold (fn _ enemy => print_enemy enemy) () enemies
    end
  end
end

let main = fn _ => match process::args () with
| [] => run_replay "replay.json"
| [path] => run_replay path
| _ => fail "Expected at most one replay path"
end
```

The decoder checks the file's shape before the simulation sees it.
The simulation then checks target validity, which is a value-level rule not established by JSON decoding.
Only a successful replay prints the final roster; failures produce a diagnostic and a nonzero exit status.
`fail` returns unit in its type, matching the output branches, while its exit operation prevents the final `return` from being reached.

Printing uses a fold with unit as the state because only the ordered output effects are needed.
Those effects belong at the boundary: `combat::replay` can be checked without a filesystem or console handler.

## Run the example

The following data belongs in `replay.json` at the project root:

```json filename="replay.json"
{
  "enemies": [
    { "name": "Slime", "health": 10 },
    { "name": "Golem", "health": 20 }
  ],
  "hits": [
    { "target": 0, "damage": 4 },
    { "target": 1, "damage": 7 },
    { "target": 0, "damage": 20 }
  ]
}
```

```sh
ruddy run
```

The slime goes from ten health to six, then to zero; the golem goes from twenty to thirteen:

```text
Slime: 0
Golem: 13
```

The CLI currently does not forward additional arguments through `ruddy run`.
A built Node executable can receive an explicit path directly:

```sh
ruddy build
node build/combat-replay.js another-replay.json
```

## Verify the contract

`combat::replay` can be exercised with literal rosters and hit arrays.
With no hits, it returns the unchanged roster.
Zero damage leaves health unchanged, exact remaining-health damage reaches zero, and excess damage stays at zero.
Repeating a replay from the same initial roster must give the same result while leaving that original roster unchanged.
An out-of-range target must return `#Error`.

Boundary checks include an absent file, malformed JSON, a missing field, an extra field, negative damage, and too many arguments.
A well-shaped file with an invalid target must fail during simulation rather than decoding.
These checks distinguish representation requirements from gameplay rules instead of treating a type-correct document as a valid replay automatically.

## Summary and exercises

A replay is an initial state plus a sequence of recorded inputs interpreted by pure rules.
Immutable updates keep earlier states available, while result types distinguish failure from a usable next state.
External input and output remain separate from the simulation.

1. Add armor to each enemy and specify how it changes zero, small, and large hits.
2. Return a sequence of roster snapshots for a replay scrubber. Explain the benefit and memory cost of retaining them.
3. Replace file input with a downloaded replay while preserving `combat::replay`.
4. Explain what must change if enemies can be removed or spawned during a replay instead of keeping stable indices.

[Selected answers](answers.md#the-replay) discuss rules, history, and identity.
