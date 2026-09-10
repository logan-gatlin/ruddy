# Mutable cells

`mut value` allocates a fresh cell, `~cell` reads its contents, and
`cell := value` replaces its contents and returns the value just stored. Copying
a cell value creates an alias; allocating twice creates two different cells.
Reading or writing does not deep-copy values stored inside a cell.

```text
let example = fn _ => do
  let a = mut 0n
  let b = a
  let result = a := b := 3n
  return { read: ~a, written: result }
end
```

Both fields are `3n`. Assignment associates to the right and binds below all
other infix operators, including pipeline. Each assignment evaluates its target
first and its value second, exactly once, before storing. In a chain, targets
are evaluated from left to right and writes complete from right to left.
`mut` and `~` have unary precedence; application and projection bind more
tightly. Write `f (~cell)` to pass a read to a function. Discard a write with
`_ = cell := value` or `let _ = cell := value`. A block may end with
`return expr` to produce a value; without it, the block produces `()`.

## Types, effects, and regions

The cell type is `mut 'r 'a` and the effect for allocation, reading, and writing
is `!mut 'r`. Written types and effects require the region argument. A region
variable has its own kind, inferred from its position; it cannot also stand for
an ordinary type, a row, or a presence.

```text
type Cell 'r 'a = mut 'r 'a
effect State 'r = !mut 'r
let read: Cell 'r Nat -> Nat + !State 'r = fn cell => ~cell
let copy = fn target => fn source => target := ~source
```

The element type is invariant. One cell cannot hold incompatible types at
different uses. Cells with different element types can share a region. A
computation has one mutation-effect entry in its ordinary effect row; all
accesses agree on that entry's region. Combining two distinct rigid regions
is a type error. Mutation has compiler-controlled semantics and cannot be
implemented or discharged by a user effect handler.

## Local state and escaping state

Region inference needs no region block. At each function body, inference can
remove the mutation effect if the region is fresh and independent of the
inputs, captured environment, result (including latent effects), and remaining
effects. Other effects remain visible. Reading or writing supplied or captured
state stays effectful, and a promised pure annotation must satisfy that rule.
A block or let initializer is not itself an isolation scope. Isolation is
conservative: an unannotated recursive definition can retain a mutation effect
even when its state appears local.

Factories that return cells or stateful closures retain their allocation
effect and use the caller's inferred region. Each invocation still allocates
fresh cells. There is no existential region package or promotion of isolated
storage. An enclosing function can isolate the state produced by factories
when none of it escapes.

```text
let make = fn value => mut value
let use = fn _ => do
  let first = make 1n
  let second = make "two"
  _ = first := 3n
  return { number: ~first, text: ~second }
end
```

`make` has type `'a -> mut 'r 'a + !mut 'r`. `use` can be pure.
Global initializers still must be pure: allocating escaping global state is
rejected. An already isolated pure helper can run inside another stateful
function without exposing its private region to that caller.

Let polymorphism depends on the initializer's immediate effects. Inference
generalizes eligible variables only when those effects are proven empty.
Otherwise unknown types remain shared, including through aliases,
destructuring, and captured closures. A function value may generalize even
when invoking it will mutate; capturing an existing cell does not make that
cell polymorphic. Isolating an enclosing function does not retroactively
generalize its internal effectful bindings. Existing presence packages retain
their ownership and guarantees.

Arrays remain persistent immutable values. A cell can hold an array; replacing
that array does not change a previously read array value. Cells nested inside
stored values retain their normal aliasing.

## JavaScript and foreign code

The JavaScript backend represents a cell as one ordinary mutable object with
one own writable data property, `value`. Allocation creates a fresh `{ value: initial }`,
reading loads `.value`, and writing assigns `.value`. The property contains the
existing Ruddy runtime representation of the element; the cell boundary does
not recursively marshal or clone its contents. Direct foreign arguments and
returned aliases preserve the object identity. Foreign code must use that
representation rather than substitute an independently boxed copy.

For example, this declaration permits a same-thread foreign implementation to
update a cell containing a `Real` (a JavaScript number):

```text
extern update: mut 'r Real -> () + !mut 'r = "host.update"
```

```javascript
globalThis.host = {
  update(cell) {
    cell.value += 1;
    return {};
  }
};
```

Foreign declarations and callers are trusted to preserve declared effects,
element types, identity, region lifetime, and thread confinement. A foreign
identity function may return the same cell without accessing its contents;
a foreign function that reads, writes, or allocates declares `!mut 'r`.
Foreign code must not disguise externally shared state as fresh local storage
or omit observable host effects. Existing restrictions on foreign element
representations, callbacks, and completion still apply; see [CPS execution](cps.md).

Retained callbacks and Promise completion are permitted under that trusted
contract. Suspension preserves cell identity and contents. There are no new
lifetime guards, revocation mechanisms, or host-thread checks. A region belongs
to one thread: both reads and writes on another thread violate its contract.
Future threading support must enforce confinement through values and captures.
A sequence of writes is not a transaction.

The compiler carries cell allocation, reading, and writing as ordinary
sequential CPS value instructions. Mutation needs no runtime handler evidence.
Artifacts preserve these operations, region parameter kinds, and the builtin
effect identity; malformed region positions are rejected before import.
