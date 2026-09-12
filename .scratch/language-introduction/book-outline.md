# The Ruddy Book — current outline

Status: implemented first draft, revised around game-development examples on 2026-09-12.
The reading path begins at [The Ruddy Book](../../docs/src/index.md).
Generated files in `docs/src/std/` remain the API reference.

## Audience and teaching approach

The book serves intermediate programmers familiar with non-functional languages and undergraduate-level computer science.
It assumes experience with functions, control flow, data structures, modules, and development tools, but no prior functional-programming knowledge.
Game logic supplies concrete motivations without requiring an engine or game framework.

Cornell's [OCaml Programming: Correct + Efficient + Beautiful](https://cs3110.github.io/textbook/cover.html) supplies the pedagogical model: motivate a problem, establish a small example, derive its evaluation and typing rules, then exercise those rules in another setting.
The original [research note](research.md) records that analysis.
The [game-development research](game-dev-research.md) records compiler-validated applications of Ruddy's richer types.

Examples distinguish what types establish from gameplay rules that still require implementation and verification.
A coordinate requirement does not establish a coordinate space; distinct tags can separate spaces.
A valid entity ID type does not establish that the entity exists.
A presence-polymorphic transition preserves state knowledge without consuming the old immutable value.

## Main reading path

### 1. Getting started with Ruddy

Create an arena project and print a player name from a richer struct.
Introduce live LSP diagnostics, hover information, and navigation as the normal development workflow, with running and building as explicit execution steps.
Use the first example to preview how a function can require only the fields it reads.

### 2. Values, expressions, and bindings

Calculate movement from speed and elapsed time, add a dash bonus with a conditional, and derive slowed movement in a nested scope.
Explain why expressions produce values, why branches need compatible types, and why shadowing preserves the outer binding.
Distinguish the result of a `do` block from returning out of a function.

### 3. Functions and application

Calculate attack damage, partially apply a weapon bonus, and count enemies across successive waves.
Derive currying through nested functions and distinguish argument grouping from arithmetic grouping.
Relate tail-recursive parameters to loop state; explain stack safety, pending-work memory, and the foreign-call exception.

### 4. Modeling data and matching its shape

Update a player's position while retaining an earlier snapshot.
Represent an optional target and enemy patrol, chase, and stunned states with the data each requires.
Introduce arrays as immutable trees, with explicit operation costs and memory implications rather than assumptions from mutable contiguous arrays.

### 5. Understanding inferred and structural types

Derive `distance_to` from its use of `x` and `y`, then apply it to players, enemies, and beacons with different additional fields.
Compare structural aliases with distinct world/screen tags when coordinate-space confusion must be rejected.
Introduce generic identity and a tick-stamped payload, then follow type errors from producers to consumers.

### 6. Higher-order programming

Factor repeated health transformations into a generic map, heal surviving entities with filtering, and fold health values into a total.
Explain composition and traversal costs using the actual immutable array representation.
Exercises distinguish retaining selected values from directly accumulating a count.

### 7. Organizing programs

Group spawn-message helpers into a module, move them into a source file, and introduce imports and bundle visibility.
Connect module boundaries to independently exercisable game rules.
Keep library signatures in generated references.

### 8. Effects and handlers

Explain purity through repeatable movement and combat calculations.
Supply a fixed clock to attack-timing rules, wrap effectful computations in pure functions, and compose combat/AI log handlers through forwarding.
Cover early exit, operations returning the empty sum that cannot resume, and immediate host effects that cannot be handled.
Explain the restriction on top-level effects through unordered mutually referring definitions and otherwise ambiguous effect ordering.

### 9. Mutable state and regions

Use a local damage accumulator and an ID-source closure to distinguish private mutation from state retained across calls.
Explain region effects, observable sharing, allocation and access costs, and reduced opportunities to reuse results.
Compare immutable replay checkpoints with shared mutable cells; mutation is an escape hatch, and neither representation removes all costs.

### 10. Input, output, and failure

Choose a level file from process arguments and handle filesystem and environment results.
Separate the operation's effects from its success/failure value.
Keep loading and output around game logic that can be exercised independently.

### 11. Structured data and network requests

Decode level settings and distinguish shape validation from a gameplay limit on enemy counts.
Compose URL and HTTP operations while treating transport, status, and payload validity as separate decisions.
Introduce only the reflection vocabulary needed for typed decoding; advanced runtime type evidence comes later.

### 12. Worked program: a combat replay

Build a complete two-module executable that decodes an initial enemy roster and ordered hits, applies pure damage rules, and prints final health.
Saturate damage at zero, reject invalid targets, and retain stable indices by keeping defeated enemies in the roster.
Verify repeated execution, unchanged initial state, empty inputs, numeric boundaries, malformed data, file errors, and argument errors.
Exercises extend armor, replay scrubbing, HTTP input, and stable identities for spawning or removing enemies.

## Advanced interfaces and integration

### 13. Rows, presence, and richer interfaces

Compare row polymorphism with familiar interface subtyping and bounded generics.
Preserve player health and projectile damage through one movement helper.
Share a row between equipment fields and legal weapon-selection cases.
Derive XOR, equality, and OR from session-state, velocity-component, and prioritized AI patterns.
Carry state knowledge through pause/resume transitions and relate struct-field presences to admitted sum cases.
Connect an audio configuration marker, callback effects, and result cases with a shared presence.

### 14. Hidden types, mirrors, and generic operations

Package heterogeneous HUD values with their display operations.
Introduce mirrors before packaging actors with different private states, related update functions, and a common position observation.
Preserve type evidence when updating and repackaging an actor.
Distinguish safe hidden-type consumption, checked downcasting, and representation/schema concerns.

### 15. JavaScript interoperability

Use an editor-tool enemy label, checked host-value conversion, callbacks, and asynchronous values to explain host contracts.
Describe where annotations place trust and which runtime guarantees stop at foreign code.

## Appendices and supporting references

- Installation, editor setup, CLI commands, and a layout for trying chapter fragments.
- Syntax reference with matching game examples.
- Dictionary as the vocabulary source of truth.
- Numbers, strings, and runtime behavior, including counters, frame intervals, and dialogue text.
- Selected exercise answers that explain why the result or rejection follows.
- Generated standard-library APIs, prelude configuration, and detailed platform contracts.

The front page also maps common game problems to the relevant chapters.
Chapter navigation follows the main reading path, and complete file examples carry filename bars.
