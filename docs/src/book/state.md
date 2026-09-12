---
doc: true
bookNavigation:
  previous:
    path: "/book/effects.html"
    title: "8. Effects and handlers"
  next:
    path: "/book/io.html"
    title: "10. Input, output, and failure"
---

# 9. Mutable state and regions

[Immutable](../dictionary.md#immutable) values are the default because a calculation can keep using its inputs without another caller changing them underneath it.
Some game services need shared state: an entity-ID allocator remembers earlier allocations, and an asset cache retains resources for later requests.
Mutability is an escape hatch for these needs, not a requirement for expressing ordinary iteration or data transformation.
A [mutable cell](../dictionary.md#mutable-cell) makes the shared state explicit.

## Creating, reading, and writing

This function puts a damage value in a cell, applies a bonus, and reads the result:

```ruddy
let damage_with_bonus = fn damage => do
  let current = mut damage
  _ = current := ~current + 5
  return ~current
end
```

`mut damage` creates the cell.
`~current` reads its current value, and `current := value` stores a new value.
Assignment returns the value written; `_ =` discards it here.
For an input of `20`, the function returns `25`.

The binding `current` still names the same cell after assignment.
The content changes, not the association between the name and the cell.
An additional binding of that cell would observe the same updates.

## The state appears in the type

A cell type has the form `mut 'r T`, where `T` is the stored type and `'r` identifies a [region](../dictionary.md#region).
Operations on cells are tracked by the effect `!mut 'r`.
The following explicit interface connects the cell's region to the effect of reading and writing it:

```ruddy
let increment: mut 'r Nat -> Nat + !mut 'r = fn counter =>
  counter := std::nat::add (~counter) 1n
```

The region is a type-level identity, not an address that application code calculates.
A signature that receives a cell and updates it records that interaction.
A function whose fresh local state cannot escape may have that local mutation effect removed from its interface.
For example, the cell in `damage_with_bonus` is created for one call, and only the resulting number leaves the function.
A caller cannot use another reference to observe that cell changing, so the calculation can remain pure from the caller's perspective.
If a cell or a stateful closure escapes, its mutation effect must remain visible.
This boundary allows an implementation to use local mutation while keeping a stable, pure contract for callers.
[Cell](../std/cell.md) provides operations with the same relationships stated explicitly.

## State retained by a function

A closure can retain a cell between calls.
This factory produces an ID source whose successive calls return increasing natural numbers:

```ruddy
let make_id_source = fn start => do
  let counter = mut start
  return fn _ => counter := std::nat::add (~counter) 1n
end
```

Two calls to the same returned function observe one cell.
Two calls to `make_id_source` create separate cells.
Each source counts independently; the type does not guarantee globally unique IDs across sources.
Calling the returned function is therefore different from repeatedly evaluating a pure function at the same argument: the stored state affects its result.

## Choosing state deliberately

The earlier `damage_with_bonus` example can be written as `fn damage => damage + 5`.
That version exposes the calculation more directly and needs no mutable cell.
An explicit [fold](higher-order.md#selecting-and-combining) is often suitable when state simply advances through a sequence.
A retained cell is appropriate when an interface intentionally represents state shared across calls.

The choice should follow the operation's contract.
Mutation that callers can observe is part of its behavior even when the cell itself is private.
Examples and tests should therefore specify sequences of calls when order matters.

## Sharing and the cost of change

An immutable value can safely serve several consumers because none can overwrite it.
Ruddy's [array trees](data.md#arrays-are-immutable-trees) use that fact to share unchanged branches between versions.
Keeping an earlier world for a replay checkpoint, rollback, or an editor undo step can therefore be cheaper than making a full independent copy of a mutable buffer.
This advantage comes from sharing, not from the word “immutable” alone.

Updates still allocate new paths through the tree, and each path involves accesses to separate pieces of memory.
Retaining many versions can also keep data alive longer than necessary.
A simulation that only needs one current world may benefit less from sharing than a workload that branches into many versions.
There is no universal performance winner.

A mutable cell needs storage for its changing content, and reading it follows that shared reference.
Repeatedly changing a cell can avoid constructing a new enclosing value for every state transition, but creates an order dependency between reads and writes.
A cached result cannot be reused merely because it was computed from the same cell reference: the cell's content may have changed.
These are reasons to keep mutable state local and its operations visible, rather than assume that an imperative spelling will make every calculation faster.

Placing an array in a cell does not turn it into a mutable flat array.
Updating that cell with `array::set` still constructs a persistent array result and pays the tree operation's cost.
The cell only chooses which array version later reads see.
An immutable array can also contain cells; sharing that array shares those cell references, whose contents remain mutable.
Such an array is not an isolated historical world: changing a shared cell changes what a later read through either version observes.

The useful comparison is between complete algorithms: their allocations, traversals, retained versions, and shared-state requirements.
Measurements for the actual workload should decide a performance-motivated use of mutation.

## Summary and exercises

Cells separate stable bindings from changing contents.
Regions connect cell types to mutation effects, and closures can retain cells across calls.

1. Compare the value behavior of `damage_with_bonus` with its expression-only version.
2. Create two counters with the same initial value and predict their outputs under interleaved calls.
3. Explain why copying a cell into another binding does not create an independent counter.
4. Compare the retained data needed for a world history with that needed for one current result.
5. Explain why storing an array in a cell does not change the complexity of `array::set`.

[Selected answers](answers.md#state) distinguish shared and separate cells.

